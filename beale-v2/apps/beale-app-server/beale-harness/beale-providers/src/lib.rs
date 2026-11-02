//! Host-only provider adapters. Credentials never enter client contracts.

mod credentials;
mod models;
mod oauth;
mod wire;

pub use credentials::{CredentialStore, ModelDefaults, OAuthCredential};
pub use models::{ModelCatalog, ModelEntry, mode_name, provider_name};
pub use oauth::{login_openai, login_openai_with_prompt, login_xai, login_xai_with_prompt};
pub use wire::{AuthMode, InferenceRequest, Provider, ProviderClient, TextMessage, TextRole};
