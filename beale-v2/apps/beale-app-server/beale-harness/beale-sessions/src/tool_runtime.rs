//! Trusted session dispatch for model tool review, execution, and redaction.
//!
//! Review policy, availability, execution, and output redaction live here so a
//! check added for one agent kind applies to all of them. Loop-local
//! orchestration tools (currently goal status) stay in their loops but pass through the same review gate first.

use std::{collections::HashSet, path::Path};

use beale_contracts::v1::SessionStreamEvent;
use beale_providers::{AuthMode, Provider};
use beale_safety::{ReviewDecision, ReviewRequest};
use beale_storage::ToolCallRow;
use beale_tools::{PluginCallArgs, Redactor, execute};
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc, watch};

#[cfg(feature = "runbooks")]
use beale_runbooks::RunbookStore;

use super::{SessionState, review_failed, until_stopped};

/// Unix milliseconds for a tool call record.
pub(crate) fn unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}

/// Whether a tool call needs Auto-Review before it runs. Plugin calls are
/// reviewed per selected server tool inside execution instead.
pub(crate) fn requires_call_review(name: &str) -> bool {
    matches!(name, "read" | "write" | "edit")
        || name == beale_tools::COMMAND_TOOL_NAME
        || super::features::reviews(name)
}

/// These tools perform their own reviews before acquiring the workspace lock.
fn defers_mutation_lock(name: &str) -> bool {
    matches!(name, "plugin_call" | "runbook_run")
}
async fn await_tool_review(
    future: impl std::future::Future<Output = ReviewDecision>,
    stop: &mut Option<watch::Receiver<bool>>,
) -> anyhow::Result<ReviewDecision> {
    anyhow::ensure!(!super::stop_requested(stop), "Tool stopped by user.");
    until_stopped(future, stop)
        .await
        .ok_or_else(|| anyhow::anyhow!("Tool stopped by user."))
}

/// Whether concurrent agents sharing a workspace must serialize this call.
fn takes_mutation_lock(name: &str) -> bool {
    #[cfg(feature = "webfetch")]
    if name == beale_webfetch::TOOL_NAME {
        return false;
    }
    name != "read"
}

/// The redactor for one turn: host credential values plus credential-named
/// variables in the app-server environment.
pub(crate) fn host_redactor(state: &SessionState) -> Redactor {
    let mut values = state.credentials.host_secret_values();
    values.push(state.token.clone());
    values.extend(
        std::env::vars()
            .filter(|(name, _)| beale_tools::process::credential_env_name(name))
            .map(|(_, value)| value),
    );
    Redactor::new(values)
}

pub(crate) struct ReviewTarget<'a> {
    pub provider: Provider,
    pub mode: AuthMode,
    pub model: &'a str,
    pub context: &'a str,
}

/// The outcome of the review gate for one call.
pub(crate) enum Gate {
    Run { reviewed: bool },
    Denied(String),
    Unavailable(String),
    Stopped,
}

impl Gate {
    /// The tool result for a call that will not run.
    pub(crate) fn blocked(self) -> Option<ToolResult> {
        match self {
            Gate::Run { .. } => None,
            Gate::Denied(reason) => Some(ToolResult::error(format!(
                "Tool denied by review: {reason}"
            ))),
            Gate::Unavailable(reason) => Some(ToolResult {
                output: reason,
                is_error: true,
                review_unavailable: true,
            }),
            Gate::Stopped => Some(ToolResult::error("Stopped by user".to_owned())),
        }
    }
}

pub(crate) struct ToolResult {
    pub output: String,
    pub is_error: bool,
    pub review_unavailable: bool,
}

impl ToolResult {
    pub(crate) fn new(output: String, is_error: bool) -> Self {
        Self {
            output,
            is_error,
            review_unavailable: false,
        }
    }

    pub(crate) fn error(output: String) -> Self {
        Self::new(output, true)
    }
}

/// Everything one agent needs to review and run its tool calls. Some fields
/// are read only by optional features.
#[allow(dead_code)]
pub(crate) struct ToolRuntime<'a> {
    pub local: Option<&'a beale_storage::swarm_state::LocalTurn>,
    pub state: &'a SessionState,
    pub workspace_id: &'a str,
    pub session_id: &'a str,
    /// Main node ID or subagent ID, used where a feature records its author.
    pub agent_id: &'a str,
    /// Stored top-level role of the calling agent.
    pub agent_role: &'a str,
    pub workspace: &'a Path,
    pub review: ReviewTarget<'a>,
    /// Tool definitions offered to this agent; anything else is unavailable.
    pub definitions: &'a [Value],
    pub callable_plugins: &'a HashSet<String>,
    pub redactor: &'a Redactor,
    /// Serializes mutating calls among concurrent subagents.
    pub mutation_lock: Option<&'a Mutex<()>>,
    pub progress: Option<&'a mpsc::Sender<SessionStreamEvent>>,
}

impl ToolRuntime<'_> {
    pub(crate) fn available(&self, name: &str) -> bool {
        self.definitions.iter().any(|tool| tool["name"] == name)
    }

    async fn review_call(&self, name: &str, arguments: &Value) -> ReviewDecision {
        if let Some(local) = self.local {
            return beale_safety::review_agent(
                &self.state.db,
                &self.state.provider,
                &self.state.credentials,
                ReviewRequest {
                    provider: self.review.provider,
                    mode: self.review.mode,
                    model: self.review.model,
                    name,
                    arguments,
                    workspace_context: self.review.context,
                },
                &local.scan_id,
                &local.agent_id,
            )
            .await;
        }
        beale_safety::review(
            &self.state.db,
            &self.state.provider,
            &self.state.credentials,
            ReviewRequest {
                provider: self.review.provider,
                mode: self.review.mode,
                model: self.review.model,
                name,
                arguments,
                workspace_context: self.review.context,
            },
        )
        .await
    }

    /// Reviews a call when its tool requires it. Calls with invalid arguments or
    /// unavailable tools are not reviewed; execution reports them instead.
    pub(crate) async fn gate(
        &self,
        name: &str,
        arguments: Option<&Value>,
        stop: &mut Option<watch::Receiver<bool>>,
    ) -> Gate {
        let Some(arguments) = arguments else {
            return Gate::Run { reviewed: false };
        };
        if !self.available(name)
            || (!requires_call_review(name)
                && !self
                    .local
                    .is_some_and(|local| local.role == beale_contracts::swarms::Role::Leader))
        {
            return Gate::Run { reviewed: false };
        }
        match until_stopped(self.review_call(name, arguments), stop).await {
            None => Gate::Stopped,
            Some(decision) if decision.approved => Gate::Run {
                reviewed: decision.reviewed,
            },
            Some(decision) if review_failed(&decision) => Gate::Unavailable(decision.reason),
            Some(decision) => Gate::Denied(decision.reason),
        }
    }

    /// Reviews and runs a host tool call.
    #[allow(dead_code)]
    pub(crate) async fn dispatch(
        &self,
        call_id: &str,
        name: &str,
        arguments: Result<Value, serde_json::Error>,
        pending: &[ToolCallRow],
        stop: &mut Option<watch::Receiver<bool>>,
    ) -> ToolResult {
        let gate = self.gate(name, arguments.as_ref().ok(), stop).await;
        let reviewed = match gate {
            Gate::Run { reviewed } => reviewed,
            blocked => {
                return blocked
                    .blocked()
                    .unwrap_or_else(|| ToolResult::error(String::new()));
            }
        };
        self.execute(call_id, name, arguments, reviewed, pending, stop)
            .await
    }

    /// Runs an already gated host tool call and redacts its output.
    pub(crate) async fn execute(
        &self,
        call_id: &str,
        name: &str,
        arguments: Result<Value, serde_json::Error>,
        reviewed: bool,
        pending: &[ToolCallRow],
        stop: &mut Option<watch::Receiver<bool>>,
    ) -> ToolResult {
        if let Some(local) = self.local
            && beale_storage::swarm_state::SwarmStore::open(self.state.db.clone())
                .and_then(|s| s.local_authorize(local))
                .is_err()
        {
            return ToolResult::error("Local execution is no longer authorized.".into());
        }
        let Ok(arguments) = arguments else {
            return ToolResult::error("Tool error: invalid JSON arguments".to_owned());
        };
        if super::stop_requested(stop) {
            return ToolResult::error("Tool stopped by user.".to_owned());
        }
        if !self.available(name) {
            return ToolResult::error("Tool error: unavailable to this agent".to_owned());
        }
        let _guard = match self.mutation_lock {
            Some(lock) if takes_mutation_lock(name) && !defers_mutation_lock(name) => {
                Some(lock.lock().await)
            }
            _ => None,
        };
        let mut result = self
            .run(call_id, name, arguments, reviewed, pending, stop)
            .await;
        result.output = self.redactor.redact(&result.output).into_owned();
        result
    }

    async fn run(
        &self,
        call_id: &str,
        name: &str,
        arguments: Value,
        reviewed: bool,
        pending: &[ToolCallRow],
        review_stop: &mut Option<watch::Receiver<bool>>,
    ) -> ToolResult {
        // Pending reviews remain cancelable. A local supervisor releases
        // capacity only after an already dispatched managed effect finishes.
        let mut execution_stop = if self.local.is_some() {
            None
        } else {
            review_stop.clone()
        };
        let stop = &mut execution_stop;
        let _ = (call_id, reviewed, pending);
        #[cfg(feature = "swarms")]
        let mut arguments = arguments;
        #[cfg(feature = "swarms")]
        if let Some(turn) = self.local {
            if name == "swarm_materialize_artifact" {
                return match self.materialize(arguments) {
                    Ok(value) => ToolResult::new(value.to_string(), false),
                    Err(error) => super::research_files::tool_error(&error),
                };
            }
            if (matches!(name, "swarm_message_parent" | "swarm_handoff")
                || name.starts_with("findings_"))
                && let Err(error) = self.attachments(
                    &super::research_files::operation(turn, call_id),
                    &mut arguments,
                )
            {
                return super::research_files::tool_error(&error);
            }
        }

        #[cfg(feature = "swarms")]
        if name.starts_with("swarm_") {
            let Some(local) = self.local else {
                return ToolResult::error("No active Swarm identity".into());
            };
            let admission = if name == "swarm_spawn_subagent" {
                crate::distributed::placement::admission(self.state, local).await
            } else {
                beale_contracts::swarm_local::Admission::Ready
            };
            return match beale_storage::swarm_state::SwarmStore::open(self.state.db.clone())
                .and_then(|store| {
                    let command = serde_json::from_value(
                        serde_json::json!({"tool":name,"arguments":arguments}),
                    )?;
                    store.local_dispatch(local, call_id, command, admission)
                }) {
                Ok(value) => ToolResult::new(value.to_string(), false),
                Err(error) => {
                    beale_host_log::error!("local Swarm dispatch rejected: {error:#}");
                    ToolResult::error("Swarm request rejected: verify the saved profile, direct-child relationship, task or handoff schema, and current agent state.".into())
                }
            };
        }
        #[cfg(feature = "webfetch")]
        if name == beale_webfetch::TOOL_NAME {
            return match until_stopped(
                super::execute_webfetch(&self.state.db, self.workspace_id, &arguments),
                stop,
            )
            .await
            {
                Some(Ok(value)) => ToolResult::new(value.output, value.is_error),
                Some(Err(error)) => ToolResult::error(format!("Webfetch error: {error:#}")),
                None => ToolResult::error("Webfetch stopped by user.".to_owned()),
            };
        }
        if name == "plugin_call" {
            return self.run_plugin(arguments, stop, review_stop).await;
        }
        #[cfg(feature = "runbooks")]
        if name == beale_runbooks::RUN {
            return self.run_runbook(arguments, stop, review_stop).await;
        }
        #[cfg(feature = "runbooks")]
        if beale_runbooks::is_tool(name) {
            return match beale_runbooks::execute(
                &self.state.db,
                self.workspace_id,
                self.session_id,
                name,
                &arguments,
            ) {
                Ok(value) => ToolResult::new(value, false),
                Err(error) => ToolResult::error(format!("Runbook tool error: {error}")),
            };
        }
        #[cfg(feature = "findings")]
        if beale_findings::is_tool(name) {
            return self
                .run_findings(call_id, name, arguments, pending, stop)
                .await;
        }
        #[cfg(feature = "memories")]
        if beale_memories::is_tool(name) {
            return match beale_memories::MemoryStore::open(self.state.workspaces.root()).and_then(
                |store| {
                    beale_memories::execute(
                        &store,
                        &self.state.db,
                        self.workspace_id,
                        self.session_id,
                        name,
                        &arguments,
                    )
                },
            ) {
                Ok(value) => ToolResult::new(value, false),
                Err(error) => ToolResult::error(format!("Memory tool error: {error}")),
            };
        }
        #[cfg(feature = "agent-feedback")]
        if name == beale_agent_feedback::TOOL_NAME {
            return match beale_agent_feedback::execute(
                self.state.workspaces.root(),
                self.workspace_id,
                self.session_id,
                self.agent_id,
                &arguments,
            ) {
                Ok(value) => ToolResult::new(value, false),
                Err(error) => ToolResult::error(format!("Agent feedback error: {error}")),
            };
        }
        #[cfg(feature = "surfaces")]
        if beale_surfaces::is_tool(name) {
            return match beale_surfaces::execute(
                &self.state.workspaces,
                self.workspace_id,
                name,
                &arguments,
                reviewed,
            ) {
                Ok(value) => ToolResult::new(value, false),
                Err(error) => ToolResult::error(format!("Surface tool error: {error}")),
            };
        }
        if matches!(name, "read" | "write" | "edit") || name == beale_tools::COMMAND_TOOL_NAME {
            if name == beale_tools::COMMAND_TOOL_NAME
                && let Some(machine) = arguments.get("machine")
            {
                let Some(machine) = machine.as_str().filter(|value| !value.is_empty()) else {
                    return ToolResult::error("Tool error: invalid machine ID".to_owned());
                };
                let Some(command) = arguments.get("command").and_then(Value::as_str) else {
                    return ToolResult::error("Tool error: invalid command".to_owned());
                };
                let timeout_seconds = match arguments.get("timeout_seconds") {
                    None => beale_tools::DEFAULT_COMMAND_TIMEOUT_SECONDS,
                    Some(value) => match value.as_u64() {
                        Some(value) => value,
                        None => {
                            return ToolResult::error(
                                "Tool error: invalid command timeout_seconds".to_owned(),
                            );
                        }
                    },
                };
                return match until_stopped(
                    beale_fleet::run_machine_command(
                        &self.state.db,
                        self.workspace_id,
                        machine,
                        command,
                        self.workspace,
                        timeout_seconds,
                    ),
                    stop,
                )
                .await
                {
                    Some(Ok(value)) => ToolResult::new(value.transcript(), !value.success),
                    Some(Err(error)) => ToolResult::error(format!("Fleet command error: {error}")),
                    None => ToolResult::error("Fleet command stopped by user.".to_owned()),
                };
            }
            return match until_stopped(
                async {
                    if self
                        .local
                        .is_some_and(|turn| turn.role != beale_contracts::swarms::Role::Leader)
                    {
                        beale_tools::execute_in_environment(
                            name,
                            &arguments,
                            self.workspace,
                            self.state.workspaces.root(),
                        )
                        .await
                    } else {
                        execute(
                            name,
                            &arguments,
                            self.workspace,
                            self.state.workspaces.root(),
                        )
                        .await
                    }
                },
                stop,
            )
            .await
            {
                Some(Ok(value)) => ToolResult::new(value.output, value.is_error),
                Some(Err(error)) => ToolResult::error(format!("Tool error: {error:#}")),
                None => ToolResult::error("Tool stopped by user.".to_owned()),
            };
        }
        ToolResult::error("Tool error: unknown tool".to_owned())
    }

    async fn run_plugin(
        &self,
        arguments: Value,
        stop: &mut Option<watch::Receiver<bool>>,
        review_stop: &mut Option<watch::Receiver<bool>>,
    ) -> ToolResult {
        let mut review_unavailable = false;
        let result = async {
            let parsed: PluginCallArgs = serde_json::from_value(arguments)?;
            anyhow::ensure!(parsed.arguments.is_object(), "arguments must be an object");
            anyhow::ensure!(
                self.callable_plugins.contains(&parsed.plugin),
                "plugin tools are unavailable in this workspace"
            );
            let review_args = json!({"plugin":parsed.plugin,"server":parsed.server,
                "tool":parsed.tool,"arguments":parsed.arguments});
            let decision =
                await_tool_review(self.review_call("plugin_call", &review_args), review_stop)
                    .await?;
            if !decision.approved {
                if review_failed(&decision) {
                    review_unavailable = true;
                    anyhow::bail!("{}", decision.reason);
                }
                anyhow::bail!("plugin tool denied by review: {}", decision.reason);
            }
            let _guard = match self.mutation_lock {
                Some(lock) => Some(
                    until_stopped(lock.lock(), review_stop)
                        .await
                        .ok_or_else(|| anyhow::anyhow!("Tool stopped by user."))?,
                ),
                None => None,
            };
            let value = self
                .state
                .plugins
                .call_tool(
                    &parsed.plugin,
                    &parsed.server,
                    &parsed.tool,
                    parsed.arguments,
                    self.workspace_id,
                )
                .await?;
            Ok::<_, anyhow::Error>((
                value.to_string(),
                value["isError"].as_bool().unwrap_or(false),
            ))
        };
        let mut result = match until_stopped(result, stop).await {
            Some(Ok((output, is_error))) => ToolResult::new(output, is_error),
            Some(Err(error)) => ToolResult::error(format!("Plugin tool error: {error}")),
            None => ToolResult::error("Plugin tool stopped by user.".to_owned()),
        };
        result.review_unavailable = review_unavailable;
        result
    }

    #[cfg(feature = "runbooks")]
    async fn run_runbook(
        &self,
        arguments: Value,
        stop: &mut Option<watch::Receiver<bool>>,
        review_stop: &mut Option<watch::Receiver<bool>>,
    ) -> ToolResult {
        use beale_contracts::v1::SessionStreamKind;
        let mut review_unavailable = false;
        let result = async {
            let runbook_id = arguments
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("id is required"))?;
            let document = self
                .state
                .db
                .runbook_get(self.workspace_id, runbook_id)?
                .ok_or_else(|| anyhow::anyhow!("runbook not found"))?;
            for cell in &document.cells {
                if let beale_contracts::runbooks::Cell::Code {
                    language, source, ..
                } = cell
                {
                    let review_args =
                        json!({"command":format!("# {language} runbook cell\n{source}")});
                    let decision = await_tool_review(
                        self.review_call(beale_tools::COMMAND_TOOL_NAME, &review_args),
                        review_stop,
                    )
                    .await?;
                    if !decision.approved {
                        if review_failed(&decision) {
                            review_unavailable = true;
                            anyhow::bail!("{}", decision.reason);
                        }
                        anyhow::bail!("runbook cell denied by review: {}", decision.reason);
                    }
                }
            }
            let _guard = match self.mutation_lock {
                Some(lock) => Some(
                    until_stopped(lock.lock(), review_stop)
                        .await
                        .ok_or_else(|| anyhow::anyhow!("Tool stopped by user."))?,
                ),
                None => None,
            };
            let run = self.state.db.runbook_run_start(
                self.workspace_id,
                self.session_id,
                runbook_id,
                document.revision,
            )?;
            let mut active = super::ActiveRunbookRun {
                db: self.state.db.clone(),
                id: run.id.clone(),
                finished: false,
            };
            let result = beale_runbooks::run_document(
                &document,
                self.workspace,
                |cell_number| {
                    let cell_name = document.cells[(cell_number - 1) as usize].name();
                    self.state
                        .db
                        .runbook_run_cell(&run.id, cell_number, cell_name)?;
                    if let Some(tx) = self.progress {
                        let _ = tx.try_send(SessionStreamEvent {
                            kind: SessionStreamKind::RunbookProgress as i32,
                            runbook_progress: Some(beale_contracts::v1::RunbookProgress {
                                runbook_id: runbook_id.to_owned(),
                                title: document.title.clone(),
                                cell_number,
                                cell_name: cell_name.to_owned(),
                            }),
                            ..Default::default()
                        });
                    }
                    Ok(())
                },
                |cell_id, success, exit_code, stdout, stderr| {
                    self.state.db.runbook_run_cell_result(
                        &run.id,
                        cell_id,
                        success,
                        exit_code,
                        &self.redactor.redact(stdout),
                        &self.redactor.redact(stderr),
                    )
                },
            )
            .await;
            let (success, output) = match result {
                Ok(result) => (result.success, result.output),
                Err(error) => (false, format!("Runbook execution error: {error}")),
            };
            active.finish(success, &self.redactor.redact(&output))?;
            Ok((
                json!({"run_id":run.id,"runbook_id":runbook_id,
                    "status":if success {"completed"} else {"failed"}})
                .to_string(),
                !success,
            ))
        };
        let mut result = match until_stopped(result, stop).await {
            Some(Ok((output, is_error))) => ToolResult::new(output, is_error),
            Some(Err(error)) => ToolResult::error(format!("Runbook tool error: {error}")),
            None => ToolResult::error("Runbook stopped by user.".to_owned()),
        };
        result.review_unavailable = review_unavailable;
        result
    }

    #[cfg(feature = "findings")]
    async fn run_findings(
        &self,
        call_id: &str,
        name: &str,
        arguments: Value,
        pending: &[ToolCallRow],
        stop: &mut Option<watch::Receiver<bool>>,
    ) -> ToolResult {
        let _ = stop;

        let evidence_role: Option<&str> = None;
        let pending = pending
            .iter()
            .map(|row| beale_findings::PendingSource {
                id: &row.id,
                name: &row.name,
                output: &row.output,
                is_error: row.is_error,
                agent_role: &row.agent_role,
                created_at_ms: row.created_at_ms,
            })
            .collect::<Vec<_>>();
        let context = beale_findings::ToolContext {
            workspace_id: self.workspace_id,
            session_id: self.session_id,
            evidence_role,
            pending: &pending,
        };
        if name == beale_findings::VERIFY {
            return self
                .run_verification(call_id, &context, &arguments, stop)
                .await;
        }
        match beale_findings::execute(&self.state.db, &context, name, &arguments) {
            Ok(value) => ToolResult::new(value, false),
            Err(error) => ToolResult::error(format!("Findings tool error: {error}")),
        }
    }

    /// Reruns a declared proof command in fresh processes and records the
    /// host-evaluated oracle outcome on the finding.
    #[cfg(feature = "findings")]
    async fn run_verification(
        &self,
        call_id: &str,
        context: &beale_findings::ToolContext<'_>,
        arguments: &Value,
        stop: &mut Option<watch::Receiver<bool>>,
    ) -> ToolResult {
        let request = match beale_findings::VerifyRequest::parse(arguments) {
            Ok(request) => request,
            Err(error) => return ToolResult::error(format!("Findings tool error: {error}")),
        };
        if let Err(error) = beale_findings::check_verifiable(&self.state.db, context, &request) {
            return ToolResult::error(format!("Findings tool error: {error}"));
        }
        let mut runs = Vec::with_capacity(beale_findings::VERIFY_RUNS);
        for _ in 0..beale_findings::VERIFY_RUNS {
            let started = std::time::Instant::now();
            let output = match until_stopped(
                beale_tools::run_command(&request.command, self.workspace, request.timeout_seconds),
                stop,
            )
            .await
            {
                None => return ToolResult::error("Verification stopped by user.".to_owned()),
                Some(output) => output,
            };
            let elapsed = started.elapsed().as_millis().min(i64::MAX as u128) as i64;
            runs.push(match output {
                Ok(output) => beale_findings::VerifyRun::observe(
                    &request.oracle,
                    output.exit_code,
                    &self.redactor.redact(&output.stdout),
                    &self.redactor.redact(&output.stderr),
                    elapsed,
                ),
                Err(error) => beale_findings::VerifyRun::failed(
                    &self.redactor.redact(&format!("{error:#}")),
                    elapsed,
                ),
            });
        }
        match beale_findings::record_verification(
            &self.state.db,
            context,
            &request,
            call_id,
            self.agent_role,
            runs,
        ) {
            Ok((value, verified)) => ToolResult::new(value, !verified),
            Err(error) => ToolResult::error(format!("Findings tool error: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_policy_covers_base_and_feature_tools_but_not_orchestration() {
        for name in ["read", "write", "edit", beale_tools::COMMAND_TOOL_NAME] {
            assert!(requires_call_review(name), "{name}");
        }
        assert!(!requires_call_review("plugin_call"));

        #[cfg(feature = "findings")]
        assert!(requires_call_review(beale_findings::VERIFY));
    }

    #[test]
    fn only_reads_and_webfetch_skip_the_subagent_mutation_lock() {
        assert!(!takes_mutation_lock("read"));
        for name in [
            "write",
            "edit",
            beale_tools::COMMAND_TOOL_NAME,
            "plugin_call",
        ] {
            assert!(takes_mutation_lock(name), "{name}");
        }
        #[cfg(feature = "webfetch")]
        assert!(!takes_mutation_lock(beale_webfetch::TOOL_NAME));
        assert!(defers_mutation_lock("plugin_call"));
        assert!(defers_mutation_lock("runbook_run"));
        assert!(!defers_mutation_lock(beale_tools::COMMAND_TOOL_NAME));
    }

    #[tokio::test]
    async fn inner_review_wait_is_cancelable_and_cannot_restart_after_stop() {
        let (sender, receiver) = watch::channel(false);
        let mut stop = Some(receiver);
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let review = async {
            entered.send(()).unwrap();
            std::future::pending::<ReviewDecision>().await
        };
        let cancel = async {
            waiting.await.unwrap();
            sender.send(true).unwrap();
        };
        let (result, ()) = tokio::join!(await_tool_review(review, &mut stop), cancel);
        assert_eq!(result.unwrap_err().to_string(), "Tool stopped by user.");
        // The cancellation has already been observed, so a later review must
        // check the current value instead of waiting for another watch change.
        let result =
            await_tool_review(async { panic!("review dispatched after stop") }, &mut stop).await;
        assert!(result.is_err());
        let decision = await_tool_review(
            async {
                ReviewDecision {
                    approved: true,
                    reviewed: true,
                    reason: "Synthetic approval".into(),
                }
            },
            &mut None,
        )
        .await
        .unwrap();
        assert!(decision.approved && decision.reviewed);
    }
}
