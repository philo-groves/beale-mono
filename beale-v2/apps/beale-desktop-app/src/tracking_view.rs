use crate::token_charts::{
    self, card, chart_color, copy, exact, legend_item, metrics, mix_chart, model_breakdown, muted,
    provider_name,
};
use crate::{
    design,
    icons::*,
    tracking_data::{self, AgentFilter, Summary},
};
use beale_contracts::{
    PROTOBUF_CONTENT_TYPE, TRACKING_PATH,
    v1::{TrackingResponse, TrackingSubscriptionPoint},
};
use chrono::{DateTime, Utc};
use prost::Message;
use reqwest::{Client, StatusCode, header::CONTENT_TYPE};
use rune::{El, LineCap, Shape, canvas, col, el, icon, row, text};
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{
    AUTH_HANDLE, ConnectionStatus, State, ThemeStyle, connection, provider_auth, ui_font,
    view_title_font,
};

const DAY_MS: u64 = 86_400_000;
const WEEK_MS: u64 = 7 * DAY_MS;
const CHART_HEIGHT: f32 = 188.0;
#[derive(Default)]
pub(crate) struct TrackingState {
    pub(crate) snapshot: Option<TrackingResponse>,
    pub(crate) loading: bool,
    pub(crate) error: String,
    pub(crate) generation: u64,
    pub(crate) filter: AgentFilter,
    pub(crate) selected_day: Option<u64>,
    pub(crate) show_subscriptions: bool,
    pub(crate) updated_at: Option<u64>,
}

async fn fetch(client: &Client, addr: SocketAddr) -> Result<TrackingResponse, String> {
    let token = provider_auth::authenticated_token(client, addr).await?;
    let response = client
        .get(format!("http://{addr}{TRACKING_PATH}"))
        .bearer_auth(token)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|_| "Tracking is unavailable".to_owned())?;
    if response.status() != StatusCode::OK
        || response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            != Some(PROTOBUF_CONTENT_TYPE)
    {
        return Err("Tracking is unavailable".to_owned());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "Could not read tracking data".to_owned())?;
    if bytes.len() > 2_000_000 {
        return Err("Tracking response is too large".to_owned());
    }
    let value = TrackingResponse::decode(bytes).map_err(|_| "Invalid tracking data".to_owned())?;
    if value.subscriptions.len() > 5000
        || value.model_days.len() > 2000
        || value.subscriptions.iter().any(|point| {
            point.remaining_percent > 100 || point.window_id.len() > 40 || point.label.len() > 40
        })
        || value.model_days.iter().any(|day| {
            day.day_start_ms % DAY_MS != 0
                || day.model.len() > 200
                || day.role.len() > 20
                || day.measured_calls > day.total_calls
                || !tracking_data::valid_cache(day)
        })
    {
        return Err("Invalid tracking data".to_owned());
    }
    Ok(value)
}

pub(crate) fn start_load(state: &mut State) {
    if state.tracking.loading || !matches!(state.connection, ConnectionStatus::Connected { .. }) {
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        return;
    };
    state.tracking.loading = true;
    state.tracking.generation = state.tracking.generation.wrapping_add(1);
    let generation = state.tracking.generation;
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
            runtime.block_on(fetch(&client, addr))
        })();
        handle.update(move |state, _| {
            if state.tracking.generation != generation {
                return;
            }
            state.tracking.loading = false;
            match result {
                Ok(snapshot) => {
                    state.tracking.snapshot = Some(snapshot);
                    state.tracking.updated_at = Some(now_ms());
                    state.tracking.error.clear();
                }
                Err(error) => state.tracking.error = error,
            }
        });
    });
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

fn date_label(ms: u64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms.min(i64::MAX as u64) as i64)
        .map_or_else(|| "—".to_owned(), |date| date.format("%b %-d").to_string())
}

fn subscription_chart(
    points: &[TrackingSubscriptionPoint],
    width: f32,
    theme: ThemeStyle<'_>,
) -> El {
    let now = now_ms();
    let since = now.saturating_sub(WEEK_MS);
    let mut series: BTreeMap<(i32, String, String), Vec<&TrackingSubscriptionPoint>> =
        BTreeMap::new();
    for point in points.iter().filter(|point| point.observed_at_ms >= since) {
        series
            .entry((point.provider, point.window_id.clone(), point.label.clone()))
            .or_default()
            .push(point);
    }
    let mut card = col()
        .key("tracking-subscription-card")
        .w_full()
        .gap(12.0)
        .padding(18.0)
        .radius(12.0)
        .fill(theme.color("#1b1b1b"))
        .child(
            text("Subscription remaining · past 7 days")
                .font(ui_font(16.0, 550.0))
                .color(theme.color("#eeeeee")),
        );
    if series.is_empty() {
        return card.child(
            text("No subscription samples yet. Beale records checks while it is running.")
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#969696")),
        );
    }
    let width = (width - 36.0).max(260.0);
    let left = 12.0;
    let right = width - 12.0;
    let top = 8.0;
    let bottom = CHART_HEIGHT - 12.0;
    let mut shape = Shape::new();
    for level in [0.0, 25.0, 50.0, 75.0, 100.0] {
        let y = bottom - level / 100.0 * (bottom - top);
        shape = shape.line(
            [left, y],
            [right, y],
            0.7,
            LineCap::Butt,
            theme.color("#343434"),
        );
    }
    let mut legend = col().w_full().gap(6.0);
    for (index, ((provider, _, label), values)) in series.iter_mut().enumerate() {
        values.sort_by_key(|point| point.observed_at_ms);
        let color = chart_color(index, theme);
        let vertices: Vec<[f32; 2]> = values
            .iter()
            .map(|point| {
                let x = left
                    + point.observed_at_ms.saturating_sub(since) as f32 / WEEK_MS as f32
                        * (right - left);
                let y = bottom - point.remaining_percent as f32 / 100.0 * (bottom - top);
                [x, y]
            })
            .collect();
        if vertices.len() > 1 {
            shape = shape.stroke(vertices.clone(), 2.0, color);
        }
        for vertex in vertices {
            shape = shape.line(vertex, vertex, 3.0, LineCap::Round, color);
        }
        let latest = values.last().unwrap();
        legend = legend.child(legend_item(
            format!(
                "{} {} · {}%",
                provider_name(*provider),
                label,
                latest.remaining_percent
            ),
            color,
            theme,
        ));
    }
    card = card.child(
        canvas(shape.view_box(width, CHART_HEIGHT))
            .key("tracking-subscription-chart")
            .size(width, CHART_HEIGHT),
    );
    card.child(
        row()
            .w_full()
            .child(
                text(date_label(since))
                    .font(ui_font(11.0, 400.0))
                    .color(theme.color("#777777")),
            )
            .child(el().grow(1.0))
            .child(
                text("Today")
                    .font(ui_font(11.0, 400.0))
                    .color(theme.color("#777777")),
            ),
    )
    .child(legend)
}

pub(crate) fn sidenav(state: &State, width: f32, theme: ThemeStyle<'_>) -> El {
    let connected = matches!(state.connection, ConnectionStatus::Connected { .. });
    col()
        .key("tracking-sidenav")
        .role("navigation")
        .label("Tracking sections")
        .w(width)
        .h_full()
        .px(8.0)
        .pt(12.0)
        .pb(18.0)
        .fill(theme.color("#111111"))
        .child(crate::sidenav_heading("Tracking", theme).ml(4.0).mb(12.0))
        .child(
            row()
                .key("tracking-tokens-nav")
                .role("button")
                .label("Token Tracking, selected")
                .w_full()
                .h(34.0)
                .px(8.0)
                .center_y()
                .gap(12.0)
                .radius(8.0)
                .fill(theme.subtle_accent())
                .hover(theme.color("#303030"))
                .on_press(|state: &mut State| state.navigate_to(crate::ViewLocation::Tracking))
                .child(icon(&SESSION_TOKENS, 16.0).color(theme.accent()))
                .child(copy("Token Tracking", 14.0, 550.0, theme).color(theme.accent())),
        )
        .child(el().grow(1.0))
        .child(
            col()
                .w_full()
                .px(12.0)
                .gap(8.0)
                .child(design::eyebrow("RESEARCH ACTIVITY", theme))
                .child(muted(
                    "All workspaces on this app server. Main and subagent calls are included.",
                    theme,
                ))
                .child(
                    row()
                        .center_y()
                        .gap(6.0)
                        .mt(8.0)
                        .child(
                            el().size(6.0, 6.0)
                                .radius(3.0)
                                .fill(theme.color(if connected { "#8fd0a8" } else { "#969696" })),
                        )
                        .child(muted(
                            if connected {
                                "Connected"
                            } else {
                                "Disconnected"
                            },
                            theme,
                        )),
                ),
        )
}

fn filter_tabs(state: &State, theme: ThemeStyle<'_>) -> El {
    token_charts::agent_filters(
        state.tracking.filter,
        "tracking",
        |state, filter| state.tracking.filter = filter,
        theme,
    )
}

pub(crate) fn render(state: &State, theme: ThemeStyle<'_>, page_width: f32) -> Vec<El> {
    let now = now_ms();
    let mut content = vec![
        design::breadcrumb_title(
            "Tracking",
            copy("Token Tracking", 24.0, 600.0, theme)
                .key("tracking-title")
                .font(view_title_font())
                .role("heading")
                .truncate_fade(),
            theme,
        ),
        design::subtitle(
            "Follow the tokens behind your research. See what went in, what came out, and what was reused.",
            theme,
        ),
    ];
    if !state.tracking.error.is_empty() {
        content.push(
            muted(
                format!(
                    "{}{}",
                    state.tracking.error,
                    if state.tracking.snapshot.is_some() {
                        " · Showing the last received snapshot."
                    } else {
                        ""
                    }
                ),
                theme,
            )
            .color(theme.color("#e07676")),
        );
    }
    let Some(snapshot) = &state.tracking.snapshot else {
        content.push(design::empty_state(
            &TRACKING,
            if state.tracking.loading {
                "Gathering your activity"
            } else if matches!(state.connection, ConnectionStatus::Connected { .. }) {
                "Token history is unavailable"
            } else {
                "Connect to see your token usage"
            },
            if state.tracking.loading {
                "Your token history will appear here when the app server responds."
            } else if matches!(state.connection, ConnectionStatus::Connected { .. }) {
                "We'll retry automatically while this view is open."
            } else {
                "Token Tracking uses the model calls recorded by your connected app server."
            },
            theme,
        ));
        return content;
    };
    let summary = tracking_data::summarize(
        &snapshot.model_days,
        now,
        state.tracking.filter,
        state.tracking.selected_day,
    );
    let selection = summary.selected_day.map_or_else(
        || "Past 7 days · UTC".into(),
        |day| format!("{} · Clear", date_label(day)),
    );
    let control = if summary.selected_day.is_some() {
        crate::form_button(selection, theme)
            .child(icon(&CLOSE, 13.0).color(theme.color("#969696")))
            .label("Clear day selection and show all seven days")
            .on_press(|state: &mut State| state.tracking.selected_day = None)
    } else {
        row()
            .h(32.0)
            .px(10.0)
            .gap(7.0)
            .center_y()
            .child(icon(&CLOCK, 13.0).color(theme.color("#969696")))
            .child(muted(selection, theme))
    }
    .key("tracking-clear-day");
    let widths = AgentFilter::ALL.map(|filter| design::segmented_tab_width(filter.label(), true));
    content.push(design::tabs_with_control(
        filter_tabs(state, theme),
        design::segmented_group_width(&widths),
        control,
        165.0,
        page_width,
    ));
    if snapshot.model_days.is_empty() {
        content.push(card(theme).key("tracking-empty")
            .child(design::empty_state(&SESSION_TOKENS, "Your token story starts here", "Run a research session to see daily usage, model breakdowns, and cache reuse. Only reported token counts appear here.", theme)
                .child(design::primary_button("Start research", theme).self_start().mt(8.0).on_press(State::start_new_research))));
    } else {
        content.push(metrics(summary.total, &summary.days, page_width, theme));
        if page_width >= 900.0 {
            let trend_width = (page_width - 16.0) * 0.66;
            content.push(
                row()
                    .w_full()
                    .gap(16.0)
                    .child(activity_chart(&summary, trend_width, theme))
                    .child(mix_chart(
                        summary.total,
                        page_width - 16.0 - trend_width,
                        theme,
                    )),
            );
        } else {
            content.push(activity_chart(&summary, page_width, theme));
            content.push(mix_chart(summary.total, page_width, theme));
        }
        content.push(model_breakdown(
            summary.total,
            &summary.models,
            page_width,
            theme,
        ));
        let total = summary.total;
        content.push(muted(format!("Reporting coverage · {} of {} calls include token counts. Cache telemetry is available for {} calls. Missing counts are excluded, not estimated.", total.measured, total.calls, total.cache_calls), theme));
    }
    content.push(
        row()
            .key("tracking-subscription-toggle")
            .role("button")
            .label("Subscription headroom")
            .w_full()
            .h(38.0)
            .center_y()
            .gap(8.0)
            .radius(8.0)
            .hover(theme.color("#242424"))
            .on_press(|state: &mut State| {
                state.tracking.show_subscriptions = !state.tracking.show_subscriptions
            })
            .child(
                icon(
                    if state.tracking.show_subscriptions {
                        &CHEVRON_DOWN
                    } else {
                        &CHEVRON_RIGHT
                    },
                    14.0,
                )
                .color(theme.color("#969696")),
            )
            .child(copy("Subscription headroom", 14.0, 550.0, theme))
            .child(el().grow(1.0))
            .child(muted("Past 7 days", theme)),
    );
    if state.tracking.show_subscriptions {
        content.push(subscription_chart(
            &snapshot.subscriptions,
            page_width,
            theme,
        ));
    }
    if let Some(at) = state.tracking.updated_at {
        let time = DateTime::<Utc>::from_timestamp_millis(at as i64)
            .map_or_else(|| "—".into(), |v| v.format("%H:%M UTC").to_string());
        content.push(muted(
            format!(
                "{} · Last received {time}",
                if state.tracking.loading {
                    "Updating"
                } else if !matches!(state.connection, ConnectionStatus::Connected { .. }) {
                    "Offline"
                } else {
                    "Refreshes automatically"
                }
            ),
            theme,
        ));
    }
    content
}

fn activity_chart(summary: &Summary, width: f32, theme: ThemeStyle<'_>) -> El {
    let points: Vec<_> = summary.days.iter().enumerate().map(|(index, day)| {
        let at = summary.first_day + index as u64 * DAY_MS;
        let label = DateTime::<Utc>::from_timestamp_millis(at as i64).map_or_else(
            || "—".into(), |d| d.format(if width < 520.0 { "%-d" } else { "%a" }).to_string());
        let description = if day.tokens_unreported() {
            format!("{} UTC: {} calls. Token counts were not reported. Select to focus this day.", date_label(at), day.calls)
        } else {
            format!("{} UTC: {} input, {} output, {} total tokens. {} of {} calls reported tokens. Select to focus this day.", date_label(at), exact(day.input), exact(day.output), exact(day.tokens()), day.measured, day.calls)
        };
        token_charts::ActivityPoint { id: at, key: format!("tracking-day-{index}"), label, description, total: *day, selected: summary.selected_day == Some(at) }
    }).collect();
    token_charts::activity_chart(
        &points,
        "Select a day to focus the breakdown below.",
        Some(|state, at| {
            state.tracking.selected_day = if state.tracking.selected_day == Some(at) {
                None
            } else {
                Some(at)
            };
        }),
        width,
        theme,
    )
}
