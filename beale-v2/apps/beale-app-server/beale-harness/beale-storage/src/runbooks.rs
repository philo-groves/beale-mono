use super::HostDatabase;
use anyhow::{Context, Result, ensure};
use beale_contracts::runbooks::{Cell, Document, QueryRequest, Summary};
use rusqlite::{OptionalExtension, Transaction, params};
use std::{
    collections::HashSet,
    time::{SystemTime, UNIX_EPOCH},
};

fn now_ms() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .min(i64::MAX as u128) as u64)
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
        !document.title.is_empty() && document.title.len() <= 200,
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
}
