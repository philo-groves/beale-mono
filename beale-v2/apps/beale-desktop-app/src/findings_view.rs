#[path = "findings_plot.rs"]
mod plot;

use crate::icons::{EVIDENCE, SESSION_SIDENAV, TRACKING, WORKSPACE_SURFACES};
use crate::settings_scroll::SettingsScroll;
use beale_contracts::findings::{
    self, CreateRequest, CreateResponse, Finding, GetRequest, GetResponse, QueryPage, QueryRequest,
    Status, Summary,
};
use reqwest::{Client, StatusCode};
use rune::{Color, El, ScrollSpec, TextEdit, col, el, icon, row, text, text_input};
use std::net::SocketAddr;

use crate::{State, ThemeStyle, form_button, ui_font};

static FINDING_DIAMOND: rune::Icon = rune::Icon {
    name: "finding-diamond",
    view: 24.0,
    body: r#"<path d="M12 1 23 12 12 23 1 12Z" fill="currentColor"/>"#,
};

pub(crate) fn lane_scrolls() -> [SettingsScroll; 4] {
    [
        SettingsScroll::for_key("findings-lane-candidate-scroll"),
        SettingsScroll::for_key("findings-lane-validated-scroll"),
        SettingsScroll::for_key("findings-lane-screened-scroll"),
        SettingsScroll::for_key("findings-lane-reported-scroll"),
    ]
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum FindingsMode {
    #[default]
    Ledger,
    Plot,
}

#[derive(Default)]
pub(crate) struct FindingsBoard {
    pub(crate) records: Vec<Summary>,
    pub(crate) mode: FindingsMode,
    pub(crate) plot_selection: Option<(u8, u8)>,
    hidden_plot_stages: Vec<Status>,
    unrated_expanded: bool,
    pub(crate) loaded: bool,
    pub(crate) loading: bool,
    pub(crate) error: String,
    generation: u64,
    selected_id: Option<String>,
    selected: Option<Finding>,
    detail_loading: bool,
    detail_error: String,
    detail_generation: u64,
    rejected_expanded: bool,
    create_open: bool,
    create_busy: bool,
    create_generation: u64,
    create_title: TextEdit,
    create_summary: TextEdit,
    create_hypothesis: TextEdit,
    create_error: String,
}

impl FindingsBoard {
    fn plot_stage_visible(&self, status: Status) -> bool {
        status != Status::Rejected && !self.hidden_plot_stages.contains(&status)
    }

    pub(crate) fn toggle_plot_stage(&mut self, status: Status) {
        if status == Status::Rejected {
            return;
        }
        if let Some(index) = self
            .hidden_plot_stages
            .iter()
            .position(|hidden| *hidden == status)
        {
            self.hidden_plot_stages.remove(index);
        } else {
            self.hidden_plot_stages.push(status);
        }
        self.plot_selection = None;
    }

    pub(crate) fn preview_detail(&mut self) {
        let record: Finding = serde_json::from_value(serde_json::json!({
            "id": "F1", "workspace_id": "00000000-0000-4000-8000-000000000001",
            "title": "Example boundary mismatch", "summary": "Synthetic candidate for visual review; no vulnerability is confirmed.",
            "surface_ids": ["S1"], "state": {"candidate": {"hypothesis": "Compare the reader and owner roles against a local synthetic object.", "revision": 1, "completed_at_ms": 1800000000000_u64}},
            "created_at_ms": 1800000000000_u64, "updated_at_ms": 1800000000000_u64,
            "revision": 1, "history": []
        })).expect("valid synthetic finding");
        self.selected_id = Some(record.id.clone());
        self.selected = Some(record);
    }

    pub(crate) fn reset(&mut self) {
        let generation = self.generation.wrapping_add(1);
        let detail_generation = self.detail_generation.wrapping_add(1);
        let create_generation = self.create_generation.wrapping_add(1);
        *self = Self::default();
        self.generation = generation;
        self.detail_generation = detail_generation;
        self.create_generation = create_generation;
    }

    fn finish_load(&mut self, generation: u64, result: Result<Vec<Summary>, String>) {
        if generation != self.generation {
            return;
        }
        self.loading = false;
        match result {
            Ok(records) => {
                self.records = records;
                self.loaded = true;
                self.error.clear();
            }
            Err(error) => self.error = error,
        }
    }

    fn finish_detail(&mut self, generation: u64, id: &str, result: Result<Finding, String>) {
        if generation != self.detail_generation || self.selected_id.as_deref() != Some(id) {
            return;
        }
        self.detail_loading = false;
        match result {
            Ok(record) => {
                self.selected = Some(record);
                self.detail_error.clear();
            }
            Err(error) => self.detail_error = error,
        }
    }
}

async fn query_all(
    client: &Client,
    addr: SocketAddr,
    workspace_id: &str,
) -> Result<Vec<Summary>, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let mut records = Vec::new();
    let mut offset = 0_u32;
    loop {
        let response = client
            .post(format!("http://{addr}{}", findings::QUERY_PATH))
            .bearer_auth(&token)
            .json(&QueryRequest {
                workspace_id: workspace_id.to_owned(),
                text: String::new(),
                status: None,
                surface_ids: Vec::new(),
                limit: 50,
                offset,
            })
            .send()
            .await
            .map_err(|_| "Could not load findings.".to_owned())?;
        if response.status() == StatusCode::FORBIDDEN {
            return Err("Findings is not enabled for this workspace.".to_owned());
        }
        if response.status() != StatusCode::OK {
            return Err(format!("Could not load findings ({}).", response.status()));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| "Could not read findings.".to_owned())?;
        if bytes.len() > 1_000_000 {
            return Err("Findings response is too large.".to_owned());
        }
        let page: QueryPage =
            serde_json::from_slice(&bytes).map_err(|_| "Invalid findings response.".to_owned())?;
        records.extend(page.records);
        if records.len() > 10_000 {
            return Err("Findings ledger exceeds the display limit.".to_owned());
        }
        match page.next_offset {
            Some(next) if next > offset => offset = next,
            Some(_) => return Err("Invalid findings pagination.".to_owned()),
            None => return Ok(records),
        }
    }
}

async fn get_record(
    client: &Client,
    addr: SocketAddr,
    workspace_id: &str,
    id: &str,
) -> Result<Finding, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let response = client
        .post(format!("http://{addr}{}", findings::GET_PATH))
        .bearer_auth(token)
        .json(&GetRequest {
            workspace_id: workspace_id.to_owned(),
            id: id.to_owned(),
            history_offset: None,
        })
        .send()
        .await
        .map_err(|_| "Could not open finding.".to_owned())?;
    if response.status() != StatusCode::OK {
        return Err(format!("Could not open finding ({}).", response.status()));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "Could not read finding.".to_owned())?;
    if bytes.len() > 1_000_000 {
        return Err("Finding response is too large.".to_owned());
    }
    let page: GetResponse =
        serde_json::from_slice(&bytes).map_err(|_| "Invalid finding response.".to_owned())?;
    if page.record.id != id || page.record.workspace_id != workspace_id {
        return Err("Finding response does not match the selection.".to_owned());
    }
    Ok(page.record)
}

pub(crate) fn start_load(state: &mut State, workspace_id: uuid::Uuid) {
    state.workspace_findings.generation = state.workspace_findings.generation.wrapping_add(1);
    let generation = state.workspace_findings.generation;
    state.workspace_findings.loading = true;
    state.workspace_findings.error.clear();
    let Some(handle) = crate::AUTH_HANDLE.get().cloned() else {
        state.workspace_findings.loading = false;
        state.workspace_findings.error = "Desktop connection is not ready.".to_owned();
        return;
    };
    std::thread::spawn(move || {
        let result = (|| {
            let addr = crate::connection::configured_addr()
                .map_err(|_| "Invalid local app-server address".to_owned())?;
            let client = crate::workspaces::client()?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize Findings connection".to_owned())?;
            runtime.block_on(query_all(&client, addr, &workspace_id.to_string()))
        })();
        handle.update(move |state, _| {
            if state.workspace_id == Some(workspace_id) {
                state.workspace_findings.finish_load(generation, result);
            }
        });
    });
}

fn open_record(state: &mut State, id: String) {
    let Some(workspace_id) = state.workspace_id else {
        return;
    };
    state.workspace_findings.selected_id = Some(id.clone());
    state.workspace_findings.selected = None;
    state.workspace_findings.detail_loading = true;
    state.workspace_findings.detail_error.clear();
    state.workspace_findings.detail_generation =
        state.workspace_findings.detail_generation.wrapping_add(1);
    let generation = state.workspace_findings.detail_generation;
    let Some(handle) = crate::AUTH_HANDLE.get().cloned() else {
        state.workspace_findings.detail_loading = false;
        state.workspace_findings.detail_error = "Desktop connection is not ready.".to_owned();
        return;
    };
    std::thread::spawn(move || {
        let result = (|| {
            let addr = crate::connection::configured_addr()
                .map_err(|_| "Invalid local app-server address".to_owned())?;
            let client = crate::workspaces::client()?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize Findings connection".to_owned())?;
            runtime.block_on(get_record(&client, addr, &workspace_id.to_string(), &id))
        })();
        handle.update(move |state, _| {
            if state.workspace_id == Some(workspace_id) {
                state
                    .workspace_findings
                    .finish_detail(generation, &id, result);
            }
        });
    });
}

pub(crate) fn close_record(state: &mut State) {
    state.workspace_findings.detail_generation =
        state.workspace_findings.detail_generation.wrapping_add(1);
    state.workspace_findings.selected_id = None;
    state.workspace_findings.selected = None;
    state.workspace_findings.detail_loading = false;
    state.workspace_findings.detail_error.clear();
}

pub(crate) fn open_create(state: &mut State) {
    close_record(state);
    let board = &mut state.workspace_findings;
    board.create_generation = board.create_generation.wrapping_add(1);
    board.create_open = true;
    board.create_busy = false;
    board.create_title = TextEdit::new("");
    board.create_summary = TextEdit::new("");
    board.create_hypothesis = TextEdit::new("");
    board.create_error.clear();
}

fn close_create(state: &mut State) {
    let board = &mut state.workspace_findings;
    if board.create_busy {
        return;
    }
    board.create_generation = board.create_generation.wrapping_add(1);
    board.create_open = false;
    board.create_error.clear();
}

async fn post_create(
    client: &Client,
    addr: SocketAddr,
    request: &CreateRequest,
) -> Result<CreateResponse, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let response = client
        .post(format!("http://{addr}{}", findings::CREATE_PATH))
        .bearer_auth(token)
        .json(request)
        .send()
        .await
        .map_err(|_| "Could not add finding.".to_owned())?;
    if response.status() == StatusCode::FORBIDDEN {
        return Err("Findings is not enabled for this workspace.".to_owned());
    }
    if response.status() != StatusCode::OK {
        return Err(format!("Could not add finding ({}).", response.status()));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "Could not read created finding.".to_owned())?;
    if bytes.len() > 16_384 {
        return Err("Created finding response is too large.".to_owned());
    }
    let created: CreateResponse = serde_json::from_slice(&bytes)
        .map_err(|_| "Invalid created finding response.".to_owned())?;
    if created.record.id.is_empty() || created.record.status != Status::Candidate {
        return Err("Invalid created finding response.".to_owned());
    }
    Ok(created)
}

fn start_create(state: &mut State) {
    let Some(workspace_id) = state.workspace_id else {
        return;
    };
    let board = &mut state.workspace_findings;
    if !board.create_open || board.create_busy {
        return;
    }
    let title = board.create_title.text.trim().to_owned();
    let summary = board.create_summary.text.trim().to_owned();
    let hypothesis = board.create_hypothesis.text.trim().to_owned();
    if title.is_empty()
        || title.len() > 200
        || summary.is_empty()
        || summary.len() > 4_000
        || hypothesis.is_empty()
        || hypothesis.len() > 4_000
    {
        board.create_error =
            "Enter a title (up to 200 characters), summary, and hypothesis (up to 4,000 characters each)."
                .to_owned();
        return;
    }
    let request = CreateRequest {
        workspace_id: workspace_id.to_string(),
        title,
        summary,
        hypothesis,
    };
    board.create_error.clear();
    board.create_busy = true;
    let generation = board.create_generation;
    let Some(handle) = crate::AUTH_HANDLE.get().cloned() else {
        board.create_busy = false;
        board.create_error = "Desktop connection is not ready.".to_owned();
        return;
    };
    std::thread::spawn(move || {
        let result = (|| {
            let addr = crate::connection::configured_addr()
                .map_err(|_| "Invalid local app-server address".to_owned())?;
            let client = crate::workspaces::client()?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize Findings connection".to_owned())?;
            runtime.block_on(post_create(&client, addr, &request))
        })();
        handle.update(move |state, _| {
            if state.workspace_id != Some(workspace_id)
                || state.workspace_findings.create_generation != generation
                || !state.workspace_findings.create_open
            {
                return;
            }
            state.workspace_findings.create_busy = false;
            match result {
                Ok(_) => {
                    close_create(state);
                    start_load(state, workspace_id);
                }
                Err(error) => state.workspace_findings.create_error = error,
            }
        });
    });
}

const LANES: [(Status, &str, &str); 4] = [
    (Status::Candidate, "Candidate", "#a0a0a0"),
    (Status::Validated, "Validated", "#7aa9e6"),
    (Status::Screened, "Screened", "#c6a976"),
    (Status::Reported, "Reported", "#77bf93"),
];

fn date_label(ms: u64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms.min(i64::MAX as u64) as i64)
        .map(|date| {
            date.with_timezone(&chrono::Local)
                .format("%b %-d, %Y")
                .to_string()
        })
        .unwrap_or_else(|| "Unknown date".to_owned())
}

/// Impact bands for the recorded screening score. Colour follows the score the record holds;
/// unscored records stay neutral rather than implying a severity.
fn impact_color(score: Option<u8>, theme: ThemeStyle<'_>) -> Color {
    match score {
        Some(score) if score >= HIGH_IMPACT => theme.color("#e07676"),
        Some(score) if score >= MEDIUM_IMPACT => theme.color("#dca15f"),
        _ => theme.color("#a4a4a4"),
    }
}

const HIGH_IMPACT: u8 = 70;
const MEDIUM_IMPACT: u8 = 40;

fn impact_chip(record: &Summary, theme: ThemeStyle<'_>) -> El {
    let color = impact_color(record.impact_score, theme);
    let label = record
        .impact_score
        .map_or("Unscored".to_owned(), |score| format!("Impact {score}"));
    row()
        .key(format!("finding-row-{}-impact", record.id))
        .h(20.0)
        .px(7.0)
        .gap(5.0)
        .center_y()
        .radius(6.0)
        .continuous_corners()
        .fill(color.alpha(0.12))
        .child(el().size(6.0, 6.0).radius(3.0).fill(color))
        .child(text(label).font(ui_font(11.0, 600.0)).color(color))
}

fn finding_meta(glyph: &'static rune::Icon, label: String, color: Color) -> El {
    row()
        .gap(4.0)
        .center_y()
        .child(icon(glyph, 12.0).color(color))
        .child(text(label).font(ui_font(11.0, 500.0)).color(color))
}

fn finding_row(record: &Summary, theme: ThemeStyle<'_>) -> El {
    let id = record.id.clone();
    let surface_count = record.surface_ids.len();
    let muted = theme.color("#858585");
    let mut header = row()
        .w_full()
        .gap(8.0)
        .center_y()
        .child(impact_chip(record, theme));
    if let Some(reachability) = record.reachability_score {
        header = header.child(
            text(format!("Reach {reachability}"))
                .font(ui_font(11.0, 500.0))
                .color(theme.color("#999999")),
        );
    }
    header = header.child(el().grow(1.0)).child(
        text(record.id.clone())
            .font(ui_font(11.0, 600.0))
            .color(theme.color("#c0c0c0")),
    );
    // Past Candidate, a record without evidence is a gap worth seeing on the board.
    let evidence_gap = record.evidence_count == 0 && record.status != Status::Candidate;
    let evidence_label = match record.evidence_count {
        0 => "No evidence".to_owned(),
        1 => "1 evidence ref".to_owned(),
        count => format!("{count} evidence refs"),
    };
    col()
        .key(format!("finding-row-{}", record.id))
        .role("button")
        .label(format!(
            "Open {}: {}, {}, {}",
            record.id,
            record.title,
            record
                .impact_score
                .map_or("unscored".to_owned(), |score| format!("impact {score}")),
            evidence_label.to_lowercase()
        ))
        .w_full()
        .padding(12.0)
        .gap(7.0)
        .hover(theme.color("#242424"))
        .child(header)
        .child(
            text(record.title.clone())
                .w_full()
                .wrap()
                .font(ui_font(14.0, 550.0))
                .color(theme.color("#ededed")),
        )
        .child(
            text(record.summary.clone())
                .w_full()
                .wrap()
                .font(ui_font(12.0, 400.0))
                .color(theme.color("#a4a4a4")),
        )
        .child(
            row()
                .w_full()
                .gap(12.0)
                .center_y()
                .child(
                    finding_meta(&WORKSPACE_SURFACES, surface_count.to_string(), muted).help(
                        format!(
                            "{surface_count} {}",
                            if surface_count == 1 {
                                "surface"
                            } else {
                                "surfaces"
                            }
                        ),
                    ),
                )
                .child(
                    finding_meta(
                        &EVIDENCE,
                        if record.evidence_count == 0 {
                            "None".to_owned()
                        } else {
                            record.evidence_count.to_string()
                        },
                        if evidence_gap {
                            theme.color("#dca15f")
                        } else {
                            muted
                        },
                    )
                    .help(evidence_label),
                )
                .child(el().grow(1.0))
                .child(
                    text(date_label(record.updated_at_ms))
                        .font(ui_font(11.0, 400.0))
                        .color(muted),
                ),
        )
        .on_press(move |state: &mut State| open_record(state, id.clone()))
}

fn finding_separator(before: &str, theme: ThemeStyle<'_>) -> El {
    el().key(format!("finding-separator-before-{before}"))
        .w_full()
        .h(1.0)
        .fill(theme.color("#303030"))
}

fn lane(
    state: &State,
    lane_index: usize,
    stage: (Status, &str, &str),
    records: &[Summary],
    size: [f32; 2],
    theme: ThemeStyle<'_>,
) -> El {
    let (status, label, accent) = stage;
    let [width, height] = size;
    let items: Vec<_> = records
        .iter()
        .filter(|item| item.status == status)
        .collect();
    let count = items.len();
    let scroll_watch = state.findings_lane_scrolls[lane_index].clone();
    let scroll_bar = scroll_watch.clone();
    let mut list = col().w_full();
    if items.is_empty() {
        list = list.child(
            text("No findings in this stage")
                .padding(12.0)
                .font(ui_font(12.0, 400.0))
                .color(theme.color("#777777")),
        );
    }
    for (index, item) in items.into_iter().enumerate() {
        if index > 0 {
            list = list.child(finding_separator(&item.id, theme));
        }
        list = list.child(finding_row(item, theme));
    }
    col()
        .key(format!("findings-lane-{}", label.to_lowercase()))
        .w(width)
        .h(height)
        .min_h(0.0)
        .child(
            row()
                .key(format!("findings-lane-{}-header", label.to_lowercase()))
                .w_full()
                .h(42.0)
                .px(12.0)
                .center_y()
                .gap(8.0)
                .child(
                    icon(&FINDING_DIAMOND, 9.0)
                        .key(format!("findings-lane-{}-diamond", label.to_lowercase()))
                        .color(theme.color(accent)),
                )
                .child(
                    text(label)
                        .font(ui_font(14.0, 600.0))
                        .color(theme.color("#dedede")),
                )
                .child(el().grow(1.0))
                .child(
                    text(count.to_string())
                        .font(ui_font(12.0, 500.0))
                        .color(theme.color("#999999")),
                ),
        )
        .child(el().w_full().h(1.0).fill(theme.color("#303030")))
        .child(
            row()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .child(
                    col()
                        .key(scroll_watch.key())
                        .grow(1.0)
                        .min_w(0.0)
                        .min_h(0.0)
                        .scroll(ScrollSpec {
                            stick_bottom: false,
                        })
                        .scroll_edge_fade(16.0)
                        .on_scroll_offset(move |position| scroll_watch.watch_scroll(position))
                        .on_wheel(move |state: &mut State, event, tx| {
                            state.findings_lane_scrolls[lane_index].wheel(event, tx)
                        })
                        .child(list),
                )
                .child(scroll_bar.bar(theme)),
        )
}

pub(crate) fn mode_toggle(state: &State, labeled: bool, theme: ThemeStyle<'_>) -> El {
    let board = &state.workspace_findings;
    let ledger = if labeled {
        crate::design::segmented_tab(
            "Ledger",
            Some(&SESSION_SIDENAV),
            board.mode == FindingsMode::Ledger,
            true,
            theme,
        )
    } else {
        crate::design::segmented_icon_tab(
            "Ledger",
            &SESSION_SIDENAV,
            board.mode == FindingsMode::Ledger,
            theme,
        )
    };
    let plot = if labeled {
        crate::design::segmented_tab(
            "Plot",
            Some(&TRACKING),
            board.mode == FindingsMode::Plot,
            true,
            theme,
        )
    } else {
        crate::design::segmented_icon_tab(
            "Plot",
            &TRACKING,
            board.mode == FindingsMode::Plot,
            theme,
        )
    };
    crate::design::segmented_control(theme)
        .key("findings-view-toggle")
        .min_w(mode_toggle_width(labeled))
        .child(
            ledger
                .key("findings-mode-ledger")
                .on_press(|state: &mut State| state.workspace_findings.mode = FindingsMode::Ledger),
        )
        .child(
            plot.key("findings-mode-plot")
                .on_press(|state: &mut State| state.workspace_findings.mode = FindingsMode::Plot),
        )
}

pub(crate) fn mode_toggle_width(labeled: bool) -> f32 {
    if labeled {
        crate::design::segmented_group_width(&[
            crate::design::segmented_tab_width("Ledger", true),
            crate::design::segmented_tab_width("Plot", true),
        ])
    } else {
        crate::design::segmented_group_width(&[crate::design::CONTROL_HEIGHT; 2])
    }
}

pub(crate) fn render(
    state: &State,
    theme: ThemeStyle<'_>,
    page_width: f32,
    page_height: f32,
) -> El {
    let board = &state.workspace_findings;
    let rejected: Vec<_> = board
        .records
        .iter()
        .filter(|item| item.status == Status::Rejected)
        .collect();
    let mut page = col()
        .key("workspace-findings-view")
        .w_full()
        .min_w(0.0)
        .gap(15.0);
    if !board.error.is_empty() {
        page = page
            .child(
                text(board.error.clone())
                    .key("findings-error")
                    .font(ui_font(13.0, 400.0))
                    .color(theme.color("#e07676")),
            )
            .child(form_button("Retry", theme).key("findings-retry").on_press(
                |state: &mut State| {
                    if let Some(id) = state.workspace_id {
                        start_load(state, id);
                    }
                },
            ));
    }
    if board.loading && !board.loaded {
        return page.child(
            text("Loading workspace findings…")
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#969696")),
        );
    }
    if board.mode == FindingsMode::Plot {
        page = page.child(plot::render(state, theme, page_width));
    } else {
        let columns = if page_width >= 920.0 {
            4
        } else if page_width >= 460.0 {
            2
        } else {
            1
        };
        let width = ((page_width - (columns - 1) as f32) / columns as f32).max(180.0);
        let reserved = 25.0 + 24.0 + 56.0 + 14.0 + if rejected.is_empty() { 0.0 } else { 53.0 };
        let available = (page_height - reserved).max(280.0);
        let rows = 4 / columns;
        let lane_height = ((available - (rows - 1) as f32) / rows as f32).max(280.0);
        let mut lanes = col().key("findings-lanes").w_full();
        for (group_index, group) in LANES.chunks(columns).enumerate() {
            if group_index > 0 {
                lanes = lanes.child(
                    el().key(format!("findings-row-divider-{group_index}"))
                        .w_full()
                        .h(1.0)
                        .fill(theme.color("#303030")),
                );
            }
            let mut line = row().w_full();
            for (index, &(status, label, accent)) in group.iter().enumerate() {
                if index > 0 {
                    line = line.child(
                        el().key(format!("findings-lane-divider-{group_index}-{index}"))
                            .w(1.0)
                            .h(lane_height)
                            .fill(theme.color("#303030")),
                    );
                }
                line = line.child(lane(
                    state,
                    group_index * columns + index,
                    (status, label, accent),
                    &board.records,
                    [width, lane_height],
                    theme,
                ));
            }
            lanes = lanes.child(line);
        }
        page = page.child(lanes);
    }
    if !rejected.is_empty() {
        let count = rejected.len();
        page = page.child(
            row()
                .key("findings-rejected-toggle")
                .role("button")
                .label(if board.rejected_expanded {
                    "Hide rejected findings"
                } else {
                    "Show rejected findings"
                })
                .w_full()
                .h(38.0)
                .px(12.0)
                .center_y()
                .gap(8.0)
                .radius(10.0)
                .continuous_corners()
                .fill(theme.color("#171717"))
                .hover(theme.color("#222222"))
                .child(
                    text(format!("Rejected · {count}"))
                        .font(ui_font(13.0, 500.0))
                        .color(theme.color("#999999")),
                )
                .child(el().grow(1.0))
                .child(
                    text(if board.rejected_expanded {
                        "Hide"
                    } else {
                        "Show"
                    })
                    .font(ui_font(12.0, 400.0))
                    .color(theme.color("#8a8a8a")),
                )
                .on_press(|state: &mut State| {
                    state.workspace_findings.rejected_expanded =
                        !state.workspace_findings.rejected_expanded
                }),
        );
        if board.rejected_expanded {
            let mut list = col().key("findings-rejected-list").w_full();
            for (index, item) in rejected.into_iter().enumerate() {
                if index > 0 {
                    list = list.child(finding_separator(&item.id, theme));
                }
                list = list.child(finding_row(item, theme));
            }
            page = page.child(list);
        }
    }
    page
}

fn detail_line(label: &str, value: &str, theme: ThemeStyle<'_>) -> El {
    col()
        .w_full()
        .gap(3.0)
        .child(
            text(label)
                .font(ui_font(11.0, 600.0))
                .color(theme.color("#8d8d8d")),
        )
        .child(
            text(value.to_owned())
                .w_full()
                .wrap()
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#dedede")),
        )
}

fn create_field(
    edit: &TextEdit,
    label: &'static str,
    key: &'static str,
    placeholder: &'static str,
    single_line: bool,
    theme: ThemeStyle<'_>,
    update: impl for<'a> Fn(&'a mut State) -> &'a mut TextEdit + 'static,
) -> El {
    let mut input = text_input(edit, ui_font(14.0, 400.0))
        .key(key)
        .role("textbox")
        .label(label)
        .clip_rounded(8.0)
        .w_full()
        .h(if single_line { 36.0 } else { 84.0 })
        .px(10.0)
        .py(8.0)
        .radius(8.0)
        .continuous_corners()
        .fill(theme.color("#282828"))
        .color(theme.color("#e2e2e2"))
        .placeholder(placeholder, theme.color("#888888"))
        .on_edit(update);
    if single_line {
        input = input.single_line();
    }
    col()
        .w_full()
        .gap(6.0)
        .child(
            text(label)
                .font(ui_font(13.0, 550.0))
                .color(theme.color("#bdbdbd")),
        )
        .child(input)
}

fn create_dialog(state: &State, theme: ThemeStyle<'_>) -> [El; 2] {
    let board = &state.workspace_findings;
    let mut body = col()
        .w_full()
        .max_h(560.0)
        .gap(14.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(create_field(
            &board.create_title,
            "Title",
            "finding-create-title",
            "Short finding title",
            true,
            theme,
            |state| &mut state.workspace_findings.create_title,
        ))
        .child(create_field(
            &board.create_summary,
            "Summary",
            "finding-create-summary",
            "What has been observed or proposed?",
            false,
            theme,
            |state| &mut state.workspace_findings.create_summary,
        ))
        .child(create_field(
            &board.create_hypothesis,
            "Candidate hypothesis",
            "finding-create-hypothesis",
            "What should research test?",
            false,
            theme,
            |state| &mut state.workspace_findings.create_hypothesis,
        ));
    if !board.create_error.is_empty() {
        body = body.child(
            text(board.create_error.clone())
                .key("finding-create-error")
                .wrap()
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#e07676")),
        );
    }
    let add = form_button(
        if board.create_busy {
            "Adding…"
        } else {
            "Add Finding"
        },
        theme,
    )
    .key("finding-create-submit");
    let actions = row()
        .w_full()
        .gap(8.0)
        .child(el().grow(1.0))
        .child(
            form_button("Cancel", theme)
                .key("finding-create-cancel")
                .on_press(close_create),
        )
        .child(if board.create_busy {
            add
        } else {
            add.on_press(start_create)
        });
    crate::modal_dialog::render(
        crate::modal_dialog::DialogSpec {
            key: "finding-create-dialog",
            title: "Add a Finding",
            width: 560.0,
            focus_return: "workspace-add-finding",
        },
        body,
        actions,
        theme,
        close_create,
    )
}

pub(crate) fn dialog(state: &State, theme: ThemeStyle<'_>) -> Option<[El; 2]> {
    let board = &state.workspace_findings;
    if board.create_open {
        return Some(create_dialog(state, theme));
    }
    let id = board.selected_id.as_deref()?;
    let mut body = col()
        .w_full()
        .max_h(560.0)
        .min_h(0.0)
        .gap(14.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    if let Some(record) = board.selected.as_ref().filter(|record| record.id == id) {
        body = body
            .child(
                text(record.title.clone())
                    .w_full()
                    .wrap()
                    .font(ui_font(17.0, 550.0))
                    .color(theme.color("#ededed")),
            )
            .child(detail_line("Summary", &record.summary, theme))
            .child(detail_line(
                "Surfaces",
                &record.surface_ids.join(" · "),
                theme,
            ))
            .child(detail_line(
                "Candidate hypothesis",
                &record.state.candidate.hypothesis,
                theme,
            ));
        if let Some(root) = &record.root_cause {
            body = body.child(detail_line("Root cause", &root.summary, theme));
        }
        if let Some(path) = &record.attack_path {
            body = body
                .child(detail_line("Attack path", &path.summary, theme))
                .child(detail_line(
                    "Reachability",
                    &path.reachability.summary,
                    theme,
                ))
                .child(detail_line(
                    "Attacker role",
                    &path.reachability.attacker_role,
                    theme,
                ))
                .child(detail_line(
                    "Entrypoint",
                    &path.reachability.entrypoint,
                    theme,
                ))
                .child(detail_line(
                    "Source → sink",
                    &format!("{} → {}", path.reachability.source, path.reachability.sink),
                    theme,
                ));
        }
        if let Some(validation) = &record.state.validated {
            body = body.child(detail_line(
                "Validation",
                &validation.validation_result,
                theme,
            ));
        }
        if let Some(screening) = &record.state.screened {
            body = body
                .child(detail_line(
                    "Impact score",
                    &screening
                        .impact_score
                        .map_or("Unrated".to_owned(), |score| format!("{score} / 100")),
                    theme,
                ))
                .child(detail_line(
                    "Reachability score",
                    &screening
                        .reachability_score
                        .map_or("Unrated".to_owned(), |score| format!("{score} / 100")),
                    theme,
                ));
            body = body.child(detail_line(
                "Screening",
                &screening.scope_review.conclusion,
                theme,
            ));
        }
        if let Some(report) = &record.state.reported {
            body = body.child(detail_line("Report", &report.report_reference, theme));
        }
        if let Some(rejected) = &record.state.rejected {
            body = body.child(detail_line("Rejection reason", &rejected.reason, theme));
        }
        if let Some(remediation) = &record.remediation {
            body = body.child(detail_line("Remediation", remediation, theme));
        }
        body = body.child(detail_line(
            "Updated",
            &date_label(record.updated_at_ms),
            theme,
        ));
    } else if board.detail_loading {
        body = body.child(
            text("Loading finding…")
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#969696")),
        );
    } else {
        body = body.child(
            text(board.detail_error.clone())
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#e07676")),
        );
    }
    let actions = row().w_full().child(el().grow(1.0)).child(
        form_button("Close", theme)
            .key("finding-detail-close")
            .on_press(close_record),
    );
    Some(crate::modal_dialog::render(
        crate::modal_dialog::DialogSpec {
            key: "finding-detail-dialog",
            title: "Finding details",
            width: 640.0,
            focus_return: "workspace-tab-findings",
        },
        body,
        actions,
        theme,
        close_record,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PrimaryView, WorkspaceSummary, workspace_view::WorkspaceTab};
    use rune::{Env, Runtime};

    #[test]
    fn older_findings_request_cannot_replace_a_refreshed_ledger() {
        let mut board = FindingsBoard {
            generation: 2,
            loading: true,
            ..FindingsBoard::default()
        };
        board.finish_load(1, Ok(Vec::new()));
        assert!(board.loading);
        assert!(!board.loaded);
        board.finish_load(2, Ok(Vec::new()));
        assert!(!board.loading);
        assert!(board.loaded);
        board.reset();
        board.finish_load(2, Ok(Vec::new()));
        assert!(!board.loaded);
    }

    #[test]
    fn findings_tab_shows_four_lanes_and_keeps_rejected_outside_them() {
        let workspace_id = uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000073").unwrap();
        let mut state = State {
            primary_view: PrimaryView::Workspace,
            workspace_id: Some(workspace_id),
            workspace_tab: WorkspaceTab::Findings,
            workspaces: vec![WorkspaceSummary {
                id: workspace_id.to_string(),
                name: "Example Workspace".to_owned(),
                directory: "example-directory".to_owned(),
            }],
            features: crate::extensions_view::bundled_features(),
            ..State::default()
        };
        state
            .features
            .iter_mut()
            .find(|feature| feature.id == beale_findings::FEATURE_ID)
            .unwrap()
            .enabled = true;
        state
            .features
            .iter_mut()
            .find(|feature| feature.id == beale_surfaces::FEATURE_ID)
            .unwrap()
            .enabled = true;
        state.workspace_findings.loaded = true;
        for (index, status) in [
            Status::Candidate,
            Status::Validated,
            Status::Screened,
            Status::Reported,
            Status::Rejected,
        ]
        .into_iter()
        .enumerate()
        {
            state.workspace_findings.records.push(Summary {
                impact_score: None,
                reachability_score: None,
                id: format!("F{}", index + 1),
                title: format!("Example finding {}", index + 1),
                summary: "Synthetic UI example.".to_owned(),
                surface_ids: vec!["S1".to_owned()],
                status,
                updated_at_ms: 1_700_000_000_000,
                revision: 1,
                evidence_count: 0,
                verified: false,
            });
        }
        let mut runtime = Runtime::new(state, crate::view, (1800.0, 900.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let tabs = [
            "workspace-tab-surfaces",
            "workspace-tab-findings",
            "workspace-tab-rules",
        ];
        let positions = tabs.map(|key| runtime.presented(key).unwrap().0[0]);
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
        let (header, _) = runtime.presented("workspace-view-header").unwrap();
        let (title, _) = runtime.presented("workspace-title-squircle").unwrap();
        let (title_text, _) = runtime.presented("workspace-view-title").unwrap();
        let (add, _) = runtime.presented("workspace-add-finding").unwrap();
        let (toggle, _) = runtime.presented("findings-view-toggle").unwrap();
        let (workspace_tabs, _) = runtime.presented("workspace-view-tabs").unwrap();
        let (findings_page, _) = runtime.presented("workspace-findings-view").unwrap();
        assert!((0.0..=24.0).contains(&(toggle[0] - title[0] - title[2])));
        assert!((0.0..=24.0).contains(&(toggle[0] - title_text[0] - title_text[2])));
        assert!(toggle[0] + toggle[2] < add[0]);
        assert!(((add[1] + add[3] / 2.0) - (toggle[1] + toggle[3] / 2.0)).abs() < 1.0);
        assert!((title_text[1] + title_text[3] / 2.0 - toggle[1] - toggle[3] / 2.0).abs() < 2.0);
        assert!(add[0] + add[2] < workspace_tabs[0]);
        assert!(toggle[1] >= header[1] && toggle[1] + toggle[3] <= header[1] + header[3]);
        assert!(findings_page[1] >= header[1] + header[3]);
        assert!(!runtime.elements().iter().any(|element| {
            element.text.as_deref() == Some("Findings")
                && element.rect[0] < toggle[0]
                && element.rect[1] >= findings_page[1]
        }));
        assert!(!runtime.elements().iter().any(|element| {
            element
                .text
                .as_deref()
                .is_some_and(|value| value.contains(" active"))
        }));
        for lane in ["candidate", "validated", "screened", "reported"] {
            assert!(runtime.find(&format!("findings-lane-{lane}")).is_some());
            assert!(
                runtime
                    .find(&format!("findings-lane-{lane}-diamond"))
                    .is_some()
            );
            assert!(
                runtime
                    .find(&format!("findings-lane-{lane}-scroll"))
                    .is_some()
            );
        }
        assert!(runtime.find("findings-lane-divider-0-1").is_some());
        assert!(runtime.find("findings-lane-rejected").is_none());
        assert!(runtime.find("finding-row-F5").is_none());
        let (x, y) = runtime.find("findings-rejected-toggle").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0 / 60.0);
        assert!(runtime.find("finding-row-F5").is_some());
    }

    #[test]
    fn compact_findings_and_workspace_tabs_center_their_icons() {
        let assert_balanced_padding = |runtime: &Runtime<State>| {
            for (group, first, last) in [
                (
                    "findings-view-toggle",
                    "findings-mode-ledger",
                    "findings-mode-plot",
                ),
                (
                    "workspace-view-tabs",
                    "workspace-tab-surfaces",
                    "workspace-tab-settings",
                ),
            ] {
                let (group_rect, _) = runtime.presented(group).unwrap();
                let (first_rect, _) = runtime.presented(first).unwrap();
                let (last_rect, _) = runtime.presented(last).unwrap();
                let left = first_rect[0] - group_rect[0];
                let right = group_rect[0] + group_rect[2] - last_rect[0] - last_rect[2];
                assert!(
                    (left - right).abs() < 1.0,
                    "{group} has uneven padding: left={left}, right={right}"
                );
            }
        };
        let state = crate::visual_preview::fixture("findings");
        let mut narrow = Runtime::new(state, crate::view, (840.0, 800.0), Env::new(true, 1.0));
        narrow.frame(0.0);
        for (button_key, icon_key) in [
            ("findings-mode-ledger", "segmented-tab-icon-ledger"),
            ("findings-mode-plot", "segmented-tab-icon-plot"),
            ("workspace-tab-findings", "segmented-tab-icon-findings"),
            ("workspace-tab-rules", "segmented-tab-icon-rules"),
            ("workspace-tab-settings", "segmented-tab-icon-settings"),
        ] {
            let (button, _) = narrow.presented(button_key).unwrap();
            let (glyph, _) = narrow.presented(icon_key).unwrap();
            assert_eq!(button[2], crate::design::CONTROL_HEIGHT, "{button_key}");
            let button_center = button[0] + button[2] / 2.0;
            let glyph_center = glyph[0] + glyph[2] / 2.0;
            assert!(
                (button_center - glyph_center).abs() < 1.0,
                "{button_key} icon is off center: {button:?}, {glyph:?}"
            );
        }
        let (toggle, _) = narrow.presented("findings-view-toggle").unwrap();
        let (add, _) = narrow.presented("workspace-add-finding").unwrap();
        let (title, _) = narrow.presented("workspace-title-squircle").unwrap();
        let (title_text, _) = narrow.presented("workspace-view-title").unwrap();
        let (tabs, _) = narrow.presented("workspace-view-tabs").unwrap();
        assert!((0.0..=24.0).contains(&(toggle[0] - title[0] - title[2])));
        assert!((0.0..=24.0).contains(&(toggle[0] - title_text[0] - title_text[2])));
        assert!(toggle[0] + toggle[2] < add[0]);
        assert!(((add[1] + add[3] / 2.0) - (toggle[1] + toggle[3] / 2.0)).abs() < 1.0);
        assert!((title_text[1] + title_text[3] / 2.0 - toggle[1] - toggle[3] / 2.0).abs() < 2.0);
        assert!(add[0] + add[2] < tabs[0]);
        assert_balanced_padding(&narrow);

        let state = crate::visual_preview::fixture("findings");
        let mut wide = Runtime::new(state, crate::view, (1440.0, 800.0), Env::new(true, 1.0));
        wide.frame(0.0);
        assert_balanced_padding(&wide);
        for key in ["findings-mode-ledger", "findings-mode-plot"] {
            let (button, _) = wide.presented(key).unwrap();
            assert!(button[2] > crate::design::CONTROL_HEIGHT, "{key}");
        }
    }

    #[test]
    fn add_finding_header_opens_candidate_form_and_checks_required_fields() {
        let state = crate::visual_preview::fixture("findings");
        let mut runtime = Runtime::new(state, crate::view, (1440.0, 800.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let click = |runtime: &mut Runtime<State>, key: &str| {
            let (x, y) = runtime.find(key).unwrap();
            runtime.pointer_down(x, y);
            runtime.pointer_up(x, y);
            runtime.frame(1.0);
        };
        click(&mut runtime, "workspace-add-finding");
        assert!(runtime.find("finding-create-dialog").is_some());
        click(&mut runtime, "finding-create-submit");
        assert!(runtime.find("finding-create-error").is_some());
        assert!(!runtime.state.workspace_findings.create_busy);
        click(&mut runtime, "finding-create-cancel");
        assert!(runtime.find("finding-create-dialog").is_none());
    }

    #[test]
    fn ledger_lanes_scroll_independently() {
        let mut state = crate::visual_preview::fixture("findings");
        let example = state
            .workspace_findings
            .records
            .iter()
            .find(|record| record.status == Status::Candidate)
            .unwrap()
            .clone();
        for index in 0..18 {
            state.workspace_findings.records.push(Summary {
                id: format!("F-example-{index}"),
                title: format!("Example candidate {index}"),
                ..example.clone()
            });
        }
        let mut runtime = Runtime::new(state, crate::view, (1440.0, 900.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        runtime.frame(0.0);
        assert!(runtime.state.findings_lane_scrolls[0].position().max[1] > 0.0);
        assert!(
            runtime
                .find("finding-separator-before-F-example-0")
                .is_some()
        );
        assert!(runtime.find("finding-separator-before-F6").is_none());
        let (viewport, _) = runtime.presented("findings-lane-candidate-scroll").unwrap();
        runtime.wheel_event(rune::WheelEvent::new(0.0, -120.0).at(
            viewport[0] + viewport[2] / 2.0,
            viewport[1] + viewport[3] / 2.0,
        ));
        runtime.frame(1.0 / 60.0);
        assert!(runtime.state.findings_lane_scrolls[0].position().offset[1] > 0.0);
        assert_eq!(
            runtime.state.findings_lane_scrolls[1].position().offset[1],
            0.0
        );

        let mut narrow = Runtime::new(
            crate::visual_preview::fixture("findings"),
            crate::view,
            (840.0, 800.0),
            Env::new(true, 1.0),
        );
        narrow.frame(0.0);
        assert!(narrow.find("findings-row-divider-1").is_some());
        assert!(narrow.find("findings-lane-divider-0-1").is_some());
    }

    #[test]
    fn ledger_lanes_reach_the_window_bottom() {
        for height in [800.0, 1200.0] {
            let mut state = crate::visual_preview::fixture("findings");
            state
                .workspace_findings
                .records
                .retain(|record| record.status != Status::Rejected);
            let mut runtime =
                Runtime::new(state, crate::view, (1440.0, height), Env::new(true, 1.0));
            runtime.frame(0.0);
            runtime.frame(1.0);
            let (content, _) = runtime.presented("content-surface").unwrap();
            let (lanes, _) = runtime.presented("findings-lanes").unwrap();
            let gap = content[1] + content[3] - lanes[1] - lanes[3];
            assert!(
                (0.0..=45.0).contains(&gap),
                "height {height}: bottom gap {gap}"
            );
        }
    }
    fn click(runtime: &mut Runtime<State>, key: &str) {
        runtime.update(|_, tx| {
            tx.instant();
            tx.scroll_into_view(key);
        });
        runtime.frame(1.0);
        let (x, y) = runtime.find(key).unwrap_or_else(|| panic!("missing {key}"));
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
    }

    #[test]
    fn plot_toggle_groups_matching_scores_and_keeps_unrated_and_rejected_accessible() {
        let state = crate::visual_preview::fixture("findings");
        let mut runtime = Runtime::new(state, crate::view, (1440.0, 1100.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        assert!(runtime.find("findings-lanes").is_some());
        assert!(runtime.find("findings-refresh").is_none());
        click(&mut runtime, "findings-mode-plot");
        assert_eq!(runtime.state.workspace_findings.mode, FindingsMode::Plot);
        assert!(runtime.find("findings-plot").is_some());
        assert!(runtime.find("findings-lanes").is_none());
        let points = runtime
            .elements()
            .into_iter()
            .filter(|el| {
                el.key.as_deref().is_some_and(|key| {
                    key.starts_with("finding-point-") && !key.ends_with("-diamond")
                })
            })
            .count();
        assert_eq!(points, 7);
        assert!(runtime.find("finding-point-88-86-diamond").is_some());
        assert!(
            runtime
                .find("findings-plot-stage-candidate-diamond")
                .is_some()
        );
        assert!(runtime.find("finding-point-0-0").is_some());
        assert!(runtime.find("finding-point-100-100").is_some());
        assert!(runtime.find("finding-point-90-90").is_none());
        click(&mut runtime, "finding-point-70-32");
        assert_eq!(
            runtime.state.workspace_findings.selected_id.as_deref(),
            Some("F2")
        );
        close_record(&mut runtime.state);
        runtime.frame(1.0);
        click(&mut runtime, "finding-point-88-86");
        assert_eq!(
            runtime.state.workspace_findings.plot_selection,
            Some((88, 86))
        );
        assert!(runtime.find("finding-row-F1").is_some());
        assert!(runtime.find("finding-row-F8").is_some());
        assert!(runtime.find("finding-separator-before-F8").is_some());
        click(&mut runtime, "finding-row-F1");
        assert_eq!(
            runtime.state.workspace_findings.selected_id.as_deref(),
            Some("F1")
        );
        assert!(runtime.find("finding-detail-dialog").is_some());
        close_record(&mut runtime.state);
        runtime.frame(1.0);
        click(&mut runtime, "findings-plot-selection-close");
        click(&mut runtime, "findings-unrated-toggle");
        assert!(runtime.find("finding-row-F6").is_some());
        assert!(runtime.find("finding-row-F7").is_some());
        click(&mut runtime, "findings-rejected-toggle");
        assert!(runtime.find("finding-row-F11").is_some());
        click(&mut runtime, "findings-mode-ledger");
        assert_eq!(runtime.state.workspace_findings.mode, FindingsMode::Ledger);
        assert!(runtime.find("findings-lanes").is_some());
    }

    #[test]
    fn plot_axes_and_toggle_fit_all_themes_and_larger_text() {
        for theme in [
            crate::AppearanceTheme::Light,
            crate::AppearanceTheme::Dark,
            crate::AppearanceTheme::Cream,
            crate::AppearanceTheme::Midnight,
        ] {
            for (width, large) in [(1440.0, false), (840.0, false), (840.0, true)] {
                let mut state = crate::visual_preview::fixture("findings-plot");
                state.appearance.theme = theme;
                if large {
                    state.appearance.text_size = crate::TextSize::ExtraLarge;
                }
                let mut runtime =
                    Runtime::new(state, crate::view, (width, 1200.0), Env::new(true, 1.0));
                runtime.frame(0.0);
                let (page, _) = runtime.presented("workspace-findings-view").unwrap();
                for key in [
                    "findings-plot",
                    "findings-plot-legend",
                    "findings-plot-stage-candidate",
                    "findings-plot-stage-validated",
                    "findings-plot-stage-screened",
                    "findings-plot-stage-reported",
                    "finding-point-0-0",
                    "finding-point-100-100",
                ] {
                    let (bounds, _) = runtime.presented(key).unwrap();
                    assert!(
                        bounds[0] >= page[0] && bounds[0] + bounds[2] <= page[0] + page[2] + 1.0,
                        "{key} overflows {width}, large={large}: {bounds:?}, page={page:?}"
                    );
                }
                let (toggle, _) = runtime.presented("findings-view-toggle").unwrap();
                let (tabs, _) = runtime.presented("workspace-view-tabs").unwrap();
                assert!(toggle[0] + toggle[2] <= tabs[0]);
                let (low, _) = runtime.presented("finding-point-0-0").unwrap();
                let (high, _) = runtime.presented("finding-point-100-100").unwrap();
                assert!(high[0] > low[0] && high[1] < low[1]);
            }
        }
    }

    #[test]
    fn unrated_plot_does_not_invent_zero_scores() {
        for variant in ["findings-plot-empty", "findings-plot-unrated"] {
            let state = crate::visual_preview::fixture(variant);
            let mut runtime =
                Runtime::new(state, crate::view, (1440.0, 960.0), Env::new(true, 1.0));
            runtime.frame(0.0);
            assert!(runtime.find("findings-plot-empty").is_some());
            assert!(runtime.find("finding-point-0-0").is_none());
        }
    }
    #[test]
    fn plot_legend_toggles_every_stage_and_rebuilds_mixed_groups() {
        let mut state = crate::visual_preview::fixture("findings-plot");
        let mut candidate = state.workspace_findings.records[5].clone();
        candidate.id = "F12".to_owned();
        candidate.impact_score = Some(45);
        candidate.reachability_score = Some(10);
        state.workspace_findings.records.push(candidate);
        state.workspace_findings.records[7].status = Status::Reported;
        state.workspace_findings.plot_selection = Some((88, 86));
        let mut runtime = Runtime::new(state, crate::view, (1440.0, 1300.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let rated_count = |runtime: &Runtime<State>| {
            runtime
                .elements()
                .into_iter()
                .find(|el| el.key.as_deref() == Some("findings-plot-rated-count"))
                .unwrap()
                .text
                .unwrap()
        };
        assert_eq!(rated_count(&runtime), "9 rated");
        for (status, label, _) in LANES {
            assert!(runtime.state.workspace_findings.plot_stage_visible(status));
            let key = format!("findings-plot-stage-{}", label.to_lowercase());
            let control = runtime
                .elements()
                .into_iter()
                .find(|el| el.key.as_deref() == Some(key.as_str()))
                .unwrap();
            assert_eq!(control.role, "switch");
            assert!(control.label.unwrap().contains("shown"));
        }
        click(&mut runtime, "findings-plot-stage-candidate");
        assert_eq!(runtime.state.workspace_findings.plot_selection, None);
        assert!(runtime.find("finding-point-45-10").is_none());
        assert_eq!(rated_count(&runtime), "8 rated");
        click(&mut runtime, "findings-unrated-toggle");
        assert!(runtime.find("finding-row-F6").is_none());
        assert!(runtime.find("finding-row-F7").is_some());
        click(&mut runtime, "findings-plot-stage-validated");
        assert!(runtime.find("finding-point-70-32").is_none());
        assert!(runtime.find("finding-point-0-0").is_none());
        assert!(runtime.find("findings-unrated-toggle").is_none());
        assert_eq!(rated_count(&runtime), "6 rated");
        runtime.state.workspace_findings.plot_selection = Some((88, 86));
        runtime.frame(0.0);
        click(&mut runtime, "findings-plot-stage-screened");
        assert!(runtime.find("finding-point-35-78").is_none());
        assert!(runtime.find("finding-point-18-28").is_none());
        assert!(runtime.find("findings-plot-selection").is_none());
        let point = runtime
            .elements()
            .into_iter()
            .find(|el| el.key.as_deref() == Some("finding-point-88-86"))
            .unwrap();
        assert!(point.label.unwrap().starts_with("F8 ·"));
        assert_eq!(rated_count(&runtime), "3 rated");
        click(&mut runtime, "findings-plot-stage-reported");
        assert_eq!(rated_count(&runtime), "0 rated");
        assert!(runtime.elements().iter().all(|el| {
            !el.key
                .as_deref()
                .is_some_and(|key| key.starts_with("finding-point-"))
        }));
        assert!(
            runtime
                .elements()
                .iter()
                .any(|el| el.text.as_deref() == Some("No stages selected"))
        );
        for (_, label, _) in LANES {
            click(
                &mut runtime,
                &format!("findings-plot-stage-{}", label.to_lowercase()),
            );
        }
        assert_eq!(rated_count(&runtime), "9 rated");
        assert!(runtime.find("findings-plot-empty").is_none());
        let point = runtime
            .elements()
            .into_iter()
            .find(|el| el.key.as_deref() == Some("finding-point-88-86"))
            .unwrap();
        assert!(point.label.unwrap().starts_with("2 findings"));
        assert!(runtime.find("findings-rejected-toggle").is_some());
    }

    #[test]
    fn plot_stage_selection_survives_reload_and_view_switches_and_resets_with_board() {
        let state = crate::visual_preview::fixture("findings-plot");
        let mut runtime = Runtime::new(state, crate::view, (1440.0, 1100.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        click(&mut runtime, "findings-plot-stage-screened");
        let board = &mut runtime.state.workspace_findings;
        board.finish_load(board.generation, Ok(board.records.clone()));
        runtime.frame(0.0);
        assert!(runtime.find("finding-point-88-86").is_none());
        click(&mut runtime, "findings-mode-ledger");
        assert!(runtime.find("finding-row-F1").is_some());
        click(&mut runtime, "findings-mode-plot");
        assert!(
            !runtime
                .state
                .workspace_findings
                .plot_stage_visible(Status::Screened)
        );
        assert!(runtime.find("finding-point-88-86").is_none());
        runtime.state.workspace_findings.reset();
        for (status, _, _) in LANES {
            assert!(runtime.state.workspace_findings.plot_stage_visible(status));
        }
    }
}
