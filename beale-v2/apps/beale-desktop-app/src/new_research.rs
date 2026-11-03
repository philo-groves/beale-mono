use beale_contracts::v1::{AuthMethod, SessionRole, SessionToolSummary};
use rune::{
    Align, Color, El, Key, KeyEvent, ScrollSpec, Side, Tx, col, el, icon, row, text, text_input,
};

use crate::{
    Provider, SelectionMenu, State, ThemeStyle, catalog_index, effective_model_method,
    icons::{ARROW_UP, CHECK, CHEVRON_DOWN, CHEVRON_RIGHT, CHEVRON_UP, FOLDER_OPEN, REPEAT},
    in_place_menu, load_tool_detail_page, start_session_turn, toggle_selection_menu,
    toggle_tool_detail, ui_font,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModelSelection {
    pub(crate) provider: Provider,
    pub(crate) method: AuthMethod,
    pub(crate) id: String,
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
    [Provider::OpenAi, Provider::Xai]
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
        .collect()
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
                    .find(|choice| choice.selection.provider == provider && choice.provider_default)
                    .or_else(|| {
                        choices
                            .iter()
                            .find(|choice| choice.selection.provider == provider)
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
        MenuSection::Provider => [Provider::OpenAi, Provider::Xai]
            .into_iter()
            .filter_map(|provider| {
                choices
                    .iter()
                    .find(|choice| choice.selection.provider == provider && choice.provider_default)
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
            .collect(),
        MenuSection::Model => choices
            .into_iter()
            .filter(|choice| {
                selected.as_ref().is_some_and(|selection| {
                    choice.selection.provider == selection.provider
                        && choice.selection.method == selection.method
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
            MenuChoice::Model(selection) => state.new_research_model = Some(selection.clone()),
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
    let provider_name = selected.map_or("None", |choice| choice.selection.provider.name());
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

fn transcript_message(key: String, content: String, user: bool, theme: ThemeStyle<'_>) -> El {
    let message = text(content)
        .font(ui_font(14.0, 400.0))
        .color(theme.color("#eeeeee"))
        .wrap();
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
                .gap(8.0)
                .px(10.0)
                .py(7.0)
                .radius(8.0)
                .continuous_corners()
                .fill(theme.color("#202020"))
                .hover(theme.color("#303030"))
                .on_press(move |state: &mut State| toggle_tool_detail(state, id.clone()))
                .child(
                    icon(
                        if expanded {
                            &CHEVRON_DOWN
                        } else {
                            &CHEVRON_RIGHT
                        },
                        13.0,
                    )
                    .color(theme.color("#969696")),
                )
                .child(
                    text(tool.summary.clone())
                        .font(ui_font(13.0, 400.0))
                        .color(theme.color(if tool.is_error { "#e07676" } else { "#b4b4b4" }))
                        .wrap(),
                ),
        );
    if expanded {
        let mut detail = col()
            .key(format!("session-tool-detail-{}", tool.id))
            .w_full()
            .gap(6.0)
            .px(12.0)
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

pub(crate) fn render(state: &State, theme: ThemeStyle<'_>) -> El {
    let choices = choices(state);
    let selected = selected(state, &choices);
    let model_label = selected.map_or_else(
        || {
            if state
                .auth_status
                .iter()
                .any(|status| status.subscription || status.api_key)
            {
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
                choice.selection.provider.name(),
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
    let composer = col()
        .key("new-research-composer")
        .w_full()
        .h(107.0)
        .min_h(0.0)
        .mt(-8.0)
        .radius(20.0)
        .continuous_corners()
        .fill(theme.color("#242424"))
        .child(
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
                .on_submit(|state: &mut State, _| state.new_research_prompt.insert("\n")),
        )
        .child(
            row()
                .key("new-research-composer-controls")
                .w_full()
                .h(38.0)
                .px(7.0)
                .center_y()
                .child(model_button)
                .child(el().grow(1.0))
                .child({
                    let available =
                        !state.new_research_prompt.text.trim().is_empty() && !state.session_busy;
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
                        .radius(8.0)
                        .continuous_corners()
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
    let ribbon = row()
        .key("new-research-ribbon")
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
        .child(workspace_button)
        .child(el().grow(1.0))
        .child(
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
                ))),
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
        for (index, message) in detail.messages.iter().enumerate() {
            let user = message.role == SessionRole::User as i32;
            for tool in &message.tools {
                transcript = transcript.child(transcript_tool(tool, state, theme));
            }
            transcript = transcript.child(transcript_message(
                format!("session-message-{index}"),
                message.text.clone(),
                user,
                theme,
            ));
        }
    }
    if let Some(prompt) = &state.session_pending_prompt {
        transcript = transcript.child(transcript_message(
            "session-pending-message".to_owned(),
            prompt.clone(),
            true,
            theme,
        ));
    }
    if state.session_busy {
        transcript = transcript.child(
            text("Beale is responding…")
                .key("session-progress")
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#969696")),
        );
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
            el().grow(1.0)
        })
        .child(
            col()
                .key("new-research-composer-stack")
                .w_full()
                .max_w(750.0)
                .self_center()
                .child(col().w_full().px(20.0).child(ribbon))
                .child(composer),
        )
}
