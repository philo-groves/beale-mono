#[path = "surface_graph.rs"]
mod surface_graph;
#[path = "surface_web.rs"]
mod surface_web;

pub(crate) fn surface_filter_items(state: &State) -> Vec<crate::in_place_menu::MenuItem> {
    surface_graph::filter_items(state)
}

pub(crate) fn select_surface_filter(state: &mut State, value: &str) {
    surface_graph::select_filter(state, value);
}

pub(crate) fn surface_filter_menu(state: &State, theme: ThemeStyle<'_>) -> El {
    surface_graph::filter_menu(state, theme)
}

use crate::design::FadeText;
use beale_contracts::v1::{
    SurfaceDefinition, SurfaceKind, SurfaceLocator, SurfaceRelationship, SurfaceScope,
    SurfaceScopeState, WorkspaceDetails, WorkspaceSurface,
};
use rune::{Color, El, Icon, ScrollSpec, TextEdit, col, el, icon, row, text, text_input};

use crate::{
    SelectionMenu, State, ThemeStyle, ViewLocation, form_button,
    form_controls::icon_action_button,
    form_label_font,
    icons::{EVIDENCE, FINDINGS, PEN, PLUS, SAFETY, SETTINGS, TARGET, TRASH, WORKSPACE_SURFACES},
    modal_dialog, selection_button, settings_form_card, toggle_selection_menu, ui_font,
    view_title_font,
    workspace_creation::{
        SurfaceKind as DraftKind, SurfaceScope as DraftScope, display_directory, field,
        row_with_control, value_with_actions,
    },
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum WorkspaceTab {
    Campaign,
    Foundation,
    Surfaces,
    Findings,
    #[default]
    Rules,
}

impl WorkspaceTab {
    const ALL: [Self; 4] = [
        Self::Surfaces,
        Self::Findings,
        Self::Rules,
        Self::Foundation,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Campaign => "Campaign",
            Self::Surfaces => "Surfaces",
            Self::Findings => "Findings",
            Self::Rules => "Guidance",
            Self::Foundation => "Identity",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Campaign => "workspace-tab-campaign",
            Self::Surfaces => "workspace-tab-surfaces",
            Self::Findings => "workspace-tab-findings",
            Self::Rules => "workspace-tab-guidance",
            Self::Foundation => "workspace-tab-identity",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum GuidanceView {
    Agents,
    #[default]
    Rules,
}

impl GuidanceView {
    fn label(self) -> &'static str {
        match self {
            Self::Agents => "AGENTS.md",
            Self::Rules => "Rules",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Agents => "workspace-guidance-agents",
            Self::Rules => "workspace-guidance-rules",
        }
    }

    fn icon(self) -> &'static Icon {
        match self {
            Self::Agents => &EVIDENCE,
            Self::Rules => &SAFETY,
        }
    }
}

fn guidance_toggle(state: &State, labeled: bool, theme: ThemeStyle<'_>) -> El {
    let mut tabs = crate::design::segmented_control(theme)
        .key("workspace-guidance-view-toggle")
        .label("Guidance views")
        .min_w(guidance_toggle_width(labeled));
    for view in [GuidanceView::Rules, GuidanceView::Agents] {
        let button = if labeled {
            crate::design::segmented_tab(
                view.label(),
                Some(view.icon()),
                state.workspace_guidance_view == view,
                true,
                theme,
            )
        } else {
            crate::design::segmented_icon_tab(
                view.label(),
                view.icon(),
                state.workspace_guidance_view == view,
                theme,
            )
        };
        tabs = tabs.child(
            button
                .key(view.key())
                .on_press_tx(move |state: &mut State, tx| {
                    state.workspace_guidance_view = view;
                    tx.scroll_to_y("workspace-scroll", 0.0);
                }),
        );
    }
    tabs
}

fn guidance_toggle_width(labeled: bool) -> f32 {
    if labeled {
        crate::design::segmented_group_width(&[
            crate::design::segmented_tab_width("Rules", true),
            crate::design::segmented_tab_width("AGENTS.md", true),
        ])
    } else {
        crate::design::segmented_group_width(&[crate::design::CONTROL_HEIGHT; 2])
    }
}

fn tab_icon(tab: WorkspaceTab) -> &'static Icon {
    match tab {
        WorkspaceTab::Surfaces => &WORKSPACE_SURFACES,
        WorkspaceTab::Findings => &FINDINGS,
        WorkspaceTab::Campaign => &TARGET,
        WorkspaceTab::Foundation => &SETTINGS,
        WorkspaceTab::Rules => &SAFETY,
    }
}

/// Width the labeled workspace view tabs need.
fn labeled_tabs_width(tabs: &[WorkspaceTab]) -> f32 {
    let widths: Vec<f32> = tabs
        .iter()
        .map(|tab| crate::design::segmented_tab_width(tab.label(), true))
        .collect();
    crate::design::segmented_group_width(&widths)
}

/// Narrowest workspace title kept beside labeled tabs before the tabs drop their labels.
const TITLE_MIN_WIDTH_BESIDE_LABELS: f32 = 280.0;

fn tab_button(
    tab: WorkspaceTab,
    selected: bool,
    labeled: bool,
    id: uuid::Uuid,
    theme: ThemeStyle<'_>,
) -> El {
    let button = if labeled {
        crate::design::segmented_tab(tab.label(), Some(tab_icon(tab)), selected, true, theme)
    } else {
        crate::design::segmented_icon_tab(tab.label(), tab_icon(tab), selected, theme)
    };
    button
        .key(tab.key())
        .on_press_tx(move |state: &mut State, tx| {
            state.navigate_to(ViewLocation::Workspace(id, tab));
            tx.scroll_to_y("workspace-scroll", 0.0);
        })
}

fn read_only_value(value: String, width: f32, theme: ThemeStyle<'_>) -> El {
    row()
        .w(width)
        .min_w(0.0)
        .h(32.0)
        .px(10.0)
        .center_y()
        .radius(8.0)
        .continuous_corners()
        .fill(theme.color("#282828"))
        .child(
            text(value)
                .min_w(0.0)
                .fade_truncate()
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#e2e2e2")),
        )
}

fn draft_kind(kind: i32) -> DraftKind {
    match SurfaceKind::try_from(kind) {
        Ok(SurfaceKind::Repository) => DraftKind::Repository,
        Ok(SurfaceKind::Documentation) => DraftKind::Documentation,
        Ok(SurfaceKind::Binary) => DraftKind::Binary,
        Ok(SurfaceKind::Service) => DraftKind::Service,
        Ok(SurfaceKind::Domain) => DraftKind::Domain,
        _ => DraftKind::Other,
    }
}

fn surface_type_id(kind: i32) -> &'static str {
    match SurfaceKind::try_from(kind) {
        Ok(SurfaceKind::Repository) => "repository",
        Ok(SurfaceKind::Documentation) => "documentation",
        Ok(SurfaceKind::Binary) => "binary",
        Ok(SurfaceKind::Service) => "service",
        Ok(SurfaceKind::Domain) => "domain",
        _ => "other",
    }
}

fn scope_state_label(value: i32) -> &'static str {
    match SurfaceScopeState::try_from(value) {
        Ok(SurfaceScopeState::Candidate) => "Candidate",
        Ok(SurfaceScopeState::Approved) => "Approved",
        _ => "Unknown",
    }
}

pub(crate) fn surface_value(surface: &WorkspaceSurface) -> &str {
    surface
        .definition
        .as_ref()
        .and_then(|item| item.locator.as_ref())
        .map_or("", |locator| locator.value.as_str())
}

fn surface_scope(surface: &WorkspaceSurface) -> i32 {
    surface.definition.as_ref().map_or(0, |item| item.scope)
}

fn surface_scope_state(surface: &WorkspaceSurface) -> i32 {
    surface
        .definition
        .as_ref()
        .map_or(0, |item| item.scope_state)
}

fn surface_kind(surface: &WorkspaceSurface) -> i32 {
    (match surface
        .definition
        .as_ref()
        .map_or("", |item| item.type_id.as_str())
    {
        "repository" | "code.module" | "code.package" | "code.file" => SurfaceKind::Repository,
        "documentation" => SurfaceKind::Documentation,
        "binary" | "configuration" => SurfaceKind::Binary,
        "service" | "api.endpoint" => SurfaceKind::Service,
        "domain" | "host" => SurfaceKind::Domain,
        _ => SurfaceKind::Other,
    }) as i32
}

fn surface_revision(surface: &WorkspaceSurface) -> u64 {
    surface.lineage.last().map_or(0, |event| event.number)
}

fn surface_updated_at(surface: &WorkspaceSurface) -> u64 {
    surface.lineage.last().map_or(0, |event| event.at_ms)
}

fn approve_surface_scope(state: &mut State, index: usize) {
    if state.workspace_save_busy.is_none()
        && let Some(surface) = state.workspace_surface_edits.get_mut(index)
        && let Some(definition) = &mut surface.definition
    {
        definition.scope_state = SurfaceScopeState::Approved as i32;
    }
}

fn open_surface_editor(state: &mut State, index: Option<usize>, scope: DraftScope) {
    state.workspace_surface_edit_index = index;
    state.workspace_surface_dialog_scope = Some(scope);
    state.workspace_surface_error.clear();
    let surface = index.and_then(|index| state.workspace_surface_edits.get(index));
    state.workspace_surface_kind = surface.map_or(DraftKind::default(), |surface| {
        draft_kind(surface_kind(surface))
    });
    state.workspace_surface_value = TextEdit::new(surface.map_or("", surface_value));
    let definition = surface.and_then(|surface| surface.definition.as_ref());
    state.workspace_surface_type_id =
        TextEdit::new(definition.map_or("", |item| item.type_id.as_str()));
    state.workspace_surface_name = TextEdit::new(definition.map_or("", |item| item.name.as_str()));
    state.workspace_surface_locator_kind = TextEdit::new(
        definition
            .and_then(|item| item.locator.as_ref())
            .map_or("freeform", |locator| locator.kind.as_str()),
    );
    state.workspace_surface_locator_value = TextEdit::new(
        definition
            .and_then(|item| item.locator.as_ref())
            .map_or("", |locator| locator.value.as_str()),
    );
    state.workspace_surface_description =
        TextEdit::new(definition.map_or("", |item| item.description.as_str()));
}

fn foundation(state: &State, details: &WorkspaceDetails, theme: ThemeStyle<'_>) -> El {
    let Some(summary) = details.summary.as_ref() else {
        return col();
    };
    let busy = state.workspace_save_busy.is_some();
    let name_actions = if busy {
        Vec::new()
    } else {
        vec![
            icon_action_button(
                "saved-workspace-name-edit",
                "Edit workspace name",
                &PEN,
                theme,
            )
            .on_press(|state: &mut State| {
                state.workspace_name_dialog_value = TextEdit::new(&state.workspace_name_edit.text);
                state.workspace_name_dialog_error.clear();
                state.workspace_name_dialog_open = true;
            }),
        ]
    };
    let name = value_with_actions(
        "saved-workspace-name-value",
        state.workspace_name_edit.text.clone(),
        320.0,
        name_actions,
        theme,
    );
    col()
        .key("saved-workspace-details-form")
        .w_full()
        .child(settings_form_card(
            [
                row_with_control(
                    "Workspace Name",
                    "The name shown throughout Beale.",
                    name,
                    theme,
                ),
                row_with_control(
                    "Workspace Directory",
                    "Directory for this workspace on the host.",
                    value_with_actions(
                        "saved-workspace-directory-value",
                        display_directory(&summary.directory),
                        320.0,
                        Vec::new(),
                        theme,
                    ),
                    theme,
                ),
                row_with_control(
                    "Research Kit",
                    "Optional surface and rule acquisition kit.",
                    read_only_value("None".to_owned(), 100.0, theme),
                    theme,
                ),
            ],
            theme,
        ))
}

fn agents_guidance(state: &State, theme: ThemeStyle<'_>) -> El {
    let guidance = if state.workspace_save_busy.is_some() {
        col()
            .w_full()
            .min_h(130.0)
            .padding(10.0)
            .radius(8.0)
            .fill(theme.color("#282828"))
            .child(
                text(state.workspace_guidance_edit.text.clone())
                    .w_full()
                    .wrap()
                    .font(ui_font(14.0, 400.0))
                    .color(theme.color("#e2e2e2")),
            )
    } else {
        text_input(&state.workspace_guidance_edit, ui_font(14.0, 400.0))
            .key("saved-workspace-guidance-input")
            .w_full()
            .h(130.0)
            .padding(10.0)
            .radius(8.0)
            .fill(theme.color("#282828"))
            .color(theme.color("#e2e2e2"))
            .on_edit(|state: &mut State| &mut state.workspace_guidance_edit)
    };
    col()
        .key("saved-workspace-guidance-form")
        .w_full()
        .child(settings_form_card(
            [col()
                .key("saved-workspace-guidance-row")
                .w_full()
                .py(12.0)
                .gap(8.0)
                .child(
                    col()
                        .gap(4.0)
                        .child(
                            text("AGENTS.md")
                                .key("saved-workspace-guidance-label")
                                .font(form_label_font())
                                .color(theme.color("#e2e2e2")),
                        )
                        .child(
                            text("Instructions for research in this workspace.")
                                .key("saved-workspace-guidance-description")
                                .font(ui_font(14.0, 400.0))
                                .color(theme.color("#969696")),
                        ),
                )
                .child(guidance)],
            theme,
        ))
}

fn close_name_dialog(state: &mut State) {
    state.workspace_name_dialog_open = false;
    state.workspace_name_dialog_value.clear();
    state.workspace_name_dialog_error.clear();
}

fn apply_name_dialog(state: &mut State) {
    if !state.workspace_name_dialog_open {
        return;
    }
    let value = state.workspace_name_dialog_value.text.trim();
    if value.is_empty() || value.chars().count() > 200 || value.chars().any(char::is_control) {
        state.workspace_name_dialog_error = "Enter a name of at most 200 characters.".to_owned();
        return;
    }
    state.workspace_name_edit = TextEdit::new(value);
    state.workspace_save_error.clear();
    state.workspace_save_message.clear();
    close_name_dialog(state);
}

pub(crate) fn name_dialog(state: &State, theme: ThemeStyle<'_>) -> [El; 2] {
    let mut body = col().w_full().gap(10.0).child(settings_form_card(
        [row_with_control(
            "Workspace Name",
            "Save Changes to apply the new name.",
            field(
                &state.workspace_name_dialog_value,
                "saved-workspace-name-dialog-input",
                "Name",
                theme,
                |state| &mut state.workspace_name_dialog_value,
            )
            .w(300.0),
            theme,
        )],
        theme,
    ));
    if !state.workspace_name_dialog_error.is_empty() {
        body = body.child(
            text(state.workspace_name_dialog_error.clone())
                .key("saved-workspace-name-dialog-error")
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#e07676")),
        );
    }
    let actions = row()
        .w_full()
        .gap(8.0)
        .child(el().grow(1.0))
        .child(
            form_button("Cancel", theme)
                .key("saved-workspace-name-dialog-cancel")
                .on_press(close_name_dialog),
        )
        .child(
            form_button("Apply", theme)
                .key("saved-workspace-name-dialog-apply")
                .on_press(apply_name_dialog),
        );
    modal_dialog::render(
        modal_dialog::DialogSpec {
            key: "saved-workspace-name-dialog",
            title: "Edit Workspace Name",
            width: 540.0,
            focus_return: "saved-workspace-name-edit",
        },
        body,
        actions,
        theme,
        close_name_dialog,
    )
}

static REPOSITORY_ICON: Icon = Icon {
    name: "beale-surface-repository",
    view: 24.0,
    body: r#"<path d="M4 3h16v18H4zM8 3v18M11 8h6M11 12h6" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/>"#,
};
static DOCUMENTATION_ICON: Icon = Icon {
    name: "beale-surface-documentation",
    view: 24.0,
    body: r#"<path d="M5 3h10l4 4v14H5zM15 3v5h4M8 12h8M8 16h8" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/>"#,
};
static BINARY_ICON: Icon = Icon {
    name: "beale-surface-binary",
    view: 24.0,
    body: r#"<rect x="3" y="5" width="18" height="14" rx="2" fill="none" stroke="currentColor" stroke-width="1.8"/><path d="m7 10 3 2-3 2m6 0h4" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/>"#,
};
static SERVICE_ICON: Icon = Icon {
    name: "beale-surface-service",
    view: 24.0,
    body: r#"<rect x="4" y="3" width="16" height="7" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.8"/><rect x="4" y="14" width="16" height="7" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.8"/><path d="M8 6.5h.01M8 17.5h.01" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"/>"#,
};
static DOMAIN_ICON: Icon = Icon {
    name: "beale-surface-domain",
    view: 24.0,
    body: r#"<circle cx="12" cy="12" r="9" fill="none" stroke="currentColor" stroke-width="1.8"/><path d="M3 12h18M12 3c-5 5-5 13 0 18M12 3c5 5 5 13 0 18" fill="none" stroke="currentColor" stroke-width="1.8"/>"#,
};
static OTHER_ICON: Icon = Icon {
    name: "beale-surface-other",
    view: 24.0,
    body: r#"<circle cx="12" cy="12" r="9" fill="none" stroke="currentColor" stroke-width="1.8"/><path d="M12 8v4m0 4h.01" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>"#,
};

fn surface_icon(kind: i32) -> &'static Icon {
    match SurfaceKind::try_from(kind) {
        Ok(SurfaceKind::Repository) => &REPOSITORY_ICON,
        Ok(SurfaceKind::Documentation) => &DOCUMENTATION_ICON,
        Ok(SurfaceKind::Binary) => &BINARY_ICON,
        Ok(SurfaceKind::Service) => &SERVICE_ICON,
        Ok(SurfaceKind::Domain) => &DOMAIN_ICON,
        _ => &OTHER_ICON,
    }
}

fn network(state: &State, theme: ThemeStyle<'_>, width: f32, height: f32, header: El) -> El {
    surface_graph::render(state, theme, width, height, header)
}

fn surface_detail(label: &str, value: &str, theme: ThemeStyle<'_>) -> El {
    row()
        .w_full()
        .gap(14.0)
        .child(
            text(label.to_owned())
                .w(112.0)
                .font(ui_font(12.0, 500.0))
                .color(theme.color("#999999")),
        )
        .child(
            text(value.to_owned())
                .grow(1.0)
                .min_w(0.0)
                .wrap()
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#dddddd")),
        )
}

fn surface_canvas_section(key: &str, title: &str, body: El, theme: ThemeStyle<'_>) -> El {
    col()
        .key(format!("surface-canvas-section-{key}"))
        .w_full()
        .gap(8.0)
        .child(
            text(title.to_owned())
                .font(ui_font(13.0, 600.0))
                .color(theme.color("#d8d8d8")),
        )
        .child(
            body.w_full()
                .padding(12.0)
                .gap(12.0)
                .radius(8.0)
                .border(1.0, theme.color("#383838")),
        )
}

pub(crate) fn surface_canvas_details(state: &State, theme: ThemeStyle<'_>) -> El {
    let Some(index) = state.workspace_surface_selected else {
        return col();
    };
    let Some(surface) = state.workspace_surface_edits.get(index) else {
        return col();
    };
    let updated = (surface_updated_at(surface) > 0)
        .then(|| surface_updated_at(surface))
        .and_then(|ms| chrono::DateTime::from_timestamp_millis(ms as i64))
        .map_or_else(
            || "Unknown (predates tracking)".to_owned(),
            |time| time.format("%Y-%m-%d %H:%M UTC").to_string(),
        );
    let kind = draft_kind(surface_kind(surface)).label();
    let scope_color = match SurfaceScope::try_from(surface_scope(surface)) {
        Ok(SurfaceScope::InScope) => theme.accent(),
        Ok(SurfaceScope::OutOfScope) => theme.color("#e07676"),
        _ => theme.color("#dca15f"),
    };
    let review_color = if surface_scope_state(surface) == SurfaceScopeState::Approved as i32 {
        theme.color("#77bf93")
    } else {
        theme.color("#dca15f")
    };
    let mut details = col()
        .key("surface-canvas-details")
        .w_full()
        .gap(16.0)
        .padding(20.0)
        .child(
            col()
                .key("surface-canvas-summary")
                .w_full()
                .padding(12.0)
                .gap(10.0)
                .radius(8.0)
                .border(1.0, theme.color("#383838"))
                .child(
                    row()
                        .w_full()
                        .center_y()
                        .gap(8.0)
                        .child(
                            icon(surface_icon(surface_kind(surface)), 16.0)
                                .color(theme.color("#d8d8d8")),
                        )
                        .child(
                            text(kind)
                                .font(ui_font(13.0, 600.0))
                                .color(theme.color("#e4e4e4")),
                        )
                        .child(el().grow(1.0))
                        .child(
                            text(surface_graph::scope(surface))
                                .font(ui_font(12.0, 500.0))
                                .color(scope_color),
                        )
                        .child(
                            text("·")
                                .font(ui_font(12.0, 400.0))
                                .color(theme.color("#929292")),
                        )
                        .child(
                            text(scope_state_label(surface_scope_state(surface)))
                                .font(ui_font(12.0, 500.0))
                                .color(review_color),
                        ),
                )
                .child(
                    text(format!(
                        "{} · {} updates · Updated {}",
                        surface.id,
                        surface_revision(surface).saturating_sub(1),
                        updated
                    ))
                    .w_full()
                    .wrap()
                    .font(ui_font(12.0, 400.0))
                    .color(theme.color("#929292")),
                ),
        );
    let mut actions = row()
        .key("surface-canvas-actions")
        .w_full()
        .center_y()
        .gap(8.0);
    let mut has_actions = false;
    if surface_scope_state(surface) != SurfaceScopeState::Approved as i32 {
        actions = actions.child(
            form_button("Approve Scope", theme)
                .key("workspace-surface-approve-scope")
                .on_press(move |state: &mut State| approve_surface_scope(state, index)),
        );
        has_actions = true;
    }
    if state.workspace_surface_edits.len() > 1 {
        let connecting = state.workspace_surface_connecting == Some(index);
        actions = actions.child(
            form_button(
                if connecting {
                    "Cancel Connection"
                } else {
                    "Connect to Surface"
                },
                theme,
            )
            .key("workspace-surface-connect")
            .on_press(move |state: &mut State| {
                state.workspace_surface_connecting = if connecting { None } else { Some(index) };
            }),
        );
        has_actions = true;
    }
    if has_actions {
        details = details.child(actions);
    }
    if state.workspace_surface_connecting == Some(index) {
        details = details.child(
            text("Select the other surface in the network.")
                .font(ui_font(12.0, 400.0))
                .color(theme.color("#a6a6a6")),
        );
    }
    let mut definition_rows = col().w_full().gap(12.0);
    if let Some(definition) = &surface.definition {
        definition_rows =
            definition_rows.child(surface_detail("Specific type", &definition.type_id, theme));
        if let Some(locator) = &definition.locator {
            definition_rows = definition_rows.child(surface_detail(
                "Locator",
                &format!("{}: {}", locator.kind, locator.value),
                theme,
            ));
        }
        if !definition.description.is_empty() {
            definition_rows = definition_rows.child(surface_detail(
                "Description",
                &definition.description,
                theme,
            ));
        }
        if !definition.out_of_scope_reason.is_empty() {
            definition_rows = definition_rows.child(surface_detail(
                "Exclusion reason",
                &definition.out_of_scope_reason,
                theme,
            ));
        }
    } else {
        definition_rows = definition_rows.child(
            text("No definition recorded.")
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#929292")),
        );
    }
    details = details.child(surface_canvas_section(
        "definition",
        "Definition",
        definition_rows,
        theme,
    ));
    if let Some(definition) = &surface.definition
        && !definition.evidence_refs.is_empty()
    {
        let mut evidence_rows = col().w_full().gap(12.0);
        for evidence in &definition.evidence_refs {
            evidence_rows = evidence_rows.child(surface_detail(
                &evidence.kind,
                &format!(
                    "{}{}",
                    evidence.reference,
                    if evidence.note.is_empty() {
                        String::new()
                    } else {
                        format!(" — {}", evidence.note)
                    }
                ),
                theme,
            ));
        }
        details = details.child(surface_canvas_section(
            "evidence",
            "Evidence",
            evidence_rows,
            theme,
        ));
    }
    if !surface.relationships.is_empty() {
        let mut connections = col().w_full().gap(12.0);
        for (link_index, relationship) in surface.relationships.iter().enumerate() {
            let target = state
                .workspace_surface_edits
                .iter()
                .find(|candidate| candidate.id == relationship.target_id)
                .map_or("Unknown surface", surface_value);
            let label = if relationship.label.is_empty() {
                if relationship.type_id.is_empty() {
                    "Connection"
                } else {
                    &relationship.type_id
                }
            } else {
                &relationship.label
            };
            let metadata = if relationship.type_id.is_empty() {
                format!("{} references", relationship.evidence_refs.len())
            } else {
                format!(
                    "{} · {} references",
                    relationship.type_id,
                    relationship.evidence_refs.len()
                )
            };
            connections = connections.child(
                row()
                    .w_full()
                    .center_y()
                    .gap(8.0)
                    .child(
                        col()
                            .grow(1.0)
                            .min_w(0.0)
                            .gap(3.0)
                            .child(
                                text(format!("{label} → {target}"))
                                    .w_full()
                                    .fade_truncate()
                                    .font(ui_font(13.0, 500.0))
                                    .color(theme.color("#dddddd")),
                            )
                            .child(
                                text(metadata)
                                    .font(ui_font(12.0, 400.0))
                                    .color(theme.color("#929292")),
                            ),
                    )
                    .child(
                        icon_action_button(
                            format!("workspace-surface-unlink-{index}-{link_index}"),
                            "Remove connection",
                            &TRASH,
                            theme,
                        )
                        .on_press(move |state: &mut State| {
                            if state.workspace_save_busy.is_some() {
                                return;
                            }
                            if let Some(surface) = state.workspace_surface_edits.get_mut(index)
                                && link_index < surface.relationships.len()
                            {
                                surface.relationships.remove(link_index);
                            }
                        }),
                    ),
            );
        }
        details = details.child(surface_canvas_section(
            "connections",
            "Connections",
            connections,
            theme,
        ));
    }
    if !surface.lineage.is_empty() {
        let mut history = col().w_full().gap(12.0);
        for event in surface.lineage.iter().rev().take(5) {
            let fields = event
                .changes
                .iter()
                .map(|change| change.field.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let summary = if fields.is_empty() {
                format!("{} · {}", event.actor, event.action)
            } else {
                format!("{} · {} · {}", event.actor, event.action, fields)
            };
            history = history.child(surface_detail(
                &format!("Revision {}", event.number),
                &summary,
                theme,
            ));
        }
        details = details.child(surface_canvas_section("history", "History", history, theme));
    }
    details
}

fn close_relationship_dialog(state: &mut State) {
    state.workspace_surface_relationship_target = None;
    state.workspace_surface_relationship_label.clear();
    state.workspace_surface_relationship_type_id.clear();
    state.workspace_surface_relationship_error.clear();
}

fn commit_relationship(state: &mut State) {
    let (Some(source), Some(target)) = (
        state.workspace_surface_selected,
        state.workspace_surface_relationship_target,
    ) else {
        return;
    };
    let label = state.workspace_surface_relationship_label.text.trim();
    if label.is_empty() || label.chars().count() > 80 || label.chars().any(char::is_control) {
        state.workspace_surface_relationship_error =
            "Enter a relationship label of at most 80 characters.".to_owned();
        return;
    }
    let supplied_type_id = state.workspace_surface_relationship_type_id.text.trim();
    let type_id = if supplied_type_id.is_empty() && label.eq_ignore_ascii_case("contains") {
        "contains"
    } else {
        supplied_type_id
    };
    if type_id.len() > 64
        || !type_id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'.' || byte == b'_'
        })
    {
        state.workspace_surface_relationship_error =
            "Use a lowercase relationship type of at most 64 characters.".to_owned();
        return;
    }
    let Some(target_id) = state
        .workspace_surface_edits
        .get(target)
        .map(|surface| surface.id.clone())
    else {
        close_relationship_dialog(state);
        return;
    };
    if type_id == "contains" {
        let hierarchy = crate::workspace_surface_layout::hierarchy(&state.workspace_surface_edits);
        if hierarchy.parent[target].is_some() {
            state.workspace_surface_relationship_error =
                "This surface already has a parent.".to_owned();
            return;
        }
        let mut ancestor = Some(source);
        while let Some(index) = ancestor {
            if index == target {
                state.workspace_surface_relationship_error =
                    "A surface cannot contain its ancestor.".to_owned();
                return;
            }
            ancestor = hierarchy.parent[index];
        }
    }
    let Some(surface) = state.workspace_surface_edits.get_mut(source) else {
        close_relationship_dialog(state);
        return;
    };
    if surface
        .relationships
        .iter()
        .any(|relationship| relationship.target_id == target_id)
    {
        state.workspace_surface_relationship_error =
            "These surfaces are already connected.".to_owned();
        return;
    }
    surface.relationships.push(SurfaceRelationship {
        target_id,
        label: label.to_owned(),
        type_id: type_id.to_owned(),
        evidence_refs: Vec::new(),
    });
    close_relationship_dialog(state);
}

pub(crate) fn relationship_dialog(state: &State, theme: ThemeStyle<'_>) -> [El; 2] {
    let source = state
        .workspace_surface_selected
        .and_then(|index| state.workspace_surface_edits.get(index));
    let target = state
        .workspace_surface_relationship_target
        .and_then(|index| state.workspace_surface_edits.get(index));
    let description = format!(
        "{} → {}",
        source.map_or("Surface", surface_value),
        target.map_or("Surface", surface_value)
    );
    let mut body = col()
        .w_full()
        .gap(10.0)
        .child(
            text(description)
                .w_full()
                .wrap()
                .font(ui_font(13.0, 500.0))
                .color(theme.color("#bdbdbd")),
        )
        .child(settings_form_card(
            [
                row_with_control(
                    "Relationship",
                    "How these surfaces are connected.",
                    field(
                        &state.workspace_surface_relationship_label,
                        "workspace-surface-relationship-input",
                        "e.g. calls",
                        theme,
                        |state| &mut state.workspace_surface_relationship_label,
                    ),
                    theme,
                ),
                row_with_control(
                    "Connection type",
                    "Optional machine-readable type, such as contains or depends_on.",
                    field(
                        &state.workspace_surface_relationship_type_id,
                        "workspace-surface-relationship-type",
                        "contains",
                        theme,
                        |state| &mut state.workspace_surface_relationship_type_id,
                    ),
                    theme,
                ),
            ],
            theme,
        ));
    if !state.workspace_surface_relationship_error.is_empty() {
        body = body.child(
            text(state.workspace_surface_relationship_error.clone())
                .font(ui_font(12.0, 400.0))
                .color(theme.color("#e07676")),
        );
    }
    let actions = row()
        .w_full()
        .gap(8.0)
        .child(el().grow(1.0))
        .child(form_button("Cancel", theme).on_press(close_relationship_dialog))
        .child(
            form_button("Add Connection", theme)
                .key("workspace-surface-relationship-add")
                .on_press(commit_relationship),
        );
    modal_dialog::render(
        modal_dialog::DialogSpec {
            key: "workspace-surface-relationship-dialog",
            title: "Connect Surfaces",
            width: 520.0,
            focus_return: "workspace-surface-connect",
        },
        body,
        actions,
        theme,
        close_relationship_dialog,
    )
}

fn open_rule_dialog(state: &mut State) {
    state.workspace_rule_input.clear();
    state.workspace_rule_dialog_error.clear();
    state.workspace_rule_dialog_open = true;
}

fn rules(state: &State, theme: ThemeStyle<'_>) -> El {
    let mut list = col().key("saved-workspace-rules-form").w_full();
    if state.workspace_rule_edits.is_empty() {
        list = list.child(
            text("No workspace rules recorded.")
                .px(20.0)
                .py(12.0)
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#969696")),
        );
    }
    for (index, rule) in state.workspace_rule_edits.iter().enumerate() {
        if index > 0 {
            list = list.child(
                el().key(format!("saved-workspace-rule-separator-{index}"))
                    .role("separator")
                    .w_full()
                    .h(1.0)
                    .fill(theme.color("#303030")),
            );
        }
        let remove = icon_action_button(
            format!("saved-workspace-rule-remove-{index}"),
            "Remove rule",
            &TRASH,
            theme,
        );
        list = list.child(
            row()
                .key(format!("saved-workspace-rule-{index}"))
                .w_full()
                .center_y()
                .gap(8.0)
                .px(20.0)
                .py(8.0)
                .child(
                    text(rule.clone())
                        .grow(1.0)
                        .min_w(0.0)
                        .wrap()
                        .font(ui_font(14.0, 400.0))
                        .color(theme.color("#e2e2e2")),
                )
                .child(if state.workspace_save_busy.is_some() {
                    remove
                } else {
                    remove.on_press(move |state: &mut State| {
                        state.workspace_rule_edits.remove(index);
                    })
                }),
        );
    }
    list
}

fn close_rule_dialog(state: &mut State) {
    state.workspace_rule_dialog_open = false;
    state.workspace_rule_dialog_error.clear();
    state.workspace_rule_input.clear();
}

fn add_rule(state: &mut State) {
    if !state.workspace_rule_dialog_open {
        return;
    }
    let rule = state.workspace_rule_input.text.trim();
    if rule.is_empty()
        || rule.len() > 2_000
        || rule
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
    {
        state.workspace_rule_dialog_error = "Enter a rule of at most 2,000 characters.".to_owned();
        return;
    }
    if state.workspace_rule_edits.iter().any(|item| item == rule) {
        state.workspace_rule_dialog_error = "That rule is already listed.".to_owned();
        return;
    }
    state.workspace_rule_edits.push(rule.to_owned());
    state.workspace_save_error.clear();
    state.workspace_save_message.clear();
    close_rule_dialog(state);
}

pub(crate) fn rule_dialog(state: &State, theme: ThemeStyle<'_>) -> [El; 2] {
    let form = settings_form_card(
        [row_with_control(
            "Rule",
            "Operating constraint for research sessions.",
            field(
                &state.workspace_rule_input,
                "saved-workspace-rule-input",
                "Enter a rule",
                theme,
                |state| &mut state.workspace_rule_input,
            ),
            theme,
        )],
        theme,
    );
    let mut body = col().w_full().gap(10.0).child(form);
    if !state.workspace_rule_dialog_error.is_empty() {
        body = body.child(
            text(state.workspace_rule_dialog_error.clone())
                .key("saved-workspace-rule-dialog-error")
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#e07676")),
        );
    }
    let actions = row()
        .w_full()
        .gap(8.0)
        .child(el().grow(1.0))
        .child(
            form_button("Cancel", theme)
                .key("saved-workspace-rule-dialog-cancel")
                .on_press(close_rule_dialog),
        )
        .child(
            form_button("Add Rule", theme)
                .key("saved-workspace-rule-dialog-add")
                .on_press(add_rule),
        );
    modal_dialog::render(
        modal_dialog::DialogSpec {
            key: "saved-workspace-rule-dialog",
            title: "Add a Rule",
            width: 540.0,
            focus_return: "saved-workspace-add-rule",
        },
        body,
        actions,
        theme,
        close_rule_dialog,
    )
}

fn close_surface_dialog(state: &mut State) {
    state.workspace_surface_dialog_scope = None;
    state.workspace_surface_edit_index = None;
    state.workspace_surface_value.clear();
    state.workspace_surface_error.clear();
    state.selection_menu = None;
}

fn commit_surface(state: &mut State) {
    let Some(scope) = state.workspace_surface_dialog_scope else {
        return;
    };
    let value = state.workspace_surface_value.text.trim();
    if value.is_empty() || value.chars().count() > 2_048 {
        state.workspace_surface_error = "Enter a surface of at most 2,048 characters.".to_owned();
        return;
    }
    let kind = match state.workspace_surface_kind {
        DraftKind::Repository => SurfaceKind::Repository,
        DraftKind::Documentation => SurfaceKind::Documentation,
        DraftKind::Binary => SurfaceKind::Binary,
        DraftKind::Service => SurfaceKind::Service,
        DraftKind::Domain => SurfaceKind::Domain,
        DraftKind::Other => SurfaceKind::Other,
    };
    if state
        .workspace_surface_edits
        .iter()
        .enumerate()
        .any(|(index, item)| {
            Some(index) != state.workspace_surface_edit_index
                && surface_kind(item) == kind as i32
                && surface_value(item).eq_ignore_ascii_case(value)
        })
    {
        state.workspace_surface_error = "That surface is already listed.".to_owned();
        return;
    }
    let definition = SurfaceDefinition {
        type_id: if state.workspace_surface_type_id.text.trim().is_empty() {
            surface_type_id(kind as i32).to_owned()
        } else {
            state.workspace_surface_type_id.text.trim().to_owned()
        },
        name: state.workspace_surface_name.text.trim().to_owned(),
        locator: Some(SurfaceLocator {
            kind: state.workspace_surface_locator_kind.text.trim().to_owned(),
            value: if state.workspace_surface_locator_value.text.trim().is_empty()
                || state
                    .workspace_surface_edit_index
                    .and_then(|index| state.workspace_surface_edits.get(index))
                    .is_some_and(|surface| {
                        surface
                            .definition
                            .as_ref()
                            .and_then(|item| item.locator.as_ref())
                            .is_some_and(|locator| {
                                locator.value == state.workspace_surface_locator_value.text.trim()
                            })
                            && !surface_value(surface).eq_ignore_ascii_case(value)
                    })
            {
                value.to_owned()
            } else {
                state.workspace_surface_locator_value.text.trim().to_owned()
            },
        }),
        scope: if scope == DraftScope::InScope {
            SurfaceScope::InScope
        } else {
            SurfaceScope::OutOfScope
        } as i32,
        scope_state: SurfaceScopeState::Approved as i32,
        description: state.workspace_surface_description.text.trim().to_owned(),
        out_of_scope_reason: if scope == DraftScope::OutOfScope {
            "Set in workspace".to_owned()
        } else {
            String::new()
        },
        evidence_refs: state
            .workspace_surface_edit_index
            .and_then(|index| state.workspace_surface_edits.get(index))
            .and_then(|surface| surface.definition.as_ref())
            .map_or_else(Vec::new, |item| item.evidence_refs.clone()),
    };
    if let Some(index) = state.workspace_surface_edit_index {
        let Some(surface) = state.workspace_surface_edits.get_mut(index) else {
            close_surface_dialog(state);
            return;
        };
        surface.definition = Some(definition);
    } else {
        let mut temporary_number = 1;
        while state
            .workspace_surface_edits
            .iter()
            .any(|surface| surface.id == format!("M{temporary_number}"))
        {
            temporary_number += 1;
        }
        let new_id = format!("M{temporary_number}");
        if let Some(parent_id) = state.workspace_surface_explorer.focus_path.last()
            && let Some(parent) = state
                .workspace_surface_edits
                .iter_mut()
                .find(|surface| &surface.id == parent_id)
        {
            parent.relationships.push(SurfaceRelationship {
                target_id: new_id.clone(),
                label: "contains".to_owned(),
                type_id: "contains".to_owned(),
                ..Default::default()
            });
        }
        state.workspace_surface_edits.push(WorkspaceSurface {
            id: new_id,
            definition: Some(definition),
            ..Default::default()
        });
    }
    close_surface_dialog(state);
}

pub(crate) fn surface_dialog(state: &State, scope: DraftScope, theme: ThemeStyle<'_>) -> [El; 2] {
    let editing = state.workspace_surface_edit_index.is_some();
    let kind = selection_button(
        state.workspace_surface_kind.label(),
        state.selection_menu == Some(SelectionMenu::WorkspaceSurfaceKind),
        |state| toggle_selection_menu(state, SelectionMenu::WorkspaceSurfaceKind),
        theme,
    )
    .key("saved-workspace-surface-kind-control");
    let mut scope_picker = row().gap(6.0);
    for (choice, label, key) in [
        (DraftScope::InScope, "In scope", "surface-scope-in"),
        (DraftScope::OutOfScope, "Out of scope", "surface-scope-out"),
    ] {
        scope_picker = scope_picker.child(
            form_button(label, theme)
                .key(key)
                .label(format!(
                    "{label}{}",
                    if scope == choice { ", selected" } else { "" }
                ))
                .fill(theme.color(if scope == choice {
                    "#303030"
                } else {
                    "#191919"
                }))
                .on_press(move |state: &mut State| {
                    state.workspace_surface_dialog_scope = Some(choice)
                }),
        );
    }
    let form = settings_form_card(
        [
            row_with_control(
                "Scope",
                "Recorded authorization boundary.",
                scope_picker,
                theme,
            ),
            row_with_control("Type", "Kind of target or source material.", kind, theme),
            row_with_control(
                "Surface",
                "Identifier, URL, or path.",
                field(
                    &state.workspace_surface_value,
                    "saved-workspace-surface-input",
                    "Enter surface",
                    theme,
                    |state| &mut state.workspace_surface_value,
                ),
                theme,
            ),
            row_with_control(
                "Specific type",
                "For example code.module or api.endpoint.",
                field(
                    &state.workspace_surface_type_id,
                    "saved-workspace-surface-type-id",
                    "repository",
                    theme,
                    |state| &mut state.workspace_surface_type_id,
                ),
                theme,
            ),
            row_with_control(
                "Name",
                "Short display name; defaults to the identifier.",
                field(
                    &state.workspace_surface_name,
                    "saved-workspace-surface-name",
                    "Display name",
                    theme,
                    |state| &mut state.workspace_surface_name,
                ),
                theme,
            ),
            row_with_control(
                "Locator kind",
                "freeform, git_url, repository_path, url, domain, or artifact.",
                field(
                    &state.workspace_surface_locator_kind,
                    "saved-workspace-surface-locator-kind",
                    "freeform",
                    theme,
                    |state| &mut state.workspace_surface_locator_kind,
                ),
                theme,
            ),
            row_with_control(
                "Locator",
                "Typed address or path; defaults to the identifier.",
                field(
                    &state.workspace_surface_locator_value,
                    "saved-workspace-surface-locator",
                    "Locator value",
                    theme,
                    |state| &mut state.workspace_surface_locator_value,
                ),
                theme,
            ),
            row_with_control(
                "Description",
                "What this surface is and why it matters to the research.",
                field(
                    &state.workspace_surface_description,
                    "saved-workspace-surface-description",
                    "Description",
                    theme,
                    |state| &mut state.workspace_surface_description,
                ),
                theme,
            ),
        ],
        theme,
    );
    let mut body = col()
        .key("saved-workspace-surface-dialog-body")
        .w_full()
        .max_h(480.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .gap(10.0)
        .child(form);
    if !state.workspace_surface_error.is_empty() {
        body = body.child(
            text(state.workspace_surface_error.clone())
                .key("saved-workspace-surface-dialog-error")
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#e07676")),
        );
    }
    let actions = row()
        .w_full()
        .gap(8.0)
        .child(el().grow(1.0))
        .child(
            form_button("Cancel", theme)
                .key("saved-workspace-surface-dialog-cancel")
                .on_press(close_surface_dialog),
        )
        .child(
            form_button(
                if editing {
                    "Save Surface"
                } else {
                    "Add Surface"
                },
                theme,
            )
            .key(if editing {
                "saved-workspace-edit-surface"
            } else {
                "saved-workspace-add-surface"
            })
            .on_press(commit_surface),
        );
    modal_dialog::render(
        modal_dialog::DialogSpec {
            key: "saved-workspace-surface-dialog",
            title: match (editing, scope) {
                (true, DraftScope::InScope) => "Edit an In-Scope Surface",
                (true, DraftScope::OutOfScope) => "Edit an Out-of-Scope Surface",
                (false, DraftScope::InScope) => "Add an In-Scope Surface",
                (false, DraftScope::OutOfScope) => "Add an Out-of-Scope Surface",
            },
            width: 540.0,
            focus_return: "saved-workspace-add-in-scope-surface",
        },
        body,
        actions,
        theme,
        close_surface_dialog,
    )
}

fn close_surface_delete_dialog(state: &mut State) {
    state.workspace_surface_delete_index = None;
}

fn confirm_surface_delete(state: &mut State) {
    if let Some(index) = state.workspace_surface_delete_index.take()
        && index < state.workspace_surface_edits.len()
    {
        let removed = state.workspace_surface_edits.remove(index);
        for surface in &mut state.workspace_surface_edits {
            surface
                .relationships
                .retain(|relationship| relationship.target_id != removed.id);
        }
        crate::surface_canvas::close(state, &removed.id);
        crate::surface_canvas::sync(state);
        state.workspace_surface_connecting = None;
        state.workspace_save_error.clear();
        state.workspace_save_message.clear();
    }
}

pub(crate) fn surface_delete_dialog(state: &State, theme: ThemeStyle<'_>) -> [El; 2] {
    let surface = state
        .workspace_surface_delete_index
        .and_then(|index| state.workspace_surface_edits.get(index));
    let identity = surface.map_or_else(
        || "This surface".to_owned(),
        |surface| {
            format!(
                "{} · {}",
                draft_kind(surface_kind(surface)).label(),
                surface_value(surface)
            )
        },
    );
    let body = col()
        .w_full()
        .gap(10.0)
        .child(
            text(identity)
                .w_full()
                .fade_truncate()
                .font(ui_font(14.0, 500.0))
                .color(theme.color("#e2e2e2")),
        )
        .child(
            text("Remove this surface from the list? Save Changes to apply the deletion to the workspace.")
                .w_full()
                .wrap()
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#b8b8b8")),
        );
    let actions = row()
        .w_full()
        .gap(8.0)
        .child(el().grow(1.0))
        .child(
            form_button("Cancel", theme)
                .key("saved-workspace-surface-delete-cancel")
                .on_press(close_surface_delete_dialog),
        )
        .child(
            form_button("Remove Surface", theme)
                .key("saved-workspace-surface-delete-confirm")
                .fill(theme.color("#e07676").alpha(0.14))
                .hover(theme.color("#e07676").alpha(0.22))
                .on_press(confirm_surface_delete),
        );
    modal_dialog::render(
        modal_dialog::DialogSpec {
            key: "saved-workspace-surface-delete-dialog",
            title: "Remove Surface?",
            width: 480.0,
            focus_return: "workspace-tab-surfaces",
        },
        body,
        actions,
        theme,
        close_surface_delete_dialog,
    )
}

fn header_add_button(
    key: &'static str,
    label: &'static str,
    help: &'static str,
    width: f32,
    theme: ThemeStyle<'_>,
    action: fn(&mut State),
    enabled: bool,
) -> El {
    let button = row()
        .key(key)
        .role("button")
        .label(help)
        .help(help)
        .size(width, 36.0)
        .center()
        .gap(crate::design::TAB_ICON_GAP)
        .radius(12.0)
        .continuous_corners()
        .fill(theme.color("#0d0d0d"))
        .border(1.0, theme.color("#2a2a2a").alpha(0.65))
        .hover(theme.color("#303030"))
        .child(icon(&PLUS, crate::design::TAB_ICON_SIZE).color(theme.color("#bdbdbd")))
        .child(
            text(label)
                .font(ui_font(13.0, 550.0))
                .color(theme.color("#bdbdbd")),
        );
    let button = if enabled {
        button.on_press(action)
    } else {
        button
    };
    el().size(width, 40.0).self_start().center().child(button)
}

pub(crate) fn render(
    state: &State,
    theme: ThemeStyle<'_>,
    resource_height: f32,
    page_width: f32,
) -> El {
    let Some(id) = state.workspace_id else {
        return col();
    };
    let title = state.workspace_tab.label().to_owned();
    let surface_graph_open = graph_open(state);
    let visible_tabs: Vec<WorkspaceTab> = WorkspaceTab::ALL
        .into_iter()
        .filter(|tab| match tab {
            WorkspaceTab::Surfaces => {
                crate::extensions_view::surfaces_enabled(state, &id.to_string())
            }
            WorkspaceTab::Findings => {
                crate::extensions_view::findings_enabled(state, &id.to_string())
            }
            _ => true,
        })
        .collect();
    let labeled_width = labeled_tabs_width(&visible_tabs);
    let secondary_toggle_width = match state.workspace_tab {
        WorkspaceTab::Surfaces if surface_graph_open => surface_graph::view_toggle_width(true),
        WorkspaceTab::Findings => crate::findings_view::mode_toggle_width(true),
        WorkspaceTab::Rules => guidance_toggle_width(true),
        _ => 0.0,
    };
    let guidance_rules = state.workspace_tab == WorkspaceTab::Rules
        && state.workspace_guidance_view == GuidanceView::Rules;
    let header_gaps =
        if surface_graph_open || state.workspace_tab == WorkspaceTab::Findings || guidance_rules {
            48.0
        } else if secondary_toggle_width > 0.0 {
            36.0
        } else {
            24.0
        };
    let add_action_width = match state.workspace_tab {
        WorkspaceTab::Surfaces if surface_graph_open => {
            crate::design::segmented_tab_width("Add Surface", true)
        }
        WorkspaceTab::Findings => crate::design::segmented_tab_width("Add Finding", true),
        WorkspaceTab::Rules if guidance_rules => {
            crate::design::segmented_tab_width("Add Rule", true)
        }
        _ => 0.0,
    };
    let labeled =
        page_width - labeled_width - secondary_toggle_width - add_action_width - header_gaps
            >= TITLE_MIN_WIDTH_BESIDE_LABELS;
    let tabs_width = if labeled {
        labeled_width
    } else {
        crate::design::segmented_group_width(&vec![
            crate::design::CONTROL_HEIGHT;
            visible_tabs.len()
        ])
    };
    // The title yields space first; the tab group keeps one line at its full width.
    let mut tabs = crate::design::segmented_control(theme)
        .key("workspace-view-tabs")
        .label("Workspace views")
        .min_w(tabs_width);
    for tab in visible_tabs {
        tabs = tabs.child(tab_button(
            tab,
            state.workspace_tab == tab,
            labeled,
            id,
            theme,
        ));
    }
    let mut header = row()
        .key("workspace-view-header")
        .w_full()
        .center_y()
        .gap(12.0)
        .child(
            row()
                .key("workspace-title-squircle")
                .shrink()
                .help(&title)
                .ml(-18.0)
                .min_w(0.0)
                .max_w(420.0)
                .h(56.0)
                .pl(18.0)
                .pr(10.0)
                .center_y()
                .child(
                    text(title)
                        .key("workspace-view-title")
                        .offset(0.0, -8.0)
                        .min_w(0.0)
                        .fade_truncate()
                        .font(view_title_font())
                        .color(theme.color("#f2f2f2")),
                ),
        );
    if surface_graph_open {
        header = header.child(surface_graph::view_toggle(state, labeled, theme));
        let help = if state.workspace_surface_explorer.focus_path.is_empty() {
            "Add Surface"
        } else {
            "Add Child Surface"
        };
        header = header.child(header_add_button(
            "saved-workspace-add-in-scope-surface",
            "Add Surface",
            help,
            add_action_width,
            theme,
            |state| open_surface_editor(state, None, DraftScope::InScope),
            state.workspace_save_busy.is_none(),
        ));
    } else if state.workspace_tab == WorkspaceTab::Findings {
        header = header.child(crate::findings_view::mode_toggle(state, labeled, theme));
        header = header.child(header_add_button(
            "workspace-add-finding",
            "Add Finding",
            "Add Finding",
            add_action_width,
            theme,
            crate::findings_view::open_create,
            true,
        ));
    } else if state.workspace_tab == WorkspaceTab::Rules {
        header = header.child(guidance_toggle(state, labeled, theme));
        if state.workspace_guidance_view == GuidanceView::Rules {
            header = header.child(header_add_button(
                "saved-workspace-add-rule",
                "Add Rule",
                "Add Rule",
                add_action_width,
                theme,
                open_rule_dialog,
                state.workspace_save_busy.is_none(),
            ));
        }
    }
    let header = header.child(el().grow(1.0)).child(tabs);
    if surface_graph_open {
        return network(state, theme, page_width, resource_height, header);
    }
    let mut page = col()
        .key("workspace-view-page")
        .w_full()
        .gap(14.0)
        .child(header);
    if state.workspace_tab == WorkspaceTab::Campaign {
        return page.child(
            el().key("workspace-campaign-empty")
                .w_full()
                .min_h(resource_height)
                .center()
                .child(
                    crate::design::empty_state(&TARGET, "Keep your research connected", "Open Surfaces or Findings above, or a session runbook in Canvas, to review this workspace's research. Enable additional capabilities in Extensions.", theme),
                ),
        );
    }
    if state.workspace_tab == WorkspaceTab::Findings {
        return page.child(crate::findings_view::render(
            state,
            theme,
            page_width,
            resource_height,
        ));
    }
    let details = state.workspace_details.as_ref().filter(|details| {
        details
            .summary
            .as_ref()
            .is_some_and(|summary| summary.id == id.to_string())
    });
    if let Some(details) = details {
        let forms = match state.workspace_tab {
            WorkspaceTab::Surfaces => Vec::new(),
            WorkspaceTab::Findings => Vec::new(),
            WorkspaceTab::Rules => match state.workspace_guidance_view {
                GuidanceView::Agents => vec![agents_guidance(state, theme)],
                GuidanceView::Rules => vec![rules(state, theme)],
            },
            WorkspaceTab::Foundation => vec![foundation(state, details, theme)],
            WorkspaceTab::Campaign => Vec::new(),
        };
        for form in forms {
            page = page.child(form);
        }
        let dirty = match state.workspace_tab {
            WorkspaceTab::Foundation => details
                .summary
                .as_ref()
                .is_some_and(|summary| state.workspace_name_edit.text.trim() != summary.name),
            WorkspaceTab::Surfaces => state.workspace_surface_edits != details.surfaces,
            WorkspaceTab::Findings => false,
            WorkspaceTab::Rules => match state.workspace_guidance_view {
                GuidanceView::Agents => {
                    state.workspace_guidance_edit.text.trim_end() != details.guidance.trim_end()
                }
                GuidanceView::Rules => state.workspace_rule_edits != details.rules,
            },
            WorkspaceTab::Campaign => false,
        };
        let save_tab = if state.workspace_tab == WorkspaceTab::Rules
            && state.workspace_guidance_view == GuidanceView::Agents
        {
            WorkspaceTab::Foundation
        } else {
            state.workspace_tab
        };
        let saving = state.workspace_save_busy == Some(save_tab);
        let save = form_button(if saving { "Saving…" } else { "Save Changes" }, theme).key(
            if dirty && !saving && !state.workspace_details_loading {
                "saved-workspace-save"
            } else {
                "saved-workspace-save-disabled"
            },
        );
        if state.workspace_tab != WorkspaceTab::Surfaces {
            page = page.child(row().w_full().child(el().grow(1.0)).child(
                if dirty && !saving && !state.workspace_details_loading {
                    save.on_press(move |state: &mut State| {
                        crate::start_workspace_save(state, save_tab)
                    })
                } else {
                    save
                },
            ));
        }
        if !state.workspace_save_error.is_empty() {
            page = page.child(
                text(state.workspace_save_error.clone())
                    .key("saved-workspace-save-error")
                    .font(ui_font(14.0, 400.0))
                    .color(theme.color("#e07676")),
            );
            if state.workspace_save_error == crate::workspaces::WORKSPACE_CONFLICT_MESSAGE {
                page = page.child(
                    row().w_full().child(el().grow(1.0)).child(
                        form_button("Reload Workspace", theme)
                            .key("saved-workspace-reload")
                            .on_press(move |state: &mut State| {
                                crate::start_workspace_details(state, id);
                            }),
                    ),
                );
            }
        } else if !state.workspace_save_message.is_empty() {
            page = page.child(
                text(state.workspace_save_message.clone())
                    .key("saved-workspace-save-message")
                    .font(ui_font(14.0, 400.0))
                    .color(theme.color("#969696")),
            );
        }
        if !state.workspace_details_error.is_empty() {
            page = page.child(
                text(state.workspace_details_error.clone())
                    .key("workspace-details-status")
                    .font(ui_font(14.0, 400.0))
                    .color(theme.color("#e07676")),
            );
        }
    } else {
        let message = if state.workspace_details_loading {
            "Loading workspace details…".to_owned()
        } else if state.workspace_details_error.is_empty() {
            "Workspace details are unavailable.".to_owned()
        } else {
            state.workspace_details_error.clone()
        };
        page = page.child(
            text(message)
                .key("workspace-details-status")
                .font(ui_font(14.0, 400.0))
                .color(theme.color(if state.workspace_details_error.is_empty() {
                    "#969696"
                } else {
                    "#e07676"
                })),
        );
    }
    page
}

pub(crate) fn graph_open(state: &State) -> bool {
    state.primary_view == crate::PrimaryView::Workspace
        && state.workspace_tab == WorkspaceTab::Surfaces
        && state
            .workspace_id
            .is_some_and(|id| crate::extensions_view::surfaces_enabled(state, &id.to_string()))
        && state.workspace_id.is_some_and(|id| {
            state.workspace_details.as_ref().is_some_and(|details| {
                details
                    .summary
                    .as_ref()
                    .is_some_and(|summary| summary.id == id.to_string())
            })
        })
}

#[cfg(test)]
mod scope_state_tests {
    use super::*;

    #[test]
    fn operator_approval_updates_candidate_surface() {
        let mut state = State {
            workspace_surface_edits: vec![WorkspaceSurface {
                definition: Some(SurfaceDefinition {
                    scope_state: SurfaceScopeState::Candidate as i32,
                    ..Default::default()
                }),
                ..Default::default()
            }],
            ..State::default()
        };
        approve_surface_scope(&mut state, 0);
        assert_eq!(
            state.workspace_surface_edits[0]
                .definition
                .as_ref()
                .unwrap()
                .scope_state,
            SurfaceScopeState::Approved as i32
        );
    }
}
