//! Fleet feature metadata and authenticated host descriptor.

mod browsers;
mod http;
mod virtual_machines;
pub use http::router;

pub const FEATURE_ID: &str = "fleet";
pub const FEATURE_NAME: &str = "Fleet";
pub const FEATURE_DESCRIPTION: &str = "Manage app-server connections over Tailscale.";
pub const HAS_PROMPT_EXTENSION: bool = false;
pub const TOOL_COUNT: u32 = 0;
pub const PROMPT_EXTENSION: &str = "";
