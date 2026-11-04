use qrcodegen::{QrCode, QrCodeEcc};
use rune::{Color, El, Icon, Pixels, TextEdit, col, el, icon, row, text, text_input};

use crate::{
    AUTH_HANDLE, ConnectionStatus, PrimaryView, State, ThemeStyle, ViewLocation,
    fleet_connection::{self, FleetServer, Mutation, MutationResult},
    form_button,
    form_controls::toggle_button,
    icons::{
        EXTENSION_WINDOWS, FLEET, FLEET_APPLE, FLEET_BROWSER, FLEET_LINUX, FLEET_QR,
        FLEET_VIRTUAL_MACHINE, INFO, PLUS,
    },
    modal_dialog, sidenav_heading, ui_font,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum FleetSection {
    #[default]
    AppServers,
    Browsers,
    VirtualMachines,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum FleetTab {
    #[default]
    All,
    Available,
    Enabled,
}

impl FleetTab {
    const ALL: [Self; 3] = [Self::All, Self::Available, Self::Enabled];

    fn label(self, section: FleetSection) -> String {
        match self {
            Self::All => format!("All {}", section.label()),
            Self::Available => "Available".to_owned(),
            Self::Enabled => "Enabled".to_owned(),
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::All => "fleet-tab-all",
            Self::Available => "fleet-tab-available",
            Self::Enabled => "fleet-tab-enabled",
        }
    }
}

pub(crate) struct FleetState {
    pub(crate) tab: FleetTab,
    tabs: [FleetTab; 3],
    local: FleetServer,
    servers: Vec<FleetServer>,
    loading: bool,
    busy: bool,
    error: String,
    generation: u64,
    pub(crate) connect_dialog_open: bool,
    endpoint_edit: TextEdit,
    code_edit: TextEdit,
    display_name_edit: TextEdit,
    pub(crate) details_id: Option<String>,
    details_name_edit: TextEdit,
    remove_confirm: bool,
    pub(crate) qr_dialog_open: bool,
    qr_pixels: Option<Pixels>,
    qr_loading: bool,
    qr_error: String,
    qr_generation: u64,
}

impl Default for FleetState {
    fn default() -> Self {
        Self {
            tab: FleetTab::All,
            tabs: [FleetTab::All; 3],
            local: fleet_connection::local_placeholder(),
            servers: Vec::new(),
            loading: false,
            busy: false,
            error: String::new(),
            generation: 0,
            connect_dialog_open: false,
            endpoint_edit: TextEdit::new(""),
            code_edit: TextEdit::new(""),
            display_name_edit: TextEdit::new(""),
            details_id: None,
            details_name_edit: TextEdit::new(""),
            remove_confirm: false,
            qr_dialog_open: false,
            qr_pixels: None,
            qr_loading: false,
            qr_error: String::new(),
            qr_generation: 0,
        }
    }
}

impl FleetSection {
    const ALL: [Self; 3] = [Self::AppServers, Self::Browsers, Self::VirtualMachines];

    fn index(self) -> usize {
        match self {
            Self::AppServers => 0,
            Self::Browsers => 1,
            Self::VirtualMachines => 2,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::AppServers => "App Servers",
            Self::Browsers => "Browsers",
            Self::VirtualMachines => "Virtual Machines",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::AppServers => "fleet-app-servers",
            Self::Browsers => "fleet-browsers",
            Self::VirtualMachines => "fleet-virtual-machines",
        }
    }

    fn glyph(self) -> &'static Icon {
        match self {
            Self::AppServers => &FLEET,
            Self::Browsers => &FLEET_BROWSER,
            Self::VirtualMachines => &FLEET_VIRTUAL_MACHINE,
        }
    }
}

impl FleetState {
    pub(crate) fn tab_for(&self, section: FleetSection) -> FleetTab {
        self.tabs[section.index()]
    }

    pub(crate) fn select_tab(&mut self, section: FleetSection, tab: FleetTab) {
        self.tabs[section.index()] = tab;
        self.tab = tab;
    }
}

pub(crate) fn sidenav(state: &State, width: f32, theme: ThemeStyle<'_>) -> El {
    let mut items = col().w_full().gap(2.0);
    for section in FleetSection::ALL {
        let active = state.fleet_section == section;
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
                    state.navigate_to(ViewLocation::Fleet(section, state.fleet.tab_for(section)));
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
        .key("fleet-sidenav")
        .role("navigation")
        .label("Fleet sections")
        .w(width)
        .h_full()
        .px(8.0)
        .pt(12.0)
        .fill(theme.color("#111111"))
        .child(
            sidenav_heading("Fleet", theme)
                .key("fleet-sidenav-title")
                .ml(4.0)
                .mb(12.0),
        )
        .child(items)
}

pub(crate) fn start_load(state: &mut State) {
    if state.primary_view != PrimaryView::Fleet
        || state.fleet_section != FleetSection::AppServers
        || state.fleet.loading
        || state.fleet.busy
    {
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        return;
    };
    state.fleet.loading = true;
    state.fleet.generation = state.fleet.generation.wrapping_add(1);
    let generation = state.fleet.generation;
    std::thread::spawn(move || {
        let result = (|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize Fleet connection.".to_owned())?;
            runtime.block_on(fleet_connection::load())
        })();
        handle.update(move |state, _| {
            if state.fleet.generation != generation {
                return;
            }
            state.fleet.loading = false;
            match result {
                Ok(snapshot) => {
                    state.fleet.local = snapshot.local;
                    state.fleet.servers = snapshot.servers;
                    state.fleet.error.clear();
                }
                Err(error) => state.fleet.error = error,
            }
        });
    });
}

fn start_mutation(state: &mut State, action: Mutation) {
    if state.fleet.busy {
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        state.fleet.error = "Desktop is not ready.".to_owned();
        return;
    };
    state.fleet.busy = true;
    state.fleet.loading = false;
    state.fleet.generation = state.fleet.generation.wrapping_add(1);
    state.fleet.error.clear();
    let generation = state.fleet.generation;
    std::thread::spawn(move || {
        let result = (|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize Fleet connection.".to_owned())?;
            runtime.block_on(fleet_connection::mutate(action))
        })();
        handle.update(move |state, _| {
            if state.fleet.generation != generation {
                return;
            }
            state.fleet.busy = false;
            match result {
                Ok(MutationResult::Saved(server)) => {
                    let server = *server;
                    if let Some(saved) = state
                        .fleet
                        .servers
                        .iter_mut()
                        .find(|saved| saved.id == server.id)
                    {
                        *saved = server;
                    } else {
                        state.fleet.servers.push(server);
                    }
                    state.fleet.connect_dialog_open = false;
                    state.fleet.endpoint_edit.clear();
                    state.fleet.code_edit.clear();
                    state.fleet.display_name_edit.clear();
                    state.fleet.details_id = None;
                    state.fleet.remove_confirm = false;
                    start_load(state);
                }
                Ok(MutationResult::Removed(id)) => {
                    state.fleet.servers.retain(|server| server.id != id);
                    state.fleet.details_id = None;
                    state.fleet.remove_confirm = false;
                }
                Ok(MutationResult::LocalName(display_name)) => {
                    state.fleet.local.display_name = display_name;
                    state.fleet.details_id = None;
                }
                Err(error) => state.fleet.error = error,
            }
        });
    });
}

fn qr_pixels(uri: &str) -> Result<Pixels, String> {
    let qr = QrCode::encode_text(uri, QrCodeEcc::Medium)
        .map_err(|_| "Could not render the authenticator QR code.".to_owned())?;
    let modules = u32::try_from(qr.size()).map_err(|_| "Invalid QR code size.".to_owned())?;
    let side = (modules + 8) * 5;
    let mut rgba = vec![255_u8; (side * side * 4) as usize];
    for y in 0..side {
        for x in 0..side {
            let module_x = i32::try_from(x / 5).unwrap_or_default() - 4;
            let module_y = i32::try_from(y / 5).unwrap_or_default() - 4;
            if qr.get_module(module_x, module_y) {
                let start = ((y * side + x) * 4) as usize;
                rgba[start..start + 3].fill(0);
            }
        }
    }
    let pixels = Pixels::new();
    pixels.set(side, side, rgba);
    Ok(pixels)
}

fn start_qr(state: &mut State) {
    if state.fleet.qr_loading {
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        state.fleet.qr_error = "Desktop is not ready.".to_owned();
        return;
    };
    state.fleet.qr_dialog_open = true;
    state.fleet.qr_loading = true;
    state.fleet.qr_error.clear();
    state.fleet.qr_pixels = None;
    state.fleet.qr_generation = state.fleet.qr_generation.wrapping_add(1);
    let generation = state.fleet.qr_generation;
    std::thread::spawn(move || {
        let result = (|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize the local connection.".to_owned())?;
            let uri = runtime.block_on(fleet_connection::provisioning_uri())?;
            qr_pixels(&uri)
        })();
        handle.update(move |state, _| {
            if state.fleet.qr_generation != generation {
                return;
            }
            state.fleet.qr_loading = false;
            match result {
                Ok(pixels) => state.fleet.qr_pixels = Some(pixels),
                Err(error) => state.fleet.qr_error = error,
            }
        });
    });
}
fn tab_button(tab: FleetTab, section: FleetSection, active: bool, theme: ThemeStyle<'_>) -> El {
    row()
        .key(tab.key())
        .role("tab")
        .label(tab.label(section))
        .h(26.0)
        .px(10.0)
        .center_y()
        .radius(9.0)
        .continuous_corners()
        .fill(theme.color(if active { "#414141" } else { "#242424" }))
        .hover(theme.color("#4a4a4a"))
        .on_press(move |state: &mut State| {
            state.navigate_to(ViewLocation::Fleet(section, tab));
        })
        .child(
            text(tab.label(section))
                .font(ui_font(14.0, if active { 600.0 } else { 450.0 }))
                .color(theme.color(if active { "#f2f2f2" } else { "#c8c8c8" })),
        )
}

fn stat_row(label: &'static str, value: String, theme: ThemeStyle<'_>) -> El {
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
            text(value)
                .font(ui_font(13.0, 500.0))
                .color(theme.color("#f2f2f2")),
        )
}

fn server_name(server: &FleetServer) -> &str {
    if server.display_name.is_empty() {
        &server.host_name
    } else {
        &server.display_name
    }
}

fn server_mark(server: &FleetServer, theme: ThemeStyle<'_>) -> El {
    let (glyph, background, foreground) = match server.os_vendor.as_str() {
        "Apple" => (&FLEET_APPLE, "#393939", "#e5e5e5"),
        "Microsoft" => (&EXTENSION_WINDOWS, "#273e58", "#c7dff3"),
        "Linux" => (&FLEET_LINUX, "#50432b", "#f0dda9"),
        _ => (&FLEET, "#303030", "#e2e2e2"),
    };
    el().size(40.0, 40.0)
        .radius(12.0)
        .continuous_corners()
        .fill(theme.color(background))
        .center()
        .child(icon(glyph, 23.0).color(theme.color(foreground)))
}

fn server_card(server: &FleetServer, state: &State, theme: ThemeStyle<'_>) -> El {
    let is_local = server.id == "local";
    let available = !state.fleet.busy && !is_local;
    let id = server.id.clone();
    let enabled = server.enabled;
    let display_name = server.display_name.clone();
    let toggle = if is_local {
        text("Local")
            .font(ui_font(13.0, 450.0))
            .color(theme.color("#969696"))
    } else {
        let mut toggle = toggle_button(
            format!("fleet-server-toggle-{id}"),
            format!("{} connection", server_name(server)),
            enabled,
            available,
            theme,
        );
        if available {
            toggle = toggle.on_press(move |state: &mut State| {
                start_mutation(
                    state,
                    Mutation::Update {
                        id: id.clone(),
                        enabled: !enabled,
                        display_name: display_name.clone(),
                    },
                );
            });
        }
        toggle
    };
    let details_id = server.id.clone();
    col()
        .key(format!("fleet-server-card-{}", server.id))
        .w_full()
        .min_w(0.0)
        .min_h(214.0)
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
                .child(server_mark(server, theme))
                .child(
                    text(server_name(server).to_owned())
                        .grow(1.0)
                        .min_w(0.0)
                        .max_lines(2)
                        .font(ui_font(16.0, 600.0))
                        .color(theme.color("#f2f2f2")),
                ),
        )
        .child(el().grow(1.0))
        .child(stat_row("OS", server.os_name.clone(), theme))
        .child(stat_row(
            "Contract",
            format!("v{}.{}", server.protocol_major, server.protocol_minor),
            theme,
        ))
        .child(stat_row("Condition", server.condition.clone(), theme))
        .child(
            row()
                .w_full()
                .pt(12.0)
                .center_y()
                .child(toggle)
                .child(el().grow(1.0))
                .child(if is_local {
                    el().key("fleet-local-qr")
                        .role("button")
                        .label("Show authenticator QR code")
                        .help("Show authenticator QR code")
                        .size(26.0, 26.0)
                        .center()
                        .radius(8.0)
                        .continuous_corners()
                        .hover(theme.color("#303030"))
                        .on_press(start_qr)
                        .child(icon(&FLEET_QR, 17.0).color(theme.color("#bdbdbd")))
                } else {
                    el().size(0.0, 0.0)
                })
                .child(
                    el().key(format!("fleet-server-details-{}", server.id))
                        .role("button")
                        .label(format!("View {} app-server details", server_name(server)))
                        .help("View app-server details")
                        .size(26.0, 26.0)
                        .mr(-5.0)
                        .center()
                        .radius(8.0)
                        .continuous_corners()
                        .hover(theme.color("#303030"))
                        .on_press(move |state: &mut State| {
                            if let Some(server) = state
                                .fleet
                                .servers
                                .iter()
                                .find(|server| server.id == details_id)
                                .or_else(|| (details_id == "local").then_some(&state.fleet.local))
                            {
                                state.fleet.details_name_edit = TextEdit::new(&server.display_name);
                                state.fleet.details_id = Some(details_id.clone());
                                state.fleet.remove_confirm = false;
                            }
                        })
                        .child(icon(&INFO, 17.0).color(theme.color("#bdbdbd"))),
                ),
        )
}

pub(crate) fn render(state: &State, theme: ThemeStyle<'_>, page_width: f32) -> Vec<El> {
    let section = state.fleet_section;
    let title = text(section.label())
        .key("fleet-view-title")
        .font(crate::view_title_font())
        .color(theme.color("#f2f2f2"));
    let mut tabs = row()
        .key("fleet-view-tabs")
        .role("tablist")
        .label("Fleet views")
        .w_full()
        .h(26.0)
        .gap(6.0)
        .center_y()
        .mt(20.0);
    for tab in FleetTab::ALL {
        tabs = tabs.child(tab_button(tab, section, state.fleet.tab == tab, theme));
    }
    tabs = tabs.child(el().grow(1.0));
    if section == FleetSection::AppServers {
        tabs = tabs.child(
            el().key("fleet-add-server")
                .role("button")
                .label("Add an app server")
                .help("Add an app server")
                .size(26.0, 26.0)
                .center()
                .radius(8.0)
                .continuous_corners()
                .hover(theme.color("#303030"))
                .on_press(|state: &mut State| {
                    state.fleet.error.clear();
                    state.fleet.connect_dialog_open = true;
                })
                .child(icon(&PLUS, 17.0).color(theme.color("#bdbdbd"))),
        );
    }
    let mut content = vec![title, tabs];
    if !state.fleet.error.is_empty() {
        content.push(
            text(state.fleet.error.clone())
                .key("fleet-error")
                .mt(10.0)
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#e07676")),
        );
    }
    let mut cards = row()
        .key("fleet-cards")
        .w_full()
        .mt(16.0)
        .gaps(16.0, 16.0)
        .equal_columns(
            ((page_width + 16.0) / (220.0 + 16.0))
                .floor()
                .clamp(1.0, 4.0) as usize,
        );
    let mut count = 0;
    if section == FleetSection::AppServers {
        if state.fleet.tab != FleetTab::Available {
            let mut local = state.fleet.local.clone();
            local.condition = if matches!(state.connection, ConnectionStatus::Connected { .. }) {
                "Online".to_owned()
            } else {
                "Offline".to_owned()
            };
            count += 1;
            cards = cards.child(server_card(&local, state, theme));
        }
        for server in state
            .fleet
            .servers
            .iter()
            .filter(|server| match state.fleet.tab {
                FleetTab::All => true,
                FleetTab::Available => !server.enabled,
                FleetTab::Enabled => server.enabled,
            })
        {
            count += 1;
            cards = cards.child(server_card(server, state, theme));
        }
    }
    if count == 0 {
        let empty = match (section, state.fleet.tab) {
            (FleetSection::AppServers, FleetTab::All) if state.fleet.loading => {
                "Loading app servers…"
            }
            (FleetSection::AppServers, FleetTab::All) => "No remote app servers connected.",
            (FleetSection::AppServers, FleetTab::Available) => "No app servers available.",
            (FleetSection::AppServers, FleetTab::Enabled) => "No app servers enabled.",
            (FleetSection::Browsers, _) => "No browsers available.",
            (FleetSection::VirtualMachines, _) => "No virtual machines available.",
        };
        cards = cards.child(
            text(empty)
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#969696")),
        );
    }
    content.push(cards);
    content
}

fn input_field(
    label: &'static str,
    key: &'static str,
    edit: &TextEdit,
    secure: bool,
    accessor: fn(&mut State) -> &mut TextEdit,
    theme: ThemeStyle<'_>,
) -> El {
    let mut input = text_input(edit, ui_font(14.5, 400.0))
        .key(key)
        .single_line()
        .w_full()
        .h(36.0)
        .padding(8.0)
        .radius(8.0)
        .continuous_corners()
        .fill(theme.color("#242424"))
        .color(theme.color("#e2e2e2"))
        .on_edit(accessor);
    if secure {
        input = input.secure();
    }
    col()
        .w_full()
        .gap(6.0)
        .child(
            text(label)
                .font(ui_font(14.0, 500.0))
                .color(theme.color("#d4d4d4")),
        )
        .child(input)
}

fn close_connect(state: &mut State) {
    if state.fleet.busy {
        return;
    }
    state.fleet.connect_dialog_open = false;
    state.fleet.code_edit.clear();
    state.fleet.error.clear();
}

pub(crate) fn connect_dialog(state: &State, theme: ThemeStyle<'_>) -> [El; 2] {
    let body = col().w_full().gap(12.0)
        .child(input_field("Tailscale HTTPS URL", "fleet-endpoint-input", &state.fleet.endpoint_edit, false,
            |state| &mut state.fleet.endpoint_edit, theme))
        .child(input_field("Authenticator Code", "fleet-token-input", &state.fleet.code_edit, false,
            |state| &mut state.fleet.code_edit, theme))
        .child(input_field("Display Name (optional)", "fleet-name-input", &state.fleet.display_name_edit, false,
            |state| &mut state.fleet.display_name_edit, theme))
        .child(text("Enter the current six-digit code from the authenticator entry shown by the remote app server's QR code. Both hosts must be on the same Tailscale network.")
            .font(ui_font(13.0, 400.0)).color(theme.color("#969696")).wrap())
        .child(text(state.fleet.error.clone()).font(ui_font(13.0, 400.0)).color(theme.color("#e07676")));
    let mut actions = row().w_full().child(el().grow(1.0)).child(
        form_button("Cancel", theme)
            .key("fleet-connect-cancel")
            .on_press(close_connect),
    );
    if !state.fleet.busy {
        actions = actions.child(
            form_button("Connect", theme)
                .key("fleet-connect-save")
                .on_press(|state: &mut State| {
                    let endpoint = state.fleet.endpoint_edit.text.trim().to_owned();
                    let code = state.fleet.code_edit.text.trim().to_owned();
                    let display_name = state.fleet.display_name_edit.text.trim().to_owned();
                    start_mutation(
                        state,
                        Mutation::Connect {
                            endpoint,
                            code,
                            display_name,
                        },
                    );
                }),
        );
    }
    modal_dialog::render(
        modal_dialog::DialogSpec {
            key: "fleet-connect-dialog",
            title: "Add an App Server",
            width: 520.0,
            focus_return: "fleet-add-server",
        },
        body,
        actions,
        theme,
        close_connect,
    )
}

fn close_details(state: &mut State) {
    if state.fleet.busy {
        return;
    }
    state.fleet.details_id = None;
    state.fleet.remove_confirm = false;
    state.fleet.error.clear();
}

pub(crate) fn details_dialog(state: &State, theme: ThemeStyle<'_>) -> [El; 2] {
    let server = state.fleet.details_id.as_ref().and_then(|id| {
        if id == "local" {
            Some(&state.fleet.local)
        } else {
            state.fleet.servers.iter().find(|server| server.id == *id)
        }
    });
    let mut body = col().w_full().gap(12.0);
    if let Some(server) = server {
        body = body
            .child(input_field(
                "Display Name (optional)",
                "fleet-details-name-input",
                &state.fleet.details_name_edit,
                false,
                |state| &mut state.fleet.details_name_edit,
                theme,
            ))
            .child(stat_row("Host PC", server.host_name.clone(), theme))
            .child(stat_row(
                "Tailscale Link",
                if server.endpoint.is_empty() {
                    "Unavailable".to_owned()
                } else {
                    server.endpoint.clone()
                },
                theme,
            ))
            .child(stat_row("OS", server.os_name.clone(), theme))
            .child(stat_row(
                "Contract",
                format!("v{}.{}", server.protocol_major, server.protocol_minor),
                theme,
            ))
            .child(stat_row(
                "Condition",
                if server.id == "local" {
                    if matches!(state.connection, ConnectionStatus::Connected { .. }) {
                        "Online".to_owned()
                    } else {
                        "Offline".to_owned()
                    }
                } else {
                    server.condition.clone()
                },
                theme,
            ))
            .child(
                text(server.condition_detail.clone())
                    .font(ui_font(13.0, 400.0))
                    .color(theme.color("#969696"))
                    .wrap(),
            );
    }
    if state.fleet.remove_confirm {
        body = body.child(
            text("Remove this saved app-server connection?")
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#dca15f")),
        );
    }
    if !state.fleet.error.is_empty() {
        body = body.child(
            text(state.fleet.error.clone())
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#e07676")),
        );
    }
    let mut actions = row().w_full().gap(8.0);
    if state.fleet.remove_confirm {
        actions = actions.child(
            form_button("Keep", theme)
                .key("fleet-remove-cancel")
                .on_press(|state: &mut State| state.fleet.remove_confirm = false),
        );
        if !state.fleet.busy {
            actions = actions.child(
                form_button("Remove", theme)
                    .key("fleet-remove-confirm")
                    .on_press(|state: &mut State| {
                        if let Some(id) = state.fleet.details_id.clone() {
                            start_mutation(state, Mutation::Remove { id });
                        }
                    }),
            );
        }
    } else {
        if state.fleet.details_id.as_deref() != Some("local") {
            actions = actions.child(
                form_button("Remove", theme)
                    .key("fleet-remove")
                    .on_press(|state: &mut State| state.fleet.remove_confirm = true),
            );
        }
        actions = actions.child(el().grow(1.0)).child(
            form_button("Close", theme)
                .key("fleet-details-close")
                .on_press(close_details),
        );
        if !state.fleet.busy {
            actions = actions.child(
                form_button("Save Name", theme)
                    .key("fleet-details-save")
                    .on_press(|state: &mut State| {
                        if let Some(id) = state.fleet.details_id.clone() {
                            let display_name = state.fleet.details_name_edit.text.trim().to_owned();
                            if id == "local" {
                                start_mutation(state, Mutation::UpdateLocalName { display_name });
                                return;
                            }
                            if let Some(server) =
                                state.fleet.servers.iter().find(|server| server.id == id)
                            {
                                start_mutation(
                                    state,
                                    Mutation::Update {
                                        id,
                                        enabled: server.enabled,
                                        display_name,
                                    },
                                );
                            }
                        }
                    }),
            );
        }
    }
    modal_dialog::render(
        modal_dialog::DialogSpec {
            key: "fleet-details-dialog",
            title: "App Server Details",
            width: 520.0,
            focus_return: "fleet-app-servers",
        },
        body,
        actions,
        theme,
        close_details,
    )
}

fn close_qr(state: &mut State) {
    state.fleet.qr_dialog_open = false;
    state.fleet.qr_generation = state.fleet.qr_generation.wrapping_add(1);
    state.fleet.qr_pixels = None;
    state.fleet.qr_loading = false;
    state.fleet.qr_error.clear();
}

pub(crate) fn qr_dialog(state: &State, theme: ThemeStyle<'_>) -> [El; 2] {
    let mut body = col().w_full().gap(12.0).child(
        text("Scan this QR code with an authenticator app. Use its current six-digit code with the Tailscale link to add this app server on another client.")
            .font(ui_font(13.5, 400.0)).color(theme.color("#bdbdbd")).wrap(),
    ).child(text("Keep this QR private; anyone who scans it can generate pairing codes.")
        .font(ui_font(13.0, 400.0)).color(theme.color("#969696")).wrap());
    if let Some(pixels) = &state.fleet.qr_pixels {
        let (width, height) = pixels.size();
        body = body.child(
            row().w_full().center().child(
                el().key("fleet-authenticator-qr")
                    .size(width as f32, height as f32)
                    .pixels(pixels),
            ),
        );
    } else if state.fleet.qr_loading {
        body = body.child(
            text("Loading QR code…")
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#969696")),
        );
    }
    if !state.fleet.qr_error.is_empty() {
        body = body.child(
            text(state.fleet.qr_error.clone())
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#e07676")),
        );
    }
    body = body.child(stat_row(
        "Tailscale Link",
        if state.fleet.local.endpoint.is_empty() {
            "Unavailable".to_owned()
        } else {
            state.fleet.local.endpoint.clone()
        },
        theme,
    ));
    let actions = row().w_full().child(el().grow(1.0)).child(
        form_button("Close", theme)
            .key("fleet-qr-close")
            .on_press(close_qr),
    );
    modal_dialog::render(
        modal_dialog::DialogSpec {
            key: "fleet-qr-dialog",
            title: "Authenticator QR Code",
            width: 420.0,
            focus_return: "fleet-local-qr",
        },
        body,
        actions,
        theme,
        close_qr,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authenticator_uri_renders_a_scannable_square() {
        let pixels = qr_pixels("otpauth://totp/Beale:example?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=Beale&algorithm=SHA1&digits=6&period=30").unwrap();
        let (width, height) = pixels.size();
        assert_eq!(width, height);
        assert!((120..=400).contains(&width));
    }
}
