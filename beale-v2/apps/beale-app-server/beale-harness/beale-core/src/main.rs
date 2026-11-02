use anyhow::{Result, bail};
use axum::{
    Router,
    http::{HeaderMap, HeaderValue, StatusCode},
    routing::get,
};
use beale_contracts::{PROTOCOL_MAJOR, PROTOCOL_MINOR, v1::HealthResponse};
use beale_providers::{
    AuthMode, CredentialStore, InferenceRequest, Provider, ProviderClient, TextMessage,
};
use prost::Message;
use std::io::Read;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        None | Some("serve") => serve().await,
        Some("login-openai") => {
            let client = ProviderClient::new()?;
            beale_providers::login_openai(client.http(), &CredentialStore::new()?).await?;
            println!("OpenAI sign-in saved on this host.");
            Ok(())
        }
        Some("login-xai") => {
            let client = ProviderClient::new()?;
            beale_providers::login_xai(client.http(), &CredentialStore::new()?).await?;
            println!("xAI sign-in saved on this host.");
            Ok(())
        }
        Some("infer") => {
            let provider = match args.next().as_deref() {
                Some("openai") => Provider::OpenAi,
                Some("xai") => Provider::Xai,
                _ => bail!("provider must be openai or xai"),
            };
            let auth = match args.next().as_deref() {
                Some("api-key") => AuthMode::ApiKey,
                Some("subscription") => AuthMode::Subscription,
                _ => bail!("auth mode must be api-key or subscription"),
            };
            let model = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("model is required"))?;
            if args.next().is_some() {
                bail!("unexpected argument");
            }
            let mut prompt = String::new();
            std::io::stdin()
                .take(1_048_577)
                .read_to_string(&mut prompt)?;
            let client = ProviderClient::new()?;
            let response = client
                .infer(
                    provider,
                    auth,
                    &InferenceRequest {
                        model,
                        messages: vec![TextMessage {
                            role: beale_providers::TextRole::User,
                            text: prompt,
                        }],
                    },
                    &CredentialStore::new()?,
                )
                .await?;
            println!("{response}");
            Ok(())
        }
        _ => bail!(
            "usage: beale-core [serve | login-openai | login-xai | infer <openai|xai> <api-key|subscription> <model>]"
        ),
    }
}

async fn serve() -> Result<()> {
    // Client authentication and Tailscale binding are separate contract work.
    // This first host slice only exposes a bounded local health response.
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    println!(
        "Beale app-server listening on http://{}",
        listener.local_addr()?
    );
    let app = Router::new().route("/v1/health", get(health));
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

async fn health() -> (StatusCode, HeaderMap, Vec<u8>) {
    let body = HealthResponse {
        protocol_major: PROTOCOL_MAJOR,
        protocol_minor: PROTOCOL_MINOR,
        service: "beale-app-server".to_owned(),
    }
    .encode_to_vec();
    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type",
        HeaderValue::from_static("application/x-protobuf"),
    );
    (StatusCode::OK, headers, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn health_uses_versioned_contract() {
        let (_, headers, body) = health().await;
        assert_eq!(headers["content-type"], "application/x-protobuf");
        let message = HealthResponse::decode(body.as_slice()).unwrap();
        assert_eq!(message.protocol_major, 1);
    }
}
