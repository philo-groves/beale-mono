use rune::{El, Icon, col, icon, row, text};

use crate::icons::{FOLDER_PLUS, NEW_RESEARCH, QUICK_CHAT, SEARCH};
use crate::{State, ThemeStyle, sidenav_heading, ui_font};

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
        action: None,
    },
    SidebarAction {
        key: "home-quick-chat",
        label: "Quick Chat",
        icon: &QUICK_CHAT,
        action: None,
    },
];

const WORKSPACE_ACTIONS: [SidebarAction; 2] = [
    SidebarAction {
        key: "home-search-workspaces",
        label: "Search sessions",
        icon: &SEARCH,
        action: None,
    },
    SidebarAction {
        key: "home-add-workspace",
        label: "Add workspace",
        icon: &FOLDER_PLUS,
        action: Some(State::start_workspace_creation),
    },
];

struct HomeSidebarSpec {
    title: &'static str,
    workspace_heading: &'static str,
    empty_workspaces: &'static str,
}

const HOME_SIDEBAR: HomeSidebarSpec = HomeSidebarSpec {
    title: "Beale",
    workspace_heading: "Workspaces",
    empty_workspaces: "No Workspaces Yet...",
};

fn primary_action(spec: &SidebarAction, theme: ThemeStyle<'_>) -> El {
    let color = |value| theme.color(value);
    let available = spec.action.is_some();
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
        .child(icon(spec.icon, 16.0).color(color(if available { "#bdbdbd" } else { "#777777" })))
        .child(
            text(spec.label)
                .font(ui_font(13.0, 400.0))
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
        .size(28.0, 28.0)
        .center()
        .radius(6.0)
        .child(
            icon(spec.icon, 15.0).color(theme.color(if spec.action.is_some() {
                "#bdbdbd"
            } else {
                "#777777"
            })),
        );
    if let Some(action) = spec.action {
        button = button.hover(theme.color("#282828")).on_press(action);
    }
    button
}

pub(crate) fn render(width: f32, theme: ThemeStyle<'_>) -> El {
    let color = |value| theme.color(value);
    let mut primary = col().w_full().gap(2.0);
    for spec in &PRIMARY_ACTIONS {
        primary = primary.child(primary_action(spec, theme));
    }
    let mut tools = row().center_y();
    for spec in &WORKSPACE_ACTIONS {
        tools = tools.child(workspace_action(spec, theme));
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
        .child(
            col()
                .key("home-workspaces")
                .w_full()
                .grow(1.0)
                .mt(18.0)
                .gap(6.0)
                .child(
                    row()
                        .key("home-workspaces-header")
                        .w_full()
                        .h(28.0)
                        .center_y()
                        .child(
                            text(HOME_SIDEBAR.workspace_heading)
                                .font(ui_font(15.0, 400.0))
                                .color(color("#969696")),
                        )
                        .child(row().grow(1.0))
                        .child(tools),
                )
                .child(
                    text(HOME_SIDEBAR.empty_workspaces)
                        .key("home-workspaces-empty")
                        .font(ui_font(13.0, 400.0))
                        .color(color("#969696"))
                        .ml(8.0),
                ),
        )
}
