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
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct ProviderAuthStatusResponse {
        #[prost(message, repeated, tag = "1")]
        pub providers: Vec<ProviderAuthStatus>,
        #[prost(enumeration = "ProviderId", tag = "2")]
        pub lead_provider: i32,
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
}

pub const PROTOCOL_MAJOR: u32 = 1;
pub const PROTOCOL_MINOR: u32 = 3;
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
