//! Shared token visualizations for host Tracking and single-session Insights.
use crate::{
    State, ThemeStyle, design,
    tracking_data::{AgentFilter, ModelTotal, Totals},
    ui_font,
};
use beale_contracts::v1::{AuthMethod, ProviderId};
use rune::{Color, El, LineCap, Shape, canvas, col, el, row, text};

const COLORS: [&str; 8] = [
    "#82a7ed", "#d99d78", "#8fbfa2", "#c1a0df", "#d7c078", "#82bfc7", "#d888aa", "#aab18a",
];

pub(crate) fn agent_filters(
    selected: AgentFilter,
    key_prefix: &'static str,
    on_select: fn(&mut State, AgentFilter),
    theme: ThemeStyle<'_>,
) -> El {
    use crate::icons::{AGENT, FEATURES, WORKSPACE_SURFACES};
    let mut tabs = design::segmented_control(theme)
        .key(format!("{key_prefix}-agent-filters"))
        .label("Agent activity");
    for filter in AgentFilter::ALL {
        let glyph = match filter {
            AgentFilter::All => &FEATURES,
            AgentFilter::Main => &AGENT,
            AgentFilter::Subagents => &WORKSPACE_SURFACES,
        };
        tabs = tabs.child(
            design::segmented_tab(filter.label(), Some(glyph), filter == selected, true, theme)
                .key(format!("{key_prefix}-filter-{filter:?}"))
                .on_press(move |state: &mut State| on_select(state, filter)),
        );
    }
    tabs
}

pub(crate) fn chart_color(index: usize, theme: ThemeStyle<'_>) -> Color {
    const LIGHT: [&str; 8] = [
        "#365fb0", "#985626", "#34704d", "#78549f", "#7b651f", "#2b737d", "#a24670", "#59672d",
    ];
    let palette = if matches!(theme.name(), "Light" | "Cream") {
        &LIGHT
    } else {
        &COLORS
    };
    rune::color::hex(palette[index % palette.len()]).unwrap()
}

pub(crate) fn provider_name(provider: i32) -> &'static str {
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

pub(crate) fn legend_item(label: String, color: Color, theme: ThemeStyle<'_>) -> El {
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

pub(crate) fn compact(value: u64) -> String {
    if value >= 1_000_000_000 {
        format!("{:.2}B", value as f64 / 1_000_000_000.0)
    } else if value >= 1_000_000 {
        format!("{:.2}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}K", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

pub(crate) fn exact(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

pub(crate) fn copy(label: impl Into<String>, size: f32, weight: f32, theme: ThemeStyle<'_>) -> El {
    text(label.into())
        .font(ui_font(size, weight))
        .color(theme.color("#eeeeee"))
}

pub(crate) fn muted(label: impl Into<String>, theme: ThemeStyle<'_>) -> El {
    copy(label, 12.0, 400.0, theme)
        .color(theme.color("#969696"))
        .min_w(0.0)
        .wrap()
}

pub(crate) fn card(theme: ThemeStyle<'_>) -> El {
    col()
        .w_full()
        .min_w(0.0)
        .padding(20.0)
        .gap(14.0)
        .radius(14.0)
        .fill(theme.color("#191919"))
        .border(1.0, theme.color("#2a2a2a").alpha(0.8))
}

fn token_value(total: Totals, value: u64) -> String {
    if total.tokens_unreported() {
        "—".into()
    } else {
        compact(value)
    }
}

fn sparkline(trend: &[Totals], width: f32, color: Color) -> El {
    if trend.len() < 2 {
        return el().size(width, 28.0);
    }
    let peak = trend.iter().map(|d| d.tokens()).max().unwrap_or(0).max(1);
    let points: Vec<_> = trend
        .iter()
        .enumerate()
        .map(|(i, day)| {
            [
                2.0 + i as f32 * (width - 4.0) / trend.len().saturating_sub(1).max(1) as f32,
                25.0 - day.tokens() as f32 / peak as f32 * 21.0,
            ]
        })
        .collect();
    canvas(
        Shape::new()
            .stroke(points, 2.0, color)
            .view_box(width, 28.0),
    )
    .size(width, 28.0)
}

pub(crate) fn metrics(total: Totals, trend: &[Totals], width: f32, theme: ThemeStyle<'_>) -> El {
    let columns = if width >= 900.0 {
        4
    } else if width >= 410.0 {
        2
    } else {
        1
    };
    let cache = total
        .cache_rate()
        .map_or_else(|| "—".into(), |rate| format!("{rate:.1}%"));
    let metrics = [
        (
            "TOTAL TOKENS",
            token_value(total, total.tokens()),
            format!("{} measured calls", exact(total.measured)),
            theme.accent(),
        ),
        (
            "INPUT TOKENS",
            token_value(total, total.input),
            "Includes cached input".into(),
            chart_color(0, theme),
        ),
        (
            "OUTPUT TOKENS",
            token_value(total, total.output),
            "Generated by your models".into(),
            chart_color(3, theme),
        ),
        (
            "CACHE HIT RATE",
            cache,
            if total.cache_calls > 0 {
                format!("{} tokens reused", compact(total.cached))
            } else {
                "No cache telemetry".into()
            },
            chart_color(2, theme),
        ),
    ];
    let mut grid = row()
        .key("tracking-metrics")
        .w_full()
        .gap(12.0)
        .equal_columns(columns);
    for (index, (label, value, note, color)) in metrics.into_iter().enumerate() {
        let accessible = if index < 3 && total.tokens_unreported() {
            format!(
                "{label}: not reported. {} calls without token counts.",
                total.calls
            )
        } else {
            match index {
                0 => format!(
                    "Total tokens: {}. {} calls with reported token counts out of {} calls.",
                    exact(total.tokens()),
                    total.measured,
                    total.calls
                ),
                1 => format!(
                    "Input tokens: {}. Cached input is included.",
                    exact(total.input)
                ),
                2 => format!("Output tokens: {}.", exact(total.output)),
                _ => format!(
                    "Cache hit rate: {value}. {} cache-read tokens out of {} input tokens on {} calls with cache telemetry.",
                    total.cached, total.cache_input, total.cache_calls
                ),
            }
        };
        let mut metric = card(theme)
            .key(format!("tracking-metric-{index}"))
            .label(accessible.clone())
            .help(accessible)
            .padding(16.0)
            .gap(9.0)
            .child(copy(label, 10.0, 600.0, theme).color(theme.color("#969696")))
            .child(
                copy(value, 29.0, 600.0, theme)
                    .key(format!("tracking-metric-value-{index}"))
                    .color(color),
            )
            .child(muted(note, theme));
        if index == 0 {
            metric = metric.child(sparkline(trend, 110.0, theme.accent().alpha(0.7)));
        } else if index == 3 {
            let progress = total.cache_rate().unwrap_or(0.0) as f32 / 100.0;
            metric = metric.child(
                el().w_full().h(28.0).center_y().child(
                    el().w_full()
                        .h(4.0)
                        .radius(2.0)
                        .fill(theme.color("#2a2a2a"))
                        .child(
                            el().w(
                                ((width - (columns - 1) as f32 * 12.0) / columns as f32 - 34.0)
                                    .max(0.0)
                                    * progress,
                            )
                            .h(4.0)
                            .radius(2.0)
                            .fill(color),
                        ),
                ),
            );
        } else {
            let share = if total.tokens() == 0 {
                0.0
            } else if index == 1 {
                total.input as f64 / total.tokens() as f64
            } else {
                total.output as f64 / total.tokens() as f64
            };
            metric = metric.child(
                copy(
                    if total.tokens_unreported() {
                        "Not reported".into()
                    } else {
                        format!("{:.1}% of tokens", share * 100.0)
                    },
                    11.0,
                    500.0,
                    theme,
                )
                .h(28.0)
                .color(color),
            );
        }
        grid = grid.child(metric);
    }
    grid
}

pub(crate) struct ActivityPoint {
    pub(crate) id: u64,
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) description: String,
    pub(crate) total: Totals,
    pub(crate) selected: bool,
}

pub(crate) fn activity_chart(
    points: &[ActivityPoint],
    caption: &str,
    on_select: Option<fn(&mut State, u64)>,
    width: f32,
    theme: ThemeStyle<'_>,
) -> El {
    let inner = (width - 42.0).max(120.0);
    let plot_width = (inner - 42.0).max(80.0);
    let peak = points
        .iter()
        .map(|point| point.total.tokens())
        .max()
        .unwrap_or(0)
        .max(1);
    let mut grid = Shape::new();
    for fraction in [0.0, 0.5, 1.0] {
        let y = 176.0 - fraction * 172.0;
        grid = grid.line(
            [0.0, y],
            [plot_width, y],
            0.7,
            LineCap::Butt,
            theme.color("#2a2a2a"),
        );
    }
    let mut bars = row().w_full().h(216.0).gap(2.0);
    for point in points {
        let day = point.total;
        let at = point.id;
        let selected = point.selected;
        let input_height = day.input as f32 / peak as f32 * 166.0;
        let output_height = day.output as f32 / peak as f32 * 166.0;
        let mut bar = col()
            .key(&point.key)
            .role(if on_select.is_some() {
                "button"
            } else {
                "group"
            })
            .label(&point.description)
            .help(&point.description)
            .grow(1.0)
            .min_w(0.0)
            .h(216.0)
            .px(3.0)
            .gap(10.0)
            .radius(7.0)
            .fill(if selected {
                theme.subtle_accent()
            } else {
                Color::TRANSPARENT
            })
            .hover(theme.color("#242424").alpha(0.55))
            .child(
                col()
                    .w_full()
                    .h(180.0)
                    .px(if width < 520.0 { 3.0 } else { 9.0 })
                    .child(el().grow(1.0))
                    .child(
                        el().w_full()
                            .h(output_height)
                            .radius(3.0)
                            .fill(chart_color(3, theme)),
                    )
                    .child(
                        el().w_full()
                            .h(if day.tokens_unreported() {
                                2.0
                            } else {
                                input_height
                            })
                            .radius(3.0)
                            .fill(if day.tokens_unreported() {
                                theme.color("#969696")
                            } else {
                                chart_color(0, theme)
                            }),
                    ),
            )
            .child(
                copy(
                    &point.label,
                    11.0,
                    if selected { 600.0 } else { 400.0 },
                    theme,
                )
                .w_full()
                .align(rune::Align::Center)
                .color(if selected {
                    theme.accent()
                } else {
                    theme.color("#969696")
                }),
            );
        if let Some(select) = on_select {
            bar = bar.on_press(move |state: &mut State| select(state, at));
        }
        bars = bars.child(bar);
    }
    let has_reports = points.iter().any(|point| point.total.measured > 0);
    let ticks = col()
        .w(40.0)
        .h(180.0)
        .child(muted(
            if has_reports {
                compact(peak)
            } else {
                "—".into()
            },
            theme,
        ))
        .child(el().grow(1.0))
        .child(muted(
            if has_reports {
                compact(peak / 2)
            } else {
                "—".into()
            },
            theme,
        ))
        .child(el().grow(1.0))
        .child(muted("0", theme));
    card(theme)
        .key("tracking-model-card")
        .w(width)
        .min_h(350.0)
        .child(copy("Token activity", 16.0, 600.0, theme))
        .child(
            row()
                .w_full()
                .flex_wrap()
                .gap(12.0)
                .child(legend_item("Input".into(), chart_color(0, theme), theme))
                .child(legend_item("Output".into(), chart_color(3, theme), theme)),
        )
        .child(
            row()
                .key("tracking-model-chart")
                .w_full()
                .gap(2.0)
                .child(ticks)
                .child(
                    el().w(plot_width)
                        .h(216.0)
                        .child(
                            canvas(grid.view_box(plot_width, 180.0))
                                .absolute(0.0, 0.0)
                                .size(plot_width, 180.0),
                        )
                        .child(bars),
                ),
        )
        .child(muted(caption, theme))
}

pub(crate) fn mix_chart(total: Totals, width: f32, theme: ThemeStyle<'_>) -> El {
    let mut shape = Shape::new();
    let center = 80.0;
    let radius = 60.0;
    let tau = std::f32::consts::TAU;
    let arc = |start: f32, end: f32| -> Vec<[f32; 2]> {
        (0..=100)
            .map(|i| {
                let a = start + (end - start) * i as f32 / 100.0 - std::f32::consts::FRAC_PI_2;
                [center + radius * a.cos(), center + radius * a.sin()]
            })
            .collect()
    };
    shape = shape.stroke(arc(0.0, tau), 18.0, theme.color("#2a2a2a"));
    let share = if total.tokens() > 0 {
        total.input as f32 / total.tokens() as f32
    } else {
        0.0
    };
    if total.input > 0 {
        shape = shape.stroke(
            arc(0.015, (share * tau - 0.015).max(0.015)),
            18.0,
            chart_color(0, theme),
        );
    }
    if total.output > 0 {
        shape = shape.stroke(
            arc((share * tau + 0.015).min(tau - 0.015), tau - 0.015),
            18.0,
            chart_color(3, theme),
        );
    }
    let ring = el()
        .key("tracking-token-mix")
        .label(if total.tokens_unreported() {
            "Token mix: token counts were not reported".into()
        } else {
            format!(
                "Token mix: {} input and {} output tokens",
                exact(total.input),
                exact(total.output)
            )
        })
        .size(160.0, 160.0)
        .self_center()
        .center()
        .child(
            canvas(shape.view_box(160.0, 160.0))
                .size(160.0, 160.0)
                .absolute(0.0, 0.0),
        )
        .child(
            col()
                .items_center()
                .gap(3.0)
                .child(copy(token_value(total, total.tokens()), 25.0, 600.0, theme))
                .child(muted(
                    if total.tokens_unreported() {
                        "not reported"
                    } else {
                        "tokens"
                    },
                    theme,
                )),
        );
    let mut panel = card(theme)
        .key("tracking-mix-card")
        .w(width)
        .min_h(350.0)
        .child(copy("Input / output", 16.0, 600.0, theme))
        .child(ring);
    for (index, label, value) in [(0, "Input", total.input), (3, "Output", total.output)] {
        let percent = if total.tokens() > 0 {
            value as f64 / total.tokens() as f64 * 100.0
        } else {
            0.0
        };
        panel = panel.child(
            row()
                .w_full()
                .center_y()
                .gap(8.0)
                .child(legend_item(label.into(), chart_color(index, theme), theme))
                .child(el().grow(1.0))
                .child(copy(
                    if total.tokens_unreported() {
                        "—".into()
                    } else {
                        format!("{}  ·  {percent:.1}%", compact(value))
                    },
                    12.0,
                    550.0,
                    theme,
                )),
        );
    }
    panel.child(muted("Cache reads are included in input tokens.", theme))
}

pub(crate) fn model_breakdown(
    total: Totals,
    models: &[ModelTotal],
    width: f32,
    theme: ThemeStyle<'_>,
) -> El {
    let total_tokens = total.tokens();
    let mut panel = card(theme).key("tracking-model-breakdown").child(
        row()
            .w_full()
            .center_y()
            .gap(10.0)
            .child(copy("Models & agents", 16.0, 600.0, theme))
            .child(el().grow(1.0))
            .child(design::badge(
                format!(
                    "{} {}",
                    models.len(),
                    if models.len() == 1 { "model" } else { "models" }
                ),
                theme,
            )),
    );
    if models.is_empty() {
        return panel.child(muted(
            "No model calls match this selection. Choose another day or agent group.",
            theme,
        ));
    }
    for (index, model) in models.iter().enumerate() {
        let total = model.total;
        let share = if total_tokens > 0 {
            total.tokens() as f64 / total_tokens as f64
        } else {
            0.0
        };
        let method = match AuthMethod::try_from(model.method) {
            Ok(AuthMethod::Subscription) => "Subscription",
            Ok(AuthMethod::ApiKey) => "API key",
            _ => "Unknown source",
        };
        let roles = model
            .roles
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        let cache = total
            .cache_rate()
            .map_or_else(|| "Cache —".into(), |v| format!("Cache {v:.1}%"));
        let mut item = col()
            .key(format!("tracking-model-row-{index}"))
            .w_full()
            .min_w(0.0)
            .gap(9.0)
            .py(10.0)
            .child(
                row()
                    .w_full()
                    .center_y()
                    .gap(12.0)
                    .child(
                        el().size(28.0, 28.0)
                            .radius(8.0)
                            .center()
                            .fill(chart_color(index, theme).alpha(0.12))
                            .child(
                                copy(format!("{:02}", index + 1), 11.0, 600.0, theme)
                                    .color(chart_color(index, theme)),
                            ),
                    )
                    .child(
                        col()
                            .grow(1.0)
                            .min_w(0.0)
                            .gap(3.0)
                            .child(
                                copy(&model.model, 14.0, 550.0, theme)
                                    .min_w(0.0)
                                    .truncate_fade()
                                    .help(&model.model),
                            )
                            .child(muted(
                                format!("{} · {method} · {roles}", provider_name(model.provider)),
                                theme,
                            )),
                    )
                    .child(
                        col()
                            .items_end()
                            .gap(3.0)
                            .child(
                                copy(token_value(total, total.tokens()), 16.0, 600.0, theme).help(
                                    if total.tokens_unreported() {
                                        "Token counts were not reported".into()
                                    } else {
                                        format!("{} tokens", exact(total.tokens()))
                                    },
                                ),
                            )
                            .child(muted(
                                if total.tokens_unreported() {
                                    "Not reported".into()
                                } else {
                                    format!("{:.1}% of total", share * 100.0)
                                },
                                theme,
                            )),
                    ),
            )
            .child(
                el().w_full()
                    .h(4.0)
                    .radius(2.0)
                    .fill(theme.color("#2a2a2a"))
                    .child(
                        el().w(((width - 42.0) * share as f32).max(0.0))
                            .h(4.0)
                            .radius(2.0)
                            .fill(chart_color(index, theme)),
                    ),
            );
        item = item.child(
            row()
                .w_full()
                .flex_wrap()
                .gap(14.0)
                .child(muted(
                    format!(
                        "{} input",
                        if total.tokens_unreported() {
                            "—".into()
                        } else {
                            exact(total.input)
                        }
                    ),
                    theme,
                ))
                .child(muted(
                    format!(
                        "{} output",
                        if total.tokens_unreported() {
                            "—".into()
                        } else {
                            exact(total.output)
                        }
                    ),
                    theme,
                ))
                .child(muted(
                    format!("{}/{} calls measured", total.measured, total.calls),
                    theme,
                ))
                .child(muted(cache, theme)),
        );
        panel = panel.child(item);
        if index + 1 < models.len() {
            panel = panel.child(el().w_full().h(1.0).fill(theme.color("#2a2a2a")));
        }
    }
    panel
}
