use super::*;
use beale_storage::swarm_state::SwarmStore;
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

#[tokio::test]
async fn completed_coordinator_owner_loss_finalizes_retention_without_reinference() {
    completed_coordinator_recovery(false).await;
}

#[tokio::test]
async fn completed_coordinator_end_write_failure_retries_through_normal_maintenance() {
    completed_coordinator_recovery(true).await;
}

async fn completed_coordinator_recovery(failed_end: bool) {
    use beale_contracts::swarms::*;
    use beale_storage::swarm_state::EnvironmentChange;
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let provider = Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(move || {
            count.fetch_add(1, Ordering::SeqCst);
            async {
                let chunk = json!({"choices":[{"delta":{"content":"Synthetic final answer"}}]});
                (
                    [("content-type", "text/event-stream")],
                    format!("data: {chunk}\n\ndata: [DONE]\n\n"),
                )
            }
        }),
    );
    let (root, state, mut request, server) = regression_session(provider).await;
    hierarchy::run(state.clone(), request.clone(), None, None)
        .await
        .unwrap();
    let scan: String = state
        .db
        .with_connection(|db| Ok(db.query_row("SELECT id FROM swarm_scans", [], |r| r.get(0))?))
        .unwrap();
    request.session_id = scan.clone();
    let store = SwarmStore::open(state.db.clone()).unwrap();
    assert!(store.load(&scan, true).unwrap().ended_at_ms.is_some());
    let before = state.db.session(&scan).unwrap().unwrap().turns;
    let prior_calls = calls.load(Ordering::SeqCst);
    request.prompt = "  Finish the synthetic follow-up.\n".into();
    if failed_end {
        state.db.with_connection(|db| {
            db.execute_batch("CREATE TRIGGER reject_completed_scan_end BEFORE UPDATE OF document ON swarm_scans WHEN json_extract(NEW.document,'$.ended_at_ms') IS NOT NULL BEGIN SELECT RAISE(ABORT,'synthetic finalization failure'); END;")?;
            Ok(())
        }).unwrap();
        // Exercise the real supervisor: transcript/checkpoint succeed, but the
        // completion transaction cannot clear recovery until scan end commits.
        assert!(matches!(
            hierarchy::run(state.clone(), request.clone(), None, None).await,
            Err(engine::TurnError::Internal)
        ));
    } else {
        store.reopen_scan(&scan, true).unwrap();
        let saved = store.load(&scan, true).unwrap();
        let leader = saved
            .agents
            .iter()
            .find(|a| a.summary.role == Role::Leader)
            .unwrap();
        store
            .local_message(
                &scan,
                "final-follow-up",
                &leader.summary.id,
                &request.prompt,
            )
            .unwrap();
        let key = Uuid::new_v4();
        let owner = distributed::acquire_local_execution_lock(&state, key)
            .unwrap()
            .unwrap();
        let turn = store
            .local_claim_owned(&scan, &leader.summary.id, 1, 64, key)
            .unwrap()
            .unwrap();
        let lock = Arc::new(Mutex::new(()));
        engine::run_one_turn(
            &state,
            SubmitSessionTurnRequest {
                prompt: turn.prompt.clone(),
                swarm_id: String::new(),
                ..request.clone()
            },
            None,
            false,
            &lock,
            None,
            Some(&turn),
        )
        .await
        .unwrap();
        let context = json!({"session_id":scan,"saved":true});
        research_files::checkpoint(&state, &turn, &context).unwrap();
        store
            .local_finish(&turn, context, "Synthetic final answer", false)
            .unwrap();
        // Model host loss at the durable boundary after the successful checkpoint
        // and before coordinator completion. Maintenance must finish it itself.
        drop(owner);
    }
    assert_eq!(calls.load(Ordering::SeqCst), prior_calls + 1);
    let pending = store.coordinator_recovery_page(Some(&scan), "").unwrap();
    assert_eq!(pending.len(), 1);
    let (turn, key) = store.coordinator_owner(&pending[0]).unwrap();
    let completed = store.load(&scan, true).unwrap();
    assert!(completed.ready_to_end().unwrap());
    assert!(completed.ended_at_ms.is_none());
    assert_eq!(
        completed.agent(&turn.agent_id).unwrap().summary.state,
        AgentState::Completed
    );
    let transcript = state.db.session(&scan).unwrap().unwrap().turns;
    assert_eq!(transcript.len(), before.len() + 1);
    assert_eq!(&transcript[..before.len()], before.as_slice());
    assert_eq!(transcript.last().unwrap().prompt, request.prompt);
    assert_eq!(transcript.last().unwrap().answer, "Synthetic final answer");
    for (id, keep) in [("retention-eligible", false), ("retention-kept", true)] {
        store
            .put_environment(
                &scan,
                EnvironmentState {
                    id: id.into(),
                    agent_id: turn.agent_id.clone(),
                    generation: 1,
                    retention_deadline_ms: None,
                    keep,
                    active_references: 0,
                    pending_publications: vec![],
                    termination_confirmed: true,
                    deletion: DeletionState::Retained,
                },
            )
            .unwrap();
    }
    let waiting = store.load(&scan, true).unwrap();
    // A fresh runtime uses normal maintenance and the persisted ownership record.
    let fresh = build_state("example-token".into(), state.workspaces.clone()).unwrap();
    let runtime = SessionRuntime { state: fresh };
    let maintenance = runtime.start_automation_scheduler();
    if failed_end {
        tokio::time::sleep(Duration::from_secs(6)).await;
        assert_eq!(store.load(&scan, true).unwrap(), waiting);
        assert_eq!(
            store.coordinator_recovery_page(Some(&scan), "").unwrap(),
            pending
        );
        assert_eq!(state.db.session(&scan).unwrap().unwrap().turns, transcript);
        state
            .db
            .with_connection(|db| {
                db.execute_batch("DROP TRIGGER reject_completed_scan_end;")?;
                Ok(())
            })
            .unwrap();
    }
    tokio::time::timeout(Duration::from_secs(20), async {
        while !store
            .coordinator_recovery_page(Some(&scan), "")
            .unwrap()
            .is_empty()
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    let finalized = store.load(&scan, true).unwrap();
    let end = finalized
        .ended_at_ms
        .expect("successful completion must start retention");
    let deadline =
        end + i64::try_from(finalized.config.retention.ended_scan_seconds).unwrap() * 1000;
    assert_eq!(finalized.agents, waiting.agents);
    assert_eq!(finalized.attempts, waiting.attempts);
    for env in &finalized.environments {
        assert_eq!(env.retention_deadline_ms, Some(deadline));
        assert_eq!(env.generation, 2);
    }
    tokio::time::sleep(Duration::from_secs(6)).await;
    maintenance.abort();
    let _ = maintenance.await;
    store
        .complete_coordinator(&turn.attempt_id, key, end + 1)
        .unwrap();
    store
        .interrupt_coordinator(&turn.attempt_id, key, end + 2)
        .unwrap();
    let reopened_db = HostDatabase::open(&root.join("host")).unwrap();
    let reopened_store = SwarmStore::open(reopened_db.clone()).unwrap();
    assert_eq!(reopened_store.load(&scan, true).unwrap(), finalized);
    assert_eq!(
        reopened_db.session(&scan).unwrap().unwrap().turns,
        transcript
    );
    assert_eq!(calls.load(Ordering::SeqCst), prior_calls + 1);
    for env in &finalized.environments {
        assert!(
            store
                .change_environment(
                    &scan,
                    &env.id,
                    env.generation,
                    deadline - 1,
                    EnvironmentChange::ClaimCleanup
                )
                .is_err()
        );
        let result = store.change_environment(
            &scan,
            &env.id,
            env.generation,
            deadline,
            EnvironmentChange::ClaimCleanup,
        );
        if env.keep {
            assert!(result.is_err());
        } else {
            let claimed = result.unwrap();
            assert_eq!(
                store
                    .change_environment(
                        &scan,
                        &env.id,
                        claimed.generation,
                        deadline,
                        EnvironmentChange::ConfirmDeleted
                    )
                    .unwrap()
                    .deletion,
                DeletionState::Deleted
            );
        }
    }
    server.abort();
    drop(runtime);
    drop(reopened_store);
    drop(reopened_db);
    drop(store);
    drop(state);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn client_disconnect_during_managed_research_preserves_completion_on_reconnect() {
    let calls = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let (count, started, gate) = (calls.clone(), entered.clone(), release.clone());
    let provider = Router::new().route("/v1/chat/completions", axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
        let (count, started, gate) = (count.clone(), started.clone(), gate.clone());
        async move {
            let system = body["messages"][0]["content"].as_str().unwrap();
            let delta = if system.contains("host-side reviewer") {
                json!({"content":json!({"decision":"approved","reason":"Managerial delegation"}).to_string()})
            } else if system.contains("Role: Deputy") {
                assert_eq!(count.fetch_add(1, Ordering::SeqCst), 0, "worker must execute only once");
                started.notify_one();
                gate.notified().await;
                json!({"content":"Deputy completed synthetic research after client disconnect"})
            } else if !body["messages"].as_array().unwrap().iter().any(|m| m["role"] == "tool")
                && !system.contains("Role: Worker")
                && count.load(Ordering::SeqCst) == 0 {
                json!({"tool_calls":[{"index":0,"id":"spawn-example","type":"function","function":{"name":"swarm_spawn_subagent","arguments":json!({"profile":{"id":"research_deputy","revision":1},"task":{"question":"Inspect synthetic fixture","completion_criteria":["Return evidence"],"evidence_refs":[]},"independent":false}).to_string()}}]})
            } else {
                json!({"content":"Leader retained the synthetic result"})
            };
            let chunk = json!({"choices":[{"delta":delta}]});
            ([("content-type","text/event-stream")],format!("data: {chunk}\n\ndata: [DONE]\n\n"))
        }
    }));
    let (root, state, request, provider_server) = regression_session(provider).await;
    let route = Router::new()
        .route(SESSION_TURN_STREAM_PATH, axum::routing::post(stream_turn))
        .route(SESSION_DETAIL_PATH, axum::routing::get(details))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let host = tokio::spawn(async move {
        axum::serve(listener, route).await.unwrap();
    });
    let turn_id = Uuid::new_v4();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(45))
        .build()
        .unwrap();
    let response = client
        .post(format!("{url}{SESSION_TURN_STREAM_PATH}"))
        .bearer_auth("example-token")
        .header("content-type", beale_contracts::PROTOBUF_CONTENT_TYPE)
        .header(SESSION_TURN_ID_HEADER, turn_id.to_string())
        .body(request.encode_to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    tokio::time::timeout(Duration::from_secs(30), entered.notified())
        .await
        .unwrap();
    let scan: String = state
        .db
        .with_connection(|db| Ok(db.query_row("SELECT id FROM swarm_scans", [], |r| r.get(0))?))
        .unwrap();
    let store = SwarmStore::open(state.db.clone()).unwrap();
    assert!(
        store
            .load(&scan, true)
            .unwrap()
            .agents
            .iter()
            .any(|a| a.summary.role == beale_contracts::swarms::Role::Deputy
                && a.summary.state == beale_contracts::swarms::AgentState::Running)
    );
    drop(response);
    drop(client);
    // Closing the HTTP stream is not an explicit stop request.
    assert!(!*state.turn_stops.lock().await[&turn_id].borrow());
    assert!(store.load(&scan, true).unwrap().ended_at_ms.is_none());
    assert!(
        beale_storage::fleet_allocation::AllocationStore::open(state.db.clone())
            .unwrap()
            .environments()
            .unwrap()
            .iter()
            .all(|e| e.deadline_ms.is_none())
    );
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(30), async {
        while state.turn_stops.lock().await.contains_key(&turn_id) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let path = SESSION_DETAIL_PATH.replace("{id}", &scan);
    let bytes = reqwest::Client::new()
        .get(format!("{url}{path}"))
        .bearer_auth("example-token")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let detail = SessionDetail::decode(bytes).unwrap();
    assert_eq!(detail.swarm_agents.len(), 2);
    assert!(detail.swarm_agents.iter().all(|a| a.status == "completed"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let ended = store.load(&scan, true).unwrap().ended_at_ms.unwrap();
    assert!(
        beale_storage::fleet_allocation::AllocationStore::open(state.db.clone())
            .unwrap()
            .environments()
            .unwrap()
            .iter()
            .all(|e| e.deadline_ms == Some(ended + 604800000))
    );
    let deputy = detail
        .swarm_agents
        .iter()
        .find(|a| a.agent_kind == "deputy")
        .unwrap();
    // Swarm children have centrally persisted sessions keyed by stable agent ID.
    let child_path = SESSION_DETAIL_PATH.replace("{id}", &deputy.id);
    let bytes = reqwest::Client::new()
        .get(format!("{url}{child_path}"))
        .bearer_auth("example-token")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let child = SessionDetail::decode(bytes).unwrap();
    assert!(child.messages.iter().any(|item| {
        item.text
            .contains("Deputy completed synthetic research after client disconnect")
    }));
    assert!(
        store
            .load(&scan, true)
            .unwrap()
            .attempts
            .iter()
            .all(|a| a.state == beale_contracts::swarms::ExecutionState::Stopped)
    );
    host.abort();
    provider_server.abort();
    let _ = host.await;
    let _ = provider_server.await;
    drop(store);
    drop(state);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn recovery_notices_survive_reopen_are_bounded_and_never_expose_event_payloads() {
    let provider = Router::new().route("/v1/chat/completions", axum::routing::post(|| async {
        ([("content-type", "text/event-stream")], "data: {\"choices\":[{\"delta\":{\"content\":\"Synthetic completed result\"}}]}\n\ndata: [DONE]\n\n")
    }));
    let (root, state, request, server) = regression_session(provider).await;
    let original = engine::run_turn(state.clone(), request, None, None)
        .await
        .unwrap();
    let scan = original.summary.as_ref().unwrap().id.clone();
    let store = SwarmStore::open(state.db.clone()).unwrap();
    let agent = store.load(&scan, true).unwrap().agents[0]
        .summary
        .id
        .clone();
    let kinds = [
        "local-recovery",
        "coordinator-interrupted",
        "worker-termination-unconfirmed",
        "inference-interrupted",
        "plugin-outcome-unknown",
        "plugin-resources-unavailable",
        "peer-recovery-pending",
        "worker-event",
        "unrecognized-example",
    ];
    for (index, kind) in kinds.iter().cycle().take(90).enumerate() {
        let event = beale_contracts::swarms::LifecycleEvent {
            id: format!("event-example-{index}"),
            agent_id: agent.clone(),
            sequence: index as u64 + 1,
            kind: (*kind).into(),
            payload: json!({"warning":"synthetic-secret-must-not-be-projected", "event":{"kind":"warning", "message":"synthetic-secret-must-not-be-projected"}}),
        };
        store.append_event(&scan, event.clone()).unwrap();
        store.append_event(&scan, event).unwrap();
    }
    drop(store);
    drop(state);
    let reopened = build_state(
        "example-token".into(),
        WorkspaceStore::at(root.join("host")).unwrap(),
    )
    .unwrap();
    // Existing scans retain their recovery history even when new Swarms are disabled.
    reopened.db.set_feature_enabled("swarms", false).unwrap();
    let route = Router::new()
        .route(SESSION_DETAIL_PATH, axum::routing::get(details))
        .with_state(reopened.clone());
    let mut previous = None;
    for _ in 0..2 {
        let response = route
            .clone()
            .oneshot(
                Request::builder()
                    .uri(SESSION_DETAIL_PATH.replace("{id}", &scan))
                    .header("authorization", "Bearer example-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let detail =
            SessionDetail::decode(to_bytes(response.into_body(), 4_500_000).await.unwrap())
                .unwrap();
        let notices: Vec<_> = detail
            .messages
            .iter()
            .flat_map(|m| &m.commentary)
            .filter(|c| c.text.starts_with("Recovery history"))
            .collect();
        assert_eq!(notices.len(), 8);
        assert!(
            notices
                .iter()
                .all(|c| c.text.contains("10 recorded events") && c.text.len() < 256)
        );
        assert!(!format!("{detail:?}").contains("synthetic-secret"));
        assert_eq!(
            detail.messages.last().unwrap().text,
            original.messages.last().unwrap().text
        );
        if let Some(previous) = previous {
            assert_eq!(detail, previous);
        }
        previous = Some(detail);
    }
    assert!(
        !reopened
            .db
            .session(&scan)
            .unwrap()
            .unwrap()
            .turns
            .iter()
            .any(|t| t.answer.contains("Recovery history"))
    );
    server.abort();
    let _ = server.await;
    drop(route);
    drop(reopened);
    let _ = std::fs::remove_dir_all(root);
}
