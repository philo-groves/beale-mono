use anyhow::{Context, Result, bail, ensure};
use axum::{
    Router,
    http::{HeaderMap, HeaderValue, StatusCode},
    routing::get,
};
use beale_contracts::{
    DEFAULT_LOCAL_ADDR, HEALTH_PATH, PROTOBUF_CONTENT_TYPE, PROTOCOL_MAJOR, PROTOCOL_MINOR,
    SERVICE_NAME, v1::HealthResponse,
};
use beale_providers::{
    AuthMode, CredentialStore, InferenceRequest, Provider, ProviderClient, TextMessage,
};
use prost::Message;
use std::{io::Read, net::SocketAddr};
use tokio::net::TcpListener;

mod auth;
mod session;
mod workspace;

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
    let configured = match std::env::var("BEALE_V2_APP_SERVER_ADDR") {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => DEFAULT_LOCAL_ADDR.to_owned(),
        Err(std::env::VarError::NotUnicode(_)) => {
            bail!("BEALE_V2_APP_SERVER_ADDR must contain a valid address")
        }
    };
    let addr = local_listen_addr(&configured)?;
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to listen on {addr}"))?;
    println!(
        "Beale app-server listening on http://{}",
        listener.local_addr()?
    );
    let store = CredentialStore::new()?;
    let token = store.control_token()?;
    let workspaces = workspace::WorkspaceStore::new()?;
    let app = Router::new()
        .route(HEALTH_PATH, get(health))
        .merge(workspace::router(token.clone(), workspaces.clone()))
        .merge(session::router(token.clone(), workspaces)?)
        .merge(auth::router(token, store));
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

fn local_listen_addr(value: &str) -> Result<SocketAddr> {
    let addr: SocketAddr = value
        .parse()
        .with_context(|| format!("invalid BEALE_V2_APP_SERVER_ADDR: {value}"))?;
    ensure!(
        addr.ip().is_loopback() && addr.port() != 0,
        "BEALE_V2_APP_SERVER_ADDR must be a loopback address with a nonzero port"
    );
    Ok(addr)
}

async fn health() -> (StatusCode, HeaderMap, Vec<u8>) {
    let body = HealthResponse {
        protocol_major: PROTOCOL_MAJOR,
        protocol_minor: PROTOCOL_MINOR,
        service: SERVICE_NAME.to_owned(),
    }
    .encode_to_vec();
    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type",
        HeaderValue::from_static(PROTOBUF_CONTENT_TYPE),
    );
    (StatusCode::OK, headers, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn health_uses_versioned_contract() {
        let (_, headers, body) = health().await;
        assert_eq!(headers["content-type"], PROTOBUF_CONTENT_TYPE);
        let message = HealthResponse::decode(body.as_slice()).unwrap();
        assert_eq!(message.protocol_major, PROTOCOL_MAJOR);
        assert_eq!(message.service, SERVICE_NAME);
    }

    #[test]
    fn listen_address_stays_on_loopback() {
        assert!(local_listen_addr(DEFAULT_LOCAL_ADDR).is_ok());
        assert!(local_listen_addr("[::1]:43831").is_ok());
        assert!(local_listen_addr("0.0.0.0:43831").is_err());
        assert!(local_listen_addr("127.0.0.1:0").is_err());
    }
}
