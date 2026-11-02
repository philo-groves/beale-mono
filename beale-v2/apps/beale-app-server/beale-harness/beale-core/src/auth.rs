use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode, header},
    routing::{get, post},
};
use beale_contracts::{
    AUTH_API_KEY_PATH, AUTH_PROVE_PATH, AUTH_STATUS_PATH, AUTH_SUBSCRIPTION_FORGET_PATH,
    AUTH_SUBSCRIPTION_START_PATH, CONTROL_CHALLENGE_BYTES, LEAD_PROVIDER_PATH,
    MODELS_DEFAULTS_PATH, MODELS_LIST_PATH, PROTOBUF_CONTENT_TYPE, control_proof,
    v1::{
        AuthMethod, ControlChallengeRequest, ControlChallengeResponse, ListModelsRequest,
        ModelCatalogResponse, ModelEntry, ProviderAuthStatus, ProviderAuthStatusResponse,
        ProviderId, ProviderRequest, SetApiKeyRequest, SetModelDefaultsRequest,
        StartSubscriptionResponse,
    },
};
use beale_providers::{
    AuthMode, CredentialStore, ModelCatalog, ModelDefaults, Provider, ProviderClient,
    login_openai_with_prompt, login_xai_with_prompt, mode_name, provider_name,
};
use prost::Message;
use std::{sync::Arc, time::Duration};
use tokio::sync::{Mutex, oneshot};

#[derive(Clone)]
struct AuthState {
    token: String,
    store: Arc<CredentialStore>,
    login: Arc<Mutex<[LoginState; 2]>>,
    lead_lock: Arc<Mutex<()>>,
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
        .route(LEAD_PROVIDER_PATH, post(set_lead_provider))
        .route(MODELS_LIST_PATH, post(list_models))
        .route(MODELS_DEFAULTS_PATH, post(set_model_defaults))
        .layer(DefaultBodyLimit::max(8448))
        .with_state(AuthState {
            token,
            store: Arc::new(store),
            login: Arc::new(Mutex::new(Default::default())),
            lead_lock: Arc::new(Mutex::new(())),
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

fn model_provider(value: i32) -> Result<Provider, StatusCode> {
    match ProviderId::try_from(value) {
        Ok(ProviderId::Openai) => Ok(Provider::OpenAi),
        Ok(ProviderId::Xai) => Ok(Provider::Xai),
        _ => Err(StatusCode::BAD_REQUEST),
    }
}

fn model_method(value: i32) -> Result<AuthMode, StatusCode> {
    match AuthMethod::try_from(value) {
        Ok(AuthMethod::Subscription) => Ok(AuthMode::Subscription),
        Ok(AuthMethod::ApiKey) => Ok(AuthMode::ApiKey),
        _ => Err(StatusCode::BAD_REQUEST),
    }
}

fn provider_configured(store: &CredentialStore, name: &str) -> Result<bool, StatusCode> {
    Ok(store
        .load(name)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .is_some()
        || store
            .api_key_configured(name)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?)
}

fn reconcile_lead(store: &CredentialStore) -> Result<ProviderId, StatusCode> {
    let saved = store
        .load_lead_provider()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if let Some(name) = saved.as_deref()
        && provider_configured(store, name)?
    {
        return Ok(if name == "openai" {
            ProviderId::Openai
        } else {
            ProviderId::Xai
        });
    }
    for (name, id) in [("openai", ProviderId::Openai), ("xai", ProviderId::Xai)] {
        if provider_configured(store, name)? {
            store
                .save_lead_provider(name)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            return Ok(id);
        }
    }
    if saved.is_some() {
        store
            .clear_lead_provider()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }
    Ok(ProviderId::Unspecified)
}

fn initialize_lead(store: &CredentialStore, provider: &str) -> Result<(), StatusCode> {
    if store
        .load_lead_provider()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .is_none()
    {
        store
            .save_lead_provider(provider)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }
    Ok(())
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
    let _lead_guard = state.lead_lock.lock().await;
    let lead_provider = reconcile_lead(store)?;
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
    Ok(response(ProviderAuthStatusResponse {
        providers,
        lead_provider: lead_provider as i32,
    }))
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
    let _lead_guard = state.lead_lock.lock().await;
    store
        .save_api_key(provider, &request.api_key)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    initialize_lead(store, provider)?;
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
    let _lead_guard = state.lead_lock.lock().await;
    state
        .store
        .remove_api_key(provider)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    reconcile_lead(&state.store)?;
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
    let _lead_guard = state.lead_lock.lock().await;
    state
        .store
        .remove_oauth(provider)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    reconcile_lead(&state.store)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn set_lead_provider(
    State(state): State<AuthState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let request: ProviderRequest = checked_body(&headers, &body)?;
    let (provider, _) = checked_provider(request.provider)?;
    let _lead_guard = state.lead_lock.lock().await;
    if !provider_configured(&state.store, provider)? {
        return Err(StatusCode::CONFLICT);
    }
    state
        .store
        .save_lead_provider(provider)
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
    let lead_lock = state.lead_lock.clone();
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
        let result = if result.is_ok() {
            let _lead_guard = lead_lock.lock().await;
            initialize_lead(&store, provider)
                .map_err(|_| anyhow::anyhow!("cannot initialize lead provider"))
        } else {
            result
        };
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

async fn list_models(
    State(state): State<AuthState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let request: ListModelsRequest = checked_body(&headers, &body)?;
    let provider = model_provider(request.provider)?;
    let mode = model_method(request.method)?;
    let client = ProviderClient::new().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let catalog = client
        .list_models(provider, mode, &state.store)
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    Ok(response(ModelCatalogResponse {
        provider: request.provider,
        method: request.method,
        models: catalog
            .models
            .into_iter()
            .map(|model| ModelEntry {
                id: model.id,
                display_name: model.name,
            })
            .collect(),
        lead_model: catalog.lead_model,
        small_model: catalog.small_model,
    }))
}

fn valid_selection(catalog: &ModelCatalog, lead: &str, small: &str) -> bool {
    catalog.models.iter().any(|model| model.id == lead)
        && catalog.models.iter().any(|model| model.id == small)
}

async fn set_model_defaults(
    State(state): State<AuthState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let request: SetModelDefaultsRequest = checked_body(&headers, &body)?;
    let provider = model_provider(request.provider)?;
    let mode = model_method(request.method)?;
    let client = ProviderClient::new().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let catalog = client
        .list_models(provider, mode, &state.store)
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    if !valid_selection(&catalog, &request.lead_model, &request.small_model) {
        return Err(StatusCode::BAD_REQUEST);
    }
    state
        .store
        .save_model_defaults(
            provider_name(provider),
            mode_name(mode),
            &ModelDefaults {
                lead: request.lead_model,
                small: request.small_model,
            },
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
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
        assert!(model_method(AuthMethod::ApiKey as i32).is_ok());
        assert!(model_method(0).is_err());
    }

    #[test]
    fn defaults_must_be_in_the_discovered_catalog() {
        let catalog = ModelCatalog {
            models: vec![beale_providers::ModelEntry {
                id: "example-lead".to_owned(),
                name: "Example Lead".to_owned(),
            }],
            lead_model: "example-lead".to_owned(),
            small_model: "example-lead".to_owned(),
        };
        assert!(valid_selection(&catalog, "example-lead", "example-lead"));
        assert!(!valid_selection(&catalog, "example-other", "example-lead"));
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
        assert_eq!(status.lead_provider, ProviderId::Openai as i32);
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

    #[test]
    fn first_added_provider_remains_lead_until_removed() {
        let root =
            std::env::temp_dir().join(format!("beale-lead-example-{}", uuid::Uuid::new_v4()));
        let store = CredentialStore::at(root.clone());
        assert_eq!(reconcile_lead(&store).unwrap(), ProviderId::Unspecified);
        store.save_api_key("xai", "sk-example-xai").unwrap();
        initialize_lead(&store, "xai").unwrap();
        store.save_api_key("openai", "sk-example-openai").unwrap();
        initialize_lead(&store, "openai").unwrap();
        assert_eq!(reconcile_lead(&store).unwrap(), ProviderId::Xai);
        store.save_lead_provider("openai").unwrap();
        assert_eq!(reconcile_lead(&store).unwrap(), ProviderId::Openai);
        store.remove_api_key("openai").unwrap();
        assert_eq!(reconcile_lead(&store).unwrap(), ProviderId::Xai);
        store.remove_api_key("xai").unwrap();
        assert_eq!(reconcile_lead(&store).unwrap(), ProviderId::Unspecified);
        assert!(store.load_lead_provider().unwrap().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn lead_route_requires_auth_and_a_configured_provider() {
        let root = std::env::temp_dir().join(format!("beale-lead-route-{}", uuid::Uuid::new_v4()));
        let token = "ab".repeat(32);
        let app = router(token.clone(), CredentialStore::at(root.clone()));
        let body = ProviderRequest {
            provider: ProviderId::Xai as i32,
        }
        .encode_to_vec();
        let request = Request::builder()
            .method("POST")
            .uri(LEAD_PROVIDER_PATH)
            .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
            .body(Body::from(body.clone()))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let request = Request::builder()
            .method("POST")
            .uri(LEAD_PROVIDER_PATH)
            .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(body.clone()))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::CONFLICT
        );
        CredentialStore::at(root.clone())
            .save_api_key("xai", "sk-example-xai")
            .unwrap();
        let request = Request::builder()
            .method("POST")
            .uri(LEAD_PROVIDER_PATH)
            .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(body))
            .unwrap();
        assert_eq!(
            app.oneshot(request).await.unwrap().status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            CredentialStore::at(root.clone())
                .load_lead_provider()
                .unwrap()
                .as_deref(),
            Some("xai")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
