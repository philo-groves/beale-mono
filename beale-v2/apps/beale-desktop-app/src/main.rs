use rune::tok::TextStyle;
use rune::{
    Align, AppSpec, Color, Cursor, Cx, Drag, El, Icon, Key, KeyEvent, Phase, ScrollSpec, Side,
    TextEdit, Tx, col, el, icon, row, text, text_input,
};
use std::time::{Duration, Instant};

mod appearance;
mod archive_view;
mod automations_view;
mod browser_logos;
mod color_picker;
mod connection;
mod desktop_fonts;
mod extensions_view;
mod fleet_connection;
mod fleet_view;
mod form_controls;
mod home_sidebar;
mod icons;
mod in_place_menu;
mod input_history;
mod modal_dialog;
mod new_research;
mod permission_settings;
mod provider_auth;
mod provider_custom;
mod provider_icons;
mod provider_models;
mod provider_usage;
mod quick_chat;
mod sessions;
mod settings_scroll;
mod workspace_creation;
mod workspace_view;
mod workspaces;
use appearance::{
    AppearancePreferences, AppearanceTheme, HeatLevel, InterfaceFont, PaletteColor, TextSize,
    ThemeStyle,
};
use beale_contracts::v1::{
    ArchiveEntry, AuthMethod, DaybreakMode, ProviderId, SessionDetail, SessionStreamEvent,
    SessionStreamKind, SessionSummary, SessionToolDetailPage, SubmitSessionTurnRequest,
    UpdateWorkspaceFoundationRequest, UpdateWorkspaceRulesRequest, UpdateWorkspaceSurfacesRequest,
    WorkspaceDetails, WorkspaceFoundation, WorkspaceRules, WorkspaceSummary, WorkspaceSurface,
    WorkspaceSurfaceInput, WorkspaceSurfaces,
};
use color_picker::Hsv;
use connection::ConnectionStatus;
use icons::*;
use provider_auth::{Action, AuthStatus, SignInPrompt};
use provider_icons::{ANTHROPIC, GROK, OPENAI, OPENROUTER, ZAI};
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

fn view_title_font() -> TextStyle {
    ui_font(27.0, 600.0)
}

fn form_label_font() -> TextStyle {
    ui_font(14.0, 550.0)
}

fn sidenav_heading(label: &'static str, theme: ThemeStyle<'_>) -> El {
    row().h(30.0).center_y().child(
        text(label)
            .font(ui_font(18.0, 500.0))
            .color(theme.color("#aaaaaa")),
    )
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
    Anthropic,
    Zai,
    OpenRouter,
    Custom,
}

impl Provider {
    const ALL: [Self; 5] = [
        Self::OpenAi,
        Self::Xai,
        Self::Anthropic,
        Self::Zai,
        Self::OpenRouter,
    ];

    fn from_id(id: ProviderId) -> Option<Self> {
        match id {
            ProviderId::Openai => Some(Self::OpenAi),
            ProviderId::Xai => Some(Self::Xai),
            ProviderId::Anthropic => Some(Self::Anthropic),
            ProviderId::Zai => Some(Self::Zai),
            ProviderId::Openrouter => Some(Self::OpenRouter),
            ProviderId::Custom => Some(Self::Custom),
            ProviderId::Unspecified => None,
        }
    }

    fn id(self) -> ProviderId {
        match self {
            Self::OpenAi => ProviderId::Openai,
            Self::Xai => ProviderId::Xai,
            Self::Anthropic => ProviderId::Anthropic,
            Self::Zai => ProviderId::Zai,
            Self::OpenRouter => ProviderId::Openrouter,
            Self::Custom => ProviderId::Custom,
        }
    }

    fn index(self) -> usize {
        match self {
            Self::OpenAi => 0,
            Self::Xai => 1,
            Self::Anthropic => 2,
            Self::Zai => 3,
            Self::OpenRouter => 4,
            Self::Custom => unreachable!("custom providers have dynamic identity"),
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::OpenAi => "OpenAI",
            Self::Xai => "xAI",
            Self::Anthropic => "Anthropic",
            Self::Zai => "Z.ai",
            Self::OpenRouter => "OpenRouter",
            Self::Custom => "Custom",
        }
    }

    fn logo(self) -> &'static Icon {
        match self {
            Self::OpenAi => &OPENAI,
            Self::Xai => &GROK,
            Self::Anthropic => &ANTHROPIC,
            Self::Zai => &ZAI,
            Self::OpenRouter => &OPENROUTER,
            Self::Custom => &OPENROUTER,
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::OpenAi => "Connect a ChatGPT subscription or use an OpenAI API key.",
            Self::Xai => "Connect a Grok subscription or use an xAI API key.",
            Self::Anthropic => "Use an Anthropic API key to access Claude models.",
            Self::Zai => "Use a Z.ai API key to access GLM models.",
            Self::OpenRouter => "Use an OpenRouter API key to access routed models.",
            Self::Custom => "Connect an OpenAI-compatible chat completions API.",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Xai => "xai",
            Self::Anthropic => "anthropic",
            Self::Zai => "zai",
            Self::OpenRouter => "openrouter",
            Self::Custom => "custom",
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
    GoalMode,
    SessionGraph,
    Repeat,
    ResearchWorkspace,
    ResearchWorkspaceHeading,
    QuickChatWorkspace,
    HomeCollection,
    FleetAdd,
    ResearchKit,
    SurfaceKind,
    WorkspaceSurfaceKind,
    ColorTheme,
    InterfaceFont,
    TextSize,
    PluginWorkspaces {
        index: usize,
    },
    FeatureWorkspaces {
        index: usize,
    },
    BrowserWorkspaces {
        index: usize,
    },
    VirtualMachineWorkspaces {
        index: usize,
    },
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
    #[default]
    Safety,
    Appearance,
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
        section: SettingsSection::Safety,
        key: "settings-safety",
        label: "Safety",
        icon: &SAFETY,
        render: safety_settings_view,
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
        render: archive_settings_view,
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
const NAVIGATION_DIVIDER_COLOR: &str = "#555555";
const SIDENAV_DEFAULT_WIDTH: f32 = 292.0;
const SIDENAV_MIN_WIDTH: f32 = 240.0;
const SIDENAV_MAX_WIDTH: f32 = 420.0;
const HEADER_COLLAPSED_PREFIX_WIDTH: f32 = if cfg!(target_os = "macos") {
    184.0
} else {
    106.0
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PrimaryView {
    Home,
    Automations,
    NewResearch,
    WorkspaceCreation,
    Workspace,
    Extensions,
    Fleet,
    Settings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ViewLocation {
    Home,
    Automations,
    NewResearch,
    WorkspaceCreation(workspace_creation::CreationTab),
    Workspace(uuid::Uuid, workspace_view::WorkspaceTab),
    Extensions(
        extensions_view::ExtensionSection,
        extensions_view::PluginTab,
        extensions_view::FeatureTab,
    ),
    Fleet(fleet_view::FleetSection, fleet_view::FleetTab),
    Providers(Provider),
    Appearance(AppearanceTab),
    Safety(permission_settings::PermissionTab),
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
    automations: automations_view::AutomationState,
    home_collection: home_sidebar::HomeCollection,
    expanded_session_workspaces: std::collections::HashSet<String>,
    new_research_prompt: TextEdit,
    session_input_history: input_history::InputHistory,
    new_research_workspace_id: Option<String>,
    new_research_model: Option<new_research::ModelSelection>,
    new_research_effort: new_research::Effort,
    new_research_repeat: new_research::RepeatPreset,
    new_research_goal_mode: bool,
    new_research_goal_max_turns: TextEdit,
    new_research_goal_max_duration_minutes: TextEdit,
    new_research_graph_id: String,
    new_research_graph_models: [Option<Vec<new_research::ModelSelection>>; 4],
    new_research_graph_concurrency: [Option<u32>; 4],
    graph_editor: Option<new_research::GraphEditor>,
    graph_editor_max_concurrent: TextEdit,
    quick_chat_open: bool,
    quick_chat_draft: TextEdit,
    quick_chat_input_history: input_history::InputHistory,
    quick_chat_id: Option<String>,
    quick_chat_title: String,
    quick_chat_workspace_id: Option<String>,
    quick_chat_workspace_initialized: bool,
    quick_chat_messages: Vec<beale_contracts::v1::QuickChatMessage>,
    quick_chat_pending: Option<String>,
    quick_chat_busy: bool,
    quick_chat_error: String,
    quick_chat_generation: u64,
    session_summaries: Vec<SessionSummary>,
    hovered_session_id: Option<String>,
    hovered_workspace_id: Option<String>,
    archive_session_id: Option<String>,
    archive_session_busy: bool,
    archive_session_error: String,
    archive_tab: archive_view::ArchiveTab,
    archive_entries: Vec<ArchiveEntry>,
    archive_loaded: bool,
    archive_loading: bool,
    archive_error: String,
    archive_generation: u64,
    archive_restore_busy: Option<String>,
    active_session: Option<SessionDetail>,
    session_loading_id: Option<String>,
    session_busy: bool,
    session_error: String,
    session_pending_prompt: Option<String>,
    session_generation: u64,
    session_live_events: Vec<SessionStreamEvent>,
    session_elapsed_ms: u64,
    session_work_expanded: std::collections::HashSet<usize>,
    expanded_tool_id: Option<String>,
    tool_detail: Option<SessionToolDetailPage>,
    tool_detail_busy: bool,
    tool_detail_error: String,
    tool_detail_generation: u64,
    composer_menu_open: bool,
    composer_menu_section: new_research::MenuSection,
    composer_menu_cursor: usize,
    composer_menu_reveal_pending: bool,
    workspace_draft: workspace_creation::WorkspaceDraft,
    workspaces: Vec<WorkspaceSummary>,
    workspace_list_loaded: bool,
    workspace_list_error: String,
    pending_workspace_id: Option<String>,
    workspace_create_busy: bool,
    workspace_create_error: String,
    workspace_id: Option<uuid::Uuid>,
    workspace_tab: workspace_view::WorkspaceTab,
    workspace_details: Option<WorkspaceDetails>,
    workspace_details_loading: bool,
    workspace_details_error: String,
    workspace_details_generation: u64,
    workspace_foundation_revision: u64,
    workspace_surfaces_revision: u64,
    workspace_rules_revision: u64,
    workspace_name_edit: TextEdit,
    workspace_guidance_edit: TextEdit,
    workspace_surface_edits: Vec<WorkspaceSurface>,
    workspace_rule_edits: Vec<String>,
    workspace_rule_input: TextEdit,
    workspace_rule_dialog_open: bool,
    workspace_rule_dialog_error: String,
    workspace_surface_kind: workspace_creation::SurfaceKind,
    workspace_surface_value: TextEdit,
    workspace_surface_dialog_scope: Option<workspace_creation::SurfaceScope>,
    workspace_surface_edit_index: Option<usize>,
    workspace_surface_delete_index: Option<usize>,
    workspace_surface_error: String,
    workspace_save_busy: Option<workspace_view::WorkspaceTab>,
    workspace_save_error: String,
    workspace_save_message: String,
    selected: Provider,
    custom_providers: Vec<beale_contracts::v1::CustomProvider>,
    custom_error: String,
    custom_busy: bool,
    custom_editor_open: bool,
    custom_edit_id: Option<String>,
    custom_name_edit: TextEdit,
    custom_url_edit: TextEdit,
    custom_models_edit: TextEdit,
    custom_discovered_models: Vec<beale_contracts::v1::ModelEntry>,
    custom_lead_edit: TextEdit,
    custom_small_edit: TextEdit,
    custom_key_edit: TextEdit,
    custom_reasoning: bool,
    custom_delete_id: Option<String>,
    extensions_section: extensions_view::ExtensionSection,
    fleet_section: fleet_view::FleetSection,
    fleet: fleet_view::FleetState,
    plugin_tab: extensions_view::PluginTab,
    feature_tab: extensions_view::FeatureTab,
    plugins: Vec<extensions_view::PluginEntry>,
    plugin_loading: bool,
    plugin_error: String,
    plugin_installing: Option<String>,
    plugin_uninstalling: Option<String>,
    plugin_generation: u64,
    features: Vec<extensions_view::FeatureEntry>,
    feature_loading: bool,
    feature_busy: bool,
    feature_error: String,
    feature_generation: u64,
    extension_scope_saving: Option<SelectionMenu>,
    extension_scope_generation: u64,
    feature_details_id: Option<String>,
    feature_details: Option<beale_contracts::v1::FeatureDetailsResponse>,
    feature_details_loading: bool,
    feature_details_error: String,
    feature_details_generation: u64,
    plugin_add_dialog_open: bool,
    plugin_source_edit: TextEdit,
    plugin_manifest_name: Option<String>,
    plugin_manifest_text: String,
    plugin_manifest_loading: bool,
    plugin_manifest_error: String,
    plugin_manifest_generation: u64,
    settings_section: SettingsSection,
    appearance_tab: AppearanceTab,
    permissions_tab: permission_settings::PermissionTab,
    permission_rules: Vec<beale_contracts::v1::PermissionRule>,
    permission_loaded: bool,
    permission_loading: bool,
    permission_error: String,
    permission_saving: Option<String>,
    permission_edit_id: Option<String>,
    permission_creating: bool,
    permission_name_edit: TextEdit,
    permission_description_edit: TextEdit,
    permission_edit_error: String,
    permission_generation: u64,
    view_history: ViewHistory,
    appearance: AppearancePreferences,
    appearance_path: Option<std::path::PathBuf>,
    appearance_error: String,
    heat_edits: [TextEdit; 4],
    palette_edits: [TextEdit; 14],
    color_picker: Option<ColorPickerState>,
    connection: ConnectionStatus,
    auth_status: [AuthStatus; 5],
    provider_usage: [Option<beale_contracts::v1::ProviderUsageSnapshot>; 2],
    usage_generation: u64,
    auth_loaded: bool,
    lead_provider: Option<Provider>,
    lead_custom_provider_id: String,
    lead_busy: bool,
    lead_error: String,
    native_window_border: bool,
    auth_message: String,
    auth_message_is_error: bool,
    sign_in_prompt: Option<SignInPrompt>,
    api_key_edit: TextEdit,
    api_key_dialog: bool,
    auth_busy: bool,
    source_saving: [bool; 5],
    source_errors: [String; 5],
    daybreak_mode: DaybreakMode,
    daybreak_saving: bool,
    daybreak_error: String,
    model_catalogs: [Option<ModelCatalog>; 10],
    model_loading: [bool; 10],
    model_errors: [String; 10],
    model_saving: bool,
    selection_menu: Option<SelectionMenu>,
    menu_cursor: usize,
    menu_reveal_pending: bool,
    notice: Option<&'static str>,
    rail_tooltip: Option<(&'static str, &'static str)>,
    rail_usage_tooltip: Option<Provider>,
    sidenav_visible: bool,
    sidenav_width: f32,
    settings_scroll: SettingsScroll,
    workspace_scroll: SettingsScroll,
}

impl Default for State {
    fn default() -> Self {
        Self {
            primary_view: PrimaryView::Settings,
            automations: automations_view::AutomationState::default(),
            home_collection: home_sidebar::HomeCollection::default(),
            expanded_session_workspaces: std::collections::HashSet::new(),
            new_research_prompt: TextEdit::new(""),
            session_input_history: input_history::InputHistory::default(),
            new_research_workspace_id: None,
            new_research_model: None,
            new_research_effort: new_research::Effort::default(),
            new_research_repeat: new_research::RepeatPreset::default(),
            new_research_goal_mode: false,
            new_research_goal_max_turns: TextEdit::new(""),
            new_research_goal_max_duration_minutes: TextEdit::new(""),
            new_research_graph_id: beale_graphs::SIMPLE_ID.to_owned(),
            new_research_graph_models: std::array::from_fn(|_| None),
            new_research_graph_concurrency: std::array::from_fn(|index| {
                beale_graphs::SIMPLE_GRAPH.nodes[index].default_max_concurrent()
            }),
            graph_editor: None,
            graph_editor_max_concurrent: TextEdit::new(""),
            quick_chat_open: false,
            quick_chat_draft: TextEdit::new(""),
            quick_chat_input_history: input_history::InputHistory::default(),
            quick_chat_id: None,
            quick_chat_title: "Quick Chat".to_owned(),
            quick_chat_workspace_id: None,
            quick_chat_workspace_initialized: false,
            quick_chat_messages: Vec::new(),
            quick_chat_pending: None,
            quick_chat_busy: false,
            quick_chat_error: String::new(),
            quick_chat_generation: 0,
            session_summaries: Vec::new(),
            hovered_session_id: None,
            hovered_workspace_id: None,
            archive_session_id: None,
            archive_session_busy: false,
            archive_session_error: String::new(),
            archive_tab: archive_view::ArchiveTab::default(),
            archive_entries: Vec::new(),
            archive_loaded: false,
            archive_loading: false,
            archive_error: String::new(),
            archive_generation: 0,
            archive_restore_busy: None,
            active_session: None,
            session_loading_id: None,
            session_busy: false,
            session_error: String::new(),
            session_pending_prompt: None,
            session_generation: 0,
            session_live_events: Vec::new(),
            session_elapsed_ms: 0,
            session_work_expanded: std::collections::HashSet::new(),
            expanded_tool_id: None,
            tool_detail: None,
            tool_detail_busy: false,
            tool_detail_error: String::new(),
            tool_detail_generation: 0,
            composer_menu_open: false,
            composer_menu_section: new_research::MenuSection::Model,
            composer_menu_cursor: 0,
            composer_menu_reveal_pending: false,
            workspace_draft: workspace_creation::WorkspaceDraft::default(),
            workspaces: Vec::new(),
            workspace_list_loaded: false,
            workspace_list_error: String::new(),
            pending_workspace_id: None,
            workspace_create_busy: false,
            workspace_create_error: String::new(),
            workspace_id: None,
            workspace_tab: workspace_view::WorkspaceTab::default(),
            workspace_details: None,
            workspace_details_loading: false,
            workspace_details_error: String::new(),
            workspace_details_generation: 0,
            workspace_foundation_revision: 0,
            workspace_surfaces_revision: 0,
            workspace_rules_revision: 0,
            workspace_name_edit: TextEdit::new(""),
            workspace_guidance_edit: TextEdit::new(""),
            workspace_surface_edits: Vec::new(),
            workspace_rule_edits: Vec::new(),
            workspace_rule_input: TextEdit::new(""),
            workspace_rule_dialog_open: false,
            workspace_rule_dialog_error: String::new(),
            workspace_surface_kind: workspace_creation::SurfaceKind::default(),
            workspace_surface_value: TextEdit::new(""),
            workspace_surface_dialog_scope: None,
            workspace_surface_edit_index: None,
            workspace_surface_delete_index: None,
            workspace_surface_error: String::new(),
            workspace_save_busy: None,
            workspace_save_error: String::new(),
            workspace_save_message: String::new(),
            selected: Provider::OpenAi,
            custom_providers: Vec::new(),
            custom_error: String::new(),
            custom_busy: false,
            custom_editor_open: false,
            custom_edit_id: None,
            custom_name_edit: TextEdit::new(""),
            custom_url_edit: TextEdit::new(""),
            custom_models_edit: TextEdit::new(""),
            custom_discovered_models: Vec::new(),
            custom_lead_edit: TextEdit::new(""),
            custom_small_edit: TextEdit::new(""),
            custom_key_edit: TextEdit::new(""),
            custom_reasoning: false,
            custom_delete_id: None,
            extensions_section: extensions_view::ExtensionSection::default(),
            fleet_section: fleet_view::FleetSection::default(),
            fleet: fleet_view::FleetState::default(),
            plugin_tab: extensions_view::PluginTab::default(),
            feature_tab: extensions_view::FeatureTab::default(),
            plugins: extensions_view::bundled_plugins(),
            plugin_loading: false,
            plugin_error: String::new(),
            plugin_installing: None,
            plugin_uninstalling: None,
            plugin_generation: 0,
            features: extensions_view::bundled_features(),
            feature_loading: false,
            feature_busy: false,
            feature_error: String::new(),
            feature_generation: 0,
            extension_scope_saving: None,
            extension_scope_generation: 0,
            feature_details_id: None,
            feature_details: None,
            feature_details_loading: false,
            feature_details_error: String::new(),
            feature_details_generation: 0,
            plugin_add_dialog_open: false,
            plugin_source_edit: TextEdit::new(""),
            plugin_manifest_name: None,
            plugin_manifest_text: String::new(),
            plugin_manifest_loading: false,
            plugin_manifest_error: String::new(),
            plugin_manifest_generation: 0,
            settings_section: SettingsSection::Safety,
            appearance_tab: AppearanceTab::Colors,
            permissions_tab: permission_settings::PermissionTab::AutoReview,
            permission_rules: Vec::new(),
            permission_loaded: false,
            permission_loading: false,
            permission_error: String::new(),
            permission_saving: None,
            permission_edit_id: None,
            permission_creating: false,
            permission_name_edit: TextEdit::new(""),
            permission_description_edit: TextEdit::new(""),
            permission_edit_error: String::new(),
            permission_generation: 0,
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
            provider_usage: Default::default(),
            usage_generation: 0,
            auth_loaded: false,
            lead_provider: None,
            lead_custom_provider_id: String::new(),
            lead_busy: false,
            lead_error: String::new(),
            native_window_border: false,
            auth_message: String::new(),
            auth_message_is_error: false,
            sign_in_prompt: None,
            api_key_edit: TextEdit::new("").secure(),
            api_key_dialog: false,
            auth_busy: false,
            source_saving: [false; 5],
            source_errors: std::array::from_fn(|_| String::new()),
            daybreak_mode: DaybreakMode::Blue,
            daybreak_saving: false,
            daybreak_error: String::new(),
            model_catalogs: std::array::from_fn(|_| None),
            model_loading: [false; 10],
            model_errors: std::array::from_fn(|_| String::new()),
            model_saving: false,
            selection_menu: None,
            menu_cursor: 0,
            menu_reveal_pending: false,
            notice: None,
            rail_tooltip: None,
            rail_usage_tooltip: None,
            sidenav_visible: true,
            sidenav_width: SIDENAV_DEFAULT_WIDTH,
            settings_scroll: SettingsScroll::default(),
            workspace_scroll: SettingsScroll::for_key("workspace-scroll"),
        }
    }
}

impl State {
    fn open_quick_chat(&mut self) {
        if !self.quick_chat_workspace_initialized {
            let selected = match self.primary_view {
                PrimaryView::Workspace => self.workspace_id.map(|id| id.to_string()),
                PrimaryView::NewResearch => self
                    .selected_research_workspace()
                    .map(|workspace| workspace.id.clone()),
                _ => None,
            };
            self.quick_chat_workspace_id = selected;
            self.quick_chat_workspace_initialized = true;
        }
        self.quick_chat_open = true;
    }

    fn close_quick_chat(&mut self) {
        let was_open = self.quick_chat_open;
        self.quick_chat_open = false;
        self.quick_chat_generation = self.quick_chat_generation.wrapping_add(1);
        self.quick_chat_id = None;
        self.quick_chat_title = "Quick Chat".to_owned();
        self.quick_chat_workspace_id = None;
        self.quick_chat_workspace_initialized = false;
        self.quick_chat_messages.clear();
        self.quick_chat_pending = None;
        self.quick_chat_busy = false;
        self.quick_chat_error.clear();
        self.quick_chat_draft.clear();
        self.quick_chat_input_history.reset();
        if self.selection_menu == Some(SelectionMenu::QuickChatWorkspace) {
            self.selection_menu = None;
        }
        if was_open
            && self.primary_view == PrimaryView::Settings
            && self.settings_section == SettingsSection::Archive
            && matches!(self.connection, ConnectionStatus::Connected { .. })
        {
            start_archive_load(self);
        }
    }

    fn load_appearance() -> Self {
        let mut state = Self {
            primary_view: PrimaryView::Home,
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
        if self.primary_view == PrimaryView::NewResearch {
            return ViewLocation::NewResearch;
        }
        if self.primary_view == PrimaryView::Automations {
            return ViewLocation::Automations;
        }
        if self.primary_view == PrimaryView::WorkspaceCreation {
            return ViewLocation::WorkspaceCreation(self.workspace_draft.tab);
        }
        if self.primary_view == PrimaryView::Workspace
            && let Some(id) = self.workspace_id
        {
            return ViewLocation::Workspace(id, self.workspace_tab);
        }
        if self.primary_view == PrimaryView::Extensions {
            return ViewLocation::Extensions(
                self.extensions_section,
                self.plugin_tab,
                self.feature_tab,
            );
        }
        if self.primary_view == PrimaryView::Fleet {
            return ViewLocation::Fleet(self.fleet_section, self.fleet.tab);
        }
        match self.settings_section {
            SettingsSection::Providers => ViewLocation::Providers(self.selected),
            SettingsSection::Appearance => ViewLocation::Appearance(self.appearance_tab),
            SettingsSection::Safety => ViewLocation::Safety(self.permissions_tab),
            section => ViewLocation::Settings(section),
        }
    }

    fn apply_view_location(&mut self, location: ViewLocation) {
        let mut load_workspace = None;
        match location {
            ViewLocation::Home => self.primary_view = PrimaryView::Home,
            ViewLocation::Automations => {
                self.primary_view = PrimaryView::Automations;
                self.automations.loaded = false;
            }
            ViewLocation::NewResearch => self.primary_view = PrimaryView::NewResearch,
            ViewLocation::WorkspaceCreation(tab) => {
                self.primary_view = PrimaryView::WorkspaceCreation;
                self.workspace_draft.tab = tab;
            }
            ViewLocation::Workspace(id, tab) => {
                self.primary_view = PrimaryView::Workspace;
                self.workspace_tab = tab;
                if self.workspace_id != Some(id) {
                    self.workspace_id = Some(id);
                    self.workspace_details = None;
                    load_workspace = Some(id);
                }
            }
            ViewLocation::Extensions(section, plugin_tab, feature_tab) => {
                self.primary_view = PrimaryView::Extensions;
                self.extensions_section = section;
                self.plugin_tab = plugin_tab;
                self.feature_tab = feature_tab;
            }
            ViewLocation::Fleet(section, tab) => {
                self.primary_view = if extensions_view::fleet_enabled(self) {
                    PrimaryView::Fleet
                } else {
                    PrimaryView::Home
                };
                if self.primary_view == PrimaryView::Fleet {
                    self.fleet_section = section;
                    self.fleet.select_tab(section, tab);
                }
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
            ViewLocation::Safety(tab) => {
                self.primary_view = PrimaryView::Settings;
                self.settings_section = SettingsSection::Safety;
                self.permissions_tab = tab;
            }
            ViewLocation::Settings(section) => {
                self.primary_view = PrimaryView::Settings;
                self.settings_section = section;
            }
        }
        self.notice = None;
        self.plugin_add_dialog_open = false;
        self.plugin_source_edit.clear();
        self.plugin_manifest_name = None;
        self.plugin_manifest_text.clear();
        self.plugin_manifest_loading = false;
        self.plugin_manifest_error.clear();
        self.plugin_manifest_generation = self.plugin_manifest_generation.wrapping_add(1);
        self.feature_details_id = None;
        self.feature_details = None;
        self.feature_details_loading = false;
        self.feature_details_error.clear();
        self.feature_details_generation = self.feature_details_generation.wrapping_add(1);
        self.selection_menu = None;
        self.composer_menu_open = false;
        self.color_picker = None;
        self.permission_edit_id = None;
        self.permission_creating = false;
        self.permission_name_edit.clear();
        self.permission_description_edit.clear();
        self.permission_edit_error.clear();
        self.sign_in_prompt = None;
        self.api_key_dialog = false;
        self.api_key_edit.clear();
        self.auth_message.clear();
        self.workspace_surface_dialog_scope = None;
        self.workspace_surface_edit_index = None;
        self.workspace_surface_delete_index = None;
        self.workspace_surface_value.clear();
        self.workspace_surface_error.clear();
        self.workspace_rule_dialog_open = false;
        self.workspace_rule_dialog_error.clear();
        self.workspace_rule_input.clear();
        self.workspace_save_error.clear();
        self.workspace_save_message.clear();
        if let Some(id) = load_workspace {
            start_workspace_details(self, id);
        }
        if self.primary_view == PrimaryView::Settings
            && self.settings_section == SettingsSection::Archive
        {
            start_archive_load(self);
        }
        if self.primary_view == PrimaryView::Settings
            && self.settings_section == SettingsSection::Safety
        {
            permission_settings::start_load(self);
        }
        if self.primary_view == PrimaryView::Extensions {
            extensions_view::start_load(self);
        }
        if self.primary_view == PrimaryView::Fleet {
            fleet_view::start_load(self);
        }
        if self.primary_view == PrimaryView::Automations {
            automations_view::start_load(self);
        }
    }

    fn navigate_to(&mut self, location: ViewLocation) {
        if (self.workspace_create_busy && self.primary_view == PrimaryView::WorkspaceCreation)
            || (self.workspace_save_busy.is_some() && self.primary_view == PrimaryView::Workspace)
        {
            return;
        }
        let current = self.view_location();
        if current != location {
            ViewHistory::push(&mut self.view_history.back, current);
            self.view_history.forward.clear();
        }
        self.apply_view_location(location);
    }

    fn show_section(&mut self, section: SettingsSection) {
        let location = match section {
            SettingsSection::Providers => ViewLocation::Providers(Provider::Anthropic),
            SettingsSection::Appearance => ViewLocation::Appearance(self.appearance_tab),
            SettingsSection::Safety => ViewLocation::Safety(self.permissions_tab),
            section => ViewLocation::Settings(section),
        };
        self.navigate_to(location);
    }

    fn start_workspace_creation(&mut self) {
        self.workspace_draft = workspace_creation::WorkspaceDraft::default();
        self.workspace_create_error.clear();
        self.workspace_scroll = SettingsScroll::for_key("workspace-scroll");
        self.navigate_to(ViewLocation::WorkspaceCreation(
            workspace_creation::CreationTab::Foundation,
        ));
    }

    fn start_new_research(&mut self) {
        self.session_input_history.reset();
        self.clear_tool_detail();
        self.session_live_events.clear();
        self.session_elapsed_ms = 0;
        self.session_work_expanded.clear();
        let had_session = self.active_session.take().is_some();
        let was_loading = self.session_loading_id.take().is_some();
        if had_session || was_loading {
            self.session_generation = self.session_generation.wrapping_add(1);
            self.session_busy = false;
            self.session_pending_prompt = None;
            self.session_error.clear();
            self.new_research_prompt.clear();
            self.new_research_goal_mode = false;
            self.new_research_goal_max_turns.clear();
            self.new_research_goal_max_duration_minutes.clear();
        }
        self.new_research_graph_id = beale_graphs::SIMPLE_ID.to_owned();
        self.new_research_graph_models = std::array::from_fn(|_| None);
        self.new_research_graph_concurrency = std::array::from_fn(|index| {
            beale_graphs::SIMPLE_GRAPH.nodes[index].default_max_concurrent()
        });
        self.graph_editor = None;
        self.graph_editor_max_concurrent.clear();
        self.new_research_workspace_id = self
            .selected_research_workspace()
            .map(|item| item.id.clone());
        self.navigate_to(ViewLocation::NewResearch);
    }

    fn start_new_research_in_workspace(&mut self, workspace_id: String) {
        self.start_new_research();
        self.new_research_workspace_id = Some(workspace_id);
    }

    fn finish_session_turn(&mut self, generation: u64, result: Result<SessionDetail, String>) {
        if self.session_generation != generation {
            return;
        }
        self.session_busy = false;
        match result {
            Ok(detail) => {
                self.clear_tool_detail();
                self.session_live_events.clear();
                if let Some(summary) = &detail.summary {
                    self.session_summaries.retain(|item| item.id != summary.id);
                    self.session_summaries.insert(0, summary.clone());
                    self.new_research_workspace_id = Some(summary.workspace_id.clone());
                }
                self.active_session = Some(detail);
                self.session_pending_prompt = None;
                self.session_error.clear();
            }
            Err(error) => {
                self.session_error = error;
                if let Some(prompt) = self.session_pending_prompt.take() {
                    self.new_research_prompt = TextEdit::new(prompt);
                }
            }
        }
    }

    fn finish_session_load(&mut self, generation: u64, result: Result<SessionDetail, String>) {
        if self.session_generation != generation {
            return;
        }
        self.session_loading_id = None;
        self.session_busy = false;
        self.session_live_events.clear();
        self.session_work_expanded.clear();
        match result {
            Ok(detail) => {
                self.clear_tool_detail();
                if let Some(summary) = &detail.summary {
                    self.new_research_workspace_id = Some(summary.workspace_id.clone());
                }
                if let (Ok(provider), Ok(method)) = (
                    ProviderId::try_from(detail.provider),
                    AuthMethod::try_from(detail.method),
                ) && let Some(provider) = Provider::from_id(provider)
                {
                    self.new_research_model = Some(new_research::ModelSelection {
                        provider,
                        method,
                        id: detail.model.clone(),
                    });
                }
                self.active_session = Some(detail);
                self.new_research_prompt.clear();
                self.session_error.clear();
            }
            Err(error) => self.session_error = error,
        }
    }

    fn apply_session_progress(&mut self, generation: u64, event: SessionStreamEvent) {
        if self.session_generation != generation || !self.session_busy {
            return;
        }
        match SessionStreamKind::try_from(event.kind) {
            Ok(SessionStreamKind::MessageDelta | SessionStreamKind::ReasoningDelta) => {
                if let Some(existing) = self.session_live_events.iter_mut().rev().find(|item| {
                    item.kind == event.kind && item.segment_index == event.segment_index
                }) {
                    if existing.text.len().saturating_add(event.text.len()) <= 2_097_152 {
                        existing.text.push_str(&event.text);
                    }
                } else {
                    self.session_live_events.push(event);
                }
            }
            Ok(SessionStreamKind::ToolStarted) => self.session_live_events.push(event),
            Ok(SessionStreamKind::ToolFinished) => {
                if let Some(tool) = event.tool
                    && let Some(existing) =
                        self.session_live_events.iter_mut().rev().find(|item| {
                            item.tool.as_ref().is_some_and(|prior| prior.id == tool.id)
                        })
                {
                    existing.kind = SessionStreamKind::ToolFinished as i32;
                    existing.tool = Some(tool);
                }
            }
            Ok(SessionStreamKind::TurnCompleted) => {
                if let Some(detail) = event.detail {
                    self.clear_tool_detail();
                    self.session_live_events.clear();
                    self.session_elapsed_ms = 0;
                    self.session_pending_prompt = None;
                    if let Some(summary) = &detail.summary {
                        self.session_summaries.retain(|item| item.id != summary.id);
                        self.session_summaries.insert(0, summary.clone());
                        self.new_research_workspace_id = Some(summary.workspace_id.clone());
                    }
                    self.active_session = Some(detail);
                }
            }
            _ => {}
        }
    }

    fn clear_tool_detail(&mut self) {
        self.expanded_tool_id = None;
        self.tool_detail = None;
        self.tool_detail_busy = false;
        self.tool_detail_error.clear();
        self.tool_detail_generation = self.tool_detail_generation.wrapping_add(1);
    }

    fn finish_tool_detail(
        &mut self,
        generation: u64,
        result: Result<SessionToolDetailPage, String>,
    ) {
        if self.tool_detail_generation != generation {
            return;
        }
        self.tool_detail_busy = false;
        match result {
            Ok(page) if self.expanded_tool_id.as_deref() == Some(&page.id) => {
                self.tool_detail = Some(page);
                self.tool_detail_error.clear();
            }
            Ok(_) => {}
            Err(error) => self.tool_detail_error = error,
        }
    }

    fn selected_research_workspace(&self) -> Option<&WorkspaceSummary> {
        self.new_research_workspace_id
            .as_deref()
            .and_then(|id| self.workspaces.iter().find(|item| item.id == id))
            .or_else(|| {
                self.workspace_id.and_then(|id| {
                    self.workspaces
                        .iter()
                        .find(|item| item.id == id.to_string())
                })
            })
            .or_else(|| self.workspaces.first())
    }

    fn open_workspace(&mut self, id: uuid::Uuid) {
        self.navigate_to(ViewLocation::Workspace(
            id,
            workspace_view::WorkspaceTab::Campaign,
        ));
    }

    fn cancel_workspace_creation(&mut self) {
        if self.workspace_create_busy {
            return;
        }
        self.leave_workspace_creation();
    }

    fn leave_workspace_creation(&mut self) {
        self.workspace_draft = workspace_creation::WorkspaceDraft::default();
        self.workspace_create_error.clear();
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
        if (self.workspace_create_busy && self.primary_view == PrimaryView::WorkspaceCreation)
            || (self.workspace_save_busy.is_some() && self.primary_view == PrimaryView::Workspace)
        {
            return;
        }
        if let Some(location) = self.view_history.back.pop() {
            let current = self.view_location();
            ViewHistory::push(&mut self.view_history.forward, current);
            self.apply_view_location(location);
        }
    }

    fn go_forward(&mut self) {
        if (self.workspace_create_busy && self.primary_view == PrimaryView::WorkspaceCreation)
            || (self.workspace_save_busy.is_some() && self.primary_view == PrimaryView::Workspace)
        {
            return;
        }
        if let Some(location) = self.view_history.forward.pop() {
            let current = self.view_location();
            ViewHistory::push(&mut self.view_history.back, current);
            self.apply_view_location(location);
        }
    }

    fn finish_workspace_create(&mut self, result: Result<WorkspaceSummary, String>) {
        self.workspace_create_busy = false;
        match result {
            Ok(summary) => {
                self.pending_workspace_id = Some(summary.id.clone());
                if !self.workspaces.iter().any(|item| item.id == summary.id) {
                    self.workspaces.push(summary);
                }
                self.workspace_list_loaded = true;
                self.workspace_list_error.clear();
                if self.primary_view == PrimaryView::WorkspaceCreation {
                    self.leave_workspace_creation();
                }
            }
            Err(error) => self.workspace_create_error = error,
        }
    }

    fn apply_workspace_list(&mut self, result: Result<Vec<WorkspaceSummary>, String>) -> bool {
        if self.workspace_create_busy {
            return false;
        }
        match result {
            Ok(workspaces) => {
                if self
                    .pending_workspace_id
                    .as_ref()
                    .is_some_and(|id| !workspaces.iter().any(|workspace| &workspace.id == id))
                {
                    return false;
                }
                self.pending_workspace_id = None;
                if self.workspace_list_loaded
                    && self.workspace_list_error.is_empty()
                    && self.workspaces == workspaces
                {
                    return false;
                }
                self.workspaces = workspaces;
                if self.primary_view == PrimaryView::NewResearch {
                    self.new_research_workspace_id = self
                        .selected_research_workspace()
                        .map(|item| item.id.clone());
                }
                self.workspace_list_loaded = true;
                self.workspace_list_error.clear();
            }
            Err(error) => {
                if !self.workspace_list_loaded && self.workspace_list_error == error {
                    return false;
                }
                self.workspace_list_error = error;
            }
        }
        true
    }

    fn finish_workspace_details(
        &mut self,
        id: uuid::Uuid,
        generation: u64,
        result: Result<workspaces::WorkspaceSections, String>,
    ) {
        if self.workspace_id != Some(id) || self.workspace_details_generation != generation {
            return;
        }
        self.workspace_details_loading = false;
        match result {
            Ok(sections) => {
                let _ = sections.campaign.enabled_feature_ids;
                self.workspace_foundation_revision = sections.foundation_revision;
                self.workspace_surfaces_revision = sections.surfaces_revision;
                self.workspace_rules_revision = sections.rules_revision;
                self.workspace_name_edit = TextEdit::new(
                    sections
                        .details
                        .summary
                        .as_ref()
                        .map_or("", |summary| &summary.name),
                );
                self.workspace_guidance_edit = TextEdit::new(&sections.details.guidance);
                self.workspace_surface_edits = sections.details.surfaces.clone();
                self.workspace_rule_edits = sections.details.rules.clone();
                if let Some(summary) = &sections.details.summary
                    && let Some(item) = self
                        .workspaces
                        .iter_mut()
                        .find(|item| item.id == summary.id)
                {
                    *item = summary.clone();
                }
                self.workspace_details = Some(sections.details);
                self.workspace_details_error.clear();
                self.workspace_save_error.clear();
                self.workspace_save_message.clear();
            }
            Err(error) => self.workspace_details_error = error,
        }
    }

    fn finish_workspace_save(
        &mut self,
        id: uuid::Uuid,
        tab: workspace_view::WorkspaceTab,
        result: Result<WorkspaceSaveResult, String>,
    ) {
        if self.workspace_id != Some(id) || self.workspace_save_busy != Some(tab) {
            return;
        }
        self.workspace_save_busy = None;
        match result {
            Ok(WorkspaceSaveResult::Foundation(saved)) => {
                self.workspace_foundation_revision = saved.revision;
                if let Some(details) = &mut self.workspace_details {
                    details.summary = saved.summary.clone();
                    details.guidance = saved.guidance.clone();
                }
                if let Some(summary) = saved.summary {
                    if let Some(item) = self
                        .workspaces
                        .iter_mut()
                        .find(|item| item.id == summary.id)
                    {
                        *item = summary.clone();
                    }
                    self.workspace_name_edit = TextEdit::new(summary.name);
                }
                self.workspace_guidance_edit = TextEdit::new(saved.guidance);
                self.workspace_save_message = "Foundation saved.".to_owned();
            }
            Ok(WorkspaceSaveResult::Surfaces(saved)) => {
                self.workspace_surfaces_revision = saved.revision;
                self.workspace_surface_edits = saved.surfaces.clone();
                if let Some(details) = &mut self.workspace_details {
                    details.surfaces = saved.surfaces;
                }
                self.workspace_save_message = "Surfaces saved.".to_owned();
            }
            Ok(WorkspaceSaveResult::Rules(saved)) => {
                self.workspace_rules_revision = saved.revision;
                self.workspace_rule_edits = saved.rules.clone();
                if let Some(details) = &mut self.workspace_details {
                    details.rules = saved.rules;
                }
                self.workspace_save_message = "Rules saved.".to_owned();
            }
            Err(error) => self.workspace_save_error = error,
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

pub(crate) const MAX_GOAL_DURATION_MINUTES: u64 = i64::MAX as u64 / 60_000;

fn parse_goal_limit(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() {
        Some(0)
    } else {
        value
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| value.parse::<u64>().ok())
            .flatten()
            .filter(|limit| *limit > 0 && *limit <= i64::MAX as u64)
    }
}

fn start_session_turn(state: &mut State) {
    if state.session_busy {
        return;
    }
    if state.workspaces.is_empty() {
        state.session_error = "Create a workspace before sending.".to_owned();
        return;
    }
    let prompt = state.new_research_prompt.text.trim().to_owned();
    if prompt.is_empty() {
        return;
    }
    let Some(workspace_id) = state
        .selected_research_workspace()
        .map(|item| item.id.clone())
    else {
        state.session_error = "Choose a workspace before sending.".to_owned();
        return;
    };
    let Some(model) = new_research::selected_model(state) else {
        state.session_error = "Choose an available model before sending.".to_owned();
        return;
    };
    if !matches!(state.connection, ConnectionStatus::Connected { .. }) {
        state.session_error = "App-server connection is not ready.".to_owned();
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        state.session_error = "Desktop connection is not ready.".to_owned();
        return;
    };
    let session_id = state
        .active_session
        .as_ref()
        .and_then(|detail| detail.summary.as_ref())
        .filter(|summary| summary.workspace_id == workspace_id)
        .map_or_else(String::new, |summary| summary.id.clone());
    let goal_mode = state.new_research_goal_mode
        && extensions_view::goal_mode_enabled(state)
        && session_id.is_empty();
    let (goal_max_turns, goal_max_duration_minutes) = if goal_mode {
        let Some(turns) = parse_goal_limit(&state.new_research_goal_max_turns.text) else {
            state.session_error = "Max turns must be a positive whole number.".to_owned();
            return;
        };
        let Some(minutes) = parse_goal_limit(&state.new_research_goal_max_duration_minutes.text)
            .filter(|minutes| *minutes <= MAX_GOAL_DURATION_MINUTES)
        else {
            state.session_error =
                "Max duration must be a positive whole number of minutes.".to_owned();
            return;
        };
        (turns, minutes)
    } else {
        (0, 0)
    };
    let repeat_preset =
        if session_id.is_empty() && state.new_research_repeat != new_research::RepeatPreset::None {
            state.new_research_repeat.key().to_owned()
        } else {
            String::new()
        };
    let graph_id = if session_id.is_empty() && extensions_view::graphs_enabled(state) {
        state.new_research_graph_id.clone()
    } else {
        String::new()
    };
    let graph_models = if graph_id == beale_graphs::SIMPLE_ID {
        new_research::graph_model_config(state)
    } else {
        Vec::new()
    };
    let graph_node_concurrency = if graph_id == beale_graphs::SIMPLE_ID {
        new_research::graph_concurrency_config(state)
    } else {
        Vec::new()
    };
    let request = SubmitSessionTurnRequest {
        workspace_id,
        session_id,
        provider: model.provider.id() as i32,
        method: model.method as i32,
        model: model.id,
        prompt: prompt.clone(),
        reasoning_effort: state.new_research_effort.wire() as i32,
        goal_mode,
        goal_max_turns,
        goal_max_duration_minutes,
        repeat_preset,
        graph_id,
        graph_models,
        graph_node_concurrency,
    };
    state.session_generation = state.session_generation.wrapping_add(1);
    let generation = state.session_generation;
    state.session_busy = true;
    state.session_error.clear();
    state.session_live_events.clear();
    state.session_elapsed_ms = 0;
    state.session_pending_prompt = Some(prompt);
    state.new_research_prompt.clear();
    state.session_input_history.reset();
    let timer_running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let timer_handle = handle.clone();
    let timer_flag = timer_running.clone();
    std::thread::spawn(move || {
        let started = Instant::now();
        while timer_flag.load(std::sync::atomic::Ordering::Relaxed) {
            std::thread::sleep(Duration::from_secs(1));
            if !timer_flag.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            let elapsed = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
            timer_handle.update(move |state, _| {
                if state.session_generation == generation && state.session_busy {
                    state.session_elapsed_ms = elapsed;
                }
            });
        }
    });
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid local app-server address".to_owned())?;
            let client = sessions::stream_client()?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize session connection".to_owned())?;
            let progress_handle = handle.clone();
            runtime.block_on(sessions::submit_stream(
                &client,
                addr,
                request,
                move |event| {
                    progress_handle
                        .update(move |state, _| state.apply_session_progress(generation, event));
                },
            ))
        })();
        timer_running.store(false, std::sync::atomic::Ordering::Relaxed);
        handle.update(move |state, _| state.finish_session_turn(generation, result));
    });
}

fn start_session_load(state: &mut State, id: String) {
    if !matches!(state.connection, ConnectionStatus::Connected { .. }) {
        state.session_error = "App-server connection is not ready.".to_owned();
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        state.session_error = "Desktop connection is not ready.".to_owned();
        return;
    };
    state.session_generation = state.session_generation.wrapping_add(1);
    let generation = state.session_generation;
    state.session_loading_id = Some(id.clone());
    state.session_busy = true;
    state.session_error.clear();
    state.session_pending_prompt = None;
    state.new_research_workspace_id = state
        .session_summaries
        .iter()
        .find(|summary| summary.id == id)
        .map(|summary| summary.workspace_id.clone());
    state.active_session = None;
    state.new_research_prompt.clear();
    state.session_input_history.reset();
    state.navigate_to(ViewLocation::NewResearch);
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid local app-server address".to_owned())?;
            let client = sessions::client()?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize session connection".to_owned())?;
            runtime.block_on(sessions::load(&client, addr, &id))
        })();
        handle.update(move |state, _| state.finish_session_load(generation, result));
    });
}

fn start_session_archive(state: &mut State) {
    let Some(id) = state.archive_session_id.clone() else {
        return;
    };
    if state.archive_session_busy || state.session_busy {
        return;
    }
    if !matches!(state.connection, ConnectionStatus::Connected { .. }) {
        state.archive_session_error = "App-server connection is not ready.".to_owned();
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        state.archive_session_error = "Desktop connection is not ready.".to_owned();
        return;
    };
    state.archive_session_busy = true;
    state.archive_session_error.clear();
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid local app-server address".to_owned())?;
            let client = sessions::client()?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize session connection".to_owned())?;
            runtime.block_on(sessions::archive(&client, addr, &id))
        })();
        handle.update(move |state, _| {
            if state.archive_session_id.as_deref() != Some(id.as_str()) {
                return;
            }
            state.archive_session_busy = false;
            match result {
                Ok(()) => {
                    state.session_summaries.retain(|summary| summary.id != id);
                    if state
                        .active_session
                        .as_ref()
                        .and_then(|detail| detail.summary.as_ref())
                        .is_some_and(|summary| summary.id == id)
                    {
                        state.session_generation = state.session_generation.wrapping_add(1);
                        state.active_session = None;
                        state.clear_tool_detail();
                        state.navigate_to(ViewLocation::Home);
                    }
                    state.archive_session_id = None;
                    state.archive_session_error.clear();
                }
                Err(error) => state.archive_session_error = error,
            }
        });
    });
}

fn start_archive_load(state: &mut State) {
    if state.archive_loading {
        return;
    }
    if !matches!(state.connection, ConnectionStatus::Connected { .. }) {
        state.archive_error = "App-server connection is not ready.".to_owned();
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        state.archive_error = "Desktop connection is not ready.".to_owned();
        return;
    };
    state.archive_generation = state.archive_generation.wrapping_add(1);
    let generation = state.archive_generation;
    state.archive_loading = true;
    state.archive_error.clear();
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid local app-server address".to_owned())?;
            let client = sessions::client()?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize archive connection".to_owned())?;
            runtime.block_on(archive_view::list(&client, addr))
        })();
        handle.update(move |state, _| {
            if state.archive_generation != generation {
                return;
            }
            state.archive_loading = false;
            match result {
                Ok(entries) => {
                    state.archive_entries = entries;
                    state.archive_loaded = true;
                    state.archive_error.clear();
                }
                Err(error) => state.archive_error = error,
            }
        });
    });
}

fn start_archive_restore(state: &mut State, id: String) {
    if state.archive_restore_busy.is_some() {
        return;
    }
    let Some(entry) = state
        .archive_entries
        .iter()
        .find(|entry| entry.id == id && !entry.quick_chat)
    else {
        return;
    };
    let summary = SessionSummary {
        id: entry.id.clone(),
        workspace_id: entry.workspace_id.clone(),
        title: entry.title.clone(),
        last_message_at_ms: entry.last_message_at_ms,
    };
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        state.archive_error = "Desktop connection is not ready.".to_owned();
        return;
    };
    state.archive_restore_busy = Some(id.clone());
    state.archive_error.clear();
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid local app-server address".to_owned())?;
            let client = sessions::client()?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize archive connection".to_owned())?;
            runtime.block_on(archive_view::restore_session(&client, addr, &id))
        })();
        handle.update(move |state, _| {
            if state.archive_restore_busy.as_deref() != Some(id.as_str()) {
                return;
            }
            state.archive_restore_busy = None;
            match result {
                Ok(()) => {
                    state.archive_entries.retain(|entry| entry.id != id);
                    if !state.session_summaries.iter().any(|item| item.id == id) {
                        state.session_summaries.push(summary);
                    }
                    state.archive_error.clear();
                }
                Err(error) => state.archive_error = error,
            }
        });
    });
}

fn start_quick_chat_restore(state: &mut State, id: String) {
    if state.archive_restore_busy.is_some()
        || !state
            .archive_entries
            .iter()
            .any(|entry| entry.id == id && entry.quick_chat)
    {
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        state.archive_error = "Desktop connection is not ready.".to_owned();
        return;
    };
    state.archive_restore_busy = Some(id.clone());
    state.archive_error.clear();
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid local app-server address".to_owned())?;
            let client = sessions::client()?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize archive connection".to_owned())?;
            runtime.block_on(archive_view::load_quick_chat(&client, addr, &id))
        })();
        handle.update(move |state, _| {
            if state.archive_restore_busy.as_deref() != Some(id.as_str()) {
                return;
            }
            state.archive_restore_busy = None;
            match result {
                Ok(detail) => {
                    state.close_quick_chat();
                    state.quick_chat_id = Some(detail.chat_id);
                    state.quick_chat_title = detail.title;
                    state.quick_chat_workspace_id =
                        (!detail.workspace_id.is_empty()).then_some(detail.workspace_id);
                    state.quick_chat_workspace_initialized = true;
                    state.quick_chat_messages = detail.messages;
                    state.quick_chat_open = true;
                    state.archive_error.clear();
                }
                Err(error) => state.archive_error = error,
            }
        });
    });
}

fn toggle_tool_detail(state: &mut State, id: String) {
    if state.expanded_tool_id.as_deref() == Some(&id) {
        state.clear_tool_detail();
        return;
    }
    state.clear_tool_detail();
    state.expanded_tool_id = Some(id.clone());
    load_tool_detail_page(state, id, 0);
}

fn load_tool_detail_page(state: &mut State, id: String, offset: u64) {
    if state.tool_detail_busy {
        return;
    }
    let Some(session_id) = state
        .active_session
        .as_ref()
        .and_then(|detail| detail.summary.as_ref())
        .map(|summary| summary.id.clone())
    else {
        return;
    };
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        return;
    };
    state.tool_detail_busy = true;
    state.tool_detail_error.clear();
    state.tool_detail_generation = state.tool_detail_generation.wrapping_add(1);
    let generation = state.tool_detail_generation;
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid local app-server address".to_owned())?;
            let client = sessions::client()?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize session connection".to_owned())?;
            runtime.block_on(sessions::load_tool_page(
                &client,
                addr,
                &session_id,
                &id,
                offset,
            ))
        })();
        handle.update(move |state, _| state.finish_tool_detail(generation, result));
    });
}

enum WorkspaceSaveResult {
    Foundation(WorkspaceFoundation),
    Surfaces(WorkspaceSurfaces),
    Rules(WorkspaceRules),
}

fn start_workspace_save(state: &mut State, tab: workspace_view::WorkspaceTab) {
    if state.workspace_save_busy.is_some()
        || state.workspace_details_loading
        || state.workspace_details.is_none()
    {
        return;
    }
    let Some(id) = state.workspace_id else {
        return;
    };
    if tab == workspace_view::WorkspaceTab::Campaign {
        return;
    }
    if tab == workspace_view::WorkspaceTab::Foundation
        && state.workspace_name_edit.text.trim().is_empty()
    {
        state.workspace_save_error = "Workspace name is required.".to_owned();
        return;
    }
    if tab == workspace_view::WorkspaceTab::Surfaces
        && !state
            .workspace_surface_edits
            .iter()
            .any(|surface| surface.scope == beale_contracts::v1::SurfaceScope::InScope as i32)
    {
        state.workspace_save_error = "Add at least one in-scope surface.".to_owned();
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        state.workspace_save_error = "Desktop connection is not ready.".to_owned();
        return;
    };
    let foundation = (tab == workspace_view::WorkspaceTab::Foundation).then(|| {
        UpdateWorkspaceFoundationRequest {
            expected_revision: state.workspace_foundation_revision,
            name: state.workspace_name_edit.text.clone(),
            guidance: state.workspace_guidance_edit.text.clone(),
        }
    });
    let surfaces =
        (tab == workspace_view::WorkspaceTab::Surfaces).then(|| UpdateWorkspaceSurfacesRequest {
            expected_revision: state.workspace_surfaces_revision,
            surfaces: state
                .workspace_surface_edits
                .iter()
                .map(|surface| WorkspaceSurfaceInput {
                    kind: surface.kind,
                    scope: surface.scope,
                    value: surface.value.clone(),
                })
                .collect(),
        });
    let rules = (tab == workspace_view::WorkspaceTab::Rules).then(|| UpdateWorkspaceRulesRequest {
        expected_revision: state.workspace_rules_revision,
        rules: state.workspace_rule_edits.clone(),
    });
    state.workspace_save_busy = Some(tab);
    state.workspace_save_error.clear();
    state.workspace_save_message.clear();
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid local app-server address".to_owned())?;
            let client = workspaces::client()?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize workspace connection".to_owned())?;
            runtime.block_on(async {
                if let Some(request) = foundation {
                    workspaces::save_foundation(&client, addr, id, request)
                        .await
                        .map(WorkspaceSaveResult::Foundation)
                } else if let Some(request) = surfaces {
                    workspaces::save_surfaces(&client, addr, id, request)
                        .await
                        .map(WorkspaceSaveResult::Surfaces)
                } else if let Some(request) = rules {
                    workspaces::save_rules(&client, addr, id, request)
                        .await
                        .map(WorkspaceSaveResult::Rules)
                } else {
                    Err("Invalid workspace section".to_owned())
                }
            })
        })();
        handle.update(move |state, _| state.finish_workspace_save(id, tab, result));
    });
}

fn start_workspace_details(state: &mut State, id: uuid::Uuid) {
    state.workspace_details_generation = state.workspace_details_generation.wrapping_add(1);
    let generation = state.workspace_details_generation;
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        state.workspace_details_error = "Desktop connection is not ready.".to_owned();
        return;
    };
    state.workspace_details_loading = true;
    state.workspace_details_error.clear();
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid local app-server address".to_owned())?;
            let client = workspaces::client()?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize workspace connection".to_owned())?;
            runtime.block_on(workspaces::load_sections(&client, addr, id))
        })();
        handle.update(move |state, _| state.finish_workspace_details(id, generation, result));
    });
}

fn start_workspace_create(state: &mut State) {
    if state.workspace_create_busy
        || !matches!(state.connection, ConnectionStatus::Connected { .. })
    {
        return;
    }
    let request = match workspaces::request_from_draft(&state.workspace_draft) {
        Ok(request) => request,
        Err(error) => {
            state.workspace_draft.error = Some(error);
            return;
        }
    };
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        state.workspace_create_error = "Desktop connection is not ready.".to_owned();
        return;
    };
    state.workspace_draft.error = None;
    state.workspace_create_error.clear();
    state.workspace_create_busy = true;
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid local app-server address".to_owned())?;
            let client = workspaces::client()?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize workspace connection".to_owned())?;
            runtime.block_on(workspaces::create(&client, addr, request))
        })();
        handle.update(move |state, _| state.finish_workspace_create(result));
    });
}

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
                    if let Some(provider) = prompt.external_provider {
                        match provider_auth::launch_official_sign_in(provider) {
                            Ok(message) => {
                                state.auth_message = message.to_owned();
                                state.auth_message_is_error = false;
                            }
                            Err(error) => {
                                state.auth_message = error;
                                state.auth_message_is_error = true;
                            }
                        }
                    } else if let Err(error) = tx.open_url(&prompt.url) {
                        state.auth_message = format!("Could not open browser: {error}");
                        state.auth_message_is_error = true;
                    } else {
                        state.auth_message = "Finish sign-in in your browser.".to_owned();
                        state.auth_message_is_error = false;
                    }
                    if prompt.external_provider.is_none() {
                        state.sign_in_prompt = Some(prompt);
                    }
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
    let requested_daybreak_mode = (provider == Provider::OpenAi).then_some(state.daybreak_mode);
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
            if requested_daybreak_mode.is_some_and(|mode| mode != state.daybreak_mode) {
                state.model_catalogs[index] = None;
                start_model_list(state, provider, method);
                return;
            }
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
            runtime.block_on(provider_auth::set_lead(&client, addr, provider.id(), ""))
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

fn start_custom_lead_save(state: &mut State, id: String) {
    if state.lead_busy
        || !state.auth_loaded
        || !state
            .custom_providers
            .iter()
            .any(|provider| provider.id == id)
    {
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
            runtime.block_on(provider_auth::set_lead(
                &client,
                addr,
                ProviderId::Custom,
                &id,
            ))
        })();
        handle.update(move |state, _| {
            state.lead_busy = false;
            match result {
                Ok(()) => {
                    state.lead_provider = Some(Provider::Custom);
                    state.lead_custom_provider_id = id;
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
                Ok(()) => {
                    state.daybreak_mode = mode;
                    for method in [AuthMethod::Subscription, AuthMethod::ApiKey] {
                        let index = catalog_index(Provider::OpenAi, method);
                        state.model_catalogs[index] = None;
                        let configured = match method {
                            AuthMethod::Subscription => {
                                state.auth_status[Provider::OpenAi.index()].subscription
                            }
                            AuthMethod::ApiKey => {
                                state.auth_status[Provider::OpenAi.index()].api_key
                            }
                            _ => false,
                        };
                        if configured {
                            start_model_list(state, Provider::OpenAi, method);
                        }
                    }
                }
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
        SelectionMenu::HomeCollection => home_sidebar::HomeCollection::ALL
            .into_iter()
            .map(|collection| in_place_menu::MenuItem {
                value: collection.key().to_owned(),
                label: collection.label().to_owned(),
                detail: None,
                selected: state.home_collection == collection,
            })
            .collect(),
        SelectionMenu::FleetAdd => [
            ("manual", "Manually Add", "Enter a Tailscale HTTPS URL"),
            (
                "tailscale",
                "Tailscale Import",
                "Choose a device on this tailnet",
            ),
        ]
        .into_iter()
        .map(|(value, label, detail)| in_place_menu::MenuItem {
            value: value.to_owned(),
            label: label.to_owned(),
            detail: Some(detail.to_owned()),
            selected: false,
        })
        .collect(),
        SelectionMenu::Repeat => new_research::RepeatPreset::ALL
            .into_iter()
            .map(|preset| in_place_menu::MenuItem {
                value: preset.key().to_owned(),
                label: preset.label().to_owned(),
                detail: None,
                selected: state.new_research_repeat == preset,
            })
            .collect(),
        SelectionMenu::GoalMode => vec![in_place_menu::MenuItem {
            value: "toggle".to_owned(),
            label: "Goal Mode".to_owned(),
            detail: None,
            selected: state.new_research_goal_mode,
        }],
        SelectionMenu::SessionGraph => [
            (beale_graphs::SIMPLE_ID, beale_graphs::SIMPLE_NAME),
            ("", "Independent"),
        ]
        .into_iter()
        .map(|(value, label)| in_place_menu::MenuItem {
            value: value.to_owned(),
            label: label.to_owned(),
            detail: None,
            selected: state.new_research_graph_id == value,
        })
        .collect(),
        SelectionMenu::ResearchWorkspace | SelectionMenu::ResearchWorkspaceHeading => {
            let selected_id = state
                .selected_research_workspace()
                .map(|item| item.id.as_str());
            state
                .workspaces
                .iter()
                .map(|workspace| in_place_menu::MenuItem {
                    value: workspace.id.clone(),
                    label: workspace.name.clone(),
                    detail: None,
                    selected: selected_id == Some(workspace.id.as_str()),
                })
                .collect()
        }
        SelectionMenu::QuickChatWorkspace => std::iter::once(in_place_menu::MenuItem {
            value: String::new(),
            label: "No Workspace".to_owned(),
            detail: None,
            selected: state.quick_chat_workspace_id.is_none(),
        })
        .chain(
            state
                .workspaces
                .iter()
                .map(|workspace| in_place_menu::MenuItem {
                    value: workspace.id.clone(),
                    label: workspace.name.clone(),
                    detail: None,
                    selected: state.quick_chat_workspace_id.as_deref()
                        == Some(workspace.id.as_str()),
                }),
        )
        .collect(),
        SelectionMenu::ResearchKit => vec![in_place_menu::MenuItem {
            value: "none".to_owned(),
            label: "None".to_owned(),
            detail: None,
            selected: true,
        }],
        SelectionMenu::SurfaceKind => workspace_creation::SurfaceKind::ALL
            .into_iter()
            .map(|kind| in_place_menu::MenuItem {
                value: kind.key().to_owned(),
                label: kind.label().to_owned(),
                detail: None,
                selected: state.workspace_draft.surface_kind == kind,
            })
            .collect(),
        SelectionMenu::WorkspaceSurfaceKind => workspace_creation::SurfaceKind::ALL
            .into_iter()
            .map(|kind| in_place_menu::MenuItem {
                value: kind.key().to_owned(),
                label: kind.label().to_owned(),
                detail: None,
                selected: state.workspace_surface_kind == kind,
            })
            .collect(),
        SelectionMenu::LeadProvider => {
            let mut options: Vec<_> = Provider::ALL
                .into_iter()
                .filter(|provider| {
                    let status = &state.auth_status[provider.index()];
                    status.subscription || status.api_key
                })
                .map(|provider| in_place_menu::MenuItem {
                    value: provider.key().to_owned(),
                    label: provider.name().to_owned(),
                    detail: None,
                    selected: state.lead_provider == Some(provider),
                })
                .collect();
            options.extend(
                state
                    .custom_providers
                    .iter()
                    .map(|provider| in_place_menu::MenuItem {
                        value: provider.id.clone(),
                        label: provider.name.clone(),
                        detail: Some("Custom API".to_owned()),
                        selected: state.lead_provider == Some(Provider::Custom)
                            && state.lead_custom_provider_id == provider.id,
                    }),
            );
            options
        }
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
        SelectionMenu::PluginWorkspaces { .. } | SelectionMenu::FeatureWorkspaces { .. } => {
            extensions_view::workspace_scope_items(state, menu)
        }
        SelectionMenu::BrowserWorkspaces { index } => {
            fleet_view::workspace_scope_items(state, index)
        }
        SelectionMenu::VirtualMachineWorkspaces { index } => {
            fleet_view::vm_workspace_scope_items(state, index)
        }
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

fn selection_menu_anchor(menu: SelectionMenu) -> String {
    match menu {
        SelectionMenu::PluginWorkspaces { index } => return format!("plugin-scope-{index}"),
        SelectionMenu::FeatureWorkspaces { index } => return format!("feature-scope-{index}"),
        SelectionMenu::BrowserWorkspaces { index } => {
            return format!("fleet-browser-scope-{index}");
        }
        SelectionMenu::VirtualMachineWorkspaces { index } => {
            return format!("fleet-vm-scope-{index}");
        }
        SelectionMenu::HomeCollection => "home-collection-control",
        SelectionMenu::FleetAdd => "fleet-add-server",
        SelectionMenu::Repeat => "new-research-repeat-control",
        SelectionMenu::GoalMode => "new-research-goal-control",
        SelectionMenu::SessionGraph => "new-research-graph-control",
        SelectionMenu::ResearchWorkspace => "new-research-workspace-control",
        SelectionMenu::ResearchWorkspaceHeading => "new-research-heading-workspace-control",
        SelectionMenu::QuickChatWorkspace => "quick-chat-workspace-control",
        SelectionMenu::ResearchKit => "workspace-kit-control",
        SelectionMenu::SurfaceKind => "workspace-surface-kind-control",
        SelectionMenu::WorkspaceSurfaceKind => "saved-workspace-surface-kind-control",
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
    .to_owned()
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
    if matches!(
        menu,
        SelectionMenu::GoalMode
            | SelectionMenu::SessionGraph
            | SelectionMenu::Repeat
            | SelectionMenu::ResearchWorkspace
            | SelectionMenu::ResearchWorkspaceHeading
            | SelectionMenu::QuickChatWorkspace
            | SelectionMenu::HomeCollection
    ) {
        state.composer_menu_open = false;
    }
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
    if !matches!(
        menu,
        SelectionMenu::PluginWorkspaces { .. }
            | SelectionMenu::FeatureWorkspaces { .. }
            | SelectionMenu::BrowserWorkspaces { .. }
            | SelectionMenu::VirtualMachineWorkspaces { .. }
    ) {
        state.selection_menu = None;
    }
    match menu {
        SelectionMenu::HomeCollection => {
            if let Some(collection) = home_sidebar::HomeCollection::ALL
                .into_iter()
                .find(|collection| collection.key() == value)
            {
                state.home_collection = collection;
            }
        }
        SelectionMenu::FleetAdd => match value.as_str() {
            "manual" => fleet_view::start_manual_add(state),
            "tailscale" => fleet_view::start_import(state),
            _ => {}
        },
        SelectionMenu::Repeat => {
            if let Some(preset) = new_research::RepeatPreset::ALL
                .into_iter()
                .find(|preset| preset.key() == value)
            {
                state.new_research_repeat = preset;
            }
        }
        SelectionMenu::GoalMode => state.new_research_goal_mode = !state.new_research_goal_mode,
        SelectionMenu::SessionGraph => state.new_research_graph_id = value,
        SelectionMenu::ResearchWorkspace | SelectionMenu::ResearchWorkspaceHeading => {
            state.new_research_workspace_id = Some(value);
        }
        SelectionMenu::QuickChatWorkspace => {
            state.quick_chat_workspace_id = if value.is_empty() { None } else { Some(value) };
        }
        SelectionMenu::ResearchKit => {}
        SelectionMenu::SurfaceKind => {
            if let Some(kind) = workspace_creation::SurfaceKind::ALL
                .into_iter()
                .find(|kind| kind.key() == value)
            {
                state.workspace_draft.surface_kind = kind;
            }
        }
        SelectionMenu::WorkspaceSurfaceKind => {
            if let Some(kind) = workspace_creation::SurfaceKind::ALL
                .into_iter()
                .find(|kind| kind.key() == value)
            {
                state.workspace_surface_kind = kind;
            }
        }
        SelectionMenu::LeadProvider => {
            if let Some(provider) = Provider::ALL
                .into_iter()
                .find(|provider| provider.key() == value)
            {
                start_lead_save(state, provider);
            } else if state
                .custom_providers
                .iter()
                .any(|provider| provider.id == value)
            {
                start_custom_lead_save(state, value);
            }
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
        SelectionMenu::PluginWorkspaces { .. } | SelectionMenu::FeatureWorkspaces { .. } => {
            if let Some((all, ids)) = extensions_view::next_workspace_scope(state, menu, &value) {
                extensions_view::start_workspace_scope_save(state, menu, all, ids);
            }
        }
        SelectionMenu::BrowserWorkspaces { index } => {
            fleet_view::select_workspace_scope(state, index, &value);
        }
        SelectionMenu::VirtualMachineWorkspaces { index } => {
            fleet_view::select_vm_workspace_scope(state, index, &value);
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
    if state.composer_menu_open {
        return new_research::menu_key(state, event, tx);
    }
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
        .help(label)
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
        .help(label)
        .size(30.0, 34.0)
        .center()
        .radius(6.0)
        .child(icon(glyph, 16.0).color(color(if enabled { "#c8c8c8" } else { "#777777" })));
    if enabled {
        button
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
        .pl(8.0)
        .pr(if state.sidenav_visible { 8.0 } else { 0.0 });
    if cfg!(target_os = "macos") {
        left = left.child(el().w(76.0));
    }
    left = left.child(
        header_icon_button(
            "header-sidebar-toggle",
            if state.sidenav_visible {
                "Hide Sidebar"
            } else {
                "Show Sidebar"
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
                    .fill(color(NAVIGATION_DIVIDER_COLOR)),
            )
            .child(icon(state.settings_section.icon(), 16.0).color(color("#c8c8c8")))
            .child(
                text("Settings")
                    .font(ui_font(13.0, 500.0))
                    .color(color("#e2e2e2")),
            )
            .child(
                text(state.settings_section.label())
                    .font(ui_font(13.0, 400.0))
                    .color(color("#aaaaaa")),
            );
    } else if state.primary_view == PrimaryView::Extensions {
        title = title
            .child(
                el().key("header-divider")
                    .size(1.0, 18.0)
                    .fill(color(NAVIGATION_DIVIDER_COLOR)),
            )
            .child(icon(&EXTENSIONS, 16.0).color(color("#c8c8c8")))
            .child(
                text("Extensions")
                    .font(ui_font(13.0, 500.0))
                    .color(color("#e2e2e2")),
            )
            .child(
                text(state.extensions_section.label())
                    .font(ui_font(13.0, 400.0))
                    .color(color("#aaaaaa")),
            );
    } else if state.primary_view == PrimaryView::Fleet {
        title = title
            .child(
                el().key("header-divider")
                    .size(1.0, 18.0)
                    .fill(color(NAVIGATION_DIVIDER_COLOR)),
            )
            .child(icon(&FLEET, 16.0).color(color("#c8c8c8")))
            .child(
                text("Fleet")
                    .font(ui_font(13.0, 500.0))
                    .color(color("#e2e2e2")),
            )
            .child(
                text(state.fleet_section.label())
                    .font(ui_font(13.0, 400.0))
                    .color(color("#aaaaaa")),
            );
    } else if state.primary_view == PrimaryView::Automations {
        title = title
            .child(
                el().key("header-divider")
                    .size(1.0, 18.0)
                    .fill(color(NAVIGATION_DIVIDER_COLOR)),
            )
            .child(icon(&AUTOMATIONS, 16.0).color(color("#c8c8c8")))
            .child(
                text("Automations")
                    .font(ui_font(13.0, 500.0))
                    .color(color("#e2e2e2")),
            );
    } else if state.primary_view == PrimaryView::WorkspaceCreation {
        title = title
            .child(
                el().key("header-divider")
                    .size(1.0, 18.0)
                    .fill(color(NAVIGATION_DIVIDER_COLOR)),
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
    } else if state.primary_view == PrimaryView::NewResearch {
        title = title
            .child(
                el().key("header-divider")
                    .size(1.0, 18.0)
                    .fill(color(NAVIGATION_DIVIDER_COLOR)),
            )
            .child(icon(&NEW_RESEARCH, 16.0).color(color("#c8c8c8")))
            .child(
                text("New Research")
                    .font(ui_font(13.0, 500.0))
                    .color(color("#e2e2e2")),
            );
    } else if state.primary_view == PrimaryView::Workspace {
        let name = state
            .workspace_id
            .and_then(|id| {
                state
                    .workspaces
                    .iter()
                    .find(|item| item.id == id.to_string())
            })
            .map_or("Workspace", |item| item.name.as_str());
        title = title
            .child(
                el().key("header-divider")
                    .size(1.0, 18.0)
                    .fill(color(NAVIGATION_DIVIDER_COLOR)),
            )
            .child(icon(&FOLDER_OPEN, 16.0).color(color("#c8c8c8")))
            .child(
                text(name)
                    .font(ui_font(13.0, 500.0))
                    .color(color("#e2e2e2")),
            )
            .child(
                text(state.workspace_tab.label())
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
    let glyph = if active {
        match key {
            "nav-home" => &HOME_FILLED,
            "nav-automations" => &AUTOMATIONS_FILLED,
            "nav-extensions" => &EXTENSIONS_FILLED,
            "nav-settings" => &SETTINGS_FILLED,
            "nav-fleet" => &FLEET_FILLED,
            _ => glyph,
        }
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
        .on_hover_change(move |s: &mut State, inside, _| {
            if inside {
                s.rail_tooltip = Some((key, label));
            } else if s.rail_tooltip.is_some_and(|(active, _)| active == key) {
                s.rail_tooltip = None;
            }
        })
        .hover_delays(Duration::from_millis(350), Duration::ZERO)
        .child(icon(glyph, 20.0).color(if active {
            color("#f2f2f2")
        } else {
            color("#aaaaaa")
        }));
    if key == "nav-home" {
        button.on_press(|s: &mut State| {
            s.rail_tooltip = None;
            s.navigate_to(ViewLocation::Home);
        })
    } else if key == "nav-settings" {
        button.on_press(|s: &mut State| {
            s.rail_tooltip = None;
            s.show_section(s.settings_section);
        })
    } else if key == "nav-extensions" {
        button.on_press(|s: &mut State| {
            s.rail_tooltip = None;
            s.navigate_to(ViewLocation::Extensions(
                s.extensions_section,
                s.plugin_tab,
                s.feature_tab,
            ));
        })
    } else if key == "nav-fleet" {
        button.on_press(|s: &mut State| {
            s.rail_tooltip = None;
            s.navigate_to(ViewLocation::Fleet(s.fleet_section, s.fleet.tab));
        })
    } else if key == "nav-automations" {
        button.on_press(|s: &mut State| {
            s.rail_tooltip = None;
            s.navigate_to(ViewLocation::Automations);
        })
    } else if active {
        button.on_press(|s: &mut State| {
            s.rail_tooltip = None;
            s.notice = None;
        })
    } else {
        button.on_press(move |s: &mut State| {
            s.rail_tooltip = None;
            s.notice = Some(label);
        })
    }
}

fn rail(state: &State, theme: ThemeStyle<'_>) -> El {
    let mut rail = col()
        .key("navigation")
        .role("navigation")
        .label("Main navigation")
        .w(NAVIGATION_RAIL_WIDTH)
        .h_full()
        .pt(8.0)
        .gap(6.0)
        .items_center()
        .child(nav_button(
            "nav-home",
            "Home",
            &HOME,
            state.primary_view != PrimaryView::Settings
                && state.primary_view != PrimaryView::Extensions
                && state.primary_view != PrimaryView::Fleet
                && state.primary_view != PrimaryView::Automations,
            theme,
        ))
        .child(nav_button(
            "nav-automations",
            "Automations",
            &AUTOMATIONS,
            state.primary_view == PrimaryView::Automations,
            theme,
        ))
        .child(nav_button(
            "nav-extensions",
            "Extensions",
            &EXTENSIONS,
            state.primary_view == PrimaryView::Extensions,
            theme,
        ))
        .child(nav_button(
            "nav-settings",
            "Settings",
            &SETTINGS,
            state.primary_view == PrimaryView::Settings,
            theme,
        ));
    let conditional_icons = extensions_view::fleet_enabled(state).then_some((
        "nav-fleet",
        "Fleet",
        &FLEET,
        state.primary_view == PrimaryView::Fleet,
    ));
    if let Some((key, label, glyph, active)) = conditional_icons {
        rail = rail.child(
            el().key("navigation-conditional-divider")
                .w(26.0)
                .h(1.0)
                .fill(theme.color(NAVIGATION_DIVIDER_COLOR)),
        );
        rail = rail.child(nav_button(key, label, glyph, active, theme));
    }
    rail = rail.child(el().grow(1.0));
    for provider in [Provider::OpenAi, Provider::Xai] {
        if let Some(usage) = &state.provider_usage[provider.index()]
            && !usage.windows.is_empty()
        {
            rail = rail.child(provider_usage::indicator(provider, usage, theme));
        }
    }
    rail.pb(10.0)
}

fn rail_tooltip(state: &State, theme: ThemeStyle<'_>) -> Option<El> {
    let (anchor, label) = state.rail_tooltip?;
    Some(
        row()
            .key("rail-tooltip")
            .inert()
            .px(10.0)
            .py(6.0)
            .radius(8.0)
            .continuous_corners()
            .fill(theme.color("#292929"))
            .border(1.0, theme.color("#3a3a3a"))
            .shadow(5.0, 16.0, Color::BLACK.alpha(0.45))
            .child(
                text(label)
                    .font(ui_font(12.5, 450.0))
                    .color(theme.color("#eeeeee")),
            )
            .anchored_fit(anchor, Side::Right, Align::Center, 8.0),
    )
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
        .px(8.0)
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
                .font(ui_font(14.0, if active { 550.0 } else { 400.0 }))
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
        .label("Settings sections")
        .w(width)
        .h_full()
        .px(8.0)
        .pt(12.0)
        .fill(color("#111111"))
        .child(
            sidenav_heading("Settings", theme)
                .key("settings-sidenav-title")
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
        .key(format!("provider-{}", provider.key()))
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
        .any(|status| status.subscription || status.api_key)
        || !state.custom_providers.is_empty();
    let label = if state.lead_provider == Some(Provider::Custom) {
        state
            .custom_providers
            .iter()
            .find(|provider| provider.id == state.lead_custom_provider_id)
            .map_or("Custom", |provider| provider.name.as_str())
    } else {
        state.lead_provider.map_or("None", Provider::name)
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
    settings_form_with_action(spec, rows, None, theme)
}

fn settings_form_card(rows: impl IntoIterator<Item = El>, theme: ThemeStyle<'_>) -> El {
    let color = |value| theme.color(value);
    let mut items = col().w_full().px(16.0).radius(10.0).fill(color("#191919"));
    for (index, item) in rows.into_iter().enumerate() {
        if index > 0 {
            items = items.child(el().w_full().h(1.0).fill(color("#2c2c2c")));
        }
        items = items.child(item);
    }
    items
}

fn settings_form_with_action(
    spec: FormSpec,
    rows: impl IntoIterator<Item = El>,
    action: Option<El>,
    theme: ThemeStyle<'_>,
) -> El {
    let color = |value| theme.color(value);
    let heading = text(spec.heading)
        .font(ui_font(17.0, 600.0))
        .color(color("#f2f2f2"));
    let heading = if let Some(action) = action {
        row()
            .w_full()
            .center_y()
            .child(heading.grow(1.0))
            .child(action)
    } else {
        heading
    };
    col()
        .key(spec.key)
        .w_full()
        .gap(12.0)
        .child(heading)
        .child(settings_form_card(rows, theme))
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
    if state.selected == Provider::Custom {
        return provider_custom::render(state);
    }
    let theme = state.appearance.style();
    let color = |value| theme.color(value);
    let provider = state.selected;
    let auth = &state.auth_status[provider.index()];
    let name = provider.name();
    let title_row = row()
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
    let mut detail = col()
        .key(format!("provider-detail-{}", provider.key()))
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
            AUTH_ROWS
                .iter()
                .filter(|spec| {
                    provider != Provider::OpenRouter || spec.method != AuthMethod::Subscription
                })
                .map(|spec| {
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

fn providers_settings_view(state: &State, cx: &mut Cx<State>) -> Vec<El> {
    let theme = state.appearance.style();
    let color = |value| theme.color(value);
    let tabs_width = cx.layout_size("app-surface").map_or(1040.0, |size| size[0])
        - NAVIGATION_RAIL_WIDTH
        - if state.sidenav_visible {
            state.sidenav_width + 1.0
        } else {
            0.0
        }
        - 78.0;
    let providers = [
        Provider::Anthropic,
        Provider::OpenAi,
        Provider::OpenRouter,
        Provider::Xai,
        Provider::Zai,
        Provider::Custom,
    ];
    let tabs = if tabs_width < 530.0 {
        let mut rows = col().gap(6.0);
        for chunk in providers.chunks(3) {
            let mut tab_row = row().gap(6.0);
            for &provider in chunk {
                tab_row = tab_row.child(provider_tab(provider, state.selected == provider, theme));
            }
            rows = rows.child(tab_row);
        }
        rows
    } else {
        let mut tab_row = row().gap(6.0);
        for provider in providers {
            tab_row = tab_row.child(provider_tab(provider, state.selected == provider, theme));
        }
        col().child(tab_row)
    };
    let controls = if tabs_width < 780.0 {
        col()
            .key("provider-view-tabs")
            .role("tablist")
            .label("Provider views")
            .w_full()
            .gap(8.0)
            .child(tabs)
            .child(
                row()
                    .w_full()
                    .child(el().grow(1.0))
                    .child(lead_provider_controls(state)),
            )
    } else {
        col()
            .key("provider-view-tabs")
            .role("tablist")
            .label("Provider views")
            .w_full()
            .child(
                row()
                    .w_full()
                    .h(26.0)
                    .child(tabs)
                    .child(el().grow(1.0))
                    .child(lead_provider_controls(state)),
            )
    };
    let mut content = vec![controls];
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

fn archive_settings_view(state: &State, _cx: &mut Cx<State>) -> Vec<El> {
    archive_view::render(state, state.appearance.style())
}

fn safety_settings_view(state: &State, _cx: &mut Cx<State>) -> Vec<El> {
    permission_settings::render(state, state.appearance.style())
}

fn view(state: &State, cx: &mut Cx<State>) -> El {
    let _text_size_scope = RenderTextSizeScope::enter(state.appearance.text_size);
    let theme = state.appearance.style();
    let palette = theme.palette();
    let color = |value| theme.color(value);
    let mut content = col()
        .key("settings-page")
        .w_full()
        .max_w(if state.primary_view == PrimaryView::Settings {
            760.0
        } else {
            1300.0
        })
        .self_center()
        .gap(20.0);
    if state.primary_view == PrimaryView::Settings {
        content = content.child(
            text(state.settings_section.label())
                .key("settings-view-title")
                .font(view_title_font())
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
    } else if state.primary_view == PrimaryView::Extensions {
        content = content.child(
            text(state.extensions_section.label())
                .key("extensions-view-title")
                .font(view_title_font())
                .color(color("#f2f2f2")),
        );
        let page_width = cx
            .layout_size("content-surface")
            .map_or(1040.0, |size| (size[0] - 48.0).min(1300.0));
        for item in extensions_view::render(state, theme, page_width) {
            content = content.child(item);
        }
    } else if state.primary_view == PrimaryView::Fleet {
        let page_width = cx
            .layout_size("content-surface")
            .map_or(720.0, |size| size[0] - 48.0);
        for item in fleet_view::render(state, theme, page_width) {
            content = content.child(item);
        }
    } else if state.primary_view == PrimaryView::Automations {
        for item in automations_view::render(state, theme) {
            content = content.child(item);
        }
    } else if state.primary_view == PrimaryView::WorkspaceCreation {
        content = content.child(workspace_creation::render(state, theme));
    } else if state.primary_view == PrimaryView::Workspace {
        let campaign_height = cx
            .layout_size("content-surface")
            .map_or(420.0, |size| (size[1] - 180.0).max(300.0));
        content = content.child(workspace_view::render(state, theme, campaign_height));
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
            .child(match state.primary_view {
                PrimaryView::Settings => settings_sidenav(
                    state.settings_section,
                    &state.connection,
                    state.sidenav_width,
                    theme,
                ),
                PrimaryView::Extensions => {
                    extensions_view::sidenav(state, state.sidenav_width, theme)
                }
                PrimaryView::Fleet => fleet_view::sidenav(state, state.sidenav_width, theme),
                PrimaryView::Automations => {
                    automations_view::sidenav(state, state.sidenav_width, theme)
                }
                _ => home_sidebar::render(state, state.sidenav_width, theme),
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
    if state.primary_view == PrimaryView::NewResearch {
        let graph_width = cx
            .layout_size("content-surface")
            .map_or(640.0, |size| (size[0] - 80.0).max(180.0));
        main_content = main_content.child(new_research::render(state, theme, graph_width));
    } else if state.primary_view == PrimaryView::Home {
        main_content = main_content.child(
            el().key("home-no-workspace")
                .w_full()
                .grow(1.0)
                .center()
                .child(
                    text("No workspace is selected.")
                        .key("home-no-workspace-message")
                        .font(ui_font(14.0, 400.0))
                        .color(color("#969696")),
                ),
        );
    } else {
        let scroll_watch = if matches!(
            state.primary_view,
            PrimaryView::Settings
                | PrimaryView::Extensions
                | PrimaryView::Fleet
                | PrimaryView::Automations
        ) {
            state.settings_scroll.clone()
        } else {
            state.workspace_scroll.clone()
        };
        let scroll_key = if matches!(
            state.primary_view,
            PrimaryView::Settings
                | PrimaryView::Extensions
                | PrimaryView::Fleet
                | PrimaryView::Automations
        ) {
            "settings-scroll"
        } else {
            "workspace-scroll"
        };
        let scroll_bar = scroll_watch.clone();
        let content_inset = match state.primary_view {
            PrimaryView::Settings => col().w_full().padding(36.0),
            PrimaryView::Extensions => col().w_full().px(24.0).pt(36.0).pb(24.0),
            PrimaryView::Fleet => col().w_full().px(24.0).pt(36.0).pb(24.0),
            PrimaryView::Automations => col().w_full().px(24.0).pt(36.0).pb(24.0),
            PrimaryView::Workspace => col().w_full().px(24.0).pt(36.0).pb(24.0),
            PrimaryView::WorkspaceCreation => col().w_full().px(24.0).pt(36.0).pb(24.0),
            PrimaryView::Home | PrimaryView::NewResearch => col().w_full(),
        };
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
                        if matches!(
                            state.primary_view,
                            PrimaryView::Settings
                                | PrimaryView::Extensions
                                | PrimaryView::Fleet
                                | PrimaryView::Automations
                        ) {
                            state.settings_scroll.wheel(event, tx)
                        } else {
                            state.workspace_scroll.wheel(event, tx)
                        }
                    })
                    .child(content_inset.child(content)),
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
    if state.quick_chat_open {
        window = window.child(quick_chat::render(state, theme, cx));
    }
    if let Some(tooltip) = rail_tooltip(state, theme) {
        window = window.child(tooltip);
    }
    if let Some(provider) = state.rail_usage_tooltip
        && let Some(usage) = &state.provider_usage[provider.index()]
        && !usage.windows.is_empty()
    {
        window = window.child(provider_usage::tooltip(provider, usage, theme));
    }
    if state.sidenav_visible
        && state.home_collection == home_sidebar::HomeCollection::Workspaces
        && !matches!(
            state.primary_view,
            PrimaryView::Settings | PrimaryView::Extensions | PrimaryView::Fleet
        )
        && let Some(tooltip) = home_sidebar::session_tooltip(state, theme)
            .or_else(|| home_sidebar::workspace_tooltip(state, theme))
    {
        window = window.child(tooltip);
    }
    if state.primary_view == PrimaryView::WorkspaceCreation
        && let Some(scope) = state.workspace_draft.surface_dialog_scope
    {
        for layer in workspace_creation::surface_dialog(state, scope, theme) {
            window = window.child(layer);
        }
    }
    if state.primary_view == PrimaryView::WorkspaceCreation
        && state.workspace_draft.cancel_dialog_open
    {
        for layer in workspace_creation::cancel_dialog(theme) {
            window = window.child(layer);
        }
    }
    if state.primary_view == PrimaryView::Automations
        && let Some(layers) = automations_view::confirmation(state, theme)
    {
        for layer in layers {
            window = window.child(layer);
        }
    }
    if state.primary_view == PrimaryView::Workspace
        && let Some(scope) = state.workspace_surface_dialog_scope
    {
        for layer in workspace_view::surface_dialog(state, scope, theme) {
            window = window.child(layer);
        }
    }
    if state.primary_view == PrimaryView::Workspace
        && state.workspace_surface_delete_index.is_some()
    {
        for layer in workspace_view::surface_delete_dialog(state, theme) {
            window = window.child(layer);
        }
    }
    if state.primary_view == PrimaryView::Workspace
        && state.workspace_tab == workspace_view::WorkspaceTab::Rules
        && state.workspace_rule_dialog_open
    {
        for layer in workspace_view::rule_dialog(state, theme) {
            window = window.child(layer);
        }
    }
    if state.archive_session_id.is_some() {
        for layer in home_sidebar::archive_dialog(state, theme) {
            window = window.child(layer);
        }
    }
    if state.primary_view == PrimaryView::NewResearch
        && state.active_session.is_none()
        && state.graph_editor.is_some()
    {
        for layer in new_research::graph_editor_dialog(state, theme) {
            window = window.child(layer);
        }
    }
    if state.primary_view == PrimaryView::Settings
        && state.settings_section == SettingsSection::Safety
        && (state.permission_edit_id.is_some() || state.permission_creating)
    {
        for layer in permission_settings::edit_dialog(state, theme) {
            window = window.child(layer);
        }
    }
    if state.primary_view == PrimaryView::Extensions && state.plugin_add_dialog_open {
        for layer in extensions_view::add_dialog(state, theme) {
            window = window.child(layer);
        }
    }
    if state.primary_view == PrimaryView::Extensions && state.plugin_manifest_name.is_some() {
        for layer in extensions_view::manifest_dialog(state, theme) {
            window = window.child(layer);
        }
    }
    if state.primary_view == PrimaryView::Extensions && state.feature_details_id.is_some() {
        for layer in extensions_view::feature_details_dialog(state, theme) {
            window = window.child(layer);
        }
    }
    if state.primary_view == PrimaryView::Fleet && state.fleet.connect_dialog_open {
        for layer in fleet_view::connect_dialog(state, theme) {
            window = window.child(layer);
        }
    }
    if state.primary_view == PrimaryView::Fleet && state.fleet.browser_dialog_open {
        for layer in fleet_view::browser_dialog(state, theme) {
            window = window.child(layer);
        }
    }
    if state.primary_view == PrimaryView::Fleet && state.fleet.import_dialog_open {
        for layer in fleet_view::import_dialog(state, theme) {
            window = window.child(layer);
        }
    }
    if state.primary_view == PrimaryView::Fleet && state.fleet.details_id.is_some() {
        for layer in fleet_view::details_dialog(state, theme) {
            window = window.child(layer);
        }
    }
    if state.primary_view == PrimaryView::Fleet && state.fleet.qr_dialog_open {
        for layer in fleet_view::qr_dialog(state, theme) {
            window = window.child(layer);
        }
    }
    if let Some(menu) = state.selection_menu
        && match menu {
            SelectionMenu::ResearchKit => {
                state.primary_view == PrimaryView::WorkspaceCreation
                    && state.workspace_draft.tab == workspace_creation::CreationTab::Foundation
            }
            SelectionMenu::SurfaceKind => {
                state.primary_view == PrimaryView::WorkspaceCreation
                    && state.workspace_draft.tab == workspace_creation::CreationTab::Surfaces
            }
            SelectionMenu::WorkspaceSurfaceKind => {
                state.primary_view == PrimaryView::Workspace
                    && state.workspace_tab == workspace_view::WorkspaceTab::Surfaces
                    && state.workspace_surface_dialog_scope.is_some()
            }
            SelectionMenu::HomeCollection => {
                state.primary_view != PrimaryView::Settings && state.sidenav_visible
            }
            SelectionMenu::FleetAdd => {
                state.primary_view == PrimaryView::Fleet
                    && state.fleet_section == fleet_view::FleetSection::AppServers
            }
            SelectionMenu::GoalMode => {
                state.primary_view == PrimaryView::NewResearch
                    && extensions_view::goal_mode_enabled(state)
            }
            SelectionMenu::Repeat | SelectionMenu::ResearchWorkspace => {
                state.primary_view == PrimaryView::NewResearch
            }
            SelectionMenu::ResearchWorkspaceHeading => {
                state.primary_view == PrimaryView::NewResearch && state.active_session.is_none()
            }
            SelectionMenu::SessionGraph => {
                state.primary_view == PrimaryView::NewResearch
                    && state.active_session.is_none()
                    && extensions_view::graphs_enabled(state)
            }
            SelectionMenu::QuickChatWorkspace => {
                state.quick_chat_open && state.quick_chat_id.is_none() && !state.quick_chat_busy
            }
            SelectionMenu::ColorTheme => state.settings_section == SettingsSection::Appearance,
            SelectionMenu::InterfaceFont | SelectionMenu::TextSize => {
                state.settings_section == SettingsSection::Appearance
                    && state.appearance_tab == AppearanceTab::Typography
            }
            SelectionMenu::PluginWorkspaces { index } => {
                state.primary_view == PrimaryView::Extensions
                    && state.extensions_section == extensions_view::ExtensionSection::Plugins
                    && state
                        .plugins
                        .get(index)
                        .is_some_and(|plugin| plugin.installed)
            }
            SelectionMenu::FeatureWorkspaces { index } => {
                state.primary_view == PrimaryView::Extensions
                    && state.extensions_section == extensions_view::ExtensionSection::Features
                    && state
                        .features
                        .get(index)
                        .is_some_and(|feature| feature.enabled)
            }
            SelectionMenu::BrowserWorkspaces { index } => {
                state.primary_view == PrimaryView::Fleet
                    && state.fleet_section == fleet_view::FleetSection::Browsers
                    && state.fleet.browser_scope_enabled(index)
            }
            SelectionMenu::VirtualMachineWorkspaces { index } => {
                state.primary_view == PrimaryView::Fleet
                    && state.fleet_section == fleet_view::FleetSection::VirtualMachines
                    && state.fleet.vm_scope_enabled(index)
            }
            _ => {
                state.primary_view == PrimaryView::Settings
                    && state.settings_section == SettingsSection::Providers
            }
        }
    {
        if menu == SelectionMenu::GoalMode {
            window = window.child(
                new_research::goal_menu(state, theme)
                    .modal(true, |s: &mut State| s.selection_menu = None),
            );
        } else {
            let items = selection_menu_items(state, menu);
            if !items.is_empty() {
                let (title, width) = match menu {
                    SelectionMenu::HomeCollection => ("Sidebar collection", 170.0),
                    SelectionMenu::FleetAdd => ("Add App Server", 240.0),
                    SelectionMenu::Repeat => ("Repeat schedule", 190.0),
                    SelectionMenu::GoalMode => ("Goal Mode", 150.0),
                    SelectionMenu::SessionGraph => ("Session Graph", 190.0),
                    SelectionMenu::ResearchWorkspace => ("Workspace", 280.0),
                    SelectionMenu::ResearchWorkspaceHeading => ("Workspace", 280.0),
                    SelectionMenu::QuickChatWorkspace => ("Quick Chat workspace", 280.0),
                    SelectionMenu::ResearchKit => ("Research Kit", 178.0),
                    SelectionMenu::SurfaceKind => ("Surface Type", 178.0),
                    SelectionMenu::WorkspaceSurfaceKind => ("Surface Type", 178.0),
                    SelectionMenu::LeadProvider => ("Default Provider", 178.0),
                    SelectionMenu::ColorTheme => ("Color Theme", 178.0),
                    SelectionMenu::InterfaceFont => ("Interface Font", 178.0),
                    SelectionMenu::TextSize => ("Text Size", 178.0),
                    SelectionMenu::PluginWorkspaces { .. }
                    | SelectionMenu::FeatureWorkspaces { .. }
                    | SelectionMenu::BrowserWorkspaces { .. }
                    | SelectionMenu::VirtualMachineWorkspaces { .. } => ("Workspaces", 240.0),
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
    }
    if state.primary_view == PrimaryView::NewResearch && state.composer_menu_open {
        window = window.child(new_research::model_menu(state, theme));
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
        let mut workspace_poll = 0_u8;
        let mut usage_refresh_at = Instant::now() - Duration::from_secs(20 * 60);
        let mut previous_subscriptions = [false; 2];
        let mut usage_generation = 0_u64;
        loop {
            let status = runtime.block_on(connection::probe(&client, addr));
            let poll_archive =
                workspace_poll == 0 && matches!(status, ConnectionStatus::Connected { .. });
            let workspace_result =
                if workspace_poll == 0 && matches!(status, ConnectionStatus::Connected { .. }) {
                    let workspace_client = workspaces::client();
                    Some(match workspace_client {
                        Ok(workspace_client) => {
                            runtime.block_on(workspaces::list(&workspace_client, addr))
                        }
                        Err(error) => Err(error),
                    })
                } else {
                    None
                };
            let session_result = if workspace_poll == 0
                && matches!(status, ConnectionStatus::Connected { .. })
            {
                Some(match sessions::client() {
                    Ok(session_client) => runtime.block_on(sessions::list(&session_client, addr)),
                    Err(error) => Err(error),
                })
            } else {
                None
            };
            let custom_result =
                if workspace_poll == 0 && matches!(status, ConnectionStatus::Connected { .. }) {
                    Some(runtime.block_on(provider_custom::list(&client, addr)))
                } else {
                    None
                };
            workspace_poll = (workspace_poll + 1) % 5;
            let auth_result = if matches!(status, ConnectionStatus::Connected { .. }) {
                runtime.block_on(provider_auth::status(&client, addr)).ok()
            } else {
                None
            };
            let auth_loaded = auth_result.is_some();
            let auth_snapshot = auth_result.unwrap_or_default();
            let auth_status = auth_snapshot.providers;
            let subscriptions = [auth_status[0].subscription, auth_status[1].subscription];
            if matches!(status, ConnectionStatus::Connected { .. })
                && (usage_refresh_at.elapsed() >= Duration::from_secs(20 * 60)
                    || subscriptions != previous_subscriptions)
            {
                let refresh = subscriptions != previous_subscriptions;
                usage_refresh_at = Instant::now();
                previous_subscriptions = subscriptions;
                usage_generation = usage_generation.wrapping_add(1);
                let generation = usage_generation;
                handle.update(move |state, _| state.usage_generation = generation);
                let usage_handle = handle.clone();
                let usage_client = client.clone();
                std::thread::spawn(move || {
                    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    else {
                        return;
                    };
                    let result =
                        runtime.block_on(provider_usage::fetch(&usage_client, addr, refresh));
                    if let Ok(usage) = result {
                        usage_handle.update_if(move |state, _| {
                            if state.usage_generation != generation || state.provider_usage == usage
                            {
                                return false;
                            }
                            state.provider_usage = usage;
                            true
                        });
                    }
                });
            }
            let lead_provider = auth_snapshot.lead.and_then(Provider::from_id);
            let lead_custom_provider_id = auth_snapshot.lead_custom_provider_id;
            let daybreak_mode = auth_loaded.then_some(auth_snapshot.daybreak_mode);
            if let Some(result) = workspace_result {
                handle.update_if(move |state, _| state.apply_workspace_list(result));
            }
            if let Some(Ok(summaries)) = session_result {
                handle.update_if(move |state, _| {
                    if state.session_summaries == summaries {
                        false
                    } else {
                        state.session_summaries = summaries;
                        true
                    }
                });
            }
            if let Some(Ok(providers)) = custom_result {
                handle.update_if(move |state, _| {
                    if state.custom_providers == providers {
                        false
                    } else {
                        state.custom_providers = providers;
                        true
                    }
                });
            }
            handle.update_if(move |state, _| {
                if state.connection == status
                    && state.auth_status == auth_status
                    && state.auth_loaded == auth_loaded
                    && state.lead_provider == lead_provider
                    && state.lead_custom_provider_id == lead_custom_provider_id
                    && daybreak_mode
                        .is_none_or(|mode| state.daybreak_saving || state.daybreak_mode == mode)
                {
                    false
                } else {
                    let previous = state.auth_status.clone();
                    let previous_lead = state.lead_provider;
                    let was_connected =
                        matches!(state.connection, ConnectionStatus::Connected { .. });
                    state.connection = status;
                    if !was_connected
                        && matches!(state.connection, ConnectionStatus::Connected { .. })
                        && state.primary_view == PrimaryView::Settings
                        && state.settings_section == SettingsSection::Archive
                    {
                        start_archive_load(state);
                    }
                    state.auth_status = auth_status;
                    state.auth_loaded = auth_loaded;
                    state.lead_provider = lead_provider;
                    state.lead_custom_provider_id = lead_custom_provider_id;
                    if let Some(mode) = daybreak_mode
                        && !state.daybreak_saving
                    {
                        state.daybreak_mode = mode;
                    }
                    if !auth_loaded
                        || (state.lead_provider != Some(Provider::Custom)
                            && state
                                .auth_status
                                .iter()
                                .all(|item| !item.subscription && !item.api_key))
                    {
                        state.selection_menu = None;
                    }
                    if previous_lead != lead_provider {
                        state.lead_error.clear();
                    }
                    for provider in Provider::ALL {
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
            if poll_archive {
                handle.update(|state, _| {
                    extensions_view::start_feature_load(state);
                    if state.primary_view == PrimaryView::Settings
                        && state.settings_section == SettingsSection::Archive
                    {
                        start_archive_load(state);
                    }
                    if state.primary_view == PrimaryView::Extensions {
                        extensions_view::start_load(state);
                    }
                    if state.primary_view == PrimaryView::Fleet {
                        fleet_view::start_load(state);
                    }
                    if state.primary_view == PrimaryView::Automations {
                        automations_view::start_load(state);
                    }
                });
            }
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
    fn automations_navigation_shows_future_details_and_cancel_choice() {
        let mut state = State {
            primary_view: PrimaryView::Home,
            ..State::default()
        };
        state
            .automations
            .jobs
            .push(beale_contracts::v1::AutomationJob {
                id: uuid::Uuid::new_v4().to_string(),
                session_id: uuid::Uuid::new_v4().to_string(),
                workspace_id: uuid::Uuid::new_v4().to_string(),
                title: "Example scheduled research".to_owned(),
                repeat_preset: "day".to_owned(),
                next_run_at_ms: 1_900_000_000_000,
                last_run_at_ms: 1_800_000_000_000,
                canceled: false,
            });
        state
            .automations
            .runs
            .push(beale_contracts::v1::AutomationRun {
                id: uuid::Uuid::new_v4().to_string(),
                job_id: state.automations.jobs[0].id.clone(),
                session_id: String::new(),
                title: "Example active research".to_owned(),
                started_at_ms: 1_800_000_060_000,
                completed_at_ms: 0,
                status: beale_contracts::v1::AutomationRunStatus::Active as i32,
                error: String::new(),
            });
        state
            .automations
            .runs
            .push(beale_contracts::v1::AutomationRun {
                id: uuid::Uuid::new_v4().to_string(),
                job_id: state.automations.jobs[0].id.clone(),
                session_id: uuid::Uuid::new_v4().to_string(),
                title: "Example past research".to_owned(),
                started_at_ms: 1_800_000_000_000,
                completed_at_ms: 1_800_000_030_000,
                status: beale_contracts::v1::AutomationRunStatus::Complete as i32,
                error: String::new(),
            });
        let mut runtime = Runtime::new(state, view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let (x, y) = runtime.find("nav-automations").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(runtime.state.primary_view, PrimaryView::Automations);
        assert!(runtime.find("automations-schedule-job").is_some());
        assert!(runtime.find("automations-Active-label").is_some());
        assert!(runtime.find("automations-Future-label").is_some());
        assert!(runtime.find("automations-Past-label").is_some());
        assert!(
            runtime
                .find(&format!(
                    "automation-run-active-{}",
                    runtime.state.automations.runs[0].id
                ))
                .is_some()
        );
        assert!(
            runtime
                .find(&format!(
                    "automation-run-past-{}",
                    runtime.state.automations.runs[1].id
                ))
                .is_some()
        );
        let job_key = format!(
            "automation-job-future-{}",
            runtime.state.automations.jobs[0].id
        );
        let (x, y) = runtime.find(&job_key).unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(
            runtime.state.automations.selected_id.as_deref(),
            Some(runtime.state.automations.jobs[0].id.as_str()),
        );
        assert!(runtime.find("automations-cancel-series").is_some());
        let (x, y) = runtime.find("automations-cancel-series").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert!(runtime.find("automations-cancel-dialog").is_some());
    }

    #[test]
    fn automation_sections_show_five_rows_until_expanded() {
        for section in ["active", "future", "past"] {
            let mut state = State {
                primary_view: PrimaryView::Automations,
                ..State::default()
            };
            for index in 1..=6 {
                let id = format!("00000000-0000-4000-8000-{index:012x}");
                if section == "future" {
                    state
                        .automations
                        .jobs
                        .push(beale_contracts::v1::AutomationJob {
                            id,
                            session_id: uuid::Uuid::new_v4().to_string(),
                            workspace_id: uuid::Uuid::new_v4().to_string(),
                            title: format!("Example job {index}"),
                            repeat_preset: "day".to_owned(),
                            next_run_at_ms: 1_900_000_000_000,
                            last_run_at_ms: 1_800_000_000_000,
                            canceled: false,
                        });
                } else {
                    let active = section == "active";
                    state
                        .automations
                        .runs
                        .push(beale_contracts::v1::AutomationRun {
                            id,
                            job_id: uuid::Uuid::new_v4().to_string(),
                            session_id: if active {
                                String::new()
                            } else {
                                uuid::Uuid::new_v4().to_string()
                            },
                            title: format!("Example run {index}"),
                            started_at_ms: 1_800_000_000_000,
                            completed_at_ms: if active { 0 } else { 1_800_000_030_000 },
                            status: if active {
                                beale_contracts::v1::AutomationRunStatus::Active as i32
                            } else {
                                beale_contracts::v1::AutomationRunStatus::Complete as i32
                            },
                            error: String::new(),
                        });
                }
            }
            let sixth = format!(
                "automation-{}-{section}-00000000-0000-4000-8000-000000000006",
                if section == "future" { "job" } else { "run" }
            );
            let fifth = format!(
                "automation-{}-{section}-00000000-0000-4000-8000-000000000005",
                if section == "future" { "job" } else { "run" }
            );
            let toggle = format!("automations-{section}-toggle");
            let mut runtime = Runtime::new(state, view, (1040.0, 720.0), Env::new(true, 1.0));
            runtime.frame(0.0);
            assert!(runtime.find(&fifth).is_some());
            assert!(runtime.find(&sixth).is_none());
            let (x, y) = runtime.find(&toggle).unwrap();
            runtime.pointer_down(x, y);
            runtime.pointer_up(x, y);
            runtime.frame(1.0 / 60.0);
            assert!(runtime.find(&sixth).is_some());
            let (x, y) = runtime.find(&toggle).unwrap();
            runtime.pointer_down(x, y);
            runtime.pointer_up(x, y);
            runtime.frame(1.0 / 60.0);
            assert!(runtime.find(&sixth).is_none());
        }
    }

    #[test]
    fn safety_tabs_show_editable_host_rules_without_shields() {
        let state = State {
            primary_view: PrimaryView::Settings,
            settings_section: SettingsSection::Safety,
            permission_loaded: true,
            permission_rules: vec![
                beale_contracts::v1::PermissionRule {
                    id: "auto-scope".to_owned(),
                    stage: beale_contracts::v1::ReviewStage::AutoReview as i32,
                    title: "Recorded research scope".to_owned(),
                    instruction: "Deny clear scope violations.".to_owned(),
                    enabled: true,
                },
                beale_contracts::v1::PermissionRule {
                    id: "guardian-risk".to_owned(),
                    stage: beale_contracts::v1::ReviewStage::Guardian as i32,
                    title: "Concrete risk".to_owned(),
                    instruction: "Deny ambiguous deletion.".to_owned(),
                    enabled: false,
                },
            ],
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let heading = runtime.presented("permission-rules-heading").unwrap().0;
        let description = runtime.presented("permission-rules-description").unwrap().0;
        let card = runtime.presented("permission-rules-card").unwrap().0;
        assert!(heading[1] + heading[3] <= description[1]);
        assert!(description[1] + description[3] <= card[1]);
        assert!(runtime.find("permission-add-auto-review").is_some());
        assert!(runtime.find("permission-add-guardian").is_none());
        assert!(runtime.presented("permission-toggle-auto-scope").is_some());
        assert!(
            runtime
                .presented("permission-toggle-guardian-risk")
                .is_none()
        );
        let (x, y) = runtime.find("safety-guardian").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        let heading = runtime.presented("permission-rules-heading").unwrap().0;
        let description = runtime.presented("permission-rules-description").unwrap().0;
        let card = runtime.presented("permission-rules-card").unwrap().0;
        assert!(heading[1] + heading[3] <= description[1]);
        assert!(description[1] + description[3] <= card[1]);
        let (form, _) = runtime.presented("permission-rules-form").unwrap();
        let (add, _) = runtime.presented("permission-add-guardian").unwrap();
        assert!((form[0] + form[2] - add[0] - add[2]).abs() < 1.0);
        assert!(
            runtime
                .presented("permission-toggle-guardian-risk")
                .is_some()
        );
        assert_eq!(
            runtime
                .presented("permission-edit-guardian-risk")
                .unwrap()
                .0[2],
            26.0
        );
        assert_eq!(
            runtime
                .presented("permission-toggle-guardian-risk")
                .unwrap()
                .0[2],
            38.0
        );
        assert!(
            runtime
                .presented("permission-toggle-guardian-risk-thumb")
                .is_some()
        );
        let (x, y) = runtime.find("permission-edit-guardian-risk").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(runtime.state.permission_name_edit.text, "Concrete risk");
        assert_eq!(
            runtime.state.permission_description_edit.text,
            "Deny ambiguous deletion."
        );
        assert!(runtime.presented("permission-edit-dialog").is_some());
        assert!(runtime.presented("permission-name-input").is_some());
        assert!(runtime.presented("permission-description-input").is_some());
        assert!(runtime.presented("permission-rule-input").is_none());
        assert!(runtime.presented("safety-shields").is_none());
        runtime.update(|state, _| state.permission_name_edit.clear());
        runtime.frame(0.0);
        let (x, y) = runtime.find("permission-edit-save").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert!(runtime.presented("permission-edit-error").is_some());
        let (x, y) = runtime.find("permission-edit-cancel").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert!(runtime.state.permission_edit_id.is_none());
        let (x, y) = runtime.find("permission-add-guardian").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert!(runtime.state.permission_creating);
        assert!(runtime.state.permission_name_edit.text.is_empty());
        assert!(runtime.state.permission_description_edit.text.is_empty());
        assert!(runtime.presented("permission-edit-dialog").is_some());
        let (x, y) = runtime.find("permission-edit-cancel").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert!(!runtime.state.permission_creating);
    }

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
        runtime.update(|state, _| state.settings_section = SettingsSection::Safety);
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
        let mut runtime = Runtime::new(
            State {
                settings_section: SettingsSection::Providers,
                ..State::default()
            },
            view,
            (1040.0, 720.0),
            Env::new(true, 1.0),
        );
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

        let (x, y) = runtime.find("settings-appearance").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        let (x, y) = runtime.find("settings-providers").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(runtime.state.selected, Provider::Anthropic);
        assert!(runtime.find("provider-detail-anthropic").is_some());
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
                settings_section: SettingsSection::Providers,
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
            settings_section: SettingsSection::Providers,
            auth_loaded: true,
            lead_provider: Some(Provider::Xai),
            auth_status: [
                AuthStatus::default(),
                AuthStatus {
                    api_key: true,
                    ..AuthStatus::default()
                },
                AuthStatus::default(),
                AuthStatus::default(),
                AuthStatus::default(),
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
    fn custom_provider_panel_fits_narrow_window() {
        let state = State {
            settings_section: SettingsSection::Providers,
            selected: Provider::Custom,
            custom_providers: vec![beale_contracts::v1::CustomProvider {
                id: "custom-example".to_owned(),
                name: "Example API".to_owned(),
                base_url: "http://127.0.0.1:12345/v1".to_owned(),
                models: vec![beale_contracts::v1::ModelEntry {
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
        let mut runtime = Runtime::new(state, view, (840.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let (page, _) = runtime.presented("settings-page").unwrap();
        let (detail, _) = runtime.presented("custom-provider-detail").unwrap();
        assert!(page[0] + page[2] <= 832.0);
        assert!((detail[0] - page[0]).abs() < 1.0);
        assert!((detail[0] + detail[2] - page[0] - page[2]).abs() < 1.0);
        assert!(runtime.find("custom-provider-custom-example").is_some());
    }

    #[test]
    fn model_menu_closes_on_escape_and_outside_press() {
        let state = State {
            settings_section: SettingsSection::Providers,
            auth_loaded: true,
            auth_status: [
                AuthStatus {
                    subscription: true,
                    ..AuthStatus::default()
                },
                AuthStatus::default(),
                AuthStatus::default(),
                AuthStatus::default(),
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
                None,
                None,
                None,
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
            settings_section: SettingsSection::Providers,
            auth_loaded: true,
            auth_status: [
                AuthStatus {
                    subscription: true,
                    ..AuthStatus::default()
                },
                AuthStatus::default(),
                AuthStatus::default(),
                AuthStatus::default(),
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
                None,
                None,
                None,
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
    fn source_priority_defaults_to_subscription_without_refresh_control() {
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
            settings_section: SettingsSection::Providers,
            auth_loaded: true,
            auth_status: std::array::from_fn(|index| {
                if index == 0 {
                    status.clone()
                } else {
                    AuthStatus::default()
                }
            }),
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        assert!(runtime.find("provider-view-header").is_some());
        assert!(runtime.find("refresh-provider-models").is_none());
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
            settings_section: SettingsSection::Providers,
            auth_loaded: true,
            auth_status: [
                AuthStatus {
                    subscription: true,
                    ..AuthStatus::default()
                },
                AuthStatus::default(),
                AuthStatus::default(),
                AuthStatus::default(),
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
                None,
                None,
                None,
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
            settings_section: SettingsSection::Providers,
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
    fn startup_opens_home_with_no_workspace_selected() {
        let state = State::load_appearance();
        assert_eq!(state.primary_view, PrimaryView::Home);
        let mut runtime = Runtime::new(state, view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        assert!(runtime.find("home-no-workspace-message").is_some());
        assert!(runtime.find("settings-sidenav").is_none());
    }

    #[test]
    fn home_uses_its_sidebar_and_centers_empty_workspace_message() {
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
            "home-collection-header",
            "home-collection-control",
            "home-search-workspaces",
            "home-add-workspace",
            "home-workspaces-empty",
            "home-no-workspace-message",
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
        let (surface, _) = runtime.presented("content-surface").unwrap();
        let (message, _) = runtime.presented("home-no-workspace-message").unwrap();
        let (selector, _) = runtime.presented("home-collection-control").unwrap();
        let (add, _) = runtime.presented("home-add-workspace").unwrap();
        let (search, _) = runtime.presented("home-search-workspaces").unwrap();
        assert!(selector[0] + selector[2] < add[0]);
        assert!(add[0] + add[2] <= search[0]);
        assert!((search[0] - add[0] - add[2] - 4.0).abs() < 1.0);
        assert!((message[0] + message[2] / 2.0 - surface[0] - surface[2] / 2.0).abs() < 1.0);
        assert!((message[1] + message[3] / 2.0 - surface[1] - surface[3] / 2.0).abs() < 1.0);
        let history = runtime.state.view_history.back.len();
        let (x, y) = runtime.find("home-new-research").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(runtime.state.primary_view, PrimaryView::NewResearch);
        assert!(runtime.find("home-no-workspace-message").is_none());
        assert!(runtime.find("new-research-composer-input").is_some());
        assert!(runtime.find("new-research-model-control").is_some());
        assert!(runtime.find("new-research-workspace-control").is_some());
        assert!(runtime.find("new-research-repeat-control").is_some());
        assert!(runtime.find("settings-scroll").is_none());
        let (composer, _) = runtime.presented("new-research-composer").unwrap();
        let (ribbon, _) = runtime.presented("new-research-ribbon").unwrap();
        let (location, _) = runtime.presented("new-research-workspace-control").unwrap();
        let (surface, _) = runtime.presented("content-surface").unwrap();
        assert!((composer[3] - 107.0).abs() < 1.0);
        assert!((ribbon[3] - 44.0).abs() < 1.0);
        assert!(ribbon[1] < composer[1]);
        assert!((ribbon[1] + ribbon[3] - composer[1] - 8.0).abs() < 1.0);
        assert!((ribbon[0] - composer[0] - 20.0).abs() < 1.0);
        assert!((composer[0] + composer[2] - ribbon[0] - ribbon[2] - 20.0).abs() < 1.0);
        assert!((location[0] - ribbon[0] - 8.0).abs() < 1.0);
        assert!(composer[1] > surface[1] + surface[3] * 0.5);
        assert!((surface[1] + surface[3] - composer[1] - composer[3] - 6.0).abs() < 1.0);
        assert_eq!(runtime.state.view_history.back.len(), history + 1);
        let (x, y) = runtime.find("header-back").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
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
    fn quick_chat_enters_from_right_and_grows_up_after_first_send() {
        let mut runtime = Runtime::new(
            State {
                primary_view: PrimaryView::Home,
                ..State::default()
            },
            view,
            (1040.0, 720.0),
            Env::new(true, 1.0),
        );
        runtime.frame(0.0);
        assert!(runtime.find("quick-chat-panel").is_none());
        let (x, y) = runtime.find("home-quick-chat").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        let (entering, _) = runtime.presented("quick-chat-panel").unwrap();
        runtime.frame(1.0);
        let (settled, _) = runtime.presented("quick-chat-panel").unwrap();
        assert!(entering[0] >= settled[0]);
        assert!(settled[0] + settled[2] <= 1040.0);
        assert!(runtime.find("quick-chat-input").is_some());
        assert!(runtime.find("quick-chat-content").is_none());
        assert!(runtime.find("quick-chat-workspace-control").is_some());
        assert!(runtime.find("quick-chat-title").is_none());
        let (ribbon_before, _) = runtime.presented("quick-chat-ribbon").unwrap();
        assert!((ribbon_before[0] - settled[0] - 20.0).abs() < 1.0);
        assert!((settled[2] - ribbon_before[2] - 40.0).abs() < 1.0);
        assert!(
            !runtime
                .presented_pose("quick-chat-panel")
                .unwrap()
                .shadows()
                .is_empty()
        );
        runtime.update(|state, _| {
            state.quick_chat_pending = Some("Example question".to_owned());
            state.quick_chat_busy = true;
            state.quick_chat_title = "Example question".to_owned();
        });
        runtime.frame(0.0);
        runtime.frame(0.2);
        let (ribbon_after, _) = runtime.presented("quick-chat-ribbon").unwrap();
        let (content, _) = runtime.presented("quick-chat-content").unwrap();
        assert!(ribbon_after[1] < ribbon_before[1]);
        assert!(runtime.find("quick-chat-workspace-control").is_none());
        assert!(runtime.find("quick-chat-title").is_some());
        assert!(content[3] >= 150.0);
        assert!((content[0] - ribbon_after[0]).abs() < 1.0);
        assert!((content[2] - ribbon_after[2]).abs() < 1.0);
        assert!(runtime.find("quick-chat-content-left-border").is_some());
        assert!(runtime.find("quick-chat-content-right-border").is_some());
        let (x, y) = runtime.find("quick-chat-close").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert!(!runtime.state.quick_chat_open);
        assert!(runtime.state.quick_chat_pending.is_none());
        assert!(!runtime.state.quick_chat_busy);
        assert!(runtime.find("quick-chat-panel").is_none());
        let (x, y) = runtime.find("home-quick-chat").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert!(runtime.state.quick_chat_pending.is_none());
        assert!(runtime.find("quick-chat-content").is_none());
        assert!(runtime.find("quick-chat-workspace-control").is_some());
    }

    #[test]
    fn quick_chat_history_buttons_restore_the_current_draft() {
        let mut runtime = Runtime::new(
            State {
                primary_view: PrimaryView::Home,
                quick_chat_open: true,
                quick_chat_id: Some("00000000-0000-4000-8000-000000000001".to_owned()),
                quick_chat_draft: TextEdit::new("Unsent example"),
                quick_chat_messages: vec![
                    beale_contracts::v1::QuickChatMessage {
                        role: beale_contracts::v1::SessionRole::User as i32,
                        text: "First example".to_owned(),
                    },
                    beale_contracts::v1::QuickChatMessage {
                        role: beale_contracts::v1::SessionRole::Assistant as i32,
                        text: "First reply".to_owned(),
                    },
                    beale_contracts::v1::QuickChatMessage {
                        role: beale_contracts::v1::SessionRole::User as i32,
                        text: "Second example".to_owned(),
                    },
                    beale_contracts::v1::QuickChatMessage {
                        role: beale_contracts::v1::SessionRole::Assistant as i32,
                        text: "Second reply".to_owned(),
                    },
                ],
                ..State::default()
            },
            view,
            (1040.0, 720.0),
            Env::new(true, 1.0),
        );
        runtime.frame(1.0);
        let click = |runtime: &mut Runtime<State>, key: &str| {
            let (x, y) = runtime.find(key).unwrap();
            runtime.pointer_down(x, y);
            runtime.pointer_up(x, y);
            runtime.frame(0.0);
        };
        click(&mut runtime, "quick-chat-history-previous");
        assert_eq!(runtime.state.quick_chat_draft.text, "Second example");
        click(&mut runtime, "quick-chat-history-previous");
        assert_eq!(runtime.state.quick_chat_draft.text, "First example");
        click(&mut runtime, "quick-chat-history-next");
        assert_eq!(runtime.state.quick_chat_draft.text, "Second example");
        click(&mut runtime, "quick-chat-history-next");
        assert_eq!(runtime.state.quick_chat_draft.text, "Unsent example");
        click(&mut runtime, "quick-chat-history-previous");
        let (x, y) = runtime.find("quick-chat-input").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.paste(" edited");
        runtime.frame(0.0);
        assert!(!runtime.state.quick_chat_input_history.can_next());
        runtime.state.close_quick_chat();
        assert!(!runtime.state.quick_chat_input_history.can_next());
    }

    #[test]
    fn hovered_home_session_shows_details_beside_the_sidebar() {
        let workspace_id = "00000000-0000-4000-8000-000000000001";
        let session_id = "00000000-0000-4000-8000-000000000002";
        let last_message_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            - 7_200_000;
        let state = State {
            primary_view: PrimaryView::Home,
            workspaces: vec![WorkspaceSummary {
                id: workspace_id.to_owned(),
                name: "Example Workspace".to_owned(),
                directory: "example-directory".to_owned(),
            }],
            session_summaries: vec![SessionSummary {
                id: session_id.to_owned(),
                workspace_id: workspace_id.to_owned(),
                title: "A complete example session title".to_owned(),
                last_message_at_ms,
            }],
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (840.0, 600.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let (session_row, _) = runtime
            .presented(&format!("home-session-container-{session_id}"))
            .unwrap();
        let (fade, resting_opacity) = runtime
            .presented(&format!("home-session-archive-fade-{session_id}"))
            .unwrap();
        assert!(resting_opacity < 0.01);
        runtime.pointer_move(session_row[0] + 2.0, session_row[1] + session_row[3] / 2.0);
        runtime.frame(0.3);
        let (_, hovered_opacity) = runtime
            .presented(&format!("home-session-archive-fade-{session_id}"))
            .unwrap();
        assert!(hovered_opacity > 0.99);
        let (archive, _) = runtime
            .presented(&format!("home-archive-session-{session_id}"))
            .unwrap();
        assert!(fade[0] + fade[2] <= archive[0]);
        assert_eq!(
            runtime.state.hovered_session_id.as_deref(),
            Some(session_id)
        );
        let (sidebar, _) = runtime.presented("home-sidenav").unwrap();
        let (tooltip, _) = runtime.presented("home-session-tooltip").unwrap();
        let (title, _) = runtime.presented("home-session-tooltip-title").unwrap();
        let (age, _) = runtime.presented("home-session-tooltip-age").unwrap();
        let (workspace, _) = runtime.presented("home-session-tooltip-workspace").unwrap();
        assert!(
            runtime
                .find("home-session-tooltip-workspace-icon")
                .is_some()
        );
        assert!(
            tooltip[0] >= sidebar[0] + sidebar[2],
            "tooltip={tooltip:?} sidebar={sidebar:?}"
        );
        assert!(title[0] < age[0]);
        assert!(workspace[1] > title[1]);
        let (archive_x, archive_y) = runtime
            .find(&format!("home-archive-session-{session_id}"))
            .unwrap();
        runtime.pointer_move(archive_x, archive_y);
        runtime.frame(0.32);
        runtime.pointer_down(archive_x, archive_y);
        runtime.pointer_up(archive_x, archive_y);
        runtime.frame(0.34);
        assert_eq!(
            runtime.state.archive_session_id.as_deref(),
            Some(session_id)
        );
        runtime.pointer_move(600.0, 300.0);
        runtime.frame(0.36);
        assert!(runtime.find("home-session-tooltip").is_none());
    }

    #[test]
    fn hovered_home_workspace_shows_details_beside_the_sidebar() {
        let workspace_id = "00000000-0000-4000-8000-000000000001";
        let session_id = "00000000-0000-4000-8000-000000000002";
        let state = State {
            primary_view: PrimaryView::Home,
            workspaces: vec![WorkspaceSummary {
                id: workspace_id.to_owned(),
                name: "Example Workspace".to_owned(),
                directory: "/example/workspaces/a-very-long-synthetic-directory-name/another-synthetic-directory-name/research".to_owned(),
            }],
            session_summaries: vec![SessionSummary {
                id: session_id.to_owned(),
                workspace_id: workspace_id.to_owned(),
                title: "Example session".to_owned(),
                last_message_at_ms: 1,
            }],
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (840.0, 600.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let (workspace_row, _) = runtime
            .presented(&format!("home-workspace-{workspace_id}"))
            .unwrap();
        runtime.pointer_move(
            workspace_row[0] + 2.0,
            workspace_row[1] + workspace_row[3] / 2.0,
        );
        runtime.frame(0.3);
        assert_eq!(
            runtime.state.hovered_workspace_id.as_deref(),
            Some(workspace_id)
        );
        let (sidebar, _) = runtime.presented("home-sidenav").unwrap();
        let (tooltip, _) = runtime.presented("home-workspace-tooltip").unwrap();
        let (name, _) = runtime.presented("home-workspace-tooltip-name").unwrap();
        let (count, _) = runtime
            .presented("home-workspace-tooltip-sessions")
            .unwrap();
        let (directory, _) = runtime
            .presented("home-workspace-tooltip-directory")
            .unwrap();
        let (count_icon, _) = runtime
            .presented("home-workspace-tooltip-sessions-icon")
            .unwrap();
        let (directory_icon, _) = runtime
            .presented("home-workspace-tooltip-directory-icon")
            .unwrap();
        assert!(tooltip[0] >= sidebar[0] + sidebar[2]);
        assert!(name[1] < count[1] && count[1] < directory[1]);
        assert!(count_icon[0] < count[0] && directory_icon[0] < directory[0]);
        assert!(
            directory[3] < 20.0,
            "directory should stay on one line: {directory:?}"
        );
        runtime.pointer_move(600.0, 300.0);
        runtime.frame(0.32);
        assert!(runtime.find("home-workspace-tooltip").is_none());
    }

    #[test]
    fn home_session_archive_opens_confirmation_and_cancel_preserves_session() {
        let workspace_id = "00000000-0000-4000-8000-000000000001";
        let session_id = "00000000-0000-4000-8000-000000000002";
        let state = State {
            primary_view: PrimaryView::Home,
            workspaces: vec![WorkspaceSummary {
                id: workspace_id.to_owned(),
                name: "Example Workspace".to_owned(),
                directory: "example-directory".to_owned(),
            }],
            session_summaries: vec![SessionSummary {
                id: session_id.to_owned(),
                workspace_id: workspace_id.to_owned(),
                title: "An example research session with a long title".to_owned(),
                last_message_at_ms: 1,
            }],
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (840.0, 600.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let (sidebar, _) = runtime.presented("home-sidenav").unwrap();
        let (archive, _) = runtime
            .presented(&format!("home-archive-session-{session_id}"))
            .unwrap();
        let (fade, _) = runtime
            .presented(&format!("home-session-archive-fade-{session_id}"))
            .unwrap();
        let (session_row, _) = runtime
            .presented(&format!("home-session-{session_id}"))
            .unwrap();
        assert!(archive[0] + archive[2] <= sidebar[0] + sidebar[2]);
        assert!(session_row[0] + session_row[2] >= archive[0] + archive[2]);
        assert!(fade[0] + fade[2] <= archive[0]);
        let (x, y) = runtime
            .find(&format!("home-archive-session-{session_id}"))
            .unwrap();
        runtime.pointer_move(x, y);
        runtime.frame(1.0 / 60.0);
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(2.0 / 60.0);
        assert_eq!(
            runtime.state.archive_session_id.as_deref(),
            Some(session_id)
        );
        assert!(runtime.find("home-session-archive-dialog").is_some());
        let (x, y) = runtime.find("home-session-archive-cancel").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(3.0 / 60.0);
        assert!(runtime.state.archive_session_id.is_none());
        assert!(
            runtime
                .find(&format!("home-session-{session_id}"))
                .is_some()
        );
    }

    #[test]
    fn archive_tabs_show_flat_session_and_quick_chat_rows() {
        let workspace_id = "00000000-0000-4000-8000-000000000001";
        let session_id = "00000000-0000-4000-8000-000000000002";
        let chat_id = "00000000-0000-4000-8000-000000000003";
        let state = State {
            primary_view: PrimaryView::Settings,
            settings_section: SettingsSection::Archive,
            workspaces: vec![WorkspaceSummary {
                id: workspace_id.to_owned(),
                name: "Example Workspace".to_owned(),
                directory: "example-directory".to_owned(),
            }],
            archive_entries: vec![
                ArchiveEntry {
                    id: session_id.to_owned(),
                    workspace_id: workspace_id.to_owned(),
                    title: "An example session with a long title that should fade before the Restore control".to_owned(),
                    model: "model-example".to_owned(),
                    turn_count: 2,
                    created_at_ms: 1,
                    archived_at_ms: 2,
                    quick_chat: false,
                    last_message_at_ms: 1,
                },
                ArchiveEntry {
                    id: chat_id.to_owned(),
                    workspace_id: String::new(),
                    title: "Example chat".to_owned(),
                    model: "model-example".to_owned(),
                    turn_count: 1,
                    created_at_ms: 1,
                    archived_at_ms: 1,
                    quick_chat: true,
                    last_message_at_ms: 1,
                },
            ],
            archive_loaded: true,
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (900.0, 650.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        assert!(runtime.find("archive-view-tabs").is_some());
        assert!(runtime.find("archive-refresh").is_none());
        assert!(
            runtime
                .find(&format!("archive-item-{session_id}"))
                .is_some()
        );
        assert!(
            runtime
                .find(&format!("archive-restore-{session_id}"))
                .is_some()
        );
        let (list, _) = runtime.presented("archive-list").unwrap();
        let (restore, _) = runtime
            .presented(&format!("archive-restore-{session_id}"))
            .unwrap();
        assert!(restore[0] + restore[2] <= list[0] + list[2]);
        assert!(runtime.find(&format!("archive-item-{chat_id}")).is_none());
        let (x, y) = runtime.find("archive-tab-quick-chats").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(
            runtime.state.archive_tab,
            archive_view::ArchiveTab::QuickChats
        );
        assert!(
            runtime
                .find(&format!("archive-item-{session_id}"))
                .is_none()
        );
        assert!(runtime.find(&format!("archive-item-{chat_id}")).is_some());
        runtime.update(|state, _| state.quick_chat_id = Some(chat_id.to_owned()));
        runtime.frame(2.0);
        assert!(runtime.find(&format!("archive-item-{chat_id}")).is_none());
    }

    #[test]
    fn quick_chat_workspace_picker_selects_a_saved_workspace_before_start() {
        let workspace_id = uuid::Uuid::new_v4().to_string();
        let mut runtime = Runtime::new(
            State {
                primary_view: PrimaryView::Home,
                workspaces: vec![WorkspaceSummary {
                    id: workspace_id.clone(),
                    name: "Example Workspace".to_owned(),
                    directory: "example-directory".to_owned(),
                }],
                ..State::default()
            },
            view,
            (1040.0, 720.0),
            Env::new(true, 1.0),
        );
        runtime.frame(0.0);
        let (x, y) = runtime.find("home-quick-chat").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        let (x, y) = runtime.find("quick-chat-workspace-control").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(
            runtime.state.selection_menu,
            Some(SelectionMenu::QuickChatWorkspace)
        );
        let (x, y) = runtime.find("selection-menu-item-1").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(
            runtime.state.quick_chat_workspace_id.as_deref(),
            Some(workspace_id.as_str())
        );
        assert!(runtime.find("quick-chat-workspace-control").is_some());
    }

    #[test]
    fn new_research_without_workspaces_warns_and_disables_composer() {
        let state = State {
            primary_view: PrimaryView::Home,
            workspace_list_loaded: true,
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let (x, y) = runtime.find("home-new-research").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert!(runtime.find("new-research-workspace-warning").is_some());
        assert!(runtime.find("new-research-workspace-control").is_none());
        let (x, y) = runtime.find("new-research-composer-input").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.paste("Example objective");
        runtime.frame(1.0);
        assert!(runtime.state.new_research_prompt.text.is_empty());
        runtime.state.new_research_prompt = TextEdit::new("Example objective");
        start_session_turn(&mut runtime.state);
        assert!(!runtime.state.session_busy);
        assert_eq!(
            runtime.state.session_error,
            "Create a workspace before sending."
        );
    }

    #[test]
    fn new_research_without_provider_warns_instead_of_showing_model_selector() {
        let state = State {
            primary_view: PrimaryView::NewResearch,
            auth_loaded: true,
            workspaces: vec![WorkspaceSummary {
                id: "00000000-0000-4000-8000-000000000003".to_owned(),
                name: "Example Workspace".to_owned(),
                directory: std::env::temp_dir()
                    .join("example-workspace")
                    .to_string_lossy()
                    .into_owned(),
            }],
            workspace_list_loaded: true,
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (840.0, 600.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        assert!(runtime.find("new-research-provider-warning").is_some());
        assert!(runtime.find("new-research-model-control").is_none());
        assert!(runtime.find("new-research-workspace-warning").is_none());

        runtime.update(|state, _| {
            state.auth_status[Provider::OpenAi.index()].api_key = true;
        });
        runtime.frame(0.0);
        assert!(runtime.find("new-research-provider-warning").is_none());
        assert!(runtime.find("new-research-model-control").is_some());
    }

    #[test]
    fn home_collection_dropdown_switches_between_workspaces_and_empty_tasks() {
        let workspace_id = "00000000-0000-4000-8000-000000000003";
        let state = State {
            primary_view: PrimaryView::Home,
            workspaces: vec![WorkspaceSummary {
                id: workspace_id.to_owned(),
                name: "Example Workspace".to_owned(),
                directory: std::env::temp_dir()
                    .join("example-workspace")
                    .to_string_lossy()
                    .into_owned(),
            }],
            workspace_list_loaded: true,
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.on_key = Some(selection_menu_key);
        runtime.frame(0.0);
        let workspace_key = format!("home-workspace-{workspace_id}");
        assert!(runtime.find(&workspace_key).is_some());
        let (x, y) = runtime.find("home-collection-control").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(
            runtime.state.selection_menu,
            Some(SelectionMenu::HomeCollection)
        );
        assert!(runtime.find("selection-menu-item-1").is_some());
        let (x, y) = runtime.find("selection-menu-item-1").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(
            runtime.state.home_collection,
            home_sidebar::HomeCollection::Tasks
        );
        assert!(runtime.find(&workspace_key).is_none());
        assert!(runtime.find("home-add-workspace").is_none());
        assert!(runtime.find("home-search-workspaces").is_none());
        assert!(runtime.find("home-new-research").is_some());
        let (x, y) = runtime.find("home-collection-control").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        let (x, y) = runtime.find("selection-menu-item-0").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(
            runtime.state.home_collection,
            home_sidebar::HomeCollection::Workspaces
        );
        assert!(runtime.find(&workspace_key).is_some());
        assert!(runtime.find("home-add-workspace").is_some());
    }

    #[test]
    fn new_research_repeat_presets_stay_in_the_draft() {
        let state = State {
            primary_view: PrimaryView::NewResearch,
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (840.0, 600.0), Env::new(true, 1.0));
        runtime.on_key = Some(selection_menu_key);
        runtime.frame(0.0);
        assert!(runtime.find("new-research-repeat-indicator").is_none());
        let (x, y) = runtime.find("new-research-repeat-control").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(runtime.state.selection_menu, Some(SelectionMenu::Repeat));
        assert!(runtime.find("selection-menu-item-3").is_some());
        let (x, y) = runtime.find("selection-menu-item-3").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(
            runtime.state.new_research_repeat,
            new_research::RepeatPreset::Day
        );
        assert!(runtime.find("new-research-repeat-indicator").is_some());
        let (button, _) = runtime.presented("new-research-repeat-control").unwrap();
        let (dot, _) = runtime.presented("new-research-repeat-indicator").unwrap();
        assert!(dot[0] > button[0] + button[2] / 2.0);
        assert!(dot[1] < button[1] + button[3] / 2.0);
        assert!(runtime.state.selection_menu.is_none());
        let (x, y) = runtime.find("new-research-repeat-control").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        runtime.key(KeyEvent {
            key: Key::Escape,
            mods: Default::default(),
        });
        runtime.frame(0.0);
        assert!(runtime.state.selection_menu.is_none());
        assert_eq!(
            runtime.state.new_research_repeat,
            new_research::RepeatPreset::Day
        );
        assert!(runtime.find("new-research-repeat-indicator").is_some());
        let (x, y) = runtime.find("new-research-repeat-control").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        let (x, y) = runtime.find("selection-menu-item-0").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(
            runtime.state.new_research_repeat,
            new_research::RepeatPreset::None
        );
        assert!(runtime.find("new-research-repeat-indicator").is_none());
    }

    #[test]
    fn goal_mode_is_opt_in_through_the_new_research_ribbon() {
        let disabled_state = State {
            primary_view: PrimaryView::NewResearch,
            ..State::default()
        };
        let mut disabled = Runtime::new(disabled_state, view, (840.0, 600.0), Env::new(true, 1.0));
        disabled.frame(0.0);
        assert!(disabled.find("new-research-goal-control").is_none());
        let mut state = State {
            primary_view: PrimaryView::NewResearch,
            ..State::default()
        };
        state.connection = ConnectionStatus::Connected {
            major: 1,
            minor: 30,
        };
        state.features[0].enabled = true;
        let mut runtime = Runtime::new(state, view, (840.0, 600.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        assert!(!runtime.state.new_research_goal_mode);
        assert!(runtime.find("new-research-goal-indicator").is_none());
        let (goal, _) = runtime.presented("new-research-goal-control").unwrap();
        let (repeat, _) = runtime.presented("new-research-repeat-control").unwrap();
        assert!(goal[0] + goal[2] <= repeat[0]);
        let (x, y) = runtime.find("new-research-goal-control").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.1);
        assert_eq!(runtime.state.selection_menu, Some(SelectionMenu::GoalMode));
        let (x, y) = runtime.find("goal-mode-checkbox").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.2);
        assert!(runtime.state.new_research_goal_mode);
        assert!(runtime.find("new-research-goal-indicator").is_some());
        assert_eq!(runtime.state.selection_menu, Some(SelectionMenu::GoalMode));
        assert!(runtime.find("goal-mode-max-turns").is_some());
        assert!(runtime.find("goal-mode-max-duration").is_some());
        let (x, y) = runtime.find("goal-mode-max-turns").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.3);
        assert_eq!(runtime.state.selection_menu, Some(SelectionMenu::GoalMode));
        let (x, y) = runtime.find("goal-mode-checkbox").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.4);
        assert!(!runtime.state.new_research_goal_mode);
        assert!(runtime.find("new-research-goal-indicator").is_none());
    }

    #[test]
    fn graph_choice_is_shown_before_a_session_and_defaults_to_simple() {
        let mut state = State {
            primary_view: PrimaryView::NewResearch,
            workspaces: vec![WorkspaceSummary {
                id: "00000000-0000-4000-8000-000000000010".to_owned(),
                name: "Example Workspace".to_owned(),
                directory: "example-directory".to_owned(),
            }],
            new_research_workspace_id: Some("00000000-0000-4000-8000-000000000010".to_owned()),
            ..State::default()
        };
        state.connection = ConnectionStatus::Connected {
            major: 1,
            minor: 47,
        };
        state.auth_status[Provider::OpenAi.index()].subscription = true;
        state.model_catalogs[catalog_index(Provider::OpenAi, AuthMethod::Subscription)] =
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
            });
        state
            .features
            .iter_mut()
            .find(|feature| feature.id == beale_graphs::FEATURE_ID)
            .unwrap()
            .enabled = true;
        let mut runtime = Runtime::new(state, view, (900.0, 700.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        assert_eq!(runtime.state.new_research_graph_id, beale_graphs::SIMPLE_ID);
        assert!(runtime.find("new-research-workspace-heading").is_some());
        assert!(runtime.find("new-research-graph-description").is_some());
        assert!(runtime.find("new-research-simple-graph").is_some());
        for role in beale_graphs::SIMPLE_GRAPH.nodes {
            assert!(
                runtime
                    .find(&format!("new-research-graph-node-{}", role.id()))
                    .is_some()
            );
        }
        let (x, y) = runtime.find("new-research-graph-node-scout").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.05);
        assert_eq!(
            runtime
                .state
                .graph_editor
                .as_ref()
                .map(|editor| editor.role),
            Some(beale_graphs::Role::Scout)
        );
        assert!(runtime.find("graph-editor-dialog").is_some());
        assert!(runtime.find("graph-editor-max-concurrent").is_some());
        let (x, y) = runtime
            .find("graph-editor-model-OpenAI-example-small")
            .unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.07);
        let (x, y) = runtime.find("graph-editor-save").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.08);
        assert!(runtime.state.graph_editor.is_none());
        let scout_models: Vec<_> = new_research::graph_model_config(&runtime.state)
            .into_iter()
            .filter(|model| model.node_id == "scout")
            .map(|model| model.model)
            .collect();
        assert_eq!(scout_models, ["example-lead", "example-small"]);
        assert!(
            runtime
                .find("new-research-graph-other-models-scout")
                .is_some()
        );
        assert_eq!(
            runtime.state.new_research_graph_concurrency,
            [Some(1), None, None, None]
        );
        let (x, y) = runtime.find("new-research-graph-node-scout").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.081);
        runtime.state.graph_editor_max_concurrent = TextEdit::new("3");
        runtime.frame(0.082);
        let (x, y) = runtime.find("graph-editor-save").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.083);
        assert_eq!(runtime.state.new_research_graph_concurrency[1], Some(3));
        let (x, y) = runtime.find("new-research-graph-node-main").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.085);
        assert_eq!(runtime.state.graph_editor.as_ref().unwrap().models.len(), 1);
        let (x, y) = runtime
            .find("graph-editor-model-OpenAI-example-small")
            .unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.09);
        assert_eq!(runtime.state.graph_editor.as_ref().unwrap().models.len(), 1);
        runtime.state.graph_editor_max_concurrent = TextEdit::new("2");
        runtime.frame(0.095);
        let (x, y) = runtime.find("graph-editor-save").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.098);
        let main_models: Vec<_> = new_research::graph_model_config(&runtime.state)
            .into_iter()
            .filter(|model| model.node_id == "main")
            .map(|model| model.model)
            .collect();
        assert_eq!(main_models, ["example-small"]);
        assert!(
            runtime
                .find("new-research-graph-other-models-main")
                .is_none()
        );
        assert_eq!(runtime.state.new_research_graph_concurrency[0], Some(2));
        assert_eq!(
            new_research::graph_concurrency_config(&runtime.state).len(),
            2
        );
        assert_eq!(
            new_research::graph_concurrency_config(&runtime.state)[0].max_concurrent,
            2
        );
        let (x, y) = runtime.find("new-research-graph-control").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.1);
        assert_eq!(
            runtime.state.selection_menu,
            Some(SelectionMenu::SessionGraph)
        );
        let (x, y) = runtime.find("selection-menu-item-1").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.2);
        assert!(runtime.state.new_research_graph_id.is_empty());
        assert!(runtime.find("new-research-simple-graph").is_none());
    }

    #[test]
    fn graph_nodes_stay_inside_the_preview_at_narrow_widths() {
        for width in [1200.0, 900.0, 720.0, 600.0] {
            let mut state = State {
                primary_view: PrimaryView::NewResearch,
                workspaces: vec![WorkspaceSummary {
                    id: "00000000-0000-4000-8000-000000000010".to_owned(),
                    name: "Example Workspace".to_owned(),
                    directory: "example-directory".to_owned(),
                }],
                new_research_workspace_id: Some("00000000-0000-4000-8000-000000000010".to_owned()),
                ..State::default()
            };
            state.connection = ConnectionStatus::Connected {
                major: 1,
                minor: 47,
            };
            state
                .features
                .iter_mut()
                .find(|feature| feature.id == beale_graphs::FEATURE_ID)
                .unwrap()
                .enabled = true;
            let mut runtime = Runtime::new(state, view, (width, 700.0), Env::new(true, 1.0));
            runtime.frame(0.0);
            runtime.frame(0.01);
            let (stage, _) = runtime.presented("new-research-simple-graph").unwrap();
            let (surface, _) = runtime.presented("content-surface").unwrap();
            assert!(stage[0] >= surface[0] - 1.0);
            assert!(stage[0] + stage[2] <= surface[0] + surface[2] + 1.0);
            for role in beale_graphs::SIMPLE_GRAPH.nodes {
                let (node, _) = runtime
                    .presented(&format!("new-research-graph-node-{}", role.id()))
                    .unwrap();
                assert!(
                    node[0] >= stage[0] - 1.0,
                    "{} at window width {width}",
                    role.id()
                );
                assert!(
                    node[0] + node[2] <= stage[0] + stage[2] + 1.0,
                    "{} at window width {width}",
                    role.id()
                );
                assert!(node[1] >= stage[1] - 1.0);
                assert!(node[1] + node[3] <= stage[1] + stage[3] + 1.0);
            }
        }
    }

    #[test]
    fn new_research_heading_does_not_require_graphs() {
        let state = State {
            primary_view: PrimaryView::NewResearch,
            workspaces: vec![WorkspaceSummary {
                id: "00000000-0000-4000-8000-000000000011".to_owned(),
                name: "Example Workspace".to_owned(),
                directory: "example-directory".to_owned(),
            }],
            new_research_workspace_id: Some("00000000-0000-4000-8000-000000000011".to_owned()),
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (900.0, 700.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        assert!(runtime.find("new-research-workspace-heading").is_some());
        assert!(runtime.find("new-research-graph-control").is_none());
    }

    #[test]
    fn new_research_heading_switches_workspace_in_place() {
        let first_id = "00000000-0000-4000-8000-000000000011";
        let second_id = "00000000-0000-4000-8000-000000000012";
        let state = State {
            primary_view: PrimaryView::NewResearch,
            workspaces: vec![
                WorkspaceSummary {
                    id: first_id.to_owned(),
                    name: "Example First".to_owned(),
                    directory: "example-first".to_owned(),
                },
                WorkspaceSummary {
                    id: second_id.to_owned(),
                    name: "Example Second".to_owned(),
                    directory: "example-second".to_owned(),
                },
            ],
            new_research_workspace_id: Some(first_id.to_owned()),
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (900.0, 700.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        assert!(
            runtime
                .find("new-research-heading-workspace-underline")
                .is_some()
        );
        let (x, y) = runtime
            .find("new-research-heading-workspace-control")
            .unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.1);
        assert_eq!(
            runtime.state.selection_menu,
            Some(SelectionMenu::ResearchWorkspaceHeading)
        );
        let (x, y) = runtime.find("selection-menu-item-1").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.2);
        assert_eq!(
            runtime.state.new_research_workspace_id.as_deref(),
            Some(second_id)
        );
        assert_eq!(runtime.state.selection_menu, None);
    }

    #[test]
    fn blank_goal_limits_mean_unlimited_and_invalid_values_are_rejected() {
        assert_eq!(parse_goal_limit(""), Some(0));
        assert_eq!(parse_goal_limit("  "), Some(0));
        assert_eq!(parse_goal_limit("12"), Some(12));
        assert_eq!(parse_goal_limit("0"), None);
        assert_eq!(parse_goal_limit("+1"), None);
        assert_eq!(parse_goal_limit("-1"), None);
        assert_eq!(parse_goal_limit("1.5"), None);
        assert_eq!(parse_goal_limit("9223372036854775808"), None);
    }

    #[test]
    fn goal_turn_completion_updates_transcript_without_ending_the_stream() {
        let workspace_id = "00000000-0000-4000-8000-000000000001";
        let session_id = "00000000-0000-4000-8000-000000000002";
        let mut state = State {
            session_generation: 1,
            session_busy: true,
            session_pending_prompt: Some("Example objective".to_owned()),
            ..State::default()
        };
        let detail = SessionDetail {
            summary: Some(SessionSummary {
                id: session_id.to_owned(),
                workspace_id: workspace_id.to_owned(),
                title: "Example objective".to_owned(),
                last_message_at_ms: 1,
            }),
            messages: vec![beale_contracts::v1::SessionMessage {
                role: beale_contracts::v1::SessionRole::Assistant as i32,
                text: "First observation".to_owned(),
                ..Default::default()
            }],
            provider: ProviderId::Openai as i32,
            method: AuthMethod::ApiKey as i32,
            model: "model-example".to_owned(),
            goal: Some(beale_contracts::v1::SessionGoal {
                objective: "Example objective".to_owned(),
                status: beale_contracts::v1::GoalStatus::Active as i32,
                turns_used: 1,
                required_state: String::new(),
                max_turns: 0,
                max_duration_minutes: 0,
                active_ms: 1,
            }),
            first_turn_number: 1,
            graph_id: String::new(),
            graph_models: vec![],
            graph_node_concurrency: vec![],
        };
        state.apply_session_progress(
            1,
            SessionStreamEvent {
                kind: SessionStreamKind::TurnCompleted as i32,
                text: String::new(),
                tool: None,
                detail: Some(detail),
                segment_index: 0,
            },
        );
        assert!(state.session_busy);
        assert!(state.session_pending_prompt.is_none());
        assert_eq!(
            state.active_session.unwrap().messages[0].text,
            "First observation"
        );
        assert_eq!(state.session_summaries[0].id, session_id);
    }

    #[test]
    fn new_research_workspace_selector_keeps_its_draft_choice() {
        let first = "00000000-0000-4000-8000-000000000001";
        let second = "00000000-0000-4000-8000-000000000002";
        let mut state = State {
            primary_view: PrimaryView::Home,
            workspace_id: Some(uuid::Uuid::parse_str(second).unwrap()),
            workspaces: [(first, "Example One"), (second, "Example Two")]
                .into_iter()
                .map(|(id, name)| WorkspaceSummary {
                    id: id.to_owned(),
                    name: name.to_owned(),
                    directory: "example-directory".to_owned(),
                })
                .collect(),
            ..State::default()
        };
        state.start_new_research();
        assert_eq!(state.new_research_workspace_id.as_deref(), Some(second));
        let mut runtime = Runtime::new(state, view, (840.0, 600.0), Env::new(true, 1.0));
        runtime.on_key = Some(selection_menu_key);
        runtime.frame(0.0);
        let first_session = format!("home-session-{first}");
        let second_session = format!("home-session-{second}");
        assert!(runtime.find(&second_session).is_some());
        assert!(runtime.find(&first_session).is_none());
        let (workspace_row, _) = runtime
            .presented(&format!("home-workspace-{second}"))
            .unwrap();
        let (session_row, _) = runtime.presented(&second_session).unwrap();
        assert!(session_row[1] >= workspace_row[1] + workspace_row[3]);
        let (x, y) = runtime.find("new-research-workspace-control").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(
            runtime.state.selection_menu,
            Some(SelectionMenu::ResearchWorkspace)
        );
        assert!(runtime.find("selection-menu-item-0").is_some());
        assert!(runtime.find("selection-menu-item-1").is_some());
        let (item, _) = runtime.presented("selection-menu-item-0").unwrap();
        let (composer, _) = runtime.presented("new-research-composer").unwrap();
        let overlap_left = item[0].max(composer[0]);
        let overlap_right = (item[0] + item[2]).min(composer[0] + composer[2]);
        assert!(overlap_right - overlap_left > 20.0);
        let x = overlap_left + 12.0;
        let y = item[1] + item[3] / 2.0;
        assert!(y > composer[1] && y < composer[1] + composer[3]);
        let top = runtime.element_at(x, y).unwrap();
        assert!(top.path().contains("selection-menu"), "top={top:?}");
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(
            runtime.state.new_research_workspace_id.as_deref(),
            Some(first)
        );
        assert_eq!(runtime.state.workspace_id.unwrap().to_string(), second);
        assert!(runtime.find(&first_session).is_some());
        assert!(runtime.find(&second_session).is_none());
        let history = runtime.state.view_history.back.len();
        let (x, y) = runtime.find(&first_session).unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(runtime.state.primary_view, PrimaryView::NewResearch);
        assert_eq!(runtime.state.view_history.back.len(), history);
        assert_eq!(
            runtime.state.new_research_workspace_id.as_deref(),
            Some(first)
        );
        runtime.state.navigate_to(ViewLocation::Home);
        runtime.state.start_new_research();
        runtime.frame(0.0);
        assert_eq!(
            runtime.state.new_research_workspace_id.as_deref(),
            Some(first)
        );

        let mut late = State {
            primary_view: PrimaryView::NewResearch,
            workspace_id: Some(uuid::Uuid::parse_str(second).unwrap()),
            ..State::default()
        };
        assert!(late.apply_workspace_list(Ok(runtime.state.workspaces.clone())));
        assert_eq!(late.new_research_workspace_id.as_deref(), Some(second));
    }

    #[test]
    fn opening_saved_session_keeps_its_sidebar_row_without_a_draft_row() {
        let workspace_id = "00000000-0000-4000-8000-000000000001";
        let session_id = "00000000-0000-4000-8000-000000000002";
        let state = State {
            primary_view: PrimaryView::NewResearch,
            workspaces: vec![WorkspaceSummary {
                id: workspace_id.to_owned(),
                name: "Example Workspace".to_owned(),
                directory: "example-directory".to_owned(),
            }],
            session_summaries: vec![SessionSummary {
                id: session_id.to_owned(),
                workspace_id: workspace_id.to_owned(),
                title: "Example Session".to_owned(),
                last_message_at_ms: 1,
            }],
            new_research_workspace_id: Some(workspace_id.to_owned()),
            session_loading_id: Some(session_id.to_owned()),
            session_busy: true,
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (840.0, 600.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        assert!(
            runtime
                .find(&format!("home-session-{session_id}"))
                .is_some()
        );
        assert!(
            runtime
                .find(&format!("home-session-{workspace_id}"))
                .is_none()
        );
        assert!(runtime.find("new-research-ribbon").is_none());
    }

    #[test]
    fn completed_turn_reuses_the_draft_view_and_replaces_the_new_session_row() {
        let workspace_id = "00000000-0000-4000-8000-000000000001";
        let session_id = "00000000-0000-4000-8000-000000000002";
        let mut state = State {
            primary_view: PrimaryView::NewResearch,
            workspaces: vec![WorkspaceSummary {
                id: workspace_id.to_owned(),
                name: "Example Workspace".to_owned(),
                directory: "example-directory".to_owned(),
            }],
            new_research_workspace_id: Some(workspace_id.to_owned()),
            session_pending_prompt: Some("Echo example".to_owned()),
            session_busy: true,
            session_generation: 1,
            ..State::default()
        };
        let detail = SessionDetail {
            summary: Some(SessionSummary {
                id: session_id.to_owned(),
                workspace_id: workspace_id.to_owned(),
                title: "Echo example".to_owned(),
                last_message_at_ms: 1,
            }),
            messages: vec![
                beale_contracts::v1::SessionMessage {
                    role: beale_contracts::v1::SessionRole::User as i32,
                    text: "Echo example".to_owned(),
                    tools: Vec::new(),
                    reasoning_summaries: Vec::new(),
                    duration_ms: 0,
                    commentary: Vec::new(),
                },
                beale_contracts::v1::SessionMessage {
                    role: beale_contracts::v1::SessionRole::Assistant as i32,
                    text: "Echo example".to_owned(),
                    tools: Vec::new(),
                    reasoning_summaries: Vec::new(),
                    duration_ms: 1_234,
                    commentary: Vec::new(),
                },
            ],
            provider: ProviderId::Openai as i32,
            method: AuthMethod::Subscription as i32,
            model: "model-example".to_owned(),
            goal: None,
            first_turn_number: 1,
            graph_id: String::new(),
            graph_models: vec![],
            graph_node_concurrency: vec![],
        };
        state.finish_session_turn(1, Ok(detail));
        assert_eq!(state.primary_view, PrimaryView::NewResearch);
        assert!(!state.session_busy);
        assert!(state.session_pending_prompt.is_none());
        let mut runtime = Runtime::new(state, view, (840.0, 600.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        assert!(runtime.find("new-research-ribbon").is_none());
        assert!(runtime.find("new-research-workspace-control").is_none());
        assert!(
            runtime
                .find(&format!("home-session-{session_id}"))
                .is_some()
        );
        assert!(
            runtime
                .find(&format!("home-session-{workspace_id}"))
                .is_none()
        );
        assert!(runtime.find("session-message-0").is_some());
        assert!(runtime.find("session-message-1").is_some());
        let (user_row, _) = runtime.presented("session-message-0").unwrap();
        let (user_bubble, _) = runtime.presented("session-message-0-bubble").unwrap();
        let (assistant_row, _) = runtime.presented("session-message-1").unwrap();
        assert!(user_bubble[0] > user_row[0] + user_row[2] * 0.4);
        assert!((user_bubble[0] + user_bubble[2] - user_row[0] - user_row[2]).abs() < 1.0);
        assert_eq!(assistant_row[0], user_row[0]);
        let (x, y) = runtime.find("session-message-0-text").unwrap();
        runtime.pointer_move(x, y);
        runtime.frame(1.0 / 60.0);
        assert_eq!(runtime.cursor(), Cursor::Text);
        let clipboard = rune::clipboard::memory();
        runtime.use_clipboard(&clipboard);
        runtime.update(|_, tx| tx.select_text("session-message-0-text", 0..12));
        runtime.frame(2.0 / 60.0);
        assert_eq!(runtime.selected_text().as_deref(), Some("Echo example"));
        runtime.key(KeyEvent {
            key: Key::Command('c'),
            mods: rune::Mods {
                cmd: true,
                ..rune::Mods::default()
            },
        });
        runtime.frame(3.0 / 60.0);
        assert_eq!(
            clipboard.text().as_deref().map(String::as_str),
            Some("Echo example")
        );
        assert!(runtime.find("new-research-composer").is_some());
        let saved = runtime.state.active_session.clone().unwrap();
        let mut revisited = State {
            primary_view: PrimaryView::NewResearch,
            session_generation: 2,
            session_busy: true,
            ..State::default()
        };
        revisited.finish_session_load(2, Ok(saved));
        let mut reopened = Runtime::new(revisited, view, (840.0, 600.0), Env::new(true, 1.0));
        reopened.frame(0.0);
        assert!(reopened.find("new-research-ribbon").is_none());
        assert!(reopened.find("new-research-composer").is_some());
    }

    #[test]
    fn session_history_buttons_restore_the_current_draft() {
        let workspace_id = "00000000-0000-4000-8000-000000000001";
        let session_id = "00000000-0000-4000-8000-000000000002";
        let mut runtime = Runtime::new(
            State {
                primary_view: PrimaryView::NewResearch,
                workspaces: vec![WorkspaceSummary {
                    id: workspace_id.to_owned(),
                    name: "Example Workspace".to_owned(),
                    directory: "example-directory".to_owned(),
                }],
                new_research_workspace_id: Some(workspace_id.to_owned()),
                new_research_prompt: TextEdit::new("Unsent example"),
                active_session: Some(SessionDetail {
                    summary: Some(SessionSummary {
                        id: session_id.to_owned(),
                        workspace_id: workspace_id.to_owned(),
                        title: "Example Session".to_owned(),
                        last_message_at_ms: 1,
                    }),
                    messages: vec![
                        beale_contracts::v1::SessionMessage {
                            role: beale_contracts::v1::SessionRole::User as i32,
                            text: "First example".to_owned(),
                            ..Default::default()
                        },
                        beale_contracts::v1::SessionMessage {
                            role: beale_contracts::v1::SessionRole::Assistant as i32,
                            text: "First reply".to_owned(),
                            ..Default::default()
                        },
                        beale_contracts::v1::SessionMessage {
                            role: beale_contracts::v1::SessionRole::User as i32,
                            text: "Second example".to_owned(),
                            ..Default::default()
                        },
                    ],
                    provider: ProviderId::Openai as i32,
                    method: AuthMethod::Subscription as i32,
                    model: "model-example".to_owned(),
                    goal: None,
                    first_turn_number: 1,
                    graph_id: String::new(),
                    graph_models: vec![],
                    graph_node_concurrency: vec![],
                }),
                ..State::default()
            },
            view,
            (1040.0, 720.0),
            Env::new(true, 1.0),
        );
        runtime.frame(0.0);
        let click = |runtime: &mut Runtime<State>, key: &str| {
            let (x, y) = runtime.find(key).unwrap();
            runtime.pointer_down(x, y);
            runtime.pointer_up(x, y);
            runtime.frame(0.0);
        };
        click(&mut runtime, "session-history-previous");
        assert_eq!(runtime.state.new_research_prompt.text, "Second example");
        click(&mut runtime, "session-history-previous");
        assert_eq!(runtime.state.new_research_prompt.text, "First example");
        click(&mut runtime, "session-history-next");
        assert_eq!(runtime.state.new_research_prompt.text, "Second example");
        click(&mut runtime, "session-history-next");
        assert_eq!(runtime.state.new_research_prompt.text, "Unsent example");
        click(&mut runtime, "session-history-previous");
        let (x, y) = runtime.find("new-research-composer-input").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.paste(" edited");
        runtime.frame(0.0);
        assert!(!runtime.state.session_input_history.can_next());
        runtime.state.start_new_research();
        assert!(!runtime.state.session_input_history.can_next());
    }

    #[test]
    fn composer_ribbon_slides_down_when_a_session_starts() {
        let mut runtime = Runtime::new(
            State {
                primary_view: PrimaryView::NewResearch,
                ..State::default()
            },
            view,
            (840.0, 600.0),
            Env::new(true, 1.0),
        );
        runtime.frame(0.0);
        let (before, _) = runtime.presented("new-research-ribbon").unwrap();
        runtime.update(|state, _| state.session_busy = true);
        runtime.frame(0.0);
        runtime.frame(0.1);
        let (during, _) = runtime.presented("new-research-ribbon").unwrap();
        let (composer_during, _) = runtime.presented("new-research-composer").unwrap();
        assert!(during[1] > before[1]);
        assert!(during[1] + during[3] > composer_during[1]);
        runtime.frame(1.0);
        assert!(runtime.find("new-research-ribbon").is_none());
        assert!(runtime.find("new-research-workspace-control").is_none());
        assert!(runtime.find("new-research-repeat-control").is_none());
        assert!(runtime.find("new-research-composer").is_some());
    }

    #[test]
    fn session_transcript_formats_completed_and_streaming_markdown() {
        let mut state = State {
            primary_view: PrimaryView::NewResearch,
            active_session: Some(SessionDetail {
                summary: Some(SessionSummary {
                    id: "00000000-0000-4000-8000-000000000002".to_owned(),
                    workspace_id: "00000000-0000-4000-8000-000000000001".to_owned(),
                    title: "Example".to_owned(),
                    last_message_at_ms: 1,
                }),
                messages: vec![beale_contracts::v1::SessionMessage {
                    role: beale_contracts::v1::SessionRole::Assistant as i32,
                    text: "# Findings\n\n**Checked** `example.txt`.\n\n- First\n- Second\n\n```sh\nprintf ok\n```".to_owned(),
                    tools: Vec::new(),
                    reasoning_summaries: Vec::new(),
                    duration_ms: 1_000,
                    commentary: Vec::new(),
                }],
                provider: ProviderId::Openai as i32,
                method: AuthMethod::ApiKey as i32,
                model: "model-example".to_owned(),
                goal: None,
                first_turn_number: 1,
                graph_id: String::new(),
                graph_models: vec![],
                graph_node_concurrency: vec![],
            }),
            session_busy: true,
            session_live_events: vec![SessionStreamEvent {
                kind: SessionStreamKind::MessageDelta as i32,
                text: "**Streaming** `content` and\n\n```sh\nprintf".to_owned(),
                tool: None,
                detail: None,
                segment_index: 0,
            }],
            ..State::default()
        };
        state.session_work_expanded.insert(0);
        let mut runtime = Runtime::new(state, view, (840.0, 600.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        assert!(runtime.find("session-message-0-markdown").is_some());
        assert!(runtime.find("session-live-message-0-markdown").is_some());
        assert!(runtime.find("session-reasoning-unavailable-0").is_some());
        let text = runtime
            .elements()
            .into_iter()
            .filter_map(|element| element.text)
            .collect::<Vec<_>>()
            .join("\n");
        for expected in [
            "Findings",
            "Checked",
            "example.txt",
            "First",
            "Second",
            "printf ok",
            "Streaming",
            "content",
            "printf",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in {text:?}");
        }
        assert!(!text.contains("**Checked**"));
        assert!(!text.contains("```sh"));
    }

    #[test]
    fn session_tool_details_mount_only_when_expanded() {
        let workspace_id = "00000000-0000-4000-8000-000000000001";
        let session_id = "00000000-0000-4000-8000-000000000002";
        let tool_id = "00000000-0000-4000-8000-000000000003";
        let state = State {
            primary_view: PrimaryView::NewResearch,
            active_session: Some(SessionDetail {
                summary: Some(SessionSummary {
                    id: session_id.to_owned(),
                    workspace_id: workspace_id.to_owned(),
                    title: "Example".to_owned(),
                    last_message_at_ms: 1,
                }),
                messages: vec![beale_contracts::v1::SessionMessage {
                    role: beale_contracts::v1::SessionRole::Assistant as i32,
                    text: "Done".to_owned(),
                    tools: vec![beale_contracts::v1::SessionToolSummary {
                        id: tool_id.to_owned(),
                        name: "read".to_owned(),
                        summary: "Read example.txt".to_owned(),
                        is_error: false,
                    }],
                    reasoning_summaries: vec![beale_contracts::v1::SessionReasoningSummary {
                        text: "Checking the example file.".to_owned(),
                        after_tool_count: 0,
                    }],
                    duration_ms: 2_345,
                    commentary: vec![beale_contracts::v1::SessionCommentary {
                        text: "Reading the example file.".to_owned(),
                        after_tool_count: 0,
                    }],
                }],
                provider: ProviderId::Openai as i32,
                method: AuthMethod::ApiKey as i32,
                model: "model-example".to_owned(),
                goal: None,
                first_turn_number: 1,
                graph_id: String::new(),
                graph_models: vec![],
                graph_node_concurrency: vec![],
            }),
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (840.0, 600.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        assert!(runtime.find("session-work-header-0").is_some());
        assert!(runtime.find("session-message-0").is_some());
        assert!(runtime.find("session-reasoning-0-0").is_none());
        let (x, y) = runtime.find("session-work-header-0").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.5);
        assert!(runtime.find("session-reasoning-0-0").is_some());
        assert!(runtime.find("session-commentary-0-0").is_some());
        let (reasoning_bounds, _) = runtime.presented("session-reasoning-0-0").unwrap();
        let (tool_bounds, _) = runtime
            .presented(&format!("session-tool-button-{tool_id}"))
            .unwrap();
        assert!(reasoning_bounds[1] < tool_bounds[1]);
        assert!(
            runtime
                .presented(&format!("session-tool-button-{tool_id}"))
                .is_some()
        );
        assert!(
            runtime
                .presented(&format!("session-tool-detail-{tool_id}"))
                .is_none()
        );
        let (x, y) = runtime
            .find(&format!("session-tool-button-{tool_id}"))
            .unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(runtime.state.expanded_tool_id.as_deref(), Some(tool_id));
        assert!(
            runtime
                .presented(&format!("session-tool-detail-{tool_id}"))
                .is_some()
        );
    }

    #[test]
    fn streaming_progress_updates_the_visible_work_group() {
        let tool = beale_contracts::v1::SessionToolSummary {
            id: "00000000-0000-4000-8000-000000000003".to_owned(),
            name: "read".to_owned(),
            summary: "Read example.txt".to_owned(),
            is_error: false,
        };
        let mut state = State {
            primary_view: PrimaryView::NewResearch,
            session_busy: true,
            session_generation: 7,
            session_pending_prompt: Some("Inspect example.txt".to_owned()),
            ..State::default()
        };
        for text in ["Checking ", "example"] {
            state.apply_session_progress(
                7,
                SessionStreamEvent {
                    kind: SessionStreamKind::ReasoningDelta as i32,
                    text: text.to_owned(),
                    tool: None,
                    detail: None,
                    segment_index: 1,
                },
            );
        }
        state.apply_session_progress(
            7,
            SessionStreamEvent {
                kind: SessionStreamKind::ToolStarted as i32,
                text: String::new(),
                tool: Some(tool.clone()),
                detail: None,
                segment_index: 0,
            },
        );
        state.apply_session_progress(
            7,
            SessionStreamEvent {
                kind: SessionStreamKind::ToolFinished as i32,
                text: String::new(),
                tool: Some(tool),
                detail: None,
                segment_index: 0,
            },
        );
        state.apply_session_progress(
            6,
            SessionStreamEvent {
                kind: SessionStreamKind::MessageDelta as i32,
                text: "stale".to_owned(),
                tool: None,
                detail: None,
                segment_index: 1,
            },
        );
        assert_eq!(state.session_live_events.len(), 2);
        assert_eq!(state.session_live_events[0].text, "Checking example");
        assert_eq!(
            state.session_live_events[1].kind,
            SessionStreamKind::ToolFinished as i32
        );
        let mut runtime = Runtime::new(state, view, (840.0, 600.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        assert!(runtime.find("session-current-work-header").is_some());
        assert!(runtime.find("session-live-reasoning-0").is_some());
        assert!(
            runtime
                .find("session-live-tool-00000000-0000-4000-8000-000000000003")
                .is_some()
        );
    }

    #[test]
    fn new_research_model_selector_changes_only_the_draft() {
        let state = State {
            primary_view: PrimaryView::Home,
            auth_loaded: true,
            lead_provider: Some(Provider::OpenAi),
            auth_status: [
                AuthStatus {
                    subscription: true,
                    ..AuthStatus::default()
                },
                AuthStatus {
                    subscription: true,
                    ..AuthStatus::default()
                },
                AuthStatus::default(),
                AuthStatus::default(),
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
                Some(ModelCatalog {
                    models: vec![provider_models::ModelEntry {
                        id: "example-xai".to_owned(),
                        name: "Example xAI".to_owned(),
                    }],
                    lead: "example-xai".to_owned(),
                    small: "example-xai".to_owned(),
                }),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            ],
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (840.0, 600.0), Env::new(true, 1.0));
        runtime.on_key = Some(selection_menu_key);
        runtime.frame(0.0);
        let (x, y) = runtime.find("home-new-research").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert!(runtime.state.new_research_model.is_none());
        let (x, y) = runtime.find("new-research-model-control").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert!(runtime.state.composer_menu_open);
        assert_eq!(
            runtime.state.composer_menu_section,
            new_research::MenuSection::Model
        );
        assert!(runtime.find("composer-menu-provider").is_some());
        assert!(runtime.find("composer-menu-model").is_some());
        assert!(runtime.find("composer-menu-effort").is_some());
        assert!(runtime.find("composer-menu-option-1").is_some());
        let (menu, _) = runtime.presented("composer-model-menu").unwrap();
        assert!(menu[0] >= 0.0 && menu[0] + menu[2] <= 840.0);
        assert!(menu[1] >= 0.0 && menu[1] + menu[3] <= 600.0);
        let (main_at_model, _) = runtime.presented("composer-model-main").unwrap();
        let (provider_row, _) = runtime.presented("composer-menu-provider").unwrap();
        let (option_row, _) = runtime.presented("composer-menu-option-0").unwrap();
        assert!((provider_row[3] - option_row[3]).abs() < 1.0);
        assert!((provider_row[1] - main_at_model[1] - 8.0).abs() < 1.0);
        let (control, _) = runtime.presented("new-research-model-control").unwrap();
        assert!((control[1] - (main_at_model[1] + main_at_model[3]) - 6.0).abs() < 1.0);
        let (x, y) = runtime.find("composer-menu-provider").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(
            runtime.state.composer_menu_section,
            new_research::MenuSection::Provider
        );
        let (main_at_provider, _) = runtime.presented("composer-model-main").unwrap();
        assert!((main_at_provider[1] - main_at_model[1]).abs() < 1.0);
        assert!(runtime.find("composer-menu-option-1").is_some());
        let (x, y) = runtime.find("composer-menu-option-1").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert!(!runtime.state.composer_menu_open);
        assert_eq!(
            runtime.state.new_research_model.as_ref().unwrap().provider,
            Provider::Xai
        );
        assert_eq!(
            runtime.state.new_research_model.as_ref().unwrap().id,
            "example-xai"
        );
        let (x, y) = runtime.find("new-research-model-control").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert!(runtime.state.composer_menu_open);
        assert!(runtime.find("composer-menu-option-1").is_none());
        let (x, y) = runtime.find("composer-menu-provider").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        let (x, y) = runtime.find("composer-menu-option-0").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(
            runtime.state.new_research_model.as_ref().unwrap().provider,
            Provider::OpenAi
        );
        let (x, y) = runtime.find("new-research-model-control").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        let (x, y) = runtime.find("composer-menu-option-1").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(
            runtime.state.new_research_model.as_ref().unwrap().id,
            "example-small"
        );
        let (x, y) = runtime.find("new-research-model-control").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        runtime.key(KeyEvent {
            key: Key::Escape,
            mods: Default::default(),
        });
        runtime.frame(0.0);
        assert!(!runtime.state.composer_menu_open);
        assert_eq!(runtime.state.lead_provider, Some(Provider::OpenAi));
        assert_eq!(
            runtime.state.model_catalogs[0].as_ref().unwrap().lead,
            "example-lead"
        );
        assert_eq!(
            runtime.state.new_research_effort,
            new_research::Effort::High
        );
        let (x, y) = runtime.find("new-research-model-control").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        let (main_before, _) = runtime.presented("composer-model-main").unwrap();
        let (x, y) = runtime.find("composer-menu-effort").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(
            runtime.state.composer_menu_section,
            new_research::MenuSection::Effort
        );
        let (main_after, _) = runtime.presented("composer-model-main").unwrap();
        let (submenu, _) = runtime.presented("composer-model-submenu").unwrap();
        assert!((main_after[3] - main_before[3]).abs() < 1.0);
        assert!((main_after[1] - main_before[1]).abs() < 1.0);
        assert!(submenu[3] > main_after[3]);
        assert!((submenu[1] + submenu[3] - main_after[1] - main_after[3]).abs() < 1.0);
        let (x, y) = runtime.find("composer-menu-option-2").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(runtime.state.new_research_effort, new_research::Effort::Low);
        assert_eq!(
            runtime.state.new_research_effort.wire(),
            beale_contracts::v1::ReasoningEffort::Low
        );
        assert!(!runtime.state.composer_menu_open);
        assert_eq!(
            runtime.state.new_research_model.as_ref().unwrap().id,
            "example-small"
        );
    }

    #[test]
    fn host_workspaces_appear_in_home_sidebar_and_creation_result_returns_home() {
        let summary = WorkspaceSummary {
            id: "00000000-0000-4000-8000-000000000001".to_owned(),
            name: "Example Workspace".to_owned(),
            directory: std::env::temp_dir()
                .join("example-workspace")
                .to_string_lossy()
                .into_owned(),
        };
        let mut state = State {
            primary_view: PrimaryView::WorkspaceCreation,
            connection: ConnectionStatus::Connected { major: 1, minor: 6 },
            workspace_create_busy: true,
            ..State::default()
        };
        state.workspace_draft.name = TextEdit::new("Draft Workspace");
        state.workspace_draft.tab = workspace_creation::CreationTab::Rules;
        let mut runtime = Runtime::new(state, view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        assert!(runtime.find("workspace-create-disabled").is_some());
        assert_eq!(runtime.state.primary_view, PrimaryView::WorkspaceCreation);
        runtime
            .state
            .finish_workspace_create(Err("Could not create workspace".to_owned()));
        runtime.frame(1.0 / 60.0);
        assert_eq!(
            runtime.state.workspace_create_error,
            "Could not create workspace"
        );
        assert_eq!(runtime.state.workspace_draft.name.text, "Draft Workspace");
        let mut retry = Runtime::new(runtime.state, view, (1040.0, 720.0), Env::new(true, 1.0));
        retry.frame(0.0);
        assert!(retry.find("workspace-create-error").is_some());
        assert!(retry.find("workspace-create").is_some());
        retry.state.workspace_create_busy = true;
        retry.state.finish_workspace_create(Ok(summary.clone()));
        assert_eq!(retry.state.primary_view, PrimaryView::Home);
        assert_eq!(retry.state.workspaces, vec![summary.clone()]);
        assert!(!retry.state.apply_workspace_list(Ok(Vec::new())));
        assert_eq!(retry.state.workspaces, vec![summary.clone()]);
        assert!(!retry.state.apply_workspace_list(Ok(vec![summary])));
        assert!(retry.state.pending_workspace_id.is_none());
        assert!(retry.state.workspace_draft.name.text.is_empty());
        let mut home = Runtime::new(retry.state, view, (1040.0, 720.0), Env::new(true, 1.0));
        home.frame(0.0);
        assert!(
            home.find("home-workspace-00000000-0000-4000-8000-000000000001")
                .is_some()
        );
        assert!(home.find("home-workspaces-empty").is_none());
    }

    #[test]
    fn workspace_sessions_show_five_rows_until_expanded() {
        let workspace_id = "00000000-0000-4000-8000-000000000010";
        let state = State {
            primary_view: PrimaryView::Home,
            workspaces: vec![WorkspaceSummary {
                id: workspace_id.to_owned(),
                name: "Example Workspace".to_owned(),
                directory: "example-directory".to_owned(),
            }],
            session_summaries: (1..=6)
                .map(|index| SessionSummary {
                    id: format!("00000000-0000-4000-8000-{index:012x}"),
                    workspace_id: workspace_id.to_owned(),
                    title: format!("Example session {index}"),
                    last_message_at_ms: 1_800_000_000_000,
                })
                .collect(),
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (1040.0, 720.0), Env::new(true, 1.0));
        let fifth = "home-session-container-00000000-0000-4000-8000-000000000005";
        let sixth = "home-session-container-00000000-0000-4000-8000-000000000006";
        let toggle = format!("home-session-toggle-{workspace_id}");
        runtime.frame(0.0);
        assert!(runtime.find(fifth).is_some());
        assert!(runtime.find(sixth).is_none());
        let (x, y) = runtime.find(&toggle).unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0 / 60.0);
        assert!(runtime.find(sixth).is_some());
        let (x, y) = runtime.find(&toggle).unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0 / 60.0);
        assert!(runtime.find(sixth).is_none());
    }

    #[test]
    fn saved_workspace_opens_campaign_and_shows_recorded_forms() {
        use beale_contracts::v1::{SurfaceCreator, SurfaceKind, SurfaceScope, WorkspaceSurface};
        let id = uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000002").unwrap();
        let summary = WorkspaceSummary {
            id: id.to_string(),
            name: "Example Workspace".to_owned(),
            directory: std::env::temp_dir()
                .join("example-workspace")
                .to_string_lossy()
                .into_owned(),
        };
        let state = State {
            primary_view: PrimaryView::Home,
            workspaces: vec![summary.clone()],
            workspace_list_loaded: true,
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (2200.0, 900.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let (x, y) = runtime.find(&format!("home-workspace-{id}")).unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0 / 60.0);
        assert_eq!(runtime.state.primary_view, PrimaryView::Workspace);
        assert_eq!(runtime.state.workspace_id, Some(id));
        assert!(runtime.find("workspace-campaign-empty").is_some());
        for key in [
            "workspace-tab-campaign",
            "workspace-tab-foundation",
            "workspace-tab-surfaces",
            "workspace-tab-rules",
        ] {
            assert!(runtime.find(key).is_some(), "{key}");
        }
        let tab_positions = [
            "workspace-tab-campaign",
            "workspace-tab-foundation",
            "workspace-tab-surfaces",
            "workspace-tab-rules",
        ]
        .map(|key| runtime.presented(key).unwrap().0[0]);
        assert!(tab_positions.windows(2).all(|pair| pair[0] < pair[1]));
        let details = WorkspaceDetails {
            summary: Some(summary),
            guidance: "Use only example.test.\n".to_owned(),
            surfaces: vec![WorkspaceSurface {
                kind: SurfaceKind::Domain as i32,
                scope: SurfaceScope::InScope as i32,
                value: "example.test".to_owned(),
                created_by: SurfaceCreator::Agent as i32,
                out_of_scope_reason: String::new(),
            }],
            rules: vec!["Stay within scope.".to_owned()],
        };
        let generation = runtime.state.workspace_details_generation;
        runtime.state.finish_workspace_details(
            id,
            generation,
            Ok(workspaces::WorkspaceSections {
                details,
                foundation_revision: 1,
                surfaces_revision: 1,
                rules_revision: 1,
                campaign: beale_contracts::v1::WorkspaceCampaign {
                    enabled_feature_ids: Vec::new(),
                },
            }),
        );
        let mut loaded = Runtime::new(runtime.state, view, (2200.0, 900.0), Env::new(true, 1.0));
        loaded.frame(0.0);
        let (content_rect, _) = loaded.presented("settings-page").unwrap();
        assert!(content_rect[2] >= 1299.0 && content_rect[2] <= 1301.0);
        let (workspace_title, _) = loaded.presented("workspace-view-title").unwrap();
        let mut settings =
            Runtime::new(State::default(), view, (2200.0, 900.0), Env::new(true, 1.0));
        settings.frame(0.0);
        let (settings_title, _) = settings.presented("settings-view-title").unwrap();
        assert!((workspace_title[1] - settings_title[1]).abs() < 1.0);
        for (tab, expected) in [
            (
                "workspace-tab-surfaces",
                "saved-workspace-in-scope-surfaces",
            ),
            ("workspace-tab-rules", "saved-workspace-rules-form"),
            ("workspace-tab-foundation", "saved-workspace-details-form"),
        ] {
            let (x, y) = loaded.find(tab).unwrap();
            loaded.pointer_down(x, y);
            loaded.pointer_up(x, y);
            loaded.frame(1.0 / 60.0);
            assert!(loaded.find(expected).is_some(), "{expected}");
            if tab == "workspace-tab-surfaces" {
                let (x, y) = loaded.find("saved-workspace-add-in-scope-surface").unwrap();
                loaded.pointer_down(x, y);
                loaded.pointer_up(x, y);
                loaded.frame(1.0);
                assert!(loaded.find("saved-workspace-surface-dialog").is_some());
                let (x, y) = loaded.find("saved-workspace-surface-kind-control").unwrap();
                loaded.pointer_down(x, y);
                loaded.pointer_up(x, y);
                loaded.frame(1.0);
                assert_eq!(
                    loaded.state.workspace_surface_dialog_scope,
                    Some(workspace_creation::SurfaceScope::InScope)
                );
                assert_eq!(
                    loaded.state.selection_menu,
                    Some(SelectionMenu::WorkspaceSurfaceKind)
                );
                let (x, y) = loaded.find("selection-menu-item-4").unwrap();
                loaded.pointer_down(x, y);
                loaded.pointer_up(x, y);
                loaded.frame(1.0 / 60.0);
                assert_eq!(
                    loaded.state.workspace_surface_kind,
                    workspace_creation::SurfaceKind::Domain
                );
                loaded.state.workspace_surface_value = TextEdit::new("another.example.test");
                let (x, y) = loaded.find("saved-workspace-add-surface").unwrap();
                loaded.pointer_down(x, y);
                loaded.pointer_up(x, y);
                loaded.frame(1.0 / 60.0);
                assert_eq!(loaded.state.workspace_surface_edits.len(), 2);
                assert_eq!(
                    loaded.state.workspace_surface_edits[1].scope,
                    SurfaceScope::InScope as i32
                );
                let (x, y) = loaded.find("saved-workspace-surface-edit-0").unwrap();
                loaded.pointer_down(x, y);
                loaded.pointer_up(x, y);
                loaded.frame(1.0);
                assert_eq!(loaded.state.workspace_surface_edit_index, Some(0));
                assert_eq!(loaded.state.workspace_surface_value.text, "example.test");
                loaded.state.workspace_surface_value = TextEdit::new("another.example.test");
                let (x, y) = loaded.find("saved-workspace-edit-surface").unwrap();
                loaded.pointer_down(x, y);
                loaded.pointer_up(x, y);
                loaded.frame(1.0);
                assert!(
                    loaded
                        .find("saved-workspace-surface-dialog-error")
                        .is_some()
                );
                loaded.state.workspace_surface_value = TextEdit::new("primary.example.test");
                let (x, y) = loaded.find("saved-workspace-edit-surface").unwrap();
                loaded.pointer_down(x, y);
                loaded.pointer_up(x, y);
                loaded.frame(1.0);
                assert_eq!(
                    loaded.state.workspace_surface_edits[0].value,
                    "primary.example.test"
                );
                assert_eq!(
                    loaded.state.workspace_surface_edits[0].created_by,
                    SurfaceCreator::User as i32
                );
                assert!(loaded.state.workspace_surface_edit_index.is_none());
                let (x, y) = loaded.find("saved-workspace-surface-remove-1").unwrap();
                loaded.pointer_down(x, y);
                loaded.pointer_up(x, y);
                loaded.frame(1.0);
                assert!(
                    loaded
                        .find("saved-workspace-surface-delete-dialog")
                        .is_some()
                );
                assert_eq!(loaded.state.workspace_surface_edits.len(), 2);
                let (x, y) = loaded
                    .find("saved-workspace-surface-delete-cancel")
                    .unwrap();
                loaded.pointer_down(x, y);
                loaded.pointer_up(x, y);
                loaded.frame(1.0);
                assert!(
                    loaded
                        .find("saved-workspace-surface-delete-dialog")
                        .is_none()
                );
                assert_eq!(loaded.state.workspace_surface_edits.len(), 2);
                let (x, y) = loaded.find("saved-workspace-surface-remove-1").unwrap();
                loaded.pointer_down(x, y);
                loaded.pointer_up(x, y);
                loaded.frame(1.0);
                loaded.key(KeyEvent {
                    key: Key::Escape,
                    mods: Default::default(),
                });
                loaded.frame(0.0);
                assert!(
                    loaded
                        .find("saved-workspace-surface-delete-dialog")
                        .is_none()
                );
                assert_eq!(loaded.state.workspace_surface_edits.len(), 2);
                let (x, y) = loaded.find("saved-workspace-surface-remove-1").unwrap();
                loaded.pointer_down(x, y);
                loaded.pointer_up(x, y);
                loaded.frame(1.0);
                let (x, y) = loaded
                    .find("saved-workspace-surface-delete-confirm")
                    .unwrap();
                loaded.pointer_down(x, y);
                loaded.pointer_up(x, y);
                loaded.frame(1.0);
                assert_eq!(loaded.state.workspace_surface_edits.len(), 1);
                assert!(
                    loaded
                        .find("saved-workspace-surface-delete-dialog")
                        .is_none()
                );
            } else if tab == "workspace-tab-rules" {
                assert!(loaded.find("saved-workspace-rule-editor").is_none());
                assert!(loaded.find("saved-workspace-rule-input").is_none());
                let (x, y) = loaded.find("saved-workspace-add-rule").unwrap();
                loaded.pointer_down(x, y);
                loaded.pointer_up(x, y);
                loaded.frame(1.0);
                assert!(loaded.find("saved-workspace-rule-dialog").is_some());
                let (x, y) = loaded.find("saved-workspace-rule-dialog-add").unwrap();
                loaded.pointer_down(x, y);
                loaded.pointer_up(x, y);
                loaded.frame(1.0);
                assert!(loaded.find("saved-workspace-rule-dialog-error").is_some());
                assert_eq!(loaded.state.workspace_rule_edits.len(), 1);
                let (x, y) = loaded.find("saved-workspace-rule-dialog-cancel").unwrap();
                loaded.pointer_down(x, y);
                loaded.pointer_up(x, y);
                loaded.frame(1.0);
                assert!(loaded.find("saved-workspace-rule-dialog").is_none());
                let (x, y) = loaded.find("saved-workspace-add-rule").unwrap();
                loaded.pointer_down(x, y);
                loaded.pointer_up(x, y);
                loaded.frame(1.0);
                loaded.state.workspace_rule_input = TextEdit::new("Use only example.test.");
                let (x, y) = loaded.find("saved-workspace-rule-dialog-add").unwrap();
                loaded.pointer_down(x, y);
                loaded.pointer_up(x, y);
                loaded.frame(1.0);
                assert_eq!(loaded.state.workspace_rule_edits.len(), 2);
                assert!(loaded.find("saved-workspace-rule-dialog").is_none());
            }
        }
        assert!(loaded.find("saved-workspace-name-input").is_some());
        assert!(loaded.find("saved-workspace-guidance-input").is_some());
        assert!(loaded.find("saved-workspace-save-disabled").is_some());
        loaded.state.workspace_name_edit = TextEdit::new("Renamed Example Workspace");
        let mut edited = Runtime::new(loaded.state, view, (2200.0, 900.0), Env::new(true, 1.0));
        edited.frame(0.0);
        assert!(edited.find("saved-workspace-save").is_some());
        edited.state.workspace_save_busy = Some(workspace_view::WorkspaceTab::Foundation);
        let mut saved_summary = edited
            .state
            .workspace_details
            .as_ref()
            .and_then(|details| details.summary.clone())
            .unwrap();
        saved_summary.name = "Renamed Example Workspace".to_owned();
        edited.state.finish_workspace_save(
            id,
            workspace_view::WorkspaceTab::Foundation,
            Ok(WorkspaceSaveResult::Foundation(WorkspaceFoundation {
                summary: Some(saved_summary),
                guidance: "Use only example.test.\n".to_owned(),
                revision: 2,
            })),
        );
        assert_eq!(edited.state.workspace_foundation_revision, 2);
        assert_eq!(edited.state.workspaces[0].name, "Renamed Example Workspace");
    }

    #[test]
    fn new_workspace_draft_moves_through_foundation_surfaces_and_rules() {
        fn click(runtime: &mut Runtime<State>, key: &str) {
            let (x, y) = runtime.find(key).unwrap();
            runtime.pointer_down(x, y);
            runtime.pointer_up(x, y);
            runtime.frame(1.0);
        }
        let mut runtime =
            Runtime::new(State::default(), view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let (settings_title, _) = runtime.presented("settings-view-title").unwrap();
        click(&mut runtime, "nav-home");
        click(&mut runtime, "home-add-workspace");
        assert_eq!(runtime.state.primary_view, PrimaryView::WorkspaceCreation);
        assert!(runtime.find("workspace-creation-title").is_some());
        let (creation_title, _) = runtime.presented("workspace-creation-title").unwrap();
        assert!((creation_title[1] - settings_title[1]).abs() < 1.0);
        assert!((creation_title[3] - settings_title[3]).abs() < 1.0);
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
            workspace_creation::CreationTab::Foundation
        );
        assert!(runtime.find("workspace-creation-error").is_some());
        runtime.state.workspace_draft.name = TextEdit::new("Example Workspace");
        runtime.frame(0.0);
        click(&mut runtime, "workspace-next");
        assert_eq!(
            runtime.state.workspace_draft.tab,
            workspace_creation::CreationTab::Surfaces
        );
        assert!(runtime.find("workspace-surface-dialog").is_none());
        assert!(runtime.find("workspace-add-in-scope-surface").is_some());
        assert!(runtime.find("workspace-add-out-of-scope-surface").is_some());
        click(&mut runtime, "workspace-add-in-scope-surface");
        assert!(runtime.find("workspace-surface-dialog").is_some());
        assert!(runtime.find("workspace-surface-scope-control").is_none());
        assert_eq!(
            runtime.state.workspace_draft.surface_dialog_scope,
            Some(workspace_creation::SurfaceScope::InScope),
        );
        click(&mut runtime, "workspace-add-surface");
        assert!(runtime.find("workspace-surface-dialog-error").is_some());
        click(&mut runtime, "workspace-surface-dialog-cancel");
        assert!(runtime.find("workspace-surface-dialog").is_none());
        assert!(runtime.state.workspace_draft.surfaces.is_empty());
        click(&mut runtime, "workspace-add-in-scope-surface");
        runtime.key(KeyEvent {
            key: Key::Escape,
            mods: Default::default(),
        });
        runtime.frame(0.0);
        assert!(runtime.find("workspace-surface-dialog").is_none());
        click(&mut runtime, "workspace-add-in-scope-surface");
        runtime.pointer_down(10.0, 350.0);
        runtime.pointer_up(10.0, 350.0);
        runtime.frame(0.0);
        assert!(runtime.find("workspace-surface-dialog").is_none());
        click(&mut runtime, "workspace-add-in-scope-surface");
        click(&mut runtime, "workspace-surface-kind-control");
        assert!(runtime.find("selection-menu-item-4").is_some());
        click(&mut runtime, "selection-menu-item-4");
        assert!(runtime.state.selection_menu.is_none());
        assert!(runtime.find("workspace-surface-dialog").is_some());
        runtime.state.workspace_draft.surface_value = TextEdit::new("example.test");
        assert_eq!(
            runtime.state.workspace_draft.surface_kind,
            workspace_creation::SurfaceKind::Domain,
        );
        runtime.frame(0.0);
        click(&mut runtime, "workspace-add-surface");
        assert_eq!(runtime.state.workspace_draft.surfaces.len(), 1);
        assert_eq!(
            runtime.state.workspace_draft.surfaces[0].scope,
            workspace_creation::SurfaceScope::InScope,
        );
        assert!(runtime.find("workspace-surface-dialog").is_none());
        assert_eq!(
            runtime.state.workspace_draft.surfaces[0].created_by,
            workspace_creation::SurfaceCreator::User,
        );
        assert_eq!(
            runtime.state.workspace_draft.surfaces[0].out_of_scope_reason,
            None,
        );
        click(&mut runtime, "workspace-add-out-of-scope-surface");
        assert_eq!(
            runtime.state.workspace_draft.surface_dialog_scope,
            Some(workspace_creation::SurfaceScope::OutOfScope),
        );
        assert!(runtime.find("workspace-surface-scope-control").is_none());
        runtime.state.workspace_draft.surface_value = TextEdit::new("excluded.example.test");
        runtime.frame(0.0);
        click(&mut runtime, "workspace-add-surface");
        assert_eq!(runtime.state.workspace_draft.surfaces.len(), 2);
        assert_eq!(
            runtime.state.workspace_draft.surfaces[1].scope,
            workspace_creation::SurfaceScope::OutOfScope,
        );
        assert_eq!(
            runtime.state.workspace_draft.surfaces[1]
                .out_of_scope_reason
                .as_deref(),
            Some("Set at workspace creation"),
        );
        assert!(runtime.find("workspace-in-scope-surfaces").is_some());
        assert!(runtime.find("workspace-out-of-scope-surfaces").is_some());
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
        assert_eq!(runtime.state.primary_view, PrimaryView::WorkspaceCreation);
        assert!(runtime.find("workspace-cancel-dialog").is_some());
        assert_eq!(runtime.state.workspace_draft.name.text, "Example Workspace");
        click(&mut runtime, "workspace-cancel-keep-editing");
        assert!(runtime.find("workspace-cancel-dialog").is_none());
        assert_eq!(runtime.state.workspace_draft.rules.len(), 1);
        click(&mut runtime, "workspace-cancel");
        runtime.key(KeyEvent {
            key: Key::Escape,
            mods: Default::default(),
        });
        runtime.frame(0.0);
        assert!(runtime.find("workspace-cancel-dialog").is_none());
        assert_eq!(runtime.state.workspace_draft.surfaces.len(), 2);
        click(&mut runtime, "workspace-cancel");
        click(&mut runtime, "workspace-cancel-discard");
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

        let mut runtime = Runtime::new(
            State {
                settings_section: SettingsSection::Providers,
                ..State::default()
            },
            view,
            (1040.0, 720.0),
            Env::new(true, 1.0),
        );
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
        click(&mut runtime, "settings-safety");
        assert!(runtime.state.view_history.forward.is_empty());
        click(&mut runtime, "header-forward");
        assert_eq!(
            runtime.state.view_location(),
            ViewLocation::Safety(permission_settings::PermissionTab::AutoReview)
        );
        assert!(runtime.state.view_history.forward.is_empty());
    }

    #[test]
    fn navigation_tooltips_follow_their_icons() {
        let mut runtime =
            Runtime::new(State::default(), view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.help_delay = 0.05;
        runtime.frame(0.0);
        for (key, label) in [
            ("header-sidebar-toggle", "Hide Sidebar"),
            ("header-back", "Back"),
            ("header-forward", "Forward"),
        ] {
            let (x, y) = runtime.find(key).unwrap();
            runtime.pointer_move(x, y);
            runtime.frame(0.0);
            runtime.frame(0.1);
            let (actual, tip) = runtime.help_tooltip().unwrap();
            let (button, _) = runtime.presented(key).unwrap();
            assert_eq!(actual, label);
            assert!(tip[1] >= button[1] + button[3]);
        }
        let (x, y) = runtime.find("nav-home").unwrap();
        runtime.pointer_move(x, y);
        runtime.frame(0.4);
        let (button, _) = runtime.presented("nav-home").unwrap();
        let (tooltip, _) = runtime.presented("rail-tooltip").unwrap();
        assert!(tooltip[0] >= button[0] + button[2]);
        runtime.pointer_move(500.0, 500.0);
        runtime.frame(0.0);
        assert!(runtime.find("rail-tooltip").is_none());
    }

    #[test]
    fn subscription_circles_anchor_to_rail_bottom_and_show_details() {
        let state = State {
            provider_usage: [
                Some(beale_contracts::v1::ProviderUsageSnapshot {
                    provider: ProviderId::Openai as i32,
                    checked_at_unix: 2_000_000_000,
                    windows: vec![
                        beale_contracts::v1::ProviderUsageWindow {
                            id: "primary".into(),
                            label: "Session Limit".into(),
                            used_percent: 25,
                            resets_at_unix: 2_000_001_000,
                        },
                        beale_contracts::v1::ProviderUsageWindow {
                            id: "secondary".into(),
                            label: "Weekly Limit".into(),
                            used_percent: 40,
                            resets_at_unix: 2_000_002_000,
                        },
                    ],
                }),
                None,
            ],
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let (circle, _) = runtime.presented("usage-openai").unwrap();
        let (settings, _) = runtime.presented("nav-settings").unwrap();
        assert!(circle[1] > settings[1] + 200.0);
        assert!(circle[1] + circle[3] <= 720.0);
        let (x, y) = runtime.find("usage-openai").unwrap();
        runtime.pointer_move(x, y);
        runtime.frame(0.0);
        assert!(runtime.presented("usage-tooltip").is_some());
        assert!(runtime.presented("usage-xai").is_none());
    }

    #[test]
    fn workspace_row_new_research_opens_a_draft_for_that_workspace() {
        let workspace_id = "00000000-0000-4000-8000-000000000003";
        let state = State {
            primary_view: PrimaryView::Home,
            workspaces: vec![WorkspaceSummary {
                id: workspace_id.to_owned(),
                name: "Example Workspace".to_owned(),
                directory: "example-directory".to_owned(),
            }],
            workspace_list_loaded: true,
            ..State::default()
        };
        let mut runtime = Runtime::new(state, view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let (workspace_row, _) = runtime
            .presented(&format!("home-workspace-{workspace_id}"))
            .unwrap();
        let action_key = format!("home-new-research-{workspace_id}");
        let (action, _) = runtime.presented(&action_key).unwrap();
        assert!(action[0] + action[2] <= workspace_row[0] + workspace_row[2]);
        let (x, y) = runtime.find(&action_key).unwrap();
        runtime.pointer_move(x, y);
        runtime.frame(0.0);
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert_eq!(runtime.state.primary_view, PrimaryView::NewResearch);
        assert_eq!(
            runtime.state.new_research_workspace_id.as_deref(),
            Some(workspace_id)
        );
        assert!(runtime.state.workspace_id.is_none());
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
    fn fleet_opens_below_the_conditional_rail_divider() {
        let mut state = State {
            primary_view: PrimaryView::Home,
            ..State::default()
        };
        let mut disabled =
            Runtime::new(State::default(), view, (1040.0, 720.0), Env::new(true, 1.0));
        disabled.frame(0.0);
        assert!(disabled.find("nav-fleet").is_none());
        assert!(disabled.find("navigation-conditional-divider").is_none());
        state
            .features
            .iter_mut()
            .find(|feature| feature.id == "fleet")
            .unwrap()
            .enabled = true;
        let mut runtime = Runtime::new(state, view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let (settings, _) = runtime.presented("nav-settings").unwrap();
        let (divider, _) = runtime.presented("navigation-conditional-divider").unwrap();
        let (fleet, _) = runtime.presented("nav-fleet").unwrap();
        assert!(settings[1] + settings[3] < divider[1]);
        assert!(divider[1] + divider[3] < fleet[1]);

        let (x, y) = runtime.find("nav-fleet").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert_eq!(runtime.state.primary_view, PrimaryView::Fleet);
        assert!(runtime.find("fleet-sidenav-title").is_some());
        assert!(runtime.find("fleet-view-title").is_some());
        assert_eq!(
            runtime.state.fleet_section,
            fleet_view::FleetSection::AppServers
        );
        assert!(runtime.find("fleet-app-servers").is_some());
        assert!(runtime.find("fleet-browsers").is_some());
        assert!(runtime.find("fleet-virtual-machines").is_some());
        assert!(runtime.find("fleet-server-card-local").is_some());
        assert!(runtime.find("fleet-local-qr").is_some());
        assert!(runtime.find("fleet-tab-all").is_some());
        assert!(runtime.find("fleet-tab-available").is_some());
        assert!(runtime.find("fleet-tab-enabled").is_some());
        let (x, y) = runtime.find("fleet-tab-available").unwrap();
        runtime.pointer_move(x, y);
        runtime.frame(1.0);
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert_eq!(runtime.state.fleet.tab, fleet_view::FleetTab::Available);
        assert!(runtime.find("home-no-workspace").is_none());

        let (x, y) = runtime.find("fleet-browsers").unwrap();
        runtime.pointer_move(x, y);
        runtime.frame(1.0);
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert_eq!(
            runtime.state.fleet_section,
            fleet_view::FleetSection::Browsers
        );
        assert_eq!(runtime.state.fleet.tab, fleet_view::FleetTab::All);
        assert!(runtime.find("fleet-tab-all").is_some());
        assert!(runtime.find("fleet-tab-available").is_some());
        assert!(runtime.find("fleet-tab-enabled").is_some());

        let (x, y) = runtime.find("fleet-add-browser").unwrap();
        runtime.pointer_move(x, y);
        runtime.frame(1.0);
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert!(runtime.state.fleet.browser_dialog_open);
        assert!(runtime.find("fleet-browser-name").is_some());
        assert!(runtime.find("fleet-browser-path").is_some());
        let (x, y) = runtime.find("fleet-browser-cancel").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert!(!runtime.state.fleet.browser_dialog_open);

        let (x, y) = runtime.find("fleet-virtual-machines").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert_eq!(
            runtime.state.fleet_section,
            fleet_view::FleetSection::VirtualMachines
        );
        assert_eq!(runtime.state.fleet.tab, fleet_view::FleetTab::All);
        assert!(runtime.find("fleet-tab-all").is_some());
        assert!(runtime.find("fleet-tab-available").is_some());
        assert!(runtime.find("fleet-tab-enabled").is_some());

        runtime.state.go_back();
        runtime.frame(0.0);
        assert_eq!(
            runtime.state.fleet_section,
            fleet_view::FleetSection::Browsers
        );
        runtime.state.go_back();
        runtime.frame(0.0);
        assert_eq!(
            runtime.state.fleet_section,
            fleet_view::FleetSection::AppServers
        );
        runtime.state.go_back();
        runtime.frame(0.0);
        assert_eq!(runtime.state.fleet.tab, fleet_view::FleetTab::All);
        runtime.state.go_back();
        runtime.frame(0.0);
        assert_eq!(runtime.state.primary_view, PrimaryView::Home);

        runtime.update(|state, _| {
            state.navigate_to(ViewLocation::Fleet(state.fleet_section, state.fleet.tab));
            state
                .features
                .iter_mut()
                .find(|feature| feature.id == "fleet")
                .unwrap()
                .enabled = false;
            state.navigate_to(ViewLocation::Home);
        });
        runtime.frame(0.0);
        assert!(runtime.find("nav-fleet").is_none());
        runtime.state.go_back();
        assert_eq!(runtime.state.primary_view, PrimaryView::Home);
    }

    #[test]
    fn fleet_add_menu_opens_the_manual_form() {
        let mut state = State {
            primary_view: PrimaryView::Fleet,
            ..State::default()
        };
        state
            .features
            .iter_mut()
            .find(|feature| feature.id == "fleet")
            .unwrap()
            .enabled = true;
        let mut runtime = Runtime::new(state, view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let (x, y) = runtime.find("fleet-add-server").unwrap();
        runtime.pointer_move(x, y);
        runtime.frame(1.0);
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert_eq!(runtime.state.selection_menu, Some(SelectionMenu::FleetAdd));
        let items = selection_menu_items(&runtime.state, SelectionMenu::FleetAdd);
        assert_eq!(items[0].label, "Manually Add");
        assert_eq!(items[1].label, "Tailscale Import");
        let (x, y) = runtime.find("selection-menu-item-0").unwrap();
        runtime.pointer_move(x, y);
        runtime.frame(1.0);
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert!(runtime.state.fleet.connect_dialog_open);
        assert!(runtime.find("fleet-connect-dialog").is_some());
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
        runtime.frame(1.0);
        let (forward, _) = runtime.presented("header-forward").unwrap();
        let (divider, _) = runtime.presented("header-divider").unwrap();
        assert!(
            (divider[0] - forward[0] - forward[2]).abs() < 1.0,
            "forward={forward:?}, divider={divider:?}"
        );
        assert!(runtime.find("settings-sidenav-resize").is_none());

        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        runtime.frame(1.0);
        assert_eq!(runtime.state.sidenav_width, SIDENAV_MIN_WIDTH);
        aligned(&runtime);
    }

    #[test]
    fn settings_sidenav_switches_sections_and_returns_to_providers() {
        let mut runtime =
            Runtime::new(State::default(), view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        assert_eq!(runtime.state.settings_section, SettingsSection::Safety);
        assert_eq!(runtime.state.settings_section.label(), "Safety");
        assert_eq!(
            runtime.state.settings_section.icon().name,
            "beale-shield-check"
        );
        assert!(runtime.find("settings-safety").is_some());
        let (x, y) = runtime.find("settings-providers").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
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

    #[test]
    fn extensions_navigation_opens_features_first_and_keeps_plugin_views() {
        let mut runtime =
            Runtime::new(State::default(), view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let (x, y) = runtime.find("nav-extensions").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(runtime.state.primary_view, PrimaryView::Extensions);
        assert_eq!(
            runtime.state.extensions_section,
            extensions_view::ExtensionSection::Features
        );
        assert!(runtime.find("extensions-sidenav-title").is_some());
        assert!(runtime.find("feature-tab-all").is_some());
        let (features, _) = runtime.presented("extensions-features").unwrap();
        let (plugins, _) = runtime.presented("extensions-plugins").unwrap();
        assert!(features[1] < plugins[1]);
        let (x, y) = runtime.find("extensions-plugins").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(
            runtime.state.extensions_section,
            extensions_view::ExtensionSection::Plugins
        );
        assert_eq!(runtime.state.plugin_tab, extensions_view::PluginTab::All);
        assert!(runtime.find("plugin-tab-all").is_some());
        assert!(runtime.find("plugin-tab-available").is_some());
        assert!(runtime.find("plugin-tab-installed").is_some());
        for name in [
            "apple-security-devices",
            "apple-target-flags",
            "meta-skills",
            "microsoft-security-devices",
        ] {
            assert!(runtime.find(&format!("plugin-item-{name}")).is_some());
        }
        let (x, y) = runtime
            .find("plugin-manifest-apple-security-devices")
            .unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert!(runtime.find("plugin-manifest-dialog").is_some());
        assert!(runtime.state.plugin_manifest_text.contains("\"version\""));
        let (x, y) = runtime.find("plugin-manifest-close").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert!(runtime.state.plugin_manifest_name.is_none());
        let (x, y) = runtime.find("plugin-tab-installed").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(
            runtime.state.plugin_tab,
            extensions_view::PluginTab::Installed
        );
        assert!(runtime.find("plugin-item-apple-security-devices").is_none());
        let (x, y) = runtime.find("plugin-add-source").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert!(runtime.find("plugin-source-dialog").is_some());
        let (x, y) = runtime.find("plugin-source-close").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert!(!runtime.state.plugin_add_dialog_open);
        let (x, y) = runtime.find("extensions-features").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        assert_eq!(
            runtime.state.extensions_section,
            extensions_view::ExtensionSection::Features
        );
        assert!(runtime.find("plugin-tab-all").is_none());
        assert!(runtime.find("plugin-tab-available").is_none());
        assert_eq!(runtime.state.feature_tab, extensions_view::FeatureTab::All);
        assert!(runtime.find("feature-tab-all").is_some());
        assert!(runtime.find("feature-tab-available").is_some());
        assert!(runtime.find("feature-tab-enabled").is_some());
        assert!(runtime.find("feature-item-goal-mode").is_some());
        assert!(runtime.find("feature-item-graphs").is_some());
        assert!(runtime.find("feature-toggle-graphs").is_some());
        assert!(runtime.find("feature-item-fleet").is_some());
        assert!(runtime.find("feature-toggle-fleet").is_some());
        assert!(runtime.find("feature-tools-fleet").is_some());
        assert!(runtime.find("feature-toggle-goal-mode").is_some());
        assert!(runtime.find("feature-prompt-extension-goal-mode").is_some());
        assert!(runtime.find("feature-tools-goal-mode").is_some());
        assert!(runtime.state.features[0].has_prompt_extension);
        assert_eq!(runtime.state.features[0].tool_count, 1);
        let (x, y) = runtime.find("feature-tab-enabled").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert_eq!(
            runtime.state.feature_tab,
            extensions_view::FeatureTab::Enabled
        );
        assert!(runtime.find("feature-item-goal-mode").is_none());
        runtime.state.go_back();
        runtime.frame(0.0);
        assert_eq!(runtime.state.feature_tab, extensions_view::FeatureTab::All);
    }

    #[test]
    fn active_extension_cards_show_workspace_scope_controls() {
        let mut state = State {
            primary_view: PrimaryView::Extensions,
            extensions_section: extensions_view::ExtensionSection::Plugins,
            ..State::default()
        };
        state.plugins[0].installed = true;
        state.features[0].enabled = true;
        let mut runtime = Runtime::new(state, view, (1040.0, 720.0), Env::new(true, 1.0));
        runtime.frame(0.0);
        let (x, y) = runtime.find("plugin-scope-0").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert_eq!(
            runtime.state.selection_menu,
            Some(SelectionMenu::PluginWorkspaces { index: 0 })
        );
        assert!(runtime.find("selection-menu-item-0").is_some());
        runtime.key(KeyEvent {
            key: Key::Escape,
            mods: Default::default(),
        });
        runtime.frame(0.0);
        let (x, y) = runtime.find("extensions-features").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert!(runtime.find("feature-scope-0").is_some());
    }

    #[test]
    fn feature_details_button_opens_dialog() {
        let mut runtime = Runtime::new(
            State {
                primary_view: PrimaryView::Extensions,
                ..State::default()
            },
            view,
            (1040.0, 720.0),
            Env::new(true, 1.0),
        );
        runtime.frame(0.0);
        let (x, y) = runtime.find("extensions-features").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0);
        let (x, y) = runtime.find("feature-details-goal-mode").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert_eq!(
            runtime.state.feature_details_id.as_deref(),
            Some("goal-mode")
        );
        assert!(runtime.find("feature-details-dialog").is_some());
        runtime.update(|state, _| {
            state.feature_details = Some(beale_contracts::v1::FeatureDetailsResponse {
                id: "goal-mode".to_owned(),
                name: "Goal Mode".to_owned(),
                description: "Example feature".to_owned(),
                prompt_extension: "Example prompt extension".to_owned(),
            });
            state.feature_details_error.clear();
        });
        runtime.frame(0.0);
        assert!(runtime.find("feature-details-prompt").is_some());
        let (x, y) = runtime.find("feature-details-close").unwrap();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(0.0);
        assert!(runtime.state.feature_details_id.is_none());
    }

    #[test]
    fn plugin_cards_fill_four_to_one_columns_as_content_narrows() {
        let make = |width| {
            let mut runtime = Runtime::new(
                State {
                    primary_view: PrimaryView::Extensions,
                    extensions_section: extensions_view::ExtensionSection::Plugins,
                    ..State::default()
                },
                view,
                (width, 720.0),
                Env::new(true, 1.0),
            );
            runtime.frame(1.0);
            runtime
        };
        for (window_width, columns) in [(1600.0, 4), (1250.0, 3), (980.0, 2), (760.0, 1)] {
            let runtime = make(window_width);
            if columns == 4 {
                for name in [
                    "apple-security-devices",
                    "apple-target-flags",
                    "meta-skills",
                    "microsoft-security-devices",
                ] {
                    assert!(runtime.find(&format!("plugin-mark-badge-{name}")).is_some());
                }
            }
            let (page, _) = runtime.presented("settings-page").unwrap();
            let cards = [
                "apple-security-devices",
                "apple-target-flags",
                "meta-skills",
                "microsoft-security-devices",
            ]
            .map(|name| runtime.presented(&format!("plugin-item-{name}")).unwrap().0);
            let first_row_count = cards
                .iter()
                .take_while(|card| (card[1] - cards[0][1]).abs() < 1.0)
                .count();
            assert_eq!(first_row_count, columns, "window width {window_width}");
            let expected_card_width = (page[2] - 16.0 * (columns - 1) as f32) / columns as f32;
            for card in cards.iter().take(columns) {
                assert!((card[2] - expected_card_width).abs() < 1.0);
            }
            let stat_bottoms = [
                "apple-security-devices",
                "apple-target-flags",
                "meta-skills",
                "microsoft-security-devices",
            ]
            .iter()
            .take(columns)
            .map(|name| {
                let (rect, _) = runtime
                    .presented(&format!("plugin-mcp-count-{name}"))
                    .unwrap();
                rect[1] + rect[3]
            })
            .collect::<Vec<_>>();
            assert!(
                stat_bottoms
                    .iter()
                    .all(|bottom| (bottom - stat_bottoms[0]).abs() < 1.0),
                "stat rows should align at window width {window_width}"
            );
            if columns < 4 {
                assert!(cards[columns][1] > cards[0][1]);
            }
            let (description, _) = runtime
                .presented("plugin-description-apple-security-devices")
                .unwrap();
            assert!(description[3] <= 60.0);
        }
    }
}
