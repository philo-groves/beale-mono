use beale_contracts::v1::{
    SurfaceDefinition, SurfaceKind, SurfaceLocator, SurfaceRelationship, SurfaceScope,
    SurfaceScopeState, WorkspaceDetails, WorkspaceSurface,
};
use rune::{
    Color, Drag, El, Icon, LineCap, Phase, ScrollSpec, TextEdit, Tx, canvas, col, el, icon, row,
    text, text_input,
};

use crate::{
    FormSpec, SelectionMenu, State, ThemeStyle, ViewLocation, form_button,
    form_controls::icon_action_button,
    form_label_font,
    icons::{PEN, PLUS, SAFETY, SETTINGS, TARGET, TRASH, WORKSPACE_KNOWLEDGE, WORKSPACE_SURFACES},
    modal_dialog, selection_button, settings_form, settings_form_card, settings_form_with_action,
    toggle_selection_menu, ui_font, view_title_font,
    workspace_creation::{
        SurfaceKind as DraftKind, SurfaceScope as DraftScope, display_directory, field,
        row_with_control, value_with_actions,
    },
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum WorkspaceTab {
    Campaign,
    Knowledge,
    Foundation,
    Surfaces,
    #[default]
    Rules,
}

impl WorkspaceTab {
    const ALL: [Self; 4] = [
        Self::Knowledge,
        Self::Surfaces,
        Self::Rules,
        Self::Foundation,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Campaign => "Campaign",
            Self::Knowledge => "Knowledge",
            Self::Surfaces => "Surfaces",
            Self::Rules => "Rules",
            Self::Foundation => "Settings",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Campaign => "workspace-tab-campaign",
            Self::Knowledge => "workspace-tab-knowledge",
            Self::Surfaces => "workspace-tab-surfaces",
            Self::Rules => "workspace-tab-rules",
            Self::Foundation => "workspace-tab-settings",
        }
    }
}

fn tab_button(tab: WorkspaceTab, selected: bool, id: uuid::Uuid, theme: ThemeStyle<'_>) -> El {
    let color = |value| theme.color(value);
    let tab_icon = match tab {
        WorkspaceTab::Surfaces => &WORKSPACE_SURFACES,
        WorkspaceTab::Knowledge => &WORKSPACE_KNOWLEDGE,
        WorkspaceTab::Campaign => &TARGET,
        WorkspaceTab::Foundation => &SETTINGS,
        WorkspaceTab::Rules => &SAFETY,
    };
    el().key(tab.key())
        .role("tab")
        .label(tab.label())
        .size(32.0, 32.0)
        .center()
        .radius(10.0)
        .continuous_corners()
        .fill(if selected {
            color("#333333")
        } else {
            Color::TRANSPARENT
        })
        .hover(color("#303030"))
        .child(icon(tab_icon, 17.0).color(color("#e2e2e2")))
        .on_press_tx(move |state: &mut State, tx| {
            state.workspace_surface_management_open = false;
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
                .truncate()
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

fn surface_value(surface: &WorkspaceSurface) -> &str {
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

fn surface_creator_label(surface: &WorkspaceSurface) -> &'static str {
    if surface
        .lineage
        .first()
        .is_some_and(|event| matches!(event.actor.as_str(), "agent" | "ai_import"))
    {
        "Agent"
    } else {
        "User"
    }
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
    let guidance = if busy {
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
    settings_form(
        FormSpec {
            key: "saved-workspace-details-form",
            heading: "Basic Information",
        },
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
            col()
                .key("saved-workspace-guidance-row")
                .w_full()
                .py(12.0)
                .gap(8.0)
                .child(
                    col()
                        .gap(4.0)
                        .child(
                            text("Workspace Guidance")
                                .font(form_label_font())
                                .color(theme.color("#e2e2e2")),
                        )
                        .child(
                            text("AGENTS.md instructions for research in this workspace.")
                                .font(ui_font(14.0, 400.0))
                                .color(theme.color("#969696")),
                        ),
                )
                .child(guidance),
        ],
        theme,
    )
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

fn arc(cx: f32, cy: f32, radius: f32, start: f32, end: f32) -> Vec<[f32; 2]> {
    (0..=40)
        .map(|step| {
            let angle = (start + (end - start) * step as f32 / 40.0).to_radians();
            [cx + radius * angle.cos(), cy + radius * angle.sin()]
        })
        .collect()
}

fn surface_network_background_drag(state: &mut State, drag: &mut Drag, tx: &mut Tx) {
    tx.instant();
    match drag.phase {
        Phase::Down => state.workspace_surface_pan_origin = state.workspace_surface_pan,
        Phase::Move => {
            state.workspace_surface_pan = [
                state.workspace_surface_pan_origin[0] + drag.dx,
                state.workspace_surface_pan_origin[1] + drag.dy,
            ];
        }
        Phase::Up if drag.cancelled() => {
            state.workspace_surface_pan = state.workspace_surface_pan_origin;
        }
        Phase::Up if drag.dx.abs() <= 4.0 && drag.dy.abs() <= 4.0 => {
            state.workspace_surface_selected = None;
            state.workspace_surface_connecting = None;
        }
        Phase::Up => {}
    }
}

fn network(
    state: &State,
    theme: ThemeStyle<'_>,
    available_width: f32,
    available_height: f32,
    header: El,
) -> El {
    let surfaces = &state.workspace_surface_edits;
    let width = available_width.max(1.0);
    let height = available_height.max(1.0);
    let positions = crate::workspace_surface_layout::layout(surfaces, width, height);
    let position = |index: usize| -> [f32; 2] {
        [
            positions[index][0] + state.workspace_surface_pan[0],
            positions[index][1] + state.workspace_surface_pan[1],
        ]
    };
    let hovered = state.workspace_surface_hovered;
    let mut shape = crate::grid_background::dots(width, height, theme.color("#303030"));
    for (index, surface) in surfaces.iter().enumerate() {
        for relationship in &surface.relationships {
            let Some(target) = surfaces
                .iter()
                .position(|candidate| candidate.id == relationship.target_id)
            else {
                continue;
            };
            let from = position(index);
            let to = position(target);
            let active = hovered == Some(index) || hovered == Some(target);
            shape = shape.line(
                from,
                to,
                if active { 1.8 } else { 1.2 },
                LineCap::Round,
                theme.color(if active { "#929292" } else { "#3a3a3a" }),
            );
        }
    }
    let max_updates = surfaces
        .iter()
        .map(|surface| surface_revision(surface).saturating_sub(1))
        .max()
        .unwrap_or(0);
    for (index, surface) in surfaces.iter().enumerate() {
        let [x, y] = position(index);
        let intensity = if max_updates == 0 {
            0.0
        } else {
            surface_revision(surface).saturating_sub(1) as f32 / max_updates as f32
        };
        let channel = |grey: f32, red: f32| (grey + (red - grey) * intensity).round() as u8;
        let outline = format!(
            "#{:02x}{:02x}{:02x}",
            channel(85.0, 220.0),
            channel(85.0, 65.0),
            channel(85.0, 65.0)
        );
        shape = shape.stroke(arc(x, y, 18.0, -90.0, 270.0), 3.0, theme.color(&outline));
    }
    let mut stage = el()
        .key("workspace-surface-network")
        .size(width, height)
        .child(canvas(shape).absolute(0.0, 0.0).size(width, height));
    let panel_width = (width - 48.0).min(460.0);
    let panel_bottom = if state.workspace_surface_selected.is_some() {
        (96.0
            + if state.workspace_surface_detail_height > 0.0 {
                state.workspace_surface_detail_height
            } else {
                450.0
            })
        .min(height)
    } else {
        0.0
    };
    let hit_regions = if state.workspace_surface_selected.is_some() {
        [
            [0.0, 0.0, width, 96.0],
            [0.0, 96.0, 24.0, panel_bottom - 96.0],
            [
                24.0 + panel_width,
                96.0,
                width - 24.0 - panel_width,
                panel_bottom - 96.0,
            ],
            [0.0, panel_bottom, width, height - panel_bottom],
        ]
    } else {
        [[0.0, 0.0, width, height], [0.0; 4], [0.0; 4], [0.0; 4]]
    };
    for (index, [x, y, region_width, region_height]) in hit_regions.into_iter().enumerate() {
        if region_width > 0.0 && region_height > 0.0 {
            stage = stage.child(
                el().key(if state.workspace_surface_selected.is_some() {
                    format!("workspace-surface-background-{index}")
                } else {
                    "workspace-surface-background-full".to_owned()
                })
                .absolute(x, y)
                .size(region_width, region_height)
                .on_drag(surface_network_background_drag),
            );
        }
    }
    for (index, surface) in surfaces.iter().enumerate() {
        let [x, y] = position(index);
        let out = surface_scope(surface) == SurfaceScope::OutOfScope as i32;
        let name = surface
            .definition
            .as_ref()
            .map(|definition| definition.name.trim())
            .filter(|name| !name.is_empty())
            .unwrap_or(surface_value(surface));
        let label = format!(
            "{} · {} · {}{} · {} · {} updates",
            name,
            draft_kind(surface_kind(surface)).label(),
            surface_value(surface),
            if out { " · Out of scope" } else { "" },
            scope_state_label(surface_scope_state(surface)),
            surface_revision(surface).saturating_sub(1),
        );
        let node = el()
            .key(format!("workspace-surface-node-{index}"))
            .role("button")
            .label(label.clone())
            .absolute(x - 9.0, y - 9.0)
            .size(18.0, 18.0)
            .center()
            .child(
                icon(surface_icon(surface_kind(surface)), 18.0).color(theme.color(if out {
                    "#777777"
                } else {
                    "#e0e0e0"
                })),
            )
            .on_hover_change(move |state: &mut State, inside, _| {
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
            });
        stage = stage.child(node);
        stage = stage.child(
            el().key(format!("workspace-surface-visible-name-{index}"))
                .absolute(
                    x - crate::workspace_surface_layout::NODE_LABEL_WIDTH / 2.0,
                    y + 25.0,
                )
                .w(crate::workspace_surface_layout::NODE_LABEL_WIDTH)
                .center()
                .child(
                    text(name.to_owned())
                        .truncate()
                        .font(ui_font(12.0, 500.0))
                        .color(theme.color(if out { "#888888" } else { "#c9c9c9" })),
                ),
        );
        if hovered == Some(index) {
            stage = stage.child(
                el().key(format!("workspace-surface-name-{index}"))
                    .absolute((x - 110.0).max(4.0).min((width - 224.0).max(4.0)), y + 48.0)
                    .w(220.0)
                    .px(8.0)
                    .py(5.0)
                    .radius(7.0)
                    .fill(theme.color("#303030"))
                    .child(
                        text(label)
                            .truncate()
                            .font(ui_font(12.0, 500.0))
                            .color(theme.color("#e2e2e2")),
                    ),
            );
        }
    }
    if let Some(index) = hovered {
        if let Some(surface) = surfaces.get(index) {
            for relationship in &surface.relationships {
                if let Some(target) = surfaces
                    .iter()
                    .position(|candidate| candidate.id == relationship.target_id)
                {
                    let from = position(index);
                    let to = position(target);
                    let x = ((from[0] + to[0]) / 2.0 - 70.0).max(4.0);
                    let y = (from[1] + to[1]) / 2.0 - 12.0;
                    stage = stage.child(
                        el().key(format!("workspace-surface-relationship-{index}-{target}"))
                            .absolute(x, y)
                            .w(140.0)
                            .px(6.0)
                            .py(4.0)
                            .radius(6.0)
                            .fill(theme.color("#343434"))
                            .child(
                                text(relationship.label.clone())
                                    .truncate()
                                    .font(ui_font(11.0, 500.0))
                                    .color(theme.color("#dddddd")),
                            ),
                    );
                }
            }
        }
        for (source, surface) in surfaces.iter().enumerate() {
            if source == index {
                continue;
            }
            for relationship in &surface.relationships {
                if relationship.target_id == surfaces[index].id {
                    let from = position(source);
                    let to = position(index);
                    stage = stage.child(
                        el().key(format!("workspace-surface-relationship-{source}-{index}"))
                            .absolute(
                                ((from[0] + to[0]) / 2.0 - 70.0).max(4.0),
                                (from[1] + to[1]) / 2.0 - 12.0,
                            )
                            .w(140.0)
                            .px(6.0)
                            .py(4.0)
                            .radius(6.0)
                            .fill(theme.color("#343434"))
                            .child(
                                text(relationship.label.clone())
                                    .truncate()
                                    .font(ui_font(11.0, 500.0))
                                    .color(theme.color("#dddddd")),
                            ),
                    );
                }
            }
        }
    }
    if state.workspace_surface_selected.is_some() {
        stage = stage.child(
            surface_network_details(state, theme)
                .absolute(24.0, 96.0)
                .w((width - 48.0).min(460.0))
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
    let dirty = state
        .workspace_details
        .as_ref()
        .is_some_and(|details| surfaces.as_slice() != details.surfaces.as_slice());
    let saving = state.workspace_save_busy == Some(WorkspaceTab::Surfaces);
    let mut actions = row()
        .key("workspace-surface-actions")
        .absolute(24.0, height - 50.0)
        .w((width - 48.0).max(0.0))
        .center_y()
        .gap(8.0)
        .child(el().grow(1.0));
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
        .child(header.absolute(24.0, 25.0).w((width - 48.0).max(0.0)));
    if !state.workspace_save_error.is_empty() {
        let mut status = col().key("saved-workspace-save-error").gap(8.0).child(
            text(state.workspace_save_error.clone())
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#e07676")),
        );
        if state.workspace_save_error == crate::workspaces::WORKSPACE_CONFLICT_MESSAGE {
            status = status.child(
                form_button("Reload Workspace", theme)
                    .key("saved-workspace-reload")
                    .on_press(move |state: &mut State| {
                        if let Some(id) = state.workspace_id {
                            crate::start_workspace_details(state, id);
                        }
                    }),
            );
        }
        page = page.child(
            status
                .absolute(24.0, height - 110.0)
                .w((width - 48.0).min(460.0)),
        );
    } else if !state.workspace_save_message.is_empty() {
        page = page.child(
            text(state.workspace_save_message.clone())
                .key("saved-workspace-save-message")
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#969696"))
                .absolute(24.0, height - 60.0),
        );
    }
    page.child(actions)
}

fn surface_detail(label: &str, value: &str, theme: ThemeStyle<'_>) -> El {
    col()
        .w_full()
        .gap(2.0)
        .child(
            text(label.to_owned())
                .font(ui_font(11.0, 500.0))
                .color(theme.color("#999999")),
        )
        .child(
            text(value.to_owned())
                .w_full()
                .wrap()
                .font(ui_font(12.0, 400.0))
                .color(theme.color("#dddddd")),
        )
}

fn surface_network_details(state: &State, theme: ThemeStyle<'_>) -> El {
    let Some(index) = state.workspace_surface_selected else {
        return col();
    };
    let Some(surface) = state.workspace_surface_edits.get(index) else {
        return col();
    };
    let out = surface_scope(surface) == SurfaceScope::OutOfScope as i32;
    let mut details =
        col()
            .key("workspace-surface-network-details")
            .w_full()
            .max_h(450.0)
            .min_h(0.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .gap(10.0)
            .padding(12.0)
            .radius(10.0)
            .fill(theme.color("#242424"))
            .child(
                row()
                    .w_full()
                    .center_y()
                    .gap(8.0)
                    .child(
                        icon(surface_icon(surface_kind(surface)), 17.0)
                            .color(theme.color(if out { "#777777" } else { "#e0e0e0" })),
                    )
                    .child(
                        text(surface_value(surface).to_owned())
                            .grow(1.0)
                            .min_w(0.0)
                            .truncate()
                            .font(ui_font(13.0, 600.0))
                            .color(theme.color("#e4e4e4")),
                    )
                    .child(
                        text(format!(
                            "{} · {}",
                            if out { "Out of scope" } else { "In scope" },
                            scope_state_label(surface_scope_state(surface))
                        ))
                        .font(ui_font(11.0, 400.0))
                        .color(theme.color("#999999")),
                    ),
            );
    details = details.child(surface_detail("ID", &surface.id, theme));
    details = details.child(surface_detail(
        "Updates",
        &surface_revision(surface).saturating_sub(1).to_string(),
        theme,
    ));
    let updated = chrono::DateTime::from_timestamp_millis(surface_updated_at(surface) as i64)
        .map_or_else(
            || "Unknown (predates tracking)".to_owned(),
            |time| time.format("%Y-%m-%d %H:%M UTC").to_string(),
        );
    details = details.child(surface_detail("Last updated", &updated, theme));
    for event in surface.lineage.iter().rev().take(5) {
        let fields = event
            .changes
            .iter()
            .map(|change| change.field.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        details = details.child(surface_detail(
            &format!("Revision {} · {}", event.number, event.actor),
            &format!("{} · {}", event.action, fields),
            theme,
        ));
    }
    if let Some(definition) = &surface.definition {
        details = details.child(surface_detail("Specific type", &definition.type_id, theme));
        if !definition.name.is_empty() && definition.name != surface_value(surface) {
            details = details.child(surface_detail("Name", &definition.name, theme));
        }
        if let Some(locator) = &definition.locator {
            details = details.child(surface_detail(
                "Locator",
                &format!("{}: {}", locator.kind, locator.value),
                theme,
            ));
        }
        if !definition.description.is_empty() {
            details = details.child(surface_detail(
                "Description",
                &definition.description,
                theme,
            ));
        }
        if !definition.out_of_scope_reason.is_empty() {
            details = details.child(surface_detail(
                "Exclusion reason",
                &definition.out_of_scope_reason,
                theme,
            ));
        }
        for evidence in &definition.evidence_refs {
            details = details.child(surface_detail(
                "Evidence",
                &format!(
                    "{}: {}{}",
                    evidence.kind,
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
    }
    if surface_scope_state(surface) != SurfaceScopeState::Approved as i32 {
        details = details.child(
            form_button("Approve Scope", theme)
                .key("workspace-surface-approve-scope")
                .on_press(move |state: &mut State| approve_surface_scope(state, index)),
        );
    }
    if state.workspace_surface_edits.len() > 1 {
        let connecting = state.workspace_surface_connecting == Some(index);
        details = details.child(
            row()
                .w_full()
                .center_y()
                .gap(10.0)
                .child(
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
                        state.workspace_surface_connecting =
                            if connecting { None } else { Some(index) };
                    }),
                )
                .child(
                    text(if connecting {
                        "Select the other surface in the network."
                    } else {
                        ""
                    })
                    .font(ui_font(12.0, 400.0))
                    .color(theme.color("#999999")),
                ),
        );
    }
    for (link_index, relationship) in surface.relationships.iter().enumerate() {
        let target = state
            .workspace_surface_edits
            .iter()
            .find(|candidate| candidate.id == relationship.target_id)
            .map_or("Unknown surface", surface_value);
        details = details.child(
            row()
                .w_full()
                .center_y()
                .gap(8.0)
                .child(
                    text(format!(
                        "{}{} → {}",
                        relationship.label,
                        if relationship.type_id.is_empty() {
                            String::new()
                        } else {
                            format!(" [{}]", relationship.type_id)
                        },
                        target
                    ))
                    .grow(1.0)
                    .min_w(0.0)
                    .truncate()
                    .font(ui_font(12.0, 400.0))
                    .color(theme.color("#b8b8b8")),
                )
                .child(
                    text(format!("{} refs", relationship.evidence_refs.len()))
                        .font(ui_font(10.0, 400.0))
                        .color(theme.color("#888888")),
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
    let type_id = state.workspace_surface_relationship_type_id.text.trim();
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
                        "e.g. contains, calls, depends on",
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

fn surfaces(state: &State, theme: ThemeStyle<'_>) -> Vec<El> {
    let mut forms = vec![
        row()
            .key("workspace-surface-management-header")
            .w_full()
            .center_y()
            .child(
                text("Manage Surfaces")
                    .font(ui_font(16.0, 600.0))
                    .color(theme.color("#ededed")),
            )
            .child(el().grow(1.0))
            .child(
                form_button("Back to Graph", theme)
                    .key("workspace-surface-management-back")
                    .on_press(|state: &mut State| state.workspace_surface_management_open = false),
            ),
    ];
    forms.extend(
        [SurfaceScope::InScope, SurfaceScope::OutOfScope]
            .into_iter()
            .map(|scope| {
                let mut list = col().w_full().py(12.0).gap(8.0);
                let mut count = 0;
                for (index, surface) in state.workspace_surface_edits.iter().enumerate() {
                    if surface_scope(surface) != scope as i32 {
                        continue;
                    }
                    count += 1;
                    let kind = match SurfaceKind::try_from(surface_kind(surface)) {
                        Ok(SurfaceKind::Repository) => "Repository",
                        Ok(SurfaceKind::Documentation) => "Documentation",
                        Ok(SurfaceKind::Binary) => "Binary",
                        Ok(SurfaceKind::Service) => "Service",
                        Ok(SurfaceKind::Domain) => "Domain",
                        _ => "Other",
                    };
                    let creator = surface_creator_label(surface);
                    let mut label = format!(
                        "{} · {kind} · {} · {creator} · {}",
                        surface.id,
                        surface_value(surface),
                        scope_state_label(surface_scope_state(surface))
                    );
                    if let Some(definition) = &surface.definition
                        && !definition.type_id.is_empty()
                    {
                        label.push_str(&format!(" · {}", definition.type_id));
                    }
                    if let Some(definition) = &surface.definition
                        && !definition.out_of_scope_reason.is_empty()
                    {
                        label.push_str(&format!(" · {}", definition.out_of_scope_reason));
                    }
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
                    let edit = if state.workspace_save_busy.is_some() {
                        edit
                    } else {
                        edit.on_press(move |state: &mut State| {
                            let surface = &state.workspace_surface_edits[index];
                            let scope = if surface_scope(surface) == SurfaceScope::InScope as i32 {
                                DraftScope::InScope
                            } else {
                                DraftScope::OutOfScope
                            };
                            open_surface_editor(state, Some(index), scope);
                        })
                    };
                    list = list.child(
                        row()
                            .key(format!("saved-workspace-surface-{index}"))
                            .w_full()
                            .center_y()
                            .gap(8.0)
                            .child(
                                text(label)
                                    .grow(1.0)
                                    .min_w(0.0)
                                    .wrap()
                                    .font(ui_font(14.0, 400.0))
                                    .color(theme.color("#e2e2e2")),
                            )
                            .child(edit)
                            .child(
                                if surface_scope_state(surface)
                                    != SurfaceScopeState::Approved as i32
                                {
                                    form_button("Approve", theme)
                                        .key(format!("saved-workspace-surface-approve-{index}"))
                                        .on_press(move |state: &mut State| {
                                            approve_surface_scope(state, index)
                                        })
                                } else {
                                    el()
                                },
                            )
                            .child(if state.workspace_save_busy.is_some() {
                                remove
                            } else {
                                remove.on_press(move |state: &mut State| {
                                    state.workspace_surface_delete_index = Some(index);
                                })
                            }),
                    );
                }
                if count == 0 {
                    list = list.child(
                        text("No surfaces listed.")
                            .font(ui_font(14.0, 400.0))
                            .color(theme.color("#969696")),
                    );
                }
                let draft_scope = if scope == SurfaceScope::InScope {
                    DraftScope::InScope
                } else {
                    DraftScope::OutOfScope
                };
                let add_button = el()
                    .key(if scope == SurfaceScope::InScope {
                        "saved-workspace-add-in-scope-surface"
                    } else {
                        "saved-workspace-add-out-of-scope-surface"
                    })
                    .role("button")
                    .label("Add a surface")
                    .size(26.0, 26.0)
                    .center()
                    .radius(8.0)
                    .continuous_corners()
                    .hover(theme.color("#303030"))
                    .child(icon(&PLUS, 16.0).color(theme.color("#bdbdbd")));
                let add_button = if state.workspace_save_busy.is_some() {
                    add_button
                } else {
                    add_button.on_press(move |state: &mut State| {
                        open_surface_editor(state, None, draft_scope);
                    })
                };
                settings_form_with_action(
                    FormSpec {
                        key: if scope == SurfaceScope::InScope {
                            "saved-workspace-in-scope-surfaces"
                        } else {
                            "saved-workspace-out-of-scope-surfaces"
                        },
                        heading: if scope == SurfaceScope::InScope {
                            "In-Scope Surfaces"
                        } else {
                            "Out-of-Scope Surfaces"
                        },
                    },
                    [list],
                    Some(add_button),
                    theme,
                )
            }),
    );
    forms
}

fn rules(state: &State, theme: ThemeStyle<'_>) -> Vec<El> {
    let add = icon_action_button(
        "saved-workspace-add-rule".to_owned(),
        "Add a rule",
        &PLUS,
        theme,
    );
    let add = if state.workspace_save_busy.is_some() {
        add
    } else {
        add.on_press(|state: &mut State| {
            state.workspace_rule_input.clear();
            state.workspace_rule_dialog_error.clear();
            state.workspace_rule_dialog_open = true;
        })
    };
    let mut list = col().w_full().py(12.0).gap(8.0);
    if state.workspace_rule_edits.is_empty() {
        list = list.child(
            text("No workspace rules recorded.")
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
    vec![settings_form_with_action(
        FormSpec {
            key: "saved-workspace-rules-form",
            heading: "Workspace Rules",
        },
        [list],
        Some(add),
        theme,
    )]
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
        state.workspace_surface_edits.push(WorkspaceSurface {
            id: format!("M{temporary_number}"),
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
    let form = settings_form_card(
        [
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
            focus_return: if scope == DraftScope::InScope {
                "saved-workspace-add-in-scope-surface"
            } else {
                "saved-workspace-add-out-of-scope-surface"
            },
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
        state.workspace_surface_selected = None;
        state.workspace_surface_hovered = None;
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
                .truncate()
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
                .fill(theme.color("#482525"))
                .hover(theme.color("#603030"))
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

pub(crate) fn render(
    state: &State,
    theme: ThemeStyle<'_>,
    resource_height: f32,
    page_width: f32,
) -> El {
    let Some(id) = state.workspace_id else {
        return col();
    };
    let name = state
        .workspaces
        .iter()
        .find(|workspace| workspace.id == id.to_string())
        .map_or("Workspace", |workspace| workspace.name.as_str());
    let mut tabs = row()
        .key("workspace-view-tabs")
        .role("tablist")
        .label("Workspace views")
        .h(42.0)
        .px(5.0)
        .gap(3.0)
        .center_y()
        .radius(14.0)
        .continuous_corners()
        .fill(theme.color("#0d0d0d"));
    for tab in WorkspaceTab::ALL {
        if tab == WorkspaceTab::Surfaces
            && !crate::extensions_view::surfaces_enabled(state, &id.to_string())
        {
            continue;
        }
        if tab == WorkspaceTab::Knowledge
            && !crate::extensions_view::knowledge_enabled(state, &id.to_string())
        {
            continue;
        }
        tabs = tabs.child(tab_button(tab, state.workspace_tab == tab, id, theme));
    }
    let header = row()
        .key("workspace-view-header")
        .w_full()
        .center_y()
        .gap(12.0)
        .child(
            row()
                .key("workspace-title-squircle")
                .ml(-18.0)
                .min_w(0.0)
                .max_w(420.0)
                .h(56.0)
                .px(18.0)
                .center_y()
                .radius(16.0)
                .continuous_corners()
                .fill(theme.color("#0d0d0d"))
                .child(
                    text(name)
                        .key("workspace-view-title")
                        .truncate()
                        .font(view_title_font())
                        .color(theme.color("#f2f2f2")),
                ),
        )
        .child(el().grow(1.0))
        .child(tabs);
    if graph_open(state) {
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
                    text("No campaign features are enabled.")
                        .font(ui_font(15.0, 400.0))
                        .color(theme.color("#969696")),
                ),
        );
    }
    if state.workspace_tab == WorkspaceTab::Knowledge {
        return page.child(crate::knowledge_view::render(state, theme));
    }
    let details = state.workspace_details.as_ref().filter(|details| {
        details
            .summary
            .as_ref()
            .is_some_and(|summary| summary.id == id.to_string())
    });
    if let Some(details) = details {
        let forms = match state.workspace_tab {
            WorkspaceTab::Surfaces => surfaces(state, theme),
            WorkspaceTab::Rules => rules(state, theme),
            WorkspaceTab::Foundation => vec![foundation(state, details, theme)],
            WorkspaceTab::Campaign => Vec::new(),
            WorkspaceTab::Knowledge => Vec::new(),
        };
        for form in forms {
            page = page.child(form);
        }
        let dirty = match state.workspace_tab {
            WorkspaceTab::Foundation => details.summary.as_ref().is_some_and(|summary| {
                state.workspace_name_edit.text.trim() != summary.name
                    || state.workspace_guidance_edit.text.trim_end() != details.guidance.trim_end()
            }),
            WorkspaceTab::Surfaces => state.workspace_surface_edits != details.surfaces,
            WorkspaceTab::Rules => state.workspace_rule_edits != details.rules,
            WorkspaceTab::Campaign => false,
            WorkspaceTab::Knowledge => false,
        };
        let tab = state.workspace_tab;
        let saving = state.workspace_save_busy == Some(tab);
        let save = form_button(if saving { "Saving…" } else { "Save Changes" }, theme).key(
            if dirty && !saving && !state.workspace_details_loading {
                "saved-workspace-save"
            } else {
                "saved-workspace-save-disabled"
            },
        );
        if state.workspace_tab != WorkspaceTab::Surfaces || state.workspace_surface_management_open
        {
            page = page.child(row().w_full().child(el().grow(1.0)).child(
                if dirty && !saving && !state.workspace_details_loading {
                    save.on_press(move |state: &mut State| crate::start_workspace_save(state, tab))
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
        && !state.workspace_surface_management_open
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
