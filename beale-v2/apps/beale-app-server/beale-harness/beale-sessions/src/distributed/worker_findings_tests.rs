//! Native Windows worker execution with a deterministic local provider.
use super::*;
use beale_storage::worker_spool::{SpoolFaults, SpoolWrite};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    Publication,
    Conflict,
    Checkpoint,
    Unknown,
    FailedOracle,
    Large,
    Denied,
}
struct FailOnce {
    path: PathBuf,
    write: SpoolWrite,
    fired: AtomicBool,
}
impl SpoolFaults for FailOnce {
    fn before_commit(&self, write: SpoolWrite) -> anyhow::Result<()> {
        if write == self.write
            && std::fs::read(&self.path)
                .is_ok_and(|bytes| self.write != SpoolWrite::Checkpoint || bytes == b"run\nrun\n")
            && !self.fired.swap(true, Ordering::SeqCst)
        {
            anyhow::bail!("synthetic verification spool failure");
        }
        Ok(())
    }
}
async fn exercise(case: Case) {
    let f = Fixture::new().await;
    let mut command = candidate(&f);
    if case == Case::FailedOracle {
        command.oracle = beale_contracts::findings::Oracle::ExitCode { value: 0 };
    }
    if case == Case::Denied {
        for rule in beale_safety::rules(&f.state.db).unwrap() {
            f.state
                .db
                .save_permission_rule(&rule.id, &rule.title, &rule.instruction, true)
                .unwrap();
        }
    }
    let workspace = f.root.join("worker-workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("proof.py"),format!("with open('steps.txt','ab') as f: f.write(b'run\\n')\nprint({})\nraise SystemExit({})\n",if case==Case::Large {"'x'*1100000 + 'example-observation'"}else{"'example-observation'"},if case==Case::FailedOracle {3}else{0})).unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let captured = calls.clone();
    let invocation = serde_json::to_value(&command).unwrap();
    let app=axum::Router::new().route("/v1/chat/completions",axum::routing::post(move |axum::Json(body):axum::Json<Value>| {
        let calls=captured.clone();let invocation=invocation.clone();async move {
            if case==Case::Denied && body["messages"][1]["content"].as_str().and_then(|s|serde_json::from_str::<Value>(s).ok()).is_some_and(|v|v.get("request").is_some()) {
                let content=json!({"decision":"denied","reason":"Synthetic policy denial"}).to_string();let chunk=json!({"choices":[{"delta":{"content":content}}]});
                return ([("content-type","text/event-stream")],format!("data: {chunk}\n\ndata: [DONE]\n\n"));
            }
            let delta=match calls.fetch_add(1,Ordering::SeqCst) {
                0=>json!({"tool_calls":[{"index":0,"id":"verify-example","type":"function","function":{"name":"findings_verify","arguments":invocation.to_string()}}]}),
                1=>{
                    let tool=body["messages"].as_array().unwrap().iter().find(|m|m["role"]=="tool").unwrap();
                    if case==Case::Denied {assert!(tool["content"].as_str().unwrap().contains("did not approve"));}
                    else {let output:Value=serde_json::from_str(tool["content"].as_str().unwrap()).unwrap();assert_eq!(output["verified"],!matches!(case,Case::FailedOracle|Case::Conflict));}
                    json!({"content":"Synthetic verification assignment complete"})
                },
                _=>panic!("unexpected repeated inference"),
            };
            let chunk=json!({"choices":[{"delta":delta}]});([( "content-type","text/event-stream")],format!("data: {chunk}\n\ndata: [DONE]\n\n"))
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
    if matches!(case, Case::Publication | Case::Conflict) {
        f.state.db.with_connection(|db|{db.execute_batch("CREATE TRIGGER fail_observation BEFORE INSERT ON finding_worker_observations BEGIN SELECT RAISE(ABORT,'synthetic verifier publication failure'); END;")?;Ok(())}).unwrap();
    }
    let faults: Option<Arc<dyn SpoolFaults>> = match case {
        Case::Checkpoint | Case::Unknown => Some(Arc::new(FailOnce {
            path: workspace.join("steps.txt"),
            write: if case == Case::Unknown {
                SpoolWrite::Outcome
            } else {
                SpoolWrite::Checkpoint
            },
            fired: AtomicBool::new(false),
        })),
        _ => None,
    };
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        super::super::super::worker::run_with_faults(&mut launch(), faults),
    )
    .await
    .unwrap();
    assert_eq!(
        result.is_err(),
        matches!(
            case,
            Case::Publication | Case::Conflict | Case::Checkpoint | Case::Unknown
        ),
        "{result:?}"
    );
    if matches!(case, Case::Publication | Case::Conflict | Case::Unknown) {
        assert_eq!(
            std::fs::read(workspace.join("steps.txt")).unwrap(),
            b"run\n"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
    if matches!(case, Case::Publication | Case::Conflict) {
        f.state
            .db
            .with_connection(|db| {
                db.execute_batch("DROP TRIGGER fail_observation;")?;
                Ok(())
            })
            .unwrap();
        if case == Case::Conflict {
            let mut finding = f
                .state
                .db
                .finding_get(&f.workspace_id, &command.id)
                .unwrap()
                .unwrap();
            finding.summary = "Concurrent synthetic edit".into();
            f.state
                .db
                .finding_update(
                    finding,
                    1,
                    "session-other",
                    "Synthetic edit during publication",
                )
                .unwrap();
        }
    }
    if case == Case::Unknown {
        assert!(run_assigned(launch()).await.is_err());
        assert_eq!(
            std::fs::read(workspace.join("steps.txt")).unwrap(),
            b"run\n"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    } else {
        run_assigned(launch()).await.unwrap();
        run_assigned(launch()).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        if case == Case::Denied {
            assert!(!workspace.join("steps.txt").exists());
        } else {
            assert_eq!(
                std::fs::read(workspace.join("steps.txt")).unwrap(),
                b"run\nrun\n"
            );
        }
    }
    let finding = f
        .state
        .db
        .finding_get(&f.workspace_id, &command.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        finding.revision,
        if matches!(case, Case::Unknown | Case::Denied) {
            1
        } else {
            2
        }
    );
    if !matches!(case, Case::Unknown | Case::Denied | Case::Conflict) {
        let verification = finding.state.validated.unwrap().verification.unwrap();
        assert_eq!(verification.runs.len(), 2);
        assert_eq!(verification.verified, case != Case::FailedOracle);
        assert_eq!(verification.tool_call_id, "verify-example");
    }
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
                        !matches!(request.operation, WorkerOperation::Verify { .. }),
                    beale_contracts::swarm_worker::WorkRequest::Environment { call } =>
                        call.name != "verification_run",
                })
        );
    }
    let identity = launch().identity;
    super::super::super::supervisor::reconcile_owned_attempt(
        &f.store,
        &identity,
        true,
        if case == Case::Unknown {
            WorkerDisposition::Failed
        } else {
            WorkerDisposition::Completed
        },
        unix_ms(),
    )
    .unwrap();
    if case == Case::Unknown {
        f.state
            .db
            .with_connection(|db| {
                assert_eq!(
                    db.query_row("SELECT state FROM finding_worker_verifications", [], |r| {
                        r.get::<_, String>(0)
                    })?,
                    "interrupted"
                );
                Ok(())
            })
            .unwrap();
    }
    server.abort();
    let _ = server.await;
    f.close().await;
}

#[tokio::test]
async fn assigned_verifier_replays_publication_conflict_and_checkpoint_without_repeating_commands()
{
    for case in [Case::Publication, Case::Conflict, Case::Checkpoint] {
        exercise(case).await;
    }
}
#[tokio::test]
async fn assigned_verifier_keeps_uncertain_commands_stopped_and_denial_as_feedback() {
    for case in [Case::Unknown, Case::Denied] {
        exercise(case).await;
    }
}
#[tokio::test]
async fn assigned_verifier_evaluates_complete_large_outputs_and_nonzero_results() {
    for case in [Case::Large, Case::FailedOracle] {
        exercise(case).await;
    }
}
