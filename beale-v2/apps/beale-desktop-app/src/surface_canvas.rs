use beale_contracts::v1::WorkspaceSurface;
use rune::{El, ScrollSpec, col, el, icon, row, text};

use crate::{
    CONTENT_SURFACE_COLOR, State, ThemeStyle,
    design::FadeText,
    icons::{CLOSE, WORKSPACE_SURFACES},
    ui_font, workspace_view,
};

#[derive(Default)]
pub(crate) struct SurfaceCanvas {
    tabs: Vec<String>,
    active: Option<String>,
}

impl SurfaceCanvas {
    pub(crate) fn active(&self) -> Option<&str> {
        self.active.as_deref()
    }

    pub(crate) fn clear(&mut self) {
        self.tabs.clear();
        self.active = None;
    }

    pub(crate) fn deactivate(&mut self) {
        self.active = None;
    }

    fn sync(&mut self, surfaces: &[WorkspaceSurface]) {
        self.tabs
            .retain(|id| surfaces.iter().any(|surface| surface.id == *id));
        if self
            .active
            .as_ref()
            .is_some_and(|id| !self.tabs.contains(id))
        {
            self.active = self.tabs.last().cloned();
        }
    }

    fn open(&mut self, id: String) {
        if !self.tabs.contains(&id) {
            self.tabs.push(id.clone());
        }
        self.active = Some(id);
    }

    fn close(&mut self, id: &str) {
        let Some(index) = self.tabs.iter().position(|tab| tab == id) else {
            return;
        };
        self.tabs.remove(index);
        if self.active.as_deref() == Some(id) {
            self.active = self
                .tabs
                .get(index.min(self.tabs.len().saturating_sub(1)))
                .cloned();
        }
    }
}

pub(crate) fn open(state: &mut State, index: usize) {
    let Some(surface) = state.workspace_surface_edits.get(index) else {
        return;
    };
    state.canvas_surface.open(surface.id.clone());
    state.workspace_surface_selected = Some(index);
    state.canvas_subagent = None;
    state.canvas_open = true;
    state.session_sidenav_visible = false;
    state.session_usage_menu = None;
    state.session_usage_row_hovered = None;
    state.session_usage_popup_hovered = false;
}

fn select(state: &mut State, id: &str) {
    if let Some(index) = state
        .workspace_surface_edits
        .iter()
        .position(|surface| surface.id == id)
    {
        state.canvas_surface.open(id.to_owned());
        state.workspace_surface_selected = Some(index);
        state.workspace_surface_connecting = None;
    }
}

pub(crate) fn close(state: &mut State, id: &str) {
    let was_active = state.canvas_surface.active() == Some(id);
    state.canvas_surface.close(id);
    if was_active {
        state.workspace_surface_selected = state.canvas_surface.active().and_then(|active| {
            state
                .workspace_surface_edits
                .iter()
                .position(|surface| surface.id == active)
        });
        state.workspace_surface_connecting = None;
        if state.canvas_surface.active().is_none() {
            state.canvas_open = state.canvas_runbook.has_views() || state.canvas_subagent.is_some();
        }
    }
}

pub(crate) fn sync(state: &mut State) {
    let was_active = state.canvas_surface.active().is_some();
    state.canvas_surface.sync(&state.workspace_surface_edits);
    if was_active {
        state.workspace_surface_selected = state.canvas_surface.active().and_then(|active| {
            state
                .workspace_surface_edits
                .iter()
                .position(|surface| surface.id == active)
        });
        if state.canvas_surface.active().is_none() {
            state.canvas_open = state.canvas_runbook.has_views() || state.canvas_subagent.is_some();
        }
    }
}

pub(crate) fn tab_bar(state: &State, theme: ThemeStyle<'_>) -> Option<El> {
    state.canvas_surface.active()?;
    let mut tabs = row()
        .key("surface-canvas-tab-scroll")
        .w_full()
        .h(crate::runbook_canvas::TAB_BAR_HEIGHT)
        .px(12.0)
        .center_y()
        .gap(7.0)
        .scroll_x();
    for (index, id) in state.canvas_surface.tabs.iter().enumerate() {
        let Some(surface) = state
            .workspace_surface_edits
            .iter()
            .find(|surface| &surface.id == id)
        else {
            continue;
        };
        let title = surface
            .definition
            .as_ref()
            .map(|definition| definition.name.trim())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| workspace_view::surface_value(surface))
            .to_owned();
        let title = if title.is_empty() { id.clone() } else { title };
        let title_width = rune::text::measure(&title, &[], ui_font(13.0, 500.0), None)
            .0
            .ceil()
            .clamp(1.0, 220.0);
        let active = state.canvas_surface.active() == Some(id.as_str());
        let select_id = id.clone();
        let close_id = id.clone();
        tabs = tabs.child(
            row()
                .key(format!("surface-canvas-tab-{index}"))
                .w(title_width + 64.0)
                .h(34.0)
                .px(7.0)
                .center_y()
                .gap(5.0)
                .radius(11.0)
                .fill(theme.color(if active { "#303030" } else { "#242424" }))
                .border(1.0, theme.color(if active { "#555555" } else { "#383838" }))
                .child(
                    row()
                        .key(format!("surface-canvas-tab-{index}-select"))
                        .role("tab")
                        .label(title.clone())
                        .w(title_width + 23.0)
                        .h_full()
                        .center_y()
                        .gap(6.0)
                        .on_press(move |state: &mut State| select(state, &select_id))
                        .child(
                            icon(&WORKSPACE_SURFACES, 16.0).color(theme.color(if active {
                                "#d8d8d8"
                            } else {
                                "#929292"
                            })),
                        )
                        .child(
                            text(title.clone())
                                .w(title_width + 2.0)
                                .fade_truncate()
                                .font(ui_font(13.0, 500.0))
                                .color(theme.color(if active { "#eeeeee" } else { "#aaaaaa" })),
                        ),
                )
                .child(
                    el().key(format!("surface-canvas-tab-{index}-close"))
                        .role("button")
                        .label(format!("Close {title}"))
                        .size(20.0, 20.0)
                        .center()
                        .radius(6.0)
                        .hover(theme.color("#484848"))
                        .on_press(move |state: &mut State| close(state, &close_id))
                        .child(icon(&CLOSE, 12.0).color(theme.color("#a6a6a6"))),
                ),
        );
    }
    Some(
        col()
            .key("surface-canvas-tab-bar")
            .w_full()
            .h(crate::runbook_canvas::TAB_BAR_HEIGHT)
            .fill(theme.color(CONTENT_SURFACE_COLOR))
            .child(tabs),
    )
}

pub(crate) fn render(state: &State, theme: ThemeStyle<'_>, height: f32) -> El {
    let scroll_watch = state.canvas_surface_scroll.clone();
    row()
        .key("surface-canvas-body")
        .w_full()
        .h(height)
        .min_h(0.0)
        .child(
            col()
                .key("surface-canvas-scroll")
                .grow(1.0)
                .min_w(0.0)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .scroll_edge_fade(20.0)
                .on_scroll_offset(move |position| scroll_watch.watch_scroll(position))
                .on_wheel(|state: &mut State, event, tx| {
                    state.canvas_surface_scroll.wheel(event, tx)
                })
                .child(workspace_view::surface_canvas_details(state, theme)),
        )
        .child(state.canvas_surface_scroll.bar(theme))
}
