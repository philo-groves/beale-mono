//! Bounded leader-side discovery. Slow peers never block local maintenance.
use super::peer_coordinator::{OwnedPeerAttempt, PeerWorkerCoordinator};
use anyhow::{Context, Result};
use beale_contracts::swarm_host::ExecutionTarget;
use beale_storage::{
    HostDatabase,
    fleet_coordination::FleetCoordinationStore,
    swarm_state::{PeerRecoveryWarning, SwarmStore, WorkerDisposition, WorkerProgress},
};
use std::{
    future::Future,
    path::{Path, PathBuf},
    time::Duration,
};

pub(crate) struct PeerRecovery {
    db: HostDatabase,
    root: PathBuf,
    cursor: String,
    tasks: tokio::task::JoinSet<Result<()>>,
}
impl PeerRecovery {
    pub fn new(db: HostDatabase, root: PathBuf) -> Self {
        Self {
            db,
            root,
            cursor: String::new(),
            tasks: tokio::task::JoinSet::new(),
        }
    }
    pub fn tick(&mut self) -> Result<()> {
        while let Some(result) = self.tasks.try_join_next() {
            if !matches!(result, Ok(Ok(()))) {
                beale_host_log::error!(
                    "Peer recovery remains pending; retained execution ownership was not released."
                );
            }
        }
        if self.tasks.len() >= 4 {
            return Ok(());
        }
        let records = SwarmStore::open(self.db.clone())?.peer_recovery_page(&self.cursor)?;
        let count = records.len();
        let mut consumed = 0;
        for attempt in records {
            if self.tasks.len() >= 4 {
                break;
            }
            self.cursor = attempt.clone();
            consumed += 1;
            let db = self.db.clone();
            let root = self.root.clone();
            self.tasks.spawn(async move {
                tokio::time::timeout(
                    Duration::from_secs(600),
                    recover_one(&db, &root, &attempt, |target, _| {
                        super::placement::checkpoint(&db, &root, target)
                    }),
                )
                .await
                .context("peer recovery deadline exceeded")?
            });
        }
        if consumed == count && count < 64 {
            self.cursor.clear();
        }
        Ok(())
    }
}

pub(super) async fn recover_one<F, Fut>(
    db: &HostDatabase,
    root: &Path,
    attempt: &str,
    mut checkpoint: F,
) -> Result<()>
where
    F: FnMut(ExecutionTarget, WorkerProgress) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let Some(mut owned) = OwnedPeerAttempt::acquire(db, root, attempt)? else {
        return Ok(());
    };
    let store = SwarmStore::open(db.clone())?;
    let record = owned.record();
    // No pairing is required when durable ordering proves Start was impossible.
    if !record.dispatched && !store.worker_prepared(attempt)? {
        return store.abandon_unprepared_peer(attempt, record.owner_key);
    }
    if record.terminated() {
        // Already persisted host proof and accepted receipts remain usable even
        // when a pairing is subsequently removed. No remote IO is needed.
        PeerWorkerCoordinator::finish(&store, record, WorkerDisposition::Failed, &mut checkpoint)
            .await?;
        return Ok(());
    }
    // Revoke authority before attempting pairing resolution or network I/O.
    store.peer_request_stop(attempt, record.owner_key)?;
    let coordinator = FleetCoordinationStore::open(db.clone())?
        .peer(&record.target.host_id)
        .and_then(|peer| {
            PeerWorkerCoordinator::for_recovery(
                db.clone(),
                root.to_path_buf(),
                &peer.context("saved peer unavailable")?,
            )
        });
    let coordinator = match coordinator {
        Ok(coordinator) => coordinator,
        Err(error) => {
            store.peer_recovery_warning(
                attempt,
                record.owner_key,
                PeerRecoveryWarning::PairingUnavailable,
            )?;
            if !store.worker_reconciled(&record.turn)? {
                super::supervisor::reconcile_owned_attempt(
                    &store,
                    &record.target.identity,
                    false,
                    WorkerDisposition::Failed,
                    crate::tool_runtime::unix_ms(),
                )?;
            }
            return Err(error);
        }
    };
    coordinator.recover(&mut owned, checkpoint).await?;
    Ok(())
}
