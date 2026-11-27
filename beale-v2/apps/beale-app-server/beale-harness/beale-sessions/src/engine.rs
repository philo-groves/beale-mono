//! The session turn engine: request validation, context and toolset assembly,
//! the provider and tool round loop, Goal Mode continuation, and persistence.
//!
//! The engine reports [`TurnError`] rather than HTTP status codes so routes,
//! automations share one typed failure contract.
//! HTTP handlers translate errors at the route boundary.

use super::*;

struct CompletedLocalCall {
    name: String,
    arguments: String,
    output: String,
}

/// Why a turn could not run or complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TurnError {
    /// The request is malformed or names an unsupported option.
    Invalid,
    /// The workspace or session does not exist.
    NotFound,
    /// The session is busy or changed, a required feature is off, or history is full.
    Conflict,
    /// The saved configuration cannot execute in this build.
    UnsupportedSwarm,
    /// The provider failed before any activity could be saved.
    Provider,
    /// Host storage or another internal operation failed.
    Internal,
    /// Saving an executed local turn failed; never replay its side effects.
    Checkpoint,
    /// Recovery after tool activity exhausted; the partial turn is saved.
    RecoveryExhausted,
}

impl TurnError {
    pub(crate) fn status(self) -> StatusCode {
        match self {
            TurnError::Invalid => StatusCode::BAD_REQUEST,
            TurnError::UnsupportedSwarm => StatusCode::NOT_IMPLEMENTED,
            TurnError::NotFound => StatusCode::NOT_FOUND,
            TurnError::Conflict => StatusCode::CONFLICT,
            TurnError::Provider | TurnError::RecoveryExhausted => StatusCode::BAD_GATEWAY,
            TurnError::Internal | TurnError::Checkpoint => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// A bounded, client-safe description.
    pub(crate) fn message(self) -> &'static str {
        match self {
            TurnError::Invalid => "The session request is invalid.",
            TurnError::UnsupportedSwarm => {
                "This build cannot execute this Swarm configuration. It cannot run as an ordinary local scan."
            }
            TurnError::NotFound => "Workspace or session is unavailable.",
            TurnError::Conflict => "Session changed or reached its turn limit.",
            TurnError::Provider => "The provider could not complete this turn.",
            TurnError::RecoveryExhausted => {
                "Provider recovery exhausted; accepted activity is retained."
            }
            TurnError::Internal => "Session request failed.",
            TurnError::Checkpoint => "The completed turn could not be saved after bounded retries.",
        }
    }
}

/// Retry the same atomic persistence operation, never model inference or tools.
/// Ordinary turns keep their existing single-attempt error contract.
pub(crate) async fn save_checkpoint<T>(
    local: bool,
    mut save: impl FnMut() -> anyhow::Result<T>,
) -> Result<T, TurnError> {
    let attempts = if local { 3 } else { 1 };
    for attempt in 0..attempts {
        match save() {
            Ok(value) => return Ok(value),
            Err(error) => beale_host_log::error!("session checkpoint save failed: {error:#}"),
        }
        if attempt + 1 < attempts {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }
    Err(if local {
        TurnError::Checkpoint
    } else {
        TurnError::Internal
    })
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

pub(crate) async fn run_turn(
    state: SessionState,
    mut request: SubmitSessionTurnRequest,
    progress: Option<mpsc::Sender<SessionStreamEvent>>,
    stop: Option<watch::Receiver<bool>>,
) -> Result<SessionDetail, TurnError> {
    #[cfg(feature = "swarms")]
    if super::hierarchy::applies(&state.db, &request)? {
        return super::hierarchy::run(state, request, progress, stop).await;
    }
    if request.max_concurrent_workers > 64 {
        return Err(TurnError::Invalid);
    }

    let lock = if request.session_id.is_empty() {
        Arc::new(Mutex::new(()))
    } else {
        session_lock(&state, &request.session_id).await
    };
    let _guard = lock.try_lock().map_err(|_| TurnError::Conflict)?;
    let mut automatic = false;
    loop {
        let detail = run_one_turn(
            &state,
            request.clone(),
            progress.clone(),
            automatic,
            &lock,
            stop.clone(),
            None,
        )
        .await?;
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
            request.swarm_id.clear();
            request.swarm_models.clear();
            request.swarm_node_concurrency.clear();
            automatic = true;
            continue;
        }
        return Ok(detail);
    }
}

/// Request fields resolved before any host state is read.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ValidatedRequest {
    pub(super) provider: Provider,
    pub(super) mode: AuthMode,
    pub(super) reasoning_effort: Option<&'static str>,
}

/// Checks the request shape and resolves its provider, credential method, and
/// reasoning effort. Host-dependent checks (workspace, graph feature scope,
/// saved session) follow separately.
#[cfg(any(test, feature = "swarms"))]
pub(super) fn validate_request(
    request: &SubmitSessionTurnRequest,
) -> Result<ValidatedRequest, TurnError> {
    validate_request_limit(request, 16_384)
}

fn validate_request_limit(
    request: &SubmitSessionTurnRequest,
    prompt_limit: usize,
) -> Result<ValidatedRequest, TurnError> {
    if !request.swarm_pacing.is_empty() {
        if !cfg!(feature = "swarms")
            || request.swarm_id.is_empty() && request.session_id.is_empty()
            || request.swarm_pacing.len() > 64
        {
            return Err(TurnError::Invalid);
        }
        #[cfg(feature = "swarms")]
        for target in &request.swarm_pacing {
            super::hierarchy::pacing_target(target)?;
        }
    }
    if request.swarm_retention.as_ref().is_some_and(|p| {
        !cfg!(feature = "swarms")
            || request.swarm_id.is_empty() && request.session_id.is_empty()
            || p.ended_scan_seconds > 365 * 86400
            || p.storage_reserve_bytes > i64::MAX as u64
    }) {
        return Err(TurnError::Invalid);
    }
    if !request.swarm_destinations.is_empty() {
        if !cfg!(feature = "swarms")
            || request.session_id.is_empty() && request.swarm_id.is_empty()
            || request.swarm_destinations.len() > 128
        {
            return Err(TurnError::Invalid);
        }
        let mut seen = std::collections::HashSet::new();
        for destination in &request.swarm_destinations {
            if !beale_contracts::swarms::valid_id(&destination.host_id)
                || !beale_contracts::swarms::valid_id(&destination.configuration_id)
                || !seen.insert((&destination.host_id, &destination.configuration_id))
            {
                return Err(TurnError::Invalid);
            }
        }
    }
    if request.max_concurrent_workers > 64
        || request.prompt.trim().is_empty()
        || request.prompt.len() > prompt_limit
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
    fleet_note: Option<&'a str>,
    goal_active: bool,
    goal_section: Option<&'a str>,

    _marker: std::marker::PhantomData<&'a ()>,
}

/// Model tools and extension catalog entries published for one turn.
struct Extensions {
    definitions: Vec<Value>,
    entries: String,

    callable_plugins: HashSet<String>,
}

/// Publishes enabled feature guidance and installed plugin instructions for a
/// turn and returns the tool definitions offered to the main agent and the
/// installed extensions.
async fn assemble_extensions(
    state: &SessionState,
    workspace_id: &str,
    agent_files: &AgentFiles,
    dynamic: &DynamicFeatures<'_>,
    saved_tools: Option<&[String]>,
) -> Result<Extensions, TurnError> {
    let _ = dynamic;
    let mut definitions = definitions();
    let mut entries = String::new();

    for feature in features::native_features() {
        let (enabled, tools, guidance): (bool, Vec<Value>, &str) = match feature.id {
            beale_fleet::FEATURE_ID => (
                dynamic.fleet_note.is_some(),
                Vec::new(),
                dynamic.fleet_note.unwrap_or(""),
            ),
            beale_goals::FEATURE_ID => (
                dynamic.goal_active,
                vec![beale_goals::definition()],
                dynamic
                    .goal_section
                    .unwrap_or(beale_goals::PROMPT_EXTENSION),
            ),

            _ => (
                match saved_tools {
                    Some(names) => feature
                        .tool_names
                        .iter()
                        .any(|name| names.iter().any(|saved| saved == name)),
                    None => feature_applies(&state.db, feature.id, workspace_id)?,
                },
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
    if let Some(names) = saved_tools {
        definitions.retain(|definition| {
            definition["name"]
                .as_str()
                .is_some_and(|name| names.iter().any(|saved| saved == name))
        });
    }
    Ok(Extensions {
        definitions,
        entries,

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
    local: Option<&beale_storage::swarm_state::LocalTurn>,
) -> Result<SessionDetail, TurnError> {
    if local.is_none() {
        ensure_local_execution(&state.db, &request)?;
    }
    let automatic = local.map_or(automatic, |turn| turn.operator_text.is_none());
    let transcript_prompt = local
        .map_or(request.prompt.as_str(), |turn| turn.transcript_prompt())
        .to_owned();
    let started = Instant::now();
    let _workspace_id = parse_id(&request.workspace_id)?;
    if !state.workspaces.contains(&request.workspace_id)? {
        return Err(TurnError::NotFound);
    }
    let ValidatedRequest {
        provider,
        mode,
        reasoning_effort,
    } = validate_request_limit(&request, if local.is_some() { 262_144 } else { 16_384 })?;
    validate_swarm_selection(
        &state.db,
        &request.swarm_id,
        &request.swarm_models,
        &request.swarm_node_concurrency,
        &request.session_id,
        &request.workspace_id,
    )?;

    let (mut session, mut turns) = if request.session_id.is_empty()
        || (local.is_some()
            && state
                .db
                .session(&request.session_id)
                .map_err(|_| TurnError::Internal)?
                .is_none())
    {
        let id = match local {
            Some(local) => parse_id(&local.session_id)?,
            None => Uuid::new_v4(),
        };
        register_session_lock(state, &id.to_string(), lock).await;
        let created_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| TurnError::Internal)?
            .as_millis();
        (
            StoredSession {
                id: id.to_string(),
                workspace_id: request.workspace_id.clone(),
                title: title(&transcript_prompt),
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

    let persisted_goal = saved_goal(&state.db, &session.id).map_err(|_| TurnError::Internal)?;
    let mut goal = persisted_goal;

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
    {
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
    let model_prompts = state
        .db
        .session_model_prompts(&session.id)
        .map_err(|_| TurnError::Internal)?;
    let mut messages = Vec::with_capacity(turns.len() * 2 + 1);
    for turn in &turns {
        messages.push(TextMessage {
            role: TextRole::User,
            text: model_prompts
                .get(&turn.number)
                .unwrap_or(&turn.prompt)
                .clone(),
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

    let history_bytes = messages
        .iter()
        .map(|message| message.text.len())
        .sum::<usize>();
    if history_bytes.saturating_add(INTERRUPTED_ANSWER.len()) > MAX_HISTORY_BYTES {
        return Err(TurnError::Conflict);
    }
    let workspace = state.workspaces.directory(&request.workspace_id)?;
    #[cfg(feature = "swarms")]
    let workspace = match local {
        Some(turn) => {
            super::research_files::directory(state, turn, workspace).map_err(|error| {
                beale_host_log::error!("Swarm workspace unavailable: {error:#}");
                TurnError::Internal
            })?
        }
        None => workspace,
    };
    let workspace_details = state.workspaces.details_for_host(&request.workspace_id)?;
    let saved_scan = local
        .map(|local| {
            beale_storage::swarm_state::SwarmStore::open(state.db.clone())
                .and_then(|store| store.load(&local.scan_id, true))
                .map_err(|_| TurnError::Internal)
        })
        .transpose()?;
    let saved_tools = saved_scan
        .as_ref()
        .map(|saved| saved.config.enabled_tools.as_slice());
    let mut fleet_prompt_note = fleet_prompt_note(&state.db)?;
    if let Some(machines) = beale_fleet::machine_prompt_note(&state.db, &request.workspace_id)
        .await
        .map_err(|_| TurnError::Internal)?
    {
        match &mut fleet_prompt_note {
            Some(note) => {
                note.push_str("\n\n");
                note.push_str(&machines);
            }
            None => fleet_prompt_note = Some(machines),
        }
    }
    let surface_tools_enabled = surface_feature_enabled(&state.db, &request.workspace_id)?;
    let memories_tools_enabled = match saved_tools {
        Some(names) => names.iter().any(|name| name == "memory_query"),
        None => memories_feature_enabled(&state.db, &request.workspace_id)?,
    };
    #[cfg(not(feature = "memories"))]
    let _ = memories_tools_enabled;

    let review_context = saved_scan
        .as_ref()
        .map(|saved| saved.config.authorized_scope.clone())
        .unwrap_or_else(|| {
            review_workspace_context(
                &workspace_details.guidance,
                &workspace_details.rules,
                &workspace_details.surfaces,
                surface_tools_enabled,
            )
        });

    let mut input = input_messages(&messages);
    let reuse_prior_context = turns.last().is_some_and(|turn| {
        turn.provider == request.provider
            && turn.method == request.method
            && turn.model == request.model
            && !turn.answer.starts_with(INTERRUPTED_ANSWER)
    });

    let saved_context = retained_context(
        &state.db,
        &session.id,
        local.map(|turn| turn.profile.as_str()),
    )
    .map_err(|_| TurnError::Internal)?;
    let reuse_saved_context = provider == Provider::OpenAi && reuse_prior_context;

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
    // Swarm sessions add or remove role-owned tools after assembly.
    #[cfg_attr(not(feature = "swarms"), allow(unused_mut))]
    let Extensions {
        mut definitions,
        entries: extension_entries,

        callable_plugins,
    } = assemble_extensions(
        state,
        &request.workspace_id,
        &agent_files,
        &DynamicFeatures {
            fleet_note: fleet_prompt_note.as_deref(),
            goal_active,
            goal_section: goal_section.as_deref(),

            _marker: std::marker::PhantomData,
        },
        saved_tools,
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

    if !request.agent_role.is_empty() {
        return Err(TurnError::Invalid);
    }

    let active_model = SwarmNodeModel {
        node_id: local.map_or_else(|| "main".to_owned(), |local| local.profile.clone()),
        provider: request.provider,
        method: request.method,
        model: request.model.clone(),
    };
    // Swarm sessions resolve the active role's provider; the request's own
    // provider and method were validated above for the entry role.

    #[allow(unused_mut)]
    let mut system = system;
    #[cfg(feature = "swarms")]
    if let Some(local) = local {
        let saved = saved_scan.as_ref().ok_or(TurnError::Internal)?;
        system.push_str(
            &super::hierarchy::instructions(saved, &local.agent_id)
                .map_err(|_| TurnError::Internal)?,
        );
        definitions.extend(beale_swarms::local::definitions(local.role));
        if let Some(names) = &saved_tools {
            definitions.retain(|definition| {
                definition["name"]
                    .as_str()
                    .is_some_and(|name| names.iter().any(|saved| saved == name))
            });
        }
    }

    if provider != Provider::OpenAi {
        input.insert(0, json!({"role":"system","content":system}));
    }
    let mut openai_connection = OpenAiConnection::default();
    let mut goal_signal: Option<beale_goals::Signal> = None;
    let mut tool_rows = Vec::<ToolCallRow>::new();
    // Provider call IDs identify results within this execution. Repeated replies
    // must replay the result, not perform the tool operation a second time.
    let mut completed_calls = HashMap::<String, CompletedLocalCall>::new();
    let mut replayed_calls = 0;

    let child_turns = Vec::<ChildTurnRow>::new();
    let mut reasoning_rows = Vec::<ReasoningSummaryRow>::new();
    let mut commentary_rows = Vec::<CommentaryRow>::new();
    let mut model_usage = Vec::<ModelUsageRow>::new();

    let answer: Option<String>;
    let mut segment_index = 0_u32;

    'rounds: loop {
        if local.is_some() && tool_rows.len() + replayed_calls >= 512 {
            answer = Some("Execution paused after the per-turn tool activity safeguard; saved work can be resumed.".into());
            break;
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
                            turn_number: 0,
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
            let recovering_tools = local.is_some() && !tool_rows.is_empty();
            let attempts = if recovering_tools { 3 } else { 2 };
            for attempt in 0..attempts {
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
                let retry = match &result {
                    Err(error) => {
                        recovering_tools || (attempt == 0 && provider_error_retryable(error))
                    }
                    Ok(step) => {
                        recovering_tools && step.calls.is_empty() && step.text.trim().is_empty()
                    }
                };
                if retry && attempt + 1 < attempts {
                    beale_host_log::error!(
                        "session provider step requires bounded retry; retaining tool exchange"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    continue;
                }
                return result;
            }
            unreachable!("provider step returns after its final attempt")
        };
        let attributed = async {
            if let Some(local) = local {
                beale_providers::pacing::scope(
                    state.db.clone(),
                    &local.scan_id,
                    beale_contracts::pacing::CallClass::Research,
                    provider_call,
                )
                .await
            } else {
                provider_call.await
            }
        };
        let step = until_stopped(attributed, &mut stop).await;
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
                    if local.is_none()
                        && let Some(tx) = &progress
                    {
                        let _ = tx
                            .send(SessionStreamEvent {
                                turn_number: 0,
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
            let args: Result<Value, _> = serde_json::from_str(&call.arguments);
            let call_arguments = args
                .as_ref()
                .map_or_else(|_| call.arguments.clone(), Value::to_string);
            if local.is_some()
                && let Some(completed) = completed_calls.get(&call.id)
            {
                if completed.name != call.name || completed.arguments != call_arguments {
                    answer = Some(format!(
                        "{INTERRUPTED_ANSWER} The provider reused a tool identity with different content."
                    ));
                    break 'rounds;
                }
                append_tool_result(
                    &mut input,
                    &mut anthropic_tool_results,
                    provider,
                    mode,
                    &call.id,
                    &completed.output,
                );
                replayed_calls += 1;
                continue;
            }
            let id = Uuid::new_v4().to_string();
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
            } else {
                tool_summary(&call.name, args.as_ref().unwrap_or(&Value::Null))
            };

            if let Some(tx) = &progress {
                let _ = tx
                    .send(SessionStreamEvent {
                        turn_number: 0,
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
                local,
                state,
                workspace_id: &request.workspace_id,
                session_id: &session.id,
                agent_id: local.map_or(active_model.node_id.as_str(), |local| {
                    local.agent_id.as_str()
                }),
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
            };
            let gate = runtime
                .gate(&call.name, args.as_ref().ok(), &mut stop)
                .await;
            let review_unavailable = std::cell::Cell::new(false);
            let (output, is_error) = match gate {
                tool_runtime::Gate::Run { reviewed } => match args {
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
                            turn_number: 0,
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
            if local.is_some() {
                completed_calls.insert(
                    call.id.clone(),
                    CompletedLocalCall {
                        name: call.name.clone(),
                        arguments: call_arguments,
                        output: model_output,
                    },
                );
            }
            if let Some(local) = local {
                let store = beale_storage::swarm_state::SwarmStore::open(state.db.clone())
                    .map_err(|_| TurnError::Internal)?;
                if store
                    .local_yield_requested(local)
                    .map_err(|_| TurnError::Internal)?
                {
                    answer = Some(
                        "Delegation accepted. Waiting for child execution after a safe checkpoint."
                            .into(),
                    );
                    break 'rounds;
                }
            }
            if review_unavailable.get() {
                answer = Some("Auto-Review was unavailable, so the requested tool was not run. Saved activity can be resumed after the review service is available.".to_owned());
                break 'rounds;
            }
        }
        if !anthropic_tool_results.is_empty() {
            input.push(json!({"role":"user","content":anthropic_tool_results}));
        }
    }

    let mut answer = answer.ok_or(TurnError::Provider)?;
    if answer.trim().is_empty() || history_bytes.saturating_add(answer.len()) > MAX_HISTORY_BYTES {
        if tool_rows.is_empty() {
            if local.is_none()
                && let Some(tx) = &progress
            {
                let text = if answer.trim().is_empty() {
                    "The provider completed without a message or tool call. Try again."
                } else {
                    "The provider answer exceeded this session's history limit."
                };
                let _ = tx
                    .send(SessionStreamEvent {
                        turn_number: 0,
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
        prompt: transcript_prompt,
        answer,
        duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        internal_prompt: automatic,
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

    if provider == Provider::OpenAi {
        close_unanswered_openai_calls(&mut input);
    }

    let provider_context_json =
        if provider == Provider::OpenAi && !latest.answer.starts_with(INTERRUPTED_ANSWER) {
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
    let goal_commit = goal.as_ref().map(beale_goals::GoalCommit);

    #[cfg(feature = "memories")]
    let memory_commit = memories_tools_enabled.then_some(beale_memories::QueueMemoryJobs);
    let agent_role = local.map(|local| local.profile.as_str());

    save_checkpoint(local.is_some(), || {
        let coordinator_commit = local
            .filter(|turn| turn.role == beale_contracts::swarms::Role::Leader)
            .map(beale_storage::swarm_state::CoordinatorTurnCommit);
        // Which extensions apply depends on the compiled features.
        let candidates: Vec<Option<&dyn beale_storage::TurnExtension>> = vec![
            coordinator_commit
                .as_ref()
                .map(|commit| commit as &dyn beale_storage::TurnExtension),
            goal_commit
                .as_ref()
                .map(|commit| commit as &dyn beale_storage::TurnExtension),
            #[cfg(feature = "memories")]
            memory_commit
                .as_ref()
                .map(|commit| commit as &dyn beale_storage::TurnExtension),
        ];
        let extensions: Vec<&dyn beale_storage::TurnExtension> =
            candidates.into_iter().flatten().collect();

        state.db.append_turn_with_metadata(
            &session_row,
            &TurnRow {
                number: next_turn_number,
                provider: latest.provider,
                method: latest.method,
                model: latest.model.clone(),
                prompt: latest.prompt.clone(),
                answer: latest.answer.clone(),
                duration_ms: i64::try_from(latest.duration_ms)?,
                internal_prompt: latest.internal_prompt,
            },
            next_turn_number == 1,
            TurnActivity {
                tools: &tool_rows,
                reasoning: &reasoning_rows,
                commentary: &commentary_rows,
            },
            (!repeat_preset.is_empty()).then_some((repeat_preset.as_str(), automation_options)),
            TurnMetadata {
                model_prompt: local
                    .filter(|turn| turn.operator_text.is_some())
                    .map(|turn| turn.prompt.as_str()),
                provider_context_json: provider_context_json.as_deref(),
                agent_role,
                model_usage: Some(&model_usage),
                child_turns: &child_turns,
                extensions: &extensions,
            },
        )
    })
    .await?;

    if local.is_some()
        && !tool_rows.is_empty()
        && (latest.answer.starts_with(INTERRUPTED_ANSWER)
            || (latest.answer.starts_with("Auto-Review was unavailable")
                && tool_rows.iter().any(|row| !row.is_error)))
    {
        return Err(TurnError::RecoveryExhausted);
    }
    trim_recent_turns(&mut turns);
    detail(&state.db, &session, &turns).map_err(|error| {
        if local.is_some() {
            TurnError::Checkpoint
        } else {
            error.into()
        }
    })
}

pub(super) fn retained_context(
    db: &HostDatabase,
    session: &str,
    profile: Option<&str>,
) -> anyhow::Result<Option<String>> {
    match profile {
        Some(profile) => db.swarm_provider_context(session, profile),
        None => db.provider_context(session),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_conversation_selects_the_saved_profile_without_ordinary_or_sibling_fallback() {
        let root =
            std::env::temp_dir().join(format!("beale-context-owner-example-{}", Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        db.replace_workspaces(&[beale_storage::WorkspaceRow {
            id: "workspace-example".into(),
            name: "Example".into(),
            directory: root.join("workspace").to_string_lossy().into(),
        }])
        .unwrap();
        let session = beale_storage::SessionRow {
            id: "session-example".into(),
            workspace_id: "workspace-example".into(),
            title: "Example".into(),
            created_at_ms: 1,
            last_message_at_ms: 1,
        };
        for (index, profile) in [None, Some("deputy-example"), Some("worker-example")]
            .into_iter()
            .enumerate()
        {
            let context =
                json!([{"role":"assistant","content":profile.unwrap_or("ordinary-example")}])
                    .to_string();
            db.append_turn_with_metadata(
                &session,
                &beale_storage::TurnRow {
                    number: index as i64 + 1,
                    provider: ProviderId::Openai as i32,
                    method: AuthMethod::ApiKey as i32,
                    model: "example-model".into(),
                    prompt: "Synthetic task".into(),
                    answer: "Synthetic answer".into(),
                    duration_ms: 0,
                    internal_prompt: false,
                },
                index == 0,
                beale_storage::TurnActivity {
                    tools: &[],
                    reasoning: &[],
                    commentary: &[],
                },
                None,
                beale_storage::TurnMetadata {
                    provider_context_json: Some(&context),
                    agent_role: profile,
                    ..Default::default()
                },
            )
            .unwrap();
        }
        for profile in [None, Some("deputy-example"), Some("worker-example")] {
            let retained: Value = serde_json::from_str(
                &retained_context(&db, "session-example", profile)
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(
                retained[0]["content"],
                profile.unwrap_or("ordinary-example")
            );
        }
        assert!(
            retained_context(&db, "session-example", Some("new-independent-example"))
                .unwrap()
                .is_none()
        );
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(feature = "findings")]
    #[tokio::test]
    async fn saved_tool_set_survives_feature_changes_without_enabling_new_tools() {
        let root =
            std::env::temp_dir().join(format!("beale-saved-tools-example-{}", Uuid::new_v4()));
        let state = build_state(
            "token-example".into(),
            WorkspaceStore::at(root.join("host")).unwrap(),
        )
        .unwrap();
        let files = AgentFiles::new().unwrap();
        let dynamic = DynamicFeatures {
            fleet_note: None,
            goal_active: false,
            goal_section: None,
            _marker: std::marker::PhantomData,
        };
        state
            .db
            .set_feature_enabled(beale_findings::FEATURE_ID, false)
            .unwrap();
        let saved = vec!["read".into(), "findings_create".into()];
        let active =
            assemble_extensions(&state, "workspace-example", &files, &dynamic, Some(&saved))
                .await
                .unwrap();
        let names: Vec<_> = active
            .definitions
            .iter()
            .map(|d| d["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["read", "findings_create"]);
        state
            .db
            .set_feature_enabled(beale_findings::FEATURE_ID, true)
            .unwrap();
        state
            .db
            .set_extension_scope("feature", beale_findings::FEATURE_ID, true, &[])
            .unwrap();
        let limited = assemble_extensions(
            &state,
            "workspace-example",
            &files,
            &dynamic,
            Some(&["read".into()]),
        )
        .await
        .unwrap();
        assert_eq!(limited.definitions.len(), 1);
        assert_eq!(limited.definitions[0]["name"], "read");
        let ordinary_files = AgentFiles::new().unwrap();
        let ordinary =
            assemble_extensions(&state, "workspace-example", &ordinary_files, &dynamic, None)
                .await
                .unwrap();
        assert!(
            ordinary
                .definitions
                .iter()
                .any(|d| d["name"] == "findings_create")
        );
        drop(state);
        drop(files);
        std::fs::remove_dir_all(root).unwrap();
    }

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
