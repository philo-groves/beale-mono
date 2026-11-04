use beale_contracts::v1::{
    AuthMethod, ReasoningEffort, SessionRole, SessionStreamKind, SessionToolSummary,
};
use rune::{
    Align, Color, El, Key, KeyEvent, ScrollSpec, Side, TextEdit, Transition, Tx, col, el, icon,
    row, text, text_input,
};
use rune_kit::{Markdown, MarkdownStyle};

use crate::{
    Provider, SelectionMenu, State, ThemeStyle, catalog_index, effective_model_method,
    icons::{
        ARROW_UP, BRAIN, CHECK, CHEVRON_DOWN, CHEVRON_RIGHT, CHEVRON_UP, FOLDER_OPEN, REPEAT,
        TERMINAL, WRENCH,
    },
    in_place_menu, input_history, load_tool_detail_page, start_session_turn, toggle_selection_menu,
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

pub(crate) fn render(state: &State, theme: ThemeStyle<'_>) -> El {
    let history_entries = state
        .active_session
        .as_ref()
        .map(|detail| input_history::session_entries(&detail.messages))
        .unwrap_or_default();
    let has_workspaces = !state.workspaces.is_empty();
    let no_workspaces = state.workspace_list_loaded && !has_workspaces;
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
                .child(model_button)
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
    let ribbon = row()
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
            el().grow(1.0)
        })
        .child(composer_stack)
}

#[cfg(test)]
mod tests {
    use super::{is_web_link, work_duration};

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
