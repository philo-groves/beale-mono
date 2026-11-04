use beale_contracts::{
    FEATURE_DETAILS_PATH, FEATURE_SCOPE_PATH, FEATURE_SET_ENABLED_PATH, FEATURES_PATH,
    PLUGIN_INSTALL_PATH, PLUGIN_MANIFEST_PATH, PLUGIN_SCOPE_PATH, PLUGIN_UNINSTALL_PATH,
    PLUGINS_PATH, PROTOBUF_CONTENT_TYPE,
    v1::{
        FeatureDetailsResponse, FeatureSummary, GetFeatureDetailsRequest, GetPluginManifestRequest,
        InstallPluginRequest, ListFeaturesResponse, ListPluginsResponse, PluginManifestResponse,
        PluginSummary, SetExtensionWorkspaceScopeRequest, SetFeatureEnabledRequest,
        UninstallPluginRequest,
    },
};
use prost::Message;
use reqwest::{Client, StatusCode, header::CONTENT_TYPE};
use rune::{Color, El, Icon, ScrollSpec, col, el, icon, row, text};
use serde::Deserialize;
use std::{collections::HashSet, net::SocketAddr};

use crate::{
    AUTH_HANDLE, ConnectionStatus, SelectionMenu, State, ThemeStyle, ViewLocation, connection,
    form_button,
    form_controls::toggle_button,
    icons::{
        EXTENSION_DEVICE, EXTENSION_FLAG, EXTENSION_LINKS, EXTENSION_WINDOWS, FEATURES, FLEET,
        INFO, PLUGINS, PLUS, RESEARCH_KITS, TARGET,
    },
    in_place_menu, modal_dialog, selection_button, sidenav_heading, ui_font,
    workspace_creation::field,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ExtensionSection {
    #[default]
    Plugins,
    Features,
    ResearchKits,
}

impl ExtensionSection {
    const ALL: [Self; 3] = [Self::Plugins, Self::Features, Self::ResearchKits];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Plugins => "Plugins",
            Self::Features => "Features",
            Self::ResearchKits => "Research Kits",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Plugins => "extensions-plugins",
            Self::Features => "extensions-features",
            Self::ResearchKits => "extensions-research-kits",
        }
    }

    fn glyph(self) -> &'static rune::Icon {
        match self {
            Self::Plugins => &PLUGINS,
            Self::Features => &FEATURES,
            Self::ResearchKits => &RESEARCH_KITS,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PluginTab {
    #[default]
    All,
    Available,
    Installed,
}

impl PluginTab {
    const ALL: [Self; 3] = [Self::All, Self::Available, Self::Installed];

    fn label(self) -> &'static str {
        match self {
            Self::All => "All Plugins",
            Self::Available => "Available",
            Self::Installed => "Installed",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::All => "plugin-tab-all",
            Self::Available => "plugin-tab-available",
            Self::Installed => "plugin-tab-installed",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum FeatureTab {
    #[default]
    All,
    Available,
    Enabled,
}

impl FeatureTab {
    const ALL: [Self; 3] = [Self::All, Self::Available, Self::Enabled];

    fn label(self) -> &'static str {
        match self {
            Self::All => "All Features",
            Self::Available => "Available",
            Self::Enabled => "Enabled",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::All => "feature-tab-all",
            Self::Available => "feature-tab-available",
            Self::Enabled => "feature-tab-enabled",
        }
    }
}

pub(crate) struct PluginEntry {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) installed: bool,
    pub(crate) skills: usize,
    pub(crate) mcp_servers: usize,
    pub(crate) mcp_tools: usize,
    pub(crate) workspace_ids: Vec<String>,
    pub(crate) all_workspaces: bool,
}

pub(crate) struct FeatureEntry {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) enabled: bool,
    pub(crate) has_prompt_extension: bool,
    pub(crate) tool_count: usize,
    pub(crate) workspace_ids: Vec<String>,
    pub(crate) all_workspaces: bool,
}

pub(crate) fn bundled_features() -> Vec<FeatureEntry> {
    vec![FeatureEntry {
        id: "goal-mode".to_owned(),
        name: "Goal Mode".to_owned(),
        description: "Keep working toward a New Research objective across turns, with optional turn and duration limits.".to_owned(),
        enabled: false,
        has_prompt_extension: true,
        tool_count: 1,
        workspace_ids: Vec::new(),
        all_workspaces: true,
    }, FeatureEntry {
        id: "fleet".to_owned(),
        name: "Fleet".to_owned(),
        description: "Open the Fleet area in the desktop app.".to_owned(),
        enabled: false,
        has_prompt_extension: false,
        tool_count: 0,
        workspace_ids: Vec::new(),
        all_workspaces: true,
    }]
}

pub(crate) fn fleet_enabled(state: &State) -> bool {
    state
        .features
        .iter()
        .any(|feature| feature.id == "fleet" && feature.enabled)
}

pub(crate) fn goal_mode_enabled(state: &State) -> bool {
    matches!(state.connection, ConnectionStatus::Connected { .. })
        && state.features.iter().any(|feature| {
            feature.id == "goal-mode"
                && feature.enabled
                && (feature.all_workspaces
                    || state
                        .new_research_workspace_id
                        .as_ref()
                        .is_some_and(|workspace_id| feature.workspace_ids.contains(workspace_id)))
        })
}

impl PluginEntry {
    fn display_name(&self) -> String {
        self.name
            .split('-')
            .map(|part| {
                let mut chars = part.chars();
                chars.next().map_or_else(String::new, |first| {
                    first.to_uppercase().collect::<String>() + chars.as_str()
                })
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Deserialize)]
struct PluginManifest {
    #[serde(rename = "$schema")]
    schema: String,
    name: String,
    description: String,
}

const BUNDLED_PACKAGES: [(&str, &str, Option<&str>, Option<&str>); 4] = [
    (
        include_str!("../../../extensions/plugins/apple-security-device-plugin/plugin.json"),
        include_str!(
            "../../../extensions/plugins/apple-security-device-plugin/skills/apple-security-devices/SKILL.md"
        ),
        Some(include_str!(
            "../../../extensions/plugins/apple-security-device-plugin/mcp.json"
        )),
        Some(include_str!(
            "../../../extensions/apple-security-device-tools.json"
        )),
    ),
    (
        include_str!("../../../extensions/plugins/apple-target-flags-plugin/plugin.json"),
        include_str!(
            "../../../extensions/plugins/apple-target-flags-plugin/skills/apple-target-flags/SKILL.md"
        ),
        None,
        None,
    ),
    (
        include_str!("../../../extensions/plugins/meta-skills-plugin/plugin.json"),
        include_str!(
            "../../../extensions/plugins/meta-skills-plugin/skills/meta-bug-bounty-tools/SKILL.md"
        ),
        None,
        None,
    ),
    (
        include_str!("../../../extensions/plugins/microsoft-security-devices-plugin/plugin.json"),
        include_str!(
            "../../../extensions/plugins/microsoft-security-devices-plugin/skills/microsoft-security-devices/SKILL.md"
        ),
        Some(include_str!(
            "../../../extensions/plugins/microsoft-security-devices-plugin/mcp.json"
        )),
        None,
    ),
];

pub(crate) fn bundled_plugins() -> Vec<PluginEntry> {
    BUNDLED_PACKAGES
        .into_iter()
        .map(|(source, skill, mcp, inventory)| {
            let manifest: PluginManifest =
                serde_json::from_str(source).expect("bundled plugin manifest is valid JSON");
            assert_eq!(
                manifest.schema, "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
                "bundled plugin must use Agent Plugins 1.0"
            );
            assert!(
                skill
                    .lines()
                    .next()
                    .is_some_and(|line| line.trim() == "---"),
                "bundled skill needs frontmatter"
            );
            let mcp_servers = mcp.map_or(0, |source| {
                let config: serde_json::Value =
                    serde_json::from_str(source).expect("bundled MCP configuration is valid JSON");
                assert_eq!(
                    config["$schema"].as_str(),
                    Some("https://agent-plugins.org/schemas/1.0.0/mcp.schema.json"),
                    "bundled MCP configuration must use Agent Plugins 1.0"
                );
                config["mcpServers"]
                    .as_object()
                    .expect("bundled MCP servers must be an object")
                    .len()
            });
            let mcp_tools = inventory.map_or(0, |source| {
                let names: Vec<String> =
                    serde_json::from_str(source).expect("bundled MCP tool inventory is valid JSON");
                assert!(mcp_servers > 0, "MCP tools require a server");
                names.len()
            });
            PluginEntry {
                name: manifest.name,
                description: manifest.description,
                installed: false,
                skills: 1,
                mcp_servers,
                mcp_tools,
                workspace_ids: Vec::new(),
                all_workspaces: true,
            }
        })
        .collect()
}

fn from_wire(plugin: PluginSummary) -> Result<PluginEntry, String> {
    if plugin.name.is_empty()
        || plugin.name.len() > 64
        || plugin.description.len() > 2_000
        || plugin.skill_count > 128
        || plugin.mcp_server_count > 128
        || plugin.mcp_tool_count > 4_096 * 128
        || plugin.has_mcp_server != (plugin.mcp_server_count > 0)
        || (plugin.mcp_server_count == 0 && plugin.mcp_tool_count != 0)
        || !valid_workspace_ids(&plugin.workspace_ids)
        || (plugin.all_workspaces && !plugin.workspace_ids.is_empty())
    {
        return Err("Invalid plugin catalog".to_owned());
    }
    Ok(PluginEntry {
        name: plugin.name,
        description: plugin.description,
        installed: plugin.installed,
        skills: plugin.skill_count as usize,
        mcp_servers: plugin.mcp_server_count as usize,
        mcp_tools: plugin.mcp_tool_count as usize,
        workspace_ids: plugin.workspace_ids,
        all_workspaces: plugin.all_workspaces,
    })
}

async fn decode<T: Message + Default>(response: reqwest::Response) -> Result<T, String> {
    if response.status() != StatusCode::OK
        || response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            != Some(PROTOBUF_CONTENT_TYPE)
    {
        return Err(format!("Extension request failed ({})", response.status()));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "Could not read extension response".to_owned())?;
    if bytes.len() > 70_000 {
        return Err("Extension response is too large".to_owned());
    }
    T::decode(bytes).map_err(|_| "Invalid extension response".to_owned())
}

async fn load(client: &Client, addr: SocketAddr) -> Result<Vec<PluginEntry>, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let response: ListPluginsResponse = decode(
        client
            .get(format!("http://{addr}{PLUGINS_PATH}"))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| "Could not load plugins".to_owned())?,
    )
    .await?;
    if response.plugins.len() > 128 {
        return Err("Invalid plugin catalog".to_owned());
    }
    let mut names = HashSet::new();
    let mut plugins = Vec::with_capacity(response.plugins.len());
    for plugin in response.plugins {
        if !names.insert(plugin.name.clone()) {
            return Err("Invalid plugin catalog".to_owned());
        }
        plugins.push(from_wire(plugin)?);
    }
    Ok(plugins)
}

fn feature_from_wire(feature: FeatureSummary) -> Result<FeatureEntry, String> {
    if feature.id.is_empty()
        || feature.id.len() > 64
        || !feature
            .id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || feature.name.trim().is_empty()
        || feature.name.len() > 80
        || feature.description.len() > 2_000
        || feature.tool_count > 128
        || !valid_workspace_ids(&feature.workspace_ids)
        || (feature.all_workspaces && !feature.workspace_ids.is_empty())
    {
        return Err("Invalid feature catalog".to_owned());
    }
    Ok(FeatureEntry {
        id: feature.id,
        name: feature.name,
        description: feature.description,
        enabled: feature.enabled,
        has_prompt_extension: feature.has_prompt_extension,
        tool_count: feature.tool_count as usize,
        workspace_ids: feature.workspace_ids,
        all_workspaces: feature.all_workspaces,
    })
}

fn valid_workspace_ids(ids: &[String]) -> bool {
    ids.len() <= 128
        && ids.iter().all(|id| uuid::Uuid::parse_str(id).is_ok())
        && ids.iter().collect::<HashSet<_>>().len() == ids.len()
}

async fn load_features(client: &Client, addr: SocketAddr) -> Result<Vec<FeatureEntry>, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let response: ListFeaturesResponse = decode(
        client
            .get(format!("http://{addr}{FEATURES_PATH}"))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| "Could not load features".to_owned())?,
    )
    .await?;
    if response.features.len() > 128 {
        return Err("Invalid feature catalog".to_owned());
    }
    let mut names = HashSet::new();
    response
        .features
        .into_iter()
        .map(feature_from_wire)
        .map(|feature| {
            let feature = feature?;
            if !names.insert(feature.id.clone()) {
                return Err("Invalid feature catalog".to_owned());
            }
            Ok(feature)
        })
        .collect()
}

async fn set_feature_enabled(
    client: &Client,
    addr: SocketAddr,
    id: &str,
    enabled: bool,
) -> Result<FeatureEntry, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let response: FeatureSummary = decode(
        client
            .post(format!("http://{addr}{FEATURE_SET_ENABLED_PATH}"))
            .bearer_auth(token)
            .header("content-type", PROTOBUF_CONTENT_TYPE)
            .body(
                SetFeatureEnabledRequest {
                    id: id.to_owned(),
                    enabled,
                }
                .encode_to_vec(),
            )
            .send()
            .await
            .map_err(|_| "Could not update feature".to_owned())?,
    )
    .await?;
    if response.id != id || response.enabled != enabled {
        return Err("Invalid feature update response".to_owned());
    }
    feature_from_wire(response)
}

async fn set_workspace_scope(
    client: &Client,
    addr: SocketAddr,
    menu: SelectionMenu,
    id: &str,
    all_workspaces: bool,
    workspace_ids: Vec<String>,
) -> Result<(bool, Vec<String>), String> {
    let path = match menu {
        SelectionMenu::PluginWorkspaces { .. } => PLUGIN_SCOPE_PATH,
        SelectionMenu::FeatureWorkspaces { .. } => FEATURE_SCOPE_PATH,
        _ => return Err("Invalid workspace scope target".to_owned()),
    };
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let response: SetExtensionWorkspaceScopeRequest = decode(
        client
            .post(format!("http://{addr}{path}"))
            .bearer_auth(token)
            .header("content-type", PROTOBUF_CONTENT_TYPE)
            .body(
                SetExtensionWorkspaceScopeRequest {
                    id: id.to_owned(),
                    workspace_ids: workspace_ids.clone(),
                    all_workspaces,
                }
                .encode_to_vec(),
            )
            .send()
            .await
            .map_err(|_| "Could not update workspace scope".to_owned())?,
    )
    .await?;
    if response.id != id
        || response.all_workspaces != all_workspaces
        || !valid_workspace_ids(&response.workspace_ids)
        || response.workspace_ids.iter().collect::<HashSet<_>>()
            != workspace_ids.iter().collect::<HashSet<_>>()
    {
        return Err("Invalid workspace scope response".to_owned());
    }
    Ok((response.all_workspaces, response.workspace_ids))
}

async fn install(client: &Client, addr: SocketAddr, name: &str) -> Result<PluginEntry, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let response: PluginSummary = decode(
        client
            .post(format!("http://{addr}{PLUGIN_INSTALL_PATH}"))
            .bearer_auth(token)
            .header("content-type", PROTOBUF_CONTENT_TYPE)
            .body(
                InstallPluginRequest {
                    name: name.to_owned(),
                }
                .encode_to_vec(),
            )
            .send()
            .await
            .map_err(|_| "Could not install plugin".to_owned())?,
    )
    .await?;
    if response.name != name || !response.installed {
        return Err("Invalid install response".to_owned());
    }
    from_wire(response)
}

async fn uninstall(client: &Client, addr: SocketAddr, name: &str) -> Result<PluginEntry, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let response: PluginSummary = decode(
        client
            .post(format!("http://{addr}{PLUGIN_UNINSTALL_PATH}"))
            .bearer_auth(token)
            .header("content-type", PROTOBUF_CONTENT_TYPE)
            .body(
                UninstallPluginRequest {
                    name: name.to_owned(),
                }
                .encode_to_vec(),
            )
            .send()
            .await
            .map_err(|_| "Could not uninstall plugin".to_owned())?,
    )
    .await?;
    if response.name != name || response.installed {
        return Err("Invalid uninstall response".to_owned());
    }
    from_wire(response)
}

fn format_manifest(source: &str, name: &str) -> Result<String, String> {
    if source.len() > 64_000 {
        return Err("Plugin manifest is too large".to_owned());
    }
    let value: serde_json::Value =
        serde_json::from_str(source).map_err(|_| "Invalid plugin manifest".to_owned())?;
    if value.get("name").and_then(serde_json::Value::as_str) != Some(name) {
        return Err("Invalid plugin manifest".to_owned());
    }
    serde_json::to_string_pretty(&value).map_err(|_| "Invalid plugin manifest".to_owned())
}

async fn manifest(client: &Client, addr: SocketAddr, name: &str) -> Result<String, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let response: PluginManifestResponse = decode(
        client
            .post(format!("http://{addr}{PLUGIN_MANIFEST_PATH}"))
            .bearer_auth(token)
            .header("content-type", PROTOBUF_CONTENT_TYPE)
            .body(
                GetPluginManifestRequest {
                    name: name.to_owned(),
                }
                .encode_to_vec(),
            )
            .send()
            .await
            .map_err(|_| "Could not load plugin manifest".to_owned())?,
    )
    .await?;
    if response.name != name {
        return Err("Invalid plugin manifest response".to_owned());
    }
    format_manifest(&response.manifest_json, name)
}

async fn feature_details(
    client: &Client,
    addr: SocketAddr,
    id: &str,
) -> Result<FeatureDetailsResponse, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let response: FeatureDetailsResponse = decode(
        client
            .post(format!("http://{addr}{FEATURE_DETAILS_PATH}"))
            .bearer_auth(token)
            .header("content-type", PROTOBUF_CONTENT_TYPE)
            .body(GetFeatureDetailsRequest { id: id.to_owned() }.encode_to_vec())
            .send()
            .await
            .map_err(|_| "Could not load feature details".to_owned())?,
    )
    .await?;
    if response.id != id
        || response.name.trim().is_empty()
        || response.name.len() > 80
        || response.description.len() > 2_000
        || response.prompt_extension.len() > 24_000
    {
        return Err("Invalid feature details".to_owned());
    }
    Ok(response)
}

fn start_feature_details(state: &mut State, id: String) {
    state.feature_details_generation = state.feature_details_generation.wrapping_add(1);
    let generation = state.feature_details_generation;
    state.feature_details_id = Some(id.clone());
    state.feature_details = None;
    state.feature_details_error.clear();
    state.feature_details_loading = true;
    if !matches!(state.connection, ConnectionStatus::Connected { .. }) {
        state.feature_details_loading = false;
        state.feature_details_error =
            "Connect to the app-server to view feature details.".to_owned();
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        state.feature_details_loading = false;
        state.feature_details_error = "App-server is not ready.".to_owned();
        return;
    };
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid app-server address".to_owned())?;
            let client = connection::http_client()
                .map_err(|_| "Could not initialize connection".to_owned())?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize connection".to_owned())?;
            runtime.block_on(feature_details(&client, addr, &id))
        })();
        handle.update(move |state, _| {
            if state.feature_details_generation != generation
                || state.feature_details_id.as_deref() != Some(&id)
            {
                return;
            }
            state.feature_details_loading = false;
            match result {
                Ok(details) => state.feature_details = Some(details),
                Err(error) => state.feature_details_error = error,
            }
        });
    });
}

fn start_manifest(state: &mut State, name: String) {
    state.plugin_manifest_generation = state.plugin_manifest_generation.wrapping_add(1);
    let generation = state.plugin_manifest_generation;
    state.plugin_manifest_name = Some(name.clone());
    state.plugin_manifest_text.clear();
    state.plugin_manifest_error.clear();
    state.plugin_manifest_loading = true;
    if !matches!(state.connection, ConnectionStatus::Connected { .. }) {
        let source = BUNDLED_PACKAGES.iter().find_map(|(source, _, _, _)| {
            let value: serde_json::Value = serde_json::from_str(source).ok()?;
            (value.get("name")?.as_str()? == name).then_some(*source)
        });
        match source.map(|source| format_manifest(source, &name)) {
            Some(Ok(text)) => state.plugin_manifest_text = text,
            Some(Err(error)) => state.plugin_manifest_error = error,
            None => {
                state.plugin_manifest_error =
                    "Connect to the app-server to view this manifest.".to_owned()
            }
        }
        state.plugin_manifest_loading = false;
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        state.plugin_manifest_loading = false;
        state.plugin_manifest_error = "App-server is not ready.".to_owned();
        return;
    };
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid app-server address".to_owned())?;
            let client = connection::http_client()
                .map_err(|_| "Could not initialize connection".to_owned())?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize connection".to_owned())?;
            runtime.block_on(manifest(&client, addr, &name))
        })();
        handle.update(move |state, _| {
            if state.plugin_manifest_generation != generation
                || state.plugin_manifest_name.as_deref() != Some(&name)
            {
                return;
            }
            state.plugin_manifest_loading = false;
            match result {
                Ok(text) => state.plugin_manifest_text = text,
                Err(error) => state.plugin_manifest_error = error,
            }
        });
    });
}

pub(crate) fn start_load(state: &mut State) {
    start_feature_load(state);
    if state.plugin_loading
        || state.plugin_installing.is_some()
        || state.plugin_uninstalling.is_some()
        || state.extension_scope_saving.is_some()
        || !matches!(state.connection, ConnectionStatus::Connected { .. })
    {
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        return;
    };
    state.plugin_loading = true;
    state.plugin_error.clear();
    state.plugin_generation = state.plugin_generation.wrapping_add(1);
    let generation = state.plugin_generation;
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid app-server address".to_owned())?;
            let client = connection::http_client()
                .map_err(|_| "Could not initialize connection".to_owned())?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize connection".to_owned())?;
            runtime.block_on(load(&client, addr))
        })();
        handle.update(move |state, _| {
            if state.plugin_generation != generation {
                return;
            }
            state.plugin_loading = false;
            match result {
                Ok(plugins) => state.plugins = plugins,
                Err(error) => state.plugin_error = error,
            }
        });
    });
}

pub(crate) fn start_feature_load(state: &mut State) {
    if state.feature_loading
        || state.feature_busy
        || state.extension_scope_saving.is_some()
        || !matches!(state.connection, ConnectionStatus::Connected { .. })
    {
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        return;
    };
    state.feature_loading = true;
    state.feature_generation = state.feature_generation.wrapping_add(1);
    let generation = state.feature_generation;
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid app-server address".to_owned())?;
            let client = connection::http_client()
                .map_err(|_| "Could not initialize connection".to_owned())?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize connection".to_owned())?;
            runtime.block_on(load_features(&client, addr))
        })();
        handle.update(move |state, _| {
            if state.feature_generation != generation {
                return;
            }
            state.feature_loading = false;
            match result {
                Ok(features) => {
                    state.features = features;
                    state.feature_error.clear();
                }
                Err(error) => {
                    state.features = bundled_features();
                    state.feature_error = error;
                }
            }
            if !goal_mode_enabled(state) {
                state.new_research_goal_mode = false;
                if state.selection_menu == Some(crate::SelectionMenu::GoalMode) {
                    state.selection_menu = None;
                }
            }
            if state.primary_view == crate::PrimaryView::Fleet && !fleet_enabled(state) {
                state.navigate_to(ViewLocation::Home);
            }
        });
    });
}

fn start_feature_toggle(state: &mut State, id: String, enabled: bool) {
    if state.feature_busy {
        return;
    }
    if !matches!(state.connection, ConnectionStatus::Connected { .. }) {
        state.feature_error = "App-server is not connected.".to_owned();
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        state.feature_error = "App-server is not ready.".to_owned();
        return;
    };
    state.feature_busy = true;
    state.feature_loading = false;
    state.feature_error.clear();
    state.feature_generation = state.feature_generation.wrapping_add(1);
    let generation = state.feature_generation;
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid app-server address".to_owned())?;
            let client = connection::http_client()
                .map_err(|_| "Could not initialize connection".to_owned())?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize connection".to_owned())?;
            runtime.block_on(set_feature_enabled(&client, addr, &id, enabled))
        })();
        handle.update(move |state, _| {
            if state.feature_generation != generation {
                return;
            }
            state.feature_busy = false;
            match result {
                Ok(feature) => {
                    if let Some(saved) =
                        state.features.iter_mut().find(|item| item.id == feature.id)
                    {
                        *saved = feature;
                    } else {
                        state.features.push(feature);
                    }
                    if !goal_mode_enabled(state) {
                        state.new_research_goal_mode = false;
                        if state.selection_menu == Some(crate::SelectionMenu::GoalMode) {
                            state.selection_menu = None;
                        }
                    }
                    if state.primary_view == crate::PrimaryView::Fleet && !fleet_enabled(state) {
                        state.navigate_to(ViewLocation::Home);
                    }
                }
                Err(error) => state.feature_error = error,
            }
        });
    });
}

pub(crate) fn start_workspace_scope_save(
    state: &mut State,
    menu: SelectionMenu,
    all_workspaces: bool,
    workspace_ids: Vec<String>,
) {
    if state.extension_scope_saving.is_some()
        || !valid_workspace_ids(&workspace_ids)
        || (all_workspaces && !workspace_ids.is_empty())
    {
        return;
    }
    let (id, is_plugin) = match menu {
        SelectionMenu::PluginWorkspaces { index } => match state.plugins.get(index) {
            Some(plugin) if plugin.installed => (plugin.name.clone(), true),
            _ => return,
        },
        SelectionMenu::FeatureWorkspaces { index } => match state.features.get(index) {
            Some(feature) if feature.enabled => (feature.id.clone(), false),
            _ => return,
        },
        _ => return,
    };
    if !matches!(state.connection, ConnectionStatus::Connected { .. }) {
        let error = "App-server is not connected.".to_owned();
        if is_plugin {
            state.plugin_error = error;
        } else {
            state.feature_error = error;
        }
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        return;
    };
    state.extension_scope_saving = Some(menu);
    state.extension_scope_generation = state.extension_scope_generation.wrapping_add(1);
    let generation = state.extension_scope_generation;
    if is_plugin {
        state.plugin_loading = false;
        state.plugin_generation = state.plugin_generation.wrapping_add(1);
        state.plugin_error.clear();
    } else {
        state.feature_loading = false;
        state.feature_generation = state.feature_generation.wrapping_add(1);
        state.feature_error.clear();
    }
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid app-server address".to_owned())?;
            let client = connection::http_client()
                .map_err(|_| "Could not initialize connection".to_owned())?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize connection".to_owned())?;
            runtime.block_on(set_workspace_scope(
                &client,
                addr,
                menu,
                &id,
                all_workspaces,
                workspace_ids,
            ))
        })();
        handle.update(move |state, _| {
            if state.extension_scope_generation != generation {
                return;
            }
            state.extension_scope_saving = None;
            match result {
                Ok((all, ids)) if is_plugin => {
                    if let Some(plugin) = state.plugins.iter_mut().find(|entry| entry.name == id) {
                        plugin.workspace_ids = ids;
                        plugin.all_workspaces = all;
                    }
                }
                Ok((all, ids)) => {
                    if let Some(feature) = state.features.iter_mut().find(|entry| entry.id == id) {
                        feature.workspace_ids = ids;
                        feature.all_workspaces = all;
                    }
                    if !goal_mode_enabled(state) {
                        state.new_research_goal_mode = false;
                    }
                }
                Err(error) if is_plugin => state.plugin_error = error,
                Err(error) => state.feature_error = error,
            }
        });
    });
}

fn start_install(state: &mut State, name: String) {
    if state.plugin_installing.is_some() || state.plugin_uninstalling.is_some() {
        return;
    }
    if !matches!(state.connection, ConnectionStatus::Connected { .. }) {
        state.plugin_error = "App-server is not connected.".to_owned();
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        state.plugin_error = "App-server is not ready.".to_owned();
        return;
    };
    state.plugin_installing = Some(name.clone());
    state.plugin_error.clear();
    state.plugin_generation = state.plugin_generation.wrapping_add(1);
    state.plugin_loading = false;
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid app-server address".to_owned())?;
            let client = connection::http_client()
                .map_err(|_| "Could not initialize connection".to_owned())?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize connection".to_owned())?;
            runtime.block_on(install(&client, addr, &name))
        })();
        handle.update(move |state, _| {
            if state.plugin_installing.as_deref() != Some(name.as_str()) {
                return;
            }
            state.plugin_installing = None;
            match result {
                Ok(plugin) => {
                    if let Some(existing) = state.plugins.iter_mut().find(|item| item.name == name)
                    {
                        *existing = plugin;
                    } else {
                        state.plugins.push(plugin);
                    }
                }
                Err(error) => state.plugin_error = error,
            }
        });
    });
}

fn start_uninstall(state: &mut State, name: String) {
    if state.plugin_installing.is_some() || state.plugin_uninstalling.is_some() {
        return;
    }
    if !matches!(state.connection, ConnectionStatus::Connected { .. }) {
        state.plugin_error = "App-server is not connected.".to_owned();
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        state.plugin_error = "App-server is not ready.".to_owned();
        return;
    };
    state.plugin_uninstalling = Some(name.clone());
    state.plugin_error.clear();
    state.plugin_generation = state.plugin_generation.wrapping_add(1);
    state.plugin_loading = false;
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid app-server address".to_owned())?;
            let client = connection::http_client()
                .map_err(|_| "Could not initialize connection".to_owned())?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize connection".to_owned())?;
            runtime.block_on(uninstall(&client, addr, &name))
        })();
        handle.update(move |state, _| {
            if state.plugin_uninstalling.as_deref() != Some(name.as_str()) {
                return;
            }
            state.plugin_uninstalling = None;
            match result {
                Ok(plugin) => {
                    if let Some(existing) = state.plugins.iter_mut().find(|item| item.name == name)
                    {
                        *existing = plugin;
                    } else {
                        state.plugins.push(plugin);
                    }
                }
                Err(error) => state.plugin_error = error,
            }
        });
    });
}

pub(crate) fn sidenav(state: &State, width: f32, theme: ThemeStyle<'_>) -> El {
    let mut items = col().w_full().gap(2.0);
    for section in ExtensionSection::ALL {
        let active = state.extensions_section == section;
        items = items.child(
            row()
                .key(section.key())
                .role("button")
                .label(section.label())
                .w_full()
                .h(30.0)
                .px(8.0)
                .center_y()
                .gap(12.0)
                .radius(8.0)
                .fill(if active {
                    theme.color("#282828")
                } else {
                    Color::TRANSPARENT
                })
                .hover(theme.color("#303030"))
                .on_press(move |state: &mut State| {
                    state.navigate_to(ViewLocation::Extensions(
                        section,
                        state.plugin_tab,
                        state.feature_tab,
                    ));
                })
                .child(icon(section.glyph(), 16.0).color(theme.color(if active {
                    "#f2f2f2"
                } else {
                    "#bdbdbd"
                })))
                .child(
                    text(section.label())
                        .font(ui_font(14.0, if active { 550.0 } else { 400.0 }))
                        .color(theme.color(if active { "#f2f2f2" } else { "#c8c8c8" })),
                ),
        );
    }
    col()
        .key("extensions-sidenav")
        .role("navigation")
        .label("Extensions sections")
        .w(width)
        .h_full()
        .px(8.0)
        .pt(12.0)
        .fill(theme.color("#111111"))
        .child(
            sidenav_heading("Extensions", theme)
                .key("extensions-sidenav-title")
                .ml(4.0)
                .mb(12.0),
        )
        .child(items)
}

fn tab_button(tab: PluginTab, selected: bool, theme: ThemeStyle<'_>) -> El {
    row()
        .key(tab.key())
        .role("tab")
        .label(format!(
            "{} plugins{}",
            tab.label(),
            if selected { ", selected" } else { "" }
        ))
        .h(26.0)
        .px(10.0)
        .center_y()
        .radius(9.0)
        .continuous_corners()
        .fill(theme.color(if selected { "#414141" } else { "#242424" }))
        .hover(theme.color("#4a4a4a"))
        .on_press(move |state: &mut State| {
            state.navigate_to(ViewLocation::Extensions(
                ExtensionSection::Plugins,
                tab,
                state.feature_tab,
            ));
        })
        .child(
            text(tab.label())
                .font(ui_font(14.0, if selected { 600.0 } else { 450.0 }))
                .color(theme.color(if selected { "#f2f2f2" } else { "#c8c8c8" })),
        )
}

fn plugin_rows(state: &State) -> impl Iterator<Item = (usize, &PluginEntry)> {
    state
        .plugins
        .iter()
        .enumerate()
        .filter(|(_, plugin)| match state.plugin_tab {
            PluginTab::All => true,
            PluginTab::Available => !plugin.installed,
            PluginTab::Installed => plugin.installed,
        })
}

fn feature_tab_button(tab: FeatureTab, selected: bool, theme: ThemeStyle<'_>) -> El {
    row()
        .key(tab.key())
        .role("tab")
        .label(format!(
            "{} features{}",
            tab.label(),
            if selected { ", selected" } else { "" }
        ))
        .h(26.0)
        .px(10.0)
        .center_y()
        .radius(9.0)
        .continuous_corners()
        .fill(theme.color(if selected { "#414141" } else { "#242424" }))
        .hover(theme.color("#4a4a4a"))
        .on_press(move |state: &mut State| {
            state.navigate_to(ViewLocation::Extensions(
                ExtensionSection::Features,
                state.plugin_tab,
                tab,
            ));
        })
        .child(
            text(tab.label())
                .font(ui_font(14.0, if selected { 600.0 } else { 450.0 }))
                .color(theme.color(if selected { "#f2f2f2" } else { "#c8c8c8" })),
        )
}

fn feature_rows(state: &State) -> impl Iterator<Item = (usize, &FeatureEntry)> {
    state
        .features
        .iter()
        .enumerate()
        .filter(|(_, feature)| match state.feature_tab {
            FeatureTab::All => true,
            FeatureTab::Available => !feature.enabled,
            FeatureTab::Enabled => feature.enabled,
        })
}

fn workspace_scope(state: &State, menu: SelectionMenu) -> Option<(bool, &[String])> {
    match menu {
        SelectionMenu::PluginWorkspaces { index } => state
            .plugins
            .get(index)
            .filter(|plugin| plugin.installed)
            .map(|plugin| (plugin.all_workspaces, plugin.workspace_ids.as_slice())),
        SelectionMenu::FeatureWorkspaces { index } => state
            .features
            .get(index)
            .filter(|feature| feature.enabled)
            .map(|feature| (feature.all_workspaces, feature.workspace_ids.as_slice())),
        _ => None,
    }
}

pub(crate) fn workspace_scope_items(
    state: &State,
    menu: SelectionMenu,
) -> Vec<in_place_menu::MenuItem> {
    let Some((all_workspaces, scope)) = workspace_scope(state, menu) else {
        return Vec::new();
    };
    std::iter::once(in_place_menu::MenuItem {
        value: String::new(),
        label: "All Workspaces".to_owned(),
        detail: None,
        selected: all_workspaces,
    })
    .chain(
        state
            .workspaces
            .iter()
            .map(|workspace| in_place_menu::MenuItem {
                value: workspace.id.clone(),
                label: workspace.name.clone(),
                detail: None,
                selected: scope.contains(&workspace.id),
            }),
    )
    .collect()
}

pub(crate) fn next_workspace_scope(
    state: &State,
    menu: SelectionMenu,
    value: &str,
) -> Option<(bool, Vec<String>)> {
    let (all_workspaces, scope) = workspace_scope(state, menu)?;
    if value.is_empty() {
        return Some((true, Vec::new()));
    }
    if !state
        .workspaces
        .iter()
        .any(|workspace| workspace.id == value)
    {
        return None;
    }
    if all_workspaces {
        return Some((false, vec![value.to_owned()]));
    }
    let mut next = scope.to_vec();
    if let Some(index) = next.iter().position(|id| id == value) {
        next.remove(index);
    } else {
        next.push(value.to_owned());
    }
    Some((false, next))
}

fn workspace_scope_label(state: &State, all_workspaces: bool, ids: &[String]) -> String {
    if all_workspaces {
        return "All Workspaces".to_owned();
    }
    match ids {
        [] => "No Workspaces".to_owned(),
        [id] => state
            .workspaces
            .iter()
            .find(|workspace| workspace.id == *id)
            .map_or_else(
                || "1 Workspace".to_owned(),
                |workspace| workspace.name.clone(),
            ),
        _ => format!("{} Workspaces", ids.len()),
    }
}

fn stat_row(label: &'static str, value: impl ToString, theme: ThemeStyle<'_>) -> El {
    row()
        .w_full()
        .h(32.0)
        .center_y()
        .child(
            text(label)
                .grow(1.0)
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#969696")),
        )
        .child(
            text(value.to_string())
                .font(ui_font(13.0, 500.0))
                .color(theme.color("#f2f2f2")),
        )
}

fn stat_block(rows: impl IntoIterator<Item = El>, theme: ThemeStyle<'_>) -> El {
    let divider = || el().w_full().h(1.0).fill(theme.color("#303030"));
    let mut block = col().w_full().mt(16.0).child(divider());
    for row in rows {
        block = block.child(row);
    }
    block.child(divider())
}

fn extension_mark(
    glyph: &'static Icon,
    background: &str,
    foreground: &str,
    theme: ThemeStyle<'_>,
) -> El {
    el().size(40.0, 40.0)
        .radius(12.0)
        .continuous_corners()
        .fill(theme.color(background))
        .center()
        .child(icon(glyph, 23.0).color(theme.color(foreground)))
}

fn plugin_mark(name: &str, theme: ThemeStyle<'_>) -> El {
    let (glyph, background, foreground) = match name {
        "apple-security-devices" => (&EXTENSION_DEVICE, "#2b3d4b", "#c8e0ed"),
        "apple-target-flags" => (&EXTENSION_FLAG, "#49343c", "#f0cad4"),
        "meta-skills" => (&EXTENSION_LINKS, "#273e5c", "#b8d9fa"),
        "microsoft-security-devices" => (&EXTENSION_WINDOWS, "#344454", "#d2e6f3"),
        _ => (&PLUGINS, "#303030", "#e2e2e2"),
    };
    extension_mark(glyph, background, foreground, theme)
}

fn feature_mark(id: &str, theme: ThemeStyle<'_>) -> El {
    let (glyph, background, foreground) = match id {
        "goal-mode" => (&TARGET, "#4b3b2b", "#f2d6a2"),
        "fleet" => (&FLEET, "#34404b", "#c9deeb"),
        _ => (&FEATURES, "#303030", "#e2e2e2"),
    };
    extension_mark(glyph, background, foreground, theme)
}

fn plugin_card(index: usize, plugin: &PluginEntry, state: &State, theme: ThemeStyle<'_>) -> El {
    let stats = stat_block(
        [
            stat_row("Skills", plugin.skills, theme),
            stat_row(
                "MCP Servers (Tools)",
                format!("{} ({})", plugin.mcp_servers, plugin.mcp_tools),
                theme,
            )
            .key(format!("plugin-mcp-count-{}", plugin.name)),
        ],
        theme,
    );
    let available = matches!(state.connection, ConnectionStatus::Connected { .. })
        && state.plugin_installing.is_none()
        && state.plugin_uninstalling.is_none()
        && state.extension_scope_saving.is_none();
    let name = plugin.name.clone();
    let installed = plugin.installed;
    let mut action = toggle_button(
        format!("plugin-toggle-{}", plugin.name),
        format!("{} plugin", plugin.display_name()),
        plugin.installed,
        available,
        theme,
    );
    if available {
        action = action.on_press(move |state: &mut State| {
            if installed {
                start_uninstall(state, name.clone());
            } else {
                start_install(state, name.clone());
            }
        });
    }
    col()
        .key(format!("plugin-item-{}", plugin.name))
        .w_full()
        .min_w(0.0)
        .px(12.0)
        .pt(16.0)
        .pb(14.0)
        .radius(24.0)
        .continuous_corners()
        .border(1.0, theme.color("#303030"))
        .child(
            row()
                .w_full()
                .h(40.0)
                .gap(10.0)
                .center_y()
                .child(plugin_mark(&plugin.name, theme).key(format!("plugin-logo-{}", plugin.name)))
                .child(
                    text(plugin.display_name())
                        .grow(1.0)
                        .min_w(0.0)
                        .font(ui_font(16.0, 600.0))
                        .color(theme.color("#f2f2f2"))
                        .max_lines(2),
                ),
        )
        .child(
            text(plugin.description.clone())
                .key(format!("plugin-description-{}", plugin.name))
                .w_full()
                .mt(10.0)
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#969696"))
                .max_lines(3),
        )
        .child(el().grow(1.0))
        .child(stats)
        .child(
            row()
                .w_full()
                .pt(12.0)
                .center_y()
                .child(action)
                .child(if plugin.installed {
                    let menu = SelectionMenu::PluginWorkspaces { index };
                    selection_button(
                        workspace_scope_label(state, plugin.all_workspaces, &plugin.workspace_ids),
                        state.selection_menu == Some(menu),
                        move |state| crate::toggle_selection_menu(state, menu),
                        theme,
                    )
                    .key(format!("plugin-scope-{index}"))
                    .ml(5.0)
                    .max_w(160.0)
                } else {
                    el().size(0.0, 0.0)
                })
                .child(el().grow(1.0))
                .child(
                    el().key(format!("plugin-manifest-{}", plugin.name))
                        .role("button")
                        .label(format!("View {} plugin manifest", plugin.display_name()))
                        .help("View plugin manifest")
                        .size(26.0, 26.0)
                        .mr(-5.0)
                        .center()
                        .radius(8.0)
                        .continuous_corners()
                        .hover(theme.color("#303030"))
                        .on_press({
                            let name = plugin.name.clone();
                            move |state: &mut State| start_manifest(state, name.clone())
                        })
                        .child(icon(&INFO, 17.0).color(theme.color("#bdbdbd"))),
                ),
        )
}

fn plugin_columns(page_width: f32) -> usize {
    const MIN_CARD_WIDTH: f32 = 250.0;
    const GAP: f32 = 16.0;
    ((page_width + GAP) / (MIN_CARD_WIDTH + GAP))
        .floor()
        .clamp(1.0, 4.0) as usize
}

fn plugins(state: &State, theme: ThemeStyle<'_>, page_width: f32) -> Vec<El> {
    let mut tabs = row()
        .key("plugin-view-tabs")
        .role("tablist")
        .label("Plugin views")
        .w_full()
        .h(26.0)
        .gap(6.0)
        .center_y();
    for tab in PluginTab::ALL {
        tabs = tabs.child(tab_button(tab, state.plugin_tab == tab, theme));
    }
    tabs = tabs.child(el().grow(1.0)).child(
        el().key("plugin-add-source")
            .role("button")
            .label("Add a plugin from an external source")
            .size(26.0, 26.0)
            .center()
            .radius(8.0)
            .continuous_corners()
            .hover(theme.color("#303030"))
            .on_press(|state: &mut State| {
                state.plugin_add_dialog_open = true;
                state.plugin_source_edit.clear();
            })
            .child(icon(&PLUS, 17.0).color(theme.color("#c8c8c8"))),
    );
    let mut list = row()
        .key("plugin-cards")
        .w_full()
        .mt(6.0)
        .gaps(16.0, 16.0)
        .equal_columns(plugin_columns(page_width));
    let mut count = 0;
    for (index, plugin) in plugin_rows(state) {
        count += 1;
        list = list.child(plugin_card(index, plugin, state, theme));
    }
    if count == 0 {
        list = list.child(
            text(match state.plugin_tab {
                PluginTab::All => "No plugins found.",
                PluginTab::Available => "No plugins available.",
                PluginTab::Installed => "No plugins installed.",
            })
            .font(ui_font(14.0, 400.0))
            .color(theme.color("#969696"))
            .mt(12.0),
        );
    }
    let mut content = vec![tabs];
    if !state.plugin_error.is_empty() {
        content.push(
            text(state.plugin_error.clone())
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#e07676")),
        );
    }
    content.push(list);
    content
}

fn feature_card(index: usize, feature: &FeatureEntry, state: &State, theme: ThemeStyle<'_>) -> El {
    let stats = stat_block(
        [
            row()
                .key(format!("feature-prompt-extension-{}", feature.id))
                .w_full()
                .h(32.0)
                .center_y()
                .child(
                    text("Prompt Extension")
                        .grow(1.0)
                        .font(ui_font(13.0, 400.0))
                        .color(theme.color("#969696")),
                )
                .child(
                    text(if feature.has_prompt_extension {
                        "Yes"
                    } else {
                        "No"
                    })
                    .font(ui_font(13.0, 500.0))
                    .color(theme.color("#f2f2f2")),
                ),
            stat_row("Tools", feature.tool_count, theme)
                .key(format!("feature-tools-{}", feature.id)),
        ],
        theme,
    );
    let available = matches!(state.connection, ConnectionStatus::Connected { .. })
        && !state.feature_busy
        && state.feature_error.is_empty()
        && state.extension_scope_saving.is_none();
    let next_enabled = !feature.enabled;
    let id = feature.id.clone();
    let mut action = toggle_button(
        format!("feature-toggle-{}", feature.id),
        format!("{} feature", feature.name),
        feature.enabled,
        available,
        theme,
    );
    if available {
        action = action.on_press(move |state: &mut State| {
            start_feature_toggle(state, id.clone(), next_enabled);
        });
    }
    col()
        .key(format!("feature-item-{}", feature.id))
        .w_full()
        .min_w(0.0)
        .px(12.0)
        .pt(16.0)
        .pb(14.0)
        .radius(24.0)
        .continuous_corners()
        .border(1.0, theme.color("#303030"))
        .child(
            row()
                .w_full()
                .h(40.0)
                .gap(10.0)
                .center_y()
                .child(feature_mark(&feature.id, theme).key(format!("feature-logo-{}", feature.id)))
                .child(
                    text(feature.name.clone())
                        .grow(1.0)
                        .min_w(0.0)
                        .font(ui_font(16.0, 600.0))
                        .color(theme.color("#f2f2f2"))
                        .max_lines(2),
                ),
        )
        .child(
            text(feature.description.clone())
                .w_full()
                .mt(10.0)
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#969696"))
                .max_lines(3),
        )
        .child(el().grow(1.0))
        .child(stats)
        .child(
            row()
                .w_full()
                .pt(12.0)
                .center_y()
                .child(action)
                .child(if feature.enabled {
                    let menu = SelectionMenu::FeatureWorkspaces { index };
                    selection_button(
                        workspace_scope_label(
                            state,
                            feature.all_workspaces,
                            &feature.workspace_ids,
                        ),
                        state.selection_menu == Some(menu),
                        move |state| crate::toggle_selection_menu(state, menu),
                        theme,
                    )
                    .key(format!("feature-scope-{index}"))
                    .ml(5.0)
                    .max_w(160.0)
                } else {
                    el().size(0.0, 0.0)
                })
                .child(el().grow(1.0))
                .child(
                    el().key(format!("feature-details-{}", feature.id))
                        .role("button")
                        .label(format!("View {} feature details", feature.name))
                        .help("View feature details")
                        .size(26.0, 26.0)
                        .mr(-5.0)
                        .center()
                        .radius(8.0)
                        .continuous_corners()
                        .hover(theme.color("#303030"))
                        .on_press({
                            let id = feature.id.clone();
                            move |state: &mut State| start_feature_details(state, id.clone())
                        })
                        .child(icon(&INFO, 17.0).color(theme.color("#bdbdbd"))),
                ),
        )
}

fn features(state: &State, theme: ThemeStyle<'_>, page_width: f32) -> Vec<El> {
    let mut tabs = row()
        .key("feature-view-tabs")
        .role("tablist")
        .label("Feature views")
        .w_full()
        .h(26.0)
        .gap(6.0)
        .center_y();
    for tab in FeatureTab::ALL {
        tabs = tabs.child(feature_tab_button(tab, state.feature_tab == tab, theme));
    }
    let mut cards = row()
        .key("feature-cards")
        .w_full()
        .mt(6.0)
        .gaps(16.0, 16.0)
        .equal_columns(plugin_columns(page_width));
    let mut count = 0;
    for (index, feature) in feature_rows(state) {
        count += 1;
        cards = cards.child(feature_card(index, feature, state, theme));
    }
    if count == 0 {
        cards = cards.child(
            text(match state.feature_tab {
                FeatureTab::All => "No features found.",
                FeatureTab::Available => "No features available.",
                FeatureTab::Enabled => "No features enabled.",
            })
            .font(ui_font(14.0, 400.0))
            .color(theme.color("#969696"))
            .mt(12.0),
        );
    }
    let mut content = vec![tabs];
    if !state.feature_error.is_empty() {
        content.push(
            text(state.feature_error.clone())
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#e07676")),
        );
    }
    content.push(cards);
    content
}

pub(crate) fn render(state: &State, theme: ThemeStyle<'_>, page_width: f32) -> Vec<El> {
    match state.extensions_section {
        ExtensionSection::Plugins => plugins(state, theme, page_width),
        ExtensionSection::Features => features(state, theme, page_width),
        ExtensionSection::ResearchKits => vec![
            text("No Research Kits available yet.")
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#969696")),
        ],
    }
}

fn close_add_dialog(state: &mut State) {
    state.plugin_add_dialog_open = false;
    state.plugin_source_edit.clear();
}

pub(crate) fn add_dialog(state: &State, theme: ThemeStyle<'_>) -> [El; 2] {
    let body = col()
        .w_full()
        .gap(10.0)
        .child(
            text("Plugin source")
                .font(ui_font(14.0, 550.0))
                .color(theme.color("#e2e2e2")),
        )
        .child(
            field(
                &state.plugin_source_edit,
                "plugin-source-input",
                "Git URL or local directory",
                theme,
                |state| &mut state.plugin_source_edit,
            )
            .w_full(),
        )
        .child(
            text("Agent Plugins packages use a plugin.json manifest at their root. Source registration and installation will be connected to the app-server in a later step.")
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#969696"))
                .wrap(),
        );
    let actions = row().w_full().child(el().grow(1.0)).child(
        form_button("Close", theme)
            .key("plugin-source-close")
            .on_press(close_add_dialog),
    );
    modal_dialog::render(
        modal_dialog::DialogSpec {
            key: "plugin-source-dialog",
            title: "Add a Plugin",
            width: 560.0,
            focus_return: "plugin-add-source",
        },
        body,
        actions,
        theme,
        close_add_dialog,
    )
}

fn close_manifest_dialog(state: &mut State) {
    state.plugin_manifest_generation = state.plugin_manifest_generation.wrapping_add(1);
    state.plugin_manifest_name = None;
    state.plugin_manifest_text.clear();
    state.plugin_manifest_error.clear();
    state.plugin_manifest_loading = false;
}

pub(crate) fn manifest_dialog(state: &State, theme: ThemeStyle<'_>) -> [El; 2] {
    let name = state.plugin_manifest_name.as_deref().unwrap_or_default();
    let content = if state.plugin_manifest_loading {
        "Loading manifest…".to_owned()
    } else if !state.plugin_manifest_error.is_empty() {
        state.plugin_manifest_error.clone()
    } else {
        state.plugin_manifest_text.clone()
    };
    let body = col()
        .w_full()
        .gap(12.0)
        .child(
            text(name.to_owned())
                .font(ui_font(14.0, 550.0))
                .color(theme.color("#e2e2e2")),
        )
        .child(
            col()
                .key("plugin-manifest-content")
                .w_full()
                .max_h(420.0)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(
                    text(content)
                        .selectable(true)
                        .font(ui_font(13.0, 400.0))
                        .color(theme.color(if state.plugin_manifest_error.is_empty() {
                            "#c8c8c8"
                        } else {
                            "#e07676"
                        }))
                        .wrap(),
                ),
        );
    let actions = row().w_full().child(el().grow(1.0)).child(
        form_button("Close", theme)
            .key("plugin-manifest-close")
            .on_press(close_manifest_dialog),
    );
    modal_dialog::render(
        modal_dialog::DialogSpec {
            key: "plugin-manifest-dialog",
            title: "Plugin Manifest",
            width: 620.0,
            focus_return: "plugin-tab-all",
        },
        body,
        actions,
        theme,
        close_manifest_dialog,
    )
}

fn close_feature_details_dialog(state: &mut State) {
    state.feature_details_generation = state.feature_details_generation.wrapping_add(1);
    state.feature_details_id = None;
    state.feature_details = None;
    state.feature_details_loading = false;
    state.feature_details_error.clear();
}

pub(crate) fn feature_details_dialog(state: &State, theme: ThemeStyle<'_>) -> [El; 2] {
    let id = state.feature_details_id.as_deref().unwrap_or_default();
    let feature = state.features.iter().find(|item| item.id == id);
    let details = state.feature_details.as_ref();
    let name = details.map_or_else(
        || feature.map_or("Feature", |item| item.name.as_str()),
        |item| item.name.as_str(),
    );
    let description = details.map_or_else(
        || feature.map_or("", |item| item.description.as_str()),
        |item| item.description.as_str(),
    );
    let mut body = col()
        .w_full()
        .gap(12.0)
        .child(
            text(name.to_owned())
                .font(ui_font(16.0, 600.0))
                .color(theme.color("#e2e2e2")),
        )
        .child(
            text(description.to_owned())
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#bdbdbd"))
                .wrap(),
        );
    if !state.feature_details_error.is_empty() {
        body = body.child(
            text(state.feature_details_error.clone())
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#e07676"))
                .wrap(),
        );
    } else if feature.is_some_and(|item| item.has_prompt_extension) {
        let content = if state.feature_details_loading {
            "Loading prompt extension…"
        } else {
            details.map_or("", |item| item.prompt_extension.as_str())
        };
        body = body
            .child(
                text("Prompt Extension")
                    .font(ui_font(14.0, 550.0))
                    .color(theme.color("#e2e2e2")),
            )
            .child(
                col()
                    .key("feature-details-prompt")
                    .w_full()
                    .max_h(420.0)
                    .min_h(0.0)
                    .scroll(ScrollSpec {
                        stick_bottom: false,
                    })
                    .child(
                        text(content.to_owned())
                            .selectable(true)
                            .font(ui_font(13.0, 400.0))
                            .color(theme.color("#c8c8c8"))
                            .wrap(),
                    ),
            );
    }
    let actions = row().w_full().child(el().grow(1.0)).child(
        form_button("Close", theme)
            .key("feature-details-close")
            .on_press(close_feature_details_dialog),
    );
    modal_dialog::render(
        modal_dialog::DialogSpec {
            key: "feature-details-dialog",
            title: "Feature Details",
            width: 620.0,
            focus_return: "extensions-features",
        },
        body,
        actions,
        theme,
        close_feature_details_dialog,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_bundled_plugins_are_available_and_uninstalled() {
        let plugins = bundled_plugins();
        assert_eq!(plugins.len(), 4);
        assert_eq!(
            plugins
                .iter()
                .map(|plugin| plugin.name.as_str())
                .collect::<Vec<_>>(),
            [
                "apple-security-devices",
                "apple-target-flags",
                "meta-skills",
                "microsoft-security-devices",
            ]
        );
        assert!(plugins.iter().all(|plugin| !plugin.installed));
        assert!(plugins.iter().all(|plugin| plugin.skills == 1));
        assert_eq!(
            plugins
                .iter()
                .map(|plugin| plugin.mcp_servers)
                .collect::<Vec<_>>(),
            [1, 0, 0, 0]
        );
        assert_eq!(
            plugins
                .iter()
                .map(|plugin| plugin.mcp_tools)
                .collect::<Vec<_>>(),
            [19, 0, 0, 0]
        );
    }

    #[test]
    fn plugin_tabs_partition_the_full_catalog() {
        let mut state = State {
            plugins: vec![
                PluginEntry {
                    name: "example-one".to_owned(),
                    description: "First example".to_owned(),
                    installed: false,
                    skills: 1,
                    mcp_servers: 0,
                    mcp_tools: 0,
                    workspace_ids: Vec::new(),
                    all_workspaces: true,
                },
                PluginEntry {
                    name: "example-two".to_owned(),
                    description: "Second example".to_owned(),
                    installed: true,
                    skills: 1,
                    mcp_servers: 0,
                    mcp_tools: 0,
                    workspace_ids: Vec::new(),
                    all_workspaces: true,
                },
            ],
            ..State::default()
        };
        assert_eq!(state.plugin_tab, PluginTab::All);
        assert_eq!(plugin_rows(&state).count(), 2);
        state.plugin_tab = PluginTab::Available;
        assert_eq!(
            plugin_rows(&state)
                .map(|(_, plugin)| plugin.name.as_str())
                .collect::<Vec<_>>(),
            ["example-one"]
        );
        state.plugin_tab = PluginTab::Installed;
        assert_eq!(
            plugin_rows(&state)
                .map(|(_, plugin)| plugin.name.as_str())
                .collect::<Vec<_>>(),
            ["example-two"]
        );
    }

    #[test]
    fn feature_tabs_partition_the_full_catalog() {
        let mut state = State {
            features: bundled_features(),
            ..State::default()
        };
        assert_eq!(state.feature_tab, FeatureTab::All);
        assert_eq!(feature_rows(&state).count(), 2);
        state.feature_tab = FeatureTab::Available;
        assert_eq!(feature_rows(&state).count(), 2);
        state.feature_tab = FeatureTab::Enabled;
        assert_eq!(feature_rows(&state).count(), 0);
        state.features[0].enabled = true;
        assert_eq!(feature_rows(&state).count(), 1);
        state.feature_tab = FeatureTab::Available;
        assert_eq!(feature_rows(&state).count(), 1);
        state.feature_tab = FeatureTab::All;
        assert_eq!(feature_rows(&state).count(), 2);
    }

    #[test]
    fn workspace_scope_menu_selects_multiple_workspaces() {
        let first = uuid::Uuid::from_u128(1).to_string();
        let second = uuid::Uuid::from_u128(2).to_string();
        let mut state = State::default();
        state.plugins[0].installed = true;
        state.features[0].enabled = true;
        state.workspaces = [(&first, "Example A"), (&second, "Example B")]
            .into_iter()
            .map(|(id, name)| beale_contracts::v1::WorkspaceSummary {
                id: id.clone(),
                name: name.to_owned(),
                directory: format!("example-{name}"),
            })
            .collect();
        let menu = SelectionMenu::PluginWorkspaces { index: 0 };
        assert!(workspace_scope_items(&state, menu)[0].selected);
        assert_eq!(
            next_workspace_scope(&state, menu, &first),
            Some((false, vec![first.clone()]))
        );
        state.plugins[0].all_workspaces = false;
        state.plugins[0].workspace_ids = vec![first.clone()];
        assert!(workspace_scope_items(&state, menu)[1].selected);
        assert_eq!(
            next_workspace_scope(&state, menu, &second),
            Some((false, vec![first.clone(), second.clone()]))
        );
        assert_eq!(
            next_workspace_scope(&state, menu, &first),
            Some((false, Vec::new()))
        );
        assert_eq!(
            next_workspace_scope(&state, menu, ""),
            Some((true, Vec::new()))
        );
        assert_eq!(
            workspace_scope_label(&state, false, &state.plugins[0].workspace_ids),
            "Example A"
        );
        let feature_menu = SelectionMenu::FeatureWorkspaces { index: 0 };
        assert!(workspace_scope_items(&state, feature_menu)[0].selected);
    }

    #[test]
    fn manifest_format_requires_matching_plugin_name() {
        let source = r#"{"name":"example-plugin","version":"1.0.0"}"#;
        assert!(
            format_manifest(source, "example-plugin")
                .unwrap()
                .contains("\n")
        );
        assert!(format_manifest(source, "another-plugin").is_err());
        assert!(format_manifest("not JSON", "example-plugin").is_err());
    }
}
