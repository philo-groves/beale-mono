use rune::tok::TextStyle;
use rune::{
    AppSpec, Color, Cursor, Cx, Drag, El, Icon, Key, KeyEvent, Phase, ScrollSpec, TextEdit, Tx,
    col, el, icon, row, text, text_input,
};
use std::time::Duration;

mod connection;
mod desktop_fonts;
mod icons;
mod in_place_menu;
mod provider_auth;
mod provider_models;
use beale_contracts::v1::{AuthMethod, ProviderId};
use connection::ConnectionStatus;
use icons::*;
use provider_auth::{Action, AuthStatus, SignInPrompt};
use provider_models::ModelCatalog;

#[cfg(target_os = "windows")]
mod windows_frame;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Provider {
    #[default]
    OpenAi,
    Xai,
}

impl Provider {
    fn from_id(id: ProviderId) -> Option<Self> {
        match id {
            ProviderId::Openai => Some(Self::OpenAi),
            ProviderId::Xai => Some(Self::Xai),
            ProviderId::Unspecified => None,
        }
    }

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ModelRole {
    Lead,
    Small,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SelectionMenu {
    LeadProvider,
    Model {
        provider: Provider,
        method: AuthMethod,
        role: ModelRole,
    },
}

fn catalog_index(provider: Provider, method: AuthMethod) -> usize {
    provider.index() * 2 + if method == AuthMethod::ApiKey { 1 } else { 0 }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum SettingsSection {
    General,
    Appearance,
    #[default]
    Providers,
    Features,
    Archive,
}

impl SettingsSection {
    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Appearance => "Appearance",
            Self::Providers => "Providers",
            Self::Features => "Features",
            Self::Archive => "Archive",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::General => "settings-general",
            Self::Appearance => "settings-appearance",
            Self::Providers => "settings-providers",
            Self::Features => "settings-features",
            Self::Archive => "settings-archive",
        }
    }

    fn icon(self) -> &'static Icon {
        match self {
            Self::General => &SETTINGS,
            Self::Appearance => &APPEARANCE,
            Self::Providers => &PROVIDERS,
            Self::Features => &FEATURES,
            Self::Archive => &ARCHIVE,
        }
    }
}

const SETTINGS_SECTIONS: [SettingsSection; 5] = [
    SettingsSection::General,
    SettingsSection::Appearance,
    SettingsSection::Providers,
    SettingsSection::Features,
    SettingsSection::Archive,
];

const NAVIGATION_RAIL_WIDTH: f32 = 50.0;
const SIDENAV_DEFAULT_WIDTH: f32 = 292.0;
const SIDENAV_MIN_WIDTH: f32 = 240.0;
const SIDENAV_MAX_WIDTH: f32 = 420.0;
const HEADER_MENU_FALLBACK_WIDTH: f32 = if cfg!(target_os = "macos") {
    128.0
} else {
    220.0
};

struct State {
    selected: Provider,
    settings_section: SettingsSection,
    connection: ConnectionStatus,
    auth_status: [AuthStatus; 2],
    auth_loaded: bool,
    lead_provider: Option<Provider>,
    lead_busy: bool,
    lead_error: String,
    provider_selection_initialized: bool,
    native_window_border: bool,
    auth_message: String,
    sign_in_prompt: Option<SignInPrompt>,
    api_key_edit: TextEdit,
    api_key_dialog: bool,
    auth_busy: bool,
    model_method: [AuthMethod; 2],
    model_catalogs: [Option<ModelCatalog>; 4],
    model_loading: [bool; 4],
    model_errors: [String; 4],
    model_saving: bool,
    selection_menu: Option<SelectionMenu>,
    menu_cursor: usize,
    menu_reveal_pending: bool,
    notice: Option<&'static str>,
    settings_nav_visible: bool,
    settings_nav_width: f32,
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
            lead_provider: None,
            lead_busy: false,
            lead_error: String::new(),
            provider_selection_initialized: false,
            native_window_border: false,
            auth_message: String::new(),
            sign_in_prompt: None,
            api_key_edit: TextEdit::new("").secure(),
            api_key_dialog: false,
            auth_busy: false,
            model_method: [AuthMethod::Subscription; 2],
            model_catalogs: std::array::from_fn(|_| None),
            model_loading: [false; 4],
            model_errors: std::array::from_fn(|_| String::new()),
            model_saving: false,
            selection_menu: None,
            menu_cursor: 0,
            menu_reveal_pending: false,
            notice: None,
            settings_nav_visible: true,
            settings_nav_width: SIDENAV_DEFAULT_WIDTH,
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

fn start_model_list(state: &mut State, provider: Provider, method: AuthMethod) {
    let index = catalog_index(provider, method);
    if state.model_loading[index] || !state.auth_loaded {
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        return;
    };
    state.model_loading[index] = true;
    state.model_errors[index].clear();
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid local app-server address".to_owned())?;
            let client = provider_models::client()?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize model connection".to_owned())?;
            runtime.block_on(provider_models::list(&client, addr, provider.id(), method))
        })();
        handle.update(move |state, _| {
            state.model_loading[index] = false;
            let authenticated = match method {
                AuthMethod::Subscription => state.auth_status[provider.index()].subscription,
                AuthMethod::ApiKey => state.auth_status[provider.index()].api_key,
                _ => false,
            };
            if !state.auth_loaded || !authenticated {
                return;
            }
            match result {
                Ok(catalog) => state.model_catalogs[index] = Some(catalog),
                Err(error) => state.model_errors[index] = error,
            }
        });
    });
}

fn start_lead_save(state: &mut State, provider: Provider) {
    let configured = &state.auth_status[provider.index()];
    if state.lead_busy || !state.auth_loaded || (!configured.subscription && !configured.api_key) {
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        return;
    };
    state.lead_busy = true;
    state.lead_error.clear();
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid local app-server address".to_owned())?;
            let client = connection::http_client()
                .map_err(|_| "Could not initialize app-server connection".to_owned())?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize app-server connection".to_owned())?;
            runtime.block_on(provider_auth::set_lead(&client, addr, provider.id()))
        })();
        handle.update(move |state, _| {
            state.lead_busy = false;
            match result {
                Ok(()) => {
                    state.lead_provider = Some(provider);
                }
                Err(error) => state.lead_error = error,
            }
        });
    });
}

fn start_model_save(
    state: &mut State,
    provider: Provider,
    method: AuthMethod,
    role: ModelRole,
    id: String,
) {
    let index = catalog_index(provider, method);
    if state.model_saving {
        return;
    }
    let Some(catalog) = &state.model_catalogs[index] else {
        return;
    };
    if !catalog.models.iter().any(|model| model.id == id) {
        return;
    }
    let (lead, small) = match role {
        ModelRole::Lead => (id, catalog.small.clone()),
        ModelRole::Small => (catalog.lead.clone(), id),
    };
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        return;
    };
    state.selection_menu = None;
    state.model_saving = true;
    state.model_errors[index].clear();
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid local app-server address".to_owned())?;
            let client = provider_models::client()?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize model connection".to_owned())?;
            runtime.block_on(provider_models::save(
                &client,
                addr,
                provider.id(),
                method,
                lead.clone(),
                small.clone(),
            ))
        })();
        handle.update(move |state, _| {
            state.model_saving = false;
            match result {
                Ok(()) => {
                    if let Some(catalog) = &mut state.model_catalogs[index] {
                        catalog.lead = lead;
                        catalog.small = small;
                    }
                }
                Err(error) => state.model_errors[index] = error,
            }
        });
    });
}

fn selection_menu_items(state: &State, menu: SelectionMenu) -> Vec<in_place_menu::MenuItem> {
    match menu {
        SelectionMenu::LeadProvider => [Provider::OpenAi, Provider::Xai]
            .into_iter()
            .filter(|provider| {
                let status = &state.auth_status[provider.index()];
                status.subscription || status.api_key
            })
            .map(|provider| in_place_menu::MenuItem {
                value: match provider {
                    Provider::OpenAi => "openai",
                    Provider::Xai => "xai",
                }
                .to_owned(),
                label: provider.name().to_owned(),
                detail: None,
                selected: state.lead_provider == Some(provider),
            })
            .collect(),
        SelectionMenu::Model {
            provider,
            method,
            role,
        } => {
            if state.selected != provider || state.model_method[provider.index()] != method {
                return Vec::new();
            }
            state.model_catalogs[catalog_index(provider, method)]
                .as_ref()
                .map(|catalog| {
                    let selected = match role {
                        ModelRole::Lead => &catalog.lead,
                        ModelRole::Small => &catalog.small,
                    };
                    catalog
                        .models
                        .iter()
                        .map(|model| in_place_menu::MenuItem {
                            value: model.id.clone(),
                            label: model.name.clone(),
                            detail: Some(model.id.clone()),
                            selected: model.id == *selected,
                        })
                        .collect()
                })
                .unwrap_or_default()
        }
    }
}

fn selection_menu_anchor(menu: SelectionMenu) -> &'static str {
    match menu {
        SelectionMenu::LeadProvider => "lead-provider-control",
        SelectionMenu::Model {
            role: ModelRole::Lead,
            ..
        } => "choose-lead-model",
        SelectionMenu::Model {
            role: ModelRole::Small,
            ..
        } => "choose-small-model",
    }
}

fn toggle_selection_menu(state: &mut State, menu: SelectionMenu) {
    if matches!(menu, SelectionMenu::Model { .. }) && state.model_saving {
        return;
    }
    if state.selection_menu == Some(menu) {
        state.selection_menu = None;
        return;
    }
    let items = selection_menu_items(state, menu);
    if items.is_empty() {
        return;
    }
    state.menu_cursor = items.iter().position(|item| item.selected).unwrap_or(0);
    state.menu_reveal_pending = true;
    state.selection_menu = Some(menu);
    state.open_menu = None;
}

fn choose_selection_menu_item(state: &mut State, menu: SelectionMenu, value: String) {
    if state.selection_menu != Some(menu) {
        return;
    }
    if !selection_menu_items(state, menu)
        .iter()
        .any(|item| item.value == value)
    {
        return;
    }
    state.selection_menu = None;
    match menu {
        SelectionMenu::LeadProvider => {
            let provider = if value == "openai" {
                Provider::OpenAi
            } else {
                Provider::Xai
            };
            start_lead_save(state, provider);
        }
        SelectionMenu::Model {
            provider,
            method,
            role,
        } => start_model_save(state, provider, method, role, value),
    }
}

fn selection_menu_key(state: &mut State, event: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(menu) = state.selection_menu else {
        return false;
    };
    let items = selection_menu_items(state, menu);
    if items.is_empty() {
        state.selection_menu = None;
        return false;
    }
    let last = items.len() - 1;
    match &event.key {
        Key::Escape => {
            state.selection_menu = None;
            true
        }
        Key::Down | Key::Up | Key::Home | Key::End => {
            state.menu_cursor = match &event.key {
                Key::Down => (state.menu_cursor + 1).min(last),
                Key::Up => state.menu_cursor.saturating_sub(1),
                Key::Home => 0,
                Key::End => last,
                _ => state.menu_cursor.min(last),
            };
            tx.scroll_into_view_in(
                "selection-menu-scroll",
                format!("selection-menu-item-{}", state.menu_cursor),
            );
            true
        }
        Key::Enter => {
            let index = state.menu_cursor.min(last);
            choose_selection_menu_item(state, menu, items[index].value.clone());
            true
        }
        _ => false,
    }
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
        .child(icon(glyph, 16.0).color(color("#c8c8c8")))
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
            icon(glyph, if key == "header-maximize" { 14.0 } else { 16.0 }).color(color("#c8c8c8")),
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

fn header(state: &State, prefix_width: f32) -> El {
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
        .child(
            el().key("header-divider")
                .size(1.0, 18.0)
                .fill(color("#555555")),
        )
        .child(icon(state.settings_section.icon(), 16.0).color(color("#c8c8c8")))
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
        .window_drag()
        .child(el().w(prefix_width).h_full().child(left))
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
    let glyph = if active && key == "nav-settings" {
        &SETTINGS_FILLED
    } else {
        glyph
    };
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
        .child(icon(glyph, 20.0).color(if active {
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
        .w(NAVIGATION_RAIL_WIDTH)
        .h_full()
        .pt(16.0)
        .gap(6.0)
        .items_center()
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
        .h(30.0)
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
            s.selection_menu = None;
        })
        .child(icon(section.icon(), 16.0).color(if active {
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

fn settings_sidenav(active: SettingsSection, connection: &ConnectionStatus, width: f32) -> El {
    let mut sections = col().w_full().gap(2.0);
    for section in SETTINGS_SECTIONS {
        sections = sections.child(settings_nav_button(section, section == active));
    }
    col()
        .key("settings-sidenav")
        .role("navigation")
        .label("Agent settings sections")
        .w(width)
        .h_full()
        .px(8.0)
        .pt(12.0)
        .fill(color("#111111"))
        .child(
            text("Agent Settings")
                .font(TextStyle::new(20.0, 500.0))
                .color(color("#aaaaaa"))
                .ml(4.0)
                .mb(12.0),
        )
        .child(sections)
        .child(el().grow(1.0))
        .child(
            col()
                .w_full()
                .px(12.0)
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

fn sidenav_resize_handle(width: f32) -> El {
    el().key("settings-sidenav-resize")
        .role("separator")
        .label("Resize settings sidebar")
        .absolute(width - 3.0, 0.0)
        .w(7.0)
        .h_full()
        .cursor(Cursor::ResizeColumn)
        .help("Drag to resize the sidebar")
        .on_drag(|state: &mut State, drag: &mut Drag, tx: &mut Tx| {
            tx.instant();
            match drag.phase {
                Phase::Down => drag.data = state.settings_nav_width,
                Phase::Move => {
                    state.settings_nav_width = (drag.data + drag.dx)
                        .clamp(SIDENAV_MIN_WIDTH, SIDENAV_MAX_WIDTH)
                        .round();
                }
                Phase::Up if drag.cancelled() => state.settings_nav_width = drag.data,
                Phase::Up => {}
            }
        })
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
            s.selection_menu = None;
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

fn lead_provider_controls(state: &State) -> El {
    let configured = state
        .auth_status
        .iter()
        .any(|status| status.subscription || status.api_key);
    let label = match state.lead_provider {
        Some(Provider::OpenAi) => "OpenAI",
        Some(Provider::Xai) => "xAI",
        None => "None",
    };
    row()
        .center_y()
        .gap(7.0)
        .child(
            text("Lead")
                .font(TextStyle::new(12.0, 500.0))
                .color(color("#a8a8a8")),
        )
        .child(if state.auth_loaded && configured && !state.lead_busy {
            selection_button(
                label,
                state.selection_menu == Some(SelectionMenu::LeadProvider),
                |s| toggle_selection_menu(s, SelectionMenu::LeadProvider),
            )
            .key("lead-provider-control")
        } else {
            el().child(
                text(if state.lead_busy { "Saving" } else { label })
                    .font(TextStyle::new(11.5, 400.0))
                    .color(color("#888888")),
            )
        })
}

fn form_button(label: impl Into<String>) -> El {
    row()
        .h(26.0)
        .px(12.0)
        .center()
        .gap(6.0)
        .radius(8.0)
        .continuous_corners()
        .fill(color("#303030"))
        .hover(color("#414141"))
        .child(
            text(label.into())
                .font(TextStyle::new(12.0, 550.0))
                .min_w(0.0)
                .truncate()
                .color(color("#eeeeee")),
        )
}

fn credential_button(label: &'static str, action: impl Fn(&mut State) + 'static) -> El {
    form_button(label).on_press(action)
}

fn selection_button(
    label: impl Into<String>,
    open: bool,
    action: impl Fn(&mut State) + 'static,
) -> El {
    form_button(label)
        .max_w(220.0)
        .fill(if open {
            color("#303030")
        } else {
            Color::TRANSPARENT
        })
        .border(1.0, color("#2c2c2c"))
        .child(icon(if open { &CHEVRON_UP } else { &CHEVRON_DOWN }, 13.0).color(color("#b8b8b8")))
        .on_press(action)
}

fn form_item_row() -> El {
    row().w_full().h(62.0).center_y().gap(12.0)
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
    form_item_row()
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
                        .font(TextStyle::new(12.0, 400.0))
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

fn provider_settings_group(key: &'static str, heading: &'static str, first: El, second: El) -> El {
    col()
        .key(key)
        .w_full()
        .gap(12.0)
        .child(
            text(heading)
                .font(TextStyle::new(15.0, 600.0))
                .color(color("#f2f2f2")),
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

fn model_default_row(state: &State, provider: Provider, method: AuthMethod, role: ModelRole) -> El {
    let index = catalog_index(provider, method);
    let catalog = state.model_catalogs[index].as_ref();
    let selected = catalog.map(|catalog| match role {
        ModelRole::Lead => &catalog.lead,
        ModelRole::Small => &catalog.small,
    });
    let value = selected
        .map(|id| {
            catalog
                .and_then(|catalog| {
                    catalog
                        .models
                        .iter()
                        .find(|model| &model.id == id)
                        .map(|model| model.name.as_str())
                })
                .unwrap_or(id)
        })
        .unwrap_or(if state.model_loading[index] {
            "Loading…"
        } else {
            "Unavailable"
        });
    let (label, description) = match role {
        ModelRole::Lead => ("Lead Model", "Primary model for complex research work."),
        ModelRole::Small => (
            "Small Model",
            "Lighter model for supporting research tasks.",
        ),
    };
    let menu = SelectionMenu::Model {
        provider,
        method,
        role,
    };
    form_item_row()
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
                        .font(TextStyle::new(12.0, 400.0))
                        .color(color("#969696")),
                ),
        )
        .child(if catalog.is_some() {
            selection_button(
                value.to_owned(),
                state.selection_menu == Some(menu),
                move |s| toggle_selection_menu(s, menu),
            )
            .key(match role {
                ModelRole::Lead => "choose-lead-model",
                ModelRole::Small => "choose-small-model",
            })
        } else {
            text(value.to_owned())
                .font(TextStyle::new(11.5, 450.0))
                .color(color("#bcbcbc"))
        })
}

fn model_settings(state: &State, provider: Provider) -> El {
    let method = state.model_method[provider.index()];
    let index = catalog_index(provider, method);
    let auth = &state.auth_status[provider.index()];
    let configured = match method {
        AuthMethod::Subscription => auth.subscription,
        AuthMethod::ApiKey => auth.api_key,
        _ => false,
    };
    let mut section = col().w_full().gap(12.0);
    let mut source = row().w_full().center_y().gap(8.0).child(
        text("Model access")
            .font(TextStyle::new(12.0, 500.0))
            .color(color("#a8a8a8")),
    );
    if auth.subscription {
        source = source.child(credential_button(
            if method == AuthMethod::Subscription {
                "Subscription (selected)"
            } else {
                "Subscription"
            },
            move |s| {
                s.model_method[provider.index()] = AuthMethod::Subscription;
                s.selection_menu = None;
            },
        ));
    }
    if auth.api_key {
        source = source.child(credential_button(
            if method == AuthMethod::ApiKey {
                "API key (selected)"
            } else {
                "API key"
            },
            move |s| {
                s.model_method[provider.index()] = AuthMethod::ApiKey;
                s.selection_menu = None;
            },
        ));
    }
    if configured {
        source = source.child(credential_button("Refresh", move |s| {
            let index = catalog_index(provider, method);
            s.model_catalogs[index] = None;
            start_model_list(s, provider, method);
        }));
    }
    section = section.child(source);
    section = section.child(provider_settings_group(
        "provider-model-defaults",
        "Default Models",
        model_default_row(state, provider, method, ModelRole::Lead),
        model_default_row(state, provider, method, ModelRole::Small),
    ));
    if !state.model_errors[index].is_empty() {
        section = section.child(
            text(state.model_errors[index].clone())
                .font(TextStyle::new(12.0, 400.0))
                .color(color("#cf8888")),
        );
    } else if !configured {
        section = section.child(
            text("Connect this sign-in method to load its models.")
                .font(TextStyle::new(12.0, 400.0))
                .color(color("#969696")),
        );
    }
    section
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
                .child(form_button("Open sign-in page").on_press_tx({
                    let url = prompt.url.clone();
                    move |_: &mut State, tx| {
                        let _ = tx.open_url(&url);
                    }
                }))
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
    detail.child(model_settings(state, provider))
}

fn view(state: &State, cx: &mut Cx<State>) -> El {
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
                    .w_full()
                    .h(40.0)
                    .child(
                        row()
                            .key("provider-view-tabs")
                            .role("tablist")
                            .label("Provider views")
                            .gap(6.0)
                            .child(provider_tab(
                                Provider::OpenAi,
                                state.selected == Provider::OpenAi,
                            ))
                            .child(provider_tab(Provider::Xai, state.selected == Provider::Xai)),
                    )
                    .child(el().grow(1.0))
                    .child(lead_provider_controls(state)),
            )
            .child(if state.lead_error.is_empty() {
                el()
            } else {
                text(state.lead_error.clone())
                    .font(TextStyle::new(12.0, 400.0))
                    .color(color("#cf8888"))
            })
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
    let mut body = row().key("body").w_full().grow(1.0).pr(6.0).pb(6.0);
    body = body.child(rail());
    let mut settings_surface = row()
        .key("settings-surface")
        .grow(1.0)
        .h_full()
        .radius(16.0)
        .clip_rounded(16.0)
        .fill(color("#111111"));
    if state.settings_nav_visible {
        settings_surface = settings_surface
            .child(settings_sidenav(
                state.settings_section,
                &state.connection,
                state.settings_nav_width,
            ))
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
    if state.settings_nav_visible {
        settings_surface = settings_surface.child(sidenav_resize_handle(state.settings_nav_width));
    }
    body = body.child(settings_surface);
    let mut window = col().key("window").role("window").w_full().h_full();
    if cfg!(target_os = "windows") && !state.native_window_border {
        window = window.padding(1.0).fill(color("#323232"));
    }
    let [background_width, background_height] =
        cx.layout_size("app-surface").unwrap_or([1040.0, 720.0]);
    let menu_clearance = cx
        .layout_size("header-menus")
        .map_or(HEADER_MENU_FALLBACK_WIDTH, |size| size[0] + 8.0);
    let header_prefix_width = menu_clearance.max(if state.settings_nav_visible {
        NAVIGATION_RAIL_WIDTH + state.settings_nav_width
    } else {
        0.0
    });
    let upper_left_radius = ((background_width * 0.88).hypot(background_height * 0.92)) * 0.43;
    let lower_right_radius = ((background_width * 0.88).hypot(background_height * 0.92)) * 0.48;
    let mut surface = col()
        .key("app-surface")
        .w_full()
        .grow(1.0)
        .fill(color("#151515"))
        .linear_gradient(
            145.0_f32.to_radians(),
            [(0.0, color("#1d1d1d")), (1.0, color("#151515"))],
        )
        .child(
            el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .radial_gradient(
                    [0.88, 0.92],
                    lower_right_radius,
                    [(0.0, color("#1d1d1d")), (1.0, Color::TRANSPARENT)],
                ),
        )
        .child(
            el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .radial_gradient(
                    [0.12, 0.08],
                    upper_left_radius,
                    [(0.0, color("#2a2a2a")), (1.0, Color::TRANSPARENT)],
                ),
        )
        .child(header(state, header_prefix_width))
        .child(body);
    if let Some(menu) = state.open_menu {
        surface = surface.child(menu_popup(menu));
    }
    window = window.child(surface);
    if let Some(menu) = state.selection_menu
        && state.settings_section == SettingsSection::Providers
    {
        let items = selection_menu_items(state, menu);
        if !items.is_empty() {
            let (title, width) = match menu {
                SelectionMenu::LeadProvider => ("Lead provider", 178.0),
                SelectionMenu::Model { .. } => ("Choose model", 380.0),
            };
            window = window
                .child(in_place_menu::dismiss_layer(|s: &mut State| {
                    s.selection_menu = None
                }))
                .child(
                    in_place_menu::panel(
                        selection_menu_anchor(menu),
                        title,
                        width,
                        state.menu_cursor.min(items.len() - 1),
                        items,
                        move |s: &mut State, index| {
                            if s.selection_menu == Some(menu) {
                                s.menu_cursor = index;
                            }
                        },
                        move |s: &mut State, value| choose_selection_menu_item(s, menu, value),
                    )
                    .on_layout_tx(|s: &mut State, _, tx| {
                        if s.menu_reveal_pending {
                            s.menu_reveal_pending = false;
                            tx.scroll_into_view_in(
                                "selection-menu-scroll",
                                format!("selection-menu-item-{}", s.menu_cursor),
                            );
                        }
                    }),
                );
        }
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
    let font_spec = desktop_fonts::v1_font_spec();
    let spec = AppSpec {
        scenes: &[
            ("xai", |s: &mut State| s.selected = Provider::Xai),
            ("window-menu", |s: &mut State| {
                s.open_menu = Some(HeaderMenu::Window)
            }),
        ],
        min_size: Some((840.0, 600.0)),
        fonts: font_spec.fonts,
        families: font_spec.families,
        on_key: Some(selection_menu_key),
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
            let native_border = windows_frame::configure(window);
            if let Some(handle) = AUTH_HANDLE.get() {
                handle.update(move |state, _| state.native_window_border = native_border);
            }
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
            let auth_snapshot = auth_result.unwrap_or_default();
            let auth_status = auth_snapshot.providers;
            let lead_provider = auth_snapshot.lead.and_then(Provider::from_id);
            handle.update_if(move |state, _| {
                if state.connection == status
                    && state.auth_status == auth_status
                    && state.auth_loaded == auth_loaded
                    && state.lead_provider == lead_provider
                {
                    false
                } else {
                    let previous = state.auth_status.clone();
                    let was_loaded = state.auth_loaded;
                    let previous_lead = state.lead_provider;
                    state.connection = status;
                    state.auth_status = auth_status;
                    state.auth_loaded = auth_loaded;
                    state.lead_provider = lead_provider;
                    let first_added = was_loaded
                        && previous
                            .iter()
                            .all(|item| !item.subscription && !item.api_key)
                        && state
                            .auth_status
                            .iter()
                            .any(|item| item.subscription || item.api_key);
                    if auth_loaded && (!state.provider_selection_initialized || first_added) {
                        if let Some(lead) = lead_provider {
                            state.selected = lead;
                        }
                        state.provider_selection_initialized = true;
                    } else if let Some(old) = previous_lead
                        && state.selected == old
                        && lead_provider != Some(old)
                        && !state.auth_status[old.index()].subscription
                        && !state.auth_status[old.index()].api_key
                        && let Some(new_lead) = lead_provider
                    {
                        state.selected = new_lead;
                    }
                    if !auth_loaded
                        || state
                            .auth_status
                            .iter()
                            .all(|item| !item.subscription && !item.api_key)
                    {
                        state.selection_menu = None;
                    }
                    if previous_lead != lead_provider {
                        state.lead_error.clear();
                    }
                    for provider in [Provider::OpenAi, Provider::Xai] {
                        for method in [AuthMethod::Subscription, AuthMethod::ApiKey] {
                            let index = catalog_index(provider, method);
                            let old = if method == AuthMethod::Subscription {
                                previous[provider.index()].subscription
                            } else {
                                previous[provider.index()].api_key
                            };
                            let current = if method == AuthMethod::Subscription {
                                state.auth_status[provider.index()].subscription
                            } else {
                                state.auth_status[provider.index()].api_key
                            };
                            if old != current || !auth_loaded {
                                state.model_catalogs[index] = None;
                                state.model_loading[index] = false;
                                state.model_errors[index].clear();
                            }
                            if auth_loaded
                                && current
                                && state.model_catalogs[index].is_none()
                                && !state.model_loading[index]
                                && state.model_errors[index].is_empty()
                            {
                                start_model_list(state, provider, method);
                            }
                        }
                        if !state.auth_status[provider.index()].subscription
                            && state.auth_status[provider.index()].api_key
                        {
                            state.model_method[provider.index()] = AuthMethod::ApiKey;
                        }
                    }
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
    fn lead_menu_lists_only_configured_providers_without_moving_content() {
        let state = State {
            auth_loaded: true,
            lead_provider: Some(Provider::Xai),
            auth_status: [
                AuthStatus::default(),
                AuthStatus {
                    api_key: true,
                    ..AuthStatus::default()
                },
            ],
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let detail_before = runtime.find("provider-detail-openai").unwrap();
        let (x, y) = runtime.find("lead-provider-control").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert!(runtime.find("selection-menu").is_some());
        assert!(runtime.find("selection-menu-item-0").is_some());
        assert!(runtime.find("selection-menu-item-1").is_none());
        assert_eq!(
            runtime.find("provider-detail-openai").unwrap(),
            detail_before
        );
    }

    #[test]
    fn model_menu_closes_on_escape_and_outside_press() {
        let state = State {
            auth_loaded: true,
            auth_status: [
                AuthStatus {
                    subscription: true,
                    ..AuthStatus::default()
                },
                AuthStatus::default(),
            ],
            model_catalogs: [
                Some(ModelCatalog {
                    models: vec![
                        provider_models::ModelEntry {
                            id: "example-lead".to_owned(),
                            name: "Example Lead".to_owned(),
                        },
                        provider_models::ModelEntry {
                            id: "example-small".to_owned(),
                            name: "Example Small".to_owned(),
                        },
                    ],
                    lead: "example-lead".to_owned(),
                    small: "example-small".to_owned(),
                }),
                None,
                None,
                None,
            ],
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.on_key = Some(selection_menu_key);
        runtime.frame(0.0);
        let detail_before = runtime.find("provider-detail-openai").unwrap();
        let (x, y) = runtime.find("choose-lead-model").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert!(runtime.find("selection-menu-item-1").is_some());
        let (menu_rect, _) = runtime.presented("selection-menu").unwrap();
        assert!(menu_rect[0] >= 0.0 && menu_rect[0] + menu_rect[2] <= 1040.0);
        assert!(menu_rect[1] >= 0.0 && menu_rect[1] + menu_rect[3] <= 720.0);
        assert_eq!(
            runtime.find("provider-detail-openai").unwrap(),
            detail_before
        );
        runtime.key(KeyEvent {
            key: Key::Down,
            mods: Default::default(),
        });
        runtime.frame(0.0);
        assert_eq!(runtime.state.menu_cursor, 1);
        runtime.key(KeyEvent {
            key: Key::Home,
            mods: Default::default(),
        });
        runtime.frame(0.0);
        assert_eq!(runtime.state.menu_cursor, 0);
        runtime.key(KeyEvent {
            key: Key::Escape,
            mods: Default::default(),
        });
        runtime.frame(0.0);
        assert!(runtime.find("selection-menu").is_none());
        let (x, y) = runtime.find("choose-lead-model").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        runtime.pointer_down(500.0, 100.0);
        runtime.pointer_up(500.0, 100.0);
        runtime.frame(0.0);
        assert!(runtime.find("selection-menu").is_none());
    }

    #[test]
    fn saving_one_model_keeps_both_selectors_in_place() {
        let state = State {
            auth_loaded: true,
            auth_status: [
                AuthStatus {
                    subscription: true,
                    ..AuthStatus::default()
                },
                AuthStatus::default(),
            ],
            model_catalogs: [
                Some(ModelCatalog {
                    models: vec![provider_models::ModelEntry {
                        id: "example-model".to_owned(),
                        name: "Example Model".to_owned(),
                    }],
                    lead: "example-model".to_owned(),
                    small: "example-model".to_owned(),
                }),
                None,
                None,
                None,
            ],
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let lead_before = runtime.presented("choose-lead-model").unwrap().0;
        let small_before = runtime.presented("choose-small-model").unwrap().0;

        runtime.update(|state, _| state.model_saving = true);
        runtime.frame(0.0);
        assert_eq!(
            runtime.presented("choose-lead-model").unwrap().0,
            lead_before
        );
        assert_eq!(
            runtime.presented("choose-small-model").unwrap().0,
            small_before
        );

        let (x, y) = runtime.find("choose-small-model").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert!(runtime.find("selection-menu").is_none());
    }

    #[test]
    fn long_model_menu_reveals_selected_item() {
        let models = (0..25)
            .map(|index| provider_models::ModelEntry {
                id: format!("example-model-{index}"),
                name: format!("Example Model {index}"),
            })
            .collect();
        let state = State {
            auth_loaded: true,
            auth_status: [
                AuthStatus {
                    subscription: true,
                    ..AuthStatus::default()
                },
                AuthStatus::default(),
            ],
            model_catalogs: [
                Some(ModelCatalog {
                    models,
                    lead: "example-model-20".to_owned(),
                    small: "example-model-0".to_owned(),
                }),
                None,
                None,
                None,
            ],
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let (x, y) = runtime.find("choose-lead-model").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        for _ in 0..3 {
            runtime.frame(0.0);
        }
        assert_eq!(runtime.state.menu_cursor, 20);
        let (menu_rect, _) = runtime.presented("selection-menu").unwrap();
        let (viewport_rect, _) = runtime.presented("selection-menu-scroll").unwrap();
        let (selected_rect, _) = runtime.presented("selection-menu-item-20").unwrap();
        assert!(menu_rect[3] <= 320.0);
        assert!(selected_rect[1] >= viewport_rect[1]);
        assert!(selected_rect[1] + selected_rect[3] <= viewport_rect[1] + viewport_rect[3]);
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
    fn sidenav_edge_tracks_header_and_resizes_within_bounds() {
        fn aligned(runtime: &Runtime<State>) {
            let (sidenav, _) = runtime.presented("settings-sidenav").unwrap();
            let (divider, _) = runtime.presented("header-divider").unwrap();
            assert!((sidenav[0] + sidenav[2] - divider[0]).abs() < 1.0);
        }

        let mut runtime =
            Runtime::new(State::default(), view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        aligned(&runtime);
        let (x, y) = runtime.find("settings-sidenav-resize").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_move(x + 60.0, y);
        runtime.frame(0.0);
        assert_eq!(runtime.state.settings_nav_width, 352.0);
        aligned(&runtime);
        runtime.pointer_move(x + 300.0, y);
        runtime.frame(0.0);
        assert_eq!(runtime.state.settings_nav_width, SIDENAV_MAX_WIDTH);
        aligned(&runtime);
        runtime.pointer_move(x - 300.0, y);
        runtime.frame(0.0);
        assert_eq!(runtime.state.settings_nav_width, SIDENAV_MIN_WIDTH);
        aligned(&runtime);
        runtime.pointer_up(x - 300.0, y);
        runtime.frame(0.0);

        let (x, y) = runtime.find("header-sidebar-toggle").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        let (menu, _) = runtime.presented("header-menus").unwrap();
        let (divider, _) = runtime.presented("header-divider").unwrap();
        assert!(divider[0] >= menu[0] + menu[2] + 8.0);
        assert!(runtime.find("settings-sidenav-resize").is_none());

        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert_eq!(runtime.state.settings_nav_width, SIDENAV_MIN_WIDTH);
        aligned(&runtime);
    }

    #[test]
    fn settings_sidenav_switches_sections_and_returns_to_providers() {
        let mut runtime =
            Runtime::new(State::default(), view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        assert!(runtime.find("settings-profile").is_none());
        assert!(runtime.find("settings-prompt").is_none());
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
