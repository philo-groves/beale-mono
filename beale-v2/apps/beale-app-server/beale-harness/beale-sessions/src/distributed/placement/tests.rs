use super::*;
use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::post};
use beale_contracts::{swarm_container::*, swarm_worker::ExecutionIdentity, swarms::*};
use beale_fleet::{
    allocation::{Allocator, CapacityFuture, CapacitySource, http},
    docker::*,
};
use beale_storage::{
    HostDatabase,
    swarm_state::{LocalModel, LocalSettings},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

struct Fixed;
impl CapacitySource for Fixed {
    fn measure<'a>(&'a self, _: &'a DockerEnvironments, _: &'a [Allocation]) -> CapacityFuture<'a> {
        Box::pin(async {
            Ok(Capacity {
                domain: "docker".into(),
                observed_at_ms: Allocator::now(),
                available_memory: RAM_FLOOR + DEFAULT_MEMORY + 1,
                storage: BTreeMap::from([("disk-example".into(), 128 * 1024 * 1024 * 1024)]),
                measured_memory: BTreeMap::new(),
                measured_storage: BTreeSet::new(),
            })
        })
    }
}
fn allocator(root: &Path) -> Allocator {
    let db = HostDatabase::open(root).unwrap();
    let env = DockerEnvironments::new(
        Docker::new(
            std::env::var_os("BEALE_DOCKER_CLI")
                .map(PathBuf::from)
                .unwrap_or_else(|| "docker".into()),
        ),
        DockerStore::open(db.clone()).unwrap(),
        root.into(),
        std::env::var_os("BEALE_CONTAINER_RUNTIME").map(PathBuf::from),
    )
    .unwrap();
    env.store
        .save_configuration(ImageConfiguration {
            version: 1,
            id: "image-example".into(),
            revision: 1,
            image: "alpine:3.22".into(),
            architecture: LinuxArchitecture::Amd64,
            options: ContainerOptions::default(),
        })
        .unwrap();
    Allocator::new(db, env, Arc::new(Fixed)).unwrap()
}
fn state(root: &Path) -> SessionState {
    crate::build_state(
        "synthetic-token".into(),
        beale_workspaces::WorkspaceStore::at(root.to_path_buf()).unwrap(),
    )
    .unwrap()
}
fn scan(state: &SessionState, host: &str) -> ScanState {
    let store = SwarmStore::open(state.db.clone()).unwrap();
    let saved = store
        .create_scan(
            ScanConfig {
                version: beale_contracts::swarms::VERSION,
                scan_id: "scan-example".into(),
                workspace_id: "workspace-example".into(),
                mode: ScanMode::Swarm {
                    snapshot: Box::new(beale_swarms::BASIC_SWARM.snapshot.clone()),
                },
                authorized_scope: "Synthetic fixture".into(),
                enabled_tools: vec![],
                provider_routes: vec![],
                destinations: vec![Destination {
                    host_id: host.into(),
                    configuration_id: "image-example".into(),
                }],
                pacing: vec![],
                retention: RetentionPolicy::default(),
            },
            true,
            true,
        )
        .unwrap();
    store
        .peer_initialize(
            &saved.config.scan_id,
            LocalSettings {
                models: vec![LocalModel {
                    profile: "leader".into(),
                    provider: 1,
                    method: 1,
                    model: "example-model".into(),
                }],
                reasoning_effort: 0,
            },
            "Coordinate synthetic work",
        )
        .unwrap();
    store.load(&saved.config.scan_id, true).unwrap()
}
async fn serve(
    state: &SessionState,
    host: &str,
    router: Router,
) -> (AllocationClient, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    // Synthetic loopback pairing; production remote pairings require Tailscale.
    FleetCoordinationStore::open(state.db.clone()).unwrap();
    state
        .db
        .with_connection(|db| {
            db.execute(
                "INSERT INTO fleet_coordination_peers VALUES(?1,?2,?3,1,?4)",
                beale_storage::rusqlite::params![
                    host,
                    endpoint,
                    "a".repeat(64),
                    beale_contracts::PROTOCOL_MAJOR
                ],
            )?;
            Ok(())
        })
        .unwrap();
    let client = AllocationClient::new(&peer(state, host).unwrap()).unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (client, task)
}
fn specification(state: &SessionState, saved: &ScanState, id: &str) -> AllocationSpec {
    let leader = beale_storage::fleet_executions::FleetExecutionStore::open(state.db.clone())
        .unwrap()
        .host_id()
        .unwrap();
    spec(
        saved,
        &leader,
        (&saved.agents[0].summary.id, &saved.agents[0].summary.id),
        id.into(),
        "image-example",
        0,
        false,
    )
}
async fn reserved(a: &Allocator, spec: &AllocationSpec) -> Allocation {
    http::handle(a, HostAllocationRequest::Enqueue { spec: spec.clone() })
        .await
        .unwrap();
    a.store
        .admit_next(&a.capacity().await.unwrap(), Allocator::now())
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn failed_export_and_lost_response_are_reconciled_after_restart_by_normal_maintenance() {
    for lost in [false, true] {
        let root =
            std::env::temp_dir().join(format!("beale-handoff-example-{}", uuid::Uuid::new_v4()));
        let leader = state(&root.join("leader"));
        let source = allocator(&root.join("source"));
        let destination = allocator(&root.join("destination"));
        let saved = scan(&leader, destination.environments.store.host());
        let mut old = specification(&leader, &saved, "old-example");
        let row = reserved(&source, &old).await;
        source.store.confirm_stopped(&old.id, true).unwrap();
        let target = ExecutionTarget {
            leader_id: old.leader.clone(),
            host_id: source.environments.store.host().into(),
            configuration_id: old.configuration.clone(),
            configuration_revision: 1,
            instance_id: row.request.instance,
            identity: ExecutionIdentity {
                workspace_id: saved.config.workspace_id.clone(),
                scan_id: old.scan.clone(),
                agent_id: old.agent.clone(),
                attempt_id: "attempt-example".into(),
                generation: 1,
            },
        };
        old.id = "handoff-example".into();
        save_intent(
            &leader.db,
            destination.environments.store.host(),
            &old,
            Some(&target),
        )
        .unwrap();
        let seen = Arc::new(AtomicUsize::new(0));
        let (a, db, hits) = (source.clone(), leader.db.clone(), seen.clone());
        let source_router = Router::new().route(
            PATH,
            post(move |Json(request): Json<HostAllocationRequest>| {
                let (a, db, hits) = (a.clone(), db.clone(), hits.clone());
                async move {
                    if let HostAllocationRequest::Export { target, transfer } = request {
                        assert_eq!(
                            export_target(&db, "handoff-example").unwrap(),
                            Some(target.clone())
                        );
                        hits.fetch_add(1, Ordering::SeqCst);
                        let e = a.store.environment(&target.instance_id).unwrap();
                        a.store
                            .retention(
                                &e.instance,
                                e.generation,
                                RetentionAction::Protect {
                                    reference: format!("restore-{transfer}"),
                                },
                                Allocator::now(),
                            )
                            .unwrap();
                        // Fault after the durable host mutation, before a usable reply.
                        if lost {
                            return StatusCode::BAD_GATEWAY.into_response();
                        }
                        return Json(HostAllocationReply::Rejected {
                            reason: "Synthetic export write failure".into(),
                        })
                        .into_response();
                    }
                    Json(http::handle(&a, request).await.unwrap()).into_response()
                }
            }),
        );
        let (_, source_task) =
            serve(&leader, source.environments.store.host(), source_router).await;
        let (_, destination_task) = serve(
            &leader,
            destination.environments.store.host(),
            http::router("a".repeat(64), destination.clone()),
        )
        .await;
        assert!(
            export_intent(&leader, &old.id.clone(), &mut old)
                .await
                .is_err()
        );
        assert_eq!(seen.load(Ordering::SeqCst), if lost { 3 } else { 1 });
        assert!(
            source
                .store
                .environment(&target.instance_id)
                .unwrap()
                .references
                .contains("restore-handoff-example")
        );
        SwarmStore::open(leader.db.clone())
            .unwrap()
            .local_stop(&old.scan, &old.agent)
            .unwrap();
        drop(leader);
        let restarted = state(&root.join("leader"));
        lifecycle_maintenance(&restarted).await.unwrap();
        assert!(
            source
                .store
                .environment(&target.instance_id)
                .unwrap()
                .references
                .is_empty()
        );
        assert!(
            source
                .store
                .export_canceled(&target, "handoff-example")
                .unwrap()
        );
        // Cancellation of a pre-selection intent fences any later routing;
        // no destination allocation was ever authorized.
        assert!(
            select_destination(&restarted, &saved, &old.id)
                .await
                .is_err()
        );
        assert!(
            destination
                .store
                .allocation_if_present(&old.id)
                .unwrap()
                .is_none()
        );
        source_task.abort();
        destination_task.abort();
        let _ = source_task.await;
        let _ = destination_task.await;
        drop(restarted);
        drop(source);
        drop(destination);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test]
async fn restore_retries_lost_chunk_acknowledgement_with_same_identity_and_offset() {
    let root =
        std::env::temp_dir().join(format!("beale-transfer-example-{}", uuid::Uuid::new_v4()));
    let leader = state(&root.join("leader"));
    let destination = allocator(&root.join("destination"));
    let saved = scan(&leader, destination.environments.store.host());
    let mut spec = specification(&leader, &saved, "transfer-example");
    let bytes = b"synthetic retained bytes".to_vec();
    let bundle = EnvironmentBundle {
        version: 1,
        target: ExecutionTarget {
            leader_id: spec.leader.clone(),
            host_id: "source-example".into(),
            configuration_id: spec.configuration.clone(),
            configuration_revision: 1,
            instance_id: uuid::Uuid::new_v4().to_string(),
            identity: ExecutionIdentity {
                workspace_id: saved.config.workspace_id.clone(),
                scan_id: spec.scan.clone(),
                agent_id: spec.agent.clone(),
                attempt_id: "attempt-example".into(),
                generation: 1,
            },
        },
        configuration: destination
            .environments
            .store
            .configuration("image-example")
            .unwrap()
            .configuration,
        image_digest: Some(format!("sha256:{}", "a".repeat(64))),
        archives: [
            ArchiveKind::Workspace,
            ArchiveKind::State,
            ArchiveKind::Image,
        ]
        .into_iter()
        .map(|kind| EnvironmentArchive {
            kind,
            sha256: beale_storage::swarm_files::hash(&bytes),
            bytes: bytes.len() as u64,
        })
        .collect(),
    };
    spec.restore = Some(bundle.clone());
    reserved(&destination, &spec).await;
    let _central =
        super::super::canonical_allocation::reserve(&leader, &saved, &spec.agent, &spec.id, &None)
            .await
            .unwrap()
            .unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let source_router = Router::new().route(
        PATH,
        post(move |Json(request): Json<HostAllocationRequest>| {
            let bytes = bytes.clone();
            let reads = reads.clone();
            async move {
                if reads.fetch_add(1, Ordering::SeqCst) == 0 {
                    return StatusCode::BAD_GATEWAY.into_response();
                }
                let HostAllocationRequest::ReadArchive { offset, .. } = request else {
                    panic!("unexpected source request")
                };
                Json(HostAllocationReply::Archive { offset, bytes }).into_response()
            }
        }),
    );
    let (_, source_task) = serve(&leader, "source-example", source_router).await;
    let writes = Arc::new(Mutex::new(Vec::new()));
    let begins = Arc::new(AtomicUsize::new(0));
    let finishes = Arc::new(AtomicUsize::new(0));
    let (a, observed, begin_hits, finish_hits) = (
        destination.clone(),
        writes.clone(),
        begins.clone(),
        finishes.clone(),
    );
    let destination_router = Router::new().route(
        PATH,
        post(move |Json(request): Json<HostAllocationRequest>| {
            let (a, observed, begin_hits, finish_hits) = (
                a.clone(),
                observed.clone(),
                begin_hits.clone(),
                finish_hits.clone(),
            );
            async move {
                if let HostAllocationRequest::FinishRestore { allocation } = &request {
                    assert_eq!(allocation, "transfer-example");
                    a.store.prepared(allocation).unwrap();
                    if finish_hits.fetch_add(1, Ordering::SeqCst) == 0 {
                        return StatusCode::BAD_GATEWAY.into_response();
                    }
                    return Json(HostAllocationReply::Complete).into_response();
                }
                let write = if let HostAllocationRequest::WriteArchive {
                    allocation,
                    kind,
                    offset,
                    ..
                } = &request
                {
                    Some((allocation.clone(), *kind, *offset))
                } else {
                    None
                };
                let begin = matches!(request, HostAllocationRequest::BeginRestore { .. });
                let reply = http::handle(&a, request).await.unwrap();
                if begin && begin_hits.fetch_add(1, Ordering::SeqCst) == 0 {
                    return StatusCode::BAD_GATEWAY.into_response();
                }
                if let Some(write) = write {
                    let mut observed = observed.lock().unwrap();
                    observed.push(write);
                    if observed.len() == 1 {
                        return StatusCode::BAD_GATEWAY.into_response();
                    }
                }
                Json(reply).into_response()
            }
        }),
    );
    let (client, destination_task) = serve(
        &leader,
        destination.environments.store.host(),
        destination_router,
    )
    .await;
    restore(&leader, &client, &spec.id, &bundle).await.unwrap();
    let observed = writes.lock().unwrap().clone();
    assert_eq!(observed.len(), 4);
    assert_eq!(observed[0], observed[1]);
    assert_eq!(finishes.load(Ordering::SeqCst), 2);
    assert!(begins.load(Ordering::SeqCst) >= 3);
    let row = destination.store.allocation(&spec.id).unwrap();
    assert_eq!(row.phase, AllocationPhase::Reserved);
    assert!(row.prepared && row.execution.is_none());
    assert!(row.reservation_deadline_ms > Allocator::now() + 600_000);
    drop(_central);
    source_task.abort();
    destination_task.abort();
    let _ = source_task.await;
    let _ = destination_task.await;
    drop(leader);
    drop(destination);
    std::fs::remove_dir_all(root).unwrap();
}

fn expire_canonical_reservation(state: &SessionState, id: &str) {
    state.db.with_connection(|db| {
        db.execute(
            "UPDATE fleet_allocations SET document=json_set(document,'$.reservation_deadline_ms',0) WHERE id=?1",
            [id],
        )?;
        Ok(())
    }).unwrap();
}

#[tokio::test]
async fn canonical_admission_retries_maintenance_contention_with_same_reservation() {
    use super::super::canonical_allocation::{reconcile, reserve, reserve_until};
    let root = std::env::temp_dir().join(format!(
        "beale-maintenance-contention-example-{}",
        uuid::Uuid::new_v4()
    ));
    let leader = state(&root);
    let saved = scan(&leader, "host-example");
    let agent = &saved.agents[0].summary.id;
    let owned = reserve(&leader, &saved, agent, "contention-example", &None)
        .await
        .unwrap()
        .unwrap();
    let ledger = beale_storage::fleet_allocation::AllocationStore::open(leader.db.clone()).unwrap();
    let before = ledger.allocation(&owned.id).unwrap();
    drop(owned);
    // Deterministically exercise a maintenance-length owner hold, then race
    // repeated admissions against the real reconciliation function below.
    let maintenance_owner = super::super::restart::lock_at(
        leader.workspaces.root(),
        before.request.instance.parse().unwrap(),
    )
    .unwrap()
    .unwrap();
    {
        let pending = reserve_until(
            &leader,
            &saved,
            agent,
            "contention-example",
            &None,
            tokio::time::Instant::now() + std::time::Duration::from_secs(3),
        );
        tokio::pin!(pending);
        tokio::select! {
            _ = &mut pending => panic!("maintenance contention must wait without rejecting admission"),
            _ = tokio::time::sleep(std::time::Duration::from_millis(75)) => ()
        }
        assert_eq!(ledger.allocation(&before.request.id).unwrap(), before);
        drop(maintenance_owner);
        let acquired = pending.await.unwrap().unwrap();
        assert_eq!(acquired.id, before.request.id);
        drop(acquired);
    }
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (worker, done) = (leader.clone(), stop.clone());
    let maintenance = std::thread::spawn(move || {
        while !done.load(Ordering::SeqCst) {
            reconcile(&worker).unwrap();
            std::thread::yield_now();
        }
    });
    let mut admitted = 0;
    for _ in 0..64 {
        match reserve_until(
            &leader,
            &saved,
            agent,
            "contention-example",
            &None,
            tokio::time::Instant::now() + std::time::Duration::from_secs(3),
        )
        .await
        {
            Ok(Some(owned)) => {
                admitted += 1;
                drop(owned);
            }
            _ => break,
        }
    }
    stop.store(true, Ordering::SeqCst);
    maintenance.join().unwrap();
    assert_eq!(admitted, 64);
    assert_eq!(ledger.allocation(&before.request.id).unwrap(), before);
    assert_eq!(ledger.allocations().unwrap().len(), 1);
    drop(ledger);
    drop(leader);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn canonical_live_owner_contention_is_bounded_and_never_cancels_or_rebinds() {
    use super::super::canonical_allocation::{reserve, reserve_until};
    let root = std::env::temp_dir().join(format!(
        "beale-live-contention-example-{}",
        uuid::Uuid::new_v4()
    ));
    let leader = state(&root.join("leader"));
    let destination = allocator(&root.join("destination"));
    let saved = scan(&leader, destination.environments.store.host());
    let agent = &saved.agents[0].summary.id;
    let owner = reserve(&leader, &saved, agent, "live-contention-example", &None)
        .await
        .unwrap()
        .unwrap();
    let ledger = beale_storage::fleet_allocation::AllocationStore::open(leader.db.clone()).unwrap();
    let before = ledger.allocation(&owner.id).unwrap();
    let waiter = reserve_until(
        &leader,
        &saved,
        agent,
        "live-contention-example",
        &None,
        tokio::time::Instant::now() + std::time::Duration::from_millis(75),
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
            .await
            .unwrap()
            .unwrap()
            .is_none()
    );
    let (sender, receiver) = watch::channel(false);
    let stop = Some(receiver);
    {
        let pending = reserve(&leader, &saved, agent, "live-contention-example", &stop);
        tokio::pin!(pending);
        tokio::select! {
            _ = &mut pending => panic!("the waiter must not take the live owner's reservation"),
            _ = tokio::time::sleep(std::time::Duration::from_millis(75)) => ()
        }
        sender.send(true).unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), pending)
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(ledger.allocation(&owner.id).unwrap(), before);
    drop(owner);
    // Releasing an unbound owner permits exactly the same reservation to resume.
    let owner = reserve(&leader, &saved, agent, "live-contention-example", &None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(owner.id, before.request.id);
    let spec = specification(&leader, &saved, "live-contention-example");
    let target = retained_bundle(&spec, &destination, uuid::Uuid::new_v4().to_string()).target;
    {
        let pending = reserve_until(
            &leader,
            &saved,
            agent,
            "live-contention-example",
            &None,
            tokio::time::Instant::now() + std::time::Duration::from_secs(3),
        );
        tokio::pin!(pending);
        tokio::select! {
            _ = &mut pending => panic!("the original placement still owns this reservation"),
            _ = tokio::time::sleep(std::time::Duration::from_millis(75)) => ()
        }
        ledger.bind_canonical_storage(&owner.id, &target).unwrap();
        drop(owner);
        // A waiter must reload after acquiring the lock: execution now owns it.
        assert!(pending.await.unwrap().is_none());
    }
    let after = ledger.allocation(&before.request.id).unwrap();
    assert_eq!(after.phase, AllocationPhase::Reserved);
    assert_eq!(after.execution, Some(target));
    assert_eq!(after.request, before.request);
    assert_eq!(ledger.allocations().unwrap().len(), 1);
    drop(ledger);
    drop(leader);
    drop(destination);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn live_busy_restore_keeps_expired_canonical_reservation_through_binding() {
    let root = std::env::temp_dir().join(format!(
        "beale-live-reservation-example-{}",
        uuid::Uuid::new_v4()
    ));
    let leader = state(&root.join("leader"));
    let destination = allocator(&root.join("destination"));
    let saved = scan(&leader, destination.environments.store.host());
    let mut spec = specification(&leader, &saved, "live-reservation-example");
    let bundle = retained_bundle(&spec, &destination, uuid::Uuid::new_v4().to_string());
    spec.restore = Some(bundle.clone());
    let row = reserved(&destination, &spec).await;
    save_intent(
        &leader.db,
        destination.environments.store.host(),
        &spec,
        None,
    )
    .unwrap();
    let central =
        super::super::canonical_allocation::reserve(&leader, &saved, &spec.agent, &spec.id, &None)
            .await
            .unwrap()
            .unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let signal = started.clone();
    let router =
        http::router("a".repeat(64), destination.clone()).layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let signal = signal.clone();
                async move {
                    let reply = next.run(request).await;
                    signal.notify_one();
                    reply
                }
            },
        ));
    let (client, task) = serve(&leader, destination.environments.store.host(), router).await;
    let owner = destination
        .environments
        .lock(row.request.instance.parse().unwrap())
        .unwrap();
    let request = HostAllocationRequest::BeginRestore {
        allocation: spec.id.clone(),
        bundle: bundle.clone(),
    };
    {
        let pending = send_retry(&client, &request);
        tokio::pin!(pending);
        tokio::select! {
            _ = &mut pending => panic!("restore must wait for the original instance owner"),
            _ = started.notified() => ()
        }
        // Simulate the five-minute leader deadline passing during the supported
        // eleven-minute Busy wait, without sleeping or changing the remote lease.
        expire_canonical_reservation(&leader, &central.id);
        lifecycle_maintenance(&leader).await.unwrap();
        let ledger =
            beale_storage::fleet_allocation::AllocationStore::open(leader.db.clone()).unwrap();
        assert_eq!(
            ledger.allocation(&central.id).unwrap().phase,
            AllocationPhase::Reserved
        );
        drop(owner);
        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_secs(5), pending)
                .await
                .unwrap()
                .unwrap(),
            HostAllocationReply::Complete
        ));
    }
    // The recovered placement can still hand its reservation to durable execution.
    let mut target = bundle.target;
    target.instance_id = row.request.instance;
    let ledger = beale_storage::fleet_allocation::AllocationStore::open(leader.db.clone()).unwrap();
    ledger.bind_canonical_storage(&central.id, &target).unwrap();
    assert_eq!(
        ledger.allocation(&central.id).unwrap().execution,
        Some(target)
    );
    drop(central);
    task.abort();
    let _ = task.await;
    drop(ledger);
    drop(leader);
    drop(destination);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn abandoned_canonical_operation_expires_after_owner_drop_and_restart() {
    let root = std::env::temp_dir().join(format!(
        "beale-abandoned-reservation-example-{}",
        uuid::Uuid::new_v4()
    ));
    let leader = state(&root);
    let saved = scan(&leader, "host-example");
    let agent = saved.agents[0].summary.id.clone();
    let live = super::super::canonical_allocation::reserve(
        &leader,
        &saved,
        &agent,
        "still-live-example",
        &None,
    )
    .await
    .unwrap()
    .unwrap();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let (worker_state, worker_scan) = (leader.clone(), saved.clone());
    let task = tokio::spawn(async move {
        let owned = super::super::canonical_allocation::reserve(
            &worker_state,
            &worker_scan,
            &agent,
            "abandoned-example",
            &None,
        )
        .await
        .unwrap()
        .unwrap();
        sender.send(owned.id.clone()).unwrap();
        std::future::pending::<()>().await;
        drop(owned);
    });
    let id = receiver.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    lifecycle_maintenance(&leader).await.unwrap();
    let ledger = beale_storage::fleet_allocation::AllocationStore::open(leader.db.clone()).unwrap();
    assert_eq!(
        ledger.allocation(&id).unwrap().phase,
        AllocationPhase::Reserved
    );
    let instance = ledger.allocation(&id).unwrap().request.instance;
    expire_canonical_reservation(&leader, &id);
    expire_canonical_reservation(&leader, &live.id);
    drop(ledger);
    drop(leader);
    let reopened = state(&root);
    lifecycle_maintenance(&reopened).await.unwrap();
    lifecycle_maintenance(&reopened).await.unwrap();
    let ledger =
        beale_storage::fleet_allocation::AllocationStore::open(reopened.db.clone()).unwrap();
    assert_eq!(
        ledger.allocation(&id).unwrap().phase,
        AllocationPhase::Canceled
    );
    assert!(ledger.environment(&instance).is_err());
    assert!(ledger.renew_transfer(&id, Allocator::now()).is_err());
    assert_eq!(
        ledger.allocation(&live.id).unwrap().phase,
        AllocationPhase::Reserved
    );
    drop(live);
    lifecycle_maintenance(&reopened).await.unwrap();
    assert_eq!(
        ledger
            .allocation("central-still-live-example")
            .unwrap()
            .phase,
        AllocationPhase::Canceled
    );
    drop(ledger);
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
#[ignore = "requires Docker and portable runtime for remote prepared retention"]
async fn docker_unclaimed_remote_allocation_receives_scan_end_and_expires() {
    let root = std::env::temp_dir().join(format!(
        "beale-remote-retention-example-{}",
        uuid::Uuid::new_v4()
    ));
    let leader = state(&root.join("leader"));
    let destination = allocator(&root.join("destination"));
    let saved = scan(&leader, destination.environments.store.host());
    let spec = specification(&leader, &saved, "unclaimed-example");
    save_intent(
        &leader.db,
        destination.environments.store.host(),
        &spec,
        None,
    )
    .unwrap();
    let (client, task) = serve(
        &leader,
        destination.environments.store.host(),
        http::router("a".repeat(64), destination.clone()),
    )
    .await;
    sync_host(&leader.db, &saved, &client).await.unwrap();
    http::handle(
        &destination,
        HostAllocationRequest::Enqueue { spec: spec.clone() },
    )
    .await
    .unwrap();
    let row = destination.prepare_next().await.unwrap().unwrap();
    assert!(row.prepared && row.execution.is_none());
    cancel_unstarted(&leader, &spec.scan, &spec.agent)
        .await
        .unwrap();
    assert!(
        destination
            .store
            .environment(&row.request.instance)
            .unwrap()
            .deadline_ms
            .is_none()
    );
    let store = SwarmStore::open(leader.db.clone()).unwrap();
    store.local_stop(&spec.scan, &spec.agent).unwrap();
    store
        .end_scan(&spec.scan, Allocator::now() - 8 * 86400 * 1000)
        .unwrap();
    assert!(store.load(&spec.scan, true).unwrap().attempts.is_empty());
    lifecycle_maintenance(&leader).await.unwrap();
    let e = destination
        .store
        .environment(&row.request.instance)
        .unwrap();
    assert!(e.deadline_ms.unwrap() < Allocator::now());
    let maintenance = destination.start_maintenance();
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while destination
            .store
            .environment(&row.request.instance)
            .unwrap()
            .deletion
            != DeletionStep::Deleted
        {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    maintenance.abort();
    task.abort();
    let _ = maintenance.await;
    let _ = task.await;
    drop(store);
    drop(leader);
    drop(destination);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn lost_restore_connection_waits_for_original_owner_and_busy_wait_is_bounded() {
    let root = std::env::temp_dir().join(format!(
        "beale-busy-restore-example-{}",
        uuid::Uuid::new_v4()
    ));
    let leader = state(&root.join("leader"));
    let destination = allocator(&root.join("destination"));
    let saved = scan(&leader, destination.environments.store.host());
    let spec = specification(&leader, &saved, "busy-example");
    let row = reserved(&destination, &spec).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let (a, hits, instance) = (
        destination.clone(),
        calls.clone(),
        row.request.instance.clone(),
    );
    let router =
        http::router("a".repeat(64), destination.clone()).layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let (a, hits, instance) = (a.clone(), hits.clone(), instance.clone());
                async move {
                    if hits.fetch_add(1, Ordering::SeqCst) == 0 {
                        let owner = a.environments.lock(instance.parse().unwrap()).unwrap();
                        // The proxy loses the caller connection while its dispatched
                        // operation continues owning the instance on the host.
                        tokio::spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                            a.store.prepared("busy-example").unwrap();
                            drop(owner);
                        });
                        return StatusCode::BAD_GATEWAY.into_response();
                    }
                    next.run(request).await
                }
            },
        ));
    let (client, task) = serve(&leader, destination.environments.store.host(), router).await;
    assert!(matches!(
        send_retry(
            &client,
            &HostAllocationRequest::FinishRestore {
                allocation: spec.id.clone()
            }
        )
        .await
        .unwrap(),
        HostAllocationReply::Complete
    ));
    assert!(calls.load(Ordering::SeqCst) >= 4);
    let before = destination.store.allocation(&spec.id).unwrap();
    assert!(before.prepared && before.execution.is_none());
    let owner = destination
        .environments
        .lock(row.request.instance.parse().unwrap())
        .unwrap();
    assert!(
        send_retry_until(
            &client,
            &HostAllocationRequest::Cancel {
                id: spec.id.clone()
            },
            tokio::time::Instant::now() + std::time::Duration::from_millis(40)
        )
        .await
        .is_err()
    );
    assert_eq!(destination.store.allocation(&spec.id).unwrap(), before);
    drop(owner);
    task.abort();
    let _ = task.await;
    drop(leader);
    drop(destination);
    std::fs::remove_dir_all(root).unwrap();
}

fn retained_bundle(
    spec: &AllocationSpec,
    source: &Allocator,
    instance: String,
) -> EnvironmentBundle {
    EnvironmentBundle {
        version: 1,
        target: ExecutionTarget {
            leader_id: spec.leader.clone(),
            host_id: source.environments.store.host().into(),
            configuration_id: spec.configuration.clone(),
            configuration_revision: 1,
            instance_id: instance,
            identity: ExecutionIdentity {
                workspace_id: "workspace-example".into(),
                scan_id: spec.scan.clone(),
                agent_id: spec.agent.clone(),
                attempt_id: "attempt-example".into(),
                generation: 1,
            },
        },
        configuration: source
            .environments
            .store
            .configuration("image-example")
            .unwrap()
            .configuration,
        image_digest: Some(format!("sha256:{}", "a".repeat(64))),
        archives: [
            ArchiveKind::Workspace,
            ArchiveKind::State,
            ArchiveKind::Image,
        ]
        .into_iter()
        .map(|kind| EnvironmentArchive {
            kind,
            sha256: "a".repeat(64),
            bytes: 1,
        })
        .collect(),
    }
}
#[tokio::test]
async fn restore_selection_skips_unavailable_or_incompatible_hosts_and_pins_selected_route() {
    for unavailable in [true, false] {
        let root =
            std::env::temp_dir().join(format!("beale-selection-example-{}", uuid::Uuid::new_v4()));
        let leader = state(&root.join("leader"));
        let source = allocator(&root.join("source"));
        let bad = allocator(&root.join("bad"));
        let good = allocator(&root.join("good"));
        let mut saved = scan(&leader, bad.environments.store.host());
        saved.config.destinations.push(Destination {
            host_id: good.environments.store.host().into(),
            configuration_id: "image-example".into(),
        });
        let (_, bad_task) = serve(
            &leader,
            bad.environments.store.host(),
            http::router("a".repeat(64), bad.clone()),
        )
        .await;
        let (good_client, good_task) = serve(
            &leader,
            good.environments.store.host(),
            http::router("a".repeat(64), good.clone()),
        )
        .await;
        if unavailable {
            bad_task.abort();
        } else {
            let mut config = bad
                .environments
                .store
                .configuration("image-example")
                .unwrap()
                .configuration;
            config.revision += 1;
            config.architecture = LinuxArchitecture::Arm64;
            bad.environments.store.save_configuration(config).unwrap();
        }
        let mut spec = specification(&leader, &saved, "selected-example");
        let bundle = retained_bundle(&spec, &source, uuid::Uuid::new_v4().to_string());
        spec.restore = Some(bundle.clone());
        save_intent(
            &leader.db,
            bad.environments.store.host(),
            &spec,
            Some(&bundle.target),
        )
        .unwrap();
        let (host, selected) = select_destination(&leader, &saved, &spec.id).await.unwrap();
        assert_eq!(host, good.environments.store.host());
        assert_eq!(selected.restore, spec.restore);
        // Enqueue may have committed even when its response is lost. Routing
        // must remain immutable across restart and later inventory changes.
        let HostAllocationReply::Allocation { allocation } = good_client
            .send(&HostAllocationRequest::Enqueue {
                spec: selected.clone(),
            })
            .await
            .unwrap()
        else {
            panic!("not queued")
        };
        leader
            .db
            .with_connection(|db| {
                db.execute(
                    "UPDATE fleet_coordination_peers SET enabled=0 WHERE id=?1",
                    [&host],
                )?;
                Ok(())
            })
            .unwrap();
        drop(leader);
        let restarted = state(&root.join("leader"));
        saved.config.destinations.reverse();
        let (retried_host, retried) = select_destination(&restarted, &saved, &spec.id)
            .await
            .unwrap();
        assert_eq!(retried_host, host);
        assert_eq!(retried, selected);
        assert_eq!(
            good.store.allocation(&spec.id).unwrap().request.instance,
            allocation.request.instance
        );
        assert!(bad.store.allocations().unwrap().is_empty());
        assert!(cancel_intent(&restarted, &spec.id).await.is_err()); // Source pairing is intentionally absent.
        let HostAllocationReply::Allocation { allocation: late } = good_client
            .send(&HostAllocationRequest::Enqueue {
                spec: selected.clone(),
            })
            .await
            .unwrap()
        else {
            panic!("late retry not recorded")
        };
        assert_eq!(late.phase, AllocationPhase::Canceled);
        assert_eq!(late.request.instance, allocation.request.instance);
        assert!(
            select_destination(&restarted, &saved, &spec.id)
                .await
                .is_err()
        );
        let mut canceled = specification(&restarted, &saved, "canceled-selection-example");
        canceled.restore = Some(bundle.clone());
        save_intent(
            &restarted.db,
            bad.environments.store.host(),
            &canceled,
            Some(&bundle.target),
        )
        .unwrap();
        // Missing source pairing does not undo the durable selection fence.
        assert!(cancel_intent(&restarted, &canceled.id).await.is_err());
        assert!(
            select_destination(&restarted, &saved, &canceled.id)
                .await
                .is_err()
        );
        bad_task.abort();
        good_task.abort();
        let _ = bad_task.await;
        let _ = good_task.await;
        drop(restarted);
        drop(source);
        drop(bad);
        drop(good);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test]
async fn maintenance_cancels_current_handoff_despite_unavailable_historical_host() {
    for missing_pairing in [true, false] {
        let root = std::env::temp_dir().join(format!(
            "beale-cleanup-history-example-{}",
            uuid::Uuid::new_v4()
        ));
        let leader = state(&root.join("leader"));
        let source = allocator(&root.join("source"));
        let destination = allocator(&root.join("destination"));
        let saved = scan(&leader, destination.environments.store.host());
        let historical = specification(&leader, &saved, "historical-example");
        save_intent(&leader.db, "historical-host-example", &historical, None).unwrap();
        if !missing_pairing {
            let (_, task) = serve(&leader, "historical-host-example", Router::new()).await;
            task.abort();
            let _ = task.await;
        }
        let old = specification(&leader, &saved, "source-example");
        let source_row = reserved(&source, &old).await;
        source.store.confirm_stopped(&old.id, true).unwrap();
        let mut current = specification(&leader, &saved, "current-example");
        let bundle = retained_bundle(&current, &source, source_row.request.instance.clone());
        current.restore = Some(bundle.clone());
        let environment = source
            .store
            .environment(&source_row.request.instance)
            .unwrap();
        source
            .store
            .retention(
                &environment.instance,
                environment.generation,
                RetentionAction::Protect {
                    reference: format!("restore-{}", current.id),
                },
                Allocator::now(),
            )
            .unwrap();
        save_intent(
            &leader.db,
            destination.environments.store.host(),
            &current,
            Some(&bundle.target),
        )
        .unwrap();
        let (_, source_task) = serve(
            &leader,
            source.environments.store.host(),
            http::router("a".repeat(64), source.clone()),
        )
        .await;
        let (_, destination_task) = serve(
            &leader,
            destination.environments.store.host(),
            http::router("a".repeat(64), destination.clone()),
        )
        .await;
        select_destination(&leader, &saved, &current.id)
            .await
            .unwrap();
        reserved(&destination, &current).await;
        SwarmStore::open(leader.db.clone())
            .unwrap()
            .local_stop(&current.scan, &current.agent)
            .unwrap();
        lifecycle_maintenance(&leader).await.unwrap();
        lifecycle_maintenance(&leader).await.unwrap();
        assert_eq!(
            destination.store.allocation(&current.id).unwrap().phase,
            AllocationPhase::Canceled
        );
        assert!(
            source
                .store
                .environment(&source_row.request.instance)
                .unwrap()
                .references
                .is_empty()
        );
        assert!(
            source
                .store
                .export_canceled(&bundle.target, &current.id)
                .unwrap()
        );
        source_task.abort();
        destination_task.abort();
        let _ = source_task.await;
        let _ = destination_task.await;
        drop(leader);
        drop(source);
        drop(destination);
        std::fs::remove_dir_all(root).unwrap();
    }
}
