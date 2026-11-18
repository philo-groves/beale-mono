use crate::credentials::{CredentialStore, OAuthCredential};
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use std::{
    collections::HashMap,
    fmt,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::time::{Duration, Instant, sleep};
use url::Url;

const XAI_DEVICE: &str = "https://auth.x.ai/oauth2/device/code";
const XAI_TOKEN: &str = "https://auth.x.ai/oauth2/token";
// The public Grok CLI device client currently used by Beale v1. Keep isolated
// from provider inference so a future Beale-specific registration can replace it.
const XAI_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const XAI_SCOPE: &str = "openid profile email offline_access grok-cli:access api:access";

#[derive(Debug)]
pub struct OAuthRefreshError {
    pub status: u16,
    pub code: Option<String>,
}

impl OAuthRefreshError {
    pub fn requires_reconnect(&self) -> bool {
        self.status == 401
            || matches!(
                self.code.as_deref(),
                Some(
                    "invalid_grant"
                        | "invalid_refresh_token"
                        | "token_expired"
                        | "refresh_token_expired"
                        | "refresh_token_invalidated"
                        | "refresh_token_reused"
                )
            )
    }
}

impl fmt::Display for OAuthRefreshError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "OAuth refresh rejected (HTTP {})", self.status)?;
        if let Some(code) = &self.code {
            write!(formatter, " [{code}]")?;
        }
        Ok(())
    }
}

impl std::error::Error for OAuthRefreshError {}

fn refresh_error(status: u16, body: &[u8]) -> OAuthRefreshError {
    let value = (body.len() <= 4_096)
        .then(|| serde_json::from_slice::<serde_json::Value>(body).ok())
        .flatten();
    let code = value
        .as_ref()
        .and_then(|value| {
            value
                .pointer("/error/code")
                .or_else(|| value.get("error"))
                .or_else(|| value.get("code"))
                .and_then(serde_json::Value::as_str)
        })
        .filter(|code| {
            !code.is_empty()
                && code.len() <= 80
                && code
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
        .map(str::to_ascii_lowercase);
    OAuthRefreshError { status, code }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    scope: Option<String>,
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
    login_xai_with_prompt(client, store, |url, code| {
        println!("Open {url} and enter code: {code}");
    })
    .await
}

pub async fn login_xai_with_prompt(
    client: &reqwest::Client,
    store: &CredentialStore,
    prompt: impl FnOnce(String, String) + Send,
) -> Result<()> {
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
    prompt(url.to_string(), device.user_code);
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
    ensure!(provider == "xai", "unknown provider");
    let form = HashMap::from([
        ("grant_type", "refresh_token"),
        ("client_id", saved.client_id.as_str()),
        ("refresh_token", saved.refresh_token.as_str()),
    ]);
    let mut response = client
        .post(XAI_TOKEN)
        .form(&form)
        .send()
        .await
        .context("OAuth refresh request failed")?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let mut body = Vec::new();
        if response
            .content_length()
            .is_none_or(|length| length <= 4_096)
        {
            while body.len() < 4_096 {
                match response.chunk().await {
                    Ok(Some(chunk)) if body.len() + chunk.len() <= 4_096 => {
                        body.extend_from_slice(&chunk)
                    }
                    _ => break,
                }
            }
        }
        return Err(refresh_error(status, &body).into());
    }
    let token: TokenResponse = response
        .json()
        .await
        .context("invalid OAuth refresh response")?;
    let scopes = token
        .scope
        .as_deref()
        .map(|value| parse_scopes(Some(value)))
        .unwrap_or_else(|| saved.scopes.clone());
    Ok(OAuthCredential {
        access_token: token.access_token,
        refresh_token: token
            .refresh_token
            .unwrap_or_else(|| saved.refresh_token.clone()),
        expires_at: expiry(token.expires_in.unwrap_or(3600))?,
        client_id: saved.client_id.clone(),
        // Preserve legacy identity fields when refreshing an existing xAI record.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_errors_keep_only_safe_codes() {
        let error = refresh_error(
            400,
            br#"{"error":"invalid_grant","error_description":"private upstream detail"}"#,
        );
        assert_eq!(error.code.as_deref(), Some("invalid_grant"));
        assert!(error.requires_reconnect());
        assert!(!error.to_string().contains("private upstream detail"));

        let error = refresh_error(503, br#"{"error":{"code":"temporarily_unavailable"}}"#);
        assert!(!error.requires_reconnect());
        let error = refresh_error(400, br#"{"error":"private user content\nsecret"}"#);
        assert!(error.code.is_none());
    }
}
