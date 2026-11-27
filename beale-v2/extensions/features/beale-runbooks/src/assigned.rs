//! Canonical half of assigned execution. Every write borrows the worker receipt
//! transaction; the caller verifies the active lease and performs saved-policy
//! review before preparation. No target execution occurs in these functions.
use crate::{RunbookStore, transaction_store};
use anyhow::{Context, Result, ensure};
use beale_contracts::{
    runbooks::{Cell, Document},
    swarm_runbooks::{CellOutcome, FinishedRun, PreparedRun},
    swarm_worker::ExecutionIdentity,
    tool_error::ToolError,
};
use beale_storage::rusqlite::{OptionalExtension, Transaction, params};

pub fn prepare(
    tx: &Transaction<'_>,
    identity: &ExecutionIdentity,
    session_id: &str,
    reviewed: &Document,
) -> Result<PreparedRun> {
    identity.validate().map_err(anyhow::Error::msg)?;
    ensure!(
        reviewed.workspace_id == identity.workspace_id,
        "Runbook workspace mismatch"
    );
    let store = transaction_store(tx);
    let current = store
        .runbook_get(&identity.workspace_id, &reviewed.id)?
        .context(ToolError::missing("Runbook unavailable"))?;
    ensure!(
        current == *reviewed,
        ToolError::conflict("Runbook changed after review")
    );
    ensure!(
        reviewed
            .cells
            .iter()
            .any(|c| matches!(c, Cell::Code { .. })),
        ToolError::rejected("Runbook has no code cells")
    );
    let run = store.runbook_run_start(
        &identity.workspace_id,
        session_id,
        &reviewed.id,
        reviewed.revision,
    )?;
    tx.execute(
        "INSERT INTO runbook_worker_runs(run_id,identity,document) VALUES(?1,?2,?3)",
        params![
            run.id,
            serde_json::to_string(identity)?,
            serde_json::to_string(reviewed)?
        ],
    )?;
    Ok(PreparedRun {
        run_id: run.id,
        document: reviewed.clone(),
    })
}

fn owned(
    tx: &Transaction<'_>,
    identity: &ExecutionIdentity,
    run_id: &str,
) -> Result<(Document, bool)> {
    identity.validate().map_err(anyhow::Error::msg)?;
    let (owner, document, finished): (String, String, bool) = tx.query_row(
        "SELECT identity,document,finished FROM runbook_worker_runs WHERE run_id=?1",
        [run_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    ensure!(
        serde_json::from_str::<ExecutionIdentity>(&owner)? == *identity,
        "Runbook execution owner mismatch"
    );
    Ok((serde_json::from_str(&document)?, finished))
}

fn outcomes(tx: &Transaction<'_>, run_id: &str, document: &Document) -> Result<Vec<CellOutcome>> {
    let mut result = Vec::new();
    for cell in &document.cells {
        if !matches!(cell, Cell::Code { .. }) {
            continue;
        }
        let saved: Option<String> = tx
            .query_row(
                "SELECT outcome FROM runbook_worker_cells WHERE run_id=?1 AND cell_id=?2",
                params![run_id, cell.id()],
                |r| r.get(0),
            )
            .optional()?;
        let Some(saved) = saved else {
            break;
        };
        result.push(serde_json::from_str(&saved)?);
    }
    Ok(result)
}

/// Records the next code cell before dispatch. Completion remains a separate
/// durable result, and replay cannot regress progress to a completed cell.
pub fn begin_cell(
    tx: &Transaction<'_>,
    identity: &ExecutionIdentity,
    run_id: &str,
    cell_id: &str,
) -> Result<()> {
    let (document, finished) = owned(tx, identity, run_id)?;
    let prior = outcomes(tx, run_id, &document)?;
    if prior.iter().any(|result| result.cell_id == cell_id) {
        return Ok(());
    }
    ensure!(
        !finished && prior.iter().all(|result| result.success),
        "Runbook execution has stopped"
    );
    let (index, cell) = document
        .cells
        .iter()
        .enumerate()
        .filter(|(_, cell)| matches!(cell, Cell::Code { .. }))
        .nth(prior.len())
        .context("Runbook has no pending code cell")?;
    ensure!(cell.id() == cell_id, "Runbook cell dispatch out of order");
    transaction_store(tx).runbook_run_cell(run_id, u32::try_from(index + 1)?, cell.name())
}

pub fn accept_cell(
    tx: &Transaction<'_>,
    identity: &ExecutionIdentity,
    run_id: &str,
    result: &CellOutcome,
) -> Result<()> {
    result.validate().map_err(anyhow::Error::msg)?;
    let (document, finished) = owned(tx, identity, run_id)?;
    let prior = outcomes(tx, run_id, &document)?;
    if let Some(saved) = prior.iter().find(|r| r.cell_id == result.cell_id) {
        ensure!(saved == result, "Runbook cell result changed on retry");
        return Ok(());
    }
    ensure!(
        !finished && prior.iter().all(|r| r.success),
        "Runbook execution has stopped"
    );
    let (index, cell) = document
        .cells
        .iter()
        .enumerate()
        .filter(|(_, c)| matches!(c, Cell::Code { .. }))
        .nth(prior.len())
        .context("Runbook has no pending code cell")?;
    ensure!(
        cell.id() == result.cell_id,
        "Runbook cell result out of order"
    );
    let store = transaction_store(tx);
    store.runbook_run_cell(run_id, u32::try_from(index + 1)?, cell.name())?;
    store.runbook_run_cell_result(
        run_id,
        &result.cell_id,
        result.success,
        result.exit_code,
        &result.stdout,
        &result.stderr,
    )?;
    tx.execute(
        "INSERT INTO runbook_worker_cells(run_id,cell_id,outcome) VALUES(?1,?2,?3)",
        params![run_id, result.cell_id, serde_json::to_string(result)?],
    )?;
    Ok(())
}

pub fn finish(
    tx: &Transaction<'_>,
    identity: &ExecutionIdentity,
    run_id: &str,
) -> Result<FinishedRun> {
    let (document, finished) = owned(tx, identity, run_id)?;
    if finished {
        let (status, output): (String, String) = tx.query_row(
            "SELECT status,output FROM runbook_runs WHERE id=?1",
            [run_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        ensure!(
            matches!(status.as_str(), "completed" | "failed"),
            "Runbook execution interrupted"
        );
        return Ok(FinishedRun {
            run_id: run_id.into(),
            runbook_id: document.id,
            success: status == "completed",
            output,
        });
    }
    let results = outcomes(tx, run_id, &document)?;
    let failed = results.iter().any(|r| !r.success);
    let code_count = document
        .cells
        .iter()
        .filter(|c| matches!(c, Cell::Code { .. }))
        .count();
    ensure!(
        failed || results.len() == code_count,
        "Runbook cell results incomplete"
    );
    let mut output = String::new();
    for result in &results {
        output.push_str(&format!(
            "Cell {}:\nexit code: {:?}\nstdout:\n{}\nstderr:\n{}\n",
            result.cell_id, result.exit_code, result.stdout, result.stderr
        ));
    }
    // Full accepted cell results remain in the per-run ledger. The ordinary
    // Runbook summary has a smaller bound and must not discard those records.
    if output.len() > 64_000 {
        let mut end = 63_900;
        while !output.is_char_boundary(end) {
            end -= 1;
        }
        output.truncate(end);
        output.push_str(
            "\n[Summary truncated; full cell results retained in the assigned run ledger.]",
        );
    }
    transaction_store(tx).runbook_run_finish(run_id, !failed, &output)?;
    tx.execute(
        "UPDATE runbook_worker_runs SET finished=1 WHERE run_id=?1",
        [run_id],
    )?;
    Ok(FinishedRun {
        run_id: run_id.into(),
        runbook_id: document.id,
        success: !failed,
        output,
    })
}

/// Called only by host reconciliation after the attempt has stopped. Completed
/// records remain intact; partially completed runs retain their accepted cells.
pub fn interrupt_attempt(tx: &Transaction<'_>, identity: &ExecutionIdentity) -> Result<()> {
    identity.validate().map_err(anyhow::Error::msg)?;
    let owner = serde_json::to_string(identity)?;
    let mut query =
        tx.prepare("SELECT run_id FROM runbook_worker_runs WHERE identity=?1 AND finished=0")?;
    let runs = query
        .query_map([&owner], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for id in runs {
        transaction_store(tx).runbook_run_interrupt(&id)?;
        tx.execute(
            "UPDATE runbook_worker_runs SET finished=1 WHERE run_id=?1",
            [id],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use beale_storage::{HostDatabase, WorkspaceRow};
    use std::path::PathBuf;

    struct Fixture {
        root: PathBuf,
        db: HostDatabase,
        identity: ExecutionIdentity,
        document: Document,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "beale-assigned-runbook-example-{}",
                uuid::Uuid::new_v4()
            ));
            let db = HostDatabase::open(&root).unwrap();
            crate::migrate(&db).unwrap();
            db.replace_workspaces(&[WorkspaceRow {
                id: "workspace-example".into(),
                name: "Example".into(),
                directory: root.join("workspace").to_string_lossy().into(),
            }])
            .unwrap();
            let identity = ExecutionIdentity {
                workspace_id: "workspace-example".into(),
                scan_id: "scan-example".into(),
                agent_id: "agent-example".into(),
                attempt_id: "attempt-example".into(),
                generation: 1,
            };
            let document = db
                .runbook_create(
                    Document {
                        id: String::new(),
                        workspace_id: identity.workspace_id.clone(),
                        title: "Example procedure".into(),
                        purpose: "Exercise synthetic results".into(),
                        cells: vec![
                            Cell::Markdown {
                                id: String::new(),
                                name: String::new(),
                                source: "Synthetic instructions".into(),
                            },
                            Cell::Code {
                                id: String::new(),
                                name: "First".into(),
                                language: "python".into(),
                                source: "print('example one')".into(),
                            },
                            Cell::Code {
                                id: String::new(),
                                name: "Second".into(),
                                language: "python".into(),
                                source: "print('example two')".into(),
                            },
                        ],
                        next_cell_number: 1,
                        created_at_ms: 0,
                        updated_at_ms: 0,
                        revision: 0,
                    },
                    "agent",
                    &identity.agent_id,
                )
                .unwrap();
            Self {
                root,
                db,
                identity,
                document,
            }
        }
        fn tx<T>(&self, call: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
            self.db.with_connection(|c| {
                let tx = c.transaction()?;
                let value = call(&tx)?;
                tx.commit()?;
                Ok(value)
            })
        }
        fn prepare(&self) -> PreparedRun {
            self.tx(|tx| prepare(tx, &self.identity, &self.identity.agent_id, &self.document))
                .unwrap()
        }
        fn outcome(&self, index: usize, success: bool) -> CellOutcome {
            CellOutcome {
                cell_id: self.document.cells[index].id().into(),
                success,
                exit_code: Some(if success { 0 } else { 1 }),
                stdout: "Synthetic output".into(),
                stderr: String::new(),
            }
        }
        fn close(self) {
            drop(self.db);
            std::fs::remove_dir_all(self.root).unwrap();
        }
    }

    #[test]
    fn assigned_results_bind_attempt_order_and_reviewed_revision() {
        let f = Fixture::new();
        let run = f.prepare();
        assert!(
            f.tx(|tx| begin_cell(tx, &f.identity, &run.run_id, f.document.cells[2].id()))
                .is_err()
        );
        f.tx(|tx| begin_cell(tx, &f.identity, &run.run_id, f.document.cells[1].id()))
            .unwrap();
        assert_eq!(
            f.db.session_runbook_runs(
                &f.identity.workspace_id,
                &f.identity.agent_id,
                &f.document.id
            )
            .unwrap()[0]
                .current_cell_number,
            2
        );
        assert!(
            f.db.session_runbook_cell_results(
                &f.identity.workspace_id,
                &f.identity.agent_id,
                &f.document.id
            )
            .unwrap()
            .is_empty()
        );
        for change in 0..5 {
            let mut identity = f.identity.clone();
            match change {
                0 => identity.workspace_id = "foreign-workspace".into(),
                1 => identity.scan_id = "foreign-scan".into(),
                2 => identity.agent_id = "foreign-agent".into(),
                3 => identity.attempt_id = "new-attempt".into(),
                _ => identity.generation += 1,
            }
            assert!(
                f.tx(|tx| accept_cell(tx, &identity, &run.run_id, &f.outcome(1, true)))
                    .is_err()
            );
            assert!(f.tx(|tx| finish(tx, &identity, &run.run_id)).is_err());
        }
        assert!(f.tx(|tx| finish(tx, &f.identity, &run.run_id)).is_err());
        assert!(
            f.tx(|tx| accept_cell(tx, &f.identity, &run.run_id, &f.outcome(2, true)))
                .is_err()
        );
        f.tx(|tx| accept_cell(tx, &f.identity, &run.run_id, &f.outcome(1, true)))
            .unwrap();
        let mut changed = f.outcome(1, true);
        changed.stdout = "Changed result".into();
        assert!(
            f.tx(|tx| accept_cell(tx, &f.identity, &run.run_id, &changed))
                .is_err()
        );
        f.tx(|tx| accept_cell(tx, &f.identity, &run.run_id, &f.outcome(1, true)))
            .unwrap();
        let mut edited = f.document.clone();
        if let Cell::Code { source, .. } = &mut edited.cells[2] {
            *source = "print('changed example')".into();
        }
        f.db.runbook_edit(edited, 1, "agent", &f.identity.agent_id, "Synthetic edit")
            .unwrap();
        assert!(
            f.tx(|tx| prepare(tx, &f.identity, &f.identity.agent_id, &f.document))
                .is_err()
        );
        // Results belong to the prepared revision even after the current document changes.
        f.tx(|tx| accept_cell(tx, &f.identity, &run.run_id, &f.outcome(2, true)))
            .unwrap();
        let complete = f.tx(|tx| finish(tx, &f.identity, &run.run_id)).unwrap();
        assert!(complete.success);
        assert_eq!(
            f.tx(|tx| finish(tx, &f.identity, &run.run_id)).unwrap(),
            complete
        );
        let current =
            f.db.session_runbook_cell_results(
                &f.identity.workspace_id,
                &f.identity.agent_id,
                &f.document.id,
            )
            .unwrap();
        assert_eq!(current.len(), 1); // Changed code is not given an old cell result.
        f.db.with_connection(|db| {
            let stored: String = db.query_row(
                "SELECT document FROM runbook_worker_runs WHERE run_id=?1",
                [&run.run_id],
                |r| r.get(0),
            )?;
            assert_eq!(serde_json::from_str::<Document>(&stored)?, f.document);
            let count: i64 = db.query_row(
                "SELECT COUNT(*) FROM runbook_worker_cells WHERE run_id=?1",
                [&run.run_id],
                |r| r.get(0),
            )?;
            assert_eq!(count, 2);
            Ok(())
        })
        .unwrap();
        f.close();
    }

    #[test]
    fn assigned_run_writes_share_receipt_transaction_and_survive_host_restart() {
        let f = Fixture::new();
        let failed: Result<()> = f.tx(|tx| {
            prepare(tx, &f.identity, &f.identity.agent_id, &f.document)?;
            anyhow::bail!("synthetic receipt write failed")
        });
        assert!(failed.is_err());
        assert!(
            f.db.session_runbook_runs(
                &f.identity.workspace_id,
                &f.identity.agent_id,
                &f.document.id
            )
            .unwrap()
            .is_empty()
        );
        let run = f.prepare();
        let failed: Result<()> = f.tx(|tx| {
            accept_cell(tx, &f.identity, &run.run_id, &f.outcome(1, true))?;
            anyhow::bail!("synthetic cell receipt failed")
        });
        assert!(failed.is_err());
        assert!(
            f.db.session_runbook_cell_results(
                &f.identity.workspace_id,
                &f.identity.agent_id,
                &f.document.id
            )
            .unwrap()
            .is_empty()
        );
        f.tx(|tx| accept_cell(tx, &f.identity, &run.run_id, &f.outcome(1, true)))
            .unwrap();
        let local =
            f.db.runbook_run_start(
                &f.identity.workspace_id,
                &f.identity.agent_id,
                &f.document.id,
                1,
            )
            .unwrap();
        let reopened = HostDatabase::open(&f.root).unwrap();
        crate::migrate(&reopened).unwrap();
        assert_eq!(
            reopened
                .runbook_run_status(&f.identity.agent_id, &run.run_id)
                .unwrap()
                .unwrap()
                .status,
            "running"
        );
        assert_eq!(
            reopened
                .runbook_run_status(&f.identity.agent_id, &local.id)
                .unwrap()
                .unwrap()
                .status,
            "interrupted"
        );
        reopened
            .with_connection(|c| {
                let tx = c.transaction()?;
                accept_cell(&tx, &f.identity, &run.run_id, &f.outcome(2, false))?;
                tx.commit()?;
                Ok(())
            })
            .unwrap();
        let failed: Result<()> = f.tx(|tx| {
            finish(tx, &f.identity, &run.run_id)?;
            anyhow::bail!("synthetic finish receipt failed")
        });
        assert!(failed.is_err());
        assert_eq!(
            reopened
                .runbook_run_status(&f.identity.agent_id, &run.run_id)
                .unwrap()
                .unwrap()
                .status,
            "running"
        );
        let complete = f.tx(|tx| finish(tx, &f.identity, &run.run_id)).unwrap();
        assert!(!complete.success);
        assert_eq!(
            reopened
                .runbook_run_status(&f.identity.agent_id, &run.run_id)
                .unwrap()
                .unwrap()
                .status,
            "failed"
        );
        drop(reopened);
        f.close();
    }

    #[test]
    fn assigned_failure_and_confirmed_interruption_preserve_prior_cells() {
        let f = Fixture::new();
        let failed = f.prepare();
        f.tx(|tx| accept_cell(tx, &f.identity, &failed.run_id, &f.outcome(1, false)))
            .unwrap();
        assert!(
            f.tx(|tx| accept_cell(tx, &f.identity, &failed.run_id, &f.outcome(2, true)))
                .is_err()
        );
        assert!(
            !f.tx(|tx| finish(tx, &f.identity, &failed.run_id))
                .unwrap()
                .success
        );
        let interrupted = f.prepare();
        f.tx(|tx| accept_cell(tx, &f.identity, &interrupted.run_id, &f.outcome(1, true)))
            .unwrap();
        f.tx(|tx| interrupt_attempt(tx, &f.identity)).unwrap();
        f.tx(|tx| interrupt_attempt(tx, &f.identity)).unwrap();
        assert!(
            f.tx(|tx| finish(tx, &f.identity, &interrupted.run_id))
                .is_err()
        );
        assert!(
            f.tx(|tx| accept_cell(tx, &f.identity, &interrupted.run_id, &f.outcome(2, true)))
                .is_err()
        );
        assert_eq!(
            f.db.runbook_run_status(&f.identity.agent_id, &failed.run_id)
                .unwrap()
                .unwrap()
                .status,
            "failed"
        );
        assert_eq!(
            f.db.runbook_run_status(&f.identity.agent_id, &interrupted.run_id)
                .unwrap()
                .unwrap()
                .status,
            "interrupted"
        );
        f.db.with_connection(|db| {
            let count: i64 = db.query_row(
                "SELECT COUNT(*) FROM runbook_worker_cells WHERE run_id=?1",
                [&interrupted.run_id],
                |r| r.get(0),
            )?;
            assert_eq!(count, 1);
            Ok(())
        })
        .unwrap();
        f.close();
    }

    #[test]
    fn bounded_run_summary_retains_full_unicode_cell_results() {
        let mut f = Fixture::new();
        let mut edited = f.document.clone();
        edited.cells = (0..4)
            .map(|_| Cell::Code {
                id: String::new(),
                name: String::new(),
                language: "python".into(),
                source: "print('example')".into(),
            })
            .collect();
        f.document =
            f.db.runbook_edit(
                edited,
                1,
                "agent",
                &f.identity.agent_id,
                "Synthetic output coverage",
            )
            .unwrap();
        let run = f.prepare();
        let output = "é".repeat(10_000);
        for cell in &f.document.cells {
            f.tx(|tx| {
                accept_cell(
                    tx,
                    &f.identity,
                    &run.run_id,
                    &CellOutcome {
                        cell_id: cell.id().into(),
                        success: true,
                        exit_code: Some(0),
                        stdout: output.clone(),
                        stderr: String::new(),
                    },
                )
            })
            .unwrap();
        }
        let summary = f.tx(|tx| finish(tx, &f.identity, &run.run_id)).unwrap();
        assert!(summary.success && summary.output.len() <= 64_000);
        assert!(summary.output.contains("Summary truncated"));
        let saved = f.tx(|tx| outcomes(tx, &run.run_id, &f.document)).unwrap();
        assert_eq!(saved.len(), 4);
        assert!(saved.iter().all(|cell| cell.stdout == output));
        f.close();
    }
}
