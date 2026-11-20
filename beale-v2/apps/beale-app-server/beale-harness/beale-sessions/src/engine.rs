//! The session turn engine: request validation, context and toolset assembly,
//! the provider and tool round loop, Goal Mode continuation, and persistence.
//!
//! The engine reports [`TurnError`] rather than HTTP status codes so routes,
//! automations, and peer graph orchestration share one typed failure contract.
//! HTTP handlers translate errors at the route boundary.

use super::*;

/// Why a turn could not run or complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TurnError {
    /// The request is malformed or names an unsupported option.
    Invalid,
    /// The workspace or session does not exist.
    NotFound,
    /// The session is busy or changed, a required feature is off, or history is full.
    Conflict,
    /// The provider failed before any activity could be saved.
    Provider,
    /// Host storage or another internal operation failed.
    Internal,
}

impl TurnError {
    pub(crate) fn status(self) -> StatusCode {
        match self {
            TurnError::Invalid => StatusCode::BAD_REQUEST,
            TurnError::NotFound => StatusCode::NOT_FOUND,
            TurnError::Conflict => StatusCode::CONFLICT,
            TurnError::Provider => StatusCode::BAD_GATEWAY,
            TurnError::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// A bounded, client-safe description.
    pub(crate) fn message(self) -> &'static str {
        match self {
            TurnError::Invalid => "The session request is invalid.",
            TurnError::NotFound => "Workspace or session is unavailable.",
            TurnError::Conflict => "Session changed or reached its turn limit.",
            TurnError::Provider => "The provider could not complete this turn.",
            TurnError::Internal => "Session request failed.",
        }
    }
}

/// Shared host helpers (workspace registry, transcript loading, session detail)
/// still report status codes; the engine maps them to its own contract.
impl From<StatusCode> for TurnError {
    fn from(status: StatusCode) -> Self {
        match status {
            StatusCode::BAD_REQUEST => TurnError::Invalid,
            StatusCode::NOT_FOUND => TurnError::NotFound,
            StatusCode::CONFLICT => TurnError::Conflict,
            StatusCode::BAD_GATEWAY => TurnError::Provider,
            _ => TurnError::Internal,
        }
    }
}

pub(crate) fn reasoning_effort_name(value: i32) -> Result<Option<&'static str>, TurnError> {
    match ReasoningEffort::try_from(value) {
        Ok(ReasoningEffort::Unspecified) => Ok(None),
        Ok(ReasoningEffort::None) => Ok(Some("none")),
        Ok(ReasoningEffort::Minimal) => Ok(Some("minimal")),
        Ok(ReasoningEffort::Low) => Ok(Some("low")),
        Ok(ReasoningEffort::Medium) => Ok(Some("medium")),
        Ok(ReasoningEffort::High) => Ok(Some("high")),
        Ok(ReasoningEffort::Xhigh) => Ok(Some("xhigh")),
        Ok(ReasoningEffort::Max) => Ok(Some("max")),
        Err(_) => Err(TurnError::Invalid),
    }
}

pub(crate) fn stop_requested(stop: &Option<watch::Receiver<bool>>) -> bool {
    stop.as_ref().is_some_and(|receiver| *receiver.borrow())
}

pub(crate) async fn until_stopped<F: Future>(
    future: F,
    stop: &mut Option<watch::Receiver<bool>>,
) -> Option<F::Output> {
    tokio::pin!(future);
    if let Some(receiver) = stop.as_mut() {
        tokio::select! {
            result = &mut future => Some(result),
            changed = receiver.changed() => {
                if changed.is_ok() && *receiver.borrow() {
                    None
                } else {
                    Some(future.await)
                }
            }
        }
    } else {
        Some(future.await)
    }
}

#[cfg(feature = "goal-mode")]
pub(crate) fn goal_signal_for_answer(
    answer: &str,
    signal: Option<beale_goals::Signal>,
) -> Option<beale_goals::Signal> {
    if answer.starts_with(INTERRUPTED_ANSWER) || answer == STOPPED_ANSWER {
        None
    } else {
        signal.or(Some(beale_goals::Signal::Continue))
    }
}

#[cfg(feature = "goal-mode")]
pub(crate) async fn run_turn(
    state: SessionState,
    mut request: SubmitSessionTurnRequest,
    progress: Option<mpsc::Sender<SessionStreamEvent>>,
    stop: Option<watch::Receiver<bool>>,
) -> Result<SessionDetail, TurnError> {
    let lock = if request.session_id.is_empty() {
        Arc::new(Mutex::new(()))
    } else {
        session_lock(&state, &request.session_id).await
    };
    let _guard = lock.try_lock().map_err(|_| TurnError::Conflict)?;
    let mut automatic = false;
    loop {
        #[cfg(feature = "graphs")]
        let peer = peer_graph::applies(&state.db, &request)?;
        #[cfg(not(feature = "graphs"))]
        let peer = false;
        let detail = if peer {
            #[cfg(feature = "graphs")]
            {
                peer_graph::run(
                    &state,
                    request.clone(),
                    progress.clone(),
                    &lock,
                    stop.clone(),
                )
                .await?
            }
            #[cfg(not(feature = "graphs"))]
            {
                unreachable!()
            }
        } else {
            run_one_turn(
                &state,
                request.clone(),
                progress.clone(),
                automatic,
                &lock,
                stop.clone(),
                #[cfg(feature = "graphs")]
                None,
            )
            .await?
        };
        if let Some(goal) = &detail.goal
            && goal.status == GoalStatus::Active as i32
        {
            if stop_requested(&stop) {
                let id = &detail.summary.as_ref().ok_or(TurnError::Internal)?.id;
                state
                    .db
                    .set_goal_status(id, GoalStatus::Paused as i32)
                    .map_err(|_| TurnError::Internal)?;
                let (session, turns) = load_persisted(&state.db, parse_id(id)?)?;
                return Ok(self::detail(&state.db, &session, &turns)?);
            }
            if !state
                .db
                .feature_enabled(beale_goals::FEATURE_ID)
                .map_err(|_| TurnError::Internal)?
                || !state
                    .db
                    .extension_applies_to(
                        "feature",
                        beale_goals::FEATURE_ID,
                        &detail
                            .summary
                            .as_ref()
                            .ok_or(TurnError::Internal)?
                            .workspace_id,
                    )
                    .map_err(|_| TurnError::Internal)?
            {
                let id = detail
                    .summary
                    .as_ref()
                    .ok_or(TurnError::Internal)?
                    .id
                    .clone();
                state
                    .db
                    .set_goal_status(&id, GoalStatus::Paused as i32)
                    .map_err(|_| TurnError::Internal)?;
                let (session, turns) = load_persisted(&state.db, parse_id(&id)?)?;
                return Ok(self::detail(&state.db, &session, &turns)?);
            }
            if let Some(tx) = &progress {
                let _ = tx
                    .send(SessionStreamEvent {
                        runbook_progress: None,
                        kind: SessionStreamKind::TurnCompleted as i32,
                        text: String::new(),
                        tool: None,
                        detail: Some(detail.clone()),
                        segment_index: 0,
                        ..Default::default()
                    })
                    .await;
            }
            request.session_id = detail
                .summary
                .as_ref()
                .ok_or(TurnError::Internal)?
                .id
                .clone();
            request.prompt = beale_goals::continuation_prompt(&goal.objective)
                .map_err(|_| TurnError::Internal)?;
            request.goal_mode = false;
            request.goal_max_turns = 0;
            request.goal_max_duration_minutes = 0;
            request.repeat_preset.clear();
            request.graph_id.clear();
            request.graph_models.clear();
            request.graph_node_concurrency.clear();
            automatic = true;
            continue;
        }
        return Ok(detail);
    }
}

#[cfg(not(feature = "goal-mode"))]
pub(crate) async fn run_turn(
    state: SessionState,
    request: SubmitSessionTurnRequest,
    progress: Option<mpsc::Sender<SessionStreamEvent>>,
    stop: Option<watch::Receiver<bool>>,
) -> Result<SessionDetail, TurnError> {
    if request.goal_mode {
        return Err(TurnError::Invalid);
    }
    let lock = if request.session_id.is_empty() {
        Arc::new(Mutex::new(()))
    } else {
        session_lock(&state, &request.session_id).await
    };
    let _guard = lock.try_lock().map_err(|_| TurnError::Conflict)?;
    #[cfg(feature = "graphs")]
    if peer_graph::applies(&state.db, &request)? {
        return peer_graph::run(&state, request, progress, &lock, stop).await;
    }
    run_one_turn(
        &state,
        request,
        progress,
        false,
        &lock,
        stop,
        #[cfg(feature = "graphs")]
        None,
    )
    .await
}

/// Request fields resolved before any host state is read.
#[derive(Debug, PartialEq, Eq)]
struct ValidatedRequest {
    provider: Provider,
    mode: AuthMode,
    reasoning_effort: Option<&'static str>,
}

/// Checks the request shape and resolves its provider, credential method, and
/// reasoning effort. Host-dependent checks (workspace, graph feature scope,
/// saved session) follow separately.
fn validate_request(request: &SubmitSessionTurnRequest) -> Result<ValidatedRequest, TurnError> {
    if request.prompt.trim().is_empty()
        || request.prompt.len() > 16_384
        || request.model.is_empty()
        || request.model.len() > 200
        || !request.model.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
        })
    {
        return Err(TurnError::Invalid);
    }
    let provider = match ProviderId::try_from(request.provider) {
        Ok(ProviderId::Openai) => Provider::OpenAi,
        Ok(ProviderId::Xai) => Provider::Xai,
        Ok(ProviderId::Anthropic) => Provider::Anthropic,
        Ok(ProviderId::Zai) => Provider::Zai,
        Ok(ProviderId::Openrouter) => Provider::OpenRouter,
        Ok(ProviderId::Custom) => Provider::Custom,
        _ => return Err(TurnError::Invalid),
    };
    let mode = match AuthMethod::try_from(request.method) {
        Ok(AuthMethod::Subscription) => AuthMode::Subscription,
        Ok(AuthMethod::ApiKey) => AuthMode::ApiKey,
        _ => return Err(TurnError::Invalid),
    };
    if provider == Provider::Custom && mode != AuthMode::ApiKey {
        return Err(TurnError::Invalid);
    }
    let reasoning_effort = reasoning_effort_name(request.reasoning_effort)?;
    if !request.goal_mode && (request.goal_max_turns != 0 || request.goal_max_duration_minutes != 0)
    {
        return Err(TurnError::Invalid);
    }
    if request.goal_max_turns > i64::MAX as u64
        || request.goal_max_duration_minutes > i64::MAX as u64 / 60_000
    {
        return Err(TurnError::Invalid);
    }
    if !request.repeat_preset.is_empty()
        && (!request.session_id.is_empty()
            || !matches!(
                request.repeat_preset.as_str(),
                "minute" | "hour" | "day" | "week" | "month"
            ))
    {
        return Err(TurnError::Invalid);
    }
    Ok(ValidatedRequest {
        provider,
        mode,
        reasoning_effort,
    })
}

/// Runtime state that decides which dynamic features apply to a turn.
struct DynamicFeatures<'a> {
    #[cfg(feature = "fleet")]
    fleet_note: Option<&'a str>,
    #[cfg(feature = "goal-mode")]
    goal_active: bool,
    #[cfg(feature = "goal-mode")]
    goal_section: Option<&'a str>,
    #[cfg(feature = "graphs")]
    graph: Option<&'static dyn beale_graphs::GraphRuntime>,
    #[cfg(feature = "graphs")]
    graph_section: Option<&'a str>,
    _marker: std::marker::PhantomData<&'a ()>,
}

/// Model tools and extension catalog entries published for one turn.
struct Extensions {
    definitions: Vec<Value>,
    entries: String,
    #[cfg(feature = "graphs")]
    child_entries: String,
    #[cfg(feature = "graphs")]
    child_tools: Arc<Vec<Value>>,
    callable_plugins: HashSet<String>,
}

/// Publishes enabled feature guidance and installed plugin instructions for a
/// turn and returns the tool definitions offered to the main agent and the
/// subset offered to graph subagents.
async fn assemble_extensions(
    state: &SessionState,
    workspace_id: &str,
    agent_files: &AgentFiles,
    dynamic: &DynamicFeatures<'_>,
) -> Result<Extensions, TurnError> {
    let _ = dynamic;
    let mut definitions = definitions();
    let mut entries = String::new();
    #[cfg(feature = "graphs")]
    let mut child_entries = String::new();
    #[cfg(feature = "graphs")]
    let mut child_tools = Vec::new();
    for feature in features::native_features() {
        let (enabled, tools, guidance): (bool, Vec<Value>, &str) = match feature.id {
            #[cfg(feature = "fleet")]
            beale_fleet::FEATURE_ID => (
                dynamic.fleet_note.is_some(),
                Vec::new(),
                dynamic.fleet_note.unwrap_or(""),
            ),
            #[cfg(feature = "goal-mode")]
            beale_goals::FEATURE_ID => (
                dynamic.goal_active,
                vec![beale_goals::definition()],
                dynamic
                    .goal_section
                    .unwrap_or(beale_goals::PROMPT_EXTENSION),
            ),
            #[cfg(feature = "graphs")]
            beale_graphs::FEATURE_ID => (
                dynamic.graph.is_some(),
                dynamic
                    .graph
                    .map(|runtime| {
                        let mut tools = vec![runtime.tool_definition()];
                        tools.extend(beale_graphs::subagent_tools());
                        tools
                    })
                    .unwrap_or_default(),
                dynamic
                    .graph_section
                    .unwrap_or(beale_graphs::PROMPT_EXTENSION),
            ),
            _ => (
                feature_applies(&state.db, feature.id, workspace_id)?,
                feature
                    .definitions
                    .map_or_else(Vec::new, |definitions| definitions()),
                feature.prompt_extension,
            ),
        };
        if !enabled || (tools.is_empty() && guidance.is_empty()) {
            continue;
        }
        let id = feature.id;
        let path = agent_files
            .write(&format!("features/{id}/FEATURE.md"), guidance.as_bytes())
            .map_err(|_| TurnError::Internal)?;
        let entry = format!(
            "- Feature `{id}`: {} Read {}.\n",
            feature.use_case,
            json!(path.to_string_lossy())
        );
        entries.push_str(&entry);
        #[cfg(feature = "graphs")]
        if feature.subagents {
            child_entries.push_str(&entry);
            child_tools.extend(tools.iter().cloned());
        }
        definitions.extend(tools);
    }
    let available_plugins = state.plugins.available(workspace_id).map_err(|error| {
        beale_host_log::error!("plugin directory failed: {error:#}");
        TurnError::Internal
    })?;
    let mut callable_plugins = HashSet::new();
    for plugin in &available_plugins {
        match state
            .plugins
            .publish(&plugin.name, workspace_id, agent_files)
            .await
        {
            Ok((path, has_tools)) => {
                entries.push_str(&format!(
                    "- Plugin `{}`: {} Read {}.\n",
                    plugin.name,
                    plugin.description,
                    json!(path.to_string_lossy())
                ));
                if has_tools {
                    callable_plugins.insert(plugin.name.clone());
                }
            }
            Err(error) => {
                beale_host_log::error!("plugin preparation failed: {error:#}");
                entries.push_str(&format!("- Plugin `{}` is unavailable this turn because its files or servers could not be prepared.\n", plugin.name));
            }
        }
    }
    if !callable_plugins.is_empty() {
        definitions.push(plugin_call_definition());
    }
    Ok(Extensions {
        definitions,
        entries,
        #[cfg(feature = "graphs")]
        child_entries,
        #[cfg(feature = "graphs")]
        child_tools: Arc::new(child_tools),
        callable_plugins,
    })
}

pub(crate) async fn run_one_turn(
    state: &SessionState,
    request: SubmitSessionTurnRequest,
    progress: Option<mpsc::Sender<SessionStreamEvent>>,
    automatic: bool,
    lock: &Arc<Mutex<()>>,
    mut stop: Option<watch::Receiver<bool>>,
    #[cfg(feature = "graphs")] peer: Option<PeerRun>,
) -> Result<SessionDetail, TurnError> {
    let started = Instant::now();
    let _workspace_id = parse_id(&request.workspace_id)?;
    if !state.workspaces.contains(&request.workspace_id)? {
        return Err(TurnError::NotFound);
    }
    let ValidatedRequest {
        provider,
        mode,
        reasoning_effort,
    } = validate_request(&request)?;
    validate_graph_selection(
        &state.db,
        &request.graph_id,
        &request.graph_models,
        &request.graph_node_concurrency,
        &request.session_id,
        &request.workspace_id,
    )?;
    #[cfg(feature = "graphs")]
    if !request.graph_id.is_empty()
        && request.graph_models.iter().any(|model| {
            Some(model.node_id.as_str())
                == beale_graphs::entry_role(&request.graph_id).map(|role| role.id())
                && (model.provider != request.provider
                    || model.method != request.method
                    || model.model != request.model)
        })
    {
        return Err(TurnError::Invalid);
    }
    let graph_config = encoded_graph_config(&request);
    #[cfg(not(feature = "goal-mode"))]
    if request.goal_mode {
        return Err(TurnError::Invalid);
    }
    #[cfg(not(feature = "goal-mode"))]
    let _ = automatic;
    let (mut session, mut turns) = if request.session_id.is_empty() {
        let id = Uuid::new_v4();
        register_session_lock(state, &id.to_string(), lock).await;
        let created_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| TurnError::Internal)?
            .as_millis();
        (
            StoredSession {
                id: id.to_string(),
                workspace_id: request.workspace_id.clone(),
                title: title(&request.prompt),
                created_at_ms,
                last_message_at_ms: u64::try_from(created_at_ms)
                    .map_err(|_| TurnError::Internal)?,
            },
            Vec::new(),
        )
    } else {
        let id = parse_id(&request.session_id)?;
        let (session, turns) = load_persisted(&state.db, id)?;
        if session.workspace_id != request.workspace_id {
            return Err(TurnError::Conflict);
        }
        (session, turns)
    };
    let next_turn_number = turns.last().map_or(Ok(1), |turn| {
        turn.number.checked_add(1).ok_or(TurnError::Conflict)
    })?;
    #[cfg(feature = "graphs")]
    let next_turn_number = peer
        .as_ref()
        .map_or(next_turn_number, |peer| peer.turn_number);
    let persisted_goal = saved_goal(&state.db, &session.id).map_err(|_| TurnError::Internal)?;
    #[cfg(feature = "goal-mode")]
    let mut goal = persisted_goal;
    #[cfg(all(feature = "graphs", feature = "goal-mode"))]
    if peer.as_ref().is_some_and(|peer| {
        !matches!(
            peer.role,
            beale_graphs::Role::AuditTargets
                | beale_graphs::Role::Discovery
                | beale_graphs::Role::DiscoveryHarness
        )
    }) {
        goal = None;
    }
    #[cfg(not(feature = "goal-mode"))]
    let goal = persisted_goal;
    if let Some(saved) = &goal
        && (saved.turns_used < 0
            || saved.active_ms < 0
            || saved.max_turns < 0
            || saved.max_duration_minutes < 0
            || !matches!(
                GoalStatus::try_from(saved.status),
                Ok(GoalStatus::Active
                    | GoalStatus::Complete
                    | GoalStatus::Waiting
                    | GoalStatus::Paused)
            ))
    {
        return Err(TurnError::Internal);
    }
    #[cfg(feature = "goal-mode")]
    {
        let feature_enabled = state
            .db
            .feature_enabled(beale_goals::FEATURE_ID)
            .map_err(|_| TurnError::Internal)?
            && state
                .db
                .extension_applies_to("feature", beale_goals::FEATURE_ID, &session.workspace_id)
                .map_err(|_| TurnError::Internal)?;
        if request.goal_mode && !feature_enabled {
            return Err(TurnError::Conflict);
        }
        if automatic && !feature_enabled {
            state
                .db
                .set_goal_status(&session.id, GoalStatus::Paused as i32)
                .map_err(|_| TurnError::Internal)?;
            return Ok(detail(&state.db, &session, &turns)?);
        }
        if !feature_enabled
            && let Some(goal) = goal.as_mut()
            && goal.status == GoalStatus::Active as i32
        {
            goal.status = GoalStatus::Paused as i32;
        }
        if request.goal_mode && !turns.is_empty() {
            return Err(TurnError::Invalid);
        }
        if request.goal_mode && turns.is_empty() {
            goal = Some(GoalRow {
                session_id: session.id.clone(),
                objective: request.prompt.clone(),
                status: GoalStatus::Active as i32,
                turns_used: 0,
                required_state: String::new(),
                max_turns: request.goal_max_turns as i64,
                max_duration_minutes: request.goal_max_duration_minutes as i64,
                active_ms: 0,
            });
        } else if !automatic
            && feature_enabled
            && let Some(goal) = goal.as_mut()
            && matches!(
                GoalStatus::try_from(goal.status),
                Ok(GoalStatus::Waiting | GoalStatus::Paused)
            )
        {
            let limits = beale_goals::Limits {
                max_turns: goal.max_turns as u64,
                max_duration_minutes: goal.max_duration_minutes as u64,
            };
            if !limits.reached(goal.turns_used as u64, goal.active_ms as u64) {
                goal.status = GoalStatus::Active as i32;
                goal.required_state.clear();
            }
        }
    }
    let goal_active = goal
        .as_ref()
        .is_some_and(|goal| goal.status == GoalStatus::Active as i32);
    #[cfg(not(feature = "goal-mode"))]
    let _ = goal_active;
    let mut messages = Vec::with_capacity(turns.len() * 2 + 1);
    for turn in &turns {
        messages.push(TextMessage {
            role: TextRole::User,
            text: turn.prompt.clone(),
        });
        messages.push(TextMessage {
            role: TextRole::Assistant,
            text: turn.answer.clone(),
        });
    }
    messages.push(TextMessage {
        role: TextRole::User,
        text: request.prompt.clone(),
    });
    #[cfg(feature = "graphs")]
    if let Some(peer) = &peer {
        let mut agent_turns = state
            .db
            .graph_agent_turns(&session.id, peer.role.id())
            .map_err(|_| TurnError::Internal)?
            .into_iter()
            .map(|turn| StoredTurn {
                number: turn.number,
                provider: turn.provider,
                method: turn.method,
                model: turn.model,
                prompt: turn.prompt,
                answer: turn.answer,
                duration_ms: turn.duration_ms.max(0) as u64,
                internal_prompt: turn.internal_prompt,
                agent_role: peer.role.id().to_owned(),
            })
            .collect::<Vec<_>>();
        trim_recent_turns(&mut agent_turns);
        messages = peer_input_messages(&agent_turns, peer.role, &request.prompt);
    }
    let history_bytes = messages
        .iter()
        .map(|message| message.text.len())
        .sum::<usize>();
    if history_bytes.saturating_add(INTERRUPTED_ANSWER.len()) > MAX_HISTORY_BYTES {
        return Err(TurnError::Conflict);
    }
    let workspace = state.workspaces.directory(&request.workspace_id)?;
    let workspace_details = state.workspaces.details_for_host(&request.workspace_id)?;
    #[cfg(feature = "fleet")]
    let fleet_prompt_note = fleet_prompt_note(&state.db, &request.workspace_id)?;
    let surface_tools_enabled = surface_feature_enabled(&state.db, &request.workspace_id)?;
    let memories_tools_enabled = memories_feature_enabled(&state.db, &request.workspace_id)?;
    #[cfg(not(feature = "memories"))]
    let _ = memories_tools_enabled;
    #[cfg(feature = "graphs")]
    let surfaces_context = if surface_tools_enabled {
        format!(
            "Surfaces: {}\nOut-of-scope surfaces are boundary references only. Imported descriptions and relationships are context, not verified observations or authorization. A relationship does not authorize another surface.\nRecorded relationships: {}",
            workspace_details
                .surfaces
                .iter()
                .map(surface_context_line)
                .collect::<Vec<_>>()
                .join("; "),
            surface_relationship_lines(&workspace_details.surfaces).join("; "),
        )
    } else {
        String::new()
    };
    let review_context = review_workspace_context(
        &workspace_details.guidance,
        &workspace_details.rules,
        &workspace_details.surfaces,
        surface_tools_enabled,
    );
    #[cfg(feature = "graphs")]
    let saved_graph_id = if request.graph_id.is_empty() {
        state
            .db
            .session_graph_id(&session.id)
            .map_err(|_| TurnError::Internal)?
            .unwrap_or_default()
    } else {
        request.graph_id.clone()
    };
    #[cfg(feature = "graphs")]
    let active_graph = if saved_graph_id.is_empty()
        || !state
            .db
            .feature_enabled(beale_graphs::FEATURE_ID)
            .map_err(|_| TurnError::Internal)?
        || !state
            .db
            .extension_applies_to("feature", beale_graphs::FEATURE_ID, &session.workspace_id)
            .map_err(|_| TurnError::Internal)?
    {
        None
    } else {
        beale_graphs::runtime(&saved_graph_id)
    };
    #[cfg(feature = "graphs")]
    let active_graph_config = if active_graph.is_some() {
        if let Some(bytes) = &graph_config {
            SessionGraphConfig::decode(bytes.as_slice()).map_err(|_| TurnError::Internal)?
        } else {
            state
                .db
                .session_graph_config(&session.id)
                .map_err(|_| TurnError::Internal)?
                .filter(|bytes| !bytes.is_empty())
                .map(|bytes| SessionGraphConfig::decode(bytes.as_slice()))
                .transpose()
                .map_err(|_| TurnError::Internal)?
                .unwrap_or_default()
        }
    } else {
        SessionGraphConfig::default()
    };
    #[cfg(feature = "graphs")]
    let graph_fallback = GraphNodeModel {
        node_id: beale_graphs::Role::Main.id().to_owned(),
        provider: request.provider,
        method: request.method,
        model: request.model.clone(),
    };
    #[cfg(feature = "graphs")]
    let graph_section =
        active_graph.map(|runtime| runtime.system_section(&active_graph_config, &graph_fallback));
    let mut input = input_messages(&messages);
    let reuse_prior_context = turns.last().is_some_and(|turn| {
        turn.provider == request.provider
            && turn.method == request.method
            && turn.model == request.model
            && !turn.answer.starts_with(INTERRUPTED_ANSWER)
    });
    #[cfg(feature = "graphs")]
    let reuse_prior_context = peer.is_some() || (active_graph.is_none() && reuse_prior_context);
    #[cfg(feature = "graphs")]
    let saved_context = if let Some(peer) = &peer {
        state.db.graph_provider_context(&session.id, peer.role.id())
    } else {
        state.db.provider_context(&session.id)
    }
    .map_err(|_| TurnError::Internal)?;
    #[cfg(not(feature = "graphs"))]
    let saved_context = state
        .db
        .provider_context(&session.id)
        .map_err(|_| TurnError::Internal)?;
    let reuse_saved_context = provider == Provider::OpenAi && reuse_prior_context;
    #[cfg(feature = "graphs")]
    let reuse_saved_context = reuse_saved_context || peer.is_some();
    if reuse_saved_context && let Some(saved) = saved_context {
        let mut previous: Vec<Value> =
            serde_json::from_str(&saved).map_err(|_| TurnError::Internal)?;
        if provider == Provider::OpenAi {
            close_unanswered_openai_calls(&mut previous);
        }
        if provider != Provider::OpenAi {
            previous.retain(|item| item.get("role").and_then(Value::as_str) != Some("system"));
        }
        previous.push(json!({"role":"user","content":request.prompt}));
        input = previous;
    }
    #[cfg(feature = "goal-mode")]
    let goal_section = goal.as_ref().filter(|_| goal_active).map(|goal| {
        beale_goals::system_section(
            beale_goals::Limits {
                max_turns: goal.max_turns as u64,
                max_duration_minutes: goal.max_duration_minutes as u64,
            },
            goal.turns_used as u64,
            goal.active_ms as u64,
        )
    });
    let agent_files = Arc::new(AgentFiles::new().map_err(|error| {
        beale_host_log::error!("agent files failed: {error:#}");
        TurnError::Internal
    })?);
    let redactor = Arc::new(tool_runtime::host_redactor(state));
    // Graph sessions add or remove role-owned tools after assembly.
    #[cfg_attr(not(feature = "graphs"), allow(unused_mut))]
    let Extensions {
        mut definitions,
        entries: extension_entries,
        #[cfg(feature = "graphs")]
            child_entries: child_extension_entries,
        #[cfg(feature = "graphs")]
            child_tools: child_feature_tools,
        callable_plugins,
    } = assemble_extensions(
        state,
        &request.workspace_id,
        &agent_files,
        &DynamicFeatures {
            #[cfg(feature = "fleet")]
            fleet_note: fleet_prompt_note.as_deref(),
            #[cfg(feature = "goal-mode")]
            goal_active,
            #[cfg(feature = "goal-mode")]
            goal_section: goal_section.as_deref(),
            #[cfg(feature = "graphs")]
            graph: active_graph,
            #[cfg(feature = "graphs")]
            graph_section: graph_section.as_deref(),
            _marker: std::marker::PhantomData,
        },
    )
    .await?;
    let catalog_section =
        system_prompt::extension_catalog(&agent_files, "EXTENSIONS.md", &extension_entries)
            .map_err(|_| TurnError::Internal)?;
    #[cfg(feature = "memories")]
    let memory_section = if memories_tools_enabled {
        match beale_memories::MemoryStore::open(state.workspaces.root())
            .and_then(|store| store.recall(&request.workspace_id, &request.prompt))
        {
            Ok(value) => value,
            Err(error) => {
                beale_host_log::error!("memory recall failed: {error:#}");
                String::new()
            }
        }
    } else {
        String::new()
    };
    #[cfg(not(feature = "memories"))]
    let memory_section = String::new();
    let sections = [catalog_section.as_str(), memory_section.as_str()];
    let system = system_prompt::load(state.workspaces.root(), &sections).map_err(|error| {
        beale_host_log::error!("session system prompt failed: {error:#}");
        TurnError::Internal
    })?;
    #[cfg(feature = "graphs")]
    let child_system = if active_graph.is_some() {
        let catalog_section = system_prompt::extension_catalog(
            &agent_files,
            "subagents/EXTENSIONS.md",
            &child_extension_entries,
        )
        .map_err(|_| TurnError::Internal)?;
        system_prompt::load(
            state.workspaces.root(),
            &[catalog_section.as_str(), memory_section.as_str()],
        )
        .map_err(|error| {
            beale_host_log::error!("subagent system prompt failed: {error:#}");
            TurnError::Internal
        })?
    } else {
        String::new()
    };
    #[cfg(feature = "graphs")]
    let requested_role = requested_top_level_role(
        active_graph.and_then(|_| beale_graphs::graph_definition(&saved_graph_id)),
        &request.agent_role,
    )?;
    #[cfg(not(feature = "graphs"))]
    if !request.agent_role.is_empty() {
        return Err(TurnError::Invalid);
    }
    #[cfg(feature = "graphs")]
    let previous_role = if active_graph.is_some() && !turns.is_empty() {
        state
            .db
            .session_last_top_level_role(&session.id)
            .map_err(|_| TurnError::Internal)?
    } else {
        None
    };
    #[cfg(feature = "graphs")]
    let mut active_role = if active_graph.is_some() && !turns.is_empty() {
        requested_role
            .or_else(|| {
                previous_role
                    .as_deref()
                    .and_then(beale_graphs::Role::from_id)
                    .filter(|role| {
                        beale_graphs::graph_definition(&saved_graph_id)
                            .is_some_and(|graph| graph.nodes.contains(role))
                    })
            })
            .or_else(|| beale_graphs::entry_role(&saved_graph_id))
            .unwrap_or(beale_graphs::Role::Main)
    } else {
        requested_role
            .or_else(|| active_graph.and_then(|_| beale_graphs::entry_role(&saved_graph_id)))
            .unwrap_or(beale_graphs::Role::Main)
    };
    #[cfg(feature = "graphs")]
    if let Some(peer) = &peer {
        active_role = peer.role;
    }
    #[cfg(all(feature = "graphs", feature = "findings"))]
    if peer.is_some() && beale_graphs::is_pipeline_graph(&saved_graph_id) {
        definitions.retain(|tool| {
            let name = tool["name"].as_str().unwrap_or_default();
            !beale_findings::is_tool(name)
                || tool_runtime::pipeline_finding_visible(active_role, name)
        });
    }
    #[cfg(feature = "graphs")]
    let mut active_model = if active_graph.is_some() {
        beale_graphs::top_level_model(&active_graph_config, active_role, &graph_fallback)
    } else {
        graph_fallback.clone()
    };
    #[cfg(not(feature = "graphs"))]
    let active_model = GraphNodeModel {
        node_id: "main".to_owned(),
        provider: request.provider,
        method: request.method,
        model: request.model.clone(),
    };
    // Graph sessions resolve the active role's provider; the request's own
    // provider and method were validated above for the entry role.
    #[cfg(feature = "graphs")]
    let _ = (provider, mode);
    #[cfg(feature = "graphs")]
    let (mut provider, mut mode) = graphs::provider_for(&active_model).ok_or(TurnError::Invalid)?;
    #[cfg(feature = "graphs")]
    if let Some(tool) = beale_graphs::handoff_definition(&saved_graph_id, active_role) {
        definitions.push(tool);
    }
    #[cfg(feature = "graphs")]
    let base_system = system.clone();
    #[allow(unused_mut)]
    let mut system = system;
    #[cfg(feature = "graphs")]
    if active_graph.is_some() {
        system.push_str(&format!(
            "\n\n## Active agent: {}\n\n{}",
            active_role.label(),
            active_role.instruction()
        ));
    }
    #[cfg(feature = "graphs")]
    let system_before_stage_reminder = system.clone();
    if provider != Provider::OpenAi {
        input.insert(0, json!({"role":"system","content":system}));
    }
    let mut openai_connection = OpenAiConnection::default();
    #[cfg(feature = "goal-mode")]
    let mut goal_signal: Option<beale_goals::Signal> = None;
    let mut tool_rows = Vec::<ToolCallRow>::new();
    #[cfg(feature = "graphs")]
    let mut child_turns = Vec::<ChildTurnRow>::new();
    #[cfg(feature = "graphs")]
    let child_message_sequence = Arc::new(AtomicU32::new(0x8000_0000));
    #[cfg(feature = "graphs")]
    let mut subagents = HashMap::<beale_graphs::Role, graphs::Subagents>::new();
    #[cfg(feature = "graphs")]
    let child_usage = Arc::new(Mutex::new(Vec::<ModelUsageRow>::new()));
    #[cfg(not(feature = "graphs"))]
    let child_turns = Vec::<ChildTurnRow>::new();
    let mut reasoning_rows = Vec::<ReasoningSummaryRow>::new();
    let mut commentary_rows = Vec::<CommentaryRow>::new();
    let mut model_usage = Vec::<ModelUsageRow>::new();
    #[cfg(feature = "graphs")]
    let mut peer_messages = Vec::<String>::new();
    let answer: Option<String>;
    let mut segment_index = 0_u32;
    #[cfg(feature = "graphs")]
    let mut stage_reminder_sent = false;
    'rounds: loop {
        #[cfg(feature = "graphs")]
        if let Some(manager) = subagents.get(&active_role) {
            let incoming = manager.parent_messages().await;
            if !incoming.is_empty() {
                for (from, message) in &incoming {
                    if let Some(from_role) = manager.role_of(from).await {
                        tool_rows.push(
                            graphs::received_message(
                                from,
                                from_role,
                                message,
                                graphs::ReceiveContext {
                                    agent_role: active_role,
                                    subagent_id: "",
                                    turn_number: next_turn_number,
                                    ordinal: tool_rows.len() as i64,
                                    progress: progress.as_ref(),
                                },
                            )
                            .await,
                        );
                    }
                }
                input.extend(input_messages(&[TextMessage {
                    role: TextRole::User,
                    text: format!(
                        "Messages from your subagents:\n{}",
                        incoming
                            .iter()
                            .map(|(from, message)| format!("From {from}: {message}"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    ),
                }]));
            }
        }
        #[cfg(feature = "graphs")]
        if !stage_reminder_sent
            && peer
                .as_ref()
                .and_then(|peer| peer.soft_reminder.as_ref())
                .is_some_and(|receiver| *receiver.borrow())
        {
            system.push_str("\n\n## Sequential stage deadline\n\nYour stage time has ended. Finish the current action, summarize completed work and evidence, and yield. Leave remaining queued work for the next cycle.");
            if provider != Provider::OpenAi {
                input[0]["content"] = json!(system);
            }
            stage_reminder_sent = true;
        }
        // Reserve one stream segment for the answer and one for each reasoning part.
        segment_index = segment_index.saturating_add(17);
        if stop_requested(&stop) {
            answer = Some(STOPPED_ANSWER.to_owned());
            break;
        }
        let (provider_tx, forward) = if let Some(client_tx) = progress.clone() {
            let (provider_tx, mut provider_rx) = mpsc::channel::<ProviderProgress>(32);
            let stream_role = active_model.node_id.clone();
            let forward = tokio::spawn(async move {
                let mut reasoning_segments = std::collections::BTreeMap::new();
                while let Some(update) = provider_rx.recv().await {
                    let (kind, text, item_segment) = match update {
                        ProviderProgress::MessageDelta(text) => {
                            (SessionStreamKind::MessageDelta, text, segment_index)
                        }
                        ProviderProgress::ReasoningDelta(text, key) => {
                            let next = reasoning_segments.len() as u32 + 1;
                            let part = *reasoning_segments.entry(key).or_insert(next);
                            (
                                SessionStreamKind::ReasoningDelta,
                                text,
                                segment_index.saturating_add(part),
                            )
                        }
                    };
                    if client_tx
                        .send(SessionStreamEvent {
                            runbook_progress: None,
                            kind: kind as i32,
                            text,
                            tool: None,
                            detail: None,
                            segment_index: item_segment,
                            agent_role: stream_role.clone(),
                            tool_detail: None,
                            usage_delta: None,
                            subagent_id: String::new(),
                            subagent: None,
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });
            (Some(provider_tx), Some(forward))
        } else {
            (None, None)
        };
        let call_started = Instant::now();
        let provider_call = async {
            for attempt in 0..2 {
                let step_request = ToolStepRequest {
                    model: &active_model.model,
                    input: &input,
                    tools: &definitions,
                    instructions: (provider == Provider::OpenAi).then_some(system.as_str()),
                    reasoning_effort,
                    workspace_root: Some(&workspace),
                };
                let result = if provider == Provider::OpenAi {
                    state
                        .provider
                        .openai_tool_step(
                            mode,
                            step_request,
                            &state.credentials,
                            provider_tx.as_ref(),
                            &mut openai_connection,
                        )
                        .await
                } else {
                    state
                        .provider
                        .tool_step(
                            provider,
                            mode,
                            step_request,
                            &state.credentials,
                            provider_tx.as_ref(),
                        )
                        .await
                };
                if let Err(error) = &result
                    && attempt == 0
                    && provider_error_retryable(error)
                {
                    beale_host_log::error!(
                        "transient session provider failure; retrying once: {error:#}"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    continue;
                }
                return result;
            }
            unreachable!("provider step returns after its second attempt")
        };
        let step = until_stopped(provider_call, &mut stop).await;
        drop(provider_tx);
        if let Some(forward) = forward {
            let _ = forward.await;
        }
        let Some(step) = step else {
            answer = Some(STOPPED_ANSWER.to_owned());
            break;
        };
        let step = match step {
            Ok(step) => step,
            Err(error) => {
                beale_host_log::error!("session provider turn failed: {error:#}");
                if tool_rows.is_empty() {
                    if let Some(tx) = &progress {
                        let _ = tx
                            .send(SessionStreamEvent {
                                runbook_progress: None,
                                kind: SessionStreamKind::Failed as i32,
                                text: provider_error_message(provider, mode, &error),
                                tool: None,
                                detail: None,
                                segment_index: 0,
                                agent_role: active_model.node_id.clone(),
                                tool_detail: None,
                                usage_delta: None,
                                subagent_id: String::new(),
                                subagent: None,
                            })
                            .await;
                    }
                    return Err(TurnError::Provider);
                }
                answer = Some(format!(
                    "{INTERRUPTED_ANSWER} {}",
                    provider_error_message(provider, mode, &error)
                ));
                break;
            }
        };
        if stop_requested(&stop) {
            answer = Some(STOPPED_ANSWER.to_owned());
            break;
        }
        model_usage.push(ModelUsageRow {
            agent_id: active_model.node_id.clone(),
            role: active_model.node_id.clone(),
            provider: active_model.provider,
            method: active_model.method,
            model: active_model.model.clone(),
            input_tokens: step.usage.map(|usage| usage.input_tokens as i64),
            output_tokens: step.usage.map(|usage| usage.output_tokens as i64),
            cache_read_tokens: step
                .usage
                .and_then(|usage| usage.cache_read_tokens.map(|count| count as i64)),
            work_elapsed_ms: Some(started.elapsed().as_millis().min(i64::MAX as u128) as i64),
            duration_ms: call_started.elapsed().as_millis().min(i64::MAX as u128) as i64,
            observed_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| {
                    duration.as_millis().min(i64::MAX as u128) as i64
                }),
        });
        if let Some(tx) = &progress
            && let Some(item) = model_usage.last()
        {
            let _ = tx.send(usage_update(item, &active_model.node_id)).await;
        }
        for summary in step.reasoning_summaries {
            if summary.trim().is_empty() {
                continue;
            }
            reasoning_rows.push(ReasoningSummaryRow {
                turn_number: next_turn_number,
                ordinal: reasoning_rows.len() as i64,
                after_tool_count: tool_rows.len() as i64,
                text: summary,
                agent_role: active_model.node_id.clone(),
            });
        }
        let prior_input_len = input.len();
        input.extend(step.continuation);
        if step.calls.is_empty() {
            answer = Some(step.text);
            break;
        }
        if !step.text.trim().is_empty() {
            let text = step.text;
            if !text.trim().is_empty() {
                commentary_rows.push(CommentaryRow {
                    turn_number: next_turn_number,
                    ordinal: commentary_rows.len() as i64,
                    after_tool_count: tool_rows.len() as i64,
                    text,
                    agent_role: active_model.node_id.clone(),
                });
            }
        }
        let mut anthropic_tool_results = Vec::new();
        #[cfg(feature = "graphs")]
        let mut pending_handoff: Option<(beale_graphs::Role, String)> = None;
        for call in step.calls {
            if stop_requested(&stop) {
                input.truncate(prior_input_len);
                answer = Some(STOPPED_ANSWER.to_owned());
                break 'rounds;
            }
            if call.id.is_empty()
                || call.id.len() > 256
                || call.name.len() > 128
                || call.arguments.len() > 65_536
            {
                if tool_rows.is_empty() {
                    return Err(TurnError::Provider);
                }
                answer = Some(format!(
                    "{INTERRUPTED_ANSWER} The provider returned an invalid tool call."
                ));
                break 'rounds;
            }
            let id = Uuid::new_v4().to_string();
            let args: Result<Value, _> = serde_json::from_str(&call.arguments);
            let summary = if call.name == "goal_status" {
                goal_status_summary(args.as_ref().unwrap_or(&Value::Null))
            } else if cfg!(feature = "agent-feedback") && call.name == "agent_feedback" {
                "Record agent feedback".to_owned()
            } else if cfg!(feature = "surfaces") && call.name.starts_with("surface_") {
                #[cfg(feature = "surfaces")]
                {
                    beale_surfaces::summary(
                        &state.workspaces,
                        &request.workspace_id,
                        &call.name,
                        args.as_ref().unwrap_or(&Value::Null),
                    )
                }
                #[cfg(not(feature = "surfaces"))]
                {
                    base_tool_summary(&call.name, args.as_ref().unwrap_or(&Value::Null))
                }
            } else if cfg!(feature = "findings") && call.name.starts_with("findings_") {
                #[cfg(feature = "findings")]
                {
                    beale_findings::summary(
                        &state.db,
                        &request.workspace_id,
                        &call.name,
                        args.as_ref().unwrap_or(&Value::Null),
                    )
                }
                #[cfg(not(feature = "findings"))]
                {
                    base_tool_summary(&call.name, args.as_ref().unwrap_or(&Value::Null))
                }
            } else if cfg!(feature = "memories") && call.name.starts_with("memory_") {
                #[cfg(feature = "memories")]
                {
                    beale_memories::summary(
                        state.workspaces.root(),
                        &request.workspace_id,
                        &call.name,
                        args.as_ref().unwrap_or(&Value::Null),
                    )
                }
                #[cfg(not(feature = "memories"))]
                {
                    base_tool_summary(&call.name, args.as_ref().unwrap_or(&Value::Null))
                }
            } else if cfg!(feature = "runbooks") && call.name.starts_with("runbook_") {
                #[cfg(feature = "runbooks")]
                {
                    beale_runbooks::summary(
                        &state.db,
                        &request.workspace_id,
                        &session.id,
                        &call.name,
                        args.as_ref().unwrap_or(&Value::Null),
                    )
                }
                #[cfg(not(feature = "runbooks"))]
                {
                    base_tool_summary(&call.name, args.as_ref().unwrap_or(&Value::Null))
                }
            } else if cfg!(feature = "graphs") && call.name.starts_with("graph_") {
                #[cfg(feature = "graphs")]
                {
                    graph_tool_summary(
                        state,
                        &session.id,
                        active_role,
                        &subagents,
                        &call.name,
                        args.as_ref().unwrap_or(&Value::Null),
                    )
                    .await
                }
                #[cfg(not(feature = "graphs"))]
                {
                    base_tool_summary(&call.name, args.as_ref().unwrap_or(&Value::Null))
                }
            } else {
                tool_summary(&call.name, args.as_ref().unwrap_or(&Value::Null))
            };
            #[cfg(feature = "graphs")]
            let summary = if peer.is_none()
                && active_graph.is_some()
                && matches!(
                    saved_graph_id.as_str(),
                    beale_graphs::CLOUDFLARE_ID
                        | beale_graphs::OPENAI_ID
                        | beale_graphs::SEQUENTIAL_ID
                ) {
                format!("{}: {summary}", active_role.label())
            } else {
                summary
            };
            if let Some(tx) = &progress {
                let _ = tx
                    .send(SessionStreamEvent {
                        runbook_progress: None,
                        kind: SessionStreamKind::ToolStarted as i32,
                        text: String::new(),
                        tool: Some(SessionToolSummary {
                            id: id.clone(),
                            name: call.name.clone(),
                            summary: summary.clone(),
                            is_error: false,
                            agent_role: active_model.node_id.clone(),
                        }),
                        detail: None,
                        segment_index: 0,
                        agent_role: active_model.node_id.clone(),
                        tool_detail: Some(SessionToolDetailPage {
                            id: id.clone(),
                            name: call.name.clone(),
                            arguments: call.arguments.clone(),
                            output: String::new(),
                            offset: 0,
                            next_offset: 0,
                            total_bytes: 0,
                            is_error: false,
                        }),
                        usage_delta: None,
                        subagent_id: String::new(),
                        subagent: None,
                    })
                    .await;
            }
            let runtime = tool_runtime::ToolRuntime {
                state,
                workspace_id: &request.workspace_id,
                session_id: &session.id,
                agent_id: &active_model.node_id,
                agent_role: &active_model.node_id,
                workspace: &workspace,
                review: tool_runtime::ReviewTarget {
                    provider,
                    mode,
                    model: &active_model.model,
                    context: &review_context,
                },
                definitions: &definitions,
                callable_plugins: &callable_plugins,
                redactor: &redactor,
                mutation_lock: None,
                progress: progress.as_ref(),
                #[cfg(feature = "graphs")]
                pipeline_role: (peer.is_some() && beale_graphs::is_pipeline_graph(&saved_graph_id))
                    .then_some(active_role),
            };
            let gate = runtime
                .gate(&call.name, args.as_ref().ok(), &mut stop)
                .await;
            let review_unavailable = std::cell::Cell::new(false);
            let (output, is_error) = match gate {
                tool_runtime::Gate::Run { reviewed } => match args {
                    #[cfg(feature = "goal-mode")]
                    Ok(args) if call.name == beale_goals::TOOL_NAME && goal_active => {
                        if goal_signal.is_some() {
                            (
                                "Goal status was already recorded for this turn.".to_owned(),
                                true,
                            )
                        } else {
                            match beale_goals::parse_signal(&args) {
                                Ok(signal) => {
                                    goal_signal = Some(signal);
                                    ("Goal status recorded.".to_owned(), false)
                                }
                                Err(error) => (format!("Invalid goal status: {error}"), true),
                            }
                        }
                    }
                    #[cfg(feature = "graphs")]
                    Ok(args)
                        if call.name == beale_graphs::HANDOFF_TOOL && active_graph.is_some() =>
                    {
                        if peer.is_none() && pending_handoff.is_some() {
                            (
                                "Finish the pending handoff before adding another.".to_owned(),
                                true,
                            )
                        } else {
                            match beale_graphs::handoff_target(&saved_graph_id, active_role, &args)
                            {
                                Ok((role, task)) => {
                                    if let Some(peer) = &peer {
                                        let item = beale_graphs::GraphHandoffRow {
                                            id: Uuid::new_v4().to_string(),
                                            from_role: active_role.id().to_owned(),
                                            to_role: role.id().to_owned(),
                                            task,
                                            status: "pending".to_owned(),
                                        };
                                        let now = SystemTime::now()
                                            .duration_since(UNIX_EPOCH)
                                            .map_or(0, |time| {
                                                time.as_millis().min(i64::MAX as u128) as i64
                                            });
                                        match state.db.enqueue_graph_handoff(
                                            &session.id,
                                            &item,
                                            now,
                                        ) {
                                            Ok(()) => {
                                                let _ = peer.notify.send(());
                                                (
                                                    format!(
                                                        "Queued a handoff to {}.",
                                                        role.label()
                                                    ),
                                                    false,
                                                )
                                            }
                                            Err(error) => (
                                                format!("Handoff could not be saved: {error}"),
                                                true,
                                            ),
                                        }
                                    } else {
                                        pending_handoff = Some((role, task));
                                        (format!("Handing off to {}.", role.label()), false)
                                    }
                                }
                                Err(error) => (format!("Invalid handoff: {error}"), true),
                            }
                        }
                    }
                    #[cfg(feature = "graphs")]
                    Ok(args) if call.name == beale_graphs::SPAWN_TOOL && active_graph.is_some() => {
                        let reservation = active_graph
                            .ok_or_else(|| "Graph runtime is unavailable.".to_owned())
                            .and_then(|runtime| {
                                validate_delegations(
                                    runtime,
                                    &args,
                                    &active_graph_config,
                                    &graph_fallback,
                                )
                            });
                        if let Err(error) = reservation {
                            (error, true)
                        } else {
                            let assignments = active_graph
                                .and_then(|runtime| {
                                    runtime
                                        .assignments(&args, &active_graph_config, &graph_fallback)
                                        .ok()
                                })
                                .unwrap_or_default();
                            let manager = subagents
                                .entry(active_role)
                                .or_insert_with(|| graphs::Subagents::new(&active_graph_config));
                            let output = manager
                                .spawn(
                                    assignments,
                                    graphs::ChildContext {
                                        parent_role: active_role,
                                        state: state.clone(),
                                        workspace: workspace.clone(),
                                        agent_files: Arc::clone(&agent_files),
                                        workspace_id: request.workspace_id.clone(),
                                        session_id: session.id.clone(),
                                        redactor: Arc::clone(&redactor),
                                        feature_tools: Arc::clone(&child_feature_tools),
                                        workspace_context: review_context.clone(),
                                        surfaces_context: surfaces_context.clone(),
                                        base_system: child_system.clone(),
                                        reasoning_effort: reasoning_effort.map(str::to_owned),
                                        turn_number: next_turn_number,
                                        turn_started: started,
                                        progress: progress.clone(),
                                        message_sequence: Arc::clone(&child_message_sequence),
                                        model_usage: Arc::clone(&child_usage),
                                    },
                                )
                                .await;
                            match output {
                                Ok(output) => (output, false),
                                Err(error) => (error.to_owned(), true),
                            }
                        }
                    }
                    #[cfg(feature = "graphs")]
                    Ok(args) if call.name == beale_graphs::LIST_TOOL && active_graph.is_some() => {
                        if !args.as_object().is_some_and(serde_json::Map::is_empty) {
                            ("Invalid subagent list arguments.".to_owned(), true)
                        } else {
                            let manager = subagents
                                .entry(active_role)
                                .or_insert_with(|| graphs::Subagents::new(&active_graph_config));
                            (manager.list().await, false)
                        }
                    }
                    #[cfg(feature = "graphs")]
                    Ok(args)
                        if call.name == beale_graphs::INSPECT_TOOL && active_graph.is_some() =>
                    {
                        match graphs::InspectRequest::parse(&args) {
                            Err(error) => (error.to_owned(), true),
                            Ok(request) => {
                                if let Some(value) = match subagents.get(&active_role) {
                                    Some(manager) => manager.inspect(&request).await,
                                    None => None,
                                } {
                                    (value, false)
                                } else {
                                    match graphs::inspect_saved(
                                        &state.db,
                                        &session.id,
                                        active_role,
                                        &request,
                                    ) {
                                        Ok(Some(value)) => (value, false),
                                        Ok(None) => (
                                            "Subagent not found for this top-level agent."
                                                .to_owned(),
                                            true,
                                        ),
                                        Err(error) => {
                                            beale_host_log::error!(
                                                "saved graph subagent inspect failed: {error:#}"
                                            );
                                            (
                                                "Subagent transcript could not be read.".to_owned(),
                                                true,
                                            )
                                        }
                                    }
                                }
                            }
                        }
                    }
                    #[cfg(feature = "graphs")]
                    Ok(args)
                        if call.name == beale_graphs::MESSAGE_TOOL && active_graph.is_some() =>
                    {
                        let manager = subagents
                            .entry(active_role)
                            .or_insert_with(|| graphs::Subagents::new(&active_graph_config));
                        match manager.message("parent", &args).await {
                            Ok(output) => (output, false),
                            Err(error) => (error.to_owned(), true),
                        }
                    }
                    #[cfg(feature = "graphs")]
                    Ok(args) if call.name == beale_graphs::KILL_TOOL && active_graph.is_some() => {
                        let manager = subagents
                            .entry(active_role)
                            .or_insert_with(|| graphs::Subagents::new(&active_graph_config));
                        match manager.kill(&args).await {
                            Ok(output) => (output, false),
                            Err(error) => (error.to_owned(), true),
                        }
                    }
                    args => {
                        let result = runtime
                            .execute(&id, &call.name, args, reviewed, &tool_rows, &mut stop)
                            .await;
                        review_unavailable.set(result.review_unavailable);
                        (result.output, result.is_error)
                    }
                },
                blocked => {
                    let result = blocked.blocked().unwrap_or_else(|| {
                        tool_runtime::ToolResult::error("Tool was not run.".to_owned())
                    });
                    review_unavailable.set(result.review_unavailable);
                    (result.output, result.is_error)
                }
            };
            let model_output = {
                if let Some(tx) = &progress {
                    let _ = tx
                        .send(SessionStreamEvent {
                            runbook_progress: None,
                            kind: SessionStreamKind::ToolFinished as i32,
                            text: String::new(),
                            tool: Some(SessionToolSummary {
                                id: id.clone(),
                                name: call.name.clone(),
                                summary: summary.clone(),
                                is_error,
                                agent_role: active_model.node_id.clone(),
                            }),
                            detail: None,
                            segment_index: 0,
                            agent_role: active_model.node_id.clone(),
                            tool_detail: Some(SessionToolDetailPage {
                                id: id.clone(),
                                name: call.name.clone(),
                                arguments: call.arguments.clone(),
                                output: page(&output, 0, TOOL_PAGE_BYTES)
                                    .map_or_else(|_| String::new(), |(first, _)| first),
                                offset: 0,
                                next_offset: page(&output, 0, TOOL_PAGE_BYTES)
                                    .map_or(0, |(_, next)| next as u64),
                                total_bytes: output.len() as u64,
                                is_error,
                            }),
                            usage_delta: None,
                            subagent_id: String::new(),
                            subagent: None,
                        })
                        .await;
                }
                tool_rows.push(ToolCallRow {
                    id: id.clone(),
                    turn_number: next_turn_number,
                    ordinal: tool_rows.len() as i64,
                    name: call.name.clone(),
                    summary,
                    arguments: call.arguments,
                    output: output.clone(),
                    is_error,
                    agent_role: active_model.node_id.clone(),
                    created_at_ms: tool_runtime::unix_ms(),
                });
                model_tool_preview(&call.name, &id, &output, is_error, &agent_files)
            };
            append_tool_result(
                &mut input,
                &mut anthropic_tool_results,
                provider,
                mode,
                &call.id,
                &model_output,
            );
            if review_unavailable.get() {
                answer = Some("Auto-Review was unavailable, so the requested tool was not run. Saved activity can be resumed after the review service is available.".to_owned());
                break 'rounds;
            }
        }
        if !anthropic_tool_results.is_empty() {
            input.push(json!({"role":"user","content":anthropic_tool_results}));
        }
        #[cfg(feature = "graphs")]
        if let Some((role, task)) = pending_handoff {
            peer_messages.push(format!(
                "{} → {}: {}",
                active_role.label(),
                role.label(),
                task
            ));
            active_role = role;
            active_model =
                beale_graphs::top_level_model(&active_graph_config, role, &graph_fallback);
            (provider, mode) = graphs::provider_for(&active_model).ok_or(TurnError::Invalid)?;
            openai_connection = OpenAiConnection::default();
            definitions.retain(|tool| tool["name"] != beale_graphs::HANDOFF_TOOL);
            if let Some(tool) = beale_graphs::handoff_definition(&saved_graph_id, role) {
                definitions.push(tool);
            }
            system = format!(
                "{base_system}\n\n## Active agent: {}\n\n{}",
                role.label(),
                role.instruction()
            );
            input = input_messages(&messages);
            input.push(json!({"role":"user","content":format!("Peer handoff to {}. Task: {}\n\nHandoffs in this turn:\n{}\n\nWork in this session. You may delegate to your own subagents and hand off to a peer when needed.", role.label(), task, peer_messages.join("\n"))}));
            if provider != Provider::OpenAi {
                input.insert(0, json!({"role":"system","content":system}));
            }
            continue 'rounds;
        }
    }
    #[cfg(feature = "graphs")]
    for (_, mut manager) in subagents {
        if stop_requested(&stop) {
            manager.abort_all().await;
        }
        let result = manager.finish().await;
        for mut tool in result.tools {
            tool.ordinal = tool_rows.len() as i64;
            tool_rows.push(tool);
        }
        child_turns.extend(result.child_turns);
    }
    #[cfg(feature = "graphs")]
    model_usage.extend(child_usage.lock().await.drain(..));
    let mut answer = answer.ok_or(TurnError::Provider)?;
    if answer.trim().is_empty() || history_bytes.saturating_add(answer.len()) > MAX_HISTORY_BYTES {
        if tool_rows.is_empty() {
            if let Some(tx) = &progress {
                let text = if answer.trim().is_empty() {
                    "The provider completed without a message or tool call. Try again."
                } else {
                    "The provider answer exceeded this session's history limit."
                };
                let _ = tx
                    .send(SessionStreamEvent {
                        runbook_progress: None,
                        kind: SessionStreamKind::Failed as i32,
                        text: text.to_owned(),
                        tool: None,
                        detail: None,
                        segment_index: 0,
                        agent_role: active_model.node_id.clone(),
                        tool_detail: None,
                        usage_delta: None,
                        subagent_id: String::new(),
                        subagent: None,
                    })
                    .await;
            }
            return Err(TurnError::Provider);
        }
        answer = if answer.trim().is_empty() {
            format!("{INTERRUPTED_ANSWER} The provider completed without a message.")
        } else {
            format!(
                "{INTERRUPTED_ANSWER} The provider answer exceeded the session history boundary."
            )
        };
    }
    #[cfg(feature = "goal-mode")]
    if let Some(goal) = goal.as_mut().filter(|_| goal_active) {
        goal.turns_used = goal.turns_used.checked_add(1).ok_or(TurnError::Conflict)?;
        goal.active_ms = goal.active_ms.saturating_add(
            i64::try_from(started.elapsed().as_millis().min(i64::MAX as u128))
                .map_err(|_| TurnError::Internal)?,
        );
        let signal = goal_signal_for_answer(&answer, goal_signal);
        match beale_goals::after_turn(
            signal,
            goal.turns_used as u64,
            goal.active_ms as u64,
            beale_goals::Limits {
                max_turns: goal.max_turns as u64,
                max_duration_minutes: goal.max_duration_minutes as u64,
            },
        ) {
            beale_goals::Outcome::Active => {}
            beale_goals::Outcome::Complete => goal.status = GoalStatus::Complete as i32,
            beale_goals::Outcome::Waiting(required) => {
                goal.status = GoalStatus::Waiting as i32;
                goal.required_state = required;
            }
            beale_goals::Outcome::Paused => goal.status = GoalStatus::Paused as i32,
        }
        if stop_requested(&stop) && goal.status == GoalStatus::Active as i32 {
            goal.status = GoalStatus::Paused as i32;
        }
    }
    turns.push(StoredTurn {
        number: next_turn_number,
        provider: active_model.provider,
        method: active_model.method,
        model: active_model.model.clone(),
        prompt: request.prompt,
        answer,
        duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        internal_prompt: automatic || {
            #[cfg(feature = "graphs")]
            {
                peer.as_ref().is_some_and(|peer| peer.handoff)
            }
            #[cfg(not(feature = "graphs"))]
            {
                false
            }
        },
        agent_role: active_model.node_id.clone(),
    });
    let repeat_preset = request.repeat_preset;
    let automation_options = AutomationOptions {
        reasoning_effort: request.reasoning_effort,
        goal_mode: request.goal_mode,
        goal_max_turns: request.goal_max_turns,
        goal_max_duration_minutes: request.goal_max_duration_minutes,
    };
    let latest = turns.last().ok_or(TurnError::Internal)?;
    #[cfg(feature = "graphs")]
    let save_peer_context = peer.is_some();
    #[cfg(not(feature = "graphs"))]
    let save_peer_context = false;
    if provider == Provider::OpenAi {
        close_unanswered_openai_calls(&mut input);
    }
    #[cfg(feature = "graphs")]
    if stage_reminder_sent && provider != Provider::OpenAi {
        input[0]["content"] = json!(system_before_stage_reminder);
    }
    let provider_context_json = if save_peer_context
        || (provider == Provider::OpenAi && !latest.answer.starts_with(INTERRUPTED_ANSWER))
    {
        let context = if provider == Provider::OpenAi {
            compact_openai_context(&input)
        } else {
            input.clone()
        };
        let encoded = serde_json::to_string(&context).map_err(|_| TurnError::Internal)?;
        (encoded.len() <= 8_388_608).then_some(encoded)
    } else {
        None
    };
    session.last_message_at_ms = u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| TurnError::Internal)?
            .as_millis(),
    )
    .map_err(|_| TurnError::Internal)?;
    let session_row = SessionRow {
        id: session.id.clone(),
        workspace_id: session.workspace_id.clone(),
        title: session.title.clone(),
        created_at_ms: i64::try_from(session.created_at_ms).map_err(|_| TurnError::Internal)?,
        last_message_at_ms: i64::try_from(session.last_message_at_ms)
            .map_err(|_| TurnError::Internal)?,
    };
    #[cfg(feature = "goal-mode")]
    let goal_commit = goal.as_ref().map(beale_goals::GoalCommit);
    #[cfg(feature = "graphs")]
    let graph_commit = beale_graphs::GraphCommit {
        selection: (!request.graph_id.is_empty()).then(|| {
            (
                request.graph_id.as_str(),
                graph_config.as_deref().unwrap_or_default(),
            )
        }),
    };
    #[cfg(feature = "memories")]
    let memory_commit = memories_tools_enabled.then_some(beale_memories::QueueMemoryJobs);
    // Which extensions apply depends on the compiled features.
    let candidates: Vec<Option<&dyn beale_storage::TurnExtension>> = vec![
        #[cfg(feature = "goal-mode")]
        goal_commit
            .as_ref()
            .map(|commit| commit as &dyn beale_storage::TurnExtension),
        #[cfg(feature = "graphs")]
        Some(&graph_commit),
        #[cfg(feature = "memories")]
        memory_commit
            .as_ref()
            .map(|commit| commit as &dyn beale_storage::TurnExtension),
    ];
    let extensions: Vec<&dyn beale_storage::TurnExtension> =
        candidates.into_iter().flatten().collect();
    #[cfg(feature = "graphs")]
    let agent_role = peer.as_ref().map(|peer| peer.role.id());
    #[cfg(not(feature = "graphs"))]
    let agent_role = None;
    #[cfg(not(feature = "graphs"))]
    let _ = &graph_config;
    state
        .db
        .append_turn_with_metadata(
            &session_row,
            &TurnRow {
                number: next_turn_number,
                provider: latest.provider,
                method: latest.method,
                model: latest.model.clone(),
                prompt: latest.prompt.clone(),
                answer: latest.answer.clone(),
                duration_ms: i64::try_from(latest.duration_ms).map_err(|_| TurnError::Internal)?,
                internal_prompt: latest.internal_prompt,
            },
            next_turn_number == 1 && {
                #[cfg(feature = "graphs")]
                {
                    peer.is_none()
                }
                #[cfg(not(feature = "graphs"))]
                {
                    true
                }
            },
            TurnActivity {
                tools: &tool_rows,
                reasoning: &reasoning_rows,
                commentary: &commentary_rows,
            },
            (!repeat_preset.is_empty()).then_some((repeat_preset.as_str(), automation_options)),
            TurnMetadata {
                provider_context_json: provider_context_json.as_deref(),
                agent_role,
                model_usage: Some(&model_usage),
                child_turns: &child_turns,
                extensions: &extensions,
            },
        )
        .map_err(|error| {
            beale_host_log::error!("session turn save failed: {error:#}");
            TurnError::Internal
        })?;
    #[cfg(feature = "graphs")]
    if peer.is_some() {
        let (saved, turns) = load_persisted(&state.db, parse_id(&session.id)?)?;
        return Ok(detail(&state.db, &saved, &turns)?);
    }
    trim_recent_turns(&mut turns);
    Ok(detail(&state.db, &session, &turns)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example_request() -> SubmitSessionTurnRequest {
        SubmitSessionTurnRequest {
            workspace_id: Uuid::new_v4().to_string(),
            prompt: "Inspect the example module".to_owned(),
            model: "example-model".to_owned(),
            provider: ProviderId::Openai as i32,
            method: AuthMethod::ApiKey as i32,
            ..Default::default()
        }
    }

    #[test]
    fn request_validation_resolves_provider_mode_and_effort() {
        let mut request = example_request();
        request.reasoning_effort = ReasoningEffort::High as i32;
        assert_eq!(
            validate_request(&request),
            Ok(ValidatedRequest {
                provider: Provider::OpenAi,
                mode: AuthMode::ApiKey,
                reasoning_effort: Some("high"),
            })
        );
    }

    #[test]
    fn request_validation_rejects_malformed_requests_before_host_reads() {
        let invalid: [fn(&mut SubmitSessionTurnRequest); 8] = [
            |request| request.prompt = "  ".to_owned(),
            |request| request.model = "example model".to_owned(),
            |request| request.provider = 99,
            |request| request.method = 99,
            |request| {
                request.provider = ProviderId::Custom as i32;
                request.method = AuthMethod::Subscription as i32;
            },
            |request| request.goal_max_turns = 3,
            |request| request.repeat_preset = "fortnight".to_owned(),
            |request| request.reasoning_effort = 99,
        ];
        for change in invalid {
            let mut request = example_request();
            change(&mut request);
            assert_eq!(validate_request(&request), Err(TurnError::Invalid));
        }
    }

    #[test]
    fn turn_errors_round_trip_through_route_status_codes() {
        for error in [
            TurnError::Invalid,
            TurnError::NotFound,
            TurnError::Conflict,
            TurnError::Provider,
            TurnError::Internal,
        ] {
            assert_eq!(TurnError::from(error.status()), error);
            assert!(!error.message().is_empty());
        }
        assert_eq!(TurnError::from(StatusCode::FORBIDDEN), TurnError::Internal);
    }
}
