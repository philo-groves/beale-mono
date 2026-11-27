//! Discover saved same-host launch intents without recreating execution.
use crate::{SessionState, research_files, tool_runtime};
use anyhow::{Result, ensure};
use beale_storage::swarm_state::{LocalWorkerRecord, SwarmStore, WorkerDisposition};
use std::fs::{File, OpenOptions, TryLockError};
use uuid::Uuid;

pub(crate) fn lock(state: &SessionState, key: Uuid) -> Result<Option<File>> {
    lock_at(state.workspaces.root(), key)
}

pub(super) fn lock_at(root: &std::path::Path, key: Uuid) -> Result<Option<File>> {
    let root = root.join("swarm-worker-owners");
    std::fs::create_dir_all(&root)?;
    ensure!(
        std::fs::symlink_metadata(&root)?.file_type().is_dir(),
        "invalid local ownership directory"
    );
    let path = root.join(format!("{key}.lock"));
    if path.exists() {
        ensure!(
            std::fs::symlink_metadata(&path)?.file_type().is_file(),
            "invalid local ownership lock"
        );
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error.into()),
    }
}

pub(crate) fn recover_page(
    state: &SessionState,
    scan: Option<&str>,
    after: &str,
) -> Result<String> {
    let store = SwarmStore::open(state.db.clone())?;
    let records = store.local_worker_recovery_page(scan, after)?;
    let next = if records.len() == 64 {
        records.last().cloned().unwrap_or_default()
    } else {
        String::new()
    };
    for record in records {
        if let Err(error) = store
            .local_worker_record(&record)
            .and_then(|record| recover_one(state, &store, &record))
        {
            beale_host_log::error!("Saved worker recovery remains pending: {error:#}");
        }
    }
    Ok(next)
}

fn recover_one(state: &SessionState, store: &SwarmStore, record: &LocalWorkerRecord) -> Result<()> {
    let Some(_owner) = lock(state, record.owner_key)? else {
        return Ok(());
    };
    let turn = &record.turn;
    let saved = store.load(&turn.scan_id, true)?;
    ensure!(
        saved.config.destinations.is_empty(),
        "saved local placement has selected destinations"
    );
    if !store.worker_prepared(&turn.attempt_id)? {
        // Launch intent precedes preparation, which precedes process creation.
        // No bootstrap can have been released without that atomic preparation.
        return store.reconcile_unlaunched_worker(record);
    }
    let identity = store.worker_provenance(&turn.attempt_id)?.identity;
    ensure!(
        identity.scan_id == turn.scan_id
            && identity.agent_id == turn.agent_id
            && identity.generation == turn.generation,
        "saved local execution mismatch"
    );
    if !store.worker_reconciled(turn)? {
        store.fence_attempt(&turn.scan_id, &turn.attempt_id, false)?;
        let terminated = if record.backend != beale_storage::swarm_state::local_worker_backend() {
            false
        } else {
            #[cfg(target_os = "linux")]
            {
                beale_tools::linux_worker::LinuxWorker::open(
                    &state.workspaces.root().join("swarm-linux-supervisors"),
                    record.owner_key,
                )?
                .recover_terminated()?
            }
            #[cfg(not(target_os = "linux"))]
            {
                beale_tools::process::ProcessTree::recover_terminated(record.owner_key)?
            }
        };
        if terminated {
            let progress = store.worker_progress(&turn.attempt_id)?;
            research_files::checkpoint(state, turn, &progress.context)?;
        }
        let result = super::supervisor::reconcile_owned_attempt(
            store,
            &identity,
            terminated,
            WorkerDisposition::Failed,
            tool_runtime::unix_ms(),
        )?;
        if result == beale_storage::swarm_state::WorkerReconciliation::TerminationUnconfirmed {
            return Ok(());
        }
    }
    let recovery = state
        .workspaces
        .root()
        .join("swarm-worker-recovery")
        .join(&turn.attempt_id);
    super::recovery::publications(state, &identity, &recovery)?;
    store.complete_local_worker_recovery(record)
}

/// A separate cursor prevents large worker queues from starving coordinators.
pub(crate) fn recover_coordinators(
    state: &SessionState,
    scan: Option<&str>,
    after: &str,
) -> Result<String> {
    let store = SwarmStore::open(state.db.clone())?;
    let ids = store.coordinator_recovery_page(scan, after)?;
    let next = if ids.len() == 64 {
        ids.last().cloned().unwrap_or_default()
    } else {
        String::new()
    };
    for id in ids {
        let recover = || -> Result<()> {
            let (_, key) = store.coordinator_owner(&id)?;
            let Some(_owner) = lock(state, key)? else {
                return Ok(());
            };
            store.interrupt_coordinator(&id, key, tool_runtime::unix_ms())
        };
        if let Err(error) = recover() {
            beale_host_log::error!("Interrupted coordinator recovery remains pending: {error:#}");
        }
    }
    Ok(next)
}

/// No intent was committed for this random key, so no recovery reader can own
/// its file. Remove empty probe locks without touching registered ownership.
pub(crate) fn discard_unclaimed_lock(state: &SessionState, key: Uuid, owner: File) {
    drop(owner);
    let _ = std::fs::remove_file(
        state
            .workspaces
            .root()
            .join("swarm-worker-owners")
            .join(format!("{key}.lock")),
    );
}
