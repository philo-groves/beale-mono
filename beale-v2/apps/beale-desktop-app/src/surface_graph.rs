//! Surface overview: recorded properties and relationships, with no inferred risk scores.
use super::*;
use crate::design::FadeText;
use crate::workspace_surface_layout::{self as atlas, Focus};
use rune::{Align, ScrollSpec, Side};

fn footer_button(label: impl Into<String>, theme: ThemeStyle<'_>) -> El {
    form_button(label, theme)
        .fill(theme.color("#0d0d0d"))
        .border(1.0, theme.color("#2a2a2a").alpha(0.65))
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

pub(super) fn filter_items(state: &State) -> Vec<crate::in_place_menu::MenuItem> {
    let surfaces = &state.workspace_surface_edits;
    let included = |surface: &&WorkspaceSurface| {
        state.workspace_surface_explorer.show_out_of_scope || !atlas::out_of_scope(surface)
    };
    let mut items: Vec<_> = [
        (
            "all",
            Focus::All,
            "All",
            surfaces.iter().filter(included).count(),
        ),
        (
            "review",
            Focus::Review,
            "Needs review",
            surfaces
                .iter()
                .filter(|s| included(s) && atlas::needs_review(s))
                .count(),
        ),
        (
            "unevidenced",
            Focus::Unevidenced,
            "No refs",
            surfaces
                .iter()
                .filter(|s| included(s) && atlas::evidence_count(s) == 0)
                .count(),
        ),
        (
            "excluded",
            Focus::Excluded,
            "Excluded",
            surfaces.iter().filter(|s| atlas::out_of_scope(s)).count(),
        ),
    ]
    .into_iter()
    .map(
        |(value, focus, label, count)| crate::in_place_menu::MenuItem {
            value: value.to_owned(),
            label: format!("{label} ({count})"),
            detail: None,
            selected: state.workspace_surface_focus == focus,
        },
    )
    .collect();
    items.push(crate::in_place_menu::MenuItem {
        value: "show_out_of_scope".to_owned(),
        label: "Show excluded".to_owned(),
        detail: None,
        selected: state.workspace_surface_explorer.show_out_of_scope,
    });
    items
}

pub(super) fn select_filter(state: &mut State, value: &str) {
    if value == "show_out_of_scope" {
        let show = !state.workspace_surface_explorer.show_out_of_scope;
        if !show && state.workspace_surface_explorer.neighborhood.is_some() {
            back_to_surfaces(state);
        }
        state.workspace_surface_explorer.show_out_of_scope = show;
        if !show && state.workspace_surface_focus == Focus::Excluded {
            state.workspace_surface_focus = Focus::All;
        }
        state.workspace_surface_explorer.focus_path.clear();
        state.workspace_surface_explorer.focus_scroll.clear();
        reset_page(state);
        return;
    }
    let focus = match value {
        "all" => Focus::All,
        "review" => Focus::Review,
        "unevidenced" => Focus::Unevidenced,
        "excluded" => Focus::Excluded,
        _ => return,
    };
    if focus == Focus::All {
        back_to_surfaces(state);
    }
    if focus == Focus::Excluded {
        state.workspace_surface_explorer.show_out_of_scope = true;
    }
    state.workspace_surface_focus = focus;
    state.workspace_surface_explorer.focus_path.clear();
    state.workspace_surface_explorer.focus_scroll.clear();
    reset_page(state);
}

pub(super) fn filter_menu(state: &State, theme: ThemeStyle<'_>) -> El {
    let sort = form_button(
        match state.workspace_surface_explorer.sort {
            atlas::Sort::Recorded => "Sort: recorded",
            atlas::Sort::Name => "Sort: name",
            atlas::Sort::Type => "Sort: type",
            atlas::Sort::Connections => "Sort: links",
        },
        theme,
    )
    .key("workspace-surface-sort")
    .w_full()
    .on_press(|state: &mut State| {
        state.workspace_surface_explorer.sort = match state.workspace_surface_explorer.sort {
            atlas::Sort::Recorded => atlas::Sort::Name,
            atlas::Sort::Name => atlas::Sort::Type,
            atlas::Sort::Type => atlas::Sort::Connections,
            atlas::Sort::Connections => atlas::Sort::Recorded,
        };
        reset_page(state);
    });
    let mut options = col()
        .key("selection-menu-scroll")
        .w_full()
        .h(142.0)
        .gap(2.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    for (index, item) in filter_items(state).into_iter().take(4).enumerate() {
        let value = item.value;
        options = options.child(
            row()
                .key(format!("selection-menu-item-{index}"))
                .role("menuitem")
                .label(item.label.clone())
                .w_full()
                .h(34.0)
                .px(10.0)
                .center_y()
                .gap(8.0)
                .radius(6.0)
                .fill(if index == state.menu_cursor {
                    theme.color("#383838")
                } else {
                    Color::TRANSPARENT
                })
                .hover(theme.color("#424242"))
                .on_hover(move |state: &mut State| state.menu_cursor = index)
                .on_press_tx(move |state: &mut State, tx| {
                    crate::choose_selection_menu_item(
                        state,
                        SelectionMenu::SurfaceFilter,
                        value.clone(),
                        tx,
                    );
                })
                .child(
                    text(item.label)
                        .grow(1.0)
                        .min_w(0.0)
                        .fade_truncate()
                        .font(ui_font(14.0, if item.selected { 550.0 } else { 400.0 }))
                        .color(theme.color("#eeeeee")),
                )
                .child(if item.selected {
                    icon(&crate::icons::CHECK, 14.0).color(theme.color("#c8c8c8"))
                } else {
                    el().size(14.0, 14.0)
                }),
        );
    }
    let show_out_of_scope = state.workspace_surface_explorer.show_out_of_scope;
    let scope_toggle = row()
        .key("workspace-surface-show-out-of-scope")
        .role("checkbox")
        .label(format!(
            "Show excluded: {}",
            if show_out_of_scope { "on" } else { "off" }
        ))
        .w_full()
        .h(34.0)
        .px(10.0)
        .center_y()
        .gap(8.0)
        .radius(6.0)
        .fill(if state.menu_cursor == 4 {
            theme.color("#383838")
        } else {
            Color::TRANSPARENT
        })
        .hover(theme.color("#424242"))
        .on_hover(|state: &mut State| state.menu_cursor = 4)
        .on_press_tx(|state: &mut State, tx| {
            crate::choose_selection_menu_item(
                state,
                SelectionMenu::SurfaceFilter,
                "show_out_of_scope".to_owned(),
                tx,
            );
        })
        .child(
            text("Show excluded")
                .grow(1.0)
                .font(ui_font(14.0, if show_out_of_scope { 550.0 } else { 400.0 }))
                .color(theme.color("#eeeeee")),
        )
        .child(if show_out_of_scope {
            icon(&crate::icons::CHECK, 14.0).color(theme.color("#c8c8c8"))
        } else {
            el().size(14.0, 14.0)
        });
    let search_font = ui_font(12.0, 400.0);
    let search = text_input(&state.workspace_surface_query, search_font)
        .key("workspace-surface-search")
        .label("Find a surface; combine words, type:repository, scope:in, scope:out, or id:S1")
        .single_line()
        .w_full()
        .h(32.0)
        .px(10.0)
        .py(((32.0 - search_font.line_px()) * 0.5).max(0.0))
        .font(search_font)
        .placeholder("Search surfaces…", theme.color("#888888"))
        .radius(8.0)
        .fill(theme.color("#191919"))
        .border(1.0, theme.color("#303030"))
        .color(theme.color("#dddddd"))
        .on_edit(|state: &mut State| &mut state.workspace_surface_query)
        .on_submit(|state: &mut State, _| {
            state.selection_menu = None;
            state.workspace_surface_explorer.focus_path.clear();
            state.workspace_surface_explorer.focus_scroll.clear();
            reset_page(state);
        });
    let divider = || el().w_full().h(1.0).fill(theme.color("#3a3a3a"));
    crate::in_place_menu::popup_surface(
        col()
            .key("selection-menu")
            .role("dialog")
            .label("Filter surfaces")
            .w(230.0)
            .h(294.0)
            .padding(6.0)
            .gap(6.0)
            .child(sort)
            .child(divider().key("workspace-surface-filter-top-divider"))
            .child(options)
            .child(divider().key("workspace-surface-filter-scope-divider"))
            .child(scope_toggle)
            .child(divider().key("workspace-surface-filter-bottom-divider"))
            .child(search),
        theme.color("#242424"),
    )
    .anchored_fit(
        "workspace-surface-filter-control",
        Side::Bottom,
        Align::End,
        4.0,
    )
}

fn reset_page(state: &mut State) {
    state.workspace_surface_explorer.page = 0;
    state.workspace_surface_explorer.page_anchor = None;
    state.workspace_surface_explorer.query = state.workspace_surface_query.text.clone();
}

fn open_neighborhood(state: &mut State, index: usize) {
    if state.workspace_surface_explorer.neighborhood.is_none() {
        let explorer = &state.workspace_surface_explorer;
        state.workspace_surface_explorer.return_to = Some(atlas::ReturnTo {
            view: explorer.view(state.workspace_surface_edits.len()),
            sort: explorer.sort,
            focus: state.workspace_surface_focus,
            show_out_of_scope: explorer.show_out_of_scope,
            query: state.workspace_surface_query.text.clone(),
            page: if explorer.query == state.workspace_surface_query.text {
                explorer.page
            } else {
                0
            },
            page_anchor: if explorer.query == state.workspace_surface_query.text {
                explorer.page_anchor.clone()
            } else {
                None
            },
        });
    }
    state.workspace_surface_explorer.neighborhood =
        Some(state.workspace_surface_edits[index].id.clone());
    state.workspace_surface_explorer.focus_path =
        vec![state.workspace_surface_edits[index].id.clone()];
    state.workspace_surface_explorer.focus_scroll.clear();
    state.workspace_surface_explorer.view = Some(atlas::View::Graph);
    state.workspace_surface_focus = Focus::All;
    state.workspace_surface_query.clear();
    state.workspace_surface_connecting = None;
    reset_page(state);
}

pub(super) fn back_to_surfaces(state: &mut State) {
    state.workspace_surface_explorer.neighborhood = None;
    state.workspace_surface_explorer.focus_path.clear();
    state.workspace_surface_explorer.focus_scroll.clear();
    state.workspace_surface_connecting = None;
    if let Some(previous) = state.workspace_surface_explorer.return_to.take() {
        state.workspace_surface_explorer.view = Some(previous.view);
        state.workspace_surface_explorer.sort = previous.sort;
        state.workspace_surface_explorer.page = previous.page;
        state.workspace_surface_explorer.page_anchor = previous.page_anchor;
        state.workspace_surface_explorer.query = previous.query.clone();
        state.workspace_surface_query = TextEdit::new(previous.query);
        state.workspace_surface_focus = previous.focus;
        state.workspace_surface_explorer.show_out_of_scope = previous.show_out_of_scope;
    } else {
        state.workspace_surface_explorer.view = Some(atlas::View::Inventory);
        state.workspace_surface_focus = Focus::All;
        state.workspace_surface_query.clear();
        reset_page(state);
    }
}

fn switch_view(state: &mut State, mode: atlas::View) {
    if state.workspace_surface_explorer.neighborhood.is_some() {
        back_to_surfaces(state);
        if state.workspace_surface_explorer.view == Some(mode) {
            return;
        }
    }
    state.workspace_surface_explorer.view = Some(mode);
    reset_page(state);
}

pub(super) fn view_toggle(state: &State, labeled: bool, theme: ThemeStyle<'_>) -> El {
    let selected = state
        .workspace_surface_explorer
        .view(state.workspace_surface_edits.len());
    let mut tabs = crate::design::segmented_control(theme)
        .key("workspace-surface-view-toggle")
        .label("Surface views")
        .min_w(view_toggle_width(labeled));
    for (mode, label, glyph) in [
        (atlas::View::Graph, "Graph", &WORKSPACE_SURFACES),
        (
            atlas::View::Inventory,
            "List",
            &crate::icons::SESSION_SIDENAV,
        ),
    ] {
        let button = if labeled {
            crate::design::segmented_tab(label, Some(glyph), selected == mode, true, theme)
        } else {
            crate::design::segmented_icon_tab(label, glyph, selected == mode, theme)
        };
        tabs = tabs.child(
            button
                .key(format!("workspace-surface-view-{mode:?}"))
                .on_press(move |state: &mut State| switch_view(state, mode)),
        );
    }
    tabs
}

pub(super) fn view_toggle_width(labeled: bool) -> f32 {
    if labeled {
        crate::design::segmented_group_width(&[
            crate::design::segmented_tab_width("Graph", true),
            crate::design::segmented_tab_width("List", true),
        ])
    } else {
        crate::design::segmented_group_width(&[crate::design::CONTROL_HEIGHT; 2])
    }
}

fn inventory(
    state: &State,
    theme: ThemeStyle<'_>,
    analysis: &atlas::Analysis,
    indices: &[usize],
    width: f32,
    height: f32,
) -> El {
    let wide = width >= 850.0;
    let mut list = col()
        .key(format!(
            "surface-inventory-page-{:?}-{}-{}",
            state.workspace_surface_explorer.sort,
            indices.first().copied().unwrap_or(0),
            state.workspace_surface_query.text
        ))
        .size(width, height)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    for (position, &index) in indices.iter().enumerate() {
        if position > 0 {
            list = list.child(
                el().key(format!("workspace-surface-list-separator-{index}"))
                    .w_full()
                    .h(1.0)
                    .fill(theme.color("#303030")),
            );
        }
        let surface = &state.workspace_surface_edits[index];
        let color = scope_color(surface, theme);
        let evidence = atlas::evidence_count(surface);
        let type_id = surface
            .definition
            .as_ref()
            .map_or("Unspecified", |d| d.type_id.as_str());
        let mut info = col()
            .grow(1.0)
            .min_w(0.0)
            .gap(5.0)
            .child(
                text(atlas::name(surface).to_owned())
                    .key(format!("workspace-surface-inventory-name-{index}"))
                    .w_full()
                    .fade_truncate()
                    .font(ui_font(14.0, 600.0))
                    .color(if atlas::out_of_scope(surface) {
                        theme.color("#e07676")
                    } else {
                        theme.color("#eeeeee")
                    }),
            )
            .child(
                text(format!(
                    "{} · {type_id} · {}",
                    surface.id,
                    surface_value(surface)
                ))
                .key(format!("workspace-surface-list-meta-{index}"))
                .w_full()
                .fade_truncate()
                .font(ui_font(12.0, 400.0))
                .color(theme.color("#a4a4a4")),
            );
        if !wide {
            info = info.child(
                text(format!(
                    "{} · {} · {evidence} refs · {} in / {} out",
                    scope(surface),
                    scope_state_label(surface_scope_state(surface)),
                    analysis.incoming[index],
                    analysis.outgoing[index]
                ))
                .w_full()
                .fade_truncate()
                .font(ui_font(11.0, 400.0))
                .color(theme.color("#999999")),
            );
        }
        let mut entry = row()
            .w_full()
            .min_w(0.0)
            .h(if wide { 76.0 } else { 84.0 })
            .px(12.0)
            .gap(14.0)
            .center_y()
            .key(format!("workspace-surface-row-{index}"))
            .role("button")
            .label(format!(
                "Open {}; {}; {}; {evidence} definition/link references; {} incoming; {} outgoing",
                atlas::name(surface),
                scope(surface),
                scope_state_label(surface_scope_state(surface)),
                analysis.incoming[index],
                analysis.outgoing[index]
            ))
            .hover(theme.color("#242424"))
            .on_press(move |state: &mut State| crate::surface_canvas::open(state, index))
            .child(
                el().size(34.0, 34.0)
                    .center()
                    .radius(9.0)
                    .fill(theme.color("#242424"))
                    .child(icon(surface_icon(surface_kind(surface)), 17.0).color(
                        if atlas::out_of_scope(surface) {
                            theme.color("#e07676")
                        } else {
                            theme.color("#bdbdbd")
                        },
                    )),
            )
            .child(info);
        if wide {
            entry = entry
                .child(
                    col()
                        .w(116.0)
                        .gap(5.0)
                        .child(
                            row()
                                .self_start()
                                .h(22.0)
                                .px(8.0)
                                .center_y()
                                .radius(6.0)
                                .fill(color.alpha(0.12))
                                .child(
                                    text(scope(surface)).font(ui_font(11.0, 550.0)).color(color),
                                ),
                        )
                        .child(
                            text(scope_state_label(surface_scope_state(surface)))
                                .font(ui_font(11.0, 400.0))
                                .color(theme.color("#999999")),
                        ),
                )
                .child(
                    text(format!("{evidence} refs"))
                        .w(66.0)
                        .font(ui_font(12.0, 500.0))
                        .color(theme.color(if evidence == 0 { "#dca15f" } else { "#b8b8b8" })),
                )
                .child(
                    text(format!(
                        "{} in · {} out",
                        analysis.incoming[index], analysis.outgoing[index]
                    ))
                    .w(92.0)
                    .font(ui_font(12.0, 400.0))
                    .color(theme.color("#999999")),
                );
        }
        entry = entry.child(
            footer_button("Connections", theme)
                .key(format!("workspace-surface-neighborhood-{index}"))
                .label(format!(
                    "Show recorded neighborhood of {}",
                    atlas::name(surface)
                ))
                .on_press(move |state: &mut State| open_neighborhood(state, index)),
        );
        let edit = icon_action_button(
            format!("saved-workspace-surface-edit-{index}"),
            "Edit surface",
            &PEN,
            theme,
        );
        let remove = icon_action_button(
            format!("saved-workspace-surface-remove-{index}"),
            "Remove surface",
            &TRASH,
            theme,
        );
        entry = entry
            .child(if state.workspace_save_busy.is_some() {
                edit
            } else {
                edit.on_press(move |state: &mut State| {
                    let scope = if surface_scope(&state.workspace_surface_edits[index])
                        == SurfaceScope::OutOfScope as i32
                    {
                        DraftScope::OutOfScope
                    } else {
                        DraftScope::InScope
                    };
                    open_surface_editor(state, Some(index), scope);
                })
            })
            .child(if state.workspace_save_busy.is_some() {
                remove
            } else {
                remove.on_press(move |state: &mut State| {
                    state.workspace_surface_delete_index = Some(index)
                })
            });
        list = list.child(entry);
    }
    col()
        .key("workspace-surface-inventory")
        .size(width, height)
        .child(list.scroll_edge_fade(12.0))
}

pub(super) fn render(
    state: &State,
    theme: ThemeStyle<'_>,
    width: f32,
    height: f32,
    header: El,
) -> El {
    if state
        .workspace_surface_explorer
        .view(state.workspace_surface_edits.len())
        == atlas::View::Graph
    {
        return super::surface_web::render(state, theme, width, height, header);
    }
    let surfaces = &state.workspace_surface_edits;
    let analysis = atlas::Analysis::new(surfaces);
    let explorer = &state.workspace_surface_explorer;
    let matching = analysis.matching(
        surfaces,
        state.workspace_surface_focus,
        &state.workspace_surface_query.text,
        explorer,
    );
    let requested = if explorer.query == state.workspace_surface_query.text {
        explorer.page
    } else {
        0
    };
    let (page, visible) = atlas::page(&matching, requested, atlas::INVENTORY_PAGE_SIZE);
    let page_count = matching.len().div_ceil(atlas::INVENTORY_PAGE_SIZE).max(1);
    let mut result = el()
        .key("workspace-view-page")
        .size(width, height)
        .child(
            inventory(
                state,
                theme,
                &analysis,
                visible,
                (width - 48.0).max(1.0),
                (height - 196.0).max(100.0),
            )
            .absolute(24.0, 118.0),
        )
        .child(header.absolute(24.0, 25.0).w((width - 48.0).max(1.0)));
    if !surfaces.is_empty() && matching.is_empty() {
        result = result.child(
            form_button("Clear filters", theme)
                .key("workspace-surface-clear-filters")
                .absolute(24.0, 86.0)
                .on_press(|state: &mut State| {
                    state.workspace_surface_focus = Focus::All;
                    state.workspace_surface_query.clear();
                    reset_page(state);
                }),
        );
    }
    let mut actions = row()
        .key("workspace-surface-actions")
        .absolute(24.0, height - 56.0)
        .w((width - 48.0).max(1.0))
        .h(32.0)
        .center_y()
        .gap(6.0);
    if page > 0 {
        actions = actions.child(
            footer_button("‹", theme)
                .key("workspace-surface-page-prev")
                .on_press(move |state: &mut State| {
                    state.workspace_surface_explorer.page = page - 1
                }),
        );
    }
    if page_count > 1 {
        actions = actions.child(
            text(format!(
                "{}–{} / {}",
                page * atlas::INVENTORY_PAGE_SIZE + 1,
                page * atlas::INVENTORY_PAGE_SIZE + visible.len(),
                matching.len()
            ))
            .key("workspace-surface-page-count")
            .font(ui_font(10.0, 500.0))
            .color(theme.color("#999999")),
        );
    }
    if page + 1 < page_count {
        actions = actions.child(
            footer_button("›", theme)
                .key("workspace-surface-page-next")
                .on_press(move |state: &mut State| {
                    state.workspace_surface_explorer.page = page + 1
                }),
        );
    }
    actions = actions.child(el().grow(1.0));
    let dirty = state
        .workspace_details
        .as_ref()
        .is_some_and(|details| surfaces.as_slice() != details.surfaces.as_slice());
    let saving = state.workspace_save_busy == Some(WorkspaceTab::Surfaces);
    if dirty || saving {
        let save =
            footer_button(if saving { "Saving…" } else { "Save Changes" }, theme).key(if saving {
                "saved-workspace-save-disabled"
            } else {
                "saved-workspace-save"
            });
        actions = actions.child(if dirty && !saving && !state.workspace_details_loading {
            save.on_press(|state: &mut State| {
                crate::start_workspace_save(state, WorkspaceTab::Surfaces)
            })
        } else {
            save
        });
    }
    actions = actions
        .child(
            text(format!("{} / {} surfaces", matching.len(), surfaces.len()))
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
                    if state.workspace_surface_focus != Focus::All
                        || state.workspace_surface_explorer.show_out_of_scope
                    {
                        theme.accent()
                    } else {
                        theme.color("#bdbdbd")
                    },
                ))
                .on_press(|state: &mut State| {
                    toggle_selection_menu(state, SelectionMenu::SurfaceFilter)
                }),
        );
    result.child(actions)
}
