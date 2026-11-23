//! Surface-only overview and recursive parent web.
use super::*;
use crate::workspace_surface_layout::{self as atlas, Focus};
use rune::{Align, InlineIcon, LineCap, ScrollSpec, Shape, Span, canvas, rich};
use std::collections::{HashMap, HashSet};

const CELL_WIDTH: f32 = 360.0;
const CELL_HEIGHT: f32 = 280.0;
const WEB_CENTER_Y: f32 = 170.0;
const VIEWPORT_TOP: f32 = 124.0;
const VIEWPORT_BOTTOM: f32 = 72.0;
const SCROLL_KEY: &str = "workspace-surface-network-scroll";

struct GraphLayout {
    columns: usize,
    content_height: f32,
    offset: [f32; 2],
}

impl GraphLayout {
    fn new(count: usize, viewport: [f32; 2]) -> Self {
        let columns = ((viewport[0] / CELL_WIDTH).floor() as usize)
            .max(1)
            .min(count.max(1));
        let rows = count.div_ceil(columns);
        let world_width = columns as f32 * CELL_WIDTH;
        let world_height = rows as f32 * CELL_HEIGHT;
        Self {
            columns,
            content_height: world_height.max(viewport[1]),
            offset: [
                (viewport[0] - world_width).max(0.0) / 2.0,
                (viewport[1] - world_height).max(0.0) / 2.0,
            ],
        }
    }
}

fn detail(surface: &WorkspaceSurface) -> String {
    let Some(definition) = surface.definition.as_ref() else {
        return surface.id.clone();
    };
    let locator = definition
        .locator
        .as_ref()
        .map_or("", |value| value.value.as_str());
    format!(
        "{} · {} · {}\n{}\n{}",
        definition.name, surface.id, definition.type_id, locator, definition.description
    )
}

fn marker(
    state: &State,
    theme: ThemeStyle<'_>,
    index: usize,
    center: [f32; 2],
    is_center: bool,
) -> El {
    let surface = &state.workspace_surface_edits[index];
    let radius = if is_center { 8.0 } else { 6.0 };
    let muted = theme.color("#969696");
    el().key(format!("workspace-surface-web-node-{index}"))
        .role("button")
        .label(detail(surface))
        .help(detail(surface))
        .absolute(center[0] - 12.0, center[1] - 12.0)
        .size(24.0, 24.0)
        .center()
        .child(
            el().size(radius * 2.0, radius * 2.0)
                .radius(radius)
                .fill(muted.alpha(if is_center { 0.85 } else { 0.62 }))
                .border(1.0, muted.alpha(0.6)),
        )
        .on_press_tx(move |state: &mut State, tx| {
            if let Some(source) = state.workspace_surface_connecting.take() {
                if source != index {
                    state.workspace_surface_relationship_target = Some(index);
                    state.workspace_surface_relationship_label = TextEdit::new("");
                    state.workspace_surface_relationship_type_id = TextEdit::new("");
                    state.workspace_surface_relationship_error.clear();
                    state.workspace_surface_selected = Some(source);
                }
                return;
            }
            if is_center {
                crate::surface_canvas::open(state, index);
            } else {
                state
                    .workspace_surface_explorer
                    .focus_scroll
                    .push(state.workspace_surface_graph_scroll.offset_y());
                state
                    .workspace_surface_explorer
                    .focus_path
                    .push(state.workspace_surface_edits[index].id.clone());
                tx.scroll_to_y(SCROLL_KEY, 0.0);
            }
        })
}

fn title(state: &State, theme: ThemeStyle<'_>, index: usize, center_x: f32, top: f32) -> El {
    let surface = &state.workspace_surface_edits[index];
    let name = atlas::name(surface).to_owned();
    let kind = surface_kind(surface);
    let type_name = surface
        .definition
        .as_ref()
        .map_or("Unspecified", |d| d.type_id.as_str())
        .to_owned();
    let content = format!("{name} {}", surface.id);
    let id_start = name.len() + 1;
    let id_end = content.len();
    let heading_color = if surface_scope(surface) == SurfaceScope::OutOfScope as i32 {
        theme.color("#e07676")
    } else {
        theme.color("#eeeeee")
    };
    row()
        .absolute(center_x - 150.0, top)
        .w(300.0)
        .h(72.0)
        .center()
        .center_y()
        .child(
            row()
                .key(format!("workspace-surface-web-title-{index}"))
                .role("button")
                .label(format!("Open {name} details"))
                .help(format!("Surface type: {type_name}"))
                .shrink()
                .min_h(32.0)
                .px(8.0)
                .center_y()
                .radius(8.0)
                .continuous_corners()
                .border(
                    1.0,
                    if state.workspace_surface_hovered == Some(index) {
                        theme.color("#969696").alpha(0.55)
                    } else {
                        Color::TRANSPARENT
                    },
                )
                .on_hover_change(move |state: &mut State, inside, _| {
                    if inside {
                        state.workspace_surface_hovered = Some(index);
                    } else if state.workspace_surface_hovered == Some(index) {
                        state.workspace_surface_hovered = None;
                    }
                })
                .on_press(move |state: &mut State| crate::surface_canvas::open(state, index))
                .child(
                    rich(
                        content,
                        vec![
                            Span::new(id_start, id_end)
                                .weight(400.0)
                                .size(12.0)
                                .color(theme.color("#888888")),
                        ],
                    )
                    .inline_icon(0, InlineIcon::glyph(surface_icon(kind)).size(17.0).gap(6.0))
                    .max_w(270.0)
                    .shrink()
                    .max_lines(3)
                    .wrap_anywhere()
                    .align(Align::Center)
                    .font(ui_font(14.0, 550.0))
                    .color(heading_color),
                ),
        )
}

fn button(label: &str, theme: ThemeStyle<'_>) -> El {
    form_button(label, theme)
        .fill(theme.color("#0d0d0d"))
        .border(1.0, theme.color("#2a2a2a").alpha(0.65))
}

pub(super) fn render(
    state: &State,
    theme: ThemeStyle<'_>,
    width: f32,
    height: f32,
    header: El,
) -> El {
    let surfaces = &state.workspace_surface_edits;
    let explorer = &state.workspace_surface_explorer;
    let analysis = atlas::Analysis::new(surfaces);
    let matches = if explorer.focus_path.is_empty() {
        analysis.matching(
            surfaces,
            state.workspace_surface_focus,
            &state.workspace_surface_query.text,
            explorer,
        )
    } else {
        (0..surfaces.len()).collect()
    };
    let matching: HashSet<_> = matches.iter().copied().collect();
    let hierarchy = atlas::hierarchy(surfaces);
    let mut branches = matching.clone();
    for &index in &matching {
        let mut ancestor = hierarchy.parent[index];
        while let Some(parent) = ancestor {
            if !branches.insert(parent) {
                break;
            }
            ancestor = hierarchy.parent[parent];
        }
    }
    let focused = explorer
        .focus_path
        .last()
        .and_then(|id| analysis.indices.get(id))
        .copied()
        .filter(|index| matching.contains(index));
    let roots: Vec<_> = if let Some(index) = focused {
        vec![index]
    } else {
        hierarchy
            .roots
            .iter()
            .copied()
            .filter(|index| branches.contains(index))
            .collect()
    };
    let viewport = [width, (height - VIEWPORT_TOP - VIEWPORT_BOTTOM).max(120.0)];
    let layout = GraphLayout::new(roots.len(), viewport);
    let mut points = HashMap::new();
    let mut titles = Vec::new();
    for (slot, &root) in roots.iter().enumerate() {
        let column = slot % layout.columns;
        let row = slot / layout.columns;
        let cx = (column as f32 + 0.5) * CELL_WIDTH + layout.offset[0];
        let cy = row as f32 * CELL_HEIGHT + WEB_CENTER_Y + layout.offset[1];
        titles.push(title(state, theme, root, cx, cy - 132.0));
        points.insert(root, [cx, cy]);
        let children: Vec<_> = hierarchy.children[root]
            .iter()
            .copied()
            .filter(|index| branches.contains(index))
            .collect();
        for (position, child) in children.iter().enumerate() {
            let angle = (position as f32 * 2.399_963_1) - std::f32::consts::FRAC_PI_2;
            let ring = (position / 8) as f32;
            let radius = (55.0 + 28.0 * ring).min(95.0);
            points.insert(
                *child,
                [cx + radius * angle.cos(), cy + radius * angle.sin()],
            );
        }
    }
    let mut lines = Shape::new();
    let mut line_tips = Vec::new();
    for (&source, &from) in &points {
        for (number, relationship) in surfaces[source].relationships.iter().enumerate() {
            let Some(&target) = analysis.indices.get(&relationship.target_id) else {
                continue;
            };
            let Some(&to) = points.get(&target) else {
                continue;
            };
            if source == target {
                continue;
            }
            let color = theme
                .color("#969696")
                .alpha(if relationship.type_id == "contains" {
                    0.32
                } else {
                    0.45
                });
            lines = lines.line(from, to, 1.0, LineCap::Round, color);
            let tooltip = format!(
                "{} → {}\n{} · {}\n{} evidence references",
                atlas::name(&surfaces[source]),
                atlas::name(&surfaces[target]),
                relationship.type_id,
                relationship.label,
                relationship.evidence_refs.len()
            );
            for sample in 2..=8 {
                let t = sample as f32 / 10.0;
                let x = from[0] + (to[0] - from[0]) * t;
                let y = from[1] + (to[1] - from[1]) * t;
                line_tips.push(
                    el().key(format!(
                        "workspace-surface-web-link-{source}-{target}-{number}-{sample}"
                    ))
                    .role("img")
                    .label(tooltip.clone())
                    .help(tooltip.clone())
                    .absolute(x - 6.0, y - 6.0)
                    .size(12.0, 12.0),
                );
            }
        }
    }
    let mut stage = el()
        .key("workspace-surface-network-content")
        .size(viewport[0], layout.content_height)
        .child(
            el().key("workspace-surface-background-full")
                .absolute(0.0, 0.0)
                .size(viewport[0], layout.content_height)
                .on_press(|state: &mut State| state.workspace_surface_connecting = None),
        )
        .child(
            canvas(lines)
                .absolute(0.0, 0.0)
                .size(viewport[0], layout.content_height),
        );
    for item in line_tips {
        stage = stage.child(item);
    }
    for (&index, &center) in &points {
        stage = stage.child(marker(state, theme, index, center, roots.contains(&index)));
    }
    for item in titles {
        stage = stage.child(item);
    }
    let scroll_watch = state.workspace_surface_graph_scroll.clone();
    let scroll_bar = scroll_watch.clone();
    let graph = el()
        .key("workspace-surface-network")
        .absolute(0.0, VIEWPORT_TOP)
        .size(viewport[0], viewport[1])
        .clip_rounded(0.0)
        .child(
            canvas(crate::grid_background::dots(
                viewport[0],
                viewport[1],
                theme.color("#303030").alpha(0.34),
            ))
            .absolute(0.0, 0.0)
            .size(viewport[0], viewport[1]),
        )
        .child(
            col()
                .key(SCROLL_KEY)
                .absolute(0.0, 0.0)
                .size(viewport[0], viewport[1])
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .on_scroll_offset(move |position| scroll_watch.watch_scroll(position))
                .on_wheel(|state: &mut State, event, tx| {
                    state.workspace_surface_graph_scroll.wheel(event, tx)
                })
                .child(stage),
        )
        .child(scroll_bar.bar(theme));
    let mut page_el = el()
        .key("workspace-view-page")
        .size(width, height)
        .child(graph)
        .child(header.absolute(24.0, 25.0).w((width - 48.0).max(1.0)));
    if focused.is_some() {
        page_el = page_el.child(
            row()
                .absolute(24.0, 89.0)
                .gap(8.0)
                .child(
                    button("‹ Back", theme)
                        .key("workspace-surface-web-back")
                        .on_press_tx(|state: &mut State, tx| {
                            if state.workspace_surface_explorer.focus_path.len() == 1
                                && state.workspace_surface_explorer.return_to.is_some()
                            {
                                super::surface_graph::back_to_surfaces(state);
                            } else {
                                state.workspace_surface_explorer.focus_path.pop();
                                let scroll = state
                                    .workspace_surface_explorer
                                    .focus_scroll
                                    .pop()
                                    .unwrap_or(0.0);
                                tx.scroll_to_y(SCROLL_KEY, scroll);
                            }
                        }),
                )
                .child(
                    button("All surfaces", theme)
                        .key("workspace-surface-web-all")
                        .on_press_tx(|state: &mut State, tx| {
                            if state.workspace_surface_explorer.return_to.is_some() {
                                super::surface_graph::back_to_surfaces(state);
                            } else {
                                state.workspace_surface_explorer.focus_path.clear();
                                let scroll = state
                                    .workspace_surface_explorer
                                    .focus_scroll
                                    .first()
                                    .copied()
                                    .unwrap_or(0.0);
                                state.workspace_surface_explorer.focus_scroll.clear();
                                tx.scroll_to_y(SCROLL_KEY, scroll);
                            }
                        }),
                ),
        );
    }
    if roots.is_empty() {
        page_el = page_el.child(
            text(if surfaces.is_empty() {
                "Add a surface to start the graph"
            } else {
                "No surfaces match the filter"
            })
            .absolute(24.0, 145.0)
            .color(theme.color("#999999")),
        );
        if !surfaces.is_empty() {
            page_el = page_el.child(
                button("Clear filters", theme)
                    .key("workspace-surface-clear-filters")
                    .absolute(24.0, 175.0)
                    .on_press_tx(|state: &mut State, tx| {
                        state.workspace_surface_focus = Focus::All;
                        state.workspace_surface_query.clear();
                        state.workspace_surface_explorer.focus_path.clear();
                        state.workspace_surface_explorer.focus_scroll.clear();
                        tx.scroll_to_y(SCROLL_KEY, 0.0);
                    }),
            );
        }
    }
    let mut footer = row()
        .key("workspace-surface-actions")
        .absolute(24.0, height - 56.0)
        .w((width - 48.0).max(1.0))
        .h(32.0)
        .center_y()
        .gap(6.0);
    footer = footer.child(el().grow(1.0));
    let dirty = state
        .workspace_details
        .as_ref()
        .is_some_and(|details| surfaces.as_slice() != details.surfaces.as_slice());
    let saving = state.workspace_save_busy == Some(WorkspaceTab::Surfaces);
    if dirty || saving {
        let save = button(if saving { "Saving…" } else { "Save Changes" }, theme).key(if saving {
            "saved-workspace-save-disabled"
        } else {
            "saved-workspace-save"
        });
        footer = footer.child(if dirty && !saving && !state.workspace_details_loading {
            save.on_press(|state: &mut State| {
                crate::start_workspace_save(state, WorkspaceTab::Surfaces)
            })
        } else {
            save
        });
    }
    footer = footer
        .child(
            text(format!("{} / {} surfaces", matches.len(), surfaces.len()))
                .key("workspace-surface-match-count")
                .font(ui_font(12.0, 550.0))
                .color(theme.color("#b8b8b8")),
        )
        .child(
            el().key("workspace-surface-filter-control")
                .role("button")
                .label("Filter surfaces")
                .help("Filter surfaces")
                .size(30.0, 30.0)
                .center()
                .radius(8.0)
                .fill(theme.color("#0d0d0d"))
                .border(1.0, theme.color("#2a2a2a").alpha(0.65))
                .child(icon(&crate::icons::FILTER, 16.0).color(
                    if state.workspace_surface_focus != Focus::All {
                        theme.accent()
                    } else {
                        theme.color("#bdbdbd")
                    },
                ))
                .on_press(|state: &mut State| {
                    toggle_selection_menu(state, SelectionMenu::SurfaceFilter)
                }),
        );
    page_el.child(footer)
}

#[cfg(test)]
mod tests {
    use super::GraphLayout;

    #[test]
    fn columns_fit_the_content_width_without_a_fixed_cap() {
        for (width, expected) in [
            (359.0, 1),
            (360.0, 1),
            (719.0, 1),
            (720.0, 2),
            (1439.0, 3),
            (1440.0, 4),
            (2880.0, 8),
        ] {
            let layout = GraphLayout::new(8, [width, 600.0]);
            assert_eq!(layout.columns, expected, "width {width}");
        }
        assert_eq!(GraphLayout::new(2, [2880.0, 600.0]).columns, 2);
    }
}
