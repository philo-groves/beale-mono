use super::*;
use beale_contracts::swarm_plugins::{
    ResourceManifest, ResourceReply, ResourceRequest, ResourceResult,
};
fn resource_request(f: &Fixture, id: &str, request: ResourceRequest) -> WorkerRequest {
    f.request(id, WorkerOperation::PluginResources { request })
}
async fn resource(f: &Fixture, id: &str, request: ResourceRequest) -> ResourceResult {
    let request = resource_request(f, id, request);
    let value = acknowledged(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    serde_json::from_value::<ResourceReply>(value)
        .unwrap()
        .result
}
fn skill(f: &Fixture) -> PathBuf {
    let root = f
        .state
        .workspaces
        .root()
        .join("plugins/packages/example-plugin/skills/example");
    std::fs::create_dir_all(root.join("assets")).unwrap();
    std::fs::write(
        root.join("SKILL.md"),
        b"Read assets/example.txt. Synthetic example.test guidance.",
    )
    .unwrap();
    std::fs::write(
        root.join("assets/example.txt"),
        format!(
            "{} host-control-example {}",
            "x".repeat(400_000),
            f.credential.bearer()
        ),
    )
    .unwrap();
    root
}
async fn prepare(f: &Fixture) -> (String, ResourceManifest) {
    let ResourceResult::Prepared { bundle, manifest } = resource(
        f,
        "resource-prepare",
        ResourceRequest::Prepare {
            plugin: "example-plugin".into(),
        },
    )
    .await
    else {
        panic!("expected prepared resources");
    };
    (bundle, manifest)
}
#[tokio::test]
async fn resource_snapshots_replay_after_receipt_loss_and_uninstall_with_original_bytes() {
    let f = Fixture::new().await;
    let plugin = Plugin::new(&f).await;
    let root = skill(&f);
    assert!(
        matches!(resource(&f,"resource-catalog",ResourceRequest::Catalog {}).await,ResourceResult::Catalog {plugins} if plugins==["example-plugin"])
    );
    f.state.db.with_connection(|db|{db.execute_batch("CREATE TRIGGER resource_receipt_full BEFORE INSERT ON swarm_receipts BEGIN SELECT RAISE(ABORT,'synthetic resource receipt full'); END;")?;Ok(())}).unwrap();
    let request = resource_request(
        &f,
        "resource-prepare",
        ResourceRequest::Prepare {
            plugin: "example-plugin".into(),
        },
    );
    assert!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .is_err()
    );
    f.state
        .db
        .with_connection(|db| {
            let count: i64 =
                db.query_row("SELECT count(*) FROM swarm_plugin_resources", [], |r| {
                    r.get(0)
                })?;
            assert_eq!(count, 0);
            db.execute_batch("DROP TRIGGER resource_receipt_full;")?;
            Ok(())
        })
        .unwrap();
    f.lose_response.store(true, Ordering::SeqCst);
    assert!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .is_err()
    );
    f.state.db.uninstall_plugin("example-plugin").unwrap();
    std::fs::write(root.join("assets/example.txt"), b"Changed package resource").unwrap();
    let (bundle, manifest) = prepare(&f).await;
    let file = manifest
        .files
        .iter()
        .find(|v| v.path.ends_with("example.txt"))
        .unwrap();
    let mut bytes = Vec::new();
    while (bytes.len() as u64) < file.size {
        let offset = bytes.len() as u64;
        let result = resource(
            &f,
            &format!("resource-read-{offset}"),
            ResourceRequest::Read {
                plugin: manifest.plugin.clone(),
                bundle: bundle.clone(),
                path: file.path.clone(),
                offset,
            },
        )
        .await;
        let ResourceResult::Chunk {
            bytes: chunk,
            sha256,
            ..
        } = result
        else {
            panic!("expected resource chunk");
        };
        assert_eq!(beale_storage::swarm_files::hash(&chunk), sha256);
        bytes.extend(chunk);
    }
    assert_eq!(beale_storage::swarm_files::hash(&bytes), file.sha256);
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.len() > 400_000);
    assert!(!text.contains("host-control-example"));
    assert!(!text.contains(f.credential.bearer()));
    assert!(text.contains("redacted"));
    let again = resource(
        &f,
        "resource-prepare-another",
        ResourceRequest::Prepare {
            plugin: manifest.plugin.clone(),
        },
    )
    .await;
    assert!(matches!(again,ResourceResult::Prepared {bundle:other,..} if other==bundle));
    assert_eq!(plugin.calls.load(Ordering::SeqCst), 0);
    plugin.close().await;
    f.close().await;
}
#[tokio::test]
async fn resources_reject_undeclared_files_corruption_and_revoked_attempts() {
    let f = Fixture::new().await;
    let plugin = Plugin::new(&f).await;
    skill(&f);
    let (bundle, manifest) = prepare(&f).await;
    for (name, path) in [
        ("wrong-path", "skills/example/assets/missing"),
        ("private-path", "mcp.json"),
    ] {
        let request = resource_request(
            &f,
            name,
            ResourceRequest::Read {
                plugin: manifest.plugin.clone(),
                bundle: bundle.clone(),
                path: path.into(),
                offset: 0,
            },
        );
        assert!(
            f.client()
                .send(&request, Duration::from_secs(5))
                .await
                .is_err()
        );
    }
    let file = &manifest.files[0];
    std::fs::write(
        f.state
            .workspaces
            .root()
            .join("swarm-files/blobs")
            .join(&file.sha256),
        b"corrupt",
    )
    .unwrap();
    let request = resource_request(
        &f,
        "corrupt-resource",
        ResourceRequest::Read {
            plugin: manifest.plugin.clone(),
            bundle,
            path: file.path.clone(),
            offset: 0,
        },
    );
    assert!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .is_err()
    );
    f.store
        .fence_attempt("scan-example", &f.attempt.id, true)
        .unwrap();
    let request = resource_request(&f, "fenced-resources", ResourceRequest::Catalog {});
    assert!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .is_err()
    );
    plugin.close().await;
    f.close().await;
}
#[tokio::test]
async fn resource_publication_reports_binary_secrets_and_respects_saved_tools() {
    let disabled = Fixture::with_tools(false).await;
    let request = resource_request(&disabled, "disabled-resources", ResourceRequest::Catalog {});
    assert!(
        disabled
            .client()
            .send(&request, Duration::from_secs(5))
            .await
            .is_err()
    );
    disabled.close().await;
    let f = Fixture::new().await;
    let plugin = Plugin::new(&f).await;
    let root = skill(&f);
    let mut binary = vec![255];
    binary.extend_from_slice(b"host-control-example");
    std::fs::write(root.join("assets/example.bin"), binary).unwrap();
    for _ in 0..2 {
        assert!(matches!(
            resource(
                &f,
                "unsafe-binary",
                ResourceRequest::Prepare {
                    plugin: "example-plugin".into()
                }
            )
            .await,
            ResourceResult::Unavailable { .. }
        ));
    }
    let saved = f.store.load("scan-example", true).unwrap();
    assert_eq!(
        saved
            .events
            .iter()
            .filter(|e| e.kind == "plugin-resources-unavailable")
            .count(),
        1
    );
    let guidance = f
        .store
        .worker_plugin_guidance(&f.store.worker_bootstrap(&f.attempt.id).unwrap().identity)
        .unwrap();
    assert!(guidance.contains("preparation warning"));
    assert!(!guidance.contains("host-control-example"));
    plugin.close().await;
    f.close().await;
}
struct BootstrapFailure(AtomicBool);
impl beale_storage::worker_spool::SpoolFaults for BootstrapFailure {
    fn before_commit(&self, write: beale_storage::worker_spool::SpoolWrite) -> anyhow::Result<()> {
        if write == beale_storage::worker_spool::SpoolWrite::Checkpoint
            && !self.0.swap(true, Ordering::SeqCst)
        {
            anyhow::bail!("synthetic startup checkpoint failure");
        }
        Ok(())
    }
}
async fn worker_resources_recovery(partition: bool) {
    let f = Fixture::new().await;
    let plugin = Plugin::new(&f).await;
    let root = skill(&f);
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
            let count = count.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                let system = body["messages"][0]["content"].as_str().unwrap();
                assert!(system.contains("Installed plugin resources in your assigned workspace"));
                assert!(system.contains("/example-plugin/PLUGIN.md"));
                let chunk =
                    json!({"choices":[{"delta":{"content":"Synthetic resources accepted"}}]});
                (
                    [("content-type", "text/event-stream")],
                    format!("data: {chunk}\n\ndata: [DONE]\n\n"),
                )
            }
        }),
    );
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
    f.block_resource_chunks.store(partition, Ordering::SeqCst);
    let faults: Option<Arc<dyn beale_storage::worker_spool::SpoolFaults>> = if partition {
        None
    } else {
        Some(Arc::new(BootstrapFailure(AtomicBool::new(false))))
    };
    assert!(
        super::super::super::worker::run_with_faults(&mut launch(), faults)
            .await
            .is_err()
    );
    f.block_resource_chunks.store(false, Ordering::SeqCst);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let (bundle, manifest) = prepare(&f).await;
    let destination = workspace.join(
        beale_storage::swarm_state::plugin_resource_directory(
            &launch().identity,
            &bundle,
            &manifest.plugin,
        )
        .unwrap(),
    );
    if partition {
        assert!(
            !destination
                .join("skills/example/assets/example.txt")
                .exists()
        );
    }
    std::fs::write(root.join("SKILL.md"), b"Edited installed guidance").unwrap();
    f.state.db.uninstall_plugin("example-plugin").unwrap();
    run_assigned(launch()).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    for file in &manifest.files {
        assert_eq!(
            beale_storage::swarm_files::hash(&std::fs::read(destination.join(&file.path)).unwrap()),
            file.sha256
        );
    }

    std::fs::write(
        destination.join("skills/example/SKILL.md"),
        b"Private worker edit",
    )
    .unwrap();
    run_assigned(launch()).await.unwrap();
    assert_eq!(
        std::fs::read(destination.join("skills/example/SKILL.md")).unwrap(),
        b"Private worker edit"
    );
    server.abort();
    let _ = server.await;
    plugin.close().await;
    f.close().await;
}

#[tokio::test]
async fn worker_resources_survive_startup_failure_and_reach_the_model_as_local_guidance() {
    worker_resources_recovery(false).await;
}
#[tokio::test]
async fn interrupted_resource_chunk_download_resumes_the_original_bundle() {
    worker_resources_recovery(true).await;
}

#[tokio::test]
async fn resource_commit_rechecks_scope_installation_and_execution_authority() {
    use beale_storage::swarm_state::{PluginResourceInput, PreparedPluginBundle};
    for change in ["scope", "uninstall", "fence"] {
        let f = Fixture::new().await;
        let plugin = Plugin::new(&f).await;
        skill(&f);
        let snapshot = f
            .state
            .plugins
            .snapshot("example-plugin", &f.workspace_id)
            .await
            .unwrap();
        match change {
            "scope" => {
                f.state
                    .db
                    .set_extension_scope("plugin", "example-plugin", false, &[])
                    .unwrap();
            }
            "uninstall" => {
                f.state.db.uninstall_plugin("example-plugin").unwrap();
            }
            _ => {
                f.store
                    .fence_attempt("scan-example", &f.attempt.id, true)
                    .unwrap();
            }
        }
        let files = crate::research_files::store(&f.state).unwrap();
        let request = resource_request(
            &f,
            "late-resource-commit",
            ResourceRequest::Prepare {
                plugin: "example-plugin".into(),
            },
        );
        let input = PluginResourceInput::Snapshot(PreparedPluginBundle {
            manifest: snapshot.manifest,
            bytes: snapshot.resources.into_iter().map(|r| r.bytes).collect(),
        });
        let result = f.store.worker_plugin_resources(
            &files,
            f.credential.bearer(),
            &request,
            crate::tool_runtime::unix_ms(),
            input,
        );
        if change == "fence" {
            assert!(result.is_err());
        } else {
            assert!(matches!(
                serde_json::from_value::<ResourceReply>(result.unwrap().result)
                    .unwrap()
                    .result,
                ResourceResult::Unavailable { .. }
            ));
        }
        f.state
            .db
            .with_connection(|db| {
                let count: i64 =
                    db.query_row("SELECT count(*) FROM swarm_plugin_resources", [], |r| {
                        r.get(0)
                    })?;
                assert_eq!(count, 0);
                Ok(())
            })
            .unwrap();
        drop(files);
        plugin.close().await;
        f.close().await;
    }
}
