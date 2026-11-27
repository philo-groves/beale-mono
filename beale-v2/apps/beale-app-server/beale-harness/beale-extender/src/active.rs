//! Registry locks protect references only. RPCs serialize on their own server.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) struct ActivePlugin {
    pub(super) enabled: AtomicBool,
    pub(super) activation: Mutex<()>,
    pub(super) servers: Mutex<HashMap<String, Arc<ActiveServer>>>,
}
impl Default for ActivePlugin {
    fn default() -> Self {
        Self {
            enabled: AtomicBool::new(true),
            activation: Mutex::new(()),
            servers: Mutex::default(),
        }
    }
}
pub(super) struct ActiveServer {
    pub(super) enabled: AtomicBool,
    pub(super) runtime: Mutex<mcp_runtime::RunningServer>,
    pub(super) tool_count: u32,
    pub(super) prompt_count: u32,
    pub(super) resource_count: u32,
    pub(super) tool_names: Vec<String>,
    pub(super) tools: Vec<Value>,
}
impl ActiveServer {
    pub(super) fn new(server: mcp_runtime::RunningServer) -> Self {
        Self {
            enabled: AtomicBool::new(true),
            tool_count: server.tool_count,
            prompt_count: server.prompt_count,
            resource_count: server.resource_count,
            tool_names: server.tool_names.clone(),
            tools: server.tools.clone(),
            runtime: Mutex::new(server),
        }
    }
    pub(super) fn alive(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
            && self
                .runtime
                .try_lock()
                .map_or(true, |mut server| server.alive())
    }
}

impl Catalog {
    pub(super) async fn active_plugin(&self, name: &str) -> Option<Arc<ActivePlugin>> {
        self.active.lock().await.get(name).cloned()
    }
    pub(super) async fn active_servers(&self, name: &str) -> HashMap<String, Arc<ActiveServer>> {
        match self.active_plugin(name).await {
            Some(plugin) => plugin.servers.lock().await.clone(),
            None => HashMap::new(),
        }
    }
}

/// An outer error means no `tools/call` request was sent. After dispatch,
/// transport loss is distinct from a completed MCP result (including isError).
pub enum GuardedToolResult {
    Completed(Value),
    OutcomeUnknown(anyhow::Error),
}

impl PluginRuntime {
    /// Validate current host configuration, serialize this server, then invoke
    /// the durable host claim immediately before the first request write.
    /// The callback commits synchronously and must not wait on an asynchronous lock.
    pub async fn call_tool_guarded(
        &self,
        invocation: &beale_contracts::swarm_plugins::Invocation,
        workspace_id: &str,
        before_dispatch: impl FnOnce() -> Result<()>,
    ) -> Result<GuardedToolResult> {
        invocation.validate().map_err(anyhow::Error::msg)?;
        self.require_available(&invocation.plugin, workspace_id)?;
        let plugin = self
            .catalog
            .active_plugin(&invocation.plugin)
            .await
            .context("plugin server is unavailable; prepare the plugin first")?;
        let server = plugin
            .servers
            .lock()
            .await
            .get(&invocation.server)
            .cloned()
            .context("plugin server is unavailable")?;
        let mut runtime = timeout(Duration::from_secs(60), server.runtime.lock())
            .await
            .context("plugin server is busy")?;
        self.require_available(&invocation.plugin, workspace_id)?;
        ensure!(
            plugin.enabled.load(Ordering::SeqCst) && server.enabled.load(Ordering::SeqCst),
            "plugin server was deactivated"
        );
        runtime
            .call_tool_guarded(
                &invocation.tool,
                invocation.arguments.clone(),
                before_dispatch,
            )
            .await
    }
    pub(super) fn require_available(&self, plugin: &str, workspace_id: &str) -> Result<()> {
        ensure!(
            self.db
                .installed_plugin_names()?
                .iter()
                .any(|name| name == plugin),
            "plugin is not installed"
        );
        ensure!(
            self.db
                .extension_applies_to("plugin", plugin, workspace_id)?,
            "plugin is unavailable in this workspace"
        );
        Ok(())
    }
}

#[cfg(test)]
#[path = "active_tests.rs"]
mod tests;
