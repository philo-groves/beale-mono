//! Prepared worker turns and candidate checkpoints. Worker completion records
//! never release execution ownership or complete assignments by themselves.
use super::*;
use beale_contracts::swarm_worker::WorkerBootstrap;

pub(super) const SCHEMA: &str = "CREATE TABLE swarm_worker_runtime (
 attempt_id TEXT PRIMARY KEY, scan_id TEXT NOT NULL REFERENCES swarm_scans(id),
 agent_id TEXT NOT NULL, turn TEXT NOT NULL, bootstrap TEXT NOT NULL,
 revision INTEGER NOT NULL DEFAULT 0, context TEXT NOT NULL DEFAULT '{}', answer TEXT
);";

pub(super) const ACTIVE_TIME_SCHEMA: &str = "ALTER TABLE swarm_worker_runtime ADD COLUMN active_elapsed_ms INTEGER NOT NULL DEFAULT 0 CHECK(active_elapsed_ms>=0);";

pub(super) const ASSIGNMENT_SCHEMA: &str =
    "ALTER TABLE swarm_worker_runtime ADD COLUMN assignment_id TEXT;";

/// Host-owned provenance fixed before execution, independent of later resume.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerProvenance {
    pub identity: beale_contracts::swarm_worker::ExecutionIdentity,
    pub assignment: Option<String>,
}

pub(super) fn provenance_in(db: &Connection, attempt: &str) -> Result<WorkerProvenance> {
    let (bootstrap, assignment): (String, Option<String>) = db.query_row(
        "SELECT bootstrap,assignment_id FROM swarm_worker_runtime WHERE attempt_id=?1",
        [attempt],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let bootstrap: WorkerBootstrap = serde_json::from_str(&bootstrap)?;
    checked(bootstrap.validate())?;
    ensure!(
        bootstrap.identity.attempt_id == attempt,
        "prepared provenance identity mismatch"
    );
    let saved = read_scan(db, &bootstrap.identity.scan_id)?;
    let agent = saved.agent(&bootstrap.identity.agent_id)?;
    ensure!(
        saved.config.workspace_id == bootstrap.identity.workspace_id
            && saved.attempts.iter().any(|a| a.id == attempt
                && a.agent_id == bootstrap.identity.agent_id
                && a.generation == bootstrap.identity.generation),
        "prepared provenance belongs to another execution"
    );
    if agent.summary.role == Role::Leader {
        ensure!(
            assignment.is_none(),
            "leader provenance has a child assignment"
        );
    } else {
        ensure!(
            assignment.as_ref().is_some_and(|id| saved
                .assignments
                .iter()
                .any(|a| &a.id == id && a.child_id == bootstrap.identity.agent_id)),
            "original worker assignment unavailable"
        );
    }
    Ok(WorkerProvenance {
        identity: bootstrap.identity,
        assignment,
    })
}

pub struct WorkerProgress {
    pub turn: LocalTurn,
    pub revision: u64,
    pub active_elapsed_ms: u64,
    pub context: Value,
    pub answer: Option<String>,
}

impl SwarmStore {
    /// Trusted launcher only. A turn can be prepared once; a lost launch must be
    /// fenced and reconciled, never replaced with another concurrent owner.
    pub fn prepare_worker(
        &self,
        turn: &LocalTurn,
        bootstrap: &WorkerBootstrap,
        registration: &WorkerRegistration,
        now_ms: i64,
    ) -> Result<WorkerCredential> {
        checked(bootstrap.validate())?;
        checked(registration.validate())?;
        self.transaction(|tx| {
            let mut state = read_scan(tx, &turn.scan_id)?;
            local::ensure_local_turn(&state, turn)?;
            ensure_worker_branch_open(tx, &turn.scan_id, &turn.agent_id)?;
            let identity = &bootstrap.identity;
            ensure!(identity.scan_id == turn.scan_id && identity.agent_id == turn.agent_id && identity.attempt_id == turn.attempt_id && identity.generation == turn.generation && identity.workspace_id == state.config.workspace_id, "worker bootstrap identity mismatch");
            ensure!(state.ended_at_ms.is_none() && state.attempts.iter().any(|a| a.id == turn.attempt_id && a.expires_at_ms > now_ms) && now_ms >= 0, "worker execution expired");
            let allowed = bootstrap.shell.map_tools(&state.config.enabled_tools);
            ensure!(bootstrap.enabled_tools.iter().all(|name| allowed.contains(name)) && state.config.provider_routes.iter().any(|route| route.id == bootstrap.route_id), "worker bootstrap policy mismatch");
            let has_credential: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM swarm_credentials WHERE attempt_id=?1)", [&turn.attempt_id], |r| r.get(0))?;
            ensure!(!has_credential, "worker turn already has execution credentials");
            // Match the primary assignment selected by local_claim when several
            // unread follow-ups are combined into one turn. Never resolve this
            // again from the current assignment during late evidence recovery.
            let assignment = if turn.role == Role::Leader { None } else {
                Some(state.assignments.iter().rev().find(|a| a.child_id == turn.agent_id)
                    .context("worker has no task provenance")?.id.clone())
            };
            tx.execute("INSERT INTO swarm_worker_runtime(attempt_id,scan_id,agent_id,turn,bootstrap,assignment_id) VALUES(?1,?2,?3,?4,?5,?6)",
                params![turn.attempt_id,turn.scan_id,turn.agent_id,serde_json::to_string(turn)?,serde_json::to_string(bootstrap)?,assignment])?;
            let credential = WorkerCredential(format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()));
            let expires = state.attempts.iter().find(|a| a.id == turn.attempt_id).context("unknown attempt")?.expires_at_ms;
            tx.execute("INSERT INTO swarm_credentials(digest,scan_id,agent_id,attempt_id,generation,audience,expires_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![digest(credential.bearer()),turn.scan_id,turn.agent_id,turn.attempt_id,i64::try_from(turn.generation)?,WORKER_AUDIENCE,expires])?;
            for environment in state.environments.iter_mut().filter(|e| e.agent_id == turn.agent_id) {
                ensure!(environment.deletion == DeletionState::Retained && environment.active_references > 0, "worker environment is not protected");
                environment.termination_confirmed = false;
                environment.generation = next(environment.generation)?;
            }
            write_scan(tx, &mut state)?;
            Ok(credential)
        })
    }

    /// Check durable launch ownership without decoding candidate state. A corrupt
    /// checkpoint or failed database read is never proof that no worker exists.
    pub fn worker_prepared(&self, attempt: &str) -> Result<bool> {
        self.db.with_connection(|db| {
            Ok(db.query_row(
                "SELECT EXISTS(SELECT 1 FROM swarm_worker_runtime WHERE attempt_id=?1)",
                [attempt],
                |r| r.get(0),
            )?)
        })
    }

    pub fn worker_provenance(&self, attempt: &str) -> Result<WorkerProvenance> {
        self.db.with_connection(|db| provenance_in(db, attempt))
    }

    pub fn worker_progress(&self, attempt: &str) -> Result<WorkerProgress> {
        self.transaction(|tx| {
            let (turn, revision, context, answer, elapsed): (String, i64, String, Option<String>, i64) = tx.query_row(
                "SELECT turn,revision,context,answer,active_elapsed_ms FROM swarm_worker_runtime WHERE attempt_id=?1", [attempt], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
            Ok(WorkerProgress { active_elapsed_ms: u64::try_from(elapsed)?, turn: serde_json::from_str(&turn)?, revision: u64::try_from(revision)?, context: serde_json::from_str(&context)?, answer })
        })
    }

    pub fn worker_bootstrap(&self, attempt: &str) -> Result<WorkerBootstrap> {
        self.transaction(|tx| {
            let raw: String = tx.query_row(
                "SELECT bootstrap FROM swarm_worker_runtime WHERE attempt_id=?1",
                [attempt],
                |r| r.get(0),
            )?;
            let bootstrap: WorkerBootstrap = serde_json::from_str(&raw)?;
            checked(bootstrap.validate())?;
            Ok(bootstrap)
        })
    }

    pub fn worker_runtime_request(
        &self,
        bearer: &str,
        request: &WorkerRequest,
        now_ms: i64,
    ) -> Result<MutationAcknowledgement> {
        self.transaction(|tx| {
            authorize(tx, bearer, WORKER_AUDIENCE, request, now_ms)?;
            let content = serde_json::to_value(&request.operation)?;
            if let Some(result) = receipt(tx, &request.scan_id, &request.agent_id, &request.operation_id, &content)? {
                return Ok(MutationAcknowledgement { version: VERSION, operation_id: request.operation_id.clone(), result });
            }
            let (bootstrap, revision, context, answer, elapsed): (String, i64, String, Option<String>, i64) = tx.query_row(
                "SELECT bootstrap,revision,context,answer,active_elapsed_ms FROM swarm_worker_runtime WHERE attempt_id=?1 AND scan_id=?2 AND agent_id=?3",
                params![request.attempt_id,request.scan_id,request.agent_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
            let result = match &request.operation {
                WorkerOperation::Bootstrap {} => serde_json::from_str(&bootstrap)?,
                WorkerOperation::Checkpoint { revision: incoming, active_elapsed_ms, context: value } => {
                    ensure!(answer.is_none(), "worker already submitted its final checkpoint");
                    let incoming = i64::try_from(*incoming)?;
                    let active_elapsed_ms = i64::try_from(*active_elapsed_ms)?;
                    ensure!(active_elapsed_ms >= elapsed, "worker active duration regressed");
                    ensure!(incoming >= revision, "worker checkpoint regressed");
                    if incoming == revision { ensure!(serde_json::from_str::<Value>(&context)? == *value && active_elapsed_ms == elapsed, "worker checkpoint revision changed"); }
                    else { tx.execute("UPDATE swarm_worker_runtime SET revision=?2,context=?3,active_elapsed_ms=?4 WHERE attempt_id=?1", params![request.attempt_id,incoming,serde_json::to_string(value)?,active_elapsed_ms])?; }
                    json!({"revision":incoming})
                }
                WorkerOperation::Finish { revision: incoming, answer: value } => {
                    ensure!(u64::try_from(revision)? == *incoming && *incoming > 0, "worker finish has no matching durable checkpoint");
                    if let Some(answer) = answer { ensure!(answer == *value, "worker final answer changed"); }
                    else { tx.execute("UPDATE swarm_worker_runtime SET answer=?2 WHERE attempt_id=?1", params![request.attempt_id,value])?; }
                    json!({"recorded":true,"termination_confirmed":false})
                }
                _ => anyhow::bail!("not a worker runtime operation"),
            };
            acknowledge(tx, &request.scan_id, &request.agent_id, &request.operation_id, &content, &result)?;
            Ok(MutationAcknowledgement { version: VERSION, operation_id: request.operation_id.clone(), result })
        })
    }
}
