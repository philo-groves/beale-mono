use beale_contracts::{
    PROTOBUF_CONTENT_TYPE, PROVIDER_USAGE_PATH,
    v1::{ProviderId, ProviderUsageResponse, ProviderUsageSnapshot, ProviderUsageWindow},
};
use prost::Message;
use reqwest::{Client, StatusCode, header::CONTENT_TYPE};
use rune::{Align, Color, El, Icon, Side, col, el, icon, row, text};
use std::{
    net::SocketAddr,
    sync::OnceLock,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{
    HEALTHY_RING_COLOR, PROVIDER_ERROR_COLOR, PROVIDER_WARNING_COLOR, Provider, State, ThemeStyle,
    provider_auth,
    provider_icons::{GROK, OPENAI},
    ui_font,
};

pub(crate) async fn fetch(
    client: &Client,
    addr: SocketAddr,
    refresh: bool,
) -> Result<[Option<ProviderUsageSnapshot>; 2], String> {
    let token = provider_auth::authenticated_token(client, addr).await?;
    let mut request = client
        .get(format!("http://{addr}{PROVIDER_USAGE_PATH}"))
        .bearer_auth(token)
        .timeout(Duration::from_secs(20));
    if refresh {
        request = request.header("Cache-Control", "no-cache")
    }
    let response = request
        .send()
        .await
        .map_err(|_| "Subscription usage is unavailable".to_owned())?;
    if response.status() != StatusCode::OK
        || response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            != Some(PROTOBUF_CONTENT_TYPE)
    {
        return Err("Subscription usage is unavailable".to_owned());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "Could not read subscription usage".to_owned())?;
    if bytes.len() > 4096 {
        return Err("Subscription usage response is too large".to_owned());
    }
    decode(&bytes)
}

fn decode(bytes: &[u8]) -> Result<[Option<ProviderUsageSnapshot>; 2], String> {
    let response = ProviderUsageResponse::decode(bytes)
        .map_err(|_| "Invalid subscription usage".to_owned())?;
    let mut result = [None, None];
    for mut item in response.providers {
        let index = match ProviderId::try_from(item.provider) {
            Ok(ProviderId::Openai) => 0,
            Ok(ProviderId::Xai) => 1,
            _ => return Err("Invalid usage provider".to_owned()),
        };
        if result[index].is_some() {
            return Err("Duplicate usage provider".to_owned());
        }
        if item.windows.len() > 2 {
            return Err("Too many usage windows".to_owned());
        }
        for window in &mut item.windows {
            if window.label.len() > 40 || window.id.len() > 40 || window.used_percent > 100 {
                return Err("Invalid usage window".to_owned());
            }
        }
        result[index] = Some(item);
    }
    Ok(result)
}

pub(crate) const INDICATOR_SIZE: f32 = 34.0;
const RING_VIEW_SIZE: f32 = 36.0;
const RING_ICON_SIZE: f32 = 32.0;
const OUTER_RING_RADIUS: f32 = 16.0;
const INNER_RING_RADIUS: f32 = 10.5;
const RING_STROKE_WIDTH: f32 = 1.8;

fn arc_icon(percent_left: u32, inner: bool) -> &'static Icon {
    static OUTER: OnceLock<Vec<Icon>> = OnceLock::new();
    static INNER: OnceLock<Vec<Icon>> = OnceLock::new();
    let icons = if inner { &INNER } else { &OUTER }.get_or_init(|| (0..=100).map(|p| {
        let r = if inner { INNER_RING_RADIUS } else { OUTER_RING_RADIUS };
        let path = if p == 0 { String::new() } else if p == 100 {
            format!("M18 {} A{r} {r} 0 1 1 18 {} A{r} {r} 0 1 1 18 {}", 18.0-r, 18.0+r, 18.0-r)
        } else {
            let angle = (-90.0 + f64::from(p) * 3.6).to_radians();
            let x = 18.0 + r * angle.cos() as f32;
            let y = 18.0 + r * angle.sin() as f32;
            format!("M18 {} A{r} {r} 0 {} 1 {x} {y}", 18.0-r, u8::from(p > 50))
        };
        Icon {
            name: Box::leak(format!("usage-{}-{p}", if inner { "inner" } else { "outer" }).into_boxed_str()),
            view: RING_VIEW_SIZE,
            body: Box::leak(format!("<path d=\"{path}\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"{RING_STROKE_WIDTH}\" stroke-linecap=\"round\"/>").into_boxed_str()),
        }
    }).collect());
    &icons[percent_left.min(100) as usize]
}

fn ring_arc(percent_left: u32, inner: bool, color: Color) -> El {
    let inset = (INDICATOR_SIZE - RING_ICON_SIZE) / 2.0;
    icon(arc_icon(percent_left, inner), RING_ICON_SIZE)
        .absolute(inset, inset)
        .color(color)
}

pub(crate) fn full_outer_ring(color: Color) -> El {
    ring_arc(100, false, color)
}

fn ring_track(radius: f32, theme: ThemeStyle<'_>) -> El {
    let scale = RING_ICON_SIZE / RING_VIEW_SIZE;
    let diameter = 2.0 * radius * scale;
    let inset = (INDICATOR_SIZE - diameter) / 2.0;
    el().absolute(inset, inset)
        .size(diameter, diameter)
        .radius(diameter / 2.0)
        .border(RING_STROKE_WIDTH * scale, theme.color("#444444"))
}

pub(crate) fn outer_ring_track(theme: ThemeStyle<'_>) -> El {
    ring_track(OUTER_RING_RADIUS, theme)
}

fn remaining(window: &ProviderUsageWindow) -> u32 {
    100 - window.used_percent.min(100)
}

fn progress_color(remaining_percent: u32) -> &'static str {
    if remaining_percent < 20 {
        PROVIDER_ERROR_COLOR
    } else if remaining_percent < 40 {
        PROVIDER_WARNING_COLOR
    } else {
        HEALTHY_RING_COLOR
    }
}

pub(crate) fn indicator(
    provider: Provider,
    usage: &ProviderUsageSnapshot,
    theme: ThemeStyle<'_>,
) -> El {
    let session = usage.windows.iter().find(|w| w.label == "Session Limit");
    let outer = usage
        .windows
        .iter()
        .find(|w| w.label == "Weekly Limit")
        .or_else(|| usage.windows.iter().find(|w| Some(*w) != session))
        .or(session)
        .unwrap_or(&usage.windows[0]);
    let inner = session.filter(|w| *w != outer);
    let key = if provider == Provider::OpenAi {
        "usage-openai"
    } else {
        "usage-xai"
    };
    let logo = if provider == Provider::OpenAi {
        &OPENAI
    } else {
        &GROK
    };
    let mut ring = el()
        .key(key)
        .role("button")
        .label(format!(
            "Open {} provider settings from subscription usage",
            if provider == Provider::OpenAi {
                "OpenAI"
            } else {
                "xAI"
            }
        ))
        .size(INDICATOR_SIZE, INDICATOR_SIZE)
        .radius(INDICATOR_SIZE / 2.0)
        .hover(theme.color("#292929"))
        .on_hover_change(move |s: &mut State, inside, _| {
            s.rail_usage_tooltip = if inside { Some(provider) } else { None };
        })
        .on_press(move |s: &mut State| {
            s.rail_usage_tooltip = None;
            s.navigate_to(crate::ViewLocation::Providers(provider));
        });
    let outer_remaining = remaining(outer);
    ring = ring.child(outer_ring_track(theme)).child(ring_arc(
        outer_remaining,
        false,
        theme.color(progress_color(outer_remaining)),
    ));
    if let Some(inner) = inner {
        let inner_remaining = remaining(inner);
        ring = ring
            .child(ring_track(INNER_RING_RADIUS, theme))
            .child(ring_arc(
                inner_remaining,
                true,
                theme.color(progress_color(inner_remaining)),
            ));
    }
    let logo_size = if provider == Provider::OpenAi {
        14.0
    } else {
        19.0
    };
    let logo_inset = (INDICATOR_SIZE - logo_size) / 2.0;
    ring.child(
        icon(logo, logo_size)
            .absolute(logo_inset, logo_inset)
            .color(theme.color("#bdbdbd")),
    )
}

fn time_left(reset: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    if reset == 0 {
        return String::new();
    }
    if reset <= now {
        return " · reset due".to_owned();
    }
    let minutes = (reset - now).div_ceil(60);
    if minutes >= 1440 {
        format!(" · resets in {}d {}h", minutes / 1440, minutes % 1440 / 60)
    } else if minutes >= 60 {
        format!(" · resets in {}h {}m", minutes / 60, minutes % 60)
    } else {
        format!(" · resets in {minutes}m")
    }
}

pub(crate) fn tooltip(
    provider: Provider,
    usage: &ProviderUsageSnapshot,
    theme: ThemeStyle<'_>,
) -> El {
    let key = if provider == Provider::OpenAi {
        "usage-openai"
    } else {
        "usage-xai"
    };
    let title = if provider == Provider::OpenAi {
        "OpenAI (Codex)"
    } else {
        "xAI"
    };
    let mut content = col()
        .key("usage-tooltip")
        .inert()
        .gap(6.0)
        .px(12.0)
        .py(10.0)
        .radius(12.0)
        .continuous_corners()
        .fill(theme.color("#292929"))
        .shadow(5.0, 16.0, Color::BLACK.alpha(0.45))
        .child(
            text(title)
                .font(ui_font(13.0, 600.0))
                .color(theme.color("#eeeeee")),
        );
    for window in &usage.windows {
        content = content.child(
            row()
                .gap(10.0)
                .child(
                    text(window.label.trim_end_matches(" Limit"))
                        .font(ui_font(12.0, 400.0))
                        .color(theme.color("#bdbdbd")),
                )
                .child(
                    text(format!(
                        "{}% left{}",
                        remaining(window),
                        time_left(window.resets_at_unix)
                    ))
                    .font(ui_font(12.0, 400.0))
                    .color(theme.color("#bdbdbd")),
                ),
        );
    }
    content.anchored_fit(key, Side::Right, Align::End, 8.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaining_usage_colors_change_at_the_requested_thresholds() {
        assert_eq!(progress_color(40), HEALTHY_RING_COLOR);
        assert_eq!(progress_color(39), PROVIDER_WARNING_COLOR);
        assert_eq!(progress_color(20), PROVIDER_WARNING_COLOR);
        assert_eq!(progress_color(19), PROVIDER_ERROR_COLOR);
    }

    #[test]
    fn rejects_out_of_range_windows() {
        let payload = ProviderUsageResponse {
            providers: vec![ProviderUsageSnapshot {
                provider: ProviderId::Xai as i32,
                checked_at_unix: 1,
                windows: vec![ProviderUsageWindow {
                    id: "subscription".into(),
                    label: "Weekly Limit".into(),
                    used_percent: 101,
                    resets_at_unix: 0,
                }],
            }],
        }
        .encode_to_vec();
        assert!(decode(&payload).is_err());
    }
}
