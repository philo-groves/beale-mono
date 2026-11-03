use rune::{El, Icon, col, el, icon, row, text};

use crate::icons::{FOLDER_OPEN, NEW_RESEARCH, PLUS, QUICK_CHAT, SEARCH};
use crate::{
    SelectionMenu, State, ThemeStyle, selection_button, sidenav_heading, start_session_load,
    toggle_selection_menu, ui_font,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum HomeCollection {
    #[default]
    Workspaces,
    Tasks,
}

impl HomeCollection {
    pub(crate) const ALL: [Self; 2] = [Self::Workspaces, Self::Tasks];

    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Workspaces => "workspaces",
            Self::Tasks => "tasks",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Workspaces => "Workspaces",
            Self::Tasks => "Tasks",
        }
    }
}

struct SidebarAction {
    key: &'static str,
    label: &'static str,
    icon: &'static Icon,
    action: Option<fn(&mut State)>,
}

const PRIMARY_ACTIONS: [SidebarAction; 2] = [
    SidebarAction {
        key: "home-new-research",
        label: "New Research",
        icon: &NEW_RESEARCH,
        action: Some(State::start_new_research),
    },
    SidebarAction {
        key: "home-quick-chat",
        label: "Quick Chat",
        icon: &QUICK_CHAT,
        action: None,
    },
];

const SEARCH_WORKSPACES: SidebarAction = SidebarAction {
    key: "home-search-workspaces",
    label: "Search sessions",
    icon: &SEARCH,
    action: None,
};

const NEW_WORKSPACE: SidebarAction = SidebarAction {
    key: "home-add-workspace",
    label: "New Workspace",
    icon: &PLUS,
    action: Some(State::start_workspace_creation),
};

struct HomeSidebarSpec {
    title: &'static str,
    empty_workspaces: &'static str,
}

const HOME_SIDEBAR: HomeSidebarSpec = HomeSidebarSpec {
    title: "Beale",
    empty_workspaces: "No Workspaces Yet...",
};

fn primary_action(spec: &SidebarAction, state: &State, theme: ThemeStyle<'_>) -> El {
    let color = |value| theme.color(value);
    let available = spec.action.is_some();
    let selected = spec.key == "home-new-research"
        && state.primary_view == crate::PrimaryView::NewResearch
        && (state.home_collection != HomeCollection::Workspaces
            || state.selected_research_workspace().is_none());
    let mut button = row()
        .key(spec.key)
        .role("button")
        .label(if available {
            spec.label.to_owned()
        } else {
            format!("{} unavailable", spec.label)
        })
        .w_full()
        .h(30.0)
        .px(4.0)
        .center_y()
        .gap(12.0)
        .radius(8.0)
        .fill(if selected {
            color("#303030")
        } else {
            rune::Color::TRANSPARENT
        })
        .child(icon(spec.icon, 16.0).color(color(if available { "#bdbdbd" } else { "#777777" })))
        .child(
            text(spec.label)
                .font(ui_font(14.0, if selected { 550.0 } else { 400.0 }))
                .color(color(if available { "#dddddd" } else { "#888888" })),
        );
    if let Some(action) = spec.action {
        button = button.hover(color("#282828")).on_press(action);
    }
    button
}

fn workspace_action(spec: &SidebarAction, theme: ThemeStyle<'_>) -> El {
    let mut button = row()
        .key(spec.key)
        .role("button")
        .label(if spec.action.is_some() {
            spec.label.to_owned()
        } else {
            format!("{} unavailable", spec.label)
        })
        .size(26.0, 26.0)
        .center()
        .radius(8.0)
        .continuous_corners()
        .border(1.0, theme.color("#2c2c2c"))
        .child(
            icon(spec.icon, 15.0).color(theme.color(if spec.action.is_some() {
                "#bdbdbd"
            } else {
                "#777777"
            })),
        );
    if let Some(action) = spec.action {
        button = button.hover(theme.color("#414141")).on_press(action);
    }
    button
}

fn session_row(key: String, label: &str, italic: bool, active: bool, theme: ThemeStyle<'_>) -> El {
    let color = |value| theme.color(value);
    let font = ui_font(14.0, if active { 550.0 } else { 400.0 });
    row()
        .key(key)
        .role("button")
        .label(label.to_owned())
        .w_full()
        .min_h(30.0)
        .px(4.0)
        .center_y()
        .gap(10.0)
        .radius(8.0)
        .continuous_corners()
        .fill(if active {
            color("#303030")
        } else {
            rune::Color::TRANSPARENT
        })
        .hover(color("#282828"))
        .child(el().size(15.0, 15.0))
        .child(
            text(label.to_owned())
                .font(if italic { font.italic() } else { font })
                .color(color(if active { "#f2f2f2" } else { "#b8b8b8" }))
                .min_w(0.0)
                .truncate(),
        )
}

pub(crate) fn render(state: &State, width: f32, theme: ThemeStyle<'_>) -> El {
    let color = |value| theme.color(value);
    let mut primary = col().w_full().gap(2.0);
    for spec in &PRIMARY_ACTIONS {
        primary = primary.child(primary_action(spec, state, theme));
    }
    let collection_open = state.selection_menu == Some(SelectionMenu::HomeCollection);
    let collection_control = selection_button(
        state.home_collection.label(),
        collection_open,
        |state| toggle_selection_menu(state, SelectionMenu::HomeCollection),
        theme,
    )
    .key("home-collection-control")
    .role("button")
    .label(format!(
        "Choose sidebar collection: {}",
        state.home_collection.label()
    ));
    let mut selector_tools = row().center_y().gap(4.0).child(collection_control);
    if state.home_collection == HomeCollection::Workspaces {
        selector_tools = selector_tools.child(workspace_action(&NEW_WORKSPACE, theme));
    }
    let mut header = row()
        .key("home-collection-header")
        .w_full()
        .h(28.0)
        .mb(12.0)
        .center_y()
        .child(selector_tools)
        .child(el().grow(1.0));
    if state.home_collection == HomeCollection::Workspaces {
        header = header.child(workspace_action(&SEARCH_WORKSPACES, theme));
    }
    let mut collection = col()
        .key("home-collection")
        .w_full()
        .grow(1.0)
        .mt(18.0)
        .gap(6.0)
        .child(header);
    if state.home_collection == HomeCollection::Workspaces && state.workspaces.is_empty() {
        let label = if !state.workspace_list_error.is_empty() {
            "Workspaces unavailable"
        } else if state.workspace_list_loaded {
            HOME_SIDEBAR.empty_workspaces
        } else if matches!(state.connection, crate::ConnectionStatus::Connected { .. }) {
            "Loading workspaces…"
        } else {
            HOME_SIDEBAR.empty_workspaces
        };
        collection = collection.child(
            text(label)
                .key("home-workspaces-empty")
                .font(ui_font(13.0, 400.0))
                .color(color("#969696"))
                .ml(8.0),
        );
    } else if state.home_collection == HomeCollection::Workspaces {
        let new_session_workspace = (state.primary_view == crate::PrimaryView::NewResearch
            && state.active_session.is_none())
        .then(|| {
            state
                .selected_research_workspace()
                .map(|item| item.id.as_str())
        })
        .flatten();
        for workspace in &state.workspaces {
            let id = uuid::Uuid::parse_str(&workspace.id).ok();
            let selected =
                state.primary_view == crate::PrimaryView::Workspace && state.workspace_id == id;
            let mut item = row()
                .key(format!("home-workspace-{}", workspace.id))
                .role("button")
                .label(format!("Open {}", workspace.name))
                .w_full()
                .min_h(30.0)
                .px(4.0)
                .center_y()
                .gap(10.0)
                .radius(8.0)
                .continuous_corners()
                .fill(if selected {
                    color("#303030")
                } else {
                    rune::Color::TRANSPARENT
                })
                .child(icon(&FOLDER_OPEN, 15.0).color(color("#9d9d9d")))
                .child(
                    text(workspace.name.clone())
                        .font(ui_font(14.0, if selected { 550.0 } else { 400.0 }))
                        .color(color("#dddddd"))
                        .wrap(),
                );
            if let Some(id) = id {
                item = item
                    .hover(color("#282828"))
                    .on_press(move |state: &mut State| state.open_workspace(id));
            }
            let mut group = col().w_full().gap(1.0).child(item);
            if new_session_workspace == Some(workspace.id.as_str()) {
                group = group.child(
                    session_row(
                        format!("home-session-{}", workspace.id),
                        "New Session",
                        true,
                        true,
                        theme,
                    )
                    .label(format!("New Session in {}", workspace.name))
                    .on_press(State::start_new_research),
                );
            }
            for session in state
                .session_summaries
                .iter()
                .filter(|session| session.workspace_id == workspace.id)
            {
                let session_id = session.id.clone();
                let active = state.primary_view == crate::PrimaryView::NewResearch
                    && state
                        .active_session
                        .as_ref()
                        .and_then(|detail| detail.summary.as_ref())
                        .is_some_and(|summary| summary.id == session.id);
                group = group.child(
                    session_row(
                        format!("home-session-{}", session.id),
                        &session.title,
                        false,
                        active,
                        theme,
                    )
                    .on_press(move |state: &mut State| {
                        if state
                            .active_session
                            .as_ref()
                            .and_then(|detail| detail.summary.as_ref())
                            .is_some_and(|summary| summary.id == session_id)
                        {
                            state.navigate_to(crate::ViewLocation::NewResearch);
                        } else {
                            start_session_load(state, session_id.clone());
                        }
                    }),
                );
            }
            collection = collection.child(group);
        }
    }
    if state.home_collection == HomeCollection::Workspaces && !state.workspace_list_error.is_empty()
    {
        collection = collection.child(
            text(state.workspace_list_error.clone())
                .key("home-workspaces-error")
                .font(ui_font(13.0, 400.0))
                .color(color("#e07676"))
                .ml(8.0)
                .wrap(),
        );
    }
    col()
        .key("home-sidenav")
        .role("navigation")
        .label("Home sidebar")
        .w(width)
        .h_full()
        .px(12.0)
        .pt(12.0)
        .fill(color("#111111"))
        .child(
            sidenav_heading(HOME_SIDEBAR.title, theme)
                .key("home-wordmark")
                .mb(12.0),
        )
        .child(primary)
        .child(collection)
}
