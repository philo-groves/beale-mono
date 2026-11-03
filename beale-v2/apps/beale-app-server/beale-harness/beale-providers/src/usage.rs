//! Host-only subscription window probes. No credential or upstream payload is returned to clients.

use crate::{AuthMode, CredentialStore, Provider, ProviderClient};
use anyhow::{Context, Result, ensure};
use futures_util::StreamExt;
use serde_json::Value;
use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_USAGE_BYTES: usize = 65_536;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageWindow {
    pub id: &'static str,
    pub label: &'static str,
    pub used_percent: u32,
    pub resets_at_unix: u64,
}

fn percent(value: Option<&Value>) -> Option<u32> {
    let value = value?.as_f64()?;
    value
        .is_finite()
        .then(|| value.round().clamp(0.0, 100.0) as u32)
}

fn unix_seconds(value: Option<&Value>) -> u64 {
    value.and_then(Value::as_u64).unwrap_or(0)
}

pub fn parse_codex_usage(value: &Value) -> Vec<UsageWindow> {
    let limits = value
        .pointer("/rate_limits_by_limit_id/codex")
        .or_else(|| value.get("rate_limit"))
        .or_else(|| value.get("rate_limits"));
    let Some(limits) = limits else {
        return Vec::new();
    };
    [
        ("primary", "Session Limit", "primary_window"),
        ("secondary", "Weekly Limit", "secondary_window"),
    ]
    .into_iter()
    .filter_map(|(id, label, key)| {
        let window = limits.get(key)?;
        let used_percent = percent(window.get("used_percent"))?;
        let duration = unix_seconds(window.get("limit_window_seconds"));
        let label = if duration >= 30 * 24 * 3600 {
            "Monthly Limit"
        } else if duration >= 7 * 24 * 3600 {
            "Weekly Limit"
        } else {
            label
        };
        Some(UsageWindow {
            id,
            label,
            used_percent,
            resets_at_unix: unix_seconds(window.get("reset_at")),
        })
    })
    .collect()
}

pub fn parse_xai_usage(value: &Value) -> Vec<UsageWindow> {
    let Some(config) = value.get("config") else {
        return Vec::new();
    };
    let period = &config["currentPeriod"];
    let period_type = period["type"].as_str().unwrap_or("");
    let reset = period["end"]
        .as_str()
        .and_then(|s| {
            // xAI returns RFC3339 here; retain a bounded timestamp only when parseable.
            chrono::DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|d| d.timestamp().max(0) as u64)
        })
        .unwrap_or(0);
    let used_percent = percent(config.get("creditUsagePercent")).or_else(|| {
        (reset > 0
            && matches!(
                period_type,
                "USAGE_PERIOD_TYPE_WEEKLY" | "USAGE_PERIOD_TYPE_MONTHLY"
            ))
        .then_some(0)
    });
    let Some(used_percent) = used_percent else {
        return Vec::new();
    };
    let label = if period_type.ends_with("WEEKLY") {
        "Weekly Limit"
    } else if period_type.ends_with("MONTHLY") {
        "Monthly Limit"
    } else {
        "Subscription Limit"
    };
    vec![UsageWindow {
        id: "subscription",
        label,
        used_percent,
        resets_at_unix: reset,
    }]
}

async fn read_json(request: reqwest::RequestBuilder) -> Result<Value> {
    let response = request.timeout(Duration::from_secs(8)).send().await?;
    ensure!(response.status().is_success(), "usage service unavailable");
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        ensure!(
            bytes.len().saturating_add(chunk.len()) <= MAX_USAGE_BYTES,
            "usage response too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).context("invalid usage response")
}

fn codex_auth_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("APP_SERVER_CODEX_AUTH_FILE")
        .or_else(|| std::env::var_os("BEALE_OPENAI_CODEX_AUTH_FILE"))
    {
        return Some(PathBuf::from(path));
    }
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
    Some(PathBuf::from(home).join(".codex").join("auth.json"))
}

pub async fn openai_codex_usage(client: &reqwest::Client) -> Result<Vec<UsageWindow>> {
    let path = codex_auth_path().context("Codex sign-in unavailable")?;
    ensure!(
        std::fs::metadata(&path)?.len() <= MAX_USAGE_BYTES as u64,
        "Codex credential file too large"
    );
    let bytes = std::fs::read(path).context("Codex sign-in unavailable")?;
    ensure!(
        bytes.len() <= MAX_USAGE_BYTES,
        "Codex credential file too large"
    );
    let auth: Value = serde_json::from_slice(&bytes).context("invalid Codex credential file")?;
    let token = auth
        .pointer("/tokens/access_token")
        .and_then(Value::as_str)
        .context("Codex access token unavailable")?;
    let account = auth
        .pointer("/tokens/account_id")
        .and_then(Value::as_str)
        .context("Codex account unavailable")?;
    ensure!(
        !token.is_empty() && !account.is_empty(),
        "Codex sign-in unavailable"
    );
    let value = read_json(
        client
            .get("https://chatgpt.com/backend-api/wham/usage")
            .bearer_auth(token)
            .header("ChatGPT-Account-Id", account)
            .header("Accept", "application/json"),
    )
    .await?;
    Ok(parse_codex_usage(&value))
}

pub async fn xai_usage(
    client: &ProviderClient,
    store: &CredentialStore,
) -> Result<Vec<UsageWindow>> {
    if store.load("xai")?.is_none() {
        return Ok(Vec::new());
    }
    let token = client
        .token(Provider::Xai, AuthMode::Subscription, store)
        .await?;
    let value = read_json(
        client
            .http()
            .get("https://cli-chat-proxy.grok.com/v1/billing?format=credits")
            .bearer_auth(token)
            .header("X-XAI-Token-Auth", "xai-grok-cli")
            .header("Accept", "application/json"),
    )
    .await?;
    Ok(parse_xai_usage(&value))
}

pub fn checked_at_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn codex_windows_are_bounded_and_labeled() {
        let windows = parse_codex_usage(&json!({"rate_limit": {
            "primary_window": {"used_percent": 42.4, "limit_window_seconds": 18000, "reset_at": 2000000000},
            "secondary_window": {"used_percent": 120, "limit_window_seconds": 604800, "reset_at": 2000000001}
        }}));
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].label, "Session Limit");
        assert_eq!(windows[0].used_percent, 42);
        assert_eq!(windows[1].used_percent, 100);
    }

    #[test]
    fn xai_omitted_zero_percent_is_reported_only_for_a_current_period() {
        let windows = parse_xai_usage(
            &json!({"config": {"currentPeriod": {"type": "USAGE_PERIOD_TYPE_WEEKLY", "end": "2033-05-17T00:00:00Z"}}}),
        );
        assert_eq!(windows[0].used_percent, 0);
        assert_eq!(windows[0].label, "Weekly Limit");
        assert!(parse_xai_usage(&json!({"config": {}})).is_empty());
    }
}
