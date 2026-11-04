//! Desktop-owned app-server connections. Tokens never cross the local app-server API.

use beale_contracts::{
    FLEET_BROWSER_PREFERRED_PATH, FLEET_BROWSER_REMOVE_PATH, FLEET_BROWSER_SETTINGS_PATH,
    FLEET_BROWSERS_PATH, FLEET_DESCRIPTOR_PATH, FLEET_PAIR_PATH, FLEET_TOTP_PROVISION_PATH,
    FLEET_VIRTUAL_MACHINE_SETTINGS_PATH, FLEET_VIRTUAL_MACHINES_PATH, PROTOBUF_CONTENT_TYPE,
    PROTOCOL_MAJOR, PROTOCOL_MINOR, SERVICE_NAME,
    v1::{
        FleetAddBrowserRequest, FleetAppServerDescriptor, FleetBrowser, FleetBrowsersResponse,
        FleetPairingRequest, FleetPairingResponse, FleetRemoveBrowserRequest,
        FleetSetBrowserSettingsRequest, FleetSetPreferredBrowserRequest,
        FleetSetVirtualMachineSettingsRequest, FleetTotpProvisioningResponse, FleetVirtualMachine,
        FleetVirtualMachinesResponse,
    },
};
use futures_util::future::join_all;
use prost::Message;
use reqwest::{Client, header::CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{process::Command, time::timeout};
use url::Url;

const MAX_CONNECTIONS: usize = 32;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct FleetServer {
    pub(crate) id: String,
    pub(crate) endpoint: String,
    token: String,
    pub(crate) host_name: String,
    pub(crate) display_name: String,
    pub(crate) os_name: String,
    pub(crate) os_vendor: String,
    pub(crate) protocol_major: u32,
    pub(crate) protocol_minor: u32,
    pub(crate) enabled: bool,
    #[serde(skip)]
    pub(crate) condition: String,
    #[serde(skip)]
    pub(crate) condition_detail: String,
}

#[derive(Serialize, Deserialize)]
struct SavedConnections {
    version: u32,
    #[serde(default)]
    local_display_name: String,
    servers: Vec<FleetServer>,
}

pub(crate) struct FleetSnapshot {
    pub(crate) local: FleetServer,
    pub(crate) servers: Vec<FleetServer>,
}

#[derive(Clone)]
pub(crate) struct BrowserItem {
    pub(crate) host_id: String,
    pub(crate) host_name: String,
    pub(crate) browser: FleetBrowser,
}

pub(crate) struct BrowserSnapshot {
    pub(crate) items: Vec<BrowserItem>,
    pub(crate) servers: Vec<FleetServer>,
    pub(crate) error: String,
}

#[derive(Clone)]
pub(crate) struct VirtualMachineItem {
    pub(crate) host_id: String,
    pub(crate) host_name: String,
    pub(crate) machine: FleetVirtualMachine,
}

pub(crate) struct VirtualMachineSnapshot {
    pub(crate) items: Vec<VirtualMachineItem>,
    pub(crate) servers: Vec<FleetServer>,
    pub(crate) error: String,
}

#[derive(Clone)]
pub(crate) struct VirtualMachineMutation {
    pub(crate) host_id: String,
    pub(crate) id: String,
    pub(crate) enabled: bool,
    pub(crate) all_workspaces: bool,
    pub(crate) workspace_ids: Vec<String>,
    pub(crate) can_run: bool,
    pub(crate) can_clone: bool,
}

pub(crate) enum BrowserMutation {
    Add {
        host_id: String,
        name: String,
        executable: String,
    },
    Remove {
        host_id: String,
        id: String,
    },
    SetSettings {
        host_id: String,
        id: String,
        enabled: bool,
        all_workspaces: bool,
        workspace_ids: Vec<String>,
    },
    SetPreferred {
        host_id: String,
        id: String,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct TailnetDevice {
    pub(crate) dns_name: String,
    pub(crate) host_name: String,
    pub(crate) os_name: String,
    pub(crate) online: bool,
}

pub(crate) enum Mutation {
    Connect {
        endpoint: String,
        code: String,
        display_name: String,
    },
    Update {
        id: String,
        enabled: bool,
        display_name: String,
    },
    Remove {
        id: String,
    },
    UpdateLocalName {
        display_name: String,
    },
}

pub(crate) enum MutationResult {
    Saved(Box<FleetServer>),
    Removed(String),
    LocalName(String),
}

fn preference_path() -> Result<PathBuf, String> {
    let root = if let Some(path) = std::env::var_os("BEALE_V2_HOME") {
        PathBuf::from(path)
    } else {
        PathBuf::from(
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .ok_or("Home directory is unavailable.")?,
        )
        .join(".beale")
        .join("v2")
    };
    Ok(root.join("desktop-fleet.json"))
}

fn valid_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= max
        && value.chars().all(|character| !character.is_control())
}

pub(crate) fn normalize_endpoint(value: &str) -> Result<(String, String, u16), String> {
    let url = Url::parse(value.trim()).map_err(|_| "Enter a Tailscale HTTPS URL.".to_owned())?;
    let host = url.host_str().ok_or("Enter a Tailscale HTTPS URL.")?;
    let valid_host = host.ends_with(".ts.net")
        && host.split('.').all(|part| {
            !part.is_empty()
                && !part.starts_with('-')
                && !part.ends_with('-')
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        });
    if url.scheme() != "https"
        || !valid_host
        || url.username() != ""
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Enter a Tailscale HTTPS origin ending in .ts.net.".to_owned());
    }
    let port = url
        .port_or_known_default()
        .ok_or("Invalid Tailscale port.")?;
    Ok((url.origin().ascii_serialization(), host.to_owned(), port))
}

fn valid_server(server: &FleetServer) -> bool {
    uuid::Uuid::parse_str(&server.id).is_ok()
        && normalize_endpoint(&server.endpoint)
            .is_ok_and(|(endpoint, _, _)| endpoint == server.endpoint)
        && server.token.len() == 64
        && server.token.bytes().all(|byte| byte.is_ascii_hexdigit())
        && valid_text(&server.host_name, 100)
        && (server.display_name.is_empty() || valid_text(&server.display_name, 80))
        && valid_text(&server.os_name, 40)
        && valid_text(&server.os_vendor, 40)
        && server.protocol_major > 0
}

fn read_at(path: &Path) -> Result<SavedConnections, String> {
    let bytes = match std::fs::metadata(path) {
        Ok(metadata) if metadata.len() <= 65_536 => {
            std::fs::read(path).map_err(|_| "Could not read saved Fleet connections.".to_owned())?
        }
        Ok(_) => return Err("Saved Fleet connections are too large.".to_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SavedConnections {
                version: 1,
                local_display_name: String::new(),
                servers: Vec::new(),
            });
        }
        Err(_) => return Err("Could not read saved Fleet connections.".to_owned()),
    };
    let saved: SavedConnections = serde_json::from_slice(&bytes)
        .map_err(|_| "Saved Fleet connections are invalid.".to_owned())?;
    if saved.version != 1
        || (!saved.local_display_name.is_empty() && !valid_text(&saved.local_display_name, 80))
        || saved.servers.len() > MAX_CONNECTIONS
        || !saved.servers.iter().all(valid_server)
        || saved
            .servers
            .iter()
            .map(|server| &server.id)
            .collect::<HashSet<_>>()
            .len()
            != saved.servers.len()
        || saved
            .servers
            .iter()
            .map(|server| &server.endpoint)
            .collect::<HashSet<_>>()
            .len()
            != saved.servers.len()
    {
        return Err("Saved Fleet connections are invalid.".to_owned());
    }
    Ok(SavedConnections {
        version: 1,
        local_display_name: saved.local_display_name,
        servers: saved
            .servers
            .into_iter()
            .map(|mut server| {
                server.condition = if server.enabled {
                    "Checking"
                } else {
                    "Disabled"
                }
                .to_owned();
                server.condition_detail = if server.enabled {
                    "Checking connection."
                } else {
                    "Connection disabled."
                }
                .to_owned();
                server
            })
            .collect(),
    })
}

fn write_at(path: &Path, saved: &SavedConnections) -> Result<(), String> {
    let parent = path.parent().ok_or("Fleet preferences path is invalid.")?;
    std::fs::create_dir_all(parent)
        .map_err(|_| "Could not create Fleet preferences directory.".to_owned())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| "Could not protect Fleet preferences directory.".to_owned())?;
    }
    let bytes =
        serde_json::to_vec(saved).map_err(|_| "Could not encode Fleet preferences.".to_owned())?;
    let temporary = parent.join(format!("desktop-fleet-{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&temporary, bytes)
        .map_err(|_| "Could not save Fleet preferences.".to_owned())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))
            .map_err(|_| "Could not protect Fleet preferences.".to_owned())?;
    }
    #[cfg(windows)]
    {
        let user =
            std::env::var("USERNAME").map_err(|_| "Windows user is unavailable.".to_owned())?;
        let domain =
            std::env::var("USERDOMAIN").map_err(|_| "Windows domain is unavailable.".to_owned())?;
        let status = std::process::Command::new("icacls")
            .arg(&temporary)
            .arg("/inheritance:r")
            .arg("/grant:r")
            .arg(format!("{domain}\\{user}:F"))
            .output()
            .map_err(|_| "Could not protect Fleet preferences.".to_owned())?;
        if !status.status.success() {
            return Err("Could not protect Fleet preferences.".to_owned());
        }
    }
    std::fs::rename(&temporary, path).map_err(|_| "Could not replace Fleet preferences.".to_owned())
}

fn read_saved() -> Result<SavedConnections, String> {
    read_at(&preference_path()?)
}

#[derive(Deserialize)]
struct TailnetStatus {
    #[serde(rename = "BackendState")]
    backend_state: String,
    #[serde(rename = "Self")]
    self_peer: TailnetPeer,
    #[serde(rename = "Peer", default)]
    peers: HashMap<String, TailnetPeer>,
}

#[derive(Deserialize)]
struct TailnetPeer {
    #[serde(rename = "DNSName", default)]
    dns_name: String,
    #[serde(rename = "HostName", default)]
    host_name: String,
    #[serde(rename = "OS", default)]
    os_name: String,
    #[serde(rename = "Online", default)]
    online: bool,
    #[serde(rename = "TailscaleIPs", default)]
    ips: Vec<String>,
}

fn devices_from_status(status: &TailnetStatus) -> Result<Vec<TailnetDevice>, String> {
    if status.backend_state != "Running" {
        return Err("Tailscale is not connected.".to_owned());
    }
    let self_name = status
        .self_peer
        .dns_name
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if self_name.len() > 253 {
        return Err("Tailscale MagicDNS is unavailable.".to_owned());
    }
    normalize_endpoint(&format!("https://{self_name}"))
        .map_err(|_| "Tailscale MagicDNS is unavailable.".to_owned())?;
    let (_, tailnet) = self_name
        .split_once('.')
        .ok_or("Tailscale MagicDNS is unavailable.")?;
    let mut devices = status
        .peers
        .values()
        .filter_map(|peer| {
            let dns_name = peer.dns_name.trim_end_matches('.').to_ascii_lowercase();
            if dns_name.len() > 253
                || dns_name.split_once('.').map(|(_, suffix)| suffix) != Some(tailnet)
                || normalize_endpoint(&format!("https://{dns_name}")).is_err()
            {
                return None;
            }
            let host_name = if valid_text(&peer.host_name, 100) {
                peer.host_name.clone()
            } else {
                dns_name.split('.').next()?.to_owned()
            };
            let os_name = match peer.os_name.to_ascii_lowercase().as_str() {
                "windows" => "Windows".to_owned(),
                "darwin" | "macos" => "macOS".to_owned(),
                "linux" => "Linux".to_owned(),
                "ios" => "iOS".to_owned(),
                "android" => "Android".to_owned(),
                _ if valid_text(&peer.os_name, 40) => peer.os_name.clone(),
                _ => "Other".to_owned(),
            };
            Some(TailnetDevice {
                dns_name,
                host_name,
                os_name,
                online: peer.online,
            })
        })
        .collect::<Vec<_>>();
    devices.sort_by(|left, right| {
        right.online.cmp(&left.online).then_with(|| {
            left.host_name
                .to_ascii_lowercase()
                .cmp(&right.host_name.to_ascii_lowercase())
        })
    });
    let mut seen = HashSet::new();
    devices.retain(|device| seen.insert(device.dns_name.clone()));
    devices.truncate(128);
    Ok(devices)
}

pub(crate) async fn import_devices() -> Result<Vec<TailnetDevice>, String> {
    devices_from_status(&tailnet_status().await?)
}

pub(crate) fn imported_endpoint(device: &TailnetDevice, port_text: &str) -> Result<String, String> {
    let port = port_text
        .trim()
        .parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .ok_or("Enter a valid HTTPS port.")?;
    normalize_endpoint(&format!("https://{}:{port}", device.dns_name))
        .map(|(endpoint, _, _)| endpoint)
}

fn peer_ip(status: &TailnetStatus, host: &str) -> Result<IpAddr, String> {
    if status.backend_state != "Running" {
        return Err("Tailscale is not connected.".to_owned());
    }
    let self_name = status
        .self_peer
        .dns_name
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let tailnet = self_name
        .split_once('.')
        .map(|(_, tailnet)| tailnet)
        .ok_or("Tailscale MagicDNS is unavailable.")?;
    if !self_name.ends_with(".ts.net")
        || host.split_once('.').map(|(_, name)| name) != Some(tailnet)
    {
        return Err("App server is outside this Tailscale network.".to_owned());
    }
    let peer = status
        .peers
        .values()
        .find(|peer| {
            peer.dns_name
                .trim_end_matches('.')
                .eq_ignore_ascii_case(host)
        })
        .ok_or("App server is not a Tailscale peer.")?;
    if !peer.online {
        return Err("App server is offline on Tailscale.".to_owned());
    }
    peer.ips
        .iter()
        .find_map(|value| value.parse().ok())
        .ok_or("Tailscale peer address is unavailable.".to_owned())
}

async fn tailnet_status() -> Result<TailnetStatus, String> {
    let mut candidates = Vec::new();
    if let Some(command) = std::env::var_os("BEALE_TAILSCALE_COMMAND") {
        candidates.push(PathBuf::from(command));
    }
    #[cfg(target_os = "windows")]
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        candidates.push(
            PathBuf::from(program_files)
                .join("Tailscale")
                .join("tailscale.exe"),
        );
    }
    #[cfg(target_os = "macos")]
    candidates.extend(
        [
            "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
            "/opt/homebrew/bin/tailscale",
            "/usr/local/bin/tailscale",
        ]
        .into_iter()
        .map(PathBuf::from),
    );
    candidates.push(PathBuf::from(if cfg!(windows) {
        "tailscale.exe"
    } else {
        "tailscale"
    }));
    for candidate in candidates {
        let result = timeout(
            Duration::from_secs(5),
            Command::new(candidate)
                .args(["status", "--json"])
                .kill_on_drop(true)
                .output(),
        )
        .await;
        match result {
            Ok(Ok(output)) if output.status.success() && output.stdout.len() <= 2_000_000 => {
                return serde_json::from_slice(&output.stdout)
                    .map_err(|_| "Tailscale status is invalid.".to_owned());
            }
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => continue,
            _ => break,
        }
    }
    Err("Tailscale status is unavailable.".to_owned())
}

async fn read_bounded(mut response: reqwest::Response) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "App-server response was interrupted.".to_owned())?
    {
        if bytes.len() + chunk.len() > 8_192 {
            return Err("App-server response is too large.".to_owned());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn remote_client(endpoint: &str, status: &TailnetStatus) -> Result<Client, String> {
    let (_, host, port) = normalize_endpoint(endpoint)?;
    let address = SocketAddr::new(peer_ip(status, &host)?, port);
    Client::builder()
        .resolve(&host, address)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| "Could not initialize Tailscale connection.".to_owned())
}

async fn exchange_authenticator_code(
    endpoint: &str,
    code: &str,
    status: &TailnetStatus,
) -> Result<String, String> {
    let client = remote_client(endpoint, status)?;
    let response = client
        .post(format!("{endpoint}{FLEET_PAIR_PATH}"))
        .header(CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
        .body(
            FleetPairingRequest {
                code: code.to_owned(),
            }
            .encode_to_vec(),
        )
        .send()
        .await
        .map_err(|_| "Could not reach app server over Tailscale.".to_owned())?;
    match response.status() {
        reqwest::StatusCode::UNAUTHORIZED => {
            return Err("Authenticator code is invalid or already used.".to_owned());
        }
        reqwest::StatusCode::TOO_MANY_REQUESTS => {
            return Err("Too many failed codes. Wait before retrying.".to_owned());
        }
        reqwest::StatusCode::FORBIDDEN => {
            return Err("Fleet is disabled on the remote app server.".to_owned());
        }
        reqwest::StatusCode::OK => {}
        _ => return Err("App-server pairing failed.".to_owned()),
    }
    if response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some(PROTOBUF_CONTENT_TYPE)
    {
        return Err("Invalid app-server pairing response.".to_owned());
    }
    let reply = FleetPairingResponse::decode(read_bounded(response).await?.as_slice())
        .map_err(|_| "Invalid app-server pairing response.".to_owned())?;
    if reply.token.len() != 64 || !reply.token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Invalid app-server pairing response.".to_owned());
    }
    Ok(reply.token)
}

pub(crate) async fn provisioning_uri() -> Result<String, String> {
    let addr = crate::connection::configured_addr()
        .map_err(|_| "Invalid local app-server address.".to_owned())?;
    let client = Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| "Could not initialize local app-server connection.".to_owned())?;
    let token = crate::provider_auth::authenticated_token(&client, addr).await?;
    let response = client
        .get(format!("http://{addr}{FLEET_TOTP_PROVISION_PATH}"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| "Could not load the authenticator QR code.".to_owned())?;
    if response.status() != reqwest::StatusCode::OK
        || response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            != Some(PROTOBUF_CONTENT_TYPE)
    {
        return Err("Could not load the authenticator QR code.".to_owned());
    }
    let reply = FleetTotpProvisioningResponse::decode(read_bounded(response).await?.as_slice())
        .map_err(|_| "Invalid authenticator QR response.".to_owned())?;
    if reply.otpauth_uri.len() > 300 || !reply.otpauth_uri.starts_with("otpauth://totp/Beale:") {
        return Err("Invalid authenticator QR response.".to_owned());
    }
    Ok(reply.otpauth_uri)
}

async fn probe(server: &mut FleetServer, status: &TailnetStatus) -> Result<(), String> {
    let client = remote_client(&server.endpoint, status)?;
    let response = client
        .get(format!("{}{FLEET_DESCRIPTOR_PATH}", server.endpoint))
        .bearer_auth(&server.token)
        .send()
        .await
        .map_err(|_| "Could not reach app server over Tailscale.".to_owned())?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        server.condition = "Issue".to_owned();
        return Err("App-server authentication failed.".to_owned());
    }
    if response.status() == reqwest::StatusCode::FORBIDDEN {
        server.condition = "Issue".to_owned();
        return Err("Fleet is disabled on the remote app server.".to_owned());
    }
    if response.status() != reqwest::StatusCode::OK
        || response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            != Some(PROTOBUF_CONTENT_TYPE)
    {
        server.condition = "Issue".to_owned();
        return Err("Remote app-server descriptor is unavailable.".to_owned());
    }
    let descriptor = FleetAppServerDescriptor::decode(read_bounded(response).await?.as_slice())
        .map_err(|_| "Remote app-server descriptor is invalid.".to_owned())?;
    if descriptor.service != SERVICE_NAME
        || descriptor.protocol_major != PROTOCOL_MAJOR
        || descriptor.protocol_minor < PROTOCOL_MINOR
        || !valid_text(&descriptor.host_name, 100)
        || !valid_text(&descriptor.os_name, 40)
        || !valid_text(&descriptor.os_vendor, 40)
    {
        server.condition = "Issue".to_owned();
        return Err("Remote app-server contract is incompatible.".to_owned());
    }
    server.host_name = descriptor.host_name;
    server.os_name = descriptor.os_name;
    server.os_vendor = descriptor.os_vendor;
    server.protocol_major = descriptor.protocol_major;
    server.protocol_minor = descriptor.protocol_minor;
    server.condition = "Online".to_owned();
    server.condition_detail = "Connected to a compatible app server.".to_owned();
    Ok(())
}

fn local_os() -> (&'static str, &'static str) {
    match std::env::consts::OS {
        "windows" => ("Windows", "Microsoft"),
        "macos" => ("macOS", "Apple"),
        "linux" => ("Linux", "Linux"),
        _ => ("Other", "Other"),
    }
}

pub(crate) fn local_placeholder() -> FleetServer {
    let (os_name, os_vendor) = local_os();
    let host_name = std::env::var(if cfg!(windows) {
        "COMPUTERNAME"
    } else {
        "HOSTNAME"
    })
    .ok()
    .filter(|name| valid_text(name, 100))
    .unwrap_or_else(|| "Local App Server".to_owned());
    FleetServer {
        id: "local".to_owned(),
        endpoint: String::new(),
        token: String::new(),
        host_name,
        display_name: String::new(),
        os_name: os_name.to_owned(),
        os_vendor: os_vendor.to_owned(),
        protocol_major: PROTOCOL_MAJOR,
        protocol_minor: PROTOCOL_MINOR,
        enabled: true,
        condition: "Checking".to_owned(),
        condition_detail: "Local app-server connection.".to_owned(),
    }
}

fn local_endpoint(status: &TailnetStatus) -> Option<String> {
    if status.backend_state != "Running" {
        return None;
    }
    let host = status
        .self_peer
        .dns_name
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if !host.ends_with(".ts.net") || host.split('.').any(|part| part.is_empty()) {
        return None;
    }
    let port = std::env::var("BEALE_V2_FLEET_HTTPS_PORT")
        .ok()
        .map(|value| value.parse::<u16>().ok().filter(|port| *port > 0))
        .unwrap_or(Some(43832))?;
    Some(format!("https://{host}:{port}"))
}

pub(crate) async fn load() -> Result<FleetSnapshot, String> {
    let saved = read_saved()?;
    let status = tailnet_status().await;
    let mut local = local_placeholder();
    local.display_name = saved.local_display_name;
    if let Ok(Ok(output)) = timeout(
        Duration::from_secs(2),
        Command::new("hostname").kill_on_drop(true).output(),
    )
    .await
    {
        let name = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if valid_text(&name, 100) {
            local.host_name = name;
        }
    }
    if let Ok(status) = &status {
        local.endpoint = local_endpoint(status).unwrap_or_default();
    }
    let servers = join_all(saved.servers.into_iter().map(|mut server| {
        let status = &status;
        async move {
            if server.enabled {
                match status {
                    Ok(status) => {
                        if let Err(error) = probe(&mut server, status).await {
                            if server.condition != "Issue" {
                                server.condition = "Offline".to_owned();
                            }
                            server.condition_detail = error;
                        }
                    }
                    Err(error) => {
                        server.condition = "Offline".to_owned();
                        server.condition_detail = error.clone();
                    }
                }
            }
            server
        }
    }))
    .await;
    Ok(FleetSnapshot { local, servers })
}

async fn local_browser_client() -> Result<(Client, String, String), String> {
    let addr = crate::connection::configured_addr()
        .map_err(|_| "Invalid local app-server address.".to_owned())?;
    let client = Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| "Could not initialize local app-server connection.".to_owned())?;
    if !matches!(
        crate::connection::probe(&client, addr).await,
        crate::ConnectionStatus::Connected { .. }
    ) {
        return Err("Local app-server needs the current Beale protocol.".to_owned());
    }
    let token = crate::provider_auth::authenticated_token(&client, addr).await?;
    Ok((client, format!("http://{addr}"), token))
}

async fn browser_client(host_id: &str) -> Result<(Client, String, String), String> {
    if host_id == "local" {
        return local_browser_client().await;
    }
    let mut server = read_saved()?
        .servers
        .into_iter()
        .find(|server| server.id == host_id && server.enabled)
        .ok_or("App server is unavailable.")?;
    let status = tailnet_status().await?;
    probe(&mut server, &status).await?;
    let client = remote_client(&server.endpoint, &status)?;
    Ok((client, server.endpoint, server.token))
}

async fn fetch_browsers(
    client: &Client,
    base: &str,
    token: &str,
) -> Result<Vec<FleetBrowser>, String> {
    let response = client
        .get(format!("{base}{FLEET_BROWSERS_PATH}"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| "Could not reach app-server browser inventory.".to_owned())?;
    if !response.status().is_success()
        || response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            != Some(PROTOBUF_CONTENT_TYPE)
    {
        return Err("App-server browser inventory is unavailable.".to_owned());
    }
    let response = FleetBrowsersResponse::decode(read_bounded(response).await?.as_slice())
        .map_err(|_| "Invalid app-server browser inventory.".to_owned())?;
    if response.browsers.len() > 64
        || response
            .browsers
            .iter()
            .filter(|browser| browser.preferred)
            .count()
            > 1
        || response.browsers.iter().any(|browser| {
            !valid_text(&browser.id, 80)
                || !valid_text(&browser.name, 80)
                || browser.workspace_ids.len() > 128
                || (browser.all_workspaces && !browser.workspace_ids.is_empty())
                || browser
                    .workspace_ids
                    .iter()
                    .any(|id| uuid::Uuid::parse_str(id).is_err())
                || browser.workspace_ids.iter().collect::<HashSet<_>>().len()
                    != browser.workspace_ids.len()
                || (browser.preferred && (!browser.enabled || !browser.available))
        })
    {
        return Err("Invalid app-server browser inventory.".to_owned());
    }
    Ok(response.browsers)
}

pub(crate) async fn load_browsers() -> BrowserSnapshot {
    let mut items = Vec::new();
    let mut errors = Vec::new();
    let mut servers = Vec::new();
    match local_browser_client().await {
        Ok((client, base, token)) => match fetch_browsers(&client, &base, &token).await {
            Ok(browsers) => items.extend(browsers.into_iter().map(|browser| BrowserItem {
                host_id: "local".to_owned(),
                host_name: "Local App Server".to_owned(),
                browser,
            })),
            Err(error) => errors.push(error),
        },
        Err(error) => errors.push(error),
    }
    match read_saved() {
        Ok(saved) => {
            servers = saved.servers;
            let enabled = servers
                .iter()
                .filter(|server| server.enabled)
                .collect::<Vec<_>>();
            let status = if enabled.is_empty() {
                None
            } else {
                Some(tailnet_status().await)
            };
            let remote = join_all(enabled.into_iter().map(|server| {
                let status = status.as_ref();
                async move {
                    let result = match status {
                        Some(Ok(status)) => {
                            let mut verified = server.clone();
                            match probe(&mut verified, status).await {
                                Ok(()) => match remote_client(&server.endpoint, status) {
                                    Ok(client) => {
                                        fetch_browsers(&client, &server.endpoint, &server.token)
                                            .await
                                    }
                                    Err(error) => Err(error),
                                },
                                Err(error) => Err(error),
                            }
                        }
                        Some(Err(error)) => Err(error.clone()),
                        None => Err("Tailscale status is unavailable.".to_owned()),
                    };
                    (server, result)
                }
            }))
            .await;
            for (server, result) in remote {
                match result {
                    Ok(browsers) => items.extend(browsers.into_iter().map(|browser| BrowserItem {
                        host_id: server.id.clone(),
                        host_name: if server.display_name.is_empty() {
                            server.host_name.clone()
                        } else {
                            server.display_name.clone()
                        },
                        browser,
                    })),
                    Err(_) => errors.push(format!(
                        "Browser inventory unavailable on {}.",
                        if server.display_name.is_empty() {
                            &server.host_name
                        } else {
                            &server.display_name
                        }
                    )),
                }
            }
        }
        Err(error) => errors.push(error),
    }
    items.sort_by(|left, right| {
        left.browser
            .name
            .cmp(&right.browser.name)
            .then(left.host_name.cmp(&right.host_name))
    });
    BrowserSnapshot {
        items,
        servers,
        error: errors.join(" "),
    }
}

async fn fetch_virtual_machines(
    client: &Client,
    base: &str,
    token: &str,
) -> Result<Vec<FleetVirtualMachine>, String> {
    let response = client
        .get(format!("{base}{FLEET_VIRTUAL_MACHINES_PATH}"))
        .bearer_auth(token)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|_| "Could not reach app-server VM inventory.".to_owned())?;
    if !response.status().is_success()
        || response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            != Some(PROTOBUF_CONTENT_TYPE)
    {
        return Err("App-server VM inventory is unavailable.".to_owned());
    }
    let response = FleetVirtualMachinesResponse::decode(read_bounded(response).await?.as_slice())
        .map_err(|_| "Invalid app-server VM inventory.".to_owned())?;
    if response.virtual_machines.len() > 256
        || response.virtual_machines.iter().any(|machine| {
            machine
                .id
                .strip_prefix("hyper-v:")
                .is_none_or(|id| uuid::Uuid::parse_str(id).is_err())
                || !valid_text(&machine.name, 120)
                || machine.provider != "Hyper-V"
                || !valid_text(&machine.state, 40)
                || !matches!(machine.generation, 1 | 2)
                || machine.workspace_ids.len() > 128
                || (machine.all_workspaces && !machine.workspace_ids.is_empty())
                || machine
                    .workspace_ids
                    .iter()
                    .any(|id| uuid::Uuid::parse_str(id).is_err())
                || machine.workspace_ids.iter().collect::<HashSet<_>>().len()
                    != machine.workspace_ids.len()
        })
        || response
            .virtual_machines
            .iter()
            .map(|machine| &machine.id)
            .collect::<HashSet<_>>()
            .len()
            != response.virtual_machines.len()
    {
        return Err("Invalid app-server VM inventory.".to_owned());
    }
    Ok(response.virtual_machines)
}

pub(crate) async fn load_virtual_machines() -> VirtualMachineSnapshot {
    let mut items = Vec::new();
    let mut errors = Vec::new();
    let mut servers = Vec::new();
    match local_browser_client().await {
        Ok((client, base, token)) => match fetch_virtual_machines(&client, &base, &token).await {
            Ok(machines) => items.extend(machines.into_iter().map(|machine| VirtualMachineItem {
                host_id: "local".to_owned(),
                host_name: "Local App Server".to_owned(),
                machine,
            })),
            Err(error) => errors.push(error),
        },
        Err(error) => errors.push(error),
    }
    match read_saved() {
        Ok(saved) => {
            servers = saved.servers;
            let enabled = servers
                .iter()
                .filter(|server| server.enabled)
                .collect::<Vec<_>>();
            let status = if enabled.is_empty() {
                None
            } else {
                Some(tailnet_status().await)
            };
            let remote = join_all(enabled.into_iter().map(|server| {
                let status = status.as_ref();
                async move {
                    let result = match status {
                        Some(Ok(status)) => {
                            let mut verified = server.clone();
                            match probe(&mut verified, status).await {
                                Ok(()) => match remote_client(&server.endpoint, status) {
                                    Ok(client) => {
                                        fetch_virtual_machines(
                                            &client,
                                            &server.endpoint,
                                            &server.token,
                                        )
                                        .await
                                    }
                                    Err(error) => Err(error),
                                },
                                Err(error) => Err(error),
                            }
                        }
                        Some(Err(error)) => Err(error.clone()),
                        None => Err("Tailscale status is unavailable.".to_owned()),
                    };
                    (server, result)
                }
            }))
            .await;
            for (server, result) in remote {
                match result {
                    Ok(machines) => {
                        items.extend(machines.into_iter().map(|machine| VirtualMachineItem {
                            host_id: server.id.clone(),
                            host_name: if server.display_name.is_empty() {
                                server.host_name.clone()
                            } else {
                                server.display_name.clone()
                            },
                            machine,
                        }))
                    }
                    Err(_) => errors.push(format!(
                        "VM inventory unavailable on {}.",
                        if server.display_name.is_empty() {
                            &server.host_name
                        } else {
                            &server.display_name
                        }
                    )),
                }
            }
        }
        Err(error) => errors.push(error),
    }
    items.sort_by(|left, right| {
        left.machine
            .name
            .cmp(&right.machine.name)
            .then(left.host_name.cmp(&right.host_name))
    });
    VirtualMachineSnapshot {
        items,
        servers,
        error: errors.join(" "),
    }
}

pub(crate) async fn mutate_virtual_machine(action: VirtualMachineMutation) -> Result<(), String> {
    let (client, base, token) = browser_client(&action.host_id).await?;
    let request = FleetSetVirtualMachineSettingsRequest {
        id: action.id,
        enabled: action.enabled,
        all_workspaces: action.all_workspaces,
        workspace_ids: action.workspace_ids,
        can_run: action.can_run,
        can_clone: action.can_clone,
    };
    let response = client
        .post(format!("{base}{FLEET_VIRTUAL_MACHINE_SETTINGS_PATH}"))
        .bearer_auth(token)
        .header(CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
        .timeout(Duration::from_secs(15))
        .body(request.encode_to_vec())
        .send()
        .await
        .map_err(|_| "Could not reach app-server VM settings.".to_owned())?;
    match response.status() {
        reqwest::StatusCode::NO_CONTENT => Ok(()),
        reqwest::StatusCode::BAD_REQUEST => Err("Invalid VM workspace selection.".to_owned()),
        reqwest::StatusCode::NOT_FOUND => Err("Virtual machine is no longer available.".to_owned()),
        reqwest::StatusCode::FORBIDDEN => {
            Err("Fleet is disabled on the selected app server.".to_owned())
        }
        _ => Err("Could not update app-server VM settings.".to_owned()),
    }
}

pub(crate) async fn mutate_browser(action: BrowserMutation) -> Result<(), String> {
    let host_id = match &action {
        BrowserMutation::Add { host_id, .. }
        | BrowserMutation::Remove { host_id, .. }
        | BrowserMutation::SetSettings { host_id, .. }
        | BrowserMutation::SetPreferred { host_id, .. } => host_id,
    };
    let (client, base, token) = browser_client(host_id).await?;
    let (path, body) = match action {
        BrowserMutation::Add {
            name, executable, ..
        } => (
            FLEET_BROWSERS_PATH,
            FleetAddBrowserRequest { name, executable }.encode_to_vec(),
        ),
        BrowserMutation::Remove { id, .. } => (
            FLEET_BROWSER_REMOVE_PATH,
            FleetRemoveBrowserRequest { id }.encode_to_vec(),
        ),
        BrowserMutation::SetSettings {
            id,
            enabled,
            all_workspaces,
            workspace_ids,
            ..
        } => (
            FLEET_BROWSER_SETTINGS_PATH,
            FleetSetBrowserSettingsRequest {
                id,
                enabled,
                all_workspaces,
                workspace_ids,
            }
            .encode_to_vec(),
        ),
        BrowserMutation::SetPreferred { id, .. } => (
            FLEET_BROWSER_PREFERRED_PATH,
            FleetSetPreferredBrowserRequest { id }.encode_to_vec(),
        ),
    };
    let response = client
        .post(format!("{base}{path}"))
        .bearer_auth(token)
        .header(CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
        .body(body)
        .send()
        .await
        .map_err(|_| "Could not reach app-server browser inventory.".to_owned())?;
    match response.status() {
        reqwest::StatusCode::CREATED | reqwest::StatusCode::NO_CONTENT => Ok(()),
        reqwest::StatusCode::BAD_REQUEST => Err("Enter a name and an absolute path to an existing browser executable on the selected host.".to_owned()),
        reqwest::StatusCode::CONFLICT => Err("That browser executable is already added.".to_owned()),
        reqwest::StatusCode::FORBIDDEN => Err("Fleet is disabled on the selected app server.".to_owned()),
        _ => Err("Could not update app-server browser inventory.".to_owned()),
    }
}

pub(crate) async fn mutate(action: Mutation) -> Result<MutationResult, String> {
    let path = preference_path()?;
    let mut saved = read_at(&path)?;
    match action {
        Mutation::Connect {
            endpoint,
            code,
            display_name,
        } => {
            let (endpoint, _, _) = normalize_endpoint(&endpoint)?;
            if code.len() != 6
                || !code.bytes().all(|byte| byte.is_ascii_digit())
                || (!display_name.is_empty() && !valid_text(&display_name, 80))
            {
                return Err(
                    "Enter a six-digit authenticator code and a valid display name.".to_owned(),
                );
            }
            if saved.servers.len() >= MAX_CONNECTIONS
                && !saved
                    .servers
                    .iter()
                    .any(|server| server.endpoint == endpoint)
            {
                return Err("Fleet app-server limit reached.".to_owned());
            }
            let status = tailnet_status().await?;
            let token = exchange_authenticator_code(&endpoint, &code, &status).await?;
            let mut server = FleetServer {
                id: saved
                    .servers
                    .iter()
                    .find(|server| server.endpoint == endpoint)
                    .map_or_else(
                        || uuid::Uuid::new_v4().to_string(),
                        |server| server.id.clone(),
                    ),
                endpoint,
                token,
                host_name: "App Server".to_owned(),
                display_name,
                os_name: "Other".to_owned(),
                os_vendor: "Other".to_owned(),
                protocol_major: PROTOCOL_MAJOR,
                protocol_minor: PROTOCOL_MINOR,
                enabled: true,
                condition: "Checking".to_owned(),
                condition_detail: String::new(),
            };
            probe(&mut server, &status).await?;
            if let Some(existing) = saved.servers.iter_mut().find(|entry| entry.id == server.id) {
                if server.display_name.is_empty() {
                    server.display_name = existing.display_name.clone();
                }
                *existing = server.clone();
            } else {
                saved.servers.push(server.clone());
            }
            write_at(&path, &saved)?;
            Ok(MutationResult::Saved(Box::new(server)))
        }
        Mutation::Update {
            id,
            enabled,
            display_name,
        } => {
            if !display_name.is_empty() && !valid_text(&display_name, 80) {
                return Err("Invalid display name.".to_owned());
            }
            let server = saved
                .servers
                .iter_mut()
                .find(|server| server.id == id)
                .ok_or("App server was not found.")?;
            server.enabled = enabled;
            server.display_name = display_name;
            server.condition = if enabled { "Checking" } else { "Disabled" }.to_owned();
            server.condition_detail = if enabled {
                "Checking connection."
            } else {
                "Connection disabled."
            }
            .to_owned();
            let updated = server.clone();
            write_at(&path, &saved)?;
            Ok(MutationResult::Saved(Box::new(updated)))
        }
        Mutation::Remove { id } => {
            let before = saved.servers.len();
            saved.servers.retain(|server| server.id != id);
            if saved.servers.len() == before {
                return Err("App server was not found.".to_owned());
            }
            write_at(&path, &saved)?;
            Ok(MutationResult::Removed(id))
        }
        Mutation::UpdateLocalName { display_name } => {
            if !display_name.is_empty() && !valid_text(&display_name, 80) {
                return Err("Invalid display name.".to_owned());
            }
            saved.local_display_name = display_name.clone();
            write_at(&path, &saved)?;
            Ok(MutationResult::LocalName(display_name))
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_requires_tailscale_https_origin() {
        assert!(normalize_endpoint("https://node.example.ts.net:43832").is_ok());
        for value in [
            "http://node.example.ts.net",
            "https://example.test",
            "https://node.example.ts.net/path",
            "https://user@node.example.ts.net",
            "https://node.example.ts.net?q=1",
        ] {
            assert!(normalize_endpoint(value).is_err(), "{value}");
        }
    }

    #[test]
    fn peer_check_rejects_other_tailnets_and_missing_peers() {
        let status: TailnetStatus = serde_json::from_str(r#"{"BackendState":"Running","Self":{"DNSName":"local.example.ts.net."},"Peer":{"one":{"DNSName":"remote.example.ts.net.","Online":true,"TailscaleIPs":["192.0.2.10"]}}}"#).unwrap();
        assert_eq!(
            peer_ip(&status, "remote.example.ts.net")
                .unwrap()
                .to_string(),
            "192.0.2.10"
        );
        assert!(peer_ip(&status, "remote.other.ts.net").is_err());
        assert!(peer_ip(&status, "missing.example.ts.net").is_err());
    }

    #[test]
    fn import_lists_only_valid_tailnet_peers_and_keeps_otp_endpoint_bounded() {
        let status: TailnetStatus = serde_json::from_str(
            r#"{"BackendState":"Running","Self":{"DNSName":"local.example.ts.net."},"Peer":{"one":{"DNSName":"online.example.ts.net.","HostName":"Example workstation","OS":"windows","Online":true},"two":{"DNSName":"offline.example.ts.net.","HostName":"Example server","OS":"linux","Online":false},"three":{"DNSName":"other.other.ts.net.","HostName":"Other network","Online":true}}}"#,
        )
        .unwrap();
        let devices = devices_from_status(&status).unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].host_name, "Example workstation");
        assert_eq!(devices[0].os_name, "Windows");
        assert!(devices[0].online);
        assert!(!devices[1].online);
        assert_eq!(
            imported_endpoint(&devices[0], "43832").unwrap(),
            "https://online.example.ts.net:43832"
        );
        assert!(imported_endpoint(&devices[0], "0").is_err());
        let stopped: TailnetStatus = serde_json::from_str(
            r#"{"BackendState":"Stopped","Self":{"DNSName":"local.example.ts.net."}}"#,
        )
        .unwrap();
        assert!(devices_from_status(&stopped).is_err());
    }

    #[test]
    fn saved_connections_keep_multiple_servers_and_tokens_local() {
        let root =
            std::env::temp_dir().join(format!("beale-fleet-example-{}", uuid::Uuid::new_v4()));
        let path = root.join("fleet.json");
        let server = FleetServer {
            id: uuid::Uuid::new_v4().to_string(),
            endpoint: "https://one.example.ts.net:43832".to_owned(),
            token: "a".repeat(64),
            host_name: "example-one".to_owned(),
            display_name: String::new(),
            os_name: "Linux".to_owned(),
            os_vendor: "Linux".to_owned(),
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            enabled: true,
            condition: "Online".to_owned(),
            condition_detail: "Connected".to_owned(),
        };
        let mut second = server.clone();
        second.id = uuid::Uuid::new_v4().to_string();
        second.endpoint = "https://two.example.ts.net:43832".to_owned();
        second.display_name = "Lab server".to_owned();
        second.protocol_minor = PROTOCOL_MINOR.saturating_sub(1);
        write_at(
            &path,
            &SavedConnections {
                version: 1,
                local_display_name: "My desktop".to_owned(),
                servers: vec![server, second],
            },
        )
        .unwrap();
        let saved = read_at(&path).unwrap();
        assert_eq!(saved.servers.len(), 2);
        assert_eq!(saved.local_display_name, "My desktop");
        assert_eq!(saved.servers[1].display_name, "Lab server");
        assert_eq!(saved.servers[0].condition, "Checking");
        std::fs::remove_dir_all(root).unwrap();
    }
}
