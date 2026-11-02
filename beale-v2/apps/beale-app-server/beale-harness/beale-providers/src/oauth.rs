use crate::credentials::{CredentialStore, OAuthCredential};
use anyhow::{Context, Result, bail, ensure};
use axum::{
    Router,
    extract::{Query, State},
    routing::get,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::TcpListener,
    sync::{Mutex, oneshot},
    time::{Duration, Instant, sleep, timeout},
};
use url::Url;

const OPENAI_AUTH: &str = "https://auth.openai.com/api/accounts/authorize";
const OPENAI_TOKEN: &str = "https://auth.openai.com/api/accounts/oauth/token";
const OPENAI_JWKS: &str = "https://auth.openai.com/.well-known/jwks.json";
const OPENAI_RESOURCE: &str = "https://api.openai.com/v1";
const XAI_DEVICE: &str = "https://auth.x.ai/oauth2/device/code";
const XAI_TOKEN: &str = "https://auth.x.ai/oauth2/token";
// The public Grok CLI device client currently used by Beale v1. Keep isolated
// from provider inference so a future Beale-specific registration can replace it.
const XAI_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const XAI_SCOPE: &str = "openid profile email offline_access grok-cli:access api:access";

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    id_token: Option<String>,
    scope: Option<String>,
}

#[derive(Deserialize)]
struct IdClaims {
    sub: String,
    nonce: String,
}

#[derive(Deserialize)]
struct Callback {
    code: Option<String>,
    state: Option<String>,
    client_id: Option<String>,
    error: Option<String>,
}

type CallbackSender = Arc<Mutex<Option<oneshot::Sender<Callback>>>>;

async fn callback_handler(
    State(sender): State<CallbackSender>,
    Query(callback): Query<Callback>,
) -> &'static str {
    if let Some(sender) = sender.lock().await.take() {
        let _ = sender.send(callback);
    }
    "Beale sign-in received. Return to the app-server terminal."
}

pub async fn login_openai(client: &reqwest::Client, store: &CredentialStore) -> Result<()> {
    let host_id = {
        let _lock = store.lock("openai").await?;
        store.host_id()?
    };
    let existing = store.load("openai")?;
    let (callback_tx, callback_rx) = oneshot::channel();
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let redirect = format!(
        "http://127.0.0.1:{}/auth/callback",
        listener.local_addr()?.port()
    );
    let sender = Arc::new(Mutex::new(Some(callback_tx)));
    let app = Router::new()
        .route("/auth/callback", get(callback_handler))
        .with_state(sender);
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let state = uuid::Uuid::new_v4().to_string();
    let nonce = uuid::Uuid::new_v4().to_string();
    let verifier = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut url = Url::parse(OPENAI_AUTH)?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair(
            "client_id",
            existing
                .as_ref()
                .map_or("dynamic_agent_client", |v| &v.client_id),
        );
        if existing.is_none() {
            query.append_pair("agent_name_hint", "Beale");
        }
        query.append_pair("ext_agent_host_id", &host_id);
        query.append_pair("response_type", "code");
        query.append_pair("redirect_uri", &redirect);
        query.append_pair(
            "scope",
            "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct",
        );
        query.append_pair("resource", OPENAI_RESOURCE);
        query.append_pair("state", &state);
        query.append_pair("nonce", &nonce);
        query.append_pair("code_challenge_method", "S256");
        query.append_pair("code_challenge", &challenge);
    }
    println!("Open this URL to sign in with ChatGPT:\n{url}");

    let callback_result = timeout(Duration::from_secs(300), callback_rx).await;
    let _ = shutdown_tx.send(());
    server.await??;
    let callback = callback_result.context("OpenAI sign-in timed out")??;
    ensure!(
        callback.state.as_deref() == Some(state.as_str()),
        "OpenAI sign-in state mismatch"
    );
    if callback.error.is_some() {
        bail!("OpenAI sign-in was denied or cancelled");
    }
    let code = callback
        .code
        .context("OpenAI sign-in did not return a code")?;
    let client_id = match existing.as_ref() {
        Some(previous) => {
            ensure!(
                callback
                    .client_id
                    .as_ref()
                    .is_none_or(|id| id == &previous.client_id),
                "OpenAI client registration changed"
            );
            previous.client_id.clone()
        }
        None => callback
            .client_id
            .context("OpenAI registration did not return an issued client ID")?,
    };
    ensure!(
        !client_id.is_empty() && client_id.len() <= 256 && client_id != "dynamic_agent_client",
        "invalid issued OpenAI client ID"
    );
    let response = client
        .post(OPENAI_TOKEN)
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", client_id.as_str()),
            ("code", code.as_str()),
            ("code_verifier", verifier.as_str()),
            ("redirect_uri", redirect.as_str()),
            ("resource", OPENAI_RESOURCE),
        ])
        .send()
        .await
        .context("OpenAI token exchange failed")?;
    ensure!(
        response.status().is_success(),
        "OpenAI token exchange rejected the code ({})",
        response.status()
    );
    let token: TokenResponse = response
        .json()
        .await
        .context("invalid OpenAI token response")?;
    let id_token = token
        .id_token
        .as_deref()
        .context("OpenAI did not return an ID token")?;
    let subject = validate_openai_id_token(client, id_token, &client_id, &nonce).await?;
    if let Some(previous) = existing {
        ensure!(
            previous.subject.as_deref() == Some(&subject),
            "OpenAI account changed during reauthorization"
        );
    }
    let scopes = parse_scopes(token.scope.as_deref());
    ensure!(
        scopes
            .iter()
            .any(|scope| scope == "chatgpt.tokens.use.direct"),
        "OpenAI did not grant ChatGPT plan access"
    );
    let _lock = store.lock("openai").await?;
    store.save(
        "openai",
        &OAuthCredential {
            access_token: token.access_token,
            refresh_token: token
                .refresh_token
                .context("OpenAI did not return a refresh token")?,
            expires_at: expiry(token.expires_in.unwrap_or(3600))?,
            client_id,
            id_token: Some(id_token.to_owned()),
            subject: Some(subject),
            scopes,
        },
    )?;
    Ok(())
}

async fn validate_openai_id_token(
    client: &reqwest::Client,
    token: &str,
    audience: &str,
    nonce: &str,
) -> Result<String> {
    let header = decode_header(token).context("invalid OpenAI ID token header")?;
    ensure!(
        header.alg == Algorithm::RS256,
        "unsupported OpenAI ID token algorithm"
    );
    let kid = header.kid.context("OpenAI ID token has no key ID")?;
    let jwks: JwkSet = client
        .get(OPENAI_JWKS)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let key = jwks
        .find(&kid)
        .context("OpenAI ID token signing key is unavailable")?;
    let key = DecodingKey::from_jwk(key)?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&["https://auth.openai.com"]);
    validation.set_audience(&[audience]);
    let claims = decode::<IdClaims>(token, &key, &validation)
        .context("OpenAI ID token validation failed")?
        .claims;
    ensure!(claims.nonce == nonce, "OpenAI ID token nonce mismatch");
    ensure!(!claims.sub.is_empty(), "OpenAI ID token subject is empty");
    Ok(claims.sub)
}

#[derive(Deserialize)]
struct DeviceResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: Option<String>,
    interval: Option<u64>,
    expires_in: u64,
}

#[derive(Deserialize)]
struct OAuthError {
    error: Option<String>,
}

pub async fn login_xai(client: &reqwest::Client, store: &CredentialStore) -> Result<()> {
    let response = client
        .post(XAI_DEVICE)
        .form(&[
            ("client_id", XAI_CLIENT_ID),
            ("scope", XAI_SCOPE),
            ("referrer", "beale"),
        ])
        .send()
        .await
        .context("xAI device authorization failed")?;
    ensure!(
        response.status().is_success(),
        "xAI device authorization failed ({})",
        response.status()
    );
    let device: DeviceResponse = response
        .json()
        .await
        .context("invalid xAI device response")?;
    let raw_url = device
        .verification_uri_complete
        .as_deref()
        .unwrap_or(&device.verification_uri);
    let url = Url::parse(raw_url).context("invalid xAI verification URL")?;
    ensure!(
        url.scheme() == "https",
        "xAI verification URL must use HTTPS"
    );
    println!("Open {url} and enter code: {}", device.user_code);
    let deadline = Instant::now() + Duration::from_secs(device.expires_in.min(900));
    let mut interval = device.interval.unwrap_or(5).max(1);
    loop {
        if Instant::now() >= deadline {
            bail!("xAI device sign-in timed out");
        }
        sleep(Duration::from_secs(interval)).await;
        let response = client
            .post(XAI_TOKEN)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", XAI_CLIENT_ID),
                ("device_code", device.device_code.as_str()),
            ])
            .send()
            .await
            .context("xAI device token polling failed")?;
        if response.status().is_success() {
            let token: TokenResponse = response
                .json()
                .await
                .context("invalid xAI token response")?;
            let _lock = store.lock("xai").await?;
            store.save(
                "xai",
                &OAuthCredential {
                    access_token: token.access_token,
                    refresh_token: token
                        .refresh_token
                        .context("xAI did not return a refresh token")?,
                    expires_at: expiry(token.expires_in.unwrap_or(3600))?,
                    client_id: XAI_CLIENT_ID.to_owned(),
                    id_token: None,
                    subject: None,
                    scopes: parse_scopes(token.scope.as_deref()),
                },
            )?;
            return Ok(());
        }
        let error: OAuthError = response
            .json()
            .await
            .context("invalid xAI polling response")?;
        match error.error.as_deref() {
            Some("authorization_pending") => {}
            Some("slow_down") => interval = interval.saturating_add(5).min(30),
            Some("access_denied" | "authorization_denied") => {
                bail!("xAI device authorization denied")
            }
            Some("expired_token") => bail!("xAI device code expired"),
            _ => bail!("xAI device token request failed"),
        }
    }
}

pub(crate) async fn refresh(
    client: &reqwest::Client,
    provider: &str,
    saved: &OAuthCredential,
) -> Result<OAuthCredential> {
    let (endpoint, resource) = match provider {
        "openai" => (OPENAI_TOKEN, Some(OPENAI_RESOURCE)),
        "xai" => (XAI_TOKEN, None),
        _ => bail!("unknown provider"),
    };
    let mut form = HashMap::from([
        ("grant_type", "refresh_token"),
        ("client_id", saved.client_id.as_str()),
        ("refresh_token", saved.refresh_token.as_str()),
    ]);
    if let Some(resource) = resource {
        form.insert("resource", resource);
    }
    let response = client
        .post(endpoint)
        .form(&form)
        .send()
        .await
        .context("OAuth refresh request failed")?;
    ensure!(
        response.status().is_success(),
        "OAuth refresh rejected ({})",
        response.status()
    );
    let token: TokenResponse = response
        .json()
        .await
        .context("invalid OAuth refresh response")?;
    let scopes = token
        .scope
        .as_deref()
        .map(|value| parse_scopes(Some(value)))
        .unwrap_or_else(|| saved.scopes.clone());
    if provider == "openai" {
        ensure!(
            scopes
                .iter()
                .any(|scope| scope == "chatgpt.tokens.use.direct"),
            "OpenAI plan access is no longer granted"
        );
    }
    Ok(OAuthCredential {
        access_token: token.access_token,
        refresh_token: match provider {
            "openai" => token
                .refresh_token
                .context("OpenAI did not rotate the refresh token")?,
            _ => token
                .refresh_token
                .unwrap_or_else(|| saved.refresh_token.clone()),
        },
        expires_at: expiry(token.expires_in.unwrap_or(3600))?,
        client_id: saved.client_id.clone(),
        // A replacement ID token needs its own validation. Keep the last verified identity hint.
        id_token: saved.id_token.clone(),
        subject: saved.subject.clone(),
        scopes,
    })
}

fn parse_scopes(value: Option<&str>) -> Vec<String> {
    value
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

fn expiry(seconds: u64) -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_secs()
        .saturating_add(seconds))
}
