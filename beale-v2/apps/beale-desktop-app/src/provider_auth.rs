use beale_contracts::{
    AUTH_API_KEY_PATH, AUTH_PROVE_PATH, AUTH_STATUS_PATH, AUTH_SUBSCRIPTION_FORGET_PATH,
    AUTH_SUBSCRIPTION_START_PATH, CONTROL_CHALLENGE_BYTES, PROTOBUF_CONTENT_TYPE, control_proof,
    v1::{
        ControlChallengeRequest, ControlChallengeResponse, ProviderAuthStatusResponse, ProviderId,
        ProviderRequest, SetApiKeyRequest, StartSubscriptionResponse,
    },
};
use prost::Message;
use reqwest::{Client, StatusCode, header::CONTENT_TYPE};
use std::{net::SocketAddr, path::PathBuf};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct AuthStatus {
    pub subscription: bool,
    pub api_key: bool,
    pub in_progress: bool,
    pub error: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SignInPrompt {
    pub url: String,
    pub code: String,
}

pub(crate) enum Action {
    SetApiKey(ProviderId, String),
    RemoveApiKey(ProviderId),
    StartSubscription(ProviderId),
    ForgetSubscription(ProviderId),
}

fn control_token_path() -> Result<PathBuf, &'static str> {
    let root = if let Some(path) = std::env::var_os("BEALE_V2_HOME") {
        PathBuf::from(path)
    } else {
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .ok_or("Home directory is unavailable")?;
        PathBuf::from(home).join(".beale").join("v2")
    };
    Ok(root.join("app-server-control-token"))
}

fn token() -> Result<String, &'static str> {
    let value = std::fs::read_to_string(control_token_path()?)
        .map_err(|_| "App-server control token is unavailable")?;
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("App-server control token is invalid");
    }
    Ok(value)
}

fn url(addr: SocketAddr, path: &str) -> String {
    format!("http://{addr}{path}")
}

async fn verify_host(client: &Client, addr: SocketAddr, token: &str) -> Result<(), String> {
    let mut nonce = Vec::with_capacity(CONTROL_CHALLENGE_BYTES);
    nonce.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    nonce.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    let response = client
        .post(url(addr, AUTH_PROVE_PATH))
        .header(CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
        .timeout(std::time::Duration::from_secs(3))
        .body(
            ControlChallengeRequest {
                nonce: nonce.clone(),
            }
            .encode_to_vec(),
        )
        .send()
        .await
        .map_err(|_| "Could not verify the local app-server".to_owned())?;
    if response.status() != StatusCode::OK
        || response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            != Some(PROTOBUF_CONTENT_TYPE)
    {
        return Err("Could not verify the local app-server".to_owned());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "Could not verify the local app-server".to_owned())?;
    if bytes.len() > 128 {
        return Err("Could not verify the local app-server".to_owned());
    }
    let reply = ControlChallengeResponse::decode(bytes)
        .map_err(|_| "Could not verify the local app-server".to_owned())?;
    if !valid_proof(token, &nonce, &reply.proof) {
        return Err("Could not verify the local app-server".to_owned());
    }
    Ok(())
}

fn valid_proof(token: &str, nonce: &[u8], proof: &[u8]) -> bool {
    let expected = control_proof(token, nonce);
    proof.len() == expected.len()
        && proof
            .iter()
            .zip(expected)
            .fold(0_u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

pub(crate) async fn status(client: &Client, addr: SocketAddr) -> Result<[AuthStatus; 2], String> {
    let token = token().map_err(str::to_owned)?;
    verify_host(client, addr, &token).await?;
    let response = client
        .get(url(addr, AUTH_STATUS_PATH))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| "Could not reach the app-server".to_owned())?;
    if response.status() != StatusCode::OK
        || response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            != Some(PROTOBUF_CONTENT_TYPE)
    {
        return Err("App-server credential status is unavailable".to_owned());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "Could not read credential status".to_owned())?;
    if bytes.len() > 4096 {
        return Err("Credential status was too large".to_owned());
    }
    let reply = ProviderAuthStatusResponse::decode(bytes)
        .map_err(|_| "Invalid credential status".to_owned())?;
    let mut result = [AuthStatus::default(), AuthStatus::default()];
    for provider in reply.providers {
        let index = match ProviderId::try_from(provider.provider) {
            Ok(ProviderId::Openai) => 0,
            Ok(ProviderId::Xai) => 1,
            _ => return Err("Unknown provider in credential status".to_owned()),
        };
        result[index] = AuthStatus {
            subscription: provider.subscription_configured,
            api_key: provider.api_key_configured,
            in_progress: provider.login_in_progress,
            error: provider.login_error.chars().take(160).collect(),
        };
    }
    Ok(result)
}

pub(crate) fn check_cli() -> std::process::ExitCode {
    let result = (|| {
        let addr = crate::connection::configured_addr()
            .map_err(|_| "Invalid local app-server address".to_owned())?;
        let client = crate::connection::http_client()
            .map_err(|_| "Could not initialize app-server connection".to_owned())?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "Could not initialize app-server connection".to_owned())?;
        runtime.block_on(status(&client, addr))
    })();
    match result {
        Ok(providers) => {
            for (name, provider) in [("OpenAI", &providers[0]), ("xAI", &providers[1])] {
                println!(
                    "{name}: subscription={}, API key={}, sign-in={}",
                    if provider.subscription {
                        "configured"
                    } else {
                        "not configured"
                    },
                    if provider.api_key {
                        "configured"
                    } else {
                        "not configured"
                    },
                    if provider.in_progress {
                        "in progress"
                    } else {
                        "idle"
                    }
                );
            }
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

pub(crate) async fn perform(
    addr: SocketAddr,
    action: Action,
) -> Result<Option<SignInPrompt>, String> {
    let client = Client::builder()
        .connect_timeout(std::time::Duration::from_secs(1))
        .timeout(std::time::Duration::from_secs(35))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not initialize app-server connection".to_owned())?;
    let token = token().map_err(str::to_owned)?;
    verify_host(&client, addr, &token).await?;
    let (path, body, remove) = match action {
        Action::SetApiKey(provider, api_key) => (
            AUTH_API_KEY_PATH,
            SetApiKeyRequest {
                provider: provider as i32,
                api_key,
            }
            .encode_to_vec(),
            false,
        ),
        Action::RemoveApiKey(provider) => (
            AUTH_API_KEY_PATH,
            ProviderRequest {
                provider: provider as i32,
            }
            .encode_to_vec(),
            true,
        ),
        Action::StartSubscription(provider) => (
            AUTH_SUBSCRIPTION_START_PATH,
            ProviderRequest {
                provider: provider as i32,
            }
            .encode_to_vec(),
            false,
        ),
        Action::ForgetSubscription(provider) => (
            AUTH_SUBSCRIPTION_FORGET_PATH,
            ProviderRequest {
                provider: provider as i32,
            }
            .encode_to_vec(),
            false,
        ),
    };
    let request = if remove {
        client.delete(url(addr, path))
    } else {
        client.post(url(addr, path))
    };
    let response = request
        .bearer_auth(token)
        .header(CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
        .body(body)
        .send()
        .await
        .map_err(|_| "Could not reach the app-server".to_owned())?;
    if !response.status().is_success() {
        return Err(format!(
            "App-server rejected the change ({})",
            response.status()
        ));
    }
    if path == AUTH_SUBSCRIPTION_START_PATH {
        if response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            != Some(PROTOBUF_CONTENT_TYPE)
        {
            return Err("Invalid sign-in response".to_owned());
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| "Could not read sign-in response".to_owned())?;
        if bytes.len() > 4096 {
            return Err("Sign-in response was too large".to_owned());
        }
        let reply = StartSubscriptionResponse::decode(bytes)
            .map_err(|_| "Invalid sign-in response".to_owned())?;
        let parsed = reqwest::Url::parse(&reply.verification_url)
            .map_err(|_| "Invalid sign-in URL".to_owned())?;
        if parsed.scheme() != "https" || parsed.host_str().is_none() {
            return Err("Invalid sign-in URL".to_owned());
        }
        return Ok(Some(SignInPrompt {
            url: reply.verification_url,
            code: reply.user_code,
        }));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_proof_rejects_wrong_host_token_or_nonce() {
        let nonce = [7_u8; CONTROL_CHALLENGE_BYTES];
        let proof = control_proof(&"ab".repeat(32), &nonce);
        assert!(valid_proof(&"ab".repeat(32), &nonce, &proof));
        assert!(!valid_proof(&"cd".repeat(32), &nonce, &proof));
        assert!(!valid_proof(
            &"ab".repeat(32),
            &[8_u8; CONTROL_CHALLENGE_BYTES],
            &proof
        ));
    }
}
