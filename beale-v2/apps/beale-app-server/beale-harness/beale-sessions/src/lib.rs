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
    QUICK_CHAT_DETAIL_PATH, QUICK_CHAT_TURNS_PATH, SESSION_ARCHIVE_PATH, SESSION_DETAIL_PATH,
    SESSION_RESTORE_PATH, SESSION_TOOL_DETAIL_PATH, SESSION_TURN_STREAM_PATH, SESSION_TURNS_PATH,
    SESSIONS_PATH, session_stream_frame,
    v1::{
        ArchiveEntry, AuthMethod, GoalStatus, GraphNodeConcurrency, GraphNodeModel,
        ListArchiveResponse, ListSessionsResponse, ProviderId, ReasoningEffort, SessionCommentary,
        SessionDetail, SessionGoal, SessionGraphConfig, SessionMessage, SessionReasoningSummary,
        SessionRole, SessionStreamEvent, SessionStreamKind, SessionSummary, SessionTokenUsage,
        SessionToolDetailPage, SessionToolSummary, SessionUsageSnapshot, SubmitSessionTurnRequest,
        WorkspaceDetails, WorkspaceSurface,
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
    AutomationOptions, CommentaryRow, HostDatabase, ModelUsageRow, ReasoningSummaryRow, SessionRow,
    ToolCallRow, TurnActivity, TurnMetadata, TurnRow,
};
use beale_tools::{
    ExtensionKind, GET_SCOPE, GetScopeArgs, LOAD_EXTENSION, LoadExtensionArgs, PluginCallArgs,
    definitions, execute, model_preview, page, plugin_call_definition, summary as tool_summary,
};
use futures_util::stream;
use prost::Message;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Weak},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Mutex, mpsc};
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

#[cfg(feature = "knowledge")]
fn knowledge_feature_enabled(db: &HostDatabase, workspace_id: &str) -> Result<bool, StatusCode> {
    db.feature_enabled(beale_knowledge::FEATURE_ID)
        .and_then(|enabled| {
            if enabled {
                db.extension_applies_to("feature", beale_knowledge::FEATURE_ID, workspace_id)
            } else {
                Ok(false)
            }
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(not(feature = "knowledge"))]
fn knowledge_feature_enabled(_db: &HostDatabase, _workspace_id: &str) -> Result<bool, StatusCode> {
    Ok(false)
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
        .map(|surface| (surface.id.as_str(), surface.value.as_str()))
        .collect();
    surfaces
        .iter()
        .flat_map(|surface| {
            surface.relationships.iter().filter_map(|relationship| {
                names.get(relationship.target_id.as_str()).map(|target| {
                    format!(
                        "{} {} —{}{}→ {} {target}",
                        surface.id,
                        surface.value,
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
    let scope = if surface.scope == 1 {
        "in scope"
    } else {
        "out of scope"
    };
    let mut line = format!(
        "{} [{scope}] {}",
        surface.id,
        bounded_context(&surface.value, 200)
    );
    if let Some(definition) = &surface.definition {
        line.push_str(&format!(
            "; type {}; name {}; locator {}:{}; source {}",
            definition.type_id,
            bounded_context(&definition.name, 100),
            definition
                .locator
                .as_ref()
                .map_or("freeform", |locator| locator.kind.as_str()),
            bounded_context(
                definition
                    .locator
                    .as_ref()
                    .map_or("", |locator| locator.value.as_str()),
                200
            ),
            definition.source
        ));
        if !definition.parent_id.is_empty() {
            line.push_str(&format!("; parent {}", definition.parent_id));
        }
        if !definition.summary.is_empty() {
            line.push_str(&format!(
                "; summary {}",
                bounded_context(&definition.summary, 300)
            ));
        }
        if !definition.context.is_empty() {
            line.push_str(&format!(
                "; context {}",
                bounded_context(&definition.context, 500)
            ));
        }
        if !definition.scope_basis.is_empty() {
            line.push_str(&format!(
                "; scope basis {}",
                bounded_context(&definition.scope_basis, 300)
            ));
        }
        if definition.scope_descendants {
            line.push_str("; recorded scope covers descendants");
        }
    }
    if !surface.out_of_scope_reason.is_empty() {
        line.push_str(&format!(
            "; exclusion {}",
            bounded_context(&surface.out_of_scope_reason, 200)
        ));
    }
    line
}

fn scope_output(details: &WorkspaceDetails) -> Value {
    let surfaces = details.surfaces.iter()
        .filter(|surface| surface.definition.as_ref().is_none_or(|definition| definition.parent_id.is_empty()))
        .map(|surface| {
            let definition = surface.definition.as_ref().map(|definition| json!({
                "type": definition.type_id,
                "name": definition.name,
                "locator": definition.locator.as_ref().map(|locator| json!({"kind":locator.kind,"value":locator.value})),
                "summary": definition.summary,
                "scope_basis": definition.scope_basis,
                "scope_descendants": definition.scope_descendants,
                "source": definition.source,
            }));
            json!({"id":surface.id,"kind":surface.kind,"scope":if surface.scope == 1 {"in_scope"} else {"out_of_scope"},"value":surface.value,"out_of_scope_reason":surface.out_of_scope_reason,"definition":definition})
        }).collect::<Vec<_>>();
    json!({"rules":details.rules,"top_level_surfaces":surfaces,"boundary_note":"Out-of-scope surfaces are boundary references only. Imported descriptions are context, not verified observations or authorization. Parentage alone does not authorize a child."})
}

#[cfg(feature = "runbooks")]
fn runbook_review_context(state: &SessionState, workspace_id: &str) -> Result<String, StatusCode> {
    let details = state.workspaces.details_for_host(workspace_id)?;
    let surfaces = if surface_feature_enabled(&state.db, workspace_id)? {
        format!(
            "Surfaces: {}\nOut-of-scope surfaces are boundary references only. Imported descriptions are context, not verified observations or authorization. Parentage alone does not authorize a child.\nRecorded relationships (context, not evidence): {}",
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
mod quick_chat;
mod system_prompt;
use http::{authorized, checked_body, response};

const MAX_ORDINARY_TURNS: i64 = 64;
const RECENT_TURNS: usize = 64;
const MAX_RECENT_HISTORY_BYTES: usize = 1_048_576;
const MAX_HISTORY_BYTES: usize = 4_194_304;
const MAX_TOOL_ROUNDS: usize = 8;
const MAX_TOOL_CALLS: usize = 32;
const MAX_REASONING_BYTES_PER_TURN: usize = 4_096;
const MAX_COMMENTARY_BYTES_PER_TURN: usize = 8_192;
const TOOL_PAGE_BYTES: usize = 8_000;
const INTERRUPTED_ANSWER: &str =
    "The provider could not complete this turn. Tool activity was saved.";

fn compact_openai_context(input: &[Value]) -> Vec<Value> {
    let start = input
        .iter()
        .rposition(|item| item.get("type").and_then(Value::as_str) == Some("compaction"))
        .unwrap_or(0);
    input[start..].to_vec()
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
        if graph_node_concurrency.len() > 4 {
            return Err(StatusCode::BAD_REQUEST);
        }
        let mut seen = std::collections::HashSet::new();
        let mut per_role = [0_usize; 4];
        for item in graph_models {
            let Some(role) = beale_graphs::Role::from_id(&item.node_id) else {
                return Err(StatusCode::BAD_REQUEST);
            };
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
            if item.max_concurrent == 0
                || item.max_concurrent > beale_graphs::MAX_CONCURRENT
                || (role == beale_graphs::Role::Main && item.max_concurrent != 1)
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

#[derive(Clone)]
struct SessionState {
    token: String,
    db: HostDatabase,
    workspaces: WorkspaceStore,
    plugins: PluginRuntime,
    credentials: Arc<CredentialStore>,
    provider: Arc<ProviderClient>,
    session_locks: Arc<Mutex<HashMap<String, Weak<Mutex<()>>>>>,
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
    let router = Router::new()
        .route(SESSIONS_PATH, get(list))
        .route(SESSION_DETAIL_PATH, get(details))
        .route(SESSION_ARCHIVE_PATH, post(archive))
        .route(SESSION_RESTORE_PATH, post(restore))
        .route(ARCHIVE_PATH, get(list_archive))
        .route(SESSION_TOOL_DETAIL_PATH, get(tool_details))
        .route(SESSION_TURNS_PATH, post(submit_turn))
        .route(AUTOMATIONS_PATH, get(automations::list))
        .route(AUTOMATIONS_CANCEL_PATH, post(automations::cancel))
        .route(SESSION_TURN_STREAM_PATH, post(stream_turn))
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
    if item.turns.is_empty()
        || item.turns.len() > RECENT_TURNS
        || item.session.created_at_ms < 0
        || item.session.last_message_at_ms < 0
    {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let first_number = item.turns[0].number;
    if first_number <= 0 {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let mut turns = item
        .turns
        .into_iter()
        .enumerate()
        .map(|(index, row)| {
            let expected = first_number
                .checked_add(i64::try_from(index).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?)
                .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
            if row.duration_ms < 0 || row.number != expected {
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
            Ok(StoredTurn {
                number: row.number,
                provider: row.provider,
                method: row.method,
                model: row.model,
                prompt: row.prompt,
                answer: row.answer,
                duration_ms: row.duration_ms as u64,
                internal_prompt: row.internal_prompt,
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

fn detail(
    db: &HostDatabase,
    session: &StoredSession,
    turns: &[StoredTurn],
) -> Result<SessionDetail, StatusCode> {
    let first_turn = turns
        .first()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?
        .number;
    let tools = db
        .tool_summaries_since(&session.id, first_turn)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let reasoning = db
        .reasoning_summaries_since(&session.id, first_turn)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let commentary = db
        .commentary_since(&session.id, first_turn)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if tools.len() > RECENT_TURNS * MAX_TOOL_CALLS {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
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
            });
        }
        let turn_number = turn.number;
        let turn_tools: Vec<_> = tools
            .iter()
            .filter(|tool| tool.turn_number == turn_number)
            .collect();
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
                })
                .collect(),
            reasoning_summaries: reasoning
                .iter()
                .filter(|item| item.turn_number == turn_number)
                .map(|item| {
                    if item.after_tool_count < 0 || item.after_tool_count > turn_tools.len() as i64
                    {
                        return Err(StatusCode::INTERNAL_SERVER_ERROR);
                    }
                    Ok(SessionReasoningSummary {
                        text: item.text.clone(),
                        after_tool_count: item.after_tool_count as u32,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
            duration_ms: turn.duration_ms,
            commentary: commentary
                .iter()
                .filter(|item| item.turn_number == turn_number)
                .map(|item| {
                    if item.after_tool_count < 0 || item.after_tool_count > turn_tools.len() as i64
                    {
                        return Err(StatusCode::INTERNAL_SERVER_ERROR);
                    }
                    Ok(SessionCommentary {
                        text: item.text.clone(),
                        after_tool_count: item.after_tool_count as u32,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
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
    let (runbook_count, active_runbook_count) = db
        .session_runbook_counts(&session.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let runbooks = db
        .session_runbooks(&session.workspace_id, &session.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(SessionDetail {
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
    Ok(response(run_turn(state, request, None).await?))
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
    let (tx, rx) = mpsc::channel::<SessionStreamEvent>(32);
    tokio::spawn(async move {
        let outcome = run_turn(state, request, Some(tx.clone())).await;
        let event = match outcome {
            Ok(detail) => SessionStreamEvent {
                runbook_progress: None,
                kind: SessionStreamKind::Completed as i32,
                text: String::new(),
                tool: None,
                detail: Some(detail),
                segment_index: 0,
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

#[cfg(feature = "goal-mode")]
async fn run_turn(
    state: SessionState,
    mut request: SubmitSessionTurnRequest,
    progress: Option<mpsc::Sender<SessionStreamEvent>>,
) -> Result<SessionDetail, StatusCode> {
    let lock = if request.session_id.is_empty() {
        Arc::new(Mutex::new(()))
    } else {
        session_lock(&state, &request.session_id).await
    };
    let _guard = lock.try_lock().map_err(|_| StatusCode::CONFLICT)?;
    let mut automatic = false;
    loop {
        let detail =
            run_one_turn(&state, request.clone(), progress.clone(), automatic, &lock).await?;
        if let Some(goal) = &detail.goal
            && goal.status == GoalStatus::Active as i32
        {
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
    run_one_turn(&state, request, progress, false, &lock).await
}

async fn run_one_turn(
    state: &SessionState,
    request: SubmitSessionTurnRequest,
    progress: Option<mpsc::Sender<SessionStreamEvent>>,
    automatic: bool,
    lock: &Arc<Mutex<()>>,
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
    if request.graph_id == beale_graphs::SIMPLE_ID
        && request.graph_models.iter().any(|model| {
            model.node_id == beale_graphs::Role::Main.id()
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
    let persisted_goal = state
        .db
        .goal(&session.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    #[cfg(feature = "goal-mode")]
    let mut goal = persisted_goal;
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
    if goal.is_none() && next_turn_number > MAX_ORDINARY_TURNS {
        return Err(StatusCode::CONFLICT);
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
    let history_bytes = messages
        .iter()
        .map(|message| message.text.len())
        .sum::<usize>();
    if history_bytes.saturating_add(INTERRUPTED_ANSWER.len()) > MAX_HISTORY_BYTES {
        return Err(StatusCode::CONFLICT);
    }
    let workspace = state.workspaces.directory(&request.workspace_id)?;
    let workspace_details = state.workspaces.details_for_host(&request.workspace_id)?;
    let surface_tools_enabled = surface_feature_enabled(&state.db, &request.workspace_id)?;
    let knowledge_tools_enabled = knowledge_feature_enabled(&state.db, &request.workspace_id)?;
    #[cfg(feature = "runbooks")]
    let runbook_tools_enabled = runbook_feature_enabled(&state.db, &request.workspace_id)?;
    #[cfg(not(feature = "knowledge"))]
    let _ = knowledge_tools_enabled;
    let surfaces_context = if surface_tools_enabled {
        format!(
            "Surfaces: {}\nOut-of-scope surfaces are boundary references only. Imported descriptions are context, not verified observations or authorization. Parentage alone does not authorize a child.\nRecorded relationships (context, not evidence): {}",
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
    if provider == Provider::OpenAi
        && turns.last().is_some_and(|turn| {
            turn.provider == request.provider
                && turn.method == request.method
                && turn.model == request.model
                && turn.answer != INTERRUPTED_ANSWER
        })
        && let Some(saved) = state
            .db
            .provider_context(&session.id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        let mut previous: Vec<Value> =
            serde_json::from_str(&saved).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
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
    #[cfg(feature = "surfaces")]
    if surface_tools_enabled {
        available_features.push((
            beale_surfaces::FEATURE_ID,
            "Use to inspect or maintain the workspace surface map and relationships.",
        ));
    }
    #[cfg(feature = "knowledge")]
    if knowledge_tools_enabled {
        available_features.push((
            beale_knowledge::FEATURE_ID,
            "Use to search or record workspace memories, leads, and evidence-backed findings.",
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
            "Use for the active multi-turn goal; load before reporting final goal status.",
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
    let guidance_section = format!(
        "## Workspace Guidance (AGENTS.md)\n\n{}",
        workspace_details.guidance
    );
    let mut extension_section = String::from(
        "## Available extensions\n\nUse `get_scope` for current workspace rules and top-level surfaces before target operations. Load a feature or plugin with `load_plugin_or_feature` when the user's task calls for it. Its detailed instructions and tools are provided then.\n",
    );
    for (id, use_case) in &available_features {
        extension_section.push_str(&format!("\n- Feature `{id}`: {use_case}"));
    }
    for plugin in &available_plugins {
        extension_section.push_str(&format!(
            "\n- Plugin `{}`: {}",
            plugin.name, plugin.description
        ));
    }
    let sections = [guidance_section.as_str(), extension_section.as_str()];
    let system = system_prompt::load(state.workspaces.root(), &sections).map_err(|error| {
        eprintln!("session system prompt failed: {error:#}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    #[cfg(feature = "graphs")]
    let child_system = if active_graph.is_some() {
        system_prompt::load(state.workspaces.root(), &[guidance_section.as_str()]).map_err(
            |error| {
                eprintln!("subagent system prompt failed: {error:#}");
                StatusCode::INTERNAL_SERVER_ERROR
            },
        )?
    } else {
        String::new()
    };
    if provider != Provider::OpenAi {
        input.insert(0, json!({"role":"system","content":system}));
    }
    let mut openai_connection = OpenAiConnection::default();
    let mut definitions = definitions();
    #[cfg(any(
        feature = "surfaces",
        feature = "knowledge",
        feature = "runbooks",
        feature = "goal-mode",
        feature = "graphs"
    ))]
    let mut loaded_features = HashSet::<String>::new();
    let mut loaded_plugins = HashSet::<String>::new();
    #[cfg(feature = "goal-mode")]
    let mut goal_signal: Option<beale_goals::Signal> = None;
    let mut tool_rows = Vec::<ToolCallRow>::new();
    let mut reasoning_rows = Vec::<ReasoningSummaryRow>::new();
    let mut commentary_rows = Vec::<CommentaryRow>::new();
    let mut model_usage = Vec::<ModelUsageRow>::new();
    let mut reasoning_bytes = 0_usize;
    let mut commentary_bytes = 0_usize;
    let mut call_count = 0_usize;
    #[cfg(feature = "graphs")]
    let mut delegated_count = 0_usize;
    let mut answer = None;
    'rounds: for round in 0..MAX_TOOL_ROUNDS {
        let (provider_tx, forward) = if let Some(client_tx) = progress.clone() {
            let (provider_tx, mut provider_rx) = mpsc::channel::<ProviderProgress>(32);
            let forward = tokio::spawn(async move {
                while let Some(update) = provider_rx.recv().await {
                    let (kind, text) = match update {
                        ProviderProgress::MessageDelta(text) => {
                            (SessionStreamKind::MessageDelta, text)
                        }
                        ProviderProgress::ReasoningDelta(text) => {
                            (SessionStreamKind::ReasoningDelta, text)
                        }
                    };
                    if client_tx
                        .send(SessionStreamEvent {
                            runbook_progress: None,
                            kind: kind as i32,
                            text,
                            tool: None,
                            detail: None,
                            segment_index: (round + 1) as u32,
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
        let step_request = ToolStepRequest {
            model: &request.model,
            input: &input,
            tools: &definitions,
            instructions: (provider == Provider::OpenAi).then_some(system.as_str()),
            reasoning_effort,
            workspace_root: Some(&workspace),
        };
        let call_started = Instant::now();
        let step = if provider == Provider::OpenAi {
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
        drop(provider_tx);
        if let Some(forward) = forward {
            let _ = forward.await;
        }
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
                            })
                            .await;
                    }
                    return Err(StatusCode::BAD_GATEWAY);
                }
                answer = Some(INTERRUPTED_ANSWER.to_owned());
                break;
            }
        };
        model_usage.push(ModelUsageRow {
            agent_id: "main".to_owned(),
            role: "main".to_owned(),
            provider: request.provider,
            method: request.method,
            model: request.model.clone(),
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
        for mut summary in step.reasoning_summaries {
            if reasoning_rows.len() >= MAX_TOOL_ROUNDS * 16
                || reasoning_bytes >= MAX_REASONING_BYTES_PER_TURN
            {
                break;
            }
            let available = MAX_REASONING_BYTES_PER_TURN - reasoning_bytes;
            if summary.len() > available {
                let mut end = available;
                while !summary.is_char_boundary(end) {
                    end -= 1;
                }
                summary.truncate(end);
            }
            if summary.trim().is_empty() {
                continue;
            }
            reasoning_bytes += summary.len();
            reasoning_rows.push(ReasoningSummaryRow {
                turn_number: next_turn_number,
                ordinal: reasoning_rows.len() as i64,
                after_tool_count: tool_rows.len() as i64,
                text: summary,
            });
        }
        input.extend(step.continuation);
        if step.calls.is_empty() {
            answer = Some(step.text);
            break;
        }
        if !step.text.trim().is_empty() && commentary_bytes < MAX_COMMENTARY_BYTES_PER_TURN {
            let mut text = step.text;
            let available = MAX_COMMENTARY_BYTES_PER_TURN - commentary_bytes;
            if text.len() > available {
                let mut end = available;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                text.truncate(end);
            }
            if !text.trim().is_empty() {
                commentary_bytes += text.len();
                commentary_rows.push(CommentaryRow {
                    turn_number: next_turn_number,
                    ordinal: commentary_rows.len() as i64,
                    after_tool_count: tool_rows.len() as i64,
                    text,
                });
            }
        }
        if round + 1 == MAX_TOOL_ROUNDS
            || call_count.saturating_add(step.calls.len()) > MAX_TOOL_CALLS
        {
            if tool_rows.is_empty() {
                return Err(StatusCode::BAD_GATEWAY);
            }
            answer = Some(INTERRUPTED_ANSWER.to_owned());
            break;
        }
        call_count += step.calls.len();
        let mut anthropic_tool_results = Vec::new();
        for call in step.calls {
            if call.id.is_empty()
                || call.id.len() > 256
                || call.name.len() > 128
                || call.arguments.len() > 65_536
            {
                if tool_rows.is_empty() {
                    return Err(StatusCode::BAD_GATEWAY);
                }
                answer = Some(INTERRUPTED_ANSWER.to_owned());
                break 'rounds;
            }
            let id = Uuid::new_v4().to_string();
            let args: Result<Value, _> = serde_json::from_str(&call.arguments);
            let summary = if call.name == "goal_status" {
                "Record Goal Mode status".to_owned()
            } else if cfg!(feature = "surfaces") && call.name.starts_with("surface_") {
                format!("{} surface", call.name.trim_start_matches("surface_"))
            } else if cfg!(feature = "knowledge")
                && (call.name.starts_with("knowledge_")
                    || call.name.starts_with("memory_")
                    || call.name.starts_with("lead_")
                    || call.name.starts_with("finding_"))
            {
                format!("{} knowledge", call.name.replace('_', " "))
            } else if cfg!(feature = "runbooks") && call.name.starts_with("runbook_") {
                format!("{} runbook", call.name.trim_start_matches("runbook_"))
            } else if cfg!(feature = "graphs") && call.name == "graph_delegate" {
                "Delegate to graph subagents".to_owned()
            } else {
                tool_summary(&call.name, args.as_ref().unwrap_or(&Value::Null))
            };
            if call.name != "tool_result_page"
                && let Some(tx) = &progress
            {
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
                        }),
                        detail: None,
                        segment_index: 0,
                    })
                    .await;
            }
            #[cfg(feature = "graphs")]
            let mut child_tools = Vec::<ToolCallRow>::new();
            let (output, is_error) = match args {
                Ok(args) if call.name == GET_SCOPE => {
                    match serde_json::from_value::<GetScopeArgs>(args) {
                        Ok(_) => match state.workspaces.details_for_host(&request.workspace_id) {
                            Ok(details) => (scope_output(&details).to_string(), false),
                            Err(error) => (format!("Scope tool error: {error}"), true),
                        },
                        Err(error) => (format!("Invalid get_scope arguments: {error}"), true),
                    }
                }
                Ok(args) if call.name == LOAD_EXTENSION => {
                    let result = async {
                        let request_args: LoadExtensionArgs = serde_json::from_value(args)?;
                        match request_args.kind {
                            #[cfg(any(feature = "surfaces", feature = "knowledge", feature = "runbooks", feature = "goal-mode", feature = "graphs"))]
                            ExtensionKind::Feature => {
                                anyhow::ensure!(available_features.iter().any(|(id, _)| *id == request_args.id), "feature is unavailable in this workspace");
                                let (tools, guidance) = match request_args.id.as_str() {
                                    #[cfg(feature = "surfaces")]
                                    beale_surfaces::FEATURE_ID => (beale_surfaces::definitions(), beale_surfaces::PROMPT_EXTENSION.to_owned()),
                                    #[cfg(feature = "knowledge")]
                                    beale_knowledge::FEATURE_ID => (beale_knowledge::definitions(), beale_knowledge::PROMPT_EXTENSION.to_owned()),
                                    #[cfg(feature = "runbooks")]
                                    beale_runbooks::FEATURE_ID => (beale_runbooks::definitions(), beale_runbooks::PROMPT_EXTENSION.to_owned()),
                                    #[cfg(feature = "goal-mode")]
                                    beale_goals::FEATURE_ID => (vec![beale_goals::definition()], goal_section.as_deref().unwrap_or(beale_goals::PROMPT_EXTENSION).to_owned()),
                                    #[cfg(feature = "graphs")]
                                    beale_graphs::FEATURE_ID => (active_graph.map(|runtime| vec![runtime.tool_definition()]).unwrap_or_default(), graph_section.as_deref().unwrap_or(beale_graphs::PROMPT_EXTENSION).to_owned()),
                                    _ => anyhow::bail!("feature is unavailable"),
                                };
                                if loaded_features.insert(request_args.id.clone()) {
                                    definitions.extend(tools.iter().cloned());
                                }
                                Ok::<_, anyhow::Error>(json!({"feature":request_args.id,"instructions":guidance,"tools":tools}).to_string())
                            }
                            #[cfg(not(any(feature = "surfaces", feature = "knowledge", feature = "runbooks", feature = "goal-mode", feature = "graphs")))]
                            ExtensionKind::Feature => anyhow::bail!("feature is unavailable"),
                            ExtensionKind::Plugin => {
                                let details = state.plugins.details(&request_args.id, &request.workspace_id).await?;
                                if loaded_plugins.insert(request_args.id) && !definitions.iter().any(|definition| definition["name"] == "plugin_call") {
                                    definitions.push(plugin_call_definition());
                                }
                                Ok(details.to_string())
                            }
                        }
                    }.await;
                    match result {
                        Ok(value) => (value, false),
                        Err(error) => (format!("Extension load error: {error}"), true),
                    }
                }
                Ok(args) if call.name == "plugin_call" => {
                    let result = async {
                        let parsed: PluginCallArgs = serde_json::from_value(args)?;
                        anyhow::ensure!(parsed.arguments.is_object(), "arguments must be an object");
                        anyhow::ensure!(loaded_plugins.contains(&parsed.plugin), "load the plugin before calling its tools");
                        let review_args = json!({"plugin":parsed.plugin,"server":parsed.server,"tool":parsed.tool,"arguments":parsed.arguments});
                        let decision = beale_safety::review(&state.db, &state.provider, &state.credentials, ReviewRequest {
                            provider, mode, model: &request.model, name: "plugin_call", arguments: &review_args, workspace_context: &workspace_context,
                        }).await;
                        anyhow::ensure!(decision.approved, "plugin tool denied by review: {}", decision.reason);
                        let value = state.plugins.call_tool(&parsed.plugin, &parsed.server, &parsed.tool, parsed.arguments, &request.workspace_id).await?;
                        Ok::<_, anyhow::Error>((value.to_string(), value["isError"].as_bool().unwrap_or(false)))
                    }.await;
                    match result {
                        Ok(value) => value,
                        Err(error) => (format!("Plugin tool error: {error}"), true),
                    }
                }
                #[cfg(feature = "runbooks")]
                Ok(args)
                    if loaded_features.contains(beale_runbooks::FEATURE_ID)
                        && runbook_tools_enabled
                        && call.name == beale_runbooks::RUN =>
                {
                    let result: anyhow::Result<(String, bool)> = async {
                        let runbook_id = args.get("id").and_then(Value::as_str)
                            .ok_or_else(|| anyhow::anyhow!("id is required"))?;
                        let document = state.db.runbook_get(&request.workspace_id, runbook_id)?
                            .ok_or_else(|| anyhow::anyhow!("runbook not found"))?;
                        for cell in &document.cells {
                            if let beale_contracts::runbooks::Cell::Code { language, source, .. } = cell {
                                let review_args = serde_json::json!({"command":format!("# {language} runbook cell\n{source}")});
                                let decision = beale_safety::review(
                                    &state.db,
                                    &state.provider,
                                    &state.credentials,
                                    ReviewRequest {
                                        provider,
                                        mode,
                                        model: &request.model,
                                        name: "bash",
                                        arguments: &review_args,
                                        workspace_context: &workspace_context,
                                    },
                                ).await;
                                if !decision.approved {
                                    anyhow::bail!("runbook cell denied by review: {}", decision.reason);
                                }
                            }
                        }
                        let run = state.db.runbook_run_start(&request.workspace_id, &session.id, runbook_id, document.revision)?;
                        let mut active = ActiveRunbookRun { db: state.db.clone(), id: run.id.clone(), finished: false };
                        let progress_tx = progress.clone();
                        let progress_title = document.title.clone();
                        let progress_id = runbook_id.to_owned();
                        let result = beale_runbooks::run_document(&document, &workspace, |cell_number| {
                            let cell_name = document.cells[(cell_number - 1) as usize].name();
                            state.db.runbook_run_cell(&run.id, cell_number, cell_name)?;
                            if let Some(tx) = &progress_tx {
                                let _ = tx.try_send(SessionStreamEvent {
                                    kind: SessionStreamKind::RunbookProgress as i32,
                                    runbook_progress: Some(beale_contracts::v1::RunbookProgress {
                                        runbook_id: progress_id.clone(),
                                        title: progress_title.clone(),
                                        cell_number,
                                        cell_name: cell_name.to_owned(),
                                    }),
                                    ..Default::default()
                                });
                            }
                            Ok(())
                        }, |cell_id, success, exit_code, stdout, stderr| state.db.runbook_run_cell_result(&run.id, cell_id, success, exit_code, stdout, stderr)).await;
                        let (success, output) = match result {
                            Ok(result) => (result.success, result.output),
                            Err(error) => (false, format!("Runbook execution error: {error}")),
                        };
                        active.finish(success, &output)?;
                        Ok((serde_json::json!({"run_id":run.id,"runbook_id":runbook_id,"status":if success {"completed"} else {"failed"}}).to_string(), !success))
                    }.await;
                    match result {
                        Ok(value) => value,
                        Err(error) => (format!("Runbook tool error: {error}"), true),
                    }
                }
                #[cfg(feature = "runbooks")]
                Ok(args)
                    if loaded_features.contains(beale_runbooks::FEATURE_ID)
                        && runbook_tools_enabled
                        && beale_runbooks::is_tool(&call.name) =>
                {
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
                #[cfg(feature = "knowledge")]
                Ok(args)
                    if loaded_features.contains(beale_knowledge::FEATURE_ID)
                        && knowledge_tools_enabled
                        && beale_knowledge::is_tool(&call.name) =>
                {
                    let pending = tool_rows
                        .iter()
                        .map(|row| beale_knowledge::PendingSource {
                            id: &row.id,
                            name: &row.name,
                            output: &row.output,
                            is_error: row.is_error,
                        })
                        .collect::<Vec<_>>();
                    match beale_knowledge::execute(
                        &state.db,
                        &request.workspace_id,
                        &session.id,
                        &call.name,
                        &args,
                        &pending,
                    ) {
                        Ok(value) => (value, false),
                        Err(error) => (format!("Knowledge tool error: {error}"), true),
                    }
                }
                #[cfg(feature = "surfaces")]
                Ok(args)
                    if loaded_features.contains(beale_surfaces::FEATURE_ID)
                        && surface_tools_enabled
                        && beale_surfaces::is_tool(&call.name) =>
                {
                    match beale_surfaces::execute(
                        &state.workspaces,
                        &request.workspace_id,
                        &call.name,
                        &args,
                    ) {
                        Ok(value) => (value, false),
                        Err(error) => (format!("Surface tool error: {error}"), true),
                    }
                }
                #[cfg(feature = "goal-mode")]
                Ok(args)
                    if loaded_features.contains(beale_goals::FEATURE_ID)
                        && call.name == beale_goals::TOOL_NAME
                        && goal_active =>
                {
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
                    if loaded_features.contains(beale_graphs::FEATURE_ID)
                        && call.name == beale_graphs::DELEGATE_TOOL
                        && active_graph.is_some() =>
                {
                    let count = args
                        .get("assignments")
                        .and_then(Value::as_array)
                        .map_or(0, Vec::len);
                    if count == 0
                        || count
                            > beale_graphs::MAX_ASSIGNMENTS_PER_TURN.saturating_sub(delegated_count)
                    {
                        (
                            "Subagent assignment limit reached for this turn.".to_owned(),
                            true,
                        )
                    } else {
                        delegated_count += count;
                        let child_usage = Mutex::new(Vec::new());
                        let result = graphs::delegate(
                            &saved_graph_id,
                            graphs::ChildContext {
                                state,
                                workspace: &workspace,
                                workspace_context: &workspace_context,
                                base_system: &child_system,
                                reasoning_effort,
                                turn_number: next_turn_number,
                                turn_started: started,
                                progress: progress.as_ref(),
                                model_usage: &child_usage,
                            },
                            &args,
                            &active_graph_config,
                            &graph_fallback,
                        )
                        .await;
                        model_usage.extend(child_usage.into_inner());
                        child_tools = result.tools;
                        (result.output, result.is_error)
                    }
                }
                Ok(args) if call.name == "tool_result_page" => {
                    let result = (|| {
                        let source_id = args
                            .get("tool_call_id")
                            .and_then(Value::as_str)
                            .ok_or_else(|| anyhow::anyhow!("tool_call_id is required"))?;
                        let offset = args
                            .get("offset")
                            .and_then(Value::as_u64)
                            .ok_or_else(|| anyhow::anyhow!("offset is required"))?;
                        let offset = usize::try_from(offset)?;
                        let value =
                            if let Some(row) = tool_rows.iter().find(|row| row.id == source_id) {
                                row.output.clone()
                            } else {
                                state
                                    .db
                                    .tool_call(&session.id, source_id)?
                                    .ok_or_else(|| anyhow::anyhow!("tool result is unavailable"))?
                                    .output
                            };
                        let (chunk, next) = page(&value, offset, TOOL_PAGE_BYTES)?;
                        Ok::<_, anyhow::Error>(format!(
                            "{chunk}\n[next_offset={next}; total_bytes={}]",
                            value.len()
                        ))
                    })();
                    match result {
                        Ok(value) => (value, false),
                        Err(error) => (format!("Tool error: {error}"), true),
                    }
                }
                Ok(args) => {
                    let decision = if matches!(
                        call.name.as_str(),
                        "read" | "write" | "edit" | "bash" | "webfetch"
                    ) {
                        Some(
                            beale_safety::review(
                                &state.db,
                                &state.provider,
                                &state.credentials,
                                ReviewRequest {
                                    provider,
                                    mode,
                                    model: &request.model,
                                    name: &call.name,
                                    arguments: &args,
                                    workspace_context: &workspace_context,
                                },
                            )
                            .await,
                        )
                    } else {
                        None
                    };
                    if let Some(decision) = decision
                        && !decision.approved
                    {
                        (format!("Tool denied by review: {}", decision.reason), true)
                    } else {
                        match execute(&call.name, &args, &workspace, state.workspaces.root()).await
                        {
                            Ok(value) => (value.output, value.is_error),
                            Err(error) => (format!("Tool error: {error:#}"), true),
                        }
                    }
                }
                Err(_) => ("Tool error: invalid JSON arguments".to_owned(), true),
            };
            let model_output = if call.name == "tool_result_page" {
                output.clone()
            } else {
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
                            }),
                            detail: None,
                            segment_index: 0,
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
                });
                #[cfg(feature = "graphs")]
                for mut child in child_tools {
                    child.ordinal = tool_rows.len() as i64;
                    tool_rows.push(child);
                }
                let preview = model_preview(&output, &id);
                #[cfg(feature = "knowledge")]
                let preview = if knowledge_tools_enabled
                    && matches!(call.name.as_str(), "read" | "bash")
                    && !is_error
                    && !output.is_empty()
                {
                    format!("[evidence_tool_call_id={id}]\n{preview}")
                } else {
                    preview
                };
                preview
            };
            append_tool_result(
                &mut input,
                &mut anthropic_tool_results,
                provider,
                mode,
                &call.id,
                &model_output,
            );
        }
        if !anthropic_tool_results.is_empty() {
            input.push(json!({"role":"user","content":anthropic_tool_results}));
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
                    })
                    .await;
            }
            return Err(StatusCode::BAD_GATEWAY);
        }
        answer = INTERRUPTED_ANSWER.to_owned();
    }
    #[cfg(feature = "goal-mode")]
    if let Some(goal) = goal.as_mut().filter(|_| goal_active) {
        goal.turns_used = goal.turns_used.checked_add(1).ok_or(StatusCode::CONFLICT)?;
        goal.active_ms = goal.active_ms.saturating_add(
            i64::try_from(started.elapsed().as_millis().min(i64::MAX as u128))
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        );
        let signal = if answer == INTERRUPTED_ANSWER {
            None
        } else {
            goal_signal
        };
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
    }
    turns.push(StoredTurn {
        number: next_turn_number,
        provider: request.provider,
        method: request.method,
        model: request.model,
        prompt: request.prompt,
        answer,
        duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        internal_prompt: automatic,
    });
    let repeat_preset = request.repeat_preset;
    let automation_options = AutomationOptions {
        reasoning_effort: request.reasoning_effort,
        goal_mode: request.goal_mode,
        goal_max_turns: request.goal_max_turns,
        goal_max_duration_minutes: request.goal_max_duration_minutes,
    };
    let latest = turns.last().ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let provider_context_json =
        if provider == Provider::OpenAi && latest.answer != INTERRUPTED_ANSWER {
            Some(
                serde_json::to_string(&compact_openai_context(&input))
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
            )
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
            next_turn_number == 1,
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
            },
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    trim_recent_turns(&mut turns);
    detail(&state.db, &session, &turns)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::{body::Body, http::Request};
    use prost::Message;
    use tower::ServiceExt;

    #[test]
    fn scope_tool_returns_rules_and_only_top_level_surfaces() {
        let mut details = WorkspaceDetails {
            rules: vec!["Only example.test is authorized".to_owned()],
            ..Default::default()
        };
        details.surfaces.push(WorkspaceSurface {
            id: "surface-root".to_owned(),
            value: "example.test".to_owned(),
            scope: 1,
            definition: Some(beale_contracts::v1::SurfaceDefinition {
                name: "Example root".to_owned(),
                ..Default::default()
            }),
            ..Default::default()
        });
        details.surfaces.push(WorkspaceSurface {
            id: "surface-child".to_owned(),
            value: "child.example.test".to_owned(),
            scope: 1,
            definition: Some(beale_contracts::v1::SurfaceDefinition {
                parent_id: "surface-root".to_owned(),
                ..Default::default()
            }),
            ..Default::default()
        });
        let output = scope_output(&details);
        assert_eq!(output["rules"][0], "Only example.test is authorized");
        assert_eq!(output["top_level_surfaces"].as_array().unwrap().len(), 1);
        assert_eq!(output["top_level_surfaces"][0]["id"], "surface-root");
        assert_eq!(output["top_level_surfaces"][0]["scope"], "in_scope");
        assert!(output.get("guidance").is_none());
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
        let mut unknown_role = model;
        unknown_role.node_id = "unknown".to_owned();
        assert_eq!(
            validate_graph_selection(&db, "simple", &[unknown_role], &[], "", &workspace),
            Err(StatusCode::BAD_REQUEST)
        );
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
        };
        let decoded = SubmitSessionTurnRequest::decode(request.encode_to_vec().as_slice()).unwrap();
        assert_eq!(decoded.graph_id, "simple");
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
                },
                ReasoningSummaryRow {
                    turn_number: 1,
                    ordinal: 1,
                    after_tool_count: 1,
                    text: "The check is complete.".to_owned(),
                },
            ],
            &[CommentaryRow {
                turn_number: 1,
                ordinal: 0,
                after_tool_count: 0,
                text: "Reading the example file.".to_owned(),
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
        };
        let request = Request::builder()
            .method("POST")
            .uri(SESSION_TURNS_PATH)
            .header("authorization", "Bearer example-token")
            .header("content-type", beale_contracts::PROTOBUF_CONTENT_TYPE)
            .body(Body::from(request.encode_to_vec()))
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
