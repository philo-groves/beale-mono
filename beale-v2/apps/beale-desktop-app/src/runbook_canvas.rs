use beale_contracts::{
    runbooks::{
        self, Cell, CellChange, CellEditRequest, CellEditResponse, CellResult, CellRunRequest,
        CellRunResponse, Document, GetRequest, GetResponse, RunExecution,
    },
    v1::SessionRunbook,
};
use reqwest::{Client, StatusCode};
use rune::{
    Align, El, Icon, LoopSpec, ScrollSpec, Side, TextEdit, col, el, icon, row, text, text_input,
};
use rune_kit::syntax::{self, Highlight, SyntaxTheme};
use std::{
    net::SocketAddr,
    ops::{Deref, DerefMut},
};

use crate::{
    AUTH_HANDLE, CONTENT_SURFACE_COLOR, State, ThemeStyle,
    icons::{CHECK, CHEVRON_DOWN, CLOSE, RUNBOOK, TRASH, VM_RUN},
    in_place_menu, ui_font,
};

static SPINNER_ARC: Icon = Icon {
    name: "runbook-cell-spinner-arc",
    view: 16.0,
    body: r#"<path d="M8 2A6 6 0 0 1 13.258 10.89" stroke="currentColor" stroke-width="2" stroke-linecap="round" fill="none"/>"#,
};
pub(crate) const TAB_BAR_HEIGHT: f32 = 48.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RunbookTabKey {
    workspace_id: String,
    session_id: Option<String>,
    runbook_id: String,
}

#[derive(Default)]
pub(crate) struct RunbookView {
    pub(crate) instance_id: u64,
    pub(crate) selected: Option<SessionRunbook>,
    pub(crate) workspace_id: String,
    pub(crate) session_id: Option<String>,
    pub(crate) document: Option<Document>,
    pub(crate) runs: Vec<RunExecution>,
    pub(crate) cell_results: Vec<CellResult>,
    pub(crate) loading: bool,
    pub(crate) error: String,
    pub(crate) generation: u64,
    pub(crate) language_menu_cell: Option<String>,
    pub(crate) name_edit: Option<(String, TextEdit)>,
    pub(crate) name_error: String,
    pub(crate) action_busy: Option<String>,
}

impl RunbookView {
    fn key(&self) -> Option<RunbookTabKey> {
        Some(RunbookTabKey {
            workspace_id: self.workspace_id.clone(),
            session_id: self.session_id.clone(),
            runbook_id: self.selected.as_ref()?.id.clone(),
        })
    }
}

pub(crate) struct CanvasRunbookState {
    views: Vec<RunbookView>,
    active: Option<usize>,
    empty: RunbookView,
    next_instance: u64,
}

impl Default for CanvasRunbookState {
    fn default() -> Self {
        Self {
            views: Vec::new(),
            active: None,
            empty: RunbookView::default(),
            next_instance: 1,
        }
    }
}

impl CanvasRunbookState {
    #[cfg(test)]
    pub(crate) fn with_view(view: RunbookView) -> Self {
        let mut view = view;
        view.instance_id = 1;
        Self {
            views: vec![view],
            active: Some(0),
            empty: RunbookView::default(),
            next_instance: 2,
        }
    }

    fn active_key(&self) -> Option<RunbookTabKey> {
        self.views.get(self.active?).and_then(RunbookView::key)
    }

    fn view_mut(&mut self, key: &RunbookTabKey) -> Option<&mut RunbookView> {
        self.views
            .iter_mut()
            .find(|view| view.key().as_ref() == Some(key))
    }

    fn select(&mut self, key: &RunbookTabKey) -> bool {
        let Some(index) = self
            .views
            .iter()
            .position(|view| view.key().as_ref() == Some(key))
        else {
            return false;
        };
        if let Some(active) = self.active.and_then(|index| self.views.get_mut(index)) {
            active.language_menu_cell = None;
            active.name_edit = None;
        }
        self.active = Some(index);
        true
    }

    fn close(&mut self, key: &RunbookTabKey) {
        let Some(index) = self
            .views
            .iter()
            .position(|view| view.key().as_ref() == Some(key))
        else {
            return;
        };
        self.views.remove(index);
        self.active = match self.active {
            Some(_) if self.views.is_empty() => None,
            Some(active) if active == index => Some(index.min(self.views.len() - 1)),
            Some(active) if active > index => Some(active - 1),
            other => other,
        };
    }
}

impl Deref for CanvasRunbookState {
    type Target = RunbookView;

    fn deref(&self) -> &Self::Target {
        self.active
            .and_then(|index| self.views.get(index))
            .unwrap_or(&self.empty)
    }
}

impl DerefMut for CanvasRunbookState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self.active {
            Some(index) => &mut self.views[index],
            None => &mut self.empty,
        }
    }
}

async fn load(client: &Client, addr: SocketAddr, input: GetRequest) -> Result<GetResponse, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let response = client
        .post(format!("http://{addr}{}", runbooks::GET_PATH))
        .bearer_auth(token)
        .json(&input)
        .send()
        .await
        .map_err(|_| "Could not load runbook".to_owned())?;
    if response.status() != StatusCode::OK {
        return Err(format!("Runbook unavailable ({})", response.status()));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "Could not read runbook".to_owned())?;
    if bytes.len() > 2_500_000 {
        return Err("Runbook response is too large".to_owned());
    }
    let result: GetResponse =
        serde_json::from_slice(&bytes).map_err(|_| "Invalid runbook response".to_owned())?;
    if result.runbook.id != input.id
        || result.runbook.workspace_id != input.workspace_id
        || result.runbook.cells.len() > 64
        || result.runbook.cells.iter().any(|cell| {
            cell.name().chars().count() > 40 || cell.name().chars().any(char::is_control)
        })
        || result.runs.len() > 5
        || result.cell_results.len() > 64
        || result.cell_results.iter().any(|cell| {
            !result
                .runbook
                .cells
                .iter()
                .any(|item| item.id() == cell.cell_id)
                || !matches!(cell.status.as_str(), "completed" | "failed")
                || cell.output.len() > 20_000
                || cell.stdout.len().saturating_add(cell.stderr.len()) > 20_000
        })
        || result.runs.iter().any(|run| {
            !matches!(
                run.status.as_str(),
                "running" | "completed" | "failed" | "interrupted"
            ) || run.output.len() > 64_000
                || run.current_cell_number > 64
        })
    {
        return Err("Invalid runbook response".to_owned());
    }
    Ok(result)
}

enum ActionResult {
    Ran(CellRunResponse),
    Edited(CellEditResponse),
}

async fn post_action(
    client: &Client,
    addr: SocketAddr,
    run: CellRunRequest,
    change: Option<CellChange>,
) -> Result<ActionResult, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let (path, response) = if let Some(change) = change {
        let request = CellEditRequest {
            workspace_id: run.workspace_id,
            session_id: run.session_id,
            id: run.id,
            cell_id: run.cell_id,
            expected_revision: run.expected_revision,
            change,
        };
        let response = client
            .post(format!("http://{addr}{}", runbooks::CELL_EDIT_PATH))
            .bearer_auth(token)
            .json(&request)
            .send()
            .await
            .map_err(|_| "Could not edit runbook cell".to_owned())?;
        (runbooks::CELL_EDIT_PATH, response)
    } else {
        let response = client
            .post(format!("http://{addr}{}", runbooks::CELL_RUN_PATH))
            .bearer_auth(token)
            .json(&run)
            .send()
            .await
            .map_err(|_| "Could not run cell".to_owned())?;
        (runbooks::CELL_RUN_PATH, response)
    };
    if response.status() != StatusCode::OK {
        return Err(match response.status() {
            StatusCode::CONFLICT => {
                "Runbook changed or the session is busy. Reload and try again.".to_owned()
            }
            StatusCode::FORBIDDEN => {
                "Runbook action was denied or the feature is disabled.".to_owned()
            }
            StatusCode::NOT_FOUND => "Runbook or session is unavailable.".to_owned(),
            code => format!("Runbook action failed ({code})"),
        });
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "Could not read runbook result".to_owned())?;
    if bytes.len() > 128_000 {
        return Err("Runbook action response is too large".to_owned());
    }
    if path == runbooks::CELL_EDIT_PATH {
        serde_json::from_slice::<CellEditResponse>(&bytes)
            .map(ActionResult::Edited)
            .map_err(|_| "Invalid runbook edit response".to_owned())
    } else {
        serde_json::from_slice::<CellRunResponse>(&bytes)
            .map(ActionResult::Ran)
            .map_err(|_| "Invalid runbook run response".to_owned())
    }
}

fn record_summary(
    state: &mut State,
    key: &RunbookTabKey,
    status: &str,
    updated_at_ms: u64,
    cell_number: u32,
) {
    let selected = {
        let Some(view) = state.canvas_runbook.view_mut(key) else {
            return;
        };
        let cell_name = cell_number
            .checked_sub(1)
            .and_then(|index| view.document.as_ref()?.cells.get(index as usize))
            .map_or("", Cell::name)
            .to_owned();
        let Some(selected) = view.selected.as_mut() else {
            return;
        };
        selected.status = status.to_owned();
        selected.updated_at_ms = updated_at_ms;
        selected.current_cell_number = cell_number;
        selected.current_cell_name = cell_name;
        if status == "done" {
            selected.completed_at_ms = updated_at_ms;
        }
        selected.clone()
    };
    if let Some(detail) = state.active_session.as_mut()
        && detail.summary.as_ref().is_some_and(|summary| {
            Some(summary.id.as_str()) == key.session_id.as_deref()
                && summary.workspace_id == key.workspace_id
        })
        && let Some(book) = detail
            .runbooks
            .iter_mut()
            .find(|book| book.id == selected.id)
    {
        *book = selected;
    }
}

fn start_action(state: &mut State, cell_id: String, change: Option<CellChange>) {
    let Some(key) = state.canvas_runbook.active_key() else {
        return;
    };
    if state.canvas_runbook.action_busy.is_some() {
        return;
    }
    let Some(document) = state.canvas_runbook.document.as_ref().cloned() else {
        return;
    };
    let Some(session_id) = state.canvas_runbook.session_id.clone() else {
        state.canvas_runbook.error =
            "Save a session before changing or running its runbook cells.".to_owned();
        return;
    };
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        state.canvas_runbook.error = "Desktop connection is not ready.".to_owned();
        return;
    };
    let Some(index) = document.cells.iter().position(|cell| cell.id() == cell_id) else {
        return;
    };
    if change.is_none() && !matches!(document.cells[index], Cell::Code { .. }) {
        return;
    }
    let request = CellRunRequest {
        workspace_id: document.workspace_id.clone(),
        session_id,
        id: document.id.clone(),
        cell_id: cell_id.clone(),
        expected_revision: document.revision,
    };
    let instance_id = state.canvas_runbook.instance_id;
    let previous_summary = state.canvas_runbook.selected.clone();
    state.canvas_runbook.language_menu_cell = None;
    state.canvas_runbook.name_edit = None;
    state.canvas_runbook.action_busy = Some(cell_id.clone());
    state.canvas_runbook.error.clear();
    if change.is_none() {
        record_summary(
            state,
            &key,
            "running",
            document.updated_at_ms,
            (index + 1) as u32,
        );
    }
    std::thread::spawn(move || {
        let result = (|| {
            let addr = crate::connection::configured_addr()
                .map_err(|_| "Invalid local app-server address".to_owned())?;
            let client = if change.is_some() {
                crate::workspaces::client()?
            } else {
                crate::sessions::stream_client()?
            };
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize runbook connection".to_owned())?;
            runtime.block_on(post_action(&client, addr, request, change))
        })();
        handle.update(move |state, _| {
            let Some(view) = state.canvas_runbook.view_mut(&key) else {
                return;
            };
            if view.instance_id != instance_id || view.action_busy.as_deref() != Some(&cell_id) {
                return;
            }
            view.action_busy = None;
            match result {
                Ok(ActionResult::Ran(response)) => {
                    let updated = chrono::Utc::now().timestamp_millis().max(0) as u64;
                    if response.status == "completed" || response.status == "failed" {
                        record_summary(state, &key, "done", updated, (index + 1) as u32);
                    }
                    refresh_tab(state, &key);
                }
                Ok(ActionResult::Edited(response)) => {
                    let document = response.runbook;
                    record_summary(state, &key, "prepared", document.updated_at_ms, 0);
                    if let Some(view) = state.canvas_runbook.view_mut(&key) {
                        view.document = Some(document);
                    }
                    refresh_tab(state, &key);
                }
                Err(error) => {
                    if let Some(view) = state.canvas_runbook.view_mut(&key) {
                        view.error = error;
                        view.selected = previous_summary.clone();
                    }
                    if let Some(previous) = previous_summary
                        && let Some(detail) = state.active_session.as_mut()
                        && detail.summary.as_ref().is_some_and(|summary| {
                            Some(summary.id.as_str()) == key.session_id.as_deref()
                                && summary.workspace_id == key.workspace_id
                        })
                        && let Some(book) = detail
                            .runbooks
                            .iter_mut()
                            .find(|book| book.id == previous.id)
                    {
                        *book = previous;
                    }
                }
            }
        });
    });
}

pub(crate) fn run_cell(state: &mut State, cell_id: String) {
    start_action(state, cell_id, None);
}
pub(crate) fn edit_language(state: &mut State, cell_id: String, language: String) {
    start_action(state, cell_id, Some(CellChange::SetLanguage { language }));
}
pub(crate) fn edit_name(state: &mut State, cell_id: String, name: String) {
    start_action(state, cell_id, Some(CellChange::SetName { name }));
}

fn save_name(state: &mut State) {
    let Some((id, edit)) = state.canvas_runbook.name_edit.as_ref() else {
        return;
    };
    let name = edit.text.trim();
    if name.chars().count() > 40 || name.chars().any(char::is_control) {
        state.canvas_runbook.name_error = "Use at most 40 characters on one line.".to_owned();
        return;
    }
    let id = id.clone();
    let name = name.to_owned();
    state.canvas_runbook.name_edit = None;
    state.canvas_runbook.name_error.clear();
    edit_name(state, id, name);
}
pub(crate) fn delete_cell(state: &mut State, cell_id: String) {
    start_action(state, cell_id, Some(CellChange::Delete));
}

pub(crate) fn open(state: &mut State, selected: SessionRunbook) {
    let session = state
        .active_session
        .as_ref()
        .and_then(|detail| detail.summary.as_ref());
    let workspace_id = session
        .map(|summary| summary.workspace_id.clone())
        .or_else(|| state.new_research_workspace_id.clone());
    let key = RunbookTabKey {
        workspace_id: workspace_id.unwrap_or_default(),
        session_id: session.map(|summary| summary.id.clone()),
        runbook_id: selected.id.clone(),
    };
    if state.canvas_runbook.select(&key) {
        state.canvas_runbook.selected = Some(selected);
    } else {
        let instance_id = state.canvas_runbook.next_instance;
        state.canvas_runbook.next_instance = instance_id.wrapping_add(1);
        state.canvas_runbook.views.push(RunbookView {
            instance_id,
            selected: Some(selected),
            workspace_id: key.workspace_id.clone(),
            session_id: key.session_id.clone(),
            ..RunbookView::default()
        });
        state.canvas_runbook.active = Some(state.canvas_runbook.views.len() - 1);
    }
    state.canvas_open = true;
    state.session_sidenav_visible = false;
    state.session_usage_menu = None;
    state.session_usage_row_hovered = None;
    state.session_usage_popup_hovered = false;
    refresh(state);
}

pub(crate) fn tab_bar(state: &State, theme: ThemeStyle<'_>) -> Option<El> {
    if state.canvas_runbook.views.is_empty() {
        return None;
    }
    let mut tabs = row()
        .key("canvas-tab-scroll")
        .w_full()
        .h(TAB_BAR_HEIGHT)
        .px(12.0)
        .center_y()
        .gap(7.0)
        .scroll_x();
    for view in &state.canvas_runbook.views {
        let Some(key) = view.key() else {
            continue;
        };
        let Some(selected) = view.selected.as_ref() else {
            continue;
        };
        let title = selected.title.clone();
        let title_width = rune::text::measure(&title, &[], ui_font(11.0, 450.0), None)
            .0
            .ceil()
            .max(1.0);
        let instance_id = view.instance_id;
        let active = state.canvas_runbook.active_key().as_ref() == Some(&key);
        let select_key = key.clone();
        let close_key = key.clone();
        tabs = tabs.child(
            row()
                .key(format!("canvas-tab-{instance_id}"))
                .w(title_width + 64.0)
                .h(32.0)
                .px(7.0)
                .center_y()
                .gap(5.0)
                .radius(10.0)
                .fill(theme.color(if active { "#303030" } else { "#242424" }))
                .border(1.0, theme.color(if active { "#555555" } else { "#383838" }))
                .child(
                    row()
                        .key(format!("canvas-tab-{instance_id}-select"))
                        .role("tab")
                        .label(title.clone())
                        .w(title_width + 23.0)
                        .h_full()
                        .center_y()
                        .gap(6.0)
                        .on_press(move |state: &mut State| {
                            if state.canvas_runbook.select(&select_key) {
                                refresh(state);
                            }
                        })
                        .child(icon(&RUNBOOK, 15.0).color(theme.color(if active {
                            "#d8d8d8"
                        } else {
                            "#929292"
                        })))
                        .child(
                            el().w(title_width + 2.0).h_full().center().child(
                                text(title.clone())
                                    .key(format!("canvas-tab-{instance_id}-title"))
                                    .font(ui_font(11.0, 450.0))
                                    .color(theme.color(if active { "#eeeeee" } else { "#aaaaaa" })),
                            ),
                        ),
                )
                .child(
                    el().key(format!("canvas-tab-{instance_id}-close"))
                        .role("button")
                        .label(format!("Close {title}"))
                        .help("Close runbook tab")
                        .size(20.0, 20.0)
                        .center()
                        .radius(6.0)
                        .hover(theme.color("#484848"))
                        .on_press(move |state: &mut State| {
                            state.canvas_runbook.close(&close_key);
                            if state.canvas_runbook.views.is_empty() {
                                state.canvas_open = false;
                            }
                        })
                        .child(icon(&CLOSE, 12.0).color(theme.color("#a6a6a6"))),
                ),
        );
    }
    Some(
        col()
            .key("canvas-tab-bar")
            .w_full()
            .h(TAB_BAR_HEIGHT)
            .fill(theme.color(CONTENT_SURFACE_COLOR))
            .child(tabs),
    )
}

pub(crate) fn refresh(state: &mut State) {
    let Some(key) = state.canvas_runbook.active_key() else {
        return;
    };
    refresh_tab(state, &key);
}

fn refresh_tab(state: &mut State, key: &RunbookTabKey) {
    let summary = state
        .active_session
        .as_ref()
        .filter(|detail| {
            detail.summary.as_ref().is_some_and(|summary| {
                Some(summary.id.as_str()) == key.session_id.as_deref()
                    && summary.workspace_id == key.workspace_id
            })
        })
        .and_then(|detail| {
            detail
                .runbooks
                .iter()
                .find(|item| item.id == key.runbook_id)
        })
        .cloned();
    let Some(view) = state.canvas_runbook.view_mut(key) else {
        return;
    };
    if let Some(current) = summary {
        view.selected = Some(current);
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        view.error = "Desktop connection is not ready.".to_owned();
        return;
    };
    if view.workspace_id.is_empty() {
        view.error = "Workspace is unavailable.".to_owned();
        return;
    }
    let input = GetRequest {
        workspace_id: view.workspace_id.clone(),
        id: key.runbook_id.clone(),
        session_id: view.session_id.clone(),
    };
    view.generation = view.generation.wrapping_add(1);
    let generation = view.generation;
    let instance_id = view.instance_id;
    view.loading = true;
    view.error.clear();
    let key = key.clone();
    std::thread::spawn(move || {
        let result = (|| {
            let addr = crate::connection::configured_addr()
                .map_err(|_| "Invalid local app-server address".to_owned())?;
            let client = crate::workspaces::client()?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize runbook connection".to_owned())?;
            runtime.block_on(load(&client, addr, input))
        })();
        handle.update(move |state, _| {
            let Some(view) = state.canvas_runbook.view_mut(&key) else {
                return;
            };
            if view.instance_id != instance_id || view.generation != generation {
                return;
            }
            view.loading = false;
            match result {
                Ok(response) => {
                    if let Some(selected) = view.selected.as_mut() {
                        selected.title = response.runbook.title.clone();
                    }
                    view.document = Some(response.runbook);
                    view.runs = response.runs;
                    view.cell_results = response.cell_results;
                    view.error.clear();
                }
                Err(error) => view.error = error,
            }
        });
    });
}

fn date_label(ms: u64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms.min(i64::MAX as u64) as i64)
        .map_or_else(
            || "—".to_owned(),
            |date| {
                date.with_timezone(&chrono::Local)
                    .format("%b %-d, %-I:%M %p")
                    .to_string()
            },
        )
}

fn cell_date_label(ms: u64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms.min(i64::MAX as u64) as i64)
        .map_or_else(
            || "—".to_owned(),
            |date| {
                date.with_timezone(&chrono::Local)
                    .format("%b %-d %H:%M")
                    .to_string()
            },
        )
}

fn cell_status(
    id: &str,
    runnable: bool,
    running: bool,
    result: Option<&CellResult>,
    theme: ThemeStyle<'_>,
) -> El {
    let mut status = row()
        .key(format!("canvas-runbook-cell-{id}-status"))
        .min_w(0.0)
        .shrink()
        .center_y()
        .gap(4.0);
    if running {
        return status.label("Running").child(
            text("Running")
                .font(ui_font(11.0, 400.0))
                .color(theme.color("#aebee5")),
        );
    }
    match result {
        Some(result) if runnable && result.status == "completed" => {
            status = status
                .label(format!("Succeeded {}", date_label(result.updated_at_ms)))
                .child(icon(&CHECK, 13.0).color(theme.color("#76c98d")))
                .child(
                    text(cell_date_label(result.updated_at_ms))
                        .min_w(0.0)
                        .truncate()
                        .font(ui_font(11.0, 400.0))
                        .color(theme.color("#a9b9ad")),
                );
        }
        Some(result) if runnable && result.status == "failed" => {
            status = status
                .label(format!("Failed {}", date_label(result.updated_at_ms)))
                .child(icon(&CLOSE, 13.0).color(theme.color("#e07676")))
                .child(
                    text(cell_date_label(result.updated_at_ms))
                        .min_w(0.0)
                        .truncate()
                        .font(ui_font(11.0, 400.0))
                        .color(theme.color("#c4a0a0")),
                );
        }
        _ => {
            let label = if runnable { "Prepared" } else { "Untouched" };
            status = status.label(label).child(
                text(label)
                    .font(ui_font(11.0, 400.0))
                    .color(theme.color("#929292")),
            );
        }
    }
    status
}

struct CellDisplayOutput {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn display_output(result: &CellResult) -> CellDisplayOutput {
    if result.output.is_empty() {
        return CellDisplayOutput {
            exit_code: result.exit_code,
            stdout: result.stdout.clone(),
            stderr: result.stderr.clone(),
        };
    }
    let raw = result.output.as_str();
    let raw = raw
        .split_once('\n')
        .filter(|(heading, _)| heading.starts_with("Cell ") && heading.ends_with("):"))
        .map_or(raw, |(_, rest)| rest);
    if let Some((status, body)) = raw.split_once('\n')
        && let Some(status) = status.strip_prefix("exit status: ")
        && let Some(body) = body.strip_prefix("stdout:\n")
        && body.matches("\nstderr:\n").count() == 1
        && let Some((stdout, stderr)) = body.split_once("\nstderr:\n")
    {
        return CellDisplayOutput {
            exit_code: status
                .rsplit(':')
                .next()
                .and_then(|code| code.trim().parse().ok()),
            stdout: stdout.to_owned(),
            stderr: stderr.to_owned(),
        };
    }
    CellDisplayOutput {
        exit_code: None,
        stdout: if result.status == "failed" {
            String::new()
        } else {
            raw.to_owned()
        },
        stderr: if result.status == "failed" {
            raw.to_owned()
        } else {
            String::new()
        },
    }
}

fn status_label(status: &str) -> &str {
    match status {
        "running" => "Active",
        "prepared" => "Prepared",
        "done" | "completed" => "Completed",
        "failed" => "Failed",
        "interrupted" => "Interrupted",
        _ => "Unknown",
    }
}

fn code_line_spans(
    highlights: &[Highlight],
    start: usize,
    end: usize,
    palette: SyntaxTheme,
) -> Vec<rune::Span> {
    let clipped = highlights.iter().filter_map(|(range, paint)| {
        let from = range.start.max(start);
        let to = range.end.min(end);
        (from < to).then(|| (from - start..to - start, *paint))
    });
    palette.spans(&clipped.collect::<Vec<_>>(), 0)
}

fn code_source(id: &str, language: &str, source: &str, theme: ThemeStyle<'_>) -> El {
    let lines = if source.is_empty() {
        vec![""]
    } else {
        source.split_inclusive('\n').collect::<Vec<_>>()
    };
    let digits = lines.len().to_string().len();
    let highlight_language = match language {
        "powershell" | "ps1" | "pwsh" => "shell",
        other => other,
    };
    let highlights = syntax::highlight(Some(highlight_language), source);
    let palette = SyntaxTheme::neutral(theme.is_dark());
    let mut result = col()
        .key(format!("canvas-runbook-cell-{id}-source"))
        .w_full()
        .pl(12.0);
    let mut offset = 0;
    for (index, raw) in lines.into_iter().enumerate() {
        let line = raw.trim_end_matches(['\r', '\n']);
        let spans = code_line_spans(&highlights, offset, offset + line.len(), palette);
        result = result.child(
            row()
                .key(format!("canvas-runbook-cell-{id}-line-{}", index + 1))
                .w_full()
                .min_h(18.0)
                .gap(12.0)
                .child(
                    text(format!("{:>digits$}", index + 1))
                        .key(format!(
                            "canvas-runbook-cell-{id}-line-number-{}",
                            index + 1
                        ))
                        .w(digits as f32 * 8.0 + 4.0)
                        .font(ui_font(12.0, 400.0).mono())
                        .color(theme.color("#777777")),
                )
                .child(
                    text(line.to_owned())
                        .key(format!(
                            "canvas-runbook-cell-{id}-source-line-{}",
                            index + 1
                        ))
                        .grow(1.0)
                        .min_w(0.0)
                        .selectable(true)
                        .wrap()
                        .font(ui_font(12.0, 400.0).mono())
                        .color(theme.color("#e2e2e2"))
                        .spans(spans),
                ),
        );
        offset += raw.len();
    }
    result
}

pub(crate) fn render(state: &State, theme: ThemeStyle<'_>, height: f32) -> El {
    let Some(selected) = &state.canvas_runbook.selected else {
        return col().w_full().grow(1.0).min_h(0.0);
    };
    let current_session_matches = state
        .active_session
        .as_ref()
        .and_then(|detail| detail.summary.as_ref())
        .is_some_and(|summary| {
            Some(summary.id.as_str()) == state.canvas_runbook.session_id.as_deref()
                && summary.workspace_id == state.canvas_runbook.workspace_id
        });
    let progress = (state.session_busy && current_session_matches)
        .then(|| {
            state
                .session_live_events
                .iter()
                .filter_map(|event| event.runbook_progress.as_ref())
                .find(|progress| progress.runbook_id == selected.id)
        })
        .flatten();
    let status = if progress.is_some() {
        "running"
    } else if selected.status == "done" {
        state
            .canvas_runbook
            .runs
            .first()
            .map_or("done", |run| run.status.as_str())
    } else {
        selected.status.as_str()
    };
    let mut body = col()
        .key("canvas-runbook-body")
        .w_full()
        .padding(20.0)
        .gap(16.0);
    let mut metadata = row()
        .key("canvas-runbook-metadata")
        .w_full()
        .center_y()
        .child(
            text(format!("{} · {}", selected.id, status_label(status)))
                .key("canvas-runbook-status")
                .font(ui_font(12.0, 400.0))
                .color(theme.color("#a6a6a6")),
        )
        .child(el().grow(1.0));
    if let Some(document) = &state.canvas_runbook.document {
        metadata = metadata.child(
            text(format!(
                "Revision {} · Updated {}",
                document.revision,
                date_label(document.updated_at_ms)
            ))
            .key("canvas-runbook-revision")
            .font(ui_font(11.0, 400.0))
            .color(theme.color("#929292")),
        );
    }
    body = if let Some(document) = &state.canvas_runbook.document {
        body.child(
            col()
                .key("canvas-runbook-description")
                .w_full()
                .padding(12.0)
                .gap(10.0)
                .radius(8.0)
                .border(1.0, theme.color("#383838"))
                .child(
                    text(document.purpose.clone())
                        .key("canvas-runbook-purpose")
                        .font(ui_font(13.0, 400.0))
                        .color(theme.color("#c4c4c4"))
                        .wrap(),
                )
                .child(metadata),
        )
    } else {
        body.child(metadata)
    };
    let current_cell = progress
        .map(|progress| progress.cell_number)
        .or_else(|| (status == "running").then_some(selected.current_cell_number));
    if let Some(current_cell) = current_cell {
        let name = progress.map_or(selected.current_cell_name.as_str(), |item| {
            item.cell_name.as_str()
        });
        body = body.child(
            text(if name.is_empty() {
                format!("Running cell #{}", current_cell.max(1))
            } else {
                format!("Running {name}...")
            })
            .key("canvas-runbook-progress")
            .font(ui_font(12.0, 400.0))
            .color(theme.color("#aebee5")),
        );
    }
    if state.canvas_runbook.loading && state.canvas_runbook.document.is_none() {
        body = body.child(
            text("Loading runbook…")
                .font(ui_font(12.0, 400.0))
                .color(theme.color("#929292")),
        );
    }
    if !state.canvas_runbook.error.is_empty() {
        body = body.child(
            text(state.canvas_runbook.error.clone())
                .font(ui_font(12.0, 400.0))
                .color(theme.color("#e07676")),
        );
    }
    if let Some(document) = &state.canvas_runbook.document {
        if document.cells.is_empty() {
            body = body.child(
                text("No cells yet.")
                    .font(ui_font(12.0, 400.0))
                    .color(theme.color("#929292")),
            );
        }
        for (index, cell) in document.cells.iter().enumerate() {
            let (id, language, source) = match cell {
                Cell::Markdown { id, source, .. } => (id, "Markdown", source),
                Cell::Code {
                    id,
                    language,
                    source,
                    ..
                } => (id, language.as_str(), source),
            };
            let busy = state.canvas_runbook.action_busy.is_some();
            let runnable = matches!(cell, Cell::Code { .. });
            let play_id = id.clone();
            let language_id = id.clone();
            let delete_id = id.clone();
            let saved_result = state
                .canvas_runbook
                .cell_results
                .iter()
                .find(|result| result.cell_id == *id);
            let running = status == "running" && current_cell == Some((index + 1) as u32);
            let mut header = row()
                .key(format!("canvas-runbook-cell-{id}-header"))
                .w_full()
                .center_y()
                .child(
                    row()
                        .w(128.0)
                        .min_w(0.0)
                        .shrink()
                        .center_y()
                        .gap(6.0)
                        .child(
                            el().key(format!("canvas-runbook-cell-{id}-play"))
                                .role("button")
                                .label(format!("Run cell {}", index + 1))
                                .help(if runnable {
                                    "Run this cell"
                                } else {
                                    "Only code cells can run"
                                })
                                .size(24.0, 24.0)
                                .center()
                                .radius(5.0)
                                .hover(theme.color("#353535"))
                                .on_press(move |state: &mut State| {
                                    if runnable {
                                        run_cell(state, play_id.clone());
                                    }
                                })
                                .child(if running {
                                    icon(&SPINNER_ARC, 16.0)
                                        .key(format!("canvas-runbook-cell-{id}-spinner"))
                                        .color(theme.color("#c9c9c9"))
                                        .looping(LoopSpec::rotation(0.9))
                                } else {
                                    icon(&VM_RUN, 15.0).color(theme.color(if runnable && !busy {
                                        "#c9c9c9"
                                    } else {
                                        "#666666"
                                    }))
                                }),
                        )
                        .child(cell_status(id, runnable, running, saved_result, theme)),
                )
                .child(
                    el().key(format!("canvas-runbook-cell-{id}-name-control"))
                        .role("button")
                        .label(format!("Name cell {}", index + 1))
                        .help("Edit cell name")
                        .grow(1.0)
                        .min_w(0.0)
                        .center()
                        .on_press({
                            let id = id.clone();
                            let name = cell.name().to_owned();
                            move |state: &mut State| {
                                if state.canvas_runbook.action_busy.is_none() {
                                    state.canvas_runbook.language_menu_cell = None;
                                    state.canvas_runbook.name_edit =
                                        Some((id.clone(), TextEdit::new(name.clone())));
                                    state.canvas_runbook.name_error.clear();
                                }
                            }
                        })
                        .child(
                            text(if cell.name().is_empty() {
                                (index + 1).to_string()
                            } else {
                                cell.name().to_owned()
                            })
                            .key(format!("canvas-runbook-cell-{id}-number"))
                            .truncate()
                            .font(ui_font(11.0, 400.0))
                            .color(theme.color("#929292")),
                        ),
                )
                .child(
                    row()
                        .key(format!("canvas-runbook-cell-{id}-actions"))
                        .w(128.0)
                        .min_w(0.0)
                        .shrink()
                        .justify_end()
                        .center_y()
                        .gap(4.0)
                        .child(
                            row()
                                .key(format!("canvas-runbook-cell-{id}-language"))
                                .min_w(0.0)
                                .shrink()
                                .role("button")
                                .label(format!("Cell {} language: {language}", index + 1))
                                .h(24.0)
                                .px(6.0)
                                .center_y()
                                .gap(4.0)
                                .radius(5.0)
                                .hover(theme.color("#353535"))
                                .on_press(move |state: &mut State| {
                                    if state.canvas_runbook.action_busy.is_none() {
                                        state.canvas_runbook.language_menu_cell =
                                            (state.canvas_runbook.language_menu_cell.as_deref()
                                                != Some(language_id.as_str()))
                                            .then(|| language_id.clone());
                                    }
                                })
                                .child(
                                    text(language.to_owned())
                                        .min_w(0.0)
                                        .truncate()
                                        .font(ui_font(11.0, 400.0))
                                        .color(theme.color("#bdbdbd")),
                                )
                                .child(icon(&CHEVRON_DOWN, 11.0).color(theme.color("#929292"))),
                        )
                        .child(
                            el().key(format!("canvas-runbook-cell-{id}-delete"))
                                .role("button")
                                .label(format!("Delete cell {}", index + 1))
                                .help("Delete this cell")
                                .size(24.0, 24.0)
                                .center()
                                .radius(5.0)
                                .hover(theme.color("#353535"))
                                .on_press(move |state: &mut State| {
                                    delete_cell(state, delete_id.clone())
                                })
                                .child(icon(&TRASH, 14.0).color(theme.color(if busy {
                                    "#666666"
                                } else {
                                    "#bdbdbd"
                                }))),
                        ),
                );
            if busy && !running {
                header = header.opacity(0.6);
            }
            let mut card = col()
                .key(format!("canvas-runbook-cell-{id}"))
                .w_full()
                .padding(8.0)
                .gap(8.0)
                .radius(9.0)
                .fill(theme.color("#1b1b1b"))
                .border(1.0, theme.color("#303030"))
                .child(header);
            card = card.child(match cell {
                Cell::Markdown { .. } => {
                    crate::new_research::transcript_markdown(source.clone(), theme, false, false)
                }
                Cell::Code { .. } => code_source(id, language, source, theme),
            });
            if let Some(result) = saved_result {
                let display = display_output(result);
                if display.exit_code.is_some()
                    || !display.stdout.is_empty()
                    || !display.stderr.is_empty()
                {
                    let mut output = col()
                        .key(format!("canvas-runbook-cell-{id}-result"))
                        .w_full()
                        .gap(7.0)
                        .child(el().w_full().h(1.0).fill(theme.color("#303030")));
                    let mut heading = row()
                        .w_full()
                        .center_y()
                        .child(
                            text("Output")
                                .font(ui_font(11.0, 500.0))
                                .color(theme.color("#929292")),
                        )
                        .child(el().grow(1.0));
                    if let Some(code) = display.exit_code {
                        heading = heading.child(
                            text(format!("Exit Code: {code}"))
                                .key(format!("canvas-runbook-cell-{id}-exit-code"))
                                .font(ui_font(11.0, 400.0))
                                .color(theme.color("#929292")),
                        );
                    }
                    output = output.child(heading);
                    let stdout = display.stdout.trim_end_matches(['\r', '\n']);
                    if !stdout.is_empty() {
                        output = output.child(
                            text(stdout.to_owned())
                                .key(format!("canvas-runbook-cell-{id}-result-output"))
                                .selectable(true)
                                .wrap()
                                .font(ui_font(11.0, 400.0).mono())
                                .color(theme.color("#c8c8c8")),
                        );
                    }
                    let stderr = display.stderr.trim_end_matches(['\r', '\n']);
                    if !stderr.is_empty() {
                        output = output.child(
                            text(stderr.to_owned())
                                .key(format!("canvas-runbook-cell-{id}-result-error"))
                                .selectable(true)
                                .wrap()
                                .font(ui_font(11.0, 400.0).mono())
                                .color(theme.color("#e07676")),
                        );
                    }
                    card = card.child(output);
                }
            }
            body = body.child(card);
        }
    }
    let mut strip = col()
        .key("canvas-runbook-status-strip")
        .w(34.0)
        .pt(24.0)
        .pr(14.0)
        .gap(10.0);
    if let Some(document) = &state.canvas_runbook.document {
        for (index, cell) in document.cells.iter().enumerate() {
            let result = state
                .canvas_runbook
                .cell_results
                .iter()
                .find(|result| result.cell_id == cell.id());
            let running = status == "running" && current_cell == Some((index + 1) as u32);
            let color = match result
                .filter(|_| !running)
                .map(|result| result.status.as_str())
            {
                Some("completed") => "#76c98d",
                Some("failed") => "#e07676",
                _ => "#777777",
            };
            strip = strip.child(
                el().key(format!("canvas-runbook-status-dash-{}", cell.id()))
                    .label(format!(
                        "Cell {}: {}",
                        index + 1,
                        if running {
                            "running"
                        } else {
                            result.map_or("unrun", |result| result.status.as_str())
                        }
                    ))
                    .size(14.0, 3.0)
                    .radius(1.5)
                    .fill(theme.color(color)),
            );
        }
    }
    let scroll_watch = state.canvas_scroll.clone();
    row()
        .key("canvas-runbook")
        .w_full()
        .h(height)
        .min_h(0.0)
        .child(
            col()
                .key("canvas-runbook-scroll")
                .grow(1.0)
                .min_w(0.0)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .scroll_edge_fade(20.0)
                .on_scroll_offset(move |position| scroll_watch.watch_scroll(position))
                .on_wheel(|state: &mut State, event, tx| state.canvas_scroll.wheel(event, tx))
                .child(body),
        )
        .child(strip)
        .child(state.canvas_scroll.bar(theme))
}

pub(crate) fn language_popup(state: &State, theme: ThemeStyle<'_>) -> Option<El> {
    let cell_id = state.canvas_runbook.language_menu_cell.as_ref()?;
    let cell = state
        .canvas_runbook
        .document
        .as_ref()?
        .cells
        .iter()
        .find(|cell| cell.id() == cell_id)?;
    let current = match cell {
        Cell::Markdown { .. } => "Markdown",
        Cell::Code { language, .. } => language.as_str(),
    };
    let mut languages = vec![
        "Markdown".to_owned(),
        "Bash".to_owned(),
        "Python".to_owned(),
        "JavaScript".to_owned(),
        "PowerShell".to_owned(),
    ];
    if !languages
        .iter()
        .any(|language| language.eq_ignore_ascii_case(current))
    {
        languages.push(current.to_owned());
    }
    let mut options = col().w_full().gap(2.0);
    for language in languages {
        let selected = language.eq_ignore_ascii_case(current);
        let id = cell_id.clone();
        let value = language.clone();
        options = options.child(
            row()
                .key(format!(
                    "canvas-runbook-language-{}",
                    language.to_ascii_lowercase()
                ))
                .role("menuitem")
                .label(language.clone())
                .w_full()
                .h(30.0)
                .px(10.0)
                .center_y()
                .radius(5.0)
                .fill(if selected {
                    theme.color("#383838")
                } else {
                    rune::Color::TRANSPARENT
                })
                .hover(theme.color("#424242"))
                .on_press(move |state: &mut State| {
                    state.canvas_runbook.language_menu_cell = None;
                    if !selected {
                        edit_language(state, id.clone(), value.clone());
                    }
                })
                .child(
                    text(language)
                        .font(ui_font(12.0, 400.0))
                        .color(theme.color("#eeeeee")),
                ),
        );
    }
    Some(
        in_place_menu::popup_surface(
            col()
                .key("canvas-runbook-language-menu")
                .role("menu")
                .label("Cell language")
                .w(150.0)
                .padding(5.0)
                .child(options),
            theme.color("#242424"),
        )
        .anchored_fit(
            format!("canvas-runbook-cell-{cell_id}-language"),
            Side::Bottom,
            Align::End,
            4.0,
        )
        .modal(true, |state: &mut State| {
            state.canvas_runbook.language_menu_cell = None
        }),
    )
}

pub(crate) fn name_popup(state: &State, theme: ThemeStyle<'_>) -> Option<El> {
    let (cell_id, edit) = state.canvas_runbook.name_edit.as_ref()?;
    let mut panel = col()
        .key("canvas-runbook-name-menu")
        .role("dialog")
        .label("Cell name")
        .w(240.0)
        .padding(10.0)
        .gap(8.0)
        .child(
            text_input(edit, ui_font(12.0, 400.0))
                .key("canvas-runbook-name-input")
                .single_line()
                .w_full()
                .h(30.0)
                .padding(6.0)
                .placeholder("Optional short name", theme.color("#777777"))
                .radius(5.0)
                .fill(theme.color("#303030"))
                .color(theme.color("#eeeeee"))
                .on_edit(|state: &mut State| {
                    &mut state
                        .canvas_runbook
                        .name_edit
                        .as_mut()
                        .expect("name editor open")
                        .1
                })
                .on_submit(|state: &mut State, _| save_name(state)),
        );
    if !state.canvas_runbook.name_error.is_empty() {
        panel = panel.child(
            text(state.canvas_runbook.name_error.clone())
                .font(ui_font(11.0, 400.0))
                .color(theme.color("#e07676")),
        );
    }
    panel = panel.child(
        row()
            .w_full()
            .center_y()
            .child(
                text("40 characters max")
                    .grow(1.0)
                    .font(ui_font(10.0, 400.0))
                    .color(theme.color("#929292")),
            )
            .child(
                el().key("canvas-runbook-name-save")
                    .role("button")
                    .label("Save cell name")
                    .px(9.0)
                    .h(26.0)
                    .center()
                    .radius(5.0)
                    .fill(theme.color("#353535"))
                    .hover(theme.color("#484848"))
                    .on_press(save_name)
                    .child(
                        text("Save")
                            .font(ui_font(11.0, 500.0))
                            .color(theme.color("#eeeeee")),
                    ),
            ),
    );
    Some(
        in_place_menu::popup_surface(panel, theme.color("#242424"))
            .anchored_fit(
                format!("canvas-runbook-cell-{cell_id}-name-control"),
                Side::Bottom,
                Align::Center,
                4.0,
            )
            .modal(true, |state: &mut State| {
                state.canvas_runbook.name_edit = None;
                state.canvas_runbook.name_error.clear();
            }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_preserve_runbook_state_and_select_a_neighbor_on_close() {
        let view = |id: &str, title: &str| RunbookView {
            workspace_id: "workspace-example".to_owned(),
            session_id: Some("session-example".to_owned()),
            selected: Some(SessionRunbook {
                id: id.to_owned(),
                title: title.to_owned(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut tabs = CanvasRunbookState::with_view(view("R1", "First procedure"));
        tabs.views[0].error = "First view state".to_owned();
        let first = tabs.views[0].key().unwrap();
        let mut second_view = view("R2", "Second procedure");
        second_view.instance_id = 2;
        let second = second_view.key().unwrap();
        tabs.views.push(second_view);
        tabs.active = Some(1);
        assert!(tabs.select(&first));
        assert_eq!(tabs.error, "First view state");
        tabs.close(&first);
        assert_eq!(tabs.selected.as_ref().unwrap().id, "R2");
        tabs.close(&second);
        assert!(tabs.selected.is_none());
    }

    #[test]
    fn legacy_cell_output_splits_streams_and_removes_inferred_prefix() {
        let result = CellResult {
            cell_id: "L1".to_owned(),
            status: "completed".to_owned(),
            updated_at_ms: 1,
            output:
                "Cell L1 (python):\nexit status: exit code: 0\nstdout:\nExample output\n\nstderr:\n"
                    .to_owned(),
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
        };
        let display = display_output(&result);
        assert_eq!(display.exit_code, Some(0));
        assert_eq!(display.stdout.trim(), "Example output");
        assert!(display.stderr.is_empty());
    }

    #[test]
    fn syntax_spans_remain_within_each_source_line() {
        let source = "const first = 1;\nconst second = 2;";
        let start = source.find("const second").unwrap();
        let highlights = syntax::highlight(Some("javascript"), source);
        let spans = code_line_spans(&highlights, start, source.len(), SyntaxTheme::neutral(true));
        assert!(!spans.is_empty());
        assert!(spans.iter().any(|span| span.start == 0));
        assert!(
            spans
                .iter()
                .all(|span| (span.end as usize) <= source.len() - start)
        );
        assert!(spans.iter().all(|span| span.color.is_some()));
    }
}
