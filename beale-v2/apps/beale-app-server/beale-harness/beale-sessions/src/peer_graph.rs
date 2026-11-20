use super::engine::{reasoning_effort_name, run_one_turn};
use super::*;
use beale_graphs::GraphHandoffRow;
use tokio::task::JoinSet;

struct Work {
    role: beale_graphs::Role,
    prompt: String,
    handoff_id: Option<String>,
    soft_reminder: Option<watch::Receiver<bool>>,
}

type WorkResult = (
    beale_graphs::Role,
    Option<String>,
    i64,
    Result<SessionDetail, TurnError>,
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
    matches!(
        id,
        beale_graphs::CLOUDFLARE_ID | beale_graphs::OPENAI_ID | beale_graphs::SEQUENTIAL_ID
    )
}

pub(super) fn applies(
    db: &HostDatabase,
    request: &SubmitSessionTurnRequest,
) -> Result<bool, TurnError> {
    if request.session_id.is_empty() {
        return Ok(is_peer_graph(&request.graph_id));
    }
    if !request.graph_id.is_empty() {
        return Ok(false);
    }
    db.is_peer_graph_session(&request.session_id)
        .map_err(|_| TurnError::Internal)
}

fn start(
    tasks: &mut JoinSet<WorkResult>,
    context: &StartContext<'_>,
    work: Work,
    stop_override: Option<watch::Receiver<bool>>,
) -> Result<(), TurnError> {
    let number = context
        .state
        .db
        .reserve_graph_turn(&context.base.session_id, work.role.id())
        .map_err(|_| TurnError::Conflict)?;
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
        soft_reminder: work.soft_reminder,
    };
    let progress = context.progress.clone();
    let stop = stop_override.or_else(|| context.stop.clone());
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
) -> Result<SessionDetail, TurnError> {
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
            || (request.graph_id == beale_graphs::SEQUENTIAL_ID
                && (request.stage_duration_minutes > 1_440
                    || request.goal_mode
                    || !request.repeat_preset.is_empty()))
            || (request.graph_id != beale_graphs::SEQUENTIAL_ID
                && request.stage_duration_minutes != 0)
            || (!request.repeat_preset.is_empty()
                && !matches!(
                    request.repeat_preset.as_str(),
                    "minute" | "hour" | "day" | "week" | "month"
                ))
        {
            return Err(TurnError::Invalid);
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
        let entry = beale_graphs::entry_role(&graph_id).ok_or(TurnError::Invalid)?;
        if !request.agent_role.is_empty() && request.agent_role != entry.id() {
            return Err(TurnError::Invalid);
        }
        let mut config = SessionGraphConfig::decode(
            encoded_graph_config(&request)
                .ok_or(TurnError::Invalid)?
                .as_slice(),
        )
        .map_err(|_| TurnError::Invalid)?;
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
            .map_err(|_| TurnError::Internal)?
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
            .map_err(|_| TurnError::Internal)?;
        request.repeat_preset.clear();
        register_session_lock(state, &id, lock).await;
        request.session_id = id;
        (graph_id, config, true)
    } else {
        let id = parse_id(&request.session_id)?;
        state
            .db
            .recover_graph_session(&request.session_id)
            .map_err(|_| TurnError::Internal)?;
        let (session, _) = load_persisted(&state.db, id)?;
        if session.workspace_id != request.workspace_id {
            return Err(TurnError::Conflict);
        }
        let graph_id = state
            .db
            .session_graph_id(&request.session_id)
            .map_err(|_| TurnError::Internal)?
            .ok_or(TurnError::Invalid)?;
        if !is_peer_graph(&graph_id)
            || !state
                .db
                .is_peer_graph_session(&request.session_id)
                .map_err(|_| TurnError::Internal)?
        {
            return Err(TurnError::Invalid);
        }
        if !state
            .db
            .feature_enabled(beale_graphs::FEATURE_ID)
            .map_err(|_| TurnError::Internal)?
            || !state
                .db
                .extension_applies_to("feature", beale_graphs::FEATURE_ID, &request.workspace_id)
                .map_err(|_| TurnError::Internal)?
        {
            return Err(TurnError::Conflict);
        }
        let bytes = state
            .db
            .session_graph_config(&request.session_id)
            .map_err(|_| TurnError::Internal)?
            .ok_or(TurnError::Internal)?;
        let config =
            SessionGraphConfig::decode(bytes.as_slice()).map_err(|_| TurnError::Internal)?;
        (graph_id, config, false)
    };
    let graph = beale_graphs::graph_definition(&graph_id).ok_or(TurnError::Invalid)?;
    let role = requested_top_level_role(Some(graph), &request.agent_role)?
        .or_else(|| beale_graphs::entry_role_for_config(&graph_id, &config))
        .ok_or(TurnError::Invalid)?;
    let (notify, mut notified) = mpsc::unbounded_channel();
    let mut tasks = JoinSet::new();
    let mut active = [false; 11];
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
    if graph_id == beale_graphs::SEQUENTIAL_ID {
        let minutes = config.stage_duration_minutes;
        if !(1..=1_440).contains(&minutes) {
            return Err(TurnError::Invalid);
        }
    }
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
    if graph_id == beale_graphs::SEQUENTIAL_ID {
        return run_sequential(&context, role, request.prompt.clone(), &mut notified).await;
    }
    start(
        &mut tasks,
        &context,
        Work {
            role,
            prompt: request.prompt.clone(),
            handoff_id: None,
            soft_reminder: None,
        },
        None,
    )?;
    active[role.index()] = true;
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
        if !stop_requested(&stop) {
            for item in state
                .db
                .pending_graph_handoffs(&request.session_id)
                .map_err(|_| TurnError::Internal)?
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
                    .map_err(|_| TurnError::Internal)?;
                let prompt = format!("Handoff from {}: {}", item.from_role, item.task);
                if let Err(error) = start(
                    &mut tasks,
                    &context,
                    Work {
                        role: target,
                        prompt,
                        handoff_id: Some(item.id.clone()),
                        soft_reminder: None,
                    },
                    None,
                ) {
                    state
                        .db
                        .set_graph_handoff_status(&item.id, "pending")
                        .map_err(|_| TurnError::Internal)?;
                    return Err(error);
                }
                active[target.index()] = true;
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
            }
        }
        if tasks.is_empty() {
            break;
        }
        tokio::select! {
            Some(_) = notified.recv() => {},
            finished = tasks.join_next() => {
                let (role, handoff_id, turn_number, result) = finished
                    .ok_or(TurnError::Internal)?
                    .map_err(|_| TurnError::Internal)?;
                active[role.index()] = false;
                if let Some(id) = handoff_id {
                    state.db.set_graph_handoff_status(&id, if result.is_ok() { "completed" } else { "failed" })
                        .map_err(|_| TurnError::Internal)?;
                }
                if result.is_err() {
                    state.db.finish_graph_turn(&request.session_id, turn_number, false)
                        .map_err(|_| TurnError::Internal)?;
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
    Ok(detail(&state.db, &session, &turns)?)
}

fn discovery_stage(role: beale_graphs::Role) -> bool {
    matches!(
        role,
        beale_graphs::Role::AuditTargets | beale_graphs::Role::Discovery
    )
}

fn stage_should_advance(
    role: beale_graphs::Role,
    queue_empty: bool,
    deadline_reached: bool,
) -> bool {
    deadline_reached || (queue_empty && !discovery_stage(role))
}

const SEQUENTIAL_STAGES: [beale_graphs::Role; 5] = [
    beale_graphs::Role::AuditTargets,
    beale_graphs::Role::Discovery,
    beale_graphs::Role::Validation,
    beale_graphs::Role::Reporter,
    beale_graphs::Role::PatchGeneration,
];

fn stage_deadlines(
    started: tokio::time::Instant,
    duration_minutes: u32,
) -> (tokio::time::Instant, tokio::time::Instant) {
    let soft = started + std::time::Duration::from_secs(u64::from(duration_minutes) * 60);
    (soft, soft + std::time::Duration::from_secs(10 * 60))
}

fn queued_for_stage(
    db: &HostDatabase,
    session_id: &str,
    role: beale_graphs::Role,
) -> Result<Option<GraphHandoffRow>, TurnError> {
    Ok(db
        .pending_graph_handoffs(session_id)
        .map_err(|_| TurnError::Internal)?
        .into_iter()
        .find(|item| item.to_role == role.id()))
}

async fn sequential_started(
    context: &StartContext<'_>,
    role: beale_graphs::Role,
) -> Result<(), TurnError> {
    if let Some(tx) = &context.progress {
        let (session, turns) =
            load_persisted(&context.state.db, parse_id(&context.base.session_id)?)?;
        let _ = tx
            .send(SessionStreamEvent {
                kind: SessionStreamKind::AgentStarted as i32,
                detail: Some(detail(&context.state.db, &session, &turns)?),
                agent_role: role.id().to_owned(),
                ..Default::default()
            })
            .await;
    }
    Ok(())
}

async fn sequential_finished(
    context: &StartContext<'_>,
    finished: WorkResult,
    requeue: bool,
) -> Result<(), TurnError> {
    let (role, handoff_id, turn_number, result) = finished;
    if let Some(id) = handoff_id {
        context
            .state
            .db
            .set_graph_handoff_status(
                &id,
                if requeue {
                    "pending"
                } else if result.is_ok() {
                    "completed"
                } else {
                    "failed"
                },
            )
            .map_err(|_| TurnError::Internal)?;
    }
    if result.is_err() {
        context
            .state
            .db
            .finish_graph_turn(&context.base.session_id, turn_number, false)
            .map_err(|_| TurnError::Internal)?;
    }
    if let Some(tx) = &context.progress {
        let detail = if let Ok(detail) = result {
            detail
        } else {
            let (session, turns) =
                load_persisted(&context.state.db, parse_id(&context.base.session_id)?)?;
            detail(&context.state.db, &session, &turns)?
        };
        let _ = tx
            .send(SessionStreamEvent {
                kind: SessionStreamKind::AgentTurnCompleted as i32,
                detail: Some(detail),
                agent_role: role.id().to_owned(),
                ..Default::default()
            })
            .await;
    }
    Ok(())
}

async fn run_sequential(
    context: &StartContext<'_>,
    initial_role: beale_graphs::Role,
    initial_prompt: String,
    notified: &mut mpsc::UnboundedReceiver<()>,
) -> Result<SessionDetail, TurnError> {
    let mut stage_index = SEQUENTIAL_STAGES
        .iter()
        .position(|role| *role == initial_role)
        .ok_or(TurnError::Invalid)?;
    let mut first_prompt = Some(initial_prompt);
    loop {
        let role = SEQUENTIAL_STAGES[stage_index];
        let (deadline, hard_deadline) = stage_deadlines(
            tokio::time::Instant::now(),
            context.config.stage_duration_minutes,
        );
        let (soft_tx, soft_rx) = watch::channel(false);
        let (hard_tx, hard_rx) = watch::channel(false);
        let mut tasks = JoinSet::new();
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            if stop_requested(&context.stop) {
                hard_tx.send_replace(true);
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                soft_tx.send_replace(true);
            }
            if (now >= hard_deadline || stop_requested(&context.stop)) && !tasks.is_empty() {
                hard_tx.send_replace(true);
                if let Ok(Some(Ok(finished))) =
                    tokio::time::timeout(std::time::Duration::from_secs(2), tasks.join_next()).await
                {
                    sequential_finished(context, finished, true).await?;
                } else {
                    tasks.abort_all();
                    while tasks.join_next().await.is_some() {}
                    context
                        .state
                        .db
                        .recover_graph_session(&context.base.session_id)
                        .map_err(|_| TurnError::Internal)?;
                }
                break;
            }
            if tasks.is_empty() {
                if stop_requested(&context.stop) {
                    break;
                }
                let handoff = queued_for_stage(&context.state.db, &context.base.session_id, role)?;
                if stage_should_advance(
                    role,
                    first_prompt.is_none() && handoff.is_none(),
                    now >= deadline,
                ) {
                    break;
                }
                let work = if let Some(prompt) = first_prompt.take() {
                    Work {
                        role,
                        prompt,
                        handoff_id: None,
                        soft_reminder: Some(soft_rx.clone()),
                    }
                } else if let Some(item) = handoff {
                    context
                        .state
                        .db
                        .set_graph_handoff_status(&item.id, "running")
                        .map_err(|_| TurnError::Internal)?;
                    Work {
                        role,
                        prompt: format!("Handoff from {}: {}", item.from_role, item.task),
                        handoff_id: Some(item.id),
                        soft_reminder: Some(soft_rx.clone()),
                    }
                } else if discovery_stage(role) {
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    if tokio::time::Instant::now() >= deadline || stop_requested(&context.stop) {
                        continue;
                    }
                    Work {
                        role,
                        prompt: "Continue this discovery stage. Use current evidence and coverage; pursue fresh in-scope work, update durable records, and hand off concrete results. Summarize gaps if no further work is available.".to_owned(),
                        handoff_id: None,
                        soft_reminder: Some(soft_rx.clone()),
                    }
                } else {
                    break;
                };
                let handoff_id = work.handoff_id.clone();
                if let Err(error) = start(&mut tasks, context, work, Some(hard_rx.clone())) {
                    if let Some(id) = handoff_id {
                        context
                            .state
                            .db
                            .set_graph_handoff_status(&id, "pending")
                            .map_err(|_| TurnError::Internal)?;
                    }
                    return Err(error);
                }
                sequential_started(context, role).await?;
            }
            tokio::select! {
                _ = tick.tick() => {},
                _ = notified.recv() => {},
                finished = tasks.join_next() => {
                    let finished = finished.ok_or(TurnError::Internal)?
                        .map_err(|_| TurnError::Internal)?;
                    let failed = finished.3.is_err();
                    sequential_finished(context, finished, false).await?;
                    if failed {
                        return Err(TurnError::Provider);
                    }
                }
            }
        }
        if stop_requested(&context.stop) {
            break;
        }
        stage_index = (stage_index + 1) % SEQUENTIAL_STAGES.len();
    }
    let (session, turns) = load_persisted(&context.state.db, parse_id(&context.base.session_id)?)?;
    Ok(detail(&context.state.db, &session, &turns)?)
}

#[cfg(test)]
mod sequential_tests {
    use super::*;

    #[test]
    fn deadline_has_ten_minute_grace_after_configured_stage() {
        let started = tokio::time::Instant::now();
        let (soft, hard) = stage_deadlines(started, 60);
        assert_eq!(soft - started, std::time::Duration::from_secs(60 * 60));
        assert_eq!(hard - soft, std::time::Duration::from_secs(10 * 60));
    }

    #[test]
    fn cycle_returns_to_audit_targets_and_only_downstream_empty_queues_advance() {
        assert_eq!(SEQUENTIAL_STAGES[0], beale_graphs::Role::AuditTargets);
        assert_eq!(SEQUENTIAL_STAGES[1], beale_graphs::Role::Discovery);
        assert_eq!(SEQUENTIAL_STAGES[4], beale_graphs::Role::PatchGeneration);
        assert_eq!(
            SEQUENTIAL_STAGES[(4 + 1) % SEQUENTIAL_STAGES.len()],
            beale_graphs::Role::AuditTargets
        );
        for role in [
            beale_graphs::Role::AuditTargets,
            beale_graphs::Role::Discovery,
        ] {
            assert!(!stage_should_advance(role, true, false));
            assert!(stage_should_advance(role, true, true));
        }
        for role in [
            beale_graphs::Role::Validation,
            beale_graphs::Role::Reporter,
            beale_graphs::Role::PatchGeneration,
        ] {
            assert!(!stage_should_advance(role, false, false));
            assert!(stage_should_advance(role, true, false));
        }
    }
}
