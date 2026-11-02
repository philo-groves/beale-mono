use beale_contracts::{
    DEFAULT_LOCAL_ADDR, HEALTH_PATH, PROTOBUF_CONTENT_TYPE, PROTOCOL_MAJOR, PROTOCOL_MINOR,
    SERVICE_NAME, v1::HealthResponse,
};
use prost::Message;
use reqwest::{Client, StatusCode, header::CONTENT_TYPE};
use std::{env, net::SocketAddr, process::ExitCode, time::Duration};

const MAX_HEALTH_BYTES: usize = 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ConnectionStatus {
    Connecting,
    Connected { major: u32, minor: u32 },
    Offline,
    Incompatible,
    InvalidAddress,
}

impl ConnectionStatus {
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Connecting => "Connecting…".to_owned(),
            Self::Connected { major, minor } => format!("Connected · v{major}.{minor}"),
            Self::Offline => "Offline".to_owned(),
            Self::Incompatible => "Incompatible protocol".to_owned(),
            Self::InvalidAddress => "Invalid local address".to_owned(),
        }
    }
}

fn parse_local_addr(value: &str) -> Result<SocketAddr, ConnectionStatus> {
    let addr: SocketAddr = value
        .parse()
        .map_err(|_| ConnectionStatus::InvalidAddress)?;
    if !addr.ip().is_loopback() || addr.port() == 0 {
        return Err(ConnectionStatus::InvalidAddress);
    }
    Ok(addr)
}

pub(crate) fn configured_addr() -> Result<SocketAddr, ConnectionStatus> {
    match env::var("BEALE_V2_APP_SERVER_ADDR") {
        Ok(value) => parse_local_addr(&value),
        Err(env::VarError::NotPresent) => parse_local_addr(DEFAULT_LOCAL_ADDR),
        Err(env::VarError::NotUnicode(_)) => Err(ConnectionStatus::InvalidAddress),
    }
}

pub(crate) fn http_client() -> Result<Client, reqwest::Error> {
    Client::builder()
        .connect_timeout(Duration::from_secs(1))
        .timeout(Duration::from_secs(3))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
}

fn decode_health(content_type: Option<&str>, body: &[u8]) -> ConnectionStatus {
    let valid_type = content_type
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case(PROTOBUF_CONTENT_TYPE));
    if !valid_type || body.len() > MAX_HEALTH_BYTES {
        return ConnectionStatus::Incompatible;
    }
    let Ok(health) = HealthResponse::decode(body) else {
        return ConnectionStatus::Incompatible;
    };
    if health.service != SERVICE_NAME
        || health.protocol_major != PROTOCOL_MAJOR
        || health.protocol_minor < PROTOCOL_MINOR
    {
        return ConnectionStatus::Incompatible;
    }
    ConnectionStatus::Connected {
        major: health.protocol_major,
        minor: health.protocol_minor,
    }
}

pub(crate) async fn probe(client: &Client, addr: SocketAddr) -> ConnectionStatus {
    let url = format!("http://{addr}{HEALTH_PATH}");
    let Ok(mut response) = client.get(url).send().await else {
        return ConnectionStatus::Offline;
    };
    if response.status() != StatusCode::OK {
        return ConnectionStatus::Incompatible;
    }
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if body.len() + chunk.len() <= MAX_HEALTH_BYTES => {
                body.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Ok(Some(_)) => return ConnectionStatus::Incompatible,
            Err(_) => return ConnectionStatus::Offline,
        }
    }
    decode_health(content_type.as_deref(), &body)
}

pub(crate) fn check_cli() -> ExitCode {
    let Ok(addr) = configured_addr() else {
        eprintln!("Invalid BEALE_V2_APP_SERVER_ADDR; use a loopback IP and nonzero port.");
        return ExitCode::FAILURE;
    };
    let Ok(client) = http_client() else {
        eprintln!("Could not initialize the local app-server connection.");
        return ExitCode::FAILURE;
    };
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        eprintln!("Could not initialize the local app-server connection.");
        return ExitCode::FAILURE;
    };
    let status = runtime.block_on(probe(&client, addr));
    match status {
        ConnectionStatus::Connected { major, minor } => {
            println!("Connected to Beale app-server (protocol v{major}.{minor}).");
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("Beale app-server connection: {}.", other.label());
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_address_rejects_remote_or_ephemeral_bindings() {
        assert!(parse_local_addr(DEFAULT_LOCAL_ADDR).is_ok());
        assert!(parse_local_addr("[::1]:43831").is_ok());
        assert!(parse_local_addr("0.0.0.0:43831").is_err());
        assert!(parse_local_addr("127.0.0.1:0").is_err());
    }

    #[test]
    fn health_requires_the_shared_service_and_protocol() {
        let mut health = HealthResponse {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            service: SERVICE_NAME.to_owned(),
        };
        assert!(matches!(
            decode_health(Some(PROTOBUF_CONTENT_TYPE), &health.encode_to_vec()),
            ConnectionStatus::Connected { .. }
        ));
        health.protocol_major += 1;
        assert_eq!(
            decode_health(Some(PROTOBUF_CONTENT_TYPE), &health.encode_to_vec()),
            ConnectionStatus::Incompatible
        );
        health.protocol_major = PROTOCOL_MAJOR;
        health.protocol_minor = PROTOCOL_MINOR - 1;
        assert_eq!(
            decode_health(Some(PROTOBUF_CONTENT_TYPE), &health.encode_to_vec()),
            ConnectionStatus::Incompatible
        );
        health.protocol_minor = PROTOCOL_MINOR;
        health.service = "example-service".to_owned();
        assert_eq!(
            decode_health(Some(PROTOBUF_CONTENT_TYPE), &health.encode_to_vec()),
            ConnectionStatus::Incompatible
        );
        health.service = SERVICE_NAME.to_owned();
        assert_eq!(
            decode_health(Some("text/plain"), &health.encode_to_vec()),
            ConnectionStatus::Incompatible
        );
    }
}
