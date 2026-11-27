use super::*;
use beale_storage::swarm_state::SwarmStore;
use std::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};

#[tokio::test]
async fn background_maintenance_recovers_checkpoint_after_request_finishes() {
    recovery_after_request(false).await;
}

#[tokio::test]
async fn background_maintenance_recovers_publication_after_request_finishes() {
    recovery_after_request(true).await;
}

async fn recovery_after_request(publication: bool) {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let spawned = Arc::new(AtomicBool::new(false));
    let provider = Router::new().route("/v1/chat/completions", axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
        let count = count.clone();
        let spawned = spawned.clone();
        async move {
            let system = body["messages"][0]["content"].as_str().unwrap();
            let delta = if system.contains("host-side reviewer") {
                json!({"content":json!({"decision":"approved","reason":"Managerial delegation"}).to_string()})
            } else if system.contains("Role: Deputy") {
                assert_eq!(count.fetch_add(1, Ordering::SeqCst), 0, "recovery must not repeat research");
                if publication {
                    json!({"tool_calls":[{"index":0,"id":"read-example","type":"function","function":{"name":"read","arguments":json!({"path":"large.txt"}).to_string()}}]})
                } else {
                    json!({"content":"Synthetic completed deputy result"})
                }
            } else if !spawned.swap(true, Ordering::SeqCst) {
                json!({"tool_calls":[{"index":0,"id":"spawn-example","type":"function","function":{"name":"swarm_spawn_subagent","arguments":json!({"profile":{"id":"research_deputy","revision":1},"task":{"question":"Inspect synthetic fixture","completion_criteria":["Return evidence"],"evidence_refs":[]},"independent":false}).to_string()}}]})
            } else {
                json!({"content":"Synthetic coordinator awaiting retained result"})
            };
            let chunk = json!({"choices":[{"delta":delta}]});
            ([("content-type","text/event-stream")],format!("data: {chunk}\n\ndata: [DONE]\n\n"))
        }
    }));
    let (root, setup, request, provider_server) = regression_session(provider).await;
    let original = "Synthetic retained output ".repeat(20_000);
    std::fs::write(root.join("workspace/large.txt"), &original).unwrap();
    // Use the same public composition as Core. Router and maintenance must
    // share the service, not independently construct their execution state.
    let runtime = SessionRuntime::new(
        "example-token".into(),
        setup.workspaces.clone(),
        setup.plugins.clone(),
    )
    .unwrap();
    drop(setup);
    let state = &runtime.state;
    let store = SwarmStore::open(state.db.clone()).unwrap();
    let files = research_files::store(state).unwrap();
    state.db.with_connection(|db| {
        db.execute_batch(if publication {
            "CREATE TRIGGER fail_background_recovery BEFORE UPDATE OF accepted ON swarm_file_transfers WHEN NEW.accepted=1 BEGIN SELECT RAISE(ABORT,'synthetic publication full'); END;"
        } else {
            "CREATE TRIGGER fail_background_recovery BEFORE INSERT ON swarm_file_checkpoints BEGIN SELECT RAISE(ABORT,'synthetic checkpoint full'); END;"
        })?;
        Ok(())
    }).unwrap();
    let response = tokio::time::timeout(
        Duration::from_secs(30),
        runtime.router().oneshot(regression_request(
            SESSION_TURNS_PATH,
            request.encode_to_vec(),
        )),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let detail =
        SessionDetail::decode(to_bytes(response.into_body(), 4_500_000).await.unwrap()).unwrap();
    let scan = detail.summary.unwrap().id;
    let records = store.local_worker_records(Some(&scan), "").unwrap();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    let agent = &record.turn.agent_id;
    assert_eq!(state.worker_recovery.lock().unwrap().len(), 1);
    assert!(
        distributed::acquire_local_execution_lock(state, record.owner_key)
            .unwrap()
            .is_none()
    );
    let private = files.prepare_agent(&scan, agent).unwrap();
    std::fs::write(private.join("large.txt"), "Synthetic later edit").unwrap();

    // No request or model turn remains. Start ordinary host maintenance while
    // the fault persists, then restore storage without calling recovery helpers.
    let maintenance = runtime.start_automation_scheduler();
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(
        store.local_worker_records(Some(&scan), "").unwrap().len(),
        1
    );
    assert!(
        distributed::acquire_local_execution_lock(state, record.owner_key)
            .unwrap()
            .is_none()
    );
    if !publication {
        assert!(!store.worker_reconciled(&record.turn).unwrap());
    }
    state
        .db
        .with_connection(|db| {
            db.execute_batch("DROP TRIGGER fail_background_recovery;")?;
            Ok(())
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if store
                .local_worker_records(Some(&scan), "")
                .unwrap()
                .is_empty()
                && state.worker_recovery.lock().unwrap().is_empty()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        distributed::acquire_local_execution_lock(state, record.owner_key)
            .unwrap()
            .is_some()
    );
    assert!(store.worker_reconciled(&record.turn).unwrap());
    assert!(
        files
            .environment(&scan, agent)
            .unwrap()
            .checkpoint
            .is_some()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let saved = store.load(&scan, true).unwrap();
    if publication {
        let events: Vec<_> = saved
            .events
            .iter()
            .filter(|e| e.kind == "worker-recovery-artifact")
            .collect();
        assert_eq!(events.len(), 1);
        let artifact = files
            .artifact(
                &saved.config.workspace_id,
                events[0].payload["artifact"].as_str().unwrap(),
            )
            .unwrap();
        assert_eq!(artifact.attempt, record.turn.attempt_id);
        assert_eq!(
            artifact.sha256,
            beale_storage::swarm_files::hash(format!("1\t{original}\n").as_bytes())
        );
    } else {
        let session = state.db.session(agent).unwrap().unwrap();
        assert_eq!(session.turns.len(), 1);
        assert_eq!(session.turns[0].answer, "Synthetic completed deputy result");
        assert_eq!(
            saved.agent(agent).unwrap().summary.state,
            beale_contracts::swarms::AgentState::Completed
        );
    }
    // A later maintenance tick must not duplicate receipts or parent notices.
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(store.load(&scan, true).unwrap(), saved);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    maintenance.abort();
    let _ = maintenance.await;
    provider_server.abort();
    let _ = provider_server.await;
    drop(files);
    drop(store);
    drop(runtime);
    let _ = std::fs::remove_dir_all(root);
}
