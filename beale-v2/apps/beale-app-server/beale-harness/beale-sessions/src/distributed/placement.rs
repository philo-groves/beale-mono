//! Selected placement uses owning-host reservations and the peer supervisor.
use super::peer_coordinator::PeerWorkerCoordinator;
use crate::SessionState;
use anyhow::{Context, Result, ensure};
use beale_contracts::swarm_host::ExecutionTarget;
use beale_contracts::{
    swarm_allocation::*,
    swarm_local::Admission,
    swarm_worker::WorkerShell,
    swarms::{AgentState, ExecutionState, Role},
};
use beale_fleet::{FleetCoordinationStore, FleetPeer, allocation::http::AllocationClient};
use beale_storage::{
    swarm_files::ResearchFiles,
    swarm_state::{LocalTurn, PeerPlacement, ScanState, SwarmStore},
};
use tokio::sync::watch;

#[cfg(test)]
mod tests;

fn init_intents(db: &beale_storage::HostDatabase) -> Result<()> {
    db.migrate_schema("swarm-placement", &["CREATE TABLE swarm_placement_intents(id TEXT PRIMARY KEY, host TEXT NOT NULL, spec TEXT NOT NULL);", "ALTER TABLE swarm_placement_intents ADD COLUMN selected INTEGER NOT NULL DEFAULT 1; ALTER TABLE swarm_placement_intents ADD COLUMN canceled INTEGER NOT NULL DEFAULT 0;"])?;
    db.migrate_schema(
        "swarm-placement-exports",
        &["CREATE TABLE swarm_placement_exports(id TEXT PRIMARY KEY, target TEXT NOT NULL);"],
    )
}
fn export_target(db: &beale_storage::HostDatabase, id: &str) -> Result<Option<ExecutionTarget>> {
    init_intents(db)?;
    db.with_connection(|db| {
        use beale_storage::rusqlite::OptionalExtension;
        let json: Option<String> = db
            .query_row(
                "SELECT target FROM swarm_placement_exports WHERE id=?1",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        json.map(|s| Ok(serde_json::from_str(&s)?)).transpose()
    })
}
fn save_intent(
    db: &beale_storage::HostDatabase,
    host: &str,
    spec: &AllocationSpec,
    source: Option<&ExecutionTarget>,
) -> Result<()> {
    init_intents(db)?;
    db.with_connection(|db| {
        let tx = db.transaction()?;
        tx.execute(
            "INSERT INTO swarm_placement_intents(id,host,spec,selected) VALUES(?1,?2,?3,?4)",
            beale_storage::rusqlite::params![
                spec.id,
                host,
                serde_json::to_string(spec)?,
                source.is_none()
            ],
        )?;
        if let Some(target) = source {
            tx.execute(
                "INSERT INTO swarm_placement_exports VALUES(?1,?2)",
                beale_storage::rusqlite::params![spec.id, serde_json::to_string(target)?],
            )?;
        }
        tx.commit()?;
        Ok(())
    })
}

fn peer(state: &SessionState, host: &str) -> Result<FleetPeer> {
    FleetCoordinationStore::open(state.db.clone())?
        .peer(host)?
        .context("selected Fleet host is not paired")
}
pub(super) fn input_bytes(state: &SessionState, scan: &str) -> Result<u64> {
    let (_, manifest) =
        ResearchFiles::open(state.db.clone(), state.workspaces.root())?.snapshot(scan)?;
    manifest.entries.iter().try_fold(0u64, |sum, e| {
        sum.checked_add(match &e.kind {
            beale_contracts::swarm_files::EntryKind::File { size, .. } => *size,
            _ => 0,
        })
        .context("snapshot size overflow")
    })
}
fn spec(
    saved: &ScanState,
    leader: &str,
    hierarchy: (&str, &str),
    id: String,
    configuration: &str,
    input_bytes: u64,
    processing_result: bool,
) -> AllocationSpec {
    AllocationSpec {
        id,
        leader: leader.into(),
        scan: saved.config.scan_id.clone(),
        agent: hierarchy.0.into(),
        parent: hierarchy.1.into(),
        configuration: configuration.into(),
        input_bytes,
        storage_reserve: saved.config.retention.storage_reserve_bytes,
        processing_result,
        restore: None,
    }
}
pub(crate) async fn admission(state: &SessionState, turn: &LocalTurn) -> Admission {
    let result=async{
        let saved=SwarmStore::open(state.db.clone())?.load(&turn.scan_id,true)?;
        if saved.config.destinations.is_empty(){return super::local_allocation::admission(state,turn,&saved);}
        ensure!(super::canonical_allocation::feasible(state,&saved)?,"Leader artifact storage cannot preserve its configured reserve");
        ensure!(state.fleet_origin.is_some(),"selected execution leader origin unavailable");
        let leader=beale_storage::fleet_executions::FleetExecutionStore::open(state.db.clone())?.host_id()?;
        let bytes=input_bytes(state,&turn.scan_id)?;
        let mut release=false;
        let deadline=tokio::time::Instant::now()+std::time::Duration::from_secs(10);
        for d in &saved.config.destinations {
            if tokio::time::Instant::now()>=deadline {break;}
            let query=spec(&saved,&leader,("prospective-child",&turn.agent_id),uuid::Uuid::new_v4().to_string(),&d.configuration_id,bytes,false);
            let Ok(paired)=peer(state,&d.host_id) else {continue;};
            let Ok(client)=AllocationClient::new(&paired) else {continue;};
            let response=match tokio::time::timeout_at(deadline.min(tokio::time::Instant::now()+std::time::Duration::from_secs(2)),client.send(&HostAllocationRequest::Feasibility{spec:query})).await{Ok(Ok(response))=>response,_=>continue};
            match response{
                HostAllocationReply::Feasibility{decision:Feasibility::Ready}=>return Ok(Admission::Ready),
                HostAllocationReply::Feasibility{decision:Feasibility::ReleaseParent{..}}=>release=true,
                _=>(),
            }
        }
        Ok(if release && turn.role==Role::Deputy {Admission::YieldParent{reason:"Queued until your checkpoint and confirmed suspension release execution capacity".into()}}else{Admission::Denied{reason:"No selected destination has sufficient measured RAM/storage. Stop lingering subagents using swarm_stop_subagent or add capacity; local fallback is disabled.".into()}})
    }.await;
    result.unwrap_or_else(|error:anyhow::Error|{beale_host_log::error!("Swarm feasibility unavailable: {error:#}");Admission::Denied{reason:"Selected destination capacity is unknown; check Fleet connectivity, runtime readiness, and RAM/storage measurements.".into()}})
}

/// True means a queued turn was processed, including a durable blocked result.
pub(crate) async fn run_pending(
    state: &SessionState,
    saved: &ScanState,
    agent: &str,
    stop: Option<watch::Receiver<bool>>,
) -> Result<bool> {
    let store = SwarmStore::open(state.db.clone())?;
    let local = store.local_state(&saved.config.scan_id)?;
    let a = saved.agent(agent)?;
    let control = local
        .controls
        .iter()
        .find(|c| c.agent_id == agent)
        .context("missing agent control")?;
    if a.summary.role == Role::Leader
        || control.stop_requested
        || control.failures >= 3
        || saved
            .attempts
            .iter()
            .any(|t| t.agent_id == agent && t.state != ExecutionState::Stopped)
    {
        return Ok(false);
    }
    if let Some(parent) = &control.release_parent {
        if local
            .controls
            .iter()
            .any(|c| &c.agent_id == parent && c.failures >= 3)
        {
            store.allocation_blocked(&saved.config.scan_id,agent,"Parent checkpoint/release recovery exhausted; this child did not acquire execution capacity")?;
            return Ok(true);
        }
        if saved
            .attempts
            .iter()
            .any(|a| &a.agent_id == parent && a.state != ExecutionState::Stopped)
            || !local
                .controls
                .iter()
                .any(|c| &c.agent_id == parent && c.yield_committed)
        {
            return Ok(false);
        }
    }
    let processing_result = local
        .inbox
        .iter()
        .any(|m| m.to == agent && m.sequence > a.checkpoint.inbox_cursor);
    if !processing_result && a.summary.state != AgentState::Queued {
        return Ok(false);
    }
    if let Some(attempt) = saved
        .attempts
        .iter()
        .filter(|a| a.agent_id == agent)
        .max_by_key(|a| a.generation)
        && !store.peer_worker(&attempt.id)?.complete
    {
        return Ok(false);
    }
    let result = run_queued(state, saved, agent, processing_result, stop).await;
    match result {
        Ok(processed) => return Ok(processed),
        Err(error) => {
            beale_host_log::error!("Selected Swarm allocation/recovery remains pending: {error:#}");
            if let Err(error) = cancel_unstarted(state, &saved.config.scan_id, agent).await {
                beale_host_log::error!("Unstarted placement cleanup remains protected: {error:#}");
            }
            let current = store.load(&saved.config.scan_id, true)?;
            if !current
                .attempts
                .iter()
                .any(|a| a.agent_id == agent && a.state != ExecutionState::Stopped)
            {
                store.allocation_blocked(&saved.config.scan_id,agent,"Selected execution could not be allocated or recovered within its bounded wait. Retained files remain protected; check Fleet readiness and use swarm_stop_subagent for lingering work.")?;
            }
        }
    }
    Ok(true)
}

/// A failed handoff can release its source protection only after the destination
/// confirms that this reservation never acquired execution ownership.
async fn cancel_unstarted(state: &SessionState, scan: &str, agent: &str) -> Result<()> {
    init_intents(&state.db)?;
    let ids = state.db.with_connection(|db| {
        let mut q = db.prepare("SELECT id FROM swarm_placement_intents WHERE json_extract(spec,'$.scan')=?1 AND json_extract(spec,'$.agent')=?2 ORDER BY rowid")?;
        Ok(q.query_map(beale_storage::rusqlite::params![scan,agent], |r| r.get::<_,String>(0))?.collect::<beale_storage::rusqlite::Result<Vec<_>>>()?)
    })?;
    let mut failed = false;
    for id in ids {
        if let Err(error) = cancel_intent(state, &id).await {
            failed = true;
            beale_host_log::error!("Placement cancellation remains pending: {error:#}");
        }
    }
    ensure!(!failed, "some placement cancellations remain pending");
    Ok(())
}
async fn cancel_intent(state: &SessionState, id: &str) -> Result<()> {
    // Fence destination selection before reading its current route. A source
    // export alone does not authorize an Enqueue on the provisional host.
    let (host, json, selected) = state.db.with_connection(|db| {
        let tx = db.transaction()?;
        tx.execute(
            "UPDATE swarm_placement_intents SET canceled=1 WHERE id=?1",
            [id],
        )?;
        let row = tx.query_row(
            "SELECT host,spec,selected FROM swarm_placement_intents WHERE id=?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, bool>(2)?,
                ))
            },
        )?;
        tx.commit()?;
        Ok(row)
    })?;
    let spec: AllocationSpec = serde_json::from_str(&json)?;
    if selected {
        let mut paired = peer(state, &host)?;
        paired.enabled = true;
        let reply = send_retry(
            &AllocationClient::new(&paired)?,
            &HostAllocationRequest::Cancel { id: id.into() },
        )
        .await?;
        if !matches!(reply, HostAllocationReply::Complete) {
            return Ok(());
        }
    }
    if let Some(target) = export_target(&state.db, id)? {
        cancel_export(state, &target, id).await?;
    }
    let ledger = beale_storage::fleet_allocation::AllocationStore::open(state.db.clone())?;
    let central = format!("central-{}", spec.id);
    if let Some(row) = ledger.allocation_if_present(&central)?
        && row.execution.is_none()
    {
        ledger.release_canonical_storage(&central)?;
    }
    Ok(())
}

/// Finalize only after probing actual retained-state compatibility. Once the
/// route is selected, all uncertain mutations remain pinned to that host.
async fn select_destination(
    state: &SessionState,
    saved: &ScanState,
    id: &str,
) -> Result<(String, AllocationSpec)> {
    let (host, json, selected, canceled) = state.db.with_connection(|db| {
        Ok(db.query_row(
            "SELECT host,spec,selected,canceled FROM swarm_placement_intents WHERE id=?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, bool>(2)?,
                    r.get::<_, bool>(3)?,
                ))
            },
        )?)
    })?;
    ensure!(!canceled, "placement was canceled");
    let specification: AllocationSpec = serde_json::from_str(&json)?;
    if selected {
        return Ok((host, specification));
    }
    let bundle = specification
        .restore
        .as_ref()
        .context("pending destination requires an exported bundle")?;
    let mut waiting = None;
    let mut chosen = None;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    for destination in &saved.config.destinations {
        if destination.host_id == bundle.target.host_id {
            continue;
        }
        let Ok(paired) = peer(state, &destination.host_id) else {
            continue;
        };
        let Ok(client) = AllocationClient::new(&paired) else {
            continue;
        };
        let mut query = specification.clone();
        query.configuration = destination.configuration_id.clone();
        let response = tokio::time::timeout_at(
            deadline.min(tokio::time::Instant::now() + std::time::Duration::from_secs(2)),
            client.send(&HostAllocationRequest::Feasibility {
                spec: query.clone(),
            }),
        )
        .await;
        match response {
            Ok(Ok(HostAllocationReply::Feasibility {
                decision: Feasibility::Ready | Feasibility::ReleaseParent { .. },
            })) => {
                chosen = Some((destination.host_id.clone(), query));
                break;
            }
            Ok(Ok(HostAllocationReply::Feasibility {
                decision: Feasibility::Denied { .. },
            })) if waiting.is_none() => {
                waiting = Some((destination.host_id.clone(), query));
            }
            _ => (),
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
    }
    let (host, specification) = chosen
        .or(waiting)
        .context("no reachable compatible selected destination")?;
    state.db.with_connection(|db| {
        ensure!(db.execute("UPDATE swarm_placement_intents SET host=?2,spec=?3,selected=1 WHERE id=?1 AND selected=0 AND canceled=0", beale_storage::rusqlite::params![id,host,serde_json::to_string(&specification)?])? == 1, "placement selection was canceled or already finalized");
        Ok(())
    })?;
    Ok((host, specification))
}

async fn run_queued(
    state: &SessionState,
    saved: &ScanState,
    agent: &str,
    processing_result: bool,
    mut stop: Option<watch::Receiver<bool>>,
) -> Result<bool> {
    let store = SwarmStore::open(state.db.clone())?;
    let generation = saved
        .attempts
        .iter()
        .filter(|a| a.agent_id == agent)
        .map(|a| a.generation)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .context("generation exhausted")?;
    let id = format!(
        "allocation-{agent}-{generation}-{}",
        saved.agent(agent)?.summary.revision
    );
    let parent = saved
        .agent(agent)?
        .summary
        .parent_id
        .as_deref()
        .context("subordinate parent missing")?;
    let leader =
        beale_storage::fleet_executions::FleetExecutionStore::open(state.db.clone())?.host_id()?;
    let bytes = input_bytes(state, &saved.config.scan_id)?;
    // Durable destination intent precedes network mutation. An uncertain reply
    // is retried only on this host, never raced against another destination.
    init_intents(&state.db)?;
    let intent: Option<(String, String)> = state.db.with_connection(|db| {
        use beale_storage::rusqlite::OptionalExtension;
        Ok(db
            .query_row(
                "SELECT host,spec FROM swarm_placement_intents WHERE id=?1",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    })?;
    let (_, mut specification) = if let Some((host, json)) = intent {
        (host, serde_json::from_str::<AllocationSpec>(&json)?)
    } else {
        let mut chosen = None;
        let previous = saved
            .attempts
            .iter()
            .filter(|a| a.agent_id == agent)
            .max_by_key(|a| a.generation)
            .map(|a| store.peer_worker(&a.id))
            .transpose()?;
        let mut destinations = saved.config.destinations.iter().collect::<Vec<_>>();
        destinations.sort_by_key(|d| {
            previous
                .as_ref()
                .is_none_or(|p| p.target.host_id != d.host_id)
        });
        let mut waiting = None;
        for d in destinations {
            let Ok(paired) = peer(state, &d.host_id) else {
                continue;
            };
            let Ok(client) = AllocationClient::new(&paired) else {
                continue;
            };
            let query = spec(
                saved,
                &leader,
                (agent, parent),
                id.clone(),
                &d.configuration_id,
                bytes,
                processing_result,
            );
            if let Some(previous) = &previous {
                if d.host_id != previous.target.host_id {
                    ensure!(
                        previous.terminated() && previous.complete,
                        "original execution must finish checkpoint recovery before moving"
                    );
                    // Compatibility is checked with the actual bundle after
                    // the destination and export source are durable.
                    chosen = Some((d.host_id.clone(), query));
                    break;
                }
                if d.configuration_id != previous.target.configuration_id {
                    continue;
                }
            }
            match client
                .send(&HostAllocationRequest::Feasibility {
                    spec: query.clone(),
                })
                .await
            {
                Ok(HostAllocationReply::Feasibility {
                    decision: Feasibility::Ready | Feasibility::ReleaseParent { .. },
                }) => {
                    chosen = Some((d.host_id.clone(), query));
                    break;
                }
                Ok(HostAllocationReply::Feasibility {
                    decision: Feasibility::Denied { .. },
                }) if waiting.is_none() => {
                    // Work was already accepted. Capacity contention belongs
                    // in the owning host's bounded fair queue.
                    waiting = Some((d.host_id.clone(), query));
                }
                _ => (),
            }
        }
        let (host, query) = chosen
            .or(waiting)
            .context("no reachable selected placement")?;
        let source = previous
            .as_ref()
            .filter(|p| p.target.host_id != host)
            .map(|p| &p.target);
        save_intent(&state.db, &host, &query, source)?;
        (host, query)
    };
    export_intent(state, &id, &mut specification).await?;
    let (host, specification) = select_destination(state, saved, &id).await?;
    let paired = peer(state, &host)?;
    let Some(central) =
        super::canonical_allocation::reserve(state, saved, agent, &id, &stop).await?
    else {
        return Ok(false);
    };
    let client = AllocationClient::new(&paired)?;
    sync_host(&state.db, saved, &client).await?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
    let allocation = loop {
        if crate::engine::stop_requested(&stop)
            || store
                .local_state(&saved.config.scan_id)?
                .controls
                .iter()
                .any(|c| c.agent_id == agent && c.stop_requested)
        {
            let _ = client
                .send(&HostAllocationRequest::Cancel { id: id.clone() })
                .await;
            anyhow::bail!("queued allocation canceled");
        }
        match client
            .send(&HostAllocationRequest::Enqueue {
                spec: specification.clone(),
            })
            .await
        {
            Ok(HostAllocationReply::Allocation { allocation })
                if allocation.prepared && allocation.phase == AllocationPhase::Reserved =>
            {
                if let Some(bundle) = &specification.restore {
                    release_restore(state, bundle, &id).await?;
                }
                break allocation;
            }
            Ok(HostAllocationReply::Allocation { allocation })
                if allocation.phase == AllocationPhase::Reserved
                    && specification.restore.is_some() =>
            {
                restore(
                    state,
                    &client,
                    &id,
                    specification
                        .restore
                        .as_ref()
                        .context("restore intent missing")?,
                )
                .await?;
                // Transfer has its own bounded deadline and renewed leases.
                // Inspect its prepared result before applying queue timeout.
                continue;
            }
            Ok(HostAllocationReply::Allocation { allocation })
                if matches!(
                    allocation.phase,
                    AllocationPhase::Blocked
                        | AllocationPhase::Canceled
                        | AllocationPhase::Retained
                ) =>
            {
                anyhow::bail!("allocation blocked or expired")
            }
            Ok(HostAllocationReply::Rejected { reason }) => anyhow::bail!(reason),
            _ => (),
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "allocation recovery exhausted"
        );
        if let Some(stop) = &mut stop {
            tokio::select! {_=stop.changed()=>(),_=tokio::time::sleep(std::time::Duration::from_secs(2))=>()}
        } else {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    };
    let placement = PeerPlacement {
        leader_id: leader,
        host_id: host,
        configuration_id: allocation.request.configuration,
        configuration_revision: allocation.request.configuration_revision,
        instance_id: allocation.request.instance,
        shell: WorkerShell::Bash,
    };
    let coordinator = PeerWorkerCoordinator::new(
        state.db.clone(),
        state.workspaces.root().to_path_buf(),
        &paired,
        state
            .fleet_origin
            .clone()
            .context("reachable leader origin unavailable")?,
    )?;
    let Some(mut owned) = coordinator.claim(&saved.config.scan_id, agent, 64, &placement)? else {
        return Ok(true);
    };
    beale_storage::fleet_allocation::AllocationStore::open(state.db.clone())?
        .bind_canonical_storage(&central.id, &owned.record().target)?;
    // The durable execution record now protects publication storage. Before
    // this point the placement owner covers queueing, Busy waits, and transfer.
    drop(central);
    let (sender, receiver) = watch::channel(false);
    let receiver = stop.unwrap_or(receiver);
    let snapshot = if generation == 1 {
        Some(
            ResearchFiles::open(state.db.clone(), state.workspaces.root())?
                .snapshot(&saved.config.scan_id)?
                .0,
        )
    } else {
        None
    };
    coordinator
        .run(&mut owned, snapshot, receiver, |target, _| {
            checkpoint(&state.db, state.workspaces.root(), target)
        })
        .await?;
    drop(sender);
    Ok(true)
}

pub(crate) async fn checkpoint(
    db: &beale_storage::HostDatabase,
    root: &std::path::Path,
    target: beale_contracts::swarm_host::ExecutionTarget,
) -> Result<()> {
    let mut peer = FleetCoordinationStore::open(db.clone())?
        .peer(&target.host_id)?
        .context("checkpoint peer pairing unavailable")?;
    peer.enabled = true; // Disabling new placement must not disable recovery.
    let store = SwarmStore::open(db.clone())?;
    let record = store.peer_worker(&target.identity.attempt_id)?;
    ensure!(
        record.target == target && record.terminated(),
        "remote checkpoint lacks exact termination proof"
    );
    let client = AllocationClient::new(&peer)?;
    let HostAllocationReply::Bundle { bundle } = client
        .send(&HostAllocationRequest::Checkpoint {
            target: target.clone(),
        })
        .await?
    else {
        anyhow::bail!("checkpoint remains pending")
    };
    ensure!(bundle.target == target, "checkpoint execution mismatch");
    let recovery = root
        .join("remote-recovery")
        .join(beale_fleet::allocation::archives::bundle_id(&bundle)?);
    let archive = recovery.join("state.tar");
    client
        .download(&bundle, ArchiveKind::State, &archive)
        .await?;
    let extracted = recovery.join("extracted");
    if !extracted.join("complete").try_exists()? {
        // Failed extraction stays private; only a fully verified tree is used.
        let staging = recovery.join(uuid::Uuid::new_v4().to_string());
        beale_fleet::allocation::archives::extract_recovery(
            &archive,
            &staging,
            &target.identity.attempt_id,
        )?;
        std::fs::write(staging.join("complete"), b"verified")?;
        std::fs::rename(&staging, &extracted)?;
    }
    // Confirmed stop fences execution, but the peer recovery record and host
    // allocation still block reuse until publication and checkpoint acceptance.
    store.fence_attempt(&target.identity.scan_id, &target.identity.attempt_id, false)?;
    super::recovery::publications_at(
        db,
        root,
        &target.identity,
        &extracted.join("state").join(&target.identity.attempt_id),
    )?;
    match client
        .send(&HostAllocationRequest::CheckpointAccepted {
            target,
            digest: beale_fleet::allocation::archives::bundle_digest(&bundle)?,
        })
        .await?
    {
        HostAllocationReply::Complete => super::canonical_allocation::released(db, &bundle.target),
        _ => anyhow::bail!("checkpoint acknowledgement remains pending"),
    }
}

async fn export_intent(
    state: &SessionState,
    id: &str,
    specification: &mut AllocationSpec,
) -> Result<()> {
    let canceled: bool = state.db.with_connection(|db| {
        Ok(db.query_row(
            "SELECT canceled FROM swarm_placement_intents WHERE id=?1",
            [id],
            |r| r.get(0),
        )?)
    })?;
    ensure!(!canceled, "placement was canceled");
    if specification.restore.is_none()
        && let Some(target) = export_target(&state.db, id)?
    {
        let mut source = peer(state, &target.host_id)?;
        source.enabled = true;
        let reply = send_retry(
            &AllocationClient::new(&source)?,
            &HostAllocationRequest::Export {
                target: target.clone(),
                transfer: id.into(),
            },
        )
        .await?;
        let HostAllocationReply::Bundle { bundle } = reply else {
            anyhow::bail!("retained root/files unavailable; refusing a clean substitute")
        };
        ensure!(bundle.target == target, "restore source identity changed");
        specification.restore = Some(bundle);
        state.db.with_connection(|db| {
            db.execute(
                "UPDATE swarm_placement_intents SET spec=?2 WHERE id=?1",
                beale_storage::rusqlite::params![id, serde_json::to_string(&specification)?],
            )?;
            Ok(())
        })?;
    }
    Ok(())
}

async fn restore(
    state: &SessionState,
    destination: &AllocationClient,
    id: &str,
    bundle: &EnvironmentBundle,
) -> Result<()> {
    tokio::time::timeout(
        std::time::Duration::from_secs(1800),
        restore_transfer(state, destination, id, bundle),
    )
    .await
    .context("restore transfer deadline exhausted")?
}
async fn restore_transfer(
    state: &SessionState,
    destination: &AllocationClient,
    id: &str,
    bundle: &EnvironmentBundle,
) -> Result<()> {
    let mut source = peer(state, &bundle.target.host_id)?;
    source.enabled = true;
    let source = AllocationClient::new(&source)?;
    ensure!(
        matches!(
            send_retry(
                destination,
                &HostAllocationRequest::BeginRestore {
                    allocation: id.into(),
                    bundle: bundle.clone()
                }
            )
            .await?,
            HostAllocationReply::Complete
        ),
        "destination restore rejected"
    );
    // The caller owns the leader's canonical reservation through binding.
    // Only the remote unstarted reservation needs transfer-lease renewal.
    let mut renewed = tokio::time::Instant::now();
    for archive in &bundle.archives {
        let mut offset = 0;
        while offset < archive.bytes {
            if renewed.elapsed() >= std::time::Duration::from_secs(30) {
                ensure!(
                    matches!(
                        send_retry(
                            destination,
                            &HostAllocationRequest::BeginRestore {
                                allocation: id.into(),
                                bundle: bundle.clone()
                            }
                        )
                        .await?,
                        HostAllocationReply::Complete
                    ),
                    "restore lease renewal rejected"
                );
                renewed = tokio::time::Instant::now();
            }
            let reply = send_retry(
                &source,
                &HostAllocationRequest::ReadArchive {
                    checkpoint: beale_fleet::allocation::archives::bundle_id(bundle)?,
                    kind: archive.kind,
                    offset,
                },
            )
            .await?;
            let HostAllocationReply::Archive {
                offset: received,
                bytes,
            } = reply
            else {
                anyhow::bail!("retained source transfer unavailable")
            };
            ensure!(
                received == offset
                    && !bytes.is_empty()
                    && bytes.len() <= beale_fleet::allocation::archives::CHUNK
                    && offset + bytes.len() as u64 <= archive.bytes,
                "invalid retained archive chunk"
            );
            let next = offset + bytes.len() as u64;
            ensure!(
                matches!(send_retry(destination, &HostAllocationRequest::WriteArchive{allocation:id.into(),kind:archive.kind,offset,bytes}).await?,HostAllocationReply::Archive{offset,..} if offset==next),
                "destination restore write was not acknowledged"
            );
            offset = next;
        }
    }
    ensure!(
        matches!(
            send_retry(
                destination,
                &HostAllocationRequest::BeginRestore {
                    allocation: id.into(),
                    bundle: bundle.clone()
                }
            )
            .await?,
            HostAllocationReply::Complete
        ),
        "restore lease renewal rejected"
    );
    ensure!(
        matches!(
            send_retry(
                destination,
                &HostAllocationRequest::FinishRestore {
                    allocation: id.into()
                }
            )
            .await?,
            HostAllocationReply::Complete
        ),
        "retained environment restore failed"
    );
    Ok(())
}
async fn release_restore(
    state: &SessionState,
    bundle: &EnvironmentBundle,
    transfer: &str,
) -> Result<()> {
    cancel_export(state, &bundle.target, transfer).await
}
async fn cancel_export(
    state: &SessionState,
    target: &ExecutionTarget,
    transfer: &str,
) -> Result<()> {
    let mut paired = peer(state, &target.host_id)?;
    paired.enabled = true;
    ensure!(
        matches!(
            send_retry(
                &AllocationClient::new(&paired)?,
                &HostAllocationRequest::CancelExport {
                    target: target.clone(),
                    transfer: transfer.into(),
                }
            )
            .await?,
            HostAllocationReply::Complete
        ),
        "source export cancellation pending"
    );
    Ok(())
}

/// Replay the same immutable request after transport failure, including a lost
/// mutation acknowledgement. Semantic rejection remains explicit and bounded.
async fn send_retry(
    client: &AllocationClient,
    request: &HostAllocationRequest,
) -> Result<HostAllocationReply> {
    send_retry_until(
        client,
        request,
        tokio::time::Instant::now() + std::time::Duration::from_secs(660),
    )
    .await
}
async fn send_retry_until(
    client: &AllocationClient,
    request: &HostAllocationRequest,
    deadline: tokio::time::Instant,
) -> Result<HostAllocationReply> {
    let mut failures = 0;
    loop {
        match tokio::time::timeout_at(deadline, client.send(request))
            .await
            .context("allocation operation remained busy beyond its recovery deadline")?
        {
            Ok(HostAllocationReply::Busy {}) => (),
            Ok(reply) => return Ok(reply),
            Err(error) => {
                failures += 1;
                if failures >= 3 {
                    return Err(error);
                }
            }
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "allocation operation remained busy beyond its recovery deadline"
        );
        tokio::time::sleep_until(
            deadline.min(tokio::time::Instant::now() + std::time::Duration::from_millis(250)),
        )
        .await;
    }
}

pub(crate) async fn sync_host(
    db: &beale_storage::HostDatabase,
    saved: &ScanState,
    client: &AllocationClient,
) -> Result<()> {
    let lifecycle = ScanLifecycle {
        leader: beale_storage::fleet_executions::FleetExecutionStore::open(db.clone())?
            .host_id()?,
        scan: saved.config.scan_id.clone(),
        revision: saved.revision,
        ended_at_ms: saved.ended_at_ms,
        retention_seconds: saved.config.retention.ended_scan_seconds,
    };
    ensure!(
        matches!(
            client
                .send(&HostAllocationRequest::ScanLifecycle { lifecycle })
                .await?,
            HostAllocationReply::Complete
        ),
        "destination lifecycle update failed"
    );
    Ok(())
}
pub(crate) async fn sync_lifecycle(state: &SessionState, saved: &ScanState) -> Result<()> {
    let leader =
        beale_storage::fleet_executions::FleetExecutionStore::open(state.db.clone())?.host_id()?;
    beale_storage::fleet_allocation::AllocationStore::open(state.db.clone())?.scan_lifecycle(
        &ScanLifecycle {
            leader,
            scan: saved.config.scan_id.clone(),
            revision: saved.revision,
            ended_at_ms: saved.ended_at_ms,
            retention_seconds: saved.config.retention.ended_scan_seconds,
        },
        beale_fleet::allocation::Allocator::now(),
    )?;
    let mut hosts = std::collections::BTreeSet::new();
    init_intents(&state.db)?;
    let intents = state.db.with_connection(|db| {
        let mut q = db.prepare("SELECT i.host,e.target FROM swarm_placement_intents i LEFT JOIN swarm_placement_exports e ON e.id=i.id WHERE json_extract(i.spec,'$.scan')=?1")?;
        Ok(q.query_map([&saved.config.scan_id], |r| Ok((r.get::<_,String>(0)?, r.get::<_,Option<String>>(1)?)))?.collect::<beale_storage::rusqlite::Result<Vec<_>>>()?)
    })?;
    for (host, source) in intents {
        hosts.insert(host);
        if let Some(source) = source {
            hosts.insert(serde_json::from_str::<ExecutionTarget>(&source)?.host_id);
        }
    }
    for attempt in &saved.attempts {
        if saved.agent(&attempt.agent_id)?.summary.role == Role::Leader
            || saved.config.destinations.is_empty()
        {
            continue;
        }
        hosts.insert(
            SwarmStore::open(state.db.clone())?
                .peer_worker(&attempt.id)?
                .target
                .host_id,
        );
    }
    let mut failed = false;
    for host in hosts {
        let result = async {
            let mut paired = peer(state, &host)?;
            paired.enabled = true;
            sync_host(&state.db, saved, &AllocationClient::new(&paired)?).await
        }
        .await;
        if result.is_err() {
            failed = true;
        }
    }
    ensure!(
        !failed,
        "some retained destinations require lifecycle reconciliation"
    );
    Ok(())
}
pub(crate) async fn lifecycle_maintenance(state: &SessionState) -> Result<()> {
    super::canonical_allocation::reconcile(state)?;
    let ids = state.db.with_connection(|db| {
        let mut q = db.prepare("SELECT id FROM swarm_scans ORDER BY id")?;
        Ok(q.query_map([], |r| r.get::<_, String>(0))?
            .collect::<beale_storage::rusqlite::Result<Vec<_>>>()?)
    })?;
    for id in ids {
        let store = SwarmStore::open(state.db.clone())?;
        let saved = match store.load(&id, true) {
            Ok(saved) => saved,
            Err(error) => {
                beale_host_log::error!("Retained scan state requires repair: {error:#}");
                continue;
            }
        };
        if !saved.config.destinations.is_empty()
            && let Ok(local) = store.local_state(&id)
        {
            for control in local
                .controls
                .iter()
                .filter(|c| c.stop_requested || c.failures >= 3)
            {
                if let Err(error) = cancel_unstarted(state, &id, &control.agent_id).await {
                    beale_host_log::error!(
                        "Unstarted placement cleanup remains protected: {error:#}"
                    );
                }
            }
        }
        if let Err(error) = sync_lifecycle(state, &saved).await {
            beale_host_log::error!("Retained scan lifecycle remains pending: {error:#}");
        }
    }
    Ok(())
}
