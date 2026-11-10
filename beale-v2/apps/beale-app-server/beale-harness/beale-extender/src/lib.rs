//! Host-owned installation registry for bundled Agent Plugins packages.
//! Installed MCP servers are activated by the host and discovered at launch.

mod mcp_runtime;

use anyhow::{Context, Result, ensure};
use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode, header},
    routing::{get, post},
};
use beale_contracts::{
    PLUGIN_INSTALL_PATH, PLUGIN_MANIFEST_PATH, PLUGIN_SCOPE_PATH, PLUGIN_UNINSTALL_PATH,
    PLUGINS_PATH, PROTOBUF_CONTENT_TYPE,
    v1::{
        GetPluginManifestRequest, InstallPluginRequest, ListPluginsResponse,
        PluginManifestResponse, PluginSummary, SetExtensionWorkspaceScopeRequest,
        UninstallPluginRequest,
    },
};
use beale_storage::HostDatabase;
use prost::Message;
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::Mutex,
    time::{Duration, timeout},
};

const BUNDLES: [(&str, &str); 4] = [
    ("apple-security-devices", "apple-security-device-plugin"),
    ("apple-target-flags", "apple-target-flags-plugin"),
    ("meta-skills", "meta-skills-plugin"),
    (
        "microsoft-security-devices",
        "microsoft-security-devices-plugin",
    ),
];
const MANIFEST_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";
const MCP_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";

#[derive(Deserialize)]
struct Manifest {
    #[serde(rename = "$schema")]
    schema: String,
    name: String,
    #[serde(default)]
    description: String,
}

#[derive(Clone)]
struct Catalog {
    root: PathBuf,
    data_root: PathBuf,
    active: Arc<Mutex<HashMap<String, HashMap<String, mcp_runtime::RunningServer>>>>,
}

#[derive(Clone)]
pub struct PluginRuntime {
    catalog: Catalog,
    db: HostDatabase,
}

impl PluginRuntime {
    pub fn new(db: HostDatabase, data_root: PathBuf) -> Result<Self> {
        Ok(Self {
            catalog: Catalog::bundled(data_root)?,
            db,
        })
    }

    pub fn available(&self, workspace_id: &str) -> Result<Vec<PluginSummary>> {
        self.catalog
            .list(&self.db)?
            .into_iter()
            .filter(|plugin| plugin.installed)
            .filter_map(|plugin| {
                match self
                    .db
                    .extension_applies_to("plugin", &plugin.name, workspace_id)
                {
                    Ok(true) => Some(Ok(plugin)),
                    Ok(false) => None,
                    Err(error) => Some(Err(error)),
                }
            })
            .collect()
    }

    pub async fn details(&self, name: &str, workspace_id: &str) -> Result<Value> {
        ensure!(
            self.db
                .installed_plugin_names()?
                .iter()
                .any(|installed| installed == name),
            "plugin is not installed"
        );
        ensure!(
            self.db.extension_applies_to("plugin", name, workspace_id)?,
            "plugin is unavailable in this workspace"
        );
        let root = self
            .catalog
            .package(name)?
            .context("plugin package is unavailable")?;
        let description = self.catalog.scan(name, &root)?.description;
        let mut skills = Vec::new();
        let skills_root = root.join("skills");
        if skills_root.is_dir() {
            for entry in std::fs::read_dir(skills_root)? {
                let entry = entry?;
                let path = entry.path().join("SKILL.md");
                if path.exists() {
                    let path = contained_file(&root, &path)?;
                    ensure!(
                        path.metadata()?.len() <= 64_000,
                        "plugin skill is too large for model context"
                    );
                    skills.push(serde_json::json!({"name":entry.file_name().to_string_lossy(),"instructions":std::fs::read_to_string(path)?}));
                }
            }
        }
        timeout(Duration::from_secs(30), self.catalog.activate(name))
            .await
            .context("plugin launch timed out")??;
        let active = self.catalog.active.lock().await;
        let mut tools = Vec::new();
        if let Some(servers) = active.get(name) {
            for (server_name, server) in servers {
                for tool in &server.tools {
                    tools.push(serde_json::json!({"server":server_name,"definition":tool}));
                }
            }
        }
        tools.sort_unstable_by(|a, b| {
            a["server"].as_str().cmp(&b["server"].as_str()).then(
                a["definition"]["name"]
                    .as_str()
                    .cmp(&b["definition"]["name"].as_str()),
            )
        });
        let details = serde_json::json!({"plugin":name,"description":description,"skills":skills,"tools":tools,"call_tool":"Use plugin_call with this plugin, server, tool, and arguments."});
        ensure!(
            details.to_string().len() <= 8_000_000,
            "plugin details are too large"
        );
        Ok(details)
    }

    pub async fn call_tool(
        &self,
        plugin: &str,
        server: &str,
        tool: &str,
        arguments: Value,
        workspace_id: &str,
    ) -> Result<Value> {
        ensure!(
            self.db
                .installed_plugin_names()?
                .iter()
                .any(|installed| installed == plugin),
            "plugin is not installed"
        );
        ensure!(
            self.db
                .extension_applies_to("plugin", plugin, workspace_id)?,
            "plugin is unavailable in this workspace"
        );
        let mut active = self.catalog.active.lock().await;
        let server = active
            .get_mut(plugin)
            .and_then(|servers| servers.get_mut(server))
            .context("plugin server is unavailable; load the plugin again")?;
        server.call_tool(tool, arguments).await
    }
}

impl Catalog {
    fn bundled(data_root: PathBuf) -> Result<Self> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../../extensions/plugins")
            .canonicalize()
            .context("locate bundled plugins")?;
        let catalog = Self {
            root,
            data_root,
            active: Arc::default(),
        };
        for (name, directory) in BUNDLES {
            catalog.scan(name, &catalog.root.join(directory))?;
        }
        Ok(catalog)
    }

    fn packages_root(&self) -> PathBuf {
        self.data_root.join("plugins").join("packages")
    }

    fn entries(&self) -> Result<Vec<(String, PathBuf)>> {
        let mut entries = BUNDLES
            .iter()
            .map(|(name, directory)| ((*name).to_owned(), self.root.join(directory)))
            .collect::<Vec<_>>();
        let packages = self.packages_root();
        if packages.exists() {
            let packages = packages.canonicalize()?;
            for entry in std::fs::read_dir(&packages)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if !valid_name(&name) || BUNDLES.iter().any(|(bundled, _)| *bundled == name) {
                    continue;
                }
                let Ok(path) = entry.path().canonicalize() else {
                    continue;
                };
                if !path.starts_with(&packages) {
                    continue;
                }
                if path.is_dir() && self.scan(&name, &path).is_ok() {
                    entries.push((name, path));
                }
            }
        }
        Ok(entries)
    }

    fn package(&self, name: &str) -> Result<Option<PathBuf>> {
        Ok(self
            .entries()?
            .into_iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, path)| path))
    }

    fn scan(&self, expected_name: &str, directory: &Path) -> Result<PluginSummary> {
        let root = directory.canonicalize().context("locate plugin")?;
        let external_root = self
            .packages_root()
            .canonicalize()
            .unwrap_or_else(|_| self.packages_root());
        ensure!(
            (root.starts_with(&self.root) || root.starts_with(&external_root)) && root.is_dir(),
            "invalid plugin root"
        );
        let manifest_path = contained_file(&root, &root.join("plugin.json"))?;
        ensure!(
            manifest_path.metadata()?.len() <= 64_000,
            "plugin manifest is too large"
        );
        let manifest: Manifest = serde_json::from_slice(&std::fs::read(manifest_path)?)
            .context("read plugin manifest")?;
        ensure!(
            manifest.schema == MANIFEST_SCHEMA,
            "unsupported plugin schema"
        );
        ensure!(
            valid_name(&manifest.name) && manifest.name == expected_name,
            "invalid plugin name"
        );
        ensure!(
            manifest.description.len() <= 2_000,
            "plugin description is too large"
        );

        let mut skill_count = 0_u32;
        let mut script_count = 0_u32;
        let mut reference_count = 0_u32;
        let mut asset_count = 0_u32;
        let skills_path = root.join("skills");
        if skills_path.exists() {
            let skills = skills_path.canonicalize()?;
            ensure!(
                skills.starts_with(&root) && skills.is_dir(),
                "invalid skills directory"
            );
            for entry in std::fs::read_dir(skills)? {
                let entry = entry?;
                let skill_dir = entry.path().canonicalize()?;
                ensure!(skill_dir.starts_with(&root), "skill escapes plugin root");
                if skill_dir.is_dir() && skill_dir.join("SKILL.md").exists() {
                    let skill = contained_file(&root, &skill_dir.join("SKILL.md"))?;
                    ensure!(skill.metadata()?.len() <= 1_000_000, "skill is too large");
                    skill_count += 1;
                    script_count = script_count
                        .checked_add(count_skill_files(&root, &skill_dir.join("scripts"))?)
                        .context("too many skill scripts")?;
                    reference_count = reference_count
                        .checked_add(count_skill_files(&root, &skill_dir.join("references"))?)
                        .context("too many skill references")?;
                    asset_count = asset_count
                        .checked_add(count_skill_files(&root, &skill_dir.join("assets"))?)
                        .context("too many skill assets")?;
                }
            }
        }

        let mcp_server_count = mcp_runtime::configurations(&root)
            .map(|servers| servers.len() as u32)
            .unwrap_or(0);
        let mut mcp_tool_count = 0_u32;
        let inventory_path = self
            .root
            .parent()
            .context("locate extension inventory")?
            .join("apple-security-device-tools.json");
        if expected_name == "apple-security-devices" && inventory_path.exists() {
            let inventory_path = contained_file(
                self.root.parent().context("locate extension inventory")?,
                &inventory_path,
            )?;
            ensure!(
                inventory_path.metadata()?.len() <= 64_000,
                "MCP tool inventory is too large"
            );
            let names: Vec<String> = serde_json::from_slice(&std::fs::read(inventory_path)?)?;
            ensure!(names.len() <= 128, "too many MCP tools");
            let mut unique = HashSet::new();
            for name in &names {
                ensure!(
                    !name.trim().is_empty()
                        && name.len() <= 128
                        && !name.chars().any(char::is_control)
                        && unique.insert(name),
                    "invalid MCP tool inventory"
                );
            }
            ensure!(
                mcp_server_count > 0 || names.is_empty(),
                "MCP tools require a server"
            );
            mcp_tool_count = u32::try_from(names.len()).context("too many MCP tools")?;
        }
        Ok(PluginSummary {
            name: manifest.name,
            description: manifest.description,
            installed: false,
            skill_count,
            has_mcp_server: mcp_server_count > 0,
            mcp_server_count,
            mcp_tool_count,
            script_count,
            reference_count,
            asset_count,
            mcp_prompt_count: 0,
            mcp_resource_count: 0,
            workspace_ids: Vec::new(),
            all_workspaces: true,
        })
    }

    fn manifest(&self, name: &str) -> Result<Option<String>> {
        let Some(root) = self.package(name)? else {
            return Ok(None);
        };
        self.scan(name, &root)?;
        let path = contained_file(&root, &root.join("plugin.json"))?;
        let bytes = std::fs::read(path)?;
        ensure!(bytes.len() <= 64_000, "plugin manifest is too large");
        Ok(Some(
            String::from_utf8(bytes).context("plugin manifest is not UTF-8")?,
        ))
    }

    fn list(&self, db: &HostDatabase) -> Result<Vec<PluginSummary>> {
        let installed: HashSet<_> = db.installed_plugin_names()?.into_iter().collect();
        self.entries()?
            .into_iter()
            .map(|(name, directory)| {
                let mut summary = self.scan(&name, &directory)?;
                summary.installed = installed.contains(&name);
                let scope = db.extension_scope("plugin", &name)?;
                summary.workspace_ids = scope.workspace_ids;
                summary.all_workspaces = scope.all_workspaces;
                Ok(summary)
            })
            .collect()
    }

    async fn activate(&self, name: &str) -> Result<u32> {
        let Some(root) = self.package(name)? else {
            return Ok(0);
        };
        let configurations = match mcp_runtime::configurations(&root) {
            Ok(configurations) => configurations,
            Err(error) => {
                self.deactivate(name).await;
                return Err(error);
            }
        };
        let mut active = self.active.lock().await;
        let servers = active.entry(name.to_owned()).or_default();
        servers.retain(|server_name, server| {
            configurations.contains_key(server_name) && server.alive()
        });
        if configurations.is_empty() {
            return Ok(0);
        }
        let data_root = self.data_root.join("plugins").join(name);
        std::fs::create_dir_all(&data_root)?;
        let data_root = data_root.canonicalize()?;
        for (server_name, config) in configurations {
            if servers.contains_key(&server_name) {
                continue;
            }
            // One server's launch failure must not disable the rest of the plugin.
            if let Ok(server) = mcp_runtime::RunningServer::launch(&root, &data_root, &config).await
            {
                servers.insert(server_name, server);
            }
        }
        let count = servers.values().map(|server| server.tool_count).sum();
        Ok(count)
    }

    async fn mcp_detail_counts(&self, name: &str) -> (u32, u32) {
        let active = self.active.lock().await;
        active.get(name).map_or((0, 0), |servers| {
            (
                servers.values().map(|server| server.prompt_count).sum(),
                servers.values().map(|server| server.resource_count).sum(),
            )
        })
    }

    async fn deactivate(&self, name: &str) {
        self.active.lock().await.remove(name);
    }

    async fn tool_names(&self, name: &str, installed: bool) -> Vec<String> {
        if installed {
            let _ = timeout(Duration::from_secs(25), self.activate(name)).await;
            let active = self.active.lock().await;
            if let Some(servers) = active.get(name) {
                let mut names = servers
                    .iter()
                    .flat_map(|(server_name, server)| {
                        server.tool_names.iter().map(move |tool_name| {
                            if servers.len() == 1 {
                                tool_name.clone()
                            } else {
                                format!("{server_name}: {tool_name}")
                            }
                        })
                    })
                    .collect::<Vec<_>>();
                names.sort_unstable();
                return names;
            }
            return Vec::new();
        }
        if name == "apple-security-devices" {
            let Some(parent) = self.root.parent() else {
                return Vec::new();
            };
            let path = parent.join("apple-security-device-tools.json");
            if let Ok(bytes) = std::fs::read(path)
                && let Ok(names) = serde_json::from_slice::<Vec<String>>(&bytes)
            {
                return names;
            }
        }
        Vec::new()
    }

    async fn retain_installed(&self, installed: &HashSet<&str>) {
        self.active
            .lock()
            .await
            .retain(|name, _| installed.contains(name.as_str()));
    }

    fn install(&self, db: &HostDatabase, name: &str) -> Result<Option<PluginSummary>> {
        let Some(root) = self.package(name)? else {
            return Ok(None);
        };
        let mut summary = self.scan(name, &root)?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
        let now = i64::try_from(now).context("installation time overflow")?;
        db.install_plugin(name, now)?;
        summary.installed = true;
        let scope = db.extension_scope("plugin", name)?;
        summary.workspace_ids = scope.workspace_ids;
        summary.all_workspaces = scope.all_workspaces;
        Ok(Some(summary))
    }

    fn uninstall(&self, db: &HostDatabase, name: &str) -> Result<Option<PluginSummary>> {
        let Some(root) = self.package(name)? else {
            return Ok(None);
        };
        let summary = self.scan(name, &root)?;
        db.uninstall_plugin(name)?;
        let scope = db.extension_scope("plugin", name)?;
        Ok(Some(PluginSummary {
            workspace_ids: scope.workspace_ids,
            all_workspaces: scope.all_workspaces,
            ..summary
        }))
    }
}

fn contained_file(root: &Path, path: &Path) -> Result<PathBuf> {
    let path = path.canonicalize().context("locate plugin file")?;
    ensure!(
        path.starts_with(root) && path.is_file(),
        "plugin file escapes root"
    );
    Ok(path)
}

fn count_skill_files(root: &Path, directory: &Path) -> Result<u32> {
    match std::fs::symlink_metadata(directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
        Ok(metadata) => ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "invalid skill resource directory"
        ),
    }
    let mut pending = vec![directory.to_path_buf()];
    let mut visited = 0_usize;
    let mut count = 0_u32;
    while let Some(path) = pending.pop() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)?;
            ensure!(
                !metadata.file_type().is_symlink(),
                "skill resource cannot be a symlink"
            );
            ensure!(
                path.canonicalize()?.starts_with(root),
                "skill resource escapes plugin root"
            );
            visited += 1;
            ensure!(visited <= 8_192, "too many skill resource entries");
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                count = count
                    .checked_add(1)
                    .context("too many skill resource files")?;
                ensure!(count <= 4_096, "too many skill resource files");
            } else {
                anyhow::bail!("invalid skill resource entry");
            }
        }
    }
    Ok(count)
}

fn valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    (1..=64).contains(&bytes.len())
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
        })
        && !name.contains("--")
        && !name.contains("..")
}

#[derive(Clone)]
struct HttpState {
    token: String,
    db: HostDatabase,
    catalog: Catalog,
}

pub fn router(token: String, db: HostDatabase, data_root: PathBuf) -> Result<Router> {
    router_with_runtime(token, PluginRuntime::new(db, data_root)?)
}

pub fn router_with_runtime(token: String, runtime: PluginRuntime) -> Result<Router> {
    let PluginRuntime { catalog, db } = runtime;
    Ok(Router::new()
        .route(PLUGINS_PATH, get(list_route))
        .route(PLUGIN_INSTALL_PATH, post(install_route))
        .route(PLUGIN_UNINSTALL_PATH, post(uninstall_route))
        .route(PLUGIN_MANIFEST_PATH, post(manifest_route))
        .route(PLUGIN_SCOPE_PATH, post(scope_route))
        .layer(DefaultBodyLimit::max(8_192))
        .with_state(HttpState { token, db, catalog }))
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
            .fold(0_u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

type ProtoResponse = ([(header::HeaderName, &'static str); 1], Vec<u8>);

async fn list_route(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<ProtoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let mut plugins = state
        .catalog
        .list(&state.db)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let installed = plugins
        .iter()
        .filter(|plugin| plugin.installed)
        .map(|plugin| plugin.name.as_str())
        .collect();
    state.catalog.retain_installed(&installed).await;
    for plugin in &mut plugins {
        if plugin.installed && plugin.has_mcp_server {
            plugin.mcp_tool_count = 0;
            plugin.mcp_prompt_count = 0;
            plugin.mcp_resource_count = 0;
            if let Ok(Ok(count)) = timeout(
                Duration::from_secs(25),
                state.catalog.activate(&plugin.name),
            )
            .await
            {
                plugin.mcp_tool_count = count;
                (plugin.mcp_prompt_count, plugin.mcp_resource_count) =
                    state.catalog.mcp_detail_counts(&plugin.name).await;
            }
        }
    }
    Ok((
        [(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)],
        ListPluginsResponse { plugins }.encode_to_vec(),
    ))
}

async fn install_route(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<ProtoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some(PROTOBUF_CONTENT_TYPE)
    {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    let request = InstallPluginRequest::decode(body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let mut plugin = state
        .catalog
        .install(&state.db, &request.name)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    if plugin.has_mcp_server {
        plugin.mcp_tool_count = 0;
        plugin.mcp_prompt_count = 0;
        plugin.mcp_resource_count = 0;
        if let Ok(Ok(count)) = timeout(
            Duration::from_secs(25),
            state.catalog.activate(&plugin.name),
        )
        .await
        {
            plugin.mcp_tool_count = count;
            (plugin.mcp_prompt_count, plugin.mcp_resource_count) =
                state.catalog.mcp_detail_counts(&plugin.name).await;
        }
    }
    Ok((
        [(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)],
        plugin.encode_to_vec(),
    ))
}

async fn uninstall_route(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<ProtoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some(PROTOBUF_CONTENT_TYPE)
    {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    let request = UninstallPluginRequest::decode(body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let plugin = state
        .catalog
        .uninstall(&state.db, &request.name)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    state.catalog.deactivate(&request.name).await;
    Ok((
        [(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)],
        plugin.encode_to_vec(),
    ))
}

async fn manifest_route(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<ProtoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some(PROTOBUF_CONTENT_TYPE)
    {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    let request = GetPluginManifestRequest::decode(body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let manifest_json = state
        .catalog
        .manifest(&request.name)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let installed = state
        .db
        .installed_plugin_names()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .contains(&request.name);
    let tool_names = state.catalog.tool_names(&request.name, installed).await;
    Ok((
        [(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)],
        PluginManifestResponse {
            name: request.name,
            manifest_json,
            tool_names,
        }
        .encode_to_vec(),
    ))
}

async fn scope_route(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<ProtoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some(PROTOBUF_CONTENT_TYPE)
    {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    let request =
        SetExtensionWorkspaceScopeRequest::decode(body).map_err(|_| StatusCode::BAD_REQUEST)?;
    if state
        .catalog
        .package(&request.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .is_none()
    {
        return Err(StatusCode::NOT_FOUND);
    }
    let installed = state
        .db
        .installed_plugin_names()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !installed.contains(&request.id) {
        return Err(StatusCode::CONFLICT);
    }
    state
        .db
        .set_extension_scope(
            "plugin",
            &request.id,
            request.all_workspaces,
            &request.workspace_ids,
        )
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let scope = state
        .db
        .extension_scope("plugin", &request.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok((
        [(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)],
        SetExtensionWorkspaceScopeRequest {
            id: request.id,
            workspace_ids: scope.workspace_ids,
            all_workspaces: scope.all_workspaces,
        }
        .encode_to_vec(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use beale_storage::WorkspaceRow;
    use tower::ServiceExt;

    #[test]
    fn bundled_catalog_installs_idempotently_and_persists() {
        let catalog = Catalog::bundled(std::env::temp_dir()).unwrap();
        let root =
            std::env::temp_dir().join(format!("beale-extender-example-{}", std::process::id()));
        let db = HostDatabase::open(&root).unwrap();
        let available = catalog.list(&db).unwrap();
        assert_eq!(available.len(), 4);
        assert!(
            available
                .iter()
                .all(|plugin| !plugin.installed && plugin.skill_count == 1)
        );
        assert!(available[0].has_mcp_server);
        assert!(!available[1].has_mcp_server);
        assert_eq!(
            available
                .iter()
                .map(|plugin| plugin.script_count)
                .collect::<Vec<_>>(),
            [0, 0, 2, 1]
        );
        assert_eq!(
            available
                .iter()
                .map(|plugin| plugin.reference_count)
                .collect::<Vec<_>>(),
            [3, 1, 0, 2]
        );
        assert!(available.iter().all(|plugin| plugin.asset_count == 0));
        assert_eq!(available[0].mcp_server_count, 1);
        assert_eq!(available[1].mcp_server_count, 0);
        assert_eq!(available[0].mcp_tool_count, 19);
        assert!(
            available[1..]
                .iter()
                .all(|plugin| plugin.mcp_tool_count == 0)
        );
        assert!(
            catalog
                .manifest("apple-security-devices")
                .unwrap()
                .unwrap()
                .contains("\"version\"")
        );
        assert!(catalog.manifest("unknown-plugin").unwrap().is_none());
        assert!(catalog.install(&db, "unknown-plugin").unwrap().is_none());
        assert!(
            catalog
                .install(&db, "meta-skills")
                .unwrap()
                .unwrap()
                .installed
        );
        assert!(
            !catalog
                .uninstall(&db, "meta-skills")
                .unwrap()
                .unwrap()
                .installed
        );
        assert!(
            !catalog
                .uninstall(&db, "meta-skills")
                .unwrap()
                .unwrap()
                .installed
        );
        assert!(db.installed_plugin_names().unwrap().is_empty());
        assert!(
            catalog
                .install(&db, "meta-skills")
                .unwrap()
                .unwrap()
                .installed
        );
        assert!(
            catalog
                .install(&db, "meta-skills")
                .unwrap()
                .unwrap()
                .installed
        );
        drop(db);
        let reopened = HostDatabase::open(&root).unwrap();
        assert_eq!(reopened.installed_plugin_names().unwrap(), ["meta-skills"]);
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_invalid_names() {
        for name in ["", "Upper", "-bad", "bad-", "bad--name", "bad..name"] {
            assert!(!valid_name(name));
        }
        assert!(valid_name("example.plugin-1"));
    }

    #[tokio::test]
    async fn discovers_tools_from_a_third_party_stdio_plugin() {
        if std::process::Command::new("node")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let root =
            std::env::temp_dir().join(format!("beale-extender-third-party-{}", std::process::id()));
        let package = root.join("plugins/packages/example-tools");
        std::fs::create_dir_all(&package).unwrap();
        let invalid = root.join("plugins/packages/invalid-plugin");
        std::fs::create_dir_all(&invalid).unwrap();
        std::fs::write(invalid.join("plugin.json"), "{}").unwrap();
        std::fs::write(package.join("plugin.json"), format!(r#"{{"$schema":"{MANIFEST_SCHEMA}","name":"example-tools","description":"Example tools"}}"#)).unwrap();
        std::fs::create_dir_all(package.join("skills/example-skill")).unwrap();
        std::fs::write(
            package.join("skills/example-skill/SKILL.md"),
            "# Example skill\n\nUse for example.test research.",
        )
        .unwrap();
        for (directory, file) in [
            ("scripts", "check.sh"),
            ("references", "notes.md"),
            ("assets", "example.txt"),
        ] {
            let path = package.join("skills/example-skill").join(directory);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join(file), "Synthetic example content.").unwrap();
        }
        std::fs::write(package.join("mcp.json"), format!(r#"{{"$schema":"{MCP_SCHEMA}","mcpServers":{{"example":{{"type":"stdio","command":"node","args":["${{PLUGIN_ROOT}}/server.mjs"]}}}}}}"#)).unwrap();
        std::fs::write(
            package.join("server.mjs"),
            r#"
import readline from 'node:readline';
for await (const line of readline.createInterface({ input: process.stdin })) {
  const request = JSON.parse(line);
  if (request.id === undefined) continue;
  const result = request.method === 'server/discover'
    ? { supportedVersions: ['2026-07-28'] }
    : request.method === 'tools/list'
      ? { tools: [{ name: 'example_read', description: 'Read example data', inputSchema: { type: 'object', properties: {}, additionalProperties: false } }, { name: 'example_write' }] }
      : request.method === 'prompts/list'
        ? { prompts: [{ name: 'example_prompt' }] }
      : request.method === 'resources/list'
        ? { resources: [{ uri: 'example://resource' }] }
      : request.method === 'resources/templates/list'
        ? { resourceTemplates: [{ uriTemplate: 'example://items/{id}' }] }
      : request.method === 'tools/call'
        ? { content: [{ type: 'text', text: `Called ${request.params.name}` }] }
      : {};
  process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, result }) + '\n');
}
"#,
        )
        .unwrap();
        let catalog = Catalog::bundled(root.clone()).unwrap();
        let db = HostDatabase::open(&root).unwrap();
        let available = catalog.list(&db).unwrap();
        assert_eq!(available.len(), 5);
        assert!(
            available
                .iter()
                .any(|plugin| plugin.name == "example-tools" && plugin.mcp_server_count == 1)
        );
        let example = available
            .iter()
            .find(|plugin| plugin.name == "example-tools")
            .unwrap();
        assert_eq!(
            (
                example.skill_count,
                example.script_count,
                example.reference_count,
                example.asset_count
            ),
            (1, 1, 1, 1)
        );
        catalog.install(&db, "example-tools").unwrap();
        let config = mcp_runtime::configurations(&package.canonicalize().unwrap()).unwrap();
        mcp_runtime::RunningServer::launch(
            &package.canonicalize().unwrap(),
            &root.canonicalize().unwrap(),
            &config["example"],
        )
        .await
        .unwrap();
        assert_eq!(catalog.activate("example-tools").await.unwrap(), 2);
        assert_eq!(catalog.mcp_detail_counts("example-tools").await, (1, 2));
        let runtime = PluginRuntime {
            catalog: catalog.clone(),
            db: db.clone(),
        };
        let details = runtime
            .details("example-tools", "example-workspace")
            .await
            .unwrap();
        assert_eq!(
            details["tools"][0]["definition"]["description"],
            "Read example data"
        );
        assert_eq!(details["skills"][0]["name"], "example-skill");
        let result = runtime
            .call_tool(
                "example-tools",
                "example",
                "example_read",
                serde_json::json!({}),
                "example-workspace",
            )
            .await
            .unwrap();
        assert_eq!(result["content"][0]["text"], "Called example_read");
        assert!(
            runtime
                .call_tool(
                    "example-tools",
                    "example",
                    "unknown",
                    serde_json::json!({}),
                    "example-workspace"
                )
                .await
                .is_err()
        );
        db.set_extension_scope("plugin", "example-tools", false, &[])
            .unwrap();
        assert!(
            runtime
                .details("example-tools", "example-workspace")
                .await
                .is_err()
        );
        assert!(
            runtime
                .call_tool(
                    "example-tools",
                    "example",
                    "example_read",
                    serde_json::json!({}),
                    "example-workspace"
                )
                .await
                .is_err()
        );
        db.set_extension_scope("plugin", "example-tools", true, &[])
            .unwrap();
        assert_eq!(
            catalog.tool_names("example-tools", true).await,
            ["example_read", "example_write"]
        );
        assert_eq!(catalog.activate("example-tools").await.unwrap(), 2);
        assert_eq!(
            catalog.activate("apple-security-devices").await.unwrap(),
            19
        );
        catalog.deactivate("example-tools").await;
        catalog.deactivate("apple-security-devices").await;
        drop(runtime);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn authenticated_protocol_lists_installs_and_uninstalls_bundled_plugin() {
        let root = std::env::temp_dir().join(format!(
            "beale-extender-http-example-{}",
            std::process::id()
        ));
        let db = HostDatabase::open(&root).unwrap();
        let app = router("example-token".to_owned(), db.clone(), root.clone()).unwrap();
        let unauthorized = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(PLUGINS_PATH)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let listed = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(PLUGINS_PATH)
                    .header(header::AUTHORIZATION, "Bearer example-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(listed.status(), StatusCode::OK);
        let body = to_bytes(listed.into_body(), 64_000).await.unwrap();
        let catalog = ListPluginsResponse::decode(body).unwrap();
        assert_eq!(catalog.plugins.len(), 4);
        assert!(catalog.plugins.iter().all(|plugin| !plugin.installed));
        let apple = catalog
            .plugins
            .iter()
            .find(|plugin| plugin.name == "apple-security-devices")
            .unwrap();
        assert_eq!(
            (
                apple.skill_count,
                apple.script_count,
                apple.reference_count,
                apple.asset_count
            ),
            (1, 0, 3, 0)
        );
        let manifest = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(PLUGIN_MANIFEST_PATH)
                    .header(header::AUTHORIZATION, "Bearer example-token")
                    .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
                    .body(Body::from(
                        GetPluginManifestRequest {
                            name: "meta-skills".to_owned(),
                        }
                        .encode_to_vec(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(manifest.status(), StatusCode::OK);
        let bytes = to_bytes(manifest.into_body(), 64_000).await.unwrap();
        let manifest = PluginManifestResponse::decode(bytes).unwrap();
        assert_eq!(manifest.name, "meta-skills");
        assert!(manifest.tool_names.is_empty());
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&manifest.manifest_json).unwrap()["name"],
            "meta-skills"
        );
        let inventory = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(PLUGIN_MANIFEST_PATH)
                    .header(header::AUTHORIZATION, "Bearer example-token")
                    .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
                    .body(Body::from(
                        GetPluginManifestRequest {
                            name: "apple-security-devices".to_owned(),
                        }
                        .encode_to_vec(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(inventory.into_body(), 70_000).await.unwrap();
        let inventory = PluginManifestResponse::decode(bytes).unwrap();
        assert_eq!(inventory.tool_names.len(), 19);

        let installed = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(PLUGIN_INSTALL_PATH)
                    .header(header::AUTHORIZATION, "Bearer example-token")
                    .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
                    .body(Body::from(
                        InstallPluginRequest {
                            name: "meta-skills".to_owned(),
                        }
                        .encode_to_vec(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(installed.status(), StatusCode::OK);
        let body = to_bytes(installed.into_body(), 64_000).await.unwrap();
        let plugin = PluginSummary::decode(body).unwrap();
        assert_eq!(plugin.name, "meta-skills");
        assert!(plugin.installed);
        assert_eq!(db.installed_plugin_names().unwrap(), ["meta-skills"]);

        db.replace_workspaces(&[WorkspaceRow {
            id: "workspace-example".to_owned(),
            name: "Example workspace".to_owned(),
            directory: "example-directory".to_owned(),
        }])
        .unwrap();
        let scope = SetExtensionWorkspaceScopeRequest {
            id: "meta-skills".to_owned(),
            workspace_ids: vec!["workspace-example".to_owned()],
            all_workspaces: false,
        };
        let denied = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(PLUGIN_SCOPE_PATH)
                    .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
                    .body(Body::from(scope.encode_to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
        let saved = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(PLUGIN_SCOPE_PATH)
                    .header(header::AUTHORIZATION, "Bearer example-token")
                    .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
                    .body(Body::from(scope.encode_to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(saved.status(), StatusCode::OK);
        assert_eq!(
            db.extension_scope("plugin", "meta-skills")
                .unwrap()
                .workspace_ids,
            ["workspace-example"]
        );

        let unauthorized = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(PLUGIN_UNINSTALL_PATH)
                    .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
                    .body(Body::from(
                        UninstallPluginRequest {
                            name: "meta-skills".to_owned(),
                        }
                        .encode_to_vec(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(db.installed_plugin_names().unwrap(), ["meta-skills"]);

        let uninstalled = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(PLUGIN_UNINSTALL_PATH)
                    .header(header::AUTHORIZATION, "Bearer example-token")
                    .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
                    .body(Body::from(
                        UninstallPluginRequest {
                            name: "meta-skills".to_owned(),
                        }
                        .encode_to_vec(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(uninstalled.status(), StatusCode::OK);
        let body = to_bytes(uninstalled.into_body(), 64_000).await.unwrap();
        let plugin = PluginSummary::decode(body).unwrap();
        assert_eq!(plugin.name, "meta-skills");
        assert!(!plugin.installed);
        assert!(db.installed_plugin_names().unwrap().is_empty());
        drop(app);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
}
