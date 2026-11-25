use crate::design::FadeText;
use beale_contracts::v1::{
    AuthMethod, GoalStatus, ReasoningEffort, SessionRole, SessionStreamKind, SessionToolSummary,
};
use rune::{
    Align, Color, El, Key, KeyEvent, ScrollSpec, Side, TextEdit, Transition, Tx, auto_grow, col,
    el, icon, row, text, text_input,
};
use rune_kit::{Markdown, MarkdownStyle};

use crate::{
    Provider, SelectionMenu, State, ThemeStyle, catalog_index, effective_model_method, form_button,
    icons::{
        ARROW_RIGHT, BRAIN, CHECK, CHEVRON_DOWN, CHEVRON_RIGHT, CHEVRON_UP, PEN, REPEAT, STOP,
        TARGET, TERMINAL, WRENCH,
    },
    in_place_menu, input_history, load_tool_detail_page, modal_dialog, selection_button,
    settings_form_card, start_session_turn, toggle_selection_menu, toggle_tool_detail, ui_font,
    workspace_creation::{field, row_with_control},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModelSelection {
    pub(crate) provider: Provider,
    pub(crate) method: AuthMethod,
    pub(crate) id: String,
}

pub(crate) struct GraphEditor {
    pub(crate) role: beale_graphs::ProfileId,
    pub(crate) models: Vec<ModelSelection>,
    pub(crate) model_menu: Option<GraphModelMenu>,
}

pub(crate) struct GraphModelMenu {
    provider: Option<ModelSelection>,
    navigation: ModelMenuNavigation,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Effort {
    Off,
    Minimal,
    Low,
    Medium,
    #[default]
    High,
    XHigh,
    Max,
}

impl Effort {
    const ALL: [Self; 7] = [
        Self::Off,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::XHigh,
        Self::Max,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Minimal => "Minimal",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::XHigh => "XHigh",
            Self::Max => "Max",
        }
    }

    pub(crate) fn wire(self) -> ReasoningEffort {
        match self {
            Self::Off => ReasoningEffort::None,
            Self::Minimal => ReasoningEffort::Minimal,
            Self::Low => ReasoningEffort::Low,
            Self::Medium => ReasoningEffort::Medium,
            Self::High => ReasoningEffort::High,
            Self::XHigh => ReasoningEffort::Xhigh,
            Self::Max => ReasoningEffort::Max,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum RepeatPreset {
    #[default]
    None,
    Minute,
    Hour,
    Day,
    Week,
    Month,
}

impl RepeatPreset {
    pub(crate) const ALL: [Self; 6] = [
        Self::None,
        Self::Minute,
        Self::Hour,
        Self::Day,
        Self::Week,
        Self::Month,
    ];

    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minute => "minute",
            Self::Hour => "hour",
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::None => "No Repeat",
            Self::Minute => "Every Minute",
            Self::Hour => "Every Hour",
            Self::Day => "Every Day",
            Self::Week => "Every Week",
            Self::Month => "Every Month",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum MenuSection {
    Provider,
    #[default]
    Model,
    Effort,
}

#[derive(Default)]
pub(crate) struct ModelMenuNavigation {
    pub(crate) section: MenuSection,
    cursor: usize,
    reveal_pending: bool,
}

struct ModelChoice {
    selection: ModelSelection,
    name: String,
    provider_default: bool,
}

struct MenuOption {
    label: String,
    selected: bool,
    choice: MenuChoice,
}

enum MenuChoice {
    Model(ModelSelection),
    Effort(Effort),
    RetryModels(Provider, AuthMethod),
    LoadingModels,
}

fn choices(state: &State) -> Vec<ModelChoice> {
    let mut choices: Vec<ModelChoice> = Provider::ALL
        .into_iter()
        .filter_map(|provider| {
            let status = &state.auth_status[provider.index()];
            let method = effective_model_method(status);
            let configured = match method {
                AuthMethod::Subscription => status.subscription,
                AuthMethod::ApiKey => status.api_key,
                _ => false,
            };
            configured
                .then(|| state.model_catalogs[catalog_index(provider, method)].as_ref())
                .flatten()
                .map(|catalog| (provider, method, catalog))
        })
        .flat_map(|(provider, method, catalog)| {
            catalog.models.iter().map(move |model| ModelChoice {
                selection: ModelSelection {
                    provider,
                    method,
                    id: model.id.clone(),
                },
                name: model.name.clone(),
                provider_default: model.id == catalog.lead,
            })
        })
        .collect();
    for provider in state
        .custom_providers
        .iter()
        .filter(|provider| provider.api_key_configured)
    {
        for model in &provider.models {
            choices.push(ModelChoice {
                selection: ModelSelection {
                    provider: Provider::Custom,
                    method: AuthMethod::ApiKey,
                    id: format!("{}::{}", provider.id, model.id),
                },
                name: model.display_name.clone(),
                provider_default: model.id == provider.lead_model,
            });
        }
    }
    choices
}

fn has_configured_provider(state: &State) -> bool {
    state
        .auth_status
        .iter()
        .any(|status| status.subscription || status.api_key)
        || state
            .custom_providers
            .iter()
            .any(|provider| provider.api_key_configured)
}

fn custom_provider_id(selection: &ModelSelection) -> Option<&str> {
    (selection.provider == Provider::Custom)
        .then(|| selection.id.split_once("::").map(|(id, _)| id))
        .flatten()
}

fn provider_label<'a>(state: &'a State, selection: &ModelSelection) -> &'a str {
    if let Some(id) = custom_provider_id(selection) {
        state
            .custom_providers
            .iter()
            .find(|provider| provider.id == id)
            .map_or("Custom", |provider| provider.name.as_str())
    } else {
        selection.provider.name()
    }
}

fn selected<'a>(state: &State, choices: &'a [ModelChoice]) -> Option<&'a ModelChoice> {
    state
        .new_research_model
        .as_ref()
        .and_then(|selection| choices.iter().find(|choice| &choice.selection == selection))
        .or_else(|| default_choice(state, choices))
}

fn default_choice<'a>(state: &State, choices: &'a [ModelChoice]) -> Option<&'a ModelChoice> {
    if let Some(provider) = state.lead_provider
        && provider != Provider::Custom
        && {
            let status = &state.auth_status[provider.index()];
            status.subscription || status.api_key
        }
        && !choices
            .iter()
            .any(|choice| choice.selection.provider == provider)
    {
        return None;
    }
    state
        .lead_provider
        .and_then(|provider| {
            choices
                .iter()
                .find(|choice| {
                    choice.selection.provider == provider
                        && choice.provider_default
                        && (provider != Provider::Custom
                            || custom_provider_id(&choice.selection)
                                == Some(state.lead_custom_provider_id.as_str()))
                })
                .or_else(|| {
                    choices.iter().find(|choice| {
                        choice.selection.provider == provider
                            && (provider != Provider::Custom
                                || custom_provider_id(&choice.selection)
                                    == Some(state.lead_custom_provider_id.as_str()))
                    })
                })
        })
        .or_else(|| choices.iter().find(|choice| choice.provider_default))
        .or_else(|| choices.first())
}

pub(crate) fn default_model(state: &State) -> Option<ModelSelection> {
    let choices = choices(state);
    default_choice(state, &choices).map(|choice| choice.selection.clone())
}

pub(crate) fn selected_model(state: &State) -> Option<ModelSelection> {
    let choices = choices(state);
    selected(state, &choices).map(|choice| choice.selection.clone())
}

fn provider_options(
    state: &State,
    choices: &[ModelChoice],
    selected: Option<&ModelSelection>,
) -> Vec<MenuOption> {
    let mut options: Vec<_> = Provider::ALL
        .into_iter()
        .filter_map(|provider| {
            let available = choices
                .iter()
                .find(|choice| choice.selection.provider == provider && choice.provider_default)
                .or_else(|| {
                    choices
                        .iter()
                        .find(|choice| choice.selection.provider == provider)
                })
                .map(|choice| MenuOption {
                    label: provider.name().to_owned(),
                    selected: selected.is_some_and(|selection| selection.provider == provider),
                    choice: MenuChoice::Model(choice.selection.clone()),
                });
            if available.is_some() {
                return available;
            }
            let status = &state.auth_status[provider.index()];
            let method = effective_model_method(status);
            let configured = match method {
                AuthMethod::Subscription => status.subscription,
                AuthMethod::ApiKey => status.api_key,
                _ => false,
            };
            configured.then(|| {
                let loading = state.model_loading[catalog_index(provider, method)];
                MenuOption {
                    label: format!(
                        "{} · {}",
                        provider.name(),
                        if loading {
                            "Loading models…"
                        } else {
                            "Retry models"
                        }
                    ),
                    selected: false,
                    choice: if loading {
                        MenuChoice::LoadingModels
                    } else {
                        MenuChoice::RetryModels(provider, method)
                    },
                }
            })
        })
        .collect();
    for provider in state.custom_providers.iter() {
        if let Some(choice) = choices.iter().find(|choice| {
            choice.selection.provider == Provider::Custom
                && custom_provider_id(&choice.selection) == Some(provider.id.as_str())
                && choice.provider_default
        }) {
            options.push(MenuOption {
                label: provider.name.clone(),
                selected: selected.is_some_and(|selection| {
                    custom_provider_id(selection) == Some(provider.id.as_str())
                }),
                choice: MenuChoice::Model(choice.selection.clone()),
            });
        }
    }
    options
}

fn same_model_provider(first: &ModelSelection, second: &ModelSelection) -> bool {
    first.provider == second.provider
        && first.method == second.method
        && custom_provider_id(first) == custom_provider_id(second)
}

fn model_options(
    choices: Vec<ModelChoice>,
    provider: Option<&ModelSelection>,
    is_selected: impl Fn(&ModelSelection) -> bool,
) -> Vec<MenuOption> {
    choices
        .into_iter()
        .filter(|choice| {
            provider.is_some_and(|model| same_model_provider(&choice.selection, model))
        })
        .map(|choice| MenuOption {
            label: choice.name,
            selected: is_selected(&choice.selection),
            choice: MenuChoice::Model(choice.selection),
        })
        .collect()
}

fn options(state: &State, section: MenuSection) -> Vec<MenuOption> {
    let choices = choices(state);
    let selected = selected(state, &choices).map(|choice| choice.selection.clone());
    match section {
        MenuSection::Provider => provider_options(state, &choices, selected.as_ref()),
        MenuSection::Model => model_options(choices, selected.as_ref(), |model| {
            selected.as_ref() == Some(model)
        }),
        MenuSection::Effort => Effort::ALL
            .into_iter()
            .map(|effort| MenuOption {
                label: effort.label().to_owned(),
                selected: state.new_research_effort == effort,
                choice: MenuChoice::Effort(effort),
            })
            .collect(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ModelMenuTarget {
    Composer,
    Graph,
}

impl ModelMenuTarget {
    fn prefix(self) -> &'static str {
        match self {
            Self::Composer => "composer",
            Self::Graph => "graph-editor",
        }
    }

    fn navigation(self, state: &State) -> Option<&ModelMenuNavigation> {
        match self {
            Self::Composer => state
                .composer_menu_open
                .then_some(&state.composer_menu_navigation),
            Self::Graph => Some(&state.graph_editor.as_ref()?.model_menu.as_ref()?.navigation),
        }
    }

    fn navigation_mut(self, state: &mut State) -> Option<&mut ModelMenuNavigation> {
        match self {
            Self::Composer => state
                .composer_menu_open
                .then_some(&mut state.composer_menu_navigation),
            Self::Graph => Some(&mut state.graph_editor.as_mut()?.model_menu.as_mut()?.navigation),
        }
    }

    fn options(self, state: &State, section: MenuSection) -> Vec<MenuOption> {
        match self {
            Self::Composer => options(state, section),
            Self::Graph => {
                let editor = state.graph_editor.as_ref().expect("menu requires editor");
                let provider = editor
                    .model_menu
                    .as_ref()
                    .and_then(|menu| menu.provider.as_ref());
                let choices = choices(state);
                match section {
                    MenuSection::Provider => provider_options(state, &choices, provider),
                    MenuSection::Model => {
                        model_options(choices, provider, |model| editor.models.contains(model))
                    }
                    MenuSection::Effort => Vec::new(),
                }
            }
        }
    }

    fn close(self, state: &mut State) {
        match self {
            Self::Composer => state.composer_menu_open = false,
            Self::Graph => {
                if let Some(editor) = &mut state.graph_editor {
                    editor.model_menu = None;
                }
            }
        }
    }

    fn set_cursor(self, state: &mut State, cursor: usize, reveal: bool) {
        if let Some(menu) = self.navigation_mut(state) {
            menu.cursor = cursor;
            menu.reveal_pending |= reveal;
        }
    }

    fn reset_cursor(self, state: &mut State, section: MenuSection) {
        let cursor = self
            .options(state, section)
            .iter()
            .position(|option| option.selected)
            .unwrap_or(0);
        self.set_cursor(state, cursor, true);
    }

    fn set_section(self, state: &mut State, section: MenuSection) {
        if let Some(menu) = self.navigation_mut(state)
            && menu.section != section
        {
            menu.section = section;
            self.reset_cursor(state, section);
        }
    }

    fn select(self, state: &mut State, index: usize) {
        let Some(section) = self.navigation(state).map(|menu| menu.section) else {
            return;
        };
        let Some(option) = self.options(state, section).into_iter().nth(index) else {
            return;
        };
        match option.choice {
            MenuChoice::Model(selection) if self == Self::Graph => {
                let editor = state.graph_editor.as_mut().expect("menu requires editor");
                if section == MenuSection::Provider {
                    editor.model_menu.as_mut().expect("menu is open").provider = Some(selection);
                    self.set_section(state, MenuSection::Model);
                } else if editor.role.is_top_level() {
                    editor.models.clear();
                    editor.models.push(selection);
                    self.close(state);
                } else if let Some(index) = editor.models.iter().position(|item| *item == selection)
                {
                    if editor.models.len() > 1 {
                        editor.models.remove(index);
                    }
                } else if editor.models.len() < editor.role.max_models() {
                    editor.models.push(selection);
                }
                if section == MenuSection::Model {
                    let editor = state.graph_editor.as_ref().expect("menu requires editor");
                    let role = editor.role;
                    let models = editor.models.clone();
                    if Some(role) == beale_graphs::entry_role(&state.new_research_graph_id)
                        || (state.new_research_graph_id.is_empty()
                            && role == beale_graphs::ProfileId::Main)
                    {
                        state.new_research_model = models.first().cloned();
                    }
                    state.new_research_graph_models.insert(role, models);
                }
                return;
            }
            MenuChoice::Model(selection) => {
                state.new_research_model = Some(selection.clone());
                if let Some(models) = state
                    .new_research_graph_models
                    .get_mut(&beale_graphs::ProfileId::Main)
                {
                    models.retain(|item| *item != selection);
                    models.insert(0, selection);
                    models.truncate(beale_graphs::ProfileId::Main.max_models());
                }
            }
            MenuChoice::Effort(effort) => state.new_research_effort = effort,
            MenuChoice::RetryModels(provider, method) => {
                crate::start_model_list(state, provider, method);
                if self == Self::Graph {
                    return;
                }
            }
            MenuChoice::LoadingModels => return,
        }
        self.close(state);
    }

    fn key(self, state: &mut State, event: &KeyEvent) -> bool {
        let Some(menu) = self.navigation(state) else {
            return false;
        };
        let section = menu.section;
        let count = self.options(state, section).len();
        let cursor = menu.cursor.min(count.saturating_sub(1));
        match event.key {
            Key::Escape => self.close(state),
            Key::Left => self.set_section(
                state,
                match section {
                    MenuSection::Provider | MenuSection::Model => MenuSection::Provider,
                    MenuSection::Effort => MenuSection::Model,
                },
            ),
            Key::Right => self.set_section(
                state,
                match section {
                    MenuSection::Provider => MenuSection::Model,
                    MenuSection::Model if self == Self::Graph => MenuSection::Model,
                    MenuSection::Model | MenuSection::Effort => MenuSection::Effort,
                },
            ),
            Key::Down if count > 0 => self.set_cursor(state, (cursor + 1) % count, true),
            Key::Up if count > 0 => self.set_cursor(state, (cursor + count - 1) % count, true),
            Key::Home => self.set_cursor(state, 0, true),
            Key::End if count > 0 => self.set_cursor(state, count - 1, true),
            Key::Enter if count > 0 => self.select(state, cursor),
            _ => return false,
        }
        true
    }
}

pub(crate) fn toggle_menu(state: &mut State) {
    if state.composer_menu_open {
        ModelMenuTarget::Composer.close(state);
        return;
    }
    if choices(state).is_empty() && !has_configured_provider(state) {
        return;
    }
    state.selection_menu = None;
    state.composer_menu_open = true;
    let section = if selected_model(state).is_none() {
        MenuSection::Provider
    } else {
        MenuSection::Model
    };
    state.composer_menu_navigation.section = section;
    ModelMenuTarget::Composer.reset_cursor(state, section);
}

fn toggle_graph_model_menu(state: &mut State) {
    let Some(editor) = state.graph_editor.as_ref() else {
        return;
    };
    if editor.model_menu.is_some() {
        ModelMenuTarget::Graph.close(state);
        return;
    }
    let available = choices(state);
    let provider = editor
        .models
        .iter()
        .find(|model| {
            available
                .iter()
                .any(|choice| same_model_provider(&choice.selection, model))
        })
        .cloned()
        .or_else(|| {
            selected(state, &available)
                .or(available.first())
                .map(|choice| choice.selection.clone())
        });
    let section = if provider.is_some() {
        MenuSection::Model
    } else {
        MenuSection::Provider
    };
    let editor = state.graph_editor.as_mut().expect("editor is open");
    editor.model_menu = Some(GraphModelMenu {
        provider,
        navigation: ModelMenuNavigation {
            section,
            ..Default::default()
        },
    });
    ModelMenuTarget::Graph.reset_cursor(state, section);
}

pub(crate) fn menu_key(state: &mut State, event: &KeyEvent, _: &mut Tx) -> bool {
    ModelMenuTarget::Composer.key(state, event)
}

pub(crate) fn graph_model_menu_key(state: &mut State, event: &KeyEvent) -> bool {
    ModelMenuTarget::Graph.key(state, event)
}

fn section_row(
    target: ModelMenuTarget,
    section: MenuSection,
    label: &'static str,
    value: String,
    active: bool,
    theme: ThemeStyle<'_>,
) -> El {
    row()
        .key(format!(
            "{}-menu-{}",
            target.prefix(),
            match section {
                MenuSection::Provider => "provider",
                MenuSection::Model => "model",
                MenuSection::Effort => "effort",
            }
        ))
        .role("menuitem")
        .label(label)
        .w_full()
        .h(34.0)
        .px(10.0)
        .center_y()
        .gap(8.0)
        .radius(7.0)
        .fill(if active {
            theme.color("#383838")
        } else {
            Color::TRANSPARENT
        })
        .hover(theme.color("#424242"))
        .on_hover(move |state: &mut State| target.set_section(state, section))
        .on_press(move |state: &mut State| target.set_section(state, section))
        .child(
            text(label)
                .w(68.0)
                .font(ui_font(13.5, 450.0))
                .color(theme.color("#b8b8b8")),
        )
        .child(
            text(value)
                .grow(1.0)
                .min_w(0.0)
                .fade_truncate()
                .font(ui_font(13.5, 450.0))
                .color(theme.color("#eeeeee")),
        )
        .child(icon(&CHEVRON_RIGHT, 13.0).color(theme.color("#969696")))
}

pub(crate) fn model_menu(state: &State, theme: ThemeStyle<'_>) -> El {
    render_model_menu(state, ModelMenuTarget::Composer, theme)
}

fn render_model_menu(state: &State, target: ModelMenuTarget, theme: ThemeStyle<'_>) -> El {
    let prefix = target.prefix();
    let is_graph = target == ModelMenuTarget::Graph;
    let navigation = target.navigation(state).expect("menu is open");
    let choices = choices(state);
    let (provider_name, model_name, multiple_models) = if is_graph {
        let editor = state.graph_editor.as_ref().expect("menu requires editor");
        let provider_name = editor
            .model_menu
            .as_ref()
            .and_then(|menu| menu.provider.as_ref())
            .map_or("None", |choice| provider_label(state, choice));
        (
            provider_name,
            graph_editor_model_label(editor, &choices),
            !editor.role.is_top_level(),
        )
    } else {
        let selected = selected(state, &choices);
        let provider_name = selected.map_or_else(
            || state.lead_provider.map_or("None", Provider::name),
            |choice| provider_label(state, &choice.selection),
        );
        let model_name = selected.map_or("None", |choice| choice.name.as_str());
        (provider_name, model_name.to_owned(), false)
    };
    let mut main = col()
        .key(format!("{prefix}-model-main"))
        .role("menu")
        .label(if is_graph {
            "Graph node model settings"
        } else {
            "Lead model settings"
        })
        .w(250.0)
        .padding(8.0)
        .gap(2.0)
        .child(section_row(
            target,
            MenuSection::Provider,
            "Provider",
            provider_name.to_owned(),
            navigation.section == MenuSection::Provider,
            theme,
        ))
        .child(section_row(
            target,
            MenuSection::Model,
            "Model",
            model_name,
            navigation.section == MenuSection::Model,
            theme,
        ));
    if !is_graph {
        main = main.child(section_row(
            target,
            MenuSection::Effort,
            "Effort",
            state.new_research_effort.label().to_owned(),
            navigation.section == MenuSection::Effort,
            theme,
        ));
    }
    let main = in_place_menu::popup_surface(main, theme.color("#242424"));
    let options = target.options(state, navigation.section);
    let menu_height = if is_graph { 236.0 } else { 300.0 };
    let height = (options.len() as f32 * 36.0 + 12.0).min(menu_height);
    let mut list = col().w_full().gap(2.0);
    if is_graph && options.is_empty() {
        list = list.child(
            text(if navigation.section == MenuSection::Model {
                "Choose a provider to see its models."
            } else {
                "Connect a provider to choose models for this role."
            })
            .padding(8.0)
            .wrap()
            .font(ui_font(13.0, 400.0))
            .color(theme.color("#aaaaaa")),
        );
    }
    let role = if multiple_models && navigation.section == MenuSection::Model {
        "menuitemcheckbox"
    } else {
        "menuitemradio"
    };
    for (index, option) in options.into_iter().enumerate() {
        list = list.child(
            row()
                .key(format!("{prefix}-menu-option-{index}"))
                .role(role)
                .checked(option.selected)
                .label(option.label.clone())
                .w_full()
                .h(34.0)
                .px(10.0)
                .center_y()
                .gap(6.0)
                .radius(7.0)
                .fill(if index == navigation.cursor {
                    theme.color("#383838")
                } else {
                    Color::TRANSPARENT
                })
                .hover(theme.color("#424242"))
                .on_hover(move |state: &mut State| target.set_cursor(state, index, false))
                .on_press(move |state: &mut State| target.select(state, index))
                .child(
                    text(option.label)
                        .grow(1.0)
                        .min_w(0.0)
                        .fade_truncate()
                        .font(ui_font(13.5, if option.selected { 550.0 } else { 400.0 }))
                        .color(theme.color("#eeeeee")),
                )
                .child(if option.selected {
                    icon(&CHECK, 14.0).color(theme.color("#c8c8c8"))
                } else {
                    el().size(14.0, 14.0)
                }),
        );
    }
    let submenu = in_place_menu::popup_surface(
        col()
            .key(format!("{prefix}-model-submenu"))
            .role("menu")
            .label(match navigation.section {
                MenuSection::Provider => "Provider options",
                MenuSection::Model => "Model options",
                MenuSection::Effort => "Effort options",
            })
            .w(230.0)
            .h(if is_graph { height.max(66.0) } else { height })
            .min_h(0.0)
            .padding(6.0)
            .child(
                col()
                    .key(format!("{prefix}-model-options-scroll"))
                    .w_full()
                    .grow(1.0)
                    .min_h(0.0)
                    .scroll(ScrollSpec {
                        stick_bottom: false,
                    })
                    .child(list),
            ),
        theme.color("#242424"),
    );
    let (anchor, side, align, gap) = if is_graph {
        (
            format!(
                "graph-editor-model-control-{}",
                state
                    .graph_editor
                    .as_ref()
                    .expect("menu requires editor")
                    .role
                    .id()
            ),
            Side::Bottom,
            Align::End,
            4.0,
        )
    } else {
        (
            "new-research-model-control".to_owned(),
            Side::Top,
            Align::Start,
            6.0,
        )
    };
    let menu = row()
        .key(format!("{prefix}-model-menu"))
        .w(486.0)
        .h(if is_graph {
            height.max(86.0)
        } else {
            menu_height
        })
        .min_h(0.0)
        .gap(6.0)
        .anchored_fit(anchor, side, align, gap)
        .modal(true, move |state: &mut State| target.close(state))
        .on_layout_tx(move |state: &mut State, _, tx| {
            if let Some(navigation) = target.navigation_mut(state)
                && std::mem::take(&mut navigation.reveal_pending)
            {
                tx.scroll_into_view_in(
                    format!("{prefix}-model-options-scroll"),
                    format!("{prefix}-menu-option-{}", navigation.cursor),
                );
            }
        });
    if is_graph {
        menu.child(main.self_start()).child(submenu.self_start())
    } else {
        menu.child(main.self_end()).child(submenu.self_end())
    }
}

fn is_web_link(destination: &str) -> bool {
    reqwest::Url::parse(destination)
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host().is_some())
}

pub(crate) fn transcript_markdown(
    content: String,
    theme: ThemeStyle<'_>,
    streaming: bool,
    muted: bool,
) -> El {
    transcript_markdown_sized(content, theme, streaming, muted, false)
}

fn transcript_markdown_sized(
    content: String,
    theme: ThemeStyle<'_>,
    streaming: bool,
    muted: bool,
    compact: bool,
) -> El {
    let body = ui_font(if muted || compact { 13.0 } else { 14.0 }, 400.0);
    let heading_weight = if muted { 400.0 } else { 600.0 };
    let mut style = MarkdownStyle::neutral(theme.is_dark())
        .text(theme.color(if muted { "#969696" } else { "#eeeeee" }))
        .muted(theme.color("#969696"))
        .link(theme.color(if theme.is_dark() {
            "#a9c7eb"
        } else {
            "#355f96"
        }))
        .inline_code_bg(theme.color("#303030"))
        .code_block(theme.color("#191919"), theme.color("#323232"))
        .rule(theme.color("#323232"))
        .quote_rule(theme.color("#777777"))
        .body_font(body)
        .heading_font(
            1,
            ui_font(
                if muted {
                    16.0
                } else if compact {
                    17.0
                } else {
                    20.0
                },
                heading_weight,
            ),
        )
        .heading_font(
            2,
            ui_font(if muted || compact { 15.0 } else { 18.0 }, heading_weight),
        )
        .heading_font(
            3,
            ui_font(if muted || compact { 14.0 } else { 16.0 }, heading_weight),
        )
        .code_block_font(ui_font(if compact { 12.0 } else { 13.0 }, 400.0).mono());
    if muted {
        style = style.strong_weight(400.0);
    }
    Markdown::new(content)
        .streaming(streaming)
        .selectable(true)
        .on_link_tx(|_: &mut State, destination, tx| {
            if is_web_link(destination) {
                let _ = tx.open_url(destination);
            }
        })
        .render(&style)
}

fn role_models(state: &State, role: beale_graphs::ProfileId) -> Vec<ModelSelection> {
    let mut models: Vec<_> = state
        .new_research_graph_models
        .get(&role)
        .cloned()
        .map(|models| {
            let available = choices(state);
            models
                .into_iter()
                .filter(|model| available.iter().any(|choice| choice.selection == *model))
                .collect()
        })
        .unwrap_or_else(|| selected_model(state).into_iter().collect());
    if role.is_top_level() {
        models.truncate(role.max_models());
    }
    models
}

pub(crate) fn graph_model_config(state: &State) -> Vec<beale_contracts::v1::GraphNodeModel> {
    beale_graphs::graph_definition(&state.new_research_graph_id)
        .unwrap_or(&beale_graphs::SIMPLE_GRAPH)
        .nodes
        .iter()
        .flat_map(|role| {
            role_models(state, *role)
                .into_iter()
                .map(|model| beale_contracts::v1::GraphNodeModel {
                    node_id: role.id().to_owned(),
                    provider: model.provider.id() as i32,
                    method: model.method as i32,
                    model: model.id,
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

#[cfg(test)]
pub(crate) fn graph_model_label(state: &State, role: beale_graphs::ProfileId) -> String {
    let models = role_models(state, role);
    let choices = choices(state);
    let name = models.first().map_or("No model", |selection| {
        choices
            .iter()
            .find(|choice| choice.selection == *selection)
            .map_or(selection.id.as_str(), |choice| choice.name.as_str())
    });
    let extra = models.len().saturating_sub(1);
    if extra > 0 {
        format!("{name} + {extra}")
    } else {
        name.to_owned()
    }
}

fn open_graph_profile(state: &mut State, role: beale_graphs::ProfileId) {
    if state
        .graph_editor
        .as_ref()
        .is_some_and(|editor| editor.role == role && editor.model_menu.is_some())
    {
        ModelMenuTarget::Graph.close(state);
        return;
    }
    state.graph_editor = Some(GraphEditor {
        role,
        models: role_models(state, role),
        model_menu: None,
    });
    toggle_graph_model_menu(state);
}

fn graph_profiles(state: &State, top_level: bool) -> Vec<beale_graphs::ProfileId> {
    beale_graphs::graph_definition(&state.new_research_graph_id)
        .map_or_else(
            || vec![beale_graphs::ProfileId::Main],
            |graph| graph.nodes.to_vec(),
        )
        .into_iter()
        .filter(|role| role.is_top_level() == top_level)
        .collect()
}

fn model_summary(state: &State, roles: &[beale_graphs::ProfileId], theme: ThemeStyle<'_>) -> El {
    let mut models = Vec::new();
    for model in roles.iter().flat_map(|role| role_models(state, *role)) {
        if !models.contains(&model) {
            models.push(model);
        }
    }
    let count = models.len();
    let label = match count {
        0 if roles.is_empty() => "No subagents".to_owned(),
        0 => "Choose models".to_owned(),
        1 => choices(state)
            .iter()
            .find(|choice| choice.selection == models[0])
            .map_or_else(|| models[0].id.clone(), |choice| choice.name.clone()),
        _ => format!("{count} models selected"),
    };
    let mut summary = row().min_w(0.0).center_y().gap(7.0);
    if count == 1 {
        summary = summary.child(
            icon(models[0].provider.logo(), 15.0)
                .key("new-research-single-model-provider")
                .color(theme.color("#b8b8b8")),
        );
    }
    summary.child(
        text(label)
            .min_w(0.0)
            .fade_truncate()
            .font(ui_font(13.0, 450.0))
            .color(theme.color("#bdbdbd")),
    )
}

fn model_group_control(state: &State, top_level: bool, theme: ThemeStyle<'_>) -> El {
    let roles = graph_profiles(state, top_level);
    if let (true, [role]) = (top_level, roles.as_slice()) {
        return graph_model_control(state, *role, theme);
    }
    let key = if top_level {
        "new-scan-agent-models-edit"
    } else {
        "new-scan-subagent-models-edit"
    };
    let title = if top_level {
        "Edit agent models"
    } else {
        "Edit subagent models"
    };
    let mut edit = el()
        .key(key)
        .role("button")
        .label(title)
        .size(32.0, 32.0)
        .center()
        .radius(8.0)
        .hover(theme.color("#303030"))
        .child(icon(&PEN, 16.0).color(theme.color("#bdbdbd")));
    if !roles.is_empty() {
        edit = edit.on_press(move |state: &mut State| {
            state.graph_models_group = Some(top_level);
        });
    } else {
        edit = edit.opacity(0.45);
    }
    row()
        .min_w(0.0)
        .center_y()
        .gap(8.0)
        .child(model_summary(state, &roles, theme))
        .child(edit)
}

fn close_graph_models_group(state: &mut State) {
    state.graph_editor = None;
    state.graph_models_group = None;
}

fn graph_model_control(state: &State, role: beale_graphs::ProfileId, theme: ThemeStyle<'_>) -> El {
    let models = role_models(state, role);
    let selected = models.first();
    let label = match models.as_slice() {
        [] => "Choose models".to_owned(),
        [_] => choices(state)
            .iter()
            .find(|choice| choice.selection == models[0])
            .map_or_else(|| models[0].id.clone(), |choice| choice.name.clone()),
        _ => format!("{} models selected", models.len()),
    };
    let open = state
        .graph_editor
        .as_ref()
        .is_some_and(|editor| editor.role == role && editor.model_menu.is_some());
    row()
        .key(format!("graph-editor-model-control-{}", role.id()))
        .role("button")
        .label(format!("Choose {} models", role.label()))
        .h(34.0)
        .min_w(0.0)
        .max_w(190.0)
        .px(10.0)
        .center_y()
        .gap(6.0)
        .radius(8.0)
        .fill(theme.color(if open { "#303030" } else { "#282828" }))
        .border(1.0, theme.color("#353535"))
        .on_press(move |state: &mut State| open_graph_profile(state, role))
        .child(if models.len() == 1 {
            icon(selected.expect("one model").provider.logo(), 15.0).color(theme.color("#b8b8b8"))
        } else {
            el().size(15.0, 15.0)
        })
        .child(
            text(label)
                .min_w(0.0)
                .fade_truncate()
                .font(ui_font(13.0, 450.0))
                .color(theme.color("#e2e2e2")),
        )
        .child(
            icon(if open { &CHEVRON_UP } else { &CHEVRON_DOWN }, 13.0)
                .color(theme.color("#969696")),
        )
}

pub(crate) fn graph_models_group_dialog(state: &State, theme: ThemeStyle<'_>) -> [El; 2] {
    let top_level = state
        .graph_models_group
        .expect("group dialog requires a selection");
    let rows = graph_profiles(state, top_level).into_iter().map(|role| {
        let purpose = if role == beale_graphs::ProfileId::Main {
            "Leads the scan and coordinates specialist tasks."
        } else if role.is_top_level() {
            role.instruction()
                .split_once(". ")
                .map_or(role.instruction(), |(first, _)| first)
        } else {
            role.spawn_when()
        };
        row_with_control(
            role.label(),
            purpose,
            graph_model_control(state, role, theme),
            theme,
        )
        .key(format!("new-research-graph-node-{}", role.id()))
    });
    modal_dialog::render(
        modal_dialog::DialogSpec {
            key: "graph-models-group-dialog",
            title: if top_level {
                "Agent Models"
            } else {
                "Subagent Models"
            },
            width: 620.0,
            focus_return: if top_level {
                "new-scan-agent-models-edit"
            } else {
                "new-scan-subagent-models-edit"
            },
        },
        settings_form_card(rows, theme),
        row()
            .w_full()
            .child(el().grow(1.0))
            .child(form_button("Done", theme).on_press(close_graph_models_group)),
        theme,
        close_graph_models_group,
    )
}

fn graph_profile_form(state: &State, theme: ThemeStyle<'_>, available_width: f32) -> El {
    let selected = !state.new_research_graph_id.is_empty();
    let active = state.selection_menu == Some(SelectionMenu::SessionGraph);
    let label = beale_graphs::graph_definition(&state.new_research_graph_id)
        .map_or("No Subagents", |graph| graph.name);
    let graph = selection_button(
        label,
        active,
        |state: &mut State| toggle_selection_menu(state, SelectionMenu::SessionGraph),
        theme,
    )
    .key("new-research-graph-control")
    .role("button")
    .label(format!("Scan graph: {label}"));
    let mut rows = vec![
        row_with_control("Graph", "Choose the scan workflow.", graph, theme)
            .key("new-scan-graph-row"),
    ];
    let top_roles = graph_profiles(state, true);
    let top_count = top_roles.len();
    rows.push(
        row_with_control(
            if top_count == 1 {
                "Agent Model"
            } else {
                "Agent Models"
            },
            match top_roles.as_slice() {
                [beale_graphs::ProfileId::Main] => {
                    "Leads the scan and coordinates specialist tasks."
                }
                [role] => role
                    .instruction()
                    .split_once(". ")
                    .map_or(role.instruction(), |(first, _)| first),
                _ => "Models used by persistent scan agents.",
            },
            model_group_control(state, true, theme),
            theme,
        )
        .key("new-scan-agent-models-row"),
    );
    rows.push(
        row_with_control(
            "Subagent Models",
            "Models used by task workers.",
            model_group_control(state, false, theme),
            theme,
        )
        .key("new-scan-subagent-models-row"),
    );
    if beale_graphs::is_rotating_graph(&state.new_research_graph_id) {
        rows.push(
            row_with_control(
                "Minutes Per Stage",
                "Time before the scan moves to the next stage.",
                field(
                    &state.new_research_stage_duration_minutes,
                    "new-research-stage-duration-minutes",
                    "60",
                    theme,
                    |state| &mut state.new_research_stage_duration_minutes,
                )
                .w(86.0)
                .on_input_change(|state: &mut State, _| {
                    normalize_goal_limit(&mut state.new_research_stage_duration_minutes, 1_440);
                }),
                theme,
            )
            .key("new-scan-stage-duration-row"),
        );
    }
    settings_form_card(rows, theme)
        .w(available_width.clamp(180.0, 640.0))
        .self_center()
        .key(if !selected {
            "new-research-no-graph"
        } else if state.new_research_graph_id == beale_graphs::SIMPLE_ID {
            "new-research-simple-graph"
        } else {
            "new-research-flow-graph"
        })
}

fn graph_editor_model_label(editor: &GraphEditor, model_choices: &[ModelChoice]) -> String {
    match editor.models.as_slice() {
        [] => "Choose models".to_owned(),
        [selection] => model_choices
            .iter()
            .find(|choice| choice.selection == *selection)
            .map_or_else(|| "Choose models".to_owned(), |choice| choice.name.clone()),
        models => format!("{} models selected", models.len()),
    }
}

pub(crate) fn graph_editor_model_menu(state: &State, theme: ThemeStyle<'_>) -> El {
    render_model_menu(state, ModelMenuTarget::Graph, theme)
}

fn new_research_setup(state: &State, theme: ThemeStyle<'_>, _available_width: f32) -> El {
    let workspace_name = state
        .selected_research_workspace()
        .map_or("Select Workspace", |workspace| workspace.name.as_str());
    let mut workspace = row()
        .key("new-research-heading-workspace-control")
        .role("button")
        .label(format!("Choose workspace: {workspace_name}"))
        .help(workspace_name)
        .min_w(0.0)
        .center_y()
        .gap(6.0)
        .hover_group()
        .child(
            text(workspace_name)
                .min_w(0.0)
                .fade_truncate()
                .font(crate::view_title_font())
                .color(theme.color("#eeeeee"))
                .group_color(theme.color("#aaaaaa")),
        )
        .child(
            icon(&CHEVRON_DOWN, 15.0)
                .key("new-research-heading-workspace-chevron")
                .color(theme.color("#969696")),
        );
    if !state.workspaces.is_empty() {
        workspace = workspace.on_press(|state: &mut State| {
            toggle_selection_menu(state, SelectionMenu::ResearchWorkspaceHeading)
        });
    }
    let heading = row()
        .key("new-research-workspace-heading")
        .w_full()
        .min_w(0.0)
        .justify_center()
        .center_y()
        .gap(8.0)
        .child(
            text("Scan the security of")
                .key("new-research-heading-prompt")
                .role("heading")
                .font(crate::view_title_font())
                .color(theme.color("#eeeeee")),
        )
        .child(workspace);
    let mut content = col()
        .key("new-research-setup")
        .w_full()
        .max_w(750.0)
        .self_center()
        .gap(18.0)
        .px(20.0)
        .child(heading);
    if crate::extensions_view::graphs_enabled(state) {
        content = content.child(graph_profile_form(state, theme, _available_width));
    }
    col()
        .key("new-research-graph-scroll")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(col().w_full().min_h(0.0).py(28.0).child(content))
}

pub(crate) fn transcript_message(
    key: String,
    content: String,
    user: bool,
    streaming: bool,
    theme: ThemeStyle<'_>,
) -> El {
    transcript_message_sized(key, content, user, streaming, theme, false)
}

pub(crate) fn transcript_message_compact(
    key: String,
    content: String,
    user: bool,
    streaming: bool,
    theme: ThemeStyle<'_>,
) -> El {
    transcript_message_sized(key, content, user, streaming, theme, true)
}

fn transcript_message_sized(
    key: String,
    content: String,
    user: bool,
    streaming: bool,
    theme: ThemeStyle<'_>,
    compact: bool,
) -> El {
    let message = if user {
        text(content)
            .key(format!("{key}-text"))
            .font(ui_font(if compact { 13.0 } else { 14.0 }, 400.0))
            .color(theme.color("#eeeeee"))
            .wrap()
            .selectable(true)
            .selection_color(rune_kit::Kit::neutral(theme.is_dark()).selection())
    } else {
        transcript_markdown_sized(content, theme, streaming, false, compact)
            .key(format!("{key}-markdown"))
    };
    if user {
        row().key(key.clone()).w_full().child(el().grow(1.0)).child(
            col()
                .key(format!("{key}-bubble"))
                .max_w(if compact { 310.0 } else { 620.0 })
                .shrink()
                .px(if compact { 11.0 } else { 13.0 })
                .py(if compact { 8.0 } else { 10.0 })
                .radius(if compact { 12.0 } else { 14.0 })
                .continuous_corners()
                .fill(theme.color("#202020"))
                .child(message),
        )
    } else {
        col().key(key).w_full().child(message)
    }
}

fn webfetch_input(tool: &SessionToolSummary) -> Option<(&'static str, &str)> {
    if tool.name != "webfetch" {
        return None;
    }
    let body = tool
        .summary
        .strip_prefix("Web ")
        .or_else(|| tool.summary.split_once(": Web ").map(|(_, body)| body))?;
    if let Some(query) = body
        .strip_prefix("search ")
        .filter(|value| !value.is_empty())
    {
        return Some(("webfetch.query", query));
    }
    if let Some(url) = body.strip_prefix("view ").filter(|value| !value.is_empty()) {
        return Some(("webfetch.url", url));
    }
    // Earlier sessions stored only "Web <value>" without the input mode.
    if body.starts_with("https://") || body.starts_with("http://") {
        Some(("webfetch.url", body))
    } else if !body.is_empty() && body != "request" {
        Some(("webfetch.query", body))
    } else {
        None
    }
}

fn summary_detail_value<'a>(summary: &'a str, prefix: &str) -> Option<&'a str> {
    summary
        .strip_prefix(prefix)
        .or_else(|| {
            summary
                .split_once(": ")
                .and_then(|(_, rest)| rest.strip_prefix(prefix))
        })
        .filter(|value| !value.is_empty())
}

fn tool_series_kind(tool: &SessionToolSummary) -> Option<&'static str> {
    match tool.name.as_str() {
        "read" => Some("read"),
        "write" => Some("write"),
        "edit" => Some("edit"),
        "bash" | "powershell" => Some("command"),
        "webfetch" => webfetch_input(tool).map(|(kind, _)| kind),
        "surface_query" => Some("surface_query"),
        "surface_read" => Some("surface_read"),
        "surface_save" => Some("surface_save"),
        "surface_threat_model_edit" => Some("surface_threat_model_edit"),
        "findings_query" => Some("findings_query"),
        "findings_read" => Some("findings_read"),
        "findings_create" => Some("findings_create"),
        "findings_update" => Some("findings_update"),
        "findings_complete_state" => Some("findings_complete_state"),
        "findings_reject" => Some("findings_reject"),
        "memory_session_query" => Some("memory_session_query"),
        "memory_query" => Some("memory_query"),
        "memory_read" => Some("memory_read"),
        "memory_graph" => Some("memory_graph"),
        "runbook_query" => Some("runbook_query"),
        "runbook_get" => Some("runbook_get"),
        "runbook_create" => Some("runbook_create"),
        "runbook_edit" => Some("runbook_edit"),
        "runbook_run" => Some("runbook_run"),
        "runbook_status" => Some("runbook_status"),
        "graph_subagents_spawn" => Some("graph_subagents_spawn"),
        "graph_subagents_list" => Some("graph_subagents_list"),
        "graph_subagents_inspect" => Some("graph_subagents_inspect"),
        "graph_subagents_message" => Some("graph_subagents_message"),
        "graph_subagents_receive" => Some("graph_subagents_receive"),
        "graph_subagents_kill" => Some("graph_subagents_kill"),
        "graph_handoff" => Some("graph_handoff"),
        "agent_feedback" => Some("agent_feedback"),
        "goal_status" => Some("goal_status"),
        _ => None,
    }
}

pub(crate) fn series_tool(tool: &SessionToolSummary) -> bool {
    tool_series_kind(tool).is_some()
}

fn tool_single_label(tool: &SessionToolSummary, active: bool) -> String {
    if tool.name == "agent_feedback" {
        return if active {
            "Providing agent feedback"
        } else {
            "Provided agent feedback"
        }
        .to_owned();
    }
    if tool.name == "goal_status" {
        let verb = if active {
            "Marking goal status"
        } else {
            "Marked goal status"
        };
        return summary_detail_value(&tool.summary, "Goal status: ")
            .map_or_else(|| verb.to_owned(), |status| format!("{verb} as {status}"));
    }
    let graph_label = match tool.name.as_str() {
        "graph_subagents_spawn" => {
            let verb = if active { "Spawning" } else { "Spawned" };
            let count = summary_detail_value(&tool.summary, "Graph spawn count: ")
                .and_then(|value| value.parse::<usize>().ok());
            Some(count.map_or_else(
                || format!("{verb} subagents"),
                |count| format!("{verb} {count} subagents"),
            ))
        }
        "graph_subagents_list" => Some(
            if active {
                "Checking subagents"
            } else {
                "Checked subagents"
            }
            .to_owned(),
        ),
        "graph_subagents_inspect"
        | "graph_subagents_message"
        | "graph_subagents_kill"
        | "graph_subagents_receive"
        | "graph_handoff" => {
            let (prefix, present, past, suffix, fallback) = match tool.name.as_str() {
                "graph_subagents_inspect" => (
                    "Graph inspect role: ",
                    "Inspecting",
                    "Inspected",
                    " subagent",
                    "a subagent",
                ),
                "graph_subagents_message" => (
                    "Graph message role: ",
                    "Sending message to",
                    "Sent message to",
                    "",
                    "a subagent",
                ),
                "graph_subagents_receive" => (
                    "Graph receive role: ",
                    "Receiving message from",
                    "Received message from",
                    "",
                    "a subagent",
                ),
                "graph_subagents_kill" => (
                    "Graph kill role: ",
                    "Stopping",
                    "Stopped",
                    " subagent",
                    "a subagent",
                ),
                _ => (
                    "Graph handoff role: ",
                    "Handing off to",
                    "Handed off to",
                    " agent",
                    "an agent",
                ),
            };
            let verb = if active { present } else { past };
            let role = summary_detail_value(&tool.summary, prefix).or_else(|| {
                (tool.name == "graph_handoff")
                    .then(|| summary_detail_value(&tool.summary, "Hand off to "))
                    .flatten()
            });
            Some(role.map_or_else(
                || format!("{verb} {fallback}"),
                |role| format!("{verb} {role}{suffix}"),
            ))
        }
        _ => None,
    };
    if let Some(label) = graph_label {
        return label;
    }
    if let Some((kind, value)) = webfetch_input(tool) {
        return match (kind, active) {
            ("webfetch.query", true) => format!("Web searching \"{value}\""),
            ("webfetch.query", false) => format!("Web searched \"{value}\""),
            ("webfetch.url", true) => format!("Web viewing {value}"),
            _ => format!("Web viewed {value}"),
        };
    }
    if tool.name == "surface_query" {
        let verb = if active {
            "Searching surfaces"
        } else {
            "Searched surfaces"
        };
        return summary_detail_value(&tool.summary, "Surface query: ").map_or_else(
            || verb.to_owned(),
            |query| format!("{verb} for \"{query}\""),
        );
    }
    let surface_verb = match (tool.name.as_str(), active) {
        ("surface_read", true) => Some("Inspecting"),
        ("surface_read", false) => Some("Inspected"),
        ("surface_save", true) => Some("Persisting"),
        ("surface_save", false) => Some("Persisted"),
        ("surface_threat_model_edit", true) => Some("Modeling"),
        ("surface_threat_model_edit", false) => Some("Modeled"),
        _ => None,
    };
    if let Some(verb) = surface_verb {
        return summary_detail_value(&tool.summary, "Surface name: ").map_or_else(
            || format!("{verb} a surface"),
            |name| format!("{verb} surface of {name}"),
        );
    }
    if tool.name == "findings_query" {
        let verb = if active {
            "Searching findings"
        } else {
            "Searched findings"
        };
        return summary_detail_value(&tool.summary, "Finding query: ").map_or_else(
            || verb.to_owned(),
            |query| format!("{verb} for \"{query}\""),
        );
    }
    let finding_verb = match (tool.name.as_str(), active) {
        ("findings_read", true) => Some("Inspecting"),
        ("findings_read", false) => Some("Inspected"),
        ("findings_create", true) => Some("Creating"),
        ("findings_create", false) => Some("Created"),
        ("findings_update", true) => Some("Updating"),
        ("findings_update", false) => Some("Updated"),
        ("findings_complete_state", true) => Some("Transitioning"),
        ("findings_complete_state", false) => Some("Transitioned"),
        ("findings_reject", true) => Some("Rejecting"),
        ("findings_reject", false) => Some("Rejected"),
        _ => None,
    };
    if let Some(verb) = finding_verb {
        return summary_detail_value(&tool.summary, "Finding title: ").map_or_else(
            || format!("{verb} a finding"),
            |title| format!("{verb} finding of {title}"),
        );
    }
    if matches!(tool.name.as_str(), "memory_session_query" | "memory_query") {
        let (subject, prefix) = if tool.name == "memory_session_query" {
            ("scan", "Memory session query: ")
        } else {
            ("memory", "Memory query: ")
        };
        let verb = if active { "Searching" } else { "Searched" };
        return summary_detail_value(&tool.summary, prefix).map_or_else(
            || format!("{verb} {subject}"),
            |query| format!("{verb} {subject} for \"{query}\""),
        );
    }
    let memory_verb = match (tool.name.as_str(), active) {
        ("memory_read", true) => Some("Reading"),
        ("memory_read", false) => Some("Read"),
        ("memory_graph", true) => Some("Graphing"),
        ("memory_graph", false) => Some("Graphed"),
        _ => None,
    };
    if let Some(verb) = memory_verb {
        return summary_detail_value(&tool.summary, "Memory name: ").map_or_else(
            || format!("{verb} a memory"),
            |name| format!("{verb} memory of {name}"),
        );
    }
    if tool.name == "runbook_query" {
        let verb = if active {
            "Searching runbooks"
        } else {
            "Searched runbooks"
        };
        return summary_detail_value(&tool.summary, "Runbook query: ").map_or_else(
            || verb.to_owned(),
            |query| format!("{verb} for \"{query}\""),
        );
    }
    let runbook_verb = match (tool.name.as_str(), active) {
        ("runbook_get", true) => Some("Reading"),
        ("runbook_get", false) => Some("Read"),
        ("runbook_create", true) => Some("Creating"),
        ("runbook_create", false) => Some("Created"),
        ("runbook_edit", true) => Some("Editing"),
        ("runbook_edit", false) => Some("Edited"),
        ("runbook_run", true) => Some("Running"),
        ("runbook_run", false) => Some("Ran"),
        ("runbook_status", true) => Some("Checking"),
        ("runbook_status", false) => Some("Checked"),
        _ => None,
    };
    if let Some(verb) = runbook_verb {
        return summary_detail_value(&tool.summary, "Runbook title: ").map_or_else(
            || format!("{verb} a runbook"),
            |title| format!("{verb} runbook of {title}"),
        );
    }
    let (present, past, prefix) = match tool.name.as_str() {
        "read" => ("Reading", "Read", "Read "),
        "write" => ("Writing", "Wrote", "Write "),
        "edit" => ("Editing", "Edited", "Edit "),
        "bash" | "powershell" => ("Running", "Ran", "Run "),
        _ => {
            return if active {
                format!("{}…", tool.summary)
            } else {
                tool.summary.clone()
            };
        }
    };
    let value = tool.summary.strip_prefix(prefix).or_else(|| {
        tool.summary
            .split_once(": ")
            .and_then(|(_, rest)| rest.strip_prefix(prefix))
    });
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return tool.summary.clone();
    };
    format!("{} {value}", if active { present } else { past })
}

fn combined_tool_series_label(tools: &[&SessionToolSummary], active: bool) -> String {
    if tools.len() == 1 {
        return tool_single_label(tools[0], active);
    }
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for tool in tools {
        let Some(kind) = tool_series_kind(tool) else {
            return tool.summary.clone();
        };
        if let Some((_, count)) = counts.iter_mut().find(|(name, _)| *name == kind) {
            *count += 1;
        } else {
            counts.push((kind, 1));
        }
    }
    counts
        .into_iter()
        .enumerate()
        .map(|(index, (name, count))| {
            let (present, past, noun, plural) = match name {
                "read" => ("Reading", "Read", "file", "files"),
                "write" => ("Writing", "Wrote", "file", "files"),
                "edit" => ("Editing", "Edited", "file", "files"),
                "command" => ("Running", "Ran", "command", "commands"),
                "webfetch.query" => ("Web searching", "Web searched", "query", "queries"),
                "webfetch.url" => ("Web viewing", "Web viewed", "page", "pages"),
                "surface_query" => (
                    "Searching surfaces for",
                    "Searched surfaces for",
                    "query",
                    "queries",
                ),
                "surface_read" => ("Inspecting", "Inspected", "surface", "surfaces"),
                "surface_save" => ("Persisting", "Persisted", "surface", "surfaces"),
                "surface_threat_model_edit" => ("Modeling", "Modeled", "surface", "surfaces"),
                "findings_query" => (
                    "Searching findings for",
                    "Searched findings for",
                    "query",
                    "queries",
                ),
                "findings_read" => ("Inspecting", "Inspected", "finding", "findings"),
                "findings_create" => ("Creating", "Created", "finding", "findings"),
                "findings_update" => ("Updating", "Updated", "finding", "findings"),
                "findings_complete_state" => {
                    ("Transitioning", "Transitioned", "finding", "findings")
                }
                "findings_reject" => ("Rejecting", "Rejected", "finding", "findings"),
                "memory_session_query" => (
                    "Searching scan for",
                    "Searched scan for",
                    "query",
                    "queries",
                ),
                "memory_query" => (
                    "Searching memory for",
                    "Searched memory for",
                    "query",
                    "queries",
                ),
                "memory_read" => ("Reading", "Read", "memory", "memories"),
                "memory_graph" => ("Graphing", "Graphed", "memory", "memories"),
                "runbook_query" => (
                    "Searching runbooks for",
                    "Searched runbooks for",
                    "query",
                    "queries",
                ),
                "runbook_get" => ("Reading", "Read", "runbook", "runbooks"),
                "runbook_create" => ("Creating", "Created", "runbook", "runbooks"),
                "runbook_edit" => ("Editing", "Edited", "runbook", "runbooks"),
                "runbook_run" => ("Running", "Ran", "runbook", "runbooks"),
                "runbook_status" => ("Checking", "Checked", "runbook", "runbooks"),
                "graph_subagents_spawn" => ("Spawning", "Spawned", "subagent", "subagents"),
                "graph_subagents_list" => ("Checking", "Checked", "subagent", "subagents"),
                "graph_subagents_inspect" => ("Inspecting", "Inspected", "subagent", "subagents"),
                "graph_subagents_message" => ("Sending", "Sent", "message", "messages"),
                "graph_subagents_receive" => ("Receiving", "Received", "message", "messages"),
                "graph_subagents_kill" => ("Stopping", "Stopped", "subagent", "subagents"),
                "graph_handoff" => ("Handing off", "Handed off", "agent", "agents"),
                "agent_feedback" => ("Providing", "Provided", "feedback", "feedback"),
                "goal_status" => ("Marking", "Marked", "status", "times"),
                _ => unreachable!("only supported tools can form a series"),
            };
            let verb = if active { present } else { past };
            let verb = if index == 0 {
                verb.to_owned()
            } else {
                verb.to_lowercase()
            };
            if name == "graph_subagents_spawn" {
                let per_call: Vec<_> = tools
                    .iter()
                    .filter(|tool| tool.name == name)
                    .filter_map(|tool| {
                        summary_detail_value(&tool.summary, "Graph spawn count: ")
                            .and_then(|value| value.parse::<usize>().ok())
                    })
                    .collect();
                if per_call.len() == count {
                    format!("{verb} {} subagents", per_call.iter().sum::<usize>())
                } else {
                    format!("{verb} subagents")
                }
            } else if name == "graph_subagents_list" {
                format!("{verb} subagents")
            } else if name == "agent_feedback" {
                format!("{verb} agent feedback")
            } else if name == "goal_status" {
                if count == 1 {
                    let label = tools.iter().find(|tool| tool.name == name).map_or_else(
                        || format!("{verb} goal status"),
                        |tool| tool_single_label(tool, active),
                    );
                    if index == 0 {
                        label
                    } else {
                        let (first, rest) = label.split_at(1);
                        format!("{}{rest}", first.to_lowercase())
                    }
                } else {
                    format!("{verb} goal status {count} times")
                }
            } else if name == "graph_subagents_message" {
                if count == 1 {
                    format!("{verb} a message to a subagent")
                } else if active {
                    format!("{verb} {count} subagent messages")
                } else {
                    format!("{verb} {count} messages to subagents")
                }
            } else if name == "graph_subagents_receive" {
                if count == 1 {
                    format!("{verb} a subagent message")
                } else {
                    format!("{verb} {count} subagent messages")
                }
            } else if name == "graph_handoff" {
                if count == 1 {
                    format!("{verb} to an agent")
                } else {
                    format!("{verb} to {count} agents")
                }
            } else if count == 1
                && matches!(
                    name,
                    "surface_query"
                        | "findings_query"
                        | "memory_session_query"
                        | "memory_query"
                        | "runbook_query"
                )
            {
                verb.trim_end_matches(" for").to_owned()
            } else if count == 1 {
                format!("{verb} a {noun}")
            } else {
                format!("{verb} {count} {plural}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn transcript_tool_series(
    tools: &[&SessionToolSummary],
    active: bool,
    saved: bool,
    state: &State,
    theme: ThemeStyle<'_>,
) -> El {
    let first = tools[0];
    let label = combined_tool_series_label(tools, active);
    if tools.len() == 1 {
        return transcript_tool_view(first, state, theme, saved, &label);
    }
    let expanded = state.expanded_tool_series_id.as_deref() == Some(first.id.as_str());
    let id = first.id.clone();
    let mut series = col()
        .key(format!("session-tool-series-{}", first.id))
        .w_full()
        .gap(4.0)
        .child(
            row()
                .key(format!("session-tool-series-button-{}", first.id))
                .w_full()
                .gap(10.0)
                .py(3.0)
                .on_press(move |state: &mut State| {
                    if state.expanded_tool_series_id.as_deref() == Some(id.as_str()) {
                        state.expanded_tool_series_id = None;
                        state.clear_tool_detail();
                    } else {
                        state.expanded_tool_series_id = Some(id.clone());
                    }
                })
                .child(
                    icon(
                        if matches!(first.name.as_str(), "bash" | "powershell") {
                            &TERMINAL
                        } else {
                            &WRENCH
                        },
                        16.0,
                    )
                    .color(theme.color("#969696")),
                )
                .child(
                    text(label)
                        .font(ui_font(13.0, 400.0))
                        .color(theme.color(if tools.iter().any(|tool| tool.is_error) {
                            "#e07676"
                        } else {
                            "#969696"
                        }))
                        .fade_truncate(),
                )
                .child(
                    icon(
                        if expanded {
                            &CHEVRON_DOWN
                        } else {
                            &CHEVRON_RIGHT
                        },
                        13.0,
                    )
                    .color(theme.color("#777777")),
                ),
        );
    if expanded {
        for tool in tools {
            series = series.child(transcript_tool_view(
                tool,
                state,
                theme,
                saved,
                &tool.summary,
            ));
        }
    }
    series
}

fn transcript_tool_view(
    tool: &SessionToolSummary,
    state: &State,
    theme: ThemeStyle<'_>,
    saved: bool,
    label: &str,
) -> El {
    let expanded = state.expanded_tool_id.as_deref() == Some(&tool.id);
    let id = tool.id.clone();
    let mut entry = col()
        .key(format!("session-tool-{}", tool.id))
        .w_full()
        .gap(6.0)
        .child(
            row()
                .key(format!("session-tool-button-{}", tool.id))
                .w_full()
                .gap(10.0)
                .py(3.0)
                .on_press(move |state: &mut State| toggle_tool_detail(state, id.clone()))
                .child(
                    icon(
                        if matches!(tool.name.as_str(), "bash" | "powershell") {
                            &TERMINAL
                        } else {
                            &WRENCH
                        },
                        16.0,
                    )
                    .color(theme.color("#969696")),
                )
                .child(
                    text(label.to_owned())
                        .font(ui_font(13.0, 400.0))
                        .color(theme.color(if tool.is_error { "#e07676" } else { "#969696" }))
                        .fade_truncate(),
                )
                .child(
                    icon(
                        if expanded {
                            &CHEVRON_DOWN
                        } else {
                            &CHEVRON_RIGHT
                        },
                        13.0,
                    )
                    .color(theme.color("#777777")),
                ),
        );
    if expanded {
        let mut detail = col()
            .key(format!("session-tool-detail-{}", tool.id))
            .w_full()
            .gap(6.0)
            .px(12.0)
            .pl(26.0)
            .py(10.0)
            .radius(8.0)
            .continuous_corners()
            .fill(theme.color("#181818"));
        if let Some(page) = state.tool_detail.as_ref().filter(|page| page.id == tool.id) {
            detail = detail
                .child(
                    text("Arguments")
                        .font(ui_font(12.0, 600.0))
                        .color(theme.color("#969696")),
                )
                .child(
                    text(page.arguments.clone())
                        .font(ui_font(12.0, 400.0))
                        .color(theme.color("#d0d0d0"))
                        .wrap(),
                )
                .child(
                    text(if page.total_bytes == 0 && state.session_busy {
                        "Running…".to_owned()
                    } else {
                        format!(
                            "Result · bytes {}–{} of {}",
                            page.offset + 1,
                            page.next_offset,
                            page.total_bytes
                        )
                    })
                    .font(ui_font(12.0, 600.0))
                    .color(theme.color("#969696")),
                )
                .child(
                    text(page.output.clone())
                        .font(ui_font(12.0, 400.0))
                        .color(theme.color("#d0d0d0"))
                        .wrap(),
                );
            if page.next_offset < page.total_bytes
                && !state.tool_detail_busy
                && (saved || !state.session_busy)
            {
                let id = tool.id.clone();
                let offset = page.next_offset;
                detail = detail.child(
                    el().key(format!("session-tool-more-{}", tool.id))
                        .px(8.0)
                        .py(5.0)
                        .radius(7.0)
                        .continuous_corners()
                        .fill(theme.color("#282828"))
                        .hover(theme.color("#383838"))
                        .on_press(move |state: &mut State| {
                            load_tool_detail_page(state, id.clone(), offset)
                        })
                        .child(
                            text("Next page")
                                .font(ui_font(12.0, 400.0))
                                .color(theme.color("#d0d0d0")),
                        ),
                );
            }
            if page.offset > 0 && !state.tool_detail_busy {
                let id = tool.id.clone();
                detail = detail.child(
                    el().key(format!("session-tool-start-{}", tool.id))
                        .px(8.0)
                        .py(5.0)
                        .radius(7.0)
                        .continuous_corners()
                        .fill(theme.color("#282828"))
                        .hover(theme.color("#383838"))
                        .on_press(move |state: &mut State| {
                            load_tool_detail_page(state, id.clone(), 0)
                        })
                        .child(
                            text("First page")
                                .font(ui_font(12.0, 400.0))
                                .color(theme.color("#d0d0d0")),
                        ),
                );
            }
            if page.next_offset < page.total_bytes && state.session_busy && !saved {
                detail = detail.child(
                    text("More output becomes available when this turn finishes.")
                        .font(ui_font(12.0, 400.0))
                        .color(theme.color("#969696")),
                );
            }
        } else if state.tool_detail_busy {
            detail = detail.child(
                text("Loading tool details…")
                    .font(ui_font(12.0, 400.0))
                    .color(theme.color("#969696")),
            );
        }
        if !state.tool_detail_error.is_empty() {
            detail = detail.child(
                text(state.tool_detail_error.clone())
                    .font(ui_font(12.0, 400.0))
                    .color(theme.color("#e07676")),
            );
        }
        entry = entry.child(detail);
    }
    entry
}

pub(crate) fn transcript_reasoning(
    content: String,
    key: String,
    streaming: bool,
    theme: ThemeStyle<'_>,
) -> El {
    row()
        .key(key)
        .w_full()
        .gap(10.0)
        .py(3.0)
        .child(icon(&BRAIN, 18.0).color(theme.color("#969696")))
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .child(transcript_markdown(content, theme, streaming, true)),
        )
}

fn work_duration(ms: u64) -> String {
    let seconds = ms / 1_000;
    let hours = seconds / 3_600;
    let minutes = (seconds / 60) % 60;
    let remaining_seconds = seconds % 60;
    let mut parts = Vec::with_capacity(3);
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes}m"));
    }
    if remaining_seconds > 0 || parts.is_empty() {
        parts.push(format!("{remaining_seconds}s"));
    }
    parts.join(" ")
}

pub(crate) fn work_header(
    key: String,
    index: usize,
    duration_ms: u64,
    working: bool,
    expanded: bool,
    theme: ThemeStyle<'_>,
) -> El {
    let label = if duration_ms == 0 {
        if working { "Working" } else { "Worked" }.to_owned()
    } else {
        format!(
            "{} for {}",
            if working { "Working" } else { "Worked" },
            work_duration(duration_ms)
        )
    };
    let mut header = row().key(key).w_full().gap(5.0).py(7.0).child(
        text(label)
            .font(ui_font(14.0, 400.0))
            .color(theme.color("#969696")),
    );
    if !working {
        header = header
            .on_press(move |state: &mut State| {
                if !state.session_work_expanded.insert(index) {
                    state.session_work_expanded.remove(&index);
                }
            })
            .child(
                icon(
                    if expanded {
                        &CHEVRON_DOWN
                    } else {
                        &CHEVRON_RIGHT
                    },
                    14.0,
                )
                .color(theme.color("#777777")),
            );
    }
    col()
        .w_full()
        .child(header)
        .child(el().w_full().h(1.0).fill(theme.color("#303030")))
}

fn previous_input(state: &mut State) {
    let entries = state
        .active_session
        .as_ref()
        .map(|detail| input_history::session_entries(&detail.messages))
        .unwrap_or_default();
    if let Some(value) = state
        .session_input_history
        .previous(&entries, &state.new_research_prompt.text)
    {
        state.new_research_prompt.replace_text(value);
    }
}

fn next_input(state: &mut State) {
    let entries = state
        .active_session
        .as_ref()
        .map(|detail| input_history::session_entries(&detail.messages))
        .unwrap_or_default();
    if let Some(value) = state.session_input_history.next(&entries) {
        state.new_research_prompt.replace_text(value);
    }
}

fn composer_history_key(state: &mut State, event: &KeyEvent, _: &mut Tx) -> bool {
    if event.mods.shift
        || event.mods.cmd
        || event.mods.alt
        || event.mods.ctrl
        || event.mods.logo
        || state.new_research_prompt.preedit().is_some()
    {
        return false;
    }

    match event.key {
        Key::Up => {
            let entries = state
                .active_session
                .as_ref()
                .map(|detail| input_history::session_entries(&detail.messages))
                .unwrap_or_default();
            if !state.session_input_history.can_previous(entries.len())
                || (!state.session_input_history.can_next()
                    && state
                        .new_research_prompt
                        .line_of_byte(state.new_research_prompt.focus)
                        > 0)
            {
                return false;
            }
            previous_input(state);
            true
        }
        Key::Down if state.session_input_history.can_next() => {
            next_input(state);
            true
        }
        _ => false,
    }
}

fn normalize_goal_limit(edit: &mut TextEdit, max: u64) {
    let original = edit.text.as_bytes();
    let mut normalized = String::new();
    let mut value = 0_u64;
    let mut anchor = 0;
    let mut focus = 0;
    let mut at_limit = false;
    for (index, byte) in original.iter().copied().enumerate() {
        if byte.is_ascii_digit() && !at_limit {
            let digit = u64::from(byte - b'0');
            if !(normalized.is_empty() && digit == 0) {
                if let Some(next) = value
                    .checked_mul(10)
                    .and_then(|current| current.checked_add(digit))
                {
                    if next <= max {
                        value = next;
                        normalized.push(char::from(byte));
                        if index < edit.anchor {
                            anchor += 1;
                        }
                        if index < edit.focus {
                            focus += 1;
                        }
                    } else {
                        at_limit = true;
                    }
                } else {
                    at_limit = true;
                }
            }
        }
    }
    if normalized != edit.text {
        edit.replace_text(normalized);
        edit.set_selection(anchor, focus);
    }
}

fn goal_limit_field(
    edit: &TextEdit,
    key: &'static str,
    label: &'static str,
    theme: ThemeStyle<'_>,
    max: u64,
    accessor: fn(&mut State) -> &mut TextEdit,
) -> El {
    row()
        .w_full()
        .center_y()
        .gap(8.0)
        .child(
            text(label)
                .grow(1.0)
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#d4d4d4")),
        )
        .child(
            text_input(edit, ui_font(13.0, 400.0))
                .key(key)
                .single_line()
                .w(106.0)
                .h(32.0)
                .padding(7.0)
                .placeholder("Unlimited", theme.color("#888888"))
                .radius(8.0)
                .continuous_corners()
                .fill(theme.color("#303030"))
                .color(theme.color("#eeeeee"))
                .on_edit(accessor)
                .on_input_change(move |state: &mut State, _| {
                    normalize_goal_limit(accessor(state), max);
                }),
        )
}

pub(crate) fn goal_menu(state: &State, theme: ThemeStyle<'_>) -> El {
    let selected = state.new_research_goal_mode;
    in_place_menu::popup_surface(
        col()
            .key("goal-mode-menu")
            .role("menu")
            .label("Goal Mode")
            .w(265.0)
            .padding(8.0)
            .gap(7.0)
            .child(
                row()
                    .key("goal-mode-menu-toggle")
                    .role("menuitem")
                    .label(if selected {
                        "Goal Mode, on"
                    } else {
                        "Goal Mode, off"
                    })
                    .w_full()
                    .h(34.0)
                    .px(8.0)
                    .center_y()
                    .radius(7.0)
                    .hover(theme.color("#383838"))
                    .on_press(|state: &mut State| {
                        state.new_research_goal_mode = !state.new_research_goal_mode;
                    })
                    .child(
                        text("Goal Mode")
                            .grow(1.0)
                            .font(ui_font(14.0, if selected { 550.0 } else { 400.0 }))
                            .color(theme.color("#eeeeee")),
                    )
                    .child(
                        el().key("goal-mode-checkbox")
                            .size(18.0, 18.0)
                            .center()
                            .radius(5.0)
                            .continuous_corners()
                            .fill(theme.color(if selected { "#eeeeee" } else { "#303030" }))
                            .child(if selected {
                                icon(&CHECK, 13.0).color(theme.color("#0d0d0d"))
                            } else {
                                el().size(0.0, 0.0)
                            }),
                    ),
            )
            .child(el().w_full().h(1.0).fill(theme.color("#3b3b3b")))
            .child(goal_limit_field(
                &state.new_research_goal_max_turns,
                "goal-mode-max-turns",
                "Max turns",
                theme,
                i64::MAX as u64,
                |state| &mut state.new_research_goal_max_turns,
            ))
            .child(goal_limit_field(
                &state.new_research_goal_max_duration_minutes,
                "goal-mode-max-duration",
                "Max duration (min)",
                theme,
                crate::MAX_GOAL_DURATION_MINUTES,
                |state| &mut state.new_research_goal_max_duration_minutes,
            )),
        theme.color("#242424"),
    )
    .anchored_fit("new-research-goal-control", Side::Bottom, Align::End, 4.0)
}

fn ribbon_status_dot(key: &'static str, theme: ThemeStyle<'_>) -> El {
    el().key(key)
        .absolute(20.0, 1.0)
        .size(7.0, 7.0)
        .radius(3.5)
        .fill(theme.color("#72b783"))
}

pub(crate) fn show_composer_model_selector(state: &State) -> bool {
    !crate::extensions_view::graphs_enabled(state)
        && (!state.auth_loaded || has_configured_provider(state))
}

fn composer_model_control(state: &State, theme: ThemeStyle<'_>) -> Option<El> {
    if state.auth_loaded && !has_configured_provider(state) {
        return Some(
            text("No provider configured. Add one in Settings to start research.")
                .key("new-research-provider-warning")
                .min_w(0.0)
                .fade_truncate()
                .font(ui_font(13.0, 500.0))
                .color(theme.color("#dca15f")),
        );
    }
    if !show_composer_model_selector(state) {
        return None;
    }
    let choices = choices(state);
    let selected = selected(state, &choices);
    let model_label = selected.map_or_else(
        || {
            if let Some(provider) = state
                .lead_provider
                .filter(|_| has_configured_provider(state))
            {
                let method = effective_model_method(&state.auth_status[provider.index()]);
                if state.model_errors[catalog_index(provider, method)].is_empty() {
                    format!("{} · Loading models…", provider.name())
                } else {
                    format!("{} · Retry models", provider.name())
                }
            } else {
                "Choose a model".to_owned()
            }
        },
        |choice| choice.name.clone(),
    );
    let model_accessible_label = selected.map_or_else(
        || "Lead model unavailable".to_owned(),
        |choice| {
            format!(
                "Lead model: {} {}",
                provider_label(state, &choice.selection),
                choice.name
            )
        },
    );
    let mut model_button = row()
        .key("new-research-model-control")
        .role("button")
        .label(model_accessible_label)
        .h(28.0)
        .min_w(0.0)
        .max_w(250.0)
        .px(8.0)
        .center_y()
        .gap(7.0)
        .radius(16.0)
        .continuous_corners()
        .fill(if state.composer_menu_open {
            theme.color("#303030")
        } else {
            Color::TRANSPARENT
        })
        .child(if let Some(choice) = selected {
            icon(choice.selection.provider.logo(), 15.0).color(theme.color("#b8b8b8"))
        } else {
            el().size(15.0, 15.0)
        })
        .child(
            text(model_label)
                .min_w(0.0)
                .fade_truncate()
                .font(ui_font(13.0, 450.0))
                .color(theme.color(if selected.is_some() {
                    "#b8b8b8"
                } else {
                    "#888888"
                })),
        )
        .child(
            text(state.new_research_effort.label())
                .min_w(0.0)
                .fade_truncate()
                .font(ui_font(12.0, 400.0))
                .color(theme.color("#888888")),
        )
        .child(
            icon(
                if state.composer_menu_open {
                    &CHEVRON_UP
                } else {
                    &CHEVRON_DOWN
                },
                13.0,
            )
            .color(theme.color("#969696")),
        );
    if !choices.is_empty() || has_configured_provider(state) {
        model_button = model_button
            .hover(theme.color("#303030"))
            .on_press(toggle_menu);
    }
    Some(model_button)
}

fn composer_action(state: &State, theme: ThemeStyle<'_>, has_workspaces: bool) -> El {
    let available =
        has_workspaces && !state.new_research_prompt.text.trim().is_empty() && !state.session_busy;
    let can_stop = state.session_busy && state.session_stop_id.is_some() && !state.session_stopping;
    let mut button = el()
        .key(if state.session_busy {
            "new-research-stop"
        } else {
            "new-research-send"
        })
        .role("button")
        .label(if state.session_busy && state.session_stopping {
            "Stopping scan"
        } else if state.session_busy {
            "Stop scan"
        } else {
            "Send message"
        })
        .size(28.0, 28.0)
        .center()
        .radius(14.0)
        .fill(if available || can_stop {
            theme.accent()
        } else {
            theme.color("#303030")
        })
        .child(
            icon(
                if state.session_busy {
                    &STOP
                } else {
                    &ARROW_RIGHT
                },
                16.0,
            )
            .color(if available || can_stop {
                theme.accent_ink()
            } else {
                theme.color("#777777")
            }),
        );
    if can_stop {
        button = button
            .hover(theme.color("#eeeeee"))
            .on_press(crate::stop_session_turn);
    } else if available {
        button = button
            .hover(theme.color("#eeeeee"))
            .on_press(start_session_turn);
    }
    button
}

pub(crate) fn render(state: &State, theme: ThemeStyle<'_>, available_width: f32) -> El {
    let has_workspaces = !state.workspaces.is_empty();
    let no_workspaces = state.workspace_list_loaded && !has_workspaces;
    let show_draft_controls = state.active_session.is_none() && !state.session_busy;
    let model_control = composer_model_control(state, theme);
    let show_ribbon = show_draft_controls || model_control.is_some();
    let input_font = ui_font(15.0, 400.0);
    let composer_input = if has_workspaces {
        let input = text_input(&state.new_research_prompt, input_font)
            .key("new-research-composer-input")
            .role("textbox")
            .label("Research objective")
            .w_full()
            .px(7.0)
            .py(5.0)
            .color(theme.color("#eeeeee"))
            .placeholder(
                if state
                    .active_session
                    .as_ref()
                    .is_some_and(|detail| beale_graphs::is_peer_graph(&detail.graph_id))
                {
                    beale_graphs::ProfileId::from_id(&state.session_agent_role)
                        .map_or("Research with Beale", |role| role.label())
                } else {
                    "Research with Beale"
                },
                theme.color("#888888"),
            )
            .on_edit(|state: &mut State| &mut state.new_research_prompt)
            .on_input_change(|state: &mut State, _| state.session_input_history.reset())
            .on_input_key(composer_history_key)
            .on_submit(|state: &mut State, _| state.new_research_prompt.insert("\n"));
        auto_grow(input, 1, 8).grow(1.0).min_w(0.0)
    } else {
        el().key("new-research-composer-input")
            .role("textbox")
            .label("Research objective unavailable until a workspace is added")
            .grow(1.0)
            .min_w(0.0)
            .px(7.0)
            .py(5.0)
            .child(
                text(if state.new_research_prompt.text.is_empty() {
                    "Research with Beale".to_owned()
                } else {
                    state.new_research_prompt.text.clone()
                })
                .font(input_font)
                .color(theme.color("#707070"))
                .wrap()
                .max_lines(8),
            )
    };
    let composer = row()
        .key("new-research-composer")
        .raise()
        .w_full()
        .min_h(0.0)
        .px(7.0)
        .py(14.0)
        .gap(6.0)
        .center_y()
        .mt(-8.0)
        .radius(20.0)
        .continuous_corners()
        .fill(theme.color("#191919"))
        .border(1.0, theme.color("#2a2a2a"))
        .child(composer_input)
        .child(composer_action(state, theme, has_workspaces));
    let repeat_active = state.selection_menu == Some(SelectionMenu::Repeat);
    let goal_active = state.selection_menu == Some(SelectionMenu::GoalMode);
    let workspace_menu_active = state.selection_menu == Some(SelectionMenu::ResearchWorkspace);
    let workspace_name = state
        .selected_research_workspace()
        .map_or("Select Workspace", |workspace| workspace.name.as_str());
    let mut workspace_button = row()
        .key("new-research-workspace-control")
        .role("button")
        .label(format!("Research workspace: {workspace_name}"))
        .h(28.0)
        .max_w(300.0)
        .min_w(0.0)
        .px(8.0)
        .center_y()
        .gap(7.0)
        .radius(14.0)
        .continuous_corners()
        .fill(if workspace_menu_active {
            theme.color("#303030")
        } else {
            Color::TRANSPARENT
        })
        .child(crate::workspace_logo::mark(
            state.selected_research_workspace(),
            15.0,
            theme.color("#9d9d9d"),
        ))
        .child(
            text(workspace_name)
                .min_w(0.0)
                .fade_truncate()
                .font(ui_font(13.0, 500.0))
                .color(theme.color(if state.workspaces.is_empty() {
                    "#888888"
                } else {
                    "#dddddd"
                })),
        )
        .child(
            icon(
                if workspace_menu_active {
                    &CHEVRON_UP
                } else {
                    &CHEVRON_DOWN
                },
                13.0,
            )
            .color(theme.color("#969696")),
        );
    if !state.workspaces.is_empty() && state.active_session.is_none() {
        workspace_button =
            workspace_button
                .hover(theme.color("#303030"))
                .on_press(|state: &mut State| {
                    toggle_selection_menu(state, SelectionMenu::ResearchWorkspace)
                });
    }
    let workspace_control = if no_workspaces {
        text("No workspaces available. Create one to start research.")
            .key("new-research-workspace-warning")
            .min_w(0.0)
            .fade_truncate()
            .font(ui_font(13.0, 500.0))
            .color(theme.color("#dca15f"))
    } else {
        workspace_button
    };
    let mut ribbon = row()
        .key("new-research-ribbon")
        .transition(
            Transition::asymmetric(
                Transition::none(),
                Transition::offset(0.0, 44.0).combined(Transition::opacity()),
            )
            .hold_layout(),
        )
        .w_full()
        .h(44.0)
        .pl(8.0)
        .pr(12.0)
        .pt(3.0)
        .pb(11.0)
        .center_y()
        .gap(7.0)
        .corners([18.0, 18.0, 0.0, 0.0])
        .continuous_corners()
        .fill(theme.color("#1b1b1b"))
        .border_sides([1.0, 1.0, 0.0, 1.0], theme.color("#2a2a2a"))
        .children(show_draft_controls.then_some(workspace_control))
        .children(model_control)
        .child(el().grow(1.0));
    if show_draft_controls
        && crate::extensions_view::goal_mode_enabled(state)
        && !beale_graphs::is_rotating_graph(&state.new_research_graph_id)
    {
        ribbon = ribbon.child(
            row()
                .key("new-research-goal-control")
                .role("button")
                .label(if state.new_research_goal_mode {
                    "Goal Mode on"
                } else {
                    "Goal Mode off"
                })
                .size(28.0, 28.0)
                .justify_center()
                .center_y()
                .radius(8.0)
                .continuous_corners()
                .fill(if goal_active {
                    theme.color("#303030")
                } else {
                    Color::TRANSPARENT
                })
                .hover(theme.color("#303030"))
                .on_press(|state: &mut State| toggle_selection_menu(state, SelectionMenu::GoalMode))
                .child(
                    icon(&TARGET, 15.0).color(theme.color(if state.new_research_goal_mode {
                        "#eeeeee"
                    } else {
                        "#a0a0a0"
                    })),
                )
                .child(if state.new_research_goal_mode {
                    ribbon_status_dot("new-research-goal-indicator", theme)
                } else {
                    el().size(0.0, 0.0)
                }),
        );
    }
    if show_draft_controls && !beale_graphs::is_rotating_graph(&state.new_research_graph_id) {
        ribbon = ribbon.child(
            row()
                .key("new-research-repeat-control")
                .role("button")
                .label(format!(
                    "Repeat schedule: {}",
                    state.new_research_repeat.label()
                ))
                .size(28.0, 28.0)
                .justify_center()
                .center_y()
                .radius(8.0)
                .continuous_corners()
                .fill(if repeat_active {
                    theme.color("#303030")
                } else {
                    Color::TRANSPARENT
                })
                .hover(theme.color("#303030"))
                .on_press(|state: &mut State| toggle_selection_menu(state, SelectionMenu::Repeat))
                .child(icon(&REPEAT, 15.0).color(theme.color(
                    if state.new_research_repeat == RepeatPreset::None {
                        "#a0a0a0"
                    } else {
                        "#eeeeee"
                    },
                )))
                .child(if state.new_research_repeat != RepeatPreset::None {
                    ribbon_status_dot("new-research-repeat-indicator", theme)
                } else {
                    el().size(0.0, 0.0)
                }),
        );
    }
    let mut transcript = col()
        .key("session-transcript")
        .w_full()
        .max_w(750.0)
        .self_center()
        .gap(14.0)
        .px(14.0)
        .pt(22.0)
        .pb(24.0);
    let graph_id = state
        .active_session
        .as_ref()
        .map_or(state.new_research_graph_id.as_str(), |detail| {
            detail.graph_id.as_str()
        });
    let peer_graph = beale_graphs::is_peer_graph(graph_id);
    let selected_role = crate::research_header::selected_role(state, graph_id);
    let visible_role =
        |role: &str| !peer_graph || role == selected_role || role == "main" || role.is_empty();
    if let Some(detail) = &state.active_session {
        if peer_graph {
            if let Some(agent) = detail
                .graph_agents
                .iter()
                .find(|agent| agent.role == selected_role)
            {
                let status_text = match agent.status.as_str() {
                    "failed" => Some("The agent's last turn failed. Send a message to retry."),
                    "queued" => Some("A handoff is queued for this agent."),
                    "idle"
                        if !detail
                            .messages
                            .iter()
                            .any(|message| message.agent_role == selected_role) =>
                    {
                        Some("Waiting for a handoff or direct instruction.")
                    }
                    _ => None,
                };
                if let Some(status_text) = status_text {
                    transcript = transcript.child(
                        text(status_text)
                            .key("session-agent-status")
                            .font(ui_font(12.0, 450.0))
                            .color(theme.color(if agent.status == "failed" {
                                "#e07676"
                            } else {
                                "#969696"
                            })),
                    );
                }
            }
            for (index, item) in detail
                .graph_handoffs
                .iter()
                .filter(|item| item.to_role == selected_role)
                .take(4)
                .enumerate()
            {
                let source = beale_graphs::ProfileId::from_id(&item.from_role)
                    .map_or_else(|| item.from_role.clone(), |role| role.label().to_owned());
                transcript = transcript.child(
                    text(format!("From {source} · {}\n{}", item.status, item.task))
                        .key(format!("session-handoff-{index}"))
                        .font(ui_font(12.0, 450.0))
                        .color(theme.color("#b8b8b8"))
                        .wrap(),
                );
            }
        }
        if detail.graph_id == beale_graphs::SIMPLE_ID {
            transcript = transcript.child(
                text(beale_graphs::SIMPLE_NAME)
                    .key("session-graph-label")
                    .font(ui_font(12.0, 500.0))
                    .color(theme.color("#969696")),
            );
        }
        if detail.first_turn_number > 1 {
            transcript = transcript.child(
                text(format!(
                    "Showing recent activity from turn {}. Earlier turns remain saved.",
                    detail.first_turn_number
                ))
                .key("session-earlier-activity")
                .font(ui_font(12.0, 400.0))
                .color(theme.color("#888888"))
                .wrap(),
            );
        }
        for (index, message) in detail.messages.iter().enumerate() {
            let user = message.role == SessionRole::User as i32;
            if peer_graph && !visible_role(&message.agent_role) {
                continue;
            }
            let own_activity = message
                .tools
                .iter()
                .any(|tool| visible_role(&tool.agent_role))
                || message
                    .reasoning_summaries
                    .iter()
                    .any(|item| visible_role(&item.agent_role))
                || message
                    .commentary
                    .iter()
                    .any(|item| visible_role(&item.agent_role));
            if peer_graph && !user && !own_activity && message.agent_role != selected_role {
                continue;
            }
            if !user {
                let expanded = state.session_work_expanded.contains(&index);
                transcript = transcript.child(work_header(
                    format!("session-work-header-{index}"),
                    index,
                    message.duration_ms,
                    false,
                    expanded,
                    theme,
                ));
                if expanded
                    && !message
                        .reasoning_summaries
                        .iter()
                        .any(|item| visible_role(&item.agent_role))
                {
                    transcript = transcript.child(transcript_reasoning(
                        "No reasoning summary was received for this turn.".to_owned(),
                        format!("session-reasoning-unavailable-{index}"),
                        false,
                        theme,
                    ));
                }
                if expanded {
                    let mut summaries = message.reasoning_summaries.iter().enumerate().peekable();
                    let mut commentary = message.commentary.iter().enumerate().peekable();
                    let mut grouped_through = 0;
                    for position in 0..=message.tools.len() {
                        while summaries.peek().is_some_and(|(_, summary)| {
                            summary.after_tool_count as usize == position
                        }) {
                            if let Some((summary_index, summary)) = summaries.next() {
                                if !visible_role(&summary.agent_role) {
                                    continue;
                                }
                                transcript = transcript.child(transcript_reasoning(
                                    summary.text.clone(),
                                    format!("session-reasoning-{index}-{summary_index}"),
                                    false,
                                    theme,
                                ));
                            }
                        }
                        while commentary
                            .peek()
                            .is_some_and(|(_, item)| item.after_tool_count as usize == position)
                        {
                            if let Some((commentary_index, item)) = commentary.next() {
                                if !visible_role(&item.agent_role) {
                                    continue;
                                }
                                transcript = transcript.child(transcript_message(
                                    format!("session-commentary-{index}-{commentary_index}"),
                                    item.text.clone(),
                                    false,
                                    false,
                                    theme,
                                ));
                            }
                        }
                        if position >= grouped_through
                            && let Some(tool) = message.tools.get(position)
                            && visible_role(&tool.agent_role)
                        {
                            let mut end = position + 1;
                            if series_tool(tool) {
                                while let Some(next) = message.tools.get(end) {
                                    if !series_tool(next)
                                        || next.agent_role != tool.agent_role
                                        || !visible_role(&next.agent_role)
                                        || message
                                            .reasoning_summaries
                                            .iter()
                                            .any(|item| item.after_tool_count as usize == end)
                                        || message
                                            .commentary
                                            .iter()
                                            .any(|item| item.after_tool_count as usize == end)
                                    {
                                        break;
                                    }
                                    end += 1;
                                }
                            }
                            let series: Vec<_> = message.tools[position..end].iter().collect();
                            transcript = transcript
                                .child(transcript_tool_series(&series, false, true, state, theme));
                            grouped_through = end;
                        }
                    }
                }
            }
            if user || !peer_graph || message.agent_role == selected_role {
                transcript = transcript.child(transcript_message(
                    format!("session-message-{index}"),
                    message.text.clone(),
                    user,
                    false,
                    theme,
                ));
            }
        }
        if !state.session_busy
            && let Some(goal) = &detail.goal
        {
            let status = match GoalStatus::try_from(goal.status) {
                Ok(GoalStatus::Complete) => Some(("Goal completed".to_owned(), "#b8b8b8")),
                Ok(GoalStatus::Waiting) => Some((
                    format!("Goal waiting for: {}", goal.required_state),
                    "#dca15f",
                )),
                Ok(GoalStatus::Paused)
                    if goal.max_turns != 0 && goal.turns_used >= goal.max_turns =>
                {
                    Some(("Goal paused at the max turns limit.".to_owned(), "#dca15f"))
                }
                Ok(GoalStatus::Paused)
                    if goal.max_duration_minutes != 0
                        && goal.active_ms >= goal.max_duration_minutes.saturating_mul(60_000) =>
                {
                    Some((
                        "Goal paused at the max duration limit.".to_owned(),
                        "#dca15f",
                    ))
                }
                Ok(GoalStatus::Paused) => Some((
                    "Goal paused. Send a message to continue.".to_owned(),
                    "#dca15f",
                )),
                _ => None,
            };
            if let Some((label, color)) = status {
                transcript = transcript.child(
                    text(label)
                        .key("session-goal-status")
                        .font(ui_font(12.0, 400.0))
                        .color(theme.color(color))
                        .wrap(),
                );
            }
        }
    }
    if let Some(prompt) = &state.session_pending_prompt
        && (!peer_graph || state.session_pending_role == selected_role)
    {
        transcript = transcript.child(transcript_message(
            "session-pending-message".to_owned(),
            prompt.clone(),
            true,
            false,
            theme,
        ));
    }
    let role_busy = state.session_role_busy();
    let role_live_events = state.session_live_events.iter().any(|event| {
        event.subagent_id.is_empty() && (!peer_graph || event.agent_role == selected_role)
    });
    let role_working = role_busy || (state.session_busy && role_live_events);
    if role_busy || role_live_events {
        transcript = transcript.child(work_header(
            "session-current-work-header".to_owned(),
            state
                .active_session
                .as_ref()
                .map_or(0, |detail| detail.messages.len()),
            state.session_role_live_ms(),
            role_working,
            !role_working
                && state.session_work_expanded.contains(
                    &state
                        .active_session
                        .as_ref()
                        .map_or(0, |detail| detail.messages.len()),
                ),
            theme,
        ));
        if role_working
            || state.session_work_expanded.contains(
                &state
                    .active_session
                    .as_ref()
                    .map_or(0, |detail| detail.messages.len()),
            )
        {
            let mut grouped_through = 0;
            for (index, event) in state.session_live_events.iter().enumerate() {
                if index < grouped_through {
                    continue;
                }
                if !event.subagent_id.is_empty() {
                    continue;
                }
                if peer_graph && !event.agent_role.is_empty() && event.agent_role != selected_role {
                    continue;
                }
                match SessionStreamKind::try_from(event.kind) {
                    Ok(SessionStreamKind::ReasoningDelta) => {
                        transcript = transcript.child(transcript_reasoning(
                            event.text.clone(),
                            format!("session-live-reasoning-{index}"),
                            true,
                            theme,
                        ));
                    }
                    Ok(SessionStreamKind::MessageDelta) => {
                        transcript = transcript.child(transcript_message(
                            format!("session-live-message-{index}"),
                            event.text.clone(),
                            false,
                            true,
                            theme,
                        ));
                    }
                    Ok(SessionStreamKind::ToolStarted | SessionStreamKind::ToolFinished) => {
                        if let Some(tool) = &event.tool {
                            let mut end = index + 1;
                            if series_tool(tool) {
                                while let Some(next) = state.session_live_events.get(end) {
                                    if !matches!(
                                        SessionStreamKind::try_from(next.kind),
                                        Ok(SessionStreamKind::ToolStarted
                                            | SessionStreamKind::ToolFinished)
                                    ) || next.tool.as_ref().is_none_or(|next_tool| {
                                        !series_tool(next_tool)
                                            || next_tool.agent_role != tool.agent_role
                                            || !next.subagent_id.is_empty()
                                    }) {
                                        break;
                                    }
                                    end += 1;
                                }
                            }
                            let series: Vec<_> = state.session_live_events[index..end]
                                .iter()
                                .filter_map(|item| item.tool.as_ref())
                                .collect();
                            let active = state.session_live_events[index..end]
                                .iter()
                                .any(|item| item.kind == SessionStreamKind::ToolStarted as i32);
                            transcript = transcript.child(transcript_tool_series(
                                &series, active, false, state, theme,
                            ));
                            grouped_through = end;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    if !state.session_error.is_empty() {
        transcript = transcript.child(
            text(state.session_error.clone())
                .key("session-error")
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#e07676"))
                .wrap(),
        );
    }
    let show_transcript = state.active_session.is_some()
        || state.session_pending_prompt.is_some()
        || state.session_busy
        || !state.session_error.is_empty();
    let mut composer_stack = col()
        .key("new-research-composer-stack")
        .w_full()
        .max_w(750.0)
        .self_center();
    if show_ribbon {
        composer_stack = composer_stack.child(col().w_full().px(20.0).child(ribbon));
    }
    composer_stack = composer_stack.child(composer);
    col()
        .key("new-research-view")
        .min_w(0.0)
        .w_full()
        .h_full()
        .min_h(0.0)
        .px(20.0)
        .pb(6.0)
        .child(if show_transcript {
            col()
                .key("session-transcript-scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec { stick_bottom: true })
                .child(transcript)
        } else {
            new_research_setup(state, theme, available_width)
        })
        .child(composer_stack)
}

#[cfg(test)]
mod tests {
    use super::{
        MenuSection, choices, combined_tool_series_label, has_configured_provider, is_web_link,
        normalize_goal_limit, options, selected_model, series_tool, tool_single_label,
        work_duration,
    };
    use crate::{AuthStatus, Provider, State, view, visual_preview};
    use beale_contracts::v1::{CustomProvider, ModelEntry, SessionToolSummary};
    use rune::{Env, Runtime, TextEdit};

    #[test]
    fn collapsed_work_shows_only_the_agent_final_message() {
        let mut state = visual_preview::fixture("session-activity");
        state.session_work_expanded.clear();
        let mut runtime = Runtime::new(state, view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        assert!(runtime.find("session-work-header-1").is_some());
        assert!(runtime.find("session-message-1").is_some());
        for key in [
            "session-reasoning-1-0",
            "session-reasoning-1-1",
            "session-commentary-1-0",
            "session-tool-tool-example",
        ] {
            assert!(runtime.find(key).is_none(), "collapsed work shows {key}");
        }

        let (x, y) = runtime.find("session-work-header-1").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert!(runtime.state.session_work_expanded.contains(&1));
        for key in [
            "session-reasoning-1-0",
            "session-reasoning-1-1",
            "session-commentary-1-0",
            "session-tool-tool-example",
            "session-message-1",
        ] {
            assert!(runtime.find(key).is_some(), "expanded work omits {key}");
        }
    }

    #[test]
    fn base_tool_series_labels_match_active_and_completed_calls() {
        for (name, summary, active, past, active_plural, past_plural) in [
            (
                "read",
                "Read example.txt",
                "Reading example.txt",
                "Read example.txt",
                "Reading 2 files",
                "Read 2 files",
            ),
            (
                "write",
                "Write example.txt",
                "Writing example.txt",
                "Wrote example.txt",
                "Writing 2 files",
                "Wrote 2 files",
            ),
            (
                "edit",
                "Edit example.txt",
                "Editing example.txt",
                "Edited example.txt",
                "Editing 2 files",
                "Edited 2 files",
            ),
            (
                "bash",
                "Run cargo check",
                "Running cargo check",
                "Ran cargo check",
                "Running 2 commands",
                "Ran 2 commands",
            ),
            (
                "powershell",
                "Run Get-Process",
                "Running Get-Process",
                "Ran Get-Process",
                "Running 2 commands",
                "Ran 2 commands",
            ),
        ] {
            let tool = SessionToolSummary {
                name: name.to_owned(),
                summary: summary.to_owned(),
                ..Default::default()
            };
            assert_eq!(tool_single_label(&tool, true), active);
            assert_eq!(tool_single_label(&tool, false), past);
            assert_eq!(
                combined_tool_series_label(&[&tool, &tool], true),
                active_plural
            );
            assert_eq!(
                combined_tool_series_label(&[&tool, &tool], false),
                past_plural
            );
        }
        let prefixed = SessionToolSummary {
            name: "read".to_owned(),
            summary: "Discovery: Read example.txt".to_owned(),
            ..Default::default()
        };
        assert_eq!(tool_single_label(&prefixed, true), "Reading example.txt");
        let command = SessionToolSummary {
            name: "bash".to_owned(),
            summary: "Run whoami".to_owned(),
            ..Default::default()
        };
        let edit = SessionToolSummary {
            name: "edit".to_owned(),
            summary: "Edit example.txt".to_owned(),
            ..Default::default()
        };
        assert_eq!(tool_single_label(&command, false), "Ran whoami");
        assert_eq!(
            combined_tool_series_label(&[&command, &edit, &edit], false),
            "Ran a command, edited 2 files"
        );
        assert_eq!(
            combined_tool_series_label(&[&edit, &command, &edit], false),
            "Edited 2 files, ran a command"
        );
    }

    #[test]
    fn webfetch_series_labels_distinguish_queries_and_pages() {
        let query = SessionToolSummary {
            name: "webfetch".to_owned(),
            summary: "Web search example research".to_owned(),
            ..Default::default()
        };
        let page = SessionToolSummary {
            name: "webfetch".to_owned(),
            summary: "Web view https://example.test/a".to_owned(),
            ..Default::default()
        };
        let edit = SessionToolSummary {
            name: "edit".to_owned(),
            summary: "Edit example.txt".to_owned(),
            ..Default::default()
        };
        assert_eq!(
            tool_single_label(&query, true),
            "Web searching \"example research\""
        );
        assert_eq!(
            tool_single_label(&query, false),
            "Web searched \"example research\""
        );
        assert_eq!(
            tool_single_label(&page, true),
            "Web viewing https://example.test/a"
        );
        assert_eq!(
            tool_single_label(&page, false),
            "Web viewed https://example.test/a"
        );
        assert_eq!(
            combined_tool_series_label(&[&query, &query], true),
            "Web searching 2 queries"
        );
        assert_eq!(
            combined_tool_series_label(&[&query, &query], false),
            "Web searched 2 queries"
        );
        assert_eq!(
            combined_tool_series_label(&[&page, &page], true),
            "Web viewing 2 pages"
        );
        assert_eq!(
            combined_tool_series_label(&[&page, &page], false),
            "Web viewed 2 pages"
        );
        assert_eq!(
            combined_tool_series_label(&[&query, &page, &edit], false),
            "Web searched a query, web viewed a page, edited a file"
        );
        let old_page = SessionToolSummary {
            summary: "Web https://example.test/old".to_owned(),
            ..page.clone()
        };
        assert_eq!(
            tool_single_label(&old_page, false),
            "Web viewed https://example.test/old"
        );
    }

    #[test]
    fn surface_series_labels_use_queries_and_surface_names() {
        let edit = SessionToolSummary {
            name: "edit".to_owned(),
            summary: "Edit example.txt".to_owned(),
            ..Default::default()
        };
        for (name, summary, active, past, active_plural, past_plural, joined) in [
            (
                "surface_query",
                "Surface query: parser",
                "Searching surfaces for \"parser\"",
                "Searched surfaces for \"parser\"",
                "Searching surfaces for 2 queries",
                "Searched surfaces for 2 queries",
                "Searched surfaces, edited a file",
            ),
            (
                "surface_read",
                "Surface name: Example repository",
                "Inspecting surface of Example repository",
                "Inspected surface of Example repository",
                "Inspecting 2 surfaces",
                "Inspected 2 surfaces",
                "Inspected a surface, edited a file",
            ),
            (
                "surface_save",
                "Surface name: Example repository",
                "Persisting surface of Example repository",
                "Persisted surface of Example repository",
                "Persisting 2 surfaces",
                "Persisted 2 surfaces",
                "Persisted a surface, edited a file",
            ),
            (
                "surface_threat_model_edit",
                "Surface name: Example repository",
                "Modeling surface of Example repository",
                "Modeled surface of Example repository",
                "Modeling 2 surfaces",
                "Modeled 2 surfaces",
                "Modeled a surface, edited a file",
            ),
        ] {
            let tool = SessionToolSummary {
                name: name.to_owned(),
                summary: summary.to_owned(),
                ..Default::default()
            };
            assert_eq!(tool_single_label(&tool, true), active);
            assert_eq!(tool_single_label(&tool, false), past);
            assert_eq!(
                combined_tool_series_label(&[&tool, &tool], true),
                active_plural
            );
            assert_eq!(
                combined_tool_series_label(&[&tool, &tool], false),
                past_plural
            );
            assert_eq!(combined_tool_series_label(&[&tool, &edit], false), joined);
        }
        let prefixed = SessionToolSummary {
            name: "surface_read".to_owned(),
            summary: "Discovery: Surface name: Example repository".to_owned(),
            ..Default::default()
        };
        assert_eq!(
            tool_single_label(&prefixed, false),
            "Inspected surface of Example repository"
        );
        let old = SessionToolSummary {
            summary: "read surface".to_owned(),
            ..prefixed
        };
        assert_eq!(tool_single_label(&old, false), "Inspected a surface");
    }

    #[test]
    fn findings_series_labels_use_queries_and_titles() {
        let edit = SessionToolSummary {
            name: "edit".to_owned(),
            summary: "Edit example.txt".to_owned(),
            ..Default::default()
        };
        for (name, summary, active, past, active_plural, past_plural, joined) in [
            (
                "findings_query",
                "Finding query: parser",
                "Searching findings for \"parser\"",
                "Searched findings for \"parser\"",
                "Searching findings for 2 queries",
                "Searched findings for 2 queries",
                "Searched findings, edited a file",
            ),
            (
                "findings_read",
                "Finding title: Example finding",
                "Inspecting finding of Example finding",
                "Inspected finding of Example finding",
                "Inspecting 2 findings",
                "Inspected 2 findings",
                "Inspected a finding, edited a file",
            ),
            (
                "findings_create",
                "Finding title: Example finding",
                "Creating finding of Example finding",
                "Created finding of Example finding",
                "Creating 2 findings",
                "Created 2 findings",
                "Created a finding, edited a file",
            ),
            (
                "findings_update",
                "Finding title: Example finding",
                "Updating finding of Example finding",
                "Updated finding of Example finding",
                "Updating 2 findings",
                "Updated 2 findings",
                "Updated a finding, edited a file",
            ),
            (
                "findings_complete_state",
                "Finding title: Example finding",
                "Transitioning finding of Example finding",
                "Transitioned finding of Example finding",
                "Transitioning 2 findings",
                "Transitioned 2 findings",
                "Transitioned a finding, edited a file",
            ),
            (
                "findings_reject",
                "Finding title: Example finding",
                "Rejecting finding of Example finding",
                "Rejected finding of Example finding",
                "Rejecting 2 findings",
                "Rejected 2 findings",
                "Rejected a finding, edited a file",
            ),
        ] {
            let tool = SessionToolSummary {
                name: name.to_owned(),
                summary: summary.to_owned(),
                ..Default::default()
            };
            assert_eq!(tool_single_label(&tool, true), active);
            assert_eq!(tool_single_label(&tool, false), past);
            assert_eq!(
                combined_tool_series_label(&[&tool, &tool], true),
                active_plural
            );
            assert_eq!(
                combined_tool_series_label(&[&tool, &tool], false),
                past_plural
            );
            assert_eq!(combined_tool_series_label(&[&tool, &edit], false), joined);
        }
        let prefixed = SessionToolSummary {
            name: "findings_read".to_owned(),
            summary: "Discovery: Finding title: Example finding".to_owned(),
            ..Default::default()
        };
        assert_eq!(
            tool_single_label(&prefixed, false),
            "Inspected finding of Example finding"
        );
        let old = SessionToolSummary {
            summary: "read finding".to_owned(),
            ..prefixed
        };
        assert_eq!(tool_single_label(&old, false), "Inspected a finding");
    }

    #[test]
    fn memory_series_labels_use_queries_and_record_names() {
        let edit = SessionToolSummary {
            name: "edit".to_owned(),
            summary: "Edit example.txt".to_owned(),
            ..Default::default()
        };
        for (name, summary, active, past, active_plural, past_plural, joined) in [
            (
                "memory_session_query",
                "Memory session query: parser",
                "Searching scan for \"parser\"",
                "Searched scan for \"parser\"",
                "Searching scan for 2 queries",
                "Searched scan for 2 queries",
                "Searched scan, edited a file",
            ),
            (
                "memory_query",
                "Memory query: boundary",
                "Searching memory for \"boundary\"",
                "Searched memory for \"boundary\"",
                "Searching memory for 2 queries",
                "Searched memory for 2 queries",
                "Searched memory, edited a file",
            ),
            (
                "memory_read",
                "Memory name: Example service",
                "Reading memory of Example service",
                "Read memory of Example service",
                "Reading 2 memories",
                "Read 2 memories",
                "Read a memory, edited a file",
            ),
            (
                "memory_graph",
                "Memory name: Example service",
                "Graphing memory of Example service",
                "Graphed memory of Example service",
                "Graphing 2 memories",
                "Graphed 2 memories",
                "Graphed a memory, edited a file",
            ),
        ] {
            let tool = SessionToolSummary {
                name: name.to_owned(),
                summary: summary.to_owned(),
                ..Default::default()
            };
            assert_eq!(tool_single_label(&tool, true), active);
            assert_eq!(tool_single_label(&tool, false), past);
            assert_eq!(
                combined_tool_series_label(&[&tool, &tool], true),
                active_plural
            );
            assert_eq!(
                combined_tool_series_label(&[&tool, &tool], false),
                past_plural
            );
            assert_eq!(combined_tool_series_label(&[&tool, &edit], false), joined);
        }
        let prefixed = SessionToolSummary {
            name: "memory_read".to_owned(),
            summary: "Discovery: Memory name: Example service".to_owned(),
            ..Default::default()
        };
        assert_eq!(
            tool_single_label(&prefixed, false),
            "Read memory of Example service"
        );
        let old = SessionToolSummary {
            summary: "read memory".to_owned(),
            ..prefixed
        };
        assert_eq!(tool_single_label(&old, false), "Read a memory");
    }

    #[test]
    fn runbook_series_labels_use_queries_and_titles() {
        let edit = SessionToolSummary {
            name: "edit".to_owned(),
            summary: "Edit example.txt".to_owned(),
            ..Default::default()
        };
        for (name, summary, active, past, active_plural, past_plural, joined) in [
            (
                "runbook_query",
                "Runbook query: parser",
                "Searching runbooks for \"parser\"",
                "Searched runbooks for \"parser\"",
                "Searching runbooks for 2 queries",
                "Searched runbooks for 2 queries",
                "Searched runbooks, edited a file",
            ),
            (
                "runbook_get",
                "Runbook title: Example procedure",
                "Reading runbook of Example procedure",
                "Read runbook of Example procedure",
                "Reading 2 runbooks",
                "Read 2 runbooks",
                "Read a runbook, edited a file",
            ),
            (
                "runbook_create",
                "Runbook title: Example procedure",
                "Creating runbook of Example procedure",
                "Created runbook of Example procedure",
                "Creating 2 runbooks",
                "Created 2 runbooks",
                "Created a runbook, edited a file",
            ),
            (
                "runbook_edit",
                "Runbook title: Example procedure",
                "Editing runbook of Example procedure",
                "Edited runbook of Example procedure",
                "Editing 2 runbooks",
                "Edited 2 runbooks",
                "Edited a runbook, edited a file",
            ),
            (
                "runbook_run",
                "Runbook title: Example procedure",
                "Running runbook of Example procedure",
                "Ran runbook of Example procedure",
                "Running 2 runbooks",
                "Ran 2 runbooks",
                "Ran a runbook, edited a file",
            ),
            (
                "runbook_status",
                "Runbook title: Example procedure",
                "Checking runbook of Example procedure",
                "Checked runbook of Example procedure",
                "Checking 2 runbooks",
                "Checked 2 runbooks",
                "Checked a runbook, edited a file",
            ),
        ] {
            let tool = SessionToolSummary {
                name: name.to_owned(),
                summary: summary.to_owned(),
                ..Default::default()
            };
            assert_eq!(tool_single_label(&tool, true), active);
            assert_eq!(tool_single_label(&tool, false), past);
            assert_eq!(
                combined_tool_series_label(&[&tool, &tool], true),
                active_plural
            );
            assert_eq!(
                combined_tool_series_label(&[&tool, &tool], false),
                past_plural
            );
            assert_eq!(combined_tool_series_label(&[&tool, &edit], false), joined);
        }
        let prefixed = SessionToolSummary {
            name: "runbook_get".to_owned(),
            summary: "Discovery: Runbook title: Example procedure".to_owned(),
            ..Default::default()
        };
        assert_eq!(
            tool_single_label(&prefixed, false),
            "Read runbook of Example procedure"
        );
        let old = SessionToolSummary {
            summary: "get runbook".to_owned(),
            ..prefixed
        };
        assert_eq!(tool_single_label(&old, false), "Read a runbook");
    }

    #[test]
    fn graph_series_labels_cover_tools_and_received_messages() {
        let edit = SessionToolSummary {
            name: "edit".to_owned(),
            summary: "Edit example.txt".to_owned(),
            ..Default::default()
        };
        for (name, summary, active, past, active_plural, past_plural, joined) in [
            (
                "graph_subagents_list",
                "Graph list",
                "Checking subagents",
                "Checked subagents",
                "Checking subagents",
                "Checked subagents",
                "Checked subagents, edited a file",
            ),
            (
                "graph_subagents_inspect",
                "Graph inspect role: Scout",
                "Inspecting Scout subagent",
                "Inspected Scout subagent",
                "Inspecting 2 subagents",
                "Inspected 2 subagents",
                "Inspected a subagent, edited a file",
            ),
            (
                "graph_subagents_message",
                "Graph message role: Reviewer",
                "Sending message to Reviewer",
                "Sent message to Reviewer",
                "Sending 2 subagent messages",
                "Sent 2 messages to subagents",
                "Sent a message to a subagent, edited a file",
            ),
            (
                "graph_subagents_receive",
                "Graph receive role: Scout",
                "Receiving message from Scout",
                "Received message from Scout",
                "Receiving 2 subagent messages",
                "Received 2 subagent messages",
                "Received a subagent message, edited a file",
            ),
            (
                "graph_subagents_kill",
                "Graph kill role: Scout",
                "Stopping Scout subagent",
                "Stopped Scout subagent",
                "Stopping 2 subagents",
                "Stopped 2 subagents",
                "Stopped a subagent, edited a file",
            ),
            (
                "graph_handoff",
                "Graph handoff role: Discovery",
                "Handing off to Discovery agent",
                "Handed off to Discovery agent",
                "Handing off to 2 agents",
                "Handed off to 2 agents",
                "Handed off to an agent, edited a file",
            ),
        ] {
            let tool = SessionToolSummary {
                name: name.to_owned(),
                summary: summary.to_owned(),
                ..Default::default()
            };
            assert_eq!(tool_single_label(&tool, true), active);
            assert_eq!(tool_single_label(&tool, false), past);
            assert_eq!(
                combined_tool_series_label(&[&tool, &tool], true),
                active_plural
            );
            assert_eq!(
                combined_tool_series_label(&[&tool, &tool], false),
                past_plural
            );
            assert_eq!(combined_tool_series_label(&[&tool, &edit], false), joined);
        }
        let spawn = SessionToolSummary {
            name: "graph_subagents_spawn".to_owned(),
            summary: "Graph spawn count: 2".to_owned(),
            ..Default::default()
        };
        assert_eq!(tool_single_label(&spawn, true), "Spawning 2 subagents");
        assert_eq!(tool_single_label(&spawn, false), "Spawned 2 subagents");
        assert_eq!(
            combined_tool_series_label(&[&spawn, &spawn], true),
            "Spawning 4 subagents"
        );
        assert_eq!(
            combined_tool_series_label(&[&spawn, &spawn], false),
            "Spawned 4 subagents"
        );
        assert_eq!(
            combined_tool_series_label(&[&spawn, &edit], false),
            "Spawned 2 subagents, edited a file"
        );
        let old_spawn = SessionToolSummary {
            summary: "Spawn graph subagents".to_owned(),
            ..spawn
        };
        assert_eq!(
            combined_tool_series_label(&[&old_spawn, &old_spawn], false),
            "Spawned subagents"
        );
        let prefixed = SessionToolSummary {
            name: "graph_subagents_inspect".to_owned(),
            summary: "Discovery: Graph inspect role: Prover".to_owned(),
            ..Default::default()
        };
        assert_eq!(
            tool_single_label(&prefixed, false),
            "Inspected Prover subagent"
        );
    }

    #[test]
    fn agent_feedback_and_goal_status_series_labels() {
        let feedback = SessionToolSummary {
            name: "agent_feedback".to_owned(),
            summary: "Record agent feedback".to_owned(),
            ..Default::default()
        };
        assert!(series_tool(&feedback));
        assert_eq!(
            tool_single_label(&feedback, true),
            "Providing agent feedback"
        );
        assert_eq!(
            tool_single_label(&feedback, false),
            "Provided agent feedback"
        );
        assert_eq!(
            combined_tool_series_label(&[&feedback, &feedback], true),
            "Providing agent feedback"
        );
        assert_eq!(
            combined_tool_series_label(&[&feedback, &feedback], false),
            "Provided agent feedback"
        );

        let status = SessionToolSummary {
            name: "goal_status".to_owned(),
            summary: "Goal status: achieved".to_owned(),
            ..Default::default()
        };
        assert!(series_tool(&status));
        assert_eq!(
            tool_single_label(&status, true),
            "Marking goal status as achieved"
        );
        assert_eq!(
            tool_single_label(&status, false),
            "Marked goal status as achieved"
        );
        assert_eq!(
            combined_tool_series_label(&[&status, &status], true),
            "Marking goal status 2 times"
        );
        assert_eq!(
            combined_tool_series_label(&[&status, &status], false),
            "Marked goal status 2 times"
        );
        assert_eq!(
            combined_tool_series_label(&[&feedback, &status], false),
            "Provided agent feedback, marked goal status as achieved"
        );
        let prefixed = SessionToolSummary {
            summary: "Discovery: Goal status: waiting_on_external_state".to_owned(),
            ..status
        };
        assert_eq!(
            tool_single_label(&prefixed, false),
            "Marked goal status as waiting_on_external_state"
        );
        let old = SessionToolSummary {
            summary: "Record Goal Mode status".to_owned(),
            ..prefixed
        };
        assert_eq!(tool_single_label(&old, false), "Marked goal status");
    }

    #[test]
    fn goal_limit_fields_keep_only_positive_bounded_digits() {
        let mut edit = TextEdit::new("0");
        normalize_goal_limit(&mut edit, 120);
        assert!(edit.text.is_empty());

        let mut edit = TextEdit::new("a0012b");
        edit.set_selection(2, 6);
        normalize_goal_limit(&mut edit, 120);
        assert_eq!(edit.text, "12");
        assert_eq!((edit.anchor, edit.focus), (0, 2));

        let mut edit = TextEdit::new("1209");
        normalize_goal_limit(&mut edit, 120);
        assert_eq!(edit.text, "120");
    }

    #[test]
    fn custom_models_keep_provider_identity_in_composer() {
        let provider = |id: &str, name: &str| CustomProvider {
            id: id.to_owned(),
            name: name.to_owned(),
            base_url: "https://api.example.test/v1".to_owned(),
            models: vec![ModelEntry {
                id: "shared-model".to_owned(),
                display_name: "Shared Model".to_owned(),
            }],
            lead_model: "shared-model".to_owned(),
            small_model: "shared-model".to_owned(),
            reasoning: false,
            api_key_configured: true,
        };
        let state = State {
            custom_providers: vec![
                provider("custom-first", "First API"),
                provider("custom-second", "Second API"),
            ],
            lead_provider: Some(Provider::Custom),
            lead_custom_provider_id: "custom-second".to_owned(),
            ..State::default()
        };
        assert_eq!(
            selected_model(&state).unwrap().id,
            "custom-second::shared-model"
        );
        assert_eq!(choices(&state).len(), 2);
        let labels: Vec<_> = options(&state, MenuSection::Provider)
            .into_iter()
            .map(|option| option.label)
            .collect();
        assert_eq!(labels, ["First API", "Second API"]);
        assert_eq!(options(&state, MenuSection::Model).len(), 1);
    }

    #[test]
    fn unconfigured_custom_provider_has_no_model_choices() {
        let mut state = State {
            custom_providers: vec![CustomProvider {
                id: "custom-example".to_owned(),
                name: "Example API".to_owned(),
                base_url: "https://api.example.test/v1".to_owned(),
                models: vec![ModelEntry {
                    id: "example-model".to_owned(),
                    display_name: "Example Model".to_owned(),
                }],
                lead_model: "example-model".to_owned(),
                small_model: "example-model".to_owned(),
                reasoning: false,
                api_key_configured: false,
            }],
            ..State::default()
        };
        assert!(!has_configured_provider(&state));
        assert!(choices(&state).is_empty());
        state.custom_providers[0].api_key_configured = true;
        assert!(has_configured_provider(&state));
        assert_eq!(choices(&state).len(), 1);
    }

    #[test]
    fn configured_openai_remains_in_provider_menu_after_catalog_failure() {
        let mut state = State {
            auth_loaded: true,
            lead_provider: Some(Provider::OpenAi),
            ..State::default()
        };
        state.auth_status[Provider::OpenAi.index()] = AuthStatus {
            subscription: true,
            ..AuthStatus::default()
        };
        state.model_errors[crate::catalog_index(
            Provider::OpenAi,
            beale_contracts::v1::AuthMethod::Subscription,
        )] = "Model catalog unavailable".to_owned();
        assert!(choices(&state).is_empty());
        let providers = options(&state, MenuSection::Provider);
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].label, "OpenAI · Retry models");
        super::toggle_menu(&mut state);
        assert!(state.composer_menu_open);
        assert_eq!(
            state.composer_menu_navigation.section,
            MenuSection::Provider
        );

        state.composer_menu_open = false;
        state.auth_status[Provider::Xai.index()].subscription = true;
        state.model_catalogs
            [crate::catalog_index(Provider::Xai, beale_contracts::v1::AuthMethod::Subscription)] =
            Some(crate::provider_models::ModelCatalog {
                models: vec![crate::provider_models::ModelEntry {
                    id: "example-model".to_owned(),
                    name: "Example Model".to_owned(),
                }],
                lead: "example-model".to_owned(),
                small: "example-model".to_owned(),
            });
        assert!(selected_model(&state).is_none());
        assert_eq!(options(&state, MenuSection::Provider).len(), 2);
        super::toggle_menu(&mut state);
        assert_eq!(
            state.composer_menu_navigation.section,
            MenuSection::Provider
        );
    }

    #[test]
    fn work_duration_uses_hours_minutes_and_seconds() {
        assert_eq!(work_duration(0), "0s");
        assert_eq!(work_duration(999), "0s");
        assert_eq!(work_duration(28_999), "28s");
        assert_eq!(work_duration(60_000), "1m");
        assert_eq!(work_duration(62_000), "1m 2s");
        assert_eq!(work_duration(3_600_000), "1h");
        assert_eq!(work_duration(3_661_999), "1h 1m 1s");
    }

    #[test]
    fn transcript_links_only_open_web_destinations() {
        assert!(is_web_link("https://example.test/research"));
        assert!(is_web_link("http://example.test"));
        assert!(!is_web_link("file:///example.txt"));
        assert!(!is_web_link("javascript:alert(1)"));
        assert!(!is_web_link("https://"));
    }
}
