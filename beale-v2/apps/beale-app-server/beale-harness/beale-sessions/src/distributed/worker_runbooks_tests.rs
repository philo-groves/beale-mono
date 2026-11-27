//! Native interpreter acceptance on Windows. Unix uses the executable worker's
//! process group and requires a native subprocess acceptance run.
use super::*;
use beale_storage::worker_spool::{SpoolFaults, SpoolWrite};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    Denied,
    Begin,
    Publication,
    Checkpoint,
    Unknown,
    Nonzero,
    Unsupported,
}

struct FailOnce {
    path: PathBuf,
    write: SpoolWrite,
    after_second: bool,
    fired: AtomicBool,
}
impl SpoolFaults for FailOnce {
    fn before_commit(&self, write: SpoolWrite) -> anyhow::Result<()> {
        if write == self.write
            && std::fs::read_to_string(&self.path)
                .is_ok_and(|v| !self.after_second || v.contains("second"))
            && !self.fired.swap(true, Ordering::SeqCst)
        {
            anyhow::bail!("synthetic Runbook recovery write failed");
        }
        Ok(())
    }
}

async fn exercise(case: Case) {
    let f = Fixture::new().await;
    if case == Case::Denied {
        for rule in beale_safety::rules(&f.state.db).unwrap() {
            f.state
                .db
                .save_permission_rule(&rule.id, &rule.title, &rule.instruction, true)
                .unwrap();
        }
    }
    let mut doc = document(&f);
    doc.cells = vec![
        Cell::Code {
            id: String::new(),
            name: "First".into(),
            language: "python".into(),
            source: r#"with open('steps.txt','ab') as f: f.write(b'first\n')
print('first example output')"#
                .into(),
        },
        Cell::Code {
            id: String::new(),
            name: "Second".into(),
            language: if case == Case::Unsupported {
                "unsupported-example"
            } else {
                "python"
            }
            .into(),
            source: format!(
                r#"with open('steps.txt','ab') as f: f.write(b'second\n')
print('second example output')
{}"#,
                if case == Case::Nonzero {
                    "raise SystemExit(3)"
                } else {
                    ""
                }
            ),
        },
    ];
    doc = f
        .state
        .db
        .runbook_edit(doc, 1, "agent", &f.deputy, "Synthetic worker execution")
        .unwrap();
    let workspace = f.root.join("worker-workspace");
    std::fs::create_dir(&workspace).unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let captured = calls.clone();
    let id = doc.id.clone();
    let app=axum::Router::new().route("/v1/chat/completions",axum::routing::post(move |axum::Json(body):axum::Json<Value>| {
        let calls=captured.clone();let id=id.clone();async move {
            if case==Case::Denied && body["messages"][1]["content"].as_str().and_then(|s|serde_json::from_str::<Value>(s).ok()).is_some_and(|v|v.get("request").is_some()) {
                let content=json!({"decision":"denied","reason":"Synthetic policy denial"}).to_string();
                let chunk=json!({"choices":[{"delta":{"content":content}}]});
                return ([("content-type","text/event-stream")],format!("data: {chunk}\n\ndata: [DONE]\n\n"));
            }
            let delta=match calls.fetch_add(1,Ordering::SeqCst) {
                0=>json!({"tool_calls":[{"index":0,"id":"run-example","type":"function","function":{"name":"runbook_run","arguments":json!({"id":id}).to_string()}}]}),
                1=> {
                    let tool=body["messages"].as_array().unwrap().iter().find(|m|m["role"]=="tool").unwrap();
                    if case==Case::Denied {assert!(tool["content"].as_str().unwrap().contains("did not approve"));}
                    else {
                        let output:Value=serde_json::from_str(tool["content"].as_str().unwrap()).unwrap();
                        assert_eq!(output["status"],if matches!(case,Case::Nonzero|Case::Unsupported) {"failed"} else {"completed"});
                    }
                    json!({"content":"Synthetic Runbook assignment complete"})
                }
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
    let launch = || WorkerLaunch {
        version: 1,
        identity: f.store.worker_bootstrap(&f.attempt.id).unwrap().identity,
        leader: f.url.clone(),
        bearer: f.credential.bearer().into(),
        workspace: workspace.clone(),
        protected_root: f.root.clone(),
        recovery: f.root.join("worker-recovery"),
    };
    if case == Case::Publication {
        f.state.db.with_connection(|db| {db.execute_batch("CREATE TRIGGER fail_cell_publication BEFORE INSERT ON runbook_worker_cells BEGIN SELECT RAISE(ABORT, 'synthetic publication full'); END;")?;Ok(())}).unwrap();
    }
    if case == Case::Begin {
        f.state.db.with_connection(|db| {db.execute_batch("CREATE TRIGGER fail_cell_begin BEFORE UPDATE ON runbook_runs WHEN NEW.current_cell_number>0 BEGIN SELECT RAISE(ABORT, 'synthetic begin receipt full'); END;")?;Ok(())}).unwrap();
    }
    let faults: Option<Arc<dyn SpoolFaults>> = match case {
        Case::Checkpoint | Case::Unknown => Some(Arc::new(FailOnce {
            path: workspace.join("steps.txt"),
            write: if case == Case::Unknown {
                SpoolWrite::Outcome
            } else {
                SpoolWrite::Checkpoint
            },
            after_second: case == Case::Checkpoint,
            fired: AtomicBool::new(false),
        })),
        _ => None,
    };
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        super::super::super::worker::run_with_faults(&mut launch(), faults),
    )
    .await
    .unwrap();
    assert_eq!(
        result.is_err(),
        matches!(
            case,
            Case::Begin | Case::Publication | Case::Checkpoint | Case::Unknown
        ),
        "{result:?}"
    );
    if case == Case::Begin {
        assert!(!workspace.join("steps.txt").exists());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        f.state
            .db
            .with_connection(|db| {
                db.execute_batch("DROP TRIGGER fail_cell_begin;")?;
                Ok(())
            })
            .unwrap();
    }
    if matches!(case, Case::Publication | Case::Unknown) {
        assert_eq!(
            std::fs::read_to_string(workspace.join("steps.txt")).unwrap(),
            "first\n"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
    if case == Case::Publication {
        f.state
            .db
            .with_connection(|db| {
                db.execute_batch("DROP TRIGGER fail_cell_publication;")?;
                Ok(())
            })
            .unwrap();
        let mut changed = doc.clone();
        if let Cell::Code { source, .. } = &mut changed.cells[1] {
            *source = "raise RuntimeError('changed source must not execute')".into();
        }
        f.state
            .db
            .runbook_edit(
                changed,
                doc.revision,
                "agent",
                &f.deputy,
                "Edit while worker is disconnected",
            )
            .unwrap();
    }
    if case == Case::Unknown {
        assert!(run_assigned(launch()).await.is_err());
        assert_eq!(
            std::fs::read_to_string(workspace.join("steps.txt")).unwrap(),
            "first\n"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    } else {
        run_assigned(launch()).await.unwrap();
        run_assigned(launch()).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        if matches!(case, Case::Unsupported | Case::Denied) {
            assert!(!workspace.join("steps.txt").exists());
        } else {
            assert_eq!(
                std::fs::read_to_string(workspace.join("steps.txt")).unwrap(),
                "first\nsecond\n"
            );
        }
    }
    let runs = f
        .state
        .db
        .session_runbook_runs(&f.workspace_id, &f.deputy, &doc.id)
        .unwrap();
    assert_eq!(runs.len(), usize::from(case != Case::Denied));
    if case != Case::Unknown {
        let spool = beale_storage::worker_spool::WorkerSpool::open(
            &f.root.join("worker-recovery"),
            launch().identity,
        )
        .unwrap();
        assert!(
            spool
                .pending(0, 128)
                .unwrap()
                .iter()
                .all(|operation| match &operation.request {
                    beale_contracts::swarm_worker::WorkRequest::Leader { request } =>
                        !matches!(request.operation, WorkerOperation::Runbook { .. }),
                    beale_contracts::swarm_worker::WorkRequest::Environment { call } =>
                        call.name != "runbook_cell",
                })
        );
    }
    let disposition = if case == Case::Unknown {
        WorkerDisposition::Failed
    } else {
        WorkerDisposition::Completed
    };
    super::super::super::supervisor::reconcile_owned_attempt(
        &f.store,
        &launch().identity,
        true,
        disposition,
        unix_ms(),
    )
    .unwrap();
    if case != Case::Denied {
        let run = f
            .state
            .db
            .runbook_run_status(&f.deputy, &runs[0].id)
            .unwrap()
            .unwrap();
        assert_eq!(
            run.status,
            match case {
                Case::Unknown => "interrupted",
                Case::Nonzero | Case::Unsupported => "failed",
                _ => "completed",
            }
        );
    }
    assert!(f.state.db.session(&f.deputy).unwrap().is_some());
    assert!(!f.root.join("steps.txt").exists());
    server.abort();
    let _ = server.await;
    f.close().await;
}

#[tokio::test]
async fn assigned_runbooks_replay_published_cells_and_completed_tool_without_execution() {
    exercise(Case::Begin).await;
    exercise(Case::Publication).await;
    exercise(Case::Checkpoint).await;
}

#[tokio::test]
async fn assigned_runbooks_stop_unknown_cells_and_report_known_failures() {
    exercise(Case::Unknown).await;
    exercise(Case::Nonzero).await;
    exercise(Case::Unsupported).await;
}

#[tokio::test]
async fn assigned_runbook_denial_returns_feedback_without_execution_or_run_creation() {
    exercise(Case::Denied).await;
}
