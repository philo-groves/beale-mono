//! Temporary worker recovery storage, deliberately separate from HostDatabase.
//! A spool owns one execution identity; it is never a canonical research database.
use anyhow::{Context, Result, ensure};
use beale_contracts::{
    swarm_worker::{ExecutionIdentity, WorkRequest},
    swarms::{MAX_DOCUMENT_BYTES, ToolOperationState, valid_id},
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions},
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS worker_identity(singleton INTEGER PRIMARY KEY CHECK(singleton=1), version INTEGER NOT NULL, document TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS worker_operations(sequence INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE, request TEXT NOT NULL, state TEXT NOT NULL, result TEXT NOT NULL, acknowledged INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS worker_checkpoints(operation TEXT PRIMARY KEY, revision INTEGER NOT NULL UNIQUE, context TEXT NOT NULL);
";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpoolWrite {
    Claim,
    Outcome,
    Acknowledge,
    Checkpoint,
}

/// Fault injection before SQLite commit. Production uses the filesystem's errors.
pub trait SpoolFaults: Send + Sync {
    fn before_commit(&self, write: SpoolWrite) -> Result<()>;
}
struct NoFaults;
impl SpoolFaults for NoFaults {
    fn before_commit(&self, _: SpoolWrite) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingOperation {
    pub sequence: u64,
    pub request: WorkRequest,
    pub state: ToolOperationState,
    pub result: Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Claim {
    /// The journal commit succeeded. The caller must still check its current lease.
    Dispatch,
    /// Retransmit this exact request; only the leader decides whether to replay it.
    RetryLeader,
    /// An already committed result, including JSON null.
    Replay(Value),
    InFlight,
    OutcomeUnknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkpoint {
    pub operation: String,
    pub revision: u64,
    pub context: Value,
}

pub struct WorkerSpool {
    connection: Mutex<Connection>,
    identity: ExecutionIdentity,
    faults: Arc<dyn SpoolFaults>,
    // Dropped after the connection. OS ownership releases automatically on exit.
    _owner: SpoolOwner,
}

struct SpoolOwner(File);
impl Drop for SpoolOwner {
    fn drop(&mut self) {
        // A concurrent fork may retain this file description until exec even
        // though Rust opened it CLOEXEC. Release intentional ownership now;
        // closing only our descriptor can leave a spurious live-owner lock.
        let _ = self.0.unlock();
    }
}

#[cfg(all(test, unix))]
mod ownership_tests {
    use super::*;

    #[test]
    fn releasing_a_spool_unlocks_descriptors_duplicated_during_child_startup() {
        let root = std::env::temp_dir().join(format!(
            "beale-spool-owner-example-{}",
            uuid::Uuid::new_v4()
        ));
        let identity = ExecutionIdentity {
            workspace_id: "workspace-example".into(),
            scan_id: "scan-example".into(),
            agent_id: "agent-example".into(),
            attempt_id: "attempt-example".into(),
            generation: 1,
        };
        let first = WorkerSpool::open(&root, identity.clone()).unwrap();
        // fork inherits the same open file description until exec closes CLOEXEC
        // descriptors. A duplicate reproduces that lifetime deterministically.
        let inherited = first._owner.0.try_clone().unwrap();
        assert!(WorkerSpool::open(&root, identity.clone()).is_err());
        drop(first);
        let resumed = WorkerSpool::open(&root, identity).unwrap();
        drop(resumed);
        drop(inherited);
        fs::remove_dir_all(root).unwrap();
    }
}

fn bounded(value: &impl serde::Serialize) -> Result<String> {
    let json = serde_json::to_string(value)?;
    ensure!(
        json.len() <= MAX_DOCUMENT_BYTES,
        "worker recovery document too large"
    );
    Ok(json)
}

impl WorkerSpool {
    pub fn open(root: &Path, identity: ExecutionIdentity) -> Result<Self> {
        Self::open_inner(root, identity, false)
    }

    /// Host reconciliation cannot invent ownership for an unbound capture tree.
    pub fn open_existing(root: &Path, identity: ExecutionIdentity) -> Result<Self> {
        Self::open_inner(root, identity, true)
    }

    fn open_inner(root: &Path, identity: ExecutionIdentity, existing: bool) -> Result<Self> {
        if existing {
            ensure!(
                fs::symlink_metadata(root.join("recovery.sqlite"))?
                    .file_type()
                    .is_file(),
                "worker recovery database unavailable"
            );
        }
        identity.validate().map_err(anyhow::Error::msg)?;
        fs::create_dir_all(root)?;
        ensure!(
            !fs::symlink_metadata(root)?.file_type().is_symlink(),
            "worker recovery root must not be a link"
        );
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let owner = options.open(root.join("owner.lock"))?;
        owner
            .try_lock()
            .context("worker execution already owns this recovery spool")?;
        let owner = SpoolOwner(owner);
        let mut connection = if existing {
            Connection::open_with_flags(
                root.join("recovery.sqlite"),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
            )?
        } else {
            Connection::open(root.join("recovery.sqlite"))?
        };
        connection.busy_timeout(Duration::from_secs(2))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        let tx = connection.transaction()?;
        tx.execute_batch(SCHEMA)?;
        let saved: Option<(u32, String)> = tx
            .query_row(
                "SELECT version,document FROM worker_identity WHERE singleton=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((version, document)) = saved {
            ensure!(version == 1, "unsupported worker recovery version");
            ensure!(
                serde_json::from_str::<ExecutionIdentity>(&document)? == identity,
                "worker recovery belongs to another execution"
            );
            // A process exit does not prove the external effects of an interrupted
            // command. Keep remote requests retryable under the original ID.
            tx.execute(
                "UPDATE worker_operations SET state='unknown' WHERE state='running' AND json_extract(request,'$.location')='environment'", [],
            )?;
        } else {
            ensure!(
                !existing,
                "worker recovery has no original execution identity"
            );
            tx.execute(
                "INSERT INTO worker_identity VALUES(1,1,?1)",
                [bounded(&identity)?],
            )?;
        }
        tx.commit()?;
        Ok(Self {
            connection: Mutex::new(connection),
            identity,
            faults: Arc::new(NoFaults),
            _owner: owner,
        })
    }

    pub fn with_faults(mut self, faults: Arc<dyn SpoolFaults>) -> Self {
        self.faults = faults;
        self
    }

    pub fn identity(&self) -> &ExecutionIdentity {
        &self.identity
    }

    /// Acknowledging uncertainty records delivery, not reconciliation. Never
    /// resume inference merely because an older unknown operation was reported.
    pub fn require_known_outcomes(&self) -> Result<()> {
        let db = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("worker recovery lock failed"))?;
        let unknown: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM worker_operations WHERE state='unknown')",
            [],
            |r| r.get(0),
        )?;
        ensure!(
            !unknown,
            "local tool outcome is unknown; parent reconciliation required"
        );
        Ok(())
    }

    fn transaction<T>(
        &self,
        write: SpoolWrite,
        action: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let mut db = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("worker recovery lock failed"))?;
        let tx = db.transaction()?;
        let result = action(&tx)?;
        self.faults.before_commit(write)?;
        tx.commit()?;
        Ok(result)
    }

    /// Commit before dispatch. This is not an execution permit: lease and review
    /// checks are still mandatory and cannot be reconstructed from this journal.
    pub fn claim(&self, request: &WorkRequest) -> Result<Claim> {
        request
            .validate(&self.identity)
            .map_err(anyhow::Error::msg)?;
        let document = bounded(request)?;
        self.transaction(SpoolWrite::Claim, |tx| {
            let saved: Option<(String, String, String)> = tx.query_row(
                "SELECT request,state,result FROM worker_operations WHERE id=?1", [request.id()],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
            ).optional()?;
            if let Some((old, state, result)) = saved {
                ensure!(old == document, "worker operation reused with changed content");
                return match state.as_str() {
                    "completed" => Ok(Claim::Replay(serde_json::from_str(&result)?)),
                    "unknown" => Ok(Claim::OutcomeUnknown),
                    "running" if matches!(request, WorkRequest::Leader { .. }) => Ok(Claim::RetryLeader),
                    "running" => Ok(Claim::InFlight),
                    _ => anyhow::bail!("invalid worker journal state"),
                };
            }
            tx.execute("INSERT INTO worker_operations(id,request,state,result) VALUES(?1,?2,'running','null')", params![request.id(), document])?;
            Ok(Claim::Dispatch)
        })
    }

    /// Persist before sending an outcome acknowledgement. Errors preserve the
    /// previous journal state; callers must suspend rather than infer completion.
    pub fn complete(&self, id: &str, result: &Value) -> Result<()> {
        ensure!(valid_id(id), "invalid worker operation identity");
        let result = bounded(result)?;
        self.transaction(SpoolWrite::Outcome, |tx| {
            let (state, old): (String, String) = tx.query_row(
                "SELECT state,result FROM worker_operations WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if state == "completed" {
                ensure!(old == result, "worker result changed on retry");
                return Ok(());
            }
            ensure!(
                state == "running",
                "uncertain outcomes require host reconciliation"
            );
            tx.execute(
                "UPDATE worker_operations SET state='completed',result=?2 WHERE id=?1",
                params![id, result],
            )?;
            Ok(())
        })
    }

    /// Records delivery only after the leader has durably accepted the outcome
    /// or its uncertainty. It does not make an unknown result safe to rerun.
    pub fn acknowledge(&self, id: &str) -> Result<()> {
        self.transaction(SpoolWrite::Acknowledge, |tx| {
            let state: String = tx.query_row(
                "SELECT state FROM worker_operations WHERE id=?1",
                [id],
                |r| r.get(0),
            )?;
            ensure!(
                matches!(state.as_str(), "completed" | "unknown"),
                "cannot acknowledge unfinished work"
            );
            tx.execute(
                "UPDATE worker_operations SET acknowledged=1 WHERE id=?1",
                [id],
            )?;
            Ok(())
        })
    }

    /// Bounded pages preserve order and include every unacknowledged output.
    pub fn pending(&self, after_sequence: u64, limit: u32) -> Result<Vec<PendingOperation>> {
        ensure!((1..=256).contains(&limit), "invalid recovery page size");
        let db = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("worker recovery lock failed"))?;
        let mut query = db.prepare("SELECT sequence,request,state,result FROM worker_operations WHERE acknowledged=0 AND sequence>?1 ORDER BY sequence LIMIT ?2")?;
        let rows = query.query_map(params![i64::try_from(after_sequence)?, limit], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        rows.map(|row| {
            let (sequence, request, state, result) = row?;
            let request: WorkRequest = serde_json::from_str(&request)?;
            request
                .validate(&self.identity)
                .map_err(anyhow::Error::msg)?;
            let state = match state.as_str() {
                "running" => ToolOperationState::Running,
                "completed" => ToolOperationState::Completed,
                "unknown" => ToolOperationState::OutcomeUnknown,
                _ => anyhow::bail!("invalid worker journal state"),
            };
            Ok(PendingOperation {
                sequence: u64::try_from(sequence)?,
                request,
                state,
                result: serde_json::from_str(&result)?,
            })
        })
        .collect()
    }

    pub fn checkpoint(
        &self,
        operation: &str,
        expected_revision: u64,
        context: &Value,
    ) -> Result<Checkpoint> {
        ensure!(valid_id(operation), "invalid checkpoint operation");
        let document = bounded(context)?;
        self.transaction(SpoolWrite::Checkpoint, |tx| {
            let prior: Option<(i64, String)> = tx
                .query_row(
                    "SELECT revision,context FROM worker_checkpoints WHERE operation=?1",
                    [operation],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((revision, old)) = prior {
                ensure!(old == document, "checkpoint retry changed content");
                return Ok(Checkpoint {
                    operation: operation.into(),
                    revision: u64::try_from(revision)?,
                    context: context.clone(),
                });
            }
            let latest: i64 = tx.query_row(
                "SELECT coalesce(max(revision),0) FROM worker_checkpoints",
                [],
                |r| r.get(0),
            )?;
            ensure!(
                u64::try_from(latest)? == expected_revision,
                "stale worker checkpoint"
            );
            let revision = latest
                .checked_add(1)
                .context("checkpoint revision overflow")?;
            tx.execute(
                "INSERT INTO worker_checkpoints VALUES(?1,?2,?3)",
                params![operation, revision, document],
            )?;
            Ok(Checkpoint {
                operation: operation.into(),
                revision: u64::try_from(revision)?,
                context: context.clone(),
            })
        })
    }

    pub fn latest_checkpoint(&self) -> Result<Option<Checkpoint>> {
        let db = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("worker recovery lock failed"))?;
        let saved: Option<(String, i64, String)> = db.query_row(
            "SELECT operation,revision,context FROM worker_checkpoints ORDER BY revision DESC LIMIT 1", [],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
        ).optional()?;
        saved
            .map(|(operation, revision, context)| {
                Ok(Checkpoint {
                    operation,
                    revision: u64::try_from(revision)?,
                    context: serde_json::from_str(&context)?,
                })
            })
            .transpose()
    }
}

#[cfg(test)]
mod full_tests {
    use super::*;
    use beale_contracts::swarm_worker::ToolInvocation;
    use serde_json::json;

    #[test]
    fn sqlite_full_preserves_worker_checkpoint_and_unacknowledged_outcome_for_retry() {
        let root =
            std::env::temp_dir().join(format!("beale-full-example-{}", uuid::Uuid::new_v4()));
        let spool = WorkerSpool::open(
            &root,
            ExecutionIdentity {
                workspace_id: "workspace-example".into(),
                scan_id: "scan-example".into(),
                agent_id: "agent-example".into(),
                attempt_id: "attempt-example".into(),
                generation: 1,
            },
        )
        .unwrap();
        let request = WorkRequest::Environment {
            call: ToolInvocation {
                id: "operation-example".into(),
                name: "write".into(),
                arguments: json!({"path":"example.txt","content":"synthetic"}),
            },
        };
        assert_eq!(spool.claim(&request).unwrap(), Claim::Dispatch);
        spool
            .checkpoint("checkpoint-one", 0, &json!({"saved":1}))
            .unwrap();
        {
            let db = spool.connection.lock().unwrap();
            let pages: i64 = db.query_row("PRAGMA page_count", [], |r| r.get(0)).unwrap();
            db.pragma_update(None, "max_page_count", pages).unwrap();
        }
        let large = json!({"synthetic":"x".repeat(64 * 1024)});
        for error in [
            spool.checkpoint("checkpoint-two", 1, &large).unwrap_err(),
            spool.complete(request.id(), &large).unwrap_err(),
        ] {
            let error = error.downcast_ref::<rusqlite::Error>().unwrap();
            assert_eq!(
                error.sqlite_error_code(),
                Some(rusqlite::ErrorCode::DiskFull)
            );
        }
        assert_eq!(spool.latest_checkpoint().unwrap().unwrap().revision, 1);
        assert_eq!(
            spool.pending(0, 10).unwrap()[0].state,
            ToolOperationState::Running
        );
        assert!(spool.acknowledge(request.id()).is_err());
        spool
            .connection
            .lock()
            .unwrap()
            .pragma_update(None, "max_page_count", 1_000_000)
            .unwrap();
        assert_eq!(
            spool
                .checkpoint("checkpoint-two", 1, &large)
                .unwrap()
                .revision,
            2
        );
        spool.complete(request.id(), &large).unwrap();
        assert_eq!(spool.claim(&request).unwrap(), Claim::Replay(large.clone()));
        drop(spool);
        let spool = WorkerSpool::open(
            &root,
            ExecutionIdentity {
                workspace_id: "workspace-example".into(),
                scan_id: "scan-example".into(),
                agent_id: "agent-example".into(),
                attempt_id: "attempt-example".into(),
                generation: 1,
            },
        )
        .unwrap();
        assert_eq!(spool.pending(0, 10).unwrap()[0].result, large);
        assert_eq!(
            spool
                .checkpoint("checkpoint-two", 1, &large)
                .unwrap()
                .revision,
            2
        );
        drop(spool);
        fs::remove_dir_all(root).unwrap();
    }
}
