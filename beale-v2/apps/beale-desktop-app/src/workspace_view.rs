use beale_contracts::v1::{
    SurfaceCreator, SurfaceKind, SurfaceScope, WorkspaceDetails, WorkspaceSurface,
};
use rune::{Color, El, Icon, col, el, icon, row, text, text_input};

use crate::{
    FormSpec, SelectionMenu, State, ThemeStyle, ViewLocation, form_button, form_label_font,
    icons::{PEN, PLUS, TRASH},
    modal_dialog, selection_button, settings_form, settings_form_card, settings_form_with_action,
    toggle_selection_menu, ui_font, view_title_font,
    workspace_creation::{
        SurfaceKind as DraftKind, SurfaceScope as DraftScope, field, row_with_control,
    },
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum WorkspaceTab {
    #[default]
    Campaign,
    Foundation,
    Surfaces,
    Rules,
}

impl WorkspaceTab {
    const ALL: [Self; 4] = [
        Self::Campaign,
        Self::Foundation,
        Self::Surfaces,
        Self::Rules,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Campaign => "Campaign",
            Self::Surfaces => "Surfaces",
            Self::Rules => "Rules",
            Self::Foundation => "Foundation",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Campaign => "workspace-tab-campaign",
            Self::Surfaces => "workspace-tab-surfaces",
            Self::Rules => "workspace-tab-rules",
            Self::Foundation => "workspace-tab-foundation",
        }
    }
}

fn tab_button(tab: WorkspaceTab, selected: bool, id: uuid::Uuid, theme: ThemeStyle<'_>) -> El {
    let color = |value| theme.color(value);
    row()
        .key(tab.key())
        .role("tab")
        .label(tab.label())
        .h(26.0)
        .px(12.0)
        .center()
        .radius(8.0)
        .continuous_corners()
        .fill(if selected {
            color("#333333")
        } else {
            Color::TRANSPARENT
        })
        .hover(color("#303030"))
        .child(
            text(tab.label())
                .font(ui_font(14.0, if selected { 600.0 } else { 450.0 }))
                .color(color("#e2e2e2")),
        )
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
                .truncate()
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#e2e2e2")),
        )
}

fn icon_action_button(
    key: String,
    label: &'static str,
    glyph: &'static Icon,
    theme: ThemeStyle<'_>,
) -> El {
    el().key(key)
        .role("button")
        .label(label)
        .size(26.0, 26.0)
        .center()
        .radius(8.0)
        .continuous_corners()
        .hover(theme.color("#303030"))
        .child(icon(glyph, 16.0).color(theme.color("#bdbdbd")))
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

fn foundation(state: &State, details: &WorkspaceDetails, theme: ThemeStyle<'_>) -> El {
    let Some(summary) = details.summary.as_ref() else {
        return col();
    };
    let busy = state.workspace_save_busy.is_some();
    let name = if busy {
        read_only_value(state.workspace_name_edit.text.clone(), 240.0, theme)
    } else {
        field(
            &state.workspace_name_edit,
            "saved-workspace-name-input",
            "Name",
            theme,
            |state| &mut state.workspace_name_edit,
        )
        .w(240.0)
    };
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
            heading: "Workspace Details",
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
                read_only_value(summary.directory.clone(), 320.0, theme),
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

fn surfaces(state: &State, theme: ThemeStyle<'_>) -> Vec<El> {
    [SurfaceScope::InScope, SurfaceScope::OutOfScope]
        .into_iter()
        .map(|scope| {
            let mut list = col().w_full().py(12.0).gap(8.0);
            let mut count = 0;
            for (index, surface) in state.workspace_surface_edits.iter().enumerate() {
                if surface.scope != scope as i32 {
                    continue;
                }
                count += 1;
                let kind = match SurfaceKind::try_from(surface.kind) {
                    Ok(SurfaceKind::Repository) => "Repository",
                    Ok(SurfaceKind::Documentation) => "Documentation",
                    Ok(SurfaceKind::Binary) => "Binary",
                    Ok(SurfaceKind::Service) => "Service",
                    Ok(SurfaceKind::Domain) => "Domain",
                    _ => "Other",
                };
                let creator = if surface.created_by == SurfaceCreator::Agent as i32 {
                    "Agent"
                } else {
                    "User"
                };
                let mut label = format!("{kind} · {} · {creator}", surface.value);
                if !surface.out_of_scope_reason.is_empty() {
                    label.push_str(&format!(" · {}", surface.out_of_scope_reason));
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
                        state.workspace_surface_kind = draft_kind(surface.kind);
                        state.workspace_surface_value = rune::TextEdit::new(&surface.value);
                        state.workspace_surface_error.clear();
                        state.workspace_surface_edit_index = Some(index);
                        state.workspace_surface_dialog_scope =
                            Some(if surface.scope == SurfaceScope::InScope as i32 {
                                DraftScope::InScope
                            } else {
                                DraftScope::OutOfScope
                            });
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
                    state.workspace_surface_kind = DraftKind::default();
                    state.workspace_surface_value.clear();
                    state.workspace_surface_error.clear();
                    state.workspace_surface_edit_index = None;
                    state.workspace_surface_dialog_scope = Some(draft_scope);
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
        })
        .collect()
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
        let remove =
            form_button("Remove", theme).key(format!("saved-workspace-rule-remove-{index}"));
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
                && item.kind == kind as i32
                && item.value.eq_ignore_ascii_case(value)
        })
    {
        state.workspace_surface_error = "That surface is already listed.".to_owned();
        return;
    }
    if let Some(index) = state.workspace_surface_edit_index {
        let Some(surface) = state.workspace_surface_edits.get_mut(index) else {
            close_surface_dialog(state);
            return;
        };
        if surface.kind != kind as i32 || !surface.value.eq_ignore_ascii_case(value) {
            surface.created_by = SurfaceCreator::User as i32;
            surface.out_of_scope_reason = if scope == DraftScope::OutOfScope {
                "Set in workspace".to_owned()
            } else {
                String::new()
            };
        }
        surface.kind = kind as i32;
        surface.value = value.to_owned();
    } else {
        state.workspace_surface_edits.push(WorkspaceSurface {
            kind: kind as i32,
            scope: if scope == DraftScope::InScope {
                SurfaceScope::InScope
            } else {
                SurfaceScope::OutOfScope
            } as i32,
            value: value.to_owned(),
            created_by: SurfaceCreator::User as i32,
            out_of_scope_reason: if scope == DraftScope::OutOfScope {
                "Set in workspace".to_owned()
            } else {
                String::new()
            },
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
        ],
        theme,
    );
    let mut body = col().w_full().gap(10.0).child(form);
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
        state.workspace_surface_edits.remove(index);
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
        |surface| format!("{} · {}", draft_kind(surface.kind).label(), surface.value),
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

pub(crate) fn render(state: &State, theme: ThemeStyle<'_>, campaign_height: f32) -> El {
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
        .w_full()
        .gap(6.0);
    for tab in WorkspaceTab::ALL {
        tabs = tabs.child(tab_button(tab, state.workspace_tab == tab, id, theme));
    }
    let mut page = col()
        .key("workspace-view-page")
        .w_full()
        .gap(20.0)
        .child(
            text(name)
                .key("workspace-view-title")
                .font(view_title_font())
                .color(theme.color("#f2f2f2")),
        )
        .child(tabs);
    if state.workspace_tab == WorkspaceTab::Campaign {
        return page.child(
            el().key("workspace-campaign-empty")
                .w_full()
                .min_h(campaign_height)
                .center()
                .child(
                    text("No campaign features are enabled.")
                        .font(ui_font(15.0, 400.0))
                        .color(theme.color("#969696")),
                ),
        );
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
        page = page.child(row().w_full().child(el().grow(1.0)).child(
            if dirty && !saving && !state.workspace_details_loading {
                save.on_press(move |state: &mut State| crate::start_workspace_save(state, tab))
            } else {
                save
            },
        ));
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
