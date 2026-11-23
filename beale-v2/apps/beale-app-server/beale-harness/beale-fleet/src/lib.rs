//! Fleet feature metadata and authenticated host descriptor.

mod browsers;
mod execution;
mod store;
pub use store::{
    BrowserSettings, FleetStore, MIGRATIONS, ManualBrowserRow, VirtualMachineSettings, migrate,
};
mod http;
mod research_tools;
mod resources;
mod virtual_machines;
pub use execution::{machine_prompt_note, run_machine_command};
pub use http::router;
pub use research_tools::{allowed_tools, prompt_note};

pub const FEATURE_ID: &str = "fleet";
pub const FEATURE_NAME: &str = "Fleet";
pub const FEATURE_DESCRIPTION: &str = "Manage app-server connections over Tailscale.";
pub const HAS_PROMPT_EXTENSION: bool = true;
pub const TOOL_COUNT: u32 = 0;

/// Harness descriptor for Fleet session guidance.
pub const FEATURE: beale_feature_api::Feature = beale_feature_api::Feature {
    id: FEATURE_ID,
    name: FEATURE_NAME,
    description: FEATURE_DESCRIPTION,
    prompt_extension: PROMPT_EXTENSION,
    use_case: "Use approved research tools and running Hyper-V guests where allowed.",
    tool_names: &[],
    definitions: None,
    review: beale_feature_api::ToolReview::Exempt,
    subagents: true,
    migrations: MIGRATIONS,
    startup: &[],
};
pub const PROMPT_EXTENSION: &str = "Allowed research tools are listed in the active session only when at least one is approved locally or in VMs.";
