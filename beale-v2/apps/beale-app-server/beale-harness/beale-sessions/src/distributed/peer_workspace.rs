//! Scoped initial workspace transfer before a destination releases execution.
use super::{Lease, TransportFailure, WorkerTransport};
use anyhow::{Context, Result, ensure};
use beale_contracts::{swarm_files::*, swarm_host::HostWorkerLaunch, swarm_worker::*, swarms::*};
use beale_storage::swarm_files::SnapshotReceiver;
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

struct Transfer {
    transport: WorkerTransport,
    identity: ExecutionIdentity,
    lease: Mutex<Lease>,
}
impl Transfer {
    fn request(&self, operation_id: String, operation: WorkerOperation) -> WorkerRequest {
        WorkerRequest {
            version: 1,
            operation_id,
            workspace_id: self.identity.workspace_id.clone(),
            scan_id: self.identity.scan_id.clone(),
            agent_id: self.identity.agent_id.clone(),
            attempt_id: self.identity.attempt_id.clone(),
            generation: self.identity.generation,
            operation,
        }
    }
    fn remaining(&self) -> Result<Duration> {
        Ok(self
            .lease
            .lock()
            .map_err(|_| anyhow::anyhow!("snapshot lease unavailable"))?
            .dispatch_deadline(Instant::now())?
            .saturating_duration_since(Instant::now()))
    }
    async fn renew(&self) -> Result<()> {
        let renewal = self
            .lease
            .lock()
            .map_err(|_| anyhow::anyhow!("snapshot lease unavailable"))?
            .begin_renewal(Instant::now())?;
        let request = self.request(renewal.request_id.clone(), WorkerOperation::Heartbeat {});
        let WorkerReply::Lease { grant } = self
            .transport
            .send(&request, Duration::from_secs(5))
            .await?
        else {
            anyhow::bail!("invalid snapshot lease reply");
        };
        self.lease
            .lock()
            .map_err(|_| anyhow::anyhow!("snapshot lease unavailable"))?
            .apply(renewal, &grant, Instant::now())
    }
    async fn renew_bounded(&self) -> Result<()> {
        for retry in 0..3 {
            match self.renew().await {
                Ok(()) => return Ok(()),
                Err(error) => {
                    if retry == 2
                        || !error
                            .downcast_ref::<super::TransportError>()
                            .is_some_and(|e| e.kind == TransportFailure::Retryable)
                    {
                        return Err(error);
                    }
                    tokio::time::sleep(Duration::from_millis(250) * (1 << retry)).await;
                }
            }
        }
        anyhow::bail!("snapshot lease recovery exhausted")
    }
    async fn keep_alive(&self) -> Result<()> {
        loop {
            tokio::time::sleep(Duration::from_secs(10).min(self.remaining()? / 3)).await;
            self.remaining()?;
            self.renew_bounded().await?;
        }
    }
    async fn object(
        &self,
        receiver: &Arc<SnapshotReceiver>,
        snapshot: &str,
        blob: Option<&str>,
        size: Option<u64>,
    ) -> Result<()> {
        let digest = blob.unwrap_or(snapshot);
        if complete(receiver, digest, size).await? {
            return Ok(());
        }
        loop {
            let read = SnapshotRead {
                snapshot: snapshot.into(),
                blob: blob.map(str::to_owned),
                offset: receiver.offset(digest)?,
            };
            let request = self.request(
                uuid::Uuid::new_v4().to_string(),
                WorkerOperation::SnapshotRead {
                    request: read.clone(),
                },
            );
            let mut accepted = false;
            for retry in 0..3 {
                let timeout = self.remaining()?.min(Duration::from_secs(5));
                match self.transport.send(&request, timeout).await {
                    Ok(WorkerReply::Acknowledged { acknowledgement }) => {
                        self.remaining()?;
                        let chunk: SnapshotChunk = serde_json::from_value(acknowledgement.result)?;
                        ensure!(
                            size.is_none_or(|size| chunk.total == size),
                            "snapshot object size mismatch"
                        );
                        let receiver = receiver.clone();
                        let read = read.clone();
                        tokio::task::spawn_blocking(move || receiver.append(&read, &chunk))
                            .await??;
                        accepted = true;
                        break;
                    }
                    Ok(_) => anyhow::bail!("unexpected snapshot reply"),
                    Err(error) => {
                        if retry == 2 || error.kind != TransportFailure::Retryable {
                            return Err(error.into());
                        }
                        tokio::time::sleep(Duration::from_millis(250) * (1 << retry)).await;
                    }
                }
            }
            ensure!(accepted, "snapshot read exhausted recovery");
            if complete(receiver, digest, size).await? {
                return Ok(());
            }
        }
    }
}

async fn complete(
    receiver: &Arc<SnapshotReceiver>,
    digest: &str,
    size: Option<u64>,
) -> Result<bool> {
    let receiver = receiver.clone();
    let digest = digest.to_owned();
    tokio::task::spawn_blocking(move || receiver.complete(&digest, size)).await?
}

/// Fetches only the sealed snapshot named by trusted launch preparation. No
/// provider configuration or canonical database is opened on this destination.
/// The cache is private to this preparation and stays locked through blocking
/// materialization even when its caller is canceled. Backend recovery must
/// retain the environment while that lock is held.
pub async fn prepare_workspace(
    launch: &HostWorkerLaunch,
    identity: &ExecutionIdentity,
    cache: &Path,
    destination: &Path,
) -> Result<()> {
    launch.validate().map_err(anyhow::Error::msg)?;
    identity.validate().map_err(anyhow::Error::msg)?;
    let snapshot = launch
        .snapshot
        .as_ref()
        .context("initial snapshot not selected")?;
    let parent = destination
        .parent()
        .context("missing workspace parent")?
        .canonicalize()?;
    let destination = parent.join(destination.file_name().context("invalid workspace name")?);
    let receiver = Arc::new(SnapshotReceiver::open(cache)?);
    let transfer = Transfer {
        transport: WorkerTransport::new(&launch.leader_origin, &launch.worker_bearer)?,
        identity: identity.clone(),
        lease: Mutex::new(Lease::new(identity.clone())?),
    };
    transfer.renew_bounded().await?;
    let prepare = async {
        if receiver.installed(snapshot, identity, &destination)? {
            return Ok(());
        }
        ensure!(
            !destination.try_exists()?,
            "initial workspace already exists"
        );
        transfer.object(&receiver, snapshot, None, None).await?;
        let manifest = receiver.manifest(snapshot)?;
        let mut blobs = std::collections::BTreeMap::new();
        for entry in &manifest.entries {
            if let EntryKind::File { sha256, size, .. } = &entry.kind {
                blobs.insert(sha256, *size);
            }
        }
        for repo in &manifest.repositories {
            blobs.insert(&repo.bundle, repo.size);
        }
        for (digest, size) in blobs {
            transfer
                .object(&receiver, snapshot, Some(digest), Some(size))
                .await?;
        }
        transfer.remaining()?;
        let receiver = receiver.clone();
        let snapshot = snapshot.clone();
        let identity = identity.clone();
        tokio::task::spawn_blocking(move || receiver.install(&snapshot, &identity, &destination))
            .await??;
        transfer.remaining()?;
        Ok(())
    };
    tokio::select! { result = prepare => result, result = transfer.keep_alive() => result }
}
