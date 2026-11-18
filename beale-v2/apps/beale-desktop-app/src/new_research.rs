use beale_contracts::v1::{
    AuthMethod, GoalStatus, ReasoningEffort, SessionRole, SessionStreamKind, SessionToolSummary,
};
use rune::{
    Align, Color, El, Key, KeyEvent, LineCap, Path, ScrollSpec, Shape, Side, TextEdit, Transition,
    Tx, canvas, col, el, icon, row, text, text_input,
};
use rune_kit::{Markdown, MarkdownStyle};

use crate::{
    Provider, SelectionMenu, State, ThemeStyle, catalog_index, effective_model_method, form_button,
    icons::{
        AGENT, ARROW_UP, BRAIN, CHECK, CHEVRON_DOWN, CHEVRON_RIGHT, CHEVRON_UP, FOLDER_OPEN,
        REPEAT, STOP, TARGET, TERMINAL, WRENCH,
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
    pub(crate) role: beale_graphs::Role,
    pub(crate) models: Vec<ModelSelection>,
    pub(crate) model_menu_open: bool,
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

fn options(state: &State, section: MenuSection) -> Vec<MenuOption> {
    let choices = choices(state);
    let selected = selected(state, &choices).map(|choice| choice.selection.clone());
    match section {
        MenuSection::Provider => {
            let mut options: Vec<_> = Provider::ALL
                .into_iter()
                .filter_map(|provider| {
                    let available = choices
                        .iter()
                        .find(|choice| {
                            choice.selection.provider == provider && choice.provider_default
                        })
                        .or_else(|| {
                            choices
                                .iter()
                                .find(|choice| choice.selection.provider == provider)
                        })
                        .map(|choice| MenuOption {
                            label: provider.name().to_owned(),
                            selected: selected
                                .as_ref()
                                .is_some_and(|selection| selection.provider == provider),
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
                        selected: selected.as_ref().is_some_and(|selection| {
                            custom_provider_id(selection) == Some(provider.id.as_str())
                        }),
                        choice: MenuChoice::Model(choice.selection.clone()),
                    });
                }
            }
            options
        }
        MenuSection::Model => choices
            .into_iter()
            .filter(|choice| {
                selected.as_ref().is_some_and(|selection| {
                    choice.selection.provider == selection.provider
                        && choice.selection.method == selection.method
                        && (selection.provider != Provider::Custom
                            || custom_provider_id(&choice.selection)
                                == custom_provider_id(selection))
                })
            })
            .map(|choice| MenuOption {
                label: choice.name,
                selected: selected.as_ref() == Some(&choice.selection),
                choice: MenuChoice::Model(choice.selection),
            })
            .collect(),
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

fn select_option(state: &mut State, index: usize) {
    if !state.composer_menu_open {
        return;
    }
    if let Some(option) = options(state, state.composer_menu_section).get(index) {
        match &option.choice {
            MenuChoice::Model(selection) => {
                state.new_research_model = Some(selection.clone());
                if let Some(models) =
                    state.new_research_graph_models[beale_graphs::Role::Main.index()].as_mut()
                {
                    models.retain(|item| item != selection);
                    models.insert(0, selection.clone());
                    models.truncate(beale_graphs::Role::Main.max_models());
                }
            }
            MenuChoice::Effort(effort) => state.new_research_effort = *effort,
            MenuChoice::RetryModels(provider, method) => {
                crate::start_model_list(state, *provider, *method);
            }
            MenuChoice::LoadingModels => return,
        }
        state.composer_menu_open = false;
    }
}

fn set_section(state: &mut State, section: MenuSection) {
    if state.composer_menu_open && state.composer_menu_section != section {
        state.composer_menu_section = section;
        state.composer_menu_cursor = options(state, section)
            .iter()
            .position(|option| option.selected)
            .unwrap_or(0);
        state.composer_menu_reveal_pending = true;
    }
}

pub(crate) fn toggle_menu(state: &mut State) {
    if state.composer_menu_open {
        state.composer_menu_open = false;
        return;
    }
    if choices(state).is_empty() && !has_configured_provider(state) {
        return;
    }
    state.selection_menu = None;
    state.composer_menu_open = true;
    state.composer_menu_section = if selected_model(state).is_none() {
        MenuSection::Provider
    } else {
        MenuSection::Model
    };
    state.composer_menu_cursor = options(state, state.composer_menu_section)
        .iter()
        .position(|option| option.selected)
        .unwrap_or(0);
    state.composer_menu_reveal_pending = true;
}

pub(crate) fn menu_key(state: &mut State, event: &KeyEvent, _: &mut Tx) -> bool {
    if !state.composer_menu_open {
        return false;
    }
    let count = options(state, state.composer_menu_section).len();
    match event.key {
        Key::Escape => state.composer_menu_open = false,
        Key::Left => set_section(
            state,
            match state.composer_menu_section {
                MenuSection::Provider => MenuSection::Provider,
                MenuSection::Model => MenuSection::Provider,
                MenuSection::Effort => MenuSection::Model,
            },
        ),
        Key::Right => set_section(
            state,
            match state.composer_menu_section {
                MenuSection::Provider => MenuSection::Model,
                MenuSection::Model | MenuSection::Effort => MenuSection::Effort,
            },
        ),
        Key::Down if count > 0 => {
            state.composer_menu_cursor = (state.composer_menu_cursor + 1) % count;
            state.composer_menu_reveal_pending = true;
        }
        Key::Up if count > 0 => {
            state.composer_menu_cursor = (state.composer_menu_cursor + count - 1) % count;
            state.composer_menu_reveal_pending = true;
        }
        Key::Home => {
            state.composer_menu_cursor = 0;
            state.composer_menu_reveal_pending = true;
        }
        Key::End if count > 0 => {
            state.composer_menu_cursor = count - 1;
            state.composer_menu_reveal_pending = true;
        }
        Key::Enter if count > 0 => select_option(state, state.composer_menu_cursor),
        _ => return false,
    }
    true
}

fn section_row(
    section: MenuSection,
    label: &'static str,
    value: String,
    active: bool,
    theme: ThemeStyle<'_>,
) -> El {
    row()
        .key(match section {
            MenuSection::Provider => "composer-menu-provider",
            MenuSection::Model => "composer-menu-model",
            MenuSection::Effort => "composer-menu-effort",
        })
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
        .on_hover(move |state: &mut State| set_section(state, section))
        .on_press(move |state: &mut State| set_section(state, section))
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
                .truncate()
                .font(ui_font(13.5, 450.0))
                .color(theme.color("#eeeeee")),
        )
        .child(icon(&CHEVRON_RIGHT, 13.0).color(theme.color("#969696")))
}

pub(crate) fn model_menu(state: &State, theme: ThemeStyle<'_>) -> El {
    let choices = choices(state);
    let selected = selected(state, &choices);
    let provider_name = selected.map_or_else(
        || state.lead_provider.map_or("None", Provider::name),
        |choice| provider_label(state, &choice.selection),
    );
    let model_name = selected.map_or("None", |choice| choice.name.as_str());
    let main = in_place_menu::popup_surface(
        col()
            .key("composer-model-main")
            .role("menu")
            .label("Lead model settings")
            .w(250.0)
            .padding(8.0)
            .gap(2.0)
            .child(section_row(
                MenuSection::Provider,
                "Provider",
                provider_name.to_owned(),
                state.composer_menu_section == MenuSection::Provider,
                theme,
            ))
            .child(section_row(
                MenuSection::Model,
                "Model",
                model_name.to_owned(),
                state.composer_menu_section == MenuSection::Model,
                theme,
            ))
            .child(section_row(
                MenuSection::Effort,
                "Effort",
                state.new_research_effort.label().to_owned(),
                state.composer_menu_section == MenuSection::Effort,
                theme,
            )),
        theme.color("#242424"),
    )
    .self_end();
    let options = options(state, state.composer_menu_section);
    let height = (options.len() as f32 * 36.0 + 12.0).min(300.0);
    let mut list = col().w_full().gap(2.0);
    for (index, option) in options.into_iter().enumerate() {
        list = list.child(
            row()
                .key(format!("composer-menu-option-{index}"))
                .role("menuitemradio")
                .label(option.label.clone())
                .w_full()
                .h(34.0)
                .px(10.0)
                .center_y()
                .gap(6.0)
                .radius(7.0)
                .fill(if index == state.composer_menu_cursor {
                    theme.color("#383838")
                } else {
                    Color::TRANSPARENT
                })
                .hover(theme.color("#424242"))
                .on_hover(move |state: &mut State| state.composer_menu_cursor = index)
                .on_press(move |state: &mut State| select_option(state, index))
                .child(
                    text(option.label)
                        .grow(1.0)
                        .min_w(0.0)
                        .truncate()
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
            .key("composer-model-submenu")
            .role("menu")
            .label(match state.composer_menu_section {
                MenuSection::Provider => "Provider options",
                MenuSection::Model => "Model options",
                MenuSection::Effort => "Effort options",
            })
            .w(230.0)
            .h(height)
            .min_h(0.0)
            .padding(6.0)
            .child(
                col()
                    .key("composer-model-options-scroll")
                    .w_full()
                    .grow(1.0)
                    .min_h(0.0)
                    .scroll(ScrollSpec {
                        stick_bottom: false,
                    })
                    .child(list),
            ),
        theme.color("#242424"),
    )
    .self_end();
    row()
        .key("composer-model-menu")
        .w(486.0)
        .h(300.0)
        .min_h(0.0)
        .gap(6.0)
        .child(main)
        .child(submenu)
        .anchored_fit("new-research-model-control", Side::Top, Align::Start, 6.0)
        .modal(true, |state: &mut State| state.composer_menu_open = false)
        .on_layout_tx(|state: &mut State, _, tx| {
            if state.composer_menu_reveal_pending {
                state.composer_menu_reveal_pending = false;
                tx.scroll_into_view_in(
                    "composer-model-options-scroll",
                    format!("composer-menu-option-{}", state.composer_menu_cursor),
                );
            }
        })
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
    let body = ui_font(if muted { 13.0 } else { 14.0 }, 400.0);
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
        .heading_font(1, ui_font(if muted { 16.0 } else { 20.0 }, heading_weight))
        .heading_font(2, ui_font(if muted { 15.0 } else { 18.0 }, heading_weight))
        .heading_font(3, ui_font(if muted { 14.0 } else { 16.0 }, heading_weight))
        .code_block_font(ui_font(13.0, 400.0).mono());
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

fn role_models(state: &State, role: beale_graphs::Role) -> Vec<ModelSelection> {
    let mut models: Vec<_> = state.new_research_graph_models[role.index()]
        .clone()
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

pub(crate) fn graph_concurrency_config(
    state: &State,
) -> Vec<beale_contracts::v1::GraphNodeConcurrency> {
    beale_graphs::graph_definition(&state.new_research_graph_id)
        .unwrap_or(&beale_graphs::SIMPLE_GRAPH)
        .nodes
        .iter()
        .filter_map(|role| {
            (if role.is_top_level() {
                Some(1)
            } else {
                state.new_research_graph_concurrency[role.index()]
            })
            .map(|max_concurrent| beale_contracts::v1::GraphNodeConcurrency {
                node_id: role.id().to_owned(),
                max_concurrent,
            })
        })
        .collect()
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

#[derive(Clone, Copy)]
struct GraphNodePlacement {
    role: beale_graphs::Role,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

const GRAPH_TOP_INSET: f32 = 8.0;

pub(crate) fn graph_model_label(state: &State, role: beale_graphs::Role) -> String {
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

fn graph_node_width(state: &State, role: beale_graphs::Role, max_width: f32) -> f32 {
    let role_width = rune::text::measure(role.label(), &[], ui_font(12.0, 400.0), None).0
        + 19.0
        + (if role.is_top_level() {
            Some(1)
        } else {
            state.new_research_graph_concurrency[role.index()]
        })
        .map_or(0.0, |max| {
            rune::text::measure(&format!("{max} Max"), &[], ui_font(11.0, 400.0), None).0 + 4.0
        });
    let model_width = rune::text::measure(
        &graph_model_label(state, role),
        &[],
        ui_font(12.0, 400.0),
        None,
    )
    .0 + if role_models(state, role).is_empty() {
        0.0
    } else {
        18.0
    };
    (role_width.max(model_width) + 20.0).ceil().min(max_width)
}

fn graph_node(state: &State, placement: GraphNodePlacement, theme: ThemeStyle<'_>) -> El {
    let role = placement.role;
    let models = role_models(state, role);
    let model = models.first();
    let model_label = graph_model_label(state, role);
    let mut model_row = row().w_full().min_w(0.0).center_y().gap(5.0);
    if let Some(selection) = model {
        model_row =
            model_row.child(icon(selection.provider.logo(), 13.0).color(theme.color("#999999")));
    }
    model_row = model_row.child(
        text(model_label)
            .key(format!("new-research-graph-model-label-{}", role.id()))
            .min_w(0.0)
            .grow(1.0)
            .truncate()
            .font(ui_font(12.0, 400.0))
            .color(theme.color("#999999")),
    );
    let max_concurrent = if role.is_top_level() {
        Some(1)
    } else {
        state.new_research_graph_concurrency[role.index()]
    };
    let mut role_row = row().w_full().h(18.0).center_y().gap(4.0).child(
        icon(&AGENT, 15.0)
            .key(format!("new-research-graph-agent-icon-{}", role.id()))
            .color(theme.color("#eeeeee")),
    );
    role_row = role_row.child(
        text(role.label())
            .grow(1.0)
            .font(ui_font(12.0, 400.0))
            .color(theme.color("#eeeeee")),
    );
    if let Some(max) = max_concurrent {
        role_row = role_row.child(
            text(format!("{max} Max"))
                .key(format!("new-research-graph-node-{}-max", role.id()))
                .font(ui_font(11.0, 400.0))
                .color(theme.color("#999999")),
        );
    }
    col()
        .key(format!("new-research-graph-node-{}", role.id()))
        .role("button")
        .label(format!("Edit {} allowed models", role.label()))
        .absolute(placement.x, placement.y)
        .size(placement.width, placement.height)
        .padding(9.0)
        .gap(5.0)
        .radius(16.0)
        .continuous_corners()
        .fill(theme.color("#202020"))
        .border(1.0, theme.color("#3a3a3a"))
        .hover(theme.color("#2a2a2a"))
        .on_press(move |state: &mut State| {
            state.graph_editor = Some(GraphEditor {
                role,
                models: role_models(state, role),
                model_menu_open: false,
            });
            state.graph_editor_max_concurrent = TextEdit::new(
                state.new_research_graph_concurrency[role.index()]
                    .map_or_else(String::new, |value| value.to_string()),
            );
        })
        .child(role_row)
        .child(model_row)
}

fn graph_preview(state: &State, theme: ThemeStyle<'_>, available_width: f32) -> El {
    let graph = &beale_graphs::SIMPLE_GRAPH;
    let stage_width = available_width.clamp(180.0, 640.0);
    let preferred_widths: Vec<_> = graph
        .nodes
        .iter()
        .map(|role| graph_node_width(state, *role, stage_width))
        .collect();
    let columns = if stage_width >= 560.0
        && preferred_widths[1..]
            .iter()
            .all(|width| *width <= (stage_width - 24.0) / 3.0)
    {
        3
    } else if stage_width >= 380.0
        && preferred_widths[1..3]
            .iter()
            .all(|width| *width <= (stage_width - 12.0) / 2.0)
    {
        2
    } else {
        1
    };
    let gap = 12.0;
    let slot_width = (stage_width - gap * (columns - 1) as f32) / columns as f32;
    let layout: Vec<_> = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(index, role)| {
            let width = preferred_widths[index].min(
                if (columns == 3 && index > 0) || (columns == 2 && (index == 1 || index == 2)) {
                    slot_width
                } else {
                    stage_width
                },
            );
            let (x, y) = match (columns, index) {
                (3, 0) => ((stage_width - width) / 2.0, GRAPH_TOP_INSET),
                (3, _) => {
                    let slot = index - 1;
                    (
                        slot as f32 * (slot_width + gap) + (slot_width - width) / 2.0,
                        154.0 + GRAPH_TOP_INSET,
                    )
                }
                (2, 1) => ((slot_width - width) / 2.0, 125.0 + GRAPH_TOP_INSET),
                (2, 2) => (
                    slot_width + gap + (slot_width - width) / 2.0,
                    125.0 + GRAPH_TOP_INSET,
                ),
                (2, 3) => ((stage_width - width) / 2.0, 250.0 + GRAPH_TOP_INSET),
                (2, _) => ((stage_width - width) / 2.0, GRAPH_TOP_INSET),
                _ => (
                    (stage_width - width) / 2.0,
                    index as f32 * 85.0 + GRAPH_TOP_INSET,
                ),
            };
            GraphNodePlacement {
                role: *role,
                x,
                y,
                width,
                height: 56.0,
            }
        })
        .collect();
    let stage_height = if columns == 3 {
        240.0 + GRAPH_TOP_INSET
    } else if columns == 2 {
        340.0 + GRAPH_TOP_INSET
    } else {
        315.0 + GRAPH_TOP_INSET
    };
    let mut shape = Shape::new();
    for edge in graph.edges {
        let (Some(from), Some(to)) = (
            layout.iter().find(|node| node.role == edge.from),
            layout.iter().find(|node| node.role == edge.to),
        ) else {
            continue;
        };
        let path = if stage_width < 380.0 {
            let source = [from.x, from.y + from.height / 2.0];
            let target = [to.x, to.y + to.height / 2.0];
            let rail = (from.x - 9.0).max(0.0);
            Path::new()
                .move_to(source)
                .cubic_to([rail, source[1]], [rail, target[1]], target)
        } else {
            let source = [from.x + from.width / 2.0, from.y + from.height];
            let target = [to.x + to.width / 2.0, to.y];
            let middle = (source[1] + target[1]) / 2.0;
            Path::new()
                .move_to(source)
                .cubic_to([source[0], middle], [target[0], middle], target)
        };
        shape = shape.stroke_path(&path, 1.5, LineCap::Round, theme.color("#565656"));
    }
    let mut stage = el()
        .key("new-research-simple-graph")
        .size(stage_width, stage_height)
        .child(
            canvas(shape)
                .absolute(0.0, 0.0)
                .size(stage_width, stage_height),
        );
    for placement in layout {
        stage = stage.child(graph_node(state, placement, theme));
    }
    row().w_full().center().child(stage)
}

fn flow_graph_preview(state: &State, theme: ThemeStyle<'_>, available_width: f32) -> El {
    let Some(graph) = beale_graphs::graph_definition(&state.new_research_graph_id) else {
        return main_only_preview(state, theme, available_width);
    };
    let stage_width = available_width.clamp(180.0, 640.0);
    let roots: Vec<_> = graph
        .nodes
        .iter()
        .copied()
        .filter(|role| role.is_top_level())
        .collect();
    let specialists: Vec<_> = graph
        .nodes
        .iter()
        .copied()
        .filter(|role| !role.is_top_level())
        .collect();
    let columns = if stage_width >= 420.0 { 2 } else { 1 };
    let gap = 12.0;
    let slot_width = (stage_width - gap * (columns - 1) as f32) / columns as f32;
    let mut placements = Vec::new();
    for (index, role) in roots.iter().enumerate() {
        let width = graph_node_width(state, *role, slot_width).min(slot_width);
        let column = index % columns;
        let row = index / columns;
        placements.push(GraphNodePlacement {
            role: *role,
            x: column as f32 * (slot_width + gap) + (slot_width - width) / 2.0,
            y: GRAPH_TOP_INSET + row as f32 * 96.0,
            width,
            height: 56.0,
        });
    }
    let root_rows = roots.len().div_ceil(columns);
    let specialist_top = GRAPH_TOP_INSET + root_rows as f32 * 96.0 + 28.0;
    let specialist_columns = if stage_width >= 570.0 {
        3
    } else if stage_width >= 360.0 {
        2
    } else {
        1
    };
    let specialist_slot =
        (stage_width - gap * (specialist_columns - 1) as f32) / specialist_columns as f32;
    for (index, role) in specialists.iter().enumerate() {
        let width = graph_node_width(state, *role, specialist_slot).min(specialist_slot);
        let column = index % specialist_columns;
        placements.push(GraphNodePlacement {
            role: *role,
            x: column as f32 * (specialist_slot + gap) + (specialist_slot - width) / 2.0,
            y: specialist_top + (index / specialist_columns) as f32 * 78.0,
            width,
            height: 56.0,
        });
    }
    let specialist_rows = specialists.len().div_ceil(specialist_columns);
    let stage_height = specialist_top + specialist_rows as f32 * 78.0 + 8.0;
    let mut shape = Shape::new();
    for edge in graph
        .edges
        .iter()
        .filter(|edge| edge.from.is_top_level() && edge.to.is_top_level())
    {
        let (Some(from), Some(to)) = (
            placements.iter().find(|node| node.role == edge.from),
            placements.iter().find(|node| node.role == edge.to),
        ) else {
            continue;
        };
        let source = [from.x + from.width / 2.0, from.y + from.height / 2.0];
        let target = [to.x + to.width / 2.0, to.y + to.height / 2.0];
        shape = shape.stroke_path(
            &Path::new().move_to(source).line_to(target),
            1.5,
            LineCap::Round,
            theme.color("#565656"),
        );
    }
    let mut stage = el()
        .key(format!("new-research-{}-graph", graph.id))
        .size(stage_width, stage_height)
        .child(
            canvas(shape)
                .absolute(0.0, 0.0)
                .size(stage_width, stage_height),
        )
        .child(
            text("SUBAGENTS FOR EACH STAGE")
                .absolute(0.0, specialist_top - 30.0)
                .font(ui_font(10.0, 600.0))
                .color(theme.color("#888888")),
        );
    for placement in placements {
        stage = stage.child(graph_node(state, placement, theme));
    }
    row().w_full().center().child(stage)
}

fn main_only_preview(state: &State, theme: ThemeStyle<'_>, available_width: f32) -> El {
    let stage_width = available_width.clamp(180.0, 640.0);
    let width = graph_node_width(state, beale_graphs::Role::Main, stage_width);
    let main = GraphNodePlacement {
        role: beale_graphs::Role::Main,
        x: (stage_width - width) / 2.0,
        y: GRAPH_TOP_INSET,
        width,
        height: 56.0,
    };
    row().w_full().center().child(
        el().key("new-research-no-graph")
            .size(stage_width, 71.0 + GRAPH_TOP_INSET)
            .child(graph_node(state, main, theme)),
    )
}

fn close_graph_editor(state: &mut State) {
    state.graph_editor = None;
    state.graph_editor_max_concurrent.clear();
}

pub(crate) fn graph_editor_dialog(state: &State, theme: ThemeStyle<'_>) -> [El; 2] {
    let editor = state.graph_editor.as_ref().expect("dialog requires editor");
    let model_choices = choices(state);
    let model_label = match editor.models.as_slice() {
        [] => "Choose models".to_owned(),
        [selection] => model_choices
            .iter()
            .find(|choice| choice.selection == *selection)
            .map_or_else(|| "Choose models".to_owned(), |choice| choice.name.clone()),
        models => format!("{} models selected", models.len()),
    };
    let model_control = selection_button(
        model_label,
        editor.model_menu_open,
        |state| {
            if let Some(editor) = state.graph_editor.as_mut() {
                editor.model_menu_open = !editor.model_menu_open;
            }
        },
        theme,
    )
    .key("graph-editor-model-control");
    let max_concurrent = if editor.role.is_top_level() {
        text("1")
            .key("graph-editor-fixed-max-concurrent")
            .font(ui_font(14.0, 400.0))
            .color(theme.color("#bdbdbd"))
    } else {
        field(
            &state.graph_editor_max_concurrent,
            "graph-editor-max-concurrent",
            "Unlimited",
            theme,
            |state| &mut state.graph_editor_max_concurrent,
        )
        .w(110.0)
        .on_input_change(|state: &mut State, _| {
            normalize_goal_limit(
                &mut state.graph_editor_max_concurrent,
                beale_graphs::MAX_CONCURRENT as u64,
            );
        })
    };
    let no_graph = state.new_research_graph_id.is_empty();
    let mut rows = vec![
        row_with_control(
            "Role",
            if no_graph {
                "This session's agent."
            } else {
                "Role in this session graph."
            },
            text(editor.role.label())
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#bdbdbd")),
            theme,
        ),
        row_with_control(
            if editor.role.is_top_level() {
                "Model"
            } else {
                "Allowed Models"
            },
            if editor.role.is_top_level() {
                "Choose one model for this top-level agent."
            } else {
                "Choose up to 16 models for this role."
            },
            model_control,
            theme,
        ),
    ];
    rows.push(row_with_control(
        "Max Concurrent",
        if editor.role.is_top_level() {
            "This top-level agent runs one turn at a time."
        } else {
            "Maximum simultaneous agents for this role."
        },
        max_concurrent,
        theme,
    ));
    let body = settings_form_card(rows, theme);
    let mut save = form_button("Save", theme).key("graph-editor-save");
    if !editor.models.is_empty() {
        save = save.on_press(|state: &mut State| {
            if let Some(editor) = state.graph_editor.take() {
                if editor.role.is_top_level() {
                    state.new_research_graph_concurrency[editor.role.index()] = Some(1);
                } else if !state.new_research_graph_id.is_empty() {
                    state.new_research_graph_concurrency[editor.role.index()] = state
                        .graph_editor_max_concurrent
                        .text
                        .parse()
                        .ok()
                        .or_else(|| editor.role.default_max_concurrent());
                }
                state.graph_editor_max_concurrent.clear();
                if Some(editor.role) == beale_graphs::entry_role(&state.new_research_graph_id)
                    || (state.new_research_graph_id.is_empty()
                        && editor.role == beale_graphs::Role::Main)
                {
                    state.new_research_model = editor.models.first().cloned();
                }
                state.new_research_graph_models[editor.role.index()] = Some(editor.models);
            }
        });
    } else {
        save = save.opacity(0.45);
    }
    modal_dialog::render(
        modal_dialog::DialogSpec {
            key: "graph-editor-dialog",
            title: if no_graph {
                "Edit Main Agent"
            } else {
                "Edit Graph Node"
            },
            width: 520.0,
            focus_return: "new-research-graph-control",
        },
        body,
        row()
            .w_full()
            .gap(8.0)
            .child(el().grow(1.0))
            .child(form_button("Cancel", theme).on_press(close_graph_editor))
            .child(save),
        theme,
        close_graph_editor,
    )
}

pub(crate) fn graph_editor_model_menu(state: &State, theme: ThemeStyle<'_>) -> El {
    let editor = state.graph_editor.as_ref().expect("menu requires editor");
    let model_choices = choices(state);
    let height = (model_choices.len() as f32 * 44.0 + 12.0).clamp(58.0, 320.0);
    let mut list = col().w_full().gap(2.0);
    if model_choices.is_empty() {
        list = list.child(
            text("Connect a provider to choose models for this role.")
                .padding(10.0)
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#aaaaaa")),
        );
    }
    for choice in model_choices {
        let selection = choice.selection.clone();
        let checked = editor.models.contains(&selection);
        let single_model = editor.role.is_top_level();
        list = list.child(
            row()
                .key(format!(
                    "graph-editor-model-{}-{}",
                    choice.selection.provider.name(),
                    choice.selection.id
                ))
                .role(if single_model { "radio" } else { "checkbox" })
                .label(format!(
                    "{} {} ({})",
                    choice.selection.provider.name(),
                    choice.name,
                    if checked { "selected" } else { "not selected" }
                ))
                .w_full()
                .h(42.0)
                .px(10.0)
                .center_y()
                .gap(8.0)
                .radius(8.0)
                .hover(theme.color("#383838"))
                .on_press(move |state: &mut State| {
                    if let Some(editor) = state.graph_editor.as_mut() {
                        if single_model {
                            editor.models.clear();
                            editor.models.push(selection.clone());
                            editor.model_menu_open = false;
                        } else if let Some(index) =
                            editor.models.iter().position(|item| item == &selection)
                        {
                            editor.models.remove(index);
                        } else if editor.models.len() < editor.role.max_models() {
                            editor.models.push(selection.clone());
                        }
                    }
                })
                .child(icon(choice.selection.provider.logo(), 16.0).color(theme.color("#bbbbbb")))
                .child(
                    text(choice.name)
                        .grow(1.0)
                        .truncate()
                        .font(ui_font(14.0, 400.0))
                        .color(theme.color("#eeeeee")),
                )
                .child(if checked {
                    icon(&CHECK, 15.0).color(theme.color("#eeeeee"))
                } else {
                    el().size(15.0, 15.0)
                }),
        );
    }
    in_place_menu::popup_surface(
        col()
            .key("graph-editor-model-menu")
            .role("menu")
            .label("Allowed models")
            .w(280.0)
            .h(height)
            .min_h(0.0)
            .padding(6.0)
            .child(
                col()
                    .key("graph-editor-model-menu-scroll")
                    .w_full()
                    .grow(1.0)
                    .min_h(0.0)
                    .scroll(ScrollSpec {
                        stick_bottom: false,
                    })
                    .child(list),
            ),
        theme.color("#242424"),
    )
    .anchored_fit("graph-editor-model-control", Side::Bottom, Align::End, 4.0)
    .modal(true, |state: &mut State| {
        if let Some(editor) = state.graph_editor.as_mut() {
            editor.model_menu_open = false;
        }
    })
}

fn new_research_setup(state: &State, theme: ThemeStyle<'_>, available_width: f32) -> El {
    let workspace_name = state
        .selected_research_workspace()
        .map(|workspace| workspace.name.as_str());
    let heading = if let Some(name) = workspace_name {
        let workspace = row()
            .key("new-research-heading-workspace-control")
            .role("button")
            .label(format!("Choose workspace: {name}"))
            .help(name)
            .min_w(0.0)
            .center_y()
            .gap(6.0)
            .hover_group()
            .on_press(|state: &mut State| {
                toggle_selection_menu(state, SelectionMenu::ResearchWorkspaceHeading)
            })
            .child(
                text(name)
                    .min_w(0.0)
                    .truncate_fade()
                    .font(crate::view_title_font())
                    .color(theme.color("#eeeeee"))
                    .group_color(theme.color("#aaaaaa")),
            )
            .child(
                icon(&CHEVRON_DOWN, 15.0)
                    .key("new-research-heading-workspace-chevron")
                    .color(theme.color("#969696")),
            );
        crate::design::breadcrumb_title("New Research", workspace, theme)
            .key("new-research-workspace-heading")
    } else {
        crate::design::breadcrumb_title(
            "Home",
            text("New Research")
                .role("heading")
                .truncate_fade()
                .font(crate::view_title_font())
                .color(theme.color("#eeeeee")),
            theme,
        )
        .key("new-research-workspace-heading")
    };
    let mut content = col()
        .key("new-research-setup")
        .w_full()
        .max_w(750.0)
        .self_center()
        .gap(18.0)
        .px(20.0)
        .child(heading);
    if crate::extensions_view::graphs_enabled(state) {
        let selected = !state.new_research_graph_id.is_empty();
        let active = state.selection_menu == Some(SelectionMenu::SessionGraph);
        let label = beale_graphs::graph_definition(&state.new_research_graph_id)
            .map_or("No Graph", |graph| graph.name);
        let selector = selection_button(
            label,
            active,
            |state: &mut State| toggle_selection_menu(state, SelectionMenu::SessionGraph),
            theme,
        )
        .key("new-research-graph-control")
        .role("button")
        .label(format!("Session graph: {label}"));
        content = content.child(row().w_full().py(6.0).center().child(selector));
        if state.new_research_graph_id == beale_graphs::SEQUENTIAL_ID {
            content = content.child(
                row()
                    .w_full()
                    .center()
                    .gap(10.0)
                    .child(
                        text("Minutes per stage")
                            .font(ui_font(13.0, 450.0))
                            .color(theme.color("#bdbdbd")),
                    )
                    .child(
                        field(
                            &state.new_research_stage_duration_minutes,
                            "new-research-stage-duration-minutes",
                            "60",
                            theme,
                            |state| &mut state.new_research_stage_duration_minutes,
                        )
                        .w(86.0)
                        .on_input_change(|state: &mut State, _| {
                            normalize_goal_limit(
                                &mut state.new_research_stage_duration_minutes,
                                1_440,
                            );
                        }),
                    ),
            );
        }
        content = content.child(
            if selected && state.new_research_graph_id == beale_graphs::SIMPLE_ID {
                graph_preview(state, theme, available_width)
            } else if selected {
                flow_graph_preview(state, theme, available_width)
            } else {
                main_only_preview(state, theme, available_width)
            },
        );
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
    let message = if user {
        text(content)
            .key(format!("{key}-text"))
            .font(ui_font(14.0, 400.0))
            .color(theme.color("#eeeeee"))
            .wrap()
            .selectable(true)
            .selection_color(rune_kit::Kit::neutral(theme.is_dark()).selection())
    } else {
        transcript_markdown(content, theme, streaming, false).key(format!("{key}-markdown"))
    };
    if user {
        row().key(key.clone()).w_full().child(el().grow(1.0)).child(
            col()
                .key(format!("{key}-bubble"))
                .max_w(620.0)
                .shrink()
                .px(13.0)
                .py(10.0)
                .radius(14.0)
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

fn tool_series_kind(tool: &SessionToolSummary) -> Option<&'static str> {
    match tool.name.as_str() {
        "read" => Some("read"),
        "write" => Some("write"),
        "edit" => Some("edit"),
        "bash" => Some("bash"),
        "webfetch" => webfetch_input(tool).map(|(kind, _)| kind),
        _ => None,
    }
}

pub(crate) fn series_tool(tool: &SessionToolSummary) -> bool {
    tool_series_kind(tool).is_some()
}

fn tool_single_label(tool: &SessionToolSummary, active: bool) -> String {
    if let Some((kind, value)) = webfetch_input(tool) {
        return match (kind, active) {
            ("webfetch.query", true) => format!("Web searching \"{value}\""),
            ("webfetch.query", false) => format!("Web searched \"{value}\""),
            ("webfetch.url", true) => format!("Web viewing {value}"),
            _ => format!("Web viewed {value}"),
        };
    }
    let (present, past, prefix) = match tool.name.as_str() {
        "read" => ("Reading", "Read", "Read "),
        "write" => ("Writing", "Wrote", "Write "),
        "edit" => ("Editing", "Edited", "Edit "),
        "bash" => ("Running", "Ran", "Run "),
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
                "bash" => ("Running", "Ran", "command", "commands"),
                "webfetch.query" => ("Web searching", "Web searched", "query", "queries"),
                "webfetch.url" => ("Web viewing", "Web viewed", "page", "pages"),
                _ => unreachable!("only base tools can form a series"),
            };
            let verb = if active { present } else { past };
            let verb = if index == 0 {
                verb.to_owned()
            } else {
                verb.to_lowercase()
            };
            if count == 1 {
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
                        if first.name == "bash" {
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
                        .truncate(),
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
                        if tool.name == "bash" {
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
                        .truncate(),
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

pub(crate) fn render(state: &State, theme: ThemeStyle<'_>, available_width: f32) -> El {
    let has_workspaces = !state.workspaces.is_empty();
    let no_workspaces = state.workspace_list_loaded && !has_workspaces;
    let choices = choices(state);
    let selected = selected(state, &choices);
    let no_provider = state.auth_loaded && !has_configured_provider(state);
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
        .h(31.0)
        .max_w(250.0)
        .px(8.0)
        .center_y()
        .gap(7.0)
        .radius(8.0)
        .continuous_corners()
        .fill(Color::TRANSPARENT)
        .child(if let Some(choice) = selected {
            icon(choice.selection.provider.logo(), 15.0).color(theme.color("#b8b8b8"))
        } else {
            el().size(15.0, 15.0)
        })
        .child(
            text(model_label)
                .min_w(0.0)
                .truncate()
                .font(ui_font(13.0, 450.0))
                .color(theme.color(if selected.is_some() {
                    "#b8b8b8"
                } else {
                    "#888888"
                })),
        )
        .child(
            text(state.new_research_effort.label())
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
    let model_control = if no_provider {
        text("No provider configured. Add one in Settings to start research.")
            .key("new-research-provider-warning")
            .font(ui_font(13.0, 500.0))
            .color(theme.color("#dca15f"))
            .pl(8.0)
    } else {
        model_button
    };
    let composer_input = if has_workspaces {
        text_input(&state.new_research_prompt, ui_font(13.5, 400.0))
            .key("new-research-composer-input")
            .role("textbox")
            .label("Research objective")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .padding(14.0)
            .color(theme.color("#eeeeee"))
            .placeholder(
                if state.active_session.as_ref().is_some_and(|detail| {
                    matches!(
                        detail.graph_id.as_str(),
                        beale_graphs::CLOUDFLARE_ID
                            | beale_graphs::OPENAI_ID
                            | beale_graphs::SEQUENTIAL_ID
                    )
                }) {
                    beale_graphs::Role::from_id(&state.session_agent_role)
                        .map_or("Research with Beale", |role| role.label())
                } else {
                    "Research with Beale"
                },
                theme.color("#888888"),
            )
            .on_edit(|state: &mut State| &mut state.new_research_prompt)
            .on_input_change(|state: &mut State, _| state.session_input_history.reset())
            .on_input_key(composer_history_key)
            .on_submit(|state: &mut State, _| state.new_research_prompt.insert("\n"))
    } else {
        el().key("new-research-composer-input")
            .role("textbox")
            .label("Research objective unavailable until a workspace is added")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .padding(14.0)
            .child(
                text(if state.new_research_prompt.text.is_empty() {
                    "Research with Beale".to_owned()
                } else {
                    state.new_research_prompt.text.clone()
                })
                .font(ui_font(13.5, 400.0))
                .color(theme.color("#707070"))
                .wrap(),
            )
    };
    let composer = col()
        .key("new-research-composer")
        .raise()
        .w_full()
        .h(107.0)
        .min_h(0.0)
        .mt(-8.0)
        .radius(20.0)
        .continuous_corners()
        .fill(theme.color("#191919"))
        .border(1.0, theme.color("#2a2a2a"))
        .child(composer_input)
        .child(
            row()
                .key("new-research-composer-controls")
                .w_full()
                .h(38.0)
                .px(7.0)
                .center_y()
                .child(model_control)
                .child(el().grow(1.0))
                .child({
                    let available = has_workspaces
                        && !state.new_research_prompt.text.trim().is_empty()
                        && !state.session_busy;
                    let can_stop = state.session_busy
                        && state.session_stop_id.is_some()
                        && !state.session_stopping;
                    let mut button = row()
                        .key(if state.session_busy {
                            "new-research-stop"
                        } else {
                            "new-research-send"
                        })
                        .role("button")
                        .label(if state.session_busy {
                            if state.session_stopping {
                                "Stopping session"
                            } else {
                                "Stop session"
                            }
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
                            icon(if state.session_busy { &STOP } else { &ARROW_UP }, 16.0).color(
                                if available || can_stop {
                                    theme.accent_ink()
                                } else {
                                    theme.color("#777777")
                                },
                            ),
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
                }),
        );
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
        .radius(8.0)
        .continuous_corners()
        .fill(if workspace_menu_active {
            theme.color("#303030")
        } else {
            Color::TRANSPARENT
        })
        .child(icon(&FOLDER_OPEN, 15.0).color(theme.color("#9d9d9d")))
        .child(
            text(workspace_name)
                .min_w(0.0)
                .truncate()
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
        .child(workspace_control)
        .child(el().grow(1.0));
    if crate::extensions_view::goal_mode_enabled(state)
        && state.new_research_graph_id != beale_graphs::SEQUENTIAL_ID
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
    if state.new_research_graph_id != beale_graphs::SEQUENTIAL_ID {
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
    let peer_graph = matches!(
        graph_id,
        beale_graphs::CLOUDFLARE_ID | beale_graphs::OPENAI_ID | beale_graphs::SEQUENTIAL_ID
    );
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
                let source = beale_graphs::Role::from_id(&item.from_role)
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
                let mut summaries = message.reasoning_summaries.iter().enumerate().peekable();
                let mut commentary = message.commentary.iter().enumerate().peekable();
                let mut grouped_through = 0;
                for position in 0..=message.tools.len() {
                    while summaries
                        .peek()
                        .is_some_and(|(_, summary)| summary.after_tool_count as usize == position)
                    {
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
                    if expanded
                        && position >= grouped_through
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
                Ok(GoalStatus::Paused) if !crate::extensions_view::goal_mode_enabled(state) => {
                    Some((
                        "Goal Mode is disabled. Enable it in Features to continue.".to_owned(),
                        "#dca15f",
                    ))
                }
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
    if state.active_session.is_none() && !state.session_busy {
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
        normalize_goal_limit, options, selected_model, tool_single_label, work_duration,
    };
    use crate::{AuthStatus, Provider, State};
    use beale_contracts::v1::{CustomProvider, ModelEntry, SessionToolSummary};
    use rune::TextEdit;

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
        assert_eq!(state.composer_menu_section, MenuSection::Provider);

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
        assert_eq!(state.composer_menu_section, MenuSection::Provider);
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
