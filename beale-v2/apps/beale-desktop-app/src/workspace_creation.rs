use rune::{
    Color, El, FileDialogOptions, ScrollSpec, TextEdit, Tx, col, el, icon, row, text, text_input,
};

use crate::{
    FormSpec, SelectionMenu, State, ThemeStyle, ViewLocation, form_button,
    form_controls::icon_action_button,
    form_item_row, form_label_font,
    icons::{FOLDER_OPEN, PEN, PLUS},
    modal_dialog, selection_button, settings_form, settings_form_card, settings_form_with_action,
    toggle_selection_menu, ui_font, view_title_font,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum CreationTab {
    #[default]
    Import,
    Foundation,
    Surfaces,
    Rules,
}

struct TabSpec {
    tab: CreationTab,
    key: &'static str,
    label: &'static str,
}

const TABS: [TabSpec; 4] = [
    TabSpec {
        tab: CreationTab::Import,
        key: "workspace-tab-import",
        label: "Import",
    },
    TabSpec {
        tab: CreationTab::Foundation,
        key: "workspace-tab-foundation",
        label: "General",
    },
    TabSpec {
        tab: CreationTab::Surfaces,
        key: "workspace-tab-surfaces",
        label: "Surfaces",
    },
    TabSpec {
        tab: CreationTab::Rules,
        key: "workspace-tab-rules",
        label: "Rules",
    },
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BasicEdit {
    Name,
    Directory,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ImportSource {
    #[default]
    None,
    ResearchKit,
    CustomInstructions,
}

impl ImportSource {
    const ALL: [Self; 3] = [Self::None, Self::ResearchKit, Self::CustomInstructions];

    fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::ResearchKit => "Research Kit",
            Self::CustomInstructions => "Custom Instructions",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::None => "Configure workspace details manually.",
            Self::ResearchKit => "Prepare surfaces and rules from a Research Kit.",
            Self::CustomInstructions => {
                "Prepare a draft from your instructions with the default large model."
            }
        }
    }
}

fn home_directory() -> Option<std::path::PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
}

pub(crate) fn display_directory(value: &str) -> String {
    let path = std::path::Path::new(value);
    if let Some(home) = home_directory() {
        if let Ok(relative) = path.strip_prefix(&home) {
            let suffix = relative.to_string_lossy().replace('\\', "/");
            return if suffix.is_empty() {
                "~/".to_owned()
            } else {
                format!("~/{suffix}")
            };
        }
        if cfg!(windows) {
            let path = value.replace('\\', "/");
            let home = home.to_string_lossy().replace('\\', "/");
            let path = path.strip_prefix("//?/").unwrap_or(&path);
            let home = home.strip_prefix("//?/").unwrap_or(&home);
            if let Some(prefix) = path.get(..home.len())
                && prefix.eq_ignore_ascii_case(home)
                && let Some(suffix) = path.get(home.len()..)
                && (suffix.is_empty() || suffix.starts_with('/'))
            {
                let suffix = suffix.trim_start_matches('/');
                return if suffix.is_empty() {
                    "~/".to_owned()
                } else {
                    format!("~/{suffix}")
                };
            }
        }
    }
    value.to_owned()
}

pub(crate) fn value_with_actions(
    key: &'static str,
    value: String,
    width: f32,
    actions: Vec<El>,
    theme: ThemeStyle<'_>,
) -> El {
    let empty = value.trim().is_empty();
    let mut control = row()
        .key(key)
        .w(width)
        .min_w(0.0)
        .h(32.0)
        .center_y()
        .gap(6.0)
        .child(el().grow(1.0))
        .child(
            text(if empty { "Required".to_owned() } else { value })
                .key(format!("{key}-value"))
                .min_w(0.0)
                .truncate()
                .font(ui_font(14.0, 400.0))
                .color(theme.color(if empty { "#dca15f" } else { "#e2e2e2" })),
        );
    for action in actions {
        control = control.child(action);
    }
    control
}

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

    pub(crate) fn next(self, surfaces_enabled: bool) -> Option<Self> {
        TABS.iter()
            .skip(self.index() + 1)
            .find(|spec| surfaces_enabled || spec.tab != Self::Surfaces)
            .map(|spec| spec.tab)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum SurfaceKind {
    #[default]
    Repository,
    Documentation,
    Binary,
    Service,
    Domain,
    Other,
}

impl SurfaceKind {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SurfaceScope {
    InScope,
    OutOfScope,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SurfaceCreator {
    User,
    // Agent-created surfaces will be supplied by later app-server session operations.
    #[allow(dead_code)]
    Agent,
}

impl SurfaceCreator {
    fn label(self) -> &'static str {
        match self {
            Self::User => "User",
            Self::Agent => "Agent",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Surface {
    pub(crate) kind: SurfaceKind,
    pub(crate) scope: SurfaceScope,
    pub(crate) value: String,
    pub(crate) created_by: SurfaceCreator,
    pub(crate) out_of_scope_reason: Option<String>,
    pub(crate) id: String,
    pub(crate) definition: Option<beale_contracts::v1::SurfaceDefinition>,
    pub(crate) relationships: Vec<beale_contracts::v1::SurfaceRelationship>,
}

pub(crate) struct WorkspaceDraft {
    pub(crate) tab: CreationTab,
    unlocked: usize,
    pub(crate) import_source: ImportSource,
    pub(crate) import_instructions: TextEdit,
    pub(crate) import_pending_id: Option<uuid::Uuid>,
    pub(crate) import_cancel: Option<tokio::sync::oneshot::Sender<()>>,
    pub(crate) import_log: String,
    pub(crate) name: TextEdit,
    pub(crate) directory: TextEdit,
    pub(crate) basic_dialog: Option<BasicEdit>,
    pub(crate) basic_dialog_value: TextEdit,
    pub(crate) basic_dialog_error: Option<&'static str>,
    pub(crate) guidance: TextEdit,
    pub(crate) surface_kind: SurfaceKind,
    pub(crate) surface_value: TextEdit,
    pub(crate) surface_type_id: TextEdit,
    pub(crate) surface_name: TextEdit,
    pub(crate) surface_locator_kind: TextEdit,
    pub(crate) surface_locator_value: TextEdit,
    pub(crate) surface_description: TextEdit,
    next_surface_draft_number: u64,
    pub(crate) surfaces: Vec<Surface>,
    pub(crate) surface_dialog_scope: Option<SurfaceScope>,
    surface_error: Option<&'static str>,
    pub(crate) cancel_dialog_open: bool,
    pub(crate) rule_value: TextEdit,
    pub(crate) rules: Vec<String>,
    pub(crate) error: Option<&'static str>,
}

impl Default for WorkspaceDraft {
    fn default() -> Self {
        Self {
            tab: CreationTab::Import,
            unlocked: 0,
            import_source: ImportSource::None,
            import_instructions: TextEdit::new(""),
            import_pending_id: None,
            import_cancel: None,
            import_log: String::new(),
            name: TextEdit::new(""),
            directory: TextEdit::new(""),
            basic_dialog: None,
            basic_dialog_value: TextEdit::new(""),
            basic_dialog_error: None,
            guidance: TextEdit::new(""),
            surface_kind: SurfaceKind::default(),
            surface_value: TextEdit::new(""),
            surface_type_id: TextEdit::new(""),
            surface_name: TextEdit::new(""),
            surface_locator_kind: TextEdit::new("freeform"),
            surface_locator_value: TextEdit::new(""),
            surface_description: TextEdit::new(""),
            next_surface_draft_number: 1,
            surfaces: Vec::new(),
            surface_dialog_scope: None,
            surface_error: None,
            cancel_dialog_open: false,
            rule_value: TextEdit::new(""),
            rules: Vec::new(),
            error: None,
        }
    }
}

impl WorkspaceDraft {
    pub(crate) fn apply_import(
        &mut self,
        result: beale_contracts::v1::WorkspaceImportResult,
    ) -> Result<(), &'static str> {
        use beale_contracts::v1::SurfaceScope as WireScope;
        if result.name.trim().is_empty()
            || result.name.chars().count() > 200
            || result.name.chars().any(char::is_control)
            || result.guidance.len() > 20_000
            || result.surfaces.len() > 256
            || result.rules.len() > 128
            || result
                .rules
                .iter()
                .any(|rule| rule.is_empty() || rule.len() > 2_000)
        {
            return Err("The model returned an incomplete import.");
        }
        let mut surfaces = Vec::with_capacity(result.surfaces.len());
        for item in result.surfaces {
            let definition = item
                .definition
                .ok_or("The model returned a surface without a definition.")?;
            let value = definition
                .locator
                .as_ref()
                .ok_or("The model returned a surface without a locator.")?
                .value
                .clone();
            if value.trim().is_empty() || value.len() > 2_000 || value.chars().any(char::is_control)
            {
                return Err("The model returned an invalid surface.");
            }
            let kind = match definition.type_id.as_str() {
                "repository" | "code.module" | "code.package" | "code.file" => {
                    SurfaceKind::Repository
                }
                "documentation" => SurfaceKind::Documentation,
                "binary" | "configuration" => SurfaceKind::Binary,
                "service" | "api.endpoint" => SurfaceKind::Service,
                "domain" | "host" => SurfaceKind::Domain,
                _ => SurfaceKind::Other,
            };
            let scope = match WireScope::try_from(definition.scope) {
                Ok(WireScope::InScope) => SurfaceScope::InScope,
                Ok(WireScope::OutOfScope) => SurfaceScope::OutOfScope,
                _ => return Err("The model returned an unknown surface scope."),
            };
            surfaces.push(Surface {
                kind,
                scope,
                value,
                created_by: SurfaceCreator::Agent,
                out_of_scope_reason: (scope == SurfaceScope::OutOfScope)
                    .then_some(definition.out_of_scope_reason.clone()),
                id: item.id,
                definition: Some(definition),
                relationships: item.relationships,
            });
        }
        self.name = TextEdit::new(result.name);
        self.guidance = TextEdit::new(result.guidance);
        self.surfaces = surfaces;
        self.rules = result.rules;
        self.unlocked = self.unlocked.max(CreationTab::Foundation.index());
        Ok(())
    }

    fn open_basic_dialog(&mut self, edit: BasicEdit) {
        self.basic_dialog_value = TextEdit::new(match edit {
            BasicEdit::Name => &self.name.text,
            BasicEdit::Directory => &self.directory.text,
        });
        self.basic_dialog_error = None;
        self.basic_dialog = Some(edit);
    }

    pub(crate) fn close_basic_dialog(&mut self) {
        self.basic_dialog = None;
        self.basic_dialog_value.clear();
        self.basic_dialog_error = None;
    }

    fn apply_basic_dialog(&mut self) {
        let Some(edit) = self.basic_dialog else {
            return;
        };
        let value = self.basic_dialog_value.text.trim();
        match edit {
            BasicEdit::Name => {
                if value.is_empty()
                    || value.chars().count() > 200
                    || value.chars().any(char::is_control)
                {
                    self.basic_dialog_error = Some("Enter a name of at most 200 characters.");
                    return;
                }
                self.name = TextEdit::new(value);
            }
            BasicEdit::Directory => {
                let path = if let Some(relative) = value
                    .strip_prefix("~/")
                    .or_else(|| value.strip_prefix("~\\"))
                {
                    home_directory().map(|home| home.join(relative))
                } else {
                    Some(std::path::PathBuf::from(value))
                };
                let Some(path) = path.filter(|path| path.is_absolute()) else {
                    self.basic_dialog_error = Some("Use an absolute directory or ~/ path.");
                    return;
                };
                self.directory = TextEdit::new(path.to_string_lossy());
            }
        }
        self.error = None;
        self.close_basic_dialog();
    }
    fn open_surface_dialog(&mut self, scope: SurfaceScope) {
        self.surface_kind = SurfaceKind::default();
        self.surface_value.clear();
        self.surface_type_id.clear();
        self.surface_name.clear();
        self.surface_locator_kind = TextEdit::new("freeform");
        self.surface_locator_value.clear();
        self.surface_description.clear();
        self.surface_error = None;
        self.surface_dialog_scope = Some(scope);
    }

    pub(crate) fn close_surface_dialog(&mut self) {
        self.surface_dialog_scope = None;
        self.surface_value.clear();
        self.surface_description.clear();
        self.surface_error = None;
    }

    fn close_cancel_dialog(&mut self) {
        self.cancel_dialog_open = false;
    }

    fn validate_tab(&self, tab: CreationTab) -> Option<&'static str> {
        match tab {
            CreationTab::Import if self.import_source == ImportSource::ResearchKit => {
                Some("Research Kits are not available yet.")
            }
            CreationTab::Import
                if self.import_source == ImportSource::CustomInstructions
                    && self.import_instructions.text.trim().is_empty() =>
            {
                Some("Enter custom instructions to import.")
            }
            CreationTab::Import
                if self.import_source == ImportSource::CustomInstructions
                    && self.import_instructions.text.len() > 16_384 =>
            {
                Some("Custom instructions must be at most 16 KB.")
            }
            CreationTab::Foundation if self.name.text.trim().is_empty() => {
                Some("Workspace name is required.")
            }
            CreationTab::Foundation if self.name.text.trim().chars().count() > 200 => {
                Some("Workspace name must be at most 200 characters.")
            }
            CreationTab::Foundation if self.directory.text.trim().is_empty() => {
                Some("Choose a workspace directory.")
            }
            CreationTab::Foundation
                if !std::path::Path::new(self.directory.text.trim()).is_absolute() =>
            {
                Some("Use an absolute workspace directory.")
            }
            CreationTab::Rules
                if self.rules.is_empty()
                    && !self
                        .surfaces
                        .iter()
                        .any(|surface| surface.scope == SurfaceScope::InScope) =>
            {
                Some("Add a rule that records the authorized scope.")
            }
            _ => None,
        }
    }

    fn validate_current(&self) -> Option<&'static str> {
        self.validate_tab(self.tab)
    }

    pub(crate) fn validate_submission(&self) -> Option<&'static str> {
        self.validate_tab(CreationTab::Foundation)
            .or_else(|| self.validate_tab(CreationTab::Rules))
    }

    fn add_surface(&mut self) {
        use beale_contracts::v1::{SurfaceDefinition, SurfaceLocator};
        let Some(scope) = self.surface_dialog_scope else {
            return;
        };
        let value = self.surface_value.text.trim();
        if value.is_empty() {
            self.surface_error = Some("Enter a surface.");
        } else if self.surfaces.iter().any(|surface| {
            surface.kind == self.surface_kind && surface.value.eq_ignore_ascii_case(value)
        }) {
            self.surface_error = Some("That surface is already listed.");
        } else {
            let type_id = if self.surface_type_id.text.trim().is_empty() {
                match self.surface_kind {
                    SurfaceKind::Repository => "repository",
                    SurfaceKind::Documentation => "documentation",
                    SurfaceKind::Binary => "binary",
                    SurfaceKind::Service => "service",
                    SurfaceKind::Domain => "domain",
                    SurfaceKind::Other => "other",
                }
                .to_owned()
            } else {
                self.surface_type_id.text.trim().to_owned()
            };
            let definition = SurfaceDefinition {
                type_id,
                name: self.surface_name.text.trim().to_owned(),
                locator: Some(SurfaceLocator {
                    kind: self.surface_locator_kind.text.trim().to_owned(),
                    value: if self.surface_locator_value.text.trim().is_empty() {
                        value.to_owned()
                    } else {
                        self.surface_locator_value.text.trim().to_owned()
                    },
                }),
                scope: if scope == SurfaceScope::InScope {
                    beale_contracts::v1::SurfaceScope::InScope
                } else {
                    beale_contracts::v1::SurfaceScope::OutOfScope
                } as i32,
                scope_state: beale_contracts::v1::SurfaceScopeState::Approved as i32,
                description: self.surface_description.text.trim().to_owned(),
                out_of_scope_reason: if scope == SurfaceScope::OutOfScope {
                    "Set at workspace creation".to_owned()
                } else {
                    String::new()
                },
                evidence_refs: Vec::new(),
            };
            let id = loop {
                let candidate = format!("M{}", self.next_surface_draft_number);
                self.next_surface_draft_number += 1;
                if !self.surfaces.iter().any(|surface| surface.id == candidate) {
                    break candidate;
                }
            };
            self.surfaces.push(Surface {
                kind: self.surface_kind,
                scope,
                value: value.to_owned(),
                created_by: SurfaceCreator::User,
                out_of_scope_reason: (scope == SurfaceScope::OutOfScope)
                    .then(|| "Set at workspace creation".to_owned()),
                id,
                definition: Some(definition),
                relationships: Vec::new(),
            });
            self.close_surface_dialog();
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
    if state.workspace_draft.import_pending_id.is_some() {
        return;
    }
    if let Some(error) = state.workspace_draft.validate_current() {
        state.workspace_draft.error = Some(error);
        return;
    }
    if state.workspace_draft.tab == CreationTab::Import
        && state.workspace_draft.import_source == ImportSource::CustomInstructions
    {
        crate::start_workspace_import(state);
        return;
    }
    if let Some(next) = state
        .workspace_draft
        .tab
        .next(crate::extensions_view::surfaces_enabled_for_creation(state))
    {
        state.workspace_draft.error = None;
        state.workspace_draft.unlocked = state.workspace_draft.unlocked.max(next.index());
        state.navigate_to(ViewLocation::WorkspaceCreation(next));
        tx.scroll_to_y("workspace-scroll", 0.0);
    }
}

fn import_source_option(state: &State, source: ImportSource, theme: ThemeStyle<'_>) -> El {
    let selected = state.workspace_draft.import_source == source;
    let description = source.description();
    form_item_row()
        .key(format!("workspace-import-source-{}", source.label()))
        .role("radio")
        .label(source.label())
        .help(description)
        .child(
            col()
                .grow(1.0)
                .gap(4.0)
                .child(
                    text(source.label())
                        .font(form_label_font())
                        .color(theme.color("#e2e2e2")),
                )
                .child(
                    text(description)
                        .font(ui_font(14.0, 400.0))
                        .color(theme.color("#969696")),
                ),
        )
        .child(
            el().size(18.0, 18.0)
                .radius(9.0)
                .center()
                .border(
                    1.0,
                    theme.color(if selected { "#c8c8c8" } else { "#555555" }),
                )
                .child(if selected {
                    el().size(8.0, 8.0).radius(4.0).fill(theme.color("#e2e2e2"))
                } else {
                    el()
                }),
        )
        .on_press(move |s: &mut State| {
            if s.workspace_draft.import_pending_id.is_none() {
                s.workspace_draft.import_source = source;
                s.workspace_draft.error = None;
            }
        })
}

fn import(state: &State, theme: ThemeStyle<'_>) -> Vec<El> {
    let mut forms = vec![settings_form(
        FormSpec {
            key: "workspace-import-source-form",
            heading: "Import Source",
        },
        ImportSource::ALL
            .into_iter()
            .map(|source| import_source_option(state, source, theme)),
        theme,
    )];
    match state.workspace_draft.import_source {
        ImportSource::None => {}
        ImportSource::ResearchKit => {
            forms.push(settings_form(
                FormSpec {
                    key: "workspace-import-details-form",
                    heading: "Import Details",
                },
                [col()
                    .w_full()
                    .py(12.0)
                    .gap(6.0)
                    .child(
                        text("Research Kit")
                            .font(form_label_font())
                            .color(theme.color("#e2e2e2")),
                    )
                    .child(
                        text("No Research Kits available yet.")
                            .font(ui_font(14.0, 400.0))
                            .color(theme.color("#969696")),
                    )],
                theme,
            ));
        }
        ImportSource::CustomInstructions => {
            forms.push(settings_form(
                FormSpec { key: "workspace-import-details-form", heading: "Import Details" },
                [col().w_full().py(12.0).gap(8.0)
                    .child(text("Custom Instructions").font(form_label_font()).color(theme.color("#e2e2e2")))
                    .child(text("Describe the authorized research scope, relevant sources, and rules. The default large model will prepare a draft for review.").font(ui_font(14.0, 400.0)).color(theme.color("#969696")).wrap())
                    .child(text_input(&state.workspace_draft.import_instructions, ui_font(14.0, 400.0))
                        .key("workspace-import-instructions")
                        .w_full().h(180.0).padding(10.0).radius(8.0)
                        .fill(theme.color("#282828")).color(theme.color("#e2e2e2"))
                        .placeholder("Describe what to import", theme.color("#888888"))
                        .on_edit(|s: &mut State| &mut s.workspace_draft.import_instructions))],
                theme,
            ));
        }
    }
    forms
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

pub(crate) fn row_with_control(
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

pub(crate) fn field(
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

fn foundation(state: &State, theme: ThemeStyle<'_>) -> Vec<El> {
    let details = settings_form(
        FormSpec {
            key: "workspace-details-form",
            heading: "Basic Information",
        },
        [
            row_with_control(
                "Workspace Name",
                "The name shown throughout Beale.",
                value_with_actions(
                    "workspace-name-value",
                    state.workspace_draft.name.text.clone(),
                    320.0,
                    vec![
                        icon_action_button(
                            "workspace-name-edit",
                            "Edit workspace name",
                            &PEN,
                            theme,
                        )
                        .on_press(|s: &mut State| {
                            s.workspace_draft.open_basic_dialog(BasicEdit::Name)
                        }),
                    ],
                    theme,
                ),
                theme,
            ),
            row_with_control(
                "Workspace Directory",
                "Directory for this workspace on the host.",
                value_with_actions(
                    "workspace-directory-value",
                    display_directory(&state.workspace_draft.directory.text),
                    320.0,
                    vec![
                        icon_action_button(
                            "workspace-directory-edit",
                            "Edit workspace directory",
                            &PEN,
                            theme,
                        )
                        .on_press(|s: &mut State| {
                            s.workspace_draft.open_basic_dialog(BasicEdit::Directory)
                        }),
                        icon_action_button(
                            "workspace-directory-picker",
                            "Choose workspace directory",
                            &FOLDER_OPEN,
                            theme,
                        )
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
                        }),
                    ],
                    theme,
                ),
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

fn surfaces(state: &State, theme: ThemeStyle<'_>) -> Vec<El> {
    vec![
        surface_list(state, SurfaceScope::InScope, theme),
        surface_list(state, SurfaceScope::OutOfScope, theme),
    ]
}

pub(crate) fn basic_dialog(state: &State, edit: BasicEdit, theme: ThemeStyle<'_>) -> [El; 2] {
    let (title, label, placeholder, focus_return) = match edit {
        BasicEdit::Name => (
            "Edit Workspace Name",
            "Workspace Name",
            "Name",
            "workspace-name-edit",
        ),
        BasicEdit::Directory => (
            "Edit Workspace Directory",
            "Workspace Directory",
            "Absolute path or ~/ path",
            "workspace-directory-edit",
        ),
    };
    let mut body = col().w_full().gap(10.0).child(settings_form_card(
        [row_with_control(
            label,
            "Enter the value for this workspace.",
            field(
                &state.workspace_draft.basic_dialog_value,
                "workspace-basic-dialog-input",
                placeholder,
                theme,
                |s| &mut s.workspace_draft.basic_dialog_value,
            )
            .w(300.0),
            theme,
        )],
        theme,
    ));
    if let Some(error) = state.workspace_draft.basic_dialog_error {
        body = body.child(
            text(error)
                .key("workspace-basic-dialog-error")
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
                .key("workspace-basic-dialog-cancel")
                .on_press(|s: &mut State| s.workspace_draft.close_basic_dialog()),
        )
        .child(
            form_button("Apply", theme)
                .key("workspace-basic-dialog-apply")
                .on_press(|s: &mut State| s.workspace_draft.apply_basic_dialog()),
        );
    modal_dialog::render(
        modal_dialog::DialogSpec {
            key: "workspace-basic-dialog",
            title,
            width: 540.0,
            focus_return,
        },
        body,
        actions,
        theme,
        |s: &mut State| s.workspace_draft.close_basic_dialog(),
    )
}

pub(crate) fn surface_dialog(state: &State, scope: SurfaceScope, theme: ThemeStyle<'_>) -> [El; 2] {
    let kind = selection_button(
        state.workspace_draft.surface_kind.label(),
        state.selection_menu == Some(SelectionMenu::SurfaceKind),
        |s| toggle_selection_menu(s, SelectionMenu::SurfaceKind),
        theme,
    )
    .key("workspace-surface-kind-control");
    let form = settings_form_card(
        [
            row_with_control("Type", "Kind of target or source material.", kind, theme),
            row_with_control(
                "Surface",
                "Identifier, URL, or path.",
                field(
                    &state.workspace_draft.surface_value,
                    "workspace-surface-input",
                    "Enter surface",
                    theme,
                    |s| &mut s.workspace_draft.surface_value,
                ),
                theme,
            ),
            row_with_control(
                "Specific type",
                "For example code.module or api.endpoint.",
                field(
                    &state.workspace_draft.surface_type_id,
                    "workspace-surface-type-id",
                    "repository",
                    theme,
                    |s| &mut s.workspace_draft.surface_type_id,
                ),
                theme,
            ),
            row_with_control(
                "Name",
                "Short display name; defaults to the identifier.",
                field(
                    &state.workspace_draft.surface_name,
                    "workspace-surface-name",
                    "Display name",
                    theme,
                    |s| &mut s.workspace_draft.surface_name,
                ),
                theme,
            ),
            row_with_control(
                "Locator kind",
                "freeform, git_url, repository_path, url, domain, or artifact.",
                field(
                    &state.workspace_draft.surface_locator_kind,
                    "workspace-surface-locator-kind",
                    "freeform",
                    theme,
                    |s| &mut s.workspace_draft.surface_locator_kind,
                ),
                theme,
            ),
            row_with_control(
                "Locator",
                "Typed address or path; defaults to the identifier.",
                field(
                    &state.workspace_draft.surface_locator_value,
                    "workspace-surface-locator",
                    "Locator value",
                    theme,
                    |s| &mut s.workspace_draft.surface_locator_value,
                ),
                theme,
            ),
            row_with_control(
                "Description",
                "What this surface is and why it matters to the research.",
                field(
                    &state.workspace_draft.surface_description,
                    "workspace-surface-description",
                    "Description",
                    theme,
                    |s| &mut s.workspace_draft.surface_description,
                ),
                theme,
            ),
        ],
        theme,
    );
    let mut body = col()
        .key("workspace-surface-dialog-body")
        .w_full()
        .max_h(480.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .gap(10.0)
        .child(form);
    if let Some(error) = state.workspace_draft.surface_error {
        body = body.child(
            text(error)
                .key("workspace-surface-dialog-error")
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
                .key("workspace-surface-dialog-cancel")
                .on_press(|s: &mut State| s.workspace_draft.close_surface_dialog()),
        )
        .child(
            form_button("Add Surface", theme)
                .key("workspace-add-surface")
                .on_press(|s: &mut State| s.workspace_draft.add_surface()),
        );
    let focus_return = match scope {
        SurfaceScope::InScope => "workspace-add-in-scope-surface",
        SurfaceScope::OutOfScope => "workspace-add-out-of-scope-surface",
    };
    let title = match scope {
        SurfaceScope::InScope => "Add an In-Scope Surface",
        SurfaceScope::OutOfScope => "Add an Out-of-Scope Surface",
    };
    modal_dialog::render(
        modal_dialog::DialogSpec {
            key: "workspace-surface-dialog",
            title,
            width: 540.0,
            focus_return,
        },
        body,
        actions,
        theme,
        |s: &mut State| s.workspace_draft.close_surface_dialog(),
    )
}

fn request_cancel(state: &mut State) {
    state.selection_menu = None;
    state.workspace_draft.cancel_dialog_open = true;
}

pub(crate) fn cancel_dialog(theme: ThemeStyle<'_>) -> [El; 2] {
    let body = text("Discard this unfinished workspace draft? Your entries will be lost.")
        .w_full()
        .wrap()
        .font(ui_font(14.0, 400.0))
        .color(theme.color("#b8b8b8"));
    let actions = row()
        .w_full()
        .gap(8.0)
        .child(el().grow(1.0))
        .child(
            form_button("Keep Editing", theme)
                .key("workspace-cancel-keep-editing")
                .on_press(|s: &mut State| s.workspace_draft.close_cancel_dialog()),
        )
        .child(
            form_button("Discard Workspace", theme)
                .key("workspace-cancel-discard")
                .fill(theme.color("#482525"))
                .hover(theme.color("#603030"))
                .on_press(|s: &mut State| s.cancel_workspace_creation()),
        );
    modal_dialog::render(
        modal_dialog::DialogSpec {
            key: "workspace-cancel-dialog",
            title: "Cancel New Workspace?",
            width: 460.0,
            focus_return: "workspace-cancel",
        },
        body,
        actions,
        theme,
        |s: &mut State| s.workspace_draft.close_cancel_dialog(),
    )
}

fn surface_list(state: &State, scope: SurfaceScope, theme: ThemeStyle<'_>) -> El {
    let mut list = col().w_full().py(12.0).gap(8.0);
    let mut count = 0;
    for (index, surface) in state.workspace_draft.surfaces.iter().enumerate() {
        if surface.scope != scope {
            continue;
        }
        count += 1;
        let mut label = format!(
            "{} · {} · {} · {}{}",
            surface.kind.label(),
            surface.value,
            surface.created_by.label(),
            if surface.created_by == SurfaceCreator::Agent {
                "Candidate"
            } else {
                "Approved"
            },
            if surface.id.is_empty() {
                String::new()
            } else {
                format!(" · {}", surface.id)
            },
        );
        if let Some(reason) = &surface.out_of_scope_reason {
            label.push_str(&format!(" · {reason}"));
        }
        if let Some(definition) = &surface.definition {
            if !definition.name.is_empty() && definition.name != surface.value {
                label.push_str(&format!(" · {}", definition.name));
            }
            if !definition.type_id.is_empty() {
                label.push_str(&format!(" · {}", definition.type_id));
            }
            if let Some(locator) = &definition.locator {
                label.push_str(&format!(" · {}: {}", locator.kind, locator.value));
            }
            if !definition.description.is_empty() {
                label.push_str(&format!(" · {}", definition.description));
            }
            if !surface.relationships.is_empty() {
                label.push_str(&format!(" · {} connections", surface.relationships.len()));
                for relationship in surface.relationships.iter().take(3) {
                    label.push_str(&format!(
                        " · {} → {}",
                        relationship.label, relationship.target_id
                    ));
                }
            }
        }
        list = list.child(
            row()
                .key(format!("workspace-surface-{index}"))
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
                        .key(format!("workspace-surface-remove-{index}"))
                        .on_press(move |s: &mut State| {
                            let removed = s.workspace_draft.surfaces.remove(index);
                            for surface in &mut s.workspace_draft.surfaces {
                                surface
                                    .relationships
                                    .retain(|relationship| relationship.target_id != removed.id);
                            }
                        }),
                ),
        );
    }
    if count == 0 {
        list = list.child(
            text("No surfaces listed.")
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#969696")),
        );
    }
    let button_key = match scope {
        SurfaceScope::InScope => "workspace-add-in-scope-surface",
        SurfaceScope::OutOfScope => "workspace-add-out-of-scope-surface",
    };
    let add_button = el()
        .key(button_key)
        .role("button")
        .label(match scope {
            SurfaceScope::InScope => "Add an in-scope surface",
            SurfaceScope::OutOfScope => "Add an out-of-scope surface",
        })
        .size(26.0, 26.0)
        .center()
        .radius(8.0)
        .continuous_corners()
        .hover(theme.color("#303030"))
        .on_press(move |s: &mut State| s.workspace_draft.open_surface_dialog(scope))
        .child(icon(&PLUS, 16.0).color(theme.color("#bdbdbd")));
    settings_form_with_action(
        FormSpec {
            key: match scope {
                SurfaceScope::InScope => "workspace-in-scope-surfaces",
                SurfaceScope::OutOfScope => "workspace-out-of-scope-surfaces",
            },
            heading: match scope {
                SurfaceScope::InScope => "In-Scope Surfaces",
                SurfaceScope::OutOfScope => "Out-of-Scope Surfaces",
            },
        },
        [list],
        Some(add_button),
        theme,
    )
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
                .font(view_title_font())
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
                if spec.tab == CreationTab::Surfaces
                    && !crate::extensions_view::surfaces_enabled_for_creation(state)
                {
                    continue;
                }
                tabs = tabs.child(tab_button(spec, state, theme));
            }
            tabs
        });
    let forms = match state.workspace_draft.tab {
        CreationTab::Import => import(state, theme),
        CreationTab::Foundation => foundation(state, theme),
        CreationTab::Surfaces => surfaces(state, theme),
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
    if !state.workspace_create_error.is_empty() {
        page = page.child(
            text(state.workspace_create_error.clone())
                .key("workspace-create-error")
                .font(ui_font(14.0, 400.0))
                .color(color("#e07676")),
        );
    }
    let mut actions = row()
        .key("workspace-creation-actions")
        .w_full()
        .gap(8.0)
        .child(el().grow(1.0));
    let cancel = form_button("Cancel", theme).key("workspace-cancel");
    actions = actions.child(
        if state.workspace_create_busy || state.workspace_draft.import_pending_id.is_some() {
            cancel
        } else {
            cancel.on_press(request_cancel)
        },
    );
    if state
        .workspace_draft
        .tab
        .next(crate::extensions_view::surfaces_enabled_for_creation(state))
        .is_some()
    {
        let next = form_button(
            if state.workspace_draft.import_pending_id.is_some() {
                "Preparing…"
            } else {
                "Next"
            },
            theme,
        )
        .key("workspace-next");
        actions = actions.child(if state.workspace_draft.import_pending_id.is_some() {
            next
        } else {
            next.on_press_tx(advance)
        });
    } else {
        let ready = matches!(state.connection, crate::ConnectionStatus::Connected { .. })
            && !state.workspace_create_busy;
        actions = actions.child(if ready {
            form_button("Create Workspace", theme)
                .key("workspace-create")
                .on_press(crate::start_workspace_create)
        } else {
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
                    text(if state.workspace_create_busy {
                        "Creating…"
                    } else {
                        "Create Workspace"
                    })
                    .font(ui_font(14.0, 550.0))
                    .color(color("#777777")),
                )
        });
    }
    page = page.child(actions);
    if state.workspace_draft.tab == CreationTab::Rules
        && !matches!(state.connection, crate::ConnectionStatus::Connected { .. })
    {
        page = page.child(
            text("Connect to the app-server to create this workspace.")
                .key("workspace-create-note")
                .font(ui_font(13.0, 400.0))
                .color(color("#969696")),
        );
    }
    page
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_surfaces_keep_short_draft_ids_and_required_definitions() {
        let mut draft = WorkspaceDraft {
            name: TextEdit::new("Example Workspace"),
            directory: TextEdit::new(
                std::env::temp_dir()
                    .join("example-workspace")
                    .to_string_lossy(),
            ),
            ..Default::default()
        };
        draft.open_surface_dialog(SurfaceScope::InScope);
        draft.surface_value = TextEdit::new("example repository");
        draft.add_surface();
        assert_eq!(draft.surfaces[0].id, "M1");
        draft.open_surface_dialog(SurfaceScope::InScope);
        draft.surface_kind = SurfaceKind::Other;
        draft.surface_value = TextEdit::new("src/parser");
        draft.surface_type_id = TextEdit::new("code.module");
        draft.surface_locator_kind = TextEdit::new("repository_path");
        draft.surface_description = TextEdit::new("Example parser module.");
        draft.add_surface();
        let request = crate::workspaces::request_from_draft(&draft).unwrap();
        assert_eq!(request.surfaces[1].id, "M2");
        assert_eq!(
            request.surfaces[1].definition.as_ref().unwrap().description,
            "Example parser module."
        );
    }

    #[test]
    fn imported_draft_opens_general_with_model_surfaces_and_rules() {
        use beale_contracts::v1::{
            SurfaceDefinition, SurfaceLocator, SurfaceScope as WireScope, WorkspaceImportResult,
            WorkspaceSurfaceInput,
        };
        let mut draft = WorkspaceDraft {
            import_source: ImportSource::CustomInstructions,
            ..WorkspaceDraft::default()
        };
        draft
            .apply_import(WorkspaceImportResult {
                name: "Example Workspace".to_owned(),
                guidance: "Follow authorized scope.".to_owned(),
                surfaces: vec![WorkspaceSurfaceInput {
                    id: "draft-domain".to_owned(),
                    definition: Some(SurfaceDefinition {
                        type_id: "domain".to_owned(),
                        name: "Example domain".to_owned(),
                        locator: Some(SurfaceLocator {
                            kind: "domain".to_owned(),
                            value: "example.test".to_owned(),
                        }),
                        scope: WireScope::InScope as i32,
                        description: "Synthetic test scope.".to_owned(),
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
                rules: vec!["Stay within scope.".to_owned()],
            })
            .unwrap();
        assert_eq!(draft.name.text, "Example Workspace");
        assert!(draft.directory.text.is_empty());
        assert_eq!(draft.surfaces[0].created_by, SurfaceCreator::Agent);
        assert_eq!(draft.surfaces[0].id, "draft-domain");
        assert_eq!(
            draft.surfaces[0].definition.as_ref().unwrap().description,
            "Synthetic test scope."
        );
        draft.directory = TextEdit::new(
            std::env::temp_dir()
                .join("example-workspace")
                .to_string_lossy(),
        );
        let request = crate::workspaces::request_from_draft(&draft).unwrap();
        assert_eq!(
            request.surfaces[0].definition.as_ref().unwrap().scope_state,
            beale_contracts::v1::SurfaceScopeState::Candidate as i32
        );
        assert_eq!(draft.rules, ["Stay within scope."]);
        assert!(draft.validate_tab(CreationTab::Surfaces).is_none());
    }

    #[test]
    fn home_paths_are_compact_only_for_display() {
        let Some(home) = home_directory() else { return };
        let path = home.join("example-workspace");
        assert_eq!(
            display_directory(&path.to_string_lossy()),
            "~/example-workspace"
        );
        if cfg!(windows) {
            assert_eq!(
                display_directory(&format!(r"\\?\{}", path.to_string_lossy())),
                "~/example-workspace"
            );
        }
        let mut draft = WorkspaceDraft::default();
        draft.open_basic_dialog(BasicEdit::Directory);
        draft.basic_dialog_value = TextEdit::new("~/example-workspace");
        draft.apply_basic_dialog();
        assert_eq!(std::path::Path::new(&draft.directory.text), path);
        assert!(draft.basic_dialog.is_none());
    }
}
