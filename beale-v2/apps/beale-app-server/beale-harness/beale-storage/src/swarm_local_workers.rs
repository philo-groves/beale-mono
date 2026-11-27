//! Host launch intents survive process exit. No credentials or host paths are
//! stored here, and these records are never returned through the worker router.
use super::*;

pub(super) const SCHEMA: &str = "CREATE TABLE swarm_local_workers (
 attempt_id TEXT PRIMARY KEY, scan_id TEXT NOT NULL REFERENCES swarm_scans(id),
 owner_key TEXT NOT NULL UNIQUE, backend TEXT NOT NULL, turn TEXT NOT NULL,
 complete INTEGER NOT NULL DEFAULT 0 CHECK(complete IN (0,1))
);";

#[derive(Clone, Debug)]
pub struct LocalWorkerRecord {
    pub turn: LocalTurn,
    pub owner_key: Uuid,
    pub backend: String,
}

pub fn local_worker_backend() -> &'static str {
    if cfg!(windows) {
        "windows_job"
    } else if cfg!(target_os = "linux") {
        "linux_subreaper"
    } else {
        "unix_group"
    }
}

impl SwarmStore {
    /// Persist before creating any process and while holding owner_key's OS lock.
    pub fn register_local_worker(&self, turn: &LocalTurn, owner_key: Uuid) -> Result<()> {
        self.transaction(|tx| register_in(tx, turn, owner_key))
    }

    /// Read identities separately so one corrupt record cannot starve later work.
    pub fn local_worker_recovery_page(
        &self,
        scan: Option<&str>,
        after: &str,
    ) -> Result<Vec<String>> {
        self.db.with_connection(|db| {
            let mut statement=db.prepare("SELECT attempt_id FROM swarm_local_workers WHERE complete=0 AND (?1 IS NULL OR scan_id=?1) AND attempt_id>?2 ORDER BY attempt_id LIMIT 64")?;
            Ok(statement.query_map(params![scan,after],|r|r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn local_worker_record(&self, attempt: &str) -> Result<LocalWorkerRecord> {
        self.db.with_connection(|db| {
            let (scan,key,backend,turn):(String,String,String,String)=db.query_row("SELECT scan_id,owner_key,backend,turn FROM swarm_local_workers WHERE attempt_id=?1",[attempt],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
            let turn:LocalTurn=serde_json::from_str(&turn)?;
            ensure!(turn.attempt_id==attempt && turn.scan_id==scan && matches!(backend.as_str(),"windows_job"|"unix_group"|"linux_subreaper"),"invalid local launch record");
            Ok(LocalWorkerRecord { turn, owner_key:Uuid::parse_str(&key)?, backend })
        })
    }

    pub fn local_worker_records(
        &self,
        scan: Option<&str>,
        after: &str,
    ) -> Result<Vec<LocalWorkerRecord>> {
        self.local_worker_recovery_page(scan, after)?
            .iter()
            .map(|id| self.local_worker_record(id))
            .collect()
    }

    /// Atomic recovery of an intent that never reached preparation. It cannot
    /// have released startup input, even if a lost coordinator fenced it first.
    pub fn reconcile_unlaunched_worker(&self, record: &LocalWorkerRecord) -> Result<()> {
        self.transaction(|tx| {
            let key: String = tx.query_row(
                "SELECT owner_key FROM swarm_local_workers WHERE attempt_id=?1",
                [&record.turn.attempt_id],
                |r| r.get(0),
            )?;
            ensure!(
                key == record.owner_key.to_string(),
                "unlaunched owner mismatch"
            );
            abandon_unprepared_in(tx, &record.turn)?;
            tx.execute(
                "UPDATE swarm_local_workers SET complete=1 WHERE attempt_id=?1",
                [&record.turn.attempt_id],
            )?;
            Ok(())
        })
    }

    pub fn complete_local_worker_recovery(&self, record: &LocalWorkerRecord) -> Result<()> {
        self.transaction(|tx| {
            let state=read_scan(tx,&record.turn.scan_id)?;
            ensure!(state.attempts.iter().any(|a|a.id==record.turn.attempt_id && a.state==ExecutionState::Stopped),"local execution is not terminated");
            ensure!(tx.execute("UPDATE swarm_local_workers SET complete=1 WHERE attempt_id=?1 AND owner_key=?2",params![record.turn.attempt_id,record.owner_key.to_string()])?==1,"local launch identity mismatch");
            Ok(())
        })
    }

    /// Receipt existence, rather than a candidate answer, proves that canonical
    /// reconciliation already committed. Recovery must not checkpoint a newer
    /// generation's writable workspace using an old turn.
    pub fn worker_reconciled(&self, turn: &LocalTurn) -> Result<bool> {
        self.db.with_connection(|db| {
            let saved:Option<(String,String)>=db.query_row("SELECT content,result FROM swarm_receipts WHERE scan_id=?1 AND actor='supervisor' AND operation_id=?2",params![turn.scan_id,format!("reconcile:{}",turn.attempt_id)],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            let Some((content,result))=saved else { return Ok(false); };
            let disposition:WorkerDisposition=serde_json::from_str(&content)?;
            let result:WorkerReconciliation=serde_json::from_str(&result)?;
            ensure!(result==if disposition==WorkerDisposition::Completed {WorkerReconciliation::TurnAccepted} else {WorkerReconciliation::BranchStopped},"invalid worker reconciliation receipt");
            let state=read_scan(db,&turn.scan_id)?;
            ensure!(state.attempts.iter().any(|a|a.id==turn.attempt_id && a.agent_id==turn.agent_id && a.generation==turn.generation && a.state==ExecutionState::Stopped),"reconciliation receipt has no terminated execution");
            Ok(true)
        })
    }
}

pub(super) fn register_in(tx: &Transaction<'_>, turn: &LocalTurn, owner_key: Uuid) -> Result<()> {
    let state = read_scan(tx, &turn.scan_id)?;
    local::ensure_local_turn(&state, turn)?;
    ensure!(
        state.config.destinations.is_empty() && turn.role != Role::Leader,
        "not a local subordinate assignment"
    );
    ensure!(
        !tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM swarm_worker_runtime WHERE attempt_id=?1)",
            [&turn.attempt_id],
            |r| r.get::<_, bool>(0)
        )?,
        "worker already prepared without this launch intent"
    );
    tx.execute("INSERT INTO swarm_local_workers(attempt_id,scan_id,owner_key,backend,turn) VALUES(?1,?2,?3,?4,?5)", params![turn.attempt_id,turn.scan_id,owner_key.to_string(),local_worker_backend(),serde_json::to_string(turn)?])?;
    Ok(())
}

pub(super) fn abandon_unprepared_in(tx: &Transaction<'_>, turn: &LocalTurn) -> Result<()> {
    ensure!(
        !tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM swarm_worker_runtime WHERE attempt_id=?1)",
            [&turn.attempt_id],
            |r| r.get::<_, bool>(0)
        )?,
        "worker was prepared"
    );
    let mut state = read_scan(tx, &turn.scan_id)?;
    let attempt = state
        .attempts
        .iter()
        .find(|a| {
            a.id == turn.attempt_id
                && a.agent_id == turn.agent_id
                && a.generation == turn.generation
        })
        .context("missing unlaunched attempt")?;
    if attempt.state != ExecutionState::Stopped && state.ended_at_ms.is_some() {
        local::set_state(&mut state, &turn.agent_id, AgentState::Stopped)?;
        for assignment in state.assignments.iter_mut().filter(|a| {
            a.child_id == turn.agent_id
                && matches!(
                    a.state,
                    AssignmentState::Queued | AssignmentState::Assigned | AssignmentState::Blocked
                )
        }) {
            assignment.state = AssignmentState::Canceled;
        }
        local::prune_pending(&mut state);
        write_scan(tx, &mut state)?;
        fence_in_transaction(tx, &turn.scan_id, &turn.attempt_id, true)?;
    } else if attempt.state != ExecutionState::Stopped {
        let stopped = local::read_local(tx, &turn.scan_id)?
            .controls
            .iter()
            .any(|c| c.agent_id == turn.agent_id && c.stop_requested);
        if stopped {
            local::finish_turn(
                tx,
                turn,
                json!({}),
                "Canceled before worker startup.",
                true,
                true,
            )?;
        } else {
            local::fail_turn(tx, turn, None, true)?;
        }
    }
    Ok(())
}
