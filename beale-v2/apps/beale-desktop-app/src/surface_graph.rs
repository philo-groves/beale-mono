//! Surface overview: recorded properties and relationships, with no inferred risk scores.
use super::*;
use crate::design::FadeText;
use crate::workspace_surface_layout::{self as atlas, Focus, NODE_HEIGHT, NODE_MAX_WIDTH};
use rune::{Drag, LineCap, Phase, Shape, Tx, canvas};

fn background_drag(state: &mut State, drag: &mut Drag, tx: &mut Tx) {
    tx.instant();
    match drag.phase {
        Phase::Down => state.workspace_surface_pan_origin = state.workspace_surface_pan,
        Phase::Move => {
            state.workspace_surface_pan = [
                state.workspace_surface_pan_origin[0] + drag.dx,
                state.workspace_surface_pan_origin[1] + drag.dy,
            ]
        }
        Phase::Up if drag.cancelled() => {
            state.workspace_surface_pan = state.workspace_surface_pan_origin
        }
        Phase::Up if drag.dx.abs() <= 4.0 && drag.dy.abs() <= 4.0 => {
            state.workspace_surface_selected = None;
            state.workspace_surface_connecting = None;
        }
        Phase::Up => {}
    }
}

pub(super) fn scope(surface: &WorkspaceSurface) -> &'static str {
    match SurfaceScope::try_from(surface_scope(surface)) {
        Ok(SurfaceScope::InScope) => "In scope",
        Ok(SurfaceScope::OutOfScope) => "Out of scope",
        _ => "Scope unknown",
    }
}

fn scope_color(surface: &WorkspaceSurface, theme: ThemeStyle<'_>) -> Color {
    match SurfaceScope::try_from(surface_scope(surface)) {
        Ok(SurfaceScope::InScope) => theme.accent(),
        Ok(SurfaceScope::OutOfScope) => theme.color("#e07676"),
        _ => theme.color("#dca15f"),
    }
}

fn pill(value: &str, color: Color, zoom: f32) -> El {
    row()
        .center_y()
        .px(6.0 * zoom)
        .h(19.0 * zoom)
        .radius(5.0 * zoom)
        .fill(color.alpha(0.09))
        .child(
            text(value)
                .font(ui_font(10.0 * zoom.max(0.84), 550.0))
                .color(color),
        )
}

fn node(
    state: &State,
    theme: ThemeStyle<'_>,
    index: usize,
    point: [f32; 2],
    zoom: f32,
    max_updates: u64,
) -> El {
    let surface = &state.workspace_surface_edits[index];
    let definition = surface.definition.as_ref();
    let name = definition
        .map(|value| value.name.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            if surface_value(surface).is_empty() {
                &surface.id
            } else {
                surface_value(surface)
            }
        });
    let type_id = definition
        .map(|value| value.type_id.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or("Unspecified type");
    let selected = state.workspace_surface_selected == Some(index);
    let hovered = state.workspace_surface_hovered == Some(index);
    let matches = atlas::matches(
        surface,
        state.workspace_surface_focus,
        &state.workspace_surface_query.text,
    );
    let evidence = atlas::evidence_count(surface);
    let links = surface.relationships.len()
        + state
            .workspace_surface_edits
            .iter()
            .enumerate()
            .filter(|(source, _)| *source != index)
            .map(|(_, source)| {
                source
                    .relationships
                    .iter()
                    .filter(|link| link.target_id == surface.id)
                    .count()
            })
            .sum::<usize>();
    let revision = surface_revision(surface);
    let updates = revision.saturating_sub(1);
    let reviewed = surface_scope_state(surface) == SurfaceScopeState::Approved as i32;
    let review_color = theme.color(if reviewed { "#77bf93" } else { "#dca15f" });
    let scope_color = scope_color(surface, theme);
    let mut bars = row().h(14.0 * zoom).center_y().gap(2.0 * zoom);
    for bar in 0..5 {
        let active = updates > 0 && (bar as f32 / 5.0) < updates as f32 / max_updates.max(1) as f32;
        bars = bars.child(
            el().size(3.0 * zoom, (4.0 + bar as f32 * 2.0) * zoom)
                .radius(1.0)
                .fill(if active {
                    theme.accent().alpha(0.75)
                } else {
                    theme.color("#3a3a3a").alpha(0.5)
                }),
        );
    }
    let label = format!(
        "{name}; {type_id}; {}; {}; {}; {evidence} evidence references; {links} connections; {updates} updates",
        surface_value(surface),
        scope(surface),
        scope_state_label(surface_scope_state(surface))
    );
    let card = col()
        .key(format!("workspace-surface-node-{index}"))
        .role("button")
        .label(label)
        .absolute(
            point[0] - NODE_MAX_WIDTH * zoom / 2.0,
            point[1] - NODE_HEIGHT * zoom / 2.0,
        )
        .size(NODE_MAX_WIDTH * zoom, NODE_HEIGHT * zoom)
        .padding(12.0 * zoom)
        .gap(6.0 * zoom)
        .radius(13.0 * zoom)
        .continuous_corners()
        .fill(theme.color(if selected { "#282828" } else { "#191919" }))
        .border(
            if selected { 1.8 } else { 1.0 },
            if selected {
                theme.accent()
            } else {
                theme.color("#3a3a3a")
            },
        )
        .shadow(
            3.0,
            12.0,
            Color::BLACK.alpha(if theme.is_dark() { 0.22 } else { 0.05 }),
        )
        .hover(theme.color("#242424"))
        .opacity(if matches || selected || hovered {
            1.0
        } else {
            0.30
        });
    let card = if zoom < 0.6 {
        let text_scale = (zoom / 0.45).min(1.0);
        card.gap(5.0 * zoom)
            .child(
                row()
                    .w_full()
                    .center_y()
                    .gap(4.0 * zoom)
                    .child(
                        icon(surface_icon(surface_kind(surface)), 10.0 * text_scale)
                            .color(theme.accent()),
                    )
                    .child(
                        text(surface.id.clone())
                            .font(ui_font(8.0 * text_scale, 500.0))
                            .color(theme.color("#999999")),
                    )
                    .child(el().grow(1.0))
                    .child(
                        text(scope_state_label(surface_scope_state(surface)))
                            .font(ui_font(7.5 * text_scale, 500.0))
                            .color(review_color),
                    ),
            )
            .child(
                text(name.to_owned())
                    .key(format!("workspace-surface-visible-name-{index}"))
                    .w_full()
                    .min_w(0.0)
                    .fade_truncate()
                    .font(ui_font(11.5 * text_scale, 600.0))
                    .color(theme.color("#eeeeee")),
            )
            .child(
                row()
                    .w_full()
                    .center_y()
                    .gap(4.0 * zoom)
                    .child(
                        el().size(5.0 * text_scale, 5.0 * text_scale)
                            .radius(3.0)
                            .fill(scope_color),
                    )
                    .child(
                        text(scope(surface))
                            .font(ui_font(8.0 * text_scale, 500.0))
                            .color(scope_color),
                    )
                    .child(el().grow(1.0))
                    .child(
                        text(format!(
                            "{evidence} {}",
                            if evidence == 1 { "ref" } else { "refs" }
                        ))
                        .font(ui_font(8.0 * text_scale, 500.0))
                        .color(theme.color(if evidence == 0 { "#dca15f" } else { "#999999" })),
                    ),
            )
    } else {
        card.child(
            row()
                .w_full()
                .min_w(0.0)
                .h(16.0 * zoom)
                .center_y()
                .gap(6.0 * zoom)
                .child(icon(surface_icon(surface_kind(surface)), 13.0 * zoom).color(theme.accent()))
                .child(
                    text(type_id.to_owned())
                        .grow(1.0)
                        .min_w(0.0)
                        .fade_truncate()
                        .font(ui_font(10.5 * zoom, 500.0))
                        .color(theme.color("#999999")),
                )
                .child(
                    text(surface.id.clone())
                        .font(ui_font(9.0 * zoom, 500.0))
                        .color(theme.color("#888888")),
                ),
        )
        .child(
            text(name.to_owned())
                .key(format!("workspace-surface-visible-name-{index}"))
                .w_full()
                .min_w(0.0)
                .fade_truncate()
                .font(ui_font(14.5 * zoom.max(0.84), 600.0))
                .color(theme.color("#eeeeee")),
        )
        .child(if zoom < 0.8 {
            el().h(0.0)
        } else {
            text(
                if surface_value(surface).is_empty() {
                    "No locator recorded"
                } else {
                    surface_value(surface)
                }
                .to_owned(),
            )
            .w_full()
            .fade_truncate()
            .font(ui_font(10.5 * zoom, 400.0))
            .color(theme.color("#999999"))
        })
        .child(
            row()
                .w_full()
                .gap(5.0 * zoom)
                .child(pill(scope(surface), scope_color, zoom))
                .child(pill(
                    scope_state_label(surface_scope_state(surface)),
                    review_color,
                    zoom,
                )),
        )
        .child(el().w_full().h(1.0).fill(theme.color("#303030").alpha(0.7)))
        .child(
            row()
                .w_full()
                .center_y()
                .gap(5.0 * zoom)
                .child(
                    icon(&FINDINGS, 11.0 * zoom).color(theme.color(if evidence == 0 {
                        "#dca15f"
                    } else {
                        "#999999"
                    })),
                )
                .child(
                    text(format!(
                        "{evidence} {}",
                        if evidence == 1 { "ref" } else { "refs" }
                    ))
                    .font(ui_font(10.0 * zoom.max(0.84), 500.0))
                    .color(theme.color("#b8b8b8")),
                )
                .child(el().grow(1.0))
                .child(icon(&WORKSPACE_SURFACES, 11.0 * zoom).color(theme.color("#999999")))
                .child(
                    text(links.to_string())
                        .font(ui_font(10.0 * zoom.max(0.84), 500.0))
                        .color(theme.color("#b8b8b8")),
                )
                .child(el().grow(1.0))
                .child(bars)
                .child(
                    text(if revision == 0 {
                        "No history".to_owned()
                    } else {
                        format!(
                            "{updates} {}",
                            if updates == 1 { "update" } else { "updates" }
                        )
                    })
                    .font(ui_font(9.5 * zoom, 400.0))
                    .color(theme.color("#999999")),
                ),
        )
    };
    card.on_hover_change(move |state: &mut State, inside, _| {
        if inside {
            state.workspace_surface_hovered = Some(index);
        } else if state.workspace_surface_hovered == Some(index) {
            state.workspace_surface_hovered = None;
        }
    })
    .on_press_tx(move |state: &mut State, tx| {
        tx.instant();
        if let Some(source) = state.workspace_surface_connecting.take() {
            if source != index {
                state.workspace_surface_relationship_target = Some(index);
                state.workspace_surface_relationship_label = TextEdit::new("");
                state.workspace_surface_relationship_type_id = TextEdit::new("");
                state.workspace_surface_relationship_error.clear();
                state.workspace_surface_selected = Some(source);
            }
        } else {
            state.workspace_surface_selected = Some(index);
        }
    })
}

fn overview(state: &State, theme: ThemeStyle<'_>, width: f32) -> El {
    let surfaces = &state.workspace_surface_edits;
    let total = surfaces.len();
    let in_scope = surfaces
        .iter()
        .filter(|surface| surface_scope(surface) == SurfaceScope::InScope as i32)
        .count();
    let cells = [
        (
            Focus::All,
            "All surfaces",
            total,
            format!("{in_scope} in scope"),
            theme.accent(),
            &WORKSPACE_SURFACES,
        ),
        (
            Focus::Review,
            "Needs review",
            surfaces
                .iter()
                .filter(|surface| atlas::needs_review(surface))
                .count(),
            "Scope approval".to_owned(),
            theme.color("#dca15f"),
            &SAFETY,
        ),
        (
            Focus::Unevidenced,
            "No evidence",
            surfaces
                .iter()
                .filter(|surface| atlas::evidence_count(surface) == 0)
                .count(),
            "No references attached".to_owned(),
            theme.color("#999999"),
            &FINDINGS,
        ),
        (
            Focus::Excluded,
            "Out of scope",
            surfaces
                .iter()
                .filter(|surface| surface_scope(surface) == SurfaceScope::OutOfScope as i32)
                .count(),
            "Boundary references".to_owned(),
            theme.color("#e07676"),
            &crate::icons::CLOSE,
        ),
    ];
    let columns = if width < 602.0 { 2.0 } else { 4.0 };
    let cell_width = ((width - (columns - 1.0) * 8.0) / columns).min(220.0);
    let mut overview = row()
        .key("workspace-surface-overview")
        .w(width)
        .flex_wrap()
        .gap(8.0);
    for (focus, label, count, caption, color, glyph) in cells {
        let selected = state.workspace_surface_focus == focus;
        overview = overview.child(
            row()
                .key(format!("workspace-surface-focus-{focus:?}"))
                .role("button")
                .label(format!(
                    "{label}: {count}{}",
                    if selected { ", selected" } else { "" }
                ))
                .w(cell_width)
                .h(48.0)
                .px(10.0)
                .gap(9.0)
                .center_y()
                .radius(8.0)
                .fill(theme.color("#191919"))
                .border(
                    1.0,
                    if selected {
                        color.alpha(0.5)
                    } else {
                        theme.color("#303030")
                    },
                )
                .hover(theme.color("#242424"))
                .child(
                    col().gap(3.0).child(
                        text(count.to_string())
                            .font(ui_font(20.0, 600.0))
                            .color(color),
                    ),
                )
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(3.0)
                        .child(
                            row()
                                .center_y()
                                .gap(5.0)
                                .child(icon(glyph, 11.0).color(color))
                                .child(
                                    text(label)
                                        .font(ui_font(11.0, 550.0))
                                        .color(theme.color("#dddddd")),
                                ),
                        )
                        .child(
                            text(caption)
                                .fade_truncate()
                                .font(ui_font(9.5, 400.0))
                                .color(theme.color("#888888")),
                        ),
                )
                .on_press(move |state: &mut State| state.workspace_surface_focus = focus),
        );
    }
    overview
}

fn curve(from: [f32; 2], to: [f32; 2], zoom: f32) -> Vec<[f32; 2]> {
    let horizontal = (to[1] - from[1]).abs() < 1.0;
    let mut start = from;
    let mut end = to;
    let sign = if horizontal {
        (to[0] - from[0]).signum()
    } else {
        (to[1] - from[1]).signum()
    };
    let axis = usize::from(!horizontal);
    let half = if horizontal {
        NODE_MAX_WIDTH
    } else {
        NODE_HEIGHT
    } * zoom
        / 2.0;
    start[axis] += sign * (half + 2.0);
    end[axis] -= sign * (half + 6.0);
    let mut control_a = start;
    let mut control_b = end;
    control_a[axis] = (start[axis] + end[axis]) / 2.0;
    control_b[axis] = control_a[axis];
    // Route long links around intervening cards in the same row or column.
    if (from[1] - to[1]).abs() < 1.0
        && (from[0] - to[0]).abs() > (NODE_MAX_WIDTH + atlas::COLUMN_GAP) * zoom * 1.5
    {
        start = [from[0], from[1] - NODE_HEIGHT * zoom / 2.0 - 2.0];
        end = [to[0], to[1] - NODE_HEIGHT * zoom / 2.0 - 6.0];
        control_a = [start[0], start[1] - 62.0 * zoom];
        control_b = [end[0], end[1] - 62.0 * zoom];
    } else if (from[0] - to[0]).abs() < 1.0
        && (from[1] - to[1]).abs() > (NODE_HEIGHT + atlas::ROW_GAP) * zoom * 1.5
    {
        start = [from[0] + NODE_MAX_WIDTH * zoom / 2.0 + 2.0, from[1]];
        end = [to[0] + NODE_MAX_WIDTH * zoom / 2.0 + 6.0, to[1]];
        control_a = [start[0] + 60.0 * zoom, start[1]];
        control_b = [end[0] + 60.0 * zoom, end[1]];
    }
    (0..=32)
        .map(|step| {
            let t = step as f32 / 32.0;
            let u = 1.0 - t;
            [0, 1].map(|axis| {
                u.powi(3) * start[axis]
                    + 3.0 * u.powi(2) * t * control_a[axis]
                    + 3.0 * u * t.powi(2) * control_b[axis]
                    + t.powi(3) * end[axis]
            })
        })
        .collect()
}

fn overlaps(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[0] + b[2] + 3.0
        && a[0] + a[2] + 3.0 > b[0]
        && a[1] < b[1] + b[3] + 3.0
        && a[1] + a[3] + 3.0 > b[1]
}

/// Whether any sampled point of `path` (vertices and segment midpoints) falls inside `rect`
/// grown by a small margin.
fn path_crosses(rect: [f32; 4], path: &[[f32; 2]]) -> bool {
    let inside = |[x, y]: [f32; 2]| {
        x > rect[0] - 3.0
            && x < rect[0] + rect[2] + 3.0
            && y > rect[1] - 3.0
            && y < rect[1] + rect[3] + 3.0
    };
    path.iter().copied().any(inside)
        || path.windows(2).any(|pair| {
            inside([
                (pair[0][0] + pair[1][0]) / 2.0,
                (pair[0][1] + pair[1][1]) / 2.0,
            ])
        })
}

/// A label rect for the edge `points`: centred on the edge where possible, so the caption reads
/// as part of its own line, otherwise beside it. Spots must stay in the viewport and clear of
/// cards and labels; one clear of other edges is preferred, but a caption is never dropped
/// only because another edge passes nearby.
fn label_position(
    points: &[[f32; 2]],
    width: f32,
    height: f32,
    cards: &[[f32; 4]],
    labels: &[[f32; 4]],
    other_edges: &[&[[f32; 2]]],
    viewport: [f32; 4],
) -> Option<[f32; 4]> {
    let candidates = || label_candidates(points, width, height);
    let open = |rect: &[f32; 4]| {
        rect[0] >= viewport[0]
            && rect[1] >= viewport[1]
            && rect[0] + rect[2] <= viewport[0] + viewport[2]
            && rect[1] + rect[3] <= viewport[1] + viewport[3]
            && !cards
                .iter()
                .chain(labels)
                .any(|other| overlaps(*rect, *other))
    };
    candidates()
        .find(|rect| open(rect) && !other_edges.iter().any(|path| path_crosses(*rect, path)))
        .or_else(|| candidates().find(open))
}

fn label_candidates(
    points: &[[f32; 2]],
    width: f32,
    height: f32,
) -> impl Iterator<Item = [f32; 4]> + '_ {
    let centred = [16, 12, 20, 10, 22, 8, 24].into_iter().map(move |sample| {
        let [x, y] = points[sample];
        [x - width / 2.0, y - height / 2.0, width, height]
    });
    let beside = [16, 12, 20, 8, 24].into_iter().flat_map(move |sample| {
        let [x, y] = points[sample];
        [
            [-width / 2.0, -height - 6.0],
            [8.0, -height / 2.0],
            [-width / 2.0, 6.0],
            [-width - 8.0, -height / 2.0],
        ]
        .map(|[dx, dy]| [x + dx, y + dy, width, height])
    });
    centred.chain(beside)
}

/// Dash pattern for edges that cross the recorded scope boundary, measured along the curve.
const BOUNDARY_DASH: f32 = 6.0;
const BOUNDARY_GAP: f32 = 4.5;
/// The last stretch of a dashed edge is solid so the arrowhead always meets its line.
const BOUNDARY_SOLID_TAIL: f32 = 12.0;
const ARROW_LENGTH: f32 = 7.0;

fn lerp(a: [f32; 2], b: [f32; 2], t: f32) -> [f32; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

/// The drawn `[start, end]` runs of a boundary edge, as distances along it: even dashes,
/// then one solid run into the arrowhead.
fn dash_runs(total: f32) -> Vec<[f32; 2]> {
    let solid_from = (total - BOUNDARY_SOLID_TAIL).max(0.0);
    let mut runs = Vec::new();
    let mut at = 0.0;
    while at < solid_from {
        runs.push([at, (at + BOUNDARY_DASH).min(solid_from)]);
        at += BOUNDARY_DASH + BOUNDARY_GAP;
    }
    match runs.last_mut() {
        // A dash that reaches the tail joins it rather than leaving a sliver of gap.
        Some(last) if last[1] >= solid_from => last[1] = total,
        _ => runs.push([solid_from, total]),
    }
    runs
}

fn arrow(
    mut shape: Shape,
    points: &[[f32; 2]],
    color: Color,
    active: bool,
    boundary: bool,
) -> Shape {
    let width = if active { 2.0 } else { 1.35 };
    if boundary {
        let lengths: Vec<f32> = points
            .windows(2)
            .map(|pair| (pair[1][0] - pair[0][0]).hypot(pair[1][1] - pair[0][1]))
            .collect();
        let total: f32 = lengths.iter().sum();
        // Point at distance `d` along the polyline.
        let at = |d: f32| {
            let mut walked = 0.0;
            for (pair, length) in points.windows(2).zip(&lengths) {
                if d <= walked + length || *length <= f32::EPSILON {
                    return lerp(
                        pair[0],
                        pair[1],
                        ((d - walked) / length.max(f32::EPSILON)).clamp(0.0, 1.0),
                    );
                }
                walked += length;
            }
            *points.last().unwrap_or(&[0.0, 0.0])
        };
        for [start, end] in dash_runs(total) {
            // Follow the curve through any vertices inside the run.
            let mut previous = at(start);
            let mut walked = 0.0;
            for (pair, length) in points.windows(2).zip(&lengths) {
                walked += length;
                if walked > start && walked < end {
                    shape = shape.line(previous, pair[1], width, LineCap::Round, color);
                    previous = pair[1];
                }
            }
            shape = shape.line(previous, at(end), width, LineCap::Round, color);
        }
    } else {
        for pair in points.windows(2) {
            shape = shape.line(pair[0], pair[1], width, LineCap::Round, color);
        }
    }
    if let [.., before, end] = points {
        let dx = end[0] - before[0];
        let dy = end[1] - before[1];
        let distance = (dx * dx + dy * dy).sqrt().max(0.01);
        let [x, y] = [dx / distance, dy / distance];
        let half = ARROW_LENGTH / 2.0;
        shape = shape.fill(
            vec![
                *end,
                [
                    end[0] - x * ARROW_LENGTH + y * half,
                    end[1] - y * ARROW_LENGTH - x * half,
                ],
                [
                    end[0] - x * ARROW_LENGTH - y * half,
                    end[1] - y * ARROW_LENGTH + x * half,
                ],
            ],
            color,
        );
    }
    shape
}

pub(super) fn render(
    state: &State,
    theme: ThemeStyle<'_>,
    width: f32,
    height: f32,
    header: El,
) -> El {
    let width = width.max(1.0);
    let height = height.max(1.0);
    let surfaces = &state.workspace_surface_edits;
    let narrow = width < 650.0;
    let toolbar_y = if narrow { 196.0 } else { 140.0 };
    let graph_top = toolbar_y + 54.0;
    let panel_width = (width - 48.0).clamp(1.0, 340.0);
    let panel_height = (height - graph_top - 86.0).clamp(120.0, 450.0);
    let panel_x = width - panel_width - 24.0;
    let map_width = if state.workspace_surface_selected.is_some() && width >= 980.0 {
        panel_x - 16.0
    } else {
        width
    };
    let map_height = (height - graph_top - 74.0).max(60.0);
    let positions = atlas::layout(surfaces, width, map_height);
    let bounds = positions
        .iter()
        .fold([NODE_MAX_WIDTH, NODE_HEIGHT], |bounds, point| {
            [
                bounds[0].max(point[0] + NODE_MAX_WIDTH / 2.0),
                bounds[1].max(point[1] + NODE_HEIGHT / 2.0),
            ]
        });
    let fit = ((map_width - 48.0).max(60.0) / bounds[0])
        .min((map_height - 24.0).max(60.0) / (bounds[1] + 52.0))
        .min(1.0);
    let base_zoom = fit.max(0.01);
    let zoom = (base_zoom * state.workspace_surface_zoom).clamp(0.01, 1.8);
    let origin = [
        ((map_width - bounds[0] * zoom) / 2.0).max(24.0),
        graph_top + 52.0 * zoom + ((map_height - (bounds[1] + 52.0) * zoom) / 2.0).max(0.0),
    ];
    let position = |index: usize| {
        [
            positions[index][0] * zoom + origin[0] + state.workspace_surface_pan[0],
            positions[index][1] * zoom + origin[1] + state.workspace_surface_pan[1],
        ]
    };
    let mut shape = crate::grid_background::dots(width, height, theme.color("#303030").alpha(0.34));
    let mut labels = Vec::new();
    let mut label_rects: Vec<[f32; 4]> = Vec::new();
    let card_rects: Vec<_> = (0..surfaces.len())
        .map(|index| {
            let [x, y] = position(index);
            [
                x - NODE_MAX_WIDTH * zoom / 2.0,
                y - NODE_HEIGHT * zoom / 2.0,
                NODE_MAX_WIDTH * zoom,
                NODE_HEIGHT * zoom,
            ]
        })
        .collect();
    let matches: Vec<_> = surfaces
        .iter()
        .map(|surface| {
            atlas::matches(
                surface,
                state.workspace_surface_focus,
                &state.workspace_surface_query.text,
            )
        })
        .collect();
    struct Edge {
        source: usize,
        target: usize,
        index: usize,
        points: Vec<[f32; 2]>,
        active: bool,
        boundary: bool,
    }
    let mut relationship_count = 0;
    let mut edges = Vec::new();
    for (source, surface) in surfaces.iter().enumerate() {
        for (index, link) in surface.relationships.iter().enumerate() {
            let Some(target) = surfaces
                .iter()
                .position(|candidate| candidate.id == link.target_id)
            else {
                continue;
            };
            relationship_count += 1;
            if source == target {
                continue;
            }
            let active = [
                state.workspace_surface_hovered,
                state.workspace_surface_selected,
            ]
            .iter()
            .any(|index| *index == Some(source) || *index == Some(target));
            let boundary = surface_scope(surface) == SurfaceScope::OutOfScope as i32
                || surface_scope(&surfaces[target]) == SurfaceScope::OutOfScope as i32;
            edges.push(Edge {
                source,
                target,
                index,
                points: curve(position(source), position(target), zoom),
                active,
                boundary,
            });
        }
    }
    for edge in &edges {
        let color = if edge.active {
            theme.accent()
        } else if edge.boundary {
            theme.color("#e07676").alpha(0.6)
        } else {
            theme.color("#888888").alpha(0.6)
        };
        let color = color.alpha(if matches[edge.source] || matches[edge.target] {
            if edge.active { 1.0 } else { 0.6 }
        } else {
            0.18
        });
        shape = arrow(shape, &edge.points, color, edge.active, edge.boundary);
    }
    // Labels are placed once every edge is known so none covers another edge; active edges
    // claim space first so a hovered relationship keeps its caption.
    let mut label_order: Vec<usize> = (0..edges.len()).collect();
    label_order.sort_by_key(|index| !edges[*index].active);
    for edge_index in label_order {
        let Edge {
            source,
            target,
            index: edge,
            ref points,
            active,
            boundary,
        } = edges[edge_index];
        let surface = &surfaces[source];
        let link = &surface.relationships[edge];
        if zoom >= 0.65 && (surfaces.len() <= 24 || active) {
            let label_width = (atlas::COLUMN_GAP * zoom - 12.0).clamp(74.0, 126.0);
            let other_edges: Vec<&[[f32; 2]]> = edges
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != edge_index)
                .map(|(_, other)| other.points.as_slice())
                .collect();
            let Some(label_rect) = label_position(
                points,
                label_width,
                10.0 + 23.0 * state.appearance.text_size.factor(),
                &card_rects,
                &label_rects,
                &other_edges,
                [8.0, graph_top, map_width - 16.0, map_height],
            ) else {
                continue;
            };
            label_rects.push(label_rect);
            let caption = if link.label.is_empty() {
                &link.type_id
            } else {
                &link.label
            };
            let mut label = col()
                .key(format!(
                    "workspace-surface-relationship-{source}-{target}-{edge}"
                ))
                .label(format!(
                    "{} → {}: {} [{}]; {} evidence references",
                    surface.id,
                    surfaces[target].id,
                    link.label,
                    link.type_id,
                    link.evidence_refs.len()
                ))
                .absolute(label_rect[0], label_rect[1])
                .w(label_width)
                .px(6.0)
                .py(4.0)
                .gap(2.0)
                .radius(6.0)
                .continuous_corners()
                .fill(theme.color("#111111"))
                .border(
                    1.0,
                    if boundary {
                        theme.color("#e07676").alpha(0.35)
                    } else {
                        theme.color("#2e2e2e")
                    },
                )
                .opacity(if matches[source] || matches[target] {
                    1.0
                } else {
                    0.25
                });
            if !link.type_id.is_empty() && link.type_id != link.label {
                label = label.child(
                    text(link.type_id.to_uppercase())
                        .fade_truncate()
                        .font(ui_font(8.0, 600.0))
                        .color(if boundary {
                            theme.color("#e07676")
                        } else {
                            theme.color("#888888")
                        }),
                );
            }
            labels.push(
                label.child(
                    text(caption.clone())
                        .fade_truncate()
                        .font(ui_font(10.0, 500.0))
                        .color(theme.color("#b8b8b8")),
                ),
            );
        }
    }
    let mut stage = el()
        .key("workspace-surface-network")
        .size(width, height)
        .child(canvas(shape).absolute(0.0, 0.0).size(width, height));
    let selected = state.workspace_surface_selected.is_some();
    let measured_height = if state.workspace_surface_detail_height > 0.0 {
        state.workspace_surface_detail_height.min(panel_height)
    } else {
        panel_height
    };
    let panel_bottom = (graph_top + measured_height).min(height);
    let regions = if selected {
        [
            [0.0, 0.0, width, graph_top],
            [0.0, graph_top, panel_x, panel_bottom - graph_top],
            [
                panel_x + panel_width,
                graph_top,
                width - panel_x - panel_width,
                panel_bottom - graph_top,
            ],
            [0.0, panel_bottom, width, height - panel_bottom],
        ]
    } else {
        [[0.0, 0.0, width, height], [0.0; 4], [0.0; 4], [0.0; 4]]
    };
    for (index, [x, y, w, h]) in regions.into_iter().enumerate() {
        if w > 0.0 && h > 0.0 {
            stage = stage.child(
                el().key(if selected {
                    format!("workspace-surface-background-{index}")
                } else {
                    "workspace-surface-background-full".to_owned()
                })
                .absolute(x, y)
                .size(w, h)
                .on_drag(background_drag),
            );
        }
    }
    for label in labels {
        stage = stage.child(label);
    }
    let max_updates = surfaces
        .iter()
        .map(|surface| surface_revision(surface).saturating_sub(1))
        .max()
        .unwrap_or(0);
    for index in 0..surfaces.len() {
        stage = stage.child(node(
            state,
            theme,
            index,
            position(index),
            zoom,
            max_updates,
        ));
    }
    if surfaces.is_empty() {
        stage = stage.child(col().absolute(24.0, graph_top + 80.0).w((width - 48.0).max(1.0)).items_center().gap(12.0)
            .child(icon(&WORKSPACE_SURFACES, 34.0).color(theme.accent()))
            .child(text("Your surface map starts here").font(ui_font(18.0, 600.0)).color(theme.color("#eeeeee")))
            .child(text("Add surfaces and record their relationships to see the research landscape.").wrap().font(ui_font(13.0, 400.0)).color(theme.color("#999999"))));
    }
    if selected {
        stage = stage.child(
            surface_network_details(state, theme)
                .absolute(panel_x, graph_top)
                .w(panel_width)
                .max_h(panel_height)
                .on_layout_tx(|state: &mut State, [_, _, _, height], tx| {
                    if (state.workspace_surface_detail_height - height).abs() > 0.5 {
                        state.workspace_surface_detail_height = height;
                        tx.instant();
                    } else {
                        tx.live_only();
                    }
                }),
        );
    }
    let matching = matches.iter().filter(|value| **value).count();
    let search_font = ui_font(12.0, 400.0);
    let toolbar = row()
        .absolute(24.0, toolbar_y)
        .w((width - 48.0).max(1.0))
        .h(32.0)
        .center_y()
        .gap(10.0)
        .child(
            text(if matching == surfaces.len() {
                format!("{relationship_count} connections")
            } else {
                format!(
                    "{relationship_count} connections · {matching} of {} match",
                    surfaces.len()
                )
            })
            .key("workspace-surface-match-count")
            .min_w(0.0)
            .fade_truncate()
            .font(ui_font(12.0, 550.0))
            .color(theme.color("#b8b8b8")),
        )
        .child(el().grow(1.0))
        .child(
            text_input(&state.workspace_surface_query, search_font)
                .key("workspace-surface-search")
                .label("Find a surface")
                .w(if narrow { 180.0 } else { 232.0 })
                .h(32.0)
                .px(10.0)
                .py(((32.0 - search_font.line_px()) * 0.5).max(0.0))
                .font(search_font)
                .placeholder("Find a surface…", theme.color("#888888"))
                .radius(8.0)
                .fill(theme.color("#191919"))
                .border(1.0, theme.color("#303030"))
                .color(theme.color("#dddddd"))
                .on_edit(|state: &mut State| &mut state.workspace_surface_query),
        );
    let mut actions = row()
        .key("workspace-surface-actions")
        .absolute(24.0, height - 56.0)
        .w((width - 48.0).max(1.0))
        .h(32.0)
        .center_y()
        .gap(6.0)
        .child(
            form_button("−", theme)
                .key("workspace-surface-zoom-out")
                .label("Zoom out")
                .on_press(move |state: &mut State| {
                    state.workspace_surface_zoom =
                        (state.workspace_surface_zoom / 1.2).max(0.01 / base_zoom)
                }),
        )
        .child(
            form_button("Fit", theme)
                .key("workspace-surface-fit")
                .label("Fit surface map")
                .on_press(move |state: &mut State| {
                    state.workspace_surface_zoom = (fit / base_zoom).clamp(0.01, 1.0);
                    state.workspace_surface_pan = [0.0; 2];
                }),
        )
        .child(
            form_button("+", theme)
                .key("workspace-surface-zoom-in")
                .label("Zoom in")
                .on_press(move |state: &mut State| {
                    state.workspace_surface_zoom =
                        (state.workspace_surface_zoom * 1.2).min(1.8 / base_zoom)
                }),
        )
        .child(
            text(format!("{:.0}%", zoom * 100.0))
                .font(ui_font(10.0, 500.0))
                .color(theme.color("#999999")),
        );
    if width >= 900.0 {
        actions = actions.child(
            text(if matching != surfaces.len() {
                format!("{matching} of {} match", surfaces.len())
            } else {
                "Drag to pan · bars show updates".to_owned()
            })
            .font(ui_font(10.5, 400.0))
            .color(theme.color("#888888")),
        );
    }
    actions = actions.child(el().grow(1.0));
    let dirty = state
        .workspace_details
        .as_ref()
        .is_some_and(|details| surfaces.as_slice() != details.surfaces.as_slice());
    let saving = state.workspace_save_busy == Some(WorkspaceTab::Surfaces);
    if dirty || saving {
        let can_save = dirty && !saving && !state.workspace_details_loading;
        let save =
            form_button(if saving { "Saving…" } else { "Save Changes" }, theme).key(if can_save {
                "saved-workspace-save"
            } else {
                "saved-workspace-save-disabled"
            });
        actions = actions.child(if can_save {
            save.on_press(|state: &mut State| {
                crate::start_workspace_save(state, WorkspaceTab::Surfaces)
            })
        } else {
            save
        });
    }
    actions = actions.child(
        form_button("Manage Surfaces", theme)
            .key("workspace-surface-manage")
            .on_press(|state: &mut State| state.workspace_surface_management_open = true),
    );
    let mut page = el()
        .key("workspace-view-page")
        .size(width, height)
        .child(stage)
        .child(
            el().absolute(0.0, 0.0)
                .size(width, graph_top - 8.0)
                .on_press(|_: &mut State| {}),
        )
        .child(header.absolute(24.0, 25.0).w((width - 48.0).max(1.0)))
        .child(overview(state, theme, (width - 48.0).max(1.0)).absolute(24.0, 78.0))
        .child(toolbar)
        .child(
            el().absolute(0.0, height - 70.0)
                .size(width, 70.0)
                .on_press(|_: &mut State| {}),
        )
        .child(actions);
    if !surfaces.is_empty() && matching == 0 {
        page = page.child(
            row()
                .absolute(24.0, graph_top + 8.0)
                .w((map_width - 48.0).max(1.0))
                .center_y()
                .gap(12.0)
                .padding(10.0)
                .radius(8.0)
                .fill(theme.color("#191919"))
                .child(
                    text("No surfaces match")
                        .grow(1.0)
                        .min_w(0.0)
                        .fade_truncate()
                        .font(ui_font(12.0, 500.0))
                        .color(theme.color("#b8b8b8")),
                )
                .child(
                    form_button("Clear filters", theme)
                        .key("workspace-surface-clear-filters")
                        .on_press(|state: &mut State| {
                            state.workspace_surface_focus = Focus::All;
                            state.workspace_surface_query = TextEdit::new("");
                        }),
                ),
        );
    }
    if !state.workspace_save_error.is_empty() {
        let mut status = col().key("saved-workspace-save-error").gap(8.0).child(
            text(state.workspace_save_error.clone())
                .font(ui_font(12.0, 400.0))
                .color(theme.color("#e07676")),
        );
        if state.workspace_save_error == crate::workspaces::WORKSPACE_CONFLICT_MESSAGE {
            status = status.child(
                form_button("Reload Workspace", theme)
                    .key("saved-workspace-reload")
                    .on_press(|state: &mut State| {
                        if let Some(id) = state.workspace_id {
                            crate::start_workspace_details(state, id);
                        }
                    }),
            );
        }
        page = page.child(
            status
                .absolute(24.0, height - 120.0)
                .w((width - 48.0).min(460.0)),
        );
    } else if !state.workspace_save_message.is_empty() {
        page = page.child(
            text(state.workspace_save_message.clone())
                .key("saved-workspace-save-message")
                .font(ui_font(12.0, 400.0))
                .color(theme.color("#999999"))
                .absolute(24.0, height - 92.0),
        );
    }
    page
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_dashes_are_even_and_end_solid_at_the_arrow() {
        for total in [5.0, 40.0, 137.5, 600.0] {
            let runs = dash_runs(total);
            let last = runs.last().copied().unwrap();
            assert_eq!(last[1], total, "the arrowhead meets a drawn run");
            assert!(last[1] - last[0] >= BOUNDARY_SOLID_TAIL.min(total) - 0.01);
            for pair in runs.windows(2) {
                assert!(pair[0][1] < pair[1][0], "runs are separated by gaps");
            }
            for run in &runs[..runs.len() - 1] {
                assert!(run[1] - run[0] <= BOUNDARY_DASH + 0.01);
            }
        }
    }

    #[test]
    fn edge_labels_avoid_other_edges() {
        let points: Vec<[f32; 2]> = (0..=32).map(|step| [step as f32 * 10.0, 100.0]).collect();
        let crossing: Vec<[f32; 2]> = (0..=32).map(|step| [160.0, step as f32 * 10.0]).collect();
        let viewport = [0.0, 0.0, 400.0, 400.0];
        let alone = label_position(&points, 80.0, 30.0, &[], &[], &[], viewport).unwrap();
        assert_eq!(alone, [120.0, 85.0, 80.0, 30.0], "centred on its own edge");
        let shifted =
            label_position(&points, 80.0, 30.0, &[], &[], &[&crossing], viewport).unwrap();
        assert!(!path_crosses(shifted, &crossing));
    }
}
