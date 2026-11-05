use beale_contracts::{
    AUTOMATIONS_CANCEL_PATH, AUTOMATIONS_PATH, PROTOBUF_CONTENT_TYPE,
    v1::{
        AutomationJob, AutomationRun, AutomationRunStatus, CancelAutomationRequest,
        ListAutomationJobsResponse,
    },
};
use prost::Message;
use reqwest::{StatusCode, header::CONTENT_TYPE};
use rune::{Color, El, ScrollSpec, col, el, icon, row, text};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{
    AUTH_HANDLE, ConnectionStatus, SelectionMenu, State, ThemeStyle, form_button,
    icons::{ARCHIVE, AUTOMATIONS, NEW_RESEARCH},
    sidenav_heading, start_session_load, ui_font, view_title_font,
};

#[derive(Default)]
pub(crate) struct AutomationState {
    pub(crate) jobs: Vec<AutomationJob>,
    pub(crate) runs: Vec<AutomationRun>,
    pub(crate) selected_id: Option<String>,
    selected_run_id: Option<String>,
    pub(crate) loaded: bool,
    last_loaded_at: Option<std::time::Instant>,
    loading: bool,
    busy: bool,
    error: String,
    generation: u64,
    confirm_series: Option<bool>,
    active_expanded: bool,
    future_expanded: bool,
    past_expanded: bool,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_millis().min(u64::MAX as u128) as u64)
}

struct AutomationSnapshot {
    jobs: Vec<AutomationJob>,
    runs: Vec<AutomationRun>,
}

async fn load_jobs() -> Result<AutomationSnapshot, String> {
    let addr = crate::connection::configured_addr()
        .map_err(|_| "Invalid local app-server address".to_owned())?;
    let client = crate::sessions::client()?;
    let token = crate::provider_auth::authenticated_token(&client, addr).await?;
    let response = client
        .get(format!("http://{addr}{AUTOMATIONS_PATH}"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| "Could not load automation jobs".to_owned())?;
    read_jobs(response).await
}

async fn read_jobs(response: reqwest::Response) -> Result<AutomationSnapshot, String> {
    if response.status() != StatusCode::OK
        || response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            != Some(PROTOBUF_CONTENT_TYPE)
    {
        return Err("Could not load automation jobs".to_owned());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "Could not read automation jobs".to_owned())?;
    if bytes.len() > 1_048_576 {
        return Err("Automation response is too large".to_owned());
    }
    let reply = ListAutomationJobsResponse::decode(bytes)
        .map_err(|_| "Invalid automation response".to_owned())?;
    let jobs = reply.jobs;
    let runs = reply.runs;
    if jobs.len() > 1_024
        || jobs.iter().any(|job| {
            uuid::Uuid::parse_str(&job.id).is_err()
                || uuid::Uuid::parse_str(&job.session_id).is_err()
                || uuid::Uuid::parse_str(&job.workspace_id).is_err()
                || job.title.is_empty()
                || job.title.len() > 512
                || !matches!(
                    job.repeat_preset.as_str(),
                    "minute" | "hour" | "day" | "week" | "month"
                )
                || job.last_run_at_ms == 0
                || job.next_run_at_ms == 0
        })
    {
        return Err("Invalid automation jobs".to_owned());
    }
    if runs.len() > 1_024
        || runs.iter().any(|run| {
            uuid::Uuid::parse_str(&run.id).is_err()
                || uuid::Uuid::parse_str(&run.job_id).is_err()
                || (!run.session_id.is_empty() && uuid::Uuid::parse_str(&run.session_id).is_err())
                || run.title.is_empty()
                || run.title.len() > 512
                || run.error.len() > 512
                || run.started_at_ms == 0
                || !matches!(
                    AutomationRunStatus::try_from(run.status),
                    Ok(AutomationRunStatus::Active
                        | AutomationRunStatus::Complete
                        | AutomationRunStatus::Failed
                        | AutomationRunStatus::Interrupted)
                )
                || (run.status == AutomationRunStatus::Active as i32) == (run.completed_at_ms > 0)
        })
    {
        return Err("Invalid automation runs".to_owned());
    }
    Ok(AutomationSnapshot { jobs, runs })
}

async fn cancel_job(id: String, series: bool) -> Result<AutomationSnapshot, String> {
    let addr = crate::connection::configured_addr()
        .map_err(|_| "Invalid local app-server address".to_owned())?;
    let client = crate::sessions::client()?;
    let token = crate::provider_auth::authenticated_token(&client, addr).await?;
    let request = CancelAutomationRequest { id, series };
    let response = client
        .post(format!("http://{addr}{AUTOMATIONS_CANCEL_PATH}"))
        .bearer_auth(token)
        .header(CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
        .body(request.encode_to_vec())
        .send()
        .await
        .map_err(|_| "Could not change automation job".to_owned())?;
    read_jobs(response).await
}

pub(crate) fn start_load(state: &mut State) {
    if state.automations.loading
        || state.automations.busy
        || !matches!(state.connection, ConnectionStatus::Connected { .. })
    {
        return;
    }
    if state.automations.loaded
        && state
            .automations
            .last_loaded_at
            .is_some_and(|time| time.elapsed() < std::time::Duration::from_secs(5))
    {
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        return;
    };
    state.automations.loading = true;
    state.automations.generation = state.automations.generation.wrapping_add(1);
    let generation = state.automations.generation;
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "Could not initialize automation connection".to_owned())
            .and_then(|runtime| runtime.block_on(load_jobs()));
        handle.update(move |state, _| {
            if state.automations.generation != generation {
                return;
            }
            state.automations.loading = false;
            match result {
                Ok(snapshot) => {
                    state.automations.jobs = snapshot.jobs;
                    state.automations.runs = snapshot.runs;
                    state.automations.loaded = true;
                    state.automations.last_loaded_at = Some(std::time::Instant::now());
                    state.automations.error.clear();
                }
                Err(error) => state.automations.error = error,
            }
        });
    });
}

fn start_cancel(state: &mut State) {
    if state.automations.busy {
        return;
    }
    let Some(id) = state.automations.selected_id.clone() else {
        return;
    };
    let Some(series) = state.automations.confirm_series else {
        return;
    };
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        return;
    };
    state.automations.busy = true;
    state.automations.error.clear();
    state.automations.generation = state.automations.generation.wrapping_add(1);
    let generation = state.automations.generation;
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "Could not initialize automation connection".to_owned())
            .and_then(|runtime| runtime.block_on(cancel_job(id, series)));
        handle.update(move |state, _| {
            if state.automations.generation != generation {
                return;
            }
            state.automations.busy = false;
            match result {
                Ok(snapshot) => {
                    state.automations.jobs = snapshot.jobs;
                    state.automations.runs = snapshot.runs;
                    state.automations.loaded = true;
                    state.automations.last_loaded_at = Some(std::time::Instant::now());
                    state.automations.confirm_series = None;
                    state.automations.error.clear();
                }
                Err(error) => state.automations.error = error,
            }
        });
    });
}

fn schedule_job(state: &mut State) {
    state.start_new_research();
    state.selection_menu = Some(SelectionMenu::Repeat);
}

fn selected_job(state: &State) -> Option<&AutomationJob> {
    let id = state.automations.selected_id.as_deref()?;
    state.automations.jobs.iter().find(|job| job.id == id)
}

fn selected_run(state: &State) -> Option<&AutomationRun> {
    let id = state.automations.selected_run_id.as_deref()?;
    state.automations.runs.iter().find(|run| run.id == id)
}

fn list_row(job: &AutomationJob, future: bool, selected: bool, theme: ThemeStyle<'_>) -> El {
    let id = job.id.clone();
    let session_id = job.session_id.clone();
    let label = job.title.clone();
    let mut item = row()
        .key(format!(
            "automation-job-{}-{}",
            if future { "future" } else { "past" },
            job.id
        ))
        .role("button")
        .label(label.clone())
        .w_full()
        .h(30.0)
        .px(4.0)
        .gap(10.0)
        .center_y()
        .radius(8.0)
        .continuous_corners()
        .fill(if selected {
            theme.color("#303030")
        } else {
            Color::TRANSPARENT
        })
        .hover(theme.color("#282828"))
        .child(icon(&AUTOMATIONS, 15.0).color(theme.color("#9d9d9d")))
        .child(
            text(label)
                .grow(1.0)
                .min_w(0.0)
                .truncate_fade()
                .font(ui_font(14.0, if selected { 550.0 } else { 400.0 }))
                .color(theme.color("#b8b8b8")),
        );
    if future {
        item = item.on_press(move |state: &mut State| {
            state.automations.selected_id = Some(id.clone());
            state.automations.selected_run_id = None;
        });
    } else {
        item =
            item.on_press(move |state: &mut State| start_session_load(state, session_id.clone()));
    }
    item
}

fn run_row(run: &AutomationRun, active: bool, selected: bool, theme: ThemeStyle<'_>) -> El {
    let id = run.id.clone();
    let session_id = run.session_id.clone();
    row()
        .key(format!(
            "automation-run-{}-{id}",
            if active { "active" } else { "past" }
        ))
        .role("button")
        .label(run.title.clone())
        .w_full()
        .h(30.0)
        .px(4.0)
        .center_y()
        .gap(10.0)
        .radius(8.0)
        .continuous_corners()
        .fill(if selected {
            theme.color("#303030")
        } else {
            Color::TRANSPARENT
        })
        .hover(theme.color("#282828"))
        .on_press(move |state: &mut State| {
            if session_id.is_empty() {
                state.automations.selected_id = None;
                state.automations.selected_run_id = Some(id.clone());
            } else {
                start_session_load(state, session_id.clone());
            }
        })
        .child(
            icon(if active { &NEW_RESEARCH } else { &ARCHIVE }, 15.0).color(theme.color("#9d9d9d")),
        )
        .child(
            text(run.title.clone())
                .grow(1.0)
                .min_w(0.0)
                .truncate_fade()
                .font(ui_font(14.0, if selected { 550.0 } else { 400.0 }))
                .color(theme.color("#b8b8b8")),
        )
}

fn section_header(label: &str, glyph: &'static rune::Icon, theme: ThemeStyle<'_>) -> El {
    row()
        .key(format!("automations-{label}-label"))
        .h(30.0)
        .px(4.0)
        .mt(10.0)
        .center_y()
        .gap(10.0)
        .child(icon(glyph, 15.0).color(theme.color("#9d9d9d")))
        .child(
            text(label.to_owned())
                .font(ui_font(14.0, 500.0))
                .color(theme.color("#b8b8b8")),
        )
}

fn section_toggle(
    key: &'static str,
    expanded: bool,
    theme: ThemeStyle<'_>,
    toggle: fn(&mut State),
) -> El {
    let label = if expanded { "Show less" } else { "Show more" };
    row()
        .key(key)
        .role("button")
        .label(label)
        .w_full()
        .h(28.0)
        .px(4.0)
        .center_y()
        .gap(10.0)
        .radius(8.0)
        .hover(theme.color("#282828"))
        .on_press(toggle)
        .child(el().size(15.0, 15.0))
        .child(
            text(label)
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#969696")),
        )
}

pub(crate) fn sidenav(state: &State, width: f32, theme: ThemeStyle<'_>) -> El {
    let mut list = col().w_full().gap(2.0);
    let future: Vec<_> = state
        .automations
        .jobs
        .iter()
        .filter(|job| !job.canceled)
        .collect();
    let active: Vec<_> = state
        .automations
        .runs
        .iter()
        .filter(|run| run.status == AutomationRunStatus::Active as i32)
        .collect();
    let past: Vec<_> = state
        .automations
        .runs
        .iter()
        .filter(|run| run.status != AutomationRunStatus::Active as i32)
        .collect();
    list = list.child(section_header("Active", &NEW_RESEARCH, theme));
    if active.is_empty() {
        list = list.child(empty_section("Active", theme));
    }
    for run in active.iter().take(if state.automations.active_expanded {
        usize::MAX
    } else {
        5
    }) {
        list = list.child(run_row(
            run,
            true,
            state.automations.selected_run_id.as_deref() == Some(run.id.as_str()),
            theme,
        ));
    }
    if active.len() > 5 {
        list = list.child(section_toggle(
            "automations-active-toggle",
            state.automations.active_expanded,
            theme,
            |state| state.automations.active_expanded = !state.automations.active_expanded,
        ));
    }
    list = list.child(section_header("Future", &AUTOMATIONS, theme));
    if future.is_empty() {
        list = list.child(empty_section("Future", theme));
    }
    for job in future.iter().take(if state.automations.future_expanded {
        usize::MAX
    } else {
        5
    }) {
        list = list.child(list_row(
            job,
            true,
            state.automations.selected_id.as_deref() == Some(job.id.as_str()),
            theme,
        ));
    }
    if future.len() > 5 {
        list = list.child(section_toggle(
            "automations-future-toggle",
            state.automations.future_expanded,
            theme,
            |state| state.automations.future_expanded = !state.automations.future_expanded,
        ));
    }
    list = list.child(section_header("Past", &ARCHIVE, theme));
    if past.is_empty() {
        list = list.child(empty_section("Past", theme));
    }
    for run in past.iter().take(if state.automations.past_expanded {
        usize::MAX
    } else {
        5
    }) {
        list = list.child(run_row(
            run,
            false,
            state.automations.selected_run_id.as_deref() == Some(run.id.as_str()),
            theme,
        ));
    }
    if past.len() > 5 {
        list = list.child(section_toggle(
            "automations-past-toggle",
            state.automations.past_expanded,
            theme,
            |state| state.automations.past_expanded = !state.automations.past_expanded,
        ));
    }
    col()
        .key("automations-sidenav")
        .role("navigation")
        .label("Automations sidebar")
        .w(width)
        .h_full()
        .px(12.0)
        .pt(12.0)
        .fill(theme.color("#111111"))
        .child(sidenav_heading("Automations", theme).mb(12.0))
        .child(
            row()
                .key("automations-schedule-job")
                .role("button")
                .label("Schedule a Job")
                .w_full()
                .h(30.0)
                .px(4.0)
                .center_y()
                .gap(12.0)
                .radius(8.0)
                .hover(theme.color("#282828"))
                .on_press(schedule_job)
                .child(icon(&NEW_RESEARCH, 16.0).color(theme.color("#bdbdbd")))
                .child(
                    text("Schedule a Job")
                        .font(ui_font(14.0, 400.0))
                        .color(theme.color("#dddddd")),
                ),
        )
        .child(
            row().h(26.0).px(8.0).mt(12.0).center_y().child(
                text("Jobs")
                    .font(ui_font(13.0, 400.0))
                    .color(theme.color("#b8b8b8")),
            ),
        )
        .child(
            col()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(list),
        )
}

fn empty_section(label: &str, theme: ThemeStyle<'_>) -> El {
    text(format!("No {label} Jobs"))
        .ml(8.0)
        .font(ui_font(13.0, 400.0))
        .color(theme.color("#777777"))
}

fn repeat_label(preset: &str) -> &'static str {
    match preset {
        "minute" => "Every minute",
        "hour" => "Every hour",
        "day" => "Every day",
        "week" => "Every week",
        "month" => "Every month",
        _ => "Unknown",
    }
}

pub(crate) fn render(state: &State, theme: ThemeStyle<'_>) -> Vec<El> {
    let mut body = col().w_full().gap(18.0).child(
        text("Automations")
            .key("automations-view-title")
            .font(view_title_font())
            .color(theme.color("#f2f2f2")),
    );
    if let Some(job) = selected_job(state) {
        let workspace = state
            .workspaces
            .iter()
            .find(|workspace| workspace.id == job.workspace_id)
            .map_or("Workspace unavailable", |workspace| workspace.name.as_str());
        let schedule = if job.canceled {
            repeat_label(&job.repeat_preset).to_owned()
        } else {
            format!(
                "{} · Next occurrence: {}",
                repeat_label(&job.repeat_preset),
                format_time(job.next_run_at_ms)
            )
        };
        body = body.child(
            col()
                .w_full()
                .max_w(760.0)
                .gap(16.0)
                .child(
                    text(job.title.clone())
                        .key("automations-job-title")
                        .font(ui_font(20.0, 550.0))
                        .color(theme.color("#eeeeee")),
                )
                .child(
                    text(schedule)
                        .font(ui_font(14.0, 400.0))
                        .color(theme.color("#b8b8b8")),
                )
                .child(
                    text(format!("Workspace: {workspace}"))
                        .font(ui_font(14.0, 400.0))
                        .color(theme.color("#b8b8b8")),
                ),
        );
        if job.canceled {
            body = body.child(
                text("This series has been canceled.")
                    .font(ui_font(14.0, 400.0))
                    .color(theme.color("#969696")),
            );
        } else {
            body = body.child(
                row()
                    .gap(10.0)
                    .child(
                        form_button("Cancel Occurrence", theme)
                            .key("automations-cancel-occurrence")
                            .on_press(|s: &mut State| s.automations.confirm_series = Some(false)),
                    )
                    .child(
                        form_button("Cancel Series", theme)
                            .key("automations-cancel-series")
                            .on_press(|s: &mut State| s.automations.confirm_series = Some(true)),
                    ),
            );
        }
    } else if let Some(run) = selected_run(state) {
        body = body.child(
            text(run.title.clone())
                .font(ui_font(20.0, 550.0))
                .color(theme.color("#eeeeee")),
        );
        let message = match AutomationRunStatus::try_from(run.status) {
            Ok(AutomationRunStatus::Active) => {
                "This job is running. Its session will open after the first turn is saved."
            }
            Ok(AutomationRunStatus::Failed | AutomationRunStatus::Interrupted) => {
                run.error.as_str()
            }
            _ => "This job has completed.",
        };
        body = body.child(
            text(message.to_owned())
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#b8b8b8")),
        );
    } else if state.automations.loading {
        body = body.child(
            text("Loading jobs…")
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#969696")),
        );
    } else if !matches!(state.connection, ConnectionStatus::Connected { .. }) {
        body = body.child(
            text("Connect the app-server to load jobs.")
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#dca15f")),
        );
    } else if state.automations.jobs.is_empty() {
        body = body.child(
            text("No jobs yet. Start one with Schedule a Job.")
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#969696")),
        );
    } else {
        body = body.child(
            text("Select a future job to view its schedule.")
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#969696")),
        );
    }
    if !state.automations.error.is_empty() {
        body = body.child(
            text(state.automations.error.clone())
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#e07676")),
        );
    }
    vec![body]
}

fn format_time(ms: u64) -> String {
    let now = now_ms();
    if ms <= now {
        return "pending".to_owned();
    }
    let minutes = (ms - now).div_ceil(60_000);
    if minutes < 60 {
        format!("in {minutes}m")
    } else if minutes < 1_440 {
        format!("in {}h {}m", minutes / 60, minutes % 60)
    } else {
        format!("in {}d {}h", minutes / 1_440, (minutes / 60) % 24)
    }
}

pub(crate) fn confirmation(state: &State, theme: ThemeStyle<'_>) -> Option<[El; 2]> {
    let series = state.automations.confirm_series?;
    let title = if series {
        "Cancel Series?"
    } else {
        "Cancel Occurrence?"
    };
    let body = text(if series {
        "Future occurrences in this series will be canceled."
    } else {
        "The next occurrence will be skipped."
    })
    .font(ui_font(14.0, 400.0))
    .color(theme.color("#b8b8b8"));
    let mut confirm = form_button(
        if state.automations.busy {
            "Canceling…"
        } else {
            "Confirm Cancel"
        },
        theme,
    )
    .key("automations-cancel-confirm");
    if series {
        confirm = confirm
            .fill(theme.color("#482525"))
            .hover(theme.color("#603030"));
    }
    if !state.automations.busy {
        confirm = confirm.on_press(start_cancel);
    }
    let actions = row()
        .w_full()
        .gap(8.0)
        .child(el().grow(1.0))
        .child(
            form_button("Keep Job", theme)
                .on_press(|s: &mut State| s.automations.confirm_series = None),
        )
        .child(confirm);
    Some(crate::modal_dialog::render(
        crate::modal_dialog::DialogSpec {
            key: "automations-cancel-dialog",
            title,
            width: 450.0,
            focus_return: "automations-cancel-occurrence",
        },
        body,
        actions,
        theme,
        |s: &mut State| s.automations.confirm_series = None,
    ))
}
