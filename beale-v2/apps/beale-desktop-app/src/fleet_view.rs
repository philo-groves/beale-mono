use qrcodegen::{QrCode, QrCodeEcc};
use rune::{Color, El, Icon, Pixels, ScrollSpec, TextEdit, col, el, icon, row, text, text_input};

use crate::{
    AUTH_HANDLE, ConnectionStatus, PrimaryView, SelectionMenu, State, ThemeStyle, ViewLocation,
    extensions_view::{workspace_scope_button, workspace_scope_label},
    fleet_connection::{
        self, BrowserItem, BrowserMutation, FleetServer, Mutation, MutationResult, TailnetDevice,
        VirtualMachineItem, VirtualMachineMutation,
    },
    form_button,
    form_controls::toggle_button,
    icons::{
        EXTENSION_WINDOWS, FLEET, FLEET_APPLE, FLEET_BROWSER, FLEET_LINUX, FLEET_QR,
        FLEET_VIRTUAL_MACHINE, INFO, PLUS, STAR, STAR_FILLED, TRASH, VM_CLONE, VM_CLONE_FILLED,
        VM_RUN, VM_RUN_FILLED,
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
            Self::Available if section == FleetSection::Browsers => "Detected".to_owned(),
            Self::Available if section == FleetSection::VirtualMachines => "Running".to_owned(),
            Self::Available => "Available".to_owned(),
            Self::Enabled if section == FleetSection::Browsers => "Manually Added".to_owned(),
            Self::Enabled if section == FleetSection::VirtualMachines => "Not Running".to_owned(),
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
    browsers: Vec<BrowserItem>,
    browser_loading: bool,
    browser_busy: bool,
    browser_error: String,
    browser_generation: u64,
    virtual_machines: Vec<VirtualMachineItem>,
    vm_loading: bool,
    vm_busy: bool,
    vm_error: String,
    vm_generation: u64,
    pub(crate) browser_dialog_open: bool,
    browser_host_id: String,
    browser_name_edit: TextEdit,
    browser_path_edit: TextEdit,
    loading: bool,
    busy: bool,
    error: String,
    generation: u64,
    pub(crate) connect_dialog_open: bool,
    endpoint_edit: TextEdit,
    code_edit: TextEdit,
    display_name_edit: TextEdit,
    pub(crate) import_dialog_open: bool,
    import_devices: Vec<TailnetDevice>,
    import_loading: bool,
    import_error: String,
    import_selected: Option<String>,
    import_port_edit: TextEdit,
    import_code_edit: TextEdit,
    import_generation: u64,
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
            browsers: Vec::new(),
            browser_loading: false,
            browser_busy: false,
            browser_error: String::new(),
            browser_generation: 0,
            virtual_machines: Vec::new(),
            vm_loading: false,
            vm_busy: false,
            vm_error: String::new(),
            vm_generation: 0,
            browser_dialog_open: false,
            browser_host_id: "local".to_owned(),
            browser_name_edit: TextEdit::new(""),
            browser_path_edit: TextEdit::new(""),
            loading: false,
            busy: false,
            error: String::new(),
            generation: 0,
            connect_dialog_open: false,
            endpoint_edit: TextEdit::new(""),
            code_edit: TextEdit::new(""),
            display_name_edit: TextEdit::new(""),
            import_dialog_open: false,
            import_devices: Vec::new(),
            import_loading: false,
            import_error: String::new(),
            import_selected: None,
            import_port_edit: TextEdit::new("43832"),
            import_code_edit: TextEdit::new(""),
            import_generation: 0,
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
    pub(crate) fn browser_scope_enabled(&self, index: usize) -> bool {
        self.browsers
            .get(index)
            .is_some_and(|item| item.browser.enabled)
    }

    pub(crate) fn vm_scope_enabled(&self, index: usize) -> bool {
        self.virtual_machines
            .get(index)
            .is_some_and(|item| item.machine.enabled)
    }

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
    if state.primary_view == PrimaryView::Fleet && state.fleet_section == FleetSection::Browsers {
        start_browser_load(state);
        return;
    }
    if state.primary_view == PrimaryView::Fleet
        && state.fleet_section == FleetSection::VirtualMachines
    {
        start_virtual_machine_load(state);
        return;
    }
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

fn start_browser_load(state: &mut State) {
    if state.fleet.browser_loading || state.fleet.browser_busy {
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        return;
    };
    state.fleet.browser_loading = true;
    state.fleet.browser_generation = state.fleet.browser_generation.wrapping_add(1);
    let generation = state.fleet.browser_generation;
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map(|runtime| runtime.block_on(fleet_connection::load_browsers()));
        handle.update(move |state, _| {
            if state.fleet.browser_generation != generation {
                return;
            }
            state.fleet.browser_loading = false;
            match result {
                Ok(snapshot) => {
                    state.fleet.browsers = snapshot.items;
                    state.fleet.servers = snapshot.servers;
                    state.fleet.browser_error = snapshot.error;
                }
                Err(_) => {
                    state.fleet.browser_error = "Could not initialize browser inventory.".to_owned()
                }
            }
        });
    });
}

fn start_virtual_machine_load(state: &mut State) {
    if state.fleet.vm_loading || state.fleet.vm_busy {
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        return;
    };
    state.fleet.vm_loading = true;
    state.fleet.vm_generation = state.fleet.vm_generation.wrapping_add(1);
    let generation = state.fleet.vm_generation;
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map(|runtime| runtime.block_on(fleet_connection::load_virtual_machines()));
        handle.update(move |state, _| {
            if state.fleet.vm_generation != generation {
                return;
            }
            state.fleet.vm_loading = false;
            match result {
                Ok(snapshot) => {
                    state.fleet.virtual_machines = snapshot.items;
                    state.fleet.servers = snapshot.servers;
                    state.fleet.vm_error = snapshot.error;
                }
                Err(_) => state.fleet.vm_error = "Could not initialize VM inventory.".to_owned(),
            }
        });
    });
}

fn start_virtual_machine_mutation(state: &mut State, action: VirtualMachineMutation) {
    if state.fleet.vm_busy {
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        state.fleet.vm_error = "Desktop is not ready.".to_owned();
        return;
    };
    state.fleet.vm_busy = true;
    state.fleet.vm_error.clear();
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "Could not initialize VM settings.".to_owned())
            .and_then(|runtime| runtime.block_on(fleet_connection::mutate_virtual_machine(action)));
        handle.update(move |state, _| {
            state.fleet.vm_busy = false;
            match result {
                Ok(()) => start_virtual_machine_load(state),
                Err(error) => state.fleet.vm_error = error,
            }
        });
    });
}

fn virtual_machine_mutation(item: &VirtualMachineItem) -> VirtualMachineMutation {
    VirtualMachineMutation {
        host_id: item.host_id.clone(),
        id: item.machine.id.clone(),
        enabled: item.machine.enabled,
        all_workspaces: item.machine.all_workspaces,
        workspace_ids: item.machine.workspace_ids.clone(),
        can_run: item.machine.can_run,
        can_clone: item.machine.can_clone,
    }
}

fn start_browser_mutation(state: &mut State, action: BrowserMutation) {
    if state.fleet.browser_busy {
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        state.fleet.browser_error = "Desktop is not ready.".to_owned();
        return;
    };
    state.fleet.browser_busy = true;
    state.fleet.browser_error.clear();
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "Could not initialize browser inventory.".to_owned())
            .and_then(|runtime| runtime.block_on(fleet_connection::mutate_browser(action)));
        handle.update(move |state, _| {
            state.fleet.browser_busy = false;
            match result {
                Ok(()) => {
                    state.fleet.browser_dialog_open = false;
                    state.fleet.browser_name_edit.clear();
                    state.fleet.browser_path_edit.clear();
                    start_browser_load(state);
                }
                Err(error) => state.fleet.browser_error = error,
            }
        });
    });
}

pub(crate) fn workspace_scope_items(
    state: &State,
    index: usize,
) -> Vec<crate::in_place_menu::MenuItem> {
    let Some(browser) = state
        .fleet
        .browsers
        .get(index)
        .filter(|item| item.browser.enabled)
    else {
        return Vec::new();
    };
    std::iter::once(crate::in_place_menu::MenuItem {
        value: String::new(),
        label: "All Workspaces".to_owned(),
        detail: None,
        selected: browser.browser.all_workspaces,
    })
    .chain(
        state
            .workspaces
            .iter()
            .map(|workspace| crate::in_place_menu::MenuItem {
                value: workspace.id.clone(),
                label: workspace.name.clone(),
                detail: None,
                selected: browser.browser.workspace_ids.contains(&workspace.id),
            }),
    )
    .collect()
}

pub(crate) fn select_workspace_scope(state: &mut State, index: usize, value: &str) {
    let Some(item) = state
        .fleet
        .browsers
        .get(index)
        .filter(|item| item.browser.enabled)
    else {
        return;
    };
    let (all_workspaces, workspace_ids) = if value.is_empty() {
        (true, Vec::new())
    } else {
        if !state
            .workspaces
            .iter()
            .any(|workspace| workspace.id == value)
        {
            return;
        }
        let mut ids = if item.browser.all_workspaces {
            Vec::new()
        } else {
            item.browser.workspace_ids.clone()
        };
        if let Some(position) = ids.iter().position(|id| id == value) {
            ids.remove(position);
        } else {
            ids.push(value.to_owned());
        }
        (false, ids)
    };
    start_browser_mutation(
        state,
        BrowserMutation::SetSettings {
            host_id: item.host_id.clone(),
            id: item.browser.id.clone(),
            enabled: item.browser.enabled,
            all_workspaces,
            workspace_ids,
        },
    );
}

pub(crate) fn vm_workspace_scope_items(
    state: &State,
    index: usize,
) -> Vec<crate::in_place_menu::MenuItem> {
    let Some(item) = state
        .fleet
        .virtual_machines
        .get(index)
        .filter(|item| item.machine.enabled)
    else {
        return Vec::new();
    };
    std::iter::once(crate::in_place_menu::MenuItem {
        value: String::new(),
        label: "All Workspaces".to_owned(),
        detail: None,
        selected: item.machine.all_workspaces,
    })
    .chain(
        state
            .workspaces
            .iter()
            .map(|workspace| crate::in_place_menu::MenuItem {
                value: workspace.id.clone(),
                label: workspace.name.clone(),
                detail: None,
                selected: item.machine.workspace_ids.contains(&workspace.id),
            }),
    )
    .collect()
}

pub(crate) fn select_vm_workspace_scope(state: &mut State, index: usize, value: &str) {
    let Some(item) = state
        .fleet
        .virtual_machines
        .get(index)
        .filter(|item| item.machine.enabled)
    else {
        return;
    };
    let (all_workspaces, workspace_ids) = if value.is_empty() {
        (true, Vec::new())
    } else {
        if !state
            .workspaces
            .iter()
            .any(|workspace| workspace.id == value)
        {
            return;
        }
        let mut ids = if item.machine.all_workspaces {
            Vec::new()
        } else {
            item.machine.workspace_ids.clone()
        };
        if let Some(position) = ids.iter().position(|id| id == value) {
            ids.remove(position);
        } else {
            ids.push(value.to_owned());
        }
        (false, ids)
    };
    let mut action = virtual_machine_mutation(item);
    action.all_workspaces = all_workspaces;
    action.workspace_ids = workspace_ids;
    start_virtual_machine_mutation(state, action);
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
    let import_connect =
        state.fleet.import_dialog_open && matches!(&action, Mutation::Connect { .. });
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
                    state.fleet.import_dialog_open = false;
                    state.fleet.import_selected = None;
                    state.fleet.import_code_edit.clear();
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
                Err(error) if import_connect => state.fleet.import_error = error,
                Err(error) => state.fleet.error = error,
            }
        });
    });
}

pub(crate) fn start_import(state: &mut State) {
    if state.fleet.busy {
        return;
    }
    state.fleet.import_dialog_open = true;
    state.fleet.import_loading = true;
    state.fleet.import_error.clear();
    state.fleet.import_devices.clear();
    state.fleet.import_selected = None;
    state.fleet.import_code_edit.clear();
    state.fleet.import_port_edit = TextEdit::new(
        std::env::var("BEALE_V2_FLEET_HTTPS_PORT").unwrap_or_else(|_| "43832".to_owned()),
    );
    state.fleet.import_generation = state.fleet.import_generation.wrapping_add(1);
    let generation = state.fleet.import_generation;
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        state.fleet.import_loading = false;
        state.fleet.import_error = "Desktop is not ready.".to_owned();
        return;
    };
    std::thread::spawn(move || {
        let result = (|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize Tailscale import.".to_owned())?;
            runtime.block_on(fleet_connection::import_devices())
        })();
        handle.update(move |state, _| {
            if state.fleet.import_generation != generation || !state.fleet.import_dialog_open {
                return;
            }
            state.fleet.import_loading = false;
            match result {
                Ok(devices) => state.fleet.import_devices = devices,
                Err(error) => state.fleet.import_error = error,
            }
        });
    });
}

pub(crate) fn start_manual_add(state: &mut State) {
    state.fleet.error.clear();
    state.fleet.connect_dialog_open = true;
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

fn stat_block(rows: impl IntoIterator<Item = El>, top_spacing: f32, theme: ThemeStyle<'_>) -> El {
    let divider = || el().w_full().h(1.0).fill(theme.color("#303030"));
    let mut block = col().w_full().mt(top_spacing).child(divider());
    for row in rows {
        block = block.child(row);
    }
    block.child(divider())
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
        .child(stat_block(
            [
                stat_row("OS", server.os_name.clone(), theme),
                stat_row(
                    "Contract",
                    format!("v{}.{}", server.protocol_major, server.protocol_minor),
                    theme,
                ),
                stat_row("Condition", server.condition.clone(), theme),
            ],
            16.0,
            theme,
        ))
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

fn browser_card(index: usize, item: &BrowserItem, state: &State, theme: ThemeStyle<'_>) -> El {
    let host_id = item.host_id.clone();
    let browser_id = item.browser.id.clone();
    let name = item.browser.name.clone();
    let enabled = item.browser.enabled;
    let all_workspaces = item.browser.all_workspaces;
    let workspace_ids = item.browser.workspace_ids.clone();
    let mut action = toggle_button(
        format!("fleet-browser-toggle-{}-{}", host_id, browser_id),
        format!("{} browser", item.browser.name),
        enabled,
        !state.fleet.browser_busy,
        theme,
    );
    if !state.fleet.browser_busy {
        let host_id = host_id.clone();
        let browser_id = browser_id.clone();
        action = action.on_press(move |state: &mut State| {
            start_browser_mutation(
                state,
                BrowserMutation::SetSettings {
                    host_id: host_id.clone(),
                    id: browser_id.clone(),
                    enabled: !enabled,
                    all_workspaces,
                    workspace_ids: workspace_ids.clone(),
                },
            );
        });
    }
    let mut preferred_action = el()
        .key(format!(
            "fleet-browser-preferred-{}-{}",
            host_id, browser_id
        ))
        .role("button")
        .label(format!(
            "{} {} browser",
            if item.browser.preferred {
                "Preferred"
            } else {
                "Prefer"
            },
            item.browser.name
        ))
        .help(if item.browser.preferred {
            "Preferred browser"
        } else {
            "Make preferred browser"
        })
        .size(26.0, 26.0)
        .mr(-5.0)
        .center()
        .radius(8.0)
        .continuous_corners()
        .hover(theme.color("#303030"))
        .child(
            icon(
                if item.browser.preferred {
                    &STAR_FILLED
                } else {
                    &STAR
                },
                17.0,
            )
            .color(theme.color(if item.browser.preferred {
                "#e9c873"
            } else {
                "#bdbdbd"
            })),
        );
    if !item.browser.preferred && enabled && item.browser.available && !state.fleet.browser_busy {
        let host_id = host_id.clone();
        let browser_id = browser_id.clone();
        preferred_action = preferred_action.on_press(move |state: &mut State| {
            start_browser_mutation(
                state,
                BrowserMutation::SetPreferred {
                    host_id: host_id.clone(),
                    id: browser_id.clone(),
                },
            );
        });
    }
    let footer = row()
        .w_full()
        .pt(12.0)
        .center_y()
        .child(action)
        .child(if enabled {
            let menu = SelectionMenu::BrowserWorkspaces { index };
            workspace_scope_button(
                workspace_scope_label(
                    state,
                    item.browser.all_workspaces,
                    &item.browser.workspace_ids,
                ),
                state.selection_menu == Some(menu),
                move |state| crate::toggle_selection_menu(state, menu),
                theme,
            )
            .key(format!("fleet-browser-scope-{index}"))
            .ml(5.0)
            .min_w(0.0)
            .max_w(160.0)
        } else {
            el().size(0.0, 0.0)
        })
        .child(el().grow(1.0))
        .child(preferred_action);
    let remove = if item.browser.manual {
        el().key(format!("fleet-browser-remove-{}-{}", host_id, browser_id))
            .role("button")
            .label(format!("Remove {} browser", item.browser.name))
            .help("Remove browser")
            .size(26.0, 26.0)
            .center()
            .radius(8.0)
            .continuous_corners()
            .hover(theme.color("#303030"))
            .on_press(move |state: &mut State| {
                start_browser_mutation(
                    state,
                    BrowserMutation::Remove {
                        host_id: host_id.clone(),
                        id: browser_id.clone(),
                    },
                );
            })
            .child(icon(&TRASH, 17.0).color(theme.color("#bdbdbd")))
    } else {
        el().size(0.0, 0.0)
    };
    col()
        .key(format!(
            "fleet-browser-card-{}-{}",
            item.host_id, item.browser.id
        ))
        .w_full()
        .min_w(0.0)
        .min_h(170.0)
        .px(12.0)
        .pt(16.0)
        .pb(14.0)
        .radius(24.0)
        .continuous_corners()
        .border(1.0, theme.color("#303030"))
        .child(
            row()
                .w_full()
                .gap(10.0)
                .center_y()
                .child(browser_mark(&item.browser.name, theme))
                .child(
                    text(name)
                        .grow(1.0)
                        .min_w(0.0)
                        .max_lines(2)
                        .font(ui_font(16.0, 600.0))
                        .color(theme.color("#f2f2f2")),
                )
                .child(remove),
        )
        .child(stat_block(
            [
                stat_row("Host", item.host_name.clone(), theme),
                stat_row(
                    "Status",
                    if item.browser.available {
                        "Available"
                    } else {
                        "Missing"
                    }
                    .to_owned(),
                    theme,
                ),
                stat_row(
                    "Source",
                    if item.browser.manual {
                        "Added"
                    } else {
                        "Detected"
                    }
                    .to_owned(),
                    theme,
                ),
            ],
            12.0,
            theme,
        ))
        .child(footer)
}

fn browser_mark(name: &str, theme: ThemeStyle<'_>) -> El {
    let mark = el()
        .size(40.0, 40.0)
        .center()
        .radius(12.0)
        .fill(theme.color("#303030"));
    if let Some(pixels) = crate::browser_logos::for_name(name) {
        mark.child(el().size(32.0, 32.0).pixels(pixels))
    } else {
        mark.child(icon(&FLEET_BROWSER, 23.0).color(theme.color("#d8d8d8")))
    }
}

#[derive(Clone, Copy)]
enum VmAllowance {
    Run,
    Clone,
}

fn vm_allowance_button(
    index: usize,
    item: &VirtualMachineItem,
    allowance: VmAllowance,
    state: &State,
    theme: ThemeStyle<'_>,
) -> El {
    let (key, name, active, outline, filled) = match allowance {
        VmAllowance::Run => (
            "run",
            "Can Run",
            item.machine.can_run,
            &VM_RUN,
            &VM_RUN_FILLED,
        ),
        VmAllowance::Clone => (
            "clone",
            "Can Clone",
            item.machine.can_clone,
            &VM_CLONE,
            &VM_CLONE_FILLED,
        ),
    };
    let mut button = el()
        .key(format!("fleet-vm-{key}-{index}"))
        .role("button")
        .label(format!(
            "{} {name} allowance: {}",
            item.machine.name,
            if active { "On" } else { "Off" }
        ))
        .help(format!("{name}: {}", if active { "On" } else { "Off" }))
        .size(26.0, 26.0)
        .center()
        .radius(8.0)
        .continuous_corners()
        .hover(theme.color("#303030"))
        .child(
            icon(if active { filled } else { outline }, 17.0).color(theme.color(if active {
                "#e9c873"
            } else {
                "#bdbdbd"
            })),
        );
    if !state.fleet.vm_busy {
        let mut action = virtual_machine_mutation(item);
        match allowance {
            VmAllowance::Run => action.can_run = !active,
            VmAllowance::Clone => action.can_clone = !active,
        }
        button = button.on_press(move |state: &mut State| {
            start_virtual_machine_mutation(state, action.clone());
        });
    }
    button
}

fn virtual_machine_card(
    index: usize,
    item: &VirtualMachineItem,
    state: &State,
    theme: ThemeStyle<'_>,
) -> El {
    let enabled = item.machine.enabled;
    let mut toggle = toggle_button(
        format!("fleet-vm-toggle-{index}"),
        format!("{} virtual machine", item.machine.name),
        enabled,
        !state.fleet.vm_busy,
        theme,
    );
    if !state.fleet.vm_busy {
        let mut action = virtual_machine_mutation(item);
        action.enabled = !enabled;
        toggle = toggle.on_press(move |state: &mut State| {
            start_virtual_machine_mutation(state, action.clone());
        });
    }
    let footer = row()
        .w_full()
        .pt(12.0)
        .center_y()
        .child(toggle)
        .child(if enabled {
            let menu = SelectionMenu::VirtualMachineWorkspaces { index };
            workspace_scope_button(
                workspace_scope_label(
                    state,
                    item.machine.all_workspaces,
                    &item.machine.workspace_ids,
                ),
                state.selection_menu == Some(menu),
                move |state| crate::toggle_selection_menu(state, menu),
                theme,
            )
            .key(format!("fleet-vm-scope-{index}"))
            .ml(5.0)
            .min_w(0.0)
            .max_w(140.0)
        } else {
            el().size(0.0, 0.0)
        })
        .child(el().grow(1.0))
        .child(vm_allowance_button(
            index,
            item,
            VmAllowance::Run,
            state,
            theme,
        ))
        .child(vm_allowance_button(index, item, VmAllowance::Clone, state, theme).mr(-5.0));
    col()
        .key(format!(
            "fleet-vm-card-{}-{}",
            item.host_id, item.machine.id
        ))
        .w_full()
        .min_w(0.0)
        .min_h(170.0)
        .px(12.0)
        .pt(16.0)
        .pb(14.0)
        .radius(24.0)
        .continuous_corners()
        .border(1.0, theme.color("#303030"))
        .child(
            row()
                .w_full()
                .gap(10.0)
                .center_y()
                .child(
                    el().size(40.0, 40.0)
                        .center()
                        .radius(12.0)
                        .continuous_corners()
                        .fill(theme.color("#273e58"))
                        .child(icon(&EXTENSION_WINDOWS, 23.0).color(theme.color("#c7dff3"))),
                )
                .child(
                    text(item.machine.name.clone())
                        .grow(1.0)
                        .min_w(0.0)
                        .max_lines(2)
                        .font(ui_font(16.0, 600.0))
                        .color(theme.color("#f2f2f2")),
                ),
        )
        .child(stat_block(
            [
                stat_row("Host", item.host_name.clone(), theme),
                stat_row("Status", item.machine.state.clone(), theme),
                stat_row("Source", item.machine.provider.clone(), theme),
                stat_row("Generation", item.machine.generation.to_string(), theme),
            ],
            12.0,
            theme,
        ))
        .child(footer)
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
        .center_y();
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
                    crate::toggle_selection_menu(state, crate::SelectionMenu::FleetAdd);
                })
                .child(icon(&PLUS, 17.0).color(theme.color("#bdbdbd"))),
        );
    }
    if section == FleetSection::Browsers {
        tabs = tabs.child(
            el().key("fleet-add-browser")
                .role("button")
                .label("Add a browser")
                .help("Add a browser")
                .size(26.0, 26.0)
                .center()
                .radius(8.0)
                .continuous_corners()
                .hover(theme.color("#303030"))
                .on_press(|state: &mut State| {
                    state.fleet.browser_error.clear();
                    state.fleet.browser_dialog_open = true;
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
    if section == FleetSection::Browsers && !state.fleet.browser_error.is_empty() {
        content.push(
            text(state.fleet.browser_error.clone())
                .key("fleet-browser-error")
                .mt(10.0)
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#e07676")),
        );
    }
    if section == FleetSection::VirtualMachines && !state.fleet.vm_error.is_empty() {
        content.push(
            text(state.fleet.vm_error.clone())
                .key("fleet-vm-error")
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
    if section == FleetSection::Browsers {
        for (index, item) in state
            .fleet
            .browsers
            .iter()
            .enumerate()
            .filter(|(_, item)| match state.fleet.tab {
                FleetTab::All => true,
                FleetTab::Available => !item.browser.manual,
                FleetTab::Enabled => item.browser.manual,
            })
        {
            count += 1;
            cards = cards.child(browser_card(index, item, state, theme));
        }
    }
    if section == FleetSection::VirtualMachines {
        for (index, item) in state
            .fleet
            .virtual_machines
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                let running = item.machine.state.starts_with("Running");
                match state.fleet.tab {
                    FleetTab::All => true,
                    FleetTab::Available => running,
                    FleetTab::Enabled => !running,
                }
            })
        {
            count += 1;
            cards = cards.child(virtual_machine_card(index, item, state, theme));
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
            (FleetSection::Browsers, _) if state.fleet.browser_loading => "Loading browsers…",
            (FleetSection::Browsers, FleetTab::Enabled) => "No manually added browsers.",
            (FleetSection::Browsers, FleetTab::Available) => "No browsers detected.",
            (FleetSection::Browsers, _) => "No browsers available.",
            (FleetSection::VirtualMachines, _) if state.fleet.vm_loading => {
                "Loading virtual machines…"
            }
            (FleetSection::VirtualMachines, FleetTab::Available) => "No running virtual machines.",
            (FleetSection::VirtualMachines, FleetTab::Enabled) => {
                "No virtual machines are stopped or paused."
            }
            (FleetSection::VirtualMachines, _) => "No virtual machines detected.",
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

fn close_browser_dialog(state: &mut State) {
    if state.fleet.browser_busy {
        return;
    }
    state.fleet.browser_dialog_open = false;
    state.fleet.browser_error.clear();
}

pub(crate) fn browser_dialog(state: &State, theme: ThemeStyle<'_>) -> [El; 2] {
    let mut host_rows = col().w_full().gap(5.0);
    let host_count = 1 + state
        .fleet
        .servers
        .iter()
        .filter(|server| server.enabled)
        .count();
    let hosts_label = text("App Server")
        .font(ui_font(14.0, 500.0))
        .color(theme.color("#d4d4d4"));
    let host_choices = std::iter::once((&state.fleet.local, "local")).chain(
        state
            .fleet
            .servers
            .iter()
            .filter(|server| server.enabled)
            .map(|server| (server, server.id.as_str())),
    );
    for (server, id) in host_choices {
        let selected = state.fleet.browser_host_id == id;
        let host_id = id.to_owned();
        let label = if id == "local" {
            "Local App Server".to_owned()
        } else {
            server_name(server).to_owned()
        };
        host_rows = host_rows.child(
            row()
                .w_full()
                .h(32.0)
                .center_y()
                .gap(10.0)
                .child(
                    text(label.clone())
                        .grow(1.0)
                        .min_w(0.0)
                        .truncate()
                        .font(ui_font(13.0, 400.0))
                        .color(theme.color("#bdbdbd")),
                )
                .child(
                    form_button(if selected { "Selected" } else { "Select" }, theme)
                        .key(format!("fleet-browser-host-{id}"))
                        .on_press(move |state: &mut State| {
                            state.fleet.browser_host_id = host_id.clone()
                        })
                        .help(label.clone()),
                ),
        );
    }
    let hosts = col().w_full().gap(5.0).child(hosts_label).child(
        col()
            .w_full()
            .h((host_count as f32 * 37.0).min(170.0))
            .min_h(0.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(host_rows),
    );
    let body = col()
        .w_full()
        .gap(12.0)
        .child(
            text("Enter the browser executable path on the selected app-server host.")
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#969696"))
                .wrap(),
        )
        .child(hosts)
        .child(input_field(
            "Browser Name",
            "fleet-browser-name",
            &state.fleet.browser_name_edit,
            false,
            |state| &mut state.fleet.browser_name_edit,
            theme,
        ))
        .child(input_field(
            "Executable Path",
            "fleet-browser-path",
            &state.fleet.browser_path_edit,
            false,
            |state| &mut state.fleet.browser_path_edit,
            theme,
        ))
        .child(
            text(state.fleet.browser_error.clone())
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#e07676")),
        );
    let mut actions = row().w_full().child(el().grow(1.0)).child(
        form_button("Cancel", theme)
            .key("fleet-browser-cancel")
            .on_press(close_browser_dialog),
    );
    if !state.fleet.browser_busy {
        actions = actions.child(
            form_button("Add Browser", theme)
                .key("fleet-browser-save")
                .on_press(|state: &mut State| {
                    start_browser_mutation(
                        state,
                        BrowserMutation::Add {
                            host_id: state.fleet.browser_host_id.clone(),
                            name: state.fleet.browser_name_edit.text.trim().to_owned(),
                            executable: state.fleet.browser_path_edit.text.trim().to_owned(),
                        },
                    );
                }),
        );
    }
    modal_dialog::render(
        modal_dialog::DialogSpec {
            key: "fleet-browser-dialog",
            title: "Add a Browser",
            width: 520.0,
            focus_return: "fleet-add-browser",
        },
        body,
        actions,
        theme,
        close_browser_dialog,
    )
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

fn close_import(state: &mut State) {
    if state.fleet.busy {
        return;
    }
    state.fleet.import_dialog_open = false;
    state.fleet.import_generation = state.fleet.import_generation.wrapping_add(1);
    state.fleet.import_loading = false;
    state.fleet.import_devices.clear();
    state.fleet.import_selected = None;
    state.fleet.import_code_edit.clear();
    state.fleet.import_error.clear();
}

fn connect_import(state: &mut State) {
    let Some(device) = state.fleet.import_devices.iter().find(|device| {
        device.online && state.fleet.import_selected.as_deref() == Some(device.dns_name.as_str())
    }) else {
        state.fleet.import_error = "Select an online Tailscale device.".to_owned();
        return;
    };
    let endpoint =
        match fleet_connection::imported_endpoint(device, &state.fleet.import_port_edit.text) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                state.fleet.import_error = error;
                return;
            }
        };
    let code = state.fleet.import_code_edit.text.trim().to_owned();
    if code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        state.fleet.import_error = "Enter the current six-digit authenticator code.".to_owned();
        return;
    }
    state.fleet.import_error.clear();
    start_mutation(
        state,
        Mutation::Connect {
            endpoint,
            code,
            display_name: String::new(),
        },
    );
}

pub(crate) fn import_dialog(state: &State, theme: ThemeStyle<'_>) -> [El; 2] {
    let mut body = col().w_full().gap(12.0).child(
        text("Select an online device from this tailnet. Beale verifies the app server and requires its authenticator code before saving a connection.")
            .font(ui_font(13.0, 400.0))
            .color(theme.color("#bdbdbd"))
            .wrap(),
    );
    if state.fleet.import_loading {
        body = body.child(
            text("Loading Tailscale devices…")
                .key("fleet-import-loading")
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#969696")),
        );
    } else if state.fleet.import_devices.is_empty() && state.fleet.import_error.is_empty() {
        body = body.child(
            text("No other Tailscale devices are available.")
                .key("fleet-import-empty")
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#969696")),
        );
    } else if !state.fleet.import_devices.is_empty() {
        let mut devices = col().w_full().gap(6.0);
        for (index, device) in state.fleet.import_devices.iter().enumerate() {
            let selected = state.fleet.import_selected.as_deref() == Some(&device.dns_name);
            let dns_name = device.dns_name.clone();
            let mut details = col().grow(1.0).min_w(0.0).gap(3.0).child(
                text(device.host_name.clone())
                    .truncate()
                    .font(ui_font(14.0, 550.0))
                    .color(theme.color("#f2f2f2")),
            );
            details = details
                .child(
                    text(device.dns_name.clone())
                        .truncate()
                        .font(ui_font(12.0, 400.0))
                        .color(theme.color("#969696")),
                )
                .child(
                    text(format!(
                        "{} · {}",
                        device.os_name,
                        if device.online { "Online" } else { "Offline" }
                    ))
                    .font(ui_font(12.0, 400.0))
                    .color(theme.color("#bdbdbd")),
                );
            let action = if device.online {
                form_button(if selected { "Selected" } else { "Select" }, theme)
                    .key(format!("fleet-import-select-{index}"))
                    .on_press(move |state: &mut State| {
                        state.fleet.import_selected = Some(dns_name.clone());
                        state.fleet.import_error.clear();
                    })
            } else {
                text("Offline")
                    .font(ui_font(12.0, 400.0))
                    .color(theme.color("#969696"))
            };
            devices = devices.child(
                row()
                    .key(format!("fleet-import-device-{index}"))
                    .w_full()
                    .min_w(0.0)
                    .h(74.0)
                    .px(12.0)
                    .gap(12.0)
                    .center_y()
                    .radius(10.0)
                    .continuous_corners()
                    .border(
                        1.0,
                        theme.color(if selected { "#777777" } else { "#303030" }),
                    )
                    .child(details)
                    .child(action),
            );
        }
        body = body.child(
            col()
                .key("fleet-import-device-list")
                .w_full()
                .h(if state.fleet.import_selected.is_some() {
                    160.0
                } else {
                    260.0
                })
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(devices),
        );
    }
    if let Some(selected) = state.fleet.import_selected.as_ref() {
        body = body
            .child(
                text(format!("Selected: {selected}"))
                    .key("fleet-import-selected")
                    .font(ui_font(13.0, 500.0))
                    .color(theme.color("#d4d4d4")),
            )
            .child(input_field(
                "Tailscale HTTPS Port",
                "fleet-import-port-input",
                &state.fleet.import_port_edit,
                false,
                |state| &mut state.fleet.import_port_edit,
                theme,
            ))
            .child(
                text("Use the remote host's Tailscale Serve HTTPS port (43832 by default).")
                    .font(ui_font(12.0, 400.0))
                    .color(theme.color("#969696")),
            )
            .child(input_field(
                "Authenticator Code",
                "fleet-import-code-input",
                &state.fleet.import_code_edit,
                false,
                |state| &mut state.fleet.import_code_edit,
                theme,
            ));
    }
    if !state.fleet.import_error.is_empty() {
        body = body.child(
            text(state.fleet.import_error.clone())
                .key("fleet-import-error")
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#e07676"))
                .wrap(),
        );
    }
    let mut actions = row().w_full().gap(8.0).child(el().grow(1.0));
    if !state.fleet.busy && !state.fleet.import_loading {
        actions = actions.child(
            form_button("Refresh", theme)
                .key("fleet-import-refresh")
                .on_press(start_import),
        );
    }
    actions = actions.child(
        form_button("Cancel", theme)
            .key("fleet-import-cancel")
            .on_press(close_import),
    );
    if state.fleet.import_selected.is_some() && !state.fleet.busy {
        actions = actions.child(
            form_button("Connect", theme)
                .key("fleet-import-connect")
                .on_press(connect_import),
        );
    }
    modal_dialog::render(
        modal_dialog::DialogSpec {
            key: "fleet-import-dialog",
            title: "Tailscale Import",
            width: 620.0,
            focus_return: "fleet-add-server",
        },
        body,
        actions,
        theme,
        close_import,
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
    use rune::{Env, Runtime};

    #[test]
    fn browser_card_exposes_settings_and_workspace_scope() {
        let workspace_id = uuid::Uuid::nil().to_string();
        let mut state = State::default();
        state.fleet.browsers.push(BrowserItem {
            host_id: "local".to_owned(),
            host_name: "Local App Server".to_owned(),
            browser: beale_contracts::v1::FleetBrowser {
                id: "detected:example".to_owned(),
                name: "Example Browser".to_owned(),
                manual: false,
                available: true,
                enabled: true,
                all_workspaces: false,
                workspace_ids: vec![workspace_id.clone()],
                preferred: false,
            },
        });
        state
            .workspaces
            .push(beale_contracts::v1::WorkspaceSummary {
                id: workspace_id,
                name: "Example Workspace".to_owned(),
                directory: "example-workspace".to_owned(),
            });
        let items = workspace_scope_items(&state, 0);
        assert!(!items[0].selected);
        assert!(items[1].selected);
        let mut runtime = Runtime::new(
            state,
            |state, _| browser_card(0, &state.fleet.browsers[0], state, state.appearance.style()),
            (300.0, 240.0),
            Env::new(true, 1.0),
        );
        runtime.frame(0.0);
        assert!(
            runtime
                .find("fleet-browser-toggle-local-detected:example")
                .is_some()
        );
        assert!(runtime.find("fleet-browser-scope-0").is_some());
        assert!(
            runtime
                .find("fleet-browser-preferred-local-detected:example")
                .is_some()
        );
    }

    #[test]
    fn hyper_v_card_uses_fleet_card_layout() {
        let item = VirtualMachineItem {
            host_id: "local".to_owned(),
            host_name: "Local App Server".to_owned(),
            machine: beale_contracts::v1::FleetVirtualMachine {
                id: "hyper-v:00000000-0000-4000-8000-000000000001".to_owned(),
                name: "Example VM".to_owned(),
                provider: "Hyper-V".to_owned(),
                state: "Off".to_owned(),
                generation: 2,
                enabled: true,
                all_workspaces: true,
                workspace_ids: Vec::new(),
                can_run: false,
                can_clone: false,
            },
        };
        let mut state = State::default();
        state.fleet.virtual_machines.push(item);
        assert!(vm_workspace_scope_items(&state, 0)[0].selected);
        assert!(!state.fleet.virtual_machines[0].machine.can_run);
        assert!(!state.fleet.virtual_machines[0].machine.can_clone);
        let mut runtime = Runtime::new(
            state,
            |state, _| {
                virtual_machine_card(
                    0,
                    &state.fleet.virtual_machines[0],
                    state,
                    state.appearance.style(),
                )
            },
            (300.0, 240.0),
            Env::new(true, 1.0),
        );
        runtime.frame(0.0);
        assert!(
            runtime
                .find("fleet-vm-card-local-hyper-v:00000000-0000-4000-8000-000000000001")
                .is_some()
        );
        assert!(runtime.find("fleet-vm-toggle-0").is_some());
        assert!(runtime.find("fleet-vm-scope-0").is_some());
        assert!(runtime.find("fleet-vm-run-0").is_some());
        assert!(runtime.find("fleet-vm-clone-0").is_some());
        assert_eq!(
            FleetTab::Available.label(FleetSection::VirtualMachines),
            "Running"
        );
        assert_eq!(
            FleetTab::Enabled.label(FleetSection::VirtualMachines),
            "Not Running"
        );
    }

    #[test]
    fn authenticator_uri_renders_a_scannable_square() {
        let pixels = qr_pixels("otpauth://totp/Beale:example?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=Beale&algorithm=SHA1&digits=6&period=30").unwrap();
        let (width, height) = pixels.size();
        assert_eq!(width, height);
        assert!((120..=400).contains(&width));
    }

    #[test]
    fn import_dialog_requires_selection_and_authenticator_code() {
        let mut state = State {
            primary_view: PrimaryView::Fleet,
            ..State::default()
        };
        state.fleet.import_dialog_open = true;
        state.fleet.import_devices = vec![
            TailnetDevice {
                dns_name: "server.example.ts.net".to_owned(),
                host_name: "Example server".to_owned(),
                os_name: "Linux".to_owned(),
                online: true,
            },
            TailnetDevice {
                dns_name: "offline.example.ts.net".to_owned(),
                host_name: "Offline example".to_owned(),
                os_name: "Windows".to_owned(),
                online: false,
            },
        ];
        let mut runtime = Runtime::new(state, crate::view, (840.0, 600.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        assert!(runtime.find("fleet-import-dialog").is_some());
        assert!(runtime.find("fleet-import-connect").is_none());
        assert!(runtime.find("fleet-import-select-1").is_none());
        let (x, y) = runtime.find("fleet-import-select-0").unwrap();
        runtime.pointer_move(x, y);
        runtime.frame(1.0);
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        runtime.frame(1.0);
        assert_eq!(
            runtime.state.fleet.import_selected.as_deref(),
            Some("server.example.ts.net")
        );
        assert!(runtime.find("fleet-import-code-input").is_some());
        let (x, y) = runtime.find("fleet-import-connect").unwrap();
        runtime.pointer_move(x, y);
        runtime.frame(1.0);
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert_eq!(
            runtime.state.fleet.import_error,
            "Enter the current six-digit authenticator code."
        );
    }
}
