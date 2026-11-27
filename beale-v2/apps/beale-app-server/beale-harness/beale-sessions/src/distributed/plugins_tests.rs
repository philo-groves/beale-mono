use super::*;
use beale_contracts::swarm_plugins::{Invocation, Outcome, Reply};
use std::sync::atomic::AtomicUsize;
use tokio::sync::Notify;

struct Plugin {
    host: tokio::task::JoinHandle<()>,
    calls: Arc<AtomicUsize>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
}
impl Plugin {
    async fn new(f: &Fixture) -> Self {
        let calls = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let (c, e, r) = (calls.clone(), entered.clone(), release.clone());
        let bearer = f.credential.bearer().to_owned();
        let app=axum::Router::new().route("/mcp",axum::routing::post(move |axum::Json(request):axum::Json<Value>| {
            let (calls,entered,release,bearer)=(c.clone(),e.clone(),r.clone(),bearer.clone());async move {
                let result=match request["method"].as_str().unwrap_or_default() {
                    "server/discover"=>json!({"supportedVersions":["2026-07-28"]}),"tools/list"=>json!({"tools":[{"name":"example","inputSchema":{"type":"object"}}]}),
                    "prompts/list"=>json!({"prompts":[]}),"resources/list"=>json!({"resources":[]}),"resources/templates/list"=>json!({"resourceTemplates":[]}),
                    "tools/call"=>{
                        calls.fetch_add(1,Ordering::SeqCst);
                        match request["params"]["arguments"]["mode"].as_str().unwrap_or_default() {
                            "invalid"=>return axum::Json(json!({"id":request["id"],"jsonrpc":"2.0"})),
                            "wait"=>{entered.notify_one();release.notified().await;json!({"content":[{"type":"text","text":"Synthetic completed effect"}]})},
                            "large"=>json!({"content":[{"type":"text","text":format!("{} host-control-example {bearer}","x".repeat(300_000))}]}),
                            "error"=>json!({"isError":true,"content":[{"type":"text","text":"Synthetic tool failure"}]}),
                            _=>json!({"content":[{"type":"text","text":"Synthetic plugin result"}]}),
                        }
                    },_=>json!({}),
                };
                axum::Json(json!({"jsonrpc":"2.0","id":request["id"],"result":result}))
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let host = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let package = f
            .state
            .workspaces
            .root()
            .join("plugins/packages/example-plugin");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("plugin.json"),json!({"$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json","name":"example-plugin","description":"Synthetic plugin"}).to_string()).unwrap();
        std::fs::write(package.join("mcp.json"),json!({"$schema":"https://agent-plugins.org/schemas/1.0.0/mcp.schema.json","mcpServers":{"main":{"type":"streamable-http","url":format!("http://{address}/mcp")}}}).to_string()).unwrap();
        f.state.db.install_plugin("example-plugin", 1).unwrap();
        Self {
            host,
            calls,
            entered,
            release,
        }
    }
    async fn close(self) {
        self.host.abort();
        let _ = self.host.await;
    }
}
fn plugin_request(f: &Fixture, id: &str, mode: &str) -> WorkerRequest {
    f.request(
        id,
        WorkerOperation::Plugin {
            invocation: Invocation {
                plugin: "example-plugin".into(),
                server: "main".into(),
                tool: "example".into(),
                arguments: json!({"mode":mode}),
            },
        },
    )
}
fn decoded(reply: WorkerReply) -> Outcome {
    serde_json::from_value::<Reply>(acknowledged(reply))
        .unwrap()
        .outcome
}

#[tokio::test]
async fn plugin_capture_and_receipt_failures_replay_without_redispatch_after_uninstall() {
    for table in ["swarm_plugin_calls", "swarm_receipts"] {
        let f = Fixture::new().await;
        let plugin = Plugin::new(&f).await;
        let trigger = if table == "swarm_plugin_calls" {
            "CREATE TRIGGER plugin_disk_full BEFORE UPDATE ON swarm_plugin_calls BEGIN SELECT RAISE(ABORT,'synthetic plugin result full'); END;"
        } else {
            "CREATE TRIGGER plugin_disk_full BEFORE INSERT ON swarm_receipts BEGIN SELECT RAISE(ABORT,'synthetic plugin receipt full'); END;"
        };
        f.state
            .db
            .with_connection(|db| {
                db.execute_batch(trigger)?;
                Ok(())
            })
            .unwrap();
        let request = plugin_request(&f, "plugin-example", "ok");
        assert!(
            f.client()
                .send(&request, Duration::from_secs(5))
                .await
                .is_err()
        );
        assert_eq!(plugin.calls.load(Ordering::SeqCst), 1);
        f.state.db.uninstall_plugin("example-plugin").unwrap();
        f.state
            .db
            .with_connection(|db| {
                db.execute_batch("DROP TRIGGER plugin_disk_full;")?;
                Ok(())
            })
            .unwrap();
        let outcome = decoded(
            f.client()
                .send(&request, Duration::from_secs(5))
                .await
                .unwrap(),
        );
        assert!(matches!(
            outcome,
            Outcome::Completed {
                is_error: false,
                ..
            }
        ));
        assert_eq!(
            decoded(
                f.client()
                    .send(&request, Duration::from_secs(5))
                    .await
                    .unwrap()
            ),
            outcome
        );
        assert_eq!(plugin.calls.load(Ordering::SeqCst), 1);
        let next = plugin_request(&f, "another-plugin-call", "ok");
        assert!(matches!(
            decoded(
                f.client()
                    .send(&next, Duration::from_secs(5))
                    .await
                    .unwrap()
            ),
            Outcome::Completed { is_error: true, .. }
        ));
        plugin.close().await;
        f.close().await;
    }
}

#[tokio::test]
async fn large_plugin_results_publish_complete_redacted_artifacts_before_acknowledgement() {
    let f = Fixture::new().await;
    let plugin = Plugin::new(&f).await;
    let request = plugin_request(&f, "large-plugin-call", "large");
    crate::research_files::store(&f.state).unwrap();
    f.state.db.with_connection(|db|{db.execute_batch("CREATE TRIGGER plugin_transfer_full BEFORE INSERT ON swarm_file_transfers BEGIN SELECT RAISE(ABORT,'synthetic publication full'); END;")?;Ok(())}).unwrap();
    assert!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .is_err()
    );
    assert_eq!(plugin.calls.load(Ordering::SeqCst), 1);
    f.state
        .db
        .with_connection(|db| {
            db.execute_batch("DROP TRIGGER plugin_transfer_full;")?;
            Ok(())
        })
        .unwrap();
    let outcome = decoded(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    let Outcome::Completed {
        output,
        is_error,
        artifact_refs,
    } = outcome
    else {
        panic!("expected complete plugin result");
    };
    assert!(!is_error);
    assert!(output.len() < 40_000);
    assert_eq!(artifact_refs.len(), 1);
    let files = crate::research_files::store(&f.state).unwrap();
    let artifact = files.artifact(&f.workspace_id, &artifact_refs[0]).unwrap();
    let bytes = files.read_blob(&artifact.sha256, artifact.size).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.len() > 300_000);
    assert!(!text.contains("host-control-example"));
    assert!(!text.contains(f.credential.bearer()));
    assert!(text.contains("redacted"));
    assert_eq!(artifact.attempt, f.attempt.id);
    drop(files);
    f.client()
        .send(&request, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(plugin.calls.load(Ordering::SeqCst), 1);
    plugin.close().await;
    f.close().await;
}

#[tokio::test]
async fn plugin_preflight_errors_and_unknown_outcomes_have_distinct_recovery() {
    let f = Fixture::new().await;
    let plugin = Plugin::new(&f).await;
    let mut invalid = plugin_request(&f, "missing-tool", "ok");
    if let WorkerOperation::Plugin { invocation } = &mut invalid.operation {
        invocation.tool = "missing".into();
    }
    assert!(matches!(
        decoded(
            f.client()
                .send(&invalid, Duration::from_secs(5))
                .await
                .unwrap()
        ),
        Outcome::Completed { is_error: true, .. }
    ));
    assert_eq!(plugin.calls.load(Ordering::SeqCst), 0);
    assert!(matches!(
        decoded(
            f.client()
                .send(
                    &plugin_request(&f, "known-error", "error"),
                    Duration::from_secs(5)
                )
                .await
                .unwrap()
        ),
        Outcome::Completed { is_error: true, .. }
    ));
    let uncertain = plugin_request(&f, "uncertain-call", "invalid");
    assert!(matches!(
        decoded(
            f.client()
                .send(&uncertain, Duration::from_secs(5))
                .await
                .unwrap()
        ),
        Outcome::OutcomeUnknown {}
    ));
    assert!(matches!(
        decoded(
            f.client()
                .send(&uncertain, Duration::from_secs(5))
                .await
                .unwrap()
        ),
        Outcome::OutcomeUnknown {}
    ));
    assert_eq!(plugin.calls.load(Ordering::SeqCst), 2);
    assert!(matches!(
        decoded(
            f.client()
                .send(
                    &plugin_request(&f, "changed-call-id", "ok"),
                    Duration::from_secs(5)
                )
                .await
                .unwrap()
        ),
        Outcome::Completed { is_error: true, .. }
    ));
    assert_eq!(plugin.calls.load(Ordering::SeqCst), 2);
    plugin.close().await;
    f.close().await;
}

#[tokio::test]
async fn plugin_completion_after_fencing_retains_candidate_without_canonical_acceptance() {
    let f = Fixture::new().await;
    let plugin = Plugin::new(&f).await;
    let request = plugin_request(&f, "late-plugin", "wait");
    let client = f.client();
    let pending = tokio::spawn(async move { client.send(&request, Duration::from_secs(5)).await });
    tokio::time::timeout(Duration::from_secs(2), plugin.entered.notified())
        .await
        .unwrap();
    f.store
        .fence_attempt("scan-example", &f.attempt.id, true)
        .unwrap();
    let before = f.store.load("scan-example", true).unwrap();
    plugin.release.notify_one();
    assert!(pending.await.unwrap().is_err());
    assert_eq!(f.store.load("scan-example", true).unwrap(), before);
    f.state
        .db
        .with_connection(|db| {
            let result: String = db.query_row(
                "SELECT result FROM swarm_plugin_calls WHERE operation_id='late-plugin'",
                [],
                |row| row.get(0),
            )?;
            assert!(matches!(
                serde_json::from_str::<Reply>(&result)?.outcome,
                Outcome::Completed {
                    is_error: false,
                    ..
                }
            ));
            Ok(())
        })
        .unwrap();
    assert_eq!(plugin.calls.load(Ordering::SeqCst), 1);
    plugin.close().await;
    f.close().await;
}

struct FailPluginCheckpoint {
    calls: Arc<AtomicUsize>,
    fired: AtomicBool,
}
impl beale_storage::worker_spool::SpoolFaults for FailPluginCheckpoint {
    fn before_commit(&self, write: beale_storage::worker_spool::SpoolWrite) -> anyhow::Result<()> {
        if write == beale_storage::worker_spool::SpoolWrite::Checkpoint
            && self.calls.load(Ordering::SeqCst) > 0
            && !self.fired.swap(true, Ordering::SeqCst)
        {
            anyhow::bail!("synthetic plugin checkpoint failure");
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WorkerCase {
    Checkpoint,
    Denied,
    Unknown,
}
async fn worker_case(case: WorkerCase) {
    let f = Fixture::new().await;
    let plugin = Plugin::new(&f).await;
    if case == WorkerCase::Denied {
        for rule in beale_safety::rules(&f.state.db).unwrap() {
            f.state
                .db
                .save_permission_rule(&rule.id, &rule.title, &rule.instruction, true)
                .unwrap();
        }
    }
    let inference = Arc::new(AtomicUsize::new(0));
    let count = inference.clone();
    let invocation = match plugin_request(
        &f,
        "unused",
        if case == WorkerCase::Unknown {
            "invalid"
        } else {
            "ok"
        },
    )
    .operation
    {
        WorkerOperation::Plugin { invocation } => serde_json::to_string(&invocation).unwrap(),
        _ => unreachable!(),
    };
    let app = axum::Router::new().route("/v1/chat/completions", axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
        let count=count.clone(); let invocation=invocation.clone(); async move {
            if case==WorkerCase::Denied && body["messages"][1]["content"].as_str().and_then(|s|serde_json::from_str::<Value>(s).ok()).is_some_and(|v|v.get("request").is_some()) {
                let chunk=json!({"choices":[{"delta":{"content":json!({"decision":"denied","reason":"Synthetic policy denial"}).to_string()}}]});
                return ([("content-type","text/event-stream")],format!("data: {chunk}\n\ndata: [DONE]\n\n"));
            }
            let delta=match count.fetch_add(1,Ordering::SeqCst) {
                0=>json!({"tool_calls":[{"index":0,"id":"worker-plugin-call","type":"function","function":{"name":"plugin_call","arguments":invocation}}]}),
                1=>{
                    assert!(case != WorkerCase::Unknown, "uncertain plugin must stop inference");
                    let tool=body["messages"].as_array().unwrap().iter().find(|v|v["role"]=="tool" && v["tool_call_id"]=="worker-plugin-call").unwrap();
                    let output=tool["content"].as_str().unwrap();
                    assert!(output.contains(if case==WorkerCase::Denied {"did not approve"} else {"Synthetic plugin result"}),"{output}");
                    json!({"content":"Synthetic plugin assignment complete"})
                },
                _=>panic!("unexpected repeated inference"),
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
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let workspace = f.root.join("worker-workspace");
    std::fs::create_dir(&workspace).unwrap();
    let launch = || WorkerLaunch {
        version: 1,
        identity: f.store.worker_bootstrap(&f.attempt.id).unwrap().identity,
        leader: f.url.clone(),
        bearer: f.credential.bearer().into(),
        workspace: workspace.clone(),
        protected_root: f.root.clone(),
        recovery: f.root.join("worker-recovery"),
    };
    let faults: Option<Arc<dyn beale_storage::worker_spool::SpoolFaults>> =
        if case == WorkerCase::Checkpoint {
            Some(Arc::new(FailPluginCheckpoint {
                calls: plugin.calls.clone(),
                fired: AtomicBool::new(false),
            }))
        } else {
            None
        };
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        super::super::worker::run_with_faults(&mut launch(), faults),
    )
    .await
    .unwrap();
    assert_eq!(result.is_err(), case != WorkerCase::Denied, "{result:?}");
    for _ in 0..2 {
        let result = run_assigned(launch()).await;
        assert_eq!(result.is_err(), case == WorkerCase::Unknown, "{result:?}");
    }
    assert_eq!(
        plugin.calls.load(Ordering::SeqCst),
        usize::from(case != WorkerCase::Denied)
    );
    assert_eq!(
        inference.load(Ordering::SeqCst),
        if case == WorkerCase::Unknown { 1 } else { 2 }
    );
    if case != WorkerCase::Unknown {
        let spool = beale_storage::worker_spool::WorkerSpool::open(
            &f.root.join("worker-recovery"),
            launch().identity,
        )
        .unwrap();
        assert!(spool.pending(0,128).unwrap().iter().all(|p| !matches!(&p.request,beale_contracts::swarm_worker::WorkRequest::Leader {request} if matches!(request.operation,WorkerOperation::Plugin {..}))));
    }
    if case == WorkerCase::Denied {
        f.state
            .db
            .with_connection(|db| {
                let count: i64 =
                    db.query_row("SELECT count(*) FROM swarm_plugin_calls", [], |row| {
                        row.get(0)
                    })?;
                assert_eq!(count, 0, "denied work must not claim an effect");
                Ok(())
            })
            .unwrap();
    }
    server.abort();
    let _ = server.await;
    plugin.close().await;
    f.close().await;
}
#[tokio::test]
async fn worker_plugin_checkpoint_replay_never_repeats_the_effect() {
    worker_case(WorkerCase::Checkpoint).await;
}
#[tokio::test]
async fn worker_plugin_policy_denial_returns_feedback_without_dispatch() {
    worker_case(WorkerCase::Denied).await;
}
#[tokio::test]
async fn worker_unknown_plugin_stops_before_another_model_turn() {
    worker_case(WorkerCase::Unknown).await;
}

#[tokio::test]
async fn plugin_routes_enforce_saved_tools_and_replay_lost_replies() {
    let disabled = Fixture::with_tools(false).await;
    let plugin = Plugin::new(&disabled).await;
    assert!(
        disabled
            .client()
            .send(
                &plugin_request(&disabled, "disabled-plugin", "ok"),
                Duration::from_secs(5)
            )
            .await
            .is_err()
    );
    assert_eq!(plugin.calls.load(Ordering::SeqCst), 0);
    plugin.close().await;
    disabled.close().await;

    let f = Fixture::new().await;
    let plugin = Plugin::new(&f).await;
    let request = plugin_request(&f, "lost-plugin-reply", "ok");
    f.lose_response.store(true, Ordering::SeqCst);
    assert!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .is_err()
    );
    assert_eq!(plugin.calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        decoded(
            f.client()
                .send(&request, Duration::from_secs(5))
                .await
                .unwrap()
        ),
        Outcome::Completed {
            is_error: false,
            ..
        }
    ));
    let mut changed = request.clone();
    if let WorkerOperation::Plugin { invocation } = &mut changed.operation {
        invocation.arguments = json!({"mode":"error"});
    }
    assert!(
        f.client()
            .send(&changed, Duration::from_secs(5))
            .await
            .is_err()
    );
    assert_eq!(plugin.calls.load(Ordering::SeqCst), 1);
    plugin.close().await;
    f.close().await;
}

#[path = "plugin_resources_tests.rs"]
mod resources;
