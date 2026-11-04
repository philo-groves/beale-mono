use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode, header},
    routing::{get, post},
};
use beale_contracts::{
    FLEET_DESCRIPTOR_PATH, FLEET_PAIR_PATH, FLEET_TOTP_PROVISION_PATH, PROTOBUF_CONTENT_TYPE,
    PROTOCOL_MAJOR, PROTOCOL_MINOR, SERVICE_NAME,
    v1::{
        FleetAppServerDescriptor, FleetPairingRequest, FleetPairingResponse,
        FleetTotpProvisioningResponse,
    },
};
use beale_storage::HostDatabase;
use prost::Message;
use ring::{
    hmac,
    rand::{SecureRandom, SystemRandom},
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{process::Command, time::timeout};

#[derive(Clone)]
struct HttpState {
    token: String,
    db: HostDatabase,
    attempts: Arc<Mutex<Attempts>>,
}

#[derive(Default)]
struct Attempts {
    failures: u32,
    retry_after: Option<Instant>,
    last_accepted_step: Option<u64>,
}

impl Attempts {
    fn redeem(
        &mut self,
        secret: &[u8],
        candidate: &str,
        now: Instant,
        unix_seconds: u64,
    ) -> Result<(), StatusCode> {
        if self.retry_after.is_some_and(|until| now < until) {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        let step = unix_seconds / 30;
        let matched = (step.saturating_sub(1)..=step.saturating_add(1)).find(|counter| {
            candidate.len() == 6
                && candidate.bytes().all(|byte| byte.is_ascii_digit())
                && totp(secret, *counter).is_some_and(|expected| {
                    expected
                        .as_bytes()
                        .iter()
                        .zip(candidate.as_bytes())
                        .fold(0_u8, |difference, (left, right)| {
                            difference | (left ^ right)
                        })
                        == 0
                })
        });
        if let Some(counter) =
            matched.filter(|counter| self.last_accepted_step.is_none_or(|used| *counter > used))
        {
            self.last_accepted_step = Some(counter);
            self.failures = 0;
            self.retry_after = None;
            return Ok(());
        }
        self.failures = self.failures.saturating_add(1);
        let seconds = 1_u64 << self.failures.saturating_sub(1).min(6);
        self.retry_after = Some(now + Duration::from_secs(seconds));
        Err(StatusCode::UNAUTHORIZED)
    }
}

fn totp(secret: &[u8], counter: u64) -> Option<String> {
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret);
    let tag = hmac::sign(&key, &counter.to_be_bytes());
    let bytes = tag.as_ref();
    let offset = usize::from(bytes.last()? & 0x0f);
    let chunk: [u8; 4] = bytes.get(offset..offset + 4)?.try_into().ok()?;
    let value = u32::from_be_bytes(chunk) & 0x7fff_ffff;
    Some(format!("{:06}", value % 1_000_000))
}

fn base32(secret: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut output = String::new();
    let mut bits = 0_u32;
    let mut count = 0_u8;
    for byte in secret {
        bits = (bits << 8) | u32::from(*byte);
        count += 8;
        while count >= 5 {
            count -= 5;
            output.push(char::from(ALPHABET[((bits >> count) & 31) as usize]));
        }
    }
    if count > 0 {
        output.push(char::from(ALPHABET[((bits << (5 - count)) & 31) as usize]));
    }
    output
}

pub fn router(token: String, db: HostDatabase) -> Router {
    Router::new()
        .route(FLEET_DESCRIPTOR_PATH, get(descriptor))
        .route(FLEET_TOTP_PROVISION_PATH, get(provision))
        .route(FLEET_PAIR_PATH, post(pair))
        .layer(DefaultBodyLimit::max(256))
        .with_state(HttpState {
            token,
            db,
            attempts: Arc::new(Mutex::new(Attempts::default())),
        })
}

fn authorized(headers: &HeaderMap, token: &str) -> bool {
    let Some(presented) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return false;
    };
    presented.len() == token.len()
        && presented
            .as_bytes()
            .iter()
            .zip(token.as_bytes())
            .fold(0_u8, |difference, (left, right)| {
                difference | (left ^ right)
            })
            == 0
}

fn enabled(state: &HttpState) -> Result<(), StatusCode> {
    if state
        .db
        .feature_enabled(crate::FEATURE_ID)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

async fn provision(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<([(header::HeaderName, &'static str); 2], Vec<u8>), StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    enabled(&state)?;
    let mut candidate = [0_u8; 20];
    SystemRandom::new()
        .fill(&mut candidate)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let secret = state
        .db
        .get_or_create_fleet_totp_secret(&candidate)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let host = host_name().await;
    let encoded_host: String = url::form_urlencoded::byte_serialize(host.as_bytes()).collect();
    let uri = format!(
        "otpauth://totp/Beale:{encoded_host}?secret={}&issuer=Beale&algorithm=SHA1&digits=6&period=30",
        base32(&secret)
    );
    Ok((
        [
            (header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE),
            (header::CACHE_CONTROL, "no-store"),
        ],
        FleetTotpProvisioningResponse { otpauth_uri: uri }.encode_to_vec(),
    ))
}

async fn pair(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<([(header::HeaderName, &'static str); 2], Vec<u8>), StatusCode> {
    enabled(&state)?;
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some(PROTOBUF_CONTENT_TYPE)
    {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    let request = FleetPairingRequest::decode(body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let secret = state
        .db
        .fleet_totp_secret()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let now = Instant::now();
    let unix_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .as_secs();
    state
        .attempts
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .redeem(&secret, &request.code, now, unix_seconds)?;
    Ok((
        [
            (header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE),
            (header::CACHE_CONTROL, "no-store"),
        ],
        FleetPairingResponse { token: state.token }.encode_to_vec(),
    ))
}

async fn descriptor(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<([(header::HeaderName, &'static str); 1], Vec<u8>), StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    enabled(&state)?;
    let (os_name, os_vendor) = match std::env::consts::OS {
        "windows" => ("Windows", "Microsoft"),
        "macos" => ("macOS", "Apple"),
        "linux" => ("Linux", "Linux"),
        _ => ("Other", "Other"),
    };
    let response = FleetAppServerDescriptor {
        service: SERVICE_NAME.to_owned(),
        host_name: host_name().await,
        os_name: os_name.to_owned(),
        os_vendor: os_vendor.to_owned(),
        protocol_major: PROTOCOL_MAJOR,
        protocol_minor: PROTOCOL_MINOR,
    };
    Ok((
        [(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)],
        response.encode_to_vec(),
    ))
}
async fn host_name() -> String {
    let from_environment = std::env::var(if cfg!(windows) {
        "COMPUTERNAME"
    } else {
        "HOSTNAME"
    })
    .ok();
    if let Some(name) = from_environment.filter(|name| valid_name(name)) {
        return name;
    }
    let output = timeout(
        Duration::from_secs(2),
        Command::new("hostname").kill_on_drop(true).output(),
    )
    .await;
    if let Ok(Ok(output)) = output {
        let name = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if valid_name(&name) {
            return name;
        }
    }
    "App Server".to_owned()
}

fn valid_name(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= 100
        && value.chars().all(|character| !character.is_control())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    #[test]
    fn totp_matches_rfc_vector_and_limits_replay_and_guesses() {
        let secret = b"12345678901234567890";
        assert_eq!(base32(secret), "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ");
        assert_eq!(totp(secret, 59 / 30).as_deref(), Some("287082"));
        let now = Instant::now();
        let mut attempts = Attempts::default();
        assert_eq!(
            attempts.redeem(secret, "000000", now, 59),
            Err(StatusCode::UNAUTHORIZED)
        );
        assert_eq!(
            attempts.redeem(secret, "287082", now, 59),
            Err(StatusCode::TOO_MANY_REQUESTS)
        );
        assert!(
            attempts
                .redeem(secret, "287082", now + Duration::from_secs(1), 59)
                .is_ok()
        );
        assert_eq!(
            attempts.redeem(secret, "287082", now + Duration::from_secs(2), 59),
            Err(StatusCode::UNAUTHORIZED)
        );
        assert_eq!(
            attempts.redeem(secret, "000000", now + Duration::from_secs(2), 89),
            Err(StatusCode::TOO_MANY_REQUESTS)
        );
        assert_eq!(
            attempts.redeem(secret, "000000", now + Duration::from_secs(3), 89),
            Err(StatusCode::UNAUTHORIZED)
        );
        assert_eq!(
            attempts.redeem(secret, "000000", now + Duration::from_secs(4), 89),
            Err(StatusCode::TOO_MANY_REQUESTS)
        );
    }

    #[tokio::test]
    async fn descriptor_requires_auth_and_fleet_enablement() {
        let root =
            std::env::temp_dir().join(format!("beale-fleet-example-{}", uuid::Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        let app = router("example-token".to_owned(), db.clone());
        let request = || {
            Request::builder()
                .uri(FLEET_DESCRIPTOR_PATH)
                .header("authorization", "Bearer example-token")
                .body(Body::empty())
                .unwrap()
        };
        let missing_token = Request::builder()
            .uri(FLEET_DESCRIPTOR_PATH)
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(missing_token).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            app.clone().oneshot(request()).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        let provision_request = |with_token| {
            let mut builder = Request::builder().uri(FLEET_TOTP_PROVISION_PATH);
            if with_token {
                builder = builder.header("authorization", "Bearer example-token");
            }
            builder.body(Body::empty()).unwrap()
        };
        assert_eq!(
            app.clone()
                .oneshot(provision_request(false))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            app.clone()
                .oneshot(provision_request(true))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert!(db.fleet_totp_secret().unwrap().is_none());
        db.set_feature_enabled(crate::FEATURE_ID, true).unwrap();
        let response = app.clone().oneshot(request()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let descriptor = FleetAppServerDescriptor::decode(
            axum::body::to_bytes(response.into_body(), 4_096)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(descriptor.service, SERVICE_NAME);
        assert!(!descriptor.host_name.is_empty());
        assert!(!descriptor.os_name.is_empty());
        assert!(!descriptor.os_vendor.is_empty());
        assert_eq!(descriptor.protocol_minor, PROTOCOL_MINOR);
        let provisioning = app.clone().oneshot(provision_request(true)).await.unwrap();
        assert_eq!(provisioning.status(), StatusCode::OK);
        let reply = FleetTotpProvisioningResponse::decode(
            axum::body::to_bytes(provisioning.into_body(), 4096)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(reply.otpauth_uri.starts_with("otpauth://totp/Beale:"));
        assert!(
            reply
                .otpauth_uri
                .contains("algorithm=SHA1&digits=6&period=30")
        );
        let secret = db.fleet_totp_secret().unwrap().unwrap();
        let counter = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            / 30;
        let code = totp(&secret, counter).unwrap();
        let pair_request = || {
            Request::builder()
                .method("POST")
                .uri(FLEET_PAIR_PATH)
                .header("content-type", PROTOBUF_CONTENT_TYPE)
                .body(Body::from(
                    FleetPairingRequest { code: code.clone() }.encode_to_vec(),
                ))
                .unwrap()
        };
        let paired = app.clone().oneshot(pair_request()).await.unwrap();
        assert_eq!(paired.status(), StatusCode::OK);
        let token = FleetPairingResponse::decode(
            axum::body::to_bytes(paired.into_body(), 4096)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(token.token, "example-token");
        assert_eq!(
            app.oneshot(pair_request()).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
}
