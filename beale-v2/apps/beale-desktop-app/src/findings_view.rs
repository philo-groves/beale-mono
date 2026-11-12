use beale_contracts::findings::{
    self, Finding, GetRequest, GetResponse, QueryPage, QueryRequest, Status, Summary,
};
use reqwest::{Client, StatusCode};
use rune::{El, ScrollSpec, col, el, row, text};
use std::net::SocketAddr;

use crate::{State, ThemeStyle, form_button, ui_font};

#[derive(Default)]
pub(crate) struct FindingsBoard {
    pub(crate) records: Vec<Summary>,
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
}

impl FindingsBoard {
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
        *self = Self::default();
        self.generation = generation;
        self.detail_generation = detail_generation;
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

fn card(record: &Summary, theme: ThemeStyle<'_>) -> El {
    let id = record.id.clone();
    let surface_count = record.surface_ids.len();
    col()
        .key(format!("finding-card-{}", record.id))
        .role("button")
        .label(format!("Open {}: {}", record.id, record.title))
        .w_full()
        .padding(13.0)
        .gap(8.0)
        .radius(12.0)
        .continuous_corners()
        .fill(theme.color("#242424"))
        .hover(theme.color("#2d2d2d"))
        .border(1.0, theme.color("#353535"))
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
                .gap(8.0)
                .child(
                    text(record.id.clone())
                        .font(ui_font(11.0, 550.0))
                        .color(theme.color("#c0c0c0")),
                )
                .child(
                    text(format!(
                        "{surface_count} {}",
                        if surface_count == 1 {
                            "surface"
                        } else {
                            "surfaces"
                        }
                    ))
                    .font(ui_font(11.0, 400.0))
                    .color(theme.color("#858585")),
                )
                .child(el().grow(1.0))
                .child(
                    text(date_label(record.updated_at_ms))
                        .font(ui_font(11.0, 400.0))
                        .color(theme.color("#858585")),
                ),
        )
        .on_press(move |state: &mut State| open_record(state, id.clone()))
}

fn lane(
    status: Status,
    label: &str,
    accent: &str,
    records: &[Summary],
    width: f32,
    theme: ThemeStyle<'_>,
) -> El {
    let items: Vec<_> = records
        .iter()
        .filter(|item| item.status == status)
        .collect();
    let mut content = col()
        .key(format!("findings-lane-{}", label.to_lowercase()))
        .w(width)
        .min_h(330.0)
        .padding(12.0)
        .gap(11.0)
        .radius(16.0)
        .continuous_corners()
        .fill(theme.color("#171717"))
        .border(1.0, theme.color("#2b2b2b"))
        .child(
            row()
                .w_full()
                .center_y()
                .gap(8.0)
                .child(el().size(7.0, 7.0).radius(3.5).fill(theme.color(accent)))
                .child(
                    text(label)
                        .font(ui_font(14.0, 600.0))
                        .color(theme.color("#dedede")),
                )
                .child(el().grow(1.0))
                .child(
                    text(items.len().to_string())
                        .font(ui_font(12.0, 500.0))
                        .color(theme.color("#999999")),
                ),
        )
        .child(el().w_full().h(1.0).fill(theme.color("#303030")));
    if items.is_empty() {
        content = content.child(
            text("No findings in this stage")
                .font(ui_font(12.0, 400.0))
                .color(theme.color("#777777")),
        );
    }
    for item in items {
        content = content.child(card(item, theme));
    }
    content
}

pub(crate) fn render(state: &State, theme: ThemeStyle<'_>, page_width: f32) -> El {
    let board = &state.workspace_findings;
    let active = board
        .records
        .iter()
        .filter(|item| item.status != Status::Rejected)
        .count();
    let rejected: Vec<_> = board
        .records
        .iter()
        .filter(|item| item.status == Status::Rejected)
        .collect();
    let mut page = col()
        .key("workspace-findings-view")
        .w_full()
        .min_w(0.0)
        .gap(15.0)
        .child(
            row()
                .w_full()
                .center_y()
                .gap(10.0)
                .child(
                    text("Findings ledger")
                        .font(ui_font(21.0, 600.0))
                        .color(theme.color("#ededed")),
                )
                .child(
                    text(format!("{active} active"))
                        .font(ui_font(12.0, 400.0))
                        .color(theme.color("#969696")),
                )
                .child(el().grow(1.0))
                .child(
                    form_button(
                        if board.loading {
                            "Loading…"
                        } else {
                            "Refresh"
                        },
                        theme,
                    )
                    .key("findings-refresh")
                    .on_press(|state: &mut State| {
                        if let Some(id) = state.workspace_id {
                            start_load(state, id);
                        }
                    }),
                ),
        );
    if !board.error.is_empty() {
        page = page.child(
            text(board.error.clone())
                .key("findings-error")
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#e07676")),
        );
    }
    if board.loading && !board.loaded {
        return page.child(
            text("Loading workspace findings…")
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#969696")),
        );
    }
    let columns = if page_width >= 920.0 {
        4
    } else if page_width >= 460.0 {
        2
    } else {
        1
    };
    let width = ((page_width - 12.0 * (columns - 1) as f32) / columns as f32).max(180.0);
    let mut lanes = col().key("findings-lanes").w_full().gap(12.0);
    for group in LANES.chunks(columns) {
        let mut line = row().w_full().gap(12.0);
        for &(status, label, accent) in group {
            line = line.child(lane(status, label, accent, &board.records, width, theme));
        }
        lanes = lanes.child(line);
    }
    page = page.child(lanes);
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
            let mut list = col()
                .key("findings-rejected-list")
                .w_full()
                .max_w(600.0)
                .gap(8.0);
            for item in rejected {
                list = list.child(card(item, theme));
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

pub(crate) fn dialog(state: &State, theme: ThemeStyle<'_>) -> Option<[El; 2]> {
    let board = &state.workspace_findings;
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
            body = body.child(detail_line("Attack path", &path.summary, theme));
        }
        if let Some(validation) = &record.state.validated {
            body = body.child(detail_line(
                "Validation",
                &validation.validation_result,
                theme,
            ));
        }
        if let Some(screening) = &record.state.screened {
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
                id: format!("F{}", index + 1),
                title: format!("Example finding {}", index + 1),
                summary: "Synthetic UI example.".to_owned(),
                surface_ids: vec!["S1".to_owned()],
                status,
                updated_at_ms: 1_700_000_000_000,
                revision: 1,
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
        for lane in ["candidate", "validated", "screened", "reported"] {
            assert!(runtime.find(&format!("findings-lane-{lane}")).is_some());
        }
        assert!(runtime.find("findings-lane-rejected").is_none());
        assert!(runtime.find("finding-card-F5").is_none());
        let (x, y) = runtime.find("findings-rejected-toggle").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0 / 60.0);
        assert!(runtime.find("finding-card-F5").is_some());
    }
}
