use super::*;
use tokio::task::JoinSet;

const MAX_WORK_ITEMS_PER_REQUEST: usize = 32;

struct Work {
    role: beale_graphs::Role,
    prompt: String,
    handoff_id: Option<String>,
}

type WorkResult = (
    beale_graphs::Role,
    Option<String>,
    i64,
    Result<SessionDetail, StatusCode>,
);

struct StartContext<'a> {
    state: &'a SessionState,
    base: &'a SubmitSessionTurnRequest,
    config: &'a SessionGraphConfig,
    progress: Option<mpsc::Sender<SessionStreamEvent>>,
    lock: &'a Arc<Mutex<()>>,
    notify: &'a mpsc::UnboundedSender<()>,
    stop: Option<watch::Receiver<bool>>,
}

fn is_peer_graph(id: &str) -> bool {
    matches!(id, beale_graphs::CLOUDFLARE_ID | beale_graphs::OPENAI_ID)
}

pub(super) fn applies(
    db: &HostDatabase,
    request: &SubmitSessionTurnRequest,
) -> Result<bool, StatusCode> {
    if request.session_id.is_empty() {
        return Ok(is_peer_graph(&request.graph_id));
    }
    if !request.graph_id.is_empty() {
        return Ok(false);
    }
    db.is_peer_graph_session(&request.session_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

fn start(
    tasks: &mut JoinSet<WorkResult>,
    context: &StartContext<'_>,
    work: Work,
) -> Result<(), StatusCode> {
    let number = context
        .state
        .db
        .reserve_graph_turn(&context.base.session_id, work.role.id())
        .map_err(|_| StatusCode::CONFLICT)?;
    let fallback = GraphNodeModel {
        node_id: beale_graphs::entry_role(&context.base.graph_id)
            .unwrap_or(beale_graphs::Role::Main)
            .id()
            .to_owned(),
        provider: context.base.provider,
        method: context.base.method,
        model: context.base.model.clone(),
    };
    let model = beale_graphs::top_level_model(context.config, work.role, &fallback);
    let mut request = context.base.clone();
    request.graph_id.clear();
    request.graph_models.clear();
    request.graph_node_concurrency.clear();
    request.agent_role = work.role.id().to_owned();
    request.provider = model.provider;
    request.method = model.method;
    request.model = model.model;
    request.prompt = work.prompt;
    if work.handoff_id.is_some() {
        request.goal_mode = false;
        request.goal_max_turns = 0;
        request.goal_max_duration_minutes = 0;
        request.repeat_preset.clear();
    }
    let state = context.state.clone();
    let lock = context.lock.clone();
    let peer = PeerRun {
        role: work.role,
        turn_number: number,
        notify: context.notify.clone(),
        handoff: work.handoff_id.is_some(),
    };
    let progress = context.progress.clone();
    let stop = context.stop.clone();
    tasks.spawn(async move {
        let result = run_one_turn(&state, request, progress, false, &lock, stop, Some(peer)).await;
        (work.role, work.handoff_id, number, result)
    });
    Ok(())
}

pub(super) async fn run(
    state: &SessionState,
    mut request: SubmitSessionTurnRequest,
    progress: Option<mpsc::Sender<SessionStreamEvent>>,
    lock: &Arc<Mutex<()>>,
    stop: Option<watch::Receiver<bool>>,
) -> Result<SessionDetail, StatusCode> {
    let (graph_id, config, first) = if request.session_id.is_empty() {
        parse_id(&request.workspace_id)?;
        if !state.workspaces.contains(&request.workspace_id)?
            || request.prompt.trim().is_empty()
            || request.prompt.len() > 16_384
            || request.model.is_empty()
            || request.model.len() > 200
            || !request.model.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
            })
            || !matches!(ProviderId::try_from(request.provider), Ok(provider) if provider != ProviderId::Unspecified)
            || !matches!(AuthMethod::try_from(request.method), Ok(method) if method != AuthMethod::Unspecified)
            || (request.provider == ProviderId::Custom as i32
                && request.method != AuthMethod::ApiKey as i32)
            || (!request.goal_mode
                && (request.goal_max_turns != 0 || request.goal_max_duration_minutes != 0))
            || request.goal_max_turns > i64::MAX as u64
            || request.goal_max_duration_minutes > i64::MAX as u64 / 60_000
            || (!request.repeat_preset.is_empty()
                && !matches!(
                    request.repeat_preset.as_str(),
                    "minute" | "hour" | "day" | "week" | "month"
                ))
        {
            return Err(StatusCode::BAD_REQUEST);
        }
        reasoning_effort_name(request.reasoning_effort)?;
        validate_graph_selection(
            &state.db,
            &request.graph_id,
            &request.graph_models,
            &request.graph_node_concurrency,
            "",
            &request.workspace_id,
        )?;
        let graph_id = request.graph_id.clone();
        let entry = beale_graphs::entry_role(&graph_id).ok_or(StatusCode::BAD_REQUEST)?;
        if !request.agent_role.is_empty() && request.agent_role != entry.id() {
            return Err(StatusCode::BAD_REQUEST);
        }
        let mut config = SessionGraphConfig::decode(
            encoded_graph_config(&request)
                .ok_or(StatusCode::BAD_REQUEST)?
                .as_slice(),
        )
        .map_err(|_| StatusCode::BAD_REQUEST)?;
        if !config
            .models
            .iter()
            .any(|model| model.node_id == entry.id())
        {
            config.models.push(GraphNodeModel {
                node_id: entry.id().to_owned(),
                provider: request.provider,
                method: request.method,
                model: request.model.clone(),
            });
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .as_millis()
            .min(i64::MAX as u128) as i64;
        let id = Uuid::new_v4().to_string();
        let row = SessionRow {
            id: id.clone(),
            workspace_id: request.workspace_id.clone(),
            title: title(&request.prompt),
            created_at_ms: now,
            last_message_at_ms: now,
        };
        let repeat = (!request.repeat_preset.is_empty()).then_some((
            request.repeat_preset.as_str(),
            AutomationOptions {
                reasoning_effort: request.reasoning_effort,
                goal_mode: request.goal_mode,
                goal_max_turns: request.goal_max_turns,
                goal_max_duration_minutes: request.goal_max_duration_minutes,
            },
        ));
        state
            .db
            .create_peer_graph_session(&row, &graph_id, &config.encode_to_vec(), repeat)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        request.repeat_preset.clear();
        register_session_lock(state, &id, lock).await;
        request.session_id = id;
        (graph_id, config, true)
    } else {
        let id = parse_id(&request.session_id)?;
        state
            .db
            .recover_graph_session(&request.session_id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let (session, _) = load_persisted(&state.db, id)?;
        if session.workspace_id != request.workspace_id {
            return Err(StatusCode::CONFLICT);
        }
        let graph_id = state
            .db
            .session_graph_id(&request.session_id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .ok_or(StatusCode::BAD_REQUEST)?;
        if !is_peer_graph(&graph_id)
            || !state
                .db
                .is_peer_graph_session(&request.session_id)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        {
            return Err(StatusCode::BAD_REQUEST);
        }
        if !state
            .db
            .feature_enabled(beale_graphs::FEATURE_ID)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            || !state
                .db
                .extension_applies_to("feature", beale_graphs::FEATURE_ID, &request.workspace_id)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        {
            return Err(StatusCode::CONFLICT);
        }
        let bytes = state
            .db
            .session_graph_config(&request.session_id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
        let config = SessionGraphConfig::decode(bytes.as_slice())
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        (graph_id, config, false)
    };
    let graph = beale_graphs::graph_definition(&graph_id).ok_or(StatusCode::BAD_REQUEST)?;
    let role = requested_top_level_role(Some(graph), &request.agent_role)?
        .or_else(|| beale_graphs::entry_role(&graph_id))
        .ok_or(StatusCode::BAD_REQUEST)?;
    let (notify, mut notified) = mpsc::unbounded_channel();
    let mut tasks = JoinSet::new();
    let mut active = [false; 10];
    let mut launched = 0_usize;
    request.graph_id = graph_id.clone();
    let context = StartContext {
        state,
        base: &request,
        config: &config,
        progress: progress.clone(),
        lock,
        notify: &notify,
        stop: stop.clone(),
    };
    if first && let Some(tx) = &progress {
        let (session, turns) = load_persisted(&state.db, parse_id(&request.session_id)?)?;
        let _ = tx
            .send(SessionStreamEvent {
                kind: SessionStreamKind::SessionStarted as i32,
                detail: Some(detail(&state.db, &session, &turns)?),
                agent_role: role.id().to_owned(),
                ..Default::default()
            })
            .await;
    }
    start(
        &mut tasks,
        &context,
        Work {
            role,
            prompt: request.prompt.clone(),
            handoff_id: None,
        },
    )?;
    active[role.index()] = true;
    launched += 1;
    if let Some(tx) = &progress {
        let (session, turns) = load_persisted(&state.db, parse_id(&request.session_id)?)?;
        let _ = tx
            .send(SessionStreamEvent {
                kind: SessionStreamKind::AgentStarted as i32,
                detail: Some(detail(&state.db, &session, &turns)?),
                agent_role: role.id().to_owned(),
                ..Default::default()
            })
            .await;
    }
    loop {
        if !stop_requested(&stop) && launched < MAX_WORK_ITEMS_PER_REQUEST {
            for item in state
                .db
                .pending_graph_handoffs(&request.session_id)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            {
                if item.status != "pending" {
                    continue;
                }
                let Some(target) = beale_graphs::Role::from_id(&item.to_role) else {
                    continue;
                };
                if active[target.index()] {
                    continue;
                }
                state
                    .db
                    .set_graph_handoff_status(&item.id, "running")
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                let prompt = format!("Handoff from {}: {}", item.from_role, item.task);
                if let Err(error) = start(
                    &mut tasks,
                    &context,
                    Work {
                        role: target,
                        prompt,
                        handoff_id: Some(item.id.clone()),
                    },
                ) {
                    state
                        .db
                        .set_graph_handoff_status(&item.id, "pending")
                        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                    return Err(error);
                }
                active[target.index()] = true;
                launched += 1;
                if let Some(tx) = &progress {
                    let (session, turns) =
                        load_persisted(&state.db, parse_id(&request.session_id)?)?;
                    let _ = tx
                        .send(SessionStreamEvent {
                            kind: SessionStreamKind::AgentStarted as i32,
                            detail: Some(detail(&state.db, &session, &turns)?),
                            agent_role: target.id().to_owned(),
                            ..Default::default()
                        })
                        .await;
                }
                if launched >= MAX_WORK_ITEMS_PER_REQUEST {
                    break;
                }
            }
        }
        if tasks.is_empty() {
            break;
        }
        tokio::select! {
            Some(_) = notified.recv() => {},
            finished = tasks.join_next() => {
                let (role, handoff_id, turn_number, result) = finished
                    .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                active[role.index()] = false;
                if let Some(id) = handoff_id {
                    state.db.set_graph_handoff_status(&id, if result.is_ok() { "completed" } else { "failed" })
                        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                }
                if result.is_err() {
                    state.db.finish_graph_turn(&request.session_id, turn_number, false)
                        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                }
                if let Some(tx) = &progress {
                    let detail = if let Ok(detail) = result {
                        detail
                    } else {
                        let (session, turns) = load_persisted(&state.db, parse_id(&request.session_id)?)?;
                        detail(&state.db, &session, &turns)?
                    };
                    let _ = tx.send(SessionStreamEvent {
                        kind: SessionStreamKind::AgentTurnCompleted as i32,
                        detail: Some(detail),
                        agent_role: role.id().to_owned(),
                        ..Default::default()
                    }).await;
                }
            }
        }
    }
    let (session, turns) = load_persisted(&state.db, parse_id(&request.session_id)?)?;
    detail(&state.db, &session, &turns)
}
