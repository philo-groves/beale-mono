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
    form_label_font,
    icons::{
        ARROW_UP, BRAIN, CHECK, CHEVRON_DOWN, CHEVRON_RIGHT, CHEVRON_UP, FOLDER_OPEN, REPEAT,
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
    pub(crate) role: beale_graphs::Role,
    pub(crate) models: Vec<ModelSelection>,
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
        .or_else(|| {
            state.lead_provider.and_then(|provider| {
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
        })
        .or_else(|| choices.iter().find(|choice| choice.provider_default))
        .or_else(|| choices.first())
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
                    choices
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
    if choices(state).is_empty() {
        return;
    }
    state.selection_menu = None;
    state.composer_menu_open = true;
    state.composer_menu_section = MenuSection::Model;
    state.composer_menu_cursor = options(state, MenuSection::Model)
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
    let provider_name = selected.map_or("None", |choice| provider_label(state, &choice.selection));
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

fn transcript_markdown(content: String, theme: ThemeStyle<'_>, streaming: bool, muted: bool) -> El {
    let body = ui_font(if muted { 13.0 } else { 14.0 }, 400.0);
    let heading_weight = if muted { 400.0 } else { 600.0 };
    let mut style = MarkdownStyle::neutral(theme.is_dark())
        .text(theme.color(if muted { "#b8b8b8" } else { "#eeeeee" }))
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
    if role == beale_graphs::Role::Main {
        models.truncate(role.max_models());
    }
    models
}

pub(crate) fn graph_concurrency_config(
    state: &State,
) -> Vec<beale_contracts::v1::GraphNodeConcurrency> {
    beale_graphs::SIMPLE_GRAPH
        .nodes
        .iter()
        .filter_map(|role| {
            state.new_research_graph_concurrency[role.index()].map(|max_concurrent| {
                beale_contracts::v1::GraphNodeConcurrency {
                    node_id: role.id().to_owned(),
                    max_concurrent,
                }
            })
        })
        .collect()
}

pub(crate) fn graph_model_config(state: &State) -> Vec<beale_contracts::v1::GraphNodeModel> {
    beale_graphs::SIMPLE_GRAPH
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

fn additional_model_icon_providers(models: &[ModelSelection]) -> Vec<Provider> {
    let mut providers = Vec::new();
    for selection in models.iter().skip(1) {
        if !providers.contains(&selection.provider) {
            providers.push(selection.provider);
            if providers.len() == 2 {
                break;
            }
        }
    }
    providers
}

fn graph_node(state: &State, placement: beale_graphs::NodeLayout, theme: ThemeStyle<'_>) -> El {
    let role = placement.role;
    let models = role_models(state, role);
    let choices = choices(state);
    let model = models.first();
    let name = model.map_or("No model", |selection| {
        choices
            .iter()
            .find(|choice| choice.selection == *selection)
            .map_or(selection.id.as_str(), |choice| choice.name.as_str())
    });
    let extra = models.len().saturating_sub(1);
    let mut model_row = row().w_full().min_w(0.0).center_y().gap(5.0);
    if let Some(selection) = model {
        model_row =
            model_row.child(icon(selection.provider.logo(), 13.0).color(theme.color("#999999")));
    }
    model_row = model_row.child(
        text(name.to_owned())
            .min_w(0.0)
            .grow(1.0)
            .truncate()
            .font(ui_font(12.0, 400.0))
            .color(theme.color("#999999")),
    );
    let mut extra_row = row().w_full().h(16.0).center_y().gap(4.0);
    if extra > 0 {
        extra_row = extra_row.key(format!("new-research-graph-other-models-{}", role.id()));
        for provider in additional_model_icon_providers(&models) {
            extra_row = extra_row.child(icon(provider.logo(), 12.0).color(theme.color("#999999")));
        }
        extra_row = extra_row.child(
            text(format!(
                "+ {extra} other{}",
                if extra == 1 { "" } else { "s" }
            ))
            .min_w(0.0)
            .truncate()
            .font(ui_font(12.0, 400.0))
            .color(theme.color("#999999")),
        );
    }
    let max_concurrent = state.new_research_graph_concurrency[role.index()];
    let mut role_row = row().w_full().h(18.0).center_y().gap(4.0).child(
        text(role.label())
            .grow(1.0)
            .font(ui_font(14.0, 600.0))
            .color(theme.color("#eeeeee")),
    );
    if let Some(max) = max_concurrent {
        role_row = role_row.child(
            text(format!("{max} Max"))
                .font(ui_font(11.0, 400.0))
                .color(theme.color("#999999")),
        );
    }
    col()
        .key(format!("new-research-graph-node-{}", role.id()))
        .role("button")
        .label(format!("Edit {} allowed models", role.label()))
        .absolute(placement.x, placement.y)
        .size(170.0, 76.0)
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
            });
            state.graph_editor_max_concurrent = TextEdit::new(
                state.new_research_graph_concurrency[role.index()]
                    .map_or_else(String::new, |value| value.to_string()),
            );
        })
        .child(role_row)
        .child(model_row)
        .child(extra_row)
}

fn graph_preview(state: &State, theme: ThemeStyle<'_>, available_width: f32) -> El {
    let graph = &beale_graphs::SIMPLE_GRAPH;
    let stage_width = available_width.clamp(180.0, 640.0);
    let (layout, stage_height): (Vec<_>, f32) = if stage_width >= 560.0 {
        (
            graph
                .layout
                .iter()
                .map(|node| beale_graphs::NodeLayout {
                    x: node.x / 470.0 * (stage_width - 170.0),
                    ..*node
                })
                .collect(),
            255.0,
        )
    } else if stage_width >= 380.0 {
        (
            graph
                .nodes
                .iter()
                .enumerate()
                .map(|(index, role)| {
                    let (x, y) = match index {
                        0 => ((stage_width - 170.0) / 2.0, 15.0),
                        1 => (0.0, 140.0),
                        2 => (stage_width - 170.0, 140.0),
                        _ => ((stage_width - 170.0) / 2.0, 265.0),
                    };
                    beale_graphs::NodeLayout { role: *role, x, y }
                })
                .collect(),
            355.0,
        )
    } else {
        (
            graph
                .nodes
                .iter()
                .enumerate()
                .map(|(index, role)| beale_graphs::NodeLayout {
                    role: *role,
                    x: (stage_width - 170.0) / 2.0,
                    y: 15.0 + index as f32 * 105.0,
                })
                .collect(),
            420.0,
        )
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
            let source = [from.x, from.y + 38.0];
            let target = [to.x, to.y + 38.0];
            let rail = (from.x - 9.0).max(0.0);
            Path::new()
                .move_to(source)
                .cubic_to([rail, source[1]], [rail, target[1]], target)
        } else {
            let source = [from.x + 85.0, from.y + 76.0];
            let target = [to.x + 85.0, to.y];
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

fn close_graph_editor(state: &mut State) {
    state.graph_editor = None;
    state.graph_editor_max_concurrent.clear();
}

pub(crate) fn graph_editor_dialog(state: &State, theme: ThemeStyle<'_>) -> [El; 2] {
    let editor = state.graph_editor.as_ref().expect("dialog requires editor");
    let mut list = col()
        .key("graph-editor-model-list")
        .w_full()
        .max_h(250.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    let model_choices = choices(state);
    if model_choices.is_empty() {
        list = list.child(
            text("Connect a provider to choose models for this role.")
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#aaaaaa")),
        );
    }
    for (index, choice) in model_choices.into_iter().enumerate() {
        if index > 0 {
            list = list.child(el().w_full().h(1.0).fill(theme.color("#2c2c2c")));
        }
        let selection = choice.selection.clone();
        let checked = editor.models.contains(&selection);
        let single_model = editor.role == beale_graphs::Role::Main;
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
                .hover(theme.color("#303030"))
                .on_press(move |state: &mut State| {
                    if let Some(editor) = state.graph_editor.as_mut() {
                        if editor.role == beale_graphs::Role::Main {
                            editor.models.clear();
                            editor.models.push(selection.clone());
                        } else {
                            if let Some(index) =
                                editor.models.iter().position(|item| item == &selection)
                            {
                                editor.models.remove(index);
                            } else if editor.models.len() < editor.role.max_models() {
                                editor.models.push(selection.clone());
                            }
                        }
                    }
                })
                .child(icon(choice.selection.provider.logo(), 16.0).color(theme.color("#bbbbbb")))
                .child(
                    text(choice.name)
                        .grow(1.0)
                        .truncate()
                        .font(ui_font(13.0, 400.0))
                        .color(theme.color("#eeeeee")),
                )
                .child(if checked {
                    icon(&CHECK, 15.0).color(theme.color("#eeeeee"))
                } else {
                    el().size(15.0, 15.0)
                }),
        );
    }
    let model_section = col()
        .w_full()
        .py(12.0)
        .gap(7.0)
        .child(
            text(if editor.role == beale_graphs::Role::Main {
                "Model"
            } else {
                "Allowed Models"
            })
            .font(form_label_font())
            .color(theme.color("#e2e2e2")),
        )
        .child(
            text(if editor.role == beale_graphs::Role::Main {
                "Choose one model for the main agent."
            } else {
                "Choose up to 16 models for this role."
            })
            .font(ui_font(14.0, 400.0))
            .color(theme.color("#969696")),
        )
        .child(list);
    let max_concurrent = field(
        &state.graph_editor_max_concurrent,
        "graph-editor-max-concurrent",
        if editor.role == beale_graphs::Role::Main {
            "1"
        } else {
            "Unlimited"
        },
        theme,
        |state| &mut state.graph_editor_max_concurrent,
    )
    .w(110.0)
    .on_input_change(|state: &mut State, _| {
        normalize_goal_limit(
            &mut state.graph_editor_max_concurrent,
            beale_graphs::MAX_CONCURRENT as u64,
        );
    });
    let body = settings_form_card(
        [
            row_with_control(
                "Role",
                "Role in this session graph.",
                text(editor.role.label())
                    .font(ui_font(14.0, 400.0))
                    .color(theme.color("#bdbdbd")),
                theme,
            ),
            model_section,
            row_with_control(
                "Max Concurrent",
                "Maximum simultaneous agents for this role.",
                max_concurrent,
                theme,
            ),
        ],
        theme,
    );
    let mut save = form_button("Save", theme).key("graph-editor-save");
    if !editor.models.is_empty() {
        save = save.on_press(|state: &mut State| {
            if let Some(editor) = state.graph_editor.take() {
                state.new_research_graph_concurrency[editor.role.index()] = state
                    .graph_editor_max_concurrent
                    .text
                    .parse()
                    .ok()
                    .or_else(|| editor.role.default_max_concurrent());
                state.graph_editor_max_concurrent.clear();
                if editor.role == beale_graphs::Role::Main {
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
            title: "Edit Graph Node",
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

fn new_research_setup(state: &State, theme: ThemeStyle<'_>, available_width: f32) -> El {
    let workspace_name = state
        .selected_research_workspace()
        .map(|workspace| workspace.name.as_str());
    let heading = if let Some(name) = workspace_name {
        let underline_width = rune::text::measure(name, &[], ui_font(26.0, 600.0), None)
            .0
            .min(260.0);
        let dot_count = ((underline_width + 2.0) / 4.0).floor() as usize;
        let mut underline = row()
            .key("new-research-heading-workspace-underline")
            .w(underline_width)
            .h(2.0)
            .mt(-1.0)
            .gap(2.0);
        for _ in 0..dot_count {
            underline =
                underline.child(el().size(2.0, 2.0).radius(1.0).fill(theme.color("#777777")));
        }
        let workspace = col()
            .key("new-research-heading-workspace-control")
            .role("button")
            .label(format!("Choose workspace: {name}"))
            .min_w(0.0)
            .max_w(260.0)
            .gap(0.0)
            .hover_group()
            .on_press(|state: &mut State| {
                toggle_selection_menu(state, SelectionMenu::ResearchWorkspaceHeading)
            })
            .child(
                text(name)
                    .truncate()
                    .font(ui_font(26.0, 600.0))
                    .color(theme.color("#eeeeee"))
                    .group_color(theme.color("#aaaaaa")),
            )
            .child(underline);
        row()
            .key("new-research-workspace-heading")
            .w_full()
            .min_w(0.0)
            .center()
            .child(
                text("Conduct security research in ")
                    .font(ui_font(26.0, 600.0))
                    .color(theme.color("#eeeeee")),
            )
            .child(workspace)
            .child(
                text("?")
                    .font(ui_font(26.0, 600.0))
                    .color(theme.color("#eeeeee")),
            )
    } else {
        text("Conduct security research?")
            .key("new-research-workspace-heading")
            .w_full()
            .align(Align::Center)
            .font(ui_font(26.0, 600.0))
            .color(theme.color("#eeeeee"))
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
        let selected = state.new_research_graph_id == beale_graphs::SIMPLE_ID;
        let active = state.selection_menu == Some(SelectionMenu::SessionGraph);
        let description = if selected {
            beale_graphs::SIMPLE_DESCRIPTION.to_owned()
        } else {
            "Research with one agent, without an orchestration graph.".to_owned()
        };
        let label = if selected {
            beale_graphs::SIMPLE_NAME
        } else {
            "Independent"
        };
        let selector = selection_button(
            label,
            active,
            |state: &mut State| toggle_selection_menu(state, SelectionMenu::SessionGraph),
            theme,
        )
        .key("new-research-graph-control")
        .role("button")
        .label(format!("Session graph: {label}"));
        content = content
            .child(row().w_full().py(6.0).center().child(selector))
            .child(
                text(description)
                    .key("new-research-graph-description")
                    .w_full()
                    .align(Align::Center)
                    .wrap()
                    .font(ui_font(13.0, 400.0))
                    .color(theme.color("#aaaaaa")),
            );
        if selected {
            content = content.child(graph_preview(state, theme, available_width));
        }
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

fn transcript_tool(tool: &SessionToolSummary, state: &State, theme: ThemeStyle<'_>) -> El {
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
                    text(tool.summary.clone())
                        .font(ui_font(13.0, 400.0))
                        .color(theme.color(if tool.is_error { "#e07676" } else { "#b4b4b4" }))
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
                    text(format!(
                        "Result · bytes {}–{} of {}",
                        page.offset + 1,
                        page.next_offset,
                        page.total_bytes
                    ))
                    .font(ui_font(12.0, 600.0))
                    .color(theme.color("#969696")),
                )
                .child(
                    text(page.output.clone())
                        .font(ui_font(12.0, 400.0))
                        .color(theme.color("#d0d0d0"))
                        .wrap(),
                );
            if page.next_offset < page.total_bytes && !state.tool_detail_busy {
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

fn transcript_reasoning(
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

fn work_header(
    key: String,
    index: usize,
    duration_ms: u64,
    working: bool,
    expanded: bool,
    theme: ThemeStyle<'_>,
) -> El {
    let label = format!(
        "{} for {}",
        if working { "Working" } else { "Worked" },
        work_duration(duration_ms)
    );
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

fn live_tool(tool: &SessionToolSummary, finished: bool, theme: ThemeStyle<'_>) -> El {
    row()
        .key(format!("session-live-tool-{}", tool.id))
        .w_full()
        .gap(10.0)
        .py(3.0)
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
            text(if finished {
                tool.summary.clone()
            } else {
                format!("{}…", tool.summary)
            })
            .font(ui_font(13.0, 400.0))
            .color(theme.color(if tool.is_error { "#e07676" } else { "#b4b4b4" }))
            .truncate(),
        )
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
        state.new_research_prompt = TextEdit::new(value);
    }
}

fn next_input(state: &mut State) {
    let entries = state
        .active_session
        .as_ref()
        .map(|detail| input_history::session_entries(&detail.messages))
        .unwrap_or_default();
    if let Some(value) = state.session_input_history.next(&entries) {
        state.new_research_prompt = TextEdit::new(value);
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
    let history_entries = state
        .active_session
        .as_ref()
        .map(|detail| input_history::session_entries(&detail.messages))
        .unwrap_or_default();
    let has_workspaces = !state.workspaces.is_empty();
    let no_workspaces = state.workspace_list_loaded && !has_workspaces;
    let choices = choices(state);
    let selected = selected(state, &choices);
    let no_provider = state.auth_loaded && !has_configured_provider(state);
    let model_label = selected.map_or_else(
        || {
            if has_configured_provider(state) {
                "Loading models…".to_owned()
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
    if !choices.is_empty() {
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
            .placeholder("Research with Beale", theme.color("#888888"))
            .on_edit(|state: &mut State| &mut state.new_research_prompt)
            .on_input_change(|state: &mut State, _| state.session_input_history.reset())
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
        .fill(theme.color(if has_workspaces { "#242424" } else { "#202020" }))
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
                .child(
                    row()
                        .gap(2.0)
                        .mr(7.0)
                        .child(input_history::button(
                            "session-history-previous",
                            "Previous sent message",
                            &CHEVRON_UP,
                            state
                                .session_input_history
                                .can_previous(history_entries.len()),
                            ("#bdbdbd", "#686868"),
                            theme,
                            previous_input,
                        ))
                        .child(input_history::button(
                            "session-history-next",
                            "Next sent message",
                            &CHEVRON_DOWN,
                            state.session_input_history.can_next(),
                            ("#bdbdbd", "#686868"),
                            theme,
                            next_input,
                        )),
                )
                .child({
                    let available = has_workspaces
                        && !state.new_research_prompt.text.trim().is_empty()
                        && !state.session_busy;
                    let mut button = row()
                        .key("new-research-send")
                        .role("button")
                        .label(if state.session_busy {
                            "Sending message"
                        } else {
                            "Send message"
                        })
                        .size(28.0, 28.0)
                        .center()
                        .radius(14.0)
                        .fill(theme.color(if available { "#d0d0d0" } else { "#303030" }))
                        .child(icon(&ARROW_UP, 16.0).color(theme.color(if available {
                            "#1b1b1b"
                        } else {
                            "#777777"
                        })));
                    if available {
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
    if crate::extensions_view::goal_mode_enabled(state) {
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
    let mut transcript = col()
        .key("session-transcript")
        .w_full()
        .max_w(750.0)
        .self_center()
        .gap(14.0)
        .px(14.0)
        .pt(22.0)
        .pb(24.0);
    if let Some(detail) = &state.active_session {
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
                if expanded {
                    if message.reasoning_summaries.is_empty() {
                        transcript = transcript.child(transcript_reasoning(
                            "No reasoning summary was received for this turn.".to_owned(),
                            format!("session-reasoning-unavailable-{index}"),
                            false,
                            theme,
                        ));
                    }
                    let mut summaries = message.reasoning_summaries.iter().enumerate().peekable();
                    let mut commentary = message.commentary.iter().enumerate().peekable();
                    for position in 0..=message.tools.len() {
                        while summaries.peek().is_some_and(|(_, summary)| {
                            summary.after_tool_count as usize == position
                        }) {
                            if let Some((summary_index, summary)) = summaries.next() {
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
                                transcript = transcript.child(transcript_message(
                                    format!("session-commentary-{index}-{commentary_index}"),
                                    item.text.clone(),
                                    false,
                                    false,
                                    theme,
                                ));
                            }
                        }
                        if let Some(tool) = message.tools.get(position) {
                            transcript = transcript.child(transcript_tool(tool, state, theme));
                        }
                    }
                }
            }
            transcript = transcript.child(transcript_message(
                format!("session-message-{index}"),
                message.text.clone(),
                user,
                false,
                theme,
            ));
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
    if let Some(prompt) = &state.session_pending_prompt {
        transcript = transcript.child(transcript_message(
            "session-pending-message".to_owned(),
            prompt.clone(),
            true,
            false,
            theme,
        ));
    }
    if state.session_busy || !state.session_live_events.is_empty() {
        transcript = transcript.child(work_header(
            "session-current-work-header".to_owned(),
            state
                .active_session
                .as_ref()
                .map_or(0, |detail| detail.messages.len()),
            state.session_elapsed_ms,
            state.session_busy,
            !state.session_busy
                && state.session_work_expanded.contains(
                    &state
                        .active_session
                        .as_ref()
                        .map_or(0, |detail| detail.messages.len()),
                ),
            theme,
        ));
        if state.session_busy
            || state.session_work_expanded.contains(
                &state
                    .active_session
                    .as_ref()
                    .map_or(0, |detail| detail.messages.len()),
            )
        {
            for (index, event) in state.session_live_events.iter().enumerate() {
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
                            transcript = transcript.child(live_tool(
                                tool,
                                event.kind == SessionStreamKind::ToolFinished as i32,
                                theme,
                            ));
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
        MenuSection, ModelSelection, additional_model_icon_providers, choices,
        has_configured_provider, is_web_link, normalize_goal_limit, options, selected_model,
        work_duration,
    };
    use crate::{Provider, State};
    use beale_contracts::v1::{AuthMethod, CustomProvider, ModelEntry};
    use rune::TextEdit;

    #[test]
    fn additional_model_icons_show_distinct_providers() {
        let models = [
            Provider::Xai,
            Provider::Xai,
            Provider::Xai,
            Provider::OpenAi,
            Provider::OpenAi,
        ]
        .into_iter()
        .enumerate()
        .map(|(index, provider)| ModelSelection {
            provider,
            method: AuthMethod::ApiKey,
            id: format!("example-model-{index}"),
        })
        .collect::<Vec<_>>();

        assert_eq!(
            additional_model_icon_providers(&models),
            [Provider::Xai, Provider::OpenAi]
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
