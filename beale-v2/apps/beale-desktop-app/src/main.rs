use rune::tok::TextStyle;
use rune::{
    Align, AppSpec, Color, Cursor, Cx, Drag, El, Icon, Key, KeyEvent, Phase, ScrollSpec, Side,
    TextEdit, Tx, col, el, icon, row, text, text_input,
};
use std::time::Duration;

mod appearance;
mod color_picker;
mod connection;
mod desktop_fonts;
mod home_sidebar;
mod icons;
mod in_place_menu;
mod provider_auth;
mod provider_icons;
mod provider_models;
mod settings_scroll;
mod workspace_creation;
use appearance::{
    AppearancePreferences, AppearanceTheme, HeatLevel, InterfaceFont, PaletteColor, TextSize,
    ThemeStyle,
};
use beale_contracts::v1::{AuthMethod, DaybreakMode, ProviderId};
use color_picker::Hsv;
use connection::ConnectionStatus;
use icons::*;
use provider_auth::{Action, AuthStatus, SignInPrompt};
use provider_icons::{GROK, OPENAI};
use provider_models::ModelCatalog;
use settings_scroll::SettingsScroll;

const PROVIDER_WARNING_COLOR: &str = "#dca15f";
const PROVIDER_ERROR_COLOR: &str = "#e07676";

// Rune sizes text per element, so keep the selected scale scoped to one synchronous view build.
thread_local! {
    static RENDER_TEXT_SIZE: std::cell::Cell<TextSize> = const {
        std::cell::Cell::new(TextSize::Standard)
    };
}

struct RenderTextSizeScope(TextSize);

impl RenderTextSizeScope {
    fn enter(size: TextSize) -> Self {
        Self(RENDER_TEXT_SIZE.with(|current| current.replace(size)))
    }
}

impl Drop for RenderTextSizeScope {
    fn drop(&mut self) {
        RENDER_TEXT_SIZE.with(|current| current.set(self.0));
    }
}

fn ui_font(size: f32, weight: f32) -> TextStyle {
    let scale = RENDER_TEXT_SIZE.with(|current| current.get().factor());
    TextStyle::new(size * scale, weight)
}

fn form_label_font() -> TextStyle {
    ui_font(14.0, 550.0)
}

#[cfg(target_os = "windows")]
mod window_state;
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

    fn logo(self) -> &'static Icon {
        match self {
            Self::OpenAi => &OPENAI,
            Self::Xai => &GROK,
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
enum ModelRole {
    Lead,
    Small,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SelectionMenu {
    LeadProvider,
    ResearchKit,
    ResourceKind,
    ResourceDirection,
    ColorTheme,
    InterfaceFont,
    TextSize,
    SourcePriority {
        provider: Provider,
    },
    Model {
        provider: Provider,
        method: AuthMethod,
        role: ModelRole,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ColorTarget {
    Heat(HeatLevel),
    Theme(PaletteColor),
}

impl ColorTarget {
    fn anchor(self) -> String {
        match self {
            Self::Heat(level) => format!("heat-hex-{}", level.index()),
            Self::Theme(field) => format!("palette-hex-{}", field.key()),
        }
    }

    fn title(self) -> String {
        match self {
            Self::Heat(level) => format!("{} Heat", level.name()),
            Self::Theme(field) => field.name().to_owned(),
        }
    }
}

struct ColorPickerState {
    target: ColorTarget,
    hsv: Hsv,
    drag_start_hex: String,
}

fn catalog_index(provider: Provider, method: AuthMethod) -> usize {
    provider.index() * 2 + if method == AuthMethod::ApiKey { 1 } else { 0 }
}

fn effective_model_method(status: &AuthStatus) -> AuthMethod {
    if status.source_priority == AuthMethod::ApiKey && status.api_key {
        AuthMethod::ApiKey
    } else if status.subscription {
        AuthMethod::Subscription
    } else if status.api_key {
        AuthMethod::ApiKey
    } else {
        AuthMethod::Subscription
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum SettingsSection {
    General,
    Appearance,
    #[default]
    Providers,
    Archive,
}

impl SettingsSection {
    fn spec(self) -> &'static SettingsSectionSpec {
        SETTINGS_SECTIONS
            .iter()
            .find(|spec| spec.section == self)
            .expect("every settings section has a definition")
    }

    fn label(self) -> &'static str {
        self.spec().label
    }

    fn icon(self) -> &'static Icon {
        self.spec().icon
    }
}

struct SettingsSectionSpec {
    section: SettingsSection,
    key: &'static str,
    label: &'static str,
    icon: &'static Icon,
    render: fn(&State, &mut Cx<State>) -> Vec<El>,
}

const SETTINGS_SECTIONS: [SettingsSectionSpec; 4] = [
    SettingsSectionSpec {
        section: SettingsSection::General,
        key: "settings-general",
        label: "General",
        icon: &SETTINGS,
        render: unavailable_settings_view,
    },
    SettingsSectionSpec {
        section: SettingsSection::Appearance,
        key: "settings-appearance",
        label: "Appearance",
        icon: &APPEARANCE,
        render: appearance_settings_view,
    },
    SettingsSectionSpec {
        section: SettingsSection::Providers,
        key: "settings-providers",
        label: "Providers",
        icon: &PROVIDERS,
        render: providers_settings_view,
    },
    SettingsSectionSpec {
        section: SettingsSection::Archive,
        key: "settings-archive",
        label: "Archive",
        icon: &ARCHIVE,
        render: unavailable_settings_view,
    },
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum AppearanceTab {
    #[default]
    Colors,
    Typography,
}

impl AppearanceTab {
    const ALL: [Self; 2] = [Self::Colors, Self::Typography];

    fn label(self) -> &'static str {
        match self {
            Self::Colors => "Colors",
            Self::Typography => "Typography",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Colors => "appearance-tab-colors",
            Self::Typography => "appearance-tab-typography",
        }
    }
}

#[derive(Clone, Copy)]
struct FormSpec {
    key: &'static str,
    heading: &'static str,
}

const AUTH_FORM: FormSpec = FormSpec {
    key: "provider-authentication",
    heading: "Authentication",
};
const MODEL_FORM: FormSpec = FormSpec {
    key: "provider-model-defaults",
    heading: "Default Models",
};
const DAYBREAK_FORM: FormSpec = FormSpec {
    key: "provider-daybreak-mode",
    heading: "Daybreak Mode",
};
const HEAT_FORM: FormSpec = FormSpec {
    key: "appearance-heat-group",
    heading: "Heat Palette",
};
const THEME_PALETTE_FORM: FormSpec = FormSpec {
    key: "appearance-theme-palette",
    heading: "Theme Palette",
};
const FONT_FORM: FormSpec = FormSpec {
    key: "appearance-font-options",
    heading: "Font Options",
};

struct AuthRowSpec {
    method: AuthMethod,
    label: &'static str,
    description: &'static str,
}

const AUTH_ROWS: [AuthRowSpec; 2] = [
    AuthRowSpec {
        method: AuthMethod::Subscription,
        label: "Subscription",
        description: "Use your provider subscription account.",
    },
    AuthRowSpec {
        method: AuthMethod::ApiKey,
        label: "API Key",
        description: "Store a provider key on this host.",
    },
];

#[derive(Clone, Copy)]
enum ModelFormRow {
    SourcePriority,
    Default(ModelRole),
}

struct ModelRowSpec {
    kind: ModelFormRow,
    label: &'static str,
    description: &'static str,
}

const MODEL_ROWS: [ModelRowSpec; 3] = [
    ModelRowSpec {
        kind: ModelFormRow::SourcePriority,
        label: "Source Priority",
        description: "Preferred source when both are configured.",
    },
    ModelRowSpec {
        kind: ModelFormRow::Default(ModelRole::Lead),
        label: "Lead Model",
        description: "Primary model for complex research work.",
    },
    ModelRowSpec {
        kind: ModelFormRow::Default(ModelRole::Small),
        label: "Small Model",
        description: "Lighter model for supporting research tasks.",
    },
];

struct DaybreakRowSpec {
    mode: DaybreakMode,
    label: &'static str,
    description: &'static str,
}

const DAYBREAK_ROWS: [DaybreakRowSpec; 2] = [
    DaybreakRowSpec {
        mode: DaybreakMode::Blue,
        label: "Daybreak Blue",
        description: "For accounts with Daybreak Blue access.",
    },
    DaybreakRowSpec {
        mode: DaybreakMode::Red,
        label: "Daybreak Red",
        description: "Requires separate Daybreak Red approval.",
    },
];

struct TypographyRowSpec {
    key: &'static str,
    control_key: &'static str,
    label: &'static str,
    description: &'static str,
    menu: SelectionMenu,
}

const TYPOGRAPHY_ROWS: [TypographyRowSpec; 2] = [
    TypographyRowSpec {
        key: "appearance-font-row",
        control_key: "appearance-font-control",
        label: "Interface Font",
        description: "Font used throughout the desktop.",
        menu: SelectionMenu::InterfaceFont,
    },
    TypographyRowSpec {
        key: "appearance-size-row",
        control_key: "appearance-size-control",
        label: "Text Size",
        description: "Scale interface text throughout the desktop.",
        menu: SelectionMenu::TextSize,
    },
];

const NAVIGATION_RAIL_WIDTH: f32 = 50.0;
const SIDENAV_DEFAULT_WIDTH: f32 = 292.0;
const SIDENAV_MIN_WIDTH: f32 = 240.0;
const SIDENAV_MAX_WIDTH: f32 = 420.0;
const HEADER_COLLAPSED_PREFIX_WIDTH: f32 = if cfg!(target_os = "macos") {
    198.0
} else {
    122.0
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PrimaryView {
    Home,
    WorkspaceCreation,
    Settings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ViewLocation {
    Home,
    WorkspaceCreation(workspace_creation::CreationTab),
    Providers(Provider),
    Appearance(AppearanceTab),
    Settings(SettingsSection),
}

#[derive(Default)]
struct ViewHistory {
    back: Vec<ViewLocation>,
    forward: Vec<ViewLocation>,
}

impl ViewHistory {
    const LIMIT: usize = 64;

    fn push(stack: &mut Vec<ViewLocation>, location: ViewLocation) {
        if stack.len() == Self::LIMIT {
            stack.remove(0);
        }
        stack.push(location);
    }
}

struct State {
    primary_view: PrimaryView,
    workspace_draft: workspace_creation::WorkspaceDraft,
    selected: Provider,
    settings_section: SettingsSection,
    appearance_tab: AppearanceTab,
    view_history: ViewHistory,
    appearance: AppearancePreferences,
    appearance_path: Option<std::path::PathBuf>,
    appearance_error: String,
    heat_edits: [TextEdit; 4],
    palette_edits: [TextEdit; 14],
    color_picker: Option<ColorPickerState>,
    connection: ConnectionStatus,
    auth_status: [AuthStatus; 2],
    auth_loaded: bool,
    lead_provider: Option<Provider>,
    lead_busy: bool,
    lead_error: String,
    provider_selection_initialized: bool,
    native_window_border: bool,
    auth_message: String,
    auth_message_is_error: bool,
    sign_in_prompt: Option<SignInPrompt>,
    api_key_edit: TextEdit,
    api_key_dialog: bool,
    auth_busy: bool,
    source_saving: [bool; 2],
    source_errors: [String; 2],
    daybreak_mode: DaybreakMode,
    daybreak_saving: bool,
    daybreak_error: String,
    model_catalogs: [Option<ModelCatalog>; 4],
    model_loading: [bool; 4],
    model_errors: [String; 4],
    model_saving: bool,
    selection_menu: Option<SelectionMenu>,
    menu_cursor: usize,
    menu_reveal_pending: bool,
    notice: Option<&'static str>,
    sidenav_visible: bool,
    sidenav_width: f32,
    settings_scroll: SettingsScroll,
    workspace_scroll: SettingsScroll,
}

impl Default for State {
    fn default() -> Self {
        Self {
            primary_view: PrimaryView::Settings,
            workspace_draft: workspace_creation::WorkspaceDraft::default(),
            selected: Provider::OpenAi,
            settings_section: SettingsSection::Providers,
            appearance_tab: AppearanceTab::Colors,
            view_history: ViewHistory::default(),
            appearance: AppearancePreferences::default(),
            appearance_path: None,
            appearance_error: String::new(),
            heat_edits: std::array::from_fn(|index| {
                TextEdit::new(appearance::HEAT_DEFAULTS[AppearanceTheme::Dark.index()][index])
            }),
            palette_edits: std::array::from_fn(|index| {
                TextEdit::new(
                    AppearancePreferences::default()
                        .palette_color(AppearanceTheme::Dark, PaletteColor::ALL[index]),
                )
            }),
            color_picker: None,
            connection: ConnectionStatus::Connecting,
            auth_status: Default::default(),
            auth_loaded: false,
            lead_provider: None,
            lead_busy: false,
            lead_error: String::new(),
            provider_selection_initialized: false,
            native_window_border: false,
            auth_message: String::new(),
            auth_message_is_error: false,
            sign_in_prompt: None,
            api_key_edit: TextEdit::new("").secure(),
            api_key_dialog: false,
            auth_busy: false,
            source_saving: [false; 2],
            source_errors: std::array::from_fn(|_| String::new()),
            daybreak_mode: DaybreakMode::Blue,
            daybreak_saving: false,
            daybreak_error: String::new(),
            model_catalogs: std::array::from_fn(|_| None),
            model_loading: [false; 4],
            model_errors: std::array::from_fn(|_| String::new()),
            model_saving: false,
            selection_menu: None,
            menu_cursor: 0,
            menu_reveal_pending: false,
            notice: None,
            sidenav_visible: true,
            sidenav_width: SIDENAV_DEFAULT_WIDTH,
            settings_scroll: SettingsScroll::default(),
            workspace_scroll: SettingsScroll::for_key("workspace-scroll"),
        }
    }
}

impl State {
    fn load_appearance() -> Self {
        let mut state = Self {
            appearance_path: appearance::preference_path(),
            ..Self::default()
        };
        if let Some(path) = &state.appearance_path {
            state.appearance = AppearancePreferences::read(path);
        }
        state.sync_color_edits();
        state
    }

    fn view_location(&self) -> ViewLocation {
        if self.primary_view == PrimaryView::Home {
            return ViewLocation::Home;
        }
        if self.primary_view == PrimaryView::WorkspaceCreation {
            return ViewLocation::WorkspaceCreation(self.workspace_draft.tab);
        }
        match self.settings_section {
            SettingsSection::Providers => ViewLocation::Providers(self.selected),
            SettingsSection::Appearance => ViewLocation::Appearance(self.appearance_tab),
            section => ViewLocation::Settings(section),
        }
    }

    fn apply_view_location(&mut self, location: ViewLocation) {
        match location {
            ViewLocation::Home => self.primary_view = PrimaryView::Home,
            ViewLocation::WorkspaceCreation(tab) => {
                self.primary_view = PrimaryView::WorkspaceCreation;
                self.workspace_draft.tab = tab;
            }
            ViewLocation::Providers(provider) => {
                self.primary_view = PrimaryView::Settings;
                self.settings_section = SettingsSection::Providers;
                self.selected = provider;
            }
            ViewLocation::Appearance(tab) => {
                self.primary_view = PrimaryView::Settings;
                self.settings_section = SettingsSection::Appearance;
                self.appearance_tab = tab;
            }
            ViewLocation::Settings(section) => {
                self.primary_view = PrimaryView::Settings;
                self.settings_section = section;
            }
        }
        self.notice = None;
        self.selection_menu = None;
        self.color_picker = None;
        self.sign_in_prompt = None;
        self.api_key_dialog = false;
        self.api_key_edit.clear();
        self.auth_message.clear();
    }

    fn navigate_to(&mut self, location: ViewLocation) {
        let current = self.view_location();
        if current != location {
            ViewHistory::push(&mut self.view_history.back, current);
            self.view_history.forward.clear();
        }
        self.apply_view_location(location);
    }

    fn show_section(&mut self, section: SettingsSection) {
        let location = match section {
            SettingsSection::Providers => ViewLocation::Providers(self.selected),
            SettingsSection::Appearance => ViewLocation::Appearance(self.appearance_tab),
            section => ViewLocation::Settings(section),
        };
        self.navigate_to(location);
    }

    fn start_workspace_creation(&mut self) {
        self.workspace_draft = workspace_creation::WorkspaceDraft::default();
        self.workspace_scroll = SettingsScroll::for_key("workspace-scroll");
        self.navigate_to(ViewLocation::WorkspaceCreation(
            workspace_creation::CreationTab::Settings,
        ));
    }

    fn cancel_workspace_creation(&mut self) {
        self.workspace_draft = workspace_creation::WorkspaceDraft::default();
        self.view_history
            .back
            .retain(|location| !matches!(location, ViewLocation::WorkspaceCreation(_)));
        self.view_history
            .forward
            .retain(|location| !matches!(location, ViewLocation::WorkspaceCreation(_)));
        if self.view_history.back.last() == Some(&ViewLocation::Home) {
            self.view_history.back.pop();
        }
        self.apply_view_location(ViewLocation::Home);
    }

    fn go_back(&mut self) {
        if let Some(location) = self.view_history.back.pop() {
            let current = self.view_location();
            ViewHistory::push(&mut self.view_history.forward, current);
            self.apply_view_location(location);
        }
    }

    fn go_forward(&mut self) {
        if let Some(location) = self.view_history.forward.pop() {
            let current = self.view_location();
            ViewHistory::push(&mut self.view_history.back, current);
            self.apply_view_location(location);
        }
    }

    fn sync_heat_edits(&mut self) {
        self.heat_edits = std::array::from_fn(|index| {
            TextEdit::new(
                self.appearance
                    .heat_color(self.appearance.theme, HeatLevel::ALL[index]),
            )
        });
    }

    fn sync_palette_edits(&mut self) {
        self.palette_edits = std::array::from_fn(|index| {
            TextEdit::new(
                self.appearance
                    .palette_color(self.appearance.theme, PaletteColor::ALL[index]),
            )
        });
    }

    fn sync_color_edits(&mut self) {
        self.sync_heat_edits();
        self.sync_palette_edits();
    }

    fn color_value(&self, target: ColorTarget) -> &str {
        match target {
            ColorTarget::Heat(level) => self.appearance.heat_color(self.appearance.theme, level),
            ColorTarget::Theme(field) => {
                self.appearance.palette_color(self.appearance.theme, field)
            }
        }
    }

    fn set_color_value(&mut self, target: ColorTarget, hex: String) {
        match target {
            ColorTarget::Heat(level) => {
                self.appearance
                    .set_heat_color(self.appearance.theme, level, Some(hex.clone()));
                self.heat_edits[level.index()].text = hex;
            }
            ColorTarget::Theme(field) => {
                self.appearance
                    .set_palette_color(self.appearance.theme, field, Some(hex.clone()));
                self.palette_edits[field.index()].text = hex;
                #[cfg(target_os = "windows")]
                if field == PaletteColor::Border {
                    with_windows_window(|hwnd| {
                        windows_frame::set_border_color(hwnd, &self.appearance);
                    });
                }
            }
        }
    }

    fn save_appearance(&mut self) {
        self.appearance_error = self
            .appearance_path
            .as_ref()
            .and_then(|path| self.appearance.write(path).err())
            .map_or_else(String::new, |_| {
                "Could not save appearance settings on this device.".to_owned()
            });
    }
}

fn toggle_color_picker(state: &mut State, target: ColorTarget) {
    if state
        .color_picker
        .as_ref()
        .is_some_and(|picker| picker.target == target)
    {
        state.color_picker = None;
        return;
    }
    let hex = state.color_value(target).to_owned();
    if let Some(hsv) = Hsv::from_hex(&hex) {
        state.color_picker = Some(ColorPickerState {
            target,
            hsv,
            drag_start_hex: hex,
        });
        state.selection_menu = None;
    }
}

#[derive(Clone, Copy)]
enum PickerAxis {
    Area,
    Hue,
}

fn drag_color_picker(state: &mut State, drag: &mut Drag, tx: &mut Tx, axis: PickerAxis) {
    let Some(target) = state.color_picker.as_ref().map(|picker| picker.target) else {
        return;
    };
    tx.instant();
    if drag.phase == Phase::Down {
        let original = state.color_value(target).to_owned();
        if let Some(picker) = &mut state.color_picker {
            picker.drag_start_hex = original;
        }
    }
    if drag.phase == Phase::Up && drag.cancelled() {
        let original = state.color_picker.as_ref().unwrap().drag_start_hex.clone();
        if let Some(picker) = &mut state.color_picker {
            picker.hsv = Hsv::from_hex(&original).unwrap();
        }
        state.set_color_value(target, original);
        return;
    }
    let hsv = {
        let picker = state.color_picker.as_mut().unwrap();
        match axis {
            PickerAxis::Area => {
                picker.hsv.saturation = (drag.x / drag.w).clamp(0.0, 1.0);
                picker.hsv.value = (1.0 - drag.y / drag.h).clamp(0.0, 1.0);
            }
            PickerAxis::Hue => picker.hsv.hue = (drag.x / drag.w).clamp(0.0, 0.999_999),
        }
        picker.hsv
    };
    state.set_color_value(target, hsv.hex());
    if drag.phase == Phase::Up {
        state.save_appearance();
    }
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
    state.auth_message_is_error = false;
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
                        state.auth_message_is_error = true;
                    } else {
                        state.auth_message = "Finish sign-in in your browser.".to_owned();
                        state.auth_message_is_error = false;
                    }
                    state.sign_in_prompt = Some(prompt);
                }
                Ok(None) => {
                    state.auth_message = "Credential setting saved on the app-server.".to_owned();
                    state.auth_message_is_error = false;
                }
                Err(error) => {
                    state.auth_message = error;
                    state.auth_message_is_error = true;
                }
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

fn start_source_save(state: &mut State, provider: Provider, method: AuthMethod) {
    let index = provider.index();
    let auth = &state.auth_status[index];
    if state.source_saving[index]
        || !state.auth_loaded
        || !auth.subscription
        || !auth.api_key
        || !matches!(method, AuthMethod::Subscription | AuthMethod::ApiKey)
    {
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        return;
    };
    state.selection_menu = None;
    state.source_saving[index] = true;
    state.source_errors[index].clear();
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
            runtime.block_on(provider_auth::set_source_priority(
                &client,
                addr,
                provider.id(),
                method,
            ))
        })();
        handle.update(move |state, _| {
            state.source_saving[index] = false;
            match result {
                Ok(()) => state.auth_status[index].source_priority = method,
                Err(error) => state.source_errors[index] = error,
            }
        });
    });
}

fn start_daybreak_save(state: &mut State, mode: DaybreakMode) {
    if state.daybreak_saving
        || !state.auth_loaded
        || !matches!(state.connection, ConnectionStatus::Connected { .. })
        || !matches!(mode, DaybreakMode::Blue | DaybreakMode::Red)
        || state.daybreak_mode == mode
    {
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        return;
    };
    state.daybreak_saving = true;
    state.daybreak_error.clear();
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
            runtime.block_on(provider_auth::set_daybreak_mode(&client, addr, mode))
        })();
        handle.update(move |state, _| {
            state.daybreak_saving = false;
            match result {
                Ok(()) => state.daybreak_mode = mode,
                Err(error) => state.daybreak_error = error,
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
        SelectionMenu::ResearchKit => vec![in_place_menu::MenuItem {
            value: "none".to_owned(),
            label: "None".to_owned(),
            detail: None,
            selected: true,
        }],
        SelectionMenu::ResourceKind => workspace_creation::ResourceKind::ALL
            .into_iter()
            .map(|kind| in_place_menu::MenuItem {
                value: kind.key().to_owned(),
                label: kind.label().to_owned(),
                detail: None,
                selected: state.workspace_draft.resource_kind == kind,
            })
            .collect(),
        SelectionMenu::ResourceDirection => workspace_creation::ResourceDirection::ALL
            .into_iter()
            .map(|direction| in_place_menu::MenuItem {
                value: direction.key().to_owned(),
                label: direction.label().to_owned(),
                detail: None,
                selected: state.workspace_draft.resource_direction == direction,
            })
            .collect(),
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
        SelectionMenu::ColorTheme => AppearanceTheme::ALL
            .into_iter()
            .map(|theme| in_place_menu::MenuItem {
                value: theme.name().to_ascii_lowercase(),
                label: theme.name().to_owned(),
                detail: None,
                selected: state.appearance.theme == theme,
            })
            .collect(),
        SelectionMenu::InterfaceFont => InterfaceFont::ALL
            .into_iter()
            .map(|font| in_place_menu::MenuItem {
                value: font.name().to_ascii_lowercase(),
                label: font.name().to_owned(),
                detail: None,
                selected: state.appearance.interface_font == font,
            })
            .collect(),
        SelectionMenu::TextSize => TextSize::ALL
            .into_iter()
            .map(|size| in_place_menu::MenuItem {
                value: size.key().to_owned(),
                label: size.name().to_owned(),
                detail: None,
                selected: state.appearance.text_size == size,
            })
            .collect(),
        SelectionMenu::SourcePriority { provider } => {
            let status = &state.auth_status[provider.index()];
            if state.selected != provider || !status.subscription || !status.api_key {
                return Vec::new();
            }
            [
                (AuthMethod::Subscription, "subscription", "Subscription"),
                (AuthMethod::ApiKey, "api-key", "API Key"),
            ]
            .into_iter()
            .map(|(method, value, label)| in_place_menu::MenuItem {
                value: value.to_owned(),
                label: label.to_owned(),
                detail: None,
                selected: status.source_priority == method,
            })
            .collect()
        }
        SelectionMenu::Model {
            provider,
            method,
            role,
        } => {
            if state.selected != provider
                || effective_model_method(&state.auth_status[provider.index()]) != method
            {
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
        SelectionMenu::ResearchKit => "workspace-kit-control",
        SelectionMenu::ResourceKind => "workspace-resource-kind-control",
        SelectionMenu::ResourceDirection => "workspace-resource-direction-control",
        SelectionMenu::LeadProvider => "lead-provider-control",
        SelectionMenu::ColorTheme => "color-theme-control",
        SelectionMenu::InterfaceFont => "appearance-font-control",
        SelectionMenu::TextSize => "appearance-size-control",
        SelectionMenu::SourcePriority { .. } => "source-priority-control",
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
    if let SelectionMenu::SourcePriority { provider } = menu
        && state.source_saving[provider.index()]
    {
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
    state.color_picker = None;
}

fn choose_selection_menu_item(state: &mut State, menu: SelectionMenu, value: String, tx: &mut Tx) {
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
        SelectionMenu::ResearchKit => {}
        SelectionMenu::ResourceKind => {
            if let Some(kind) = workspace_creation::ResourceKind::ALL
                .into_iter()
                .find(|kind| kind.key() == value)
            {
                state.workspace_draft.resource_kind = kind;
            }
        }
        SelectionMenu::ResourceDirection => {
            if let Some(direction) = workspace_creation::ResourceDirection::ALL
                .into_iter()
                .find(|direction| direction.key() == value)
            {
                state.workspace_draft.resource_direction = direction;
            }
        }
        SelectionMenu::LeadProvider => {
            let provider = if value == "openai" {
                Provider::OpenAi
            } else {
                Provider::Xai
            };
            start_lead_save(state, provider);
        }
        SelectionMenu::ColorTheme => {
            if let Some(theme) = AppearanceTheme::ALL
                .into_iter()
                .find(|theme| theme.name().eq_ignore_ascii_case(&value))
            {
                state.appearance.theme = theme;
                state.sync_color_edits();
                state.save_appearance();
                #[cfg(target_os = "windows")]
                with_windows_window(|hwnd| {
                    windows_frame::set_border_color(hwnd, &state.appearance);
                });
            }
        }
        SelectionMenu::InterfaceFont => {
            if let Some(font) = InterfaceFont::ALL
                .into_iter()
                .find(|font| font.name().eq_ignore_ascii_case(&value))
            {
                state.appearance.interface_font = font;
                state.save_appearance();
                let (sans, mono) = desktop_fonts::families(font);
                tx.set_font_families(sans, mono);
            }
        }
        SelectionMenu::TextSize => {
            if let Some(size) = TextSize::ALL.into_iter().find(|size| size.key() == value) {
                state.appearance.text_size = size;
                state.save_appearance();
            }
        }
        SelectionMenu::SourcePriority { provider } => {
            let method = if value == "api-key" {
                AuthMethod::ApiKey
            } else {
                AuthMethod::Subscription
            };
            start_source_save(state, provider, method);
        }
        SelectionMenu::Model {
            provider,
            method,
            role,
        } => start_model_save(state, provider, method, role, value),
    }
}

fn selection_menu_key(state: &mut State, event: &KeyEvent, tx: &mut Tx) -> bool {
    if state.color_picker.is_some() && matches!(event.key, Key::Escape) {
        state.color_picker = None;
        return true;
    }
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
            choose_selection_menu_item(state, menu, items[index].value.clone(), tx);
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

fn header_icon_button(
    key: &'static str,
    label: &'static str,
    glyph: &'static Icon,
    theme: ThemeStyle<'_>,
) -> El {
    let color = |value| theme.color(value);
    el().key(key)
        .role("button")
        .label(label)
        .size(34.0, 34.0)
        .center()
        .radius(6.0)
        .hover(color("#303030"))
        .child(icon(glyph, 16.0).color(color("#c8c8c8")))
}

#[derive(Clone, Copy)]
enum HistoryDirection {
    Back,
    Forward,
}

fn history_button(direction: HistoryDirection, enabled: bool, theme: ThemeStyle<'_>) -> El {
    let color = |value| theme.color(value);
    let (key, label, glyph) = match direction {
        HistoryDirection::Back => ("header-back", "Back", &ARROW_LEFT),
        HistoryDirection::Forward => ("header-forward", "Forward", &ARROW_RIGHT),
    };
    let button = el()
        .key(key)
        .role("button")
        .label(if enabled {
            label.to_owned()
        } else {
            format!("{label} unavailable")
        })
        .size(30.0, 34.0)
        .center()
        .radius(6.0)
        .child(icon(glyph, 16.0).color(color(if enabled { "#c8c8c8" } else { "#777777" })));
    if enabled {
        button
            .cursor(Cursor::Pointer)
            .hover(color("#303030"))
            .on_press(move |state: &mut State| match direction {
                HistoryDirection::Back => state.go_back(),
                HistoryDirection::Forward => state.go_forward(),
            })
    } else {
        button
    }
}

fn window_control(
    key: &'static str,
    label: &'static str,
    glyph: &'static Icon,
    theme: ThemeStyle<'_>,
) -> El {
    let color = |value| theme.color(value);
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
    let theme = state.appearance.style();
    let color = |value| theme.color(value);
    let mut left = row()
        .key("header-leading")
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
            if state.sidenav_visible {
                "Hide sidebar"
            } else {
                "Show sidebar"
            },
            &SIDEBAR,
            theme,
        )
        .on_press(|s: &mut State| {
            s.sidenav_visible = !s.sidenav_visible;
        }),
    );
    left = left
        .child(history_button(
            HistoryDirection::Back,
            !state.view_history.back.is_empty(),
            theme,
        ))
        .child(history_button(
            HistoryDirection::Forward,
            !state.view_history.forward.is_empty(),
            theme,
        ));
    let mut title = row()
        .key("header-title")
        .grow(1.0)
        .h_full()
        .center_y()
        .gap(11.0)
        .window_drag();
    if state.primary_view == PrimaryView::Settings {
        title = title
            .child(
                el().key("header-divider")
                    .size(1.0, 18.0)
                    .fill(color("#555555")),
            )
            .child(icon(state.settings_section.icon(), 16.0).color(color("#c8c8c8")))
            .child(
                text("Agent Settings")
                    .font(ui_font(13.0, 500.0))
                    .color(color("#e2e2e2")),
            )
            .child(
                text(state.settings_section.label())
                    .font(ui_font(13.0, 400.0))
                    .color(color("#aaaaaa")),
            );
    } else if state.primary_view == PrimaryView::WorkspaceCreation {
        title = title
            .child(
                el().key("header-divider")
                    .size(1.0, 18.0)
                    .fill(color("#555555")),
            )
            .child(icon(&HOME, 16.0).color(color("#c8c8c8")))
            .child(
                text("New Workspace")
                    .font(ui_font(13.0, 500.0))
                    .color(color("#e2e2e2")),
            )
            .child(
                text(state.workspace_draft.tab.label())
                    .font(ui_font(13.0, 400.0))
                    .color(color("#aaaaaa")),
            );
    }
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
                .child(window_control(
                    "header-minimize",
                    "Minimize",
                    &MINIMIZE,
                    theme,
                ))
                .child(window_control(
                    "header-maximize",
                    "Maximize",
                    &MAXIMIZE,
                    theme,
                ))
                .child(window_control("header-close", "Close", &CLOSE, theme)),
        );
    }
    bar
}

fn nav_button(
    key: &'static str,
    label: &'static str,
    glyph: &'static Icon,
    active: bool,
    theme: ThemeStyle<'_>,
) -> El {
    let color = |value| theme.color(value);
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
    if key == "nav-home" {
        button.on_press(|s: &mut State| s.navigate_to(ViewLocation::Home))
    } else if key == "nav-settings" {
        button.on_press(|s: &mut State| s.show_section(s.settings_section))
    } else if active {
        button.on_press(|s: &mut State| {
            s.notice = None;
        })
    } else {
        button.on_press(move |s: &mut State| {
            s.notice = Some(label);
        })
    }
}

fn rail(state: &State, theme: ThemeStyle<'_>) -> El {
    col()
        .key("navigation")
        .role("navigation")
        .label("Main navigation")
        .w(NAVIGATION_RAIL_WIDTH)
        .h_full()
        .pt(16.0)
        .gap(6.0)
        .items_center()
        .child(nav_button(
            "nav-home",
            "Home",
            &HOME,
            state.primary_view != PrimaryView::Settings,
            theme,
        ))
        .child(nav_button(
            "nav-automations",
            "Automations",
            &AUTOMATIONS,
            false,
            theme,
        ))
        .child(nav_button("nav-fleet", "Fleet", &FLEET, false, theme))
        .child(nav_button("nav-plugins", "Plugins", &PLUGINS, false, theme))
        .child(nav_button(
            "nav-settings",
            "Agent Settings",
            &SETTINGS,
            state.primary_view == PrimaryView::Settings,
            theme,
        ))
}

fn settings_nav_button(spec: &SettingsSectionSpec, active: bool, theme: ThemeStyle<'_>) -> El {
    let color = |value| theme.color(value);
    let section = spec.section;
    row()
        .key(spec.key)
        .role("button")
        .label(format!("{} settings", spec.label))
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
            s.show_section(section);
        })
        .child(icon(spec.icon, 16.0).color(if active {
            color("#f2f2f2")
        } else {
            color("#bdbdbd")
        }))
        .child(
            text(spec.label)
                .font(ui_font(13.0, if active { 550.0 } else { 400.0 }))
                .color(if active {
                    color("#f2f2f2")
                } else {
                    color("#c8c8c8")
                }),
        )
}

fn settings_sidenav(
    active: SettingsSection,
    connection: &ConnectionStatus,
    width: f32,
    theme: ThemeStyle<'_>,
) -> El {
    let color = |value| theme.color(value);
    let mut sections = col().w_full().gap(2.0);
    for spec in &SETTINGS_SECTIONS {
        sections = sections.child(settings_nav_button(spec, spec.section == active, theme));
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
                .font(ui_font(18.0, 500.0))
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
                        .font(ui_font(10.5, 600.0))
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
                                .font(ui_font(12.0, 400.0))
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
                Phase::Down => drag.data = state.sidenav_width,
                Phase::Move => {
                    state.sidenav_width = (drag.data + drag.dx)
                        .clamp(SIDENAV_MIN_WIDTH, SIDENAV_MAX_WIDTH)
                        .round();
                }
                Phase::Up if drag.cancelled() => state.sidenav_width = drag.data,
                Phase::Up => {}
            }
        })
}

fn provider_tab(provider: Provider, selected: bool, theme: ThemeStyle<'_>) -> El {
    let color = |value| theme.color(value);
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
        .h(26.0)
        .px(10.0)
        .center_y()
        .radius(9.0)
        .continuous_corners()
        .fill(if selected {
            color("#414141")
        } else {
            color("#242424")
        })
        .hover(color("#4a4a4a"))
        .on_press(move |s: &mut State| {
            s.navigate_to(ViewLocation::Providers(provider));
        })
        .child(
            text(name)
                .font(ui_font(14.0, if selected { 600.0 } else { 450.0 }))
                .color(if selected {
                    color("#f2f2f2")
                } else {
                    color("#c8c8c8")
                }),
        )
}

fn appearance_tab_button(tab: AppearanceTab, selected: bool, theme: ThemeStyle<'_>) -> El {
    let color = |value| theme.color(value);
    row()
        .key(tab.key())
        .role("tab")
        .label(format!(
            "{} appearance view{}",
            tab.label(),
            if selected { ", selected" } else { "" }
        ))
        .h(26.0)
        .px(10.0)
        .center_y()
        .radius(9.0)
        .continuous_corners()
        .fill(if selected {
            color("#414141")
        } else {
            color("#242424")
        })
        .hover(color("#4a4a4a"))
        .on_press(move |s: &mut State| {
            s.navigate_to(ViewLocation::Appearance(tab));
        })
        .child(
            text(tab.label())
                .font(ui_font(14.0, if selected { 600.0 } else { 450.0 }))
                .color(if selected {
                    color("#f2f2f2")
                } else {
                    color("#c8c8c8")
                }),
        )
}

fn lead_provider_controls(state: &State) -> El {
    let theme = state.appearance.style();
    let color = |value| theme.color(value);
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
            text("Default Provider")
                .font(ui_font(14.0, 500.0))
                .color(color("#a8a8a8")),
        )
        .child(if state.auth_loaded && configured && !state.lead_busy {
            selection_button(
                label,
                state.selection_menu == Some(SelectionMenu::LeadProvider),
                |s| toggle_selection_menu(s, SelectionMenu::LeadProvider),
                theme,
            )
            .key("lead-provider-control")
        } else {
            el().child(
                text(if state.lead_busy { "Saving" } else { label })
                    .font(ui_font(13.5, 400.0))
                    .color(color("#888888")),
            )
        })
}

fn color_theme_controls(state: &State) -> El {
    let theme = state.appearance.style();
    let color = |value| theme.color(value);
    row()
        .key("color-theme-selector")
        .center_y()
        .gap(7.0)
        .child(
            text("Color Theme")
                .font(ui_font(14.0, 500.0))
                .color(color("#a8a8a8")),
        )
        .child(
            selection_button(
                theme.name(),
                state.selection_menu == Some(SelectionMenu::ColorTheme),
                |s| toggle_selection_menu(s, SelectionMenu::ColorTheme),
                theme,
            )
            .key("color-theme-control"),
        )
}

fn form_button(label: impl Into<String>, theme: ThemeStyle<'_>) -> El {
    let color = |value| theme.color(value);
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
                .font(ui_font(14.0, 550.0))
                .min_w(0.0)
                .truncate()
                .color(color("#eeeeee")),
        )
}

fn credential_button(
    label: &'static str,
    action: impl Fn(&mut State) + 'static,
    theme: ThemeStyle<'_>,
) -> El {
    form_button(label, theme).on_press(action)
}

fn selection_button(
    label: impl Into<String>,
    open: bool,
    action: impl Fn(&mut State) + 'static,
    theme: ThemeStyle<'_>,
) -> El {
    let color = |value| theme.color(value);
    form_button(label, theme)
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
    spec: &AuthRowSpec,
    theme: ThemeStyle<'_>,
) -> El {
    let color = |value| theme.color(value);
    let subscription = spec.method == AuthMethod::Subscription;
    let configured = if subscription {
        status.subscription
    } else {
        status.api_key
    };
    let status_label = if !loaded {
        "Unavailable"
    } else if subscription && status.in_progress {
        "Signing in"
    } else if configured {
        "Configured"
    } else {
        "Not Configured"
    };
    let action = if subscription {
        if configured {
            credential_button(
                "Forget",
                move |s| start_auth_action(s, Action::ForgetSubscription(provider.id())),
                theme,
            )
        } else {
            credential_button(
                "Sign in",
                move |s| start_auth_action(s, Action::StartSubscription(provider.id())),
                theme,
            )
        }
    } else if configured {
        credential_button(
            "Forget",
            move |s| start_auth_action(s, Action::RemoveApiKey(provider.id())),
            theme,
        )
    } else {
        credential_button(
            "Configure",
            move |s| {
                if !s.auth_busy && matches!(s.connection, ConnectionStatus::Connected { .. }) {
                    s.api_key_edit.clear();
                    s.api_key_dialog = true;
                    s.auth_message.clear();
                }
            },
            theme,
        )
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
                            text(spec.label)
                                .font(form_label_font())
                                .color(color("#e2e2e2")),
                        )
                        .child(text(status_label).font(ui_font(14.0, 400.0)).color(
                            if configured {
                                color("#72b783")
                            } else {
                                color("#929292")
                            },
                        )),
                )
                .child(
                    text(spec.description)
                        .font(ui_font(14.0, 400.0))
                        .color(color("#969696")),
                ),
        )
        .child(if !loaded || busy || (subscription && status.in_progress) {
            el().child(
                text(if loaded { "Please wait" } else { "Unavailable" })
                    .font(ui_font(13.5, 400.0))
                    .color(color("#777777")),
            )
        } else {
            action
        })
}

fn settings_form(spec: FormSpec, rows: impl IntoIterator<Item = El>, theme: ThemeStyle<'_>) -> El {
    let color = |value| theme.color(value);
    let mut items = col().w_full().px(16.0).radius(10.0).fill(color("#191919"));
    for (index, item) in rows.into_iter().enumerate() {
        if index > 0 {
            items = items.child(el().w_full().h(1.0).fill(color("#2c2c2c")));
        }
        items = items.child(item);
    }
    col()
        .key(spec.key)
        .w_full()
        .gap(12.0)
        .child(
            text(spec.heading)
                .font(ui_font(17.0, 600.0))
                .color(color("#f2f2f2")),
        )
        .child(items)
}

fn source_priority_row(state: &State, provider: Provider, spec: &ModelRowSpec) -> El {
    let theme = state.appearance.style();
    let color = |value| theme.color(value);
    let status = &state.auth_status[provider.index()];
    let menu = SelectionMenu::SourcePriority { provider };
    form_item_row()
        .child(
            col()
                .grow(1.0)
                .gap(4.0)
                .child(
                    text(spec.label)
                        .font(form_label_font())
                        .color(color("#e2e2e2")),
                )
                .child(
                    text(spec.description)
                        .font(ui_font(14.0, 400.0))
                        .color(color("#969696")),
                ),
        )
        .child(
            selection_button(
                if status.source_priority == AuthMethod::ApiKey {
                    "API Key"
                } else {
                    "Subscription"
                },
                state.selection_menu == Some(menu),
                move |s| toggle_selection_menu(s, menu),
                theme,
            )
            .key("source-priority-control"),
        )
}

fn model_default_row(
    state: &State,
    provider: Provider,
    method: AuthMethod,
    role: ModelRole,
    spec: &ModelRowSpec,
) -> El {
    let theme = state.appearance.style();
    let color = |value| theme.color(value);
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
                    text(spec.label)
                        .font(form_label_font())
                        .color(color("#e2e2e2")),
                )
                .child(
                    text(spec.description)
                        .font(ui_font(14.0, 400.0))
                        .color(color("#969696")),
                ),
        )
        .child(if catalog.is_some() {
            selection_button(
                value.to_owned(),
                state.selection_menu == Some(menu),
                move |s| toggle_selection_menu(s, menu),
                theme,
            )
            .key(match role {
                ModelRole::Lead => "choose-lead-model",
                ModelRole::Small => "choose-small-model",
            })
        } else {
            text(value.to_owned())
                .font(ui_font(13.5, 450.0))
                .color(color("#bcbcbc"))
        })
}

fn model_settings(state: &State, provider: Provider) -> El {
    let theme = state.appearance.style();
    let color = |value| theme.color(value);
    let auth = &state.auth_status[provider.index()];
    let method = effective_model_method(auth);
    let index = catalog_index(provider, method);
    let configured = match method {
        AuthMethod::Subscription => auth.subscription,
        AuthMethod::ApiKey => auth.api_key,
        _ => false,
    };
    let rows = MODEL_ROWS.iter().filter_map(|spec| match spec.kind {
        ModelFormRow::SourcePriority if auth.subscription && auth.api_key => {
            Some(source_priority_row(state, provider, spec))
        }
        ModelFormRow::SourcePriority => None,
        ModelFormRow::Default(role) => Some(model_default_row(state, provider, method, role, spec)),
    });
    let mut section = col()
        .w_full()
        .gap(12.0)
        .child(settings_form(MODEL_FORM, rows, theme));
    if !state.source_errors[provider.index()].is_empty() {
        section = section.child(
            text(state.source_errors[provider.index()].clone())
                .font(ui_font(14.0, 400.0))
                .color(color(PROVIDER_ERROR_COLOR)),
        );
    }
    if !state.model_errors[index].is_empty() {
        section = section.child(
            text(state.model_errors[index].clone())
                .font(ui_font(14.0, 400.0))
                .color(color(PROVIDER_ERROR_COLOR)),
        );
    } else if !configured {
        section = section.child(
            text("Connect this sign-in method to load its models.")
                .font(ui_font(14.0, 400.0))
                .color(color(PROVIDER_WARNING_COLOR)),
        );
    }
    section
}

fn daybreak_option(state: &State, spec: &DaybreakRowSpec) -> El {
    let theme = state.appearance.style();
    let color = |value| theme.color(value);
    let mode = spec.mode;
    let selected = state.daybreak_mode == mode;
    form_item_row()
        .key(if mode == DaybreakMode::Blue {
            "daybreak-blue"
        } else {
            "daybreak-red"
        })
        .role("radio")
        .label(spec.label)
        .help(spec.description)
        .cursor(Cursor::Pointer)
        .child(
            col()
                .grow(1.0)
                .gap(4.0)
                .child(
                    text(spec.label)
                        .font(form_label_font())
                        .color(color("#e2e2e2")),
                )
                .child(
                    text(spec.description)
                        .font(ui_font(14.0, 400.0))
                        .color(color("#969696")),
                ),
        )
        .child(
            el().size(18.0, 18.0)
                .radius(9.0)
                .center()
                .border(1.0, color(if selected { "#c8c8c8" } else { "#555555" }))
                .child(if selected {
                    el().size(8.0, 8.0).radius(4.0).fill(color("#e2e2e2"))
                } else {
                    el()
                }),
        )
        .on_press(move |s: &mut State| start_daybreak_save(s, mode))
}

fn daybreak_settings(state: &State) -> El {
    let theme = state.appearance.style();
    let color = |value| theme.color(value);
    let mut section = col().w_full().gap(12.0).child(settings_form(
        DAYBREAK_FORM,
        DAYBREAK_ROWS
            .iter()
            .map(|spec| daybreak_option(state, spec)),
        theme,
    ));
    if !state.daybreak_error.is_empty() {
        section = section.child(
            text(state.daybreak_error.clone())
                .font(ui_font(14.0, 400.0))
                .color(color(PROVIDER_ERROR_COLOR)),
        );
    }
    section
}

fn provider_detail(state: &State) -> El {
    let theme = state.appearance.style();
    let color = |value| theme.color(value);
    let provider = state.selected;
    let auth = &state.auth_status[provider.index()];
    let name = provider.name();
    let method = effective_model_method(auth);
    let configured = auth.subscription || auth.api_key;
    let mut title_row = row()
        .w_full()
        .center_y()
        .gap(9.0)
        .child(
            el().key("provider-brand-logo")
                .size(32.0, 32.0)
                .center()
                .child(
                    icon(
                        provider.logo(),
                        if provider == Provider::Xai {
                            32.0
                        } else {
                            26.0
                        },
                    )
                    .color(color("#f2f2f2")),
                ),
        )
        .child(
            text(name)
                .font(ui_font(23.0, 600.0))
                .color(color("#f2f2f2")),
        );
    if configured {
        title_row = title_row.child(el().grow(1.0)).child(
            credential_button(
                "Refresh",
                move |s| {
                    let index = catalog_index(provider, method);
                    if !s.model_loading[index] {
                        s.model_catalogs[index] = None;
                        start_model_list(s, provider, method);
                    }
                },
                theme,
            )
            .key("refresh-provider-models"),
        );
    }
    let mut detail = col()
        .key(match provider {
            Provider::OpenAi => "provider-detail-openai",
            Provider::Xai => "provider-detail-xai",
        })
        .role("tabpanel")
        .label(format!("{name} provider settings"))
        .w_full()
        .gap(22.0)
        .child(
            col()
                .key("provider-view-header")
                .w_full()
                .gap(7.0)
                .padding(14.0)
                .radius(16.0)
                .continuous_corners()
                .border(1.0, color("#303030"))
                .child(title_row)
                .child(
                    text(provider.description())
                        .w_full()
                        .min_w(0.0)
                        .wrap()
                        .font(ui_font(14.5, 400.0))
                        .color(color("#969696")),
                ),
        )
        .child(settings_form(
            AUTH_FORM,
            AUTH_ROWS.iter().map(|spec| {
                authentication_row(
                    provider,
                    auth,
                    state.auth_loaded,
                    state.auth_busy,
                    spec,
                    theme,
                )
            }),
            theme,
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
                        .font(ui_font(15.0, 550.0))
                        .color(color("#eeeeee")),
                )
                .child(
                    text_input(&state.api_key_edit, ui_font(15.0, 400.0))
                        .key("api-key-input")
                        .single_line()
                        .secure()
                        .w_full()
                        .h(38.0)
                        .padding(8.0)
                        .radius(6.0)
                        .fill(color("#282828"))
                        .color(color("#e2e2e2"))
                        .placeholder("Paste API key", color("#888888"))
                        .on_edit(|s: &mut State| &mut s.api_key_edit),
                )
                .child(
                    row()
                        .gap(8.0)
                        .child(credential_button(
                            "Save",
                            move |s| {
                                let key = s.api_key_edit.text.trim().to_owned();
                                if key.is_empty() {
                                    s.auth_message = "Enter an API key.".to_owned();
                                    s.auth_message_is_error = true;
                                    return;
                                }
                                s.api_key_edit.clear();
                                s.api_key_dialog = false;
                                start_auth_action(s, Action::SetApiKey(provider.id(), key));
                            },
                            theme,
                        ))
                        .child(credential_button(
                            "Cancel",
                            |s| {
                                s.api_key_edit.clear();
                                s.api_key_dialog = false;
                            },
                            theme,
                        )),
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
                .font(ui_font(14.0, 400.0))
                .color(color(
                    if !auth.error.is_empty() || state.auth_message_is_error {
                        PROVIDER_ERROR_COLOR
                    } else {
                        "#c8c8c8"
                    },
                )),
        );
    }
    if let Some(prompt) = &state.sign_in_prompt {
        detail = detail.child(
            col()
                .gap(7.0)
                .child(
                    text("Finish sign-in in your browser. Open the link again if needed:")
                        .font(ui_font(14.0, 400.0))
                        .color(color("#b8b8b8")),
                )
                .child(form_button("Open sign-in page", theme).on_press_tx({
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
                    .font(ui_font(15.0, 550.0))
                    .color(color("#eeeeee")),
                ),
        );
    }
    detail = detail.child(model_settings(state, provider));
    if provider == Provider::OpenAi {
        detail = detail.child(daybreak_settings(state));
    }
    detail
}

fn theme_preview(candidate: ThemeStyle<'_>) -> El {
    let p = candidate.palette();
    let fill = |value| rune::color::hex(value).unwrap_or(Color::BLACK);
    col()
        .size(52.0, 34.0)
        .radius(7.0)
        .border(1.0, fill(p.border))
        .fill(fill(p.chrome))
        .child(el().w_full().h(8.0).fill(fill(p.raised)))
        .child(
            row()
                .grow(1.0)
                .child(el().w(15.0).h_full().fill(fill(p.sidebar)))
                .child(
                    el().grow(1.0)
                        .h_full()
                        .padding(5.0)
                        .fill(fill(p.panel))
                        .child(el().w_full().h(3.0).radius(1.5).fill(fill(p.text))),
                ),
        )
}

fn color_picker_menu(state: &State) -> El {
    let picker = state.color_picker.as_ref().unwrap();
    let theme = state.appearance.style();
    let color = |value| theme.color(value);
    let fill = |hex: &str| rune::color::hex(hex).unwrap_or(Color::BLACK);
    let hue = fill(
        &Hsv {
            hue: picker.hsv.hue,
            saturation: 1.0,
            value: 1.0,
        }
        .hex(),
    );
    let current = state.color_value(picker.target).to_owned();
    let area = el()
        .key("color-picker-area")
        .role("group")
        .label("Saturation and brightness")
        .help("Drag to choose saturation and brightness")
        .size(208.0, 136.0)
        .radius(8.0)
        .clip_rounded(8.0)
        .fill(Color::WHITE)
        .linear_gradient(
            std::f32::consts::FRAC_PI_2,
            [(0.0, Color::WHITE), (1.0, hue)],
        )
        .cursor(Cursor::Crosshair)
        .child(
            el().size(208.0, 136.0)
                .key("color-picker-area-overlay")
                .gradient(Color::TRANSPARENT, Color::BLACK)
                .on_drag(|s: &mut State, drag, tx| {
                    drag_color_picker(s, drag, tx, PickerAxis::Area);
                }),
        )
        .child(
            el().absolute(
                picker.hsv.saturation * 196.0,
                (1.0 - picker.hsv.value) * 124.0,
            )
            .size(12.0, 12.0)
            .radius(6.0)
            .fill(fill(&current))
            .border(2.0, Color::WHITE)
            .shadow(1.0, 4.0, Color::BLACK.alpha(0.65)),
        );
    let hue_stops = [
        (0.0, fill("#ff0000")),
        (1.0 / 6.0, fill("#ffff00")),
        (2.0 / 6.0, fill("#00ff00")),
        (3.0 / 6.0, fill("#00ffff")),
        (4.0 / 6.0, fill("#0000ff")),
        (5.0 / 6.0, fill("#ff00ff")),
        (1.0, fill("#ff0000")),
    ];
    let hue_strip = el()
        .key("color-picker-hue")
        .role("group")
        .label("Hue")
        .help("Drag to choose hue")
        .size(208.0, 16.0)
        .radius(8.0)
        .clip_rounded(8.0)
        .linear_gradient(std::f32::consts::FRAC_PI_2, hue_stops)
        .cursor(Cursor::Crosshair)
        .on_drag(|s: &mut State, drag, tx| {
            drag_color_picker(s, drag, tx, PickerAxis::Hue);
        })
        .child(
            el().absolute(picker.hsv.hue * 200.0, 0.0)
                .size(8.0, 16.0)
                .radius(4.0)
                .border(2.0, Color::WHITE)
                .shadow(1.0, 3.0, Color::BLACK.alpha(0.65)),
        );
    in_place_menu::popup_surface(
        col()
            .key("color-picker-menu")
            .role("dialog")
            .label(format!("{} color selector", picker.target.title()))
            .w(232.0)
            .padding(12.0)
            .gap(10.0),
        color("#242424"),
    )
    .anchored_fit(picker.target.anchor(), Side::Bottom, Align::Start, 6.0)
    .child(
        row()
            .w_full()
            .center_y()
            .gap(8.0)
            .child(
                text(picker.target.title())
                    .font(ui_font(14.0, 550.0))
                    .color(color("#eeeeee")),
            )
            .child(el().grow(1.0))
            .child(
                text(current.clone())
                    .font(ui_font(12.5, 400.0))
                    .color(color("#b8b8b8")),
            ),
    )
    .child(area)
    .child(hue_strip)
    .child(
        row()
            .center_y()
            .gap(8.0)
            .child(el().size(20.0, 20.0).radius(5.0).fill(fill(&current)))
            .child(
                text("Use HEX for exact colors.")
                    .font(ui_font(11.5, 400.0))
                    .color(color("#969696")),
            ),
    )
    .modal(true, |s: &mut State| s.color_picker = None)
}

fn appearance_theme_palette_settings(state: &State) -> El {
    let theme = state.appearance.style();
    let color = |value| theme.color(value);
    let input_y_padding = (26.0 - 17.5 * state.appearance.text_size.factor()) * 0.5;
    let mut rows = Vec::new();
    for field in PaletteColor::ALL {
        let index = field.index();
        let hex = state
            .appearance
            .palette_color(state.appearance.theme, field);
        let revert = if state
            .appearance
            .palette_color_is_default(state.appearance.theme, field)
        {
            el().size(27.0, 27.0)
        } else {
            el().key(format!("palette-revert-{}", field.key()))
                .role("button")
                .label(format!("Revert {} theme color to default", field.name()))
                .size(27.0, 27.0)
                .center()
                .radius(8.0)
                .continuous_corners()
                .hover(color("#303030"))
                .child(icon(&REVERT, 16.0).color(color("#a8a8a8")))
                .on_press(move |s: &mut State| {
                    s.appearance
                        .set_palette_color(s.appearance.theme, field, None);
                    s.sync_palette_edits();
                    s.save_appearance();
                    #[cfg(target_os = "windows")]
                    if field == PaletteColor::Border {
                        with_windows_window(|hwnd| {
                            windows_frame::set_border_color(hwnd, &s.appearance);
                        });
                    }
                })
        };
        rows.push(
            row()
                .w_full()
                .min_h(62.0)
                .py(8.0)
                .center_y()
                .gap(12.0)
                .key(format!("palette-color-{}", field.key()))
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(4.0)
                        .child(
                            text(field.name())
                                .w_full()
                                .wrap()
                                .font(form_label_font())
                                .color(color("#e2e2e2")),
                        )
                        .child(
                            text(field.description())
                                .w_full()
                                .wrap()
                                .font(ui_font(14.0, 400.0))
                                .color(color("#969696")),
                        ),
                )
                .child(revert)
                .child(
                    el().key(format!("palette-swatch-{}", field.key()))
                        .role("button")
                        .label(format!("Choose {} theme color", field.name()))
                        .size(27.0, 27.0)
                        .radius(7.0)
                        .cursor(Cursor::Pointer)
                        .fill(rune::color::hex(hex).unwrap_or(Color::BLACK))
                        .on_press_tx(move |s: &mut State, tx| {
                            tx.instant();
                            toggle_color_picker(s, ColorTarget::Theme(field));
                        }),
                )
                .child(
                    text_input(&state.palette_edits[index], ui_font(14.0, 450.0))
                        .key(format!("palette-hex-{}", field.key()))
                        .single_line()
                        .w(96.0)
                        .h(26.0)
                        .px(12.0)
                        .py(input_y_padding)
                        .align(Align::Center)
                        .radius(8.0)
                        .continuous_corners()
                        .fill(color("#303030"))
                        .color(color("#e2e2e2"))
                        .on_edit(move |s: &mut State| &mut s.palette_edits[index])
                        .on_input_change(move |s: &mut State, _| {
                            if let Some(value) =
                                appearance::normalize_hex(&s.palette_edits[index].text)
                            {
                                s.appearance.set_palette_color(
                                    s.appearance.theme,
                                    field,
                                    Some(value),
                                );
                                s.save_appearance();
                                #[cfg(target_os = "windows")]
                                if field == PaletteColor::Border {
                                    with_windows_window(|hwnd| {
                                        windows_frame::set_border_color(hwnd, &s.appearance);
                                    });
                                }
                            }
                        }),
                ),
        );
    }
    settings_form(THEME_PALETTE_FORM, rows, theme)
}

fn appearance_colors_settings(state: &State) -> El {
    let theme = state.appearance.style();
    let color = |value| theme.color(value);
    let heat_input_y_padding = (26.0 - 17.5 * state.appearance.text_size.factor()) * 0.5;
    let header = row()
        .key("appearance-color-header")
        .w_full()
        .center_y()
        .gap(12.0)
        .padding(14.0)
        .radius(16.0)
        .continuous_corners()
        .border(1.0, color("#303030"))
        .child(theme_preview(theme))
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(4.0)
                .child(
                    text(theme.name())
                        .font(ui_font(23.0, 600.0))
                        .color(color("#f2f2f2")),
                )
                .child(
                    text(theme.description())
                        .w_full()
                        .wrap()
                        .font(ui_font(14.5, 400.0))
                        .color(color("#969696")),
                ),
        );

    let mut heat_rows = Vec::new();
    for level in HeatLevel::ALL {
        let index = level.index();
        let hex = state.appearance.heat_color(state.appearance.theme, level);
        let revert = if state
            .appearance
            .heat_color_is_default(state.appearance.theme, level)
        {
            el().size(27.0, 27.0)
        } else {
            el().key(format!("heat-revert-{}", level.name().to_ascii_lowercase()))
                .role("button")
                .label(format!("Revert {} heat color to default", level.name()))
                .size(27.0, 27.0)
                .center()
                .radius(8.0)
                .continuous_corners()
                .hover(color("#303030"))
                .child(icon(&REVERT, 16.0).color(color("#a8a8a8")))
                .on_press(move |s: &mut State| {
                    s.appearance.set_heat_color(s.appearance.theme, level, None);
                    s.sync_color_edits();
                    s.save_appearance();
                })
        };
        heat_rows.push(
            form_item_row()
                .key(format!("heat-level-{}", level.name().to_ascii_lowercase()))
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(4.0)
                        .child(
                            text(level.name())
                                .font(form_label_font())
                                .color(color("#e2e2e2")),
                        )
                        .child(
                            text(level.description())
                                .w_full()
                                .wrap()
                                .font(ui_font(14.0, 400.0))
                                .color(color("#969696")),
                        ),
                )
                .child(revert)
                .child(
                    el().key(format!("heat-swatch-{}", level.name().to_ascii_lowercase()))
                        .role("button")
                        .label(format!("Choose {} heat color", level.name()))
                        .size(27.0, 27.0)
                        .radius(7.0)
                        .cursor(Cursor::Pointer)
                        .fill(rune::color::hex(hex).unwrap_or(Color::BLACK))
                        .on_press_tx(move |s: &mut State, tx| {
                            tx.instant();
                            toggle_color_picker(s, ColorTarget::Heat(level));
                        }),
                )
                .child(
                    text_input(&state.heat_edits[index], ui_font(14.0, 450.0))
                        .key(format!("heat-hex-{index}"))
                        .single_line()
                        .w(96.0)
                        .h(26.0)
                        .px(12.0)
                        .py(heat_input_y_padding)
                        .align(Align::Center)
                        .radius(8.0)
                        .continuous_corners()
                        .fill(color("#303030"))
                        .color(color("#e2e2e2"))
                        .on_edit(move |s: &mut State| &mut s.heat_edits[index])
                        .on_input_change(move |s: &mut State, _| {
                            if let Some(value) =
                                appearance::normalize_hex(&s.heat_edits[index].text)
                            {
                                s.appearance
                                    .set_heat_color(s.appearance.theme, level, Some(value));
                                s.save_appearance();
                            }
                        }),
                ),
        );
    }
    let mut heat = settings_form(HEAT_FORM, heat_rows, theme);
    if !state.appearance_error.is_empty() {
        heat = heat.child(
            text(state.appearance_error.clone())
                .font(ui_font(14.0, 400.0))
                .color(color(PROVIDER_ERROR_COLOR)),
        );
    }
    col()
        .w_full()
        .gap(22.0)
        .child(header)
        .child(heat)
        .child(appearance_theme_palette_settings(state))
}

fn typography_row(state: &State, spec: &TypographyRowSpec, theme: ThemeStyle<'_>) -> El {
    let color = |value| theme.color(value);
    let value = match spec.menu {
        SelectionMenu::InterfaceFont => state.appearance.interface_font.name(),
        SelectionMenu::TextSize => state.appearance.text_size.name(),
        _ => unreachable!("typography rows use typography menus"),
    };
    let menu = spec.menu;
    row()
        .key(spec.key)
        .w_full()
        .min_h(62.0)
        .py(8.0)
        .center_y()
        .gap(12.0)
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(4.0)
                .child(
                    text(spec.label)
                        .w_full()
                        .wrap()
                        .font(form_label_font())
                        .color(color("#e2e2e2")),
                )
                .child(
                    text(spec.description)
                        .w_full()
                        .wrap()
                        .font(ui_font(14.0, 400.0))
                        .color(color("#969696")),
                ),
        )
        .child(
            selection_button(
                value,
                state.selection_menu == Some(menu),
                move |s| toggle_selection_menu(s, menu),
                theme,
            )
            .key(spec.control_key),
        )
}

fn appearance_text_settings(state: &State) -> El {
    let theme = state.appearance.style();
    let color = |value| theme.color(value);
    let options = settings_form(
        FONT_FORM,
        TYPOGRAPHY_ROWS
            .iter()
            .map(|spec| typography_row(state, spec, theme)),
        theme,
    );
    let sample = col()
        .key("appearance-text-sample")
        .w_full()
        .gap(12.0)
        .child(
            text("Sample")
                .font(ui_font(17.0, 600.0))
                .color(color("#f2f2f2")),
        )
        .child(
            col()
                .key("appearance-sample-card")
                .w_full()
                .padding(16.0)
                .gap(6.0)
                .radius(10.0)
                .fill(color("#191919"))
                .child(
                    text("The quick brown fox jumps over the lazy dog.")
                        .w_full()
                        .wrap()
                        .font(ui_font(16.0, 500.0))
                        .color(color("#e2e2e2")),
                )
                .child(
                    text("A preview of the selected interface font and size.")
                        .w_full()
                        .wrap()
                        .font(ui_font(14.0, 400.0))
                        .color(color("#969696")),
                ),
        );
    let mut content = col().w_full().gap(22.0).child(options).child(sample);
    if !state.appearance_error.is_empty() {
        content = content.child(
            text(state.appearance_error.clone())
                .font(ui_font(14.0, 400.0))
                .color(color(PROVIDER_ERROR_COLOR)),
        );
    }
    content
}

fn providers_settings_view(state: &State, _cx: &mut Cx<State>) -> Vec<El> {
    let theme = state.appearance.style();
    let color = |value| theme.color(value);
    let mut content = vec![
        row()
            .w_full()
            .h(26.0)
            .child(
                row()
                    .key("provider-view-tabs")
                    .role("tablist")
                    .label("Provider views")
                    .gap(6.0)
                    .child(provider_tab(
                        Provider::OpenAi,
                        state.selected == Provider::OpenAi,
                        theme,
                    ))
                    .child(provider_tab(
                        Provider::Xai,
                        state.selected == Provider::Xai,
                        theme,
                    )),
            )
            .child(el().grow(1.0))
            .child(lead_provider_controls(state)),
    ];
    if !state.lead_error.is_empty() {
        content.push(
            text(state.lead_error.clone())
                .font(ui_font(12.0, 400.0))
                .color(color(PROVIDER_ERROR_COLOR)),
        );
    }
    content.push(provider_detail(state).mt(6.0));
    content
}

fn appearance_settings_view(state: &State, cx: &mut Cx<State>) -> Vec<El> {
    let theme = state.appearance.style();
    let mut tabs = row()
        .key("appearance-view-tabs")
        .role("tablist")
        .label("Appearance views")
        .gap(6.0);
    for tab in AppearanceTab::ALL {
        tabs = tabs.child(appearance_tab_button(
            tab,
            state.appearance_tab == tab,
            theme,
        ));
    }
    let tabs_width = cx.layout_size("app-surface").map_or(1040.0, |size| size[0])
        - NAVIGATION_RAIL_WIDTH
        - if state.sidenav_visible {
            state.sidenav_width + 1.0
        } else {
            0.0
        }
        - 78.0;
    let controls = if tabs_width < 380.0 {
        col().w_full().gap(8.0).child(tabs).child(
            row()
                .w_full()
                .child(el().grow(1.0))
                .child(color_theme_controls(state)),
        )
    } else {
        row()
            .w_full()
            .h(26.0)
            .child(tabs)
            .child(el().grow(1.0))
            .child(color_theme_controls(state))
    };
    let detail = match state.appearance_tab {
        AppearanceTab::Colors => appearance_colors_settings(state),
        AppearanceTab::Typography => appearance_text_settings(state),
    };
    vec![controls, detail.mt(6.0)]
}

fn unavailable_settings_view(state: &State, _cx: &mut Cx<State>) -> Vec<El> {
    let theme = state.appearance.style();
    let color = |value| theme.color(value);
    vec![
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
                .font(ui_font(13.0, 400.0))
                .color(color("#aaaaaa")),
            ),
    ]
}

fn view(state: &State, cx: &mut Cx<State>) -> El {
    let _text_size_scope = RenderTextSizeScope::enter(state.appearance.text_size);
    let theme = state.appearance.style();
    let palette = theme.palette();
    let color = |value| theme.color(value);
    let mut content = col()
        .key("settings-page")
        .w_full()
        .max_w(760.0)
        .self_center()
        .gap(20.0);
    if state.primary_view == PrimaryView::Settings {
        content = content.child(
            text(state.settings_section.label())
                .key("settings-view-title")
                .font(ui_font(27.0, 600.0))
                .color(color("#f2f2f2")),
        );
        for item in (state.settings_section.spec().render)(state, cx) {
            content = content.child(item);
        }
        if let Some(label) = state.notice {
            content = content.child(
                text(format!("{label} is not available in this preview."))
                    .font(ui_font(12.0, 400.0))
                    .color(color("#c8c8c8")),
            );
        }
    } else if state.primary_view == PrimaryView::WorkspaceCreation {
        content = content.child(workspace_creation::render(state, theme));
    }
    let mut body = row().key("body").w_full().grow(1.0).pr(6.0).pb(6.0);
    body = body.child(rail(state, theme));
    let mut settings_surface = row()
        .key("settings-surface")
        .grow(1.0)
        .h_full()
        .radius(16.0)
        .clip_rounded(16.0)
        .fill(color("#111111"));
    if state.sidenav_visible {
        settings_surface = settings_surface
            .child(if state.primary_view != PrimaryView::Settings {
                home_sidebar::render(state.sidenav_width, theme)
            } else {
                settings_sidenav(
                    state.settings_section,
                    &state.connection,
                    state.sidenav_width,
                    theme,
                )
            })
            .child(el().w(1.0).h_full().fill(color("#2a2a2a")));
    }
    let mut main_content = col()
        .key("content-surface")
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .min_h(0.0)
        .fill(color("#0d0d0d"));
    if state.primary_view != PrimaryView::Home {
        let scroll_watch = if state.primary_view == PrimaryView::Settings {
            state.settings_scroll.clone()
        } else {
            state.workspace_scroll.clone()
        };
        let scroll_key = if state.primary_view == PrimaryView::Settings {
            "settings-scroll"
        } else {
            "workspace-scroll"
        };
        let scroll_bar = scroll_watch.clone();
        main_content = main_content
            .child(
                col()
                    .key(scroll_key)
                    .grow(1.0)
                    .w_full()
                    .min_w(0.0)
                    .min_h(0.0)
                    .scroll(ScrollSpec {
                        stick_bottom: false,
                    })
                    .scroll_edge_fade(16.0)
                    .on_scroll_offset(move |position| scroll_watch.watch_scroll(position))
                    .on_wheel(|state: &mut State, event, tx| {
                        if state.primary_view == PrimaryView::Settings {
                            state.settings_scroll.wheel(event, tx)
                        } else {
                            state.workspace_scroll.wheel(event, tx)
                        }
                    })
                    .child(col().w_full().padding(36.0).child(content)),
            )
            .child(scroll_bar.bar(theme));
    }
    settings_surface = settings_surface.child(main_content);
    if state.sidenav_visible {
        settings_surface = settings_surface.child(sidenav_resize_handle(state.sidenav_width));
    }
    body = body.child(settings_surface);
    let mut window = col().key("window").role("window").w_full().h_full();
    if cfg!(target_os = "windows") && !state.native_window_border {
        window = window.padding(1.0).fill(color("#323232"));
    }
    let [background_width, background_height] =
        cx.layout_size("app-surface").unwrap_or([1040.0, 720.0]);
    let header_prefix_width = if state.sidenav_visible {
        NAVIGATION_RAIL_WIDTH + state.sidenav_width
    } else {
        HEADER_COLLAPSED_PREFIX_WIDTH
    };
    let upper_left_radius = ((background_width * 0.88).hypot(background_height * 0.92)) * 0.43;
    let lower_right_radius = ((background_width * 0.88).hypot(background_height * 0.92)) * 0.48;
    let surface = col()
        .key("app-surface")
        .w_full()
        .grow(1.0)
        .fill(color(palette.chrome))
        .linear_gradient(
            145.0_f32.to_radians(),
            [
                (0.0, color(palette.gradient_middle)),
                (1.0, color(palette.gradient_end)),
            ],
        )
        .child(
            el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .radial_gradient(
                    [0.88, 0.92],
                    lower_right_radius,
                    [
                        (0.0, color(palette.gradient_middle)),
                        (1.0, Color::TRANSPARENT),
                    ],
                ),
        )
        .child(
            el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .radial_gradient(
                    [0.12, 0.08],
                    upper_left_radius,
                    [
                        (0.0, color(palette.gradient_start)),
                        (1.0, Color::TRANSPARENT),
                    ],
                ),
        )
        .child(header(state, header_prefix_width))
        .child(body);
    window = window.child(surface);
    if let Some(menu) = state.selection_menu
        && match menu {
            SelectionMenu::ResearchKit => {
                state.primary_view == PrimaryView::WorkspaceCreation
                    && state.workspace_draft.tab == workspace_creation::CreationTab::Settings
            }
            SelectionMenu::ResourceKind | SelectionMenu::ResourceDirection => {
                state.primary_view == PrimaryView::WorkspaceCreation
                    && state.workspace_draft.tab == workspace_creation::CreationTab::Resources
            }
            SelectionMenu::ColorTheme => state.settings_section == SettingsSection::Appearance,
            SelectionMenu::InterfaceFont | SelectionMenu::TextSize => {
                state.settings_section == SettingsSection::Appearance
                    && state.appearance_tab == AppearanceTab::Typography
            }
            _ => {
                state.primary_view == PrimaryView::Settings
                    && state.settings_section == SettingsSection::Providers
            }
        }
    {
        let items = selection_menu_items(state, menu);
        if !items.is_empty() {
            let (title, width) = match menu {
                SelectionMenu::ResearchKit => ("Research Kit", 178.0),
                SelectionMenu::ResourceKind => ("Resource Type", 178.0),
                SelectionMenu::ResourceDirection => ("Scope", 178.0),
                SelectionMenu::LeadProvider => ("Default Provider", 178.0),
                SelectionMenu::ColorTheme => ("Color Theme", 178.0),
                SelectionMenu::InterfaceFont => ("Interface Font", 178.0),
                SelectionMenu::TextSize => ("Text Size", 178.0),
                SelectionMenu::SourcePriority { .. } => ("Source Priority", 178.0),
                SelectionMenu::Model { .. } => ("Choose model", 380.0),
            };
            window = window.child(
                in_place_menu::panel(
                    selection_menu_anchor(menu),
                    title,
                    width,
                    state.menu_cursor.min(items.len() - 1),
                    items,
                    theme,
                    (
                        move |s: &mut State, index| {
                            if s.selection_menu == Some(menu) {
                                s.menu_cursor = index;
                            }
                        },
                        move |s: &mut State, value, tx| {
                            choose_selection_menu_item(s, menu, value, tx);
                        },
                    ),
                )
                .modal(true, |s: &mut State| s.selection_menu = None)
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
    if state.primary_view == PrimaryView::Settings
        && state.color_picker.is_some()
        && state.settings_section == SettingsSection::Appearance
        && state.appearance_tab == AppearanceTab::Colors
    {
        window = window.child(color_picker_menu(state));
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
    #[cfg(target_os = "macos")]
    if let Err(error) = rune::set_app_icon(include_bytes!("../assets/app-icon.png")) {
        eprintln!("Beale app icon could not be loaded: {error}");
    }
    let font_spec = desktop_fonts::v1_font_spec();
    let saved_font = State::load_appearance().appearance.interface_font;
    let spec = AppSpec {
        scenes: &[
            ("home", |s: &mut State| s.primary_view = PrimaryView::Home),
            ("workspace-creation", |s: &mut State| {
                s.primary_view = PrimaryView::WorkspaceCreation;
            }),
            ("xai", |s: &mut State| s.selected = Provider::Xai),
            ("appearance-dark", |s: &mut State| {
                s.settings_section = SettingsSection::Appearance;
                s.appearance.theme = AppearanceTheme::Dark;
                s.sync_color_edits();
            }),
            ("appearance-light", |s: &mut State| {
                s.settings_section = SettingsSection::Appearance;
                s.appearance.theme = AppearanceTheme::Light;
                s.sync_color_edits();
            }),
            ("appearance-narrow-nav", |s: &mut State| {
                s.settings_section = SettingsSection::Appearance;
                s.sidenav_width = 420.0;
                s.appearance.theme = AppearanceTheme::Light;
                s.sync_color_edits();
            }),
            ("appearance-narrow-nav-large", |s: &mut State| {
                s.settings_section = SettingsSection::Appearance;
                s.sidenav_width = SIDENAV_MAX_WIDTH;
                s.appearance.theme = AppearanceTheme::Light;
                s.appearance.text_size = TextSize::Large;
                s.sync_color_edits();
            }),
            ("appearance-custom-heat", |s: &mut State| {
                s.settings_section = SettingsSection::Appearance;
                s.sidenav_width = SIDENAV_MAX_WIDTH;
                s.appearance.theme = AppearanceTheme::Light;
                s.appearance.text_size = TextSize::Large;
                s.appearance.set_heat_color(
                    AppearanceTheme::Light,
                    HeatLevel::Low,
                    Some("#123456".to_owned()),
                );
                s.appearance.set_heat_color(
                    AppearanceTheme::Light,
                    HeatLevel::High,
                    Some("#654321".to_owned()),
                );
                s.appearance.set_palette_color(
                    AppearanceTheme::Light,
                    PaletteColor::Panel,
                    Some("#f7fafc".to_owned()),
                );
                s.appearance.set_palette_color(
                    AppearanceTheme::Light,
                    PaletteColor::Sidebar,
                    Some("#e6edf2".to_owned()),
                );
                s.sync_color_edits();
            }),
            ("appearance-picker", |s: &mut State| {
                s.settings_section = SettingsSection::Appearance;
                s.sidenav_width = SIDENAV_MAX_WIDTH;
                s.appearance.theme = AppearanceTheme::Light;
                s.appearance.text_size = TextSize::Large;
                s.sync_color_edits();
                toggle_color_picker(s, ColorTarget::Heat(HeatLevel::Low));
            }),
            ("appearance-cream", |s: &mut State| {
                s.settings_section = SettingsSection::Appearance;
                s.appearance.theme = AppearanceTheme::Cream;
                s.sync_color_edits();
            }),
            ("appearance-midnight", |s: &mut State| {
                s.settings_section = SettingsSection::Appearance;
                s.appearance.theme = AppearanceTheme::Midnight;
                s.sync_color_edits();
            }),
            ("appearance-text", |s: &mut State| {
                s.settings_section = SettingsSection::Appearance;
                s.appearance_tab = AppearanceTab::Typography;
            }),
            ("appearance-text-large", |s: &mut State| {
                s.settings_section = SettingsSection::Appearance;
                s.appearance_tab = AppearanceTab::Typography;
                s.appearance.text_size = TextSize::Large;
            }),
            ("appearance-text-extra-large", |s: &mut State| {
                s.settings_section = SettingsSection::Appearance;
                s.appearance_tab = AppearanceTab::Typography;
                s.sidenav_width = SIDENAV_MAX_WIDTH;
                s.appearance.text_size = TextSize::ExtraLarge;
            }),
            ("appearance-text-light", |s: &mut State| {
                s.settings_section = SettingsSection::Appearance;
                s.appearance_tab = AppearanceTab::Typography;
                s.sidenav_width = SIDENAV_MAX_WIDTH;
                s.appearance.theme = AppearanceTheme::Light;
                s.appearance.text_size = TextSize::Large;
            }),
            ("providers-light", |s: &mut State| {
                s.appearance.theme = AppearanceTheme::Light
            }),
            ("providers-large", |s: &mut State| {
                s.appearance.text_size = TextSize::Large;
            }),
        ],
        min_size: Some((840.0, 600.0)),
        fonts: font_spec.fonts,
        families: desktop_fonts::families(saved_font),
        on_key: Some(selection_menu_key),
        ..AppSpec::new("Beale", (1040.0, 720.0), State::load_appearance, view)
    };
    let app = rune::App::new(spec).chrome(rune::Chrome {
        drag_regions: true,
        ..rune::Chrome::DEFAULT
    });
    #[cfg(target_os = "windows")]
    let app = app
        .on_close(|_, _| with_windows_window(window_state::save_now))
        .window_options(rune::WindowOptions {
            borderless: true,
            ..rune::WindowOptions::default()
        })
        .on_window(|window| {
            let slot = MAIN_WINDOW.get_or_init(|| std::sync::Mutex::new(None));
            if let Ok(mut current) = slot.lock() {
                *current = Some(window.clone());
            }
            let appearance = State::load_appearance().appearance;
            let native_border = windows_frame::configure(window, &appearance);
            window_state::restore_and_track(window);
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
            let daybreak_mode = auth_loaded.then_some(auth_snapshot.daybreak_mode);
            handle.update_if(move |state, _| {
                if state.connection == status
                    && state.auth_status == auth_status
                    && state.auth_loaded == auth_loaded
                    && state.lead_provider == lead_provider
                    && daybreak_mode
                        .is_none_or(|mode| state.daybreak_saving || state.daybreak_mode == mode)
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
                    if let Some(mode) = daybreak_mode
                        && !state.daybreak_saving
                    {
                        state.daybreak_mode = mode;
                    }
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
                        if previous[provider.index()].subscription
                            != state.auth_status[provider.index()].subscription
                            || previous[provider.index()].api_key
                                != state.auth_status[provider.index()].api_key
                        {
                            state.source_errors[provider.index()].clear();
                        }
                        if state.selection_menu == Some(SelectionMenu::SourcePriority { provider })
                            && (!state.auth_status[provider.index()].subscription
                                || !state.auth_status[provider.index()].api_key)
                        {
                            state.selection_menu = None;
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
    use rune::{Env, Runtime, WheelEvent};

    #[test]
    fn settings_scrollbar_tracks_overflow_and_can_reach_the_bottom() {
        let state = State {
            settings_section: SettingsSection::Appearance,
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (840.0, 600.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        runtime.frame(1.0 / 60.0);
        let position = runtime.state.settings_scroll.position();
        let metrics = runtime.state.settings_scroll.metrics();
        assert!(position.max[1] > 0.0, "{position:?}");
        assert!(metrics.0 > 0.0 && metrics.1 > 0.0, "{metrics:?}");
        assert!(metrics.2 < metrics.0, "thumb leaves room to travel");
        let (bar, _) = runtime.presented("settings-scrollbar").unwrap();
        let (surface, _) = runtime.presented("content-surface").unwrap();
        assert_eq!(bar[1], surface[1], "bar meets the panel top");
        assert_eq!(
            bar[1] + bar[3],
            surface[1] + surface[3],
            "bar meets the panel bottom"
        );
        assert_eq!(
            bar[0] + bar[2],
            surface[0] + surface[2],
            "bar meets the panel right"
        );
        let (down, _) = runtime.presented("settings-scroll-down").unwrap();
        let (up, _) = runtime.presented("settings-scroll-up").unwrap();
        let click = |runtime: &mut Runtime<State>, rect: [f32; 4]| {
            let x = rect[0] + rect[2] / 2.0;
            let y = rect[1] + rect[3] / 2.0;
            runtime.pointer_down(x, y);
            runtime.pointer_up(x, y);
            runtime.frame(1.0 / 60.0);
        };
        click(&mut runtime, down);
        assert!(runtime.state.settings_scroll.position().offset[1] > 0.0);
        click(&mut runtime, up);
        assert!(runtime.state.settings_scroll.position().offset[1] < 1.0);
        let (track, _) = runtime.presented("settings-scrollbar-track").unwrap();
        runtime.pointer_down(track[0] + track[2] / 2.0, track[1] + track[3] - 6.0);
        runtime.pointer_up(track[0] + track[2] / 2.0, track[1] + track[3] - 6.0);
        runtime.frame(1.0 / 60.0);
        assert!(
            runtime.state.settings_scroll.position().offset[1] > position.max[1] * 0.9,
            "track click jumps near bottom"
        );
        let (thumb, _) = runtime.presented("settings-scrollbar-thumb").unwrap();
        let thumb_x = thumb[0] + thumb[2] / 2.0;
        let thumb_y = thumb[1] + thumb[3] / 2.0;
        runtime.pointer_down(thumb_x, thumb_y);
        runtime.pointer_move(thumb_x, track[1] + 6.0);
        runtime.pointer_up(thumb_x, track[1] + 6.0);
        runtime.frame(1.0 / 60.0);
        assert!(
            runtime.state.settings_scroll.position().offset[1] < position.max[1] * 0.1,
            "thumb drag reaches top"
        );
        runtime.update(|state, _| state.settings_section = SettingsSection::General);
        runtime.frame(0.0);
        runtime.frame(0.0);
        assert!(runtime.state.settings_scroll.position().max[1] <= 1.0);
        assert_eq!(runtime.state.settings_scroll.metrics().1, 0.0);
    }

    #[test]
    fn settings_wheel_glides_notched_steps_and_preserves_precise_scrolling() {
        let state = State {
            settings_section: SettingsSection::Appearance,
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (840.0, 600.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        runtime.frame(0.0);
        let (viewport, _) = runtime.presented("settings-scroll").unwrap();
        let x = viewport[0] + viewport[2] / 2.0;
        let y = viewport[1] + viewport[3] / 2.0;
        runtime.wheel_event(WheelEvent::new(0.0, -120.0).at(x, y));
        runtime.frame(1.0 / 60.0);
        let first = runtime.state.settings_scroll.position().offset[1];
        assert!(
            first > 0.0 && first < 120.0,
            "notched wheel glides: {first}"
        );
        for _ in 0..30 {
            runtime.frame(1.0 / 60.0);
        }
        let settled = runtime.state.settings_scroll.position().offset[1];
        assert!((settled - 120.0).abs() < 2.0, "glide settles: {settled}");
        runtime.wheel_event(WheelEvent::new(0.0, -30.0).at(x, y).precise(true));
        runtime.frame(1.0 / 60.0);
        assert!(
            (runtime.state.settings_scroll.position().offset[1] - 150.0).abs() < 2.0,
            "precise wheel follows its delta"
        );
    }

    #[test]
    fn selecting_a_provider_updates_the_detail() {
        let mut runtime =
            Runtime::new(State::default(), view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        assert!(runtime.presented("provider-daybreak-mode").is_some());
        let (x, y) = runtime.find("provider-xai").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert_eq!(runtime.state.selected, Provider::Xai);
        assert!(runtime.find("provider-detail-xai").is_some());
        assert!(runtime.find("provider-detail-openai").is_none());
        assert!(runtime.find("provider-daybreak-mode").is_none());
    }

    #[test]
    fn appearance_theme_and_heat_color_controls_update_state() {
        let state = State {
            settings_section: SettingsSection::Appearance,
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (1040.0, 800.0), Env::new(true, 1.0));
        runtime.on_key = Some(selection_menu_key);
        runtime.frame(0.0);
        assert!(runtime.find("appearance-color-header").is_some());
        assert!(runtime.find("appearance-theme-group").is_none());
        assert!(runtime.find("heat-theme-dark").is_none());
        assert!(runtime.find("heat-reset").is_none());
        assert!(runtime.find("heat-revert-low").is_none());
        let (x, y) = runtime.find("color-theme-control").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert!(runtime.find("selection-menu-item-0").is_some());
        runtime.key(KeyEvent {
            key: Key::Home,
            mods: Default::default(),
        });
        runtime.frame(0.0);
        runtime.key(KeyEvent {
            key: Key::Enter,
            mods: Default::default(),
        });
        runtime.frame(0.0);
        assert_eq!(runtime.state.appearance.theme, AppearanceTheme::Light);
        assert_eq!(runtime.state.heat_edits[0].text, "#e6ce7f");
        runtime.update(|state, _| {
            let edit = &mut state.heat_edits[0];
            edit.focused = true;
            edit.anchor = 0;
            edit.focus = edit.text.len();
        });
        runtime.frame(0.0);
        runtime.paste("#123456");
        runtime.frame(0.0);
        assert_eq!(
            runtime
                .state
                .appearance
                .heat_color(AppearanceTheme::Light, HeatLevel::Low),
            "#123456"
        );
        assert_eq!(
            runtime
                .state
                .appearance
                .heat_color(AppearanceTheme::Dark, HeatLevel::Low),
            "#857300"
        );
        assert!(runtime.find("heat-revert-low").is_some());
        assert!(runtime.find("heat-revert-medium").is_none());
        runtime.update(|state, _| {
            state.appearance.set_heat_color(
                AppearanceTheme::Light,
                HeatLevel::Medium,
                Some("#123456".to_owned()),
            );
            state.sync_heat_edits();
        });
        runtime.frame(0.0);
        assert!(runtime.find("heat-revert-medium").is_some());
        let (x, y) = runtime.find("heat-revert-low").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert_eq!(
            runtime
                .state
                .appearance
                .heat_color(AppearanceTheme::Light, HeatLevel::Low),
            "#e6ce7f"
        );
        assert_eq!(runtime.state.heat_edits[0].text, "#e6ce7f");
        assert!(runtime.find("heat-revert-low").is_none());
        assert!(runtime.find("heat-revert-medium").is_some());
        assert_eq!(
            runtime
                .state
                .appearance
                .heat_color(AppearanceTheme::Light, HeatLevel::Medium),
            "#123456"
        );
    }

    #[test]
    fn appearance_heat_rows_fit_min_window_with_wide_sidenav() {
        let mut state = State {
            settings_section: SettingsSection::Appearance,
            sidenav_width: SIDENAV_MAX_WIDTH,
            appearance: AppearancePreferences {
                theme: AppearanceTheme::Light,
                text_size: TextSize::Large,
                ..AppearancePreferences::default()
            },
            ..State::default()
        };
        state.appearance.set_heat_color(
            AppearanceTheme::Light,
            HeatLevel::Low,
            Some("#123456".to_owned()),
        );
        state.sync_heat_edits();
        let mut runtime = Runtime::new(state, view, (840.0, 600.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let (page, _) = runtime.presented("settings-page").unwrap();
        assert!(page[0] + page[2] <= 832.0, "page fits minimum window");
        for level in HeatLevel::ALL {
            let key = format!("heat-level-{}", level.name().to_ascii_lowercase());
            let (row, _) = runtime.presented(&key).unwrap();
            assert!(row[0] + row[2] <= page[0] + page[2], "{key} fits page");
        }
        assert!(runtime.find("heat-revert-low").is_some());
        assert!(runtime.find("heat-revert-medium").is_none());
    }

    #[test]
    fn theme_palette_edit_and_revert_update_active_colors() {
        let mut state = State {
            settings_section: SettingsSection::Appearance,
            appearance: AppearancePreferences {
                theme: AppearanceTheme::Light,
                ..AppearancePreferences::default()
            },
            ..State::default()
        };
        state.sync_color_edits();
        let mut runtime = Runtime::new(state, view, (1040.0, 1300.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        assert!(runtime.find("appearance-theme-palette").is_some());
        assert!(runtime.find("palette-revert-panel").is_none());
        runtime.update(|state, _| {
            let edit = &mut state.palette_edits[PaletteColor::Panel.index()];
            edit.focused = true;
            edit.anchor = 0;
            edit.focus = edit.text.len();
        });
        runtime.frame(0.0);
        runtime.paste("#123456");
        runtime.frame(0.0);
        assert_eq!(
            runtime
                .state
                .appearance
                .palette_color(AppearanceTheme::Light, PaletteColor::Panel),
            "#123456"
        );
        assert_eq!(
            runtime.state.appearance.style().color("#0d0d0d"),
            rune::color::hex("#123456").unwrap()
        );
        assert!(runtime.find("palette-revert-panel").is_some());
        assert!(runtime.find("palette-revert-card").is_none());
        let (x, y) = runtime.find("palette-revert-panel").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert_eq!(
            runtime
                .state
                .appearance
                .palette_color(AppearanceTheme::Light, PaletteColor::Panel),
            "#ffffff"
        );
        assert_eq!(
            runtime.state.palette_edits[PaletteColor::Panel.index()].text,
            "#ffffff"
        );
        assert!(runtime.find("palette-revert-panel").is_none());
    }

    #[test]
    fn color_swatches_open_selector_and_drag_changes_only_the_selected_color() {
        let mut state = State {
            settings_section: SettingsSection::Appearance,
            sidenav_width: SIDENAV_MAX_WIDTH,
            appearance: AppearancePreferences {
                theme: AppearanceTheme::Light,
                ..AppearancePreferences::default()
            },
            ..State::default()
        };
        state.sync_color_edits();
        let mut runtime = Runtime::new(state, view, (840.0, 600.0), Env::new(true, 1.0));
        runtime.on_key = Some(selection_menu_key);
        runtime.frame(0.0);
        let (x, y) = runtime.find("heat-swatch-low").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        let (menu, _) = runtime.presented("color-picker-menu").unwrap();
        let (page, _) = runtime.presented("settings-page").unwrap();
        assert!(menu[0] >= 8.0 && menu[0] + menu[2] <= 832.0);
        assert!(menu[1] >= 8.0 && menu[1] + menu[3] <= 592.0);
        assert!(menu[0] >= page[0] && menu[0] + menu[2] <= page[0] + page[2]);
        let (area, _) = runtime.presented("color-picker-area").unwrap();
        assert!(runtime.find("color-picker-area-overlay").is_some());
        let before = runtime.state.color_picker.as_ref().unwrap().hsv;
        runtime.pointer_down(area[0] + area[2] * 0.75, area[1] + area[3] * 0.2);
        runtime.pointer_move(area[0] + area[2] * 0.8, area[1] + area[3] * 0.25);
        runtime.frame(0.0);
        assert_ne!(
            runtime.state.color_picker.as_ref().unwrap().hsv,
            before,
            "picker drag changed HSV"
        );
        runtime.pointer_up(area[0] + area[2] * 0.8, area[1] + area[3] * 0.25);
        runtime.frame(0.0);
        let picked = runtime
            .state
            .appearance
            .heat_color(AppearanceTheme::Light, HeatLevel::Low);
        assert_ne!(picked, "#e6ce7f");
        assert_eq!(runtime.state.heat_edits[0].text, picked);
        assert_eq!(
            runtime
                .state
                .appearance
                .heat_color(AppearanceTheme::Light, HeatLevel::Medium),
            "#ce9564"
        );
        assert!(runtime.find("heat-revert-low").is_some());
        runtime.key(KeyEvent {
            key: Key::Escape,
            mods: Default::default(),
        });
        runtime.frame(0.0);
        assert!(runtime.find("color-picker-menu").is_none());
    }

    #[test]
    fn theme_swatch_picker_updates_palette_and_hex_field() {
        let mut state = State {
            settings_section: SettingsSection::Appearance,
            appearance: AppearancePreferences {
                theme: AppearanceTheme::Light,
                ..AppearancePreferences::default()
            },
            ..State::default()
        };
        state.sync_color_edits();
        let mut runtime = Runtime::new(state, view, (1040.0, 1300.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let (x, y) = runtime.find("palette-swatch-panel").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert!(runtime.find("color-picker-hue").is_some());
        let (hue, _) = runtime.presented("color-picker-hue").unwrap();
        runtime.pointer_down(hue[0] + hue[2] * 0.65, hue[1] + hue[3] * 0.5);
        runtime.pointer_up(hue[0] + hue[2] * 0.65, hue[1] + hue[3] * 0.5);
        runtime.frame(0.0);
        let (area, _) = runtime.presented("color-picker-area").unwrap();
        runtime.pointer_down(area[0] + area[2] * 0.8, area[1] + area[3] * 0.1);
        runtime.pointer_up(area[0] + area[2] * 0.8, area[1] + area[3] * 0.1);
        runtime.frame(0.0);
        let picked = runtime
            .state
            .appearance
            .palette_color(AppearanceTheme::Light, PaletteColor::Panel);
        assert_ne!(picked, "#ffffff");
        assert_eq!(
            runtime.state.palette_edits[PaletteColor::Panel.index()].text,
            picked
        );
        assert_eq!(
            runtime
                .state
                .appearance
                .palette_color(AppearanceTheme::Dark, PaletteColor::Panel),
            "#0d0d0d"
        );
    }

    #[test]
    fn appearance_text_controls_switch_fonts_and_scale() {
        let state = State {
            settings_section: SettingsSection::Appearance,
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (1040.0, 800.0), Env::new(true, 1.0));
        runtime.on_key = Some(selection_menu_key);
        runtime.frame(0.0);
        let (x, y) = runtime.find("appearance-tab-typography").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(runtime.state.appearance_tab, AppearanceTab::Typography);
        assert!(runtime.find("appearance-font-options").is_some());
        assert!(runtime.find("appearance-text-sample").is_some());
        assert!(runtime.find("appearance-sample-card").is_some());
        assert!(runtime.find("appearance-font-group").is_none());
        assert!(runtime.find("appearance-color-header").is_none());
        assert!(runtime.find("color-theme-control").is_some());
        for key in ["appearance-font-control", "appearance-size-control"] {
            let (button, _) = runtime.presented(key).unwrap();
            assert!(button[2] < 144.0, "{key} fits its value: {button:?}");
        }

        let (x, y) = runtime.find("appearance-font-control").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        let font_items = selection_menu_items(&runtime.state, SelectionMenu::InterfaceFont);
        assert_eq!(font_items.len(), 2);
        assert!(font_items[0].selected);
        for _ in 0..20 {
            runtime.frame(1.0 / 60.0);
        }
        let (x, y) = runtime.find("selection-menu-item-1").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert_eq!(
            runtime.state.appearance.interface_font,
            InterfaceFont::System
        );
        assert_eq!(
            runtime.font_families(),
            desktop_fonts::families(InterfaceFont::System)
        );

        let (x, y) = runtime.find("appearance-size-control").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        let size_items = selection_menu_items(&runtime.state, SelectionMenu::TextSize);
        assert_eq!(
            size_items
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            ["Extra Small", "Small", "Standard", "Large", "Extra Large"]
        );
        for _ in 0..20 {
            runtime.frame(1.0 / 60.0);
        }
        let (x, y) = runtime.find("selection-menu-item-4").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert_eq!(runtime.state.appearance.text_size, TextSize::ExtraLarge);
        {
            let _scope = RenderTextSizeScope::enter(TextSize::ExtraLarge);
            assert!((ui_font(14.0, 400.0).size - 16.8).abs() < 0.001);
        }
        assert_eq!(ui_font(14.0, 400.0).size, 14.0);
    }

    #[test]
    fn appearance_text_options_fit_extra_large_at_minimum_width() {
        let state = State {
            settings_section: SettingsSection::Appearance,
            appearance_tab: AppearanceTab::Typography,
            sidenav_width: SIDENAV_MAX_WIDTH,
            appearance: AppearancePreferences {
                text_size: TextSize::ExtraLarge,
                ..AppearancePreferences::default()
            },
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (840.0, 600.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        runtime.frame(0.0);
        let (page, _) = runtime.presented("settings-page").unwrap();
        let (tabs, _) = runtime.presented("appearance-view-tabs").unwrap();
        let (theme, _) = runtime.presented("color-theme-selector").unwrap();
        let (options, _) = runtime.presented("appearance-font-options").unwrap();
        let (sample, _) = runtime.presented("appearance-text-sample").unwrap();
        assert!(
            tabs[1] + tabs[3] <= theme[1],
            "theme selector clears tabs: page={page:?}, tabs={tabs:?}, theme={theme:?}"
        );
        assert!(theme[0] + theme[2] <= page[0] + page[2]);
        assert!(options[0] + options[2] <= page[0] + page[2]);
        for key in ["appearance-font-control", "appearance-size-control"] {
            let (button, _) = runtime.presented(key).unwrap();
            assert!(
                button[0] + button[2] <= options[0] + options[2],
                "{key} fits form"
            );
        }
        assert!(sample[0] + sample[2] <= page[0] + page[2]);
        assert!(options[1] + options[3] < sample[1]);
    }

    #[test]
    fn provider_details_fill_the_tab_area_at_narrow_and_wide_sizes() {
        for width in [840.0, 1040.0, 1320.0] {
            let state = State {
                selected: Provider::OpenAi,
                appearance: AppearancePreferences {
                    text_size: TextSize::Large,
                    ..AppearancePreferences::default()
                },
                ..State::default()
            };
            let mut runtime = Runtime::new(state, view, (width, 800.0), Env::new(true, 1.0));
            runtime.frame(0.0);
            let (page, _) = runtime.presented("settings-page").unwrap();
            assert!(page[0] + page[2] <= width - 8.0, "page fits window");
            for key in [
                "provider-detail-openai",
                "provider-view-header",
                "provider-authentication",
                "provider-model-defaults",
                "provider-daybreak-mode",
            ] {
                let (area, _) = runtime.presented(key).unwrap();
                assert!((area[0] - page[0]).abs() < 1.0, "{key} left edge");
                assert!(
                    (area[0] + area[2] - page[0] - page[2]).abs() < 1.0,
                    "{key} right edge"
                );
            }
        }
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
        runtime.pointer_down(10.0, 300.0);
        runtime.pointer_up(10.0, 300.0);
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
        let mut runtime = Runtime::new(state, view, (840.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let lead_before = runtime.presented("choose-lead-model").unwrap().0;
        let small_before = runtime.presented("choose-small-model").unwrap().0;
        let group = runtime.presented("provider-model-defaults").unwrap().0;
        assert!(lead_before[0] + lead_before[2] <= group[0] + group[2]);
        assert!(small_before[0] + small_before[2] <= group[0] + group[2]);

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
    fn source_priority_defaults_to_subscription_and_refresh_sits_in_header() {
        let mut status = AuthStatus {
            api_key: true,
            ..AuthStatus::default()
        };
        assert_eq!(effective_model_method(&status), AuthMethod::ApiKey);
        status.subscription = true;
        assert_eq!(effective_model_method(&status), AuthMethod::Subscription);
        status.source_priority = AuthMethod::ApiKey;
        assert_eq!(effective_model_method(&status), AuthMethod::ApiKey);
        status.api_key = false;
        assert_eq!(effective_model_method(&status), AuthMethod::Subscription);
        status.api_key = true;

        let state = State {
            auth_loaded: true,
            auth_status: [status, AuthStatus::default()],
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let (header, _) = runtime.presented("provider-view-header").unwrap();
        let (refresh, _) = runtime.presented("refresh-provider-models").unwrap();
        assert!(refresh[0] > header[0] + header[2] * 0.5);
        assert!(refresh[1] >= header[1] && refresh[1] + refresh[3] <= header[1] + header[3]);
        assert!(runtime.find("source-priority-control").is_some());
        let items = selection_menu_items(
            &runtime.state,
            SelectionMenu::SourcePriority {
                provider: Provider::OpenAi,
            },
        );
        assert_eq!(items.len(), 2);
        assert!(!items[0].selected);
        assert!(items[1].selected);
        assert!(runtime.find("choose-lead-model").is_none());
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
    fn home_uses_its_sidebar_and_keeps_the_main_area_blank() {
        let mut runtime =
            Runtime::new(State::default(), view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let (x, y) = runtime.find("nav-home").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(runtime.state.primary_view, PrimaryView::Home);
        for key in [
            "home-sidenav",
            "home-wordmark",
            "home-new-research",
            "home-quick-chat",
            "home-workspaces-header",
            "home-search-workspaces",
            "home-add-workspace",
            "home-workspaces-empty",
            "content-surface",
        ] {
            assert!(runtime.find(key).is_some(), "{key} is visible");
        }
        for key in [
            "settings-sidenav",
            "settings-page",
            "settings-scroll",
            "header-divider",
        ] {
            assert!(runtime.find(key).is_none(), "{key} is absent from Home");
        }
        let history = runtime.state.view_history.back.len();
        let (x, y) = runtime.find("home-new-research").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert_eq!(runtime.state.primary_view, PrimaryView::Home);
        assert_eq!(runtime.state.view_history.back.len(), history);
        let (x, y) = runtime.find("header-back").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(runtime.state.primary_view, PrimaryView::Settings);
        assert!(runtime.find("settings-sidenav").is_some());
    }

    #[test]
    fn new_workspace_draft_moves_through_settings_resources_and_rules() {
        fn click(runtime: &mut Runtime<State>, key: &str) {
            let (x, y) = runtime.find(key).unwrap();
            runtime.pointer_down(x, y);
            runtime.pointer_up(x, y);
            runtime.frame(1.0);
        }
        let mut runtime =
            Runtime::new(State::default(), view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        click(&mut runtime, "nav-home");
        click(&mut runtime, "home-add-workspace");
        assert_eq!(runtime.state.primary_view, PrimaryView::WorkspaceCreation);
        assert!(runtime.find("workspace-creation-title").is_some());
        assert!(runtime.find("workspace-kit-control").is_some());
        assert!(runtime.find("workspace-name-input").is_some());
        assert!(runtime.find("workspace-directory-input").is_some());
        assert!(runtime.find("workspace-guidance-input").is_some());
        assert!(runtime.find("workspace-research-subject-input").is_none());
        let (name, _) = runtime.presented("workspace-name-input").unwrap();
        let (directory, _) = runtime.presented("workspace-directory-input").unwrap();
        let (picker, _) = runtime.presented("workspace-directory-picker").unwrap();
        let (kit, _) = runtime.presented("workspace-kit-control").unwrap();
        let (guidance, _) = runtime.presented("workspace-guidance-row").unwrap();
        assert!(name[1] < directory[1] && directory[1] < kit[1]);
        assert!(picker[0] + picker[2] <= directory[0]);
        assert!(kit[1] < guidance[1]);
        assert!(runtime.find("workspace-guidance-form").is_none());
        click(&mut runtime, "workspace-directory-picker");
        assert_eq!(
            runtime.file_dialog().map(|(kind, _)| kind),
            Some(rune::FileDialogKind::Folder)
        );
        let selected_directory = std::env::temp_dir();
        runtime
            .resolve_file_dialog(Some(selected_directory.clone()))
            .unwrap();
        runtime.frame(1.0);
        assert_eq!(
            runtime.state.workspace_draft.directory.text,
            selected_directory.to_string_lossy()
        );
        click(&mut runtime, "workspace-directory-picker");
        assert_eq!(
            runtime
                .file_dialog()
                .and_then(|(_, options)| options.directory.as_ref()),
            Some(&selected_directory)
        );
        runtime.resolve_file_dialog(None).unwrap();
        runtime.frame(1.0);
        assert_eq!(
            runtime.state.workspace_draft.directory.text,
            selected_directory.to_string_lossy()
        );
        let kits = selection_menu_items(&runtime.state, SelectionMenu::ResearchKit);
        assert_eq!(kits.len(), 1);
        assert_eq!(kits[0].label, "None");
        click(&mut runtime, "workspace-kit-control");
        assert!(runtime.find("selection-menu-item-0").is_some());
        assert!(runtime.find("selection-menu-item-1").is_none());
        click(&mut runtime, "selection-menu-item-0");
        assert!(runtime.state.selection_menu.is_none());
        click(&mut runtime, "workspace-next");
        assert_eq!(
            runtime.state.workspace_draft.tab,
            workspace_creation::CreationTab::Settings
        );
        assert!(runtime.find("workspace-creation-error").is_some());
        runtime.state.workspace_draft.name = TextEdit::new("Example Workspace");
        runtime.frame(0.0);
        click(&mut runtime, "workspace-next");
        assert_eq!(
            runtime.state.workspace_draft.tab,
            workspace_creation::CreationTab::Resources
        );
        runtime.state.workspace_draft.resource_value = TextEdit::new("example.test");
        runtime.state.workspace_draft.resource_kind = workspace_creation::ResourceKind::Domain;
        runtime.frame(0.0);
        click(&mut runtime, "workspace-add-resource");
        assert_eq!(runtime.state.workspace_draft.resources.len(), 1);
        click(&mut runtime, "workspace-next");
        assert_eq!(
            runtime.state.workspace_draft.tab,
            workspace_creation::CreationTab::Rules
        );
        assert!(runtime.find("workspace-create-disabled").is_some());
        runtime.state.workspace_draft.rule_value = TextEdit::new("Use only authorized scope.");
        runtime.frame(0.0);
        click(&mut runtime, "workspace-add-rule");
        assert_eq!(runtime.state.workspace_draft.rules.len(), 1);
        click(&mut runtime, "workspace-cancel");
        assert_eq!(runtime.state.primary_view, PrimaryView::Home);
        assert!(runtime.find("workspace-creation-page").is_none());
        assert!(runtime.state.workspace_draft.name.text.is_empty());
    }

    #[test]
    fn header_history_moves_between_views_and_clears_forward_on_branch() {
        fn click(runtime: &mut Runtime<State>, key: &str) {
            let (x, y) = runtime.find(key).unwrap();
            runtime.pointer_down(x, y);
            runtime.pointer_up(x, y);
            runtime.frame(1.0);
        }

        let mut runtime =
            Runtime::new(State::default(), view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        assert!(runtime.find("header-back").is_some());
        assert!(runtime.find("header-forward").is_some());
        click(&mut runtime, "header-back");
        assert!(runtime.state.view_history.back.is_empty());

        click(&mut runtime, "provider-xai");
        assert_eq!(
            runtime.state.view_location(),
            ViewLocation::Providers(Provider::Xai)
        );
        click(&mut runtime, "settings-appearance");
        assert_eq!(
            runtime.state.view_location(),
            ViewLocation::Appearance(AppearanceTab::Colors)
        );
        click(&mut runtime, "appearance-tab-typography");
        assert_eq!(
            runtime.state.view_location(),
            ViewLocation::Appearance(AppearanceTab::Typography)
        );
        assert_eq!(runtime.state.view_history.back.len(), 3);

        click(&mut runtime, "header-back");
        assert_eq!(
            runtime.state.view_location(),
            ViewLocation::Appearance(AppearanceTab::Colors)
        );
        click(&mut runtime, "header-back");
        assert_eq!(
            runtime.state.view_location(),
            ViewLocation::Providers(Provider::Xai)
        );
        click(&mut runtime, "header-forward");
        assert_eq!(
            runtime.state.view_location(),
            ViewLocation::Appearance(AppearanceTab::Colors)
        );
        click(&mut runtime, "settings-general");
        assert!(runtime.state.view_history.forward.is_empty());
        click(&mut runtime, "header-forward");
        assert_eq!(
            runtime.state.view_location(),
            ViewLocation::Settings(SettingsSection::General)
        );
        assert!(runtime.state.view_history.forward.is_empty());
    }

    #[test]
    fn header_toggles_navigation_without_application_menus() {
        let mut runtime =
            Runtime::new(State::default(), view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        for key in ["menu-file", "menu-view", "menu-window", "header-popup"] {
            assert!(runtime.find(key).is_none(), "{key} is removed");
        }
        let (x, y) = runtime.find("header-sidebar-toggle").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert!(!runtime.state.sidenav_visible);
        assert!(runtime.find("settings-sidenav").is_none());
        assert!(runtime.find("header-divider").is_some());
        if cfg!(target_os = "windows") {
            assert!(runtime.find("header-close").is_some());
        }
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
        assert_eq!(runtime.state.sidenav_width, 352.0);
        aligned(&runtime);
        runtime.pointer_move(x + 300.0, y);
        runtime.frame(0.0);
        assert_eq!(runtime.state.sidenav_width, SIDENAV_MAX_WIDTH);
        aligned(&runtime);
        runtime.pointer_move(x - 300.0, y);
        runtime.frame(0.0);
        assert_eq!(runtime.state.sidenav_width, SIDENAV_MIN_WIDTH);
        aligned(&runtime);
        runtime.pointer_up(x - 300.0, y);
        runtime.frame(0.0);

        let (x, y) = runtime.find("header-sidebar-toggle").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        let (leading, _) = runtime.presented("header-leading").unwrap();
        let (divider, _) = runtime.presented("header-divider").unwrap();
        assert!(divider[0] >= leading[0] + leading[2] + 8.0);
        assert!(runtime.find("settings-sidenav-resize").is_none());

        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert_eq!(runtime.state.sidenav_width, SIDENAV_MIN_WIDTH);
        aligned(&runtime);
    }

    #[test]
    fn settings_sidenav_switches_sections_and_returns_to_providers() {
        let mut runtime =
            Runtime::new(State::default(), view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        assert!(runtime.find("settings-profile").is_none());
        assert!(runtime.find("settings-prompt").is_none());
        assert!(runtime.find("settings-features").is_none());
        let (providers_title, _) = runtime.presented("settings-view-title").unwrap();
        let (tabs, _) = runtime.presented("provider-view-tabs").unwrap();
        let (provider_header, _) = runtime.presented("provider-view-header").unwrap();
        assert!(providers_title[1] + providers_title[3] < tabs[1]);
        let gap_above_tabs = tabs[1] - (providers_title[1] + providers_title[3]);
        let gap_below_tabs = provider_header[1] - (tabs[1] + tabs[3]);
        assert!((gap_below_tabs - gap_above_tabs - 6.0).abs() < 1.0);
        let (x, y) = runtime.find("settings-appearance").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert_eq!(runtime.state.settings_section, SettingsSection::Appearance);
        let (appearance_title, _) = runtime.presented("settings-view-title").unwrap();
        assert_eq!(appearance_title[0], providers_title[0]);
        assert_eq!(appearance_title[1], providers_title[1]);
        assert!(runtime.find("provider-openai").is_none());
        let (x, y) = runtime.find("settings-providers").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert_eq!(runtime.state.settings_section, SettingsSection::Providers);
        assert!(runtime.find("provider-openai").is_some());
    }
}
