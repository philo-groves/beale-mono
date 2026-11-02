//! Host-only provider adapters. Credentials never enter client contracts.

mod credentials;
mod oauth;
mod wire;

pub use credentials::{CredentialStore, OAuthCredential};
pub use oauth::{login_openai, login_xai};
pub use wire::{AuthMode, InferenceRequest, Provider, ProviderClient, TextMessage, TextRole};
