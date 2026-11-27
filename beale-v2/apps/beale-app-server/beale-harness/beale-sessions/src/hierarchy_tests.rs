use super::*;
use axum::{body::to_bytes, http::Request, response::IntoResponse};
use tower::ServiceExt;

#[path = "hierarchy_transcript_tests.rs"]
mod transcripts;

#[path = "hierarchy_workflow_tests.rs"]
mod workflows;

#[path = "hierarchy_closeout_tests.rs"]
mod closeout;

#[path = "hierarchy_allocation_tests.rs"]
mod allocation;
#[path = "hierarchy_maintenance_tests.rs"]
mod maintenance;

#[test]
fn scan_pacing_contract_roundtrips_units_and_rejects_invalid_targets() {
    use beale_contracts::swarms::PacingAmount;
    use beale_contracts::v1::SwarmPacingTarget;
    let target = SwarmPacingTarget {
        budget_group_id: "account-example".into(),
        interval_seconds: 3600,
        unit: "subscription".into(),
        denomination: "primary".into(),
        amount: 100,
    };
    let request = SubmitSessionTurnRequest {
        swarm_pacing: vec![target.clone()],
        ..Default::default()
    };
    assert_eq!(
        SubmitSessionTurnRequest::decode(request.encode_to_vec().as_slice()).unwrap(),
        request
    );
    assert!(matches!(
        hierarchy::pacing_target(&target).unwrap().amount,
        PacingAmount::Subscription {
            basis_points: 100,
            ..
        }
    ));
    let mut value = target.clone();
    value.unit = "currency".into();
    value.denomination = "USD".into();
    value.amount = 1_000_000;
    assert!(matches!(
        hierarchy::pacing_target(&value).unwrap().amount,
        PacingAmount::Currency {
            micros: 1_000_000,
            ..
        }
    ));
    value.unit = "credits".into();
    assert!(hierarchy::pacing_target(&value).is_err());
    value.denomination.clear();
    assert!(hierarchy::pacing_target(&value).is_ok());
    for amount in [0, 10001, u64::MAX] {
        let mut invalid = target.clone();
        invalid.amount = amount;
        assert!(hierarchy::pacing_target(&invalid).is_err());
    }
    let mut invalid = target;
    invalid.interval_seconds = 0;
    assert!(hierarchy::pacing_target(&invalid).is_err());
    let root =
        std::env::temp_dir().join(format!("beale-pacing-contract-example-{}", Uuid::new_v4()));
    let db = beale_storage::HostDatabase::open(&root).unwrap();
    assert!(ensure_local_execution(&db, &request).is_err());
    drop(db);
    let path = root.canonicalize().unwrap();
    assert!(path.starts_with(std::env::temp_dir().canonicalize().unwrap()));
    std::fs::remove_dir_all(path).unwrap();
    let detail = SessionDetail {
        pacing_warnings: vec!["Synthetic accounting warning".into()],
        ..Default::default()
    };
    assert_eq!(
        SessionDetail::decode(detail.encode_to_vec().as_slice()).unwrap(),
        detail
    );
}

#[tokio::test]
async fn ordinary_scans_remain_local_without_swarm_tools_or_leader_rules() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let app = Router::new().route("/v1/chat/completions", axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
        let observed = observed.clone();
        async move {
            observed.fetch_add(1, Ordering::SeqCst);
            let tools = body["tools"].as_array().unwrap();
            assert!(tools.iter().all(|t| !t["function"]["name"].as_str().unwrap().starts_with("swarm_")));
            assert!(!body["messages"][0]["content"].as_str().unwrap().contains("Swarm agent identity:"));
            let chunk = json!({"choices":[{"delta":{"content":"Synthetic ordinary research complete"}}]});
            ([("content-type", "text/event-stream")], format!("data: {chunk}\n\ndata: [DONE]\n\n"))
        }
    }));
    let (root, state, mut request, server) = regression_session(app).await;
    request.swarm_id.clear();
    for enabled in [false, true] {
        state.db.set_feature_enabled("swarms", enabled).unwrap();
        let detail = engine::run_turn(state.clone(), request.clone(), None, None)
            .await
            .unwrap();
        assert!(detail.swarm_id.is_empty());
        assert!(detail.swarm_agents.is_empty());
        assert!(detail.scoped_review_rules.is_empty());
        assert_eq!(
            detail.messages.last().unwrap().text,
            "Synthetic ordinary research complete"
        );
        let mut invalid = request.clone();
        invalid
            .swarm_destinations
            .push(beale_contracts::v1::SwarmDestination {
                host_id: "host-example".into(),
                configuration_id: "image-example".into(),
            });
        assert!(
            engine::run_turn(state.clone(), invalid, None, None)
                .await
                .is_err()
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    server.abort();
    drop(state);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn local_stop_finishes_managed_tool_before_releasing_execution() {
    use beale_contracts::swarms::*;
    use beale_storage::swarm_state::*;
    let root = std::env::temp_dir().join(format!("beale-local-tool-example-{}", Uuid::new_v4()));
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let state = build_state(
        "synthetic-token".into(),
        WorkspaceStore::at(root.join("host")).unwrap(),
    )
    .unwrap();
    let store = SwarmStore::open(state.db.clone()).unwrap();
    let scan = Uuid::new_v4().to_string();
    let saved = store
        .create_scan(
            ScanConfig {
                version: VERSION,
                scan_id: scan.clone(),
                workspace_id: "workspace-example".into(),
                mode: ScanMode::Swarm {
                    snapshot: Box::new(beale_swarms::BASIC_SWARM.snapshot.clone()),
                },
                authorized_scope: "Synthetic local fixture".into(),
                enabled_tools: vec![],
                provider_routes: vec![],
                destinations: vec![],
                pacing: vec![],
                retention: RetentionPolicy::default(),
            },
            true,
            true,
        )
        .unwrap();
    let leader = &saved.agents[0].summary.id;
    store
        .local_initialize(
            &scan,
            LocalSettings {
                models: vec![LocalModel {
                    profile: "leader".into(),
                    provider: 1,
                    method: 1,
                    model: "example-model".into(),
                }],
                reasoning_effort: 0,
            },
            "Coordinate",
        )
        .unwrap();
    let turn = store.local_claim(&scan, leader, 1, 1).unwrap().unwrap();
    let redactor = beale_tools::Redactor::new(Vec::<String>::new());
    let defs = beale_tools::definitions();
    let plugins = HashSet::new();
    let runtime = tool_runtime::ToolRuntime {
        local: Some(&turn),
        state: &state,
        workspace_id: "workspace-example",
        session_id: &scan,
        agent_id: leader,
        agent_role: "leader",
        workspace: &workspace,
        review: tool_runtime::ReviewTarget {
            provider: Provider::OpenAi,
            mode: AuthMode::ApiKey,
            model: "example-model",
            context: "Synthetic fixture",
        },
        definitions: &defs,
        callable_plugins: &plugins,
        redactor: &redactor,
        mutation_lock: None,
        progress: None,
    };
    let (tx, rx) = watch::channel(false);
    let mut stop = Some(rx);
    #[cfg(windows)]
    let command = "Set-Content -LiteralPath started.txt -Value started; Start-Sleep -Milliseconds 500; Set-Content -LiteralPath finished.txt -Value finished";
    #[cfg(not(windows))]
    let command = "printf started > started.txt; sleep 0.5; printf finished > finished.txt";
    let execute = runtime.execute(
        "example-tool",
        beale_tools::COMMAND_TOOL_NAME,
        Ok(json!({"command":command,"timeout_seconds":10})),
        true,
        &[],
        &mut stop,
    );
    let cancel = async {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !workspace.join("started.txt").exists() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        tx.send(true).unwrap();
        store.local_stop(&scan, leader).unwrap();
        assert!(
            store
                .load(&scan, true)
                .unwrap()
                .attempts
                .iter()
                .any(|a| a.state == ExecutionState::Running)
        );
    };
    let (result, ()) = tokio::join!(execute, cancel);
    assert!(!result.is_error, "{}", result.output);
    assert!(workspace.join("finished.txt").exists());
    store
        .local_finish(&turn, json!({"partial":"preserved"}), &result.output, true)
        .unwrap();
    assert!(
        store
            .load(&scan, true)
            .unwrap()
            .attempts
            .iter()
            .all(|a| a.state == ExecutionState::Stopped)
    );
    drop(store);
    drop(state);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn local_tree_executes_and_resumes_after_feature_disable() {
    let root = std::env::temp_dir().join(format!("beale-local-example-{}", Uuid::new_v4()));
    let workspaces = WorkspaceStore::at(root.join("host")).unwrap();
    let create = beale_contracts::v1::CreateWorkspaceRequest {
        name: "Example Workspace".into(),
        directory: root.join("workspace").to_string_lossy().into_owned(),
        guidance: "Synthetic example.test research only".into(),
        rules: vec!["Only synthetic example.test fixtures are authorized.".into()],
        ..Default::default()
    };
    let response = beale_workspaces::router("example-token".into(), workspaces.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(beale_contracts::WORKSPACES_PATH)
                .header("authorization", "Bearer example-token")
                .header("content-type", beale_contracts::PROTOBUF_CONTENT_TYPE)
                .body(Body::from(create.encode_to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let workspace = beale_contracts::v1::WorkspaceSummary::decode(
        to_bytes(response.into_body(), 65536).await.unwrap(),
    )
    .unwrap();
    let state = build_state("example-token".into(), workspaces).unwrap();
    state.db.set_feature_enabled("swarms", true).unwrap();
    let counts = Arc::new(std::sync::Mutex::new(HashMap::<String, usize>::new()));
    let captured = counts.clone();
    let failure = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fault = failure.clone();
    let db = state.db.clone();
    let app = Router::new().route("/v1/chat/completions",axum::routing::post(move |axum::Json(body):axum::Json<Value>| {
        let counts=captured.clone(); let db=db.clone(); let fault=fault.clone();
        async move {
            let system = body["messages"][0]["content"].as_str().unwrap();
            let delta = if fault.load(std::sync::atomic::Ordering::SeqCst) { json!({"content":""}) } else if system.contains("host-side reviewer") {
                assert!(system.contains("Trusted host role: leader"));
                json!({"content":json!({"decision":"approved","reason":"Managerial delegation"}).to_string()})
            } else {
                let role = if system.contains("Role: Leader") { "leader" } else if system.contains("Role: Deputy") {"deputy"} else {"worker"};
                let mut counts=counts.lock().unwrap();
                let count=counts.entry(role.into()).or_default(); *count+=1;
                if *count==1 && role!="worker" {
                    if role=="deputy" { db.set_feature_enabled("swarms",false).unwrap(); }
                    let profile = if role=="leader" {"research_deputy"} else {"investigator"};
                    // Profile IDs come from the selected immutable bundled definition.
                    json!({"tool_calls":[{"index":0,"id":format!("call-{role}"),"type":"function","function":{"name":"swarm_spawn_subagent","arguments":json!({"profile":{"id":profile,"revision":1},"task":{"question":"Inspect synthetic fixture","completion_criteria":["Return evidence"],"evidence_refs":[]},"independent":false}).to_string()}}]})
                } else { json!({"content":format!("{role} synthetic result")}) }
            };
            let chunk=json!({"choices":[{"delta":delta}]});
            ([("content-type","text/event-stream")],format!("data: {chunk}\n\ndata: [DONE]\n\n"))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    state
        .credentials
        .save_custom_provider(beale_providers::CustomProvider {
            id: "custom-example".into(),
            name: "Example".into(),
            base_url: format!("http://{addr}/v1"),
            models: vec![beale_providers::ModelEntry {
                id: "example".into(),
                name: "Example".into(),
            }],
            lead_model: "example".into(),
            small_model: "example".into(),
            reasoning: false,
        })
        .unwrap();
    let request = SubmitSessionTurnRequest {
        workspace_id: workspace.id,
        prompt: "Coordinate synthetic research".into(),
        swarm_id: "basic".into(),
        provider: ProviderId::Custom as i32,
        method: AuthMethod::ApiKey as i32,
        model: "custom-example::example".into(),
        ..Default::default()
    };
    let detail = engine::run_turn(state.clone(), request.clone(), None, None)
        .await
        .unwrap();
    assert_eq!(detail.swarm_agents.len(), 3, "{detail:?}");
    assert!(detail.swarm_agents.iter().all(|a| a.status == "completed"));
    assert_eq!(detail.scoped_review_rules.len(), 1);
    assert!(counts.lock().unwrap()["worker"] > 0);
    let id = detail.summary.as_ref().unwrap().id.clone();
    let ended = beale_storage::swarm_state::SwarmStore::open(state.db.clone())
        .unwrap()
        .load(&id, true)
        .unwrap()
        .ended_at_ms
        .unwrap();
    assert!(
        beale_storage::fleet_allocation::AllocationStore::open(state.db.clone())
            .unwrap()
            .environments()
            .unwrap()
            .iter()
            .all(|e| e.deadline_ms == Some(ended + 604800000))
    );
    let rule = detail.scoped_review_rules.clone();
    let snapshot = detail.swarm_template_json.clone();
    let resumed = engine::run_turn(
        state.clone(),
        SubmitSessionTurnRequest {
            session_id: id.clone(),
            swarm_id: String::new(),
            prompt: "Summarize saved results".into(),
            ..request.clone()
        },
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(resumed.scoped_review_rules, rule);
    assert_eq!(resumed.swarm_template_json, snapshot);
    let overview: beale_contracts::swarm_ui::ScanOverview =
        serde_json::from_str(&resumed.swarm_overview_json).unwrap();
    assert_eq!(overview.agents.len(), 3);
    assert_eq!(overview.definition_revision, 1);
    assert!(!overview.leader_rule.is_empty());
    assert!(
        overview
            .agents
            .iter()
            .any(|a| a.summary.parent_id.is_some())
    );
    let (persisted, turns) = load_persisted(&state.db, parse_id(&id).unwrap()).unwrap();
    let reconnected = super::detail(&state.db, &persisted, &turns).unwrap();
    assert_eq!(reconnected.swarm_overview_json, resumed.swarm_overview_json);
    assert_eq!(reconnected.scoped_review_rules, rule);

    let child = detail
        .swarm_agents
        .iter()
        .find(|a| a.agent_kind == "worker")
        .unwrap();
    assert_eq!(
        engine::run_turn(
            state.clone(),
            SubmitSessionTurnRequest {
                session_id: child.id.clone(),
                swarm_id: String::new(),
                ..request.clone()
            },
            None,
            None
        )
        .await,
        Err(TurnError::Conflict)
    );
    assert_eq!(
        engine::run_turn(state.clone(), request.clone(), None, None).await,
        Err(TurnError::Conflict)
    );
    let (_, turns) = load_persisted(&state.db, parse_id(&id).unwrap()).unwrap();
    assert_eq!(turns[0].prompt, "Coordinate synthetic research");
    state.db.set_feature_enabled("swarms", true).unwrap();
    failure.store(true, std::sync::atomic::Ordering::SeqCst);
    let failed = engine::run_turn(state.clone(), request, None, None)
        .await
        .unwrap();
    assert!(
        failed
            .messages
            .last()
            .unwrap()
            .text
            .contains("bounded local recovery")
    );
    assert!(failed.swarm_agents.iter().all(|a| a.status == "stopped"));
    let saved = beale_storage::swarm_state::SwarmStore::open(state.db.clone())
        .unwrap()
        .load(&failed.summary.unwrap().id, true)
        .unwrap();
    assert!(saved.ended_at_ms.is_some());
    assert_eq!(
        saved
            .events
            .iter()
            .filter(|e| e.kind == "local-recovery")
            .count(),
        3
    );
    server.abort();
    drop(state);
    let _ = std::fs::remove_dir_all(root);
}

async fn regression_session(
    app: Router,
) -> (
    std::path::PathBuf,
    SessionState,
    SubmitSessionTurnRequest,
    tokio::task::JoinHandle<()>,
) {
    let root = std::env::temp_dir().join(format!("beale-recovery-example-{}", Uuid::new_v4()));
    let workspaces = WorkspaceStore::at(root.join("host")).unwrap();
    let create = beale_contracts::v1::CreateWorkspaceRequest {
        name: "Example Workspace".into(),
        directory: root.join("workspace").to_string_lossy().into_owned(),
        rules: vec!["Only synthetic example.test fixtures are authorized.".into()],
        ..Default::default()
    };
    let response = beale_workspaces::router("example-token".into(), workspaces.clone())
        .oneshot(regression_request(
            beale_contracts::WORKSPACES_PATH,
            create.encode_to_vec(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let workspace = beale_contracts::v1::WorkspaceSummary::decode(
        to_bytes(response.into_body(), 65536).await.unwrap(),
    )
    .unwrap();
    let state = build_state("example-token".into(), workspaces).unwrap();
    state.db.set_feature_enabled("swarms", true).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    state
        .credentials
        .save_custom_provider(beale_providers::CustomProvider {
            id: "custom-example".into(),
            name: "Example".into(),
            base_url: format!("http://{addr}/v1"),
            models: vec![beale_providers::ModelEntry {
                id: "example".into(),
                name: "Example".into(),
            }],
            lead_model: "example".into(),
            small_model: "example".into(),
            reasoning: false,
        })
        .unwrap();
    let request = SubmitSessionTurnRequest {
        workspace_id: workspace.id,
        prompt: "Coordinate synthetic research".into(),
        swarm_id: "basic".into(),
        provider: ProviderId::Custom as i32,
        method: AuthMethod::ApiKey as i32,
        model: "custom-example::example".into(),
        ..Default::default()
    };
    (root, state, request, server)
}

fn regression_request(path: &str, bytes: Vec<u8>) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("authorization", "Bearer example-token")
        .header("content-type", beale_contracts::PROTOBUF_CONTENT_TYPE)
        .body(Body::from(bytes))
        .unwrap()
}

#[tokio::test]
async fn local_stream_stays_open_through_provider_recovery_and_exhaustion() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    // Both provider errors and empty completions must defer termination to the supervisor.
    for mode in ["empty", "error", "exhausted"] {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let app = Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(move || {
                let count = count.clone();
                async move {
                    let n = count.fetch_add(1, Ordering::SeqCst);
                    if mode == "error" && n == 0 {
                        return (StatusCode::BAD_REQUEST, "Synthetic provider failure")
                            .into_response();
                    }
                    let content = if n == 0 || mode == "exhausted" {
                        ""
                    } else {
                        "Recovered synthetic answer"
                    };
                    let chunk = json!({"choices":[{"delta":{"content":content}}]});
                    (
                        [("content-type", "text/event-stream")],
                        format!("data: {chunk}\n\ndata: [DONE]\n\n"),
                    )
                        .into_response()
                }
            }),
        );
        let (root, state, request, server) = regression_session(app).await;
        let route = Router::new()
            .route(SESSION_TURN_STREAM_PATH, axum::routing::post(stream_turn))
            .with_state(state.clone());
        let response = route
            .oneshot(regression_request(
                SESSION_TURN_STREAM_PATH,
                request.encode_to_vec(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            to_bytes(response.into_body(), 4_500_000),
        )
        .await
        .unwrap()
        .unwrap();
        let mut frames = bytes.as_ref();
        let mut events = vec![];
        while !frames.is_empty() {
            let len = u32::from_be_bytes(frames[..4].try_into().unwrap()) as usize;
            events.push(SessionStreamEvent::decode(&frames[4..4 + len]).unwrap());
            frames = &frames[4 + len..];
        }
        assert!(
            !events
                .iter()
                .any(|e| e.kind == SessionStreamKind::Failed as i32),
            "{mode}: {events:?}"
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| e.kind == SessionStreamKind::Completed as i32)
                .count(),
            1
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            if mode == "exhausted" { 3 } else { 2 }
        );
        let detail = events.last().unwrap().detail.as_ref().unwrap();
        let answer = &detail.messages.last().unwrap().text;
        if mode == "exhausted" {
            assert!(answer.contains("bounded local recovery"));
            assert!(detail.swarm_agents.iter().all(|a| a.status == "stopped"));
        } else {
            assert_eq!(answer, "Recovered synthetic answer");
        }
        let saved = state
            .db
            .session(&detail.summary.as_ref().unwrap().id)
            .unwrap()
            .unwrap();
        assert_eq!(&saved.turns.last().unwrap().answer, answer);
        server.abort();
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }
}

#[tokio::test]
async fn checkpoint_exhaustion_does_not_repeat_accepted_spawn() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    // Exercise both transcript persistence and the subsequent supervisor checkpoint.
    for fault in ["transcript", "supervisor"] {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let app = Router::new().route("/v1/chat/completions", axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
            let count = count.clone();
            async move {
                let system = body["messages"][0]["content"].as_str().unwrap();
                let delta = if system.contains("host-side reviewer") {
                    json!({"content":json!({"decision":"approved","reason":"Managerial delegation"}).to_string()})
                } else {
                    assert!(system.contains("Role: Leader"), "Child must not execute after leader checkpoint failure");
                    let n = count.fetch_add(1, Ordering::SeqCst);
                    if n.is_multiple_of(2) {
                        json!({"tool_calls":[{"index":0,"id":"same-provider-call","type":"function","function":{"name":"swarm_spawn_subagent","arguments":json!({"profile":{"id":"research_deputy","revision":1},"task":{"question":"Inspect synthetic fixture","completion_criteria":["Return evidence"],"evidence_refs":[]},"independent":false}).to_string()}}]})
                    } else { json!({"content":"Delegation queued; waiting for result"}) }
                };
                let chunk = json!({"choices":[{"delta":delta}]});
                ([("content-type", "text/event-stream")], format!("data: {chunk}\n\ndata: [DONE]\n\n"))
            }
        }));
        let (root, state, request, server) = regression_session(app).await;
        beale_storage::swarm_state::SwarmStore::open(state.db.clone()).unwrap();
        state.db.with_connection(|c| {
            c.execute_batch(if fault == "transcript" {
                "CREATE TRIGGER fail_checkpoint BEFORE INSERT ON session_turns WHEN NEW.answer LIKE 'Delegation queued%' BEGIN SELECT RAISE(FAIL, 'synthetic checkpoint failure'); END;"
            } else {
                "CREATE TRIGGER fail_checkpoint BEFORE UPDATE ON swarm_scans WHEN json_extract(NEW.document, '$.agents[0].checkpoint.context.saved') = 1 BEGIN SELECT RAISE(FAIL, 'synthetic checkpoint failure'); END;"
            })?;
            Ok(())
        }).unwrap();
        let detail = engine::run_turn(state.clone(), request, None, None)
            .await
            .unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "{fault}: inference must not replay after persistence failure"
        );
        let store = beale_storage::swarm_state::SwarmStore::open(state.db.clone()).unwrap();
        let scan = &detail.summary.as_ref().unwrap().id;
        let saved = store.load(scan, true).unwrap();
        assert_eq!(saved.assignments.len(), 1);
        assert_eq!(saved.agents.len(), 2);
        assert!(
            saved
                .agents
                .iter()
                .all(|a| a.summary.state == beale_contracts::swarms::AgentState::Stopped)
        );
        assert!(
            saved
                .events
                .iter()
                .any(|e| e.payload["checkpoint_exhausted"] == true)
        );
        assert_eq!(
            store.local_state(scan).unwrap().controls[0]
                .tool_results
                .as_object()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(state.db.sessions_excluding("").unwrap().len(), 1);
        server.abort();
        drop(store);
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }
}

#[tokio::test]
async fn transient_checkpoint_retries_same_atomic_write() {
    let root = std::env::temp_dir().join(format!("beale-checkpoint-example-{}", Uuid::new_v4()));
    let db = HostDatabase::open(&root).unwrap();
    db.with_connection(|c| {
        c.execute_batch("CREATE TRIGGER fail_once BEFORE INSERT ON session_turns BEGIN SELECT RAISE(FAIL, 'synthetic checkpoint failure'); END;")?;
        Ok(())
    }).unwrap();
    let session = SessionRow {
        id: Uuid::new_v4().to_string(),
        workspace_id: "workspace-example".into(),
        title: "Synthetic checkpoint".into(),
        created_at_ms: 1,
        last_message_at_ms: 1,
    };
    let turn = TurnRow {
        number: 1,
        provider: ProviderId::Custom as i32,
        method: AuthMethod::ApiKey as i32,
        model: "custom-example::example".into(),
        prompt: "Coordinate synthetic research".into(),
        answer: "Same completed turn".into(),
        duration_ms: 1,
        internal_prompt: false,
    };
    let mut attempts = 0;
    engine::save_checkpoint(true, || {
        attempts += 1;
        let result = db.append_turn(&session, &turn, true);
        if attempts == 1 {
            assert!(result.is_err());
            assert!(
                db.session(&session.id).unwrap().is_none(),
                "Failed transaction must roll back the session too"
            );
            db.with_connection(|c| {
                c.execute_batch("DROP TRIGGER fail_once")?;
                Ok(())
            })
            .unwrap();
        }
        result
    })
    .await
    .unwrap();
    assert_eq!(attempts, 2);
    let saved = db.session(&session.id).unwrap().unwrap();
    assert_eq!(saved.turns, vec![turn]);
    let mut failures = 0;
    assert_eq!(
        engine::save_checkpoint(true, || -> anyhow::Result<()> {
            failures += 1;
            anyhow::bail!("synthetic persistent failure")
        })
        .await,
        Err(engine::TurnError::Checkpoint)
    );
    assert_eq!(failures, 3);
    failures = 0;
    assert_eq!(
        engine::save_checkpoint(false, || -> anyhow::Result<()> {
            failures += 1;
            anyhow::bail!("synthetic ordinary failure")
        })
        .await,
        Err(engine::TurnError::Internal)
    );
    assert_eq!(failures, 1);
    drop(db);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn provider_recovery_preserves_exchange_and_deduplicates_tool_replies() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    for mode in [
        "error",
        "empty",
        "replay",
        "changed",
        "distinct",
        "exhausted",
        "empty-exhausted",
        "review-exhausted",
    ] {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let reviews = Arc::new(AtomicUsize::new(0));
        let app = Router::new().route("/v1/chat/completions", axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
            let count = count.clone();
            let reviews = reviews.clone();
            async move {
                let system = body["messages"][0]["content"].as_str().unwrap();
                let delta = if system.contains("host-side reviewer") {
                    if reviews.fetch_add(1, Ordering::SeqCst) > 0 && mode == "review-exhausted" {
                        return (StatusCode::BAD_REQUEST, "Synthetic review interruption").into_response();
                    }
                    json!({"content":json!({"decision":"approved","reason":"Managerial delegation"}).to_string()})
                } else if system.contains("Role: Leader") {
                    let n = count.fetch_add(1, Ordering::SeqCst);
                    if n == 1 || ((mode == "exhausted" || mode == "empty-exhausted") && n > 0) {
                        assert!(body["messages"].to_string().contains("assignment_id"), "{mode}: {:?}", body["messages"].as_array().unwrap().iter().filter(|m|m["role"]=="tool").collect::<Vec<_>>());
                        if mode == "empty" || mode == "empty-exhausted" {
                            let chunk = json!({"choices":[{"delta":{"content":""}}]});
                            return ([("content-type","text/event-stream")],format!("data: {chunk}\n\ndata: [DONE]\n\n")).into_response();
                        }
                        return (StatusCode::BAD_REQUEST, "Synthetic provider interruption").into_response();
                    }
                    if n == 2 {
                        assert!(body["messages"].to_string().contains("assignment_id"), "Recovery lost the accepted spawn result");
                        assert_eq!(body["messages"].as_array().unwrap().iter().filter(|m| m["role"] == "user").count(), 1, "Recovery must not repeat the original assignment");
                    }
                    if n == 0 || (n == 2 && matches!(mode, "replay" | "changed" | "distinct" | "review-exhausted")) {
                        let id = if n == 2 && matches!(mode, "distinct" | "review-exhausted") { "second-call" } else { "same-call" };
                        let question = if n == 2 && mode == "changed" { "Changed synthetic task" } else { "Inspect synthetic fixture" };
                        let arguments = json!({"profile":{"id":"research_deputy","revision":1},"task":{"question":question,"completion_criteria":["Return evidence"],"evidence_refs":[]},"independent":false});
                        let arguments = if n == 2 && mode == "replay" { serde_json::to_string_pretty(&arguments).unwrap() } else { arguments.to_string() };
                        json!({"tool_calls":[{"index":0,"id":id,"type":"function","function":{"name":"swarm_spawn_subagent","arguments":arguments}}]})
                    } else {
                        if n == 3 && mode == "replay" {
                            let results = body["messages"].as_array().unwrap().iter().filter(|m| m["role"] == "tool").collect::<Vec<_>>();
                            assert_eq!(results.len(), 2);
                            assert_eq!(results[0]["content"], results[1]["content"]);
                        }
                        json!({"content":"Delegation accepted; summarize available results"})
                    }
                } else { json!({"content":"Synthetic deputy result"}) };
                let chunk = json!({"choices":[{"delta":delta}]});
                ([("content-type","text/event-stream")],format!("data: {chunk}\n\ndata: [DONE]\n\n")).into_response()
            }
        }));
        let (root, state, request, server) = regression_session(app).await;
        let detail = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            engine::run_turn(state.clone(), request, None, None),
        )
        .await
        .unwrap()
        .unwrap();
        let store = beale_storage::swarm_state::SwarmStore::open(state.db.clone()).unwrap();
        let scan = &detail.summary.as_ref().unwrap().id;
        let saved = store.load(scan, true).unwrap();
        let expected = if mode == "distinct" { 2 } else { 1 };
        assert_eq!(saved.assignments.len(), expected, "{mode}");
        assert_eq!(
            store.local_state(scan).unwrap().controls[0]
                .tool_results
                .as_object()
                .unwrap()
                .len(),
            expected
        );
        let exhausted = matches!(
            mode,
            "changed" | "exhausted" | "empty-exhausted" | "review-exhausted"
        );
        assert!(
            saved.agents.iter().all(|a| a.summary.state
                == if exhausted {
                    beale_contracts::swarms::AgentState::Stopped
                } else {
                    beale_contracts::swarms::AgentState::Completed
                }),
            "{mode}: {saved:?}"
        );
        if exhausted {
            assert_eq!(
                calls.load(Ordering::SeqCst),
                if matches!(mode, "changed" | "review-exhausted") {
                    3
                } else {
                    4
                }
            );
            assert!(
                saved
                    .events
                    .iter()
                    .any(|e| e.payload["provider_exhausted"] == true)
            );
        }
        assert_eq!(
            state.db.tool_summaries(scan).unwrap().len(),
            expected + usize::from(mode == "review-exhausted")
        );
        server.abort();
        drop(store);
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }
}

#[tokio::test]
async fn workers_inherit_saved_deputy_stage_on_execution_and_resume() {
    use beale_contracts::{swarm_local::*, swarms::*};
    use beale_storage::swarm_state::*;
    let prompts = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let captured = prompts.clone();
    let app = Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
            let captured = captured.clone();
            async move {
                captured
                    .lock()
                    .unwrap()
                    .push(body["messages"][0]["content"].as_str().unwrap().to_owned());
                let chunk = json!({"choices":[{"delta":{"content":"Synthetic worker result"}}]});
                (
                    [("content-type", "text/event-stream")],
                    format!("data: {chunk}\n\ndata: [DONE]\n\n"),
                )
            }
        }),
    );
    let (root, state, request, server) = regression_session(app).await;
    let store = SwarmStore::open(state.db.clone()).unwrap();
    for swarm_id in ["basic", "vdh_vvs", "oai_948"] {
        let definition = beale_swarms::swarm_definition(swarm_id).unwrap();
        let deputies = definition
            .snapshot
            .profiles
            .iter()
            .filter(|p| p.role == Role::Deputy)
            .map(|p| p.profile.clone())
            .collect::<Vec<_>>();
        for deputy_profile in deputies {
            let mut snapshot = definition.snapshot.clone();
            let saved_stage = snapshot
                .profiles
                .iter_mut()
                .find(|p| p.profile == deputy_profile)
                .unwrap();
            saved_stage.instruction = format!(
                "Saved stage fixture for {}.\n{}",
                deputy_profile.id, saved_stage.instruction
            );
            let instruction = saved_stage.instruction.clone();
            let scan = Uuid::new_v4().to_string();
            let saved = store
                .create_scan(
                    ScanConfig {
                        version: VERSION,
                        scan_id: scan.clone(),
                        workspace_id: request.workspace_id.clone(),
                        mode: ScanMode::Swarm {
                            snapshot: Box::new(snapshot),
                        },
                        authorized_scope: "Synthetic example.test fixture".into(),
                        enabled_tools: vec![],
                        provider_routes: vec![],
                        destinations: vec![],
                        pacing: vec![],
                        retention: RetentionPolicy::default(),
                    },
                    true,
                    true,
                )
                .unwrap();
            research_files::capture(&state, &scan, &request.workspace_id).unwrap();
            let leader_id = &saved.agents[0].summary.id;
            store
                .local_initialize(
                    &scan,
                    LocalSettings {
                        models: vec![LocalModel {
                            profile: "leader".into(),
                            provider: request.provider,
                            method: request.method,
                            model: request.model.clone(),
                        }],
                        reasoning_effort: 0,
                    },
                    "Coordinate synthetic fixture",
                )
                .unwrap();
            let leader = store.local_claim(&scan, leader_id, 1, 64).unwrap().unwrap();
            let task = Task {
                question: "Inspect synthetic fixture".into(),
                completion_criteria: vec!["Return evidence".into()],
                evidence_refs: vec![],
            };
            let deputy_id = store
                .local_dispatch(
                    &leader,
                    "spawn-deputy",
                    SwarmCommand::Spawn(Spawn {
                        handoff_id: None,
                        profile: deputy_profile.clone(),
                        task: task.clone(),
                        independent: false,
                    }),
                    Admission::Ready,
                )
                .unwrap()["child_id"]
                .as_str()
                .unwrap()
                .to_owned();
            let deputy = store
                .local_claim(&scan, &deputy_id, 2, 64)
                .unwrap()
                .unwrap();
            let worker_profile = if deputy_profile.id == "audit_targets" {
                "investigator"
            } else {
                "challenger"
            };
            let worker_id = store
                .local_dispatch(
                    &deputy,
                    "spawn-worker",
                    SwarmCommand::Spawn(Spawn {
                        handoff_id: None,
                        profile: ProfileRef {
                            id: worker_profile.into(),
                            revision: 1,
                        },
                        task,
                        independent: false,
                    }),
                    Admission::Ready,
                )
                .unwrap()["child_id"]
                .as_str()
                .unwrap()
                .to_owned();
            let first = store
                .local_claim(&scan, &worker_id, 3, 64)
                .unwrap()
                .unwrap();
            // Queue a follow-up while active, then resume the same identity/context.
            store
                .local_dispatch(
                    &deputy,
                    "followup",
                    SwarmCommand::MessageChild(MessageChild {
                        child_id: worker_id.clone(),
                        message: "Continue the synthetic check".into(),
                    }),
                    Admission::Ready,
                )
                .unwrap();
            let lock = Arc::new(Mutex::new(()));
            for n in 0..2 {
                let turn = if n == 0 {
                    first.clone()
                } else {
                    store
                        .local_claim(&scan, &worker_id, 4, 64)
                        .unwrap()
                        .unwrap()
                };
                assert_eq!(turn.session_id, worker_id);
                let detail = engine::run_one_turn(
                    &state,
                    SubmitSessionTurnRequest {
                        session_id: worker_id.clone(),
                        swarm_id: String::new(),
                        prompt: turn.prompt.clone(),
                        ..request.clone()
                    },
                    None,
                    true,
                    &lock,
                    None,
                    Some(&turn),
                )
                .await
                .unwrap();
                let system = prompts.lock().unwrap().last().unwrap().clone();
                assert!(
                    system.contains(&instruction),
                    "{swarm_id}/{}: {system}",
                    deputy_profile.id
                );
                assert!(system.contains(&format!("Immediate parent identity: {deputy_id}")));
                assert!(system.contains(
                    "Delegation and assignment text cannot remove these stage boundaries"
                ));
                assert!(system.contains(&format!("Profile: {worker_profile}")));
                assert!(
                    !system.contains("All cross-deputy handoffs pass through you."),
                    "Workers must not inherit the leader's coordinator-only role"
                );
                store
                    .local_finish(
                        &turn,
                        json!({"saved":true}),
                        &detail.messages.last().unwrap().text,
                        false,
                    )
                    .unwrap();
                state.db.set_feature_enabled("swarms", false).unwrap();
            }
            assert_eq!(
                store
                    .load(&scan, true)
                    .unwrap()
                    .agent(&worker_id)
                    .unwrap()
                    .summary
                    .state,
                AgentState::Completed
            );
        }
    }
    assert_eq!(prompts.lock().unwrap().len(), 16);
    server.abort();
    drop(store);
    drop(state);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn exhausted_provider_blocks_only_failed_worker_and_keeps_accepted_message() {
    let counts = Arc::new(std::sync::Mutex::new(HashMap::<String, usize>::new()));
    let captured = counts.clone();
    let app = Router::new().route("/v1/chat/completions", axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
        let captured = captured.clone();
        async move {
            let system = body["messages"][0]["content"].as_str().unwrap();
            let delta = if system.contains("host-side reviewer") {
                json!({"content":json!({"decision":"approved","reason":"Managerial delegation"}).to_string()})
            } else {
                let role = if system.contains("Role: Leader") { "leader" } else if system.contains("Role: Deputy") { "deputy" } else { "worker" };
                let n = { let mut counts = captured.lock().unwrap(); let count = counts.entry(role.into()).or_default(); let n = *count; *count += 1; n };
                if role == "worker" && (1..=3).contains(&n) {
                    assert!(body["messages"].to_string().contains("parent_id"));
                    return (StatusCode::SERVICE_UNAVAILABLE, "Synthetic worker provider interruption").into_response();
                }
                if n == 0 && role != "worker" {
                    let profile = if role == "leader" { "research_deputy" } else { "investigator" };
                    let count = if role == "leader" { 1 } else { 2 };
                    let calls = (0..count).map(|i| json!({"index":i,"id":format!("spawn-{i}"),"type":"function","function":{"name":"swarm_spawn_subagent","arguments":json!({"profile":{"id":profile,"revision":1},"task":{"question":"Inspect synthetic fixture","completion_criteria":["Return evidence"],"evidence_refs":[]},"independent":false}).to_string()}})).collect::<Vec<_>>();
                    json!({"tool_calls":calls})
                } else if n == 0 && role == "worker" {
                    json!({"tool_calls":[{"index":0,"id":"accepted-message","type":"function","function":{"name":"swarm_message_parent","arguments":json!({"message":"Synthetic partial evidence retained","handoff":null}).to_string()}}]})
                } else { json!({"content":format!("{role} synthetic result")}) }
            };
            let chunk = json!({"choices":[{"delta":delta}]});
            ([("content-type","text/event-stream")],format!("data: {chunk}\n\ndata: [DONE]\n\n")).into_response()
        }
    }));
    let (root, state, request, server) = regression_session(app).await;
    let detail = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        engine::run_turn(state.clone(), request, None, None),
    )
    .await
    .unwrap()
    .unwrap();
    let store = beale_storage::swarm_state::SwarmStore::open(state.db.clone()).unwrap();
    let scan = &detail.summary.as_ref().unwrap().id;
    let saved = store.load(scan, true).unwrap();
    let blocked = saved
        .agents
        .iter()
        .filter(|a| a.summary.state == beale_contracts::swarms::AgentState::Blocked)
        .collect::<Vec<_>>();
    assert_eq!(blocked.len(), 1);
    assert_eq!(
        blocked[0].summary.role,
        beale_contracts::swarms::Role::Worker
    );
    assert_eq!(
        saved
            .agents
            .iter()
            .filter(|a| a.summary.state == beale_contracts::swarms::AgentState::Completed)
            .count(),
        3
    );
    assert_eq!(counts.lock().unwrap()["worker"], 5);
    let local = store.local_state(scan).unwrap();
    assert_eq!(
        local
            .controls
            .iter()
            .find(|c| c.agent_id == blocked[0].summary.id)
            .unwrap()
            .tool_results
            .as_object()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        state
            .db
            .tool_summaries(&blocked[0].summary.id)
            .unwrap()
            .len(),
        1
    );
    assert!(
        saved
            .events
            .iter()
            .any(|e| e.agent_id == blocked[0].summary.id && e.payload["worker_exhausted"] == true)
    );
    assert!(
        local.inbox.is_empty(),
        "Parents acknowledged all partial results and recovery notifications"
    );
    server.abort();
    drop(store);
    drop(state);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn live_status_stream_reports_changes_while_provider_is_blocked() {
    use beale_storage::{provider_accounting::AccountingStore, swarm_state::SwarmStore};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let entered = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let calls = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route("/v1/chat/completions", axum::routing::post({
        let entered = entered.clone(); let release = release.clone();
        move || {
            let entered = entered.clone(); let release = release.clone(); let calls = calls.clone();
            async move {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    entered.add_permits(1);
                    release.acquire().await.unwrap().forget();
                }
                let chunk = json!({"choices":[{"delta":{"content":"Synthetic completed coordination"}}]});
                ([("content-type", "text/event-stream")], format!("data: {chunk}\n\ndata: [DONE]\n\n"))
            }
        }
    }));
    let (root, state, request, server) = regression_session(app).await;
    let (tx, mut rx) = mpsc::channel(32);
    let task = tokio::spawn(hierarchy::run(state.clone(), request, Some(tx), None));
    tokio::time::timeout(std::time::Duration::from_secs(15), entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let first = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let event = rx.recv().await.unwrap();
            if event.kind == SessionStreamKind::SwarmUpdated as i32 {
                let detail = event.detail.unwrap();
                if detail.swarm_agents.iter().any(|a| a.status == "running") {
                    break detail;
                }
            }
        }
    })
    .await
    .unwrap();
    assert!(!task.is_finished());
    assert!(first.messages.is_empty()); // First-turn status does not fabricate durable messages.
    let scan = first.summary.as_ref().unwrap().id.clone();
    let leader = first
        .swarm_agents
        .iter()
        .find(|a| a.agent_kind == "leader")
        .unwrap()
        .id
        .clone();
    AccountingStore::open(state.db.clone())
        .unwrap()
        .warn(&scan, "Synthetic pacing warning")
        .unwrap();
    SwarmStore::open(state.db.clone())
        .unwrap()
        .local_message(
            &scan,
            &Uuid::new_v4().to_string(),
            &leader,
            "Synthetic follow-up",
        )
        .unwrap();
    let updated = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let event = rx.recv().await.unwrap();
            if event.kind == SessionStreamKind::SwarmUpdated as i32 {
                let detail = event.detail.unwrap();
                let overview: beale_contracts::swarm_ui::ScanOverview =
                    serde_json::from_str(&detail.swarm_overview_json).unwrap();
                if detail
                    .pacing_warnings
                    .iter()
                    .any(|w| w == "Synthetic pacing warning")
                    && overview.agents.iter().any(|a| a.queued_wakes > 0)
                {
                    break detail;
                }
            }
        }
    })
    .await
    .unwrap();
    assert!(!task.is_finished());
    assert_eq!(updated.summary.as_ref().unwrap().id, scan);
    release.add_permits(1);
    let final_detail = tokio::time::timeout(std::time::Duration::from_secs(20), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        final_detail
            .pacing_warnings
            .contains(&"Synthetic pacing warning".to_owned())
    );
    assert_eq!(final_detail.committed_turn_number, 2);
    let mut streamed_turns = std::collections::BTreeSet::new();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while let Some(event) = rx.recv().await {
            if event.kind == SessionStreamKind::MessageDelta as i32 {
                streamed_turns.insert(event.turn_number);
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(streamed_turns, [1, 2].into_iter().collect());
    server.abort();
    drop(state);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn live_and_reconnected_details_read_consistent_usage_during_commits() {
    use beale_storage::{ModelUsageRow, TurnActivity, TurnMetadata};
    use std::sync::atomic::{AtomicBool, Ordering};
    let (root, state, request, server) = regression_session(Router::new()).await;
    beale_storage::provider_accounting::AccountingStore::open(state.db.clone()).unwrap();
    let session = SessionRow {
        id: Uuid::new_v4().to_string(),
        workspace_id: request.workspace_id.clone(),
        title: "Synthetic review".into(),
        created_at_ms: 1,
        last_message_at_ms: 1,
    };
    let append = |db: &HostDatabase, s: &SessionRow, n: i64| {
        db.append_turn_with_metadata(
            s,
            &TurnRow {
                number: n,
                provider: 1,
                method: 1,
                model: "example-model".into(),
                prompt: "Synthetic prompt".into(),
                answer: "Synthetic answer".into(),
                duration_ms: 1,
                internal_prompt: false,
            },
            n == 1,
            TurnActivity {
                tools: &[],
                reasoning: &[],
                commentary: &[],
            },
            None,
            TurnMetadata {
                model_usage: Some(&[ModelUsageRow {
                    agent_id: "main".into(),
                    role: "main".into(),
                    provider: 1,
                    method: 1,
                    model: "example-model".into(),
                    input_tokens: Some(100),
                    output_tokens: Some(10),
                    cache_read_tokens: None,
                    work_elapsed_ms: Some(1),
                    duration_ms: 1,
                    observed_at_ms: n,
                }]),
                ..Default::default()
            },
        )
        .unwrap();
    };
    append(&state.db, &session, 1);
    let writing = Arc::new(AtomicBool::new(true));
    let flag = writing.clone();
    let db = state.db.clone();
    let row = session.clone();
    let writer = std::thread::spawn(move || {
        for n in 2..=300 {
            append(&db, &row, n);
            std::thread::yield_now();
        }
        flag.store(false, Ordering::SeqCst);
    });
    let mut invalid = 0;
    let mut checked = 0;
    while writing.load(Ordering::SeqCst) || checked < 100 {
        let d = if checked % 2 == 0 {
            let mut headers = HeaderMap::new();
            headers.insert("authorization", "Bearer example-token".parse().unwrap());
            let response = details(State(state.clone()), RoutePath(session.id.clone()), headers)
                .await
                .unwrap()
                .into_response();
            SessionDetail::decode(to_bytes(response.into_body(), 4_500_000).await.unwrap()).unwrap()
        } else {
            hierarchy::live_detail(&state, &session.id, &request, 1).unwrap()
        };
        assert_eq!(
            d.committed_turn_number * 100,
            d.token_usage.as_ref().unwrap().input_tokens
        );
        let tokens = d.token_usage.as_ref().unwrap().input_tokens;
        let model_tokens: u64 = d
            .model_usage
            .iter()
            .map(|m| m.usage.as_ref().unwrap().input_tokens)
            .sum();
        if tokens != model_tokens
            || d.usage_snapshots.last().is_some_and(|p| {
                p.input_tokens != tokens || p.work_duration_ms != d.total_duration_ms
            })
        {
            invalid += 1;
        }
        checked += 1;
    }
    writer.join().unwrap();
    server.abort();
    drop(state);
    let _ = std::fs::remove_dir_all(root);
    assert_eq!(
        invalid, 0,
        "{invalid}/{checked} snapshots violate Desktop's usage validation"
    );
}
