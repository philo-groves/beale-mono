//! The detached Swarm scheduler runs agents concurrently, lets later
//! submissions attach to a running scan, and never blocks on a subscriber.
//! Synchronization uses barriers and notifications with bounded timeouts.
use super::*;
use beale_contracts::swarms::{AgentState, ExecutionState};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const LIMIT: Duration = Duration::from_secs(60);

fn sse(delta: Value) -> axum::response::Response {
    let chunk = json!({"choices":[{"delta":delta}]});
    (
        [("content-type", "text/event-stream")],
        format!("data: {chunk}\n\ndata: [DONE]\n\n"),
    )
        .into_response()
}

fn role(system: &str) -> &'static str {
    if system.contains("host-side reviewer") {
        "reviewer"
    } else if system.contains("Role: Leader") {
        "leader"
    } else if system.contains("Role: Deputy") {
        "deputy"
    } else {
        "worker"
    }
}

fn approved() -> Value {
    json!({"content":json!({"decision":"approved","reason":"Managerial delegation"}).to_string()})
}

fn spawn(profile: &str, count: usize) -> Value {
    let calls = (0..count)
        .map(|i| json!({"index":i,"id":format!("spawn-{profile}-{i}"),"type":"function","function":{"name":"swarm_spawn_subagent","arguments":json!({"profile":{"id":profile,"revision":1},"task":{"question":"Inspect synthetic fixture","completion_criteria":["Return evidence"],"evidence_refs":[]},"independent":false}).to_string()}}))
        .collect::<Vec<_>>();
    json!({ "tool_calls": calls })
}

fn follow_up(
    request: &SubmitSessionTurnRequest,
    scan: &str,
    prompt: &str,
) -> SubmitSessionTurnRequest {
    SubmitSessionTurnRequest {
        session_id: scan.to_owned(),
        swarm_id: String::new(),
        prompt: prompt.to_owned(),
        ..request.clone()
    }
}

async fn scan_id(state: &SessionState) -> String {
    tokio::time::timeout(LIMIT, async {
        loop {
            let id: Option<String> = state
                .db
                .with_connection(|db| {
                    use beale_storage::rusqlite::OptionalExtension;
                    Ok(db
                        .query_row("SELECT id FROM swarm_scans", [], |r| r.get(0))
                        .optional()?)
                })
                .unwrap();
            if let Some(id) = id {
                return id;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

fn leader_prompts(state: &SessionState, scan: &str) -> Vec<String> {
    state
        .db
        .session(scan)
        .unwrap()
        .map(|s| s.turns.into_iter().map(|t| t.prompt).collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn sibling_workers_run_at_the_same_time() {
    let running = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let both = Arc::new(tokio::sync::Notify::new());
    let counts = Arc::new(std::sync::Mutex::new(HashMap::<&'static str, usize>::new()));
    let app = Router::new().route(
        "/v1/chat/completions",
        axum::routing::post({
            let (running, peak, both, counts) =
                (running.clone(), peak.clone(), both.clone(), counts.clone());
            move |axum::Json(body): axum::Json<Value>| {
                let (running, peak, both, counts) =
                    (running.clone(), peak.clone(), both.clone(), counts.clone());
                async move {
                    let system = body["messages"][0]["content"].as_str().unwrap();
                    let role = role(system);
                    let n = {
                        let mut counts = counts.lock().unwrap();
                        let count = counts.entry(role).or_default();
                        let n = *count;
                        *count += 1;
                        n
                    };
                    sse(match role {
                        "reviewer" => approved(),
                        "leader" if n == 0 => spawn("research_deputy", 1),
                        "deputy" if n == 0 => spawn("investigator", 2),
                        "worker" => {
                            // Hold each worker's first inference until its sibling
                            // is also inside one, or a bounded wait passes.
                            let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                            peak.fetch_max(now, Ordering::SeqCst);
                            if now >= 2 {
                                both.notify_waiters();
                            } else {
                                let _ =
                                    tokio::time::timeout(Duration::from_secs(20), both.notified())
                                        .await;
                            }
                            running.fetch_sub(1, Ordering::SeqCst);
                            json!({"content":"worker synthetic result"})
                        }
                        other => json!({"content":format!("{other} synthetic result")}),
                    })
                }
            }
        }),
    );
    let (root, state, request, server) = regression_session(app).await;
    let detail = tokio::time::timeout(LIMIT, engine::run_turn(state.clone(), request, None, None))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        peak.load(Ordering::SeqCst),
        2,
        "sibling workers did not overlap"
    );
    let saved = beale_storage::swarm_state::SwarmStore::open(state.db.clone())
        .unwrap()
        .load(&detail.summary.unwrap().id, true)
        .unwrap();
    assert_eq!(saved.agents.len(), 4);
    assert!(
        saved
            .agents
            .iter()
            .all(|a| a.summary.state == AgentState::Completed)
    );
    server.abort();
    drop(state);
    let _ = std::fs::remove_dir_all(root);
}

/// The leader answers a follow-up and commits its turn while a subagent's
/// inference is still blocked; both requests receive the same outcome.
#[tokio::test]
async fn follow_up_attaches_and_leader_commits_while_a_subagent_runs() {
    let entered = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let counts = Arc::new(std::sync::Mutex::new(HashMap::<&'static str, usize>::new()));
    let app = Router::new().route(
        "/v1/chat/completions",
        axum::routing::post({
            let (entered, release, counts) = (entered.clone(), release.clone(), counts.clone());
            move |axum::Json(body): axum::Json<Value>| {
                let (entered, release, counts) = (entered.clone(), release.clone(), counts.clone());
                async move {
                    let system = body["messages"][0]["content"].as_str().unwrap();
                    let role = role(system);
                    let n = {
                        let mut counts = counts.lock().unwrap();
                        let count = counts.entry(role).or_default();
                        let n = *count;
                        *count += 1;
                        n
                    };
                    sse(match role {
                        "reviewer" => approved(),
                        "leader" if n == 0 => spawn("research_deputy", 1),
                        "leader" => json!({"content":format!("Leader synthetic answer {n}")}),
                        "deputy" if n == 0 => {
                            entered.add_permits(1);
                            release.acquire().await.unwrap().forget();
                            json!({"content":"Deputy synthetic result"})
                        }
                        other => json!({"content":format!("{other} synthetic result")}),
                    })
                }
            }
        }),
    );
    let (root, state, request, server) = regression_session(app).await;
    let first = tokio::spawn(engine::run_turn(state.clone(), request.clone(), None, None));
    tokio::time::timeout(LIMIT, entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let scan = scan_id(&state).await;
    let prompt = "Synthetic operator follow-up";
    let second = tokio::spawn(engine::run_turn(
        state.clone(),
        follow_up(&request, &scan, prompt),
        None,
        None,
    ));
    // The leader's follow-up turn commits while the deputy is still blocked.
    tokio::time::timeout(LIMIT, async {
        while !leader_prompts(&state, &scan).iter().any(|p| p == prompt) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(!first.is_finished() && !second.is_finished());
    release.add_permits(1);
    let first = tokio::time::timeout(LIMIT, first)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let second = tokio::time::timeout(LIMIT, second)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(first, second);
    // The operator message is its own transcript turn.
    let prompts = leader_prompts(&state, &scan);
    assert_eq!(prompts.iter().filter(|p| *p == prompt).count(), 1);
    let saved = beale_storage::swarm_state::SwarmStore::open(state.db.clone())
        .unwrap()
        .load(&scan, true)
        .unwrap();
    assert!(
        saved
            .attempts
            .iter()
            .all(|a| a.state == ExecutionState::Stopped)
    );
    server.abort();
    drop(state);
    let _ = std::fs::remove_dir_all(root);
}

/// A stop sent through an attached request stops the whole scan.
#[tokio::test]
async fn stop_through_an_attached_request_stops_the_scan() {
    let entered = Arc::new(tokio::sync::Semaphore::new(0));
    let deputy_calls = Arc::new(AtomicUsize::new(0));
    let counts = Arc::new(std::sync::Mutex::new(HashMap::<&'static str, usize>::new()));
    let app = Router::new().route(
        "/v1/chat/completions",
        axum::routing::post({
            let (entered, deputy_calls, counts) =
                (entered.clone(), deputy_calls.clone(), counts.clone());
            move |axum::Json(body): axum::Json<Value>| {
                let (entered, deputy_calls, counts) =
                    (entered.clone(), deputy_calls.clone(), counts.clone());
                async move {
                    let system = body["messages"][0]["content"].as_str().unwrap();
                    let role = role(system);
                    let n = {
                        let mut counts = counts.lock().unwrap();
                        let count = counts.entry(role).or_default();
                        let n = *count;
                        *count += 1;
                        n
                    };
                    sse(match role {
                        "reviewer" => approved(),
                        "leader" if n == 0 => spawn("research_deputy", 1),
                        "deputy" => {
                            deputy_calls.fetch_add(1, Ordering::SeqCst);
                            entered.add_permits(1);
                            // Never answers; only a stop ends this execution.
                            std::future::pending::<()>().await;
                            unreachable!()
                        }
                        other => json!({"content":format!("{other} synthetic result")}),
                    })
                }
            }
        }),
    );
    let (root, state, request, server) = regression_session(app).await;
    let first = tokio::spawn(engine::run_turn(state.clone(), request.clone(), None, None));
    tokio::time::timeout(LIMIT, entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let scan = scan_id(&state).await;
    let (stop, stopped) = watch::channel(false);
    let second = tokio::spawn(engine::run_turn(
        state.clone(),
        follow_up(&request, &scan, "Synthetic follow-up before stop"),
        None,
        Some(stopped),
    ));
    stop.send_replace(true);
    let first = tokio::time::timeout(LIMIT, first)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let second = tokio::time::timeout(LIMIT, second)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(first, second);
    let saved = beale_storage::swarm_state::SwarmStore::open(state.db.clone())
        .unwrap()
        .load(&scan, true)
        .unwrap();
    assert!(saved.ended_at_ms.is_some());
    assert!(
        saved
            .attempts
            .iter()
            .all(|a| a.state == ExecutionState::Stopped)
    );
    assert!(
        saved
            .agents
            .iter()
            .all(|a| a.summary.state == AgentState::Stopped)
    );
    // The stopped deputy was never claimed again.
    assert_eq!(deputy_calls.load(Ordering::SeqCst), 1);
    server.abort();
    drop(state);
    let _ = std::fs::remove_dir_all(root);
}

/// Follow-ups racing each other and the scheduler's idle exit are each
/// delivered as their own leader turn; none is lost or rejected.
#[tokio::test]
async fn racing_follow_ups_are_never_lost() {
    let app = Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(move |axum::Json(body): axum::Json<Value>| async move {
            let system = body["messages"][0]["content"].as_str().unwrap();
            sse(match role(system) {
                "reviewer" => approved(),
                _ => json!({"content":"Synthetic coordination answer"}),
            })
        }),
    );
    let (root, state, request, server) = regression_session(app).await;
    let detail = tokio::time::timeout(
        LIMIT,
        engine::run_turn(state.clone(), request.clone(), None, None),
    )
    .await
    .unwrap()
    .unwrap();
    let scan = detail.summary.unwrap().id;
    let mut expected = vec![request.prompt.clone()];
    for round in 0..12u64 {
        let early = format!("Synthetic follow-up {round}a");
        let late = format!("Synthetic follow-up {round}b");
        let a = tokio::spawn(engine::run_turn(
            state.clone(),
            follow_up(&request, &scan, &early),
            None,
            None,
        ));
        // Vary the gap so the second request meets every scheduler phase.
        tokio::time::sleep(Duration::from_millis(round * 7 % 40)).await;
        let b = tokio::spawn(engine::run_turn(
            state.clone(),
            follow_up(&request, &scan, &late),
            None,
            None,
        ));
        let a = tokio::time::timeout(LIMIT, a).await.unwrap().unwrap();
        let b = tokio::time::timeout(LIMIT, b).await.unwrap().unwrap();
        assert!(a.is_ok() && b.is_ok(), "round {round}: {a:?} {b:?}");
        expected.push(early);
        expected.push(late);
    }
    let mut prompts = leader_prompts(&state, &scan);
    prompts.sort();
    expected.sort();
    assert_eq!(prompts, expected);
    server.abort();
    drop(state);
    let _ = std::fs::remove_dir_all(root);
}

/// A subscriber that never reads cannot hold back the leader or its own
/// request's completion.
#[tokio::test]
async fn unread_subscriber_does_not_block_the_scan() {
    let app = Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(move |axum::Json(body): axum::Json<Value>| async move {
            let system = body["messages"][0]["content"].as_str().unwrap();
            let text = "Synthetic streamed answer ".repeat(64);
            let mut frames = String::new();
            for word in text.split_inclusive(' ') {
                let chunk = json!({"choices":[{"delta":{"content":word}}]});
                frames.push_str(&format!("data: {chunk}\n\n"));
            }
            frames.push_str("data: [DONE]\n\n");
            if role(system) == "reviewer" {
                sse(approved())
            } else {
                ([("content-type", "text/event-stream")], frames).into_response()
            }
        }),
    );
    let (root, state, request, server) = regression_session(app).await;
    // Room for one event, and nothing ever reads it.
    let (tx, _unread) = mpsc::channel(1);
    let detail = tokio::time::timeout(
        LIMIT,
        engine::run_turn(state.clone(), request, Some(tx), None),
    )
    .await
    .expect("an unread subscriber blocked the scan")
    .unwrap();
    assert_eq!(
        detail.messages.last().map(|m| m.text.trim()),
        Some("Synthetic streamed answer ".repeat(64).trim())
    );
    server.abort();
    drop(state);
    let _ = std::fs::remove_dir_all(root);
}
