use rune::tok::TextStyle;
use rune::{
    AppSpec, Color, Cx, El, Icon, ScrollSpec, TextEdit, Tx, col, el, icon, row, text, text_input,
};
use std::time::Duration;

mod connection;
mod provider_auth;
use beale_contracts::v1::ProviderId;
use connection::ConnectionStatus;
use provider_auth::{Action, AuthStatus, SignInPrompt};

#[cfg(target_os = "windows")]
mod windows_frame;

static HOME: Icon = Icon {
    name: "beale-home",
    view: 24.0,
    body: "<path d=\"m3 10 9-7 9 7v10H3z\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\" stroke-linejoin=\"round\"/><path d=\"M9 20v-7h6v7\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\" stroke-linejoin=\"round\"/>",
};
static AUTOMATIONS: Icon = Icon {
    name: "beale-automations",
    view: 24.0,
    body: "<rect x=\"3\" y=\"5\" width=\"18\" height=\"16\" rx=\"2\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\"/><path d=\"M7 3v4M17 3v4M3 10h18M12 13v4l2 1\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\" stroke-linecap=\"round\"/>",
};
static FLEET: Icon = Icon {
    name: "beale-fleet",
    view: 24.0,
    body: "<rect x=\"3\" y=\"3\" width=\"18\" height=\"7\" rx=\"2\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\"/><rect x=\"3\" y=\"14\" width=\"18\" height=\"7\" rx=\"2\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\"/><circle cx=\"7\" cy=\"6.5\" r=\"1\"/><circle cx=\"7\" cy=\"17.5\" r=\"1\"/>",
};
static PLUGINS: Icon = Icon {
    name: "beale-plugins",
    view: 24.0,
    body: "<path d=\"M6 8h12v5a4 4 0 0 1-4 4h-4a4 4 0 0 1-4-4zM9 2v6m6-6v6m-3 9v5\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>",
};
static SETTINGS: Icon = Icon {
    name: "beale-settings",
    view: 24.0,
    body: "<path d=\"M12 3.5 13.5 5l2-.3.8 1.9 2 .7-.1 2 1.4 1.5-1.4 1.5.1 2-2 .7-.8 1.9-2-.3-1.5 1.5-1.5-1.5-2 .3-.8-1.9-2-.7.1-2L4.4 11l1.4-1.5-.1-2 2-.7.8-1.9 2 .3z\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\" stroke-linejoin=\"round\"/><circle cx=\"12\" cy=\"11\" r=\"2.5\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\"/>",
};
static SIDEBAR: Icon = Icon {
    name: "beale-sidebar",
    view: 24.0,
    body: "<rect x=\"3\" y=\"4\" width=\"18\" height=\"16\" rx=\"2\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\"/><path d=\"M9 4v16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\"/>",
};
static MINIMIZE: Icon = Icon {
    name: "beale-minimize",
    view: 24.0,
    body: "<path d=\"M5 12h14\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\" stroke-linecap=\"round\"/>",
};
static MAXIMIZE: Icon = Icon {
    name: "beale-maximize",
    view: 24.0,
    body: "<rect x=\"5\" y=\"5\" width=\"14\" height=\"14\" rx=\"1\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\"/>",
};
static CLOSE: Icon = Icon {
    name: "beale-close",
    view: 24.0,
    body: "<path d=\"M5 5l14 14M19 5 5 19\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\" stroke-linecap=\"round\"/>",
};
static APPEARANCE: Icon = Icon {
    name: "beale-appearance",
    view: 24.0,
    body: "<path d=\"M12 3a9 9 0 1 0 0 18h1.5a2 2 0 0 0 1.5-3.3 1.7 1.7 0 0 1 1.3-2.7H18a3 3 0 0 0 3-3 9 9 0 0 0-9-9Z\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\"/><circle cx=\"7.5\" cy=\"11\" r=\"1\"/><circle cx=\"10\" cy=\"7\" r=\"1\"/><circle cx=\"15\" cy=\"7.5\" r=\"1\"/>",
};
static PROVIDERS: Icon = Icon {
    name: "beale-providers",
    view: 24.0,
    body: "<rect x=\"3\" y=\"4\" width=\"18\" height=\"7\" rx=\"2\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\"/><path d=\"M7 7.5h.01M3 17h8m6-3 1 1.3 1.6.3-.3 1.6 1 1.2-1 1.2.3 1.6-1.6.3-1 1.3-1-1.3-1.6-.3.3-1.6-1-1.2 1-1.2-.3-1.6 1.6-.3Z\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.6\" stroke-linecap=\"round\"/><circle cx=\"17\" cy=\"17.5\" r=\"1\" fill=\"none\" stroke=\"currentColor\"/>",
};
static PROFILE: Icon = Icon {
    name: "beale-profile",
    view: 24.0,
    body: "<circle cx=\"10\" cy=\"8\" r=\"3\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\"/><path d=\"M3 20v-2a6 6 0 0 1 10-4.5M17 15l.6 1 1.2.2-.2 1.2.8.8-.8.8.2 1.2-1.2.2-.6 1-.6-1-1.2-.2.2-1.2-.8-.8.8-.8-.2-1.2 1.2-.2Z\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\" stroke-linejoin=\"round\"/>",
};
static PROMPT: Icon = Icon {
    name: "beale-prompt",
    view: 24.0,
    body: "<path d=\"M5 3h10l4 4v14H5zM15 3v5h4M8 12h8M8 16h8\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\" stroke-linejoin=\"round\"/>",
};
static FEATURES: Icon = Icon {
    name: "beale-features",
    view: 24.0,
    body: "<path d=\"M4 6h7m4 0h5M4 12h3m4 0h9M4 18h10m4 0h2\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\" stroke-linecap=\"round\"/><circle cx=\"13\" cy=\"6\" r=\"2\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\"/><circle cx=\"9\" cy=\"12\" r=\"2\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\"/><circle cx=\"16\" cy=\"18\" r=\"2\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\"/>",
};
static ARCHIVE: Icon = Icon {
    name: "beale-archive",
    view: 24.0,
    body: "<rect x=\"3\" y=\"4\" width=\"18\" height=\"5\" rx=\"1\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\"/><path d=\"M5 9v11h14V9M10 13h4\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\" stroke-linecap=\"round\"/>",
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Provider {
    #[default]
    OpenAi,
    Xai,
}

impl Provider {
    fn id(self) -> ProviderId {
        match self {
            Self::OpenAi => ProviderId::Openai,
            Self::Xai => ProviderId::Xai,
        }
    }

    fn index(self) -> usize {
        match self {
            Self::OpenAi => 0,
            Self::Xai => 1,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::OpenAi => "OpenAI",
            Self::Xai => "xAI",
        }
    }

    fn initial(self) -> &'static str {
        match self {
            Self::OpenAi => "O",
            Self::Xai => "x",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::OpenAi => "Connect a ChatGPT subscription or use an OpenAI API key.",
            Self::Xai => "Connect a Grok subscription or use an xAI API key.",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HeaderMenu {
    File,
    View,
    Window,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum SettingsSection {
    General,
    Appearance,
    #[default]
    Providers,
    Profile,
    Prompt,
    Features,
    Archive,
}

impl SettingsSection {
    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Appearance => "Appearance",
            Self::Providers => "Providers",
            Self::Profile => "Profile",
            Self::Prompt => "Prompt",
            Self::Features => "Features",
            Self::Archive => "Archive",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::General => "settings-general",
            Self::Appearance => "settings-appearance",
            Self::Providers => "settings-providers",
            Self::Profile => "settings-profile",
            Self::Prompt => "settings-prompt",
            Self::Features => "settings-features",
            Self::Archive => "settings-archive",
        }
    }

    fn icon(self) -> &'static Icon {
        match self {
            Self::General => &SETTINGS,
            Self::Appearance => &APPEARANCE,
            Self::Providers => &PROVIDERS,
            Self::Profile => &PROFILE,
            Self::Prompt => &PROMPT,
            Self::Features => &FEATURES,
            Self::Archive => &ARCHIVE,
        }
    }
}

const SETTINGS_SECTIONS: [SettingsSection; 7] = [
    SettingsSection::General,
    SettingsSection::Appearance,
    SettingsSection::Providers,
    SettingsSection::Profile,
    SettingsSection::Prompt,
    SettingsSection::Features,
    SettingsSection::Archive,
];

struct State {
    selected: Provider,
    settings_section: SettingsSection,
    connection: ConnectionStatus,
    auth_status: [AuthStatus; 2],
    auth_loaded: bool,
    auth_message: String,
    sign_in_prompt: Option<SignInPrompt>,
    api_key_edit: TextEdit,
    api_key_dialog: bool,
    auth_busy: bool,
    notice: Option<&'static str>,
    settings_nav_visible: bool,
    open_menu: Option<HeaderMenu>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            selected: Provider::OpenAi,
            settings_section: SettingsSection::Providers,
            connection: ConnectionStatus::Connecting,
            auth_status: Default::default(),
            auth_loaded: false,
            auth_message: String::new(),
            sign_in_prompt: None,
            api_key_edit: TextEdit::new("").secure(),
            api_key_dialog: false,
            auth_busy: false,
            notice: None,
            settings_nav_visible: true,
            open_menu: None,
        }
    }
}

fn color(value: &str) -> Color {
    rune::color::hex(value).unwrap_or(Color::BLACK)
}

static AUTH_HANDLE: std::sync::OnceLock<rune::AppHandle<State>> = std::sync::OnceLock::new();

fn start_auth_action(state: &mut State, action: Action) {
    if state.auth_busy || !matches!(state.connection, ConnectionStatus::Connected { .. }) {
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        return;
    };
    state.auth_busy = true;
    state.auth_message.clear();
    state.sign_in_prompt = None;
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid local app-server address".to_owned())?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize app-server connection".to_owned())?;
            runtime.block_on(provider_auth::perform(addr, action))
        })();
        handle.update(move |state, tx| {
            state.auth_busy = false;
            match result {
                Ok(Some(prompt)) => {
                    if let Err(error) = tx.open_url(&prompt.url) {
                        state.auth_message = format!("Could not open browser: {error}");
                    } else {
                        state.auth_message = "Finish sign-in in your browser.".to_owned();
                    }
                    state.sign_in_prompt = Some(prompt);
                }
                Ok(None) => {
                    state.auth_message = "Credential setting saved on the app-server.".to_owned()
                }
                Err(error) => state.auth_message = error,
            }
        });
    });
}

#[cfg(target_os = "windows")]
static MAIN_WINDOW: std::sync::OnceLock<std::sync::Mutex<Option<rune::WindowHandle>>> =
    std::sync::OnceLock::new();

#[cfg(target_os = "windows")]
fn with_windows_window(action: impl FnOnce(windows_sys::Win32::Foundation::HWND)) {
    use raw_window_handle::RawWindowHandle;
    let Some(window) = MAIN_WINDOW.get().and_then(|slot| slot.lock().ok()?.clone()) else {
        return;
    };
    let _ = window.with_raw_handle(|handle| {
        if let RawWindowHandle::Win32(win32) = handle.as_raw() {
            action(win32.hwnd.get() as _);
        }
    });
}

fn minimize_window(tx: &mut Tx) {
    #[cfg(target_os = "windows")]
    {
        let _ = tx;
        with_windows_window(|hwnd| {
            // SAFETY: Rune invokes the button handler on the window's UI thread, and
            // with_raw_handle keeps the native window alive for this callback.
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(
                    hwnd,
                    windows_sys::Win32::UI::WindowsAndMessaging::SW_MINIMIZE,
                );
            }
        });
    }
    #[cfg(not(target_os = "windows"))]
    tx.minimize();
}

#[cfg(target_os = "windows")]
fn toggle_maximize_window() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IsZoomed, SW_MAXIMIZE, SW_RESTORE, ShowWindow,
    };
    with_windows_window(|hwnd| {
        // SAFETY: Rune invokes the button handler on the window's UI thread, and
        // with_raw_handle keeps the native window alive for this callback.
        unsafe {
            ShowWindow(
                hwnd,
                if IsZoomed(hwnd) != 0 {
                    SW_RESTORE
                } else {
                    SW_MAXIMIZE
                },
            );
        }
    });
}

fn header_icon_button(key: &'static str, label: &'static str, glyph: &'static Icon) -> El {
    el().key(key)
        .role("button")
        .label(label)
        .size(34.0, 34.0)
        .center()
        .radius(6.0)
        .hover(color("#303030"))
        .child(icon(glyph, 15.0).color(color("#c8c8c8")))
}

fn menu_button(label: &'static str, menu: HeaderMenu, open: bool) -> El {
    el().key(match menu {
        HeaderMenu::File => "menu-file",
        HeaderMenu::View => "menu-view",
        HeaderMenu::Window => "menu-window",
    })
    .role("button")
    .label(format!("{label} menu"))
    .h(27.0)
    .px(9.0)
    .center()
    .radius(5.0)
    .fill(if open {
        color("#242424")
    } else {
        Color::TRANSPARENT
    })
    .hover(color("#242424"))
    .on_press(move |s: &mut State| {
        s.open_menu = if s.open_menu == Some(menu) {
            None
        } else {
            Some(menu)
        };
    })
    .child(
        text(label)
            .font(TextStyle::new(13.0, 400.0))
            .color(color("#c8c8c8")),
    )
}

fn window_control(key: &'static str, label: &'static str, glyph: &'static Icon) -> El {
    let button = el()
        .key(key)
        .role("button")
        .label(label)
        .size(44.0, 38.0)
        .center()
        .hover(if key == "header-close" {
            color("#a83232")
        } else {
            color("#303030")
        })
        .child(
            icon(glyph, if key == "header-maximize" { 13.0 } else { 15.0 }).color(color("#c8c8c8")),
        );
    match key {
        "header-minimize" => button.on_press_tx(|_: &mut State, tx| minimize_window(tx)),
        "header-maximize" => button.on_press(|_: &mut State| {
            #[cfg(target_os = "windows")]
            toggle_maximize_window();
        }),
        _ => button.on_press_tx(|_: &mut State, tx| tx.close_window()),
    }
}

fn header(state: &State) -> El {
    let mut left = row()
        .key("header-menus")
        .h_full()
        .center_y()
        .gap(2.0)
        .px(8.0);
    if cfg!(target_os = "macos") {
        left = left.child(el().w(76.0));
    }
    left = left.child(
        header_icon_button(
            "header-sidebar-toggle",
            if state.settings_nav_visible {
                "Hide settings sidebar"
            } else {
                "Show settings sidebar"
            },
            &SIDEBAR,
        )
        .on_press(|s: &mut State| {
            s.settings_nav_visible = !s.settings_nav_visible;
            s.open_menu = None;
        }),
    );
    if !cfg!(target_os = "macos") {
        left = left
            .child(menu_button(
                "File",
                HeaderMenu::File,
                state.open_menu == Some(HeaderMenu::File),
            ))
            .child(menu_button(
                "View",
                HeaderMenu::View,
                state.open_menu == Some(HeaderMenu::View),
            ))
            .child(menu_button(
                "Window",
                HeaderMenu::Window,
                state.open_menu == Some(HeaderMenu::Window),
            ));
    }
    let title = row()
        .key("header-title")
        .grow(1.0)
        .h_full()
        .center_y()
        .gap(11.0)
        .window_drag()
        .child(el().size(1.0, 18.0).fill(color("#555555")))
        .child(icon(state.settings_section.icon(), 15.0).color(color("#c8c8c8")))
        .child(
            text("Agent Settings")
                .font(TextStyle::new(13.0, 500.0))
                .color(color("#e2e2e2")),
        )
        .child(
            text(state.settings_section.label())
                .font(TextStyle::new(13.0, 400.0))
                .color(color("#aaaaaa")),
        );
    let mut bar = row()
        .key("custom-header")
        .role("banner")
        .w_full()
        .h(44.0)
        .center_y()
        .fill(color("#151515"))
        .window_drag()
        .child(left)
        .child(title);
    if cfg!(target_os = "windows") {
        bar = bar.child(
            row()
                .key("header-window-controls")
                .h_full()
                .center_y()
                .child(window_control("header-minimize", "Minimize", &MINIMIZE))
                .child(window_control("header-maximize", "Maximize", &MAXIMIZE))
                .child(window_control("header-close", "Close", &CLOSE)),
        );
    }
    bar
}

fn popup_action(key: &'static str, label: &'static str, action: fn(&mut State, &mut Tx)) -> El {
    el().key(key)
        .role("menuitem")
        .label(label)
        .w_full()
        .h(30.0)
        .px(10.0)
        .center_y()
        .radius(5.0)
        .hover(color("#303030"))
        .on_press_tx(action)
        .child(
            text(label)
                .font(TextStyle::new(12.0, 400.0))
                .color(color("#e2e2e2")),
        )
}

fn menu_popup(menu: HeaderMenu) -> El {
    let x = match menu {
        HeaderMenu::File => 49.0,
        HeaderMenu::View => 91.0,
        HeaderMenu::Window => 137.0,
    };
    let mut popup = col()
        .key("header-popup")
        .role("menu")
        .label("Application menu")
        .w(204.0)
        .absolute(x, 43.0)
        .padding(5.0)
        .radius(7.0)
        .fill(color("#222222"))
        .border(1.0, color("#393939"));
    match menu {
        HeaderMenu::File | HeaderMenu::View => {
            popup = popup.child(
                text("No commands available yet")
                    .font(TextStyle::new(11.5, 400.0))
                    .color(color("#969696"))
                    .ml(8.0)
                    .my(8.0),
            );
        }
        HeaderMenu::Window => {
            popup = popup.child(popup_action("popup-minimize", "Minimize", |s, tx| {
                s.open_menu = None;
                minimize_window(tx);
            }));
            if cfg!(target_os = "windows") {
                popup = popup.child(popup_action("popup-maximize", "Maximize", |s, _| {
                    s.open_menu = None;
                    #[cfg(target_os = "windows")]
                    toggle_maximize_window();
                }));
            }
            popup = popup.child(popup_action("popup-close", "Close", |s, tx| {
                s.open_menu = None;
                tx.close_window();
            }));
        }
    }
    popup
}

fn nav_button(key: &'static str, label: &'static str, glyph: &'static Icon, active: bool) -> El {
    let button = el()
        .key(key)
        .role("button")
        .label(label)
        .size(38.0, 38.0)
        .center()
        .radius(12.0)
        .fill(if active {
            color("#333333")
        } else {
            Color::TRANSPARENT
        })
        .hover(color("#303030"))
        .child(icon(glyph, 18.0).color(if active {
            color("#f2f2f2")
        } else {
            color("#aaaaaa")
        }));
    if active {
        button.on_press(|s: &mut State| {
            s.notice = None;
            s.open_menu = None;
        })
    } else {
        button.on_press(move |s: &mut State| {
            s.notice = Some(label);
            s.open_menu = None;
        })
    }
}

fn rail() -> El {
    col()
        .key("navigation")
        .role("navigation")
        .label("Main navigation")
        .w(50.0)
        .h_full()
        .pt(16.0)
        .gap(6.0)
        .items_center()
        .fill(color("#151515"))
        .child(nav_button("nav-home", "Home", &HOME, false))
        .child(nav_button(
            "nav-automations",
            "Automations",
            &AUTOMATIONS,
            false,
        ))
        .child(nav_button("nav-fleet", "Fleet", &FLEET, false))
        .child(nav_button("nav-plugins", "Plugins", &PLUGINS, false))
        .child(nav_button(
            "nav-settings",
            "Agent Settings",
            &SETTINGS,
            true,
        ))
}

fn settings_nav_button(section: SettingsSection, active: bool) -> El {
    row()
        .key(section.key())
        .role("button")
        .label(format!("{} settings", section.label()))
        .w_full()
        .h(34.0)
        .px(10.0)
        .center_y()
        .gap(12.0)
        .radius(8.0)
        .fill(if active {
            color("#282828")
        } else {
            Color::TRANSPARENT
        })
        .hover(color("#303030"))
        .on_press(move |s: &mut State| {
            s.settings_section = section;
            s.notice = None;
            s.open_menu = None;
        })
        .child(icon(section.icon(), 15.0).color(if active {
            color("#f2f2f2")
        } else {
            color("#bdbdbd")
        }))
        .child(
            text(section.label())
                .font(TextStyle::new(13.0, if active { 550.0 } else { 400.0 }))
                .color(if active {
                    color("#f2f2f2")
                } else {
                    color("#c8c8c8")
                }),
        )
}

fn settings_sidenav(active: SettingsSection, connection: &ConnectionStatus) -> El {
    let mut sections = col().w_full().gap(2.0);
    for section in SETTINGS_SECTIONS {
        sections = sections.child(settings_nav_button(section, section == active));
    }
    col()
        .key("settings-sidenav")
        .role("navigation")
        .label("Agent settings sections")
        .w(272.0)
        .h_full()
        .px(12.0)
        .pt(12.0)
        .fill(color("#181818"))
        .child(
            text("Agent Settings")
                .font(TextStyle::new(22.0, 500.0))
                .color(color("#aaaaaa"))
                .mb(14.0),
        )
        .child(sections)
        .child(el().grow(1.0))
        .child(
            col()
                .w_full()
                .px(8.0)
                .pb(16.0)
                .gap(7.0)
                .child(
                    text("APP-SERVER")
                        .font(TextStyle::new(10.5, 600.0))
                        .color(color("#777777")),
                )
                .child(
                    row()
                        .center_y()
                        .gap(7.0)
                        .child(el().size(7.0, 7.0).radius(3.5).fill(match connection {
                            ConnectionStatus::Connected { .. } => color("#72b783"),
                            ConnectionStatus::Connecting => color("#b9a15e"),
                            ConnectionStatus::Offline => color("#777777"),
                            ConnectionStatus::Incompatible | ConnectionStatus::InvalidAddress => {
                                color("#cf7777")
                            }
                        }))
                        .child(
                            text(connection.label())
                                .font(TextStyle::new(12.0, 400.0))
                                .color(color("#aaaaaa")),
                        ),
                ),
        )
}

fn provider_tab(provider: Provider, selected: bool) -> El {
    let name = provider.name();
    row()
        .key(match provider {
            Provider::OpenAi => "provider-openai",
            Provider::Xai => "provider-xai",
        })
        .role("tab")
        .label(format!(
            "{name} provider view{}",
            if selected { ", selected" } else { "" }
        ))
        .h(40.0)
        .px(14.0)
        .center_y()
        .gap(8.0)
        .radius(20.0)
        .fill(if selected {
            color("#303030")
        } else {
            color("#1b1b1b")
        })
        .hover(color("#383838"))
        .on_press(move |s: &mut State| {
            s.selected = provider;
            s.notice = None;
            s.open_menu = None;
            s.sign_in_prompt = None;
            s.api_key_dialog = false;
            s.api_key_edit.clear();
            s.auth_message.clear();
        })
        .child(
            el().size(21.0, 21.0)
                .center()
                .radius(6.0)
                .fill(color("#424242"))
                .child(
                    text(provider.initial())
                        .font(TextStyle::new(12.0, 650.0))
                        .color(color("#f2f2f2")),
                ),
        )
        .child(
            text(name)
                .font(TextStyle::new(13.0, if selected { 550.0 } else { 400.0 }))
                .color(if selected {
                    color("#f2f2f2")
                } else {
                    color("#c8c8c8")
                }),
        )
}

fn provider_setting_row(label: &'static str, description: &'static str) -> El {
    row()
        .w_full()
        .h(62.0)
        .center_y()
        .gap(12.0)
        .child(
            col()
                .grow(1.0)
                .gap(4.0)
                .child(
                    text(label)
                        .font(TextStyle::new(13.0, 550.0))
                        .color(color("#e2e2e2")),
                )
                .child(
                    text(description)
                        .font(TextStyle::new(11.5, 400.0))
                        .color(color("#969696")),
                ),
        )
        .child(
            text("Unavailable")
                .font(TextStyle::new(11.5, 400.0))
                .color(color("#777777")),
        )
}

fn credential_button(label: &'static str, action: impl Fn(&mut State) + 'static) -> El {
    el().h(30.0)
        .px(12.0)
        .center()
        .radius(7.0)
        .fill(color("#303030"))
        .hover(color("#414141"))
        .on_press(action)
        .child(
            text(label)
                .font(TextStyle::new(11.5, 550.0))
                .color(color("#eeeeee")),
        )
}

fn authentication_row(
    provider: Provider,
    status: &AuthStatus,
    loaded: bool,
    busy: bool,
    subscription: bool,
) -> El {
    let configured = if subscription {
        status.subscription
    } else {
        status.api_key
    };
    let label = if subscription {
        "Subscription"
    } else {
        "API key"
    };
    let description = if subscription {
        "Use your provider subscription account."
    } else {
        "Store a provider key on this host."
    };
    let status_label = if !loaded {
        "Unavailable"
    } else if subscription && status.in_progress {
        "Signing in"
    } else if configured {
        "Configured"
    } else {
        "Not configured"
    };
    let action = if subscription {
        if configured {
            credential_button("Forget", move |s| {
                start_auth_action(s, Action::ForgetSubscription(provider.id()))
            })
        } else {
            credential_button("Sign in", move |s| {
                start_auth_action(s, Action::StartSubscription(provider.id()))
            })
        }
    } else if configured {
        credential_button("Forget", move |s| {
            start_auth_action(s, Action::RemoveApiKey(provider.id()))
        })
    } else {
        credential_button("Configure", move |s| {
            if !s.auth_busy && matches!(s.connection, ConnectionStatus::Connected { .. }) {
                s.api_key_edit.clear();
                s.api_key_dialog = true;
                s.auth_message.clear();
            }
        })
        .key("configure-api-key")
    };
    row()
        .w_full()
        .h(70.0)
        .center_y()
        .gap(12.0)
        .child(
            col()
                .grow(1.0)
                .gap(4.0)
                .child(
                    row()
                        .gap(8.0)
                        .center_y()
                        .child(
                            text(label)
                                .font(TextStyle::new(13.0, 550.0))
                                .color(color("#e2e2e2")),
                        )
                        .child(text(status_label).font(TextStyle::new(11.0, 400.0)).color(
                            if configured {
                                color("#72b783")
                            } else {
                                color("#929292")
                            },
                        )),
                )
                .child(
                    text(description)
                        .font(TextStyle::new(11.5, 400.0))
                        .color(color("#969696")),
                ),
        )
        .child(if !loaded || busy || (subscription && status.in_progress) {
            el().child(
                text(if loaded { "Please wait" } else { "Unavailable" })
                    .font(TextStyle::new(11.5, 400.0))
                    .color(color("#777777")),
            )
        } else {
            action
        })
}

fn provider_settings_group(
    key: &'static str,
    heading: &'static str,
    description: &'static str,
    first: El,
    second: El,
) -> El {
    col()
        .key(key)
        .w_full()
        .gap(9.0)
        .child(
            text(heading)
                .font(TextStyle::new(15.0, 600.0))
                .color(color("#f2f2f2")),
        )
        .child(
            text(description)
                .font(TextStyle::new(12.0, 400.0))
                .color(color("#969696")),
        )
        .child(
            col()
                .w_full()
                .px(16.0)
                .radius(10.0)
                .fill(color("#191919"))
                .child(first)
                .child(el().w_full().h(1.0).fill(color("#2c2c2c")))
                .child(second),
        )
}

fn provider_detail(state: &State) -> El {
    let provider = state.selected;
    let auth = &state.auth_status[provider.index()];
    let name = provider.name();
    let mut detail = col()
        .key(match provider {
            Provider::OpenAi => "provider-detail-openai",
            Provider::Xai => "provider-detail-xai",
        })
        .role("tabpanel")
        .label(format!("{name} provider settings"))
        .w_full()
        .max_w(750.0)
        .gap(22.0)
        .child(
            col()
                .gap(7.0)
                .child(
                    row()
                        .center_y()
                        .gap(9.0)
                        .child(
                            el().size(28.0, 28.0)
                                .center()
                                .radius(8.0)
                                .fill(color("#303030"))
                                .child(
                                    text(provider.initial())
                                        .font(TextStyle::new(16.0, 650.0))
                                        .color(color("#f2f2f2")),
                                ),
                        )
                        .child(
                            text(name)
                                .font(TextStyle::new(21.0, 600.0))
                                .color(color("#f2f2f2")),
                        ),
                )
                .child(
                    text(provider.description())
                        .font(TextStyle::new(12.5, 400.0))
                        .color(color("#969696")),
                ),
        )
        .child(provider_settings_group(
            "provider-authentication",
            "Authentication",
            "Choose how Beale authenticates with this provider.",
            authentication_row(provider, auth, state.auth_loaded, state.auth_busy, true),
            authentication_row(provider, auth, state.auth_loaded, state.auth_busy, false),
        ));
    if state.api_key_dialog {
        detail = detail.child(
            col()
                .key("api-key-form")
                .w_full()
                .gap(10.0)
                .padding(16.0)
                .radius(10.0)
                .fill(color("#191919"))
                .child(
                    text(format!("Configure {} API key", provider.name()))
                        .font(TextStyle::new(13.0, 550.0))
                        .color(color("#eeeeee")),
                )
                .child(
                    text_input(&state.api_key_edit, TextStyle::new(13.0, 400.0))
                        .key("api-key-input")
                        .single_line()
                        .secure()
                        .w_full()
                        .h(38.0)
                        .padding(8.0)
                        .radius(6.0)
                        .fill(color("#282828"))
                        .placeholder("Paste API key", color("#888888"))
                        .on_edit(|s: &mut State| &mut s.api_key_edit),
                )
                .child(
                    row()
                        .gap(8.0)
                        .child(credential_button("Save", move |s| {
                            let key = s.api_key_edit.text.trim().to_owned();
                            if key.is_empty() {
                                s.auth_message = "Enter an API key.".to_owned();
                                return;
                            }
                            s.api_key_edit.clear();
                            s.api_key_dialog = false;
                            start_auth_action(s, Action::SetApiKey(provider.id(), key));
                        }))
                        .child(credential_button("Cancel", |s| {
                            s.api_key_edit.clear();
                            s.api_key_dialog = false;
                        })),
                ),
        );
    }
    let message = if auth.error.is_empty() {
        &state.auth_message
    } else {
        &auth.error
    };
    if !message.is_empty() {
        detail = detail.child(
            text(message.clone())
                .font(TextStyle::new(12.0, 400.0))
                .color(color("#c8c8c8")),
        );
    }
    if let Some(prompt) = &state.sign_in_prompt {
        detail = detail.child(
            col()
                .gap(7.0)
                .child(
                    text("Finish sign-in in your browser. Open the link again if needed:")
                        .font(TextStyle::new(12.0, 400.0))
                        .color(color("#b8b8b8")),
                )
                .child(
                    el().h(30.0)
                        .px(12.0)
                        .center()
                        .radius(7.0)
                        .fill(color("#303030"))
                        .hover(color("#414141"))
                        .on_press_tx({
                            let url = prompt.url.clone();
                            move |_: &mut State, tx| {
                                let _ = tx.open_url(&url);
                            }
                        })
                        .child(
                            text("Open sign-in page")
                                .font(TextStyle::new(11.5, 550.0))
                                .color(color("#eeeeee")),
                        ),
                )
                .child(
                    text(if prompt.code.is_empty() {
                        String::new()
                    } else {
                        format!("Device code: {}", prompt.code)
                    })
                    .font(TextStyle::new(13.0, 550.0))
                    .color(color("#eeeeee")),
                ),
        );
    }
    detail.child(provider_settings_group(
        "provider-model-defaults",
        "Default Models",
        "Choose models for primary and supporting research after connection.",
        provider_setting_row("Large Model", "Primary model for complex research work."),
        provider_setting_row(
            "Small Model",
            "Lighter model for supporting research tasks.",
        ),
    ))
}

fn view(state: &State, _: &mut Cx<State>) -> El {
    let mut content = col()
        .key("settings-page")
        .w_full()
        .max_w(760.0)
        .self_center()
        .gap(20.0);
    if state.settings_section == SettingsSection::Providers {
        content = content
            .child(
                row()
                    .key("provider-view-tabs")
                    .role("tablist")
                    .label("Provider views")
                    .w_full()
                    .h(40.0)
                    .gap(6.0)
                    .child(provider_tab(
                        Provider::OpenAi,
                        state.selected == Provider::OpenAi,
                    ))
                    .child(provider_tab(Provider::Xai, state.selected == Provider::Xai)),
            )
            .child(provider_detail(state));
    } else {
        content = content
            .child(
                text(state.settings_section.label())
                    .font(TextStyle::new(25.0, 600.0))
                    .color(color("#f2f2f2")),
            )
            .child(
                col()
                    .w_full()
                    .padding(22.0)
                    .radius(12.0)
                    .fill(color("#171717"))
                    .border(1.0, color("#2a2a2a"))
                    .child(
                        text(format!(
                            "{} settings are not available in this preview.",
                            state.settings_section.label()
                        ))
                        .font(TextStyle::new(13.0, 400.0))
                        .color(color("#aaaaaa")),
                    ),
            );
    }
    if let Some(label) = state.notice {
        content = content.child(
            text(format!("{label} is not available in this preview."))
                .font(TextStyle::new(12.0, 400.0))
                .color(color("#c8c8c8")),
        );
    }
    let mut body = row()
        .key("body")
        .w_full()
        .grow(1.0)
        .pr(6.0)
        .pb(6.0)
        .fill(color("#151515"));
    body = body.child(rail());
    let mut settings_surface = row()
        .key("settings-surface")
        .grow(1.0)
        .h_full()
        .radius(16.0)
        .clip_rounded(16.0)
        .fill(color("#181818"));
    if state.settings_nav_visible {
        settings_surface = settings_surface
            .child(settings_sidenav(state.settings_section, &state.connection))
            .child(el().w(1.0).h_full().fill(color("#2a2a2a")));
    }
    settings_surface = settings_surface.child(
        col()
            .key("content-surface")
            .grow(1.0)
            .h_full()
            .min_h(0.0)
            .fill(color("#0d0d0d"))
            .child(
                col()
                    .key("settings-scroll")
                    .grow(1.0)
                    .w_full()
                    .min_h(0.0)
                    .scroll(ScrollSpec {
                        stick_bottom: false,
                    })
                    .child(
                        col()
                            .w_full()
                            .padding(if state.settings_section == SettingsSection::Providers {
                                20.0
                            } else {
                                36.0
                            })
                            .child(content),
                    ),
            ),
    );
    body = body.child(settings_surface);
    let mut window = col()
        .key("window")
        .role("window")
        .w_full()
        .h_full()
        .fill(color("#151515"))
        .child(header(state))
        .child(body);
    if let Some(menu) = state.open_menu {
        window = window.child(menu_popup(menu));
    }
    window
}

fn main() -> std::process::ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args == ["--check-connection"] {
        return connection::check_cli();
    }
    if args == ["--provider-status"] {
        return provider_auth::check_cli();
    }
    let spec = AppSpec {
        scenes: &[
            ("xai", |s: &mut State| s.selected = Provider::Xai),
            ("window-menu", |s: &mut State| {
                s.open_menu = Some(HeaderMenu::Window)
            }),
        ],
        min_size: Some((840.0, 600.0)),
        ..AppSpec::new("Beale", (1040.0, 720.0), State::default, view)
    };
    let app = rune::App::new(spec).chrome(rune::Chrome {
        drag_regions: true,
        ..rune::Chrome::DEFAULT
    });
    #[cfg(target_os = "windows")]
    let app = app
        .window_options(rune::WindowOptions {
            borderless: true,
            ..rune::WindowOptions::default()
        })
        .on_window(|window| {
            let slot = MAIN_WINDOW.get_or_init(|| std::sync::Mutex::new(None));
            if let Ok(mut current) = slot.lock() {
                *current = Some(window.clone());
            }
            windows_frame::configure(window);
        });
    let handle = app.handle();
    let _ = AUTH_HANDLE.set(handle.clone());
    std::thread::spawn(move || {
        let Ok(addr) = connection::configured_addr() else {
            handle.update(|state, _| state.connection = ConnectionStatus::InvalidAddress);
            return;
        };
        let Ok(client) = connection::http_client() else {
            handle.update(|state, _| state.connection = ConnectionStatus::Offline);
            return;
        };
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            handle.update(|state, _| state.connection = ConnectionStatus::Offline);
            return;
        };
        loop {
            let status = runtime.block_on(connection::probe(&client, addr));
            let auth_result = if matches!(status, ConnectionStatus::Connected { .. }) {
                runtime.block_on(provider_auth::status(&client, addr)).ok()
            } else {
                None
            };
            let auth_loaded = auth_result.is_some();
            let auth_status = auth_result.unwrap_or_default();
            handle.update_if(move |state, _| {
                if state.connection == status
                    && state.auth_status == auth_status
                    && state.auth_loaded == auth_loaded
                {
                    false
                } else {
                    state.connection = status;
                    state.auth_status = auth_status;
                    state.auth_loaded = auth_loaded;
                    if state.auth_status[state.selected.index()].subscription
                        && !state.auth_status[state.selected.index()].in_progress
                    {
                        state.sign_in_prompt = None;
                    }
                    true
                }
            });
            std::thread::sleep(Duration::from_secs(2));
        }
    });
    app.run();
    std::process::ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn selecting_a_provider_updates_the_detail() {
        let mut runtime =
            Runtime::new(State::default(), view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let (x, y) = runtime.find("provider-xai").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert_eq!(runtime.state.selected, Provider::Xai);
        assert!(runtime.find("provider-detail-xai").is_some());
        assert!(runtime.find("provider-detail-openai").is_none());
    }

    #[test]
    fn api_key_configuration_uses_a_masked_field() {
        let state = State {
            connection: ConnectionStatus::Connected { major: 1, minor: 1 },
            auth_loaded: true,
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let (x, y) = runtime.find("configure-api-key").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert!(runtime.find("api-key-input").is_some());
        assert!(runtime.state.api_key_edit.is_secure());
    }

    #[test]
    fn header_toggles_navigation_and_opens_window_menu() {
        let mut runtime =
            Runtime::new(State::default(), view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let (x, y) = runtime.find("header-sidebar-toggle").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert!(!runtime.state.settings_nav_visible);
        assert!(runtime.find("settings-sidenav").is_none());
        let (x, y) = runtime.find("menu-window").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert_eq!(runtime.state.open_menu, Some(HeaderMenu::Window));
        assert!(runtime.find("popup-close").is_some());
    }

    #[test]
    fn settings_sidenav_switches_sections_and_returns_to_providers() {
        let mut runtime =
            Runtime::new(State::default(), view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let (x, y) = runtime.find("settings-appearance").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert_eq!(runtime.state.settings_section, SettingsSection::Appearance);
        assert!(runtime.find("provider-openai").is_none());
        let (x, y) = runtime.find("settings-providers").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert_eq!(runtime.state.settings_section, SettingsSection::Providers);
        assert!(runtime.find("provider-openai").is_some());
    }
}
