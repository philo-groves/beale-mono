//! Two independent host databases and HTTP audiences with a real assigned child.
//! Native execution here is a test environment, never a selected-Fleet fallback.
use super::*;
use crate::distributed::{ProcessFuture, peer_coordinator::PeerWorkerCoordinator, peer_host::*};
use beale_contracts::{swarm_host::*, swarm_worker::WorkerShell};
use beale_fleet::worker_host::{BackendFuture, BackendRejection};
use beale_storage::fleet_executions::{BackendObservation, FleetExecutionStore};
use std::{ffi::OsString, path::PathBuf, sync::atomic::AtomicUsize};

struct NativeFixture {
    target: ExecutionTarget,
    key: Uuid,
    root: PathBuf,
    workspace: PathBuf,
    starts: AtomicUsize,
}
impl PreparedEnvironments for NativeFixture {
    fn available(&self) -> bool {
        true
    }
    fn validate(&self, target: &ExecutionTarget) -> anyhow::Result<(), BackendRejection> {
        if *target == self.target {
            Ok(())
        } else {
            Err(BackendRejection::Conflict)
        }
    }
    fn spawn<'a>(
        &'a self,
        target: &'a ExecutionTarget,
        launch: &'a HostWorkerLaunch,
    ) -> ProcessFuture<'a, PreparedWorker> {
        Box::pin(async move {
            anyhow::ensure!(target == &self.target, "synthetic environment mismatch");
            assert_eq!(self.starts.fetch_add(1, Ordering::SeqCst), 0);
            crate::distributed::peer_workspace::prepare_workspace(
                launch,
                &target.identity,
                &self.root.join("snapshot-cache"),
                &self.workspace,
            )
            .await?;
            #[cfg(target_os = "linux")]
            beale_tools::linux_worker::LinuxWorker::prepare(
                &self.root.join("supervisors"),
                self.key,
                &std::env::current_exe()?,
                &arguments(),
                &self.workspace,
            )?;
            #[cfg(target_os = "linux")]
            let process =
                WorkerProcess::spawn_guarded(beale_tools::linux_worker::LinuxWorker::open(
                    &self.root.join("supervisors"),
                    self.key,
                )?)?;
            #[cfg(not(target_os = "linux"))]
            let process = WorkerProcess::spawn_recoverable(
                &std::env::current_exe()?,
                &arguments(),
                &self.workspace,
                self.key,
            )
            .await?;
            Ok(PreparedWorker {
                process: Box::new(process),
                workspace: self.workspace.clone(),
                protected_root: self.root.clone(),
                recovery: self.root.join("worker-recovery"),
            })
        })
    }
    fn recover<'a>(&'a self, target: &'a ExecutionTarget) -> BackendFuture<'a> {
        Box::pin(async move {
            anyhow::ensure!(target == &self.target, "synthetic environment mismatch");
            // A canceled blocking materialization must release its receiver
            // before the fixture can claim all preparation work ended.
            let _receiver = beale_storage::swarm_files::SnapshotReceiver::open(
                &self.root.join("snapshot-cache"),
            )?;
            #[cfg(target_os = "linux")]
            let stopped = beale_tools::linux_worker::LinuxWorker::open(
                &self.root.join("supervisors"),
                self.key,
            )?
            .recover_terminated()?;
            #[cfg(not(target_os = "linux"))]
            let stopped = beale_tools::process::ProcessTree::recover_terminated(self.key)?;
            Ok(if stopped {
                BackendObservation::Stopped
            } else {
                BackendObservation::TerminationUnconfirmed
            })
        })
    }
}

#[tokio::test]
async fn peer_snapshot_transfer_recovers_partition_and_rejects_foreign_or_revoked_reads() {
    use crate::distributed::peer_workspace::prepare_workspace;
    use beale_contracts::swarm_files::{SNAPSHOT_CHUNK_BYTES, SnapshotChunk, SnapshotRead};
    let f = Fixture::new().await;
    std::fs::create_dir_all(f.root.join("workspace")).unwrap();
    let bytes = vec![b'z'; SNAPSHOT_CHUNK_BYTES * 2 + 7];
    std::fs::write(f.root.join("workspace/seed.txt"), &bytes).unwrap();
    crate::research_files::capture(&f.state, "scan-example", &f.workspace_id).unwrap();
    let digest = crate::research_files::store(&f.state)
        .unwrap()
        .snapshot("scan-example")
        .unwrap()
        .0;
    let blocked = Arc::new(AtomicBool::new(true));
    let block = blocked.clone();
    let app = crate::distributed::gateway::router()
        .with_state(f.state.clone())
        .layer(middleware::from_fn(
            move |request: Request<Body>, next: Next| {
                let block = block.clone();
                async move {
                    let response = next.run(request).await;
                    if !response.status().is_success() {
                        return response;
                    }
                    let (parts, body) = response.into_parts();
                    let bytes = axum::body::to_bytes(body, MAX_DOCUMENT_BYTES)
                        .await
                        .unwrap();
                    let chunk =
                        serde_json::from_slice::<WorkerReply>(&bytes)
                            .ok()
                            .and_then(|reply| match reply {
                                WorkerReply::Acknowledged { acknowledgement } => {
                                    serde_json::from_value::<SnapshotChunk>(acknowledgement.result)
                                        .ok()
                                }
                                _ => None,
                            });
                    if block.load(Ordering::SeqCst)
                        && chunk.is_some_and(|chunk| {
                            chunk.request.blob.is_some() && chunk.request.offset > 0
                        })
                    {
                        Response::builder()
                            .status(StatusCode::SERVICE_UNAVAILABLE)
                            .body(Body::empty())
                            .unwrap()
                    } else {
                        Response::from_parts(parts, Body::from(bytes))
                    }
                }
            },
        ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let launch = HostWorkerLaunch {
        leader_origin: format!("http://{}", listener.local_addr().unwrap()),
        worker_bearer: f.credential.bearer().into(),
        snapshot: Some(digest.clone()),
    };
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let identity = f.store.worker_bootstrap(&f.attempt.id).unwrap().identity;
    let cache = f.root.join("snapshot-cache");
    let destination = f.root.join("received-workspace");
    assert!(
        prepare_workspace(&launch, &identity, &cache, &destination)
            .await
            .is_err()
    );
    assert!(!destination.exists());
    let blob = beale_storage::swarm_files::hash(&bytes);
    assert_eq!(
        std::fs::metadata(cache.join(format!("{blob}.part")))
            .unwrap()
            .len(),
        SNAPSHOT_CHUNK_BYTES as u64
    );
    blocked.store(false, Ordering::SeqCst);
    std::fs::write(f.root.join("workspace/seed.txt"), "Later leader edit").unwrap();
    prepare_workspace(&launch, &identity, &cache, &destination)
        .await
        .unwrap();
    assert_eq!(std::fs::read(destination.join("seed.txt")).unwrap(), bytes);
    std::fs::write(destination.join("seed.txt"), "Retained worker edit").unwrap();
    prepare_workspace(&launch, &identity, &cache, &destination)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(destination.join("seed.txt")).unwrap(),
        "Retained worker edit"
    );
    let transport = WorkerTransport::new(&launch.leader_origin, &launch.worker_bearer).unwrap();
    let read = f.request(
        "foreign-snapshot-blob",
        WorkerOperation::SnapshotRead {
            request: SnapshotRead {
                snapshot: digest,
                blob: Some(beale_storage::swarm_files::hash(b"foreign-content")),
                offset: 0,
            },
        },
    );
    assert!(transport.send(&read, Duration::from_secs(5)).await.is_err());
    f.store.local_stop("scan-example", &f.deputy).unwrap();
    assert!(
        prepare_workspace(&launch, &identity, &cache, &destination)
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(destination.join("seed.txt")).unwrap(),
        "Retained worker edit"
    );
    server.abort();
    let _ = server.await;
    f.close().await;
}
fn arguments() -> Vec<OsString> {
    [
        "--ignored",
        "--exact",
        "distributed::gateway_tests::assigned_worker_process_fixture",
    ]
    .map(OsString::from)
    .to_vec()
}

#[tokio::test]
async fn peer_host_runs_assigned_worker_once_and_preserves_leader_owned_results() {
    let root = std::env::temp_dir().join(format!("beale-peer-host-example-{}", Uuid::new_v4()));
    let db = beale_storage::HostDatabase::open(&root).unwrap();
    let store = FleetExecutionStore::open(db.clone()).unwrap();
    let f = Fixture::with_selection(
        true,
        WorkerShell::native(),
        false,
        "workspace-example",
        Some(Destination {
            host_id: store.host_id().unwrap(),
            configuration_id: "configuration-example".into(),
        }),
    )
    .await;
    // The generic gateway fixture prepares an initial attempt without starting
    // a process. Release it before assigning a new generation to this peer.
    f.store
        .reconcile_worker(&f.attempt.id, true, WorkerDisposition::Failed, unix_ms())
        .unwrap();
    f.store
        .local_message(
            "scan-example",
            "resume-on-peer",
            &f.deputy,
            "Continue synthetic research",
        )
        .unwrap();
    std::fs::create_dir_all(f.root.join("workspace")).unwrap();
    std::fs::write(f.root.join("workspace/seed.txt"), "Synthetic sealed input").unwrap();
    crate::research_files::capture(&f.state, "scan-example", &f.workspace_id).unwrap();
    let snapshot = crate::research_files::store(&f.state)
        .unwrap()
        .snapshot("scan-example")
        .unwrap()
        .0;
    std::fs::write(f.root.join("workspace/seed.txt"), "Later leader edit").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let app = axum::Router::new().route("/v1/chat/completions", axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
        let counter = counter.clone();
        async move {
            assert_eq!(body["model"], "example");
            assert!(body["messages"][0]["content"].as_str().unwrap().contains("Only synthetic example.test fixtures"));
            let index = counter.fetch_add(1, Ordering::SeqCst);
            let delta = match index {
                0 => json!({"tool_calls":[{"index":0,"id":"write-example","type":"function","function":{"name":"write","arguments":json!({"path":"result.txt","content":"Synthetic peer worker output"}).to_string()}}]}),
                1 => json!({"tool_calls":[{"index":0,"id":"message-example","type":"function","function":{"name":"swarm_message_parent","arguments":json!({"message":"Synthetic peer handoff","handoff":null}).to_string()}}]}),
                2 => json!({"content":"Synthetic peer research complete"}),
                _ => panic!("duplicate provider dispatch"),
            };
            let chunk = json!({"choices":[{"delta":delta}]});
            ([("content-type","text/event-stream")], format!("data: {chunk}\n\ndata: [DONE]\n\n"))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    f.state
        .credentials
        .save_custom_provider(beale_providers::CustomProvider {
            id: "custom-example".into(),
            name: "Example".into(),
            base_url: format!("http://{}/v1", listener.local_addr().unwrap()),
            models: vec![beale_providers::ModelEntry {
                id: "example".into(),
                name: "Example".into(),
            }],
            lead_model: "example".into(),
            small_model: "example".into(),
            reasoning: false,
        })
        .unwrap();
    let provider = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let workspace = f.root.join("peer-workspace");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut peer = beale_fleet::FleetPeer {
        id: store.host_id().unwrap(),
        endpoint: format!("http://{}", listener.local_addr().unwrap()),
        control_token: "b".repeat(64),
        enabled: true,
        protocol_major: beale_contracts::PROTOCOL_MAJOR,
    };
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
        host_id: peer.id.clone(),
        configuration_id: "configuration-example".into(),
        configuration_revision: 1,
        instance_id: "instance-example".into(),
        shell: WorkerShell::native(),
    };
    let mut owned = coordinator
        .claim("scan-example", &f.deputy, 64, &placement)
        .unwrap()
        .unwrap();
    let target = owned.record().target.clone();
    assert!(
        coordinator
            .acquire_recovery(&target.identity.attempt_id)
            .unwrap()
            .is_none()
    );
    let key = Uuid::new_v4();
    let environments = Arc::new(NativeFixture {
        target: target.clone(),
        key,
        root: root.clone(),
        workspace: workspace.clone(),
        starts: AtomicUsize::new(0),
    });
    let backend = Arc::new(AssignedWorkerBackend::new(
        root.clone(),
        environments.clone(),
    ));
    let lost = Arc::new(AtomicBool::new(true));
    let app =
        beale_fleet::worker_host::router("b".repeat(64), db.clone(), root.clone(), backend.clone())
            .layer(middleware::from_fn(
                move |request: Request<Body>, next: Next| {
                    let lost = lost.clone();
                    async move {
                        let response = next.run(request).await;
                        if response.status().is_success() && lost.swap(false, Ordering::SeqCst) {
                            Response::builder()
                                .status(StatusCode::SERVICE_UNAVAILABLE)
                                .body(Body::empty())
                                .unwrap()
                        } else {
                            response
                        }
                    }
                },
            ));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let (_cancel, canceled) = tokio::sync::watch::channel(false);
    let failed = tokio::time::timeout(
        Duration::from_secs(30),
        coordinator.run(&mut owned, Some(snapshot), canceled, |_, _| async {
            anyhow::bail!("synthetic checkpoint failure")
        }),
    )
    .await
    .unwrap();
    assert!(
        failed
            .unwrap_err()
            .to_string()
            .contains("synthetic checkpoint failure")
    );
    let record = f.store.peer_worker(&target.identity.attempt_id).unwrap();
    assert!(record.terminated());
    assert!(!record.complete);
    assert_eq!(record.disposition, Some(WorkerDisposition::Completed));
    assert_eq!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .attempts
            .iter()
            .find(|a| a.id == target.identity.attempt_id)
            .unwrap()
            .state,
        ExecutionState::TerminationUnconfirmed
    );
    assert_eq!(
        f.state.db.session(&f.deputy).unwrap().unwrap().turns.len(),
        1
    );
    // A new owner is recovery-only, including when new placements are disabled.
    drop(owned);
    peer.enabled = false;
    assert!(
        PeerWorkerCoordinator::new(
            f.state.db.clone(),
            f.state.workspaces.root().to_path_buf(),
            &peer,
            f.url.clone()
        )
        .is_err()
    );
    let recovery = PeerWorkerCoordinator::for_recovery(
        f.state.db.clone(),
        f.state.workspaces.root().to_path_buf(),
        &peer,
    )
    .unwrap();
    assert!(
        recovery
            .claim("scan-example", &f.deputy, 64, &placement)
            .is_err()
    );
    let mut owned = recovery
        .acquire_recovery(&target.identity.attempt_id)
        .unwrap()
        .unwrap();
    // Crash between canonical acceptance and the separate completion marker.
    f.state.db.with_connection(|c| { c.execute_batch("CREATE TRIGGER fail_peer_completion BEFORE UPDATE ON swarm_peer_workers WHEN NEW.complete=1 BEGIN SELECT RAISE(ABORT,'synthetic completion failure'); END;")?; Ok(()) }).unwrap();
    let checkpoint = f.root.join("synthetic-peer-checkpoint.json");
    let checkpoint_bytes = serde_json::to_vec(&json!({
        "seed":std::fs::read_to_string(workspace.join("seed.txt")).unwrap(),
        "output":std::fs::read_to_string(workspace.join("result.txt")).unwrap(),
        "context":f.store.worker_progress(&target.identity.attempt_id).unwrap().context,
    }))
    .unwrap();
    assert!(
        recovery
            .recover(&mut owned, |_, _| {
                let checkpoint = checkpoint.clone();
                let bytes = checkpoint_bytes.clone();
                async move {
                    use std::io::Write;
                    let mut file = std::fs::File::create(checkpoint)?;
                    file.write_all(&bytes)?;
                    file.sync_all()?;
                    Ok(())
                }
            })
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&checkpoint).unwrap(), checkpoint_bytes);
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
    drop(owned);
    let mut owned = recovery
        .acquire_recovery(&target.identity.attempt_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        recovery
            .recover(&mut owned, |_, _| async {
                panic!("accepted checkpoint must not run again")
            })
            .await
            .unwrap(),
        WorkerReconciliation::TurnAccepted
    );
    assert!(
        f.store
            .peer_worker(&target.identity.attempt_id)
            .unwrap()
            .complete
    );
    assert!(
        recovery
            .acquire_recovery(&target.identity.attempt_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("seed.txt")).unwrap(),
        "Synthetic sealed input"
    );
    assert!(!f.root.join("workspace/result.txt").exists());
    assert_eq!(environments.starts.load(Ordering::SeqCst), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        std::fs::read_to_string(workspace.join("result.txt")).unwrap(),
        "Synthetic peer worker output"
    );
    assert_eq!(
        f.store
            .worker_progress(&target.identity.attempt_id)
            .unwrap()
            .answer
            .as_deref(),
        Some("Synthetic peer research complete")
    );
    assert_eq!(
        f.store
            .local_state("scan-example")
            .unwrap()
            .inbox
            .iter()
            .filter(|m| m.message == "Synthetic peer handoff")
            .count(),
        1
    );
    // The destination owns supervision only. Canonical state and acceptance
    // remain on the leader; process exit does not commit a turn on its own.
    db.with_connection(|c| {
        let count: i64 = c.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name LIKE 'swarm_%'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(count, 0);
        Ok(())
    })
    .unwrap();
    assert_eq!(
        f.state.db.session(&f.deputy).unwrap().unwrap().turns.len(),
        2
    );
    assert_eq!(environments.starts.load(Ordering::SeqCst), 1);
    drop(owned);
    server.abort();
    let _ = server.await;
    provider.abort();
    let _ = provider.await;
    drop(backend);
    drop(store);
    drop(db);
    drop(environments);
    std::fs::remove_dir_all(root).unwrap();
    drop(coordinator);
    drop(recovery);
    f.close().await;
}
