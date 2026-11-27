use super::*;
use beale_contracts::swarm_plugins::Invocation;
use serde_json::json;
use std::sync::atomic::AtomicUsize;
use tokio::sync::Notify;

struct Fixture {
    root: PathBuf,
    runtime: PluginRuntime,
    calls: Arc<AtomicUsize>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
    host: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new(label: &str) -> Self {
        let calls = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let (c, e, r) = (calls.clone(), entered.clone(), release.clone());
        let app=axum::Router::new().route("/mcp",axum::routing::post(move |axum::Json(request):axum::Json<Value>| {
            let (calls,entered,release)=(c.clone(),e.clone(),r.clone());async move {
                let result=match request["method"].as_str().unwrap_or_default() {
                    "server/discover"=>json!({"supportedVersions":["2026-07-28"]}),
                    "tools/list"=>json!({"tools":[{"name":"example","inputSchema":{"type":"object"}}]}),
                    "prompts/list"=>json!({"prompts":[]}),"resources/list"=>json!({"resources":[]}),"resources/templates/list"=>json!({"resourceTemplates":[]}),
                    "tools/call"=>{
                        calls.fetch_add(1,Ordering::SeqCst);
                        match request["params"]["arguments"]["mode"].as_str().unwrap_or_default() {
                            "wait"=>{entered.notify_one();release.notified().await;},
                            "invalid"=>return axum::Json(json!({"jsonrpc":"2.0","id":request["id"]})),
                            "error"=>return axum::Json(json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32000,"message":"Synthetic rejection"}})),
                            _=>(),
                        }
                        json!({"content":[{"type":"text","text":"Synthetic result"}]})
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
        let root =
            std::env::temp_dir().join(format!("beale-plugin-guard-{label}-{}", std::process::id()));
        let db = HostDatabase::open(&root).unwrap();
        let runtime = PluginRuntime::new(db.clone(), root.clone()).unwrap();
        for name in ["example-a", "example-b"] {
            let package = root.join("plugins/packages").join(name);
            std::fs::create_dir_all(&package).unwrap();
            std::fs::write(
                package.join("plugin.json"),
                json!({"$schema":MANIFEST_SCHEMA,"name":name,"description":"Synthetic plugin"})
                    .to_string(),
            )
            .unwrap();
            std::fs::write(package.join("mcp.json"),json!({"$schema":MCP_SCHEMA,"mcpServers":{"first":{"type":"streamable-http","url":format!("http://{address}/mcp")},"second":{"type":"streamable-http","url":format!("http://{address}/mcp")}}}).to_string()).unwrap();
            runtime.catalog.install(&db, name).unwrap();
            runtime.details(name, "workspace-example").await.unwrap();
        }
        Self {
            root,
            runtime,
            calls,
            entered,
            release,
            host,
        }
    }
    fn invocation(plugin: &str, server: &str, mode: &str) -> Invocation {
        Invocation {
            plugin: plugin.into(),
            server: server.into(),
            tool: "example".into(),
            arguments: json!({"mode":mode}),
        }
    }
    async fn close(self) {
        for name in ["example-a", "example-b"] {
            self.runtime.catalog.deactivate(name).await;
        }
        self.host.abort();
        let _ = self.host.await;
        drop(self.runtime);
        std::fs::remove_dir_all(self.root).unwrap();
    }
}

#[tokio::test]
async fn plugin_dispatch_guard_precedes_effect_and_distinguishes_uncertain_responses() {
    let f = Fixture::new("outcomes").await;
    let invocation = Fixture::invocation("example-a", "first", "ok");
    assert!(
        f.runtime
            .call_tool_guarded(&invocation, "workspace-example", || anyhow::bail!(
                "synthetic claim unavailable"
            ))
            .await
            .is_err()
    );
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
    let mut invalid = invocation.clone();
    invalid.tool = "missing-tool".into();
    let mut claimed = false;
    assert!(
        f.runtime
            .call_tool_guarded(&invalid, "workspace-example", || {
                claimed = true;
                Ok(())
            })
            .await
            .is_err()
    );
    assert!(!claimed);
    for (mode, unknown) in [("ok", false), ("error", false), ("invalid", true)] {
        let invocation = Fixture::invocation("example-a", "first", mode);
        let before = f.calls.load(Ordering::SeqCst);
        let result = f
            .runtime
            .call_tool_guarded(&invocation, "workspace-example", || {
                assert_eq!(f.calls.load(Ordering::SeqCst), before);
                Ok(())
            })
            .await
            .unwrap();
        match result {
            GuardedToolResult::Completed(result) => {
                assert!(!unknown);
                assert_eq!(
                    result["isError"].as_bool().unwrap_or(false),
                    mode == "error"
                );
            }
            GuardedToolResult::OutcomeUnknown(_) => assert!(unknown),
        }
    }
    assert_eq!(f.calls.load(Ordering::SeqCst), 3);
    f.close().await;
}

#[tokio::test]
async fn busy_plugin_does_not_block_other_servers_or_discovery_and_deactivation_fences_waiters() {
    let f = Fixture::new("concurrency").await;
    let runtime = f.runtime.clone();
    let slow = tokio::spawn(async move {
        runtime
            .call_tool_guarded(
                &Fixture::invocation("example-a", "first", "wait"),
                "workspace-example",
                || Ok(()),
            )
            .await
    });
    timeout(Duration::from_secs(2), f.entered.notified())
        .await
        .unwrap();
    for (plugin, server) in [("example-a", "second"), ("example-b", "first")] {
        assert!(matches!(
            timeout(
                Duration::from_secs(2),
                f.runtime.call_tool_guarded(
                    &Fixture::invocation(plugin, server, "ok"),
                    "workspace-example",
                    || Ok(())
                )
            )
            .await
            .unwrap()
            .unwrap(),
            GuardedToolResult::Completed(_)
        ));
    }
    timeout(
        Duration::from_secs(2),
        f.runtime.details("example-a", "workspace-example"),
    )
    .await
    .unwrap()
    .unwrap();
    let claimed = Arc::new(AtomicBool::new(false));
    let flag = claimed.clone();
    let runtime = f.runtime.clone();
    let waiting = tokio::spawn(async move {
        runtime
            .call_tool_guarded(
                &Fixture::invocation("example-a", "first", "ok"),
                "workspace-example",
                || {
                    flag.store(true, Ordering::SeqCst);
                    Ok(())
                },
            )
            .await
    });
    let plugin = f.runtime.catalog.active_plugin("example-a").await.unwrap();
    let catalog = f.runtime.catalog.clone();
    let stopping = tokio::spawn(async move { catalog.deactivate("example-a").await });
    timeout(Duration::from_secs(2), async {
        while plugin.enabled.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!claimed.load(Ordering::SeqCst));
    assert!(matches!(
        f.runtime
            .call_tool_guarded(
                &Fixture::invocation("example-b", "first", "ok"),
                "workspace-example",
                || Ok(())
            )
            .await
            .unwrap(),
        GuardedToolResult::Completed(_)
    ));
    f.release.notify_one();
    assert!(matches!(
        slow.await.unwrap().unwrap(),
        GuardedToolResult::Completed(_)
    ));
    assert!(waiting.await.unwrap().is_err());
    assert!(!claimed.load(Ordering::SeqCst));
    stopping.await.unwrap();
    assert_eq!(f.calls.load(Ordering::SeqCst), 4);
    f.close().await;
}

#[tokio::test]
async fn aborting_a_plugin_handler_does_not_repeat_its_effect() {
    let f = Fixture::new("abort").await;
    let runtime = f.runtime.clone();
    let claimed = Arc::new(AtomicBool::new(false));
    let flag = claimed.clone();
    let task = tokio::spawn(async move {
        runtime
            .call_tool_guarded(
                &Fixture::invocation("example-a", "first", "wait"),
                "workspace-example",
                || {
                    flag.store(true, Ordering::SeqCst);
                    Ok(())
                },
            )
            .await
    });
    timeout(Duration::from_secs(2), f.entered.notified())
        .await
        .unwrap();
    assert!(claimed.load(Ordering::SeqCst));
    task.abort();
    let _ = task.await;
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    f.release.notify_one();
    assert!(matches!(
        f.runtime
            .call_tool_guarded(
                &Fixture::invocation("example-b", "first", "ok"),
                "workspace-example",
                || Ok(())
            )
            .await
            .unwrap(),
        GuardedToolResult::Completed(_)
    ));
    assert_eq!(f.calls.load(Ordering::SeqCst), 2);
    f.close().await;
}
