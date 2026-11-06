//! Versioned client-to-host wire contracts. No host-only types belong here.

pub mod v1 {
    /// `proto/beale/v1/host.proto` is the source wire schema. Field tags and
    /// types must change together here and in that file.
    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct HealthResponse {
        #[prost(uint32, tag = "1")]
        pub protocol_major: u32,
        #[prost(uint32, tag = "2")]
        pub protocol_minor: u32,
        #[prost(string, tag = "3")]
        pub service: String,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum ProviderId {
        Unspecified = 0,
        Openai = 1,
        Xai = 2,
        Anthropic = 3,
        Zai = 4,
        Openrouter = 5,
        Custom = 6,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct ProviderAuthStatus {
        #[prost(enumeration = "ProviderId", tag = "1")]
        pub provider: i32,
        #[prost(bool, tag = "2")]
        pub subscription_configured: bool,
        #[prost(bool, tag = "3")]
        pub api_key_configured: bool,
        #[prost(bool, tag = "4")]
        pub login_in_progress: bool,
        #[prost(string, tag = "5")]
        pub login_error: String,
        #[prost(enumeration = "AuthMethod", tag = "6")]
        pub source_priority: i32,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct ProviderAuthStatusResponse {
        #[prost(message, repeated, tag = "1")]
        pub providers: Vec<ProviderAuthStatus>,
        #[prost(enumeration = "ProviderId", tag = "2")]
        pub lead_provider: i32,
        #[prost(enumeration = "DaybreakMode", tag = "3")]
        pub openai_daybreak_mode: i32,
        #[prost(string, tag = "4")]
        pub lead_custom_provider_id: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct ProviderUsageWindow {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(string, tag = "2")]
        pub label: String,
        #[prost(uint32, tag = "3")]
        pub used_percent: u32,
        #[prost(uint64, tag = "4")]
        pub resets_at_unix: u64,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct ProviderUsageSnapshot {
        #[prost(enumeration = "ProviderId", tag = "1")]
        pub provider: i32,
        #[prost(uint64, tag = "2")]
        pub checked_at_unix: u64,
        #[prost(message, repeated, tag = "3")]
        pub windows: Vec<ProviderUsageWindow>,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct ProviderUsageResponse {
        #[prost(message, repeated, tag = "1")]
        pub providers: Vec<ProviderUsageSnapshot>,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct TrackingSubscriptionPoint {
        #[prost(enumeration = "ProviderId", tag = "1")]
        pub provider: i32,
        #[prost(string, tag = "2")]
        pub window_id: String,
        #[prost(string, tag = "3")]
        pub label: String,
        #[prost(uint64, tag = "4")]
        pub observed_at_ms: u64,
        #[prost(uint32, tag = "5")]
        pub remaining_percent: u32,
        #[prost(uint64, tag = "6")]
        pub resets_at_unix: u64,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct TrackingModelDay {
        #[prost(uint64, tag = "1")]
        pub day_start_ms: u64,
        #[prost(enumeration = "ProviderId", tag = "2")]
        pub provider: i32,
        #[prost(enumeration = "AuthMethod", tag = "3")]
        pub method: i32,
        #[prost(string, tag = "4")]
        pub model: String,
        #[prost(string, tag = "5")]
        pub role: String,
        #[prost(uint64, tag = "6")]
        pub input_tokens: u64,
        #[prost(uint64, tag = "7")]
        pub output_tokens: u64,
        #[prost(uint64, tag = "8")]
        pub duration_ms: u64,
        #[prost(uint32, tag = "9")]
        pub measured_calls: u32,
        #[prost(uint32, tag = "10")]
        pub total_calls: u32,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct TrackingResponse {
        #[prost(message, repeated, tag = "1")]
        pub subscriptions: Vec<TrackingSubscriptionPoint>,
        #[prost(message, repeated, tag = "2")]
        pub model_days: Vec<TrackingModelDay>,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum DaybreakMode {
        Unspecified = 0,
        Blue = 1,
        Red = 2,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct ProviderRequest {
        #[prost(enumeration = "ProviderId", tag = "1")]
        pub provider: i32,
        #[prost(string, tag = "2")]
        pub custom_provider_id: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct CustomProvider {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(string, tag = "2")]
        pub name: String,
        #[prost(string, tag = "3")]
        pub base_url: String,
        #[prost(message, repeated, tag = "4")]
        pub models: Vec<ModelEntry>,
        #[prost(string, tag = "5")]
        pub lead_model: String,
        #[prost(string, tag = "6")]
        pub small_model: String,
        #[prost(bool, tag = "7")]
        pub reasoning: bool,
        #[prost(bool, tag = "8")]
        pub api_key_configured: bool,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct ListCustomProvidersResponse {
        #[prost(message, repeated, tag = "1")]
        pub providers: Vec<CustomProvider>,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct SaveCustomProviderRequest {
        #[prost(message, optional, tag = "1")]
        pub provider: Option<CustomProvider>,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct CustomProviderRequest {
        #[prost(string, tag = "1")]
        pub id: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct SetCustomProviderApiKeyRequest {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(string, tag = "2")]
        pub api_key: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct DiscoverCustomModelsRequest {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(string, tag = "2")]
        pub base_url: String,
        #[prost(string, tag = "3")]
        pub api_key: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct DiscoverCustomModelsResponse {
        #[prost(message, repeated, tag = "1")]
        pub models: Vec<ModelEntry>,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct SetApiKeyRequest {
        #[prost(enumeration = "ProviderId", tag = "1")]
        pub provider: i32,
        #[prost(string, tag = "2")]
        pub api_key: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct StartSubscriptionResponse {
        #[prost(string, tag = "1")]
        pub verification_url: String,
        #[prost(string, tag = "2")]
        pub user_code: String,
        #[prost(bool, tag = "3")]
        pub launch_official_app: bool,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct ControlChallengeRequest {
        #[prost(bytes = "vec", tag = "1")]
        pub nonce: Vec<u8>,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct ControlChallengeResponse {
        #[prost(bytes = "vec", tag = "1")]
        pub proof: Vec<u8>,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum AuthMethod {
        Unspecified = 0,
        Subscription = 1,
        ApiKey = 2,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct ListModelsRequest {
        #[prost(enumeration = "ProviderId", tag = "1")]
        pub provider: i32,
        #[prost(enumeration = "AuthMethod", tag = "2")]
        pub method: i32,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct ModelEntry {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(string, tag = "2")]
        pub display_name: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct ModelCatalogResponse {
        #[prost(enumeration = "ProviderId", tag = "1")]
        pub provider: i32,
        #[prost(enumeration = "AuthMethod", tag = "2")]
        pub method: i32,
        #[prost(message, repeated, tag = "3")]
        pub models: Vec<ModelEntry>,
        #[prost(string, tag = "4")]
        pub lead_model: String,
        #[prost(string, tag = "5")]
        pub small_model: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct SetModelDefaultsRequest {
        #[prost(enumeration = "ProviderId", tag = "1")]
        pub provider: i32,
        #[prost(enumeration = "AuthMethod", tag = "2")]
        pub method: i32,
        #[prost(string, tag = "3")]
        pub lead_model: String,
        #[prost(string, tag = "4")]
        pub small_model: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct SetSourcePriorityRequest {
        #[prost(enumeration = "ProviderId", tag = "1")]
        pub provider: i32,
        #[prost(enumeration = "AuthMethod", tag = "2")]
        pub method: i32,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct SetDaybreakModeRequest {
        #[prost(enumeration = "DaybreakMode", tag = "1")]
        pub mode: i32,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum SurfaceKind {
        Unspecified = 0,
        Repository = 1,
        Documentation = 2,
        Binary = 3,
        Service = 4,
        Domain = 5,
        Other = 6,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum SurfaceScope {
        Unspecified = 0,
        InScope = 1,
        OutOfScope = 2,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct WorkspaceSurfaceInput {
        #[prost(enumeration = "SurfaceKind", tag = "1")]
        pub kind: i32,
        #[prost(enumeration = "SurfaceScope", tag = "2")]
        pub scope: i32,
        #[prost(string, tag = "3")]
        pub value: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct CreateWorkspaceRequest {
        #[prost(string, tag = "1")]
        pub name: String,
        #[prost(string, tag = "2")]
        pub directory: String,
        #[prost(string, tag = "3")]
        pub guidance: String,
        #[prost(message, repeated, tag = "4")]
        pub surfaces: Vec<WorkspaceSurfaceInput>,
        #[prost(string, repeated, tag = "5")]
        pub rules: Vec<String>,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct WorkspaceSummary {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(string, tag = "2")]
        pub name: String,
        #[prost(string, tag = "3")]
        pub directory: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct ListWorkspacesResponse {
        #[prost(message, repeated, tag = "1")]
        pub workspaces: Vec<WorkspaceSummary>,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum SurfaceCreator {
        Unspecified = 0,
        User = 1,
        Agent = 2,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct WorkspaceSurface {
        #[prost(enumeration = "SurfaceKind", tag = "1")]
        pub kind: i32,
        #[prost(enumeration = "SurfaceScope", tag = "2")]
        pub scope: i32,
        #[prost(string, tag = "3")]
        pub value: String,
        #[prost(enumeration = "SurfaceCreator", tag = "4")]
        pub created_by: i32,
        #[prost(string, tag = "5")]
        pub out_of_scope_reason: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct WorkspaceDetails {
        #[prost(message, optional, tag = "1")]
        pub summary: Option<WorkspaceSummary>,
        #[prost(string, tag = "2")]
        pub guidance: String,
        #[prost(message, repeated, tag = "3")]
        pub surfaces: Vec<WorkspaceSurface>,
        #[prost(string, repeated, tag = "4")]
        pub rules: Vec<String>,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct WorkspaceCampaign {
        #[prost(string, repeated, tag = "1")]
        pub enabled_feature_ids: Vec<String>,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct WorkspaceFoundation {
        #[prost(message, optional, tag = "1")]
        pub summary: Option<WorkspaceSummary>,
        #[prost(string, tag = "2")]
        pub guidance: String,
        #[prost(uint64, tag = "3")]
        pub revision: u64,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct UpdateWorkspaceFoundationRequest {
        #[prost(uint64, tag = "1")]
        pub expected_revision: u64,
        #[prost(string, tag = "2")]
        pub name: String,
        #[prost(string, tag = "3")]
        pub guidance: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct WorkspaceSurfaces {
        #[prost(message, repeated, tag = "1")]
        pub surfaces: Vec<WorkspaceSurface>,
        #[prost(uint64, tag = "2")]
        pub revision: u64,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct UpdateWorkspaceSurfacesRequest {
        #[prost(uint64, tag = "1")]
        pub expected_revision: u64,
        #[prost(message, repeated, tag = "2")]
        pub surfaces: Vec<WorkspaceSurfaceInput>,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct WorkspaceRules {
        #[prost(string, repeated, tag = "1")]
        pub rules: Vec<String>,
        #[prost(uint64, tag = "2")]
        pub revision: u64,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct UpdateWorkspaceRulesRequest {
        #[prost(uint64, tag = "1")]
        pub expected_revision: u64,
        #[prost(string, repeated, tag = "2")]
        pub rules: Vec<String>,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum SessionRole {
        Unspecified = 0,
        User = 1,
        Assistant = 2,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct SessionMessage {
        #[prost(enumeration = "SessionRole", tag = "1")]
        pub role: i32,
        #[prost(string, tag = "2")]
        pub text: String,
        #[prost(message, repeated, tag = "3")]
        pub tools: Vec<SessionToolSummary>,
        #[prost(message, repeated, tag = "4")]
        pub reasoning_summaries: Vec<SessionReasoningSummary>,
        #[prost(uint64, tag = "5")]
        pub duration_ms: u64,
        #[prost(message, repeated, tag = "6")]
        pub commentary: Vec<SessionCommentary>,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct SessionCommentary {
        #[prost(string, tag = "1")]
        pub text: String,
        #[prost(uint32, tag = "2")]
        pub after_tool_count: u32,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum SessionStreamKind {
        Unspecified = 0,
        ReasoningDelta = 1,
        MessageDelta = 2,
        ToolStarted = 3,
        ToolFinished = 4,
        Completed = 5,
        Failed = 6,
        TurnCompleted = 7,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct SessionStreamEvent {
        #[prost(enumeration = "SessionStreamKind", tag = "1")]
        pub kind: i32,
        #[prost(string, tag = "2")]
        pub text: String,
        #[prost(message, optional, tag = "3")]
        pub tool: Option<SessionToolSummary>,
        #[prost(message, optional, tag = "4")]
        pub detail: Option<SessionDetail>,
        #[prost(uint32, tag = "5")]
        pub segment_index: u32,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct SessionReasoningSummary {
        #[prost(string, tag = "1")]
        pub text: String,
        #[prost(uint32, tag = "2")]
        pub after_tool_count: u32,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct SessionToolSummary {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(string, tag = "2")]
        pub name: String,
        #[prost(string, tag = "3")]
        pub summary: String,
        #[prost(bool, tag = "4")]
        pub is_error: bool,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct SessionToolDetailPage {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(string, tag = "2")]
        pub name: String,
        #[prost(string, tag = "3")]
        pub arguments: String,
        #[prost(string, tag = "4")]
        pub output: String,
        #[prost(uint64, tag = "5")]
        pub offset: u64,
        #[prost(uint64, tag = "6")]
        pub next_offset: u64,
        #[prost(uint64, tag = "7")]
        pub total_bytes: u64,
        #[prost(bool, tag = "8")]
        pub is_error: bool,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct SessionSummary {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(string, tag = "2")]
        pub workspace_id: String,
        #[prost(string, tag = "3")]
        pub title: String,
        #[prost(uint64, tag = "4")]
        pub last_message_at_ms: u64,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct ListSessionsResponse {
        #[prost(message, repeated, tag = "1")]
        pub sessions: Vec<SessionSummary>,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct ArchiveEntry {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(string, tag = "2")]
        pub workspace_id: String,
        #[prost(string, tag = "3")]
        pub title: String,
        #[prost(string, tag = "4")]
        pub model: String,
        #[prost(uint64, tag = "5")]
        pub turn_count: u64,
        #[prost(uint64, tag = "6")]
        pub created_at_ms: u64,
        #[prost(uint64, tag = "7")]
        pub archived_at_ms: u64,
        #[prost(bool, tag = "8")]
        pub quick_chat: bool,
        #[prost(uint64, tag = "9")]
        pub last_message_at_ms: u64,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct ListArchiveResponse {
        #[prost(message, repeated, tag = "1")]
        pub entries: Vec<ArchiveEntry>,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct SessionDetail {
        #[prost(message, optional, tag = "1")]
        pub summary: Option<SessionSummary>,
        #[prost(message, repeated, tag = "2")]
        pub messages: Vec<SessionMessage>,
        #[prost(enumeration = "ProviderId", tag = "3")]
        pub provider: i32,
        #[prost(enumeration = "AuthMethod", tag = "4")]
        pub method: i32,
        #[prost(string, tag = "5")]
        pub model: String,
        #[prost(message, optional, tag = "6")]
        pub goal: Option<SessionGoal>,
        #[prost(uint64, tag = "7")]
        pub first_turn_number: u64,
        #[prost(string, tag = "8")]
        pub graph_id: String,
        #[prost(message, repeated, tag = "9")]
        pub graph_models: Vec<GraphNodeModel>,
        #[prost(message, repeated, tag = "10")]
        pub graph_node_concurrency: Vec<GraphNodeConcurrency>,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct GraphNodeModel {
        #[prost(string, tag = "1")]
        pub node_id: String,
        #[prost(enumeration = "ProviderId", tag = "2")]
        pub provider: i32,
        #[prost(enumeration = "AuthMethod", tag = "3")]
        pub method: i32,
        #[prost(string, tag = "4")]
        pub model: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct GraphNodeConcurrency {
        #[prost(string, tag = "1")]
        pub node_id: String,
        #[prost(uint32, tag = "2")]
        pub max_concurrent: u32,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct SessionGraphConfig {
        #[prost(message, repeated, tag = "1")]
        pub models: Vec<GraphNodeModel>,
        #[prost(message, repeated, tag = "2")]
        pub concurrency: Vec<GraphNodeConcurrency>,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum GoalStatus {
        Unspecified = 0,
        Active = 1,
        Complete = 2,
        Waiting = 3,
        Paused = 4,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct SessionGoal {
        #[prost(string, tag = "1")]
        pub objective: String,
        #[prost(enumeration = "GoalStatus", tag = "2")]
        pub status: i32,
        #[prost(uint64, tag = "3")]
        pub turns_used: u64,
        #[prost(string, tag = "4")]
        pub required_state: String,
        #[prost(uint64, tag = "5")]
        pub max_turns: u64,
        #[prost(uint64, tag = "6")]
        pub max_duration_minutes: u64,
        #[prost(uint64, tag = "7")]
        pub active_ms: u64,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum ReasoningEffort {
        Unspecified = 0,
        None = 1,
        Minimal = 2,
        Low = 3,
        Medium = 4,
        High = 5,
        Xhigh = 6,
        Max = 7,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct SubmitSessionTurnRequest {
        #[prost(string, tag = "1")]
        pub workspace_id: String,
        #[prost(string, tag = "2")]
        pub session_id: String,
        #[prost(enumeration = "ProviderId", tag = "3")]
        pub provider: i32,
        #[prost(enumeration = "AuthMethod", tag = "4")]
        pub method: i32,
        #[prost(string, tag = "5")]
        pub model: String,
        #[prost(string, tag = "6")]
        pub prompt: String,
        #[prost(enumeration = "ReasoningEffort", tag = "7")]
        pub reasoning_effort: i32,
        #[prost(bool, tag = "8")]
        pub goal_mode: bool,
        #[prost(uint64, tag = "9")]
        pub goal_max_turns: u64,
        #[prost(uint64, tag = "10")]
        pub goal_max_duration_minutes: u64,
        #[prost(string, tag = "11")]
        pub repeat_preset: String,
        #[prost(string, tag = "12")]
        pub graph_id: String,
        #[prost(message, repeated, tag = "13")]
        pub graph_models: Vec<GraphNodeModel>,
        #[prost(message, repeated, tag = "14")]
        pub graph_node_concurrency: Vec<GraphNodeConcurrency>,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct AutomationJob {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(string, tag = "2")]
        pub session_id: String,
        #[prost(string, tag = "3")]
        pub workspace_id: String,
        #[prost(string, tag = "4")]
        pub title: String,
        #[prost(string, tag = "5")]
        pub repeat_preset: String,
        #[prost(uint64, tag = "6")]
        pub next_run_at_ms: u64,
        #[prost(uint64, tag = "7")]
        pub last_run_at_ms: u64,
        #[prost(bool, tag = "8")]
        pub canceled: bool,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct ListAutomationJobsResponse {
        #[prost(message, repeated, tag = "1")]
        pub jobs: Vec<AutomationJob>,
        #[prost(message, repeated, tag = "2")]
        pub runs: Vec<AutomationRun>,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum AutomationRunStatus {
        Unspecified = 0,
        Active = 1,
        Complete = 2,
        Failed = 3,
        Interrupted = 4,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct AutomationRun {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(string, tag = "2")]
        pub job_id: String,
        #[prost(string, tag = "3")]
        pub session_id: String,
        #[prost(string, tag = "4")]
        pub title: String,
        #[prost(uint64, tag = "5")]
        pub started_at_ms: u64,
        #[prost(uint64, tag = "6")]
        pub completed_at_ms: u64,
        #[prost(enumeration = "AutomationRunStatus", tag = "7")]
        pub status: i32,
        #[prost(string, tag = "8")]
        pub error: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct CancelAutomationRequest {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(bool, tag = "2")]
        pub series: bool,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct QuickChatTurnRequest {
        #[prost(string, tag = "1")]
        pub chat_id: String,
        #[prost(string, tag = "2")]
        pub prompt: String,
        #[prost(string, tag = "3")]
        pub workspace_id: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct QuickChatMessage {
        #[prost(enumeration = "SessionRole", tag = "1")]
        pub role: i32,
        #[prost(string, tag = "2")]
        pub text: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct QuickChatDetail {
        #[prost(string, tag = "1")]
        pub chat_id: String,
        #[prost(string, tag = "2")]
        pub title: String,
        #[prost(message, repeated, tag = "3")]
        pub messages: Vec<QuickChatMessage>,
        #[prost(string, tag = "4")]
        pub workspace_id: String,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum ReviewStage {
        Unspecified = 0,
        AutoReview = 1,
        Guardian = 2,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct PermissionRule {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(enumeration = "ReviewStage", tag = "2")]
        pub stage: i32,
        #[prost(string, tag = "3")]
        pub title: String,
        #[prost(string, tag = "4")]
        pub instruction: String,
        #[prost(bool, tag = "5")]
        pub enabled: bool,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct PermissionRulesResponse {
        #[prost(message, repeated, tag = "1")]
        pub rules: Vec<PermissionRule>,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct UpdatePermissionRuleRequest {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(string, tag = "2")]
        pub instruction: String,
        #[prost(bool, tag = "3")]
        pub enabled: bool,
        #[prost(string, tag = "4")]
        pub title: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct CreatePermissionRuleRequest {
        #[prost(enumeration = "ReviewStage", tag = "1")]
        pub stage: i32,
        #[prost(string, tag = "2")]
        pub title: String,
        #[prost(string, tag = "3")]
        pub instruction: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct PluginSummary {
        #[prost(string, tag = "1")]
        pub name: String,
        #[prost(string, tag = "2")]
        pub description: String,
        #[prost(bool, tag = "3")]
        pub installed: bool,
        #[prost(uint32, tag = "4")]
        pub skill_count: u32,
        #[prost(bool, tag = "5")]
        pub has_mcp_server: bool,
        #[prost(uint32, tag = "6")]
        pub mcp_server_count: u32,
        #[prost(uint32, tag = "7")]
        pub mcp_tool_count: u32,
        #[prost(string, repeated, tag = "8")]
        pub workspace_ids: Vec<String>,
        #[prost(bool, tag = "9")]
        pub all_workspaces: bool,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct ListPluginsResponse {
        #[prost(message, repeated, tag = "1")]
        pub plugins: Vec<PluginSummary>,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct FeatureSummary {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(string, tag = "2")]
        pub name: String,
        #[prost(string, tag = "3")]
        pub description: String,
        #[prost(bool, tag = "4")]
        pub enabled: bool,
        #[prost(bool, tag = "5")]
        pub has_prompt_extension: bool,
        #[prost(uint32, tag = "6")]
        pub tool_count: u32,
        #[prost(string, repeated, tag = "7")]
        pub workspace_ids: Vec<String>,
        #[prost(bool, tag = "8")]
        pub all_workspaces: bool,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct SetExtensionWorkspaceScopeRequest {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(string, repeated, tag = "2")]
        pub workspace_ids: Vec<String>,
        #[prost(bool, tag = "3")]
        pub all_workspaces: bool,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct ListFeaturesResponse {
        #[prost(message, repeated, tag = "1")]
        pub features: Vec<FeatureSummary>,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct GetFeatureDetailsRequest {
        #[prost(string, tag = "1")]
        pub id: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct FeatureDetailsResponse {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(string, tag = "2")]
        pub name: String,
        #[prost(string, tag = "3")]
        pub description: String,
        #[prost(string, tag = "4")]
        pub prompt_extension: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct SetFeatureEnabledRequest {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(bool, tag = "2")]
        pub enabled: bool,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct InstallPluginRequest {
        #[prost(string, tag = "1")]
        pub name: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct UninstallPluginRequest {
        #[prost(string, tag = "1")]
        pub name: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct GetPluginManifestRequest {
        #[prost(string, tag = "1")]
        pub name: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct PluginManifestResponse {
        #[prost(string, tag = "1")]
        pub name: String,
        #[prost(string, tag = "2")]
        pub manifest_json: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct FleetAppServerDescriptor {
        #[prost(string, tag = "1")]
        pub service: String,
        #[prost(string, tag = "2")]
        pub host_name: String,
        #[prost(string, tag = "3")]
        pub os_name: String,
        #[prost(string, tag = "4")]
        pub os_vendor: String,
        #[prost(uint32, tag = "5")]
        pub protocol_major: u32,
        #[prost(uint32, tag = "6")]
        pub protocol_minor: u32,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct FleetTotpProvisioningResponse {
        #[prost(string, tag = "1")]
        pub otpauth_uri: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct FleetPairingRequest {
        #[prost(string, tag = "1")]
        pub code: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct FleetPairingResponse {
        #[prost(string, tag = "1")]
        pub token: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct FleetBrowser {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(string, tag = "2")]
        pub name: String,
        #[prost(bool, tag = "3")]
        pub manual: bool,
        #[prost(bool, tag = "4")]
        pub available: bool,
        #[prost(bool, tag = "5")]
        pub enabled: bool,
        #[prost(bool, tag = "6")]
        pub all_workspaces: bool,
        #[prost(string, repeated, tag = "7")]
        pub workspace_ids: Vec<String>,
        #[prost(bool, tag = "8")]
        pub preferred: bool,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct FleetBrowsersResponse {
        #[prost(message, repeated, tag = "1")]
        pub browsers: Vec<FleetBrowser>,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct FleetAddBrowserRequest {
        #[prost(string, tag = "1")]
        pub name: String,
        #[prost(string, tag = "2")]
        pub executable: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct FleetRemoveBrowserRequest {
        #[prost(string, tag = "1")]
        pub id: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct FleetSetBrowserSettingsRequest {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(bool, tag = "2")]
        pub enabled: bool,
        #[prost(bool, tag = "3")]
        pub all_workspaces: bool,
        #[prost(string, repeated, tag = "4")]
        pub workspace_ids: Vec<String>,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct FleetSetPreferredBrowserRequest {
        #[prost(string, tag = "1")]
        pub id: String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct FleetVirtualMachine {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(string, tag = "2")]
        pub name: String,
        #[prost(string, tag = "3")]
        pub provider: String,
        #[prost(string, tag = "4")]
        pub state: String,
        #[prost(uint32, tag = "5")]
        pub generation: u32,
        #[prost(bool, tag = "6")]
        pub enabled: bool,
        #[prost(bool, tag = "7")]
        pub all_workspaces: bool,
        #[prost(string, repeated, tag = "8")]
        pub workspace_ids: Vec<String>,
        #[prost(bool, tag = "9")]
        pub can_run: bool,
        #[prost(bool, tag = "10")]
        pub can_clone: bool,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct FleetVirtualMachinesResponse {
        #[prost(message, repeated, tag = "1")]
        pub virtual_machines: Vec<FleetVirtualMachine>,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct FleetSetVirtualMachineSettingsRequest {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(bool, tag = "2")]
        pub enabled: bool,
        #[prost(bool, tag = "3")]
        pub all_workspaces: bool,
        #[prost(string, repeated, tag = "4")]
        pub workspace_ids: Vec<String>,
        #[prost(bool, tag = "5")]
        pub can_run: bool,
        #[prost(bool, tag = "6")]
        pub can_clone: bool,
    }
}

pub const PROTOCOL_MAJOR: u32 = 1;
pub const PROTOCOL_MINOR: u32 = 48;
pub const SERVICE_NAME: &str = "beale-app-server";
pub const HEALTH_PATH: &str = "/v1/health";
pub const AUTH_STATUS_PATH: &str = "/v1/providers/status";
pub const PROVIDER_USAGE_PATH: &str = "/v1/providers/usage";
pub const TRACKING_PATH: &str = "/v1/tracking";
pub const AUTH_PROVE_PATH: &str = "/v1/providers/prove";
pub const AUTH_API_KEY_PATH: &str = "/v1/providers/api-key";
pub const AUTH_SUBSCRIPTION_START_PATH: &str = "/v1/providers/subscription/start";
pub const AUTH_SUBSCRIPTION_FORGET_PATH: &str = "/v1/providers/subscription/forget";
pub const LEAD_PROVIDER_PATH: &str = "/v1/providers/lead";
pub const MODELS_LIST_PATH: &str = "/v1/providers/models/list";
pub const MODELS_DEFAULTS_PATH: &str = "/v1/providers/models/defaults";
pub const SOURCE_PRIORITY_PATH: &str = "/v1/providers/source-priority";
pub const DAYBREAK_MODE_PATH: &str = "/v1/providers/openai/daybreak-mode";
pub const CUSTOM_PROVIDERS_PATH: &str = "/v1/providers/custom";
pub const CUSTOM_PROVIDER_API_KEY_PATH: &str = "/v1/providers/custom/api-key";
pub const CUSTOM_PROVIDER_DISCOVER_PATH: &str = "/v1/providers/custom/discover";
pub const WORKSPACES_PATH: &str = "/v1/workspaces";
pub const WORKSPACE_DETAILS_PATH: &str = "/v1/workspaces/{id}";
pub const WORKSPACE_CAMPAIGN_PATH: &str = "/v1/workspaces/{id}/campaign";
pub const WORKSPACE_FOUNDATION_PATH: &str = "/v1/workspaces/{id}/foundation";
pub const WORKSPACE_SURFACES_PATH: &str = "/v1/workspaces/{id}/surfaces";
pub const WORKSPACE_RULES_PATH: &str = "/v1/workspaces/{id}/rules";
pub const SESSIONS_PATH: &str = "/v1/sessions";
pub const SESSION_DETAIL_PATH: &str = "/v1/sessions/{id}";
pub const SESSION_ARCHIVE_PATH: &str = "/v1/sessions/{id}/archive";
pub const SESSION_RESTORE_PATH: &str = "/v1/sessions/{id}/restore";
pub const ARCHIVE_PATH: &str = "/v1/archive";
pub const QUICK_CHAT_DETAIL_PATH: &str = "/v1/quick-chats/{id}";
pub const SESSION_TURNS_PATH: &str = "/v1/sessions/turns";
pub const QUICK_CHAT_TURNS_PATH: &str = "/v1/quick-chats/turns";
pub const SESSION_TURN_STREAM_PATH: &str = "/v1/sessions/turns/stream";
pub const SESSION_TOOL_DETAIL_PATH: &str = "/v1/sessions/{id}/tools/{tool_id}";
pub const AUTOMATIONS_PATH: &str = "/v1/automations";
pub const AUTOMATIONS_CANCEL_PATH: &str = "/v1/automations/cancel";
pub const PERMISSION_RULES_PATH: &str = "/v1/permissions/rules";
pub const PLUGINS_PATH: &str = "/v1/plugins";
pub const FEATURES_PATH: &str = "/v1/features";
pub const FEATURE_DETAILS_PATH: &str = "/v1/features/details";
pub const FEATURE_SET_ENABLED_PATH: &str = "/v1/features/set-enabled";
pub const FEATURE_SCOPE_PATH: &str = "/v1/features/scope";
pub const FLEET_DESCRIPTOR_PATH: &str = "/v1/fleet/descriptor";
pub const FLEET_TOTP_PROVISION_PATH: &str = "/v1/fleet/totp-provision";
pub const FLEET_PAIR_PATH: &str = "/v1/fleet/pair";
pub const FLEET_BROWSERS_PATH: &str = "/v1/fleet/browsers";
pub const FLEET_BROWSER_SETTINGS_PATH: &str = "/v1/fleet/browsers/settings";
pub const FLEET_BROWSER_PREFERRED_PATH: &str = "/v1/fleet/browsers/preferred";
pub const FLEET_BROWSER_REMOVE_PATH: &str = "/v1/fleet/browsers/remove";
pub const FLEET_VIRTUAL_MACHINES_PATH: &str = "/v1/fleet/virtual-machines";
pub const FLEET_VIRTUAL_MACHINE_SETTINGS_PATH: &str = "/v1/fleet/virtual-machines/settings";
pub const PLUGIN_INSTALL_PATH: &str = "/v1/plugins/install";
pub const PLUGIN_UNINSTALL_PATH: &str = "/v1/plugins/uninstall";
pub const PLUGIN_MANIFEST_PATH: &str = "/v1/plugins/manifest";
pub const PLUGIN_SCOPE_PATH: &str = "/v1/plugins/scope";
pub const PROTOBUF_CONTENT_TYPE: &str = "application/x-protobuf";
pub const PROTOBUF_STREAM_CONTENT_TYPE: &str = "application/x-beale-protobuf-stream";
pub const SESSION_STREAM_MAX_FRAME_BYTES: usize = 4_500_000;

pub fn session_stream_frame(event: &v1::SessionStreamEvent) -> Option<Vec<u8>> {
    use prost::Message;
    let size = event.encoded_len();
    if size == 0 || size > SESSION_STREAM_MAX_FRAME_BYTES {
        return None;
    }
    let mut frame = Vec::with_capacity(4 + size);
    frame.extend_from_slice(&(size as u32).to_be_bytes());
    event.encode(&mut frame).ok()?;
    Some(frame)
}
pub const DEFAULT_LOCAL_ADDR: &str = "127.0.0.1:43831";
pub const CONTROL_CHALLENGE_BYTES: usize = 32;

pub fn control_proof(token: &str, nonce: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"Beale local control proof v1\0");
    hasher.update(token.as_bytes());
    hasher.update(nonce);
    hasher.finalize().into()
}
