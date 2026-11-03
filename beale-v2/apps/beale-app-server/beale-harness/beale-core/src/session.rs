use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path as RoutePath, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use beale_contracts::{
    SESSION_DETAIL_PATH, SESSION_TURNS_PATH, SESSIONS_PATH,
    v1::{
        AuthMethod, ListSessionsResponse, ProviderId, SessionDetail, SessionMessage, SessionRole,
        SessionSummary, SubmitSessionTurnRequest,
    },
};
use beale_providers::{
    AuthMode, CredentialStore, InferenceRequest, Provider, ProviderClient, TextMessage, TextRole,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    auth::{authorized, checked_body, response},
    workspace::WorkspaceStore,
};

const MAX_TURNS: usize = 32;
const MAX_HISTORY_BYTES: usize = 4_194_304;

#[derive(Clone)]
struct SessionState {
    token: String,
    root: PathBuf,
    workspaces: WorkspaceStore,
    credentials: Arc<CredentialStore>,
    provider: Arc<ProviderClient>,
    write_lock: Arc<Mutex<()>>,
}

#[derive(Serialize, Deserialize)]
struct StoredSession {
    schema_version: u32,
    id: String,
    workspace_id: String,
    title: String,
    created_at_ms: u128,
}

#[derive(Serialize, Deserialize)]
struct StoredTurn {
    provider: i32,
    method: i32,
    model: String,
    prompt: String,
    answer: String,
}

pub(super) fn router(token: String, workspaces: WorkspaceStore) -> anyhow::Result<Router> {
    let root = workspaces.root().join("sessions");
    let credentials = Arc::new(CredentialStore::at(workspaces.root().to_path_buf()));
    Ok(Router::new()
        .route(SESSIONS_PATH, get(list))
        .route(SESSION_DETAIL_PATH, get(details))
        .route(SESSION_TURNS_PATH, post(submit_turn))
        .layer(DefaultBodyLimit::max(20_000))
        .with_state(SessionState {
            token,
            root,
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

fn session_dir(root: &Path, id: Uuid) -> PathBuf {
    root.join(id.to_string())
}

fn read_bounded(path: &Path, max: usize) -> Result<Vec<u8>, StatusCode> {
    let bytes = fs::read(path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if bytes.len() > max {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    Ok(bytes)
}

fn load_session(root: &Path, id: Uuid) -> Result<StoredSession, StatusCode> {
    let path = session_dir(root, id).join("session.json");
    if !path.exists() {
        return Err(StatusCode::NOT_FOUND);
    }
    let session: StoredSession = serde_json::from_slice(&read_bounded(&path, 4_096)?)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if session.schema_version != 1 || session.id != id.to_string() {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    Ok(session)
}

fn load_turns(root: &Path, id: Uuid) -> Result<Vec<StoredTurn>, StatusCode> {
    let directory = session_dir(root, id);
    let mut turns = Vec::new();
    for number in 1..=MAX_TURNS {
        let path = directory.join(format!("turn-{number:04}.json"));
        if !path.exists() {
            break;
        }
        let turn: StoredTurn = serde_json::from_slice(&read_bounded(&path, MAX_HISTORY_BYTES)?)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        turns.push(turn);
    }
    Ok(turns)
}

fn summary(session: &StoredSession) -> SessionSummary {
    SessionSummary {
        id: session.id.clone(),
        workspace_id: session.workspace_id.clone(),
        title: session.title.clone(),
    }
}

fn detail(session: &StoredSession, turns: &[StoredTurn]) -> Result<SessionDetail, StatusCode> {
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
        });
        messages.push(SessionMessage {
            role: SessionRole::Assistant as i32,
            text: turn.answer.clone(),
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

fn write_new(path: &Path, value: &impl Serialize) -> Result<(), StatusCode> {
    let bytes = serde_json::to_vec(value).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
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
    let mut sessions = Vec::new();
    let entries = match fs::read_dir(&state.root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(response(ListSessionsResponse {
                sessions: Vec::new(),
            }));
        }
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };
    for entry in entries {
        let entry = entry.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok(id) = parse_id(&name) else {
            continue;
        };
        if let Ok(session) = load_session(&state.root, id)
            && state.workspaces.contains(&session.workspace_id)?
        {
            sessions.push((session.created_at_ms, summary(&session)));
        }
        if sessions.len() > 1_024 {
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }
    sessions.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    Ok(response(ListSessionsResponse {
        sessions: sessions.into_iter().map(|(_, summary)| summary).collect(),
    }))
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
    let session = load_session(&state.root, id)?;
    if !state.workspaces.contains(&session.workspace_id)? {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(response(detail(&session, &load_turns(&state.root, id)?)?))
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
    let (id, session, mut turns) = if request.session_id.is_empty() {
        let id = Uuid::new_v4();
        (
            id,
            StoredSession {
                schema_version: 1,
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
        let session = load_session(&state.root, id)?;
        if session.workspace_id != request.workspace_id {
            return Err(StatusCode::CONFLICT);
        }
        let turns = load_turns(&state.root, id)?;
        (id, session, turns)
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
    if history_bytes > MAX_HISTORY_BYTES {
        return Err(StatusCode::CONFLICT);
    }
    let answer = state
        .provider
        .infer(
            provider,
            mode,
            &InferenceRequest {
                model: request.model.clone(),
                messages,
            },
            &state.credentials,
        )
        .await
        .map_err(|error| {
            eprintln!("session provider turn failed: {error:#}");
            StatusCode::BAD_GATEWAY
        })?;
    if answer.trim().is_empty() || history_bytes.saturating_add(answer.len()) > MAX_HISTORY_BYTES {
        return Err(StatusCode::BAD_GATEWAY);
    }
    turns.push(StoredTurn {
        provider: request.provider,
        method: request.method,
        model: request.model,
        prompt: request.prompt,
        answer,
    });
    let directory = session_dir(&state.root, id);
    fs::create_dir_all(&directory).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    write_new(
        &directory.join(format!("turn-{:04}.json", turns.len())),
        turns.last().ok_or(StatusCode::INTERNAL_SERVER_ERROR)?,
    )?;
    if turns.len() == 1 {
        write_new(&directory.join("session.json"), &session)?;
    }
    Ok(response(detail(&session, &turns)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use prost::Message;
    use tower::ServiceExt;

    #[test]
    fn completed_turn_round_trips_as_a_transcript() {
        let root = std::env::temp_dir().join(format!("beale-session-example-{}", Uuid::new_v4()));
        let id = Uuid::new_v4();
        let directory = session_dir(&root, id);
        fs::create_dir_all(&directory).unwrap();
        let session = StoredSession {
            schema_version: 1,
            id: id.to_string(),
            workspace_id: Uuid::new_v4().to_string(),
            title: title("Echo example\nmore details"),
            created_at_ms: 1,
        };
        let turn = StoredTurn {
            provider: ProviderId::Openai as i32,
            method: AuthMethod::Subscription as i32,
            model: "model-example".to_owned(),
            prompt: "Echo example".to_owned(),
            answer: "Echo example".to_owned(),
        };
        write_new(&directory.join("session.json"), &session).unwrap();
        write_new(&directory.join("turn-0001.json"), &turn).unwrap();
        let loaded = load_session(&root, id).unwrap();
        let transcript = detail(&loaded, &load_turns(&root, id).unwrap()).unwrap();
        assert_eq!(loaded.title, "Echo example");
        assert_eq!(transcript.messages.len(), 2);
        assert_eq!(transcript.messages[0].text, "Echo example");
        assert_eq!(transcript.messages[1].role, SessionRole::Assistant as i32);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn routes_require_host_auth_and_a_registered_workspace() {
        let root = std::env::temp_dir().join(format!("beale-session-example-{}", Uuid::new_v4()));
        let app = router("example-token".to_owned(), WorkspaceStore::at(root.clone())).unwrap();
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
}
