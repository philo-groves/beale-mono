use crate::design::{FadeText, SIDENAV_ACTIVE_FOREGROUND};
use qrcodegen::{QrCode, QrCodeEcc};
use rune::{Color, El, Icon, Pixels, ScrollSpec, TextEdit, col, el, icon, row, text, text_input};

use crate::{
    AUTH_HANDLE, ConnectionStatus, PrimaryView, SelectionMenu, State, ThemeStyle, ViewLocation,
    extensions_view::{workspace_scope_button, workspace_scope_label},
    fleet_connection::{
        self, FleetServer, Mutation, MutationResult, ResearchToolHost, ResearchToolMutation,
        TailnetDevice, VirtualMachineItem, VirtualMachineMutation,
    },
    form_button,
    form_controls::toggle_button,
    icons::{
        CHECK, FLEET, FLEET_APPLE, FLEET_LINUX, FLEET_QR, FLEET_VIRTUAL_MACHINE, FLEET_WINDOWS,
        INFO, PLUS, VM_CLONE, VM_CLONE_FILLED, VM_RUN, VM_RUN_FILLED, WRENCH,
    },
    modal_dialog, ui_font,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum FleetSection {
    #[default]
    AppServers,
    VirtualMachines,
    ResearchTools,
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
            Self::Available if section == FleetSection::VirtualMachines => "Running".to_owned(),
            Self::Available => "Available".to_owned(),
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
    virtual_machines: Vec<VirtualMachineItem>,
    vm_loading: bool,
    vm_busy: bool,
    vm_error: String,
    vm_generation: u64,
    tool_hosts: Vec<ResearchToolHost>,
    selected_tool_host_id: String,
    tools_loading: bool,
    tools_busy: bool,
    tools_error: String,
    tools_generation: u64,
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
            virtual_machines: Vec::new(),
            vm_loading: false,
            vm_busy: false,
            vm_error: String::new(),
            vm_generation: 0,
            tool_hosts: Vec::new(),
            selected_tool_host_id: "local".to_owned(),
            tools_loading: false,
            tools_busy: false,
            tools_error: String::new(),
            tools_generation: 0,
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
    const ALL: [Self; 3] = [Self::AppServers, Self::VirtualMachines, Self::ResearchTools];

    fn index(self) -> usize {
        match self {
            Self::AppServers => 0,
            Self::VirtualMachines => 1,
            Self::ResearchTools => 2,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::AppServers => "App Servers",
            Self::VirtualMachines => "Virtual Machines",
            Self::ResearchTools => "Research Tools",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::AppServers => "fleet-app-servers",
            Self::VirtualMachines => "fleet-virtual-machines",
            Self::ResearchTools => "fleet-research-tools",
        }
    }

    fn glyph(self) -> &'static Icon {
        match self {
            Self::AppServers => &FLEET,
            Self::VirtualMachines => &FLEET_VIRTUAL_MACHINE,
            Self::ResearchTools => &WRENCH,
        }
    }
}

impl FleetState {
    pub(crate) fn preview_fixture(&mut self) {
        self.local.host_name = "example-research-host".into();
        self.local.display_name = "Example research host".into();
        self.local.condition = "Connected".into();
        self.local.os_name = "Windows".into();
        self.local.os_vendor = "Microsoft".into();
        self.virtual_machines.push(VirtualMachineItem {
            host_id: "local".into(),
            host_name: "Example research host".into(),
            machine: beale_contracts::v1::FleetVirtualMachine {
                id: "example-vm".into(),
                name: "Example analysis environment".into(),
                provider: "Hyper-V".into(),
                state: "Off".into(),
                generation: 2,
                enabled: true,
                all_workspaces: true,
                ..Default::default()
            },
        });
        self.tool_hosts.push(ResearchToolHost {
            id: "local".into(),
            name: "Example research host".into(),
            tools: [
                (
                    "ripgrep",
                    "ripgrep",
                    "Source",
                    "Fast search across source and artifacts.",
                ),
                (
                    "opengrep",
                    "Opengrep",
                    "Source",
                    "Rule-based source scanning.",
                ),
                ("git", "Git", "Source", "Repository history and changes."),
                (
                    "radare2",
                    "radare2",
                    "Binary",
                    "Scriptable binary inspection and analysis.",
                ),
                (
                    "ghidra",
                    "Ghidra",
                    "Binary",
                    "Disassembly and decompilation.",
                ),
                (
                    "llvm",
                    "LLVM tools",
                    "Binary",
                    "Object, symbol, and disassembly utilities.",
                ),
                (
                    "jadx",
                    "JADX",
                    "Mobile",
                    "Android APK and DEX decompilation.",
                ),
                (
                    "ipsw",
                    "ipsw",
                    "Mobile",
                    "Apple firmware and binary inspection.",
                ),
                (
                    "python",
                    "Python 3",
                    "Runtimes",
                    "Runtime for research scripts.",
                ),
                (
                    "java",
                    "Java JDK",
                    "Runtimes",
                    "Runtime required by Java-based tools.",
                ),
                (
                    "node",
                    "Node.js",
                    "Runtimes",
                    "Runtime for JavaScript research tooling.",
                ),
            ]
            .into_iter()
            .map(
                |(id, name, category, description)| beale_contracts::v1::FleetResearchTool {
                    id: id.into(),
                    name: name.into(),
                    category: category.into(),
                    description: description.into(),
                    allow_locally: id == "ripgrep",
                    allow_in_vms: id == "ghidra",
                },
            )
            .collect(),
        });
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
                .h(crate::design::SIDENAV_ROW_HEIGHT)
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
                    SIDENAV_ACTIVE_FOREGROUND
                } else {
                    "#bdbdbd"
                })))
                .child(
                    text(section.label())
                        .font(ui_font(14.0, if active { 550.0 } else { 400.0 }))
                        .color(theme.color(if active {
                            SIDENAV_ACTIVE_FOREGROUND
                        } else {
                            "#c8c8c8"
                        })),
                ),
        );
    }
    col()
        .key("fleet-sidenav")
        .role("navigation")
        .label("Fleet sections")
        .w(width)
        .px(8.0)
        .pt(10.0)
        .child(items)
}

pub(crate) fn start_load(state: &mut State) {
    if state.primary_view == PrimaryView::Fleet
        && state.fleet_section == FleetSection::ResearchTools
    {
        start_research_tools_load(state);
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

fn start_research_tools_load(state: &mut State) {
    if state.fleet.tools_loading || state.fleet.tools_busy {
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        return;
    };
    state.fleet.tools_loading = true;
    state.fleet.tools_generation = state.fleet.tools_generation.wrapping_add(1);
    let generation = state.fleet.tools_generation;
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map(|runtime| runtime.block_on(fleet_connection::load_research_tools()));
        handle.update(move |state, _| {
            if state.fleet.tools_generation != generation {
                return;
            }
            state.fleet.tools_loading = false;
            match result {
                Ok(snapshot) => {
                    state.fleet.tool_hosts = snapshot.hosts;
                    state.fleet.tools_error = snapshot.error;
                    if !state
                        .fleet
                        .tool_hosts
                        .iter()
                        .any(|host| host.id == state.fleet.selected_tool_host_id)
                        && let Some(host) = state.fleet.tool_hosts.first()
                    {
                        state.fleet.selected_tool_host_id = host.id.clone();
                    }
                }
                Err(_) => {
                    state.fleet.tools_error = "Could not initialize research tools.".to_owned()
                }
            }
        });
    });
}

fn start_research_tool_mutation(state: &mut State, action: ResearchToolMutation) {
    if state.fleet.tools_busy {
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        state.fleet.tools_error = "Desktop is not ready.".to_owned();
        return;
    };
    state.fleet.tools_busy = true;
    state.fleet.tools_error.clear();
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "Could not initialize research tool settings.".to_owned())
            .and_then(|runtime| runtime.block_on(fleet_connection::mutate_research_tool(action)));
        handle.update(move |state, _| {
            state.fleet.tools_busy = false;
            match result {
                Ok(()) => start_research_tools_load(state),
                Err(error) => state.fleet.tools_error = error,
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
    let glyph = match (tab, section) {
        (FleetTab::All, _) => section.glyph(),
        (FleetTab::Available, FleetSection::VirtualMachines) => &VM_RUN,
        (FleetTab::Enabled, FleetSection::VirtualMachines) => &crate::icons::STOP,
        (FleetTab::Available, FleetSection::AppServers) => &PLUS,
        (FleetTab::Enabled, FleetSection::AppServers) => &crate::icons::CHECK,
        (_, FleetSection::ResearchTools) => &WRENCH,
    };
    crate::design::segmented_tab(tab.label(section), Some(glyph), active, true, theme)
        .key(tab.key())
        .on_press(move |state: &mut State| {
            state.navigate_to(ViewLocation::Fleet(section, tab));
        })
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

fn stat_block(
    rows: impl IntoIterator<Item = El>,
    top_spacing: f32,
    top_divider: bool,
    theme: ThemeStyle<'_>,
) -> El {
    let divider = || el().w_full().h(1.0).fill(theme.color("#303030"));
    let mut block = col().w_full().mt(top_spacing);
    if top_divider {
        block = block.child(divider());
    }
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

fn resource_size(bytes: u64) -> String {
    const MIB: f64 = 1_048_576.0;
    const GIB: f64 = 1_073_741_824.0;
    const TIB: u64 = 1_099_511_627_776;
    if bytes >= TIB {
        format!("{:.1} TiB", bytes as f64 / TIB as f64)
    } else if bytes >= GIB as u64 {
        format!("{:.1} GiB", bytes as f64 / GIB)
    } else {
        format!("{:.1} MiB", bytes as f64 / MIB)
    }
}

fn resource_values(server: &FleetServer, detailed: bool) -> [String; 3] {
    let Some(resources) = &server.resources else {
        return std::array::from_fn(|_| "Unavailable".to_owned());
    };
    let cpu = if resources.cpu_logical_cores == 0 {
        "Unavailable".to_owned()
    } else {
        format!("{} logical cores", resources.cpu_logical_cores)
    };
    let size = |total, available| {
        if total == 0 {
            "Unavailable".to_owned()
        } else if detailed {
            format!(
                "{} available / {} total",
                resource_size(available),
                resource_size(total)
            )
        } else {
            resource_size(total)
        }
    };
    [
        cpu,
        size(
            resources.memory_total_bytes,
            resources.memory_available_bytes,
        ),
        size(resources.disk_total_bytes, resources.disk_available_bytes),
    ]
}

fn server_mark(server: &FleetServer, theme: ThemeStyle<'_>) -> El {
    let (glyph, foreground, size) = match server.os_vendor.as_str() {
        "Apple" => (&FLEET_APPLE, "#f2f2f2", 28.0),
        "Microsoft" => (&FLEET_WINDOWS, "#0078d4", 28.0),
        "Linux" => (&FLEET_LINUX, "#f2f2f2", 30.0),
        _ => (&FLEET, "#bdbdbd", 25.0),
    };
    el().size(40.0, 40.0)
        .center()
        .child(icon(glyph, size).color(theme.color(foreground)))
}

/// Card mark (40) plus its gap (10) beside a card title.
const CARD_MARK_WIDTH: f32 = 50.0;

fn fleet_columns(page_width: f32) -> usize {
    ((page_width + 16.0) / (220.0 + 16.0))
        .floor()
        .clamp(1.0, 4.0) as usize
}

fn server_card(server: &FleetServer, state: &State, text_width: f32, theme: ThemeStyle<'_>) -> El {
    let is_local = server.id == "local";
    let available = !state.fleet.busy && !is_local;
    let id = server.id.clone();
    let enabled = server.enabled;
    let display_name = server.display_name.clone();
    let [cpu, memory, disk] = resource_values(server, false);
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
        .radius(crate::design::CARD_RADIUS)
        .continuous_corners()
        .fill(theme.color("#191919"))
        .border(1.0, theme.color("#2a2a2a"))
        .child(
            row()
                .w_full()
                .h(40.0)
                .gap(10.0)
                .center_y()
                .child(server_mark(server, theme))
                .child(
                    crate::design::faded_lines(
                        server_name(server).to_owned(),
                        ui_font(16.0, 600.0),
                        theme.color("#f2f2f2"),
                        2,
                        text_width - CARD_MARK_WIDTH,
                    )
                    .grow(1.0)
                    .min_w(0.0),
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
            12.0,
            true,
            theme,
        ))
        .child(stat_block(
            [
                stat_row("CPU", cpu, theme),
                stat_row("Memory", memory, theme),
                stat_row("Disk", disk, theme),
            ],
            2.0,
            false,
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
                "#f2f2f2"
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
    text_width: f32,
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
        .radius(crate::design::CARD_RADIUS)
        .continuous_corners()
        .fill(theme.color("#191919"))
        .border(1.0, theme.color("#2a2a2a"))
        .child(
            row()
                .w_full()
                .gap(10.0)
                .center_y()
                .child(
                    el().size(40.0, 40.0)
                        .center()
                        .child(icon(&FLEET_WINDOWS, 28.0).color(theme.color("#0078d4")))
                        .child(
                            el().key(format!("fleet-vm-badge-{index}"))
                                .absolute(18.0, 27.0)
                                .size(22.0, 13.0)
                                .center()
                                .radius(6.5)
                                .continuous_corners()
                                .fill(theme.color("#242424"))
                                .fill(theme.color("#191919"))
                                .border(1.0, theme.color("#2a2a2a"))
                                .child(
                                    text("VM")
                                        .font(ui_font(9.0, 650.0))
                                        .color(theme.color("#f2f2f2")),
                                ),
                        ),
                )
                .child(
                    crate::design::faded_lines(
                        item.machine.name.clone(),
                        ui_font(16.0, 600.0),
                        theme.color("#f2f2f2"),
                        2,
                        text_width - CARD_MARK_WIDTH,
                    )
                    .grow(1.0)
                    .min_w(0.0),
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
            true,
            theme,
        ))
        .child(footer)
}

pub(crate) fn render(state: &State, theme: ThemeStyle<'_>, page_width: f32) -> Vec<El> {
    let section = state.fleet_section;
    if section == FleetSection::ResearchTools {
        return render_research_tools(state, theme);
    }
    let mut tabs = crate::design::segmented_control(theme)
        .key("fleet-view-tabs")
        .label("Fleet views");
    for tab in FleetTab::ALL {
        tabs = tabs.child(tab_button(tab, section, state.fleet.tab == tab, theme));
    }
    let mut tabs = row()
        .w_full()
        .center_y()
        .gap(12.0)
        .child(tabs)
        .child(el().grow(1.0));
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
    let mut content = vec![tabs];
    if !state.fleet.error.is_empty() {
        content.push(
            text(state.fleet.error.clone())
                .key("fleet-error")
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
        .mt(6.0)
        .gaps(16.0, 16.0)
        .equal_columns(fleet_columns(page_width));
    let text_width =
        crate::design::grid_card_text_width(page_width, fleet_columns(page_width), 16.0, 12.0);
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
            cards = cards.child(server_card(&local, state, text_width, theme));
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
            cards = cards.child(server_card(server, state, text_width, theme));
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
            cards = cards.child(virtual_machine_card(index, item, state, text_width, theme));
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
            (FleetSection::VirtualMachines, _) if state.fleet.vm_loading => {
                "Loading virtual machines…"
            }
            (FleetSection::VirtualMachines, FleetTab::Available) => "No running virtual machines.",
            (FleetSection::VirtualMachines, FleetTab::Enabled) => {
                "No virtual machines are stopped or paused."
            }
            (FleetSection::VirtualMachines, _) => "No virtual machines detected.",
            (FleetSection::ResearchTools, _) => "No research tools available.",
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

fn research_tool_permission_button(
    key: String,
    label: &'static str,
    selected: bool,
    enabled: bool,
    action: ResearchToolMutation,
    theme: ThemeStyle<'_>,
) -> El {
    let mut button = el()
        .key(key)
        .role("checkbox")
        .label(format!(
            "{label} for {}: {}",
            action.id,
            if selected { "allowed" } else { "not allowed" }
        ))
        .size(104.0, 36.0)
        .center()
        .radius(8.0)
        .hover(theme.color("#303030"))
        .child(
            el().size(22.0, 22.0)
                .center()
                .radius(6.0)
                .continuous_corners()
                .fill(theme.color(if selected { "#3c513d" } else { "#242424" }))
                .border(
                    1.0,
                    theme.color(if selected { "#3c513d" } else { "#696969" }),
                )
                .child(if selected {
                    icon(&CHECK, 15.0).color(theme.color("#d8efd7"))
                } else {
                    el().size(0.0, 0.0)
                }),
        );
    if enabled {
        button = button
            .on_press(move |state: &mut State| start_research_tool_mutation(state, action.clone()));
    }
    button
}

fn render_research_tools(state: &State, theme: ThemeStyle<'_>) -> Vec<El> {
    let mut content = Vec::new();
    if !state.fleet.tools_error.is_empty() {
        content.push(
            text(state.fleet.tools_error.clone())
                .key("fleet-research-tools-error")
                .mt(10.0)
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#e07676")),
        );
    }
    let mut hosts = row()
        .key("fleet-research-tool-hosts")
        .w_full()
        .mt(14.0)
        .gaps(8.0, 8.0);
    for host in &state.fleet.tool_hosts {
        let id = host.id.clone();
        let active = id == state.fleet.selected_tool_host_id;
        hosts = hosts.child(
            el().key(format!("fleet-research-tool-host-{id}"))
                .role("button")
                .label(host.name.clone())
                .h(32.0)
                .px(12.0)
                .center()
                .radius(16.0)
                .fill(theme.color(if active { "#353535" } else { "#222222" }))
                .hover(theme.color("#393939"))
                .on_press(move |state: &mut State| state.fleet.selected_tool_host_id = id.clone())
                .child(
                    text(host.name.clone())
                        .font(ui_font(13.0, if active { 550.0 } else { 400.0 }))
                        .color(theme.color(if active { "#f2f2f2" } else { "#bdbdbd" })),
                ),
        );
    }
    content.push(hosts);
    let Some(host) = state
        .fleet
        .tool_hosts
        .iter()
        .find(|host| host.id == state.fleet.selected_tool_host_id)
    else {
        content.push(
            text(if state.fleet.tools_loading {
                "Loading research tools…"
            } else {
                "No app-server research tools are available."
            })
            .mt(18.0)
            .font(ui_font(14.0, 400.0))
            .color(theme.color("#969696")),
        );
        return content;
    };
    let divider = || el().w_full().h(1.0).fill(theme.color("#303030"));
    let header_font = ui_font(12.5, 500.0);
    let header_color = theme.color("#969696");
    let headers = row()
        .key("fleet-research-tool-headers")
        .w_full()
        .h(34.0)
        .px(12.0)
        .center_y()
        .gap(12.0)
        .child(text("Tool").grow(1.0).font(header_font).color(header_color))
        .child(
            row()
                .gap(8.0)
                .child(
                    el().w(104.0)
                        .center()
                        .child(text("Allow Locally").font(header_font).color(header_color)),
                )
                .child(
                    el().w(104.0)
                        .center()
                        .child(text("Allow in VMs").font(header_font).color(header_color)),
                ),
        );
    let mut tools = col()
        .key("fleet-research-tool-list")
        .w_full()
        .mt(16.0)
        .child(headers)
        .child(divider());
    for tool in &host.tools {
        let local_action = ResearchToolMutation {
            host_id: host.id.clone(),
            id: tool.id.clone(),
            allow_locally: !tool.allow_locally,
            allow_in_vms: tool.allow_in_vms,
        };
        let vm_action = ResearchToolMutation {
            host_id: host.id.clone(),
            id: tool.id.clone(),
            allow_locally: tool.allow_locally,
            allow_in_vms: !tool.allow_in_vms,
        };
        let buttons = row()
            .gap(8.0)
            .child(research_tool_permission_button(
                format!("fleet-tool-local-{}-{}", host.id, tool.id),
                "Allow Locally",
                tool.allow_locally,
                !state.fleet.tools_busy,
                local_action,
                theme,
            ))
            .child(research_tool_permission_button(
                format!("fleet-tool-vms-{}-{}", host.id, tool.id),
                "Allow in VMs",
                tool.allow_in_vms,
                !state.fleet.tools_busy,
                vm_action,
                theme,
            ));
        tools = tools
            .child(
                row()
                    .key(format!("fleet-tool-row-{}-{}", host.id, tool.id))
                    .w_full()
                    .h(54.0)
                    .px(12.0)
                    .center_y()
                    .gap(12.0)
                    .child(
                        text(tool.name.clone())
                            .grow(1.0)
                            .min_w(0.0)
                            .font(ui_font(14.0, 550.0))
                            .color(theme.color("#f2f2f2")),
                    )
                    .child(buttons),
            )
            .child(divider());
    }
    content.push(tools);
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
                    .fade_truncate()
                    .font(ui_font(14.0, 550.0))
                    .color(theme.color("#f2f2f2")),
            );
            details = details
                .child(
                    text(device.dns_name.clone())
                        .fade_truncate()
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
        let [cpu, memory, disk] = resource_values(server, true);
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
            .child(stat_row("CPU", cpu, theme))
            .child(stat_row("Memory", memory, theme))
            .child(stat_row("Disk", disk, theme))
            .child(stat_row(
                "Allowed Tools",
                if server.allowed_research_tools.is_empty() {
                    "None".to_owned()
                } else {
                    server.allowed_research_tools.len().to_string()
                },
                theme,
            ))
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
        if !server.allowed_research_tools.is_empty() {
            let allowed = server
                .allowed_research_tools
                .iter()
                .map(|tool| {
                    let where_allowed = match (tool.allow_locally, tool.allow_in_vms) {
                        (true, true) => "Local, VMs",
                        (true, false) => "Local",
                        (false, true) => "VMs",
                        (false, false) => "",
                    };
                    format!("{} ({where_allowed})", tool.name)
                })
                .collect::<Vec<_>>()
                .join(", ");
            body = body.child(
                text(allowed)
                    .font(ui_font(13.0, 400.0))
                    .color(theme.color("#bdbdbd"))
                    .wrap(),
            );
        }
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
    use beale_contracts::v1::FleetSystemResources;
    use rune::{Env, Runtime};

    #[test]
    fn research_tools_have_a_sidenav_item_and_separate_permissions() {
        let mut state = State {
            primary_view: PrimaryView::Fleet,
            fleet_section: FleetSection::ResearchTools,
            ..State::default()
        };
        state.fleet.tool_hosts.push(ResearchToolHost {
            id: "local".to_owned(),
            name: "Local App Server".to_owned(),
            tools: vec![beale_contracts::v1::FleetResearchTool {
                id: "ripgrep".to_owned(),
                name: "ripgrep".to_owned(),
                category: "Source".to_owned(),
                description: "Fast search across source and artifacts.".to_owned(),
                allow_locally: true,
                allow_in_vms: false,
            }],
        });
        let mut runtime = Runtime::new(
            state,
            |state, _| {
                let theme = state.appearance.style();
                col()
                    .child(sidenav(state, 220.0, theme))
                    .children(render(state, theme, 820.0))
            },
            (1100.0, 700.0),
            Env::new(true, 1.0),
        );
        runtime.frame(0.0);
        assert!(runtime.find("fleet-research-tools").is_some());
        assert!(runtime.find("fleet-research-tool-list").is_some());
        assert!(runtime.find("fleet-tool-row-local-ripgrep").is_some());
        assert!(runtime.find("fleet-tool-local-local-ripgrep").is_some());
        assert!(runtime.find("fleet-tool-vms-local-ripgrep").is_some());
        assert!(runtime.find("fleet-tool-install-local-ripgrep").is_none());
    }

    #[test]
    fn app_server_resource_values_show_capacity_and_availability() {
        let mut server = fleet_connection::local_placeholder();
        server.resources = Some(FleetSystemResources {
            cpu_logical_cores: 8,
            memory_total_bytes: 32 * 1024 * 1024 * 1024,
            memory_available_bytes: 12 * 1024 * 1024 * 1024,
            disk_total_bytes: 512 * 1024 * 1024 * 1024,
            disk_available_bytes: 256 * 1024 * 1024 * 1024,
        });
        assert_eq!(
            resource_values(&server, false),
            ["8 logical cores", "32.0 GiB", "512.0 GiB"]
        );
        assert_eq!(
            resource_values(&server, true),
            [
                "8 logical cores",
                "12.0 GiB available / 32.0 GiB total",
                "256.0 GiB available / 512.0 GiB total",
            ]
        );
        server.resources = None;
        assert_eq!(resource_values(&server, false), ["Unavailable"; 3]);
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
                    300.0,
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
        assert!(runtime.find("fleet-vm-badge-0").is_some());
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
