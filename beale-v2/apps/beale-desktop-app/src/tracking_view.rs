use beale_contracts::{
    PROTOBUF_CONTENT_TYPE, TRACKING_PATH,
    v1::{ProviderId, TrackingModelDay, TrackingResponse, TrackingSubscriptionPoint},
};
use chrono::{DateTime, Utc};
use prost::Message;
use reqwest::{Client, StatusCode, header::CONTENT_TYPE};
use rune::{Color, El, LineCap, Shape, canvas, col, el, row, text};
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
const COLORS: [&str; 8] = [
    "#82a7ed", "#d99d78", "#8fbfa2", "#c1a0df", "#d7c078", "#82bfc7", "#d888aa", "#aab18a",
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Measure {
    #[default]
    Duration,
    Tokens,
}

#[derive(Default)]
pub(crate) struct TrackingState {
    pub(crate) snapshot: Option<TrackingResponse>,
    pub(crate) loading: bool,
    pub(crate) error: String,
    pub(crate) generation: u64,
    pub(crate) measure: Measure,
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
            day.model.len() > 200 || day.role.len() > 20 || day.measured_calls > day.total_calls
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

fn provider_name(provider: i32) -> &'static str {
    match ProviderId::try_from(provider) {
        Ok(ProviderId::Openai) => "OpenAI",
        Ok(ProviderId::Xai) => "xAI",
        Ok(ProviderId::Anthropic) => "Anthropic",
        Ok(ProviderId::Zai) => "Z.ai",
        Ok(ProviderId::Openrouter) => "OpenRouter",
        Ok(ProviderId::Custom) => "Custom",
        _ => "Provider",
    }
}

fn date_label(ms: u64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms.min(i64::MAX as u64) as i64)
        .map_or_else(|| "—".to_owned(), |date| date.format("%b %-d").to_string())
}

fn measure_label(value: u64, measure: Measure) -> String {
    match measure {
        Measure::Duration => format!("{:.1} s", value as f64 / 1000.0),
        Measure::Tokens => format!("{value} tokens"),
    }
}

fn legend_item(label: String, color: Color, theme: ThemeStyle<'_>) -> El {
    row()
        .gap(7.0)
        .center_y()
        .child(el().size(9.0, 9.0).radius(4.5).fill(color))
        .child(
            text(label)
                .font(ui_font(12.0, 400.0))
                .color(theme.color("#b8b8b8")),
        )
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
        let color = theme.color(COLORS[index % COLORS.len()]);
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

fn model_chart(
    days: &[TrackingModelDay],
    measure: Measure,
    width: f32,
    theme: ThemeStyle<'_>,
) -> El {
    let today = now_ms() / DAY_MS * DAY_MS;
    let first = today.saturating_sub(6 * DAY_MS);
    let mut series: BTreeMap<(i32, String, String), [u64; 7]> = BTreeMap::new();
    let mut measured = 0_u64;
    let mut total = 0_u64;
    for day in days {
        if day.day_start_ms < first || day.day_start_ms > today {
            continue;
        }
        let index = ((day.day_start_ms - first) / DAY_MS) as usize;
        let value = match measure {
            Measure::Duration => day.duration_ms,
            Measure::Tokens => day.input_tokens.saturating_add(day.output_tokens),
        };
        series
            .entry((day.provider, day.model.clone(), day.role.clone()))
            .or_default()[index] += value;
        measured += u64::from(day.measured_calls);
        total += u64::from(day.total_calls);
    }
    let title = match measure {
        Measure::Duration => "Model time by day",
        Measure::Tokens => "Model tokens by day",
    };
    let mut card = col()
        .key("tracking-model-card")
        .w_full()
        .gap(12.0)
        .padding(18.0)
        .radius(12.0)
        .fill(theme.color("#1b1b1b"))
        .child(
            row()
                .w_full()
                .center_y()
                .child(
                    text(title)
                        .font(ui_font(16.0, 550.0))
                        .color(theme.color("#eeeeee")),
                )
                .child(el().grow(1.0))
                .child(measure_button(
                    "Duration",
                    Measure::Duration,
                    measure,
                    theme,
                ))
                .child(measure_button("Tokens", Measure::Tokens, measure, theme)),
        );
    if series.is_empty()
        || series
            .values()
            .all(|values| values.iter().all(|&value| value == 0))
    {
        return card.child(
            text("No measured model usage for this view yet.")
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#969696")),
        );
    }
    let width = (width - 36.0).max(260.0);
    let top = 8.0;
    let bottom = CHART_HEIGHT - 12.0;
    let plot_height = bottom - top;
    let slot = (width - 24.0) / 7.0;
    let bar_width = slot * 0.56;
    let max_total = (0..7)
        .map(|day| series.values().map(|values| values[day]).sum::<u64>())
        .max()
        .unwrap_or(1)
        .max(1);
    card = card.child(
        text(format!(
            "Peak daily total: {}",
            measure_label(max_total, measure)
        ))
        .font(ui_font(12.0, 400.0))
        .color(theme.color("#969696")),
    );
    let mut shape = Shape::new();
    for level in [0.0, 0.25, 0.5, 0.75, 1.0] {
        let y = bottom - level * plot_height;
        shape = shape.line(
            [12.0, y],
            [width - 12.0, y],
            0.7,
            LineCap::Butt,
            theme.color("#343434"),
        );
    }
    let mut legend = col().w_full().gap(6.0);
    for (index, ((provider, model, role), values)) in series.iter().enumerate() {
        let color = theme.color(COLORS[index % COLORS.len()]);
        for day in 0..7 {
            let prior: u64 = series
                .iter()
                .take(index)
                .map(|(_, previous)| previous[day])
                .sum();
            let lower = bottom - prior as f32 / max_total as f32 * plot_height;
            let upper =
                bottom - prior.saturating_add(values[day]) as f32 / max_total as f32 * plot_height;
            if values[day] > 0 {
                let x = 12.0 + (day as f32 + 0.5) * slot;
                shape = shape.fill(
                    vec![
                        [x - bar_width / 2.0, lower],
                        [x + bar_width / 2.0, lower],
                        [x + bar_width / 2.0, upper],
                        [x - bar_width / 2.0, upper],
                    ],
                    color,
                );
            }
        }
        legend = legend.child(legend_item(
            format!(
                "{} · {} · {} · {}",
                provider_name(*provider),
                model,
                role,
                measure_label(values.iter().sum(), measure)
            ),
            color,
            theme,
        ));
    }
    card = card.child(
        canvas(shape.view_box(width, CHART_HEIGHT))
            .key("tracking-model-chart")
            .size(width, CHART_HEIGHT),
    );
    let mut dates = row().w_full();
    for day in 0..7 {
        dates = dates.child(
            text(date_label(first + day * DAY_MS))
                .grow(1.0)
                .font(ui_font(11.0, 400.0))
                .color(theme.color("#777777")),
        );
    }
    card = card.child(dates).child(legend);
    if measure == Measure::Tokens && measured < total {
        card = card.child(text(format!("Token counts reported for {measured} of {total} model calls; unreported calls are excluded."))
            .font(ui_font(12.0, 400.0)).color(theme.color("#969696")));
    }
    card
}

fn measure_button(
    label: &'static str,
    value: Measure,
    selected: Measure,
    theme: ThemeStyle<'_>,
) -> El {
    el().key(format!("tracking-measure-{}", label.to_ascii_lowercase()))
        .role("button")
        .label(format!("Show {label}"))
        .px(10.0)
        .h(28.0)
        .center()
        .radius(8.0)
        .fill(if value == selected {
            theme.color("#303030")
        } else {
            Color::TRANSPARENT
        })
        .hover(theme.color("#383838"))
        .on_press(move |state: &mut State| state.tracking.measure = value)
        .child(
            text(label)
                .font(ui_font(12.0, 500.0))
                .color(theme.color("#dddddd")),
        )
}

pub(crate) fn render(state: &State, theme: ThemeStyle<'_>, page_width: f32) -> Vec<El> {
    let mut content = vec![
        text("Tracking")
            .key("tracking-title")
            .font(view_title_font())
            .color(theme.color("#f2f2f2")),
    ];
    if !state.tracking.error.is_empty() {
        content.push(
            text(state.tracking.error.clone())
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#e07676")),
        );
    }
    if let Some(snapshot) = &state.tracking.snapshot {
        content.push(subscription_chart(
            &snapshot.subscriptions,
            page_width,
            theme,
        ));
        content.push(model_chart(
            &snapshot.model_days,
            state.tracking.measure,
            page_width,
            theme,
        ));
    } else if state.tracking.loading {
        content.push(
            text("Loading tracking data…")
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#969696")),
        );
    } else {
        content.push(
            text("Tracking data is available while connected to the app-server.")
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#969696")),
        );
    }
    content
}
