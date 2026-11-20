//! Read the host Codex sign-in. Codex owns this file and its renewal lifecycle.
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug)]
pub(crate) struct CodexSignInError;

impl std::fmt::Display for CodexSignInError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Codex sign-in is unavailable")
    }
}

impl std::error::Error for CodexSignInError {}

const MAX_AUTH_BYTES: u64 = 1_048_576;

#[derive(Clone, Debug)]
pub(crate) struct CodexCredential {
    pub access_token: String,
    pub account_id: String,
}

fn auth_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("APP_SERVER_CODEX_AUTH_FILE")
        .or_else(|| std::env::var_os("BEALE_OPENAI_CODEX_AUTH_FILE"))
    {
        return Some(PathBuf::from(path));
    }
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .map(|home| PathBuf::from(home).join(".codex"))
        })?;
    Some(home.join("auth.json"))
}

pub(crate) fn credential() -> Result<CodexCredential> {
    read_credential().map_err(|error| error.context(CodexSignInError))
}

fn read_credential() -> Result<CodexCredential> {
    let path = auth_path().context("Sign in to Codex to use the OpenAI subscription")?;
    ensure!(
        std::fs::metadata(&path)
            .context("Sign in to Codex to use the OpenAI subscription")?
            .len()
            <= MAX_AUTH_BYTES,
        "Codex credential file is too large"
    );
    let bytes = std::fs::read(path).context("Codex sign-in is unavailable")?;
    ensure!(
        bytes.len() as u64 <= MAX_AUTH_BYTES,
        "Codex credential file is too large"
    );
    parse_credential(&bytes)
}

pub fn configured() -> bool {
    credential().is_ok()
}

/// String credential values in the Codex sign-in file, for output redaction.
pub(crate) fn secret_values() -> Vec<String> {
    let Some(path) = auth_path() else {
        return Vec::new();
    };
    match std::fs::metadata(&path) {
        Ok(metadata) if metadata.len() <= MAX_AUTH_BYTES => {}
        _ => return Vec::new(),
    }
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    secret_values_from(&bytes)
}

fn secret_values_from(bytes: &[u8]) -> Vec<String> {
    let Ok(auth) = serde_json::from_slice::<Value>(bytes) else {
        return Vec::new();
    };
    let mut values = Vec::new();
    if let Some(tokens) = auth.get("tokens").and_then(Value::as_object) {
        values.extend(
            ["access_token", "refresh_token", "id_token"]
                .into_iter()
                .filter_map(|key| tokens.get(key).and_then(Value::as_str))
                .map(str::to_owned),
        );
    }
    if let Some(key) = auth.get("OPENAI_API_KEY").and_then(Value::as_str) {
        values.push(key.to_owned());
    }
    values
}

fn parse_credential(bytes: &[u8]) -> Result<CodexCredential> {
    let auth: Value = serde_json::from_slice(bytes).context("Invalid Codex sign-in")?;
    ensure!(
        auth["auth_mode"] == "chatgpt",
        "Codex ChatGPT sign-in is required"
    );
    let token = auth
        .pointer("/tokens/access_token")
        .and_then(Value::as_str)
        .context("Codex access token unavailable")?;
    let account = auth
        .pointer("/tokens/account_id")
        .and_then(Value::as_str)
        .context("Codex account unavailable")?;
    ensure!(
        !token.is_empty()
            && token.len() <= 16_384
            && token
                .bytes()
                .all(|byte| { byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-') }),
        "Invalid Codex access token"
    );
    ensure!(
        !account.is_empty()
            && account.len() <= 256
            && account
                .bytes()
                .all(|byte| { byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-') }),
        "Invalid Codex account"
    );
    Ok(CodexCredential {
        access_token: token.to_owned(),
        account_id: account.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_values_cover_every_codex_token() {
        let auth = br#"{"auth_mode":"chatgpt","OPENAI_API_KEY":"sk-synthetic-key","tokens":{"access_token":"synthetic-access","refresh_token":"synthetic-refresh","id_token":"synthetic-id","account_id":"example-account"}}"#;
        assert_eq!(
            secret_values_from(auth),
            [
                "synthetic-access",
                "synthetic-refresh",
                "synthetic-id",
                "sk-synthetic-key"
            ]
        );
        assert!(secret_values_from(b"not json").is_empty());
    }

    #[test]
    fn accepts_codex_chatgpt_credential_only() {
        let auth = br#"{"auth_mode":"chatgpt","tokens":{"access_token":"synthetic-token","account_id":"example-account"}}"#;
        let credential = parse_credential(auth).unwrap();
        assert_eq!(credential.account_id, "example-account");
        assert!(parse_credential(br#"{"auth_mode":"api-key","tokens":{"access_token":"synthetic-token","account_id":"example-account"}}"#).is_err());
        assert!(
            parse_credential(
                br#"{"auth_mode":"chatgpt","tokens":{"access_token":"synthetic-token"}}"#
            )
            .is_err()
        );
        assert!(parse_credential(br#"{"auth_mode":"chatgpt","tokens":{"access_token":"synthetic-token","account_id":"bad\nheader"}}"#).is_err());
    }
}
