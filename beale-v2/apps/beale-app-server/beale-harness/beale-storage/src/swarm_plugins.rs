//! Durable ownership of plugin side effects. A lost response is uncertainty,
//! not a license to dispatch again. No plugin or network IO runs in a SQL lock.
use super::*;
use beale_contracts::{
    swarm_plugins::{Outcome, Reply},
    swarm_worker::ExecutionIdentity,
};
use std::{
    fs::{File, OpenOptions, TryLockError},
    path::PathBuf,
};

pub(super) const SCHEMA: &str = "
CREATE TABLE swarm_plugin_calls (
 scan_id TEXT NOT NULL REFERENCES swarm_scans(id), agent_id TEXT NOT NULL,
 operation_id TEXT NOT NULL, request TEXT NOT NULL, claim TEXT NOT NULL,
 journal_id TEXT NOT NULL, result TEXT,
 PRIMARY KEY(scan_id,agent_id,operation_id), UNIQUE(scan_id,journal_id)
);
";
pub enum PluginClaim {
    Dispatch(String),
    Replay(MutationAcknowledgement),
    Pending,
}

/// OS ownership is retained across asynchronous work and released on process
/// exit or task cancellation. The private claim authorizes candidate capture,
/// but active worker authority is separately required for canonical acceptance.
pub struct PluginOwner {
    store: SwarmStore,
    request: WorkerRequest,
    _lock: File,
}

fn identity(request: &WorkerRequest) -> ExecutionIdentity {
    ExecutionIdentity {
        workspace_id: request.workspace_id.clone(),
        scan_id: request.scan_id.clone(),
        agent_id: request.agent_id.clone(),
        attempt_id: request.attempt_id.clone(),
        generation: request.generation,
    }
}
impl SwarmStore {
    pub fn try_own_plugin(
        &self,
        bearer: &str,
        request: &WorkerRequest,
        now_ms: i64,
    ) -> Result<Option<PluginOwner>> {
        ensure!(
            matches!(request.operation, WorkerOperation::Plugin { .. }),
            "not a plugin request"
        );
        self.transaction(|tx| {
            authorize(tx, bearer, WORKER_AUDIENCE, request, now_ms)?;
            Ok(())
        })?;
        let root = self.db.with_connection(|db| {
            Ok(
                PathBuf::from(db.path().context("plugin calls require durable storage")?)
                    .parent()
                    .context("host database parent missing")?
                    .join("swarm-plugin-locks"),
            )
        })?;
        std::fs::create_dir_all(&root)?;
        ensure!(
            !std::fs::symlink_metadata(&root)?.file_type().is_symlink(),
            "plugin lock directory cannot be a link"
        );
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
            Ok(()) => Ok(Some(PluginOwner {
                store: self.clone(),
                request: request.clone(),
                _lock: lock,
            })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(error.into()),
        }
    }
}
impl PluginOwner {
    /// Inspect recovery before preflight/review. Previously dispatched work is
    /// recovered even if plugin configuration has since changed.
    pub fn prior(&self, bearer: &str, now_ms: i64) -> Result<Option<PluginClaim>> {
        let r = &self.request;
        self.store.transaction(|tx| {
            authorize(tx,bearer,WORKER_AUDIENCE,r,now_ms)?;
            if let Some(result)=receipt(tx,&r.scan_id,&r.agent_id,&r.operation_id,&serde_json::to_value(r)?)? {
                return Ok(Some(PluginClaim::Replay(MutationAcknowledgement {version:VERSION,operation_id:r.operation_id.clone(),result})));
            }
            let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM swarm_plugin_calls WHERE scan_id=?1 AND agent_id=?2 AND operation_id=?3)",params![r.scan_id,r.agent_id,r.operation_id],|row|row.get(0))?;
            Ok(exists.then_some(PluginClaim::Pending))
        })
    }

    pub fn staged(&self) -> Result<Option<Reply>> {
        let r = &self.request;
        self.store.transaction(|tx| {
            let value:Option<String>=tx.query_row("SELECT result FROM swarm_plugin_calls WHERE scan_id=?1 AND agent_id=?2 AND operation_id=?3",params![r.scan_id,r.agent_id,r.operation_id],|row|row.get(0))?;
            value.map(|value| {let reply:Reply=serde_json::from_str(&value)?;reply.validate_for(&identity(r)).map_err(anyhow::Error::msg)?;Ok(reply)}).transpose()
        })
    }

    /// Restore a response from the host's immutable capture after a result-table
    /// write failed. This never creates a claim or authorizes another dispatch.
    pub fn restore_captured(&self, outcome: Outcome) -> Result<()> {
        let r = &self.request;
        let claim=self.store.transaction(|tx|Ok(tx.query_row("SELECT claim FROM swarm_plugin_calls WHERE scan_id=?1 AND agent_id=?2 AND operation_id=?3",params![r.scan_id,r.agent_id,r.operation_id],|row|row.get::<_,String>(0))?))?;
        self.stage_result(&claim, outcome)
    }

    /// Atomic feedback when preflight or review rejects before any side effect.
    /// An existing effect cannot be overwritten by a later validation failure.
    pub fn reject_without_dispatch(
        &self,
        bearer: &str,
        message: &str,
        now_ms: i64,
    ) -> Result<MutationAcknowledgement> {
        let r = &self.request;
        let reply = Reply {
            identity: identity(r),
            outcome: Outcome::Completed {
                output: message.into(),
                is_error: true,
                artifact_refs: vec![],
            },
        };
        reply
            .validate_for(&identity(r))
            .map_err(anyhow::Error::msg)?;
        self.store.transaction(|tx| {
            authorize(tx,bearer,WORKER_AUDIENCE,r,now_ms)?;
            let content=serde_json::to_value(r)?;
            if let Some(result)=receipt(tx,&r.scan_id,&r.agent_id,&r.operation_id,&content)? {return Ok(MutationAcknowledgement {version:VERSION,operation_id:r.operation_id.clone(),result});}
            let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM swarm_plugin_calls WHERE scan_id=?1 AND agent_id=?2 AND operation_id=?3)",params![r.scan_id,r.agent_id,r.operation_id],|row|row.get(0))?;
            ensure!(!exists,"plugin effect already claimed");
            let result=serde_json::to_value(reply)?;
            acknowledge(tx,&r.scan_id,&r.agent_id,&r.operation_id,&content,&result)?;
            Ok(MutationAcknowledgement {version:VERSION,operation_id:r.operation_id.clone(),result})
        })
    }
    /// The caller must finish availability preflight and saved-policy review
    /// before this claim. Dispatch exactly once only for `Dispatch`; all other
    /// cases recover an existing effect without running the plugin again.
    pub fn claim(&self, bearer: &str, now_ms: i64) -> Result<PluginClaim> {
        let r = &self.request;
        self.store.transaction(|tx| {
            authorize(tx,bearer,WORKER_AUDIENCE,r,now_ms)?;
            let content=serde_json::to_value(r)?;
            if let Some(result)=receipt(tx,&r.scan_id,&r.agent_id,&r.operation_id,&content)? {
                return Ok(PluginClaim::Replay(MutationAcknowledgement {version:VERSION,operation_id:r.operation_id.clone(),result}));
            }
            let prior:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM swarm_plugin_calls WHERE scan_id=?1 AND agent_id=?2 AND operation_id=?3)",params![r.scan_id,r.agent_id,r.operation_id],|row|row.get(0))?;
            if prior {return Ok(PluginClaim::Pending);}
            let mut state=read_scan(tx,&r.scan_id)?;
            // A new tool ID must not evade a previously uncertain side effect.
            let journals=tx.prepare("SELECT journal_id FROM swarm_plugin_calls WHERE scan_id=?1 AND agent_id=?2")?.query_map(params![r.scan_id,r.agent_id],|row|row.get::<_,String>(0))?.collect::<Result<HashSet<_>,_>>()?;
            ensure!(!state.operations.iter().any(|op| journals.contains(&op.id) && matches!(op.state,ToolOperationState::Running | ToolOperationState::OutcomeUnknown)), "earlier plugin effect requires reconciliation");
            let claim=Uuid::new_v4().to_string();
            let journal_id=format!("plugin-{}",crate::swarm_files::hash(serde_json::to_string(r)?.as_bytes()));
            ensure!(!state.operations.iter().any(|op|op.id==journal_id),"plugin journal identity conflict");
            state.operations.push(ToolOperation {id:journal_id.clone(),agent_id:r.agent_id.clone(),attempt_id:r.attempt_id.clone(),request:content,state:ToolOperationState::Running,result:Value::Null,revision:1});
            tx.execute("INSERT INTO swarm_plugin_calls VALUES(?1,?2,?3,?4,?5,?6,NULL)",params![r.scan_id,r.agent_id,r.operation_id,serde_json::to_string(r)?,claim,journal_id])?;
            write_scan(tx,&mut state)?;
            Ok(PluginClaim::Dispatch(claim))
        })
    }

    /// Capture a host-redacted result even after the worker is fenced. This
    /// writes candidate data only; it cannot acknowledge or alter a newer turn.
    pub fn stage_result(&self, claim: &str, outcome: Outcome) -> Result<()> {
        outcome.validate().map_err(anyhow::Error::msg)?;
        let r = &self.request;
        self.store.transaction(|tx| {
            let (saved,owner,previous):(String,String,Option<String>)=tx.query_row("SELECT request,claim,result FROM swarm_plugin_calls WHERE scan_id=?1 AND agent_id=?2 AND operation_id=?3",params![r.scan_id,r.agent_id,r.operation_id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?;
            ensure!(owner==claim && serde_json::from_str::<WorkerRequest>(&saved)?==*r,"plugin claim identity changed");
            let result=serde_json::to_string(&Reply {identity:identity(r),outcome})?;
            if let Some(previous)=previous {ensure!(previous==result,"plugin result changed");return Ok(());}
            tx.execute("UPDATE swarm_plugin_calls SET result=?4 WHERE scan_id=?1 AND agent_id=?2 AND operation_id=?3",params![r.scan_id,r.agent_id,r.operation_id,result])?;
            Ok(())
        })
    }

    /// Reuse a staged response, or durably expose an abandoned call as unknown.
    /// No path through this function dispatches the original effect again.
    pub fn recover(&self, bearer: &str, now_ms: i64) -> Result<MutationAcknowledgement> {
        let r = &self.request;
        self.store.transaction(|tx| {
            authorize(tx,bearer,WORKER_AUDIENCE,r,now_ms)?;
            let content=serde_json::to_value(r)?;
            if let Some(result)=receipt(tx,&r.scan_id,&r.agent_id,&r.operation_id,&content)? {
                return Ok(MutationAcknowledgement {version:VERSION,operation_id:r.operation_id.clone(),result});
            }
            let (journal_id,saved):(String,Option<String>)=tx.query_row("SELECT journal_id,result FROM swarm_plugin_calls WHERE scan_id=?1 AND agent_id=?2 AND operation_id=?3",params![r.scan_id,r.agent_id,r.operation_id],|row|Ok((row.get(0)?,row.get(1)?)))?;
            let reply=match saved {Some(saved)=>serde_json::from_str::<Reply>(&saved)?,None=>Reply {identity:identity(r),outcome:Outcome::OutcomeUnknown {}}};
            reply.validate_for(&identity(r)).map_err(anyhow::Error::msg)?;
            let result=serde_json::to_value(&reply)?;
            let mut state=read_scan(tx,&r.scan_id)?;
            let operation=state.operations.iter_mut().find(|op|op.id==journal_id).context("plugin journal missing")?;
            ensure!(operation.state==ToolOperationState::Running && operation.agent_id==r.agent_id && operation.attempt_id==r.attempt_id && operation.request==content,"plugin journal changed");
            operation.revision=next(operation.revision)?;
            match &reply.outcome {
                Outcome::Completed {artifact_refs,..}=>{
                    crate::swarm_files::validate_record_artifacts(tx,&r.scan_id,&r.workspace_id,&json!({"artifact_refs":artifact_refs}))?;
                    operation.state=ToolOperationState::Completed;operation.result=result.clone();
                },
                Outcome::OutcomeUnknown {}=>{
                    operation.state=ToolOperationState::OutcomeUnknown;
                    let sequence=next(u64::try_from(state.events.len())?)?;
                    state.events.push(LifecycleEvent {id:format!("plugin-unknown-{}",crate::swarm_files::hash(journal_id.as_bytes())),agent_id:r.agent_id.clone(),sequence,kind:"plugin-outcome-unknown".into(),payload:json!({"attempt_id":r.attempt_id,"generation":r.generation,"operation_id":r.operation_id,"journal_id":journal_id,"warning":"Plugin outcome is unknown. Do not repeat this effect until it has been reconciled."})});
                },
            }
            write_scan(tx,&mut state)?;
            tx.execute("UPDATE swarm_plugin_calls SET result=?4 WHERE scan_id=?1 AND agent_id=?2 AND operation_id=?3",params![r.scan_id,r.agent_id,r.operation_id,serde_json::to_string(&reply)?])?;
            acknowledge(tx,&r.scan_id,&r.agent_id,&r.operation_id,&content,&result)?;
            Ok(MutationAcknowledgement {version:VERSION,operation_id:r.operation_id.clone(),result})
        })
    }
}
