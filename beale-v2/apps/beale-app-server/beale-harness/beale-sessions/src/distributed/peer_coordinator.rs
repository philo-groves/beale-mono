//! Leader lifecycle for an admitted peer instance. Physical admission and
//! checkpoint implementations belong to the owning environment backend.
use anyhow::{Result, ensure};
use beale_contracts::{swarm_host::*, swarms::*};
use beale_fleet::{
    FleetPeer,
    worker_host::{HostExecutionClient, PeerFailure},
};
use beale_storage::{
    HostDatabase,
    fleet_executions::FleetExecutionStore,
    swarm_state::{
        PeerPlacement, PeerRecoveryWarning, PeerWorkerRecord, SwarmStore, WorkerDisposition,
        WorkerProgress, WorkerReconciliation,
    },
};
use std::{fs::File, future::Future, path::PathBuf, time::Duration};
use tokio::sync::watch;

pub struct OwnedPeerAttempt {
    record: PeerWorkerRecord,
    _owner: PeerOwner,
}
struct PeerOwner(File);
impl Drop for PeerOwner {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}
impl OwnedPeerAttempt {
    pub(crate) fn acquire(
        db: &HostDatabase,
        root: &std::path::Path,
        attempt: &str,
    ) -> Result<Option<Self>> {
        let record = SwarmStore::open(db.clone())?.peer_worker(attempt)?;
        ensure!(
            record.target.leader_id == FleetExecutionStore::open(db.clone())?.host_id()?,
            "peer recovery leader identity mismatch"
        );
        if record.complete {
            return Ok(None);
        }
        Ok(
            super::restart::lock_at(root, record.owner_key)?.map(|owner| Self {
                record,
                _owner: PeerOwner(owner),
            }),
        )
    }
    pub fn record(&self) -> &PeerWorkerRecord {
        &self.record
    }
}

pub struct PeerWorkerCoordinator {
    db: HostDatabase,
    root: PathBuf,
    client: HostExecutionClient,
    peer_id: String,
    leader_id: String,
    leader_origin: String,
    launch_allowed: bool,
}
impl PeerWorkerCoordinator {
    pub fn new(
        db: HostDatabase,
        root: PathBuf,
        peer: &FleetPeer,
        leader_origin: String,
    ) -> Result<Self> {
        validate_origin(&leader_origin).map_err(anyhow::Error::msg)?;
        Ok(Self {
            leader_id: FleetExecutionStore::open(db.clone())?.host_id()?,
            db,
            root,
            client: HostExecutionClient::for_peer(peer)?,
            peer_id: peer.id.clone(),
            leader_origin,
            launch_allowed: true,
        })
    }
    /// Disabling new placement must not discard credentials needed to stop an
    /// existing instance. This coordinator cannot claim or launch any new work.
    pub fn for_recovery(db: HostDatabase, root: PathBuf, peer: &FleetPeer) -> Result<Self> {
        let peer = FleetPeer {
            id: peer.id.clone(),
            endpoint: peer.endpoint.clone(),
            control_token: peer.control_token.clone(),
            protocol_major: peer.protocol_major,
            enabled: true,
        };
        let mut result = Self::new(db, root, &peer, "http://127.0.0.1:1".into())?;
        result.launch_allowed = false;
        Ok(result)
    }
    pub fn claim(
        &self,
        scan: &str,
        agent: &str,
        capacity: usize,
        placement: &PeerPlacement,
    ) -> Result<Option<OwnedPeerAttempt>> {
        ensure!(
            self.launch_allowed
                && placement.host_id == self.peer_id
                && placement.leader_id == self.leader_id,
            "peer placement identity unavailable"
        );
        let key = uuid::Uuid::new_v4();
        let owner = PeerOwner(
            super::restart::lock_at(&self.root, key)?
                .ok_or_else(|| anyhow::anyhow!("peer coordinator ownership unavailable"))?,
        );
        let store = SwarmStore::open(self.db.clone())?;
        let turn = store.peer_claim_owned(
            scan,
            agent,
            crate::tool_runtime::unix_ms(),
            capacity,
            key,
            placement,
        )?;
        match turn {
            Some(turn) => Ok(Some(OwnedPeerAttempt {
                record: store.peer_worker(&turn.attempt_id)?,
                _owner: owner,
            })),
            None => Ok(None),
        }
    }
    pub fn acquire_recovery(&self, attempt: &str) -> Result<Option<OwnedPeerAttempt>> {
        let owned = OwnedPeerAttempt::acquire(&self.db, &self.root, attempt)?;
        if let Some(owned) = &owned {
            ensure!(
                owned.record.target.host_id == self.peer_id,
                "peer recovery host identity mismatch"
            );
        }
        Ok(owned)
    }

    fn request(
        record: &PeerWorkerRecord,
        operation: HostExecutionOperation,
    ) -> HostExecutionRequest {
        let kind = match operation {
            HostExecutionOperation::Start { .. } => "start",
            HostExecutionOperation::Stop {} => "stop",
            HostExecutionOperation::Inspect {} => "inspect",
        };
        HostExecutionRequest {
            version: 1,
            operation_id: format!("{kind}:{}", record.turn.attempt_id),
            target: record.target.clone(),
            operation,
        }
    }
    async fn send(
        &self,
        store: &SwarmStore,
        record: &PeerWorkerRecord,
        operation: HostExecutionOperation,
    ) -> Result<PeerWorkerRecord> {
        let request = Self::request(record, operation);
        for retry in 0..3 {
            match self.client.send(&request, Duration::from_secs(5)).await {
                Ok(reply) => {
                    return store.peer_observe(
                        &record.turn.attempt_id,
                        record.owner_key,
                        &reply.report,
                    );
                }
                Err(error) => {
                    if retry == 2 || error.kind != PeerFailure::Retryable {
                        return Err(error.into());
                    }
                    tokio::time::sleep(Duration::from_millis(250) * (1 << retry)).await;
                }
            }
        }
        anyhow::bail!("peer transport recovery exhausted")
    }
    async fn stop(
        &self,
        store: &SwarmStore,
        record: &PeerWorkerRecord,
    ) -> Result<PeerWorkerRecord> {
        let mut record = store.peer_request_stop(&record.turn.attempt_id, record.owner_key)?;
        if record.terminated() {
            return Ok(record);
        }
        for retry in 0..3 {
            record = self
                .send(store, &record, HostExecutionOperation::Stop {})
                .await?;
            if record.terminated() {
                break;
            }
            if retry < 2 {
                tokio::time::sleep(Duration::from_millis(250) * (1 << retry)).await;
            }
        }
        Ok(record)
    }
    pub(super) async fn finish<F, Fut>(
        store: &SwarmStore,
        record: &PeerWorkerRecord,
        disposition: WorkerDisposition,
        checkpoint: &mut F,
    ) -> Result<WorkerReconciliation>
    where
        F: FnMut(ExecutionTarget, WorkerProgress) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        if !record.terminated() {
            return super::supervisor::reconcile_owned_attempt(
                store,
                &record.target.identity,
                false,
                disposition,
                crate::tool_runtime::unix_ms(),
            );
        }
        let disposition =
            store.peer_disposition(&record.turn.attempt_id, record.owner_key, disposition)?;
        if !store.worker_reconciled(&record.turn)? {
            // The opaque checkpoint boundary must acknowledge retained private
            // storage before central ownership can be released for reuse.
            if let Err(error) = checkpoint(
                record.target.clone(),
                store.worker_progress(&record.turn.attempt_id)?,
            )
            .await
            {
                store.peer_recovery_warning(
                    &record.turn.attempt_id,
                    record.owner_key,
                    PeerRecoveryWarning::CheckpointPending,
                )?;
                return Err(error);
            }
        }
        let result = super::supervisor::reconcile_owned_attempt(
            store,
            &record.target.identity,
            true,
            disposition,
            crate::tool_runtime::unix_ms(),
        )?;
        store.complete_peer_worker(&record.turn.attempt_id, record.owner_key)?;
        Ok(result)
    }
    pub async fn run<F, Fut>(
        &self,
        owned: &mut OwnedPeerAttempt,
        snapshot: Option<String>,
        mut cancel: watch::Receiver<bool>,
        mut checkpoint: F,
    ) -> Result<WorkerReconciliation>
    where
        F: FnMut(ExecutionTarget, WorkerProgress) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        ensure!(self.launch_allowed, "recovery coordinator cannot launch");
        let store = SwarmStore::open(self.db.clone())?;
        let original = &owned.record;
        let record = store.peer_worker(&original.turn.attempt_id)?;
        ensure!(
            record.owner_key == original.owner_key
                && record.target == original.target
                && !record.dispatched
                && !record.complete,
            "peer launch identity changed"
        );
        let (_, credential) = super::prepare_worker(
            &self.db,
            &record.turn,
            record.shell,
            &WorkerRegistration {
                version: VERSION,
                audience: WORKER_AUDIENCE.into(),
                capabilities: vec![
                    WorkerCapability::CanonicalRouting,
                    WorkerCapability::ArtifactTransfer,
                    WorkerCapability::DurableEvents,
                ],
            },
            crate::tool_runtime::unix_ms(),
        )?;
        let launch = HostWorkerLaunch {
            leader_origin: self.leader_origin.clone(),
            worker_bearer: credential.bearer().into(),
            snapshot,
        };
        launch.validate().map_err(anyhow::Error::msg)?;
        let record = store.peer_dispatch(&record.turn.attempt_id, record.owner_key)?;
        // The intent is already durable. Dropping this future leaves recovery
        // to fence and send Stop; it cannot safely reissue Start after restart.
        let started = if *cancel.borrow() || cancel.has_changed().is_err() {
            None
        } else {
            tokio::select! {
                biased;
                _ = cancel.changed() => None,
                result = self.send(&store,&record,HostExecutionOperation::Start { launch }) => result.ok(),
            }
        };
        let mut interrupted = started.is_none();
        let mut record = started.unwrap_or(record);
        let mut disposition = WorkerDisposition::Completed;
        loop {
            let local = store.local_state(&record.turn.scan_id)?;
            let canceled = *cancel.borrow()
                || cancel.has_changed().is_err()
                || local
                    .controls
                    .iter()
                    .any(|c| c.agent_id == record.turn.agent_id && c.stop_requested);
            if record.terminated() {
                if canceled || record.stop_requested {
                    disposition = WorkerDisposition::Canceled;
                }
                break;
            }
            let saved = store.load(&record.turn.scan_id, true)?;
            let expired = saved
                .attempts
                .iter()
                .find(|a| a.id == record.turn.attempt_id)
                .is_none_or(|a| {
                    a.state != ExecutionState::Running
                        || a.expires_at_ms <= crate::tool_runtime::unix_ms()
                });
            if canceled || interrupted || expired || record.stop_requested {
                disposition = if canceled {
                    WorkerDisposition::Canceled
                } else {
                    WorkerDisposition::Failed
                };
                record = match self.stop(&store, &record).await {
                    Ok(record) => record,
                    Err(_) => store.peer_worker(&record.turn.attempt_id)?,
                };
                break;
            }
            tokio::select! {
                biased;
                _ = cancel.changed() => (),
                _ = tokio::time::sleep(Duration::from_millis(250)) => {
                    match self.send(&store,&record,HostExecutionOperation::Inspect {}).await {
                        Ok(observed)=>record=observed, Err(_)=>interrupted=true,
                    }
                }
            }
        }
        if disposition == WorkerDisposition::Completed
            && store
                .worker_progress(&record.turn.attempt_id)?
                .answer
                .is_none()
        {
            disposition = WorkerDisposition::Failed;
        }
        Self::finish(&store, &record, disposition, &mut checkpoint).await
    }
    pub async fn recover<F, Fut>(
        &self,
        owned: &mut OwnedPeerAttempt,
        mut checkpoint: F,
    ) -> Result<WorkerReconciliation>
    where
        F: FnMut(ExecutionTarget, WorkerProgress) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        let store = SwarmStore::open(self.db.clone())?;
        let record = store.peer_worker(&owned.record.turn.attempt_id)?;
        ensure!(
            record.owner_key == owned.record.owner_key && record.target == owned.record.target,
            "peer recovery ownership changed"
        );
        if !record.dispatched && !store.worker_prepared(&record.turn.attempt_id)? {
            store.abandon_unprepared_peer(&record.turn.attempt_id, record.owner_key)?;
            return Ok(WorkerReconciliation::BranchStopped);
        }
        let record = match self.stop(&store, &record).await {
            Ok(record) => record,
            Err(_) => store.peer_worker(&record.turn.attempt_id)?,
        };
        Self::finish(&store, &record, WorkerDisposition::Failed, &mut checkpoint).await
    }
}
