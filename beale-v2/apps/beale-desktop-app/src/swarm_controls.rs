//! Desktop controls consume host snapshots; they never open runtime storage.
use crate::{AUTH_HANDLE, State, ThemeStyle, fleet_connection, ui_font};
use beale_contracts::{
    pacing::{self, AccountBinding, AccountingReply, AccountingRequest, AccountingSnapshot},
    swarm_allocation::{
        self, Allocation, DeletionStep, DiscardPreview, HostAllocationReply, HostAllocationRequest,
        RetainedEnvironment, RetentionAction,
    },
    swarm_container::{self, ConfigurationReadiness, ImageConfiguration, LinuxArchitecture},
    swarm_ui::ScanOverview,
    v1::{SwarmDestination, SwarmPacingTarget},
};
use rune::{El, TextEdit, col, el, icon, row, text, text_input};
use serde::{Serialize, de::DeserializeOwned};

#[derive(Clone)]
pub(crate) struct DestinationChoice {
    pub host_name: String,
    pub readiness: ConfigurationReadiness,
}
#[derive(Clone)]
pub(crate) struct HostInventory {
    pub connection: String,
    pub name: String,
    pub allocations: Vec<Allocation>,
    pub environments: Vec<RetainedEnvironment>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct AccountContext {
    host: String,
    model: crate::new_research::ModelSelection,
}
fn account_context(state: &State) -> Option<AccountContext> {
    Some(AccountContext {
        host: state.swarm_controls.account_host.clone(),
        model: crate::new_research::selected_model(state)?,
    })
}
fn resolved_account(state: &State) -> Result<&AccountBinding, String> {
    let c = &state.swarm_controls;
    if c.account_context.is_none() || c.account_context != account_context(state) {
        return Err("Resolve the current host and provider account first.".into());
    }
    c.account
        .as_ref()
        .and_then(|a| a.binding.as_ref())
        .ok_or_else(|| "Resolve the provider account first.".into())
}
fn apply_account_resolution(
    state: &mut State,
    epoch: u64,
    context: AccountContext,
    result: Result<AccountingSnapshot, String>,
) {
    if state.swarm_controls.account_epoch != epoch {
        return;
    }
    state.swarm_controls.busy = false;
    if account_context(state).as_ref() != Some(&context) {
        return;
    }
    match result {
        Ok(snapshot) => {
            if let Some(b) = &snapshot.binding {
                state.swarm_controls.group = TextEdit::new(&b.group_id);
                state.swarm_controls.owner = TextEdit::new(&b.owner_host_id);
                state.swarm_controls.participants = TextEdit::new(b.participants.join(", "));
            }
            state.swarm_controls.account = Some(snapshot);
            state.swarm_controls.account_context = Some(context);
            state.swarm_controls.error.clear();
        }
        Err(error) => state.swarm_controls.error = error,
    }
}
pub(crate) struct Controls {
    pub destinations: Vec<SwarmDestination>,
    pub pacing: Vec<SwarmPacingTarget>,
    pub choices: Vec<DestinationChoice>,
    pub inventories: Vec<HostInventory>,
    pub storage: Vec<(String, beale_contracts::v1::FleetSystemResources)>,
    pub hosts: Vec<(String, String)>,
    pub selector_open: bool,
    pub add_open: bool,
    pub image: TextEdit,
    pub container_host: String,
    pub arm64: bool,
    pub loading: bool,
    refresh_pending: bool,
    inventory_epoch: u64,
    inventory_mutating: bool,
    pub busy: bool,
    pub error: String,
    refresh_error: String,
    account_context: Option<AccountContext>,
    account_epoch: u64,
    pub notice: String,
    pub account_open: bool,
    pub account_host: String,
    pub account: Option<AccountingSnapshot>,
    pub group: TextEdit,
    pub owner: TextEdit,
    pub participants: TextEdit,
    pub pace_amount: TextEdit,
    pub pace_interval: TextEdit,
    pub denomination: TextEdit,
    pub pace_kind: usize,
    pub discard: Option<(String, DiscardPreview)>,
}
impl Default for Controls {
    fn default() -> Self {
        Self {
            destinations: vec![],
            pacing: vec![],
            choices: vec![],
            inventories: vec![],
            storage: vec![],
            hosts: vec![("local".into(), "Local app server".into())],
            selector_open: false,
            add_open: false,
            image: TextEdit::new(""),
            container_host: "local".into(),
            arm64: false,
            loading: false,
            refresh_pending: false,
            inventory_epoch: 0,
            inventory_mutating: false,
            busy: false,
            error: String::new(),
            refresh_error: String::new(),
            account_context: None,
            account_epoch: 0,
            notice: String::new(),
            account_open: false,
            account_host: "local".into(),
            account: None,
            group: TextEdit::new(""),
            owner: TextEdit::new(""),
            participants: TextEdit::new(""),
            pace_amount: TextEdit::new(""),
            pace_interval: TextEdit::new("60"),
            denomination: TextEdit::new("USD"),
            pace_kind: 0,
            discard: None,
        }
    }
}

pub(crate) async fn request<T: DeserializeOwned>(
    host: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<T, String> {
    let (client, base, token) = fleet_connection::fleet_client(host).await?;
    let req = match body {
        Some(body) => client.post(format!("{base}{path}")).json(&body),
        None => client.get(format!("{base}{path}")),
    };
    let response = req
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| "App-server connection unavailable.".to_owned())?;
    if !response.status().is_success() {
        return Err(format!(
            "The host rejected this operation ({}). Refresh its current state before retrying.",
            response.status().as_u16()
        ));
    }
    serde_json::from_slice(&fleet_connection::read_bounded(response).await?).map_err(|_| {
        "Unsupported app-server response; rebuild participating hosts together.".into()
    })
}
fn body(value: &impl Serialize) -> Result<serde_json::Value, String> {
    serde_json::to_value(value).map_err(|_| "Could not encode host request.".into())
}

struct InventorySnapshot {
    hosts: Vec<(String, String)>,
    choices: Vec<DestinationChoice>,
    inventories: Vec<HostInventory>,
    storage: Vec<(String, beale_contracts::v1::FleetSystemResources)>,
    error: String,
}
async fn fetch_inventory() -> Result<InventorySnapshot, String> {
    let fleet = fleet_connection::load().await?;
    let storage = std::iter::once(&fleet.local)
        .chain(fleet.servers.iter().filter(|s| s.enabled))
        .filter_map(|s| {
            s.resources.clone().map(|r| {
                (
                    if s.display_name.is_empty() {
                        s.host_name.clone()
                    } else {
                        s.display_name.clone()
                    },
                    r,
                )
            })
        })
        .collect::<Vec<_>>();
    let mut hosts = vec![("local".to_owned(), "Local app server".to_owned())];
    hosts.extend(fleet.servers.into_iter().filter(|s| s.enabled).map(|s| {
        (
            s.id,
            if s.display_name.is_empty() {
                s.host_name
            } else {
                s.display_name
            },
        )
    }));
    let mut choices = Vec::new();
    let mut inventories = Vec::new();
    let mut errors = Vec::new();
    for (connection, name) in &hosts {
        match request::<Vec<ConfigurationReadiness>>(
            connection,
            swarm_container::CONFIGURATIONS_PATH,
            None,
        )
        .await
        {
            Ok(configs) => choices.extend(configs.into_iter().map(|readiness| DestinationChoice {
                host_name: name.clone(),
                readiness,
            })),
            Err(e) => errors.push(format!("{name}: {e}")),
        }
        match request::<HostAllocationReply>(
            connection,
            swarm_allocation::PATH,
            Some(body(&HostAllocationRequest::Inventory)?),
        )
        .await
        {
            Ok(HostAllocationReply::Inventory {
                allocations,
                environments,
            }) => inventories.push(HostInventory {
                connection: connection.clone(),
                name: name.clone(),
                allocations,
                environments,
            }),
            Ok(_) => errors.push(format!("{name}: unsupported allocation inventory")),
            Err(e) => errors.push(format!("{name}: {e}")),
        }
    }
    Ok(InventorySnapshot {
        hosts,
        choices,
        inventories,
        storage,
        error: errors.join("\n"),
    })
}
fn apply_inventory(state: &mut State, epoch: u64, result: Result<InventorySnapshot, String>) {
    if epoch != state.swarm_controls.inventory_epoch {
        return;
    }
    match result {
        Ok(snapshot) => {
            state.swarm_controls.hosts = snapshot.hosts;
            state.swarm_controls.choices = snapshot.choices;
            state.swarm_controls.inventories = snapshot.inventories;
            state.swarm_controls.storage = snapshot.storage;
            state.swarm_controls.refresh_error = snapshot.error;
        }
        Err(error) => state.swarm_controls.refresh_error = error,
    }
}
pub(crate) fn refresh(state: &mut State) {
    if state.swarm_controls.loading || state.swarm_controls.inventory_mutating {
        state.swarm_controls.refresh_pending = true;
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        return;
    };
    state.swarm_controls.loading = true;
    state.swarm_controls.inventory_epoch = state.swarm_controls.inventory_epoch.wrapping_add(1);
    let epoch = state.swarm_controls.inventory_epoch;
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "Could not start Fleet connection.".to_owned())
            .and_then(|rt| rt.block_on(fetch_inventory()));
        handle.update(move |state, _| {
            state.swarm_controls.loading = false;
            apply_inventory(state, epoch, result);
            if std::mem::take(&mut state.swarm_controls.refresh_pending) {
                refresh(state);
            }
        });
    });
}

/// A mode change is an explicit user action. Feature refresh never calls this.
pub(crate) fn select_mode(state: &mut State, mode: String) {
    if mode.is_empty()
        && (!state.swarm_controls.destinations.is_empty()
            || !state.swarm_controls.pacing.is_empty())
    {
        state.swarm_controls.destinations.clear();
        state.swarm_controls.pacing.clear();
        state.swarm_controls.notice =
            "Destinations and pacing cleared. This new scan will run locally with No Subagents."
                .into();
    }
    state.new_research_swarm_id = mode;
}
pub(crate) fn validate_draft(
    mode: &str,
    enabled: bool,
    destinations: &[SwarmDestination],
    pacing: &[SwarmPacingTarget],
) -> Result<(), String> {
    if mode.is_empty() && (!destinations.is_empty() || !pacing.is_empty()) {
        return Err("No Subagents cannot have destinations or Swarm pacing. Clear these selections before Start.".into());
    }
    if !mode.is_empty() && !enabled {
        return Err("Swarms is disabled for new scans. Enable it or intentionally choose No Subagents to run locally.".into());
    }
    let mut seen = std::collections::HashSet::new();
    if destinations.len() > 128
        || destinations.iter().any(|d| {
            !beale_contracts::swarms::valid_id(&d.host_id)
                || !beale_contracts::swarms::valid_id(&d.configuration_id)
                || !seen.insert((&d.host_id, &d.configuration_id))
        })
    {
        return Err(
            "Invalid or duplicate saved destination. Correct the draft before Start.".into(),
        );
    }
    if pacing.len() > 64 {
        return Err("Too many saved pacing targets.".into());
    }
    let mut groups = std::collections::HashSet::new();
    for p in pacing {
        if !groups.insert(&p.budget_group_id)
            || !beale_contracts::swarms::valid_id(&p.budget_group_id)
            || !(1..=604800).contains(&p.interval_seconds)
            || p.amount == 0
            || p.amount > 1_000_000_000_000
            || !match p.unit.as_str() {
                "subscription" => {
                    p.amount <= 10000 && beale_contracts::swarms::valid_id(&p.denomination)
                }
                "currency" => {
                    p.denomination.len() == 3
                        && p.denomination.bytes().all(|b| b.is_ascii_uppercase())
                }
                "credits" => p.denomination.is_empty(),
                _ => false,
            }
        {
            return Err("Invalid saved pacing target. Correct the amount, unit, account, and interval before Start.".into());
        }
    }
    Ok(())
}
fn label(value: impl Into<String>, theme: ThemeStyle<'_>) -> El {
    text(value.into())
        .font(ui_font(13.0, 400.0))
        .color(theme.color("#bdbdbd"))
        .wrap()
}
fn button(key: impl Into<String>, title: impl Into<String>, theme: ThemeStyle<'_>) -> El {
    let title = title.into();
    el().key(key.into())
        .role("button")
        .focusable(true)
        .label(title.clone())
        .min_h(32.0)
        .min_w(0.0)
        .px(10.0)
        .py(6.0)
        .center()
        .radius(8.0)
        .fill(theme.color("#292929"))
        .hover(theme.color("#414141"))
        .child(
            text(title)
                .min_w(0.0)
                .wrap()
                .font(ui_font(13.0, 500.0))
                .color(theme.color("#eeeeee")),
        )
}
fn input(
    key: &'static str,
    title: &'static str,
    edit: &TextEdit,
    accessor: fn(&mut State) -> &mut TextEdit,
    theme: ThemeStyle<'_>,
) -> El {
    col().w_full().gap(5.0).child(label(title, theme)).child(
        text_input(edit, ui_font(14.0, 400.0))
            .key(key)
            .label(title)
            .single_line()
            .w_full()
            .h(34.0)
            .padding(8.0)
            .radius(8.0)
            .fill(theme.color("#242424"))
            .color(theme.color("#eeeeee"))
            .on_edit(accessor),
    )
}
pub(crate) fn new_scan(state: &State, theme: ThemeStyle<'_>) -> El {
    let c = &state.swarm_controls;
    let enabled =
        crate::extensions_view::swarms_enabled(state) && !state.new_research_swarm_id.is_empty();
    let mut plus = crate::form_controls::icon_action_button(
        "scan-add-container",
        "Add Container",
        &crate::icons::PLUS,
        theme,
    );
    let mut selector = button(
        "scan-destinations",
        if enabled {
            if c.destinations.is_empty() {
                "Local execution".into()
            } else {
                format!("{} destinations selected", c.destinations.len())
            }
        } else {
            "Local-only execution".into()
        },
        theme,
    );
    if enabled {
        plus = plus.focusable(true).on_press(|s: &mut State| {
            s.swarm_controls.add_open = !s.swarm_controls.add_open;
            refresh(s);
        });
        selector = selector.on_press(|s: &mut State| {
            s.swarm_controls.selector_open = !s.swarm_controls.selector_open;
            if s.swarm_controls.selector_open {
                refresh(s);
            }
        });
    } else {
        plus = plus.focusable(false).opacity(0.4);
        selector = selector.focusable(false).opacity(0.5);
    }
    let mut view=col().key("scan-execution-controls").w_full().gap(10.0)
        .child(label("VMs / Containers",theme))
        .child(label("Choose isolated destinations for subagents. Empty selections run locally; selected destinations never fall back locally. Remote hosts require a reachable leader origin configured by the host operator.",theme))
        .child(row().gap(6.0).child(plus).child(selector));
    if !crate::extensions_view::swarms_enabled(state) && !state.new_research_swarm_id.is_empty() {
        view = view.child(
            button("scan-local-mode", "Choose No Subagents", theme)
                .on_press(|s: &mut State| select_mode(s, String::new())),
        );
    }
    if c.selector_open && enabled {
        let mut list = col().key("scan-destination-menu").gap(6.0);
        for (index, choice) in c.choices.iter().enumerate() {
            let dest = SwarmDestination {
                host_id: choice.readiness.host_id.clone(),
                configuration_id: choice.readiness.configuration.id.clone(),
            };
            let selected = c.destinations.contains(&dest);
            list = list.child(
                button(
                    format!("scan-destination-{index}"),
                    format!(
                        "{} {} — {}",
                        if selected { "Selected:" } else { "Select:" },
                        choice.readiness.configuration.image,
                        choice.host_name
                    ),
                    theme,
                )
                .on_press(move |s: &mut State| {
                    if let Some(i) = s
                        .swarm_controls
                        .destinations
                        .iter()
                        .position(|d| d == &dest)
                    {
                        s.swarm_controls.destinations.remove(i);
                    } else {
                        s.swarm_controls.destinations.push(dest.clone());
                    }
                }),
            );
        }
        for (i, d) in c.destinations.iter().enumerate().filter(|(_, d)| {
            !c.choices.iter().any(|x| {
                x.readiness.host_id == d.host_id
                    && x.readiness.configuration.id == d.configuration_id
            })
        }) {
            let d = d.clone();
            list = list.child(
                button(
                    format!("unavailable-destination-{i}"),
                    format!(
                        "Unavailable saved destination {} — remove",
                        d.configuration_id
                    ),
                    theme,
                )
                .on_press(move |s: &mut State| s.swarm_controls.destinations.retain(|x| x != &d)),
            );
        }
        if c.choices.is_empty() {
            list=list.child(label(if c.loading{"Loading destinations…"}else{"No saved container configurations. Add an image to begin. VM execution will be supported later."},theme));
        }
        list = list.child(
            button("scan-clear-destinations", "Clear destinations", theme).on_press(
                |s: &mut State| {
                    s.swarm_controls.destinations.clear();
                },
            ),
        );
        view = view.child(list);
    }
    if enabled {
        view = view.child(
            button("scan-account-pacing", "Account & pacing", theme).on_press(|s: &mut State| {
                s.swarm_controls.account_open = !s.swarm_controls.account_open;
                refresh(s);
            }),
        );
    }
    if c.account_open && enabled {
        view = view.child(account_form(state, theme));
    }
    for (i, p) in c.pacing.iter().enumerate() {
        view = view.child(
            row()
                .gap(8.0)
                .child(label(
                    format!(
                        "{}: {} {} {} / {} seconds",
                        p.budget_group_id,
                        display_amount(p),
                        if p.unit == "subscription" {
                            "percentage points"
                        } else {
                            &p.unit
                        },
                        p.denomination,
                        p.interval_seconds
                    ),
                    theme,
                ))
                .child(
                    button(format!("remove-pace-{i}"), "Remove", theme).on_press(
                        move |s: &mut State| {
                            s.swarm_controls.pacing.remove(i);
                        },
                    ),
                ),
        );
    }
    if c.add_open && enabled {
        view = view.child(container_form(state, theme));
    }
    if !c.notice.is_empty() {
        view = view.child(label(&c.notice, theme));
    }
    if !c.error.is_empty() {
        view = view.child(label(&c.error, theme));
    }
    if !c.refresh_error.is_empty() {
        view = view.child(label(&c.refresh_error, theme));
    }
    if let Err(e) = validate_draft(
        &state.new_research_swarm_id,
        crate::extensions_view::swarms_enabled(state),
        &c.destinations,
        &c.pacing,
    ) {
        view = view.child(label(e, theme));
    }
    view
}

fn save_container(state: &mut State) {
    if state.swarm_controls.busy {
        return;
    }
    let c = &state.swarm_controls;
    let config = ImageConfiguration {
        version: 1,
        id: uuid::Uuid::new_v4().to_string(),
        revision: 1,
        image: c.image.text.trim().to_owned(),
        architecture: if c.arm64 {
            LinuxArchitecture::Arm64
        } else {
            LinuxArchitecture::Amd64
        },
        options: Default::default(),
    };
    if let Err(e) = config.validate() {
        state.swarm_controls.error = e.into();
        return;
    }
    let host = c.container_host.clone();
    run(state, move || async move {
        let _: ConfigurationReadiness = request(
            &host,
            swarm_container::CONFIGURATIONS_PATH,
            Some(body(&config)?),
        )
        .await?;
        Ok("Container configuration saved. Instances are allocated as research requires.".into())
    });
}
pub(crate) fn container_form(state: &State, theme: ThemeStyle<'_>) -> El {
    let c = &state.swarm_controls;
    let hosts = row()
        .gaps(6.0, 6.0)
        .children(c.hosts.iter().map(|(id, name)| {
            let id = id.clone();
            button(
                format!("container-host-{id}"),
                format!(
                    "{}{}",
                    if c.container_host == id {
                        "Selected: "
                    } else {
                        ""
                    },
                    name
                ),
                theme,
            )
            .on_press(move |s: &mut State| s.swarm_controls.container_host = id.clone())
        }));
    col().key("add-container-form").w_full().gap(10.0).child(row().gap(8.0).child(icon(&crate::icons::DOCKER,22.0).color(theme.color("#eeeeee"))).child(label("Add Container",theme))).child(hosts)
        .child(input("container-image","Docker image",&c.image,|s|&mut s.swarm_controls.image,theme))
        .child(label("Enter an image name, such as ubuntu:24.04 or alpine:3.21. Some images may be incompatible. More instances may be allocated as research requires, with one subagent per instance.",theme))
        .child(button("container-architecture",if c.arm64{"Linux Arm64"}else{"Linux AMD64"},theme).on_press(|s:&mut State|s.swarm_controls.arm64 = !s.swarm_controls.arm64))
        .child(if c.busy{label("Saving…",theme)}else{button("container-save","Save container",theme).on_press(save_container)})
}
/// Reload even after a rejected action or a lost acknowledgement. The original
/// action result is retained independently of inventory/transport health.
async fn mutate_and_reload<T>(
    operation: impl std::future::Future<Output = Result<String, String>>,
    reload: impl FnOnce() -> T,
) -> (Result<String, String>, Result<InventorySnapshot, String>)
where
    T: std::future::Future<Output = Result<InventorySnapshot, String>>,
{
    let result = operation.await;
    (result, reload().await)
}
fn finish_mutation(
    state: &mut State,
    epoch: u64,
    result: Result<String, String>,
    inventory: Result<InventorySnapshot, String>,
) {
    state.swarm_controls.busy = false;
    state.swarm_controls.inventory_mutating = false;
    state.swarm_controls.notice.clear();
    match result {
        Ok(message) => {
            state.swarm_controls.error.clear();
            state.swarm_controls.notice = message;
        }
        Err(error) => state.swarm_controls.error = error,
    }
    apply_inventory(state, epoch, inventory);
    if std::mem::take(&mut state.swarm_controls.refresh_pending) {
        refresh(state);
    }
}
fn run<F, Fut>(state: &mut State, operation: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    if state.swarm_controls.busy {
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        return;
    };
    state.swarm_controls.busy = true;
    state.swarm_controls.inventory_mutating = true;
    state.swarm_controls.inventory_epoch = state.swarm_controls.inventory_epoch.wrapping_add(1);
    let epoch = state.swarm_controls.inventory_epoch;
    std::thread::spawn(move || {
        let (result, inventory) = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt.block_on(mutate_and_reload(operation(), fetch_inventory)),
            Err(_) => (
                Err("Could not start host connection.".into()),
                Err("Current environment state unavailable.".into()),
            ),
        };
        handle.update(move |s, _| finish_mutation(s, epoch, result, inventory));
    });
}

fn resolve_account(state: &mut State) {
    let Some(context) = account_context(state) else {
        state.swarm_controls.error = "Choose a provider/model first.".into();
        return;
    };
    let host = context.host.clone();
    let Some(model) = crate::new_research::selected_model(state) else {
        state.swarm_controls.error = "Choose a provider/model first.".into();
        return;
    };
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        return;
    };
    if state.swarm_controls.busy {
        return;
    }
    state.swarm_controls.busy = true;
    state.swarm_controls.account_epoch = state.swarm_controls.account_epoch.wrapping_add(1);
    let epoch = state.swarm_controls.account_epoch;
    state.swarm_controls.account = None;
    state.swarm_controls.account_context = None;
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "Could not start account connection.".to_owned())
            .and_then(|rt| {
                rt.block_on(async {
                    let reply: AccountingReply = request(
                        &host,
                        pacing::PATH,
                        Some(body(&AccountingRequest::Resolve {
                            provider: model.provider.id() as i32,
                            subscription: model.method
                                == beale_contracts::v1::AuthMethod::Subscription,
                            model: model.id,
                        })?),
                    )
                    .await?;
                    let AccountingReply::Binding(binding) = reply else {
                        return Err("Invalid account binding.".into());
                    };
                    let reply: AccountingReply = request(
                        &host,
                        pacing::PATH,
                        Some(body(&AccountingRequest::Status {
                            group_id: binding.group_id,
                        })?),
                    )
                    .await?;
                    match reply {
                        AccountingReply::Status(s) => Ok(*s),
                        _ => Err("Invalid account status.".into()),
                    }
                })
            });
        handle.update(move |s, _| apply_account_resolution(s, epoch, context, result));
    });
}
fn account_form(state: &State, theme: ThemeStyle<'_>) -> El {
    let c = &state.swarm_controls;
    let mut view=col().key("account-pacing-form").w_full().gap(8.0)
        .child(label("Provider/account pacing is best effort, not a spending cap. Resolve the selected model's account before setting a target.",theme))
        .child(button("account-resolve","Resolve / refresh account",theme).on_press(resolve_account));
    if let Some(snapshot) = &c.account
        && let Ok(b) = resolved_account(state)
    {
        view = view.child(label(
            format!(
                "Provider {} · account group {} · authority {} · generation {} · {} · {:?}",
                b.provider_id,
                b.group_id,
                b.owner_host_id,
                b.generation,
                if b.limited_identity {
                    "Limited local identity"
                } else {
                    "Explicit / stable identity"
                },
                snapshot.coordination_mode
            ),
            theme,
        ));
    }
    view = view.child(
        row()
            .gaps(6.0, 6.0)
            .children(c.hosts.iter().map(|(id, name)| {
                let id = id.clone();
                button(
                    format!("account-host-{id}"),
                    format!(
                        "{}{}",
                        if c.account_host == id {
                            "Selected: "
                        } else {
                            ""
                        },
                        name
                    ),
                    theme,
                )
                .on_press(move |s: &mut State| {
                    s.swarm_controls.account_host = id.clone();
                    s.swarm_controls.account = None;
                })
            })),
    );
    view=view.child(input("account-group","Opaque account-budget group",&c.group,|s|&mut s.swarm_controls.group,theme))
        .child(input("account-owner","Accounting authority host ID",&c.owner,|s|&mut s.swarm_controls.owner,theme))
        .child(input("account-participants","Participating leader host IDs (comma separated)",&c.participants,|s|&mut s.swarm_controls.participants,theme))
        .child(label("Use a new opaque group to replace automatic grouping, and the same group and authority on each participating host. Changing an established authority requires drained handover; it cannot be overwritten here.",theme))
        .child(button("account-save","Save explicit grouping",theme).on_press(save_binding))
        .child(button("pace-kind",["API currency","Provider credits","Subscription allowance"][c.pace_kind],theme).on_press(|s:&mut State|{s.swarm_controls.pace_kind=(s.swarm_controls.pace_kind+1)%3;s.swarm_controls.denomination=TextEdit::new(match s.swarm_controls.pace_kind{0=>"USD",1=>"",_=>"primary"});}))
        .child(input("pace-amount",if c.pace_kind==2{"Percentage points per interval"}else{"Currency / credit units per interval"},&c.pace_amount,|s|&mut s.swarm_controls.pace_amount,theme))
        .child(input("pace-denomination",if c.pace_kind==2{"Named allowance window"}else{"Currency code (empty for credits)"},&c.denomination,|s|&mut s.swarm_controls.denomination,theme))
        .child(input("pace-interval","Interval in seconds",&c.pace_interval,|s|&mut s.swarm_controls.pace_interval,theme))
        .child(button("pace-add","Set account target",theme).on_press(add_pacing));
    view
}
fn save_binding(state: &mut State) {
    let host = state.swarm_controls.account_host.clone();
    let Some(model) = crate::new_research::selected_model(state) else {
        return;
    };
    let c = &state.swarm_controls;
    let old = match resolved_account(state) {
        Ok(b) => b,
        Err(e) => {
            state.swarm_controls.error = e;
            return;
        }
    };
    let b = AccountBinding {
        group_id: c.group.text.trim().into(),
        provider_id: old.provider_id.clone(),
        owner_host_id: c.owner.text.trim().into(),
        generation: old.generation,
        participants: c
            .participants
            .text
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect(),
        limited_identity: false,
    };
    let custom = if model.provider == crate::Provider::Custom {
        model.id.split_once("::").map_or("", |(id, _)| id)
    } else {
        ""
    };
    let route = format!(
        "provider-{}:{}:{custom}",
        model.provider.id() as i32,
        if model.method == beale_contracts::v1::AuthMethod::Subscription {
            "subscription"
        } else {
            "api"
        }
    );
    run(state, move || async move {
        fleet_connection::sync_account_authority(&host, &b.owner_host_id).await?;
        let reply: AccountingReply = request(
            &host,
            pacing::PATH,
            Some(body(&AccountingRequest::Bind { route, binding: b })?),
        )
        .await?;
        if !matches!(reply, AccountingReply::Saved) {
            return Err("The host did not accept the account grouping.".into());
        }
        Ok(
            "Account grouping saved. Resolve / refresh account to inspect its current state."
                .into(),
        )
    });
}
fn display_amount(target: &SwarmPacingTarget) -> String {
    let (scale, digits) = if target.unit == "subscription" {
        (100, 2)
    } else {
        (1_000_000, 6)
    };
    let amount = format!(
        "{}.{:0width$}",
        target.amount / scale,
        target.amount % scale,
        width = digits
    );
    amount
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
}
fn decimal_units(value: &str, scale: u64) -> Option<u64> {
    let (whole, fraction) = value.trim().split_once('.').unwrap_or((value.trim(), ""));
    let digits = scale.ilog10() as usize;
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() > digits
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let fraction = if fraction.is_empty() {
        0
    } else {
        fraction
            .parse::<u64>()
            .ok()?
            .checked_mul(10u64.pow((digits - fraction.len()) as u32))?
    };
    whole
        .parse::<u64>()
        .ok()?
        .checked_mul(scale)?
        .checked_add(fraction)
}
fn add_pacing(state: &mut State) {
    let binding = match resolved_account(state) {
        Ok(b) => b,
        Err(e) => {
            state.swarm_controls.error = e;
            return;
        }
    };
    if state.swarm_controls.group.text.trim() != binding.group_id {
        state.swarm_controls.error =
            "Save and resolve this account group before setting its target.".into();
        return;
    }
    let c = &state.swarm_controls;
    let Some(amount) = decimal_units(
        &c.pace_amount.text,
        if c.pace_kind == 2 { 100 } else { 1_000_000 },
    ) else {
        state.swarm_controls.error =
            "Enter a positive amount with at most two subscription or six currency decimal places."
                .into();
        return;
    };
    let target = SwarmPacingTarget {
        budget_group_id: c.group.text.trim().into(),
        interval_seconds: c.pace_interval.text.parse().unwrap_or(0),
        unit: ["currency", "credits", "subscription"][c.pace_kind].into(),
        denomination: c.denomination.text.trim().into(),
        amount,
    };
    if let Err(e) = validate_draft(
        &state.new_research_swarm_id,
        crate::extensions_view::swarms_enabled(state),
        &c.destinations,
        std::slice::from_ref(&target),
    ) {
        state.swarm_controls.error = e;
        return;
    }
    state
        .swarm_controls
        .pacing
        .retain(|p| p.budget_group_id != target.budget_group_id);
    state.swarm_controls.pacing.push(target);
    state.swarm_controls.error.clear();
}

pub(crate) fn session(detail: &beale_contracts::v1::SessionDetail, theme: ThemeStyle<'_>) -> El {
    let mut view = col().key("session-swarm-overview").w_full().gap(6.0);
    for warning in &detail.pacing_warnings {
        view = view.child(label(warning, theme));
    }
    if detail.swarm_id.is_empty() {
        return view;
    }
    let Ok(snapshot) = serde_json::from_str::<ScanOverview>(&detail.swarm_overview_json) else {
        return view.child(label("Swarm status unavailable. Refresh from a compatible app server; this scan retains its saved execution configuration.",theme));
    };
    view=view.child(label(format!("{} · definition revision {} · {}",snapshot.definition_id,snapshot.definition_revision,if snapshot.destinations.is_empty(){"Local execution".into()}else{format!("{} saved destinations",snapshot.destinations.len())}),theme))
        .child(label(format!("Leader-only review: {}\nGlobal toggles apply to new scans and cannot disable this saved rule. Edit a new definition revision for future scans. Use End scan to stop this hierarchy.",snapshot.leader_rule),theme));
    for agent in &snapshot.agents {
        let uncertain = agent.termination_unconfirmed || agent.unknown_commands > 0;
        let saved_state =
            if uncertain && agent.summary.state == beale_contracts::swarms::AgentState::Completed {
                "Completion awaiting reconciliation".to_owned()
            } else {
                format!("{:?}", agent.summary.state)
            };
        let status = if agent.termination_unconfirmed {
            "Termination unconfirmed".into()
        } else if agent.unknown_commands > 0 {
            format!("{} command outcomes unknown", agent.unknown_commands)
        } else if agent.summary.state == beale_contracts::swarms::AgentState::Queued {
            "Queued assignment — instance not started".into()
        } else {
            "Execution state confirmed".into()
        };
        if agent.unknown_commands > 0 && agent.termination_unconfirmed {
            view = view.child(label(
                format!(
                    "{} command outcomes unknown; reconcile before repeating effects.",
                    agent.unknown_commands
                ),
                theme,
            ));
        }
        view = view.child(label(
            format!(
                "{:?} {} · parent {} · {saved_state} · {status} · {} pending · {} queued wakes{}",
                agent.summary.role,
                agent.summary.profile.id,
                agent
                    .summary
                    .parent_id
                    .as_deref()
                    .and_then(|id| snapshot.agents.iter().find(|a| a.summary.id == id))
                    .map_or("none", |a| a.summary.profile.id.as_str()),
                agent.pending_assignments,
                agent.queued_wakes,
                if agent.resumable && !agent.termination_unconfirmed && agent.unknown_commands == 0
                {
                    " · resumable with relevant context"
                } else {
                    ""
                }
            ),
            theme,
        ));
    }
    view
}

pub(crate) fn fleet(state: &State, theme: ThemeStyle<'_>) -> Vec<El> {
    let c = &state.swarm_controls;
    let mut views =
        vec![button("fleet-container-refresh", "Refresh environments", theme).on_press(refresh)];
    if c.add_open {
        views.push(container_form(state, theme));
    }
    if !c.error.is_empty() {
        views.push(label(&c.error, theme));
    }
    if !c.refresh_error.is_empty() {
        views.push(label(&c.refresh_error, theme));
    }
    if !c.notice.is_empty() {
        views.push(label(&c.notice, theme));
    }
    if state.fleet.tab == crate::fleet_view::FleetTab::Available {
        return views;
    }
    for (name, resources) in &c.storage {
        views.push(label(
            format!(
                "{name} · host storage: {} GiB used / {} GiB total ({} GiB available)",
                (resources
                    .disk_total_bytes
                    .saturating_sub(resources.disk_available_bytes))
                    / (1024 * 1024 * 1024),
                resources.disk_total_bytes / (1024 * 1024 * 1024),
                resources.disk_available_bytes / (1024 * 1024 * 1024)
            ),
            theme,
        ));
    }
    for (i, choice) in c.choices.iter().enumerate() {
        views.push(
            col()
                .key(format!("fleet-container-config-{i}"))
                .w_full()
                .gap(5.0)
                .child(
                    row()
                        .gap(8.0)
                        .child(icon(&crate::icons::PACKAGE, 18.0).color(theme.color("#eeeeee")))
                        .child(label(
                            format!(
                                "Saved configuration · {} · {}",
                                choice.readiness.configuration.image, choice.host_name
                            ),
                            theme,
                        )),
                )
                .child(label(
                    format!(
                        "{:?}{}",
                        choice.readiness.state,
                        choice
                            .readiness
                            .reason
                            .as_ref()
                            .map_or(String::new(), |r| format!(": {r}"))
                    ),
                    theme,
                )),
        );
    }
    for inventory in &c.inventories {
        for a in inventory.allocations.iter().filter(|a| {
            state.fleet.tab != crate::fleet_view::FleetTab::Enabled || a.request.domain == "docker"
        }) {
            views.push(label(
                format!(
                    "{} instance {} · {} · {:?} · reserved storage {} MiB{}",
                    if a.request.domain == "native" {
                        "Native local (no OS isolation)"
                    } else {
                        "Container"
                    },
                    a.request.instance,
                    inventory.name,
                    a.phase,
                    a.request.storage.iter().map(|s| s.bytes).sum::<u64>() / (1024 * 1024),
                    a.reason
                        .as_ref()
                        .map_or(String::new(), |r| format!(" · {r}"))
                ),
                theme,
            ));
        }
        for e in &inventory.environments {
            let native = inventory
                .allocations
                .iter()
                .any(|a| a.request.instance == e.instance && a.request.domain == "native");
            if native && state.fleet.tab == crate::fleet_view::FleetTab::Enabled {
                continue;
            }
            let deleting = e.deletion != DeletionStep::Retained;
            let mut card=col().key(format!("environment-{}",e.instance)).w_full().gap(6.0).child(label(format!("{} environment {} · {} · {} · deadline {} · {} protected references · {} pending outputs",if native{"Native local (no OS isolation)"}else{"Retained container"},e.instance,inventory.name,if e.deletion==DeletionStep::Deleted{"Deleted"}else if deleting{"Deletion in progress"}else if e.keep{"Keep"}else{"Retained"},e.deadline_ms.map_or("while scan is active".into(),|ms|chrono::DateTime::from_timestamp_millis(ms).map_or("unknown".into(),|d|d.to_rfc3339())),e.references.len(),e.pending_outputs.len()),theme));
            if !deleting {
                let (host, instance, generation, keep) = (
                    inventory.connection.clone(),
                    e.instance.clone(),
                    e.generation,
                    !e.keep,
                );
                card = card.child(
                    button(
                        format!("environment-keep-{}", e.instance),
                        if keep { "Keep" } else { "Release Keep" },
                        theme,
                    )
                    .on_press(move |s: &mut State| {
                        retention(
                            s,
                            host.clone(),
                            instance.clone(),
                            generation,
                            RetentionAction::Keep { value: keep },
                        )
                    }),
                );
                let blocked = e.keep
                    || !e.references.is_empty()
                    || !e.termination_confirmed
                    || !e.checkpoint_confirmed
                    || !e.pending_outputs.is_empty();
                if blocked {
                    card = card.child(label(
                        format!(
                            "Delete now blocked: {}{}{}{}{}",
                            if e.keep { "Keep enabled. " } else { "" },
                            if !e.references.is_empty() {
                                "Active/protected references. "
                            } else {
                                ""
                            },
                            if !e.termination_confirmed {
                                "Termination unconfirmed. "
                            } else {
                                ""
                            },
                            if !e.checkpoint_confirmed {
                                "Checkpoint unconfirmed. "
                            } else {
                                ""
                            },
                            if !e.pending_outputs.is_empty() {
                                "Unpublished pending outputs. "
                            } else {
                                ""
                            }
                        ),
                        theme,
                    ));
                } else {
                    let (host, instance, generation) = (
                        inventory.connection.clone(),
                        e.instance.clone(),
                        e.generation,
                    );
                    card = card.child(
                        button(
                            format!("environment-delete-{}", e.instance),
                            "Delete now",
                            theme,
                        )
                        .on_press(move |s: &mut State| {
                            retention(
                                s,
                                host.clone(),
                                instance.clone(),
                                generation,
                                RetentionAction::ClaimDelete { now: true },
                            )
                        }),
                    );
                }
                if !e.pending_outputs.is_empty() {
                    let (host, instance, generation) = (
                        inventory.connection.clone(),
                        e.instance.clone(),
                        e.generation,
                    );
                    card = card.child(
                        button(
                            format!("environment-discard-{}", e.instance),
                            "Review pending-output discard",
                            theme,
                        )
                        .on_press(move |s: &mut State| {
                            preview_discard(s, host.clone(), instance.clone(), generation)
                        }),
                    );
                }
            }
            views.push(card);
        }
    }
    if let Some((host, preview)) = &c.discard {
        let (host, preview) = (host.clone(), preview.clone());
        views.push(col().key("discard-preview").gap(8.0).child(label(format!("Discard these unpublished operations from {}: {}. Published evidence remains intact. This cannot be undone.",preview.instance,preview.operations.iter().cloned().collect::<Vec<_>>().join(", ")),theme)).child(button("discard-confirm","Discard listed pending outputs",theme).on_press(move |s:&mut State|{let(host,preview)=(host.clone(),preview.clone());run(s,move ||async move{let reply:HostAllocationReply=request(&host,swarm_allocation::PATH,Some(body(&HostAllocationRequest::ConfirmDiscard{preview})?)).await?;allocation_result(reply)});s.swarm_controls.discard=None;})).child(button("discard-cancel","Cancel",theme).on_press(|s:&mut State|s.swarm_controls.discard=None)));
    }
    views
}
fn allocation_result(reply: HostAllocationReply) -> Result<String, String> {
    match reply{HostAllocationReply::Rejected{reason}=>Err(reason),HostAllocationReply::Busy{}=>Err("The host is busy; refresh and retry.".into()),_=>Ok("Environment updated. Refresh shows whether Keep/resume or cleanup won; deletion never removes published evidence.".into())}
}
fn retention(
    state: &mut State,
    host: String,
    instance: String,
    generation: u64,
    action: RetentionAction,
) {
    run(state, move || async move {
        let reply: HostAllocationReply = request(
            &host,
            swarm_allocation::PATH,
            Some(body(&HostAllocationRequest::Retention {
                instance,
                generation,
                action,
            })?),
        )
        .await?;
        allocation_result(reply)
    });
}
fn preview_discard(state: &mut State, host: String, instance: String, generation: u64) {
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        return;
    };
    if state.swarm_controls.busy {
        return;
    }
    state.swarm_controls.busy = true;
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "Could not start host connection.".to_owned())
            .and_then(|rt| {
                rt.block_on(async {
                    request::<HostAllocationReply>(
                        &host,
                        swarm_allocation::PATH,
                        Some(body(&HostAllocationRequest::PreviewDiscard {
                            instance,
                            generation,
                        })?),
                    )
                    .await
                })
            });
        handle.update(move |s, _| {
            s.swarm_controls.busy = false;
            match result {
                Ok(HostAllocationReply::DiscardPreview { preview }) => {
                    s.swarm_controls.discard = Some((host, preview))
                }
                Ok(reply) => {
                    if let Err(e) = allocation_result(reply) {
                        s.swarm_controls.error = e;
                    }
                }
                Err(e) => s.swarm_controls.error = e,
            }
        });
    });
}

/// Synthetic offline snapshots only; never resolves hosts or starts inference.
pub(crate) fn preview(state: &mut State, variant: &str) {
    if !variant.starts_with("stage9-") {
        return;
    }
    let c = &mut state.swarm_controls;
    c.choices=vec![DestinationChoice{host_name:"Example Linux host".into(),readiness:ConfigurationReadiness{configuration:ImageConfiguration{version:1,id:"image-example".into(),revision:1,image:"example/research-environment-with-a-long-image-name:synthetic-review-build".into(),architecture:LinuxArchitecture::Amd64,options:Default::default()},host_id:"host-example".into(),state:swarm_container::Readiness::Unavailable,reason:Some("Preparation failed: shell unavailable. Choose a compatible image or retry preparation.".into()),image_digest:None}}];
    c.storage = vec![(
        "Example host".into(),
        beale_contracts::v1::FleetSystemResources {
            disk_total_bytes: 256 * 1024 * 1024 * 1024,
            disk_available_bytes: 128 * 1024 * 1024 * 1024,
            ..Default::default()
        },
    )];
    c.selector_open = variant == "stage9-destinations";
    c.add_open = variant == "stage9-container-form";
    c.account_open = variant == "stage9-pacing";
    c.image = TextEdit::new("ubuntu:24.04");
    c.group = TextEdit::new("account-example");
    c.owner = TextEdit::new("host-example");
    c.participants = TextEdit::new("host-example, second-host-example");
    c.pace_amount = TextEdit::new("1.25");
    if variant == "stage9-local" {
        state.new_research_swarm_id.clear();
        c.notice = "Destinations cleared. This scan will run locally with No Subagents.".into();
        c.choices.clear();
    }
    if variant == "stage9-empty" {
        c.choices.clear();
        state.fleet = Default::default();
    }
    if variant == "stage9-retention" {
        c.inventories.push(HostInventory {
            connection: "local".into(),
            name: "Example host".into(),
            allocations: vec![],
            environments: vec![RetainedEnvironment {
                instance: "instance-example".into(),
                leader: "leader-example".into(),
                scan: "scan-example".into(),
                agent: "agent-example".into(),
                generation: 1,
                ended_at_ms: Some(1000),
                deadline_ms: Some(604801000),
                keep: true,
                references: ["published-reference-example".into()].into_iter().collect(),
                pending_outputs: ["pending-operation-example".into()].into_iter().collect(),
                termination_confirmed: false,
                checkpoint_confirmed: true,
                deletion: DeletionStep::Retained,
            }],
        });
    }
    if variant == "stage9-status"
        && let Some(detail) = &mut state.active_session
    {
        use beale_contracts::swarms::*;
        detail.swarm_id = "basic".into();
        detail.swarm_template_json = beale_swarms::template_snapshot("basic").unwrap();
        detail.pacing_warnings = vec![
            "Accounting authority unavailable. Local pacing is degraded and remains best effort."
                .into(),
        ];
        let overview=ScanOverview{revision:2,definition_id:"basic".into(),definition_revision:1,leader_rule:"Coordinate work and review handoffs. Delegate dedicated research to deputies and workers.".into(),destinations:vec![Destination{host_id:"host-example".into(),configuration_id:"image-example".into()}],pacing:vec![],agents:vec![beale_contracts::swarm_ui::AgentOverview{summary:AgentSummary{id:"deputy-example".into(),parent_id:Some("leader-example".into()),profile:ProfileRef{id:"research_deputy".into(),revision:1},role:Role::Deputy,state:AgentState::Suspended,revision:2},pending_assignments:1,queued_wakes:1,termination_unconfirmed:true,unknown_commands:1,resumable:true}]};
        detail.swarm_overview_json = serde_json::to_string(&overview).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};
    fn destination() -> SwarmDestination {
        SwarmDestination {
            host_id: "host-example".into(),
            configuration_id: "image-example".into(),
        }
    }
    fn state() -> State {
        let mut s = crate::visual_preview::fixture("research");
        s.swarm_controls.choices = vec![DestinationChoice {
            host_name: "Example host".into(),
            readiness: ConfigurationReadiness {
                configuration: ImageConfiguration {
                    version: 1,
                    id: "image-example".into(),
                    revision: 1,
                    image: "example/research:latest".into(),
                    architecture: LinuxArchitecture::Amd64,
                    options: Default::default(),
                },
                host_id: "host-example".into(),
                state: swarm_container::Readiness::Unprepared,
                reason: None,
                image_digest: None,
            },
        }];
        s
    }
    #[test]
    fn drafts_reject_invalid_saved_modes_and_feature_disable_without_conversion() {
        let mut s = state();
        s.swarm_controls.destinations.push(destination());
        assert!(validate_draft("basic", true, &s.swarm_controls.destinations, &[]).is_ok());
        assert!(
            validate_draft("basic", false, &s.swarm_controls.destinations, &[])
                .unwrap_err()
                .contains("disabled")
        );
        assert!(validate_draft("", true, &s.swarm_controls.destinations, &[]).is_err());
        assert_eq!(s.swarm_controls.destinations.len(), 1);
        select_mode(&mut s, String::new());
        assert!(s.swarm_controls.destinations.is_empty());
        assert!(s.swarm_controls.notice.contains("locally"));
        select_mode(&mut s, "basic".into());
        assert!(s.swarm_controls.destinations.is_empty());
        assert!(validate_draft("", false, &[], &[]).is_ok());
        assert!(validate_draft("basic", true, &[destination(), destination()], &[]).is_err());
    }
    #[test]
    fn multiple_destinations_and_both_pacing_modes_roundtrip_submission() {
        use prost::Message;
        let mut s = state();
        s.swarm_controls.destinations = vec![
            destination(),
            SwarmDestination {
                host_id: "second-host".into(),
                configuration_id: "second-image".into(),
            },
        ];
        resolve_fixture(&mut s, "account-api");
        s.swarm_controls.pace_amount = TextEdit::new("1.25");
        add_pacing(&mut s);
        resolve_fixture(&mut s, "account-subscription");
        s.swarm_controls.pace_kind = 2;
        s.swarm_controls.pace_amount = TextEdit::new("2.75");
        s.swarm_controls.denomination = TextEdit::new("primary");
        add_pacing(&mut s);
        assert_eq!(s.swarm_controls.pacing.len(), 2);
        assert_eq!(s.swarm_controls.pacing[0].amount, 1_250_000);
        assert_eq!(s.swarm_controls.pacing[1].amount, 275);
        let request = beale_contracts::v1::SubmitSessionTurnRequest {
            swarm_id: s.new_research_swarm_id,
            swarm_destinations: s.swarm_controls.destinations,
            swarm_pacing: s.swarm_controls.pacing,
            ..Default::default()
        };
        let decoded = beale_contracts::v1::SubmitSessionTurnRequest::decode(
            request.encode_to_vec().as_slice(),
        )
        .unwrap();
        assert_eq!(decoded, request);
        for value in ["-1", "NaN", "1.001", "1e2", "0.0001"] {
            assert!(decimal_units(value, 100).is_none());
        }
    }
    #[test]
    fn destination_selector_is_multiselect_and_disabled_local_mode_has_no_action() {
        let mut s = state();
        s.swarm_controls.selector_open = true;
        let mut rt = Runtime::new(
            s,
            |s, _| new_scan(s, s.appearance.style()),
            (700.0, 900.0),
            Env::new(true, 1.0),
        );
        rt.frame(0.0);
        let (x, y) = rt.find("scan-destination-0").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert_eq!(rt.state.swarm_controls.destinations, vec![destination()]);
        rt.update(|s, _| select_mode(s, String::new()));
        rt.frame(0.0);
        assert!(rt.find("scan-destination-0").is_none());
        let (x, y) = rt.find("scan-destinations").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert!(rt.state.swarm_controls.destinations.is_empty());
    }
    #[test]
    fn keyboard_opens_selector_and_selects_destination() {
        let mut rt = Runtime::new(
            state(),
            |s, _| new_scan(s, s.appearance.style()),
            (700.0, 900.0),
            Env::new(true, 1.0),
        );
        rt.frame(0.0);
        for target in ["scan-destinations", "scan-destination-0"] {
            for _ in 0..12 {
                if rt
                    .elements()
                    .iter()
                    .any(|e| e.focused && e.key.as_deref() == Some(target))
                {
                    break;
                }
                rt.key(rune::KeyEvent {
                    key: rune::Key::Tab,
                    mods: Default::default(),
                });
                rt.frame(0.0);
            }
            assert!(
                rt.elements()
                    .iter()
                    .any(|e| e.focused && e.key.as_deref() == Some(target)),
                "{target} must be keyboard reachable"
            );
            rt.key(rune::KeyEvent {
                key: rune::Key::Text(" ".into()),
                mods: Default::default(),
            });
            rt.frame(0.0);
        }
        assert_eq!(rt.state.swarm_controls.destinations, vec![destination()]);
    }
    #[test]
    fn pacing_form_remains_reachable_in_minimum_window_and_formats_human_units() {
        let s = crate::visual_preview::fixture("stage9-pacing");
        let mut rt = Runtime::new(s, crate::view, (840.0, 600.0), Env::new(true, 1.0));
        rt.frame(0.0);
        for _ in 0..80 {
            if rt
                .elements()
                .iter()
                .any(|e| e.focused && e.key.as_deref() == Some("pace-add"))
            {
                break;
            }
            rt.key(rune::KeyEvent {
                key: rune::Key::Tab,
                mods: Default::default(),
            });
            rt.frame(0.0);
        }
        assert!(
            rt.elements()
                .iter()
                .any(|e| e.focused && e.key.as_deref() == Some("pace-add"))
        );
        for _ in 0..60 {
            rt.frame(1.0 / 60.0);
        }
        let (bounds, _) = rt.presented("pace-add").unwrap();
        assert!(bounds[0] >= 0.0 && bounds[0] + bounds[2] <= 840.0);
        assert!(
            bounds[1] >= 0.0 && bounds[1] + bounds[3] <= 600.0,
            "{bounds:?}"
        );
        let p = SwarmPacingTarget {
            amount: 1_250_000,
            unit: "currency".into(),
            ..Default::default()
        };
        assert_eq!(display_amount(&p), "1.25");
        assert_eq!(
            display_amount(&SwarmPacingTarget {
                amount: 275,
                unit: "subscription".into(),
                ..Default::default()
            }),
            "2.75"
        );
    }
    #[test]
    fn image_registration_has_no_instance_count_and_filters_separate_configurations() {
        let mut s = state();
        s.fleet_section = crate::fleet_view::FleetSection::VirtualMachines;
        s.swarm_controls.add_open = true;
        let mut rt = Runtime::new(
            s,
            |s, _| col().children(fleet(s, s.appearance.style())),
            (700.0, 900.0),
            Env::new(true, 1.0),
        );
        rt.frame(0.0);
        assert!(rt.find("container-image").is_some());
        assert!(rt.find("container-instance-count").is_none());
        assert!(rt.find("fleet-container-config-0").is_some());
        rt.update(|s, _| s.fleet.tab = crate::fleet_view::FleetTab::Available);
        rt.frame(0.0);
        assert!(rt.find("fleet-container-config-0").is_none());
        rt.update(|s, _| s.fleet.tab = crate::fleet_view::FleetTab::Enabled);
        rt.frame(0.0);
        assert!(rt.find("fleet-container-config-0").is_some());
    }
    #[test]
    fn reconnect_status_keeps_uncertain_work_and_saved_rule_distinct_from_completion() {
        use beale_contracts::swarms::*;
        let overview = ScanOverview {
            revision: 3,
            definition_id: "basic".into(),
            definition_revision: 1,
            leader_rule: "Coordinate research; delegate dedicated investigation.".into(),
            destinations: vec![Destination {
                host_id: "host-example".into(),
                configuration_id: "image-example".into(),
            }],
            pacing: vec![],
            agents: vec![beale_contracts::swarm_ui::AgentOverview {
                summary: AgentSummary {
                    id: "agent-example".into(),
                    parent_id: Some("leader-example".into()),
                    profile: ProfileRef {
                        id: "research_deputy".into(),
                        revision: 1,
                    },
                    role: Role::Deputy,
                    state: AgentState::Suspended,
                    revision: 2,
                },
                pending_assignments: 1,
                queued_wakes: 2,
                termination_unconfirmed: true,
                unknown_commands: 1,
                resumable: true,
            }],
        };
        let detail = beale_contracts::v1::SessionDetail {
            swarm_id: "basic".into(),
            swarm_overview_json: serde_json::to_string(&overview).unwrap(),
            pacing_warnings: vec!["Combined pacing is degraded".into()],
            ..Default::default()
        };
        let mut s = state();
        s.active_session = Some(detail);
        let mut rt = Runtime::new(
            s,
            |s, _| session(s.active_session.as_ref().unwrap(), s.appearance.style()),
            (700.0, 900.0),
            Env::new(true, 1.0),
        );
        rt.frame(0.0);
        let texts = rt
            .elements()
            .into_iter()
            .filter_map(|e| e.text)
            .collect::<Vec<_>>()
            .join("\n");
        for expected in [
            "Termination unconfirmed",
            "2 queued wakes",
            "definition revision 1",
            "Global toggles",
            "degraded",
        ] {
            assert!(texts.contains(expected), "missing {expected}: {texts}");
        }
        assert!(!texts.contains("Completed"));
        rt.update(|s, _| {
            let d = s.active_session.as_mut().unwrap();
            let mut o: ScanOverview = serde_json::from_str(&d.swarm_overview_json).unwrap();
            o.agents[0].summary.state = AgentState::Completed;
            d.swarm_overview_json = serde_json::to_string(&o).unwrap();
        });
        rt.frame(0.0);
        let text = rt
            .elements()
            .into_iter()
            .filter_map(|e| e.text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!text.contains("Completed"));
        assert!(text.contains("Completion awaiting reconciliation"));
    }
    #[test]
    fn cleanup_claim_hides_keep_and_delete_and_pending_outputs_require_preview() {
        let mut s = state();
        s.swarm_controls.inventories.push(HostInventory {
            connection: "local".into(),
            name: "Example host".into(),
            allocations: vec![],
            environments: vec![RetainedEnvironment {
                instance: "instance-example".into(),
                leader: "leader-example".into(),
                scan: "scan-example".into(),
                agent: "agent-example".into(),
                generation: 1,
                ended_at_ms: Some(1000),
                deadline_ms: Some(2000),
                keep: false,
                references: Default::default(),
                pending_outputs: ["output-example".into()].into_iter().collect(),
                termination_confirmed: true,
                checkpoint_confirmed: true,
                deletion: DeletionStep::Retained,
            }],
        });
        let mut rt = Runtime::new(
            s,
            |s, _| col().children(fleet(s, s.appearance.style())),
            (700.0, 900.0),
            Env::new(true, 1.0),
        );
        rt.frame(0.0);
        assert!(rt.find("environment-delete-instance-example").is_none());
        assert!(rt.find("environment-discard-instance-example").is_some());
        rt.update(|s, _| {
            s.swarm_controls.inventories[0].environments[0].deletion = DeletionStep::Claimed
        });
        rt.frame(0.0);
        assert!(rt.find("environment-keep-instance-example").is_none());
        assert!(rt.find("environment-discard-instance-example").is_none());
    }
    fn resolve_fixture(state: &mut State, group: &str) {
        let context = account_context(state).unwrap();
        let snapshot = AccountingSnapshot {
            binding: Some(AccountBinding {
                group_id: group.into(),
                provider_id: format!("provider-{}", context.model.provider.id() as i32),
                owner_host_id: "host-example".into(),
                generation: 1,
                participants: vec!["host-example".into()],
                limited_identity: true,
            }),
            ..Default::default()
        };
        apply_account_resolution(
            state,
            state.swarm_controls.account_epoch,
            context,
            Ok(snapshot),
        );
    }
    #[test]
    fn resolved_accounts_are_fenced_by_provider_host_and_request_generation() {
        let mut s = state();
        resolve_fixture(&mut s, "account-example");
        let context = s.swarm_controls.account_context.clone().unwrap();
        let snapshot = s.swarm_controls.account.clone().unwrap();
        s.auth_status[crate::Provider::Anthropic.index()].api_key = true;
        s.model_catalogs[crate::catalog_index(
            crate::Provider::Anthropic,
            beale_contracts::v1::AuthMethod::ApiKey,
        )] = Some(crate::provider_models::ModelCatalog {
            models: vec![crate::provider_models::ModelEntry {
                id: "example-model".into(),
                name: "Example".into(),
            }],
            lead: "example-model".into(),
            small: "example-model".into(),
        });
        s.new_research_model = Some(crate::new_research::ModelSelection {
            provider: crate::Provider::Anthropic,
            method: beale_contracts::v1::AuthMethod::ApiKey,
            id: "example-model".into(),
        });
        assert!(resolved_account(&s).is_err());
        save_binding(&mut s);
        assert!(s.swarm_controls.error.contains("current host and provider"));
        s.swarm_controls.pace_amount = TextEdit::new("1");
        add_pacing(&mut s);
        assert!(s.swarm_controls.pacing.is_empty());
        s.new_research_model = Some(context.model.clone());
        s.swarm_controls.account_host = "second-host".into();
        s.swarm_controls.account = None;
        s.swarm_controls.busy = true;
        apply_account_resolution(&mut s, 0, context.clone(), Ok(snapshot.clone()));
        assert!(s.swarm_controls.account.is_none());
        assert!(!s.swarm_controls.busy);
        s.swarm_controls.account_host = context.host.clone();
        s.swarm_controls.account_epoch = 1;
        apply_account_resolution(&mut s, 0, context.clone(), Ok(snapshot.clone()));
        assert!(s.swarm_controls.account.is_none());
        apply_account_resolution(&mut s, 1, context, Ok(snapshot));
        assert!(resolved_account(&s).is_ok());
        s.swarm_controls.group = TextEdit::new("unsaved-group-example");
        add_pacing(&mut s);
        assert!(s.swarm_controls.pacing.is_empty());
    }
    fn allocation_fixture(domain: &str) -> Allocation {
        use swarm_allocation::*;
        Allocation {
            request: AllocationRequest {
                version: 1,
                id: format!("{domain}-allocation"),
                leader: "leader-example".into(),
                scan: "scan-example".into(),
                agent: format!("{domain}-agent"),
                branch: "branch-example".into(),
                domain: domain.into(),
                configuration: format!("{domain}-config"),
                configuration_revision: 1,
                instance: format!("{domain}-instance"),
                memory: DEFAULT_MEMORY,
                storage: vec![StorageDemand {
                    pool: "disk-example".into(),
                    key: format!("{domain}-private"),
                    bytes: DEFAULT_STORAGE,
                }],
                storage_reserve: STORAGE_RESERVE,
                release_parent: None,
                processing_result: false,
            },
            phase: AllocationPhase::Retained,
            prepared: false,
            execution: None,
            created_at_ms: 1000,
            reservation_deadline_ms: 2000,
            reason: None,
            sequence: 1,
        }
    }
    fn inventory_fixture(
        allocations: Vec<Allocation>,
        environments: Vec<RetainedEnvironment>,
    ) -> InventorySnapshot {
        InventorySnapshot {
            hosts: vec![],
            choices: vec![],
            inventories: vec![HostInventory {
                connection: "local".into(),
                name: "Example host".into(),
                allocations,
                environments,
            }],
            storage: vec![],
            error: String::new(),
        }
    }
    #[tokio::test]
    async fn retention_reloads_cleanup_winner_after_rejection_and_lost_response() {
        use beale_storage::{HostDatabase, fleet_allocation::AllocationStore};
        use swarm_allocation::*;
        for lost in [false, true] {
            let root = std::env::temp_dir().join(format!(
                "beale-ui-retention-example-{}",
                uuid::Uuid::new_v4()
            ));
            let store = AllocationStore::open(HostDatabase::open(&root).unwrap()).unwrap();
            let allocation = allocation_fixture("docker");
            let capacity = Capacity {
                domain: "docker".into(),
                observed_at_ms: 1000,
                available_memory: RAM_FLOOR + DEFAULT_MEMORY + 1,
                storage: std::collections::BTreeMap::from([(
                    "disk-example".into(),
                    STORAGE_RESERVE + DEFAULT_STORAGE * 8,
                )]),
                measured_memory: Default::default(),
                measured_storage: Default::default(),
            };
            store
                .enqueue(allocation.request.clone(), &capacity, 1000)
                .unwrap();
            store.admit_next(&capacity, 1000).unwrap().unwrap();
            store.confirm_stopped(&allocation.request.id, true).unwrap();
            let environment = store.environment(&allocation.request.instance).unwrap();
            let stale = store
                .retention(
                    &environment.instance,
                    environment.generation,
                    RetentionAction::End {
                        ended_at_ms: 1000,
                        retention_seconds: 0,
                    },
                    1000,
                )
                .unwrap();
            let mut s = state();
            apply_inventory(
                &mut s,
                0,
                Ok(inventory_fixture(
                    store.allocations().unwrap(),
                    vec![stale.clone()],
                )),
            );
            let (result, inventory) = mutate_and_reload(
                async {
                    store
                        .retention(
                            &stale.instance,
                            stale.generation,
                            RetentionAction::ClaimDelete { now: true },
                            1001,
                        )
                        .unwrap();
                    if lost {
                        Err("Synthetic response lost after commit".into())
                    } else {
                        store
                            .retention(
                                &stale.instance,
                                stale.generation,
                                RetentionAction::Keep { value: true },
                                1002,
                            )
                            .map(|_| "Unexpected Keep".into())
                            .map_err(|_| "Synthetic stale generation rejection".into())
                    }
                },
                || async {
                    Ok(inventory_fixture(
                        store.allocations().unwrap(),
                        store.environments().unwrap(),
                    ))
                },
            )
            .await;
            assert!(result.is_err());
            finish_mutation(&mut s, 0, result, inventory);
            assert!(!s.swarm_controls.error.is_empty());
            assert_eq!(
                s.swarm_controls.inventories[0].environments[0].deletion,
                DeletionStep::Claimed
            );
            let mut rt = Runtime::new(
                s,
                |s, _| col().children(fleet(s, s.appearance.style())),
                (700.0, 900.0),
                Env::new(true, 1.0),
            );
            rt.frame(0.0);
            assert!(rt.find("environment-keep-docker-instance").is_none());
            assert!(rt.find("environment-delete-docker-instance").is_none());
            assert!(
                rt.elements()
                    .iter()
                    .filter_map(|e| e.text.as_ref())
                    .any(|t| t.contains("Deletion in progress"))
            );
            drop(store);
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn mutation_refresh_is_ordered_and_keeps_both_failure_messages() {
        let mut s = state();
        s.swarm_controls.inventory_epoch = 2;
        s.swarm_controls.inventory_mutating = true;
        s.swarm_controls.busy = true;
        refresh(&mut s);
        assert!(s.swarm_controls.refresh_pending);
        assert_eq!(s.swarm_controls.inventory_epoch, 2);
        apply_inventory(
            &mut s,
            1,
            Ok(inventory_fixture(
                vec![allocation_fixture("native")],
                vec![],
            )),
        );
        assert!(s.swarm_controls.inventories.is_empty());
        finish_mutation(
            &mut s,
            2,
            Err("Synthetic mutation rejection".into()),
            Err("Synthetic refresh outage".into()),
        );
        assert_eq!(s.swarm_controls.error, "Synthetic mutation rejection");
        assert_eq!(s.swarm_controls.refresh_error, "Synthetic refresh outage");
        assert!(!s.swarm_controls.inventory_mutating);
    }
    #[test]
    fn native_only_inventory_keeps_container_empty_state() {
        let mut s = state();
        s.fleet_section = crate::fleet_view::FleetSection::VirtualMachines;
        s.fleet.tab = crate::fleet_view::FleetTab::Enabled;
        apply_inventory(
            &mut s,
            0,
            Ok(inventory_fixture(
                vec![allocation_fixture("native")],
                vec![],
            )),
        );
        let mut rt = Runtime::new(
            s,
            |s, _| col().children(crate::fleet_view::render(s, s.appearance.style(), 840.0)),
            (840.0, 900.0),
            Env::new(true, 1.0),
        );
        rt.frame(0.0);
        assert!(
            rt.elements()
                .iter()
                .filter_map(|e| e.text.as_ref())
                .any(|t| t.contains("No container configurations or instances"))
        );
    }
    #[test]
    fn container_filter_excludes_native_allocations_but_all_retains_native_controls() {
        let mut s = state();
        let allocations = vec![allocation_fixture("native"), allocation_fixture("docker")];
        let environments = allocations
            .iter()
            .map(|a| RetainedEnvironment {
                instance: a.request.instance.clone(),
                leader: "leader-example".into(),
                scan: "scan-example".into(),
                agent: a.request.agent.clone(),
                generation: 1,
                ended_at_ms: Some(1000),
                deadline_ms: Some(2000),
                keep: false,
                references: Default::default(),
                pending_outputs: Default::default(),
                termination_confirmed: true,
                checkpoint_confirmed: true,
                deletion: DeletionStep::Retained,
            })
            .collect();
        apply_inventory(&mut s, 0, Ok(inventory_fixture(allocations, environments)));
        let mut rt = Runtime::new(
            s,
            |s, _| col().children(fleet(s, s.appearance.style())),
            (700.0, 1000.0),
            Env::new(true, 1.0),
        );
        rt.frame(0.0);
        assert!(rt.find("environment-keep-native-instance").is_some());
        assert!(
            rt.elements()
                .iter()
                .filter_map(|e| e.text.as_ref())
                .any(|t| t.contains("Native local (no OS isolation)"))
        );
        rt.update(|s, _| s.fleet.tab = crate::fleet_view::FleetTab::Enabled);
        rt.frame(1.0);
        rt.frame(2.0);
        assert!(rt.find("environment-keep-native-instance").is_none());
        assert!(rt.find("environment-keep-docker-instance").is_some());
        assert!(
            !rt.elements()
                .iter()
                .filter_map(|e| e.text.as_ref())
                .any(|t| t.contains("native-instance"))
        );
        rt.update(|s, _| s.fleet.tab = crate::fleet_view::FleetTab::Available);
        rt.frame(0.0);
        assert!(rt.find("environment-keep-docker-instance").is_none());
    }
}
