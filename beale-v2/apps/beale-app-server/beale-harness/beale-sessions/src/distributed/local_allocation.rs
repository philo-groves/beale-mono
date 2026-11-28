//! Native execution uses the same atomic owning-host resource ledger as Fleet.
use crate::SessionState;
use anyhow::{Context, Result};
use beale_contracts::{
    swarm_allocation::*,
    swarm_local::Admission,
    swarms::{AgentState, ExecutionState, Role},
};
use beale_storage::{
    fleet_allocation::AllocationStore,
    swarm_state::{LocalTurn, ScanState, SwarmStore},
};
use std::collections::BTreeMap;
use tokio::sync::watch;
pub(crate) struct OwnedAllocation {
    pub id: String,
    _owner: std::fs::File,
}
impl Drop for OwnedAllocation {
    fn drop(&mut self) {
        let _ = self._owner.unlock();
    }
}
fn ledger(state: &SessionState) -> Result<AllocationStore> {
    state.db.migrate_schema("swarm-local-allocation",&["CREATE TABLE swarm_local_allocation_attempts(allocation TEXT PRIMARY KEY, attempt TEXT NOT NULL UNIQUE);"])?;
    AllocationStore::open(state.db.clone())
}
fn capacity(state: &SessionState) -> Result<Capacity> {
    let (pool, free) = beale_fleet::allocation::capacity::host_storage(state.workspaces.root())?;
    Ok(Capacity {
        domain: "native".into(),
        observed_at_ms: beale_fleet::allocation::Allocator::now(),
        available_memory: beale_fleet::allocation::capacity::local_memory()?,
        storage: BTreeMap::from([(pool, free)]),
        measured_memory: BTreeMap::new(),
        measured_storage: ledger(state)?.reflected_storage_keys()?,
    })
}
fn request(
    state: &SessionState,
    saved: &ScanState,
    agent: &str,
    parent: &str,
    id: String,
    processing_result: bool,
) -> Result<(AllocationRequest, Capacity)> {
    let store = ledger(state)?;
    let capacity = capacity(state)?;
    let leader =
        beale_storage::fleet_executions::FleetExecutionStore::open(state.db.clone())?.host_id()?;
    let previous = store.allocations()?;
    let instance = store
        .environments()?
        .into_iter()
        .find(|e| e.leader == leader && e.scan == saved.config.scan_id && e.agent == agent)
        .map(|e| e.instance)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let release_parent = previous
        .iter()
        .rev()
        .find(|a| {
            a.request.domain == "native"
                && a.request.scan == saved.config.scan_id
                && a.request.agent == parent
                && matches!(
                    a.phase,
                    AllocationPhase::Reserved | AllocationPhase::Started
                )
        })
        .map(|a| a.request.id.clone());
    let bytes = super::placement::input_bytes(state, &saved.config.scan_id)?
        .checked_add(256 * 1024 * 1024)
        .context("local input size overflow")?;
    Ok((
        AllocationRequest {
            version: 1,
            id,
            leader,
            scan: saved.config.scan_id.clone(),
            agent: agent.into(),
            branch: parent.into(),
            domain: "native".into(),
            configuration: "native".into(),
            configuration_revision: 1,
            instance: instance.clone(),
            memory: DEFAULT_MEMORY,
            storage: capacity
                .storage
                .keys()
                .map(|pool| StorageDemand {
                    pool: pool.clone(),
                    key: format!("private-{instance}"),
                    bytes,
                })
                .collect(),
            storage_reserve: saved.config.retention.storage_reserve_bytes,
            release_parent,
            processing_result,
        },
        capacity,
    ))
}
pub(crate) fn admission(
    state: &SessionState,
    turn: &LocalTurn,
    saved: &ScanState,
) -> Result<Admission> {
    let (r, c) = request(
        state,
        saved,
        "prospective-child",
        &turn.agent_id,
        uuid::Uuid::new_v4().to_string(),
        false,
    )?;
    Ok(
        match ledger(state)?.feasibility(&r, &c, c.observed_at_ms)? {
            Feasibility::Ready => Admission::Ready,
            Feasibility::ReleaseParent { .. } if turn.role == Role::Deputy => {
                Admission::YieldParent {
                    reason: "Queued until your checkpoint releases native execution capacity"
                        .into(),
                }
            }
            Feasibility::ReleaseParent { .. } => Admission::Denied {
                reason: "Native capacity is unavailable".into(),
            },
            Feasibility::Denied { reason } => Admission::Denied { reason },
        },
    )
}
pub(crate) async fn reserve(
    state: &SessionState,
    saved: &ScanState,
    agent: &str,
    stop: &Option<watch::Receiver<bool>>,
) -> Result<Option<OwnedAllocation>> {
    let swarm = SwarmStore::open(state.db.clone())?;
    let a = saved.agent(agent)?;
    let local = swarm.local_state(&saved.config.scan_id)?;
    let control = local
        .controls
        .iter()
        .find(|c| c.agent_id == agent)
        .context("local control missing")?;
    let result = local
        .inbox
        .iter()
        .any(|m| m.to == agent && m.sequence > a.checkpoint.inbox_cursor);
    if control.stop_requested
        || control.failures >= 3
        || (!result && a.summary.state != AgentState::Queued)
        || saved
            .attempts
            .iter()
            .any(|a| a.agent_id == agent && a.state != ExecutionState::Stopped)
    {
        return Ok(None);
    }
    if let Some(parent) = &control.release_parent {
        if local
            .controls
            .iter()
            .any(|c| &c.agent_id == parent && c.failures >= 3)
        {
            swarm.allocation_blocked(&saved.config.scan_id,agent,"Parent checkpoint/release recovery exhausted; this child did not acquire native execution capacity")?;
            return Ok(None);
        }
        if !local
            .controls
            .iter()
            .any(|c| &c.agent_id == parent && c.yield_committed)
        {
            return Ok(None);
        }
    }
    let generation = saved
        .attempts
        .iter()
        .filter(|a| a.agent_id == agent)
        .map(|a| a.generation)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .context("generation exhausted")?;
    let id = format!("allocation-{agent}-{generation}-{}", a.summary.revision);
    let store = ledger(state)?;
    let row = if let Some(old) = store.allocation_if_present(&id)? {
        old
    } else {
        let (r, c) = request(
            state,
            saved,
            agent,
            a.summary.parent_id.as_deref().context("parent missing")?,
            id.clone(),
            result,
        )?;
        store.enqueue(r, &c, c.observed_at_ms)?
    };
    let Some(owner) =
        super::restart::lock_at(state.workspaces.root(), row.request.instance.parse()?)?
    else {
        return Ok(None);
    };
    let owned = OwnedAllocation {
        id: id.clone(),
        _owner: owner,
    };
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
    loop {
        if crate::engine::stop_requested(stop)
            || swarm
                .local_state(&saved.config.scan_id)?
                .controls
                .iter()
                .any(|c| c.agent_id == agent && c.stop_requested)
        {
            // This owner is held before admission through launch binding.
            // A reservation without a bound local turn cannot have executed.
            if store.allocation(&id)?.phase == AllocationPhase::Reserved {
                store.confirm_stopped(&id, true)?;
            }
            store.cancel_queued(&id)?;
            return Ok(None);
        }
        let c = capacity(state)?;
        store.admit_next(&c, c.observed_at_ms)?;
        let row = store.allocation(&id)?;
        match row.phase {
            AllocationPhase::Reserved => {
                store.prepared(&id)?;
                return Ok(Some(owned));
            }
            AllocationPhase::Blocked | AllocationPhase::Canceled => {
                swarm.allocation_blocked(&saved.config.scan_id,agent,"Native RAM/storage capacity unavailable; stop lingering subagents with swarm_stop_subagent or add capacity")?;
                return Ok(None);
            }
            AllocationPhase::Started | AllocationPhase::Retained => {
                anyhow::bail!("previous native allocation requires recovery")
            }
            AllocationPhase::Queued => (),
        }
        if tokio::time::Instant::now() >= deadline {
            store.cancel_queued(&id)?;
            swarm.allocation_blocked(&saved.config.scan_id, agent, "Native allocation wait exhausted; no execution was launched. Stop lingering subagents or add capacity before retrying.")?;
            return Ok(None);
        }
        // Capacity changes when allocations bind, release, or reconcile; the
        // interval covers changes made by other processes.
        let mut stop = stop.clone();
        tokio::select! {
            _ = state.swarm_scans.allocations_changed.notified() => {}
            _ = async {
                match stop.as_mut() {
                    Some(receiver) => {
                        let _ = receiver.changed().await;
                    }
                    None => std::future::pending().await,
                }
            } => {}
            _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
        }
    }
}
pub(crate) fn bind(state: &SessionState, owned: &OwnedAllocation, turn: &LocalTurn) -> Result<()> {
    let store = ledger(state)?;
    state.db.with_connection(|db| {
        db.execute(
            "INSERT INTO swarm_local_allocation_attempts VALUES(?1,?2)",
            beale_storage::rusqlite::params![owned.id, turn.attempt_id],
        )?;
        Ok(())
    })?;
    store.started(&owned.id)
}
pub(crate) fn unclaimed(state: &SessionState, owned: &OwnedAllocation) -> Result<()> {
    ledger(state)?.confirm_stopped(&owned.id, true)
}
pub(crate) fn reconcile(state: &SessionState) -> Result<()> {
    let store = ledger(state)?;
    for row in store.allocations()?.into_iter().filter(|r| {
        r.request.domain == "native"
            && matches!(
                r.phase,
                AllocationPhase::Reserved | AllocationPhase::Started | AllocationPhase::Retained
            )
    }) {
        let Some(_owner) =
            super::restart::lock_at(state.workspaces.root(), row.request.instance.parse()?)?
        else {
            continue;
        };
        let attempt: Option<String> = state.db.with_connection(|db| {
            use beale_storage::rusqlite::OptionalExtension;
            Ok(db
                .query_row(
                    "SELECT attempt FROM swarm_local_allocation_attempts WHERE allocation=?1",
                    [&row.request.id],
                    |r| r.get(0),
                )
                .optional()?)
        })?;
        if let Some(attempt) = attempt {
            let complete: bool = state.db.with_connection(|db| {
                Ok(db.query_row(
                    "SELECT complete FROM swarm_local_workers WHERE attempt_id=?1",
                    [attempt],
                    |r| r.get(0),
                )?)
            })?;
            if complete && row.phase != AllocationPhase::Retained {
                store.confirm_stopped(&row.request.id, true)?;
            }
        } else if row.reservation_deadline_ms < beale_fleet::allocation::Allocator::now() {
            store.confirm_stopped(&row.request.id, true)?;
        }
    }
    let allocations = store.allocations()?;
    for mut environment in store.environments()?.into_iter().filter(|e| {
        allocations
            .iter()
            .any(|a| a.request.instance == e.instance && a.request.domain == "native")
    }) {
        if environment.deletion == DeletionStep::Deleted {
            continue;
        }
        if environment.deletion == DeletionStep::Retained {
            match store.retention(
                &environment.instance,
                environment.generation,
                RetentionAction::ClaimDelete { now: false },
                beale_fleet::allocation::Allocator::now(),
            ) {
                Ok(claim) => environment = claim,
                Err(_) => continue,
            }
        }
        let Some(_owner) =
            super::restart::lock_at(state.workspaces.root(), environment.instance.parse()?)?
        else {
            continue;
        };
        let result = (|| -> Result<()> {
            if environment.deletion == DeletionStep::Claimed {
                beale_storage::swarm_files::ResearchFiles::open(
                    state.db.clone(),
                    state.workspaces.root(),
                )?
                .delete_private_environment(&environment.scan, &environment.agent)?;
                store.deletion_step(
                    &environment.instance,
                    environment.generation,
                    DeletionStep::ContainerRemoved,
                )?;
                environment.deletion = DeletionStep::ContainerRemoved;
            }
            if environment.deletion == DeletionStep::ContainerRemoved {
                store.deletion_step(
                    &environment.instance,
                    environment.generation,
                    DeletionStep::VolumeRemoved,
                )?;
            }
            store.deletion_step(
                &environment.instance,
                environment.generation,
                DeletionStep::Deleted,
            )
        })();
        if let Err(error) = result {
            beale_host_log::error!("Native environment cleanup remains pending: {error:#}");
        }
    }
    Ok(())
}
