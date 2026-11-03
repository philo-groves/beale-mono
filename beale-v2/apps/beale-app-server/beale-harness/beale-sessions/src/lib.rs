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
use beale_storage::{HostDatabase, SessionRow, TurnRow};
use std::{
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
    Ok(response(detail(&session, &turns)?))
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
    let latest = turns.last().ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    state
        .db
        .append_turn(
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
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
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
        let transcript = detail(&loaded, &turns).unwrap();
        assert_eq!(loaded.title, "Echo example");
        assert_eq!(transcript.messages.len(), 2);
        assert_eq!(transcript.messages[0].text, "Echo example");
        assert_eq!(transcript.messages[1].role, SessionRole::Assistant as i32);
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
}
