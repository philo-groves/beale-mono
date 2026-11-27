//! Incremental publication storage is reserved on the leader, independently of
//! the execution host's storage and memory. This grants no execution authority.
use crate::SessionState;
use anyhow::{Result, ensure};
use beale_contracts::swarm_allocation::*;
use beale_storage::{
    fleet_allocation::AllocationStore,
    swarm_state::{ScanState, SwarmStore},
};
use std::collections::BTreeMap;

/// Held from admission through execution binding. A slow live placement must
/// not lose its publication reservation merely because its deadline elapsed.
/// Dropping the operation (including task cancellation or process exit) releases
/// the lock, allowing normal maintenance to reclaim expired unbound storage.
pub(super) struct OwnedReservation {
    pub id: String,
    _owner: std::fs::File,
}
impl Drop for OwnedReservation {
    fn drop(&mut self) {
        let _ = self._owner.unlock();
    }
}
fn capacity(state: &SessionState) -> Result<Capacity> {
    let (pool, bytes) = beale_fleet::allocation::capacity::host_storage(state.workspaces.root())?;
    Ok(Capacity {
        domain: "canonical-storage".into(),
        observed_at_ms: beale_fleet::allocation::Allocator::now(),
        available_memory: u64::MAX,
        storage: BTreeMap::from([(pool, bytes)]),
        measured_memory: BTreeMap::new(),
        measured_storage: AllocationStore::open(state.db.clone())?.reflected_storage_keys()?,
    })
}
fn request(
    state: &SessionState,
    saved: &ScanState,
    agent: &str,
    id: String,
    c: &Capacity,
) -> Result<AllocationRequest> {
    let leader =
        beale_storage::fleet_executions::FleetExecutionStore::open(state.db.clone())?.host_id()?;
    Ok(AllocationRequest {
        version: 1,
        id: id.clone(),
        leader,
        scan: saved.config.scan_id.clone(),
        agent: agent.into(),
        branch: agent.into(),
        domain: "canonical-storage".into(),
        configuration: "canonical-storage".into(),
        configuration_revision: 1,
        instance: uuid::Uuid::new_v4().to_string(),
        memory: 1,
        storage: c
            .storage
            .keys()
            .map(|pool| StorageDemand {
                pool: pool.clone(),
                key: format!("artifacts-{id}"),
                bytes: 256 * 1024 * 1024,
            })
            .collect(),
        storage_reserve: saved.config.retention.storage_reserve_bytes,
        release_parent: None,
        processing_result: false,
    })
}
pub(super) fn feasible(state: &SessionState, saved: &ScanState) -> Result<bool> {
    let c = capacity(state)?;
    let store = AllocationStore::open(state.db.clone())?;
    Ok(matches!(
        store.feasibility(
            &request(
                state,
                saved,
                "prospective-child",
                uuid::Uuid::new_v4().to_string(),
                &c
            )?,
            &c,
            c.observed_at_ms
        )?,
        Feasibility::Ready
    ))
}
pub(super) async fn reserve(
    state: &SessionState,
    saved: &ScanState,
    agent: &str,
    id: &str,
    stop: &Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<Option<OwnedReservation>> {
    reserve_until(
        state,
        saved,
        agent,
        id,
        stop,
        tokio::time::Instant::now() + std::time::Duration::from_secs(300),
    )
    .await
}

pub(super) async fn reserve_until(
    state: &SessionState,
    saved: &ScanState,
    agent: &str,
    id: &str,
    stop: &Option<tokio::sync::watch::Receiver<bool>>,
    deadline: tokio::time::Instant,
) -> Result<Option<OwnedReservation>> {
    let id = format!("central-{id}");
    let store = AllocationStore::open(state.db.clone())?;
    let c = capacity(state)?;
    if store.allocation_if_present(&id)?.is_none() {
        store.enqueue(
            request(state, saved, agent, id.clone(), &c)?,
            &c,
            c.observed_at_ms,
        )?;
    }
    let row = store.allocation(&id)?;
    let instance = row.request.instance.parse()?;
    let owner = loop {
        // A waiter has no authority to cancel another placement's reservation.
        // Maintenance uses this lock too; its brief ownership is not a failure.
        if crate::engine::stop_requested(stop)
            || SwarmStore::open(state.db.clone())?
                .local_state(&saved.config.scan_id)?
                .controls
                .iter()
                .any(|c| c.agent_id == agent && c.stop_requested)
            || tokio::time::Instant::now() >= deadline
        {
            return Ok(None);
        }
        if let Some(owner) = super::restart::lock_at(state.workspaces.root(), instance)? {
            break owner;
        }
        tokio::time::sleep_until(
            deadline.min(tokio::time::Instant::now() + std::time::Duration::from_millis(50)),
        )
        .await;
    };
    let owned = OwnedReservation {
        id: id.clone(),
        _owner: owner,
    };
    // The former owner may have bound execution while this caller waited.
    // Durable execution now owns this storage; it cannot be claimed again.
    if store.allocation(&id)?.execution.is_some() {
        return Ok(None);
    }
    loop {
        if crate::engine::stop_requested(stop)
            || SwarmStore::open(state.db.clone())?
                .local_state(&saved.config.scan_id)?
                .controls
                .iter()
                .any(|c| c.agent_id == agent && c.stop_requested)
        {
            store.release_canonical_storage(&id)?;
            anyhow::bail!("canonical storage wait canceled before launch");
        }
        let c = capacity(state)?;
        store.admit_next(&c, c.observed_at_ms)?;
        match store.allocation(&id)?.phase {
            AllocationPhase::Reserved => return Ok(Some(owned)),
            AllocationPhase::Queued => (),
            _ => anyhow::bail!("Leader artifact storage admission expired or was canceled"),
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "Leader artifact storage cannot preserve its reserve; publication admission is blocked"
        );
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}
pub(super) fn released(
    db: &beale_storage::HostDatabase,
    target: &beale_contracts::swarm_host::ExecutionTarget,
) -> Result<()> {
    let store = AllocationStore::open(db.clone())?;
    for row in store.allocations()?.into_iter().filter(|a| {
        a.request.domain == "canonical-storage"
            && a.execution.as_ref() == Some(target)
            && a.phase != AllocationPhase::Canceled
    }) {
        store.release_canonical_storage(&row.request.id)?;
    }
    Ok(())
}
pub(super) fn reconcile(state: &SessionState) -> Result<()> {
    let store = AllocationStore::open(state.db.clone())?;
    for row in store
        .allocations()?
        .into_iter()
        .filter(|a| a.request.domain == "canonical-storage" && a.phase != AllocationPhase::Canceled)
    {
        let Some(_owner) =
            super::restart::lock_at(state.workspaces.root(), row.request.instance.parse()?)?
        else {
            continue;
        };
        // Admission or binding may have advanced after the maintenance snapshot.
        // Read again under the same lock used by the live placement operation.
        let row = store.allocation(&row.request.id)?;
        if row.phase == AllocationPhase::Canceled {
            continue;
        }
        let complete = if let Some(target) = &row.execution {
            SwarmStore::open(state.db.clone())?
                .peer_worker(&target.identity.attempt_id)?
                .complete
        } else {
            row.reservation_deadline_ms < beale_fleet::allocation::Allocator::now()
        };
        if complete {
            store.release_canonical_storage(&row.request.id)?;
        }
    }
    Ok(())
}
