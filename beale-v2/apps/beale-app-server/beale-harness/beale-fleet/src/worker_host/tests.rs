use super::*;
use beale_contracts::swarm_worker::ExecutionIdentity;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

struct Backend {
    launches: AtomicUsize,
    stops: AtomicUsize,
    gate: AtomicBool,
    fail: AtomicBool,
    stop_uncertain: AtomicBool,
    entered: tokio::sync::Notify,
    resume: tokio::sync::Notify,
    state: Mutex<BackendObservation>,
}
impl Default for Backend {
    fn default() -> Self {
        Self {
            launches: AtomicUsize::new(0),
            stops: AtomicUsize::new(0),
            gate: AtomicBool::new(false),
            fail: AtomicBool::new(false),
            stop_uncertain: AtomicBool::new(false),
            entered: tokio::sync::Notify::new(),
            resume: tokio::sync::Notify::new(),
            state: Mutex::new(BackendObservation::TerminationUnconfirmed),
        }
    }
}
impl HostExecutionBackend for Backend {
    fn available(&self) -> bool {
        true
    }
    fn validate(&self, target: &ExecutionTarget) -> Result<(), BackendRejection> {
        if target.configuration_id == "configuration-example" {
            Ok(())
        } else {
            Err(BackendRejection::Conflict)
        }
    }
    fn launch<'a>(
        &'a self,
        _: &'a ExecutionTarget,
        launch: &'a HostWorkerLaunch,
    ) -> BackendFuture<'a> {
        Box::pin(async move {
            assert_eq!(launch.worker_bearer, "c".repeat(64));
            self.launches.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_one();
            if self.gate.load(Ordering::SeqCst) {
                self.resume.notified().await;
            }
            if self.fail.load(Ordering::SeqCst) {
                anyhow::bail!("synthetic interrupted launch");
            }
            *self.state.lock().unwrap() = BackendObservation::Running;
            Ok(BackendObservation::Running)
        })
    }
    fn inspect<'a>(&'a self, _: &'a ExecutionTarget) -> BackendFuture<'a> {
        Box::pin(async { Ok(*self.state.lock().unwrap()) })
    }
    fn stop<'a>(&'a self, _: &'a ExecutionTarget) -> BackendFuture<'a> {
        Box::pin(async {
            self.stops.fetch_add(1, Ordering::SeqCst);
            if self.stop_uncertain.load(Ordering::SeqCst) {
                return Ok(BackendObservation::TerminationUnconfirmed);
            }
            *self.state.lock().unwrap() = BackendObservation::Stopped;
            Ok(BackendObservation::Stopped)
        })
    }
}

struct Fixture {
    root: PathBuf,
    db: HostDatabase,
    store: FleetExecutionStore,
    backend: Arc<Backend>,
    client: HostExecutionClient,
    server: tokio::task::JoinHandle<()>,
    origin: String,
}
impl Fixture {
    async fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "beale-peer-execution-example-{}",
            uuid::Uuid::new_v4()
        ));
        let db = HostDatabase::open(&root).unwrap();
        let store = FleetExecutionStore::open(db.clone()).unwrap();
        let backend = Arc::new(Backend::default());
        let app = router("b".repeat(64), db.clone(), root.clone(), backend.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let client = client(&origin, &store.host_id().unwrap());
        let server = tokio::spawn(async {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            root,
            db,
            store,
            backend,
            client,
            server,
            origin,
        }
    }
    fn request(&self) -> HostExecutionRequest {
        HostExecutionRequest {
            version: 1,
            operation_id: "start-example".into(),
            target: ExecutionTarget {
                host_id: self.store.host_id().unwrap(),
                leader_id: "leader-example".into(),
                configuration_id: "configuration-example".into(),
                configuration_revision: 1,
                instance_id: "instance-example".into(),
                identity: ExecutionIdentity {
                    workspace_id: "workspace-example".into(),
                    scan_id: "scan-example".into(),
                    agent_id: "agent-example".into(),
                    attempt_id: "attempt-example".into(),
                    generation: 1,
                },
            },
            operation: HostExecutionOperation::Start {
                launch: HostWorkerLaunch {
                    snapshot: None,
                    leader_origin: "https://leader.example.ts.net".into(),
                    worker_bearer: "c".repeat(64),
                },
            },
        }
    }
    async fn send(&self, request: &HostExecutionRequest) -> HostExecutionReply {
        self.client
            .send(request, Duration::from_secs(5))
            .await
            .unwrap()
    }
    async fn close(self) {
        self.server.abort();
        let _ = self.server.await;
        drop(self.client);
        drop(self.store);
        drop(self.db);
        let _ = std::fs::remove_dir_all(self.root);
    }
}
fn client(origin: &str, host: &str) -> HostExecutionClient {
    HostExecutionClient::for_peer(&crate::FleetPeer {
        id: host.into(),
        endpoint: origin.into(),
        control_token: "b".repeat(64),
        enabled: true,
        protocol_major: beale_contracts::PROTOCOL_MAJOR,
    })
    .unwrap()
}

#[tokio::test]
async fn paired_transport_launches_once_and_stop_tombstones_prevent_resurrection() {
    let f = Fixture::new().await;
    let start = f.request();
    let discovered =
        HostExecutionClient::discover(&f.origin, &"b".repeat(64), Duration::from_secs(5))
            .await
            .unwrap();
    assert_eq!(discovered.host_id, start.target.host_id);
    assert!(discovered.execution_available);
    let first = f.send(&start).await;
    assert_eq!(first.report.phase, HostExecutionPhase::Running);
    assert_eq!(f.send(&start).await, first);
    let mut replay = start.clone();
    replay.operation_id = "replay-start".into();
    assert_eq!(f.send(&replay).await.report, first.report);
    assert_eq!(f.backend.launches.load(Ordering::SeqCst), 1);
    let mut stop = start.clone();
    stop.operation_id = "stop-example".into();
    stop.operation = HostExecutionOperation::Stop {};
    assert_eq!(
        f.send(&stop).await.report.phase,
        HostExecutionPhase::Stopped
    );
    assert_eq!(
        f.send(&start).await.report.phase,
        HostExecutionPhase::Stopped
    );
    let mut future = start.clone();
    future.operation_id = "future-start".into();
    future.target.identity.attempt_id = "attempt-future".into();
    future.target.identity.generation = 2;
    stop = future.clone();
    stop.operation_id = "future-stop".into();
    stop.operation = HostExecutionOperation::Stop {};
    assert_eq!(
        f.send(&stop).await.report.phase,
        HostExecutionPhase::Stopped
    );
    assert_eq!(
        f.send(&future).await.report.phase,
        HostExecutionPhase::Stopped
    );
    assert_eq!(f.backend.launches.load(Ordering::SeqCst), 1);
    f.close().await;
}

#[tokio::test]
async fn restarted_host_maintenance_observes_pending_work_without_launching_again() {
    let f = Fixture::new().await;
    let start = f.request();
    // Simulate loss after the durable launch claim, before its acknowledgement.
    assert!(f.store.accept(&start).unwrap().launch);
    *f.backend.state.lock().unwrap() = BackendObservation::Running;
    let reopened = HostDatabase::open(&f.root).unwrap();
    let mut cursor = None;
    recover_page(&reopened, &f.root, f.backend.clone(), &mut cursor)
        .await
        .unwrap();
    assert_eq!(
        f.store.inspect(&start.target).unwrap().unwrap().phase,
        HostExecutionPhase::Running
    );
    assert_eq!(f.backend.launches.load(Ordering::SeqCst), 0);
    let mut stop = start.clone();
    stop.operation_id = "stop-restarted".into();
    stop.operation = HostExecutionOperation::Stop {};
    f.store.accept(&stop).unwrap();
    let state = HostState {
        token: String::new(),
        db: reopened.clone(),
        root: f.root.clone(),
        backend: f.backend.clone(),
    };
    let owner = lock(&state, &start.target).unwrap().unwrap();
    recover_page(&reopened, &f.root, f.backend.clone(), &mut cursor)
        .await
        .unwrap();
    assert_eq!(f.backend.stops.load(Ordering::SeqCst), 0);
    drop(owner);
    recover_page(&reopened, &f.root, f.backend.clone(), &mut cursor)
        .await
        .unwrap();
    assert_eq!(
        f.store.inspect(&start.target).unwrap().unwrap().phase,
        HostExecutionPhase::Stopped
    );
    assert_eq!(f.backend.stops.load(Ordering::SeqCst), 1);
    assert_eq!(f.backend.launches.load(Ordering::SeqCst), 0);
    drop(state);
    drop(reopened);
    f.close().await;
}

#[tokio::test]
async fn uncertain_stop_keeps_cancellation_intent_for_restarted_host_recovery() {
    let f = Fixture::new().await;
    let start = f.request();
    f.send(&start).await;
    f.backend.stop_uncertain.store(true, Ordering::SeqCst);
    let mut stop = start.clone();
    stop.operation_id = "uncertain-stop".into();
    stop.operation = HostExecutionOperation::Stop {};
    let report = f.send(&stop).await.report;
    assert_eq!(report.phase, HostExecutionPhase::TerminationUnconfirmed);
    assert!(report.stop_requested);
    let reopened = HostDatabase::open(&f.root).unwrap();
    let mut cursor = None;
    recover_page(&reopened, &f.root, f.backend.clone(), &mut cursor)
        .await
        .unwrap();
    assert_eq!(f.backend.stops.load(Ordering::SeqCst), 2);
    assert_eq!(
        f.store.inspect(&start.target).unwrap().unwrap().phase,
        HostExecutionPhase::TerminationUnconfirmed
    );
    f.backend.stop_uncertain.store(false, Ordering::SeqCst);
    recover_page(&reopened, &f.root, f.backend.clone(), &mut cursor)
        .await
        .unwrap();
    assert_eq!(
        f.store.inspect(&start.target).unwrap().unwrap().phase,
        HostExecutionPhase::Stopped
    );
    assert_eq!(f.backend.launches.load(Ordering::SeqCst), 1);
    drop(reopened);
    f.close().await;
}

#[tokio::test]
async fn stop_during_launch_commits_before_busy_backend_and_wins_the_acknowledgement() {
    let f = Fixture::new().await;
    let start = f.request();
    f.backend.gate.store(true, Ordering::SeqCst);
    let client = f.client.clone();
    let launching = start.clone();
    let pending = tokio::spawn(async move {
        client
            .send(&launching, Duration::from_secs(5))
            .await
            .unwrap()
    });
    tokio::time::timeout(Duration::from_secs(3), f.backend.entered.notified())
        .await
        .unwrap();
    let mut stop = start.clone();
    stop.operation_id = "stop-during-launch".into();
    stop.operation = HostExecutionOperation::Stop {};
    assert_eq!(
        f.send(&stop).await.report.phase,
        HostExecutionPhase::StopRequested
    );
    f.backend.resume.notify_one();
    assert_eq!(
        pending.await.unwrap().report.phase,
        HostExecutionPhase::Stopped
    );
    assert_eq!(f.backend.launches.load(Ordering::SeqCst), 1);
    assert_eq!(f.backend.stops.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.send(&start).await.report.phase,
        HostExecutionPhase::Stopped
    );
    f.close().await;
}

#[tokio::test]
async fn failed_acknowledgement_persistence_never_redispatches_backend_launch() {
    let f = Fixture::new().await;
    let start = f.request();
    f.db.with_connection(|db| { db.execute_batch("CREATE TRIGGER fail_receipt BEFORE INSERT ON fleet_execution_operations BEGIN SELECT RAISE(ABORT,'synthetic receipt full'); END;")?; Ok(()) }).unwrap();
    assert_eq!(
        f.client
            .send(&start, Duration::from_secs(5))
            .await
            .unwrap_err()
            .kind,
        PeerFailure::Retryable
    );
    assert_eq!(f.backend.launches.load(Ordering::SeqCst), 0);
    f.db.with_connection(|db| { db.execute_batch("DROP TRIGGER fail_receipt; CREATE TRIGGER fail_observe BEFORE UPDATE ON fleet_executions BEGIN SELECT RAISE(ABORT,'synthetic observation full'); END;")?; Ok(()) }).unwrap();
    assert_eq!(
        f.client
            .send(&start, Duration::from_secs(5))
            .await
            .unwrap_err()
            .kind,
        PeerFailure::Retryable
    );
    assert_eq!(f.backend.launches.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.store.inspect(&start.target).unwrap().unwrap().phase,
        HostExecutionPhase::LaunchRequested
    );
    f.db.with_connection(|db| {
        db.execute_batch("DROP TRIGGER fail_observe;")?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        f.send(&start).await.report.phase,
        HostExecutionPhase::Running
    );
    assert_eq!(f.backend.launches.load(Ordering::SeqCst), 1);
    f.close().await;
}

#[tokio::test]
async fn uncertain_launch_remains_owned_across_reopen_and_cannot_be_replaced() {
    let f = Fixture::new().await;
    let start = f.request();
    f.backend.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        f.send(&start).await.report.phase,
        HostExecutionPhase::TerminationUnconfirmed
    );
    let reopened = FleetExecutionStore::open(HostDatabase::open(&f.root).unwrap()).unwrap();
    assert_eq!(
        reopened.inspect(&start.target).unwrap().unwrap().phase,
        HostExecutionPhase::TerminationUnconfirmed
    );
    assert_eq!(
        f.send(&start).await.report.phase,
        HostExecutionPhase::TerminationUnconfirmed
    );
    assert_eq!(f.backend.launches.load(Ordering::SeqCst), 1);
    let mut replacement = start.clone();
    replacement.operation_id = "replacement".into();
    replacement.target.identity.attempt_id = "attempt-replacement".into();
    replacement.target.identity.generation += 1;
    assert_eq!(
        f.client
            .send(&replacement, Duration::from_secs(5))
            .await
            .unwrap_err()
            .kind,
        PeerFailure::Rejected
    );
    *f.backend.state.lock().unwrap() = BackendObservation::Stopped;
    let mut inspect = start;
    inspect.operation_id = "inspect".into();
    inspect.operation = HostExecutionOperation::Inspect {};
    assert_eq!(
        f.send(&inspect).await.report.phase,
        HostExecutionPhase::Stopped
    );
    drop(reopened);
    f.close().await;
}

#[tokio::test]
async fn host_endpoint_rejects_worker_authority_and_unbounded_or_rebound_requests() {
    let f = Fixture::new().await;
    let start = f.request();
    let http = reqwest::Client::new();
    let endpoint = format!("{}{}", f.origin, HOST_EXECUTION_PATH);
    let response = http
        .post(&endpoint)
        .bearer_auth("c".repeat(64))
        .json(&start)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let mut invalid = serde_json::to_value(&start).unwrap();
    invalid["operation"]["launch"]["command"] = "synthetic command".into();
    assert_eq!(
        http.post(&endpoint)
            .bearer_auth("b".repeat(64))
            .json(&invalid)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        http.post(&endpoint)
            .bearer_auth("b".repeat(64))
            .header("content-type", "application/json")
            .body(vec![b'x'; HOST_EXECUTION_MAX_BYTES + 1])
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let mut wrong = start.clone();
    wrong.target.host_id = "wrong-host".into();
    assert_eq!(
        http.post(&endpoint)
            .bearer_auth("b".repeat(64))
            .json(&wrong)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(f.backend.launches.load(Ordering::SeqCst), 0);
    f.send(&start).await;
    let mut changed = start;
    changed.target.identity.agent_id = "other-agent".into();
    assert_eq!(
        f.client
            .send(&changed, Duration::from_secs(5))
            .await
            .unwrap_err()
            .kind,
        PeerFailure::Rejected
    );
    drop(http);
    f.close().await;
}

#[tokio::test]
async fn production_fleet_router_advertises_unavailable_backend_without_local_fallback() {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    let f = Fixture::new().await;
    let app = crate::router("b".repeat(64), f.db.clone(), f.root.clone());
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(HOST_EXECUTION_PATH)
                .header("authorization", format!("Bearer {}", "b".repeat(64)))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), HOST_EXECUTION_MAX_BYTES)
        .await
        .unwrap();
    let descriptor: HostExecutionDescriptor = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(descriptor.host_id, f.store.host_id().unwrap());
    assert!(!descriptor.execution_available);
    let start = f.request();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(HOST_EXECUTION_PATH)
                .header("authorization", format!("Bearer {}", "b".repeat(64)))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&start).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    assert!(f.store.inspect(&start.target).unwrap().is_none());
    assert_eq!(f.db.schema_version("swarms").unwrap(), 0);
    f.close().await;
}

#[tokio::test]
async fn peer_client_rejects_redirects_oversized_and_wrong_identity_replies_without_leaking_secrets()
 {
    let f = Fixture::new().await;
    let start = f.request();
    for mode in ["redirect", "oversized", "wrong"] {
        let target = start.target.clone();
        let app = Router::new().route(
            HOST_EXECUTION_PATH,
            axum::routing::post(move || async move {
                use axum::response::IntoResponse;
                match mode {
                    "redirect" => (
                        StatusCode::TEMPORARY_REDIRECT,
                        [("location", "http://127.0.0.1:1/private-example")],
                    )
                        .into_response(),
                    "oversized" => vec![b'x'; HOST_EXECUTION_MAX_BYTES + 1].into_response(),
                    _ => Json(HostExecutionReply {
                        version: 1,
                        operation_id: "wrong-operation".into(),
                        report: HostExecutionReport {
                            target,
                            phase: HostExecutionPhase::Running,
                            revision: 1,
                            stop_requested: false,
                        },
                    })
                    .into_response(),
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let peer = client(
            &format!("http://{}", listener.local_addr().unwrap()),
            &start.target.host_id,
        );
        let server = tokio::spawn(async {
            axum::serve(listener, app).await.unwrap();
        });
        let error = peer.send(&start, Duration::from_secs(5)).await.unwrap_err();
        assert_eq!(error.kind, PeerFailure::Protocol);
        assert!(
            !format!("{error:?}").contains(&"b".repeat(64))
                && !error.to_string().contains("private-example")
        );
        server.abort();
        let _ = server.await;
    }
    assert!(
        HostExecutionClient::for_peer(&crate::FleetPeer {
            id: start.target.host_id.clone(),
            endpoint: f.origin.clone(),
            control_token: "b".repeat(64),
            enabled: false,
            protocol_major: beale_contracts::PROTOCOL_MAJOR
        })
        .is_err()
    );
    assert_eq!(
        f.client
            .send(&start, Duration::ZERO)
            .await
            .unwrap_err()
            .kind,
        PeerFailure::Protocol
    );
    f.close().await;
}
