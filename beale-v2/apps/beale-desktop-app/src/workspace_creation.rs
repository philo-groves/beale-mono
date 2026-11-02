use rune::{
    Color, Cursor, El, FileDialogOptions, TextEdit, Tx, col, el, icon, row, text, text_input,
};

use crate::{
    FormSpec, SelectionMenu, State, ThemeStyle, ViewLocation, form_button, form_label_font,
    icons::FOLDER_OPEN, selection_button, settings_form, toggle_selection_menu, ui_font,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum CreationTab {
    #[default]
    Settings,
    Resources,
    Rules,
}

struct TabSpec {
    tab: CreationTab,
    key: &'static str,
    label: &'static str,
}

const TABS: [TabSpec; 3] = [
    TabSpec {
        tab: CreationTab::Settings,
        key: "workspace-tab-settings",
        label: "Settings",
    },
    TabSpec {
        tab: CreationTab::Resources,
        key: "workspace-tab-resources",
        label: "Resources",
    },
    TabSpec {
        tab: CreationTab::Rules,
        key: "workspace-tab-rules",
        label: "Rules",
    },
];

impl CreationTab {
    pub(crate) fn label(self) -> &'static str {
        TABS.iter()
            .find(|spec| spec.tab == self)
            .expect("creation tab is defined")
            .label
    }

    fn index(self) -> usize {
        TABS.iter()
            .position(|spec| spec.tab == self)
            .expect("creation tab is defined")
    }

    fn next(self) -> Option<Self> {
        TABS.get(self.index() + 1).map(|spec| spec.tab)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ResourceKind {
    #[default]
    Repository,
    Documentation,
    Binary,
    Service,
    Domain,
    Other,
}

impl ResourceKind {
    pub(crate) const ALL: [Self; 6] = [
        Self::Repository,
        Self::Documentation,
        Self::Binary,
        Self::Service,
        Self::Domain,
        Self::Other,
    ];

    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Repository => "repo",
            Self::Documentation => "documentation",
            Self::Binary => "binary",
            Self::Service => "service",
            Self::Domain => "domain",
            Self::Other => "other",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Repository => "Repository",
            Self::Documentation => "Documentation",
            Self::Binary => "Binary",
            Self::Service => "Service",
            Self::Domain => "Domain",
            Self::Other => "Other",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ResourceDirection {
    #[default]
    InScope,
    OutOfScope,
}

impl ResourceDirection {
    pub(crate) const ALL: [Self; 2] = [Self::InScope, Self::OutOfScope];

    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::InScope => "in-scope",
            Self::OutOfScope => "out-of-scope",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::InScope => "In Scope",
            Self::OutOfScope => "Out of Scope",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Resource {
    pub(crate) kind: ResourceKind,
    pub(crate) direction: ResourceDirection,
    pub(crate) value: String,
}

pub(crate) struct WorkspaceDraft {
    pub(crate) tab: CreationTab,
    unlocked: usize,
    pub(crate) name: TextEdit,
    pub(crate) directory: TextEdit,
    pub(crate) guidance: TextEdit,
    pub(crate) resource_kind: ResourceKind,
    pub(crate) resource_direction: ResourceDirection,
    pub(crate) resource_value: TextEdit,
    pub(crate) resources: Vec<Resource>,
    pub(crate) rule_value: TextEdit,
    pub(crate) rules: Vec<String>,
    error: Option<&'static str>,
}

impl Default for WorkspaceDraft {
    fn default() -> Self {
        Self {
            tab: CreationTab::Settings,
            unlocked: 0,
            name: TextEdit::new(""),
            directory: TextEdit::new(""),
            guidance: TextEdit::new(""),
            resource_kind: ResourceKind::default(),
            resource_direction: ResourceDirection::default(),
            resource_value: TextEdit::new(""),
            resources: Vec::new(),
            rule_value: TextEdit::new(""),
            rules: Vec::new(),
            error: None,
        }
    }
}

impl WorkspaceDraft {
    fn validate_current(&self) -> Option<&'static str> {
        match self.tab {
            CreationTab::Settings if self.name.text.trim().is_empty() => {
                Some("Workspace name is required.")
            }
            CreationTab::Settings if self.name.text.trim().chars().count() > 200 => {
                Some("Workspace name must be at most 200 characters.")
            }
            CreationTab::Settings if self.directory.text.trim().is_empty() => {
                Some("Choose a workspace directory.")
            }
            CreationTab::Settings
                if !std::path::Path::new(self.directory.text.trim()).is_absolute() =>
            {
                Some("Use an absolute workspace directory.")
            }
            CreationTab::Resources
                if !self
                    .resources
                    .iter()
                    .any(|resource| resource.direction == ResourceDirection::InScope) =>
            {
                Some("Add at least one in-scope resource.")
            }
            _ => None,
        }
    }

    fn add_resource(&mut self) {
        let value = self.resource_value.text.trim();
        if value.is_empty() {
            self.error = Some("Enter a resource.");
        } else if self.resources.iter().any(|resource| {
            resource.kind == self.resource_kind && resource.value.eq_ignore_ascii_case(value)
        }) {
            self.error = Some("That resource is already listed.");
        } else {
            self.resources.push(Resource {
                kind: self.resource_kind,
                direction: self.resource_direction,
                value: value.to_owned(),
            });
            self.resource_value.clear();
            self.error = None;
        }
    }

    fn add_rule(&mut self) {
        let value = self.rule_value.text.trim();
        if value.is_empty() {
            self.error = Some("Enter a workspace rule.");
        } else if value.chars().count() > 2_000 {
            self.error = Some("Workspace rules must be at most 2,000 characters.");
        } else if self.rules.iter().any(|rule| rule == value) {
            self.error = Some("That rule is already listed.");
        } else {
            self.rules.push(value.to_owned());
            self.rule_value.clear();
            self.error = None;
        }
    }
}

pub(crate) fn advance(state: &mut State, tx: &mut Tx) {
    if let Some(error) = state.workspace_draft.validate_current() {
        state.workspace_draft.error = Some(error);
        return;
    }
    if let Some(next) = state.workspace_draft.tab.next() {
        state.workspace_draft.error = None;
        state.workspace_draft.unlocked = state.workspace_draft.unlocked.max(next.index());
        state.navigate_to(ViewLocation::WorkspaceCreation(next));
        tx.scroll_to_y("workspace-scroll", 0.0);
    }
}

fn tab_button(spec: &TabSpec, state: &State, theme: ThemeStyle<'_>) -> El {
    let selected = state.workspace_draft.tab == spec.tab;
    let unlocked = spec.tab.index() <= state.workspace_draft.unlocked;
    let color = |value| theme.color(value);
    let button = row()
        .key(spec.key)
        .role("tab")
        .label(if unlocked {
            spec.label.to_owned()
        } else {
            format!("{} locked", spec.label)
        })
        .h(25.0)
        .px(12.0)
        .center()
        .radius(8.0)
        .continuous_corners()
        .fill(if selected {
            color("#333333")
        } else {
            Color::TRANSPARENT
        })
        .child(
            text(spec.label)
                .font(ui_font(14.0, if selected { 600.0 } else { 450.0 }))
                .color(color(if unlocked { "#e2e2e2" } else { "#777777" })),
        );
    if unlocked {
        let tab = spec.tab;
        button
            .hover(color("#303030"))
            .on_press_tx(move |s: &mut State, tx| {
                s.workspace_draft.error = None;
                s.navigate_to(ViewLocation::WorkspaceCreation(tab));
                tx.scroll_to_y("workspace-scroll", 0.0);
            })
    } else {
        button
    }
}

fn row_with_control(
    label: &'static str,
    description: &'static str,
    control: El,
    theme: ThemeStyle<'_>,
) -> El {
    let color = |value| theme.color(value);
    row()
        .w_full()
        .min_h(62.0)
        .py(10.0)
        .center_y()
        .gap(12.0)
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(4.0)
                .child(text(label).font(form_label_font()).color(color("#e2e2e2")))
                .child(
                    text(description)
                        .font(ui_font(14.0, 400.0))
                        .color(color("#969696"))
                        .wrap(),
                ),
        )
        .child(control)
}

fn field(
    edit: &TextEdit,
    key: &'static str,
    placeholder: &'static str,
    theme: ThemeStyle<'_>,
    update: impl for<'a> Fn(&'a mut State) -> &'a mut TextEdit + 'static,
) -> El {
    let font = ui_font(14.0, 400.0);
    text_input(edit, font)
        .key(key)
        .single_line()
        .w(180.0)
        .h(32.0)
        .px(10.0)
        .py(((32.0 - font.line_px()) * 0.5).max(0.0))
        .radius(8.0)
        .continuous_corners()
        .fill(theme.color("#282828"))
        .color(theme.color("#e2e2e2"))
        .placeholder(placeholder, theme.color("#888888"))
        .on_edit(update)
}

fn settings(state: &State, theme: ThemeStyle<'_>) -> Vec<El> {
    let kit = selection_button(
        "None",
        state.selection_menu == Some(SelectionMenu::ResearchKit),
        |s| toggle_selection_menu(s, SelectionMenu::ResearchKit),
        theme,
    )
    .key("workspace-kit-control");
    let details = settings_form(
        FormSpec {
            key: "workspace-details-form",
            heading: "Workspace Details",
        },
        [
            row_with_control(
                "Workspace Name",
                "The name shown throughout Beale.",
                field(
                    &state.workspace_draft.name,
                    "workspace-name-input",
                    "Name",
                    theme,
                    |s| &mut s.workspace_draft.name,
                ),
                theme,
            ),
            row_with_control(
                "Workspace Directory",
                "Directory for this workspace on the host.",
                row()
                    .center_y()
                    .gap(6.0)
                    .child(
                        el().key("workspace-directory-picker")
                            .role("button")
                            .label("Choose workspace directory")
                            .size(32.0, 32.0)
                            .center()
                            .radius(8.0)
                            .continuous_corners()
                            .border(1.0, theme.color("#2c2c2c"))
                            .hover(theme.color("#303030"))
                            .cursor(Cursor::Pointer)
                            .on_press_tx(|s: &mut State, tx| {
                                let path =
                                    std::path::Path::new(s.workspace_draft.directory.text.trim());
                                let directory = path.is_dir().then(|| path.to_path_buf());
                                tx.open_folder(
                                    FileDialogOptions {
                                        title: "Choose Workspace Directory".to_owned(),
                                        directory,
                                        ..Default::default()
                                    },
                                    |s: &mut State, result, _| {
                                        if s.primary_view != crate::PrimaryView::WorkspaceCreation {
                                            return;
                                        }
                                        match result {
                                            Ok(Some(path)) => {
                                                s.workspace_draft.directory =
                                                    TextEdit::new(path.to_string_lossy());
                                                s.workspace_draft.error = None;
                                            }
                                            Ok(None) => {}
                                            Err(_) => {
                                                s.workspace_draft.error =
                                                    Some("Could not open the folder picker.")
                                            }
                                        }
                                    },
                                );
                            })
                            .child(icon(&FOLDER_OPEN, 16.0).color(theme.color("#bdbdbd"))),
                    )
                    .child(field(
                        &state.workspace_draft.directory,
                        "workspace-directory-input",
                        "Absolute path",
                        theme,
                        |s| &mut s.workspace_draft.directory,
                    )),
                theme,
            ),
            row_with_control(
                "Research Kit",
                "Optional resource and rule acquisition kit.",
                kit,
                theme,
            ),
            col()
                .key("workspace-guidance-row")
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
                .child(
                    text_input(&state.workspace_draft.guidance, ui_font(14.0, 400.0))
                        .key("workspace-guidance-input")
                        .w_full()
                        .h(130.0)
                        .padding(10.0)
                        .radius(8.0)
                        .fill(theme.color("#282828"))
                        .color(theme.color("#e2e2e2"))
                        .placeholder("Add workspace guidance", theme.color("#888888"))
                        .on_edit(|s: &mut State| &mut s.workspace_draft.guidance),
                ),
        ],
        theme,
    );
    vec![details]
}

fn resources(state: &State, theme: ThemeStyle<'_>) -> Vec<El> {
    let kind = selection_button(
        state.workspace_draft.resource_kind.label(),
        state.selection_menu == Some(SelectionMenu::ResourceKind),
        |s| toggle_selection_menu(s, SelectionMenu::ResourceKind),
        theme,
    )
    .key("workspace-resource-kind-control");
    let direction = selection_button(
        state.workspace_draft.resource_direction.label(),
        state.selection_menu == Some(SelectionMenu::ResourceDirection),
        |s| toggle_selection_menu(s, SelectionMenu::ResourceDirection),
        theme,
    )
    .key("workspace-resource-direction-control");
    let editor = settings_form(
        FormSpec {
            key: "workspace-resource-form",
            heading: "Authorized Resources",
        },
        [
            row_with_control("Type", "Kind of target or source material.", kind, theme),
            row_with_control(
                "Scope",
                "Whether research may test this resource.",
                direction,
                theme,
            ),
            row_with_control(
                "Resource",
                "Identifier, URL, or path.",
                field(
                    &state.workspace_draft.resource_value,
                    "workspace-resource-input",
                    "Enter resource",
                    theme,
                    |s| &mut s.workspace_draft.resource_value,
                ),
                theme,
            ),
        ],
        theme,
    );
    let mut list = col().key("workspace-resource-list").w_full().gap(8.0);
    for (index, resource) in state.workspace_draft.resources.iter().enumerate() {
        let label = format!(
            "{} · {} · {}",
            resource.kind.label(),
            resource.direction.label(),
            resource.value
        );
        list = list.child(
            row()
                .key(format!("workspace-resource-{index}"))
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
                .child(
                    form_button("Remove", theme)
                        .key(format!("workspace-resource-remove-{index}"))
                        .on_press(move |s: &mut State| {
                            s.workspace_draft.resources.remove(index);
                        }),
                ),
        );
    }
    if state.workspace_draft.resources.is_empty() {
        list = list.child(
            text("No resources recorded.")
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#969696")),
        );
    }
    vec![
        editor,
        row().w_full().child(el().grow(1.0)).child(
            form_button("Add Resource", theme)
                .key("workspace-add-resource")
                .on_press(|s: &mut State| s.workspace_draft.add_resource()),
        ),
        settings_form(
            FormSpec {
                key: "workspace-recorded-resources",
                heading: "Recorded Resources",
            },
            [col().w_full().py(12.0).child(list)],
            theme,
        ),
    ]
}

fn rules(state: &State, theme: ThemeStyle<'_>) -> Vec<El> {
    let editor = settings_form(
        FormSpec {
            key: "workspace-rules-form",
            heading: "Workspace Rules",
        },
        [row_with_control(
            "New Rule",
            "Operating constraint for research sessions.",
            field(
                &state.workspace_draft.rule_value,
                "workspace-rule-input",
                "Add a rule",
                theme,
                |s| &mut s.workspace_draft.rule_value,
            ),
            theme,
        )],
        theme,
    );
    let mut list = col().key("workspace-rule-list").w_full().gap(8.0);
    for (index, rule) in state.workspace_draft.rules.iter().enumerate() {
        list = list.child(
            row()
                .key(format!("workspace-rule-{index}"))
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
                .child(
                    form_button("Remove", theme)
                        .key(format!("workspace-rule-remove-{index}"))
                        .on_press(move |s: &mut State| {
                            s.workspace_draft.rules.remove(index);
                        }),
                ),
        );
    }
    if state.workspace_draft.rules.is_empty() {
        list = list.child(
            text("No workspace rules recorded.")
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#969696")),
        );
    }
    vec![
        editor,
        row().w_full().child(el().grow(1.0)).child(
            form_button("Add Rule", theme)
                .key("workspace-add-rule")
                .on_press(|s: &mut State| s.workspace_draft.add_rule()),
        ),
        settings_form(
            FormSpec {
                key: "workspace-recorded-rules",
                heading: "Recorded Rules",
            },
            [col().w_full().py(12.0).child(list)],
            theme,
        ),
    ]
}

pub(crate) fn render(state: &State, theme: ThemeStyle<'_>) -> El {
    let color = |value| theme.color(value);
    let mut page = col()
        .key("workspace-creation-page")
        .w_full()
        .gap(20.0)
        .child(
            text("New Workspace")
                .key("workspace-creation-title")
                .font(ui_font(27.0, 600.0))
                .color(color("#f2f2f2")),
        )
        .child({
            let mut tabs = row()
                .key("workspace-creation-tabs")
                .role("tablist")
                .label("New Workspace views")
                .w_full()
                .gap(6.0);
            for spec in &TABS {
                tabs = tabs.child(tab_button(spec, state, theme));
            }
            tabs
        });
    let forms = match state.workspace_draft.tab {
        CreationTab::Settings => settings(state, theme),
        CreationTab::Resources => resources(state, theme),
        CreationTab::Rules => rules(state, theme),
    };
    for form in forms {
        page = page.child(form);
    }
    if let Some(error) = state.workspace_draft.error {
        page = page.child(
            text(error)
                .key("workspace-creation-error")
                .font(ui_font(14.0, 400.0))
                .color(color("#e07676")),
        );
    }
    let mut actions = row()
        .key("workspace-creation-actions")
        .w_full()
        .gap(8.0)
        .child(el().grow(1.0))
        .child(
            form_button("Cancel", theme)
                .key("workspace-cancel")
                .on_press(|s: &mut State| s.cancel_workspace_creation()),
        );
    if state.workspace_draft.tab.next().is_some() {
        actions = actions.child(
            form_button("Next", theme)
                .key("workspace-next")
                .on_press_tx(advance),
        );
    } else {
        actions = actions.child(
            row()
                .key("workspace-create-disabled")
                .role("button")
                .label("Create Workspace unavailable")
                .h(26.0)
                .px(12.0)
                .center()
                .radius(8.0)
                .continuous_corners()
                .fill(color("#282828"))
                .child(
                    text("Create Workspace")
                        .font(ui_font(14.0, 550.0))
                        .color(color("#777777")),
                ),
        );
    }
    page = page.child(actions);
    if state.workspace_draft.tab == CreationTab::Rules {
        page = page.child(
            text("Workspace creation awaits the app-server workspace contract.")
                .key("workspace-create-note")
                .font(ui_font(13.0, 400.0))
                .color(color("#969696")),
        );
    }
    page
}
