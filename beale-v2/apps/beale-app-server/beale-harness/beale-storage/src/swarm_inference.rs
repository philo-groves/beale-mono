//! Durable ownership of asynchronous provider calls. Network work never holds a
//! database transaction. A pending claim is not permission to dispatch again.
use super::*;
use anyhow::{Result, ensure};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions, TryLockError},
    path::PathBuf,
};
use uuid::Uuid;

pub(super) const SCHEMA: &str = "
CREATE TABLE swarm_inference (
 scan_id TEXT NOT NULL REFERENCES swarm_scans(id), agent_id TEXT NOT NULL,
 operation_id TEXT NOT NULL, request TEXT NOT NULL, claim TEXT NOT NULL,
 result TEXT, PRIMARY KEY(scan_id,agent_id,operation_id)
);
";

pub(super) const ACTIVITY_SCHEMA: &str = "
CREATE TABLE swarm_inference_activity (
 sequence INTEGER PRIMARY KEY AUTOINCREMENT,
 scan_id TEXT NOT NULL, agent_id TEXT NOT NULL, operation_id TEXT NOT NULL,
 started_at_ms INTEGER NOT NULL CHECK(started_at_ms>=0),
 completed_at_ms INTEGER CHECK(completed_at_ms>=0),
 after_tool_count INTEGER NOT NULL CHECK(after_tool_count>=0),
 interrupted INTEGER NOT NULL DEFAULT 0 CHECK(interrupted IN (0,1)),
 UNIQUE(scan_id,agent_id,operation_id),
 FOREIGN KEY(scan_id,agent_id,operation_id) REFERENCES swarm_inference(scan_id,agent_id,operation_id)
);";

pub enum InferenceClaim {
    Dispatch(String),
    Replay(MutationAcknowledgement),
    Pending,
}

/// Exclusive provider-dispatch ownership. Dropping this value (including task
/// cancellation or process exit) releases the OS lock. Lock files are retained:
/// unlinking a live lock could let another process lock a different inode.
pub struct InferenceOwner {
    store: SwarmStore,
    request: WorkerRequest,
    _lock: File,
}

impl SwarmStore {
    pub fn try_own_inference(
        &self,
        bearer: &str,
        request: &WorkerRequest,
        now_ms: i64,
    ) -> Result<Option<InferenceOwner>> {
        ensure!(
            matches!(request.operation, WorkerOperation::Infer { .. }),
            "not an inference request"
        );
        self.transaction(|tx| {
            authorize(tx, bearer, WORKER_AUDIENCE, request, now_ms)?;
            Ok(())
        })?;
        let root = self.db.with_connection(|db| {
            let path = db
                .path()
                .context("inference requires durable host storage")?;
            Ok(PathBuf::from(path)
                .parent()
                .context("host database parent missing")?
                .join("swarm-inference-locks"))
        })?;
        std::fs::create_dir_all(&root)?;
        ensure!(
            !std::fs::symlink_metadata(&root)?.file_type().is_symlink(),
            "inference lock directory cannot be a link"
        );
        // A worker consumes one provider response at a time. Serialize that
        // agent's inference without blocking unrelated agents or scans.
        let key = crate::swarm_files::hash(
            serde_json::to_string(&(&request.scan_id, &request.agent_id))?.as_bytes(),
        );
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options.open(root.join(key))?;
        match lock.try_lock() {
            Ok(()) => Ok(Some(InferenceOwner {
                store: self.clone(),
                request: request.clone(),
                _lock: lock,
            })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(error.into()),
        }
    }
}

impl InferenceOwner {
    pub fn claim(&self, bearer: &str, now_ms: i64) -> Result<InferenceClaim> {
        let request = &self.request;
        ensure!(
            matches!(request.operation, WorkerOperation::Infer { .. }),
            "not an inference request"
        );
        self.store.transaction(|tx| {
            authorize(tx, bearer, WORKER_AUDIENCE, request, now_ms)?;
            if let Some(result) = receipt(tx, &request.scan_id, &request.agent_id, &request.operation_id, &serde_json::to_value(&request.operation)?)? {
                return Ok(InferenceClaim::Replay(MutationAcknowledgement { version: VERSION, operation_id: request.operation_id.clone(), result }));
            }
            let saved: Option<String> = tx.query_row("SELECT request FROM swarm_inference WHERE scan_id=?1 AND agent_id=?2 AND operation_id=?3",
                params![request.scan_id,request.agent_id,request.operation_id], |r| r.get(0)).optional()?;
            if let Some(saved) = saved {
                ensure!(serde_json::from_str::<WorkerRequest>(&saved)? == *request, "inference request identity changed");
                return Ok(InferenceClaim::Pending);
            }
            let claim = Uuid::new_v4().to_string();
            tx.execute("INSERT INTO swarm_inference VALUES(?1,?2,?3,?4,?5,NULL)",
                params![request.scan_id,request.agent_id,request.operation_id,serde_json::to_string(request)?,claim])?;
            let state = read_scan(tx, &request.scan_id)?;
            let tool_count = state.events.iter().filter(|event|
                event.agent_id == request.agent_id && event.kind == "worker-event"
                && event.payload["attempt_id"] == request.attempt_id
                && event.payload["event"]["kind"] == "tool_outcome").count();
            tx.execute("INSERT INTO swarm_inference_activity(scan_id,agent_id,operation_id,started_at_ms,after_tool_count) VALUES(?1,?2,?3,?4,?5)",
                params![request.scan_id,request.agent_id,request.operation_id,now_ms,i64::try_from(tool_count)?])?;
            Ok(InferenceClaim::Dispatch(claim))
        })
    }

    /// Called only by the host that owns the claim. Current execution authority
    /// is required for acknowledgement, even if it was valid before awaiting IO.
    pub fn complete(
        &self,
        bearer: &str,
        claim: &str,
        result: Value,
        now_ms: i64,
    ) -> Result<MutationAcknowledgement> {
        let request = &self.request;
        ensure!(
            matches!(request.operation, WorkerOperation::Infer { .. }),
            "not an inference request"
        );
        ensure!(
            serde_json::to_vec(&result)?.len() <= MAX_DOCUMENT_BYTES / 2,
            "inference result too large"
        );
        serde_json::from_value::<beale_contracts::swarm_worker::InferenceResult>(result.clone())?
            .validate()
            .map_err(anyhow::Error::msg)?;
        self.store.transaction(|tx| {
            authorize(tx, bearer, WORKER_AUDIENCE, request, now_ms)?;
            let (saved, owner, previous): (String, String, Option<String>) = tx.query_row(
                "SELECT request,claim,result FROM swarm_inference WHERE scan_id=?1 AND agent_id=?2 AND operation_id=?3",
                params![request.scan_id,request.agent_id,request.operation_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
            ensure!(owner == claim && serde_json::from_str::<WorkerRequest>(&saved)? == *request, "inference claim identity changed");
            if let Some(previous) = previous {
                ensure!(serde_json::from_str::<Value>(&previous)? == result, "inference result changed");
            } else {
                ensure!(tx.execute("UPDATE swarm_inference_activity SET completed_at_ms=?4 WHERE scan_id=?1 AND agent_id=?2 AND operation_id=?3 AND completed_at_ms IS NULL",
                    params![request.scan_id,request.agent_id,request.operation_id,now_ms])? == 1, "inference activity missing or already completed");
            }
            tx.execute("UPDATE swarm_inference SET result=?4 WHERE scan_id=?1 AND agent_id=?2 AND operation_id=?3",
                params![request.scan_id,request.agent_id,request.operation_id,serde_json::to_string(&result)?])?;
            Ok(())
        })?;
        self.store.transaction(|tx| {
            authorize(tx, bearer, WORKER_AUDIENCE, request, now_ms)?;
            let content = serde_json::to_value(&request.operation)?;
            if let Some(saved) = receipt(
                tx,
                &request.scan_id,
                &request.agent_id,
                &request.operation_id,
                &content,
            )? {
                ensure!(saved == result, "inference receipt changed");
            } else {
                acknowledge(
                    tx,
                    &request.scan_id,
                    &request.agent_id,
                    &request.operation_id,
                    &content,
                    &result,
                )?;
            }
            Ok(MutationAcknowledgement {
                version: VERSION,
                operation_id: request.operation_id.clone(),
                result,
            })
        })
    }
    /// Only reachable while holding this agent's OS dispatch lock. Never
    /// redispatch the old operation: acknowledge its stored output, or resolve
    /// the abandoned response as unavailable and preserve an explicit warning.
    pub fn recover(&self, bearer: &str, now_ms: i64) -> Result<MutationAcknowledgement> {
        let request = &self.request;
        let (claim, result) = self.store.transaction(|tx| {
            authorize(tx, bearer, WORKER_AUDIENCE, request, now_ms)?;
            let (saved, claim, result): (String, String, Option<String>) = tx.query_row(
                "SELECT request,claim,result FROM swarm_inference WHERE scan_id=?1 AND agent_id=?2 AND operation_id=?3",
                params![request.scan_id,request.agent_id,request.operation_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
            ensure!(serde_json::from_str::<WorkerRequest>(&saved)? == *request, "inference recovery identity changed");
            if let Some(result) = result {
                return Ok((claim, serde_json::from_str::<Value>(&result)?));
            }
            let result = json!({"status":"unavailable","retryable":true});
            let mut state = read_scan(tx, &request.scan_id)?;
            let id = format!("inference-recovery-{}", crate::swarm_files::hash(serde_json::to_string(request)?.as_bytes()));
            let sequence = next(u64::try_from(state.events.len())?)?;
            state.events.push(LifecycleEvent {
                id, agent_id: request.agent_id.clone(), sequence, kind: "inference-interrupted".into(),
                payload: json!({"attempt_id":request.attempt_id,"generation":request.generation,"operation_id":request.operation_id,
                    "warning":"Provider response was interrupted before durable completion. Its usage may be incomplete; the original request will not be dispatched again."}),
            });
            write_scan(tx, &mut state)?;
            ensure!(tx.execute("UPDATE swarm_inference_activity SET completed_at_ms=?4,interrupted=1 WHERE scan_id=?1 AND agent_id=?2 AND operation_id=?3 AND completed_at_ms IS NULL",
                params![request.scan_id,request.agent_id,request.operation_id,now_ms])? == 1, "inference activity missing or already completed");
            tx.execute("UPDATE swarm_inference SET result=?4 WHERE scan_id=?1 AND agent_id=?2 AND operation_id=?3",
                params![request.scan_id,request.agent_id,request.operation_id,serde_json::to_string(&result)?])?;
            Ok((claim, result))
        })?;
        self.complete(bearer, &claim, result, now_ms)
    }
}
