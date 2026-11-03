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
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct ListSessionsResponse {
        #[prost(message, repeated, tag = "1")]
        pub sessions: Vec<SessionSummary>,
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
    }
}

pub const PROTOCOL_MAJOR: u32 = 1;
pub const PROTOCOL_MINOR: u32 = 10;
pub const SERVICE_NAME: &str = "beale-app-server";
pub const HEALTH_PATH: &str = "/v1/health";
pub const AUTH_STATUS_PATH: &str = "/v1/providers/status";
pub const AUTH_PROVE_PATH: &str = "/v1/providers/prove";
pub const AUTH_API_KEY_PATH: &str = "/v1/providers/api-key";
pub const AUTH_SUBSCRIPTION_START_PATH: &str = "/v1/providers/subscription/start";
pub const AUTH_SUBSCRIPTION_FORGET_PATH: &str = "/v1/providers/subscription/forget";
pub const LEAD_PROVIDER_PATH: &str = "/v1/providers/lead";
pub const MODELS_LIST_PATH: &str = "/v1/providers/models/list";
pub const MODELS_DEFAULTS_PATH: &str = "/v1/providers/models/defaults";
pub const SOURCE_PRIORITY_PATH: &str = "/v1/providers/source-priority";
pub const DAYBREAK_MODE_PATH: &str = "/v1/providers/openai/daybreak-mode";
pub const WORKSPACES_PATH: &str = "/v1/workspaces";
pub const WORKSPACE_DETAILS_PATH: &str = "/v1/workspaces/{id}";
pub const WORKSPACE_CAMPAIGN_PATH: &str = "/v1/workspaces/{id}/campaign";
pub const WORKSPACE_FOUNDATION_PATH: &str = "/v1/workspaces/{id}/foundation";
pub const WORKSPACE_SURFACES_PATH: &str = "/v1/workspaces/{id}/surfaces";
pub const WORKSPACE_RULES_PATH: &str = "/v1/workspaces/{id}/rules";
pub const SESSIONS_PATH: &str = "/v1/sessions";
pub const SESSION_DETAIL_PATH: &str = "/v1/sessions/{id}";
pub const SESSION_TURNS_PATH: &str = "/v1/sessions/turns";
pub const SESSION_TOOL_DETAIL_PATH: &str = "/v1/sessions/{id}/tools/{tool_id}";
pub const PROTOBUF_CONTENT_TYPE: &str = "application/x-protobuf";
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
