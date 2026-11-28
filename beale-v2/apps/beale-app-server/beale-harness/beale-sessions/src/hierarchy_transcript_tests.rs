//! Exercise the model-input / user-transcript boundary through real persistence.
use super::*;
use beale_contracts::{swarm_local::*, swarm_workflows::*, swarms::*};
use beale_storage::swarm_state::{
    EnvironmentChange, LocalModel, LocalSettings, LocalTurn, SwarmStore,
};

#[tokio::test]
async fn retired_custody_preserves_exact_operator_transcript_after_reopen() {
    for ending in [Ending::Complete, Ending::BeforeSave] {
        transcript(false, ending, false).await;
    }
}

#[tokio::test]
async fn recovery_delivery_preserves_visible_operator_transcript_after_reopen() {
    for ending in [Ending::Complete, Ending::BeforeSave] {
        transcript(true, ending, false).await;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ending {
    Complete,
    BeforeSave,
    AfterSave,
    RejectedSave,
    SupervisorCheckpoint,
}

#[tokio::test]
async fn existing_session_custody_interruption_and_duplicate_recovery_preserve_transcript() {
    for ending in [Ending::BeforeSave, Ending::AfterSave, Ending::RejectedSave] {
        transcript(false, ending, true).await;
    }
}

#[tokio::test]
async fn existing_session_redelivery_interruption_and_duplicate_recovery_preserve_transcript() {
    for ending in [Ending::BeforeSave, Ending::AfterSave, Ending::RejectedSave] {
        transcript(true, ending, true).await;
    }
}

fn visible(detail: &SessionDetail) -> Vec<String> {
    detail
        .messages
        .iter()
        .filter(|m| m.role == SessionRole::User as i32)
        .map(|m| m.text.clone())
        .collect()
}

fn recover_transcript(
    state: &SessionState,
    root: &std::path::Path,
    turn: &LocalTurn,
    key: Uuid,
    expected: &[String],
    committed: bool,
) {
    let db = HostDatabase::open(&root.join("host")).unwrap();
    let store = SwarmStore::open(db.clone()).unwrap();
    let before = db.session(&turn.scan_id).unwrap();
    let before_scan = store.load(&turn.scan_id, true).unwrap();
    // Fail the final recovery write: transcript insertion, its attempt binding,
    // hierarchy stop and fences must all roll back together.
    db.with_connection(|db| { db.execute_batch("CREATE TRIGGER reject_recovery BEFORE UPDATE OF complete ON swarm_local_coordinators BEGIN SELECT RAISE(ABORT,'synthetic recovery storage failure'); END;")?; Ok(()) }).unwrap();
    assert!(
        store
            .interrupt_coordinator(&turn.attempt_id, key, 5)
            .is_err()
    );
    assert_eq!(
        db.session(&turn.scan_id).unwrap().map(|s| s.turns),
        before.map(|s| s.turns)
    );
    assert_eq!(store.load(&turn.scan_id, true).unwrap(), before_scan);
    db.with_connection(|db| {
        db.execute_batch("DROP TRIGGER reject_recovery;")?;
        Ok(())
    })
    .unwrap();
    // Exercise the normal host recovery entry point with its owner lock.
    for _ in 0..2 {
        distributed::recover_coordinators(state, Some(&turn.scan_id), "").unwrap();
    }
    store
        .interrupt_coordinator(&turn.attempt_id, key, 6)
        .unwrap();
    let reopened = HostDatabase::open(&root.join("host")).unwrap();
    let (session, turns) =
        load_persisted(&reopened, Uuid::parse_str(&turn.scan_id).unwrap()).unwrap();
    assert_eq!(
        visible(&detail(&reopened, &session, &turns).unwrap()),
        expected
    );
    let latest = turns.last().unwrap();
    if committed {
        assert_eq!(latest.answer, "Synthetic coordination complete");
    } else {
        assert!(latest.answer.starts_with("Host recovery:"));
    }
    assert_eq!(
        reopened.session_model_prompts(&turn.scan_id).unwrap()[&latest.number],
        turn.prompt
    );
    assert_eq!(turns.len(), expected.len());
    assert!(
        store
            .coordinator_recovery_page(Some(&turn.scan_id), "")
            .unwrap()
            .is_empty()
    );
    let saved = store.load(&turn.scan_id, true).unwrap();
    assert_eq!(
        saved
            .events
            .iter()
            .filter(|e| e.kind == "coordinator-interrupted"
                && e.payload["attempt_id"] == turn.attempt_id)
            .count(),
        1
    );
    assert_eq!(
        saved.agent(&turn.agent_id).unwrap().summary.state,
        AgentState::Blocked
    );
}

fn finish(store: &SwarmStore, turn: &LocalTurn) {
    store
        .local_finish(turn, json!({}), "Synthetic coordination", false)
        .unwrap();
}

async fn transcript(recover: bool, ending: Ending, existing: bool) {
    let seen = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let captured = seen.clone();
    let app = Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
            let captured = captured.clone();
            async move {
                captured.lock().unwrap().push(body.clone());
                let answer = if body.to_string().contains("host-side reviewer") {
                    json!({"decision":"approved","reason":"Synthetic coordination"}).to_string()
                } else {
                    "Synthetic coordination complete".into()
                };
                let chunk = json!({"choices":[{"delta":{"content":answer}}]});
                (
                    [("content-type", "text/event-stream")],
                    format!("data: {chunk}\n\ndata: [DONE]\n\n"),
                )
            }
        }),
    );
    let (root, state, request, server) = regression_session(app).await;
    let store = SwarmStore::open(state.db.clone()).unwrap();
    let definition = beale_swarms::swarm_definition("basic").unwrap();
    let scan = Uuid::new_v4().to_string();
    let saved = store
        .create_scan(
            ScanConfig {
                version: VERSION,
                scan_id: scan.clone(),
                workspace_id: request.workspace_id.clone(),
                mode: ScanMode::Swarm {
                    snapshot: Box::new(definition.snapshot.clone()),
                },
                authorized_scope: "Synthetic example.test only".into(),
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
    let leader_id = saved.agents[0].summary.id.clone();
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
            "Coordinate synthetic setup",
        )
        .unwrap();
    let initial_key = Uuid::new_v4();
    let leader = if existing {
        store.local_claim_owned(&scan, &leader_id, 1, 8, initial_key)
    } else {
        store.local_claim(&scan, &leader_id, 1, 8)
    }
    .unwrap()
    .unwrap();
    let lock = Arc::new(Mutex::new(()));
    if existing {
        engine::run_one_turn(
            &state,
            SubmitSessionTurnRequest {
                session_id: scan.clone(),
                swarm_id: String::new(),
                prompt: leader.prompt.clone(),
                ..request.clone()
            },
            None,
            false,
            &lock,
            None,
            Some(&leader),
        )
        .await
        .unwrap();
    }
    let task = Task {
        question: "Review synthetic evidence".into(),
        completion_criteria: vec!["Return evidence".into()],
        evidence_refs: vec![],
    };
    let spawn = |operation: &str| {
        store
            .local_dispatch(
                &leader,
                operation,
                SwarmCommand::Spawn(Spawn {
                    handoff_id: None,
                    profile: ProfileRef {
                        id: "research_deputy".into(),
                        revision: 1,
                    },
                    task: task.clone(),
                    independent: false,
                }),
                Admission::Ready,
            )
            .unwrap()["child_id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let source = spawn("source");
    let source = store.local_claim(&scan, &source, 2, 8).unwrap().unwrap();
    let handoff = HandoffDraft {
        to_profile: "research_deputy".into(),
        route_id: None,
        task: task.clone(),
        scope: vec!["Synthetic example.test".into()],
        record_revisions: vec![],
        artifact_refs: vec![],
        observations: vec!["Synthetic observation".into()],
        unresolved_questions: vec![],
        payload: HandoffPayload::Task {},
    };
    let submitted = store
        .local_dispatch(
            &source,
            "submit",
            SwarmCommand::MessageParent(Box::new(MessageParent {
                message: "Synthetic handoff".into(),
                handoff: Some(handoff),
            })),
            Admission::Ready,
        )
        .unwrap();
    let handoff_id = submitted["handoff_id"].as_str().unwrap().to_owned();
    if !recover {
        let destination = spawn("destination");
        store
            .local_dispatch(
                &leader,
                "forward",
                SwarmCommand::Handoff(Box::new(HandoffAction::Forward {
                    id: handoff_id.clone(),
                    to: destination,
                })),
                Admission::Ready,
            )
            .unwrap();
    }
    finish(&store, &source);
    finish(&store, &leader);
    if existing {
        store
            .complete_coordinator(&leader.attempt_id, initial_key, 3)
            .unwrap();
    }
    while let Some(turn) = store.local_claim(&scan, &leader_id, 3, 8).unwrap() {
        finish(&store, &turn);
    }
    // Two queued operator messages must remain two exact, visible user turns.
    // Recovery also adds a model-only delivery before the first operator text.
    let texts = [
        "  Coordinate the synthetic retry — keep this text.\n",
        "Continue synthetic follow-up.",
    ];
    if ending == Ending::SupervisorCheckpoint {
        supervisor_checkpoint(
            &state,
            &root,
            SubmitSessionTurnRequest {
                session_id: scan.clone(),
                swarm_id: String::new(),
                prompt: texts[0].into(),
                ..request.clone()
            },
            recover,
            &seen,
        )
        .await;
        server.abort();
        drop(store);
        drop(state);
        let _ = std::fs::remove_dir_all(root);
        return;
    }
    for (index, text) in texts.iter().enumerate() {
        store
            .local_message(&scan, &format!("operator-{index}"), &leader_id, text)
            .unwrap();
    }
    for (index, text) in texts.iter().enumerate() {
        let key = Uuid::new_v4();
        let claimed = store
            .local_claim_owned(&scan, &leader_id, 4, 8, key)
            .unwrap()
            .unwrap();
        // Read the durable owner record through a fresh database connection before
        // execution, rather than relying on an in-memory copy of operator text.
        let reopened = HostDatabase::open(&root.join("host")).unwrap();
        let recovered = SwarmStore::open(reopened.clone()).unwrap();
        let (turn, saved_key) = recovered.coordinator_owner(&claimed.attempt_id).unwrap();
        assert_eq!(saved_key, key);
        assert_eq!(turn.operator_text.as_deref(), Some(*text));
        if index == 0 {
            assert!(turn.prompt.contains(if recover {
                "Retry executor-blocked handoff"
            } else {
                "handoff-custody"
            }));
        }
        let mut expected: Vec<String> = if existing {
            vec!["Coordinate synthetic setup".into()]
        } else {
            vec![]
        };
        expected.extend(texts[..=index].iter().map(|s| (*s).to_owned()));
        if ending == Ending::BeforeSave {
            recover_transcript(&state, &root, &turn, key, &expected, false);
            break;
        }
        let calls_before = seen.lock().unwrap().len();
        if ending == Ending::RejectedSave {
            state.db.with_connection(|db| { db.execute_batch("CREATE TRIGGER reject_commit BEFORE UPDATE OF committed_turn ON swarm_local_coordinators BEGIN SELECT RAISE(ABORT,'synthetic commit storage failure'); END;")?; Ok(()) }).unwrap();
        }
        let response = engine::run_one_turn(
            &state,
            SubmitSessionTurnRequest {
                session_id: scan.clone(),
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
        .await;
        if ending == Ending::RejectedSave {
            assert!(matches!(response, Err(engine::TurnError::Checkpoint)));
            assert_eq!(
                seen.lock().unwrap().len(),
                calls_before + 1,
                "persistence retries repeated inference"
            );
            let saved = state.db.session(&scan).unwrap().unwrap();
            assert_eq!(
                saved.turns.len(),
                1,
                "failed marker write committed a transcript"
            );
            let marker: Option<i64> = state
                .db
                .with_connection(|db| {
                    Ok(db.query_row(
                        "SELECT committed_turn FROM swarm_local_coordinators WHERE attempt_id=?1",
                        [&turn.attempt_id],
                        |r| r.get(0),
                    )?)
                })
                .unwrap();
            assert!(marker.is_none());
            state
                .db
                .with_connection(|db| {
                    db.execute_batch("DROP TRIGGER reject_commit;")?;
                    Ok(())
                })
                .unwrap();
            recover_transcript(&state, &root, &turn, key, &expected, false);
            break;
        }
        let response = response.unwrap();
        if ending == Ending::AfterSave {
            let before = state.db.session(&scan).unwrap().unwrap();
            recover_transcript(&state, &root, &turn, key, &expected, true);
            assert_eq!(
                state.db.session(&scan).unwrap().unwrap().turns,
                before.turns
            );
            break;
        }
        let user_texts = |value: &SessionDetail| {
            value
                .messages
                .iter()
                .filter(|m| m.role == SessionRole::User as i32)
                .map(|m| m.text.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(user_texts(&response), texts[..=index]);
        // The provider still receives the full custody/retry input.
        assert!(seen.lock().unwrap().iter().any(|body| {
            body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["content"].as_str() == Some(turn.prompt.as_str()))
        }));
        finish(&store, &turn);
        store
            .complete_coordinator(&turn.attempt_id, key, 5)
            .unwrap();
        let db = HostDatabase::open(&root.join("host")).unwrap();
        let (session, turns) = load_persisted(&db, Uuid::parse_str(&scan).unwrap()).unwrap();
        assert_eq!(turns.last().unwrap().prompt, *text);
        assert!(!turns.last().unwrap().internal_prompt);
        assert_eq!(
            db.session_model_prompts(&scan).unwrap()[&((index + 1) as i64)],
            turn.prompt
        );
        if index == 1 {
            let bodies = seen.lock().unwrap();
            let latest = bodies
                .iter()
                .rev()
                .find(|body| !body.to_string().contains("host-side reviewer"))
                .unwrap();
            let first_model_input = db.session_model_prompts(&scan).unwrap()[&1].clone();
            assert!(
                latest["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|m| m["content"].as_str() == Some(first_model_input.as_str())),
                "later inference lost accepted handoff input"
            );
        }
        assert_eq!(
            user_texts(&detail(&db, &session, &turns).unwrap()),
            texts[..=index]
        );
    }
    server.abort();
    drop(store);
    drop(state);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn normal_supervisor_custody_checkpoint_exhaustion_preserves_transcript() {
    transcript(false, Ending::SupervisorCheckpoint, true).await;
}

#[tokio::test]
async fn normal_supervisor_redelivery_checkpoint_exhaustion_preserves_transcript() {
    transcript(true, Ending::SupervisorCheckpoint, true).await;
}

async fn supervisor_checkpoint(
    state: &SessionState,
    root: &std::path::Path,
    request: SubmitSessionTurnRequest,
    recover: bool,
    seen: &std::sync::Mutex<Vec<Value>>,
) {
    use std::time::Duration;
    let store = SwarmStore::open(state.db.clone()).unwrap();
    let scan = request.session_id.clone();
    let text = request.prompt.clone();
    let before = state.db.session(&scan).unwrap().unwrap().turns;
    let calls = seen.lock().unwrap().len();
    let agent = store
        .load(&scan, true)
        .unwrap()
        .agents
        .iter()
        .find(|a| a.summary.role == Role::Deputy)
        .unwrap()
        .summary
        .id
        .clone();
    for (id, keep, active_references, pending_publications, termination_confirmed) in [
        ("retention-eligible", false, 0, vec![], true),
        ("retention-kept", true, 0, vec![], true),
        ("retention-active", false, 1, vec![], true),
        (
            "retention-publication",
            false,
            0,
            vec!["publication-example".into()],
            true,
        ),
        ("retention-uncertain", false, 0, vec![], false),
    ] {
        store
            .put_environment(
                &scan,
                EnvironmentState {
                    id: id.into(),
                    agent_id: agent.clone(),
                    generation: 1,
                    retention_deadline_ms: None,
                    keep,
                    active_references,
                    pending_publications,
                    termination_confirmed,
                    deletion: DeletionState::Retained,
                },
            )
            .unwrap();
    }
    state.db.with_connection(|db| {
        db.execute_batch("CREATE TRIGGER reject_commit BEFORE UPDATE OF committed_turn ON swarm_local_coordinators BEGIN SELECT RAISE(ABORT,'synthetic commit storage failure'); END;")?;
        Ok(())
    }).unwrap();
    // The scheduler claims the message, runs inference, exhausts persistence
    // retries, stops the leader and tries to complete coordinator recovery.
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        hierarchy::run(state.clone(), request, None, None),
    )
    .await
    .unwrap();
    assert!(matches!(result, Err(engine::TurnError::Internal)));
    assert_eq!(seen.lock().unwrap().len(), calls + 1);
    let pending = store.coordinator_recovery_page(Some(&scan), "").unwrap();
    assert_eq!(pending.len(), 1, "failed completion lost recovery intent");
    let (turn, key) = store.coordinator_owner(&pending[0]).unwrap();
    assert_eq!(turn.operator_text.as_deref(), Some(text.as_str()));
    assert!(turn.prompt.contains(if recover {
        "Retry executor-blocked handoff"
    } else {
        "handoff-custody"
    }));
    assert!(seen.lock().unwrap().iter().any(|body| {
        body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["content"].as_str() == Some(turn.prompt.as_str()))
    }));
    let stopped = store.load(&scan, true).unwrap();
    assert_eq!(
        stopped.agent(&turn.agent_id).unwrap().summary.state,
        AgentState::Stopped
    );
    assert_eq!(
        stopped
            .attempts
            .iter()
            .find(|a| a.id == turn.attempt_id)
            .unwrap()
            .state,
        ExecutionState::Stopped
    );
    assert!(stopped.ended_at_ms.is_none());
    assert!(
        stopped
            .environments
            .iter()
            .all(|e| e.retention_deadline_ms.is_none())
    );
    assert_eq!(state.db.session(&scan).unwrap().unwrap().turns, before);
    // Use ordinary background maintenance, including a failed sweep while the
    // fault persists. No test recovery helper completes the pending attempt.
    let runtime = SessionRuntime {
        state: state.clone(),
    };
    let maintenance = runtime.start_automation_scheduler();
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(
        store.coordinator_recovery_page(Some(&scan), "").unwrap(),
        pending
    );
    assert_eq!(state.db.session(&scan).unwrap().unwrap().turns, before);
    assert_eq!(store.load(&scan, true).unwrap(), stopped);
    state
        .db
        .with_connection(|db| {
            db.execute_batch("DROP TRIGGER reject_commit; CREATE TRIGGER reject_finalization BEFORE UPDATE OF document ON swarm_scans WHEN json_extract(NEW.document, '$.ended_at_ms') IS NOT NULL BEGIN SELECT RAISE(ABORT,'synthetic finalization storage failure'); END;")?;
            Ok(())
        })
        .unwrap();
    // Transcript repair must roll back too if scan-ended persistence fails.
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(
        store.coordinator_recovery_page(Some(&scan), "").unwrap(),
        pending
    );
    assert_eq!(state.db.session(&scan).unwrap().unwrap().turns, before);
    assert_eq!(store.load(&scan, true).unwrap(), stopped);
    state
        .db
        .with_connection(|db| {
            db.execute_batch("DROP TRIGGER reject_finalization;")?;
            Ok(())
        })
        .unwrap();
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
    let ended = finalized
        .ended_at_ms
        .expect("terminal failure must start retention");
    let deadline =
        ended + i64::try_from(finalized.config.retention.ended_scan_seconds).unwrap() * 1000;
    assert_eq!(finalized.agents, stopped.agents);
    assert_eq!(finalized.attempts, stopped.attempts);
    for env in &finalized.environments {
        let previous = stopped
            .environments
            .iter()
            .find(|e| e.id == env.id)
            .unwrap();
        let mut expected = previous.clone();
        expected.generation += 1;
        expected.retention_deadline_ms = Some(deadline);
        assert_eq!(env, &expected, "finalization changed cleanup protections");
    }
    // A subsequent sweep and replay cannot duplicate the transcript or reset
    // the retention deadline/generation after finalization.
    tokio::time::sleep(Duration::from_secs(6)).await;
    maintenance.abort();
    let _ = maintenance.await;
    store
        .complete_coordinator(&turn.attempt_id, key, 6)
        .unwrap();
    store
        .interrupt_coordinator(&turn.attempt_id, key, 7)
        .unwrap();
    let reopened = HostDatabase::open(&root.join("host")).unwrap();
    let (session, turns) = load_persisted(&reopened, Uuid::parse_str(&scan).unwrap()).unwrap();
    assert_eq!(
        visible(&detail(&reopened, &session, &turns).unwrap()),
        vec!["Coordinate synthetic setup".to_owned(), text]
    );
    assert_eq!(turns.len(), before.len() + 1);
    assert_eq!(
        &reopened.session(&scan).unwrap().unwrap().turns[..before.len()],
        before.as_slice()
    );
    let latest = turns.last().unwrap();
    assert!(latest.answer.starts_with("Host recovery:"));
    assert!(latest.answer.contains("bounded local recovery failed"));
    assert!(!latest.internal_prompt);
    assert_eq!(
        reopened.session_model_prompts(&scan).unwrap()[&latest.number],
        turn.prompt
    );
    assert_eq!(store.load(&scan, true).unwrap(), finalized);
    // Each protection blocks cleanup even after the seven-day clock expires.
    // The eligible environment is protected before expiry and reclaimable after.
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
            deadline + 1,
            EnvironmentChange::ClaimCleanup,
        );
        if env.id == "retention-eligible" {
            let claimed = result.unwrap();
            assert_eq!(claimed.deletion, DeletionState::Claimed);
            let deleted = store
                .change_environment(
                    &scan,
                    &env.id,
                    claimed.generation,
                    deadline + 2,
                    EnvironmentChange::ConfirmDeleted,
                )
                .unwrap();
            assert_eq!(deleted.deletion, DeletionState::Deleted);
        } else {
            assert!(result.is_err(), "{} lost its cleanup protection", env.id);
            assert_eq!(
                store
                    .load(&scan, true)
                    .unwrap()
                    .environments
                    .iter()
                    .find(|e| e.id == env.id)
                    .unwrap(),
                env
            );
        }
    }
    assert_eq!(
        seen.lock().unwrap().len(),
        calls + 1,
        "recovery repeated inference"
    );
}
