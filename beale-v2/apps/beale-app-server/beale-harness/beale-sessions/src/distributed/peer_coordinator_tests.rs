use super::*;
use crate::distributed::peer_coordinator::*;
use beale_contracts::{swarm_host::*, swarm_worker::WorkerShell};
use beale_storage::fleet_executions::FleetExecutionStore;
use std::sync::{Mutex, atomic::AtomicUsize};

struct Endpoint {
    starts: AtomicUsize,
    revision: AtomicUsize,
    confirmed: AtomicBool,
    block_stops: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    requests: Mutex<Vec<(String, String)>>,
}
async fn fixture() -> (
    Fixture,
    PeerWorkerCoordinator,
    OwnedPeerAttempt,
    Arc<Endpoint>,
    tokio::task::JoinHandle<()>,
) {
    let f = Fixture::with_selection(
        true,
        WorkerShell::native(),
        false,
        "workspace-example",
        Some(Destination {
            host_id: "peer-example".into(),
            configuration_id: "configuration-example".into(),
        }),
    )
    .await;
    f.store
        .reconcile_worker(&f.attempt.id, true, WorkerDisposition::Failed, unix_ms())
        .unwrap();
    f.store
        .local_message(
            "scan-example",
            "resume-peer",
            &f.deputy,
            "Continue synthetic work",
        )
        .unwrap();
    let endpoint = Arc::new(Endpoint {
        starts: AtomicUsize::new(0),
        revision: AtomicUsize::new(0),
        confirmed: AtomicBool::new(false),
        block_stops: AtomicBool::new(false),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        requests: Mutex::new(vec![]),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let peer = beale_fleet::FleetPeer {
        id: "peer-example".into(),
        endpoint: format!("http://{}", listener.local_addr().unwrap()),
        control_token: "b".repeat(64),
        enabled: true,
        protocol_major: beale_contracts::PROTOCOL_MAJOR,
    };
    beale_storage::fleet_coordination::FleetCoordinationStore::open(f.state.db.clone()).unwrap();
    // Synthetic loopback pairing bypasses only the production Tailscale-origin
    // enrollment rule. Recovery still uses the actual saved peer and HTTP client.
    f.state
        .db
        .with_connection(|c| {
            c.execute(
                "INSERT INTO fleet_coordination_peers VALUES(?1,?2,?3,0,?4)",
                beale_storage::rusqlite::params![
                    peer.id,
                    peer.endpoint,
                    peer.control_token,
                    peer.protocol_major
                ],
            )?;
            Ok(())
        })
        .unwrap();
    let control = endpoint.clone();
    let app = axum::Router::new().route(
        HOST_EXECUTION_PATH,
        axum::routing::post(
            move |axum::Json(request): axum::Json<HostExecutionRequest>| {
                let control = control.clone();
                async move {
                    request.validate().unwrap();
                    let kind = match request.operation {
                        HostExecutionOperation::Start { .. } => "start",
                        HostExecutionOperation::Stop {} => "stop",
                        HostExecutionOperation::Inspect {} => "inspect",
                    };
                    control
                        .requests
                        .lock()
                        .unwrap()
                        .push((kind.into(), request.operation_id.clone()));
                    if kind == "start" {
                        control.starts.fetch_add(1, Ordering::SeqCst);
                    }
                    if kind == "start"
                        || kind == "stop" && control.block_stops.load(Ordering::SeqCst)
                    {
                        control.entered.notify_one();
                        control.release.notified().await;
                    }
                    axum::Json(HostExecutionReply {
                        version: 1,
                        operation_id: request.operation_id,
                        report: HostExecutionReport {
                            target: request.target,
                            revision: control.revision.fetch_add(1, Ordering::SeqCst) as u64 + 1,
                            stop_requested: kind == "stop",
                            phase: if control.confirmed.load(Ordering::SeqCst) {
                                HostExecutionPhase::Stopped
                            } else {
                                HostExecutionPhase::TerminationUnconfirmed
                            },
                        },
                    })
                }
            },
        ),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let coordinator = PeerWorkerCoordinator::new(
        f.state.db.clone(),
        f.state.workspaces.root().to_path_buf(),
        &peer,
        f.url.clone(),
    )
    .unwrap();
    let placement = PeerPlacement {
        leader_id: FleetExecutionStore::open(f.state.db.clone())
            .unwrap()
            .host_id()
            .unwrap(),
        host_id: peer.id,
        configuration_id: "configuration-example".into(),
        configuration_revision: 1,
        instance_id: "instance-example".into(),
        shell: WorkerShell::native(),
    };
    let owned = coordinator
        .claim("scan-example", &f.deputy, 64, &placement)
        .unwrap()
        .unwrap();
    (f, coordinator, owned, endpoint, server)
}
#[tokio::test]
async fn peer_presignaled_cancel_never_sends_start_and_accepts_only_after_checkpoint() {
    let (f, coordinator, mut owned, endpoint, server) = fixture().await;
    endpoint.confirmed.store(true, Ordering::SeqCst);
    let (_sender, cancel) = tokio::sync::watch::channel(true);
    let checkpoints = AtomicUsize::new(0);
    let result = coordinator
        .run(&mut owned, None, cancel, |_, _| {
            checkpoints.fetch_add(1, Ordering::SeqCst);
            async { Ok(()) }
        })
        .await
        .unwrap();
    assert_eq!(result, WorkerReconciliation::BranchStopped);
    assert_eq!(endpoint.starts.load(Ordering::SeqCst), 0);
    assert_eq!(checkpoints.load(Ordering::SeqCst), 1);
    assert!(
        f.store
            .peer_worker(&owned.record().turn.attempt_id)
            .unwrap()
            .complete
    );
    assert_eq!(
        f.state.db.session(&f.deputy).unwrap().unwrap().turns.len(),
        2
    );
    drop(owned);
    drop(coordinator);
    server.abort();
    let _ = server.await;
    f.close().await;
}
#[tokio::test]
async fn peer_lost_start_owner_recovers_by_stop_without_relaunch_or_unconfirmed_acceptance() {
    let (f, coordinator, mut owned, endpoint, server) = fixture().await;
    let (_sender, cancel) = tokio::sync::watch::channel(false);
    let attempt = owned.record().turn.attempt_id.clone();
    let mut run = Box::pin(coordinator.run(&mut owned, None, cancel, |_, _| async {
        panic!("launch owner cannot checkpoint after being dropped")
    }));
    tokio::time::timeout(Duration::from_secs(5),async {
        tokio::select! { _=endpoint.entered.notified()=>(),result=&mut run=>panic!("unexpected completion: {result:?}") }
    }).await.unwrap();
    assert!(f.store.peer_worker(&attempt).unwrap().dispatched);
    drop(run);
    drop(owned);
    let mut owned = coordinator.acquire_recovery(&attempt).unwrap().unwrap();
    assert_eq!(
        coordinator
            .recover(&mut owned, |_, _| async {
                panic!("unconfirmed execution cannot checkpoint")
            })
            .await
            .unwrap(),
        WorkerReconciliation::TerminationUnconfirmed
    );
    assert!(!f.store.peer_worker(&attempt).unwrap().complete);
    assert_eq!(
        f.state.db.session(&f.deputy).unwrap().unwrap().turns.len(),
        1
    );
    assert!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .events
            .iter()
            .any(|e| e.kind == "worker-termination-unconfirmed")
    );
    endpoint.confirmed.store(true, Ordering::SeqCst);
    let checkpoints = AtomicUsize::new(0);
    assert_eq!(
        coordinator
            .recover(&mut owned, |_, _| {
                checkpoints.fetch_add(1, Ordering::SeqCst);
                async { Ok(()) }
            })
            .await
            .unwrap(),
        WorkerReconciliation::BranchStopped
    );
    assert_eq!(endpoint.starts.load(Ordering::SeqCst), 1);
    assert_eq!(checkpoints.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.state.db.session(&f.deputy).unwrap().unwrap().turns.len(),
        2
    );
    let requests = endpoint.requests.lock().unwrap().clone();
    assert!(
        requests
            .iter()
            .filter(|(kind, _)| kind == "stop")
            .all(|(_, id)| id == &format!("stop:{attempt}"))
    );
    endpoint.release.notify_one();
    drop(owned);
    drop(coordinator);
    server.abort();
    let _ = server.await;
    f.close().await;
}
#[tokio::test]
async fn peer_unprepared_restart_needs_no_network_or_checkpoint() {
    let (f, coordinator, owned, endpoint, server) = fixture().await;
    let attempt = owned.record().turn.attempt_id.clone();
    drop(owned);
    let mut owned = coordinator.acquire_recovery(&attempt).unwrap().unwrap();
    assert_eq!(
        coordinator
            .recover(&mut owned, |_, _| async {
                panic!("no worker workspace exists")
            })
            .await
            .unwrap(),
        WorkerReconciliation::BranchStopped
    );
    assert!(endpoint.requests.lock().unwrap().is_empty());
    assert!(f.store.peer_worker(&attempt).unwrap().complete);
    assert_eq!(
        f.state.db.session(&f.deputy).unwrap().unwrap().turns.len(),
        1
    );
    drop(owned);
    drop(coordinator);
    server.abort();
    let _ = server.await;
    f.close().await;
}

#[tokio::test]
async fn automatic_peer_maintenance_recovers_unprepared_claims_after_owner_loss() {
    let (f, coordinator, owned, endpoint, server) = fixture().await;
    let attempt = owned.record().turn.attempt_id.clone();
    let mut maintenance = crate::distributed::peer_recovery::PeerRecovery::new(
        f.state.db.clone(),
        f.state.workspaces.root().to_path_buf(),
    );
    maintenance.tick().unwrap();
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert!(!f.store.peer_worker(&attempt).unwrap().complete);
    drop(maintenance);
    drop(owned);
    // Exercise the app-server maintenance entry point, not just the helper.
    let worker = tokio::spawn(crate::automations::run_worker(f.state.clone()));
    tokio::time::timeout(Duration::from_secs(5), async {
        while !f.store.peer_worker(&attempt).unwrap().complete {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    worker.abort();
    let _ = worker.await;
    assert!(endpoint.requests.lock().unwrap().is_empty());
    drop(coordinator);
    server.abort();
    let _ = server.await;
    f.close().await;
}
#[tokio::test]
async fn peer_maintenance_fences_missing_pairing_then_recovers_disabled_peer_without_false_checkpoint()
 {
    use crate::distributed::peer_recovery::recover_one;
    use beale_storage::fleet_coordination::FleetCoordinationStore;
    let (f, coordinator, owned, endpoint, server) = fixture().await;
    let turn = owned.record().turn.clone();
    let identity = owned.record().target.identity.clone();
    let (_, credential) = prepare_worker(
        &f.state.db,
        &turn,
        WorkerShell::native(),
        &registration(),
        unix_ms(),
    )
    .unwrap();
    f.store
        .peer_dispatch(&turn.attempt_id, owned.record().owner_key)
        .unwrap();
    let pairing = FleetCoordinationStore::open(f.state.db.clone())
        .unwrap()
        .peer("peer-example")
        .unwrap()
        .unwrap();
    assert!(!pairing.enabled);
    f.state
        .db
        .with_connection(|c| {
            c.execute("DELETE FROM fleet_coordination_peers", [])?;
            Ok(())
        })
        .unwrap();
    drop(owned);
    for _ in 0..2 {
        assert!(
            recover_one(
                &f.state.db,
                f.state.workspaces.root(),
                &turn.attempt_id,
                |_, _| async { panic!("no stop proof") }
            )
            .await
            .is_err()
        );
    }
    assert!(endpoint.requests.lock().unwrap().is_empty());
    assert!(
        f.store
            .peer_worker(&turn.attempt_id)
            .unwrap()
            .stop_requested
    );
    let warnings = f
        .store
        .load("scan-example", true)
        .unwrap()
        .events
        .into_iter()
        .filter(|e| e.kind == "peer-recovery-pending")
        .collect::<Vec<_>>();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].payload["reason"], "pairing_unavailable");
    let request = WorkerRequest {
        version: 1,
        workspace_id: identity.workspace_id,
        scan_id: identity.scan_id,
        agent_id: identity.agent_id,
        attempt_id: identity.attempt_id,
        generation: identity.generation,
        operation_id: "revoked-example".into(),
        operation: WorkerOperation::Bootstrap {},
    };
    assert!(
        WorkerTransport::new(&f.url, credential.bearer())
            .unwrap()
            .send(&request, Duration::from_secs(2))
            .await
            .is_err()
    );
    f.state
        .db
        .with_connection(|c| {
            c.execute(
                "INSERT INTO fleet_coordination_peers VALUES(?1,?2,?3,0,?4)",
                beale_storage::rusqlite::params![
                    pairing.id,
                    pairing.endpoint,
                    pairing.control_token,
                    pairing.protocol_major
                ],
            )?;
            Ok(())
        })
        .unwrap();
    endpoint.confirmed.store(true, Ordering::SeqCst);
    let mut maintenance = crate::distributed::peer_recovery::PeerRecovery::new(
        f.state.db.clone(),
        f.state.workspaces.root().to_path_buf(),
    );
    maintenance.tick().unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !f
            .store
            .load("scan-example", true)
            .unwrap()
            .events
            .iter()
            .any(|e| e.payload["reason"] == "checkpoint_pending")
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    drop(maintenance);
    assert!(f.store.peer_worker(&turn.attempt_id).unwrap().terminated());
    assert!(!f.store.peer_worker(&turn.attempt_id).unwrap().complete);
    assert_eq!(
        f.state.db.session(&f.deputy).unwrap().unwrap().turns.len(),
        1
    );
    assert_eq!(endpoint.starts.load(Ordering::SeqCst), 0);
    // Persisted termination proof and a canonical receipt need no pairing.
    f.state.db.with_connection(|c|{c.execute("DELETE FROM fleet_coordination_peers",[])?;c.execute_batch("CREATE TRIGGER fail_peer_completion BEFORE UPDATE ON swarm_peer_workers WHEN NEW.complete=1 BEGIN SELECT RAISE(ABORT,'synthetic completion failure'); END;")?;Ok(())}).unwrap();
    assert!(
        recover_one(
            &f.state.db,
            f.state.workspaces.root(),
            &turn.attempt_id,
            |_, _| async { Ok(()) }
        )
        .await
        .is_err()
    );
    assert_eq!(
        f.state.db.session(&f.deputy).unwrap().unwrap().turns.len(),
        2
    );
    f.state
        .db
        .with_connection(|c| {
            c.execute_batch("DROP TRIGGER fail_peer_completion")?;
            Ok(())
        })
        .unwrap();
    recover_one(
        &f.state.db,
        f.state.workspaces.root(),
        &turn.attempt_id,
        |_, _| async { panic!("accepted workspace must not be checkpointed again") },
    )
    .await
    .unwrap();
    assert!(f.store.peer_worker(&turn.attempt_id).unwrap().complete);
    assert_eq!(
        f.state.db.session(&f.deputy).unwrap().unwrap().turns.len(),
        2
    );
    drop(coordinator);
    server.abort();
    let _ = server.await;
    f.close().await;
}
#[tokio::test]
async fn peer_maintenance_advances_past_corrupt_and_live_records_across_pages() {
    let (f, coordinator, owned, endpoint, server) = fixture().await;
    let live = owned.record().turn.attempt_id.clone();
    f.store
        .local_message(
            "scan-example",
            "create-recovery-batch",
            &f.leader,
            "Coordinate synthetic batch",
        )
        .unwrap();
    let leader = f
        .store
        .local_claim("scan-example", &f.leader, unix_ms(), 128)
        .unwrap()
        .unwrap();
    let profile = f
        .store
        .load("scan-example", true)
        .unwrap()
        .agent(&f.deputy)
        .unwrap()
        .summary
        .profile
        .clone();
    let mut agents = vec![];
    for index in 0..66 {
        let result = f
            .store
            .local_dispatch(
                &leader,
                &format!("spawn-recovery-{index}"),
                SwarmCommand::Spawn(Spawn {
                    handoff_id: None,
                    profile: profile.clone(),
                    task: task(),
                    independent: false,
                }),
                Admission::Ready,
            )
            .unwrap();
        agents.push(result["child_id"].as_str().unwrap().to_owned());
    }
    f.store
        .local_finish(&leader, json!({}), "Waiting for synthetic batch", false)
        .unwrap();
    let mut keys = vec![];
    for (index, agent) in agents.iter().enumerate() {
        let target = &owned.record().target;
        let placement = PeerPlacement {
            leader_id: target.leader_id.clone(),
            host_id: target.host_id.clone(),
            configuration_id: target.configuration_id.clone(),
            configuration_revision: 1,
            instance_id: format!("instance-recovery-{index}"),
            shell: WorkerShell::native(),
        };
        let child = coordinator
            .claim("scan-example", agent, 128, &placement)
            .unwrap()
            .unwrap();
        keys.push(child.record().turn.attempt_id.clone());
    }
    keys.sort();
    let corrupt = &keys[0];
    f.state
        .db
        .with_connection(|c| {
            c.execute(
                "UPDATE swarm_peer_workers SET document='{' WHERE attempt=?1",
                [corrupt],
            )?;
            Ok(())
        })
        .unwrap();
    let mut maintenance = crate::distributed::peer_recovery::PeerRecovery::new(
        f.state.db.clone(),
        f.state.workspaces.root().to_path_buf(),
    );
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            maintenance.tick().unwrap();
            let remaining = f
                .state
                .db
                .with_connection(|c| {
                    Ok(c.query_row(
                        "SELECT count(*) FROM swarm_peer_workers WHERE complete=0",
                        [],
                        |r| r.get::<_, i64>(0),
                    )?)
                })
                .unwrap();
            if remaining == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(!f.store.peer_worker(&live).unwrap().complete);
    assert!(f.store.peer_worker(corrupt).is_err());
    assert!(endpoint.requests.lock().unwrap().is_empty());
    drop(owned);
    tokio::time::timeout(Duration::from_secs(5), async {
        while !f.store.peer_worker(&live).unwrap().complete {
            maintenance.tick().unwrap();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    drop(maintenance);
    drop(coordinator);
    server.abort();
    let _ = server.await;
    f.close().await;
}

#[tokio::test]
async fn slow_peer_recovery_does_not_block_other_records_and_shutdown_preserves_intent() {
    let (f, coordinator, owned, endpoint, server) = fixture().await;
    let turn = owned.record().turn.clone();
    let target = owned.record().target.clone();
    prepare_worker(
        &f.state.db,
        &turn,
        WorkerShell::native(),
        &registration(),
        unix_ms(),
    )
    .unwrap();
    f.store
        .peer_dispatch(&turn.attempt_id, owned.record().owner_key)
        .unwrap();
    drop(owned);
    f.store
        .local_message(
            "scan-example",
            "other-peer-branch",
            &f.leader,
            "Coordinate other synthetic branch",
        )
        .unwrap();
    let leader = f
        .store
        .local_claim("scan-example", &f.leader, unix_ms(), 64)
        .unwrap()
        .unwrap();
    let profile = f
        .store
        .load("scan-example", true)
        .unwrap()
        .agent(&f.deputy)
        .unwrap()
        .summary
        .profile
        .clone();
    let spawned = f
        .store
        .local_dispatch(
            &leader,
            "spawn-other-peer",
            SwarmCommand::Spawn(Spawn {
                handoff_id: None,
                profile,
                task: task(),
                independent: false,
            }),
            Admission::Ready,
        )
        .unwrap();
    f.store
        .local_finish(&leader, json!({}), "Waiting for other branch", false)
        .unwrap();
    let placement = PeerPlacement {
        leader_id: target.leader_id,
        host_id: target.host_id,
        configuration_id: target.configuration_id,
        configuration_revision: 1,
        instance_id: "instance-other-example".into(),
        shell: WorkerShell::native(),
    };
    let other = coordinator
        .claim(
            "scan-example",
            spawned["child_id"].as_str().unwrap(),
            64,
            &placement,
        )
        .unwrap()
        .unwrap();
    let other_id = other.record().turn.attempt_id.clone();
    drop(other);
    endpoint.block_stops.store(true, Ordering::SeqCst);
    let mut maintenance = crate::distributed::peer_recovery::PeerRecovery::new(
        f.state.db.clone(),
        f.state.workspaces.root().to_path_buf(),
    );
    maintenance.tick().unwrap();
    tokio::time::timeout(Duration::from_secs(3), endpoint.entered.notified())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !f.store.peer_worker(&other_id).unwrap().complete {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        coordinator
            .acquire_recovery(&turn.attempt_id)
            .unwrap()
            .is_none()
    );
    assert!(
        f.store
            .peer_worker(&turn.attempt_id)
            .unwrap()
            .stop_requested
    );
    drop(maintenance);
    let recovered = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(owned) = coordinator.acquire_recovery(&turn.attempt_id).unwrap() {
                break owned;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(!f.store.peer_worker(&turn.attempt_id).unwrap().complete);
    assert_eq!(endpoint.starts.load(Ordering::SeqCst), 0);
    endpoint.release.notify_one();
    drop(recovered);
    drop(coordinator);
    server.abort();
    let _ = server.await;
    f.close().await;
}
