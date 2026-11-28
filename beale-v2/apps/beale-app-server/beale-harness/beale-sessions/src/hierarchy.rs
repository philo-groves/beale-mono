//! Host-owned local execution. Workspace copies are owned by Stage 4 storage; Fleet placement is separate.
use super::*;
use beale_contracts::swarms::{AgentState, ExecutionState, ToolOperationState};
use beale_contracts::swarms::{
    ProviderRoute, RetentionPolicy, Role, ScanConfig, ScanMode, VERSION,
};
use beale_storage::swarm_state::{LocalModel, LocalSettings, SwarmStore};
use std::time::Duration;

pub(crate) mod scheduler;

fn store(db: &HostDatabase) -> Result<SwarmStore, TurnError> {
    SwarmStore::open(db.clone()).map_err(|_| TurnError::Internal)
}

pub(super) fn instructions(
    saved: &beale_storage::swarm_state::ScanState,
    agent_id: &str,
) -> anyhow::Result<String> {
    use anyhow::{Context, ensure};
    let identity = &saved.agent(agent_id)?.summary;
    let profile = saved
        .config
        .profile(&identity.profile)
        .context("missing saved profile")?;
    let ScanMode::Swarm { snapshot } = &saved.config.mode else {
        anyhow::bail!("not a swarm")
    };
    let mut text = format!(
        "\n\nSwarm agent identity: {}. Profile: {}. Role: {:?}.\n{}\nWorkflow: {}\nDelegate using only permitted profiles. Results and messages wake waiting parents automatically. End your response when waiting; do not poll. Independent reviewers receive only their assigned task and evidence, never participant conversations.",
        identity.id, identity.profile.id, identity.role, profile.instruction, snapshot.definition
    );
    if let Some(parent_id) = &identity.parent_id {
        text.push_str(&format!("\nImmediate parent identity: {parent_id}."));
        if identity.role == Role::Worker {
            let parent = &saved.agent(parent_id)?.summary;
            ensure!(
                parent.role == Role::Deputy,
                "worker parent must be a deputy"
            );
            let stage = saved
                .config
                .profile(&parent.profile)
                .context("missing saved deputy profile")?;
            text.push_str(&format!("\n\nInherited deputy stage ({} revision {}):\n{}\nApply your specialist instructions within these saved stage responsibilities. Delegation and assignment text cannot remove these stage boundaries.", parent.profile.id, parent.profile.revision, stage.instruction));
        }
    }
    Ok(text)
}

fn enabled_tools(state: &SessionState, workspace: &str) -> anyhow::Result<Vec<String>> {
    let mut names: Vec<String> = beale_tools::definitions()
        .iter()
        .filter_map(|d| d["name"].as_str().map(str::to_owned))
        .collect();
    for feature in features::native_features() {
        if feature.id == beale_goals::FEATURE_ID || feature.id == beale_fleet::FEATURE_ID {
            continue;
        }
        if feature.id == beale_swarms::FEATURE_ID
            || features::applies(&state.db, feature.id, workspace)?
        {
            names.extend(feature.tool_names.iter().map(|name| (*name).to_owned()));
        }
    }
    if !state.plugins.available(workspace)?.is_empty() {
        names.push("plugin_call".into());
    }
    names.sort();
    names.dedup();
    Ok(names)
}

pub(super) fn saved(db: &HostDatabase, id: &str) -> bool {
    db.with_connection(|c| {
        let exists: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='swarm_local')",
            [],
            |r| r.get(0),
        )?;
        if !exists {
            return Ok(false);
        }
        Ok(c.query_row(
            "SELECT EXISTS(SELECT 1 FROM swarm_local WHERE scan_id=?1)",
            [id],
            |r| r.get(0),
        )?)
    })
    .unwrap_or(false)
}

pub(super) fn applies(
    db: &HostDatabase,
    request: &SubmitSessionTurnRequest,
) -> Result<bool, TurnError> {
    if !request.agent_role.is_empty() {
        return Err(TurnError::Invalid);
    }
    if is_swarm_child(db, &request.session_id)? {
        return Err(TurnError::Conflict);
    }
    Ok(!request.swarm_id.is_empty() || saved(db, &request.session_id))
}

pub(super) fn enrich(
    db: &HostDatabase,
    id: &str,
    detail: &mut SessionDetail,
) -> Result<(), StatusCode> {
    if !saved(db, id) {
        return Ok(());
    }
    let store = store(db).map_err(TurnError::status)?;
    let state = store
        .load(id, true)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if let ScanMode::Swarm { snapshot } = &state.config.mode {
        let overview = beale_contracts::swarm_ui::ScanOverview {
            revision: state.revision,
            definition_id: snapshot.id.clone(),
            definition_revision: snapshot.revision,
            leader_rule: snapshot.leader_rule.instruction.clone(),
            destinations: state.config.destinations.clone(),
            pacing: state.config.pacing.clone(),
            agents: state
                .agents
                .iter()
                .map(|a| beale_contracts::swarm_ui::AgentOverview {
                    summary: a.summary.clone(),
                    pending_assignments: a.checkpoint.pending_assignments.len(),
                    queued_wakes: a.checkpoint.wake_reasons.len(),
                    termination_unconfirmed: state.attempts.iter().any(|t| {
                        t.agent_id == a.summary.id
                            && t.state == ExecutionState::TerminationUnconfirmed
                    }),
                    unknown_commands: state
                        .operations
                        .iter()
                        .filter(|o| {
                            o.agent_id == a.summary.id
                                && o.state == ToolOperationState::OutcomeUnknown
                        })
                        .count(),
                    resumable: matches!(
                        a.summary.state,
                        AgentState::Completed
                            | AgentState::Suspended
                            | AgentState::Waiting
                            | AgentState::Blocked
                    ),
                })
                .collect(),
        };
        detail.swarm_overview_json =
            serde_json::to_string(&overview).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        detail.swarm_id = snapshot.id.clone();
        detail.swarm_template_json =
            serde_json::to_string(snapshot).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }
    detail.swarm_models = store
        .local_state(id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .settings
        .models
        .iter()
        .map(|m| SwarmNodeModel {
            node_id: m.profile.clone(),
            provider: m.provider,
            method: m.method,
            model: m.model.clone(),
        })
        .collect();
    detail.swarm_agents = state
        .agents
        .iter()
        .map(|a| SessionSwarmAgent {
            id: a.summary.id.clone(),
            parent_id: a.summary.parent_id.clone().unwrap_or_default(),
            agent_kind: format!("{:?}", a.summary.role).to_lowercase(),
            role: a.summary.profile.id.clone(),
            status: format!("{:?}", a.summary.state).to_lowercase(),
        })
        .collect();
    if let Some(leader) = state.agents.iter().find(|a| a.summary.role == Role::Leader) {
        detail.scoped_review_rules = beale_safety::agent_review_scope(db, id, &leader.summary.id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .rule
            .into_iter()
            .collect();
    }
    // Reconstruct bounded host notices on every detail read, including after a
    // client reconnect. Do not copy guest event payloads into the transcript or
    // imply that a historical recovery event is still an active failure.
    if let Some(message) = detail
        .messages
        .iter_mut()
        .rev()
        .find(|message| message.role == SessionRole::Assistant as i32)
    {
        let mut notices = std::collections::BTreeMap::<&str, usize>::new();
        for event in &state.events {
            let notice = match event.kind.as_str() {
                "local-recovery" => "Execution required bounded recovery.",
                "coordinator-interrupted" => {
                    "The leader was interrupted; dependent work was stopped and partial results retained."
                }
                "worker-termination-unconfirmed" => {
                    "Worker termination could not be confirmed; replacement was withheld."
                }
                "inference-interrupted" => {
                    "A provider response was interrupted; usage may be incomplete and the original request was not replayed."
                }
                "plugin-outcome-unknown" => {
                    "A plugin outcome was unknown and requires reconciliation before repeating the effect."
                }
                "plugin-resources-unavailable" => {
                    "Plugin resources were unavailable during worker preparation."
                }
                "peer-recovery-pending" => {
                    "Peer execution required recovery; pairing or checkpoint availability prevented completion."
                }
                "worker-event"
                    if matches!(
                        event.payload["event"]["kind"].as_str(),
                        Some("warning" | "worker_failure")
                    ) =>
                {
                    "A worker reported a warning or interrupted execution; retained recovery state may require attention."
                }
                _ => continue,
            };
            *notices.entry(notice).or_default() += 1;
        }
        for (notice, count) in notices {
            message.commentary.push(SessionCommentary {
                text: format!("Recovery history ({count} recorded events): {notice}"),
                after_tool_count: u32::try_from(message.tools.len())
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
                agent_role: String::new(),
            });
        }
    }
    Ok(())
}

fn now() -> Result<i64, TurnError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|v| i64::try_from(v.as_millis()).ok())
        .ok_or(TurnError::Internal)
}

pub(super) fn live_detail(
    state: &SessionState,
    scan: &str,
    request: &SubmitSessionTurnRequest,
    started: u64,
) -> Result<SessionDetail, StatusCode> {
    let db = state
        .db
        .read_snapshot()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if db
        .session(scan)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .is_some()
    {
        let (session, turns) = load_persisted(&db, parse_id(scan)?)?;
        return detail(&db, &session, &turns);
    }
    // The durable hierarchy exists before its first transcript turn. Present its
    // configuration without inventing a completed turn or writing placeholder history.
    let mut detail = SessionDetail {
        summary: Some(beale_contracts::v1::SessionSummary {
            id: scan.into(),
            workspace_id: request.workspace_id.clone(),
            title: title(&request.prompt),
            last_message_at_ms: started,
        }),
        provider: request.provider,
        method: request.method,
        model: request.model.clone(),
        first_turn_number: 1,
        pacing_warnings: beale_storage::provider_accounting::AccountingStore::open(db.clone())
            .and_then(|s| s.warnings(scan))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        ..Default::default()
    };
    enrich(&db, scan, &mut detail)?;
    Ok(detail)
}

/// Tags a leader turn's progress with its turn number and publishes it to
/// every attached stream. Publishing never waits for a subscriber.
struct TurnProgress {
    sender: Option<mpsc::Sender<SessionStreamEvent>>,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl TurnProgress {
    fn new(output: tokio::sync::broadcast::Sender<SessionStreamEvent>, number: u64) -> Self {
        let (sender, mut receiver) = mpsc::channel::<SessionStreamEvent>(32);
        let task = tokio::spawn(async move {
            while let Some(mut event) = receiver.recv().await {
                event.turn_number = number;
                let _ = output.send(event);
            }
        });
        Self {
            sender: Some(sender),
            task: Some(task),
        }
    }
    async fn finish(mut self) {
        self.sender.take();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}
impl Drop for TurnProgress {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

/// Ends a scan again at its original end time after a follow-up could not
/// reopen it, restoring its retention deadline. Retained destinations learn the
/// restored end now or through lifecycle maintenance.
async fn restore_end(
    state: &SessionState,
    store: &SwarmStore,
    scan: &str,
    ended_at_ms: i64,
) -> Result<(), TurnError> {
    store
        .end_scan(scan, ended_at_ms)
        .map_err(|_| TurnError::Internal)?;
    let saved = store.load(scan, true).map_err(|_| TurnError::Internal)?;
    if let Err(error) = distributed::placement::sync_lifecycle(state, &saved).await {
        beale_host_log::error!("Restored scan end remains pending: {error:#}");
    }
    Ok(())
}

/// Runs a Swarm submission: starts the scan's scheduler or attaches to the
/// running one, forwards live events to `progress`, and returns the scan's
/// outcome once scheduling ends. Dropping this future detaches without
/// stopping the scan; a stop on `stop` stops it.
pub(super) async fn run(
    state: SessionState,
    request: SubmitSessionTurnRequest,
    progress: Option<mpsc::Sender<SessionStreamEvent>>,
    stop: Option<watch::Receiver<bool>>,
) -> Result<SessionDetail, TurnError> {
    scheduler::start_or_attach(&state, request)
        .await?
        .follow(progress, stop)
        .await
}

/// Checks a Swarm submission and returns its scan ID.
fn validate_start(
    state: &SessionState,
    request: &SubmitSessionTurnRequest,
) -> Result<String, TurnError> {
    engine::validate_request(request)?;
    if !request.session_id.is_empty() && !request.swarm_pacing.is_empty() {
        return Err(TurnError::Invalid);
    }
    if !request.swarm_destinations.is_empty() && state.fleet_origin.is_none() {
        return Err(TurnError::Invalid);
    }
    parse_id(&request.workspace_id)?;
    if !state.workspaces.contains(&request.workspace_id)? {
        return Err(TurnError::NotFound);
    }
    // Goal continuation belongs to the leader scheduler, not a second turn loop.
    if request.goal_mode
        || !request.repeat_preset.is_empty()
        || !request.swarm_node_concurrency.is_empty()
    {
        return Err(TurnError::Invalid);
    }
    Ok(if request.session_id.is_empty() {
        Uuid::new_v4().to_string()
    } else {
        parse_id(&request.session_id)?.to_string()
    })
}

/// A follow-up must match the saved scan's workspace, destinations, Swarm,
/// and retention.
fn validate_follow_up(
    saved: &beale_storage::swarm_state::ScanState,
    request: &SubmitSessionTurnRequest,
) -> Result<(), TurnError> {
    if request.swarm_retention.as_ref().is_some_and(|p| {
        p.ended_scan_seconds != saved.config.retention.ended_scan_seconds
            || p.storage_reserve_bytes != saved.config.retention.storage_reserve_bytes
    }) {
        return Err(TurnError::Conflict);
    }
    if saved.config.workspace_id != request.workspace_id
        || !request.swarm_destinations.is_empty()
            && request
                .swarm_destinations
                .iter()
                .map(|d| (&d.host_id, &d.configuration_id))
                .collect::<Vec<_>>()
                != saved
                    .config
                    .destinations
                    .iter()
                    .map(|d| (&d.host_id, &d.configuration_id))
                    .collect::<Vec<_>>()
    {
        return Err(TurnError::Conflict);
    }
    if let ScanMode::Swarm { snapshot } = &saved.config.mode
        && !request.swarm_id.is_empty()
        && request.swarm_id != snapshot.id
    {
        return Err(TurnError::Conflict);
    }
    Ok(())
}

fn leader_id(saved: &beale_storage::swarm_state::ScanState) -> Result<&str, TurnError> {
    saved
        .agents
        .iter()
        .find(|a| a.summary.role == Role::Leader)
        .map(|a| a.summary.id.as_str())
        .ok_or(TurnError::Internal)
}

/// Queues a follow-up for a running scan's leader. The scheduler delivers it
/// at the leader's next turn.
fn queue_follow_up(
    state: &SessionState,
    request: &SubmitSessionTurnRequest,
    scan: &str,
) -> Result<(), TurnError> {
    let store = store(&state.db)?;
    let saved = store.load(scan, true).map_err(|_| TurnError::NotFound)?;
    validate_follow_up(&saved, request)?;
    store
        .local_message(
            scan,
            &Uuid::new_v4().to_string(),
            leader_id(&saved)?,
            &request.prompt,
        )
        .map_err(|error| {
            beale_host_log::error!("Swarm follow-up was not queued: {error:#}");
            TurnError::Conflict
        })
}

/// Creates a new scan, or queues a follow-up for a saved one and reopens it
/// when it has ended. The caller holds the scan's session lock.
async fn prepare(
    state: &SessionState,
    request: &SubmitSessionTurnRequest,
    scan: &str,
) -> Result<(), TurnError> {
    let scan = scan.to_owned();
    let store = store(&state.db)?;
    if request.session_id.is_empty() {
        if !feature_applies(&state.db, "swarms", &request.workspace_id)? {
            return Err(TurnError::Conflict);
        }
        swarm_templates::load(state.workspaces.root());
        let definition =
            beale_swarms::swarm_definition(&request.swarm_id).ok_or(TurnError::Invalid)?;
        let mut models = Vec::new();
        let mut selected = HashSet::new();
        for model in &request.swarm_models {
            if !selected.insert((
                model.node_id.clone(),
                model.provider,
                model.method,
                model.model.clone(),
            )) || !definition
                .snapshot
                .profiles
                .iter()
                .any(|p| p.profile.id == model.node_id)
            {
                return Err(TurnError::Invalid);
            }
            let check = SubmitSessionTurnRequest {
                provider: model.provider,
                method: model.method,
                model: model.model.clone(),
                ..request.clone()
            };
            engine::validate_request(&check)?;
        }
        for profile in &definition.snapshot.profiles {
            let chosen: Vec<_> = request
                .swarm_models
                .iter()
                .filter(|m| m.node_id == profile.profile.id)
                .collect();
            if chosen.is_empty() {
                models.push(LocalModel {
                    profile: profile.profile.id.clone(),
                    provider: request.provider,
                    method: request.method,
                    model: request.model.clone(),
                });
            } else {
                models.extend(chosen.iter().map(|m| LocalModel {
                    profile: profile.profile.id.clone(),
                    provider: m.provider,
                    method: m.method,
                    model: m.model.clone(),
                }));
            }
        }
        if models.len() > 64 {
            return Err(TurnError::Invalid);
        }
        let details = state.workspaces.details_for_host(&request.workspace_id)?;
        let scope =
            review_workspace_context(&details.guidance, &details.rules, &details.surfaces, true);
        let config = ScanConfig {
            version: VERSION,
            scan_id: scan.clone(),
            workspace_id: request.workspace_id.clone(),
            mode: ScanMode::Swarm {
                snapshot: Box::new(definition.snapshot.clone()),
            },
            authorized_scope: if scope.trim().is_empty() {
                "Use the recorded workspace authorization boundary.".into()
            } else {
                scope
            },
            enabled_tools: enabled_tools(state, &request.workspace_id)
                .map_err(|_| TurnError::Internal)?,
            provider_routes: models
                .iter()
                .enumerate()
                .map(|(index, m)| {
                    let validated = engine::validate_request(&SubmitSessionTurnRequest {
                        provider: m.provider,
                        method: m.method,
                        model: m.model.clone(),
                        ..request.clone()
                    })?;
                    let binding = beale_providers::pacing::resolve(
                        &state.db,
                        validated.provider,
                        validated.mode,
                        &m.model,
                    )
                    .map_err(|_| TurnError::Internal)?;
                    Ok(ProviderRoute {
                        id: format!("{}-{index}", m.profile),
                        provider_id: format!("provider-{}", m.provider),
                        model: m.model.clone(),
                        budget_group_id: binding.group_id,
                    })
                })
                .collect::<Result<Vec<_>, TurnError>>()?,
            destinations: request
                .swarm_destinations
                .iter()
                .map(|d| beale_contracts::swarms::Destination {
                    host_id: d.host_id.clone(),
                    configuration_id: d.configuration_id.clone(),
                })
                .collect(),
            pacing: request
                .swarm_pacing
                .iter()
                .map(pacing_target)
                .collect::<Result<Vec<_>, _>>()?,
            retention: request.swarm_retention.as_ref().map_or_else(
                RetentionPolicy::default,
                |p| RetentionPolicy {
                    ended_scan_seconds: p.ended_scan_seconds,
                    storage_reserve_bytes: p.storage_reserve_bytes,
                },
            ),
        };
        config.validate().map_err(|_| TurnError::Invalid)?;
        store
            .create_scan(config, true, true)
            .map_err(|_| TurnError::Internal)?;
        research_files::capture(state, &scan, &request.workspace_id).map_err(|error| {
            beale_host_log::error!("Swarm snapshot failed: {error:#}");
            TurnError::Internal
        })?;
        let initialize = if request.swarm_destinations.is_empty() {
            SwarmStore::local_initialize
        } else {
            SwarmStore::peer_initialize
        };
        initialize(
            &store,
            &scan,
            LocalSettings {
                models,
                reasoning_effort: request.reasoning_effort,
            },
            &request.prompt,
        )
        .map_err(|_| TurnError::Internal)?;
        return Ok(());
    }
    let saved = store.load(&scan, true).map_err(|_| TurnError::NotFound)?;
    validate_follow_up(&saved, request)?;
    let leader = leader_id(&saved)?.to_owned();
    // An ended scan is reopened only for a follow-up that can be delivered.
    // If reopening cannot finish, the original end clock is restored, so a
    // rejected follow-up never leaves the scan open without an end.
    let reopened_from = saved.ended_at_ms;
    if let Some(ended_at_ms) = reopened_from {
        store
            .local_message_deliverable(&scan, &leader)
            .map_err(|error| {
                beale_host_log::error!("Swarm follow-up cannot be delivered: {error:#}");
                TurnError::Conflict
            })?;
        store
            .reopen_scan(&scan, true)
            .map_err(|_| TurnError::Conflict)?;
        if let Err(error) = distributed::placement::sync_lifecycle(
            state,
            &store.load(&scan, true).map_err(|_| TurnError::Internal)?,
        )
        .await
        {
            beale_host_log::error!("Retained scan cannot reopen yet: {error:#}");
            restore_end(state, &store, &scan, ended_at_ms).await?;
            return Err(TurnError::Conflict);
        }
    }
    if let Err(error) =
        store.local_message(&scan, &Uuid::new_v4().to_string(), &leader, &request.prompt)
    {
        beale_host_log::error!("Swarm follow-up was not queued: {error:#}");
        if let Some(ended_at_ms) = reopened_from {
            restore_end(state, &store, &scan, ended_at_ms).await?;
        }
        return Err(TurnError::Conflict);
    }
    Ok(())
}

pub(super) fn pacing_target(
    value: &beale_contracts::v1::SwarmPacingTarget,
) -> Result<beale_contracts::swarms::PacingTarget, TurnError> {
    use beale_contracts::swarms::{PacingAmount, PacingTarget};
    let amount = match value.unit.as_str() {
        "subscription" => PacingAmount::Subscription {
            window_id: value.denomination.clone(),
            basis_points: u32::try_from(value.amount).map_err(|_| TurnError::Invalid)?,
        },
        "currency" => PacingAmount::Currency {
            currency: value.denomination.clone(),
            micros: value.amount,
        },
        "credits" if value.denomination.is_empty() => PacingAmount::Credits {
            micros: value.amount,
        },
        _ => return Err(TurnError::Invalid),
    };
    let target = PacingTarget {
        budget_group_id: value.budget_group_id.clone(),
        interval_seconds: value.interval_seconds,
        amount,
    };
    beale_storage::provider_accounting::validate_target(&target).map_err(|_| TurnError::Invalid)?;
    Ok(target)
}
