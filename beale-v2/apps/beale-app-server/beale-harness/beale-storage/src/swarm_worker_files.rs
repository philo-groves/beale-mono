//! Scoped transfer mutations share the worker receipt transaction. Files may
//! exist before commit, but remain candidates until acceptance is durable.
use super::*;
use crate::swarm_files::{ResearchFiles, hash};

impl SwarmStore {
    pub fn worker_termination_confirmed(
        &self,
        identity: &beale_contracts::swarm_worker::ExecutionIdentity,
    ) -> Result<bool> {
        self.db
            .with_connection(|db| termination_confirmed(db, identity))
    }
    /// Trusted host recovery only, after independently confirmed termination.
    /// This is deliberately absent from the worker gateway. It can accept bytes
    /// and candidate provenance, never a tool result or assignment completion.
    pub fn recover_worker_artifact_request(
        &self,
        files: &ResearchFiles,
        request: &WorkerRequest,
    ) -> Result<MutationAcknowledgement> {
        checked(request.validate())?;
        ensure!(
            matches!(
                request.operation,
                WorkerOperation::ArtifactBegin { .. }
                    | WorkerOperation::ArtifactChunk { .. }
                    | WorkerOperation::ArtifactSeal { .. }
            ),
            "host artifact recovery accepts publication operations only"
        );
        let result = self.transaction(|tx| {
            let provenance = worker_runtime::provenance_in(tx, &request.attempt_id)?;
            ensure!(provenance.identity.matches(request), "recovery provenance mismatch");
            let mut state = read_scan(tx, &request.scan_id)?;
            ensure!(termination_confirmed(tx, &provenance.identity)?, "artifact recovery requires confirmed termination");
            let actor = format!("artifact-recovery:{}",request.agent_id);
            let content = json!({"identity":provenance.identity,"operation":request.operation});
            if let Some(saved) = receipt(tx,&request.scan_id,&actor,&request.operation_id,&content)? { return Ok(saved); }
            let already_accepted = if let WorkerOperation::ArtifactSeal { transfer_id } = &request.operation {
                tx.query_row("SELECT accepted FROM swarm_file_transfers WHERE id=?1",[transfer_id],|r|r.get::<_,bool>(0)).optional()?.unwrap_or(false)
            } else { false };
            let result = apply_artifact(tx,files,request)?;
            if let WorkerOperation::ArtifactSeal { transfer_id } = &request.operation
                && !already_accepted {
                let id = format!("recovery-artifact:{}",hash(serde_json::to_string(&(&provenance.identity,transfer_id))?.as_bytes()));
                if !state.events.iter().any(|event|event.id == id) {
                    let sequence = next(state.events.last().map_or(0,|event|event.sequence))?;
                    state.events.push(LifecycleEvent { id:id.clone(),agent_id:request.agent_id.clone(),sequence,kind:"worker-recovery-artifact".into(),
                        payload:json!({"artifact":transfer_id,"assignment":provenance.assignment,"attempt_id":request.attempt_id,"generation":request.generation,"candidate":true}) });
                    local::recovered_artifact_notice(tx,&mut state,&request.agent_id,&id,&format!("Recovered candidate artifact {transfer_id} from terminated attempt {} (generation {}). This preserves evidence only; it does not complete the current assignment.",request.attempt_id,request.generation))?;
                    write_scan(tx,&mut state)?;
                }
            }
            acknowledge(tx,&request.scan_id,&actor,&request.operation_id,&content,&result)?;
            Ok(result)
        })?;
        if let WorkerOperation::ArtifactSeal { transfer_id } = &request.operation {
            files.artifact(&request.workspace_id, transfer_id)?;
            files.cleanup_transfer(transfer_id);
        }
        Ok(MutationAcknowledgement {
            version: VERSION,
            operation_id: request.operation_id.clone(),
            result,
        })
    }

    pub fn worker_artifact_request(
        &self,
        files: &ResearchFiles,
        bearer: &str,
        request: &WorkerRequest,
        now_ms: i64,
    ) -> Result<MutationAcknowledgement> {
        let acknowledgement =
            self.dispatch_mutation(bearer, WORKER_AUDIENCE, request, now_ms, |tx, _| {
                apply_artifact(tx, files, request)
            })?;
        if let WorkerOperation::ArtifactSeal { transfer_id } = &request.operation {
            files.artifact(&request.workspace_id, transfer_id)?;
            // Never remove the transfer spool if either acceptance or its outer
            // acknowledgement rolled back. Retry still needs those source bytes.
            files.cleanup_transfer(transfer_id);
        }
        Ok(acknowledgement)
    }
}

fn termination_confirmed(
    db: &Connection,
    identity: &beale_contracts::swarm_worker::ExecutionIdentity,
) -> Result<bool> {
    let state = read_scan(db, &identity.scan_id)?;
    ensure!(
        state.config.workspace_id == identity.workspace_id,
        "termination workspace mismatch"
    );
    let attempt = state
        .attempts
        .iter()
        .find(|a| {
            a.id == identity.attempt_id
                && a.agent_id == identity.agent_id
                && a.generation == identity.generation
        })
        .context("termination identity mismatch")?;
    if attempt.state == ExecutionState::Stopped {
        return Ok(true);
    }
    let peer: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM swarm_peer_workers WHERE attempt=?1)",
        [&identity.attempt_id],
        |r| r.get(0),
    )?;
    if !peer {
        return Ok(false);
    }
    let record = peer_workers::read(db, &identity.attempt_id)?;
    Ok(record.target.identity == *identity && record.terminated())
}

fn apply_artifact(
    tx: &Transaction<'_>,
    files: &ResearchFiles,
    request: &WorkerRequest,
) -> Result<Value> {
    let owned = |id: &str| -> Result<()> {
        let artifact = files.transfer_in(tx, id)?;
        let provenance = worker_runtime::provenance_in(tx, &request.attempt_id)?;
        ensure!(
            provenance.identity.matches(request) && artifact.assignment == provenance.assignment,
            "transfer assignment provenance mismatch"
        );
        ensure!(
            artifact.workspace == request.workspace_id
                && artifact.scan == request.scan_id
                && artifact.producer == request.agent_id
                && artifact.attempt == request.attempt_id
                && artifact.generation == request.generation,
            "transfer belongs to another execution"
        );
        Ok(())
    };
    match &request.operation {
        WorkerOperation::ArtifactBegin { artifact } => {
            let provenance = worker_runtime::provenance_in(tx, &request.attempt_id)?;
            ensure!(
                provenance.identity.matches(request),
                "artifact provenance mismatch"
            );
            let assignment = provenance.assignment;
            let artifact = beale_contracts::swarm_files::Artifact {
                id: artifact.id.clone(),
                operation: request.operation_id.clone(),
                workspace: request.workspace_id.clone(),
                scan: request.scan_id.clone(),
                producer: request.agent_id.clone(),
                assignment,
                attempt: request.attempt_id.clone(),
                generation: request.generation,
                revision: artifact.revision,
                source_path: artifact.source_path.clone(),
                sha256: artifact.sha256.clone(),
                size: artifact.size,
                executable: artifact.executable,
                previous: artifact.previous.clone(),
            };
            let next_offset = files.begin_transfer_in(tx, &artifact)?;
            Ok(json!({"next_offset":next_offset}))
        }
        WorkerOperation::ArtifactChunk {
            transfer_id,
            offset,
            bytes,
            sha256,
        } => {
            owned(transfer_id)?;
            ensure!(hash(bytes) == *sha256, "chunk integrity mismatch");
            let next_offset = files.write_chunk_in(tx, transfer_id, *offset, bytes)?;
            Ok(json!({"next_offset":next_offset}))
        }
        WorkerOperation::ArtifactSeal { transfer_id } => {
            owned(transfer_id)?;
            Ok(serde_json::to_value(files.seal_in(tx, transfer_id)?)?)
        }
        WorkerOperation::ArtifactRead {
            artifact_id,
            offset,
            length,
        } => Ok(serde_json::to_value(files.artifact_chunk_in(
            tx,
            &request.workspace_id,
            artifact_id,
            *offset,
            *length,
        )?)?),
        _ => anyhow::bail!("not a worker artifact operation"),
    }
}
