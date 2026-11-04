use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode, header},
    routing::{get, post},
};
use beale_contracts::{
    FLEET_BROWSER_PREFERRED_PATH, FLEET_BROWSER_REMOVE_PATH, FLEET_BROWSER_SETTINGS_PATH,
    FLEET_BROWSERS_PATH, FLEET_DESCRIPTOR_PATH, FLEET_PAIR_PATH, FLEET_TOTP_PROVISION_PATH,
    FLEET_VIRTUAL_MACHINE_SETTINGS_PATH, FLEET_VIRTUAL_MACHINES_PATH, PROTOBUF_CONTENT_TYPE,
    PROTOCOL_MAJOR, PROTOCOL_MINOR, SERVICE_NAME,
    v1::{
        FleetAddBrowserRequest, FleetAppServerDescriptor, FleetBrowsersResponse,
        FleetPairingRequest, FleetPairingResponse, FleetRemoveBrowserRequest,
        FleetSetBrowserSettingsRequest, FleetSetPreferredBrowserRequest,
        FleetSetVirtualMachineSettingsRequest, FleetTotpProvisioningResponse, FleetVirtualMachine,
        FleetVirtualMachinesResponse,
    },
};
use beale_storage::{BrowserSettings, HostDatabase, ManualBrowserRow, VirtualMachineSettings};
use prost::Message;
use ring::{
    hmac,
    rand::{SecureRandom, SystemRandom},
};
use std::{
    collections::HashSet,
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
        .route(FLEET_BROWSERS_PATH, get(list_browsers).post(add_browser))
        .route(FLEET_BROWSER_REMOVE_PATH, post(remove_browser))
        .route(FLEET_BROWSER_SETTINGS_PATH, post(set_browser_settings))
        .route(FLEET_BROWSER_PREFERRED_PATH, post(set_preferred_browser))
        .route(FLEET_VIRTUAL_MACHINES_PATH, get(list_virtual_machines))
        .route(
            FLEET_VIRTUAL_MACHINE_SETTINGS_PATH,
            post(set_virtual_machine_settings),
        )
        .layer(DefaultBodyLimit::max(4_096))
        .with_state(HttpState {
            token,
            db,
            attempts: Arc::new(Mutex::new(Attempts::default())),
        })
}

fn require_fleet_access(state: &HttpState, headers: &HeaderMap) -> Result<(), StatusCode> {
    if !authorized(headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    enabled(state)
}

fn protobuf_request(headers: &HeaderMap) -> Result<(), StatusCode> {
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some(PROTOBUF_CONTENT_TYPE)
    {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    Ok(())
}

async fn list_browsers(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<([(header::HeaderName, &'static str); 2], Vec<u8>), StatusCode> {
    require_fleet_access(&state, &headers)?;
    let manual = state
        .db
        .manual_browsers()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut browsers = crate::browsers::inventory(&manual);
    for browser in &mut browsers {
        let settings = state
            .db
            .browser_settings(&browser.id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        browser.enabled = settings.enabled;
        browser.all_workspaces = settings.all_workspaces;
        browser.workspace_ids = settings.workspace_ids;
    }
    let saved = state
        .db
        .preferred_browser_id()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let preferred = crate::browsers::preferred_id(&browsers, &manual, saved.as_deref());
    for browser in &mut browsers {
        browser.preferred = preferred.as_deref() == Some(browser.id.as_str());
    }
    let response = FleetBrowsersResponse { browsers };
    Ok((
        [
            (header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE),
            (header::CACHE_CONTROL, "no-store"),
        ],
        response.encode_to_vec(),
    ))
}

async fn list_virtual_machines(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<([(header::HeaderName, &'static str); 2], Vec<u8>), StatusCode> {
    require_fleet_access(&state, &headers)?;
    let mut virtual_machines = crate::virtual_machines::inventory()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    for machine in &mut virtual_machines {
        let settings = state
            .db
            .virtual_machine_settings(&machine.id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        machine.enabled = settings.enabled;
        machine.all_workspaces = settings.all_workspaces;
        machine.workspace_ids = settings.workspace_ids;
        machine.can_run = settings.can_run;
        machine.can_clone = settings.can_clone;
    }
    let response = FleetVirtualMachinesResponse { virtual_machines };
    Ok((
        [
            (header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE),
            (header::CACHE_CONTROL, "no-store"),
        ],
        response.encode_to_vec(),
    ))
}

async fn set_virtual_machine_settings(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    require_fleet_access(&state, &headers)?;
    protobuf_request(&headers)?;
    let request =
        FleetSetVirtualMachineSettingsRequest::decode(body).map_err(|_| StatusCode::BAD_REQUEST)?;
    if !valid_virtual_machine_settings_request(&request) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let inventory = crate::virtual_machines::inventory()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    save_virtual_machine_settings(&state.db, &inventory, request)
}

fn valid_virtual_machine_settings_request(request: &FleetSetVirtualMachineSettingsRequest) -> bool {
    request
        .id
        .strip_prefix("hyper-v:")
        .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
        && request.workspace_ids.len() <= 128
        && (!request.all_workspaces || request.workspace_ids.is_empty())
        && request
            .workspace_ids
            .iter()
            .all(|id| uuid::Uuid::parse_str(id).is_ok())
        && request.workspace_ids.iter().collect::<HashSet<_>>().len() == request.workspace_ids.len()
}

fn save_virtual_machine_settings(
    db: &HostDatabase,
    inventory: &[FleetVirtualMachine],
    request: FleetSetVirtualMachineSettingsRequest,
) -> Result<StatusCode, StatusCode> {
    if !inventory.iter().any(|machine| machine.id == request.id) {
        return Err(StatusCode::NOT_FOUND);
    }
    db.set_virtual_machine_settings(
        &request.id,
        &VirtualMachineSettings {
            enabled: request.enabled,
            all_workspaces: request.all_workspaces,
            workspace_ids: request.workspace_ids,
            can_run: request.can_run,
            can_clone: request.can_clone,
        },
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn add_browser(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    require_fleet_access(&state, &headers)?;
    protobuf_request(&headers)?;
    let request = FleetAddBrowserRequest::decode(body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let path = crate::browsers::validate_manual(&request.name, &request.executable)
        .ok_or(StatusCode::BAD_REQUEST)?;
    let manual = state
        .db
        .manual_browsers()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if crate::browsers::detected_path(&path)
        || manual.len() >= 32
        || manual
            .iter()
            .any(|entry| std::path::Path::new(&entry.executable) == path)
    {
        return Err(StatusCode::CONFLICT);
    }
    let row = ManualBrowserRow {
        id: uuid::Uuid::new_v4().to_string(),
        name: request.name.trim().to_owned(),
        executable: path
            .into_os_string()
            .into_string()
            .map_err(|_| StatusCode::BAD_REQUEST)?,
    };
    state
        .db
        .add_manual_browser(&row)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::CREATED)
}

async fn remove_browser(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    require_fleet_access(&state, &headers)?;
    protobuf_request(&headers)?;
    let request = FleetRemoveBrowserRequest::decode(body).map_err(|_| StatusCode::BAD_REQUEST)?;
    if uuid::Uuid::parse_str(&request.id).is_err() {
        return Err(StatusCode::BAD_REQUEST);
    }
    if state
        .db
        .remove_manual_browser(&request.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

async fn set_browser_settings(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    require_fleet_access(&state, &headers)?;
    protobuf_request(&headers)?;
    let request =
        FleetSetBrowserSettingsRequest::decode(body).map_err(|_| StatusCode::BAD_REQUEST)?;
    if request.workspace_ids.len() > 128
        || (request.all_workspaces && !request.workspace_ids.is_empty())
        || request
            .workspace_ids
            .iter()
            .any(|id| uuid::Uuid::parse_str(id).is_err())
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let manual = state
        .db
        .manual_browsers()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !crate::browsers::inventory(&manual)
        .iter()
        .any(|browser| browser.id == request.id)
    {
        return Err(StatusCode::NOT_FOUND);
    }
    state
        .db
        .set_browser_settings(
            &request.id,
            &BrowserSettings {
                enabled: request.enabled,
                all_workspaces: request.all_workspaces,
                workspace_ids: request.workspace_ids,
            },
        )
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn set_preferred_browser(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    require_fleet_access(&state, &headers)?;
    protobuf_request(&headers)?;
    let request =
        FleetSetPreferredBrowserRequest::decode(body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let manual = state
        .db
        .manual_browsers()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let available = crate::browsers::inventory(&manual)
        .iter()
        .any(|browser| browser.id == request.id && browser.available);
    if !available {
        return Err(StatusCode::NOT_FOUND);
    }
    if !state
        .db
        .browser_settings(&request.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .enabled
    {
        return Err(StatusCode::CONFLICT);
    }
    state
        .db
        .set_preferred_browser_id(&request.id)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok(StatusCode::NO_CONTENT)
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
    fn virtual_machine_settings_require_a_detected_id_and_persist_allowances() {
        let root = std::env::temp_dir().join(format!("beale-vm-example-{}", uuid::Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        let id = "hyper-v:00000000-0000-4000-8000-000000000001".to_owned();
        let inventory = vec![FleetVirtualMachine {
            id: id.clone(),
            name: "Example VM".to_owned(),
            provider: "Hyper-V".to_owned(),
            state: "Off".to_owned(),
            generation: 2,
            enabled: true,
            all_workspaces: true,
            workspace_ids: Vec::new(),
            can_run: false,
            can_clone: false,
        }];
        assert_eq!(
            db.virtual_machine_settings(&id).unwrap(),
            VirtualMachineSettings::default()
        );
        let request = FleetSetVirtualMachineSettingsRequest {
            id: id.clone(),
            enabled: true,
            all_workspaces: false,
            workspace_ids: vec![uuid::Uuid::nil().to_string()],
            can_run: true,
            can_clone: false,
        };
        assert!(valid_virtual_machine_settings_request(&request));
        assert_eq!(
            save_virtual_machine_settings(&db, &[], request.clone()),
            Err(StatusCode::NOT_FOUND)
        );
        assert_eq!(
            save_virtual_machine_settings(&db, &inventory, request.clone()),
            Ok(StatusCode::NO_CONTENT)
        );
        let mut invalid = request.clone();
        invalid.workspace_ids.push(uuid::Uuid::nil().to_string());
        assert!(!valid_virtual_machine_settings_request(&invalid));
        drop(db);
        let reopened = HostDatabase::open(&root).unwrap();
        let saved = reopened.virtual_machine_settings(&id).unwrap();
        assert!(saved.enabled && saved.can_run && !saved.can_clone);
        assert!(!saved.all_workspaces);
        assert_eq!(saved.workspace_ids, request.workspace_ids);
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn browser_inventory_requires_auth_and_persists_manual_entries() {
        let root =
            std::env::temp_dir().join(format!("beale-browser-example-{}", uuid::Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        let app = router("example-token".to_owned(), db.clone());
        let request = |method: &str, path: &str, body: Vec<u8>, authenticated: bool| {
            let mut builder = Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", PROTOBUF_CONTENT_TYPE);
            if authenticated {
                builder = builder.header("authorization", "Bearer example-token");
            }
            builder.body(Body::from(body)).unwrap()
        };
        assert_eq!(
            app.clone()
                .oneshot(request("GET", FLEET_BROWSERS_PATH, vec![], false))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            app.clone()
                .oneshot(request("GET", FLEET_BROWSERS_PATH, vec![], true))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            app.clone()
                .oneshot(request("GET", FLEET_VIRTUAL_MACHINES_PATH, vec![], false))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            app.clone()
                .oneshot(request("GET", FLEET_VIRTUAL_MACHINES_PATH, vec![], true))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            app.clone()
                .oneshot(request(
                    "POST",
                    FLEET_VIRTUAL_MACHINE_SETTINGS_PATH,
                    vec![],
                    false,
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            app.clone()
                .oneshot(request(
                    "POST",
                    FLEET_VIRTUAL_MACHINE_SETTINGS_PATH,
                    vec![],
                    true,
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        db.set_feature_enabled(crate::FEATURE_ID, true).unwrap();
        assert_eq!(
            app.clone()
                .oneshot(request(
                    "POST",
                    FLEET_BROWSERS_PATH,
                    FleetAddBrowserRequest {
                        name: "Example Browser".to_owned(),
                        executable: "relative/browser".to_owned()
                    }
                    .encode_to_vec(),
                    true
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );

        let executable = root.join(if cfg!(windows) {
            "example-browser.exe"
        } else {
            "example-browser"
        });
        std::fs::write(&executable, b"example").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let add = FleetAddBrowserRequest {
            name: "Example Browser".to_owned(),
            executable: executable.to_string_lossy().into_owned(),
        };
        assert_eq!(
            app.clone()
                .oneshot(request(
                    "POST",
                    FLEET_BROWSERS_PATH,
                    add.encode_to_vec(),
                    true
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
        let listed = app
            .clone()
            .oneshot(request("GET", FLEET_BROWSERS_PATH, vec![], true))
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(listed.into_body(), 4096)
            .await
            .unwrap();
        let inventory = FleetBrowsersResponse::decode(bytes).unwrap();
        let manual = inventory
            .browsers
            .iter()
            .find(|browser| browser.name == "Example Browser")
            .unwrap();
        assert!(manual.manual && manual.available && manual.enabled && manual.all_workspaces);
        let id = manual.id.clone();
        assert!(!format!("{inventory:?}").contains(&executable.to_string_lossy().to_string()));

        let preference = FleetSetPreferredBrowserRequest { id: id.clone() };
        assert_eq!(
            app.clone()
                .oneshot(request(
                    "POST",
                    FLEET_BROWSER_PREFERRED_PATH,
                    preference.encode_to_vec(),
                    false
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            app.clone()
                .oneshot(request(
                    "POST",
                    FLEET_BROWSER_PREFERRED_PATH,
                    preference.encode_to_vec(),
                    true
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
        let listed = app
            .clone()
            .oneshot(request("GET", FLEET_BROWSERS_PATH, vec![], true))
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(listed.into_body(), 4096)
            .await
            .unwrap();
        let inventory = FleetBrowsersResponse::decode(bytes).unwrap();
        assert!(
            inventory
                .browsers
                .iter()
                .find(|browser| browser.id == id)
                .unwrap()
                .preferred
        );

        let workspace_id = uuid::Uuid::nil().to_string();
        let settings = FleetSetBrowserSettingsRequest {
            id: id.clone(),
            enabled: false,
            all_workspaces: false,
            workspace_ids: vec![workspace_id.clone()],
        };
        assert_eq!(
            app.clone()
                .oneshot(request(
                    "POST",
                    FLEET_BROWSER_SETTINGS_PATH,
                    settings.encode_to_vec(),
                    false
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            app.clone()
                .oneshot(request(
                    "POST",
                    FLEET_BROWSER_SETTINGS_PATH,
                    settings.encode_to_vec(),
                    true
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );

        drop(app);
        drop(db);
        let reopened = HostDatabase::open(&root).unwrap();
        assert_eq!(reopened.manual_browsers().unwrap().len(), 1);
        assert_eq!(
            reopened.preferred_browser_id().unwrap().as_deref(),
            Some(id.as_str())
        );
        assert_eq!(
            reopened.browser_settings(&id).unwrap(),
            BrowserSettings {
                enabled: false,
                all_workspaces: false,
                workspace_ids: vec![workspace_id],
            }
        );
        let app = router("example-token".to_owned(), reopened);
        let listed = app
            .clone()
            .oneshot(request("GET", FLEET_BROWSERS_PATH, vec![], true))
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(listed.into_body(), 4096)
            .await
            .unwrap();
        let inventory = FleetBrowsersResponse::decode(bytes).unwrap();
        let manual = inventory
            .browsers
            .iter()
            .find(|browser| browser.id == id)
            .unwrap();
        assert!(!manual.enabled && !manual.all_workspaces && manual.workspace_ids.len() == 1);
        assert!(!manual.preferred);
        let remove = FleetRemoveBrowserRequest { id: id.clone() }.encode_to_vec();
        assert_eq!(
            app.oneshot(request("POST", FLEET_BROWSER_REMOVE_PATH, remove, true))
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
        let reopened = HostDatabase::open(&root).unwrap();
        assert_eq!(
            reopened.browser_settings(&id).unwrap().workspace_ids.len(),
            0
        );
        assert!(reopened.preferred_browser_id().unwrap().is_none());
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

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
