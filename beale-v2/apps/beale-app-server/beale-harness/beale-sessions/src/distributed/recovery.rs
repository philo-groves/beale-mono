//! Host-mediated acceptance of immutable publication captures after termination.
//! No worker credential is restored and no original tool outcome is rewritten.
use crate::SessionState;
use anyhow::{Result, ensure};
use beale_contracts::{
    swarm_files::Artifact,
    swarm_worker::{ExecutionIdentity, TransferProgress},
    swarms::*,
};
use beale_storage::{
    swarm_files::{captured_worker_publications, hash},
    swarm_state::SwarmStore,
    worker_spool::WorkerSpool,
};
use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
};

pub(super) fn publications(
    state: &SessionState,
    identity: &ExecutionIdentity,
    recovery: &Path,
) -> Result<()> {
    publications_at(&state.db, state.workspaces.root(), identity, recovery)
}
pub(super) fn publications_at(
    db: &beale_storage::HostDatabase,
    root: &Path,
    identity: &ExecutionIdentity,
    recovery: &Path,
) -> Result<()> {
    let store = SwarmStore::open(db.clone())?;
    ensure!(
        store.worker_provenance(&identity.attempt_id)?.identity == *identity,
        "recovery execution mismatch"
    );
    ensure!(
        store.worker_termination_confirmed(identity)?,
        "capture recovery requires confirmed termination"
    );
    let mut captures = captured_worker_publications(recovery)?;
    let Some(first) = captures.next() else {
        return Ok(());
    };
    // Acquire the original spool's OS lock and validate its durable identity.
    // A directory of files alone is not execution provenance.
    let _spool = WorkerSpool::open_existing(recovery, identity.clone())?;
    let files = beale_storage::swarm_files::ResearchFiles::open(db.clone(), root)?;
    let request = |id: String, operation: WorkerOperation| WorkerRequest {
        version: VERSION,
        workspace_id: identity.workspace_id.clone(),
        scan_id: identity.scan_id.clone(),
        agent_id: identity.agent_id.clone(),
        attempt_id: identity.attempt_id.clone(),
        generation: identity.generation,
        operation_id: id,
        operation,
    };
    for capture in std::iter::once(first).chain(captures) {
        let capture = capture?;
        let key = capture.key();
        let upload = capture.upload();
        let begin = request(
            format!("artifact-begin:{key}"),
            WorkerOperation::ArtifactBegin {
                artifact: upload.clone(),
            },
        );
        let progress: TransferProgress = serde_json::from_value(
            store
                .recover_worker_artifact_request(&files, &begin)?
                .result,
        )?;
        let mut offset = progress.next_offset;
        ensure!(
            offset <= upload.size,
            "invalid recovered publication offset"
        );
        let mut input = capture.open()?;
        input.seek(SeekFrom::Start(offset))?;
        while offset < upload.size {
            let mut bytes = vec![0; usize::try_from((upload.size - offset).min(192 * 1024))?];
            input.read_exact(&mut bytes)?;
            let next = offset + bytes.len() as u64;
            let chunk = request(
                format!("artifact-chunk:{key}:{offset}"),
                WorkerOperation::ArtifactChunk {
                    transfer_id: upload.id.clone(),
                    offset,
                    sha256: hash(&bytes),
                    bytes,
                },
            );
            let progress: TransferProgress = serde_json::from_value(
                store
                    .recover_worker_artifact_request(&files, &chunk)?
                    .result,
            )?;
            ensure!(
                progress.next_offset == next,
                "recovered publication offset changed"
            );
            offset = next;
        }
        let seal = request(
            format!("artifact-seal:{key}"),
            WorkerOperation::ArtifactSeal {
                transfer_id: upload.id.clone(),
            },
        );
        let accepted: Artifact =
            serde_json::from_value(store.recover_worker_artifact_request(&files, &seal)?.result)?;
        ensure!(
            accepted.id == upload.id
                && accepted.sha256 == upload.sha256
                && accepted.size == upload.size,
            "recovered publication changed"
        );
    }
    Ok(())
}
