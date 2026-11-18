//! Host-only provider adapters. Credentials never enter client contracts.

mod codex;
mod credentials;
mod models;
mod oauth;
pub mod subscription;
pub mod usage;
mod wire;

pub use codex::configured as codex_configured;
pub use credentials::{CredentialStore, ModelDefaults, OAuthCredential};
pub use custom::{CustomProvider, qualified_model, split_qualified_model, valid_custom_id};
pub use models::{ModelCatalog, ModelEntry, mode_name, provider_name};
pub use oauth::{OAuthRefreshError, login_xai, login_xai_with_prompt};
pub use wire::{
    AuthMode, InferenceRequest, OpenAiConnection, Provider, ProviderClient, ProviderProgress,
    ProviderStep, TextMessage, TextRole, ToolCall, ToolStepRequest, input_messages,
    provider_error_message, provider_error_retryable, tool_output,
};
pub mod custom;
