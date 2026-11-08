use beale_contracts::v1::{
    SessionDetail, SessionStreamEvent, SessionStreamKind, SessionTokenUsage, SessionUsageSnapshot,
};
use rune::{Align, Color, El, Icon, LineCap, Shape, Side, canvas, col, el, icon, row, text};
use std::time::Duration;

use crate::{
    State, ThemeStyle,
    icons::{ARROW_LEFT, ARROW_RIGHT, SESSION_CACHE_RATE, SESSION_TOKENS, TRACKING},
    in_place_menu, ui_font,
};

pub(crate) const WIDTH: f32 = 292.0;
pub(crate) const INSET: f32 = 12.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UsageMenu {
    Tokens,
    Cache,
}

impl UsageMenu {
    fn anchor(self) -> &'static str {
        match self {
            Self::Tokens => "session-sidenav-token-row",
            Self::Cache => "session-sidenav-cache-row",
        }
    }
}

fn open_menu(state: &mut State, menu: UsageMenu) {
    if state.session_usage_menu != Some(menu) {
        state.session_usage_menu = Some(menu);
        state.session_usage_snapshot_index = state
            .active_session
            .as_ref()
            .map_or(0, |detail| detail.usage_snapshots.len().saturating_sub(1));
    }
}

fn toggle_menu(state: &mut State, menu: UsageMenu) {
    if state.session_usage_menu == Some(menu) {
        state.session_usage_menu = None;
    } else {
        open_menu(state, menu);
    }
}

fn activates_sidenav(name: &str) -> bool {
    matches!(
        name,
        "memory_create"
            | "lead_create"
            | "finding_create"
            | "runbook_create"
            | "surface_add"
            | "graph_delegate"
    )
}

pub(crate) fn has_campaign_activity(detail: &SessionDetail) -> bool {
    detail.has_campaign_activity
        || detail.messages.iter().any(|message| {
            message
                .tools
                .iter()
                .any(|tool| !tool.is_error && activates_sidenav(&tool.name))
        })
}

pub(crate) fn campaign_activity_event(event: &SessionStreamEvent) -> bool {
    event.kind == SessionStreamKind::ToolFinished as i32
        && event
            .tool
            .as_ref()
            .is_some_and(|tool| !tool.is_error && activates_sidenav(&tool.name))
}

pub(crate) fn duration_ms(detail: &SessionDetail, live_ms: u64) -> u64 {
    detail.total_duration_ms.saturating_add(live_ms)
}

fn duration_label(ms: u64) -> String {
    let seconds = ms / 1_000;
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3_600,
        seconds / 60 % 60,
        seconds % 60
    )
}

fn token_count(value: u64) -> String {
    let digits = value.to_string();
    let mut result = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            result.push(',');
        }
        result.push(digit);
    }
    result
}

fn compact_count(value: u64) -> String {
    let (scale, suffix) = if value >= 999_950_000 {
        (1_000_000_000_u64, "b")
    } else if value >= 999_950 {
        (1_000_000_u64, "m")
    } else if value >= 1_000 {
        (1_000_u64, "k")
    } else {
        return value.to_string();
    };
    let tenths = (u128::from(value) * 10 + u128::from(scale) / 2) / u128::from(scale);
    if tenths.is_multiple_of(10) {
        format!("{}{suffix}", tenths / 10)
    } else {
        format!("{}.{:01}{suffix}", tenths / 10, tenths % 10)
    }
}

fn cache_rate(usage: &SessionTokenUsage) -> Option<String> {
    (usage.cache_input_tokens > 0).then(|| {
        let tenths = (u128::from(usage.cache_read_tokens) * 1_000
            + u128::from(usage.cache_input_tokens) / 2)
            / u128::from(usage.cache_input_tokens);
        format!("{}.{:01}%", tenths / 10, tenths % 10)
    })
}

struct UsageRowContent {
    mark: &'static Icon,
    primary: String,
    stats: String,
    accessible: String,
    key: &'static str,
    stats_key: &'static str,
}

fn usage_row(menu: UsageMenu, active: bool, content: UsageRowContent, theme: ThemeStyle<'_>) -> El {
    row()
        .key(menu.anchor())
        .w_full()
        .h(30.0)
        .px(2.0)
        .center_y()
        .gap(8.0)
        .role("group")
        .label(content.accessible)
        .radius(8.0)
        .fill(if active {
            theme.color("#303030")
        } else {
            Color::TRANSPARENT
        })
        .hover(theme.color("#303030"))
        .on_hover(move |state: &mut State| {
            state.session_usage_row_hovered = Some(menu);
            open_menu(state, menu);
        })
        .on_hover_change(move |state: &mut State, inside, _| {
            if inside {
                state.session_usage_row_hovered = Some(menu);
                open_menu(state, menu);
            } else if state.session_usage_row_hovered == Some(menu) {
                state.session_usage_row_hovered = None;
                if !state.session_usage_popup_hovered && state.session_usage_menu == Some(menu) {
                    state.session_usage_menu = None;
                }
            }
        })
        .hover_delays(Duration::ZERO, Duration::from_millis(250))
        .child(icon(content.mark, 15.0).color(theme.color("#aaaaaa")))
        .child(
            text(content.primary)
                .key(content.key)
                .font(ui_font(12.0, 400.0))
                .color(theme.color("#e7e7e7")),
        )
        .child(row().grow(1.0))
        .child(
            text(content.stats)
                .key(content.stats_key)
                .font(ui_font(11.0, 400.0))
                .color(theme.color("#929292")),
        )
}

pub(crate) fn render(
    detail: Option<&SessionDetail>,
    live_ms: u64,
    active_menu: Option<UsageMenu>,
    theme: ThemeStyle<'_>,
) -> El {
    let duration = detail.map_or(live_ms, |detail| duration_ms(detail, live_ms));
    let usage = detail.and_then(|detail| detail.token_usage.as_ref());
    let tokens = usage.map_or(0, |usage| {
        usage.input_tokens.saturating_add(usage.output_tokens)
    });
    let token_value =
        if usage.is_some_and(|usage| usage.total_calls > 0 && usage.measured_calls == 0) {
            "—".to_owned()
        } else {
            token_count(tokens)
        };
    let token_display = if token_value == "—" {
        token_value.clone()
    } else {
        compact_count(tokens)
    };
    let token_stats = usage.map_or_else(
        || "0 in, 0 out".to_owned(),
        |usage| {
            if usage.total_calls > 0 && usage.measured_calls == 0 {
                "— in, — out".to_owned()
            } else {
                format!(
                    "{} in, {} out",
                    compact_count(usage.input_tokens),
                    compact_count(usage.output_tokens)
                )
            }
        },
    );
    let token_accessible = usage.map_or_else(
        || "No measured calls yet".to_owned(),
        |usage| {
            if usage.total_calls == 0 {
                "No model calls yet".to_owned()
            } else if usage.measured_calls == 0 {
                format!("0/{} calls reported tokens", usage.total_calls)
            } else {
                format!(
                    "{} in · {} out · {}/{} calls",
                    token_count(usage.input_tokens),
                    token_count(usage.output_tokens),
                    usage.measured_calls,
                    usage.total_calls
                )
            }
        },
    );
    let cache_accessible = usage.map_or_else(
        || "No cache telemetry yet".to_owned(),
        |usage| {
            if usage.cache_measured_calls == 0 {
                if usage.total_calls == 0 {
                    "No cache telemetry yet".to_owned()
                } else {
                    format!("No cache telemetry · 0/{} calls", usage.total_calls)
                }
            } else {
                format!(
                    "{} / {} input · {}/{} calls",
                    token_count(usage.cache_read_tokens),
                    token_count(usage.cache_input_tokens),
                    usage.cache_measured_calls,
                    usage.total_calls
                )
            }
        },
    );
    let cache_value = usage.and_then(cache_rate).unwrap_or_else(|| "—".to_owned());
    let cached_stats = usage.map_or_else(
        || "0 Cached".to_owned(),
        |usage| {
            if usage.total_calls > 0 && usage.cache_measured_calls == 0 {
                "— Cached".to_owned()
            } else {
                format!("{} Cached", compact_count(usage.cache_read_tokens))
            }
        },
    );
    col()
        .key("session-sidenav")
        .role("complementary")
        .label("Session sidebar")
        .w(WIDTH)
        .px(16.0)
        .pt(14.0)
        .pb(14.0)
        .radius(18.0)
        .continuous_corners()
        .fill(theme.color("#1b1b1b"))
        .border(1.0, theme.color("#303030"))
        .shadow(
            2.0,
            12.0,
            Color::BLACK.alpha(if theme.is_dark() { 0.24 } else { 0.12 }),
        )
        .gap(6.0)
        .child(
            row()
                .w_full()
                .h(24.0)
                .center_y()
                .gap(3.0)
                .child(
                    text("Session")
                        .key("session-sidenav-title")
                        .font(ui_font(14.0, 400.0))
                        .color(theme.color("#aaaaaa")),
                )
                .child(
                    el().key("session-sidenav-chart-button")
                        .role("button")
                        .label("Open session usage chart")
                        .help("Open session usage chart")
                        .size(24.0, 24.0)
                        .center()
                        .radius(6.0)
                        .hover(theme.color("#303030"))
                        .on_press(|state: &mut State| toggle_menu(state, UsageMenu::Tokens))
                        .child(icon(&TRACKING, 14.0).color(theme.color("#aaaaaa"))),
                )
                .child(row().grow(1.0))
                .child(
                    text(duration_label(duration))
                        .key("session-sidenav-duration")
                        .font(ui_font(12.0, 400.0))
                        .color(theme.color("#969696")),
                ),
        )
        .child(
            col()
                .w_full()
                .gap(2.0)
                .role("group")
                .label("Session usage")
                .child(usage_row(
                    UsageMenu::Tokens,
                    active_menu == Some(UsageMenu::Tokens),
                    UsageRowContent {
                        mark: &SESSION_TOKENS,
                        primary: format!("{token_display} Tokens"),
                        stats: token_stats,
                        accessible: format!("{token_value} session tokens. {token_accessible}"),
                        key: "session-sidenav-tokens",
                        stats_key: "session-sidenav-token-stats",
                    },
                    theme,
                ))
                .child(usage_row(
                    UsageMenu::Cache,
                    active_menu == Some(UsageMenu::Cache),
                    UsageRowContent {
                        mark: &SESSION_CACHE_RATE,
                        primary: format!("{cache_value} Hit Rate"),
                        stats: cached_stats,
                        accessible: format!("Cache hit rate {cache_value}. {cache_accessible}"),
                        key: "session-sidenav-cache-rate",
                        stats_key: "session-sidenav-cache-stats",
                    },
                    theme,
                )),
        )
}

const CHART_WIDTH: f32 = 322.0;
const CHART_HEIGHT: f32 = 122.0;

struct ChartSeries {
    color: &'static str,
    value: fn(&SessionUsageSnapshot) -> Option<f32>,
}

fn total_tokens(point: &SessionUsageSnapshot) -> Option<f32> {
    Some(point.input_tokens.saturating_add(point.output_tokens) as f32)
}

fn input_tokens(point: &SessionUsageSnapshot) -> Option<f32> {
    Some(point.input_tokens as f32)
}

fn output_tokens(point: &SessionUsageSnapshot) -> Option<f32> {
    Some(point.output_tokens as f32)
}

fn cached_tokens(point: &SessionUsageSnapshot) -> Option<f32> {
    Some(point.cache_read_tokens as f32)
}

fn hit_rate(point: &SessionUsageSnapshot) -> Option<f32> {
    (point.cache_input_tokens > 0)
        .then(|| point.cache_read_tokens as f32 / point.cache_input_tokens as f32 * 100.0)
}

fn line_chart(
    key: &'static str,
    points: &[SessionUsageSnapshot],
    selected: usize,
    series: &[ChartSeries],
    fixed_max: Option<f32>,
    theme: ThemeStyle<'_>,
) -> El {
    let left = 8.0;
    let right = CHART_WIDTH - 8.0;
    let top = 8.0;
    let bottom = CHART_HEIGHT - 10.0;
    let first_work = points.first().map_or(0, |point| point.work_duration_ms);
    let last_work = points.last().map_or(0, |point| point.work_duration_ms);
    let x = |work: u64| {
        if last_work == first_work {
            right
        } else {
            left + work.saturating_sub(first_work) as f32 / (last_work - first_work) as f32
                * (right - left)
        }
    };
    let max_value = fixed_max
        .unwrap_or_else(|| {
            points
                .iter()
                .flat_map(|point| series.iter().filter_map(|line| (line.value)(point)))
                .fold(0.0_f32, f32::max)
        })
        .max(1.0);
    let y = |value: f32| bottom - (value / max_value).clamp(0.0, 1.0) * (bottom - top);
    let mut shape = Shape::new();
    for fraction in [0.0, 0.5, 1.0] {
        let level = bottom - fraction * (bottom - top);
        shape = shape.line(
            [left, level],
            [right, level],
            0.7,
            LineCap::Butt,
            theme.color("#3b3b3b"),
        );
    }
    if let Some(point) = points.get(selected) {
        let selected_x = x(point.work_duration_ms);
        shape = shape.line(
            [selected_x, top],
            [selected_x, bottom],
            1.0,
            LineCap::Butt,
            theme.color("#666666"),
        );
    }
    for line in series {
        let color = theme.color(line.color);
        let vertices: Vec<[f32; 2]> = points
            .iter()
            .filter_map(|point| {
                (line.value)(point).map(|value| [x(point.work_duration_ms), y(value)])
            })
            .collect();
        if vertices.len() > 1 {
            shape = shape.stroke(vertices.clone(), 2.0, color);
        }
        if let Some(point) = points.get(selected)
            && let Some(value) = (line.value)(point)
        {
            let vertex = [x(point.work_duration_ms), y(value)];
            shape = shape.line(vertex, vertex, 5.0, LineCap::Round, color);
        }
    }
    canvas(shape.view_box(CHART_WIDTH, CHART_HEIGHT))
        .key(key)
        .size(CHART_WIDTH, CHART_HEIGHT)
}

fn menu_step(
    key: &'static str,
    label: &'static str,
    mark: &'static Icon,
    enabled: bool,
    next: bool,
    last: usize,
    theme: ThemeStyle<'_>,
) -> El {
    let mut control = el()
        .key(key)
        .role("button")
        .label(label)
        .size(27.0, 25.0)
        .center()
        .radius(6.0)
        .child(icon(mark, 13.0).color(theme.color(if enabled { "#c8c8c8" } else { "#666666" })));
    if enabled {
        control = control
            .hover(theme.color("#3b3b3b"))
            .on_press(move |state: &mut State| {
                state.session_usage_snapshot_index = if next {
                    (state.session_usage_snapshot_index + 1).min(last)
                } else {
                    state.session_usage_snapshot_index.saturating_sub(1)
                };
            });
    }
    control
}

fn work_label(ms: u64) -> String {
    let minutes = ms / 60_000;
    let seconds = (ms / 1_000) % 60;
    format!("{minutes}:{seconds:02} work")
}

pub(crate) fn popup(
    detail: Option<&SessionDetail>,
    menu: UsageMenu,
    selected: usize,
    theme: ThemeStyle<'_>,
) -> El {
    let points = detail.map_or(&[][..], |detail| detail.usage_snapshots.as_slice());
    let selected = selected.min(points.len().saturating_sub(1));
    let title = match menu {
        UsageMenu::Tokens => "Session tokens",
        UsageMenu::Cache => "Cache usage",
    };
    let mut panel = col()
        .key("session-usage-popup")
        .role("dialog")
        .label(title)
        .w(350.0)
        .padding(14.0)
        .gap(10.0)
        .child(
            row()
                .w_full()
                .center_y()
                .child(
                    text(title)
                        .font(ui_font(14.0, 550.0))
                        .color(theme.color("#eeeeee")),
                )
                .child(el().grow(1.0))
                .child(
                    el().key("session-usage-popup-close")
                        .role("button")
                        .label("Close usage chart")
                        .size(25.0, 25.0)
                        .center()
                        .radius(6.0)
                        .hover(theme.color("#3b3b3b"))
                        .on_press(|state: &mut State| state.session_usage_menu = None)
                        .child(
                            text("×")
                                .font(ui_font(18.0, 400.0))
                                .color(theme.color("#b8b8b8")),
                        ),
                ),
        )
        .child(
            text("Cumulative snapshots every 5 minutes of active work")
                .font(ui_font(11.0, 400.0))
                .color(theme.color("#929292")),
        );
    if points.is_empty() {
        panel = panel.child(
            text("No completed session work yet.")
                .font(ui_font(12.0, 400.0))
                .color(theme.color("#aaaaaa")),
        );
    } else {
        let chart_title = |label: &'static str| {
            text(label)
                .font(ui_font(12.0, 450.0))
                .color(theme.color("#d8d8d8"))
        };
        match menu {
            UsageMenu::Tokens => {
                panel = panel
                    .child(chart_title("Total, input, and output tokens"))
                    .child(line_chart(
                        "session-usage-token-chart",
                        points,
                        selected,
                        &[
                            ChartSeries {
                                color: "#8dabef",
                                value: total_tokens,
                            },
                            ChartSeries {
                                color: "#92c4a8",
                                value: input_tokens,
                            },
                            ChartSeries {
                                color: "#d9a27e",
                                value: output_tokens,
                            },
                        ],
                        None,
                        theme,
                    ))
                    .child(
                        row()
                            .w_full()
                            .gap(14.0)
                            .child(chart_legend("Total", "#8dabef", theme))
                            .child(chart_legend("Input", "#92c4a8", theme))
                            .child(chart_legend("Output", "#d9a27e", theme)),
                    );
            }
            UsageMenu::Cache => {
                panel = panel
                    .child(chart_title("Cache hit rate"))
                    .child(line_chart(
                        "session-usage-cache-rate-chart",
                        points,
                        selected,
                        &[ChartSeries {
                            color: "#bba3e5",
                            value: hit_rate,
                        }],
                        Some(100.0),
                        theme,
                    ))
                    .child(chart_title("Cached input tokens"))
                    .child(line_chart(
                        "session-usage-cache-tokens-chart",
                        points,
                        selected,
                        &[ChartSeries {
                            color: "#92c4a8",
                            value: cached_tokens,
                        }],
                        None,
                        theme,
                    ));
            }
        }
        let current = &points[selected];
        let detail = match menu {
            UsageMenu::Tokens => format!(
                "{} total · {} in · {} out",
                token_count(current.input_tokens.saturating_add(current.output_tokens)),
                token_count(current.input_tokens),
                token_count(current.output_tokens)
            ),
            UsageMenu::Cache => {
                if current.cache_input_tokens == 0 {
                    "Cache rate unavailable at this point".to_owned()
                } else {
                    format!(
                        "{} cached of {} eligible input",
                        token_count(current.cache_read_tokens),
                        token_count(current.cache_input_tokens)
                    )
                }
            }
        };
        panel = panel
            .child(
                row()
                    .w_full()
                    .child(
                        text(work_label(points[0].work_duration_ms))
                            .font(ui_font(10.5, 400.0))
                            .color(theme.color("#888888")),
                    )
                    .child(el().grow(1.0))
                    .child(
                        text(work_label(points.last().unwrap().work_duration_ms))
                            .font(ui_font(10.5, 400.0))
                            .color(theme.color("#888888")),
                    ),
            )
            .child(
                row()
                    .w_full()
                    .center_y()
                    .gap(4.0)
                    .child(menu_step(
                        "session-usage-previous",
                        "Previous snapshot",
                        &ARROW_LEFT,
                        selected > 0,
                        false,
                        points.len() - 1,
                        theme,
                    ))
                    .child(
                        text(work_label(current.work_duration_ms))
                            .key("session-usage-selected-work")
                            .font(ui_font(12.0, 500.0))
                            .color(theme.color("#eeeeee")),
                    )
                    .child(el().grow(1.0))
                    .child(menu_step(
                        "session-usage-next",
                        "Next snapshot",
                        &ARROW_RIGHT,
                        selected + 1 < points.len(),
                        true,
                        points.len() - 1,
                        theme,
                    )),
            )
            .child(
                text(detail)
                    .key("session-usage-selected-detail")
                    .w_full()
                    .wrap()
                    .font(ui_font(11.5, 400.0))
                    .color(theme.color("#aaaaaa")),
            );
    }
    in_place_menu::popup_surface(panel, theme.color("#242424"))
        .anchored_fit(menu.anchor(), Side::Left, Align::Start, 24.0)
        .on_hover_change(|state: &mut State, inside, _| {
            state.session_usage_popup_hovered = inside;
            if !inside && state.session_usage_row_hovered.is_none() {
                state.session_usage_menu = None;
            }
        })
        .hover_delays(Duration::ZERO, Duration::from_millis(250))
}

fn chart_legend(label: &'static str, color: &'static str, theme: ThemeStyle<'_>) -> El {
    row()
        .center_y()
        .gap(5.0)
        .child(el().size(10.0, 2.0).fill(theme.color(color)))
        .child(
            text(label)
                .font(ui_font(10.5, 400.0))
                .color(theme.color("#999999")),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use beale_contracts::v1::{SessionMessage, SessionToolSummary};

    #[test]
    fn duration_adds_only_the_running_period_to_persisted_periods() {
        let detail = SessionDetail {
            total_duration_ms: 75_000,
            messages: vec![SessionMessage {
                duration_ms: 10_000,
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(duration_ms(&detail, 5_000), 80_000);
        assert_eq!(duration_label(duration_ms(&detail, 5_000)), "00:01:20");
    }

    #[test]
    fn cache_rate_uses_only_reported_input_and_preserves_a_decimal() {
        let usage = SessionTokenUsage {
            input_tokens: 1_500,
            cache_read_tokens: 53,
            cache_input_tokens: 125,
            cache_measured_calls: 1,
            total_calls: 2,
            ..Default::default()
        };
        assert_eq!(cache_rate(&usage).as_deref(), Some("42.4%"));
        assert_eq!(token_count(1_500), "1,500");
        assert_eq!(compact_count(1_500), "1.5k");
        assert_eq!(compact_count(999_950), "1m");
        assert_eq!(cache_rate(&SessionTokenUsage::default()), None);
    }

    #[test]
    fn only_successful_creation_or_delegation_activates_sidenav() {
        let tool = |name: &str, is_error| SessionToolSummary {
            name: name.to_owned(),
            is_error,
            ..Default::default()
        };
        let detail = SessionDetail {
            messages: vec![SessionMessage {
                tools: vec![tool("memory_query", false), tool("runbook_create", true)],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(!has_campaign_activity(&detail));
        assert!(has_campaign_activity(&SessionDetail {
            has_campaign_activity: true,
            ..Default::default()
        }));
        let event = SessionStreamEvent {
            kind: SessionStreamKind::ToolFinished as i32,
            tool: Some(tool("graph_delegate", false)),
            ..Default::default()
        };
        assert!(campaign_activity_event(&event));
        assert!(!campaign_activity_event(&SessionStreamEvent {
            kind: SessionStreamKind::ToolStarted as i32,
            ..event.clone()
        }));
        assert!(!campaign_activity_event(&SessionStreamEvent {
            kind: SessionStreamKind::ToolStarted as i32,
            tool: Some(tool("runbook_create", false)),
            ..event
        }));
    }
}
