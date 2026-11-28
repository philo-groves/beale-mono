//! Durable coordinator turn ownership. Lost ownership stops the hierarchy;
//! it never fabricates completion or termination evidence for external effects.
use super::*;

pub(super) const SCHEMA: &str = "CREATE TABLE swarm_local_coordinators (
 attempt_id TEXT PRIMARY KEY, scan_id TEXT NOT NULL REFERENCES swarm_scans(id),
 owner_key TEXT NOT NULL UNIQUE, turn TEXT NOT NULL,
 complete INTEGER NOT NULL DEFAULT 0 CHECK(complete IN (0,1))
);";

pub(super) const TRANSCRIPT_SCHEMA: &str = "ALTER TABLE swarm_local_coordinators ADD COLUMN committed_turn INTEGER CHECK(committed_turn > 0);";

#[test]
fn coordinator_transcript_binding_migration_preserves_completed_intents() {
    let mut db = Connection::open_in_memory().unwrap();
    crate::schema::ensure_registry(&db).unwrap();
    db.execute_batch(super::SCHEMA).unwrap();
    db.execute(
        "INSERT INTO swarm_scans(id,document) VALUES('scan-example','{}')",
        [],
    )
    .unwrap();
    crate::schema::apply(
        &mut db,
        "swarm-local-workers",
        &[local_workers::SCHEMA, SCHEMA],
    )
    .unwrap();
    db.execute("INSERT INTO swarm_local_coordinators(attempt_id,scan_id,owner_key,turn,complete) VALUES('attempt-example','scan-example','owner-example','{}',1)", []).unwrap();
    for _ in 0..2 {
        crate::schema::apply(
            &mut db,
            "swarm-local-workers",
            &[local_workers::SCHEMA, SCHEMA, TRANSCRIPT_SCHEMA],
        )
        .unwrap();
        let saved: (String, bool, Option<i64>) = db
            .query_row(
                "SELECT turn,complete,committed_turn FROM swarm_local_coordinators",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(saved, ("{}".into(), true, None));
    }
}

/// Bind a coordinator attempt to its transcript in the transcript transaction.
/// The execution remains owned until the supervisor completes its checkpoint.
pub struct CoordinatorTurnCommit<'a>(pub &'a LocalTurn);

impl crate::TurnExtension for CoordinatorTurnCommit<'_> {
    fn commit(&self, tx: &Transaction<'_>, committed: &crate::CommittedTurn<'_>) -> Result<()> {
        let turn = self.0;
        let (saved, _) = read_owner(tx, &turn.attempt_id)?;
        let state = read_scan(tx, &turn.scan_id)?;
        ensure!(
            saved == *turn
                && committed.session.id == turn.session_id
                && committed.session.workspace_id == state.config.workspace_id
                && committed.agent_role == Some(turn.profile.as_str())
                && state
                    .attempts
                    .iter()
                    .any(|a| a.id == turn.attempt_id && a.state != ExecutionState::Stopped),
            "coordinator transcript does not match live ownership"
        );
        ensure!(tx.execute("UPDATE swarm_local_coordinators SET committed_turn=?2 WHERE attempt_id=?1 AND complete=0 AND committed_turn IS NULL", params![turn.attempt_id, committed.turn_number])? == 1,
            "coordinator transcript already committed or recovered");
        Ok(())
    }
}

pub(super) fn register_in(tx: &Transaction<'_>, turn: &LocalTurn, owner_key: Uuid) -> Result<()> {
    ensure!(
        turn.role == Role::Leader,
        "coordinator intent requires leader"
    );
    tx.execute("INSERT INTO swarm_local_coordinators(attempt_id,scan_id,owner_key,turn) VALUES(?1,?2,?3,?4)",params![turn.attempt_id,turn.scan_id,owner_key.to_string(),serde_json::to_string(turn)?])?;
    Ok(())
}

impl SwarmStore {
    pub fn coordinator_recovery_page(
        &self,
        scan: Option<&str>,
        after: &str,
    ) -> Result<Vec<String>> {
        self.db.with_connection(|db| {
            let mut stmt=db.prepare("SELECT attempt_id FROM swarm_local_coordinators WHERE complete=0 AND (?1 IS NULL OR scan_id=?1) AND attempt_id>?2 ORDER BY attempt_id LIMIT 64")?;
            Ok(stmt.query_map(params![scan,after],|r|r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn coordinator_owner(&self, attempt: &str) -> Result<(LocalTurn, Uuid)> {
        self.db.with_connection(|db| read_owner(db, attempt))
    }

    pub fn complete_coordinator(&self, attempt: &str, owner_key: Uuid, now_ms: i64) -> Result<()> {
        ensure!(now_ms >= 0, "invalid coordinator completion time");
        self.transaction(|tx| {
            let (turn, key) = read_owner(tx, attempt)?;
            ensure!(key == owner_key, "coordinator owner mismatch");
            let complete: bool = tx.query_row(
                "SELECT complete FROM swarm_local_coordinators WHERE attempt_id=?1",
                [attempt],
                |r| r.get(0),
            )?;
            if complete {
                return Ok(());
            }
            let state = read_scan(tx, &turn.scan_id)?;
            ensure!(
                state.attempts.iter().any(|a| a.id == attempt
                    && a.agent_id == turn.agent_id
                    && a.generation == turn.generation
                    && a.state == ExecutionState::Stopped),
                "coordinator turn is still executing"
            );
            // A stopped execution does not prove its transcript was saved. Keep
            // recovery pending if this write fails after checkpoint exhaustion.
            let local = local::read_local(tx, &turn.scan_id)?;
            preserve_transcript(tx, &state, &local, &turn, now_ms)?;
            finalize_terminal_coordinator(tx, &turn, now_ms)?;
            tx.execute(
                "UPDATE swarm_local_coordinators SET complete=1 WHERE attempt_id=?1",
                [attempt],
            )?;
            Ok(())
        })
    }

    /// Only after acquiring the saved owner's exclusive OS lock. A stopped
    /// attempt already has its lifecycle outcome; never stop a later generation.
    pub fn interrupt_coordinator(&self, attempt: &str, owner_key: Uuid, now_ms: i64) -> Result<()> {
        ensure!(now_ms >= 0, "invalid coordinator recovery time");
        self.transaction(|tx| {
            let (turn,key)=read_owner(tx,attempt)?;
            ensure!(key==owner_key,"coordinator owner mismatch");
            let complete:bool=tx.query_row("SELECT complete FROM swarm_local_coordinators WHERE attempt_id=?1",[attempt],|r|r.get(0))?;
            if complete { return Ok(()); }
            let mut state=read_scan(tx,&turn.scan_id)?;
            let original=state.attempts.iter().find(|a|a.id==attempt && a.agent_id==turn.agent_id && a.generation==turn.generation).context("missing coordinator attempt")?;
            let stopped = original.state == ExecutionState::Stopped;
            let mut local=local::read_local(tx,&turn.scan_id)?;
            // Transcript repair also belongs to stopped attempts. Their lifecycle
            // is already stopped, but scan finalization may still be pending.
            preserve_transcript(tx, &state, &local, &turn, now_ms)?;
            if !stopped {
                let active:Vec<_>=state.attempts.iter().filter(|a|a.state!=ExecutionState::Stopped).map(|a|a.id.clone()).collect();
                local::stop_branch(&mut state,&mut local,&turn.agent_id)?;
                local::set_state(&mut state,&turn.agent_id,AgentState::Blocked)?;
                let sequence=next(state.events.last().map_or(0,|event|event.sequence))?;
                state.events.push(LifecycleEvent { id:format!("coordinator-interrupted:{attempt}"),agent_id:turn.agent_id.clone(),sequence,kind:"coordinator-interrupted".into(),payload:json!({"attempt_id":attempt,"generation":turn.generation,"warning":"The coordinator lost host execution ownership. This hierarchy is stopped; partial results are retained. Unfinished effects remain uncertain and require reconciliation before any replacement research."}) });
                write_scan(tx,&mut state)?;
                local::write_local(tx,&turn.scan_id,&local)?;
                for id in active { fence_in_transaction(tx,&turn.scan_id,&id,false)?; }
            }
            finalize_terminal_coordinator(tx, &turn, now_ms)?;
            tx.execute("UPDATE swarm_local_coordinators SET complete=1 WHERE attempt_id=?1",[attempt])?;
            Ok(())
        })
    }
}

/// A terminal leader turn must finish retention bookkeeping before its recovery
/// intent disappears. Historical attempts must not end a replacement generation.
///
/// Subagents can run while the leader's turn ends. A running execution keeps
/// the scan open so its result is reconciled; the scheduler ends the scan once
/// its executions finish. Fenced executions (termination unconfirmed) do not
/// keep it open.
fn finalize_terminal_coordinator(
    tx: &Transaction<'_>,
    turn: &LocalTurn,
    now_ms: i64,
) -> Result<()> {
    let mut state = read_scan(tx, &turn.scan_id)?;
    if !state
        .attempts
        .iter()
        .any(|a| a.agent_id == turn.agent_id && a.generation > turn.generation)
        && !state
            .attempts
            .iter()
            .any(|a| a.state == ExecutionState::Running)
        && state.ready_to_end()?
    {
        end_scan_in_transaction(tx, &mut state, now_ms)?;
    }
    Ok(())
}

fn preserve_transcript(
    tx: &Transaction<'_>,
    state: &ScanState,
    local: &LocalState,
    turn: &LocalTurn,
    now_ms: i64,
) -> Result<()> {
    let committed: Option<i64> = tx.query_row(
        "SELECT committed_turn FROM swarm_local_coordinators WHERE attempt_id=?1",
        [&turn.attempt_id],
        |r| r.get(0),
    )?;
    if let Some(number) = committed {
        ensure!(
            tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_turns WHERE session_id=?1 AND number=?2)",
                params![turn.session_id, number],
                |r| r.get::<_, bool>(0)
            )?,
            "committed coordinator transcript is missing"
        );
        return Ok(());
    }
    // This notice represents interrupted host execution, never provider output.
    // Append to existing history; the attempt binding and recovery outcome commit
    // with this row, so retries cannot duplicate it or overwrite a real answer.
    tx.execute("INSERT INTO sessions(id,workspace_id,title,created_at_ms,last_message_at_ms) VALUES(?1,?2,'Interrupted swarm',?3,?3) ON CONFLICT(id) DO NOTHING", params![turn.session_id, state.config.workspace_id, now_ms])?;
    let workspace: String = tx.query_row(
        "SELECT workspace_id FROM sessions WHERE id=?1",
        [&turn.session_id],
        |r| r.get(0),
    )?;
    ensure!(
        workspace == state.config.workspace_id,
        "coordinator session belongs to another workspace"
    );
    let previous: i64 = tx.query_row(
        "SELECT COALESCE(MAX(number),0) FROM session_turns WHERE session_id=?1",
        [&turn.session_id],
        |r| r.get(0),
    )?;
    let number = previous
        .checked_add(1)
        .context("coordinator transcript number exhausted")?;
    let model = local
        .settings
        .models
        .iter()
        .find(|m| m.profile == turn.profile)
        .context("missing coordinator model")?;
    let exhausted = local
        .controls
        .iter()
        .any(|c| c.agent_id == turn.agent_id && c.failures >= 3)
        && !state
            .attempts
            .iter()
            .any(|a| a.agent_id == turn.agent_id && a.generation > turn.generation);
    let answer = if exhausted {
        "Host recovery: the coordinator stopped after bounded local recovery failed, before its transcript could be saved. The scan state and partial results are retained; unfinished effects remain uncertain."
    } else {
        "Host recovery: the coordinator turn ended before its transcript could be saved. Partial results are retained and unfinished effects remain uncertain."
    };
    tx.execute("INSERT INTO session_turns(session_id,number,provider,method,model,prompt,answer,duration_ms,internal_prompt,agent_role,model_prompt) VALUES(?1,?2,?3,?4,?5,?6,?7,0,?8,?9,?10)",params![turn.session_id,number,model.provider,model.method,model.model,turn.transcript_prompt(),answer,turn.operator_text.is_none(),turn.profile,turn.prompt])?;
    tx.execute(
        "UPDATE sessions SET last_message_at_ms=MAX(last_message_at_ms,?2) WHERE id=?1",
        params![turn.session_id, now_ms],
    )?;
    tx.execute(
        "UPDATE swarm_local_coordinators SET committed_turn=?2 WHERE attempt_id=?1",
        params![turn.attempt_id, number],
    )?;
    Ok(())
}

fn read_owner(db: &Connection, attempt: &str) -> Result<(LocalTurn, Uuid)> {
    let (scan, key, turn): (String, String, String) = db.query_row(
        "SELECT scan_id,owner_key,turn FROM swarm_local_coordinators WHERE attempt_id=?1",
        [attempt],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let turn: LocalTurn = serde_json::from_str(&turn)?;
    ensure!(
        turn.attempt_id == attempt && turn.scan_id == scan && turn.role == Role::Leader,
        "invalid coordinator record"
    );
    let state = read_scan(db, &scan)?;
    let agent = state.agent(&turn.agent_id)?;
    ensure!(
        turn.session_id == scan
            && agent.summary.role == Role::Leader
            && agent.summary.profile.id == turn.profile
            && state.attempts.iter().any(|a| a.id == attempt
                && a.agent_id == turn.agent_id
                && a.generation == turn.generation),
        "coordinator record does not match saved execution"
    );
    Ok((turn, Uuid::parse_str(&key)?))
}
