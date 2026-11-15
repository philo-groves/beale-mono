#[cfg(feature = "runbooks")]
use axum::Json;
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path as RoutePath, Query, State},
    http::{HeaderMap, StatusCode, header},
    routing::{get, post},
};
#[cfg(feature = "runbooks")]
use beale_contracts::runbooks::{self, CellRunRequest, CellRunResponse};
use beale_contracts::{
    ARCHIVE_PATH, AUTOMATIONS_CANCEL_PATH, AUTOMATIONS_PATH, PROTOBUF_STREAM_CONTENT_TYPE,
    QUICK_CHAT_DETAIL_PATH, QUICK_CHAT_TURNS_PATH, SESSION_ARCHIVE_PATH,
    SESSION_DETAIL_MAX_TOOL_SUMMARIES, SESSION_DETAIL_PATH, SESSION_RESTORE_PATH,
    SESSION_SUBAGENT_DETAIL_PATH, SESSION_TOOL_DETAIL_PATH, SESSION_TURN_ID_HEADER,
    SESSION_TURN_STOP_PATH, SESSION_TURN_STREAM_PATH, SESSION_TURNS_PATH, SESSIONS_PATH,
    session_stream_frame,
    v1::{
        ArchiveEntry, AuthMethod, GoalStatus, GraphNodeConcurrency, GraphNodeModel,
        ListArchiveResponse, ListSessionsResponse, ProviderId, ReasoningEffort,
        SessionAgentMetrics, SessionCommentary, SessionDetail, SessionGoal, SessionGraphAgent,
        SessionGraphConfig, SessionGraphHandoff, SessionMessage, SessionModelUsage,
        SessionReasoningSummary, SessionRole, SessionStreamEvent, SessionStreamKind,
        SessionSubagentDetail, SessionSubagentItem, SessionSubagentSummary, SessionSummary,
        SessionTokenUsage, SessionToolDetailPage, SessionToolSummary, SessionUsageSnapshot,
        SubmitSessionTurnRequest, SurfaceScopeState, WorkspaceSurface,
    },
};
use beale_extender::PluginRuntime;
use beale_providers::{
    AuthMode, CredentialStore, OpenAiConnection, Provider, ProviderClient, ProviderProgress,
    TextMessage, TextRole, ToolStepRequest, input_messages, provider_error_message, tool_output,
};
use beale_safety::ReviewRequest;
#[cfg(any(feature = "goal-mode", test))]
use beale_storage::GoalRow;
use beale_storage::{
    AutomationOptions, ChildTurnRow, CommentaryRow, HostDatabase, ModelUsageRow,
    ReasoningSummaryRow, SessionChildSummaryRow, SessionRow, ToolCallRow, TurnActivity,
    TurnMetadata, TurnRow,
};
use beale_tools::{
    AgentFiles, PluginCallArgs, definitions, execute, model_preview, page, plugin_call_definition,
    summary as base_tool_summary,
};
use futures_util::stream;
use prost::Message;
use serde_json::{Value, json};
#[cfg(feature = "graphs")]
use std::sync::atomic::AtomicU32;
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    sync::{Arc, Weak},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Mutex, mpsc, watch};
use uuid::Uuid;

#[cfg(feature = "runbooks")]
struct ActiveRunbookRun {
    db: HostDatabase,
    id: String,
    finished: bool,
}

#[cfg(feature = "runbooks")]
impl ActiveRunbookRun {
    fn finish(&mut self, success: bool, output: &str) -> anyhow::Result<()> {
        self.db.runbook_run_finish(&self.id, success, output)?;
        self.finished = true;
        Ok(())
    }
}

#[cfg(feature = "runbooks")]
impl Drop for ActiveRunbookRun {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.db.runbook_run_interrupt(&self.id);
        }
    }
}

use beale_workspaces::WorkspaceStore;
mod automations;
#[cfg(feature = "memories")]
mod memory_worker;

#[cfg(feature = "surfaces")]
fn surface_feature_enabled(db: &HostDatabase, workspace_id: &str) -> Result<bool, StatusCode> {
    db.feature_enabled(beale_surfaces::FEATURE_ID)
        .and_then(|enabled| {
            if enabled {
                db.extension_applies_to("feature", beale_surfaces::FEATURE_ID, workspace_id)
            } else {
                Ok(false)
            }
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(not(feature = "surfaces"))]
fn surface_feature_enabled(_db: &HostDatabase, _workspace_id: &str) -> Result<bool, StatusCode> {
    Ok(false)
}

#[cfg(feature = "memories")]
fn memories_feature_enabled(db: &HostDatabase, workspace_id: &str) -> Result<bool, StatusCode> {
    db.feature_enabled(beale_memories::FEATURE_ID)
        .and_then(|enabled| {
            if enabled {
                db.extension_applies_to("feature", beale_memories::FEATURE_ID, workspace_id)
            } else {
                Ok(false)
            }
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(not(feature = "memories"))]
fn memories_feature_enabled(_db: &HostDatabase, _workspace_id: &str) -> Result<bool, StatusCode> {
    Ok(false)
}

#[cfg(feature = "webfetch")]
fn webfetch_feature_enabled(db: &HostDatabase, workspace_id: &str) -> Result<bool, StatusCode> {
    db.feature_enabled(beale_webfetch::FEATURE_ID)
        .and_then(|enabled| {
            if enabled {
                db.extension_applies_to("feature", beale_webfetch::FEATURE_ID, workspace_id)
            } else {
                Ok(false)
            }
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(feature = "webfetch")]
async fn execute_webfetch(
    db: &HostDatabase,
    workspace_id: &str,
    arguments: &Value,
) -> anyhow::Result<beale_tools::ToolOutcome> {
    anyhow::ensure!(
        webfetch_feature_enabled(db, workspace_id)
            .map_err(|_| anyhow::anyhow!("could not read Webfetch availability"))?,
        "Webfetch is disabled or unavailable in this workspace"
    );
    Ok(beale_tools::ToolOutcome {
        output: beale_webfetch::execute(arguments).await?,
        is_error: false,
    })
}

fn tool_summary(name: &str, arguments: &Value) -> String {
    #[cfg(feature = "webfetch")]
    if name == beale_webfetch::TOOL_NAME {
        return beale_webfetch::summary(arguments);
    }
    base_tool_summary(name, arguments)
}

#[cfg(feature = "agent-feedback")]
fn agent_feedback_feature_enabled(
    db: &HostDatabase,
    workspace_id: &str,
) -> Result<bool, StatusCode> {
    db.feature_enabled(beale_agent_feedback::FEATURE_ID)
        .and_then(|enabled| {
            if enabled {
                db.extension_applies_to("feature", beale_agent_feedback::FEATURE_ID, workspace_id)
            } else {
                Ok(false)
            }
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(feature = "findings")]
fn findings_feature_enabled(db: &HostDatabase, workspace_id: &str) -> Result<bool, StatusCode> {
    db.feature_enabled(beale_findings::FEATURE_ID)
        .and_then(|enabled| {
            if enabled {
                db.extension_applies_to("feature", beale_findings::FEATURE_ID, workspace_id)
            } else {
                Ok(false)
            }
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(feature = "runbooks")]
fn runbook_feature_enabled(db: &HostDatabase, workspace_id: &str) -> Result<bool, StatusCode> {
    db.feature_enabled(beale_runbooks::FEATURE_ID)
        .and_then(|enabled| {
            if enabled {
                db.extension_applies_to("feature", beale_runbooks::FEATURE_ID, workspace_id)
            } else {
                Ok(false)
            }
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

fn surface_relationship_lines(surfaces: &[WorkspaceSurface]) -> Vec<String> {
    let names: HashMap<_, _> = surfaces
        .iter()
        .map(|surface| {
            (
                surface.id.as_str(),
                surface
                    .definition
                    .as_ref()
                    .and_then(|item| item.locator.as_ref())
                    .map_or("", |locator| locator.value.as_str()),
            )
        })
        .collect();
    surfaces
        .iter()
        .flat_map(|surface| {
            surface.relationships.iter().filter_map(|relationship| {
                names.get(relationship.target_id.as_str()).map(|target| {
                    format!(
                        "{} {} —{}{}→ {} {target}",
                        surface.id,
                        surface
                            .definition
                            .as_ref()
                            .and_then(|item| item.locator.as_ref())
                            .map_or("", |locator| locator.value.as_str()),
                        relationship.label,
                        if relationship.type_id.is_empty() {
                            String::new()
                        } else {
                            format!(" [{}]", relationship.type_id)
                        },
                        relationship.target_id
                    )
                })
            })
        })
        .collect()
}

fn bounded_context(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let prefix: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

fn surface_context_line(surface: &WorkspaceSurface) -> String {
    let Some(definition) = &surface.definition else {
        return format!("{} [missing definition]", surface.id);
    };
    let scope = if definition.scope == 1 {
        "in scope"
    } else {
        "out of scope"
    };
    let scope_state = scope_state_name(definition.scope_state);
    let mut line = format!(
        "{} [{scope}; {scope_state}] {}",
        surface.id,
        bounded_context(
            definition
                .locator
                .as_ref()
                .map_or("", |locator| locator.value.as_str()),
            200
        )
    );
    line.push_str(&format!(
        "; type {}; name {}",
        definition.type_id,
        bounded_context(&definition.name, 100)
    ));
    if !definition.description.is_empty() {
        line.push_str(&format!(
            "; description {}",
            bounded_context(&definition.description, 500)
        ));
    }
    if !definition.out_of_scope_reason.is_empty() {
        line.push_str(&format!(
            "; exclusion {}",
            bounded_context(&definition.out_of_scope_reason, 200)
        ));
    }
    if !definition.evidence_refs.is_empty() {
        line.push_str(&format!(
            "; {} evidence references",
            definition.evidence_refs.len()
        ));
    }
    line
}

fn scope_state_name(value: i32) -> &'static str {
    match SurfaceScopeState::try_from(value) {
        Ok(SurfaceScopeState::Candidate) => "candidate",
        Ok(SurfaceScopeState::Approved) => "approved",
        _ => "unknown",
    }
}

fn feature_tool_requires_review(name: &str) -> bool {
    let _ = name;
    #[cfg(feature = "webfetch")]
    if name == beale_webfetch::TOOL_NAME {
        return true;
    }
    #[cfg(feature = "surfaces")]
    if beale_surfaces::is_tool(name) {
        return true;
    }
    #[cfg(feature = "findings")]
    if beale_findings::is_tool(name) {
        return true;
    }
    #[cfg(feature = "memories")]
    if beale_memories::is_tool(name) {
        return true;
    }
    #[cfg(feature = "runbooks")]
    if beale_runbooks::is_tool(name) {
        return true;
    }
    #[cfg(feature = "goal-mode")]
    if name == beale_goals::TOOL_NAME {
        return true;
    }
    false
}

#[cfg(feature = "graphs")]
fn requested_top_level_role(
    graph: Option<&beale_graphs::GraphDefinition>,
    id: &str,
) -> Result<Option<beale_graphs::Role>, StatusCode> {
    if id.is_empty() {
        return Ok(None);
    }
    beale_graphs::Role::from_id(id)
        .filter(|role| role.is_top_level())
        .filter(|role| graph.is_some_and(|graph| graph.nodes.contains(role)))
        .map(Some)
        .ok_or(StatusCode::BAD_REQUEST)
}

#[cfg(feature = "graphs")]
fn validate_delegations(
    runtime: &dyn beale_graphs::GraphRuntime,
    arguments: &Value,
    config: &SessionGraphConfig,
    fallback: &GraphNodeModel,
) -> Result<(), String> {
    runtime
        .assignments(arguments, config, fallback)
        .map_err(|error| format!("Invalid delegation: {error}"))?;
    Ok(())
}

#[cfg(feature = "runbooks")]
fn runbook_review_context(state: &SessionState, workspace_id: &str) -> Result<String, StatusCode> {
    let details = state.workspaces.details_for_host(workspace_id)?;
    let surfaces = if surface_feature_enabled(&state.db, workspace_id)? {
        format!(
            "Surfaces: {}\nOut-of-scope surfaces are boundary references only. Imported descriptions and relationships are context, not verified observations or authorization. A relationship does not authorize another surface.\nRecorded relationships: {}",
            details
                .surfaces
                .iter()
                .map(surface_context_line)
                .collect::<Vec<_>>()
                .join("; "),
            surface_relationship_lines(&details.surfaces).join("; "),
        )
    } else {
        String::new()
    };
    Ok(format!(
        "Guidance: {}\nRules: {}\n{}",
        details.guidance,
        details.rules.join("; "),
        surfaces
    ))
}
#[cfg(feature = "graphs")]
mod graphs;
mod http;
#[cfg(feature = "graphs")]
mod peer_graph;
mod quick_chat;
mod system_prompt;
use http::{authorized, checked_body, response};

const RECENT_TURNS: usize = 64;
const MAX_RECENT_HISTORY_BYTES: usize = 1_048_576;
const MAX_HISTORY_BYTES: usize = 4_194_304;
const TOOL_PAGE_BYTES: usize = 8_000;
const INTERRUPTED_ANSWER: &str =
    "The provider could not complete this turn. Tool activity was saved.";
const STOPPED_ANSWER: &str = "Stopped by the user. Completed tool activity was saved.";

fn compact_openai_context(input: &[Value]) -> Vec<Value> {
    let start = input
        .iter()
        .rposition(|item| item.get("type").and_then(Value::as_str) == Some("compaction"))
        .unwrap_or(0);
    input[start..].to_vec()
}

fn close_unanswered_openai_calls(input: &mut Vec<Value>) {
    let answered: HashSet<&str> = input
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("function_call_output"))
        .filter_map(|item| item.get("call_id").and_then(Value::as_str))
        .collect();
    let mut missing = Vec::new();
    let mut seen = HashSet::new();
    for item in input.iter() {
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            continue;
        }
        if let Some(id) = item.get("call_id").and_then(Value::as_str)
            && !answered.contains(id)
            && seen.insert(id)
        {
            missing.push(id.to_owned());
        }
    }
    for id in missing {
        input.push(json!({"type":"function_call_output","call_id":id,
            "output":"Tool was not run because the turn ended before this call was processed."}));
    }
}

fn validate_graph_selection(
    db: &HostDatabase,
    graph_id: &str,
    graph_models: &[GraphNodeModel],
    graph_node_concurrency: &[GraphNodeConcurrency],
    session_id: &str,
    workspace_id: &str,
) -> Result<(), StatusCode> {
    if graph_id.is_empty() {
        return if graph_models.is_empty() && graph_node_concurrency.is_empty() {
            Ok(())
        } else {
            Err(StatusCode::BAD_REQUEST)
        };
    }
    #[cfg(feature = "graphs")]
    {
        if !session_id.is_empty() || !beale_graphs::known_graph(graph_id) {
            return Err(StatusCode::BAD_REQUEST);
        }
        if graph_models.len() > 64 {
            return Err(StatusCode::BAD_REQUEST);
        }
        let graph = beale_graphs::graph_definition(graph_id).ok_or(StatusCode::BAD_REQUEST)?;
        if graph_node_concurrency.len() > graph.nodes.len() {
            return Err(StatusCode::BAD_REQUEST);
        }
        let mut seen = std::collections::HashSet::new();
        let mut per_role = [0_usize; 10];
        for item in graph_models {
            let Some(role) = beale_graphs::Role::from_id(&item.node_id) else {
                return Err(StatusCode::BAD_REQUEST);
            };
            if !graph.nodes.contains(&role) {
                return Err(StatusCode::BAD_REQUEST);
            }
            per_role[role.index()] += 1;
            if per_role[role.index()] > role.max_models()
                || item.model.is_empty()
                || item.model.len() > 256
                || !item.model.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
                })
                || !matches!(ProviderId::try_from(item.provider), Ok(provider) if provider != ProviderId::Unspecified)
                || !matches!(AuthMethod::try_from(item.method), Ok(method) if method != AuthMethod::Unspecified)
                || !seen.insert((
                    item.node_id.as_str(),
                    item.provider,
                    item.method,
                    item.model.as_str(),
                ))
            {
                return Err(StatusCode::BAD_REQUEST);
            }
        }
        let mut limited_roles = std::collections::HashSet::new();
        for item in graph_node_concurrency {
            let Some(role) = beale_graphs::Role::from_id(&item.node_id) else {
                return Err(StatusCode::BAD_REQUEST);
            };
            if !graph.nodes.contains(&role) {
                return Err(StatusCode::BAD_REQUEST);
            }
            if item.max_concurrent == 0
                || item.max_concurrent > beale_graphs::MAX_CONCURRENT
                || (role.is_top_level() && item.max_concurrent != 1)
                || !limited_roles.insert(item.node_id.as_str())
            {
                return Err(StatusCode::BAD_REQUEST);
            }
        }
        if !db
            .feature_enabled(beale_graphs::FEATURE_ID)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            || !db
                .extension_applies_to("feature", beale_graphs::FEATURE_ID, workspace_id)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        {
            return Err(StatusCode::CONFLICT);
        }
        Ok(())
    }
    #[cfg(not(feature = "graphs"))]
    {
        let _ = (
            db,
            graph_models,
            graph_node_concurrency,
            session_id,
            workspace_id,
        );
        Err(StatusCode::BAD_REQUEST)
    }
}

fn encoded_graph_config(request: &SubmitSessionTurnRequest) -> Option<Vec<u8>> {
    if request.graph_id.is_empty() {
        return None;
    }
    let concurrency = request.graph_node_concurrency.clone();
    #[cfg(feature = "graphs")]
    let mut concurrency = concurrency;
    #[cfg(feature = "graphs")]
    if request.graph_id == beale_graphs::SIMPLE_ID
        && !concurrency
            .iter()
            .any(|item| item.node_id == beale_graphs::Role::Main.id())
        && let Some(max_concurrent) = beale_graphs::Role::Main.default_max_concurrent()
    {
        concurrency.push(GraphNodeConcurrency {
            node_id: beale_graphs::Role::Main.id().to_owned(),
            max_concurrent,
        });
    }
    Some(
        SessionGraphConfig {
            models: request.graph_models.clone(),
            concurrency,
        }
        .encode_to_vec(),
    )
}

fn append_tool_result(
    input: &mut Vec<Value>,
    anthropic_results: &mut Vec<Value>,
    provider: Provider,
    mode: AuthMode,
    id: &str,
    output: &str,
) {
    let result = tool_output(provider, mode, id, output);
    if provider == Provider::Anthropic {
        if let Some(blocks) = result.get("content").and_then(Value::as_array) {
            anthropic_results.extend(blocks.iter().cloned());
        }
    } else {
        input.push(result);
    }
}

fn model_tool_preview(
    name: &str,
    id: &str,
    output: &str,
    is_error: bool,
    files: &AgentFiles,
) -> String {
    let preview = model_preview(output, files);
    if !is_error && matches!(name, "read" | "bash") {
        format!(
            "Finding evidence reference: {{\"kind\":\"tool_call\",\"reference\":\"{id}\"}}\n{preview}"
        )
    } else {
        preview
    }
}

fn usage_update(item: &ModelUsageRow, agent_role: &str) -> SessionStreamEvent {
    let input = item.input_tokens.unwrap_or(0).max(0) as u64;
    let output = item.output_tokens.unwrap_or(0).max(0) as u64;
    let cache_read = item.cache_read_tokens.unwrap_or(0).max(0) as u64;
    SessionStreamEvent {
        kind: SessionStreamKind::UsageUpdated as i32,
        agent_role: agent_role.to_owned(),
        usage_delta: Some(SessionModelUsage {
            provider: item.provider,
            method: item.method,
            model: item.model.clone(),
            role: item.role.clone(),
            usage: Some(SessionTokenUsage {
                input_tokens: input,
                output_tokens: output,
                measured_calls: u64::from(
                    item.input_tokens.is_some() && item.output_tokens.is_some(),
                ),
                total_calls: 1,
                cache_read_tokens: cache_read,
                cache_input_tokens: if item.cache_read_tokens.is_some() {
                    input
                } else {
                    0
                },
                cache_measured_calls: u64::from(item.cache_read_tokens.is_some()),
            }),
        }),
        ..Default::default()
    }
}

fn review_failed(decision: &beale_safety::ReviewDecision) -> bool {
    !decision.approved && !decision.reviewed
}

#[cfg(all(feature = "graphs", feature = "findings"))]
fn pipeline_finding_allowed(role: beale_graphs::Role, name: &str, args: &Value) -> bool {
    let state = args.get("state").and_then(Value::as_str);
    match name {
        "findings_query" | "findings_read" => true,
        "findings_create" => role == beale_graphs::Role::Discovery,
        "findings_update" => match role {
            beale_graphs::Role::Discovery => matches!(state, Some("finding" | "candidate")),
            beale_graphs::Role::Validation => matches!(state, Some("finding" | "validated")),
            beale_graphs::Role::Reporter => {
                matches!(state, Some("finding" | "screened" | "reported"))
            }
            beale_graphs::Role::PatchGeneration => state == Some("finding"),
            _ => true,
        },
        "findings_complete_state" => match role {
            beale_graphs::Role::Validation => state == Some("validated"),
            beale_graphs::Role::Reporter => matches!(state, Some("screened" | "reported")),
            beale_graphs::Role::Discovery | beale_graphs::Role::PatchGeneration => false,
            _ => true,
        },
        "findings_reject" => role == beale_graphs::Role::Validation,
        _ => true,
    }
}

#[derive(Clone)]
struct SessionState {
    token: String,
    db: HostDatabase,
    workspaces: WorkspaceStore,
    plugins: PluginRuntime,
    credentials: Arc<CredentialStore>,
    provider: Arc<ProviderClient>,
    session_locks: Arc<Mutex<HashMap<String, Weak<Mutex<()>>>>>,
    turn_stops: Arc<Mutex<HashMap<Uuid, watch::Sender<bool>>>>,
    quick_chat_lock: Arc<Mutex<()>>,
}

struct StoredSession {
    id: String,
    workspace_id: String,
    title: String,
    created_at_ms: u128,
    last_message_at_ms: u64,
}

struct StoredTurn {
    number: i64,
    provider: i32,
    method: i32,
    model: String,
    prompt: String,
    answer: String,
    duration_ms: u64,
    internal_prompt: bool,
    agent_role: String,
}

#[cfg(feature = "graphs")]
#[derive(Clone)]
struct PeerRun {
    role: beale_graphs::Role,
    turn_number: i64,
    notify: mpsc::UnboundedSender<()>,
    handoff: bool,
}

#[cfg(feature = "graphs")]
fn peer_input_messages(
    turns: &[StoredTurn],
    role: beale_graphs::Role,
    prompt: &str,
) -> Vec<TextMessage> {
    let mut messages = Vec::new();
    for turn in turns.iter().filter(|turn| turn.agent_role == role.id()) {
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
        text: prompt.to_owned(),
    });
    messages
}

pub fn router(token: String, workspaces: WorkspaceStore) -> anyhow::Result<Router> {
    let db = HostDatabase::open(workspaces.root())?;
    let plugins = PluginRuntime::new(db, workspaces.root().to_path_buf())?;
    router_with_runtime(token, workspaces, plugins)
}

pub fn router_with_runtime(
    token: String,
    workspaces: WorkspaceStore,
    plugins: PluginRuntime,
) -> anyhow::Result<Router> {
    let state = build_state_with_runtime(token, workspaces, plugins)?;
    state.db.recover_abandoned_graph_work()?;
    let router = Router::new()
        .route(SESSIONS_PATH, get(list))
        .route(SESSION_DETAIL_PATH, get(details))
        .route(SESSION_ARCHIVE_PATH, post(archive))
        .route(SESSION_RESTORE_PATH, post(restore))
        .route(ARCHIVE_PATH, get(list_archive))
        .route(SESSION_TOOL_DETAIL_PATH, get(tool_details))
        .route(SESSION_SUBAGENT_DETAIL_PATH, get(subagent_details))
        .route(SESSION_TURNS_PATH, post(submit_turn))
        .route(AUTOMATIONS_PATH, get(automations::list))
        .route(AUTOMATIONS_CANCEL_PATH, post(automations::cancel))
        .route(SESSION_TURN_STREAM_PATH, post(stream_turn))
        .route(SESSION_TURN_STOP_PATH, post(stop_turn))
        .route(QUICK_CHAT_TURNS_PATH, post(quick_chat::turn))
        .route(QUICK_CHAT_DETAIL_PATH, get(quick_chat::details));
    #[cfg(feature = "runbooks")]
    let router = router.route(runbooks::CELL_RUN_PATH, post(run_runbook_cell));
    Ok(router
        .layer(DefaultBodyLimit::max(20_000))
        .with_state(state))
}

fn build_state(token: String, workspaces: WorkspaceStore) -> anyhow::Result<SessionState> {
    let db = HostDatabase::open(workspaces.root())?;
    let plugins = PluginRuntime::new(db, workspaces.root().to_path_buf())?;
    build_state_with_runtime(token, workspaces, plugins)
}

fn build_state_with_runtime(
    token: String,
    workspaces: WorkspaceStore,
    plugins: PluginRuntime,
) -> anyhow::Result<SessionState> {
    let db = HostDatabase::open(workspaces.root())?;
    system_prompt::ensure_file(workspaces.root())?;
    let credentials = Arc::new(CredentialStore::at(workspaces.root().to_path_buf()));
    Ok(SessionState {
        token,
        db,
        workspaces,
        plugins,
        credentials,
        provider: Arc::new(ProviderClient::new()?),
        session_locks: Arc::new(Mutex::new(HashMap::new())),
        turn_stops: Arc::new(Mutex::new(HashMap::new())),
        quick_chat_lock: Arc::new(Mutex::new(())),
    })
}

pub fn start_automation_scheduler(
    workspaces: WorkspaceStore,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let state = build_state(String::new(), workspaces)?;
    Ok(tokio::spawn(automations::run_worker(state)))
}

pub fn start_automation_scheduler_with_runtime(
    workspaces: WorkspaceStore,
    plugins: PluginRuntime,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let state = build_state_with_runtime(String::new(), workspaces, plugins)?;
    Ok(tokio::spawn(automations::run_worker(state)))
}

#[cfg(feature = "memories")]
pub fn start_memory_worker(
    workspaces: WorkspaceStore,
    plugins: PluginRuntime,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let state = build_state_with_runtime(String::new(), workspaces, plugins)?;
    Ok(tokio::spawn(memory_worker::run(state)))
}

fn parse_id(value: &str) -> Result<Uuid, StatusCode> {
    let id = Uuid::parse_str(value).map_err(|_| StatusCode::BAD_REQUEST)?;
    if id.to_string() != value {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(id)
}

async fn session_lock(state: &SessionState, id: &str) -> Arc<Mutex<()>> {
    let mut locks = state.session_locks.lock().await;
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(id).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(id.to_owned(), Arc::downgrade(&lock));
    lock
}

async fn register_session_lock(state: &SessionState, id: &str, lock: &Arc<Mutex<()>>) {
    state
        .session_locks
        .lock()
        .await
        .insert(id.to_owned(), Arc::downgrade(lock));
}

fn load_persisted(
    db: &HostDatabase,
    id: Uuid,
) -> Result<(StoredSession, Vec<StoredTurn>), StatusCode> {
    let item = db
        .session(&id.to_string())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let peer_graph = db
        .is_peer_graph_session(&id.to_string())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let rows = if peer_graph {
        db.graph_recent_turns(&id.to_string())
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    } else {
        item.turns
    };
    if (rows.is_empty() && !peer_graph)
        || rows.len() > RECENT_TURNS
        || item.session.created_at_ms < 0
        || item.session.last_message_at_ms < 0
    {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let first_number = rows.first().map_or(1, |turn| turn.number);
    if first_number <= 0 {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let mut turns = rows
        .into_iter()
        .enumerate()
        .map(|(index, row)| {
            let expected = first_number
                .checked_add(i64::try_from(index).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?)
                .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
            if row.duration_ms < 0 || row.number < expected {
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
            let agent_role = if let Some(role) = db
                .graph_turn_role(&id.to_string(), row.number)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            {
                role
            } else {
                db.session_turn_role(&id.to_string(), row.number)
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                    .unwrap_or_else(|| "main".to_owned())
            };
            Ok(StoredTurn {
                number: row.number,
                provider: row.provider,
                method: row.method,
                model: row.model,
                prompt: row.prompt,
                answer: row.answer,
                duration_ms: row.duration_ms as u64,
                internal_prompt: row.internal_prompt,
                agent_role,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if item.session.workspace_id != quick_chat::WORKSPACE_ID {
        trim_recent_turns(&mut turns);
    }
    Ok((
        StoredSession {
            id: item.session.id,
            workspace_id: item.session.workspace_id,
            title: item.session.title,
            created_at_ms: item.session.created_at_ms as u128,
            last_message_at_ms: item.session.last_message_at_ms as u64,
        },
        turns,
    ))
}

fn trim_recent_turns(turns: &mut Vec<StoredTurn>) {
    let mut bytes = turns.iter().fold(0_usize, |total, turn| {
        total
            .saturating_add(turn.prompt.len())
            .saturating_add(turn.answer.len())
    });
    while turns.len() > 1 && bytes > MAX_RECENT_HISTORY_BYTES {
        let removed = turns.remove(0);
        bytes = bytes.saturating_sub(removed.prompt.len() + removed.answer.len());
    }
}

fn summary(session: &StoredSession) -> SessionSummary {
    SessionSummary {
        id: session.id.clone(),
        workspace_id: session.workspace_id.clone(),
        title: session.title.clone(),
        last_message_at_ms: session.last_message_at_ms,
    }
}

fn graph_state(
    db: &HostDatabase,
    session_id: &str,
) -> Result<(Vec<SessionGraphAgent>, Vec<SessionGraphHandoff>), StatusCode> {
    #[cfg(feature = "graphs")]
    {
        if !db
            .is_peer_graph_session(session_id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        {
            return Ok((Vec::new(), Vec::new()));
        }
        let id = db
            .session_graph_id(session_id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
        let graph = beale_graphs::graph_definition(&id).ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
        let agents = graph
            .nodes
            .iter()
            .filter(|role| role.is_top_level())
            .map(|role| {
                Ok(SessionGraphAgent {
                    role: role.id().to_owned(),
                    status: db
                        .graph_role_status(session_id, role.id())
                        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
                })
            })
            .collect::<Result<Vec<_>, StatusCode>>()?;
        let handoffs = db
            .graph_handoffs(session_id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .into_iter()
            .map(|item| SessionGraphHandoff {
                id: item.id,
                from_role: item.from_role,
                to_role: item.to_role,
                task: item.task,
                status: item.status,
            })
            .collect();
        Ok((agents, handoffs))
    }
    #[cfg(not(feature = "graphs"))]
    {
        let _ = (db, session_id);
        Ok((Vec::new(), Vec::new()))
    }
}

fn recent_peer_turns(
    turns: &[StoredTurn],
    mut tool_count: impl FnMut(&[i64]) -> Result<usize, StatusCode>,
) -> Result<&[StoredTurn], StatusCode> {
    let mut start = turns.len().saturating_sub(8);
    loop {
        let recent = &turns[start..];
        let numbers = recent.iter().map(|turn| turn.number).collect::<Vec<_>>();
        let history_bytes = recent.iter().fold(0_usize, |total, turn| {
            total.saturating_add(turn.prompt.len().saturating_add(turn.answer.len()))
        });
        if (tool_count(&numbers)? <= SESSION_DETAIL_MAX_TOOL_SUMMARIES
            && history_bytes <= MAX_HISTORY_BYTES)
            || recent.len() <= 1
        {
            return Ok(recent);
        }
        start += 1;
    }
}

fn subagent_summary(row: &SessionChildSummaryRow) -> Result<SessionSubagentSummary, StatusCode> {
    Ok(SessionSubagentSummary {
        id: row.agent_id.clone(),
        parent_role: row.parent_role.clone(),
        role: row.role.clone(),
        task: row.task.chars().take(256).collect(),
        status: if row.is_error { "failed" } else { "completed" }.to_owned(),
        completed_at_ms: u64::try_from(row.completed_at_ms)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    })
}

fn detail(
    db: &HostDatabase,
    session: &StoredSession,
    turns: &[StoredTurn],
) -> Result<SessionDetail, StatusCode> {
    if turns.is_empty() {
        #[cfg(not(feature = "graphs"))]
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
        #[cfg(feature = "graphs")]
        {
            let (graph_agents, graph_handoffs) = graph_state(db, &session.id)?;
            let graph_id = db
                .session_graph_id(&session.id)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
            let graph_config = db
                .session_graph_config(&session.id)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
            let config = SessionGraphConfig::decode(graph_config.as_slice())
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            let model = config
                .models
                .iter()
                .find(|model| {
                    beale_graphs::entry_role(&graph_id)
                        .is_some_and(|role| model.node_id == role.id())
                })
                .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
            return Ok(SessionDetail {
                summary: Some(summary(session)),
                provider: model.provider,
                method: model.method,
                model: model.model.clone(),
                first_turn_number: 1,
                graph_id,
                graph_models: config.models,
                graph_node_concurrency: config.concurrency,
                token_usage: Some(SessionTokenUsage::default()),
                agent_metrics: graph_agents
                    .iter()
                    .map(|agent| SessionAgentMetrics {
                        role: agent.role.clone(),
                        token_usage: Some(SessionTokenUsage::default()),
                        model_usage: Vec::new(),
                        usage_snapshots: Vec::new(),
                        duration_ms: 0,
                    })
                    .collect(),
                graph_agents,
                graph_handoffs,
                ..Default::default()
            });
        }
    }
    // Graph roles keep their own provider history. Bound the combined UI transcript
    // separately so a busy team cannot exceed the detail protocol's tool cap.
    let peer_graph = db
        .is_peer_graph_session(&session.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let turns = if peer_graph {
        recent_peer_turns(turns, |numbers| {
            db.tool_summaries_for_turns(&session.id, numbers)
                .map(|rows| rows.len())
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        })?
    } else {
        turns
    };
    let first_turn = turns[0].number;
    let (graph_agents, graph_handoffs) = graph_state(db, &session.id)?;
    let selected_turns = turns.iter().map(|turn| turn.number).collect::<Vec<_>>();
    let mut tools = if peer_graph {
        db.tool_summaries_for_turns(&session.id, &selected_turns)
    } else {
        db.tool_summaries_since(&session.id, first_turn)
    }
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let reasoning = if peer_graph {
        db.reasoning_summaries_for_turns(&session.id, &selected_turns)
    } else {
        db.reasoning_summaries_since(&session.id, first_turn)
    }
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let commentary = if peer_graph {
        db.commentary_for_turns(&session.id, &selected_turns)
    } else {
        db.commentary_since(&session.id, first_turn)
    }
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if tools.len() > SESSION_DETAIL_MAX_TOOL_SUMMARIES {
        let keep_from = tools.len() - SESSION_DETAIL_MAX_TOOL_SUMMARIES;
        tools.drain(..keep_from);
    }
    let mut messages = Vec::with_capacity(turns.len() * 2);
    let mut total = 0_usize;
    for turn in turns {
        total = total.saturating_add(turn.prompt.len() + turn.answer.len());
        if total > MAX_HISTORY_BYTES {
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        if !turn.internal_prompt {
            messages.push(SessionMessage {
                role: SessionRole::User as i32,
                text: turn.prompt.clone(),
                tools: Vec::new(),
                reasoning_summaries: Vec::new(),
                duration_ms: 0,
                commentary: Vec::new(),
                agent_role: turn.agent_role.clone(),
            });
        }
        let turn_number = turn.number;
        let turn_tools: Vec<_> = tools
            .iter()
            .filter(|tool| tool.turn_number == turn_number)
            .collect();
        let first_visible_tool = turn_tools.first().map_or(0, |tool| tool.ordinal);
        #[allow(unused_mut)]
        let mut turn_commentary = commentary
            .iter()
            .filter(|item| item.turn_number == turn_number)
            .map(|item| {
                if item.after_tool_count < 0 {
                    return Err(StatusCode::INTERNAL_SERVER_ERROR);
                }
                Ok(SessionCommentary {
                    text: item.text.clone(),
                    after_tool_count: item
                        .after_tool_count
                        .saturating_sub(first_visible_tool)
                        .min(turn_tools.len() as i64) as u32,
                    agent_role: item.agent_role.clone(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        messages.push(SessionMessage {
            role: SessionRole::Assistant as i32,
            text: turn.answer.clone(),
            tools: turn_tools
                .iter()
                .map(|tool| SessionToolSummary {
                    id: tool.id.clone(),
                    name: tool.name.clone(),
                    summary: tool.summary.clone(),
                    is_error: tool.is_error,
                    agent_role: tool.agent_role.clone(),
                })
                .collect(),
            reasoning_summaries: reasoning
                .iter()
                .filter(|item| item.turn_number == turn_number)
                .map(|item| {
                    if item.after_tool_count < 0 {
                        return Err(StatusCode::INTERNAL_SERVER_ERROR);
                    }
                    Ok(SessionReasoningSummary {
                        text: item.text.clone(),
                        after_tool_count: item
                            .after_tool_count
                            .saturating_sub(first_visible_tool)
                            .min(turn_tools.len() as i64)
                            as u32,
                        agent_role: item.agent_role.clone(),
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
            duration_ms: turn.duration_ms,
            commentary: turn_commentary,
            agent_role: turn.agent_role.clone(),
        });
    }
    let latest = turns.last().ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let goal = db
        .goal(&session.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map(|goal| {
            let turns_used =
                u64::try_from(goal.turns_used).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            let max_turns =
                u64::try_from(goal.max_turns).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            let max_duration_minutes = u64::try_from(goal.max_duration_minutes)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            let active_ms =
                u64::try_from(goal.active_ms).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            if turns_used == 0
                || goal.objective.trim().is_empty()
                || goal.objective.len() > 16_384
                || !matches!(
                    GoalStatus::try_from(goal.status),
                    Ok(GoalStatus::Active
                        | GoalStatus::Complete
                        | GoalStatus::Waiting
                        | GoalStatus::Paused)
                )
                || (goal.status == GoalStatus::Waiting as i32) == goal.required_state.is_empty()
            {
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
            Ok(SessionGoal {
                objective: goal.objective,
                status: goal.status,
                turns_used,
                required_state: goal.required_state,
                max_turns,
                max_duration_minutes,
                active_ms,
            })
        })
        .transpose()?;
    let graph_config = db
        .session_graph_config(&session.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .filter(|bytes| !bytes.is_empty())
        .map(|bytes| SessionGraphConfig::decode(bytes.as_slice()))
        .transpose()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .unwrap_or_default();
    let usage = db
        .session_usage_totals(&session.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let usage_snapshots = db
        .session_usage_snapshots(&session.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let count = |value| u64::try_from(value).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR);
    let scoped_usage =
        |totals: beale_storage::SessionUsageTotals| -> Result<SessionTokenUsage, StatusCode> {
            Ok(SessionTokenUsage {
                input_tokens: count(totals.input_tokens)?,
                output_tokens: count(totals.output_tokens)?,
                measured_calls: count(totals.measured_calls)?,
                total_calls: count(totals.total_calls)?,
                cache_read_tokens: count(totals.cache_read_tokens)?,
                cache_input_tokens: count(totals.cache_input_tokens)?,
                cache_measured_calls: count(totals.cache_measured_calls)?,
            })
        };
    let model_usage = db
        .session_model_totals(&session.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .into_iter()
        .map(|row| {
            Ok(beale_contracts::v1::SessionModelUsage {
                provider: row.provider,
                method: row.method,
                model: row.model,
                role: row.role,
                usage: Some(SessionTokenUsage {
                    input_tokens: count(row.totals.input_tokens)?,
                    output_tokens: count(row.totals.output_tokens)?,
                    measured_calls: count(row.totals.measured_calls)?,
                    total_calls: count(row.totals.total_calls)?,
                    cache_read_tokens: count(row.totals.cache_read_tokens)?,
                    cache_input_tokens: count(row.totals.cache_input_tokens)?,
                    cache_measured_calls: count(row.totals.cache_measured_calls)?,
                }),
            })
        })
        .collect::<Result<Vec<_>, StatusCode>>()?;
    let (runbook_count, active_runbook_count) = db
        .session_runbook_counts(&session.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let runbooks = db
        .session_runbooks(&session.workspace_id, &session.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut agent_metrics = Vec::with_capacity(graph_agents.len());
    for agent in &graph_agents {
        let role = agent.role.as_str();
        let totals = db
            .session_usage_totals_for_role(&session.id, Some(role))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let models = db
            .session_model_totals_for_role(&session.id, Some(role))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let snapshots = db
            .session_usage_snapshots_for_role(&session.id, Some(role))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        agent_metrics.push(SessionAgentMetrics {
            role: agent.role.clone(),
            token_usage: Some(scoped_usage(totals)?),
            model_usage: models
                .into_iter()
                .map(|item| {
                    Ok(SessionModelUsage {
                        provider: item.provider,
                        method: item.method,
                        model: item.model,
                        role: item.role,
                        usage: Some(scoped_usage(item.totals)?),
                    })
                })
                .collect::<Result<_, StatusCode>>()?,
            usage_snapshots: snapshots
                .into_iter()
                .map(|point| {
                    Ok(SessionUsageSnapshot {
                        work_duration_ms: point.work_duration_ms,
                        input_tokens: count(point.totals.input_tokens)?,
                        output_tokens: count(point.totals.output_tokens)?,
                        cache_read_tokens: count(point.totals.cache_read_tokens)?,
                        cache_input_tokens: count(point.totals.cache_input_tokens)?,
                        measured_calls: count(point.totals.measured_calls)?,
                        total_calls: count(point.totals.total_calls)?,
                        cache_measured_calls: count(point.totals.cache_measured_calls)?,
                        main_usage: Some(scoped_usage(point.main_totals)?),
                        subagent_usage: Some(scoped_usage(point.subagent_totals)?),
                    })
                })
                .collect::<Result<_, StatusCode>>()?,
            duration_ms: db
                .session_agent_duration_ms(&session.id, role)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        });
    }
    Ok(SessionDetail {
        graph_agents,
        graph_handoffs,
        agent_metrics,
        subagents: db
            .session_children(&session.id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .iter()
            .map(subagent_summary)
            .collect::<Result<Vec<_>, _>>()?,
        model_usage,
        summary: Some(summary(session)),
        messages,
        provider: latest.provider,
        method: latest.method,
        model: latest.model.clone(),
        goal,
        first_turn_number: u64::try_from(first_turn)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        graph_id: db
            .session_graph_id(&session.id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .unwrap_or_default(),
        graph_models: graph_config.models,
        graph_node_concurrency: graph_config.concurrency,
        total_duration_ms: db
            .session_total_duration_ms(&session.id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        has_campaign_activity: db
            .session_has_campaign_activity(&session.id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        token_usage: Some(SessionTokenUsage {
            input_tokens: count(usage.input_tokens)?,
            output_tokens: count(usage.output_tokens)?,
            measured_calls: count(usage.measured_calls)?,
            total_calls: count(usage.total_calls)?,
            cache_read_tokens: count(usage.cache_read_tokens)?,
            cache_input_tokens: count(usage.cache_input_tokens)?,
            cache_measured_calls: count(usage.cache_measured_calls)?,
        }),
        usage_snapshots: usage_snapshots
            .into_iter()
            .map(|snapshot| {
                Ok(SessionUsageSnapshot {
                    main_usage: Some(scoped_usage(snapshot.main_totals)?),
                    subagent_usage: Some(scoped_usage(snapshot.subagent_totals)?),
                    work_duration_ms: snapshot.work_duration_ms,
                    input_tokens: count(snapshot.totals.input_tokens)?,
                    output_tokens: count(snapshot.totals.output_tokens)?,
                    cache_read_tokens: count(snapshot.totals.cache_read_tokens)?,
                    cache_input_tokens: count(snapshot.totals.cache_input_tokens)?,
                    measured_calls: count(snapshot.totals.measured_calls)?,
                    total_calls: count(snapshot.totals.total_calls)?,
                    cache_measured_calls: count(snapshot.totals.cache_measured_calls)?,
                })
            })
            .collect::<Result<_, StatusCode>>()?,
        runbook_count,
        active_runbook_count,
        runbooks: runbooks
            .into_iter()
            .map(|book| beale_contracts::v1::SessionRunbook {
                id: book.id,
                title: book.title,
                status: book.status,
                updated_at_ms: book.updated_at_ms,
                completed_at_ms: book.completed_at_ms,
                current_cell_number: book.current_cell_number,
                current_cell_name: book.current_cell_name,
            })
            .collect(),
    })
}

fn title(prompt: &str) -> String {
    let line = prompt.lines().next().unwrap_or("").trim();
    let title: String = line.chars().take(72).collect();
    if title.is_empty() {
        "New Session".to_owned()
    } else {
        title
    }
}

async fn list(
    State(state): State<SessionState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let rows = state
        .db
        .sessions_excluding(quick_chat::WORKSPACE_ID)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if rows.len() > 1_024 {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let mut sessions = Vec::new();
    for row in rows {
        if state.workspaces.contains(&row.workspace_id)? {
            sessions.push(SessionSummary {
                id: row.id,
                workspace_id: row.workspace_id,
                title: row.title,
                last_message_at_ms: u64::try_from(row.last_message_at_ms)
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
            });
        }
    }
    Ok(response(ListSessionsResponse { sessions }))
}

async fn archive(
    State(state): State<SessionState>,
    RoutePath(id): RoutePath<String>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let id = parse_id(&id)?.to_string();
    let lock = session_lock(&state, &id).await;
    let _guard = lock.try_lock().map_err(|_| StatusCode::CONFLICT)?;
    let session = state
        .db
        .session(&id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    if !state.workspaces.contains(&session.session.workspace_id)? {
        return Err(StatusCode::NOT_FOUND);
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .as_millis();
    let now = i64::try_from(now).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !state
        .db
        .archive_session(&id, now)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn list_archive(
    State(state): State<SessionState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let rows = state
        .db
        .archive_items()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if rows.len() > 1_024 {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let mut entries = Vec::with_capacity(rows.len());
    for row in rows {
        if !row.quick_chat && !state.workspaces.contains(&row.workspace_id)? {
            continue;
        }
        if row.turn_count <= 0 {
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        entries.push(ArchiveEntry {
            id: row.id,
            workspace_id: row.workspace_id,
            title: row.title,
            model: row.model,
            turn_count: u64::try_from(row.turn_count)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
            created_at_ms: u64::try_from(row.created_at_ms)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
            archived_at_ms: u64::try_from(row.archived_at_ms)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
            quick_chat: row.quick_chat,
            last_message_at_ms: u64::try_from(row.last_message_at_ms)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        });
    }
    Ok(response(ListArchiveResponse { entries }))
}

async fn restore(
    State(state): State<SessionState>,
    RoutePath(id): RoutePath<String>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let id = parse_id(&id)?.to_string();
    let lock = session_lock(&state, &id).await;
    let _guard = lock.try_lock().map_err(|_| StatusCode::CONFLICT)?;
    let session = state
        .db
        .session(&id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    if !state.workspaces.contains(&session.session.workspace_id)? {
        return Err(StatusCode::NOT_FOUND);
    }
    if !state
        .db
        .restore_session(&id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn details(
    State(state): State<SessionState>,
    RoutePath(id): RoutePath<String>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let id = parse_id(&id)?;
    let (session, turns) = load_persisted(&state.db, id)?;
    if !state.workspaces.contains(&session.workspace_id)? {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(response(detail(&state.db, &session, &turns)?))
}

async fn subagent_details(
    State(state): State<SessionState>,
    RoutePath((id, agent_id)): RoutePath<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let id = parse_id(&id)?;
    let agent_id = parse_id(&agent_id)?;
    let (session, _) = load_persisted(&state.db, id)?;
    if !state.workspaces.contains(&session.workspace_id)? {
        return Err(StatusCode::NOT_FOUND);
    }
    let child = state
        .db
        .session_child(&session.id, &agent_id.to_string())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let values: Vec<Value> = serde_json::from_str(&child.turn.messages_json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut items = Vec::new();
    let mut has_reasoning = false;
    for value in values {
        let kind = value
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match kind {
            "assistant" | "reasoning" => {
                has_reasoning |= kind == "reasoning";
                if let Some(text) = value.get("text").and_then(Value::as_str) {
                    items.push(SessionSubagentItem {
                        kind: if kind == "assistant" {
                            "message"
                        } else {
                            "reasoning"
                        }
                        .to_owned(),
                        text: text.to_owned(),
                        tool: None,
                    });
                }
            }
            "tool" => {
                if let Some(id) = value.get("tool_call_id").and_then(Value::as_str)
                    && let Some(tool) = state
                        .db
                        .tool_call(&session.id, id)
                        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                {
                    items.push(SessionSubagentItem {
                        kind: "tool".to_owned(),
                        text: String::new(),
                        tool: Some(SessionToolSummary {
                            id: tool.id,
                            name: tool.name,
                            summary: tool.summary,
                            is_error: tool.is_error,
                            agent_role: child.parent_role.clone(),
                        }),
                    });
                }
            }
            _ => {}
        }
    }
    if !has_reasoning {
        let older: Vec<String> = serde_json::from_str(&child.turn.reasoning_json)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        for text in older {
            items.push(SessionSubagentItem {
                kind: "reasoning".to_owned(),
                text,
                tool: None,
            });
        }
    }
    let total_items = items.len();
    let offset = query.get("offset").map_or(Ok(0usize), |value| {
        value.parse::<usize>().map_err(|_| StatusCode::BAD_REQUEST)
    })?;
    if offset > total_items {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut page = Vec::new();
    let mut page_bytes = 0;
    for item in items.into_iter().skip(offset) {
        let size = item.encoded_len();
        if page.len() >= 64 || (!page.is_empty() && page_bytes + size > 500_000) {
            break;
        }
        page_bytes += size;
        page.push(item);
    }
    let next_offset = offset + page.len();
    Ok(response(SessionSubagentDetail {
        summary: Some(subagent_summary(&SessionChildSummaryRow::from(&child))?),
        items: page,
        result: if next_offset == total_items {
            child.turn.result.chars().take(16_000).collect()
        } else {
            String::new()
        },
        offset: offset as u64,
        next_offset: next_offset as u64,
        total_items: total_items as u64,
        task: child.turn.task,
    }))
}

#[cfg(feature = "runbooks")]
async fn run_runbook_cell(
    State(state): State<SessionState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<CellRunResponse>, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some("application/json")
    {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    let request: CellRunRequest =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    if request.cell_id.is_empty() || request.cell_id.len() > 128 || request.expected_revision == 0 {
        return Err(StatusCode::BAD_REQUEST);
    }
    let session_id = parse_id(&request.session_id)?.to_string();
    let lock = session_lock(&state, &session_id).await;
    let _guard = lock.try_lock().map_err(|_| StatusCode::CONFLICT)?;
    let (session, turns) = load_persisted(&state.db, parse_id(&session_id)?)?;
    if session.workspace_id != request.workspace_id
        || !state.workspaces.contains(&request.workspace_id)?
    {
        return Err(StatusCode::NOT_FOUND);
    }
    if !runbook_feature_enabled(&state.db, &request.workspace_id)? {
        return Err(StatusCode::FORBIDDEN);
    }
    let document = state
        .db
        .runbook_get(&request.workspace_id, &request.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    if document.revision != request.expected_revision {
        return Err(StatusCode::CONFLICT);
    }
    let (index, language, source) = document
        .cells
        .iter()
        .enumerate()
        .find_map(|(index, cell)| match cell {
            beale_contracts::runbooks::Cell::Code {
                id,
                language,
                source,
                ..
            } if id == &request.cell_id => Some((index, language, source)),
            _ => None,
        })
        .ok_or(StatusCode::BAD_REQUEST)?;
    let latest = turns.last().ok_or(StatusCode::CONFLICT)?;
    let provider = match ProviderId::try_from(latest.provider) {
        Ok(ProviderId::Openai) => Provider::OpenAi,
        Ok(ProviderId::Xai) => Provider::Xai,
        Ok(ProviderId::Anthropic) => Provider::Anthropic,
        Ok(ProviderId::Zai) => Provider::Zai,
        Ok(ProviderId::Openrouter) => Provider::OpenRouter,
        Ok(ProviderId::Custom) => Provider::Custom,
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    let mode = match AuthMethod::try_from(latest.method) {
        Ok(AuthMethod::Subscription) => AuthMode::Subscription,
        Ok(AuthMethod::ApiKey) => AuthMode::ApiKey,
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    let workspace_context = runbook_review_context(&state, &request.workspace_id)?;
    let review_args = serde_json::json!({"command":format!("# {language} runbook cell\n{source}")});
    let decision = beale_safety::review(
        &state.db,
        &state.provider,
        &state.credentials,
        ReviewRequest {
            provider,
            mode,
            model: &latest.model,
            name: "bash",
            arguments: &review_args,
            workspace_context: &workspace_context,
        },
    )
    .await;
    if !decision.approved {
        return Err(StatusCode::FORBIDDEN);
    }
    let workspace = state.workspaces.directory(&request.workspace_id)?;
    let run = state
        .db
        .runbook_run_start(
            &request.workspace_id,
            &session_id,
            &request.id,
            document.revision,
        )
        .map_err(|_| StatusCode::CONFLICT)?;
    let mut active = ActiveRunbookRun {
        db: state.db.clone(),
        id: run.id.clone(),
        finished: false,
    };
    state
        .db
        .runbook_run_cell(
            &run.id,
            u32::try_from(index + 1).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
            document.cells[index].name(),
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let result = beale_runbooks::run_selected_cell(&document, &request.cell_id, &workspace).await;
    let (success, output, exit_code, stdout, stderr) = match result {
        Ok(output) => (
            output.success,
            output.output,
            output.exit_code,
            output.stdout,
            output.stderr,
        ),
        Err(error) => {
            let error = format!("Runbook execution error: {error}");
            (false, error.clone(), None, String::new(), error)
        }
    };
    state
        .db
        .runbook_run_cell_result(
            &run.id,
            &request.cell_id,
            success,
            exit_code,
            &stdout,
            &stderr,
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    active
        .finish(success, &output)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(CellRunResponse {
        run_id: run.id,
        status: if success { "completed" } else { "failed" }.to_owned(),
    }))
}

async fn tool_details(
    State(state): State<SessionState>,
    RoutePath((id, tool_id)): RoutePath<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let id = parse_id(&id)?;
    let tool_id = parse_id(&tool_id)?;
    let offset = query.get("offset").map_or(Ok(0usize), |value| {
        value.parse::<usize>().map_err(|_| StatusCode::BAD_REQUEST)
    })?;
    let (session, _) = load_persisted(&state.db, id)?;
    if !state.workspaces.contains(&session.workspace_id)? {
        return Err(StatusCode::NOT_FOUND);
    }
    let row = state
        .db
        .tool_call(&session.id, &tool_id.to_string())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let (output, next) =
        page(&row.output, offset, TOOL_PAGE_BYTES).map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok(response(SessionToolDetailPage {
        id: row.id,
        name: row.name,
        arguments: row.arguments,
        output,
        offset: offset as u64,
        next_offset: next as u64,
        total_bytes: row.output.len() as u64,
        is_error: row.is_error,
    }))
}

async fn submit_turn(
    State(state): State<SessionState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let request: SubmitSessionTurnRequest = checked_body(&headers, &body)?;
    Ok(response(run_turn(state, request, None, None).await?))
}

async fn stop_turn(
    State(state): State<SessionState>,
    RoutePath(id): RoutePath<String>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let id = parse_id(&id)?;
    let stops = state.turn_stops.lock().await;
    let stop = stops.get(&id).ok_or(StatusCode::NOT_FOUND)?;
    stop.send_replace(true);
    Ok(StatusCode::NO_CONTENT)
}

async fn stream_turn(
    State(state): State<SessionState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let request: SubmitSessionTurnRequest = checked_body(&headers, &body)?;
    let stop_id = headers
        .get(SESSION_TURN_ID_HEADER)
        .map(|value| {
            value
                .to_str()
                .map_err(|_| StatusCode::BAD_REQUEST)
                .and_then(parse_id)
        })
        .transpose()?;
    let stop = if let Some(id) = stop_id {
        let (sender, receiver) = watch::channel(false);
        let mut stops = state.turn_stops.lock().await;
        if stops.contains_key(&id) {
            return Err(StatusCode::CONFLICT);
        }
        stops.insert(id, sender);
        Some(receiver)
    } else {
        None
    };
    let (tx, rx) = mpsc::channel::<SessionStreamEvent>(32);
    tokio::spawn(async move {
        let outcome = run_turn(state.clone(), request, Some(tx.clone()), stop).await;
        if let Some(id) = stop_id {
            state.turn_stops.lock().await.remove(&id);
        }
        let event = match outcome {
            Ok(detail) => SessionStreamEvent {
                runbook_progress: None,
                kind: SessionStreamKind::Completed as i32,
                text: String::new(),
                tool: None,
                detail: Some(detail),
                segment_index: 0,
                ..Default::default()
            },
            Err(status) => SessionStreamEvent {
                runbook_progress: None,
                kind: SessionStreamKind::Failed as i32,
                text: match status {
                    StatusCode::NOT_FOUND => "Workspace or session is unavailable.",
                    StatusCode::CONFLICT => "Session changed or reached its turn limit.",
                    StatusCode::BAD_REQUEST => "The session request is invalid.",
                    StatusCode::BAD_GATEWAY => "The provider could not complete this turn.",
                    _ => "Session request failed.",
                }
                .to_owned(),
                tool: None,
                detail: None,
                segment_index: 0,
                ..Default::default()
            },
        };
        let event = if session_stream_frame(&event).is_some() {
            event
        } else {
            SessionStreamEvent {
                runbook_progress: None,
                kind: SessionStreamKind::Failed as i32,
                text: "Session response is too large.".to_owned(),
                tool: None,
                detail: None,
                segment_index: 0,
                ..Default::default()
            }
        };
        let _ = tx.send(event).await;
    });
    let frames = stream::unfold((rx, false), |(mut rx, finished)| async move {
        if finished {
            return None;
        }
        while let Some(event) = rx.recv().await {
            if let Some(frame) = session_stream_frame(&event) {
                let terminal = matches!(
                    SessionStreamKind::try_from(event.kind),
                    Ok(SessionStreamKind::Completed | SessionStreamKind::Failed)
                );
                return Some((
                    Ok::<Bytes, std::io::Error>(Bytes::from(frame)),
                    (rx, terminal),
                ));
            }
        }
        None
    });
    Ok((
        [(header::CONTENT_TYPE, PROTOBUF_STREAM_CONTENT_TYPE)],
        Body::from_stream(frames),
    ))
}

fn reasoning_effort_name(value: i32) -> Result<Option<&'static str>, StatusCode> {
    match ReasoningEffort::try_from(value) {
        Ok(ReasoningEffort::Unspecified) => Ok(None),
        Ok(ReasoningEffort::None) => Ok(Some("none")),
        Ok(ReasoningEffort::Minimal) => Ok(Some("minimal")),
        Ok(ReasoningEffort::Low) => Ok(Some("low")),
        Ok(ReasoningEffort::Medium) => Ok(Some("medium")),
        Ok(ReasoningEffort::High) => Ok(Some("high")),
        Ok(ReasoningEffort::Xhigh) => Ok(Some("xhigh")),
        Ok(ReasoningEffort::Max) => Ok(Some("max")),
        Err(_) => Err(StatusCode::BAD_REQUEST),
    }
}

fn stop_requested(stop: &Option<watch::Receiver<bool>>) -> bool {
    stop.as_ref().is_some_and(|receiver| *receiver.borrow())
}

async fn until_stopped<F: Future>(
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
fn goal_signal_for_answer(
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
async fn run_turn(
    state: SessionState,
    mut request: SubmitSessionTurnRequest,
    progress: Option<mpsc::Sender<SessionStreamEvent>>,
    stop: Option<watch::Receiver<bool>>,
) -> Result<SessionDetail, StatusCode> {
    let lock = if request.session_id.is_empty() {
        Arc::new(Mutex::new(()))
    } else {
        session_lock(&state, &request.session_id).await
    };
    let _guard = lock.try_lock().map_err(|_| StatusCode::CONFLICT)?;
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
                let id = &detail
                    .summary
                    .as_ref()
                    .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?
                    .id;
                state
                    .db
                    .set_goal_status(id, GoalStatus::Paused as i32)
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                let (session, turns) = load_persisted(&state.db, parse_id(id)?)?;
                return self::detail(&state.db, &session, &turns);
            }
            if !state
                .db
                .feature_enabled(beale_goals::FEATURE_ID)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                || !state
                    .db
                    .extension_applies_to(
                        "feature",
                        beale_goals::FEATURE_ID,
                        &detail
                            .summary
                            .as_ref()
                            .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?
                            .workspace_id,
                    )
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            {
                let id = detail
                    .summary
                    .as_ref()
                    .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?
                    .id
                    .clone();
                state
                    .db
                    .set_goal_status(&id, GoalStatus::Paused as i32)
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                let (session, turns) = load_persisted(&state.db, parse_id(&id)?)?;
                return self::detail(&state.db, &session, &turns);
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
                .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?
                .id
                .clone();
            request.prompt = beale_goals::continuation_prompt(&goal.objective)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
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
async fn run_turn(
    state: SessionState,
    request: SubmitSessionTurnRequest,
    progress: Option<mpsc::Sender<SessionStreamEvent>>,
    stop: Option<watch::Receiver<bool>>,
) -> Result<SessionDetail, StatusCode> {
    if request.goal_mode {
        return Err(StatusCode::BAD_REQUEST);
    }
    let lock = if request.session_id.is_empty() {
        Arc::new(Mutex::new(()))
    } else {
        session_lock(&state, &request.session_id).await
    };
    let _guard = lock.try_lock().map_err(|_| StatusCode::CONFLICT)?;
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

async fn run_one_turn(
    state: &SessionState,
    request: SubmitSessionTurnRequest,
    progress: Option<mpsc::Sender<SessionStreamEvent>>,
    automatic: bool,
    lock: &Arc<Mutex<()>>,
    mut stop: Option<watch::Receiver<bool>>,
    #[cfg(feature = "graphs")] peer: Option<PeerRun>,
) -> Result<SessionDetail, StatusCode> {
    let started = Instant::now();
    let _workspace_id = parse_id(&request.workspace_id)?;
    if !state.workspaces.contains(&request.workspace_id)? {
        return Err(StatusCode::NOT_FOUND);
    }
    if request.prompt.trim().is_empty()
        || request.prompt.len() > 16_384
        || request.model.is_empty()
        || request.model.len() > 200
        || !request.model.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
        })
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let provider = match ProviderId::try_from(request.provider) {
        Ok(ProviderId::Openai) => Provider::OpenAi,
        Ok(ProviderId::Xai) => Provider::Xai,
        Ok(ProviderId::Anthropic) => Provider::Anthropic,
        Ok(ProviderId::Zai) => Provider::Zai,
        Ok(ProviderId::Openrouter) => Provider::OpenRouter,
        Ok(ProviderId::Custom) => Provider::Custom,
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    let mode = match AuthMethod::try_from(request.method) {
        Ok(AuthMethod::Subscription) => AuthMode::Subscription,
        Ok(AuthMethod::ApiKey) => AuthMode::ApiKey,
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    if provider == Provider::Custom && mode != AuthMode::ApiKey {
        return Err(StatusCode::BAD_REQUEST);
    }
    let reasoning_effort = reasoning_effort_name(request.reasoning_effort)?;
    if !request.goal_mode && (request.goal_max_turns != 0 || request.goal_max_duration_minutes != 0)
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    if request.goal_max_turns > i64::MAX as u64
        || request.goal_max_duration_minutes > i64::MAX as u64 / 60_000
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    if !request.repeat_preset.is_empty()
        && (!request.session_id.is_empty()
            || !matches!(
                request.repeat_preset.as_str(),
                "minute" | "hour" | "day" | "week" | "month"
            ))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
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
        return Err(StatusCode::BAD_REQUEST);
    }
    let graph_config = encoded_graph_config(&request);
    #[cfg(not(feature = "goal-mode"))]
    if request.goal_mode {
        return Err(StatusCode::BAD_REQUEST);
    }
    #[cfg(not(feature = "goal-mode"))]
    let _ = automatic;
    let (mut session, mut turns) = if request.session_id.is_empty() {
        let id = Uuid::new_v4();
        register_session_lock(state, &id.to_string(), lock).await;
        let created_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .as_millis();
        (
            StoredSession {
                id: id.to_string(),
                workspace_id: request.workspace_id.clone(),
                title: title(&request.prompt),
                created_at_ms,
                last_message_at_ms: u64::try_from(created_at_ms)
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
            },
            Vec::new(),
        )
    } else {
        let id = parse_id(&request.session_id)?;
        let (session, turns) = load_persisted(&state.db, id)?;
        if session.workspace_id != request.workspace_id {
            return Err(StatusCode::CONFLICT);
        }
        (session, turns)
    };
    let next_turn_number = turns.last().map_or(Ok(1), |turn| {
        turn.number.checked_add(1).ok_or(StatusCode::CONFLICT)
    })?;
    #[cfg(feature = "graphs")]
    let next_turn_number = peer
        .as_ref()
        .map_or(next_turn_number, |peer| peer.turn_number);
    let persisted_goal = state
        .db
        .goal(&session.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    #[cfg(feature = "goal-mode")]
    let mut goal = persisted_goal;
    #[cfg(all(feature = "graphs", feature = "goal-mode"))]
    if peer.as_ref().is_some_and(|peer| {
        !matches!(
            peer.role,
            beale_graphs::Role::Discovery | beale_graphs::Role::DiscoveryHarness
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
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    #[cfg(feature = "goal-mode")]
    {
        let feature_enabled = state
            .db
            .feature_enabled(beale_goals::FEATURE_ID)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            && state
                .db
                .extension_applies_to("feature", beale_goals::FEATURE_ID, &session.workspace_id)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if request.goal_mode && !feature_enabled {
            return Err(StatusCode::CONFLICT);
        }
        if automatic && !feature_enabled {
            state
                .db
                .set_goal_status(&session.id, GoalStatus::Paused as i32)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            return detail(&state.db, &session, &turns);
        }
        if !feature_enabled
            && let Some(goal) = goal.as_mut()
            && goal.status == GoalStatus::Active as i32
        {
            goal.status = GoalStatus::Paused as i32;
        }
        if request.goal_mode && !turns.is_empty() {
            return Err(StatusCode::BAD_REQUEST);
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
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
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
        return Err(StatusCode::CONFLICT);
    }
    let workspace = state.workspaces.directory(&request.workspace_id)?;
    let workspace_details = state.workspaces.details_for_host(&request.workspace_id)?;
    #[cfg(feature = "webfetch")]
    let webfetch_tools_enabled = webfetch_feature_enabled(&state.db, &request.workspace_id)?;
    let surface_tools_enabled = surface_feature_enabled(&state.db, &request.workspace_id)?;
    let memories_tools_enabled = memories_feature_enabled(&state.db, &request.workspace_id)?;
    #[cfg(feature = "agent-feedback")]
    let agent_feedback_tools_enabled =
        agent_feedback_feature_enabled(&state.db, &request.workspace_id)?;
    #[cfg(feature = "findings")]
    let findings_tools_enabled = findings_feature_enabled(&state.db, &request.workspace_id)?;
    #[cfg(feature = "runbooks")]
    let runbook_tools_enabled = runbook_feature_enabled(&state.db, &request.workspace_id)?;
    #[cfg(not(feature = "memories"))]
    let _ = memories_tools_enabled;
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
    let workspace_context = format!(
        "Guidance: {}\nRules: {}\n{}",
        workspace_details.guidance,
        workspace_details.rules.join("; "),
        surfaces_context,
    );
    #[cfg(feature = "graphs")]
    let saved_graph_id = if request.graph_id.is_empty() {
        state
            .db
            .session_graph_id(&session.id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .unwrap_or_default()
    } else {
        request.graph_id.clone()
    };
    #[cfg(feature = "graphs")]
    let active_graph = if saved_graph_id.is_empty()
        || !state
            .db
            .feature_enabled(beale_graphs::FEATURE_ID)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        || !state
            .db
            .extension_applies_to("feature", beale_graphs::FEATURE_ID, &session.workspace_id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        None
    } else {
        beale_graphs::runtime(&saved_graph_id)
    };
    #[cfg(feature = "graphs")]
    let active_graph_config = if active_graph.is_some() {
        if let Some(bytes) = &graph_config {
            SessionGraphConfig::decode(bytes.as_slice())
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        } else {
            state
                .db
                .session_graph_config(&session.id)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                .filter(|bytes| !bytes.is_empty())
                .map(|bytes| SessionGraphConfig::decode(bytes.as_slice()))
                .transpose()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
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
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    #[cfg(not(feature = "graphs"))]
    let saved_context = state
        .db
        .provider_context(&session.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let reuse_saved_context = provider == Provider::OpenAi && reuse_prior_context;
    #[cfg(feature = "graphs")]
    let reuse_saved_context = reuse_saved_context || peer.is_some();
    if reuse_saved_context && let Some(saved) = saved_context {
        let mut previous: Vec<Value> =
            serde_json::from_str(&saved).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
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
    #[allow(unused_mut)]
    let mut available_features: Vec<(&str, &str)> = Vec::new();
    #[cfg(feature = "webfetch")]
    if webfetch_tools_enabled {
        available_features.push((
            beale_webfetch::FEATURE_ID,
            "Use to search the web or retrieve readable page text and links.",
        ));
    }
    #[cfg(feature = "surfaces")]
    if surface_tools_enabled {
        available_features.push((
            beale_surfaces::FEATURE_ID,
            "Use to inspect or maintain the workspace surface map and relationships.",
        ));
    }
    #[cfg(feature = "findings")]
    if findings_tools_enabled {
        available_features.push((
            beale_findings::FEATURE_ID,
            "Use to track candidate findings through validation, screening, and reporting.",
        ));
    }
    #[cfg(feature = "memories")]
    if memories_tools_enabled {
        available_features.push((
            beale_memories::FEATURE_ID,
            "Use to search prior items in this session and sourced workspace research memories.",
        ));
    }
    #[cfg(feature = "agent-feedback")]
    if agent_feedback_tools_enabled {
        available_features.push((
            beale_agent_feedback::FEATURE_ID,
            "Use to record an observed Beale harness or agent runtime issue for development review.",
        ));
    }
    #[cfg(feature = "runbooks")]
    if runbook_tools_enabled {
        available_features.push((
            beale_runbooks::FEATURE_ID,
            "Use to find, create, edit, run, or check reusable runbooks.",
        ));
    }
    #[cfg(feature = "goal-mode")]
    if goal_active {
        available_features.push((
            beale_goals::FEATURE_ID,
            "Use for the active multi-turn goal and reporting final goal status.",
        ));
    }
    #[cfg(feature = "graphs")]
    if active_graph.is_some() {
        available_features.push((
            beale_graphs::FEATURE_ID,
            "Use to delegate bounded research tasks to graph subagents.",
        ));
    }
    let available_plugins = state
        .plugins
        .available(&request.workspace_id)
        .map_err(|error| {
            eprintln!("plugin directory failed: {error:#}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let agent_files = AgentFiles::new().map_err(|error| {
        eprintln!("agent files failed: {error:#}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let mut definitions = definitions();
    let mut extension_entries = String::new();
    #[cfg(feature = "graphs")]
    let mut child_extension_entries = String::new();
    for (id, use_case) in &available_features {
        let (tools, guidance): (Vec<Value>, &str) = match *id {
            #[cfg(feature = "webfetch")]
            beale_webfetch::FEATURE_ID => (
                vec![beale_webfetch::definition()],
                beale_webfetch::PROMPT_EXTENSION,
            ),
            #[cfg(feature = "agent-feedback")]
            beale_agent_feedback::FEATURE_ID => (
                vec![beale_agent_feedback::definition()],
                beale_agent_feedback::PROMPT_EXTENSION,
            ),
            #[cfg(feature = "surfaces")]
            beale_surfaces::FEATURE_ID => (
                beale_surfaces::definitions(),
                beale_surfaces::PROMPT_EXTENSION,
            ),
            #[cfg(feature = "findings")]
            beale_findings::FEATURE_ID => (
                beale_findings::definitions(),
                beale_findings::PROMPT_EXTENSION,
            ),
            #[cfg(feature = "memories")]
            beale_memories::FEATURE_ID => (
                beale_memories::definitions(),
                beale_memories::PROMPT_EXTENSION,
            ),
            #[cfg(feature = "runbooks")]
            beale_runbooks::FEATURE_ID => (
                beale_runbooks::definitions(),
                beale_runbooks::PROMPT_EXTENSION,
            ),
            #[cfg(feature = "goal-mode")]
            beale_goals::FEATURE_ID => (
                vec![beale_goals::definition()],
                goal_section
                    .as_deref()
                    .unwrap_or(beale_goals::PROMPT_EXTENSION),
            ),
            #[cfg(feature = "graphs")]
            beale_graphs::FEATURE_ID => (
                active_graph
                    .map(|runtime| vec![runtime.tool_definition()])
                    .unwrap_or_default(),
                graph_section
                    .as_deref()
                    .unwrap_or(beale_graphs::PROMPT_EXTENSION),
            ),
            _ => (Vec::new(), ""),
        };
        if tools.is_empty() && guidance.is_empty() {
            continue;
        }
        let path = agent_files
            .write(&format!("features/{id}/FEATURE.md"), guidance.as_bytes())
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let entry = format!(
            "- Feature `{id}`: {use_case} Read {}.\n",
            json!(path.to_string_lossy())
        );
        extension_entries.push_str(&entry);
        #[cfg(feature = "graphs")]
        if graphs::child_supports_feature(id) {
            child_extension_entries.push_str(&entry);
        }
        definitions.extend(tools);
    }
    let mut callable_plugins = HashSet::new();
    for plugin in &available_plugins {
        match state
            .plugins
            .publish(&plugin.name, &request.workspace_id, &agent_files)
            .await
        {
            Ok((path, has_tools)) => {
                extension_entries.push_str(&format!(
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
                eprintln!("plugin preparation failed: {error:#}");
                extension_entries.push_str(&format!("- Plugin `{}` is unavailable this turn because its files or servers could not be prepared.\n", plugin.name));
            }
        }
    }
    if !callable_plugins.is_empty() {
        definitions.push(plugin_call_definition());
    }
    let catalog_section =
        system_prompt::extension_catalog(&agent_files, "EXTENSIONS.md", &extension_entries)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    #[cfg(feature = "memories")]
    let memory_section = if memories_tools_enabled {
        match beale_memories::MemoryStore::open(state.workspaces.root())
            .and_then(|store| store.recall(&request.workspace_id, &request.prompt))
        {
            Ok(value) => value,
            Err(error) => {
                eprintln!("memory recall failed: {error:#}");
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
        eprintln!("session system prompt failed: {error:#}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    #[cfg(feature = "graphs")]
    let child_system = if active_graph.is_some() {
        let catalog_section = system_prompt::extension_catalog(
            &agent_files,
            "subagents/EXTENSIONS.md",
            &child_extension_entries,
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        system_prompt::load(
            state.workspaces.root(),
            &[catalog_section.as_str(), memory_section.as_str()],
        )
        .map_err(|error| {
            eprintln!("subagent system prompt failed: {error:#}");
            StatusCode::INTERNAL_SERVER_ERROR
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
        return Err(StatusCode::BAD_REQUEST);
    }
    #[cfg(feature = "graphs")]
    let previous_role = if active_graph.is_some() && !turns.is_empty() {
        state
            .db
            .session_last_top_level_role(&session.id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
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
    if peer.is_some() && saved_graph_id == beale_graphs::OPENAI_ID {
        definitions.retain(|tool| {
            let name = tool["name"].as_str().unwrap_or_default();
            !beale_findings::is_tool(name)
                || !matches!(
                    name,
                    "findings_create" | "findings_complete_state" | "findings_reject"
                )
                || match name {
                    "findings_create" => active_role == beale_graphs::Role::Discovery,
                    "findings_complete_state" => matches!(
                        active_role,
                        beale_graphs::Role::Validation | beale_graphs::Role::Reporter
                    ),
                    "findings_reject" => active_role == beale_graphs::Role::Validation,
                    _ => true,
                }
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
    #[cfg(feature = "graphs")]
    let (mut provider, mut mode) =
        graphs::provider_for(&active_model).ok_or(StatusCode::BAD_REQUEST)?;
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
    let child_message_sequence = AtomicU32::new(0x8000_0000);
    #[cfg(not(feature = "graphs"))]
    let child_turns = Vec::<ChildTurnRow>::new();
    let mut reasoning_rows = Vec::<ReasoningSummaryRow>::new();
    let mut commentary_rows = Vec::<CommentaryRow>::new();
    let mut model_usage = Vec::<ModelUsageRow>::new();
    #[cfg(feature = "graphs")]
    let mut peer_messages = Vec::<String>::new();
    let answer: Option<String>;
    let mut segment_index = 0_u32;
    'rounds: loop {
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
        let step = {
            let step_request = ToolStepRequest {
                model: &active_model.model,
                input: &input,
                tools: &definitions,
                instructions: (provider == Provider::OpenAi).then_some(system.as_str()),
                reasoning_effort,
                workspace_root: Some(&workspace),
            };
            let provider_call = async {
                if provider == Provider::OpenAi {
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
                }
            };
            until_stopped(provider_call, &mut stop).await
        };
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
                eprintln!("session provider turn failed: {error:#}");
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
                    return Err(StatusCode::BAD_GATEWAY);
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
                    return Err(StatusCode::BAD_GATEWAY);
                }
                answer = Some(format!(
                    "{INTERRUPTED_ANSWER} The provider returned an invalid tool call."
                ));
                break 'rounds;
            }
            let id = Uuid::new_v4().to_string();
            let args: Result<Value, _> = serde_json::from_str(&call.arguments);
            let summary = if call.name == "goal_status" {
                "Record Goal Mode status".to_owned()
            } else if cfg!(feature = "agent-feedback") && call.name == "agent_feedback" {
                "Record agent feedback".to_owned()
            } else if cfg!(feature = "surfaces") && call.name.starts_with("surface_") {
                format!("{} surface", call.name.trim_start_matches("surface_"))
            } else if cfg!(feature = "findings") && call.name.starts_with("findings_") {
                format!(
                    "{} finding",
                    call.name.trim_start_matches("findings_").replace('_', " ")
                )
            } else if cfg!(feature = "memories") && call.name.starts_with("memory_") {
                format!(
                    "{} memory",
                    call.name.trim_start_matches("memory_").replace('_', " ")
                )
            } else if cfg!(feature = "runbooks") && call.name.starts_with("runbook_") {
                format!("{} runbook", call.name.trim_start_matches("runbook_"))
            } else if cfg!(feature = "graphs") && call.name == "graph_delegate" {
                "Delegate to graph subagents".to_owned()
            } else if cfg!(feature = "graphs") && call.name == "graph_handoff" {
                format!(
                    "Hand off to {}",
                    args.as_ref()
                        .ok()
                        .and_then(|value| value.get("role"))
                        .and_then(Value::as_str)
                        .unwrap_or("peer agent")
                )
            } else {
                tool_summary(&call.name, args.as_ref().unwrap_or(&Value::Null))
            };
            #[cfg(feature = "graphs")]
            let summary = if peer.is_none()
                && active_graph.is_some()
                && matches!(
                    saved_graph_id.as_str(),
                    beale_graphs::CLOUDFLARE_ID | beale_graphs::OPENAI_ID
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
            #[cfg(feature = "graphs")]
            let mut child_tools = Vec::<ToolCallRow>::new();
            let mut review_unavailable = false;
            let (review_denial, feature_review_approved) =
                if feature_tool_requires_review(&call.name) {
                    if let Ok(arguments) = args.as_ref() {
                        match until_stopped(
                            beale_safety::review(
                                &state.db,
                                &state.provider,
                                &state.credentials,
                                ReviewRequest {
                                    provider,
                                    mode,
                                    model: &active_model.model,
                                    name: &call.name,
                                    arguments,
                                    workspace_context: &workspace_context,
                                },
                            ),
                            &mut stop,
                        )
                        .await
                        {
                            Some(decision) => {
                                review_unavailable = review_failed(&decision);
                                (
                                    (!decision.approved).then_some(decision.reason),
                                    decision.approved && decision.reviewed,
                                )
                            }
                            None => (Some("Stopped by user".to_owned()), false),
                        }
                    } else {
                        (None, false)
                    }
                } else {
                    (None, false)
                };
            #[cfg(not(feature = "surfaces"))]
            let _ = feature_review_approved;
            let (output, is_error) = if let Some(reason) = review_denial {
                (format!("Tool denied by review: {reason}"), true)
            } else {
                match args {
                    #[cfg(feature = "webfetch")]
                    Ok(args) if call.name == beale_webfetch::TOOL_NAME => {
                        match until_stopped(
                            execute_webfetch(&state.db, &request.workspace_id, &args),
                            &mut stop,
                        )
                        .await
                        {
                            Some(Ok(value)) => (value.output, value.is_error),
                            Some(Err(error)) => (format!("Webfetch error: {error:#}"), true),
                            None => ("Webfetch stopped by user.".to_owned(), true),
                        }
                    }
                    Ok(args) if call.name == "plugin_call" => {
                        let result = async {
                            let parsed: PluginCallArgs = serde_json::from_value(args)?;
                            anyhow::ensure!(
                                parsed.arguments.is_object(),
                                "arguments must be an object"
                            );
                            anyhow::ensure!(
                                callable_plugins.contains(&parsed.plugin),
                                "plugin tools are unavailable in this workspace"
                            );
                            let review_args = json!({"plugin":parsed.plugin,"server":parsed.server,"tool":parsed.tool,"arguments":parsed.arguments});
                            let decision = beale_safety::review(
                                &state.db,
                                &state.provider,
                                &state.credentials,
                                ReviewRequest {
                                    provider,
                                    mode,
                                    model: &active_model.model,
                                    name: "plugin_call",
                                    arguments: &review_args,
                                    workspace_context: &workspace_context,
                                },
                            )
                            .await;
                            anyhow::ensure!(
                                decision.approved,
                                "plugin tool denied by review: {}",
                                decision.reason
                            );
                            let value = state
                                .plugins
                                .call_tool(
                                    &parsed.plugin,
                                    &parsed.server,
                                    &parsed.tool,
                                    parsed.arguments,
                                    &request.workspace_id,
                                )
                                .await?;
                            Ok::<_, anyhow::Error>((
                                value.to_string(),
                                value["isError"].as_bool().unwrap_or(false),
                            ))
                        };
                        let result = until_stopped(result, &mut stop).await;
                        match result {
                            Some(Ok(value)) => value,
                            Some(Err(error)) => (format!("Plugin tool error: {error}"), true),
                            None => ("Plugin tool stopped by user.".to_owned(), true),
                        }
                    }
                    #[cfg(feature = "runbooks")]
                    Ok(args) if runbook_tools_enabled && call.name == beale_runbooks::RUN => {
                        let result = async {
                            let runbook_id = args
                                .get("id")
                                .and_then(Value::as_str)
                                .ok_or_else(|| anyhow::anyhow!("id is required"))?;
                            let document = state
                                .db
                                .runbook_get(&request.workspace_id, runbook_id)?
                                .ok_or_else(|| anyhow::anyhow!("runbook not found"))?;
                            for cell in &document.cells {
                                if let beale_contracts::runbooks::Cell::Code {
                                    language,
                                    source,
                                    ..
                                } = cell
                                {
                                    let review_args = serde_json::json!({"command":format!("# {language} runbook cell\n{source}")});
                                    let decision = beale_safety::review(
                                        &state.db,
                                        &state.provider,
                                        &state.credentials,
                                        ReviewRequest {
                                            provider,
                                            mode,
                                            model: &active_model.model,
                                            name: "bash",
                                            arguments: &review_args,
                                            workspace_context: &workspace_context,
                                        },
                                    )
                                    .await;
                                    if !decision.approved {
                                        anyhow::bail!(
                                            "runbook cell denied by review: {}",
                                            decision.reason
                                        );
                                    }
                                }
                            }
                            let run = state.db.runbook_run_start(
                                &request.workspace_id,
                                &session.id,
                                runbook_id,
                                document.revision,
                            )?;
                            let mut active = ActiveRunbookRun {
                                db: state.db.clone(),
                                id: run.id.clone(),
                                finished: false,
                            };
                            let progress_tx = progress.clone();
                            let progress_title = document.title.clone();
                            let progress_id = runbook_id.to_owned();
                            let result = beale_runbooks::run_document(
                                &document,
                                &workspace,
                                |cell_number| {
                                    let cell_name =
                                        document.cells[(cell_number - 1) as usize].name();
                                    state.db.runbook_run_cell(&run.id, cell_number, cell_name)?;
                                    if let Some(tx) = &progress_tx {
                                        let _ = tx.try_send(SessionStreamEvent {
                                            kind: SessionStreamKind::RunbookProgress as i32,
                                            runbook_progress: Some(
                                                beale_contracts::v1::RunbookProgress {
                                                    runbook_id: progress_id.clone(),
                                                    title: progress_title.clone(),
                                                    cell_number,
                                                    cell_name: cell_name.to_owned(),
                                                },
                                            ),
                                            ..Default::default()
                                        });
                                    }
                                    Ok(())
                                },
                                |cell_id, success, exit_code, stdout, stderr| {
                                    state.db.runbook_run_cell_result(
                                        &run.id, cell_id, success, exit_code, stdout, stderr,
                                    )
                                },
                            )
                            .await;
                            let (success, output) = match result {
                                Ok(result) => (result.success, result.output),
                                Err(error) => (false, format!("Runbook execution error: {error}")),
                            };
                            active.finish(success, &output)?;
                            Ok((serde_json::json!({"run_id":run.id,"runbook_id":runbook_id,"status":if success {"completed"} else {"failed"}}).to_string(), !success))
                        };
                        let result = until_stopped(result, &mut stop).await;
                        match result {
                            Some(Ok(value)) => value,
                            Some(Err(error)) => (format!("Runbook tool error: {error}"), true),
                            None => ("Runbook stopped by user.".to_owned(), true),
                        }
                    }
                    #[cfg(feature = "runbooks")]
                    Ok(args) if runbook_tools_enabled && beale_runbooks::is_tool(&call.name) => {
                        match beale_runbooks::execute(
                            &state.db,
                            &request.workspace_id,
                            &session.id,
                            &call.name,
                            &args,
                        ) {
                            Ok(value) => (value, false),
                            Err(error) => (format!("Runbook tool error: {error}"), true),
                        }
                    }
                    #[cfg(feature = "findings")]
                    Ok(args) if findings_tools_enabled && beale_findings::is_tool(&call.name) => {
                        let allowed = {
                            #[cfg(feature = "graphs")]
                            {
                                peer.is_none()
                                    || saved_graph_id != beale_graphs::OPENAI_ID
                                    || pipeline_finding_allowed(active_role, &call.name, &args)
                            }
                            #[cfg(not(feature = "graphs"))]
                            {
                                true
                            }
                        };
                        if !allowed {
                            ("This finding action belongs to another Research Pipeline agent. Hand off the finding and evidence to that role.".to_owned(), true)
                        } else {
                            let pending = tool_rows
                                .iter()
                                .map(|row| beale_findings::PendingSource {
                                    id: &row.id,
                                    name: &row.name,
                                    output: &row.output,
                                    is_error: row.is_error,
                                })
                                .collect::<Vec<_>>();
                            match beale_findings::execute(
                                &state.db,
                                &request.workspace_id,
                                &session.id,
                                &call.name,
                                &args,
                                &pending,
                            ) {
                                Ok(value) => (value, false),
                                Err(error) => (format!("Findings tool error: {error}"), true),
                            }
                        }
                    }
                    #[cfg(feature = "memories")]
                    Ok(args) if memories_tools_enabled && beale_memories::is_tool(&call.name) => {
                        match beale_memories::MemoryStore::open(state.workspaces.root()).and_then(
                            |store| {
                                beale_memories::execute(
                                    &store,
                                    &state.db,
                                    &request.workspace_id,
                                    &session.id,
                                    &call.name,
                                    &args,
                                )
                            },
                        ) {
                            Ok(value) => (value, false),
                            Err(error) => (format!("Memory tool error: {error}"), true),
                        }
                    }
                    #[cfg(feature = "agent-feedback")]
                    Ok(args)
                        if agent_feedback_tools_enabled
                            && call.name == beale_agent_feedback::TOOL_NAME =>
                    {
                        match beale_agent_feedback::execute(
                            state.workspaces.root(),
                            &request.workspace_id,
                            &session.id,
                            &active_model.node_id,
                            &args,
                        ) {
                            Ok(value) => (value, false),
                            Err(error) => (format!("Agent feedback error: {error}"), true),
                        }
                    }
                    #[cfg(feature = "surfaces")]
                    Ok(args) if surface_tools_enabled && beale_surfaces::is_tool(&call.name) => {
                        match beale_surfaces::execute(
                            &state.workspaces,
                            &request.workspace_id,
                            &call.name,
                            &args,
                            feature_review_approved,
                        ) {
                            Ok(value) => (value, false),
                            Err(error) => (format!("Surface tool error: {error}"), true),
                        }
                    }
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
                                        let item = beale_storage::GraphHandoffRow {
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
                    Ok(args)
                        if call.name == beale_graphs::DELEGATE_TOOL && active_graph.is_some() =>
                    {
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
                            let child_usage = Mutex::new(Vec::new());
                            let result = until_stopped(
                                graphs::delegate(
                                    &saved_graph_id,
                                    graphs::ChildContext {
                                        parent_role: active_role,
                                        state,
                                        workspace: &workspace,
                                        agent_files: &agent_files,
                                        #[cfg(any(
                                            feature = "agent-feedback",
                                            feature = "webfetch"
                                        ))]
                                        workspace_id: &request.workspace_id,
                                        #[cfg(feature = "webfetch")]
                                        webfetch_enabled: webfetch_tools_enabled,
                                        #[cfg(feature = "agent-feedback")]
                                        session_id: &session.id,
                                        #[cfg(feature = "agent-feedback")]
                                        feedback_enabled: agent_feedback_tools_enabled,
                                        workspace_context: &workspace_context,
                                        surfaces_context: &surfaces_context,
                                        base_system: &child_system,
                                        reasoning_effort,
                                        turn_number: next_turn_number,
                                        turn_started: started,
                                        progress: progress.as_ref(),
                                        message_sequence: &child_message_sequence,
                                        model_usage: &child_usage,
                                    },
                                    &args,
                                    &active_graph_config,
                                    &graph_fallback,
                                ),
                                &mut stop,
                            )
                            .await;
                            model_usage.extend(child_usage.into_inner());
                            if let Some(result) = result {
                                child_tools = result.tools;
                                child_turns.extend(result.child_turns);
                                (result.output, result.is_error)
                            } else {
                                ("Delegation stopped by user.".to_owned(), true)
                            }
                        }
                    }
                    Ok(args) => {
                        let decision =
                            if matches!(call.name.as_str(), "read" | "write" | "edit" | "bash") {
                                until_stopped(
                                    beale_safety::review(
                                        &state.db,
                                        &state.provider,
                                        &state.credentials,
                                        ReviewRequest {
                                            provider,
                                            mode,
                                            model: &active_model.model,
                                            name: &call.name,
                                            arguments: &args,
                                            workspace_context: &workspace_context,
                                        },
                                    ),
                                    &mut stop,
                                )
                                .await
                            } else {
                                None
                            };
                        if decision.as_ref().is_some_and(review_failed) {
                            review_unavailable = true;
                        }
                        if stop_requested(&stop) {
                            ("Tool stopped by user.".to_owned(), true)
                        } else if let Some(decision) = decision
                            && !decision.approved
                        {
                            (format!("Tool denied by review: {}", decision.reason), true)
                        } else {
                            match until_stopped(
                                execute(&call.name, &args, &workspace, state.workspaces.root()),
                                &mut stop,
                            )
                            .await
                            {
                                Some(Ok(value)) => (value.output, value.is_error),
                                Some(Err(error)) => (format!("Tool error: {error:#}"), true),
                                None => ("Tool stopped by user.".to_owned(), true),
                            }
                        }
                    }
                    Err(_) => ("Tool error: invalid JSON arguments".to_owned(), true),
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
                });
                #[cfg(feature = "graphs")]
                for mut child in child_tools {
                    child.ordinal = tool_rows.len() as i64;
                    tool_rows.push(child);
                }
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
            if review_unavailable {
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
            (provider, mode) =
                graphs::provider_for(&active_model).ok_or(StatusCode::BAD_REQUEST)?;
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
    let mut answer = answer.ok_or(StatusCode::BAD_GATEWAY)?;
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
            return Err(StatusCode::BAD_GATEWAY);
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
        goal.turns_used = goal.turns_used.checked_add(1).ok_or(StatusCode::CONFLICT)?;
        goal.active_ms = goal.active_ms.saturating_add(
            i64::try_from(started.elapsed().as_millis().min(i64::MAX as u128))
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
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
    let latest = turns.last().ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    #[cfg(feature = "graphs")]
    let save_peer_context = peer.is_some();
    #[cfg(not(feature = "graphs"))]
    let save_peer_context = false;
    if provider == Provider::OpenAi {
        close_unanswered_openai_calls(&mut input);
    }
    let provider_context_json = if save_peer_context
        || (provider == Provider::OpenAi && !latest.answer.starts_with(INTERRUPTED_ANSWER))
    {
        let context = if provider == Provider::OpenAi {
            compact_openai_context(&input)
        } else {
            input.clone()
        };
        let encoded =
            serde_json::to_string(&context).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        (encoded.len() <= 8_388_608).then_some(encoded)
    } else {
        None
    };
    session.last_message_at_ms = u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .as_millis(),
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let session_row = SessionRow {
        id: session.id.clone(),
        workspace_id: session.workspace_id.clone(),
        title: session.title.clone(),
        created_at_ms: i64::try_from(session.created_at_ms)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        last_message_at_ms: i64::try_from(session.last_message_at_ms)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    };
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
                duration_ms: i64::try_from(latest.duration_ms)
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
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
                goal: goal.as_ref(),
            },
            (!repeat_preset.is_empty()).then_some((repeat_preset.as_str(), automation_options)),
            TurnMetadata {
                provider_context_json: provider_context_json.as_deref(),
                graph_id: (!request.graph_id.is_empty()).then_some(request.graph_id.as_str()),
                graph_config: graph_config.as_deref(),
                model_usage: Some(&model_usage),
                child_turns: &child_turns,
                queue_memories: memories_tools_enabled,
            },
        )
        .map_err(|error| {
            eprintln!("session turn save failed: {error:#}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    #[cfg(feature = "graphs")]
    if peer.is_some() {
        let (saved, turns) = load_persisted(&state.db, parse_id(&session.id)?)?;
        return detail(&state.db, &saved, &turns);
    }
    trim_recent_turns(&mut turns);
    detail(&state.db, &session, &turns)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::{body::Body, http::Request};
    #[cfg(any(feature = "agent-feedback", feature = "webfetch"))]
    use beale_storage::WorkspaceRow;
    use prost::Message;
    use tower::ServiceExt;

    #[test]
    fn disabled_review_rules_do_not_stop_approved_tools() {
        assert!(!review_failed(&beale_safety::ReviewDecision {
            approved: true,
            reviewed: false,
            reason: "No active review rules".to_owned(),
        }));
        assert!(review_failed(&beale_safety::ReviewDecision {
            approved: false,
            reviewed: false,
            reason: "Review unavailable".to_owned(),
        }));
    }

    #[test]
    fn successful_workspace_reads_expose_the_exact_finding_evidence_reference() {
        let files = AgentFiles::new().unwrap();
        let id = "tool-example-001";
        let result = model_tool_preview("read", id, "Example source", false, &files);
        assert!(result.contains("\"kind\":\"tool_call\",\"reference\":\"tool-example-001\""));
        assert!(result.ends_with("Example source"));
        assert_eq!(
            model_tool_preview("bash", id, "Denied", true, &files),
            "Denied"
        );
        assert_eq!(
            model_tool_preview("findings_read", id, "{}", false, &files),
            "{}"
        );
        std::fs::remove_dir_all(files.root()).unwrap();
    }

    #[cfg(all(feature = "graphs", feature = "findings"))]
    #[test]
    fn pipeline_finding_stages_follow_top_level_responsibilities() {
        use beale_graphs::Role;
        assert!(pipeline_finding_allowed(
            Role::Discovery,
            "findings_create",
            &json!({})
        ));
        assert!(pipeline_finding_allowed(
            Role::Discovery,
            "findings_update",
            &json!({"state":"candidate"})
        ));
        assert!(!pipeline_finding_allowed(
            Role::Discovery,
            "findings_complete_state",
            &json!({"state":"validated"})
        ));
        assert!(pipeline_finding_allowed(
            Role::Validation,
            "findings_complete_state",
            &json!({"state":"validated"})
        ));
        assert!(!pipeline_finding_allowed(
            Role::Validation,
            "findings_complete_state",
            &json!({"state":"screened"})
        ));
        assert!(pipeline_finding_allowed(
            Role::Reporter,
            "findings_complete_state",
            &json!({"state":"screened"})
        ));
        assert!(!pipeline_finding_allowed(
            Role::Reporter,
            "findings_create",
            &json!({})
        ));
    }

    #[cfg(feature = "goal-mode")]
    #[test]
    fn omitted_goal_status_continues_without_an_explicit_limit() {
        assert!(matches!(
            goal_signal_for_answer("More work remains.", None),
            Some(beale_goals::Signal::Continue)
        ));
        assert!(goal_signal_for_answer(INTERRUPTED_ANSWER, None).is_none());
        assert!(goal_signal_for_answer(STOPPED_ANSWER, None).is_none());
    }

    #[tokio::test]
    async fn stop_signal_interrupts_in_flight_work() {
        let (sender, receiver) = watch::channel(false);
        let mut stop = Some(receiver);
        sender.send_replace(true);
        let result = until_stopped(std::future::pending::<()>(), &mut stop).await;
        assert!(result.is_none());
        assert!(stop_requested(&stop));
    }

    #[tokio::test]
    async fn stop_turn_requires_auth_and_signals_only_the_requested_turn() {
        let root = std::env::temp_dir().join(format!("beale-stop-example-{}", Uuid::new_v4()));
        let state = build_state(
            "example-token".to_owned(),
            WorkspaceStore::at(root.clone()).unwrap(),
        )
        .unwrap();
        let id = Uuid::new_v4();
        let (sender, receiver) = watch::channel(false);
        state.turn_stops.lock().await.insert(id, sender);
        let app = Router::new()
            .route(SESSION_TURN_STOP_PATH, post(stop_turn))
            .with_state(state);
        let path = SESSION_TURN_STOP_PATH.replace("{id}", &id.to_string());
        let request = Request::builder()
            .method("POST")
            .uri(&path)
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let request = Request::builder()
            .method("POST")
            .uri(&path)
            .header("authorization", "Bearer example-token")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::NO_CONTENT
        );
        assert!(*receiver.borrow());
        let missing = SESSION_TURN_STOP_PATH.replace("{id}", &Uuid::new_v4().to_string());
        let request = Request::builder()
            .method("POST")
            .uri(missing)
            .header("authorization", "Bearer example-token")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.oneshot(request).await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(feature = "webfetch")]
    #[tokio::test]
    async fn webfetch_rechecks_enablement_and_scope_before_execution() {
        let root =
            std::env::temp_dir().join(format!("beale-webfetch-example-{}", uuid::Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        let id = beale_webfetch::FEATURE_ID;
        let workspace = "workspace-example-a";
        let unavailable = "Webfetch is disabled or unavailable in this workspace";
        assert!(!webfetch_feature_enabled(&db, workspace).unwrap());
        assert_eq!(
            execute_webfetch(&db, workspace, &json!({}))
                .await
                .err()
                .unwrap()
                .to_string(),
            unavailable
        );
        db.replace_workspaces(&[WorkspaceRow {
            id: workspace.to_owned(),
            name: "Example A".to_owned(),
            directory: "example-a".to_owned(),
        }])
        .unwrap();
        db.set_feature_enabled(id, true).unwrap();
        assert!(webfetch_feature_enabled(&db, workspace).unwrap());
        // Invalid arguments prove enabled calls reach the feature without making a network request.
        assert_eq!(
            execute_webfetch(&db, workspace, &json!({}))
                .await
                .err()
                .unwrap()
                .to_string(),
            "supply exactly one of query or url"
        );
        db.set_extension_scope("feature", id, false, &[workspace.to_owned()])
            .unwrap();
        assert!(webfetch_feature_enabled(&db, workspace).unwrap());
        assert!(!webfetch_feature_enabled(&db, "workspace-example-b").unwrap());
        assert_eq!(
            execute_webfetch(&db, "workspace-example-b", &json!({}))
                .await
                .err()
                .unwrap()
                .to_string(),
            unavailable
        );
        db.set_extension_scope("feature", id, false, &[]).unwrap();
        assert_eq!(
            execute_webfetch(&db, workspace, &json!({}))
                .await
                .err()
                .unwrap()
                .to_string(),
            unavailable
        );
        db.set_extension_scope("feature", id, true, &[]).unwrap();
        db.set_feature_enabled(id, false).unwrap();
        assert_eq!(
            execute_webfetch(&db, workspace, &json!({}))
                .await
                .err()
                .unwrap()
                .to_string(),
            unavailable
        );
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(feature = "agent-feedback")]
    #[test]
    fn agent_feedback_respects_enablement_and_workspace_scope() {
        let root = std::env::temp_dir().join(format!(
            "beale-feedback-scope-example-{}",
            uuid::Uuid::new_v4()
        ));
        let db = HostDatabase::open(&root).unwrap();
        assert!(!agent_feedback_feature_enabled(&db, "workspace-example-a").unwrap());
        db.replace_workspaces(&[
            WorkspaceRow {
                id: "workspace-example-a".to_owned(),
                name: "Example A".to_owned(),
                directory: "example-a".to_owned(),
            },
            WorkspaceRow {
                id: "workspace-example-b".to_owned(),
                name: "Example B".to_owned(),
                directory: "example-b".to_owned(),
            },
        ])
        .unwrap();
        db.set_feature_enabled(beale_agent_feedback::FEATURE_ID, true)
            .unwrap();
        db.set_extension_scope(
            "feature",
            beale_agent_feedback::FEATURE_ID,
            false,
            &["workspace-example-a".to_owned()],
        )
        .unwrap();
        assert!(agent_feedback_feature_enabled(&db, "workspace-example-a").unwrap());
        assert!(!agent_feedback_feature_enabled(&db, "workspace-example-b").unwrap());
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn feature_tools_require_review_without_double_reviewing_base_tools() {
        #[cfg(feature = "webfetch")]
        assert!(feature_tool_requires_review(beale_webfetch::TOOL_NAME));
        #[cfg(feature = "surfaces")]
        for tool in beale_surfaces::definitions() {
            assert!(feature_tool_requires_review(tool["name"].as_str().unwrap()));
        }
        #[cfg(feature = "findings")]
        for tool in beale_findings::definitions() {
            assert!(feature_tool_requires_review(tool["name"].as_str().unwrap()));
        }
        #[cfg(feature = "memories")]
        for tool in beale_memories::definitions() {
            assert!(feature_tool_requires_review(tool["name"].as_str().unwrap()));
        }
        #[cfg(feature = "runbooks")]
        assert!(feature_tool_requires_review(beale_runbooks::RUN));
        #[cfg(feature = "goal-mode")]
        assert!(feature_tool_requires_review(beale_goals::TOOL_NAME));
        for name in [
            #[cfg(feature = "agent-feedback")]
            beale_agent_feedback::TOOL_NAME,
            #[cfg(feature = "graphs")]
            beale_graphs::DELEGATE_TOOL,
            "read",
            "write",
            "edit",
            "bash",
            "plugin_call",
        ] {
            assert!(!feature_tool_requires_review(name));
        }
    }

    #[cfg(all(
        feature = "agent-feedback",
        feature = "webfetch",
        feature = "surfaces",
        feature = "findings",
        feature = "memories",
        feature = "runbooks",
        feature = "graphs",
        feature = "goal-mode"
    ))]
    #[test]
    fn built_in_tool_counts_include_optional_features() {
        let base = beale_tools::definitions().len();
        assert_eq!(base, 4);
        let all = base
            + beale_surfaces::definitions().len()
            + beale_findings::definitions().len()
            + beale_runbooks::definitions().len()
            + beale_memories::definitions().len()
            + beale_agent_feedback::TOOL_COUNT as usize
            + beale_webfetch::TOOL_COUNT as usize
            + beale_graphs::TOOL_COUNT as usize
            + beale_goals::TOOL_COUNT as usize;
        assert_eq!(all, 31);
    }

    #[cfg(feature = "graphs")]
    #[test]
    fn graph_selection_requires_an_enabled_in_scope_feature_and_new_session() {
        let root = std::env::temp_dir().join(format!("beale-graph-example-{}", Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        let workspace = Uuid::new_v4().to_string();
        let other = Uuid::new_v4().to_string();
        db.replace_workspaces(
            &[workspace.as_str(), other.as_str()]
                .into_iter()
                .map(|id| beale_storage::WorkspaceRow {
                    id: id.to_owned(),
                    name: "Example workspace".to_owned(),
                    directory: format!("example-directory-{id}"),
                })
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert_eq!(
            validate_graph_selection(&db, "simple", &[], &[], "", &workspace),
            Err(StatusCode::CONFLICT)
        );
        db.set_feature_enabled(beale_graphs::FEATURE_ID, true)
            .unwrap();
        assert_eq!(
            validate_graph_selection(&db, "simple", &[], &[], "", &workspace),
            Ok(())
        );
        let model = GraphNodeModel {
            node_id: "scout".to_owned(),
            provider: ProviderId::Openai as i32,
            method: AuthMethod::Subscription as i32,
            model: "model-example".to_owned(),
        };
        assert_eq!(
            validate_graph_selection(
                &db,
                "simple",
                std::slice::from_ref(&model),
                &[],
                "",
                &workspace
            ),
            Ok(())
        );
        assert_eq!(
            validate_graph_selection(
                &db,
                "simple",
                &[model.clone(), model.clone()],
                &[],
                "",
                &workspace
            ),
            Err(StatusCode::BAD_REQUEST)
        );
        let invalid_model = GraphNodeModel {
            model: "example-model\nignore-instructions".to_owned(),
            ..model.clone()
        };
        assert_eq!(
            validate_graph_selection(&db, "simple", &[invalid_model], &[], "", &workspace),
            Err(StatusCode::BAD_REQUEST)
        );
        assert_eq!(
            validate_graph_selection(&db, "", std::slice::from_ref(&model), &[], "", &workspace),
            Err(StatusCode::BAD_REQUEST)
        );
        let main = GraphNodeModel {
            node_id: "main".to_owned(),
            ..model.clone()
        };
        let another_main = GraphNodeModel {
            model: "model-example-two".to_owned(),
            ..main.clone()
        };
        assert_eq!(
            validate_graph_selection(&db, "simple", &[main, another_main], &[], "", &workspace),
            Err(StatusCode::BAD_REQUEST)
        );
        let limit = GraphNodeConcurrency {
            node_id: "scout".to_owned(),
            max_concurrent: 2,
        };
        let main_limit = GraphNodeConcurrency {
            node_id: "main".to_owned(),
            max_concurrent: 1,
        };
        assert_eq!(
            validate_graph_selection(
                &db,
                "simple",
                &[],
                std::slice::from_ref(&main_limit),
                "",
                &workspace
            ),
            Ok(())
        );
        assert_eq!(
            validate_graph_selection(
                &db,
                "simple",
                &[],
                &[GraphNodeConcurrency {
                    max_concurrent: 2,
                    ..main_limit
                }],
                "",
                &workspace
            ),
            Err(StatusCode::BAD_REQUEST)
        );
        assert_eq!(
            validate_graph_selection(
                &db,
                "simple",
                &[],
                std::slice::from_ref(&limit),
                "",
                &workspace
            ),
            Ok(())
        );
        assert_eq!(
            validate_graph_selection(
                &db,
                "simple",
                &[],
                &[limit.clone(), limit.clone()],
                "",
                &workspace
            ),
            Err(StatusCode::BAD_REQUEST)
        );
        assert_eq!(
            validate_graph_selection(
                &db,
                "simple",
                &[],
                &[GraphNodeConcurrency {
                    max_concurrent: 0,
                    ..limit
                }],
                "",
                &workspace
            ),
            Err(StatusCode::BAD_REQUEST)
        );
        let mut unknown_role = model.clone();
        unknown_role.node_id = "unknown".to_owned();
        assert_eq!(
            validate_graph_selection(&db, "simple", &[unknown_role], &[], "", &workspace),
            Err(StatusCode::BAD_REQUEST)
        );
        for graph_id in [beale_graphs::CLOUDFLARE_ID, beale_graphs::OPENAI_ID] {
            assert_eq!(
                validate_graph_selection(&db, graph_id, &[], &[], "", &workspace),
                Ok(())
            );
            let entry = beale_graphs::entry_role(graph_id).unwrap();
            assert_eq!(
                validate_graph_selection(
                    &db,
                    graph_id,
                    &[GraphNodeModel {
                        node_id: entry.id().to_owned(),
                        ..model.clone()
                    }],
                    &[],
                    "",
                    &workspace
                ),
                Ok(())
            );
            assert_eq!(
                validate_graph_selection(
                    &db,
                    graph_id,
                    &[GraphNodeModel {
                        node_id: "main".to_owned(),
                        ..model.clone()
                    }],
                    &[],
                    "",
                    &workspace
                ),
                Err(StatusCode::BAD_REQUEST)
            );
            assert_eq!(
                validate_graph_selection(
                    &db,
                    graph_id,
                    &[],
                    &[GraphNodeConcurrency {
                        node_id: entry.id().to_owned(),
                        max_concurrent: 2
                    }],
                    "",
                    &workspace
                ),
                Err(StatusCode::BAD_REQUEST)
            );
        }
        assert_eq!(
            validate_graph_selection(&db, "unknown", &[], &[], "", &workspace),
            Err(StatusCode::BAD_REQUEST)
        );
        assert_eq!(
            validate_graph_selection(&db, "simple", &[], &[], "session-example", &workspace),
            Err(StatusCode::BAD_REQUEST)
        );
        db.set_extension_scope("feature", beale_graphs::FEATURE_ID, false, &[other])
            .unwrap();
        assert_eq!(
            validate_graph_selection(&db, "simple", &[], &[], "", &workspace),
            Err(StatusCode::CONFLICT)
        );
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(feature = "graphs")]
    #[test]
    fn addressed_graph_role_must_be_a_top_level_node_in_this_flow() {
        let pipeline = beale_graphs::graph_definition(beale_graphs::OPENAI_ID);
        assert_eq!(
            requested_top_level_role(pipeline, "reporter").unwrap(),
            Some(beale_graphs::Role::Reporter)
        );
        assert_eq!(requested_top_level_role(pipeline, "").unwrap(), None);
        for role in ["scout", "validation_system", "unknown"] {
            assert_eq!(
                requested_top_level_role(pipeline, role).unwrap_err(),
                StatusCode::BAD_REQUEST
            );
        }
        assert_eq!(
            requested_top_level_role(None, "reporter").unwrap_err(),
            StatusCode::BAD_REQUEST
        );
    }

    #[cfg(feature = "graphs")]
    #[test]
    fn peer_agent_context_excludes_other_top_level_turns() {
        let make = |number, role: &str, prompt: &str, answer: &str| StoredTurn {
            number,
            provider: ProviderId::Openai as i32,
            method: AuthMethod::ApiKey as i32,
            model: "example-model".to_owned(),
            prompt: prompt.to_owned(),
            answer: answer.to_owned(),
            duration_ms: 1,
            internal_prompt: false,
            agent_role: role.to_owned(),
        };
        let turns = vec![
            make(1, "discovery", "Find an example issue", "Example lead"),
            make(2, "validation", "Check the lead", "Example check"),
            make(3, "discovery", "Continue", "Example follow-up"),
        ];
        let messages = peer_input_messages(&turns, beale_graphs::Role::Validation, "Next check");
        let text: Vec<_> = messages
            .iter()
            .map(|message| message.text.as_str())
            .collect();
        assert_eq!(text, ["Check the lead", "Example check", "Next check"]);
    }

    #[cfg(feature = "graphs")]
    #[test]
    fn peer_detail_keeps_the_newest_complete_turns_within_tool_limit() {
        let turns = (1..=3)
            .map(|number| StoredTurn {
                number,
                provider: ProviderId::Openai as i32,
                method: AuthMethod::ApiKey as i32,
                model: "example-model".to_owned(),
                prompt: "Example prompt".to_owned(),
                answer: "Example answer".to_owned(),
                duration_ms: 1,
                internal_prompt: false,
                agent_role: "discovery".to_owned(),
            })
            .collect::<Vec<_>>();
        let selected = recent_peer_turns(&turns, |numbers| {
            Ok(numbers
                .iter()
                .map(|number| match number {
                    1 => 3_000,
                    2 => 1_500,
                    _ => 708,
                })
                .sum())
        })
        .unwrap();
        assert_eq!(
            selected.iter().map(|turn| turn.number).collect::<Vec<_>>(),
            [2, 3]
        );
    }

    #[cfg(feature = "graphs")]
    #[test]
    fn delegation_validation_accepts_repeated_valid_batches() {
        let runtime = beale_graphs::runtime(beale_graphs::OPENAI_ID).unwrap();
        let config = SessionGraphConfig::default();
        let fallback = GraphNodeModel {
            node_id: "discovery".to_owned(),
            provider: ProviderId::Openai as i32,
            method: AuthMethod::ApiKey as i32,
            model: "example-model".to_owned(),
        };
        let invalid =
            json!({"assignments":[{"role":"scout","task":"Inspect example","model_index":7}]});
        assert!(validate_delegations(runtime, &invalid, &config, &fallback).is_err());
        let valid = json!({"assignments":[{"role":"scout","task":"Inspect example"}]});
        for _ in 0..20 {
            validate_delegations(runtime, &valid, &config, &fallback).unwrap();
        }
    }

    #[cfg(feature = "graphs")]
    #[test]
    fn simple_graph_defaults_only_main_concurrency() {
        let request = SubmitSessionTurnRequest {
            graph_id: beale_graphs::SIMPLE_ID.to_owned(),
            graph_node_concurrency: vec![GraphNodeConcurrency {
                node_id: "scout".to_owned(),
                max_concurrent: 3,
            }],
            ..Default::default()
        };
        let bytes = encoded_graph_config(&request).unwrap();
        let saved = SessionGraphConfig::decode(bytes.as_slice()).unwrap();
        assert_eq!(saved.concurrency.len(), 2);
        assert!(
            saved
                .concurrency
                .iter()
                .any(|item| item.node_id == "main" && item.max_concurrent == 1)
        );
        assert!(
            saved
                .concurrency
                .iter()
                .any(|item| item.node_id == "scout" && item.max_concurrent == 3)
        );
        assert!(encoded_graph_config(&SubmitSessionTurnRequest::default()).is_none());
    }

    #[test]
    fn keeps_latest_compaction_and_all_following_items_for_replay() {
        let context = vec![
            json!({"role":"user","content":"Old"}),
            json!({"type":"compaction","encrypted_content":"opaque-example"}),
            json!({"role":"user","content":"New"}),
            json!({"type":"reasoning","encrypted_content":"reasoning-example"}),
        ];
        assert_eq!(compact_openai_context(&context), context[1..]);
        assert_eq!(compact_openai_context(&context[0..1]), context[0..1]);
    }

    #[test]
    fn closes_unreviewed_parallel_calls_before_saving_or_replaying_context() {
        let mut context = vec![
            json!({"role":"user","content":"Inspect example.test"}),
            json!({"type":"function_call","call_id":"call-one","name":"read","arguments":"{}"}),
            json!({"type":"function_call","call_id":"call-two","name":"bash","arguments":"{}"}),
            json!({"type":"function_call","call_id":"call-three","name":"bash","arguments":"{}"}),
            json!({"type":"function_call_output","call_id":"call-one","output":"Example result"}),
        ];
        close_unanswered_openai_calls(&mut context);
        assert_eq!(context.len(), 7);
        assert_eq!(context[5]["call_id"], "call-two");
        assert_eq!(context[6]["call_id"], "call-three");
        assert!(context[5]["output"].as_str().unwrap().contains("not run"));
        close_unanswered_openai_calls(&mut context);
        assert_eq!(context.len(), 7);
        context.push(json!({"role":"user","content":"Continue"}));
        assert_eq!(context[7]["role"], "user");
    }

    #[test]
    fn anthropic_parallel_results_share_one_follow_up_message() {
        let mut input = vec![json!({"role":"assistant","content":[
            {"type":"tool_use","id":"tool-one","name":"read","input":{}},
            {"type":"tool_use","id":"tool-two","name":"read","input":{}}
        ]})];
        let mut results = Vec::new();
        append_tool_result(
            &mut input,
            &mut results,
            Provider::Anthropic,
            AuthMode::ApiKey,
            "tool-one",
            "Example one",
        );
        append_tool_result(
            &mut input,
            &mut results,
            Provider::Anthropic,
            AuthMode::ApiKey,
            "tool-two",
            "Example two",
        );
        input.push(json!({"role":"user","content":results}));
        assert_eq!(input.len(), 2);
        assert_eq!(input[1]["content"].as_array().unwrap().len(), 2);
        assert_eq!(input[1]["content"][0]["tool_use_id"], "tool-one");
        assert_eq!(input[1]["content"][1]["tool_use_id"], "tool-two");
    }

    #[test]
    fn session_effort_contract_rejects_unknown_values() {
        let request = SubmitSessionTurnRequest {
            workspace_id: Uuid::new_v4().to_string(),
            session_id: String::new(),
            provider: ProviderId::Openai as i32,
            method: AuthMethod::Subscription as i32,
            model: "model-example".to_owned(),
            prompt: "Example prompt".to_owned(),
            reasoning_effort: ReasoningEffort::High as i32,
            goal_mode: false,
            goal_max_turns: 0,
            goal_max_duration_minutes: 0,
            repeat_preset: String::new(),
            graph_id: "simple".to_owned(),
            graph_models: vec![GraphNodeModel {
                node_id: "scout".to_owned(),
                provider: ProviderId::Openai as i32,
                method: AuthMethod::Subscription as i32,
                model: "model-example".to_owned(),
            }],
            graph_node_concurrency: vec![GraphNodeConcurrency {
                node_id: "main".to_owned(),
                max_concurrent: 1,
            }],
            agent_role: "main".to_owned(),
        };
        let decoded = SubmitSessionTurnRequest::decode(request.encode_to_vec().as_slice()).unwrap();
        assert_eq!(decoded.graph_id, "simple");
        assert_eq!(decoded.agent_role, "main");
        assert_eq!(decoded.graph_models, request.graph_models);
        assert_eq!(
            decoded.graph_node_concurrency,
            request.graph_node_concurrency
        );
        assert_eq!(
            reasoning_effort_name(decoded.reasoning_effort),
            Ok(Some("high"))
        );
        assert_eq!(
            reasoning_effort_name(ReasoningEffort::Unspecified as i32),
            Ok(None)
        );
        assert_eq!(reasoning_effort_name(99), Err(StatusCode::BAD_REQUEST));
    }

    #[test]
    fn goal_contract_round_trips_as_an_opt_in() {
        let request = SubmitSessionTurnRequest {
            workspace_id: Uuid::new_v4().to_string(),
            session_id: String::new(),
            provider: ProviderId::Openai as i32,
            method: AuthMethod::ApiKey as i32,
            model: "model-example".to_owned(),
            prompt: "Inspect ExampleCo's test repository".to_owned(),
            reasoning_effort: ReasoningEffort::High as i32,
            goal_mode: true,
            goal_max_turns: 12,
            goal_max_duration_minutes: 45,
            repeat_preset: String::new(),
            graph_id: String::new(),
            graph_models: vec![],
            graph_node_concurrency: vec![],
            agent_role: String::new(),
        };
        let decoded = SubmitSessionTurnRequest::decode(request.encode_to_vec().as_slice()).unwrap();
        assert!(decoded.goal_mode);
        assert_eq!(decoded.goal_max_turns, 12);
        assert_eq!(decoded.goal_max_duration_minutes, 45);
        assert_eq!(decoded.prompt, request.prompt);
    }

    #[test]
    fn goal_continuation_keeps_model_context_and_hides_its_internal_prompt() {
        let root = std::env::temp_dir().join(format!("beale-goal-example-{}", Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        let id = Uuid::new_v4();
        let session = SessionRow {
            id: id.to_string(),
            workspace_id: Uuid::new_v4().to_string(),
            title: "Example objective".to_owned(),
            created_at_ms: 1,
            last_message_at_ms: 2,
        };
        let first = TurnRow {
            number: 1,
            provider: ProviderId::Openai as i32,
            method: AuthMethod::ApiKey as i32,
            model: "model-example".to_owned(),
            prompt: "Example objective".to_owned(),
            answer: "First observation".to_owned(),
            duration_ms: 1,
            internal_prompt: false,
        };
        let goal = GoalRow {
            session_id: session.id.clone(),
            objective: first.prompt.clone(),
            status: GoalStatus::Active as i32,
            turns_used: 1,
            required_state: String::new(),
            max_turns: 0,
            max_duration_minutes: 0,
            active_ms: 1,
        };
        db.append_turn_with_goal_activity(
            &session,
            &first,
            true,
            TurnActivity {
                tools: &[],
                reasoning: &[],
                commentary: &[],
                goal: Some(&goal),
            },
        )
        .unwrap();
        let second = TurnRow {
            number: 2,
            prompt: "Internal continuation".to_owned(),
            answer: "Verified result".to_owned(),
            internal_prompt: true,
            ..first
        };
        let done = GoalRow {
            status: GoalStatus::Complete as i32,
            turns_used: 2,
            ..goal
        };
        db.append_turn_with_goal_activity(
            &session,
            &second,
            false,
            TurnActivity {
                tools: &[],
                reasoning: &[],
                commentary: &[],
                goal: Some(&done),
            },
        )
        .unwrap();
        let (loaded, turns) = load_persisted(&db, id).unwrap();
        assert_eq!(turns[1].prompt, "Internal continuation");
        assert!(turns[1].internal_prompt);
        let transcript = detail(&db, &loaded, &turns).unwrap();
        assert_eq!(transcript.messages.len(), 3);
        assert_eq!(transcript.messages[2].role, SessionRole::Assistant as i32);
        assert_eq!(transcript.messages[2].text, "Verified result");
        assert_eq!(transcript.goal.unwrap().status, GoalStatus::Complete as i32);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn goal_history_can_advance_beyond_the_ordinary_session_limit() {
        let root =
            std::env::temp_dir().join(format!("beale-goal-history-example-{}", Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        let id = Uuid::new_v4();
        let session = SessionRow {
            id: id.to_string(),
            workspace_id: Uuid::new_v4().to_string(),
            title: "Example objective".to_owned(),
            created_at_ms: 1,
            last_message_at_ms: 1,
        };
        for number in 1..=70 {
            let turn = TurnRow {
                number,
                provider: ProviderId::Openai as i32,
                method: AuthMethod::ApiKey as i32,
                model: "model-example".to_owned(),
                prompt: if number == 1 {
                    "Example objective"
                } else {
                    "Internal continuation"
                }
                .to_owned(),
                answer: format!("Example observation {number}"),
                duration_ms: 1,
                internal_prompt: number != 1,
            };
            let goal = GoalRow {
                session_id: session.id.clone(),
                objective: "Example objective".to_owned(),
                status: GoalStatus::Active as i32,
                turns_used: number,
                required_state: String::new(),
                max_turns: 0,
                max_duration_minutes: 0,
                active_ms: number,
            };
            db.append_turn_with_goal_activity(
                &session,
                &turn,
                number == 1,
                TurnActivity {
                    tools: &[],
                    reasoning: &[],
                    commentary: &[],
                    goal: Some(&goal),
                },
            )
            .unwrap();
        }
        let (loaded, turns) = load_persisted(&db, id).unwrap();
        assert_eq!(turns.first().unwrap().number, 7);
        assert_eq!(turns.last().unwrap().number, 70);
        let transcript = detail(&db, &loaded, &turns).unwrap();
        assert_eq!(transcript.first_turn_number, 7);
        assert_eq!(transcript.total_duration_ms, 70);
        assert_eq!(transcript.goal.unwrap().turns_used, 70);
        assert_eq!(
            transcript.messages.last().unwrap().text,
            "Example observation 70"
        );
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn session_detail_includes_runbook_activity_counts() {
        let root =
            std::env::temp_dir().join(format!("beale-session-runbook-example-{}", Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        let workspace_id = Uuid::new_v4().to_string();
        db.replace_workspaces(&[beale_storage::WorkspaceRow {
            id: workspace_id.clone(),
            name: "Example Workspace".into(),
            directory: "example-directory".into(),
        }])
        .unwrap();
        let session = SessionRow {
            id: Uuid::new_v4().to_string(),
            workspace_id: workspace_id.clone(),
            title: "Example objective".into(),
            created_at_ms: 1,
            last_message_at_ms: 1,
        };
        let turn = TurnRow {
            number: 1,
            provider: ProviderId::Openai as i32,
            method: AuthMethod::ApiKey as i32,
            model: "model-example".into(),
            prompt: "Example objective".into(),
            answer: "Example answer".into(),
            duration_ms: 1,
            internal_prompt: false,
        };
        db.append_turn_with_tools(&session, &turn, true, &[])
            .unwrap();
        let book = db
            .runbook_create(
                beale_contracts::runbooks::Document {
                    id: String::new(),
                    workspace_id: workspace_id.clone(),
                    title: "Example runbook".into(),
                    purpose: "Synthetic check".into(),
                    cells: vec![],
                    next_cell_number: 1,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                    revision: 0,
                },
                "agent",
                &session.id,
            )
            .unwrap();
        let run = db
            .runbook_run_start(&workspace_id, &session.id, &book.id, book.revision)
            .unwrap();
        let (loaded, turns) = load_persisted(&db, Uuid::parse_str(&session.id).unwrap()).unwrap();
        let active = detail(&db, &loaded, &turns).unwrap();
        assert_eq!((active.runbook_count, active.active_runbook_count), (1, 1));
        assert_eq!(active.runbooks.len(), 1);
        assert_eq!(active.runbooks[0].title, "Example runbook");
        assert_eq!(active.runbooks[0].status, "running");
        db.runbook_run_cell(&run.id, 1, "Example check").unwrap();
        let progressing = detail(&db, &loaded, &turns).unwrap();
        assert_eq!(progressing.runbooks[0].current_cell_number, 1);
        assert_eq!(progressing.runbooks[0].current_cell_name, "Example check");
        db.runbook_run_finish(&run.id, true, "Example result")
            .unwrap();
        let done = detail(&db, &loaded, &turns).unwrap();
        assert_eq!((done.runbook_count, done.active_runbook_count), (1, 0));
        assert_eq!(done.runbooks[0].status, "done");
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn completed_turn_round_trips_as_a_transcript() {
        let root = std::env::temp_dir().join(format!("beale-session-example-{}", Uuid::new_v4()));
        let id = Uuid::new_v4();
        let session = SessionRow {
            id: id.to_string(),
            workspace_id: Uuid::new_v4().to_string(),
            title: title("Echo example\nmore details"),
            created_at_ms: 1,
            last_message_at_ms: 1,
        };
        let turn = TurnRow {
            number: 1,
            provider: ProviderId::Openai as i32,
            method: AuthMethod::Subscription as i32,
            model: "model-example".to_owned(),
            prompt: "Echo example".to_owned(),
            answer: "Echo example".to_owned(),
            duration_ms: 1_234,
            internal_prompt: false,
        };
        let db = HostDatabase::open(&root).unwrap();
        let usage = [
            ModelUsageRow {
                agent_id: "main".to_owned(),
                role: "main".to_owned(),
                provider: turn.provider,
                method: turn.method,
                model: turn.model.clone(),
                input_tokens: Some(100),
                output_tokens: Some(20),
                cache_read_tokens: Some(25),
                work_elapsed_ms: Some(100),
                duration_ms: 100,
                observed_at_ms: 1,
            },
            ModelUsageRow {
                agent_id: "scout-example".to_owned(),
                role: "scout".to_owned(),
                provider: turn.provider,
                method: turn.method,
                model: turn.model.clone(),
                input_tokens: Some(200),
                output_tokens: Some(30),
                cache_read_tokens: None,
                work_elapsed_ms: Some(200),
                duration_ms: 100,
                observed_at_ms: 1,
            },
        ];
        db.append_turn_with_metadata(
            &session,
            &turn,
            true,
            TurnActivity {
                tools: &[],
                reasoning: &[],
                commentary: &[],
                goal: None,
            },
            None,
            TurnMetadata {
                model_usage: Some(&usage),
                ..TurnMetadata::default()
            },
        )
        .unwrap();
        let (loaded, turns) = load_persisted(&db, id).unwrap();
        let transcript = detail(&db, &loaded, &turns).unwrap();
        assert_eq!(loaded.title, "Echo example");
        assert_eq!(transcript.summary.as_ref().unwrap().last_message_at_ms, 1);
        assert_eq!(transcript.messages.len(), 2);
        assert_eq!(transcript.messages[0].text, "Echo example");
        assert_eq!(transcript.messages[1].role, SessionRole::Assistant as i32);
        assert_eq!(transcript.total_duration_ms, 1_234);
        assert!(!transcript.has_campaign_activity);
        let tokens = transcript.token_usage.unwrap();
        assert_eq!((tokens.input_tokens, tokens.output_tokens), (300, 50));
        assert_eq!((tokens.measured_calls, tokens.total_calls), (2, 2));
        assert_eq!(
            (tokens.cache_read_tokens, tokens.cache_input_tokens),
            (25, 100)
        );
        assert_eq!(tokens.cache_measured_calls, 1);
        assert_eq!(transcript.usage_snapshots.len(), 2);
        assert_eq!(transcript.usage_snapshots[0].work_duration_ms, 0);
        assert_eq!(transcript.usage_snapshots[1].work_duration_ms, 1_234);
        assert_eq!(transcript.usage_snapshots[1].input_tokens, 300);
        let encoded = transcript
            .model_usage
            .iter()
            .map(|source| (source.role.as_str(), source.usage.as_ref().unwrap()))
            .collect::<Vec<_>>();
        assert_eq!(encoded.len(), 2);
        assert_eq!(encoded[0].0, "main");
        assert_eq!(encoded[0].1.input_tokens, 100);
        assert_eq!(encoded[0].1.cache_read_tokens, 25);
        assert_eq!(encoded[0].1.cache_input_tokens, 100);
        assert_eq!(encoded[1].0, "scout");
        assert_eq!(encoded[1].1.input_tokens, 200);
        assert_eq!(encoded[1].1.cache_measured_calls, 0);
        let first = &transcript.usage_snapshots[0];
        assert_eq!(first.main_usage.as_ref().unwrap().total_calls, 0);
        assert_eq!(first.subagent_usage.as_ref().unwrap().total_calls, 0);
        let last = &transcript.usage_snapshots[1];
        assert_eq!(last.main_usage.as_ref(), Some(encoded[0].1));
        assert_eq!(last.subagent_usage.as_ref(), Some(encoded[1].1));
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(feature = "graphs")]
    #[test]
    fn saved_graph_models_return_in_session_detail() {
        let root = std::env::temp_dir().join(format!("beale-graph-example-{}", Uuid::new_v4()));
        let id = Uuid::new_v4();
        let session = SessionRow {
            id: id.to_string(),
            workspace_id: Uuid::new_v4().to_string(),
            title: "Example".to_owned(),
            created_at_ms: 1,
            last_message_at_ms: 1,
        };
        let turn = TurnRow {
            number: 1,
            provider: ProviderId::Openai as i32,
            method: AuthMethod::Subscription as i32,
            model: "model-example".to_owned(),
            prompt: "Example prompt".to_owned(),
            answer: "Example answer".to_owned(),
            duration_ms: 1,
            internal_prompt: false,
        };
        let graph_model = GraphNodeModel {
            node_id: "scout".to_owned(),
            provider: ProviderId::Openai as i32,
            method: AuthMethod::Subscription as i32,
            model: "model-example".to_owned(),
        };
        let encoded = SessionGraphConfig {
            models: vec![graph_model.clone()],
            concurrency: vec![GraphNodeConcurrency {
                node_id: "main".to_owned(),
                max_concurrent: 1,
            }],
        }
        .encode_to_vec();
        let db = HostDatabase::open(&root).unwrap();
        db.append_turn_with_metadata(
            &session,
            &turn,
            true,
            TurnActivity {
                tools: &[],
                reasoning: &[],
                commentary: &[],
                goal: None,
            },
            None,
            TurnMetadata {
                graph_id: Some(beale_graphs::SIMPLE_ID),
                graph_config: Some(&encoded),
                ..TurnMetadata::default()
            },
        )
        .unwrap();
        let (loaded, turns) = load_persisted(&db, id).unwrap();
        let detail = detail(&db, &loaded, &turns).unwrap();
        assert_eq!(detail.graph_id, beale_graphs::SIMPLE_ID);
        assert_eq!(detail.graph_models, [graph_model]);
        assert_eq!(detail.graph_node_concurrency[0].max_concurrent, 1);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn transcript_keeps_full_tool_details_on_host_until_requested() {
        let root = std::env::temp_dir().join(format!("beale-session-example-{}", Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        let session = SessionRow {
            id: Uuid::new_v4().to_string(),
            workspace_id: Uuid::new_v4().to_string(),
            title: "Example".to_owned(),
            created_at_ms: 1,
            last_message_at_ms: 1,
        };
        let turn = TurnRow {
            number: 1,
            provider: ProviderId::Openai as i32,
            method: AuthMethod::ApiKey as i32,
            model: "model-example".to_owned(),
            prompt: "Read example".to_owned(),
            answer: "Done".to_owned(),
            duration_ms: 2_345,
            internal_prompt: false,
        };
        let tool = ToolCallRow {
            id: Uuid::new_v4().to_string(),
            turn_number: 1,
            ordinal: 0,
            name: "read".to_owned(),
            summary: "Read example.txt".to_owned(),
            arguments: "{\"path\":\"example.txt\"}".to_owned(),
            output: "private example output".to_owned(),
            is_error: false,
            agent_role: "main".to_owned(),
        };
        db.append_turn_with_full_activity(
            &session,
            &turn,
            true,
            std::slice::from_ref(&tool),
            &[
                ReasoningSummaryRow {
                    turn_number: 1,
                    ordinal: 0,
                    after_tool_count: 0,
                    text: "Checking the example file.".to_owned(),
                    agent_role: "main".to_owned(),
                },
                ReasoningSummaryRow {
                    turn_number: 1,
                    ordinal: 1,
                    after_tool_count: 1,
                    text: "The check is complete.".to_owned(),
                    agent_role: "main".to_owned(),
                },
            ],
            &[CommentaryRow {
                turn_number: 1,
                ordinal: 0,
                after_tool_count: 0,
                text: "Reading the example file.".to_owned(),
                agent_role: "main".to_owned(),
            }],
        )
        .unwrap();
        let (loaded, turns) = load_persisted(&db, Uuid::parse_str(&session.id).unwrap()).unwrap();
        let detail = detail(&db, &loaded, &turns).unwrap();
        let wire = detail.encode_to_vec();
        assert_eq!(detail.messages[1].tools[0].summary, "Read example.txt");
        assert_eq!(detail.messages[1].duration_ms, 2_345);
        assert_eq!(detail.messages[1].reasoning_summaries.len(), 2);
        assert_eq!(
            detail.messages[1].commentary[0].text,
            "Reading the example file."
        );
        assert_eq!(
            detail.messages[1].reasoning_summaries[0].after_tool_count,
            0
        );
        assert_eq!(
            detail.messages[1].reasoning_summaries[1].after_tool_count,
            1
        );
        assert!(!String::from_utf8_lossy(&wire).contains("private example output"));
        assert!(!String::from_utf8_lossy(&wire).contains("{\"path\":\"example.txt\"}"));
        assert_eq!(
            db.tool_call(&session.id, &tool.id).unwrap().unwrap().output,
            tool.output
        );
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn long_turn_detail_keeps_recent_tools_and_commentary_order() {
        let root = std::env::temp_dir().join(format!("beale-session-example-{}", Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        let session = SessionRow {
            id: Uuid::new_v4().to_string(),
            workspace_id: Uuid::new_v4().to_string(),
            title: "Example".to_owned(),
            created_at_ms: 1,
            last_message_at_ms: 1,
        };
        let tools = (0..4_100)
            .map(|ordinal| ToolCallRow {
                id: format!("tool-example-{ordinal}"),
                turn_number: 1,
                ordinal,
                name: "read".to_owned(),
                summary: "Read example.txt".to_owned(),
                arguments: "{}".to_owned(),
                output: String::new(),
                is_error: false,
                agent_role: "main".to_owned(),
            })
            .collect::<Vec<_>>();
        db.append_turn_with_full_activity(
            &session,
            &TurnRow {
                number: 1,
                provider: ProviderId::Openai as i32,
                method: AuthMethod::ApiKey as i32,
                model: "example-model".to_owned(),
                prompt: "Example prompt".to_owned(),
                answer: "Example answer".to_owned(),
                duration_ms: 1,
                internal_prompt: false,
            },
            true,
            &tools,
            &[],
            &[CommentaryRow {
                turn_number: 1,
                ordinal: 0,
                after_tool_count: 4_099,
                text: "Checking recent results.".to_owned(),
                agent_role: "main".to_owned(),
            }],
        )
        .unwrap();
        let (loaded, turns) = load_persisted(&db, Uuid::parse_str(&session.id).unwrap()).unwrap();
        let detail = detail(&db, &loaded, &turns).unwrap();
        let assistant = &detail.messages[1];
        assert_eq!(assistant.tools.len(), SESSION_DETAIL_MAX_TOOL_SUMMARIES);
        assert_eq!(assistant.tools.first().unwrap().id, "tool-example-4");
        assert_eq!(assistant.tools.last().unwrap().id, "tool-example-4099");
        assert_eq!(assistant.commentary[0].after_tool_count, 4_095);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(feature = "graphs")]
    #[test]
    fn saved_subagent_messages_stay_in_child_detail() {
        let root = std::env::temp_dir().join(format!("beale-session-example-{}", Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        let session = SessionRow {
            id: Uuid::new_v4().to_string(),
            workspace_id: Uuid::new_v4().to_string(),
            title: "Example".to_owned(),
            created_at_ms: 1,
            last_message_at_ms: 1,
        };
        let child = ChildTurnRow {
            agent_id: Uuid::new_v4().to_string(),
            role: "scout".to_owned(),
            task: "Inspect example input".to_owned(),
            result: "Example result".to_owned(),
            is_error: false,
            messages_json: json!([
                {"role":"assistant","text":"Checking the example input."},
                {"role":"assistant","text":"Example result"}
            ])
            .to_string(),
            reasoning_json: "[]".to_owned(),
            completed_at_ms: 2,
        };
        db.append_turn_with_metadata(
            &session,
            &TurnRow {
                number: 1,
                provider: ProviderId::Openai as i32,
                method: AuthMethod::ApiKey as i32,
                model: "example-model".to_owned(),
                prompt: "Example prompt".to_owned(),
                answer: "Example answer".to_owned(),
                duration_ms: 1,
                internal_prompt: false,
            },
            true,
            TurnActivity {
                tools: &[],
                reasoning: &[],
                commentary: &[],
                goal: None,
            },
            None,
            TurnMetadata {
                child_turns: &[child],
                ..TurnMetadata::default()
            },
        )
        .unwrap();
        let (loaded, turns) = load_persisted(&db, Uuid::parse_str(&session.id).unwrap()).unwrap();
        let detail = detail(&db, &loaded, &turns).unwrap();
        assert!(detail.messages[1].commentary.is_empty());
        assert_eq!(detail.subagents.len(), 1);
        assert_eq!(detail.subagents[0].role, "scout");
        assert_eq!(detail.subagents[0].parent_role, "main");
        assert_eq!(detail.subagents[0].status, "completed");
        let stored = db
            .session_child(&session.id, &detail.subagents[0].id)
            .unwrap()
            .unwrap();
        assert!(
            stored
                .turn
                .messages_json
                .contains("Checking the example input.")
        );
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn routes_require_host_auth_and_a_registered_workspace() {
        let root = std::env::temp_dir().join(format!("beale-session-example-{}", Uuid::new_v4()));
        let app = router(
            "example-token".to_owned(),
            WorkspaceStore::at(root.clone()).unwrap(),
        )
        .unwrap();
        let request = Request::builder()
            .uri(SESSIONS_PATH)
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let request = Request::builder()
            .uri(SESSIONS_PATH)
            .header("authorization", "Bearer example-token")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::OK
        );
        #[cfg(feature = "runbooks")]
        {
            let cell_request = |token: &str, session_id: String| {
                Request::builder()
                    .method("POST")
                    .uri(runbooks::CELL_RUN_PATH)
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "workspace_id": "workspace-example",
                            "session_id": session_id,
                            "id": "R1",
                            "cell_id": "L1",
                            "expected_revision": 1,
                        })
                        .to_string(),
                    ))
                    .unwrap()
            };
            assert_eq!(
                app.clone()
                    .oneshot(cell_request("wrong", Uuid::new_v4().to_string()))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::UNAUTHORIZED,
            );
            assert_eq!(
                app.clone()
                    .oneshot(cell_request("example-token", "invalid".into()))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::BAD_REQUEST,
            );
            assert_eq!(
                app.clone()
                    .oneshot(cell_request("example-token", Uuid::new_v4().to_string()))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::NOT_FOUND,
            );
        }
        let archive_path = format!("/v1/sessions/{}/archive", Uuid::new_v4());
        let request = Request::builder()
            .method("POST")
            .uri(&archive_path)
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let request = Request::builder()
            .method("POST")
            .uri(&archive_path)
            .header("authorization", "Bearer example-token")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
        let request = SubmitSessionTurnRequest {
            workspace_id: Uuid::new_v4().to_string(),
            session_id: String::new(),
            provider: ProviderId::Openai as i32,
            method: AuthMethod::Subscription as i32,
            model: "model-example".to_owned(),
            prompt: "Echo example".to_owned(),
            reasoning_effort: ReasoningEffort::High as i32,
            goal_mode: false,
            goal_max_turns: 0,
            goal_max_duration_minutes: 0,
            repeat_preset: String::new(),
            graph_id: String::new(),
            graph_models: vec![],
            graph_node_concurrency: vec![],
            agent_role: String::new(),
        };
        let payload = request.encode_to_vec();
        let invalid_id = Request::builder()
            .method("POST")
            .uri(SESSION_TURN_STREAM_PATH)
            .header("authorization", "Bearer example-token")
            .header("content-type", beale_contracts::PROTOBUF_CONTENT_TYPE)
            .header(SESSION_TURN_ID_HEADER, "invalid")
            .body(Body::from(payload.clone()))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(invalid_id).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
        let request = Request::builder()
            .method("POST")
            .uri(SESSION_TURNS_PATH)
            .header("authorization", "Bearer example-token")
            .header("content-type", beale_contracts::PROTOBUF_CONTENT_TYPE)
            .body(Body::from(payload))
            .unwrap();
        assert_eq!(
            app.oneshot(request).await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn archiving_hides_a_session_from_the_workspace_list() {
        let root = std::env::temp_dir().join(format!("beale-session-example-{}", Uuid::new_v4()));
        let workspace_id = Uuid::new_v4().to_string();
        let session_id = Uuid::new_v4().to_string();
        let db = HostDatabase::open(&root).unwrap();
        db.replace_workspaces(&[beale_storage::WorkspaceRow {
            id: workspace_id.clone(),
            name: "Example Workspace".to_owned(),
            directory: "example-directory".to_owned(),
        }])
        .unwrap();
        db.append_turn(
            &SessionRow {
                id: session_id.clone(),
                workspace_id,
                title: "Example session".to_owned(),
                created_at_ms: 1,
                last_message_at_ms: 1,
            },
            &TurnRow {
                number: 1,
                provider: ProviderId::Openai as i32,
                method: AuthMethod::ApiKey as i32,
                model: "model-example".to_owned(),
                prompt: "Example question".to_owned(),
                answer: "Example answer".to_owned(),
                duration_ms: 1,
                internal_prompt: false,
            },
            true,
        )
        .unwrap();
        let app = router(
            "example-token".to_owned(),
            WorkspaceStore::at(root.clone()).unwrap(),
        )
        .unwrap();
        let request = Request::builder()
            .method("POST")
            .uri(format!("/v1/sessions/{session_id}/archive"))
            .header("authorization", "Bearer example-token")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::NO_CONTENT
        );
        let request = Request::builder()
            .uri(SESSIONS_PATH)
            .header("authorization", "Bearer example-token")
            .body(Body::empty())
            .unwrap();
        let reply = app.clone().oneshot(request).await.unwrap();
        let body = to_bytes(reply.into_body(), 4096).await.unwrap();
        assert!(
            ListSessionsResponse::decode(body)
                .unwrap()
                .sessions
                .is_empty()
        );
        let request = Request::builder()
            .uri(ARCHIVE_PATH)
            .header("authorization", "Bearer example-token")
            .body(Body::empty())
            .unwrap();
        let reply = app.clone().oneshot(request).await.unwrap();
        let body = to_bytes(reply.into_body(), 4096).await.unwrap();
        let entries = ListArchiveResponse::decode(body).unwrap().entries;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, session_id);
        assert_eq!(entries[0].model, "model-example");
        assert_eq!(entries[0].turn_count, 1);
        assert_eq!(entries[0].last_message_at_ms, 1);
        let request = Request::builder()
            .method("POST")
            .uri(format!("/v1/sessions/{session_id}/restore"))
            .header("authorization", "Bearer example-token")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::NO_CONTENT
        );
        let request = Request::builder()
            .uri(SESSIONS_PATH)
            .header("authorization", "Bearer example-token")
            .body(Body::empty())
            .unwrap();
        let reply = app.oneshot(request).await.unwrap();
        let body = to_bytes(reply.into_body(), 4096).await.unwrap();
        let sessions = ListSessionsResponse::decode(body).unwrap().sessions;
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].last_message_at_ms, 1);
        assert!(db.session(&session_id).unwrap().is_some());
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn subagent_detail_is_authenticated_and_scoped_to_its_session() {
        let root = std::env::temp_dir().join(format!("beale-session-example-{}", Uuid::new_v4()));
        let workspace_id = Uuid::new_v4().to_string();
        let session_id = Uuid::new_v4().to_string();
        let child_id = Uuid::new_v4().to_string();
        let tool_id = Uuid::new_v4().to_string();
        let db = HostDatabase::open(&root).unwrap();
        db.replace_workspaces(&[beale_storage::WorkspaceRow {
            id: workspace_id.clone(),
            name: "Example Workspace".to_owned(),
            directory: "example-directory".to_owned(),
        }])
        .unwrap();
        let mut child_messages = vec![
            json!({"role":"reasoning","text":"Checking the example boundary."}),
            json!({"role":"tool","tool_call_id":tool_id,"name":"read","summary":"Read example.txt","is_error":false}),
            json!({"role":"assistant","text":"Example observation"}),
        ];
        child_messages.extend(
            (0..70)
                .map(|index| json!({"role":"assistant","text":format!("Synthetic note {index}")})),
        );
        let child = ChildTurnRow {
            agent_id: child_id.clone(),
            role: "scout".to_owned(),
            task: "Inspect the example fixture".to_owned(),
            result: "Example observation".to_owned(),
            is_error: false,
            messages_json: Value::Array(child_messages).to_string(),
            reasoning_json: "[]".to_owned(),
            completed_at_ms: 2,
        };
        let tool = ToolCallRow {
            id: tool_id,
            turn_number: 1,
            ordinal: 0,
            name: "read".to_owned(),
            summary: "Read example.txt".to_owned(),
            arguments: "{}".to_owned(),
            output: "Synthetic output".to_owned(),
            is_error: false,
            agent_role: "main".to_owned(),
        };
        db.append_turn_with_metadata(
            &SessionRow {
                id: session_id.clone(),
                workspace_id,
                title: "Example session".to_owned(),
                created_at_ms: 1,
                last_message_at_ms: 1,
            },
            &TurnRow {
                number: 1,
                provider: ProviderId::Openai as i32,
                method: AuthMethod::ApiKey as i32,
                model: "model-example".to_owned(),
                prompt: "Example question".to_owned(),
                answer: "Example answer".to_owned(),
                duration_ms: 1,
                internal_prompt: false,
            },
            true,
            TurnActivity {
                tools: &[tool],
                reasoning: &[],
                commentary: &[],
                goal: None,
            },
            None,
            TurnMetadata {
                child_turns: &[child],
                ..TurnMetadata::default()
            },
        )
        .unwrap();
        let app = router(
            "example-token".to_owned(),
            WorkspaceStore::at(root.clone()).unwrap(),
        )
        .unwrap();
        let path = SESSION_SUBAGENT_DETAIL_PATH
            .replace("{id}", &session_id)
            .replace("{agent_id}", &child_id);
        let request = |path: &str, authorized: bool| {
            let builder = Request::builder().uri(path);
            let builder = if authorized {
                builder.header("authorization", "Bearer example-token")
            } else {
                builder
            };
            builder.body(Body::empty()).unwrap()
        };
        assert_eq!(
            app.clone()
                .oneshot(request(&path, false))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let reply = app.clone().oneshot(request(&path, true)).await.unwrap();
        assert_eq!(reply.status(), StatusCode::OK);
        let body = to_bytes(reply.into_body(), 65_536).await.unwrap();
        let detail = SessionSubagentDetail::decode(body).unwrap();
        assert_eq!(detail.summary.as_ref().unwrap().id, child_id);
        assert_eq!(
            detail
                .items
                .iter()
                .take(3)
                .map(|item| item.kind.as_str())
                .collect::<Vec<_>>(),
            ["reasoning", "tool", "message"]
        );
        assert_eq!(
            (detail.offset, detail.next_offset, detail.total_items),
            (0, 64, 73)
        );
        assert!(detail.result.is_empty());
        assert_eq!(
            detail.items[1].tool.as_ref().unwrap().summary,
            "Read example.txt"
        );
        assert!(!String::from_utf8_lossy(&detail.encode_to_vec()).contains("Synthetic output"));
        let final_reply = app
            .clone()
            .oneshot(request(&format!("{path}?offset=64"), true))
            .await
            .unwrap();
        assert_eq!(final_reply.status(), StatusCode::OK);
        let final_body = to_bytes(final_reply.into_body(), 65_536).await.unwrap();
        let final_page = SessionSubagentDetail::decode(final_body).unwrap();
        assert_eq!(
            (
                final_page.offset,
                final_page.next_offset,
                final_page.total_items
            ),
            (64, 73, 73)
        );
        assert_eq!(final_page.items.len(), 9);
        assert_eq!(final_page.result, "Example observation");
        let other_path = SESSION_SUBAGENT_DETAIL_PATH
            .replace("{id}", &Uuid::new_v4().to_string())
            .replace("{agent_id}", &child_id);
        assert_eq!(
            app.oneshot(request(&other_path, true))
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn quick_chat_requires_auth_and_keeps_unconfigured_chats_out_of_sessions() {
        let root =
            std::env::temp_dir().join(format!("beale-quick-chat-example-{}", Uuid::new_v4()));
        let app = router(
            "example-token".to_owned(),
            WorkspaceStore::at(root.clone()).unwrap(),
        )
        .unwrap();
        let payload = beale_contracts::v1::QuickChatTurnRequest {
            chat_id: String::new(),
            prompt: "Example question".to_owned(),
            workspace_id: String::new(),
        }
        .encode_to_vec();
        let request = Request::builder()
            .method("POST")
            .uri(QUICK_CHAT_TURNS_PATH)
            .header("content-type", beale_contracts::PROTOBUF_CONTENT_TYPE)
            .body(Body::from(payload.clone()))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let request = Request::builder()
            .method("POST")
            .uri(QUICK_CHAT_TURNS_PATH)
            .header("authorization", "Bearer example-token")
            .header("content-type", beale_contracts::PROTOBUF_CONTENT_TYPE)
            .body(Body::from(payload))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::CONFLICT
        );
        let request = Request::builder()
            .method("POST")
            .uri(QUICK_CHAT_TURNS_PATH)
            .header("authorization", "Bearer example-token")
            .header("content-type", beale_contracts::PROTOBUF_CONTENT_TYPE)
            .body(Body::from(
                beale_contracts::v1::QuickChatTurnRequest {
                    chat_id: String::new(),
                    prompt: "Example question".to_owned(),
                    workspace_id: Uuid::new_v4().to_string(),
                }
                .encode_to_vec(),
            ))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
        let request = Request::builder()
            .uri(SESSIONS_PATH)
            .header("authorization", "Bearer example-token")
            .body(Body::empty())
            .unwrap();
        let reply = app.oneshot(request).await.unwrap();
        assert_eq!(reply.status(), StatusCode::OK);
        let body = to_bytes(reply.into_body(), 4096).await.unwrap();
        let list = ListSessionsResponse::decode(body).unwrap();
        assert!(list.sessions.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn archived_quick_chat_can_be_loaded_without_provider_access() {
        let root = std::env::temp_dir().join(format!("beale-chat-example-{}", Uuid::new_v4()));
        let id = Uuid::new_v4().to_string();
        let db = HostDatabase::open(&root).unwrap();
        db.append_new_quick_chat_turn(
            &SessionRow {
                id: id.clone(),
                workspace_id: quick_chat::WORKSPACE_ID.to_owned(),
                title: "Example chat".to_owned(),
                created_at_ms: 1,
                last_message_at_ms: 1,
            },
            &TurnRow {
                number: 1,
                provider: ProviderId::Openai as i32,
                method: AuthMethod::ApiKey as i32,
                model: "model-example".to_owned(),
                prompt: "Example question".to_owned(),
                answer: "Example answer".to_owned(),
                duration_ms: 1,
                internal_prompt: false,
            },
            "",
        )
        .unwrap();
        let app = router(
            "example-token".to_owned(),
            WorkspaceStore::at(root.clone()).unwrap(),
        )
        .unwrap();
        let request = Request::builder()
            .uri(format!("/v1/quick-chats/{id}"))
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let request = Request::builder()
            .uri(format!("/v1/quick-chats/{id}"))
            .header("authorization", "Bearer example-token")
            .body(Body::empty())
            .unwrap();
        let reply = app.clone().oneshot(request).await.unwrap();
        let body = to_bytes(reply.into_body(), 4096).await.unwrap();
        let detail = beale_contracts::v1::QuickChatDetail::decode(body).unwrap();
        assert_eq!(detail.chat_id, id);
        assert_eq!(detail.messages.len(), 2);
        let request = Request::builder()
            .uri(ARCHIVE_PATH)
            .header("authorization", "Bearer example-token")
            .body(Body::empty())
            .unwrap();
        let reply = app.oneshot(request).await.unwrap();
        let body = to_bytes(reply.into_body(), 4096).await.unwrap();
        let entries = ListArchiveResponse::decode(body).unwrap().entries;
        assert_eq!(entries.len(), 1);
        assert!(entries[0].quick_chat);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn streamed_turn_reports_validation_failure_in_framed_event() {
        let root = std::env::temp_dir().join(format!("beale-session-example-{}", Uuid::new_v4()));
        let app = router(
            "example-token".to_owned(),
            WorkspaceStore::at(root).unwrap(),
        )
        .unwrap();
        let request = SubmitSessionTurnRequest {
            workspace_id: Uuid::new_v4().to_string(),
            session_id: String::new(),
            provider: ProviderId::Openai as i32,
            method: AuthMethod::ApiKey as i32,
            model: "model-example".to_owned(),
            prompt: "Example prompt".to_owned(),
            reasoning_effort: ReasoningEffort::High as i32,
            goal_mode: false,
            goal_max_turns: 0,
            goal_max_duration_minutes: 0,
            repeat_preset: String::new(),
            graph_id: String::new(),
            graph_models: vec![],
            graph_node_concurrency: vec![],
            agent_role: String::new(),
        };
        let request = Request::builder()
            .method("POST")
            .uri(SESSION_TURN_STREAM_PATH)
            .header("authorization", "Bearer example-token")
            .header("content-type", beale_contracts::PROTOBUF_CONTENT_TYPE)
            .body(Body::from(request.encode_to_vec()))
            .unwrap();
        let reply = app.oneshot(request).await.unwrap();
        assert_eq!(reply.status(), StatusCode::OK);
        assert_eq!(
            reply.headers()[header::CONTENT_TYPE],
            PROTOBUF_STREAM_CONTENT_TYPE
        );
        let body = to_bytes(reply.into_body(), 4096).await.unwrap();
        let size = u32::from_be_bytes(body[..4].try_into().unwrap()) as usize;
        assert_eq!(body.len(), size + 4);
        let event = SessionStreamEvent::decode(&body[4..]).unwrap();
        assert_eq!(event.kind, SessionStreamKind::Failed as i32);
        assert_eq!(event.text, "Workspace or session is unavailable.");
    }

    #[tokio::test]
    async fn tool_detail_route_requires_auth_and_returns_bounded_pages() {
        let root = std::env::temp_dir().join(format!("beale-session-example-{}", Uuid::new_v4()));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let db = HostDatabase::open(&root).unwrap();
        let workspace_id = Uuid::new_v4().to_string();
        db.replace_workspaces(&[beale_storage::WorkspaceRow {
            id: workspace_id.clone(),
            name: "Example".to_owned(),
            directory: workspace.to_string_lossy().into_owned(),
        }])
        .unwrap();
        let session = SessionRow {
            id: Uuid::new_v4().to_string(),
            workspace_id,
            title: "Example".to_owned(),
            created_at_ms: 1,
            last_message_at_ms: 1,
        };
        let turn = TurnRow {
            number: 1,
            provider: ProviderId::Openai as i32,
            method: AuthMethod::ApiKey as i32,
            model: "model-example".to_owned(),
            prompt: "Read".to_owned(),
            answer: "Done".to_owned(),
            duration_ms: 3_456,
            internal_prompt: false,
        };
        let tool = ToolCallRow {
            id: Uuid::new_v4().to_string(),
            turn_number: 1,
            ordinal: 0,
            name: "read".to_owned(),
            summary: "Read example.txt".to_owned(),
            arguments: "{\"path\":\"example.txt\"}".to_owned(),
            output: "x".repeat(20_000),
            is_error: false,
            agent_role: "main".to_owned(),
        };
        db.append_turn_with_tools(&session, &turn, true, std::slice::from_ref(&tool))
            .unwrap();
        let app = router(
            "example-token".to_owned(),
            WorkspaceStore::at(root.clone()).unwrap(),
        )
        .unwrap();
        let path = SESSION_TOOL_DETAIL_PATH
            .replace("{id}", &session.id)
            .replace("{tool_id}", &tool.id);
        let request = Request::builder()
            .uri(format!("{path}?offset=0"))
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let request = Request::builder()
            .uri(format!("{path}?offset=0"))
            .header("authorization", "Bearer example-token")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let page =
            SessionToolDetailPage::decode(to_bytes(response.into_body(), 100_000).await.unwrap())
                .unwrap();
        assert_eq!(page.output.len(), TOOL_PAGE_BYTES);
        assert_eq!(page.next_offset, TOOL_PAGE_BYTES as u64);
        assert_eq!(page.total_bytes, 20_000);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
}
