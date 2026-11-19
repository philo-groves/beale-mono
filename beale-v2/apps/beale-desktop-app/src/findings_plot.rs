//! Exact screening coordinates: missing ratings never become zero-valued observations.
use super::*;
use crate::design::FadeText;
use rune::{LineCap, Shape, canvas};
use std::collections::BTreeMap;

pub(super) fn scores(record: &Summary) -> Option<(u8, u8)> {
    let impact = record.impact_score.filter(|score| *score <= 100)?;
    let reachability = record.reachability_score.filter(|score| *score <= 100)?;
    Some((impact, reachability))
}

fn coordinate(impact: u8, reachability: u8, bounds: [f32; 4]) -> [f32; 2] {
    [
        bounds[0] + bounds[2] * f32::from(reachability) / 100.0,
        bounds[1] + bounds[3] * (1.0 - f32::from(impact) / 100.0),
    ]
}

fn status_color(status: Status, theme: ThemeStyle<'_>) -> rune::Color {
    theme.color(
        LANES
            .iter()
            .find(|lane| lane.0 == status)
            .map_or("#999999", |lane| lane.2),
    )
}

fn overlaps(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[0] + b[2] + 4.0
        && a[0] + a[2] + 4.0 > b[0]
        && a[1] < b[1] + b[3] + 4.0
        && a[1] + a[3] + 4.0 > b[1]
}

pub(super) fn render(state: &State, theme: ThemeStyle<'_>, width: f32) -> El {
    let board = &state.workspace_findings;
    let mut groups: BTreeMap<(u8, u8), Vec<&Summary>> = BTreeMap::new();
    let mut unrated = Vec::new();
    for record in board
        .records
        .iter()
        .filter(|record| board.plot_stage_visible(record.status))
    {
        if let Some(pair) = scores(record) {
            groups.entry(pair).or_default().push(record);
        } else {
            unrated.push(record);
        }
    }
    let graph_width = (width - 88.0).max(80.0);
    let graph_height = (graph_width * 0.55).clamp(260.0, 420.0);
    let bounds = [52.0, 66.0, graph_width, graph_height];
    let [left, top, w, h] = bounds;
    let right = left + w;
    let bottom = top + h;
    let card_height = bottom + 85.0;
    let middle = [left + w / 2.0, top + h / 2.0];
    let mut graph = el()
        .key("findings-plot")
        .size(width, card_height)
        .radius(16.0)
        .continuous_corners()
        .fill(theme.color("#171717"))
        .border(1.0, theme.color("#303030"))
        .child(
            text("Impact ↑")
                .absolute(left, 21.0)
                .font(ui_font(13.0, 600.0))
                .color(theme.color("#dddddd")),
        )
        .child(
            text(format!(
                "{} rated",
                groups.values().map(Vec::len).sum::<usize>()
            ))
            .key("findings-plot-rated-count")
            .absolute((right - 90.0).max(left), 22.0)
            .w(90.0)
            .font(ui_font(11.0, 500.0))
            .color(theme.color("#999999")),
        );
    for (x, y, label, tint) in [
        (
            left,
            top,
            "Higher impact\nLower reachability",
            theme.color("#7aa9e6"),
        ),
        (
            middle[0],
            top,
            "Higher impact\nHigher reachability",
            theme.color("#dca15f"),
        ),
        (
            left,
            middle[1],
            "Lower impact\nLower reachability",
            theme.color("#888888"),
        ),
        (
            middle[0],
            middle[1],
            "Lower impact\nHigher reachability",
            theme.accent(),
        ),
    ] {
        graph = graph
            .child(
                el().absolute(x, y)
                    .size(w / 2.0, h / 2.0)
                    .fill(tint.alpha(0.035)),
            )
            .child(
                text(label)
                    .absolute(x + 12.0, y + 12.0)
                    .w(w / 2.0 - 24.0)
                    .wrap()
                    .font(ui_font(10.0, 500.0))
                    .color(theme.color("#888888").alpha(0.8)),
            );
    }
    let mut grid = Shape::new();
    for tick in [0, 25, 50, 75, 100] {
        let x = left + w * tick as f32 / 100.0;
        let y = bottom - h * tick as f32 / 100.0;
        let color = theme.color(if tick == 50 { "#555555" } else { "#303030" });
        grid = grid
            .line(
                [x, top],
                [x, bottom],
                if tick == 50 { 1.2 } else { 0.7 },
                LineCap::Butt,
                color,
            )
            .line(
                [left, y],
                [right, y],
                if tick == 50 { 1.2 } else { 0.7 },
                LineCap::Butt,
                color,
            );
        graph = graph
            .child(
                text(tick.to_string())
                    .absolute(x - 10.0, bottom + 10.0)
                    .w(30.0)
                    .font(ui_font(10.0, 500.0))
                    .color(theme.color("#999999")),
            )
            .child(
                text(tick.to_string())
                    .absolute(18.0, y - 6.0)
                    .w(28.0)
                    .font(ui_font(10.0, 500.0))
                    .color(theme.color("#999999")),
            );
    }
    grid = grid
        .fill(
            vec![
                [middle[0], top - 5.0],
                [middle[0] - 3.5, top + 2.0],
                [middle[0] + 3.5, top + 2.0],
            ],
            theme.color("#777777"),
        )
        .fill(
            vec![
                [right + 5.0, middle[1]],
                [right - 2.0, middle[1] - 3.5],
                [right - 2.0, middle[1] + 3.5],
            ],
            theme.color("#777777"),
        );
    graph = graph
        .child(canvas(grid).absolute(0.0, 0.0).size(width, card_height))
        .child(
            text("Reachability →")
                .absolute((right - 130.0).max(left), bottom + 36.0)
                .font(ui_font(13.0, 600.0))
                .color(theme.color("#dddddd")),
        )
        .child(
            text("Scores · 0–100")
                .absolute(left, bottom + 38.0)
                .font(ui_font(11.0, 400.0))
                .color(theme.color("#888888")),
        );
    let mut labels = Vec::<[f32; 4]>::new();
    let positions: Vec<_> = groups
        .keys()
        .map(|&(impact, reach)| coordinate(impact, reach, bounds))
        .collect();
    for (&pair, records) in &groups {
        let (impact, reachability) = pair;
        let [x, y] = coordinate(impact, reachability, bounds);
        let count = records.len();
        let first = records[0];
        let color = if records.iter().all(|record| record.status == first.status) {
            status_color(first.status, theme)
        } else {
            theme.accent()
        };
        let label = if count == 1 {
            format!("{} · {}", first.id, first.title)
        } else {
            format!("{count} findings at this point")
        };
        let stage = if records.iter().all(|record| record.status == first.status) {
            let name = LANES
                .iter()
                .find(|lane| lane.0 == first.status)
                .map_or("Unknown", |lane| lane.1);
            if matches!(first.status, Status::Candidate | Status::Validated) {
                format!("{name} · draft ratings")
            } else {
                name.to_owned()
            }
        } else {
            "Mixed stages".to_owned()
        };
        let description =
            format!("{label}\nImpact {impact}/100 · Reachability {reachability}/100\n{stage}");
        let id = first.id.clone();
        let selected = board.plot_selection == Some(pair);
        let mut marker = el()
            .key(format!("finding-point-{impact}-{reachability}"))
            .role("button")
            .label(description.clone())
            .help(description)
            .absolute(x - 13.0, y - 13.0)
            .size(26.0, 26.0)
            .center()
            .radius(13.0)
            .hover(color.alpha(0.18))
            .child(
                el().size(
                    if count > 1 { 23.0 } else { 14.0 },
                    if count > 1 { 23.0 } else { 14.0 },
                )
                .radius(12.0)
                .fill(color)
                .border(if selected { 3.0 } else { 2.0 }, theme.color("#171717"))
                .center()
                .child(if count > 1 {
                    text(count.to_string())
                        .font(ui_font(10.0, 650.0))
                        .color(theme.color("#111111"))
                } else {
                    el()
                }),
            );
        marker = marker.on_press_tx(move |state: &mut State, tx| {
            if count == 1 {
                open_record(state, id.clone());
            } else {
                state.workspace_findings.plot_selection = Some(pair);
                tx.scroll_into_view("findings-plot-selection");
            }
        });
        graph = graph.child(marker);
        if count == 1 {
            let label_width =
                rune::text::measure(&first.id, &[], ui_font(10.5, 600.0), None).0 + 10.0;
            let label_height = 20.0 * state.appearance.text_size.factor();
            for [dx, dy] in [
                [15.0, -10.0],
                [-label_width - 15.0, -10.0],
                [-label_width / 2.0, 15.0],
                [15.0, -label_height - 4.0],
                [-label_width - 15.0, 4.0],
            ] {
                let rect = [x + dx, y + dy, label_width, label_height];
                if rect[0] < left
                    || rect[0] + rect[2] > right
                    || rect[1] < top
                    || rect[1] + rect[3] > bottom
                    || labels.iter().any(|other| overlaps(rect, *other))
                    || positions
                        .iter()
                        .any(|p| overlaps(rect, [p[0] - 10.0, p[1] - 10.0, 20.0, 20.0]))
                {
                    continue;
                }
                labels.push(rect);
                graph = graph.child(
                    text(first.id.clone())
                        .absolute(rect[0], rect[1])
                        .size(rect[2], rect[3])
                        .px(5.0)
                        .py(3.0)
                        .radius(4.0)
                        .fill(theme.color("#171717"))
                        .font(ui_font(10.5, 600.0))
                        .color(color),
                );
                break;
            }
        }
    }
    if groups.is_empty() {
        let all_hidden = LANES.iter().all(|lane| !board.plot_stage_visible(lane.0));
        let (title, description) = if all_hidden {
            (
                "No stages selected",
                "Use the legend below to show findings.",
            )
        } else if !board.hidden_plot_stages.is_empty() {
            (
                "No rated findings in visible stages",
                "Show another stage or record impact and reachability scores during screening.",
            )
        } else {
            (
                "No rated findings yet",
                "Record impact and reachability scores during screening to place findings here.",
            )
        };
        graph = graph.child(
            col()
                .key("findings-plot-empty")
                .absolute(left + 16.0, middle[1] - 38.0)
                .w(w - 32.0)
                .padding(16.0)
                .gap(7.0)
                .radius(10.0)
                .fill(theme.color("#191919"))
                .child(
                    text(title)
                        .w_full()
                        .wrap()
                        .font(ui_font(16.0, 600.0))
                        .color(theme.color("#dddddd")),
                )
                .child(
                    text(description)
                        .w_full()
                        .wrap()
                        .font(ui_font(12.0, 400.0))
                        .color(theme.color("#999999")),
                ),
        );
    }
    let mut legend = row()
        .key("findings-plot-legend")
        .w_full()
        .flex_wrap()
        .center_y()
        .gap(8.0);
    for (status, label, color) in LANES {
        let visible = board.plot_stage_visible(status);
        let color = theme.color(color);
        let count = board
            .records
            .iter()
            .filter(|record| record.status == status && scores(record).is_some())
            .count();
        legend = legend.child(
            row()
                .key(format!("findings-plot-stage-{}", label.to_lowercase()))
                .role("switch")
                .label(format!(
                    "{label}, {}, {count} rated findings",
                    if visible { "shown" } else { "hidden" }
                ))
                .help(format!(
                    "{} {label} findings",
                    if visible { "Hide" } else { "Show" }
                ))
                .h(32.0)
                .px(10.0)
                .gap(7.0)
                .center_y()
                .radius(8.0)
                .fill(if visible {
                    color.alpha(0.08)
                } else {
                    theme.color("#171717")
                })
                .border(
                    1.0,
                    if visible {
                        color.alpha(0.3)
                    } else {
                        theme.color("#303030")
                    },
                )
                .hover(theme.color("#2d2d2d"))
                .child(
                    el().size(7.0, 7.0)
                        .radius(4.0)
                        .fill(if visible {
                            color
                        } else {
                            rune::Color::TRANSPARENT
                        })
                        .border(
                            1.0,
                            if visible {
                                color
                            } else {
                                theme.color("#777777")
                            },
                        ),
                )
                .child(
                    text(label)
                        .font(ui_font(11.0, 500.0))
                        .color(theme.color(if visible { "#dddddd" } else { "#888888" })),
                )
                .child(
                    text(count.to_string())
                        .font(ui_font(10.0, 500.0))
                        .color(theme.color(if visible { "#b8b8b8" } else { "#777777" })),
                )
                .on_press(move |state: &mut State| {
                    state.workspace_findings.toggle_plot_stage(status)
                }),
        );
    }
    let mut content = col().w_full().gap(12.0).child(graph).child(legend);
    if let Some(pair) = board.plot_selection
        && let Some(records) = groups.get(&pair)
    {
        let mut selected = col()
            .key("findings-plot-selection")
            .w_full()
            .gap(10.0)
            .padding(14.0)
            .radius(12.0)
            .fill(theme.color("#191919"))
            .border(1.0, theme.color("#303030"))
            .child(
                row()
                    .w_full()
                    .center_y()
                    .gap(10.0)
                    .child(
                        text(format!("Impact {} · Reachability {}", pair.0, pair.1))
                            .grow(1.0)
                            .min_w(0.0)
                            .wrap()
                            .font(ui_font(13.0, 600.0))
                            .color(theme.color("#dddddd")),
                    )
                    .child(
                        form_button("Close", theme)
                            .key("findings-plot-selection-close")
                            .on_press(|state: &mut State| {
                                state.workspace_findings.plot_selection = None
                            }),
                    ),
            );
        for record in records {
            selected = selected.child(card(record, theme));
        }
        content = content.child(selected);
    }
    if !unrated.is_empty() {
        content = content.child(
            row()
                .key("findings-unrated-toggle")
                .role("button")
                .label(format!(
                    "{} unrated findings, {}",
                    unrated.len(),
                    if board.unrated_expanded {
                        "expanded"
                    } else {
                        "collapsed"
                    }
                ))
                .w_full()
                .padding(12.0)
                .center_y()
                .gap(10.0)
                .radius(10.0)
                .fill(theme.color("#171717"))
                .hover(theme.color("#222222"))
                .child(
                    text(format!("Unrated · {}", unrated.len()))
                        .font(ui_font(13.0, 550.0))
                        .color(theme.color("#b8b8b8")),
                )
                .child(
                    text("Missing one or both scores")
                        .grow(1.0)
                        .min_w(0.0)
                        .fade_truncate()
                        .font(ui_font(11.0, 400.0))
                        .color(theme.color("#888888")),
                )
                .child(
                    text(if board.unrated_expanded {
                        "Hide"
                    } else {
                        "Show"
                    })
                    .font(ui_font(12.0, 500.0))
                    .color(theme.accent()),
                )
                .on_press(|state: &mut State| {
                    state.workspace_findings.unrated_expanded =
                        !state.workspace_findings.unrated_expanded
                }),
        );
        if board.unrated_expanded {
            for record in unrated {
                content = content.child(card(record, theme));
            }
        }
    }
    content
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axes_place_high_reachability_and_impact_in_the_upper_right() {
        let bounds = [10.0, 20.0, 400.0, 300.0];
        assert_eq!(coordinate(100, 100, bounds), [410.0, 20.0]);
        assert_eq!(coordinate(0, 0, bounds), [10.0, 320.0]);
        assert_eq!(coordinate(50, 50, bounds), [210.0, 170.0]);
    }
}
