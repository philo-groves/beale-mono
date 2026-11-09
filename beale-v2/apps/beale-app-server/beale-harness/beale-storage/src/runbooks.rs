use super::HostDatabase;
use anyhow::{Context, Result, ensure};
use beale_contracts::runbooks::{
    Cell, CellResult, Document, MAX_TITLE_CHARS, QueryRequest, RunExecution, Summary,
};
use rusqlite::{OptionalExtension, Transaction, params};
use std::{
    collections::HashSet,
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunbookRunRow {
    pub id: String,
    pub runbook_id: String,
    pub revision: u64,
    pub status: String,
    pub output: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRunbookRow {
    pub id: String,
    pub title: String,
    pub status: String,
    pub updated_at_ms: u64,
    pub completed_at_ms: u64,
    pub current_cell_number: u32,
    pub current_cell_name: String,
}

fn now_ms() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .min(i64::MAX as u128) as u64)
}

fn bounded_cell_output(output: &str, limit: usize) -> String {
    const SUFFIX: &str = "\n[output truncated]";
    if output.len() <= limit {
        return output.to_owned();
    }
    if limit <= SUFFIX.len() {
        return String::new();
    }
    let mut end = limit - SUFFIX.len();
    while !output.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &output[..end], SUFFIX)
}

fn read(tx: &Transaction<'_>, workspace_id: &str, id: &str) -> Result<Option<Document>> {
    let json: Option<String> = tx
        .query_row(
            "SELECT document_json FROM runbook_documents WHERE workspace_id=?1 AND id=?2",
            params![workspace_id, id],
            |row| row.get(0),
        )
        .optional()?;
    json.map(|value| serde_json::from_str(&value).context("decode runbook"))
        .transpose()
}

fn validate(document: &mut Document, previous: Option<&Document>) -> Result<()> {
    document.title = document.title.trim().to_owned();
    document.purpose = document.purpose.trim().to_owned();
    ensure!(
        !document.title.is_empty()
            && document.title.len() <= 200
            && ((document.title.chars().count() <= MAX_TITLE_CHARS
                && !document.title.chars().any(char::is_control))
                || previous.is_some_and(|old| old.title == document.title)),
        "invalid runbook title"
    );
    ensure!(
        !document.purpose.is_empty() && document.purpose.len() <= 4_000,
        "invalid runbook purpose"
    );
    ensure!(document.cells.len() <= 64, "too many runbook cells");
    let previous_ids: HashSet<&str> = previous.map_or_else(HashSet::new, |book| {
        book.cells.iter().map(Cell::id).collect()
    });
    let mut used = HashSet::new();
    let mut next = previous.map_or(1, |book| book.next_cell_number);
    let mut total = 0_usize;
    for cell in &mut document.cells {
        let name = cell.name_mut();
        *name = name.trim().to_owned();
        ensure!(
            name.chars().count() <= 40 && !name.chars().any(char::is_control),
            "invalid runbook cell name"
        );
        if cell.id().is_empty() {
            *cell.id_mut() = format!("L{next}");
            next = next.checked_add(1).context("runbook cell ID overflow")?;
        } else {
            ensure!(previous_ids.contains(cell.id()), "unknown runbook cell ID");
        }
        ensure!(
            used.insert(cell.id().to_owned()),
            "duplicate runbook cell ID"
        );
        match cell {
            Cell::Markdown { source, .. } => {
                ensure!(source.len() <= 16_000, "runbook cell source is too long");
                total += source.len();
            }
            Cell::Code {
                language, source, ..
            } => {
                *language = language.trim().to_ascii_lowercase();
                ensure!(
                    !language.is_empty()
                        && language.len() <= 40
                        && language
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || b"._+-#".contains(&byte)),
                    "invalid code language identifier"
                );
                ensure!(source.len() <= 16_000, "runbook cell source is too long");
                total += source.len();
            }
        }
    }
    ensure!(total <= 48_000, "runbook source is too large");
    document.next_cell_number = next;
    ensure!(
        serde_json::to_vec(document)?.len() <= 56_000,
        "runbook document is too large"
    );
    Ok(())
}

fn same_code_content(previous: &Cell, current: Option<&Cell>) -> bool {
    matches!(
        (previous, current),
        (
            Cell::Code { language: old_language, source: old_source, .. },
            Some(Cell::Code { language: new_language, source: new_source, .. })
        ) if old_language == new_language && old_source == new_source
    )
}

fn persist(
    tx: &Transaction<'_>,
    document: &Document,
    actor: &str,
    session_id: &str,
    reason: &str,
) -> Result<()> {
    let json = serde_json::to_string(document)?;
    tx.execute(
        "INSERT INTO runbook_documents(workspace_id,id,title,purpose,updated_at_ms,revision,document_json)
         VALUES(?1,?2,?3,?4,?5,?6,?7)
         ON CONFLICT(workspace_id,id) DO UPDATE SET title=excluded.title,purpose=excluded.purpose,updated_at_ms=excluded.updated_at_ms,revision=excluded.revision,document_json=excluded.document_json",
        params![document.workspace_id,document.id,document.title,document.purpose,document.updated_at_ms as i64,document.revision as i64,json],
    )?;
    tx.execute(
        "INSERT INTO runbook_revisions(workspace_id,runbook_id,number,at_ms,actor,session_id,reason,document_json)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![document.workspace_id,document.id,document.revision as i64,document.updated_at_ms as i64,actor,session_id,reason,json],
    )?;
    Ok(())
}

impl HostDatabase {
    pub fn runbook_run_start(
        &self,
        workspace_id: &str,
        session_id: &str,
        runbook_id: &str,
        expected_revision: u64,
    ) -> Result<RunbookRunRow> {
        ensure!(session_id.len() <= 128, "invalid session ID");
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let owned: i64 = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM workspaces WHERE id=?1)",
                [workspace_id],
                |row| row.get(0),
            )?;
            ensure!(owned == 1, "workspace is not registered");
            let document = read(&tx, workspace_id, runbook_id)?.context("runbook not found")?;
            ensure!(document.revision == expected_revision, "runbook changed before execution");
            let id = Uuid::new_v4().to_string();
            let now = now_ms()?;
            tx.execute(
                "INSERT INTO runbook_runs(id,workspace_id,runbook_id,session_id,revision,status,created_at_ms,updated_at_ms) VALUES (?1,?2,?3,?4,?5,'running',?6,?6)",
                params![id, workspace_id, runbook_id, session_id, i64::try_from(document.revision)?, i64::try_from(now)?],
            )?;
            tx.commit()?;
            Ok(RunbookRunRow {
                id,
                runbook_id: runbook_id.to_owned(),
                revision: document.revision,
                status: "running".to_owned(),
                output: String::new(),
            })
        })
    }

    pub fn runbook_run_finish(&self, id: &str, success: bool, output: &str) -> Result<()> {
        ensure!(output.len() <= 64_000, "runbook output is too large");
        self.with_connection(|connection| {
            let changed = connection.execute(
                "UPDATE runbook_runs SET status=?2,output=?3,updated_at_ms=?4 WHERE id=?1 AND status='running'",
                params![id, if success { "completed" } else { "failed" }, output, now_ms()? as i64],
            )?;
            ensure!(changed == 1, "runbook execution is no longer active");
            Ok(())
        })
    }

    pub fn runbook_run_cell(&self, id: &str, cell_number: u32, cell_name: &str) -> Result<()> {
        ensure!(
            cell_number > 0
                && cell_number <= 64
                && cell_name.chars().count() <= 40
                && !cell_name.chars().any(char::is_control),
            "invalid runbook cell number"
        );
        self.with_connection(|connection| {
            let changed = connection.execute(
                "UPDATE runbook_runs SET current_cell_number=?2,current_cell_name=?3,updated_at_ms=?4 WHERE id=?1 AND status='running'",
                params![id, cell_number, cell_name, i64::try_from(now_ms()?)?],
            )?;
            ensure!(changed == 1, "runbook execution is no longer active");
            Ok(())
        })
    }

    pub fn runbook_run_cell_result(
        &self,
        run_id: &str,
        cell_id: &str,
        success: bool,
        exit_code: Option<i32>,
        stdout: &str,
        stderr: &str,
    ) -> Result<()> {
        let stderr = bounded_cell_output(stderr, 20_000);
        let stdout = bounded_cell_output(stdout, 20_000 - stderr.len());
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let run: Option<(String, String, String, i64)> = tx
                .query_row(
                    "SELECT workspace_id,runbook_id,session_id,revision FROM runbook_runs WHERE id=?1 AND status='running'",
                    [run_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()?;
            let (workspace_id, runbook_id, session_id, revision) =
                run.context("runbook execution is no longer active")?;
            let document = read(&tx, &workspace_id, &runbook_id)?.context("runbook not found")?;
            let snapshot_json: String = tx.query_row(
                "SELECT document_json FROM runbook_revisions WHERE workspace_id=?1 AND runbook_id=?2 AND number=?3",
                params![workspace_id, runbook_id, revision],
                |row| row.get(0),
            )?;
            let snapshot: Document = serde_json::from_str(&snapshot_json)?;
            let original = snapshot.cells.iter().find(|cell| cell.id() == cell_id);
            let current = document.cells.iter().find(|cell| cell.id() == cell_id);
            if !original.is_some_and(|cell| same_code_content(cell, current)) {
                return Ok(());
            }
            ensure!(
                matches!(current, Some(Cell::Code { .. })),
                "runbook code cell not found"
            );
            tx.execute(
                "INSERT INTO runbook_cell_results(workspace_id,runbook_id,session_id,cell_id,status,updated_at_ms,output,exit_code,stdout,stderr)
                 VALUES(?1,?2,?3,?4,?5,?6,'',?7,?8,?9)
                 ON CONFLICT(workspace_id,runbook_id,session_id,cell_id) DO UPDATE SET status=excluded.status,updated_at_ms=excluded.updated_at_ms,output='',exit_code=excluded.exit_code,stdout=excluded.stdout,stderr=excluded.stderr",
                params![workspace_id, runbook_id, session_id, cell_id, if success { "completed" } else { "failed" }, i64::try_from(now_ms()?)?, exit_code, stdout, stderr],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    pub fn runbook_run_interrupt(&self, id: &str) -> Result<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE runbook_runs SET status='interrupted',updated_at_ms=?2 WHERE id=?1 AND status='running'",
                params![id, i64::try_from(now_ms()?)?],
            )?;
            Ok(())
        })
    }

    pub fn runbook_run_status(&self, session_id: &str, id: &str) -> Result<Option<RunbookRunRow>> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT id,runbook_id,revision,status,output FROM runbook_runs WHERE id=?1 AND session_id=?2",
                params![id, session_id],
                |row| Ok(RunbookRunRow {
                    id: row.get(0)?,
                    runbook_id: row.get(1)?,
                    revision: row.get::<_, i64>(2)? as u64,
                    status: row.get(3)?,
                    output: row.get(4)?,
                }),
            ).optional().map_err(Into::into)
        })
    }

    pub fn session_runbook_counts(&self, session_id: &str) -> Result<(u64, u64)> {
        self.with_connection(|connection| {
            let touched: i64 = connection.query_row(
                "SELECT COUNT(DISTINCT runbook_id) FROM (
                    SELECT runbook_id FROM runbook_revisions WHERE session_id=?1
                    UNION SELECT runbook_id FROM runbook_runs WHERE session_id=?1)",
                [session_id],
                |row| row.get(0),
            )?;
            let active: i64 = connection.query_row(
                "SELECT COUNT(DISTINCT runbook_id) FROM runbook_runs WHERE session_id=?1 AND status='running'",
                [session_id],
                |row| row.get(0),
            )?;
            Ok((u64::try_from(touched)?, u64::try_from(active)?))
        })
    }

    pub fn session_runbooks(
        &self,
        workspace_id: &str,
        session_id: &str,
    ) -> Result<Vec<SessionRunbookRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT d.id,d.title,d.updated_at_ms,r.status,r.created_at_ms,r.updated_at_ms,r.current_cell_number,r.current_cell_name
                 FROM runbook_documents d
                 JOIN (SELECT runbook_id FROM runbook_revisions WHERE session_id=?1
                       UNION SELECT runbook_id FROM runbook_runs WHERE session_id=?1) touched
                   ON touched.runbook_id=d.id
                 LEFT JOIN runbook_runs r ON r.id=(
                   SELECT id FROM runbook_runs
                   WHERE session_id=?1 AND workspace_id=d.workspace_id AND runbook_id=d.id
                   ORDER BY created_at_ms DESC,rowid DESC LIMIT 1)
                 WHERE d.workspace_id=?2
                 ORDER BY d.id",
            )?;
            let rows = statement.query_map(params![session_id, workspace_id], |row| {
                let updated: i64 = row.get(2)?;
                let run_status: Option<String> = row.get(3)?;
                let run_started: Option<i64> = row.get(4)?;
                let run_updated: Option<i64> = row.get(5)?;
                let status = if run_status.as_deref() == Some("running") {
                    "running"
                } else if run_started.is_some_and(|start| start >= updated) {
                    "done"
                } else {
                    "prepared"
                };
                Ok(SessionRunbookRow {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    status: status.to_owned(),
                    updated_at_ms: if status == "running" { run_updated.unwrap_or(updated).max(0) as u64 } else { updated.max(0) as u64 },
                    completed_at_ms: if status == "done" { run_updated.unwrap_or(0).max(0) as u64 } else { 0 },
                    current_cell_number: row.get::<_, Option<i64>>(6)?.unwrap_or(0).max(0) as u32,
                    current_cell_name: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
        })
    }

    pub fn session_runbook_runs(
        &self,
        workspace_id: &str,
        session_id: &str,
        runbook_id: &str,
    ) -> Result<Vec<RunExecution>> {
        ensure!(session_id.len() <= 128, "invalid session ID");
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id,revision,status,output,created_at_ms,updated_at_ms,current_cell_number
                 FROM runbook_runs
                 WHERE workspace_id=?1 AND session_id=?2 AND runbook_id=?3
                 ORDER BY created_at_ms DESC,rowid DESC LIMIT 5",
            )?;
            let rows =
                statement.query_map(params![workspace_id, session_id, runbook_id], |row| {
                    Ok(RunExecution {
                        id: row.get(0)?,
                        revision: row.get::<_, i64>(1)?.max(0) as u64,
                        status: row.get(2)?,
                        output: row.get(3)?,
                        created_at_ms: row.get::<_, i64>(4)?.max(0) as u64,
                        updated_at_ms: row.get::<_, i64>(5)?.max(0) as u64,
                        current_cell_number: row.get::<_, i64>(6)?.max(0) as u32,
                    })
                })?;
            rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
        })
    }

    pub fn session_runbook_cell_results(
        &self,
        workspace_id: &str,
        session_id: &str,
        runbook_id: &str,
    ) -> Result<Vec<CellResult>> {
        ensure!(session_id.len() <= 128, "invalid session ID");
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT cell_id,status,updated_at_ms,output,exit_code,stdout,stderr FROM runbook_cell_results
                 WHERE workspace_id=?1 AND session_id=?2 AND runbook_id=?3 ORDER BY cell_id LIMIT 64",
            )?;
            let rows = statement.query_map(params![workspace_id, session_id, runbook_id], |row| {
                Ok(CellResult {
                    cell_id: row.get(0)?,
                    status: row.get(1)?,
                    updated_at_ms: row.get::<_, i64>(2)?.max(0) as u64,
                    output: row.get(3)?,
                    exit_code: row.get(4)?,
                    stdout: row.get(5)?,
                    stderr: row.get(6)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn runbook_get(&self, workspace_id: &str, id: &str) -> Result<Option<Document>> {
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let document = read(&tx, workspace_id, id)?;
            tx.commit()?;
            Ok(document)
        })
    }

    pub fn runbook_query(&self, request: &QueryRequest) -> Result<Vec<Summary>> {
        ensure!(
            !request.workspace_id.is_empty()
                && request.text.len() <= 200
                && request.language.len() <= 40
                && (1..=50).contains(&request.limit),
            "invalid runbook query"
        );
        self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT document_json FROM runbook_documents WHERE workspace_id=?1 ORDER BY updated_at_ms DESC,id DESC")?;
            let rows = statement.query_map([&request.workspace_id], |row| row.get::<_, String>(0))?;
            let needle = request.text.to_lowercase();
            let language = request.language.to_ascii_lowercase();
            let mut result = Vec::new();
            for row in rows {
                let document: Document = serde_json::from_str(&row?)?;
                let summary = Summary::from(&document);
                if (!needle.is_empty() && !format!("{} {}", summary.title, summary.purpose).to_lowercase().contains(&needle))
                    || (!language.is_empty() && !summary.languages.contains(&language)) { continue; }
                result.push(summary);
                if result.len() >= request.limit as usize { break; }
            }
            Ok(result)
        })
    }

    pub fn runbook_create(
        &self,
        mut document: Document,
        actor: &str,
        session_id: &str,
    ) -> Result<Document> {
        ensure!(
            document.id.is_empty()
                && !document.workspace_id.is_empty()
                && actor.len() <= 80
                && session_id.len() <= 128,
            "invalid runbook create"
        );
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let exists: i64 = tx.query_row("SELECT EXISTS(SELECT 1 FROM workspaces WHERE id=?1)", [&document.workspace_id], |row| row.get(0))?;
            ensure!(exists == 1, "workspace not registered");
            validate(&mut document, None)?;
            let duplicate: i64 = tx.query_row("SELECT EXISTS(SELECT 1 FROM runbook_documents WHERE workspace_id=?1 AND lower(trim(title))=lower(?2))", params![document.workspace_id,document.title], |row| row.get(0))?;
            ensure!(duplicate == 0, "runbook title already exists; query and edit the existing runbook");
            tx.execute("INSERT OR IGNORE INTO runbook_counters(workspace_id) VALUES(?1)", [&document.workspace_id])?;
            let next: i64 = tx.query_row("SELECT next_id FROM runbook_counters WHERE workspace_id=?1", [&document.workspace_id], |row| row.get(0))?;
            tx.execute("UPDATE runbook_counters SET next_id=next_id+1 WHERE workspace_id=?1", [&document.workspace_id])?;
            document.id = format!("R{next}");
            document.revision = 1;
            document.created_at_ms = now_ms()?;
            document.updated_at_ms = document.created_at_ms;
            persist(&tx, &document, actor, session_id, "created")?;
            tx.commit()?;
            Ok(document)
        })
    }

    pub fn runbook_edit(
        &self,
        mut document: Document,
        expected_revision: u64,
        actor: &str,
        session_id: &str,
        reason: &str,
    ) -> Result<Document> {
        ensure!(
            !reason.trim().is_empty()
                && reason.len() <= 1_000
                && actor.len() <= 80
                && session_id.len() <= 128,
            "invalid runbook revision reason"
        );
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let previous = read(&tx, &document.workspace_id, &document.id)?.context("runbook not found")?;
            ensure!(previous.revision == expected_revision, "runbook revision conflict");
            validate(&mut document, Some(&previous))?;
            let duplicate: i64 = tx.query_row("SELECT EXISTS(SELECT 1 FROM runbook_documents WHERE workspace_id=?1 AND id<>?2 AND lower(trim(title))=lower(?3))", params![document.workspace_id,document.id,document.title], |row| row.get(0))?;
            ensure!(duplicate == 0, "runbook title already exists");
            document.created_at_ms = previous.created_at_ms;
            document.updated_at_ms = now_ms()?.max(previous.updated_at_ms.saturating_add(1));
            document.revision = previous.revision.checked_add(1).context("runbook revision overflow")?;
            persist(&tx, &document, actor, session_id, reason)?;
            for cell in &previous.cells {
                let same_code = same_code_content(
                    cell,
                    document.cells.iter().find(|current| current.id() == cell.id()),
                );
                if !same_code {
                    tx.execute(
                        "DELETE FROM runbook_cell_results WHERE workspace_id=?1 AND runbook_id=?2 AND cell_id=?3",
                        params![document.workspace_id, document.id, cell.id()],
                    )?;
                }
            }
            tx.commit()?;
            Ok(document)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WorkspaceRow;

    #[test]
    fn new_titles_are_short_and_existing_long_titles_remain_editable() {
        let legacy = Document {
            id: "R1".to_owned(),
            workspace_id: "workspace-example".to_owned(),
            title: "a".repeat(MAX_TITLE_CHARS + 1),
            purpose: "Synthetic procedure".to_owned(),
            cells: Vec::new(),
            next_cell_number: 1,
            created_at_ms: 1,
            updated_at_ms: 1,
            revision: 1,
        };
        assert!(validate(&mut legacy.clone(), None).is_err());
        let mut edit = legacy.clone();
        edit.purpose = "Updated synthetic procedure".to_owned();
        assert!(validate(&mut edit, Some(&legacy)).is_ok());
        edit.title = "b".repeat(MAX_TITLE_CHARS + 1);
        assert!(validate(&mut edit, Some(&legacy)).is_err());
        edit.title = "b".repeat(MAX_TITLE_CHARS);
        assert!(validate(&mut edit, Some(&legacy)).is_ok());
        edit.title = "Invalid\nname".to_owned();
        assert!(validate(&mut edit, Some(&legacy)).is_err());
    }

    #[test]
    fn cell_output_is_bounded_at_utf8_boundaries() {
        let output = bounded_cell_output(&"λ".repeat(11_000), 20_000);
        assert!(output.len() <= 20_000);
        assert!(output.ends_with("[output truncated]"));
    }

    #[test]
    fn cell_results_are_session_scoped_and_changed_cells_are_prepared_again() {
        let root =
            std::env::temp_dir().join(format!("beale-runbook-cell-example-{}", Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        db.replace_workspaces(&[WorkspaceRow {
            id: "workspace-example".into(),
            name: "Example".into(),
            directory: "example-directory".into(),
        }])
        .unwrap();
        let mut document = db
            .runbook_create(
                Document {
                    id: String::new(),
                    workspace_id: "workspace-example".into(),
                    title: "Example procedure".into(),
                    purpose: "Synthetic check".into(),
                    cells: vec![
                        Cell::Code {
                            id: String::new(),
                            name: String::new(),
                            language: "javascript".into(),
                            source: "console.log('first')".into(),
                        },
                        Cell::Code {
                            id: String::new(),
                            name: String::new(),
                            language: "javascript".into(),
                            source: "process.exit(1)".into(),
                        },
                    ],
                    next_cell_number: 1,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                    revision: 0,
                },
                "agent",
                "session-example",
            )
            .unwrap();
        let run = db
            .runbook_run_start(
                "workspace-example",
                "session-example",
                &document.id,
                document.revision,
            )
            .unwrap();
        db.runbook_run_cell_result(&run.id, "L1", true, Some(0), "Example output", "")
            .unwrap();
        db.runbook_run_cell_result(&run.id, "L2", false, Some(1), "", "Example failure")
            .unwrap();
        db.runbook_run_finish(&run.id, false, "Example failure")
            .unwrap();
        let results = db
            .session_runbook_cell_results("workspace-example", "session-example", &document.id)
            .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].status, "completed");
        assert_eq!(results[0].stdout, "Example output");
        assert_eq!(results[0].exit_code, Some(0));
        assert!(results[0].output.is_empty());
        assert_eq!(results[1].status, "failed");
        assert_eq!(results[1].stderr, "Example failure");
        assert_eq!(results[1].exit_code, Some(1));
        assert!(results.iter().all(|result| result.updated_at_ms > 0));
        assert!(
            db.session_runbook_cell_results("workspace-example", "other-session", &document.id)
                .unwrap()
                .is_empty()
        );
        *document.cells[0].name_mut() = "First check".into();
        document = db
            .runbook_edit(document, 1, "agent", "session-example", "Name first check")
            .unwrap();
        assert_eq!(
            db.session_runbook_cell_results("workspace-example", "session-example", &document.id)
                .unwrap()
                .len(),
            2,
        );
        let stale = db
            .runbook_run_start(
                "workspace-example",
                "session-example",
                &document.id,
                document.revision,
            )
            .unwrap();
        if let Cell::Code { source, .. } = &mut document.cells[1] {
            *source = "console.log('revised')".into();
        }
        document = db
            .runbook_edit(
                document,
                2,
                "agent",
                "session-example",
                "Revise second check",
            )
            .unwrap();
        db.runbook_run_cell_result(&stale.id, "L2", true, Some(0), "Stale output", "")
            .unwrap();
        let results = db
            .session_runbook_cell_results("workspace-example", "session-example", &document.id)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].cell_id, "L1");
        drop(db);
        let reopened = HostDatabase::open(&root).unwrap();
        assert_eq!(
            reopened
                .session_runbook_cell_results("workspace-example", "session-example", &document.id)
                .unwrap(),
            results,
        );
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn schema_thirty_six_adds_cell_results_without_changing_existing_runs() {
        let root = std::env::temp_dir().join(format!(
            "beale-runbook-cell-migration-example-{}",
            Uuid::new_v4()
        ));
        let db = HostDatabase::open(&root).unwrap();
        drop(db);
        let connection = rusqlite::Connection::open(root.join("state.sqlite")).unwrap();
        connection
            .execute_batch("DROP TABLE runbook_cell_results; PRAGMA user_version = 36;")
            .unwrap();
        drop(connection);
        let reopened = HostDatabase::open(&root).unwrap();
        assert!(
            reopened
                .session_runbook_cell_results("workspace-example", "session-example", "R1")
                .unwrap()
                .is_empty()
        );
        drop(reopened);
        let connection = rusqlite::Connection::open(root.join("state.sqlite")).unwrap();
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 40);
        drop(connection);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn schema_thirty_seven_adds_current_cell_name() {
        let root = std::env::temp_dir().join(format!(
            "beale-runbook-name-migration-example-{}",
            Uuid::new_v4()
        ));
        let db = HostDatabase::open(&root).unwrap();
        drop(db);
        let connection = rusqlite::Connection::open(root.join("state.sqlite")).unwrap();
        connection
            .execute_batch(
                "ALTER TABLE runbook_runs DROP COLUMN current_cell_name; PRAGMA user_version = 37;",
            )
            .unwrap();
        drop(connection);
        let reopened = HostDatabase::open(&root).unwrap();
        drop(reopened);
        let connection = rusqlite::Connection::open(root.join("state.sqlite")).unwrap();
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 40);
        let name: String = connection
            .query_row(
                "SELECT current_cell_name FROM runbook_runs LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap()
            .unwrap_or_default();
        assert!(name.is_empty());
        drop(connection);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn schema_thirty_eight_preserves_cell_results_with_empty_output() {
        let root = std::env::temp_dir().join(format!(
            "beale-runbook-output-migration-example-{}",
            Uuid::new_v4()
        ));
        let db = HostDatabase::open(&root).unwrap();
        drop(db);
        let connection = rusqlite::Connection::open(root.join("state.sqlite")).unwrap();
        connection
            .execute_batch(
                "ALTER TABLE runbook_cell_results DROP COLUMN output;
                 INSERT INTO runbook_cell_results(workspace_id,runbook_id,session_id,cell_id,status,updated_at_ms)
                 VALUES('workspace-example','R1','session-example','L1','completed',123);
                 PRAGMA user_version = 38;",
            )
            .unwrap();
        drop(connection);
        let reopened = HostDatabase::open(&root).unwrap();
        let results = reopened
            .session_runbook_cell_results("workspace-example", "session-example", "R1")
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, "completed");
        assert!(results[0].output.is_empty());
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn schema_thirty_nine_preserves_legacy_cell_output() {
        let root = std::env::temp_dir().join(format!(
            "beale-runbook-stream-migration-example-{}",
            Uuid::new_v4()
        ));
        let db = HostDatabase::open(&root).unwrap();
        drop(db);
        let connection = rusqlite::Connection::open(root.join("state.sqlite")).unwrap();
        connection
            .execute_batch(
                "ALTER TABLE runbook_cell_results DROP COLUMN exit_code;
                 ALTER TABLE runbook_cell_results DROP COLUMN stdout;
                 ALTER TABLE runbook_cell_results DROP COLUMN stderr;
                 INSERT INTO runbook_cell_results(workspace_id,runbook_id,session_id,cell_id,status,updated_at_ms,output)
                 VALUES('workspace-example','R1','session-example','L1','completed',123,'exit status: exit code: 0\nstdout:\nExample output\nstderr:\n');
                 PRAGMA user_version = 39;",
            )
            .unwrap();
        drop(connection);
        let reopened = HostDatabase::open(&root).unwrap();
        let results = reopened
            .session_runbook_cell_results("workspace-example", "session-example", "R1")
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].output.contains("Example output"));
        assert!(results[0].stdout.is_empty());
        assert!(results[0].stderr.is_empty());
        assert_eq!(results[0].exit_code, None);
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn revision_snapshots_survive_reopen_and_cell_ids_do_not_reuse() {
        let root = std::env::temp_dir().join(format!(
            "beale-runbook-storage-example-{}",
            uuid::Uuid::new_v4()
        ));
        let db = HostDatabase::open(&root).unwrap();
        db.replace_workspaces(&[WorkspaceRow {
            id: "workspace-example".into(),
            name: "Example".into(),
            directory: "example-directory".into(),
        }])
        .unwrap();
        let mut book = db
            .runbook_create(
                Document {
                    id: String::new(),
                    workspace_id: "workspace-example".into(),
                    title: "Example procedure".into(),
                    purpose: "Review synthetic behavior".into(),
                    cells: vec![Cell::Code {
                        id: String::new(),
                        name: String::new(),
                        language: "Rust".into(),
                        source: "fn main() {}".into(),
                    }],
                    next_cell_number: 1,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                    revision: 0,
                },
                "agent",
                "session-example",
            )
            .unwrap();
        assert_eq!(book.id, "R1");
        assert_eq!(book.cells[0].id(), "L1");
        book.cells.clear();
        book = db
            .runbook_edit(book, 1, "agent", "session-example", "remove obsolete step")
            .unwrap();
        book.cells.push(Cell::Markdown {
            id: String::new(),
            name: String::new(),
            source: "Revised procedure".into(),
        });
        book = db
            .runbook_edit(book, 2, "agent", "session-example", "add updated step")
            .unwrap();
        assert_eq!(book.cells[0].id(), "L2");
        let revisions: i64 = db.with_connection(|connection| Ok(connection.query_row("SELECT count(*) FROM runbook_revisions WHERE workspace_id=?1 AND runbook_id=?2", params!["workspace-example","R1"], |row| row.get(0))?)).unwrap();
        assert_eq!(revisions, 3);
        drop(db);
        let reopened = HostDatabase::open(&root).unwrap();
        let saved = reopened
            .runbook_get("workspace-example", "R1")
            .unwrap()
            .unwrap();
        assert_eq!(saved.revision, 3);
        assert_eq!(saved.cells[0].id(), "L2");
    }

    #[test]
    fn session_runbook_counts_and_execution_status_survive_reopen() {
        let root =
            std::env::temp_dir().join(format!("beale-runbook-run-example-{}", Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        db.replace_workspaces(&[WorkspaceRow {
            id: "workspace-example".into(),
            name: "Example".into(),
            directory: "example-directory".into(),
        }])
        .unwrap();
        let document = db
            .runbook_create(
                Document {
                    id: String::new(),
                    workspace_id: "workspace-example".into(),
                    title: "Example procedure".into(),
                    purpose: "Test synthetic behavior".into(),
                    cells: vec![],
                    next_cell_number: 1,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                    revision: 0,
                },
                "agent",
                "session-example",
            )
            .unwrap();
        assert_eq!(
            db.session_runbook_counts("session-example").unwrap(),
            (1, 0)
        );
        let prepared = db
            .session_runbooks("workspace-example", "session-example")
            .unwrap();
        assert_eq!(prepared.len(), 1);
        assert_eq!(prepared[0].status, "prepared");
        assert!(
            db.runbook_run_start("workspace-example", "session-example", &document.id, 2)
                .is_err()
        );
        let run = db
            .runbook_run_start(
                "workspace-example",
                "session-example",
                &document.id,
                document.revision,
            )
            .unwrap();
        assert_eq!(
            db.session_runbook_counts("session-example").unwrap(),
            (1, 1)
        );
        db.runbook_run_cell(&run.id, 2, "Example step").unwrap();
        let active = db
            .session_runbooks("workspace-example", "session-example")
            .unwrap();
        assert_eq!(active[0].status, "running");
        assert_eq!(active[0].current_cell_number, 2);
        assert_eq!(active[0].current_cell_name, "Example step");
        assert!(
            db.runbook_run_status("other-session", &run.id)
                .unwrap()
                .is_none()
        );
        db.runbook_run_finish(&run.id, true, "Example output")
            .unwrap();
        let runs = db
            .session_runbook_runs("workspace-example", "session-example", &document.id)
            .unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].output, "Example output");
        assert_eq!(runs[0].current_cell_number, 2);
        assert!(
            db.session_runbook_runs("workspace-example", "other-session", &document.id)
                .unwrap()
                .is_empty()
        );
        let done = db
            .session_runbooks("workspace-example", "session-example")
            .unwrap();
        assert_eq!(done[0].status, "done");
        assert!(done[0].completed_at_ms > 0);
        assert_eq!(
            db.session_runbook_counts("session-example").unwrap(),
            (1, 0)
        );
        assert_eq!(
            db.runbook_run_status("session-example", &run.id)
                .unwrap()
                .unwrap()
                .status,
            "completed"
        );
        let other = db
            .runbook_create(
                Document {
                    id: String::new(),
                    workspace_id: "workspace-example".into(),
                    title: "Other procedure".into(),
                    purpose: "Synthetic check".into(),
                    cells: vec![],
                    next_cell_number: 1,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                    revision: 0,
                },
                "agent",
                "other-session",
            )
            .unwrap();
        db.runbook_edit(
            other.clone(),
            1,
            "agent",
            "session-example",
            "Example revision",
        )
        .unwrap();
        assert_eq!(
            db.session_runbook_counts("session-example").unwrap(),
            (2, 0)
        );
        let listed = db
            .session_runbooks("workspace-example", "session-example")
            .unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(
            listed
                .iter()
                .find(|book| book.id == other.id)
                .unwrap()
                .status,
            "prepared"
        );
        let interrupted = db
            .runbook_run_start(
                "workspace-example",
                "session-example",
                &document.id,
                document.revision,
            )
            .unwrap();
        drop(db);
        let reopened = HostDatabase::open(&root).unwrap();
        assert_eq!(
            reopened.session_runbook_counts("session-example").unwrap(),
            (2, 0)
        );
        assert_eq!(
            reopened
                .session_runbooks("workspace-example", "session-example")
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            reopened
                .runbook_run_status("session-example", &interrupted.id)
                .unwrap()
                .unwrap()
                .status,
            "interrupted"
        );
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }
}
