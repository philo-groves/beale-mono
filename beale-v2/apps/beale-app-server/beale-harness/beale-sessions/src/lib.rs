use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path as RoutePath, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use beale_contracts::{
    SESSION_DETAIL_PATH, SESSION_TOOL_DETAIL_PATH, SESSION_TURNS_PATH, SESSIONS_PATH,
    v1::{
        AuthMethod, ListSessionsResponse, ProviderId, SessionDetail, SessionMessage, SessionRole,
        SessionSummary, SessionToolDetailPage, SessionToolSummary, SubmitSessionTurnRequest,
    },
};
use beale_providers::{
    AuthMode, CredentialStore, Provider, ProviderClient, TextMessage, TextRole, input_messages,
    tool_output,
};
use beale_storage::{HostDatabase, SessionRow, ToolCallRow, TurnRow};
use beale_tools::{definitions, execute, model_preview, page, summary as tool_summary};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;
use uuid::Uuid;

use beale_workspaces::WorkspaceStore;
mod http;
use http::{authorized, checked_body, response};

const MAX_TURNS: usize = 32;
const MAX_HISTORY_BYTES: usize = 4_194_304;
const MAX_TOOL_ROUNDS: usize = 8;
const MAX_TOOL_CALLS: usize = 32;
const TOOL_PAGE_BYTES: usize = 8_000;
const INTERRUPTED_ANSWER: &str =
    "The provider could not complete this turn. Tool activity was saved.";

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
}

pub fn router(token: String, workspaces: WorkspaceStore) -> anyhow::Result<Router> {
    let db = HostDatabase::open(workspaces.root())?;
    let credentials = Arc::new(CredentialStore::at(workspaces.root().to_path_buf()));
    Ok(Router::new()
        .route(SESSIONS_PATH, get(list))
        .route(SESSION_DETAIL_PATH, get(details))
        .route(SESSION_TOOL_DETAIL_PATH, get(tool_details))
        .route(SESSION_TURNS_PATH, post(submit_turn))
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
            if row.number
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
        });
        messages.push(SessionMessage {
            role: SessionRole::Assistant as i32,
            text: turn.answer.clone(),
            tools: tools
                .iter()
                .filter(|tool| tool.turn_number == (messages.len() / 2 + 1) as i64)
                .map(|tool| SessionToolSummary {
                    id: tool.id.clone(),
                    name: tool.name.clone(),
                    summary: tool.summary.clone(),
                    is_error: tool.is_error,
                })
                .collect(),
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
        .sessions()
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
    let _workspace_id = parse_id(&request.workspace_id)?;
    if !state.workspaces.contains(&request.workspace_id)? {
        return Err(StatusCode::NOT_FOUND);
    }
    if request.prompt.trim().is_empty()
        || request.prompt.len() > 16_384
        || request.model.is_empty()
        || request.model.len() > 128
        || !request
            .model
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let provider = match ProviderId::try_from(request.provider) {
        Ok(ProviderId::Openai) => Provider::OpenAi,
        Ok(ProviderId::Xai) => Provider::Xai,
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    let mode = match AuthMethod::try_from(request.method) {
        Ok(AuthMethod::Subscription) => AuthMode::Subscription,
        Ok(AuthMethod::ApiKey) => AuthMode::ApiKey,
        _ => return Err(StatusCode::BAD_REQUEST),
    };
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
    let mut input = input_messages(&messages);
    input.insert(0, json!({"role":"system","content":"You are Beale, working in a research workspace. Use relative paths for file tools; bash starts in the workspace. Keep answers grounded in tool results. Large tool results have a bounded preview; call tool_result_page with the supplied ID and offset to inspect omitted text."}));
    let definitions = definitions();
    let mut tool_rows = Vec::<ToolCallRow>::new();
    let mut call_count = 0_usize;
    let mut answer = None;
    'rounds: for round in 0..MAX_TOOL_ROUNDS {
        let step = state
            .provider
            .tool_step(
                provider,
                mode,
                &request.model,
                &input,
                &definitions,
                &state.credentials,
            )
            .await;
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
        if step.calls.is_empty() {
            answer = Some(step.text);
            break;
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
                    match execute(&call.name, &args, &workspace, state.workspaces.root()).await {
                        Ok(value) => (value.output, value.is_error),
                        Err(error) => (format!("Tool error: {error:#}"), true),
                    }
                }
                Err(_) => ("Tool error: invalid JSON arguments".to_owned(), true),
            };
            let model_output = if call.name == "tool_result_page" {
                output.clone()
            } else {
                tool_rows.push(ToolCallRow {
                    id: id.clone(),
                    turn_number: (turns.len() + 1) as i64,
                    ordinal: tool_rows.len() as i64,
                    name: call.name.clone(),
                    summary: tool_summary(
                        &call.name,
                        &serde_json::from_str(&call.arguments).unwrap_or(Value::Null),
                    ),
                    arguments: call.arguments,
                    output: output.clone(),
                    is_error,
                });
                model_preview(&output, &id)
            };
            input.push(tool_output(provider, mode, &call.id, &model_output));
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
    });
    let latest = turns.last().ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    state
        .db
        .append_turn_with_tools(
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
            },
            turns.len() == 1,
            &tool_rows,
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(response(detail(&state.db, &session, &turns)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::{body::Body, http::Request};
    use prost::Message;
    use tower::ServiceExt;

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
        db.append_turn_with_tools(&session, &turn, true, std::slice::from_ref(&tool))
            .unwrap();
        let (loaded, turns) = load_persisted(&db, Uuid::parse_str(&session.id).unwrap()).unwrap();
        let detail = detail(&db, &loaded, &turns).unwrap();
        let wire = detail.encode_to_vec();
        assert_eq!(detail.messages[1].tools[0].summary, "Read example.txt");
        assert!(!String::from_utf8_lossy(&wire).contains("private example output"));
        assert!(!String::from_utf8_lossy(&wire).contains("example.txt\""));
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
        let request = SubmitSessionTurnRequest {
            workspace_id: Uuid::new_v4().to_string(),
            session_id: String::new(),
            provider: ProviderId::Openai as i32,
            method: AuthMethod::Subscription as i32,
            model: "model-example".to_owned(),
            prompt: "Echo example".to_owned(),
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
