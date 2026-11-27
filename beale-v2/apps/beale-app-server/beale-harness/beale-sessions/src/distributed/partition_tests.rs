//! Real worker/command ownership across a transport partition. Synthetic
//! provider responses and a faulting gateway leave native process handling real.
use super::*;
use beale_contracts::swarm_worker::{WorkRequest, WorkerShell};
use beale_storage::worker_spool::WorkerSpool;
use std::sync::atomic::AtomicUsize;
use tokio::sync::watch;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scenario {
    Partition,
    Transient,
    Cancel,
}

// A timed-out request can cancel the middleware future while it is waiting.
struct Inflight(Arc<AtomicUsize>);
impl Drop for Inflight {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn running_command_partition_requires_termination_and_parent_reconciliation() {
    scenario(Scenario::Partition).await;
}

#[tokio::test]
async fn running_command_recovers_transient_transport_without_reexecution() {
    scenario(Scenario::Transient).await;
}

#[tokio::test]
async fn running_command_cancel_during_reconnect_cannot_resurrect_branch() {
    scenario(Scenario::Cancel).await;
}

async fn scenario(scenario: Scenario) {
    let f = Fixture::with_shell(true, WorkerShell::native(), true).await;
    std::fs::create_dir_all(f.root.join("workspace")).unwrap();
    crate::research_files::capture(&f.state, "scan-example", &f.workspace_id).unwrap();
    let files = crate::research_files::store(&f.state).unwrap();
    let workspace = files.prepare_agent("scan-example", &f.deputy).unwrap();
    let original = f.store.worker_progress(&f.attempt.id).unwrap().turn;

    // Queue an unrelated assignment before introducing the partition.
    f.store
        .local_message(
            "scan-example",
            "queue-sibling",
            &f.leader,
            "Coordinate independent synthetic work",
        )
        .unwrap();
    let leader = f
        .store
        .local_claim("scan-example", &f.leader, unix_ms(), 64)
        .unwrap()
        .unwrap();
    let sibling = f
        .store
        .local_dispatch(
            &leader,
            "spawn-sibling",
            SwarmCommand::Spawn(Spawn {
                handoff_id: None,
                profile: f
                    .store
                    .load("scan-example", true)
                    .unwrap()
                    .agent(&f.deputy)
                    .unwrap()
                    .summary
                    .profile
                    .clone(),
                task: task(),
                independent: false,
            }),
            Admission::Ready,
        )
        .unwrap()["child_id"]
        .as_str()
        .unwrap()
        .to_owned();
    f.store
        .local_finish(&leader, json!({}), "Awaiting both branches", false)
        .unwrap();

    let primary_calls = Arc::new(AtomicUsize::new(0));
    let sibling_calls = Arc::new(AtomicUsize::new(0));
    let calls = primary_calls.clone();
    let other_calls = sibling_calls.clone();
    let primary = f.deputy.clone();
    let steps = if scenario == Scenario::Transient {
        15
    } else {
        150
    };
    #[cfg(windows)]
    let command = format!(
        "Add-Content -LiteralPath started.txt -Value started; for ($i=0; $i -lt {steps}; $i++) {{ Add-Content -LiteralPath ticks.txt -Value tick; Start-Sleep -Milliseconds 200 }}; Set-Content -LiteralPath finished.txt -Value finished"
    );
    #[cfg(not(windows))]
    let command = format!(
        "printf 'started\\n' >> started.txt; i=0; while [ $i -lt {steps} ]; do printf 'tick\\n' >> ticks.txt; sleep 0.2; i=$((i+1)); done; printf finished > finished.txt"
    );
    let provider = axum::Router::new().route("/v1/chat/completions", axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
        let calls=calls.clone(); let other_calls=other_calls.clone(); let primary=primary.clone(); let command=command.clone();
        async move {
            let system=body["messages"][0]["content"].as_str().unwrap();
            let delta=if system.contains(&primary) {
                let count=calls.fetch_add(1,Ordering::SeqCst);
                if count==0 { json!({"tool_calls":[{"index":0,"id":"partition-command","type":"function","function":{"name":beale_tools::COMMAND_TOOL_NAME,"arguments":json!({"command":command,"timeout_seconds":60}).to_string()}}]}) }
                else { json!({"content":"Synthetic research reconciled"}) }
            } else {
                other_calls.fetch_add(1,Ordering::SeqCst);
                json!({"content":"Synthetic sibling complete"})
            };
            let chunk=json!({"choices":[{"delta":delta}]});
            ([("content-type","text/event-stream")],format!("data: {chunk}\n\ndata: [DONE]\n\n"))
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
        axum::serve(listener, provider).await.unwrap();
    });

    let (partition, gate) = watch::channel(false);
    let transient = Arc::new(AtomicUsize::new(0));
    let failed_requests = Arc::new(AtomicUsize::new(0));
    let inflight = Arc::new(AtomicUsize::new(0));
    let remaining = transient.clone();
    let failures = failed_requests.clone();
    let active = inflight.clone();
    let gateway = super::super::gateway::router()
        .with_state(f.state.clone())
        .layer(middleware::from_fn(
            move |request: Request<Body>, next: Next| {
                let mut gate = gate.clone();
                let remaining = remaining.clone();
                let failures = failures.clone();
                let active = active.clone();
                async move {
                    if *gate.borrow() {
                        active.fetch_add(1, Ordering::SeqCst);
                        let _inflight = Inflight(active);
                        failures.fetch_add(1, Ordering::SeqCst);
                        while *gate.borrow_and_update() {
                            if gate.changed().await.is_err() {
                                break;
                            }
                        }
                        return Response::builder()
                            .status(StatusCode::SERVICE_UNAVAILABLE)
                            .body(Body::empty())
                            .unwrap();
                    }
                    if remaining
                        .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                        .is_ok()
                    {
                        failures.fetch_add(1, Ordering::SeqCst);
                        return Response::builder()
                            .status(StatusCode::SERVICE_UNAVAILABLE)
                            .body(Body::empty())
                            .unwrap();
                    }
                    let response = next.run(request).await;
                    let (parts, body) = response.into_parts();
                    let bytes = axum::body::to_bytes(body, MAX_DOCUMENT_BYTES + 4096)
                        .await
                        .unwrap();
                    if let Ok(WorkerReply::Lease { mut grant }) =
                        serde_json::from_slice::<WorkerReply>(&bytes)
                    {
                        // Exercise the production deadline path with a bounded test lease.
                        grant.valid_for_ms = 4000;
                        Response::from_parts(
                            parts,
                            Body::from(serde_json::to_vec(&WorkerReply::Lease { grant }).unwrap()),
                        )
                    } else {
                        Response::from_parts(parts, Body::from(bytes))
                    }
                }
            },
        ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let gateway = tokio::spawn(async move {
        axum::serve(listener, gateway).await.unwrap();
    });
    let launch = WorkerLaunch {
        version: 1,
        identity: f.store.worker_bootstrap(&f.attempt.id).unwrap().identity,
        leader: url.clone(),
        bearer: f.credential.bearer().into(),
        workspace: workspace.clone(),
        protected_root: f.root.clone(),
        recovery: f.root.join("partition-recovery"),
    };
    let arguments = [
        "--ignored",
        "--exact",
        "distributed::gateway_tests::assigned_worker_process_fixture",
    ]
    .map(std::ffi::OsString::from);
    let mut process =
        WorkerProcess::spawn(&std::env::current_exe().unwrap(), &arguments, &workspace)
            .await
            .unwrap();
    let transport = WorkerTransport::new(&url, f.credential.bearer()).unwrap();
    let bootstrap = serde_json::to_vec(&launch).unwrap();
    let (cancel, receiver) = watch::channel(false);
    let control = async {
        tokio::time::timeout(Duration::from_secs(15), async {
            while !workspace.join("started.txt").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("worker command did not start");
        assert!(
            f.store
                .local_claim("scan-example", &f.deputy, unix_ms(), 64)
                .unwrap()
                .is_none()
        );
        if scenario == Scenario::Transient {
            transient.store(2, Ordering::SeqCst);
        } else {
            partition.send(true).unwrap();
            f.store
                .fence_attempt("scan-example", &f.attempt.id, false)
                .unwrap();
            assert_eq!(
                super::super::supervisor::reconcile_owned_attempt(
                    &f.store,
                    &launch.identity,
                    false,
                    WorkerDisposition::Failed,
                    unix_ms()
                )
                .unwrap(),
                WorkerReconciliation::TerminationUnconfirmed
            );
            assert!(
                f.store
                    .local_claim("scan-example", &f.deputy, unix_ms(), 64)
                    .unwrap()
                    .is_none()
            );
            assert!(
                f.store
                    .start_attempt(
                        "scan-example",
                        &f.deputy,
                        &registration(),
                        unix_ms(),
                        unix_ms() + 30_000
                    )
                    .is_err()
            );
            if scenario == Scenario::Cancel {
                // Wait for an actual reconnect request to be in flight; canceling
                // immediately after setting the flag would only test ordinary stop.
                tokio::time::timeout(Duration::from_secs(5), async {
                    while inflight.load(Ordering::SeqCst) == 0 {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .expect("no reconnect request reached the partition");
                f.store.local_stop("scan-example", &f.deputy).unwrap();
                cancel.send(true).unwrap();
            }
        }
        let sibling_turn = f
            .store
            .local_claim("scan-example", &sibling, unix_ms(), 64)
            .unwrap()
            .unwrap();
        let host = LocalWorkerHost::start(f.state.clone()).await.unwrap();
        assert_eq!(
            host.run(&sibling_turn, None).await.unwrap(),
            WorkerReconciliation::TurnAccepted
        );
        if scenario != Scenario::Transient {
            assert!(*partition.borrow());
        }
    };
    let (report, ()) = tokio::join!(
        tokio::time::timeout(
            Duration::from_secs(20),
            supervise(
                &mut process,
                launch.identity.clone(),
                &transport,
                &bootstrap,
                receiver
            )
        ),
        control,
    );
    let report = report.unwrap().unwrap();
    assert!(report.termination_confirmed);
    assert!(process.confirm_stopped().unwrap());
    assert_eq!(sibling_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .agent(&sibling)
            .unwrap()
            .summary
            .state,
        AgentState::Completed
    );
    if scenario == Scenario::Transient {
        assert_eq!(report.reason, StopReason::RootExited);
        assert_eq!(failed_requests.load(Ordering::SeqCst), 2);
        assert_eq!(primary_calls.load(Ordering::SeqCst), 2);
        assert!(workspace.join("finished.txt").exists());
    } else {
        if scenario == Scenario::Cancel {
            assert_eq!(report.reason, StopReason::Canceled);
        } else {
            // The worker and its supervisor have independent monotonic leases.
            // Either may reach expiry first and terminate the managed execution.
            assert!(matches!(
                report.reason,
                StopReason::LeaseExpired | StopReason::RootExited
            ));
        }
        assert!(failed_requests.load(Ordering::SeqCst) > 0);
        assert!(!workspace.join("finished.txt").exists());
        assert_eq!(primary_calls.load(Ordering::SeqCst), 1);
        // Fencing and even OS termination have not yet released durable ownership.
        assert!(
            f.store
                .local_claim("scan-example", &f.deputy, unix_ms(), 64)
                .unwrap()
                .is_none()
        );
    }
    partition.send(false).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while inflight.load(Ordering::SeqCst) > 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let spool = WorkerSpool::open(&launch.recovery, launch.identity.clone()).unwrap();
    if scenario == Scenario::Transient {
        spool.require_known_outcomes().unwrap();
    } else {
        assert!(spool.require_known_outcomes().is_err());
        assert!(spool.pending(0,256).unwrap().iter().any(|operation|matches!(&operation.request,WorkRequest::Environment{call} if call.id=="partition-command") && operation.state==ToolOperationState::OutcomeUnknown));
    }
    drop(spool);
    f.store
        .fence_attempt("scan-example", &f.attempt.id, false)
        .unwrap();
    let progress = f.store.worker_progress(&f.attempt.id).unwrap();
    files
        .checkpoint("scan-example", &f.deputy, &f.attempt.id, &progress.context)
        .unwrap();
    let disposition = match scenario {
        Scenario::Transient => WorkerDisposition::Completed,
        Scenario::Cancel => WorkerDisposition::Canceled,
        Scenario::Partition => WorkerDisposition::Failed,
    };
    super::super::supervisor::reconcile_owned_attempt(
        &f.store,
        &launch.identity,
        true,
        disposition,
        unix_ms(),
    )
    .unwrap();
    let tick_bytes = std::fs::read(workspace.join("ticks.txt")).unwrap();
    if scenario == Scenario::Partition {
        assert_eq!(
            f.store
                .load("scan-example", true)
                .unwrap()
                .agent(&f.deputy)
                .unwrap()
                .summary
                .state,
            AgentState::Blocked
        );
        assert!(
            f.store
                .local_claim("scan-example", &f.deputy, unix_ms(), 64)
                .unwrap()
                .is_none()
        );
        let parent = f
            .store
            .local_claim("scan-example", &f.leader, unix_ms(), 64)
            .unwrap()
            .unwrap();
        assert!(parent.prompt.contains("Reconcile uncertain effects"));
        f.store
            .local_dispatch(
                &parent,
                "reconciled-replacement",
                SwarmCommand::MessageChild(MessageChild {
                    child_id: f.deputy.clone(),
                    message: "Synthetic side effects reconciled; continue with a fresh turn".into(),
                }),
                Admission::Ready,
            )
            .unwrap();
        f.store
            .local_finish(&parent, json!({}), "Awaiting reconciled research", false)
            .unwrap();
        let replacement = f
            .store
            .local_claim("scan-example", &f.deputy, unix_ms(), 64)
            .unwrap()
            .unwrap();
        assert_eq!(replacement.generation, original.generation + 1);
        let host = LocalWorkerHost::start(f.state.clone()).await.unwrap();
        assert_eq!(
            host.run(&replacement, None).await.unwrap(),
            WorkerReconciliation::TurnAccepted
        );
        assert_eq!(primary_calls.load(Ordering::SeqCst), 2);
    } else if scenario == Scenario::Cancel {
        assert!(
            f.store
                .local_message(
                    "scan-example",
                    "late-reconnect",
                    &f.deputy,
                    "Resume synthetic research"
                )
                .is_err()
        );
        assert!(
            f.store
                .local_claim("scan-example", &f.deputy, unix_ms(), 64)
                .unwrap()
                .is_none()
        );
    }
    let saved = f.store.load("scan-example", true).unwrap();
    assert!(
        f.client()
            .send(
                &f.request(
                    "late-result",
                    WorkerOperation::Finish {
                        revision: progress.revision,
                        answer: "Stale synthetic result".into()
                    }
                ),
                Duration::from_secs(5)
            )
            .await
            .is_err()
    );
    assert_eq!(f.store.load("scan-example", true).unwrap(), saved);
    assert_eq!(
        std::fs::read(workspace.join("ticks.txt")).unwrap(),
        tick_bytes
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("started.txt"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    drop(process);
    drop(files);
    drop(transport);
    gateway.abort();
    let _ = gateway.await;
    provider.abort();
    let _ = provider.await;
    f.close().await;
}
