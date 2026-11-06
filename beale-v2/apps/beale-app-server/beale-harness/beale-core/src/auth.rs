use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode, header},
    routing::{get, post},
};
use beale_contracts::{
    AUTH_API_KEY_PATH, AUTH_PROVE_PATH, AUTH_STATUS_PATH, AUTH_SUBSCRIPTION_FORGET_PATH,
    AUTH_SUBSCRIPTION_START_PATH, CONTROL_CHALLENGE_BYTES, CUSTOM_PROVIDER_API_KEY_PATH,
    CUSTOM_PROVIDER_DISCOVER_PATH, CUSTOM_PROVIDERS_PATH, DAYBREAK_MODE_PATH, LEAD_PROVIDER_PATH,
    MODELS_DEFAULTS_PATH, MODELS_LIST_PATH, PROTOBUF_CONTENT_TYPE, PROVIDER_USAGE_PATH,
    SOURCE_PRIORITY_PATH, control_proof,
    v1::{
        AuthMethod, ControlChallengeRequest, ControlChallengeResponse,
        CustomProvider as WireCustomProvider, CustomProviderRequest, DaybreakMode,
        DiscoverCustomModelsRequest, DiscoverCustomModelsResponse, ListCustomProvidersResponse,
        ListModelsRequest, ModelCatalogResponse, ModelEntry, ProviderAuthStatus,
        ProviderAuthStatusResponse, ProviderId, ProviderRequest, ProviderUsageResponse,
        ProviderUsageSnapshot, ProviderUsageWindow, SaveCustomProviderRequest, SetApiKeyRequest,
        SetCustomProviderApiKeyRequest, SetDaybreakModeRequest, SetModelDefaultsRequest,
        SetSourcePriorityRequest, StartSubscriptionResponse,
    },
};
use beale_providers::usage;
use beale_providers::{
    AuthMode, CredentialStore, CustomProvider, ModelCatalog, ModelDefaults, Provider,
    ProviderClient, login_openai_with_prompt, login_xai_with_prompt, mode_name, provider_name,
};
use beale_storage::{HostDatabase, SubscriptionUsageRow};
use prost::Message;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, oneshot};

#[derive(Clone)]
struct AuthState {
    token: String,
    store: Arc<CredentialStore>,
    db: HostDatabase,
    login: Arc<Mutex<[LoginState; 5]>>,
    lead_lock: Arc<Mutex<()>>,
    usage_cache: Arc<Mutex<Option<(Instant, ProviderUsageResponse)>>>,
}

#[derive(Clone, Default)]
struct LoginState {
    in_progress: bool,
    error: String,
}

pub(super) fn router(token: String, store: CredentialStore, db: HostDatabase) -> Router {
    Router::new()
        .route(AUTH_PROVE_PATH, post(prove))
        .route(AUTH_STATUS_PATH, get(status))
        .route(PROVIDER_USAGE_PATH, get(provider_usage))
        .route(AUTH_API_KEY_PATH, post(set_api_key).delete(remove_api_key))
        .route(AUTH_SUBSCRIPTION_START_PATH, post(start_subscription))
        .route(AUTH_SUBSCRIPTION_FORGET_PATH, post(forget_subscription))
        .route(LEAD_PROVIDER_PATH, post(set_lead_provider))
        .route(MODELS_LIST_PATH, post(list_models))
        .route(MODELS_DEFAULTS_PATH, post(set_model_defaults))
        .route(SOURCE_PRIORITY_PATH, post(set_source_priority))
        .route(DAYBREAK_MODE_PATH, post(set_daybreak_mode))
        .route(
            CUSTOM_PROVIDERS_PATH,
            get(list_custom_providers)
                .post(save_custom_provider)
                .delete(remove_custom_provider),
        )
        .route(
            CUSTOM_PROVIDER_API_KEY_PATH,
            post(set_custom_api_key).delete(remove_custom_api_key),
        )
        .route(CUSTOM_PROVIDER_DISCOVER_PATH, post(discover_custom_models))
        .layer(DefaultBodyLimit::max(65_536))
        .with_state(AuthState {
            token,
            store: Arc::new(store),
            db,
            login: Arc::new(Mutex::new(Default::default())),
            lead_lock: Arc::new(Mutex::new(())),
            usage_cache: Arc::new(Mutex::new(None)),
        })
}

pub(super) fn authorized(headers: &HeaderMap, token: &str) -> bool {
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
        Ok(ProviderId::Anthropic) => Ok(("anthropic", 2)),
        Ok(ProviderId::Zai) => Ok(("zai", 3)),
        Ok(ProviderId::Openrouter) => Ok(("openrouter", 4)),
        _ => Err(StatusCode::BAD_REQUEST),
    }
}

fn model_provider(value: i32) -> Result<Provider, StatusCode> {
    match ProviderId::try_from(value) {
        Ok(ProviderId::Openai) => Ok(Provider::OpenAi),
        Ok(ProviderId::Xai) => Ok(Provider::Xai),
        Ok(ProviderId::Anthropic) => Ok(Provider::Anthropic),
        Ok(ProviderId::Zai) => Ok(Provider::Zai),
        Ok(ProviderId::Openrouter) => Ok(Provider::OpenRouter),
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
    if beale_providers::valid_custom_id(name) {
        return Ok(store
            .custom_provider(name)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .is_some());
    }
    let external = match name {
        "anthropic" => beale_providers::subscription::configured(Provider::Anthropic),
        "zai" => beale_providers::subscription::configured(Provider::Zai),
        _ => false,
    };
    Ok(external
        || (matches!(name, "openai" | "xai")
            && store
                .load(name)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                .is_some())
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
        return Ok(match name {
            "openai" => ProviderId::Openai,
            "xai" => ProviderId::Xai,
            "anthropic" => ProviderId::Anthropic,
            "zai" => ProviderId::Zai,
            "openrouter" => ProviderId::Openrouter,
            name if beale_providers::valid_custom_id(name) => ProviderId::Custom,
            _ => return Err(StatusCode::INTERNAL_SERVER_ERROR),
        });
    }
    for (name, id) in [
        ("openai", ProviderId::Openai),
        ("xai", ProviderId::Xai),
        ("anthropic", ProviderId::Anthropic),
        ("zai", ProviderId::Zai),
        ("openrouter", ProviderId::Openrouter),
    ] {
        if provider_configured(store, name)? {
            store
                .save_lead_provider(name)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            return Ok(id);
        }
    }
    for custom in store
        .custom_providers()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        if provider_configured(store, &custom.id)? {
            store
                .save_lead_provider(&custom.id)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            return Ok(ProviderId::Custom);
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

pub(super) fn checked_body<T: Message + Default>(
    headers: &HeaderMap,
    body: &[u8],
) -> Result<T, StatusCode> {
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some(PROTOBUF_CONTENT_TYPE)
    {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    T::decode(body).map_err(|_| StatusCode::BAD_REQUEST)
}

pub(super) fn response<T: Message>(
    message: T,
) -> ([(header::HeaderName, &'static str); 1], Vec<u8>) {
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
        (ProviderId::Anthropic, "anthropic", 2),
        (ProviderId::Zai, "zai", 3),
        (ProviderId::Openrouter, "openrouter", 4),
    ] {
        providers.push(ProviderAuthStatus {
            provider: id as i32,
            subscription_configured: if name == "anthropic" {
                beale_providers::subscription::configured(Provider::Anthropic)
            } else if name == "zai" {
                beale_providers::subscription::configured(Provider::Zai)
            } else if matches!(name, "openai" | "xai") {
                store
                    .load(name)
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                    .is_some()
            } else {
                false
            },
            api_key_configured: store
                .api_key_configured(name)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
            login_in_progress: login[index].in_progress,
            login_error: login[index].error.clone(),
            source_priority: if store
                .source_priority(name)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                == "api-key"
            {
                AuthMethod::ApiKey as i32
            } else {
                AuthMethod::Subscription as i32
            },
        });
    }
    Ok(response(ProviderAuthStatusResponse {
        providers,
        lead_provider: lead_provider as i32,
        openai_daybreak_mode: if store
            .daybreak_mode()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            == "red"
        {
            DaybreakMode::Red as i32
        } else {
            DaybreakMode::Blue as i32
        },
        lead_custom_provider_id: if lead_provider == ProviderId::Custom {
            store
                .load_lead_provider()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                .unwrap_or_default()
        } else {
            String::new()
        },
    }))
}

async fn provider_usage(
    State(state): State<AuthState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let mut cache = state.usage_cache.lock().await;
    if headers
        .get(header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        != Some("no-cache")
        && let Some((at, value)) = cache.as_ref()
        && at.elapsed() < Duration::from_secs(300)
    {
        return Ok(response(value.clone()));
    }
    let client = ProviderClient::new().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let (openai, xai) = tokio::join!(
        usage::openai_codex_usage(client.http()),
        usage::xai_usage(&client, &state.store),
    );
    let checked_at_unix = usage::checked_at_unix();
    let providers = [(ProviderId::Openai, openai), (ProviderId::Xai, xai)]
        .into_iter()
        .map(|(provider, result)| ProviderUsageSnapshot {
            provider: provider as i32,
            checked_at_unix,
            windows: result
                .unwrap_or_default()
                .into_iter()
                .map(|window| ProviderUsageWindow {
                    id: window.id.to_owned(),
                    label: window.label.to_owned(),
                    used_percent: window.used_percent,
                    resets_at_unix: window.resets_at_unix,
                })
                .collect(),
        })
        .collect();
    let value = ProviderUsageResponse { providers };
    let points = value
        .providers
        .iter()
        .flat_map(|provider| {
            provider.windows.iter().map(|window| SubscriptionUsageRow {
                provider: provider.provider,
                window_id: window.id.clone(),
                label: window.label.clone(),
                observed_at_ms: provider
                    .checked_at_unix
                    .saturating_mul(1000)
                    .min(i64::MAX as u64) as i64,
                used_percent: i64::from(window.used_percent),
                resets_at_unix: window.resets_at_unix.min(i64::MAX as u64) as i64,
            })
        })
        .collect::<Vec<_>>();
    if state.db.record_subscription_usage(&points).is_err() {
        eprintln!("could not record subscription usage history");
    }
    *cache = Some((Instant::now(), value.clone()));
    Ok(response(value))
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
    if !matches!(provider, "openai" | "xai" | "anthropic" | "zai") {
        return Err(StatusCode::BAD_REQUEST);
    }
    if state.login.lock().await[index].in_progress {
        return Err(StatusCode::CONFLICT);
    }
    let _lead_guard = state.lead_lock.lock().await;
    if matches!(provider, "anthropic" | "zai") {
        let external = if provider == "anthropic" {
            Provider::Anthropic
        } else {
            Provider::Zai
        };
        beale_providers::subscription::logout(external).map_err(|_| StatusCode::BAD_GATEWAY)?;
    } else {
        state
            .store
            .remove_oauth(provider)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }
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
    let provider = if request.provider == ProviderId::Custom as i32 {
        if !beale_providers::valid_custom_id(&request.custom_provider_id) {
            return Err(StatusCode::BAD_REQUEST);
        }
        request.custom_provider_id.as_str()
    } else {
        checked_provider(request.provider)?.0
    };
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
    if !matches!(provider, "openai" | "xai" | "anthropic" | "zai") {
        return Err(StatusCode::BAD_REQUEST);
    }
    if matches!(provider, "anthropic" | "zai") {
        return Ok(response(StartSubscriptionResponse {
            verification_url: String::new(),
            user_code: String::new(),
            launch_official_app: true,
        }));
    }
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
            launch_official_app: false,
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
    if mode == AuthMode::Subscription
        && !matches!(
            provider,
            Provider::OpenAi | Provider::Xai | Provider::Anthropic | Provider::Zai
        )
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let client = ProviderClient::new().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let catalog = if mode == AuthMode::Subscription
        && matches!(provider, Provider::Anthropic | Provider::Zai)
    {
        beale_providers::subscription::catalog(provider, &state.store)
    } else {
        client.list_models(provider, mode, &state.store).await
    }
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
    if mode == AuthMode::Subscription
        && !matches!(
            provider,
            Provider::OpenAi | Provider::Xai | Provider::Anthropic | Provider::Zai
        )
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let client = ProviderClient::new().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let catalog = if mode == AuthMode::Subscription
        && matches!(provider, Provider::Anthropic | Provider::Zai)
    {
        beale_providers::subscription::catalog(provider, &state.store)
    } else {
        client.list_models(provider, mode, &state.store).await
    }
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

async fn set_source_priority(
    State(state): State<AuthState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let request: SetSourcePriorityRequest = checked_body(&headers, &body)?;
    let (provider, _) = checked_provider(request.provider)?;
    let mode = model_method(request.method)?;
    if !matches!(provider, "openai" | "xai" | "anthropic" | "zai") {
        return Err(StatusCode::BAD_REQUEST);
    }
    let subscription = if provider == "anthropic" {
        beale_providers::subscription::configured(Provider::Anthropic)
    } else if provider == "zai" {
        beale_providers::subscription::configured(Provider::Zai)
    } else {
        state
            .store
            .load(provider)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .is_some()
    };
    if !subscription
        || !state
            .store
            .api_key_configured(provider)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        return Err(StatusCode::CONFLICT);
    }
    state
        .store
        .save_source_priority(provider, mode_name(mode))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn set_daybreak_mode(
    State(state): State<AuthState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let request: SetDaybreakModeRequest = checked_body(&headers, &body)?;
    let mode = match DaybreakMode::try_from(request.mode) {
        Ok(DaybreakMode::Blue) => "blue",
        Ok(DaybreakMode::Red) => "red",
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    state
        .store
        .save_daybreak_mode(mode)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

fn wire_custom_provider(
    store: &CredentialStore,
    provider: CustomProvider,
) -> Result<WireCustomProvider, StatusCode> {
    let api_key_configured = store
        .api_key_configured(&provider.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(WireCustomProvider {
        id: provider.id,
        name: provider.name,
        base_url: provider.base_url,
        models: provider
            .models
            .into_iter()
            .map(|model| ModelEntry {
                id: model.id,
                display_name: model.name,
            })
            .collect(),
        lead_model: provider.lead_model,
        small_model: provider.small_model,
        reasoning: provider.reasoning,
        api_key_configured,
    })
}

async fn list_custom_providers(
    State(state): State<AuthState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let providers = state
        .store
        .custom_providers()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .into_iter()
        .map(|provider| wire_custom_provider(&state.store, provider))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(response(ListCustomProvidersResponse { providers }))
}

async fn discover_custom_models(
    State(state): State<AuthState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let request: DiscoverCustomModelsRequest = checked_body(&headers, &body)?;
    if request.api_key.len() > 8192 || request.api_key.chars().any(char::is_control) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let normalized_url = beale_providers::custom::validate_base_url(&request.base_url)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let key = if !request.api_key.is_empty() {
        Some(request.api_key)
    } else if request.id.is_empty() {
        None
    } else {
        saved_discovery_key(&state.store, &request.id, &normalized_url)?
    };
    let client = ProviderClient::new().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let models =
        beale_providers::custom::discover_models(&client, &request.base_url, key.as_deref())
            .await
            .map_err(|_| StatusCode::BAD_GATEWAY)?;
    Ok(response(DiscoverCustomModelsResponse {
        models: models
            .into_iter()
            .map(|model| ModelEntry {
                id: model.id,
                display_name: model.name,
            })
            .collect(),
    }))
}

fn saved_discovery_key(
    store: &CredentialStore,
    id: &str,
    normalized_url: &str,
) -> Result<Option<String>, StatusCode> {
    let existing = store
        .custom_provider(id)
        .map_err(|_| StatusCode::BAD_REQUEST)?
        .ok_or(StatusCode::NOT_FOUND)?;
    if existing.base_url != normalized_url {
        return Ok(None);
    }
    store
        .load_api_key(id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn save_custom_provider(
    State(state): State<AuthState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let request: SaveCustomProviderRequest = checked_body(&headers, &body)?;
    let provider = request.provider.ok_or(StatusCode::BAD_REQUEST)?;
    let definition = CustomProvider {
        id: provider.id,
        name: provider.name,
        base_url: provider.base_url,
        models: provider
            .models
            .into_iter()
            .map(|model| beale_providers::ModelEntry {
                id: model.id,
                name: model.display_name,
            })
            .collect(),
        lead_model: provider.lead_model,
        small_model: provider.small_model,
        reasoning: provider.reasoning,
    };
    let id = definition.id.clone();
    let _lead_guard = state.lead_lock.lock().await;
    if let Some(existing) = state
        .store
        .custom_provider(&id)
        .map_err(|_| StatusCode::BAD_REQUEST)?
    {
        let next_url = beale_providers::custom::validate_base_url(&definition.base_url)
            .map_err(|_| StatusCode::BAD_REQUEST)?;
        if existing.base_url != next_url
            && state
                .store
                .api_key_configured(&id)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        {
            return Err(StatusCode::CONFLICT);
        }
    }
    state
        .store
        .save_custom_provider(definition)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    initialize_lead(&state.store, &id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_custom_provider(
    State(state): State<AuthState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let request: CustomProviderRequest = checked_body(&headers, &body)?;
    let _lead_guard = state.lead_lock.lock().await;
    state
        .store
        .remove_custom_provider(&request.id)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    reconcile_lead(&state.store)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn set_custom_api_key(
    State(state): State<AuthState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let request: SetCustomProviderApiKeyRequest = checked_body(&headers, &body)?;
    if state
        .store
        .custom_provider(&request.id)
        .map_err(|_| StatusCode::BAD_REQUEST)?
        .is_none()
    {
        return Err(StatusCode::NOT_FOUND);
    }
    let _lead_guard = state.lead_lock.lock().await;
    state
        .store
        .save_api_key(&request.id, &request.api_key)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    initialize_lead(&state.store, &request.id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_custom_api_key(
    State(state): State<AuthState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let request: CustomProviderRequest = checked_body(&headers, &body)?;
    if state
        .store
        .custom_provider(&request.id)
        .map_err(|_| StatusCode::BAD_REQUEST)?
        .is_none()
    {
        return Err(StatusCode::NOT_FOUND);
    }
    let _lead_guard = state.lead_lock.lock().await;
    state
        .store
        .remove_api_key(&request.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    reconcile_lead(&state.store)?;
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

    fn test_router(token: String, store: CredentialStore) -> Router {
        let root =
            std::env::temp_dir().join(format!("beale-auth-db-example-{}", uuid::Uuid::new_v4()));
        router(token, store, HostDatabase::open(&root).unwrap())
    }

    #[tokio::test]
    async fn custom_provider_lifecycle_keeps_key_out_of_list_and_reconciles_lead() {
        let root = std::env::temp_dir().join(format!(
            "beale-custom-route-example-{}",
            uuid::Uuid::new_v4()
        ));
        let store = CredentialStore::at(root.clone());
        let token = "ab".repeat(32);
        let app = test_router(token.clone(), CredentialStore::at(root.clone()));
        let provider = WireCustomProvider {
            id: "custom-example".to_owned(),
            name: "Example API".to_owned(),
            base_url: "https://api.example.test/v1".to_owned(),
            models: vec![ModelEntry {
                id: "example-model".to_owned(),
                display_name: "Example Model".to_owned(),
            }],
            lead_model: "example-model".to_owned(),
            small_model: "example-model".to_owned(),
            reasoning: false,
            api_key_configured: false,
        };
        let request = Request::builder()
            .method("POST")
            .uri(CUSTOM_PROVIDERS_PATH)
            .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(
                SaveCustomProviderRequest {
                    provider: Some(provider),
                }
                .encode_to_vec(),
            ))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(reconcile_lead(&store).unwrap(), ProviderId::Custom);
        let request = Request::builder()
            .method("POST")
            .uri(CUSTOM_PROVIDER_API_KEY_PATH)
            .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(
                SetCustomProviderApiKeyRequest {
                    id: "custom-example".to_owned(),
                    api_key: "sk-example".to_owned(),
                }
                .encode_to_vec(),
            ))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::NO_CONTENT
        );
        let request = Request::builder()
            .uri(CUSTOM_PROVIDERS_PATH)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let bytes = to_bytes(response.into_body(), 512_000).await.unwrap();
        assert!(
            !bytes
                .windows("sk-example".len())
                .any(|part| part == b"sk-example")
        );
        let list = ListCustomProvidersResponse::decode(bytes).unwrap();
        assert_eq!(list.providers.len(), 1);
        assert!(list.providers[0].api_key_configured);
        assert_eq!(reconcile_lead(&store).unwrap(), ProviderId::Custom);
        assert_eq!(
            saved_discovery_key(&store, "custom-example", "https://api.example.test/v1")
                .unwrap()
                .as_deref(),
            Some("sk-example")
        );
        assert!(
            saved_discovery_key(&store, "custom-example", "https://other.example.test/v1")
                .unwrap()
                .is_none()
        );
        let mut changed = list.providers[0].clone();
        changed.base_url = "https://other.example.test/v1".to_owned();
        let request = Request::builder()
            .method("POST")
            .uri(CUSTOM_PROVIDERS_PATH)
            .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(
                SaveCustomProviderRequest {
                    provider: Some(changed),
                }
                .encode_to_vec(),
            ))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            store
                .custom_provider("custom-example")
                .unwrap()
                .unwrap()
                .base_url,
            "https://api.example.test/v1"
        );
        let request = Request::builder()
            .method("DELETE")
            .uri(CUSTOM_PROVIDERS_PATH)
            .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(
                CustomProviderRequest {
                    id: "custom-example".to_owned(),
                }
                .encode_to_vec(),
            ))
            .unwrap();
        assert_eq!(
            app.oneshot(request).await.unwrap().status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(reconcile_lead(&store).unwrap(), ProviderId::Unspecified);
        assert!(!store.api_key_configured("custom-example").unwrap());
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

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
        let app = test_router(token.clone(), CredentialStore::at(std::env::temp_dir()));
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
    async fn usage_snapshot_requires_control_token() {
        let app = test_router("ab".repeat(32), CredentialStore::at(std::env::temp_dir()));
        let request = Request::builder()
            .uri(PROVIDER_USAGE_PATH)
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.oneshot(request).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn api_key_routes_require_token_and_keep_key_out_of_status() {
        let root =
            std::env::temp_dir().join(format!("beale-auth-example-{}", uuid::Uuid::new_v4()));
        let store = CredentialStore::at(root.clone());
        let token = "ab".repeat(32);
        let app = test_router(token.clone(), CredentialStore::at(root.clone()));
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
                    custom_provider_id: String::new(),
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
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn lead_route_requires_auth_and_a_configured_provider() {
        let root = std::env::temp_dir().join(format!("beale-lead-route-{}", uuid::Uuid::new_v4()));
        let token = "ab".repeat(32);
        let app = test_router(token.clone(), CredentialStore::at(root.clone()));
        let body = ProviderRequest {
            provider: ProviderId::Xai as i32,
            custom_provider_id: String::new(),
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

    #[tokio::test]
    async fn source_priority_requires_both_credentials_and_round_trips_in_status() {
        let root =
            std::env::temp_dir().join(format!("beale-priority-route-{}", uuid::Uuid::new_v4()));
        let token = "ab".repeat(32);
        let store = CredentialStore::at(root.clone());
        let app = test_router(token.clone(), CredentialStore::at(root.clone()));
        let body = SetSourcePriorityRequest {
            provider: ProviderId::Openai as i32,
            method: AuthMethod::ApiKey as i32,
        }
        .encode_to_vec();
        let request = Request::builder()
            .method("POST")
            .uri(SOURCE_PRIORITY_PATH)
            .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
            .body(Body::from(body.clone()))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        store
            .save_api_key("openai", "sk-example-synthetic")
            .unwrap();
        let request = Request::builder()
            .method("POST")
            .uri(SOURCE_PRIORITY_PATH)
            .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(body.clone()))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::CONFLICT
        );
        store
            .save(
                "openai",
                &beale_providers::OAuthCredential {
                    access_token: "synthetic-access".to_owned(),
                    refresh_token: "synthetic-refresh".to_owned(),
                    expires_at: 1,
                    client_id: "synthetic-client".to_owned(),
                    id_token: None,
                    subject: None,
                    scopes: Vec::new(),
                },
            )
            .unwrap();
        let request = Request::builder()
            .method("POST")
            .uri(SOURCE_PRIORITY_PATH)
            .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(body))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::NO_CONTENT
        );
        let request = Request::builder()
            .uri(AUTH_STATUS_PATH)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        let status = ProviderAuthStatusResponse::decode(bytes).unwrap();
        assert_eq!(
            status.providers[0].source_priority,
            AuthMethod::ApiKey as i32
        );
        assert_eq!(
            status.providers[1].source_priority,
            AuthMethod::Subscription as i32
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn daybreak_mode_requires_host_auth_and_round_trips() {
        let root = std::env::temp_dir().join(format!(
            "beale-daybreak-route-example-{}",
            uuid::Uuid::new_v4()
        ));
        let token = "ab".repeat(32);
        let app = test_router(token.clone(), CredentialStore::at(root.clone()));
        let request = Request::builder()
            .uri(AUTH_STATUS_PATH)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        let status = ProviderAuthStatusResponse::decode(bytes).unwrap();
        assert_eq!(status.openai_daybreak_mode, DaybreakMode::Blue as i32);
        let body = SetDaybreakModeRequest {
            mode: DaybreakMode::Red as i32,
        }
        .encode_to_vec();
        let request = Request::builder()
            .method("POST")
            .uri(DAYBREAK_MODE_PATH)
            .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
            .body(Body::from(body.clone()))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        for mode in [DaybreakMode::Unspecified, DaybreakMode::Red] {
            let request = Request::builder()
                .method("POST")
                .uri(DAYBREAK_MODE_PATH)
                .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    SetDaybreakModeRequest { mode: mode as i32 }.encode_to_vec(),
                ))
                .unwrap();
            assert_eq!(
                app.clone().oneshot(request).await.unwrap().status(),
                if mode == DaybreakMode::Red {
                    StatusCode::NO_CONTENT
                } else {
                    StatusCode::BAD_REQUEST
                }
            );
        }
        let request = Request::builder()
            .uri(AUTH_STATUS_PATH)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        let status = ProviderAuthStatusResponse::decode(bytes).unwrap();
        assert_eq!(status.openai_daybreak_mode, DaybreakMode::Red as i32);
        assert_eq!(
            CredentialStore::at(root.clone()).daybreak_mode().unwrap(),
            "red"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
