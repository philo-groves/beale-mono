use axum::{
    Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path as RoutePath, Query, State},
    http::{HeaderMap, StatusCode, header},
    routing::{get, post},
};
use beale_contracts::{
    ARCHIVE_PATH, PROTOBUF_STREAM_CONTENT_TYPE, QUICK_CHAT_DETAIL_PATH, QUICK_CHAT_TURNS_PATH,
    SESSION_ARCHIVE_PATH, SESSION_DETAIL_PATH, SESSION_RESTORE_PATH, SESSION_TOOL_DETAIL_PATH,
    SESSION_TURN_STREAM_PATH, SESSION_TURNS_PATH, SESSIONS_PATH, session_stream_frame,
    v1::{
        ArchiveEntry, AuthMethod, ListArchiveResponse, ListSessionsResponse, ProviderId,
        ReasoningEffort, SessionCommentary, SessionDetail, SessionMessage, SessionReasoningSummary,
        SessionRole, SessionStreamEvent, SessionStreamKind, SessionSummary, SessionToolDetailPage,
        SessionToolSummary, SubmitSessionTurnRequest,
    },
};
use beale_providers::{
    AuthMode, CredentialStore, Provider, ProviderClient, ProviderProgress, TextMessage, TextRole,
    ToolStepRequest, input_messages, tool_output,
};
use beale_safety::ReviewRequest;
use beale_storage::{
    CommentaryRow, HostDatabase, ReasoningSummaryRow, SessionRow, ToolCallRow, TurnRow,
};
use beale_tools::{definitions, execute, model_preview, page, summary as tool_summary};
use futures_util::stream;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Mutex, mpsc};
use uuid::Uuid;

use beale_workspaces::WorkspaceStore;
mod http;
mod quick_chat;
mod system_prompt;
use http::{authorized, checked_body, response};

const MAX_TURNS: usize = 32;
const MAX_HISTORY_BYTES: usize = 4_194_304;
const MAX_TOOL_ROUNDS: usize = 8;
const MAX_TOOL_CALLS: usize = 32;
const MAX_REASONING_BYTES_PER_TURN: usize = 4_096;
const MAX_COMMENTARY_BYTES_PER_TURN: usize = 8_192;
const TOOL_PAGE_BYTES: usize = 8_000;
const INTERRUPTED_ANSWER: &str =
    "The provider could not complete this turn. Tool activity was saved.";

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
    credentials: Arc<CredentialStore>,
    provider: Arc<ProviderClient>,
    write_lock: Arc<Mutex<()>>,
}

struct StoredSession {
    id: String,
    workspace_id: String,
    title: String,
    created_at_ms: u128,
}

struct StoredTurn {
    provider: i32,
    method: i32,
    model: String,
    prompt: String,
    answer: String,
    duration_ms: u64,
}

pub fn router(token: String, workspaces: WorkspaceStore) -> anyhow::Result<Router> {
    let db = HostDatabase::open(workspaces.root())?;
    system_prompt::ensure_file(workspaces.root())?;
    let credentials = Arc::new(CredentialStore::at(workspaces.root().to_path_buf()));
    Ok(Router::new()
        .route(SESSIONS_PATH, get(list))
        .route(SESSION_DETAIL_PATH, get(details))
        .route(SESSION_ARCHIVE_PATH, post(archive))
        .route(SESSION_RESTORE_PATH, post(restore))
        .route(ARCHIVE_PATH, get(list_archive))
        .route(SESSION_TOOL_DETAIL_PATH, get(tool_details))
        .route(SESSION_TURNS_PATH, post(submit_turn))
        .route(SESSION_TURN_STREAM_PATH, post(stream_turn))
        .route(QUICK_CHAT_TURNS_PATH, post(quick_chat::turn))
        .route(QUICK_CHAT_DETAIL_PATH, get(quick_chat::details))
        .layer(DefaultBodyLimit::max(20_000))
        .with_state(SessionState {
            token,
            db,
            workspaces,
            credentials,
            provider: Arc::new(ProviderClient::new()?),
            write_lock: Arc::new(Mutex::new(())),
        }))
}

fn parse_id(value: &str) -> Result<Uuid, StatusCode> {
    let id = Uuid::parse_str(value).map_err(|_| StatusCode::BAD_REQUEST)?;
    if id.to_string() != value {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(id)
}

fn load_persisted(
    db: &HostDatabase,
    id: Uuid,
) -> Result<(StoredSession, Vec<StoredTurn>), StatusCode> {
    let item = db
        .session(&id.to_string())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    if item.turns.is_empty() || item.turns.len() > MAX_TURNS || item.session.created_at_ms < 0 {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let turns = item
        .turns
        .into_iter()
        .enumerate()
        .map(|(index, row)| {
            if row.duration_ms < 0
                || row.number
                    != i64::try_from(index + 1).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            {
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
            Ok(StoredTurn {
                provider: row.provider,
                method: row.method,
                model: row.model,
                prompt: row.prompt,
                answer: row.answer,
                duration_ms: row.duration_ms as u64,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((
        StoredSession {
            id: item.session.id,
            workspace_id: item.session.workspace_id,
            title: item.session.title,
            created_at_ms: item.session.created_at_ms as u128,
        },
        turns,
    ))
}

fn summary(session: &StoredSession) -> SessionSummary {
    SessionSummary {
        id: session.id.clone(),
        workspace_id: session.workspace_id.clone(),
        title: session.title.clone(),
    }
}

fn detail(
    db: &HostDatabase,
    session: &StoredSession,
    turns: &[StoredTurn],
) -> Result<SessionDetail, StatusCode> {
    let tools = db
        .tool_summaries(&session.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let reasoning = db
        .reasoning_summaries(&session.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let commentary = db
        .commentary(&session.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if tools.len() > MAX_TURNS * MAX_TOOL_CALLS {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let mut messages = Vec::with_capacity(turns.len() * 2);
    let mut total = 0_usize;
    for turn in turns {
        total = total.saturating_add(turn.prompt.len() + turn.answer.len());
        if total > MAX_HISTORY_BYTES {
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        messages.push(SessionMessage {
            role: SessionRole::User as i32,
            text: turn.prompt.clone(),
            tools: Vec::new(),
            reasoning_summaries: Vec::new(),
            duration_ms: 0,
            commentary: Vec::new(),
        });
        let turn_number = (messages.len() / 2 + 1) as i64;
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
    Ok(SessionDetail {
        summary: Some(summary(session)),
        messages,
        provider: latest.provider,
        method: latest.method,
        model: latest.model.clone(),
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
    let _guard = state.write_lock.lock().await;
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
    let _guard = state.write_lock.lock().await;
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
        if row.turn_count <= 0 || row.turn_count > MAX_TURNS as i64 {
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        entries.push(ArchiveEntry {
            id: row.id,
            workspace_id: row.workspace_id,
            title: row.title,
            model: row.model,
            turn_count: row.turn_count as u32,
            created_at_ms: u64::try_from(row.created_at_ms)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
            archived_at_ms: u64::try_from(row.archived_at_ms)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
            quick_chat: row.quick_chat,
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
    let _guard = state.write_lock.lock().await;
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
    let _guard = state.write_lock.lock().await;
    let (session, turns) = load_persisted(&state.db, id)?;
    if !state.workspaces.contains(&session.workspace_id)? {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(response(detail(&state.db, &session, &turns)?))
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
    let _guard = state.write_lock.lock().await;
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
                kind: SessionStreamKind::Completed as i32,
                text: String::new(),
                tool: None,
                detail: Some(detail),
                segment_index: 0,
            },
            Err(status) => SessionStreamEvent {
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
                kind: SessionStreamKind::Failed as i32,
                text: "Session response is too large.".to_owned(),
                tool: None,
                detail: None,
                segment_index: 0,
            }
        };
        let _ = tx.send(event).await;
    });
    let frames = stream::unfold(rx, |mut rx| async move {
        while let Some(event) = rx.recv().await {
            if let Some(frame) = session_stream_frame(&event) {
                return Some((Ok::<Bytes, std::io::Error>(Bytes::from(frame)), rx));
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

async fn run_turn(
    state: SessionState,
    request: SubmitSessionTurnRequest,
    progress: Option<mpsc::Sender<SessionStreamEvent>>,
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
    let _guard = state.write_lock.lock().await;
    let (session, mut turns) = if request.session_id.is_empty() {
        let id = Uuid::new_v4();
        (
            StoredSession {
                id: id.to_string(),
                workspace_id: request.workspace_id.clone(),
                title: title(&request.prompt),
                created_at_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                    .as_millis(),
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
    if turns.len() >= MAX_TURNS {
        return Err(StatusCode::CONFLICT);
    }
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
    let workspace_context = format!(
        "Guidance: {}\nRules: {}\nSurfaces: {}",
        workspace_details.guidance,
        workspace_details.rules.join("; "),
        workspace_details
            .surfaces
            .iter()
            .map(|surface| format!(
                "{}: {} ({})",
                if surface.scope == 1 {
                    "in scope"
                } else {
                    "out of scope"
                },
                surface.value,
                surface.out_of_scope_reason
            ))
            .collect::<Vec<_>>()
            .join("; "),
    );
    let mut input = input_messages(&messages);
    let system = system_prompt::load(state.workspaces.root(), &[]).map_err(|error| {
        eprintln!("session system prompt failed: {error:#}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    input.insert(0, json!({"role":"system","content":system}));
    let definitions = definitions();
    let mut tool_rows = Vec::<ToolCallRow>::new();
    let mut reasoning_rows = Vec::<ReasoningSummaryRow>::new();
    let mut commentary_rows = Vec::<CommentaryRow>::new();
    let mut reasoning_bytes = 0_usize;
    let mut commentary_bytes = 0_usize;
    let mut call_count = 0_usize;
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
        let step = state
            .provider
            .tool_step(
                provider,
                mode,
                ToolStepRequest {
                    model: &request.model,
                    input: &input,
                    tools: &definitions,
                    reasoning_effort,
                    workspace_root: Some(&workspace),
                },
                &state.credentials,
                provider_tx.as_ref(),
            )
            .await;
        drop(provider_tx);
        if let Some(forward) = forward {
            let _ = forward.await;
        }
        let step = match step {
            Ok(step) => step,
            Err(error) => {
                eprintln!("session provider turn failed: {error:#}");
                if tool_rows.is_empty() {
                    return Err(StatusCode::BAD_GATEWAY);
                }
                answer = Some(INTERRUPTED_ANSWER.to_owned());
                break;
            }
        };
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
                turn_number: (turns.len() + 1) as i64,
                ordinal: reasoning_rows.len() as i64,
                after_tool_count: tool_rows.len() as i64,
                text: summary,
            });
        }
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
                    turn_number: (turns.len() + 1) as i64,
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
        input.extend(step.continuation);
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
            let summary = tool_summary(&call.name, args.as_ref().unwrap_or(&Value::Null));
            if call.name != "tool_result_page"
                && let Some(tx) = &progress
            {
                let _ = tx
                    .send(SessionStreamEvent {
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
            let (output, is_error) = match args {
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
                    let decision =
                        if matches!(call.name.as_str(), "read" | "write" | "edit" | "bash") {
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
                    turn_number: (turns.len() + 1) as i64,
                    ordinal: tool_rows.len() as i64,
                    name: call.name.clone(),
                    summary,
                    arguments: call.arguments,
                    output: output.clone(),
                    is_error,
                });
                model_preview(&output, &id)
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
            return Err(StatusCode::BAD_GATEWAY);
        }
        answer = INTERRUPTED_ANSWER.to_owned();
    }
    turns.push(StoredTurn {
        provider: request.provider,
        method: request.method,
        model: request.model,
        prompt: request.prompt,
        answer,
        duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
    });
    let latest = turns.last().ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    state
        .db
        .append_turn_with_full_activity(
            &SessionRow {
                id: session.id.clone(),
                workspace_id: session.workspace_id.clone(),
                title: session.title.clone(),
                created_at_ms: i64::try_from(session.created_at_ms)
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
            },
            &TurnRow {
                number: i64::try_from(turns.len())
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
                provider: latest.provider,
                method: latest.method,
                model: latest.model.clone(),
                prompt: latest.prompt.clone(),
                answer: latest.answer.clone(),
                duration_ms: i64::try_from(latest.duration_ms)
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
            },
            turns.len() == 1,
            &tool_rows,
            &reasoning_rows,
            &commentary_rows,
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
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
        };
        let decoded = SubmitSessionTurnRequest::decode(request.encode_to_vec().as_slice()).unwrap();
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
    fn completed_turn_round_trips_as_a_transcript() {
        let root = std::env::temp_dir().join(format!("beale-session-example-{}", Uuid::new_v4()));
        let id = Uuid::new_v4();
        let session = SessionRow {
            id: id.to_string(),
            workspace_id: Uuid::new_v4().to_string(),
            title: title("Echo example\nmore details"),
            created_at_ms: 1,
        };
        let turn = TurnRow {
            number: 1,
            provider: ProviderId::Openai as i32,
            method: AuthMethod::Subscription as i32,
            model: "model-example".to_owned(),
            prompt: "Echo example".to_owned(),
            answer: "Echo example".to_owned(),
            duration_ms: 1_234,
        };
        let db = HostDatabase::open(&root).unwrap();
        db.append_turn(&session, &turn, true).unwrap();
        let (loaded, turns) = load_persisted(&db, id).unwrap();
        let transcript = detail(&db, &loaded, &turns).unwrap();
        assert_eq!(loaded.title, "Echo example");
        assert_eq!(transcript.messages.len(), 2);
        assert_eq!(transcript.messages[0].text, "Echo example");
        assert_eq!(transcript.messages[1].role, SessionRole::Assistant as i32);
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
        };
        let turn = TurnRow {
            number: 1,
            provider: ProviderId::Openai as i32,
            method: AuthMethod::ApiKey as i32,
            model: "model-example".to_owned(),
            prompt: "Read example".to_owned(),
            answer: "Done".to_owned(),
            duration_ms: 2_345,
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
            },
            &TurnRow {
                number: 1,
                provider: ProviderId::Openai as i32,
                method: AuthMethod::ApiKey as i32,
                model: "model-example".to_owned(),
                prompt: "Example question".to_owned(),
                answer: "Example answer".to_owned(),
                duration_ms: 1,
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
        assert_eq!(
            ListSessionsResponse::decode(body).unwrap().sessions.len(),
            1
        );
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
            },
            &TurnRow {
                number: 1,
                provider: ProviderId::Openai as i32,
                method: AuthMethod::ApiKey as i32,
                model: "model-example".to_owned(),
                prompt: "Example question".to_owned(),
                answer: "Example answer".to_owned(),
                duration_ms: 1,
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
        };
        let turn = TurnRow {
            number: 1,
            provider: ProviderId::Openai as i32,
            method: AuthMethod::ApiKey as i32,
            model: "model-example".to_owned(),
            prompt: "Read".to_owned(),
            answer: "Done".to_owned(),
            duration_ms: 3_456,
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
