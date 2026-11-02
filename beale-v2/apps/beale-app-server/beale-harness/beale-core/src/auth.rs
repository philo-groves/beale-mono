use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode, header},
    routing::{get, post},
};
use beale_contracts::{
    AUTH_API_KEY_PATH, AUTH_PROVE_PATH, AUTH_STATUS_PATH, AUTH_SUBSCRIPTION_FORGET_PATH,
    AUTH_SUBSCRIPTION_START_PATH, CONTROL_CHALLENGE_BYTES, PROTOBUF_CONTENT_TYPE, control_proof,
    v1::{
        ControlChallengeRequest, ControlChallengeResponse, ProviderAuthStatus,
        ProviderAuthStatusResponse, ProviderId, ProviderRequest, SetApiKeyRequest,
        StartSubscriptionResponse,
    },
};
use beale_providers::{
    CredentialStore, ProviderClient, login_openai_with_prompt, login_xai_with_prompt,
};
use prost::Message;
use std::{sync::Arc, time::Duration};
use tokio::sync::{Mutex, oneshot};

#[derive(Clone)]
struct AuthState {
    token: String,
    store: Arc<CredentialStore>,
    login: Arc<Mutex<[LoginState; 2]>>,
}

#[derive(Clone, Default)]
struct LoginState {
    in_progress: bool,
    error: String,
}

pub(super) fn router(token: String, store: CredentialStore) -> Router {
    Router::new()
        .route(AUTH_PROVE_PATH, post(prove))
        .route(AUTH_STATUS_PATH, get(status))
        .route(AUTH_API_KEY_PATH, post(set_api_key).delete(remove_api_key))
        .route(AUTH_SUBSCRIPTION_START_PATH, post(start_subscription))
        .route(AUTH_SUBSCRIPTION_FORGET_PATH, post(forget_subscription))
        .layer(DefaultBodyLimit::max(8448))
        .with_state(AuthState {
            token,
            store: Arc::new(store),
            login: Arc::new(Mutex::new(Default::default())),
        })
}

fn authorized(headers: &HeaderMap, token: &str) -> bool {
    let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let Some(presented) = value.strip_prefix("Bearer ") else {
        return false;
    };
    let bytes = presented.as_bytes();
    if bytes.len() != token.len() {
        return false;
    }
    bytes
        .iter()
        .zip(token.as_bytes())
        .fold(0_u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}

fn checked_provider(value: i32) -> Result<(&'static str, usize), StatusCode> {
    match ProviderId::try_from(value) {
        Ok(ProviderId::Openai) => Ok(("openai", 0)),
        Ok(ProviderId::Xai) => Ok(("xai", 1)),
        _ => Err(StatusCode::BAD_REQUEST),
    }
}

fn checked_body<T: Message + Default>(headers: &HeaderMap, body: &[u8]) -> Result<T, StatusCode> {
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some(PROTOBUF_CONTENT_TYPE)
    {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    T::decode(body).map_err(|_| StatusCode::BAD_REQUEST)
}

fn response<T: Message>(message: T) -> ([(header::HeaderName, &'static str); 1], Vec<u8>) {
    (
        [(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)],
        message.encode_to_vec(),
    )
}

async fn prove(
    State(state): State<AuthState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    let request: ControlChallengeRequest = checked_body(&headers, &body)?;
    if request.nonce.len() != CONTROL_CHALLENGE_BYTES {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(response(ControlChallengeResponse {
        proof: control_proof(&state.token, &request.nonce).to_vec(),
    }))
}

async fn status(
    State(state): State<AuthState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let store = &state.store;
    let login = state.login.lock().await;
    let mut providers = Vec::new();
    for (id, name, index) in [
        (ProviderId::Openai, "openai", 0),
        (ProviderId::Xai, "xai", 1),
    ] {
        providers.push(ProviderAuthStatus {
            provider: id as i32,
            subscription_configured: store
                .load(name)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                .is_some(),
            api_key_configured: store
                .api_key_configured(name)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
            login_in_progress: login[index].in_progress,
            login_error: login[index].error.clone(),
        });
    }
    Ok(response(ProviderAuthStatusResponse { providers }))
}

async fn set_api_key(
    State(state): State<AuthState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let request: SetApiKeyRequest = checked_body(&headers, &body)?;
    let (provider, _) = checked_provider(request.provider)?;
    let store = &state.store;
    if request.api_key.trim().is_empty()
        || request.api_key.len() > 8192
        || request.api_key.chars().any(char::is_control)
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    store
        .save_api_key(provider, &request.api_key)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_api_key(
    State(state): State<AuthState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let request: ProviderRequest = checked_body(&headers, &body)?;
    let (provider, _) = checked_provider(request.provider)?;
    state
        .store
        .remove_api_key(provider)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn forget_subscription(
    State(state): State<AuthState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let request: ProviderRequest = checked_body(&headers, &body)?;
    let (provider, index) = checked_provider(request.provider)?;
    if state.login.lock().await[index].in_progress {
        return Err(StatusCode::CONFLICT);
    }
    state
        .store
        .remove_oauth(provider)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn start_subscription(
    State(state): State<AuthState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let request: ProviderRequest = checked_body(&headers, &body)?;
    let (provider, index) = checked_provider(request.provider)?;
    {
        let mut login = state.login.lock().await;
        if login[index].in_progress {
            return Err(StatusCode::CONFLICT);
        }
        login[index] = LoginState {
            in_progress: true,
            error: String::new(),
        };
    }
    let (prompt_tx, prompt_rx) = oneshot::channel::<(String, String)>();
    let login_state = state.login.clone();
    let store = state.store.clone();
    let task = tokio::spawn(async move {
        let result = async {
            let client = ProviderClient::new()?;
            let prompt = move |url, code| {
                let _ = prompt_tx.send((url, code));
            };
            if provider == "openai" {
                login_openai_with_prompt(client.http(), &store, prompt).await
            } else {
                login_xai_with_prompt(client.http(), &store, prompt).await
            }
        }
        .await;
        let mut login = login_state.lock().await;
        login[index].in_progress = false;
        if result.is_err() {
            login[index].error = "Sign-in failed or expired. Try again.".to_owned();
        }
    });
    match tokio::time::timeout(Duration::from_secs(30), prompt_rx).await {
        Ok(Ok((verification_url, user_code))) => Ok(response(StartSubscriptionResponse {
            verification_url,
            user_code,
        })),
        _ => {
            task.abort();
            state.login.lock().await[index] = LoginState {
                in_progress: false,
                error: "Could not start sign-in. Try again.".to_owned(),
            };
            Err(StatusCode::BAD_GATEWAY)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;

    #[test]
    fn control_token_is_required_and_provider_is_bounded() {
        let token = "ab".repeat(32);
        let mut headers = HeaderMap::new();
        assert!(!authorized(&headers, &token));
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        assert!(authorized(&headers, &token));
        assert!(checked_provider(ProviderId::Openai as i32).is_ok());
        assert!(checked_provider(0).is_err());
    }

    #[tokio::test]
    async fn local_host_proves_control_token_without_receiving_it() {
        let token = "ab".repeat(32);
        let app = router(token.clone(), CredentialStore::at(std::env::temp_dir()));
        let nonce = vec![7_u8; CONTROL_CHALLENGE_BYTES];
        let request = Request::builder()
            .method("POST")
            .uri(AUTH_PROVE_PATH)
            .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
            .body(Body::from(
                ControlChallengeRequest {
                    nonce: nonce.clone(),
                }
                .encode_to_vec(),
            ))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 128).await.unwrap();
        let proof = ControlChallengeResponse::decode(bytes).unwrap();
        assert_eq!(proof.proof, control_proof(&token, &nonce));
    }

    #[tokio::test]
    async fn api_key_routes_require_token_and_keep_key_out_of_status() {
        let root =
            std::env::temp_dir().join(format!("beale-auth-example-{}", uuid::Uuid::new_v4()));
        let store = CredentialStore::at(root.clone());
        let token = "ab".repeat(32);
        let app = router(token.clone(), CredentialStore::at(root.clone()));
        let body = SetApiKeyRequest {
            provider: ProviderId::Openai as i32,
            api_key: "sk-example-synthetic".to_owned(),
        }
        .encode_to_vec();
        let request = Request::builder()
            .method("POST")
            .uri(AUTH_API_KEY_PATH)
            .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
            .body(Body::from(body.clone()))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let request = Request::builder()
            .method("POST")
            .uri(AUTH_API_KEY_PATH)
            .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(body))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            store.load_api_key("openai").unwrap().as_deref(),
            Some("sk-example-synthetic")
        );
        #[cfg(windows)]
        assert!(
            !std::fs::read(root.join("openai-api-key"))
                .unwrap()
                .windows(20)
                .any(|part| part == b"sk-example-synthetic")
        );
        let request = Request::builder()
            .uri(AUTH_STATUS_PATH)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        let status = ProviderAuthStatusResponse::decode(bytes.as_ref()).unwrap();
        assert!(status.providers[0].api_key_configured);
        assert!(!status.providers[0].subscription_configured);
        assert!(
            !bytes
                .windows(20)
                .any(|part| part == b"sk-example-synthetic")
        );
        let request = Request::builder()
            .method("POST")
            .uri(AUTH_API_KEY_PATH)
            .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(
                SetApiKeyRequest {
                    provider: ProviderId::Openai as i32,
                    api_key: "sk-example-replaced".to_owned(),
                }
                .encode_to_vec(),
            ))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            store.load_api_key("openai").unwrap().as_deref(),
            Some("sk-example-replaced")
        );
        let request = Request::builder()
            .method("DELETE")
            .uri(AUTH_API_KEY_PATH)
            .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(
                ProviderRequest {
                    provider: ProviderId::Openai as i32,
                }
                .encode_to_vec(),
            ))
            .unwrap();
        assert_eq!(
            app.oneshot(request).await.unwrap().status(),
            StatusCode::NO_CONTENT
        );
        assert!(!store.api_key_configured("openai").unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }
}
