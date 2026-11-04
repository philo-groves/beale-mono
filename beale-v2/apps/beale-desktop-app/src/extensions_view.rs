use beale_contracts::{
    PLUGIN_INSTALL_PATH, PLUGIN_UNINSTALL_PATH, PLUGINS_PATH, PROTOBUF_CONTENT_TYPE,
    v1::{InstallPluginRequest, ListPluginsResponse, PluginSummary, UninstallPluginRequest},
};
use prost::Message;
use reqwest::{Client, StatusCode, header::CONTENT_TYPE};
use rune::{Color, El, col, el, icon, row, text};
use serde::Deserialize;
use std::{collections::HashSet, net::SocketAddr};

use crate::{
    AUTH_HANDLE, ConnectionStatus, State, ThemeStyle, ViewLocation, connection, form_button,
    icons::{FEATURES, PLUGINS, PLUS, RESEARCH_KITS},
    modal_dialog, sidenav_heading, ui_font,
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
    Available,
    Installed,
}

impl PluginTab {
    const ALL: [Self; 2] = [Self::Available, Self::Installed];

    fn label(self) -> &'static str {
        match self {
            Self::Available => "Available",
            Self::Installed => "Installed",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Available => "plugin-tab-available",
            Self::Installed => "plugin-tab-installed",
        }
    }
}

pub(crate) struct PluginEntry {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) installed: bool,
    pub(crate) skills: usize,
    pub(crate) tools: Option<usize>,
    pub(crate) resources: Option<usize>,
    pub(crate) prompts: Option<usize>,
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

const BUNDLED_PACKAGES: [(&str, &str, Option<&str>); 4] = [
    (
        include_str!("../../../extensions/plugins/apple-security-device-plugin/plugin.json"),
        include_str!(
            "../../../extensions/plugins/apple-security-device-plugin/skills/apple-security-devices/SKILL.md"
        ),
        Some(include_str!(
            "../../../extensions/plugins/apple-security-device-plugin/mcp.json"
        )),
    ),
    (
        include_str!("../../../extensions/plugins/apple-target-flags-plugin/plugin.json"),
        include_str!(
            "../../../extensions/plugins/apple-target-flags-plugin/skills/apple-target-flags/SKILL.md"
        ),
        None,
    ),
    (
        include_str!("../../../extensions/plugins/meta-skills-plugin/plugin.json"),
        include_str!(
            "../../../extensions/plugins/meta-skills-plugin/skills/meta-bug-bounty-tools/SKILL.md"
        ),
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
    ),
];

pub(crate) fn bundled_plugins() -> Vec<PluginEntry> {
    BUNDLED_PACKAGES
        .into_iter()
        .map(|(source, skill, mcp)| {
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
            let has_mcp_server = mcp.is_some_and(|source| {
                let config: serde_json::Value =
                    serde_json::from_str(source).expect("bundled MCP configuration is valid JSON");
                assert_eq!(
                    config["$schema"].as_str(),
                    Some("https://agent-plugins.org/schemas/1.0.0/mcp.schema.json"),
                    "bundled MCP configuration must use Agent Plugins 1.0"
                );
                !config["mcpServers"]
                    .as_object()
                    .expect("bundled MCP servers must be an object")
                    .is_empty()
            });
            let known_count = if has_mcp_server { None } else { Some(0) };
            PluginEntry {
                name: manifest.name,
                description: manifest.description,
                installed: false,
                skills: 1,
                tools: known_count,
                resources: known_count,
                prompts: known_count,
            }
        })
        .collect()
}

fn from_wire(plugin: PluginSummary) -> Result<PluginEntry, String> {
    if plugin.name.is_empty()
        || plugin.name.len() > 64
        || plugin.description.len() > 2_000
        || plugin.skill_count > 128
    {
        return Err("Invalid plugin catalog".to_owned());
    }
    let count = if plugin.has_mcp_server { None } else { Some(0) };
    Ok(PluginEntry {
        name: plugin.name,
        description: plugin.description,
        installed: plugin.installed,
        skills: plugin.skill_count as usize,
        tools: count,
        resources: count,
        prompts: count,
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
        return Err(format!("Plugin request failed ({})", response.status()));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "Could not read plugin response".to_owned())?;
    if bytes.len() > 64_000 {
        return Err("Plugin response is too large".to_owned());
    }
    T::decode(bytes).map_err(|_| "Invalid plugin response".to_owned())
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

pub(crate) fn start_load(state: &mut State) {
    if state.plugin_loading
        || state.plugin_installing.is_some()
        || state.plugin_uninstalling.is_some()
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
                    state.navigate_to(ViewLocation::Extensions(section, state.plugin_tab));
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
            state.navigate_to(ViewLocation::Extensions(ExtensionSection::Plugins, tab));
        })
        .child(
            text(tab.label())
                .font(ui_font(14.0, if selected { 600.0 } else { 450.0 }))
                .color(theme.color(if selected { "#f2f2f2" } else { "#c8c8c8" })),
        )
}

fn plugin_rows(state: &State) -> impl Iterator<Item = &PluginEntry> {
    state
        .plugins
        .iter()
        .filter(|plugin| state.plugin_tab == PluginTab::Available || plugin.installed)
}

fn stat_row(label: &'static str, value: Option<usize>, theme: ThemeStyle<'_>) -> El {
    let mut count = text(value.map_or_else(|| "—".to_owned(), |count| count.to_string()))
        .font(ui_font(13.0, 500.0))
        .color(theme.color("#f2f2f2"));
    if value.is_none() {
        count = count.help("MCP capability count available after installation");
    }
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
        .child(count)
}

fn plugin_card(plugin: &PluginEntry, state: &State, theme: ThemeStyle<'_>) -> El {
    let mut stats = col().w_full();
    for (label, value) in [
        ("Skills", Some(plugin.skills)),
        ("Tools", plugin.tools),
        ("Resources", plugin.resources),
        ("Prompts", plugin.prompts),
    ] {
        stats = stats
            .child(el().w_full().h(1.0).fill(theme.color("#303030")))
            .child(stat_row(label, value, theme));
    }
    stats = stats.child(el().w_full().h(1.0).fill(theme.color("#303030")));
    let action = if plugin.installed {
        let available = matches!(state.connection, ConnectionStatus::Connected { .. })
            && state.plugin_installing.is_none()
            && state.plugin_uninstalling.is_none();
        let name = plugin.name.clone();
        let mut button = row()
            .key(format!("plugin-uninstall-{}", plugin.name))
            .role("button")
            .label(format!("Uninstall {}", plugin.display_name()))
            .h(26.0)
            .px(12.0)
            .center()
            .radius(8.0)
            .continuous_corners()
            .fill(theme.color(if available { "#303030" } else { "#242424" }))
            .child(
                text(
                    if state.plugin_uninstalling.as_deref() == Some(name.as_str()) {
                        "Uninstalling…"
                    } else {
                        "Uninstall"
                    },
                )
                .font(ui_font(14.0, 550.0))
                .color(theme.color(if available { "#eeeeee" } else { "#969696" })),
            );
        if available {
            button = button
                .hover(theme.color("#414141"))
                .on_press(move |state: &mut State| {
                    start_uninstall(state, name.clone());
                });
        }
        button
    } else {
        let available = matches!(state.connection, ConnectionStatus::Connected { .. })
            && state.plugin_installing.is_none()
            && state.plugin_uninstalling.is_none();
        let name = plugin.name.clone();
        let mut button = row()
            .key(format!("plugin-install-{}", plugin.name))
            .role("button")
            .label(format!("Install {}", plugin.display_name()))
            .h(26.0)
            .px(12.0)
            .center()
            .radius(8.0)
            .continuous_corners()
            .fill(theme.color(if available { "#303030" } else { "#242424" }))
            .child(
                text(
                    if state.plugin_installing.as_deref() == Some(name.as_str()) {
                        "Installing…"
                    } else {
                        "Install"
                    },
                )
                .font(ui_font(14.0, 550.0))
                .color(theme.color(if available { "#eeeeee" } else { "#969696" })),
            );
        if available {
            button = button
                .hover(theme.color("#414141"))
                .on_press(move |state: &mut State| {
                    start_install(state, name.clone());
                });
        }
        button
    };
    col()
        .key(format!("plugin-item-{}", plugin.name))
        .w(258.0)
        .px(16.0)
        .pt(16.0)
        .pb(14.0)
        .radius(24.0)
        .continuous_corners()
        .border(1.0, theme.color("#303030"))
        .child(
            text(plugin.display_name())
                .font(ui_font(16.0, 600.0))
                .color(theme.color("#f2f2f2"))
                .max_lines(2),
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
        .child(stats.mt(16.0))
        .child(row().w_full().pt(12.0).child(action))
}

fn plugins(state: &State, theme: ThemeStyle<'_>) -> Vec<El> {
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
        .items_start()
        .flex_wrap();
    let mut count = 0;
    for plugin in plugin_rows(state) {
        count += 1;
        list = list.child(plugin_card(plugin, state, theme));
    }
    if count == 0 {
        list = list.child(
            text(if state.plugin_tab == PluginTab::Available {
                "No plugins available."
            } else {
                "No plugins installed."
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

pub(crate) fn render(state: &State, theme: ThemeStyle<'_>) -> Vec<El> {
    match state.extensions_section {
        ExtensionSection::Plugins => plugins(state, theme),
        ExtensionSection::Features => vec![
            text("No features available yet.")
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#969696")),
        ],
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
        assert_eq!(plugins[0].tools, None);
        assert!(plugins[1..].iter().all(|plugin| plugin.tools == Some(0)));
    }

    #[test]
    fn installed_is_a_filter_of_available_plugins() {
        let mut state = State {
            plugins: vec![
                PluginEntry {
                    name: "example-one".to_owned(),
                    description: "First example".to_owned(),
                    installed: false,
                    skills: 1,
                    tools: Some(0),
                    resources: Some(0),
                    prompts: Some(0),
                },
                PluginEntry {
                    name: "example-two".to_owned(),
                    description: "Second example".to_owned(),
                    installed: true,
                    skills: 1,
                    tools: Some(0),
                    resources: Some(0),
                    prompts: Some(0),
                },
            ],
            ..State::default()
        };
        assert_eq!(plugin_rows(&state).count(), 2);
        state.plugin_tab = PluginTab::Installed;
        assert_eq!(
            plugin_rows(&state)
                .map(|plugin| plugin.name.as_str())
                .collect::<Vec<_>>(),
            ["example-two"]
        );
    }
}
