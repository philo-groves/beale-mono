//! Host-only provider adapters. Credentials never enter client contracts.

mod credentials;
mod oauth;
mod wire;

pub use credentials::{CredentialStore, OAuthCredential};
pub use oauth::{login_openai, login_openai_with_prompt, login_xai, login_xai_with_prompt};
pub use wire::{AuthMode, InferenceRequest, Provider, ProviderClient, TextMessage, TextRole};
