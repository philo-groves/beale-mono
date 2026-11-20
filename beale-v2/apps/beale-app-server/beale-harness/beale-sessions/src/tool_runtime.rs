//! One dispatch path for model tool calls from main, peer, and subagent loops.
//!
//! Review policy, availability, execution, and output redaction live here so a
//! check added for one agent kind applies to all of them. Loop-local
//! orchestration tools (goal status, graph delegation, handoffs, and subagent
//! messaging) stay in their loops but pass through the same review gate first.

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
    matches!(name, "read" | "write" | "edit" | "bash") || super::features::reviews(name)
}

/// Whether concurrent agents sharing a workspace must serialize this call.
fn takes_mutation_lock(name: &str) -> bool {
    #[cfg(feature = "webfetch")]
    if name == beale_webfetch::TOOL_NAME {
        return false;
    }
    name != "read"
}

/// Pipeline role ownership of finding actions, by tool and requested state.
#[cfg(all(feature = "graphs", feature = "findings"))]
pub(crate) fn pipeline_finding_allowed(role: beale_graphs::Role, name: &str, args: &Value) -> bool {
    use beale_graphs::Role;
    let state = args.get("state").and_then(Value::as_str);
    match name {
        "findings_query" | "findings_read" => true,
        "findings_create" => role == Role::Discovery,
        "findings_update" => match role {
            Role::AuditTargets => false,
            Role::Discovery => matches!(state, Some("finding" | "candidate")),
            Role::Validation => matches!(state, Some("finding" | "validated")),
            Role::Reporter => matches!(state, Some("finding" | "screened" | "reported")),
            Role::PatchGeneration => state == Some("finding"),
            _ => true,
        },
        "findings_complete_state" => match role {
            Role::Validation => state == Some("validated"),
            Role::Reporter => matches!(state, Some("screened" | "reported")),
            Role::AuditTargets | Role::Discovery | Role::PatchGeneration => false,
            _ => true,
        },
        "findings_reject" | beale_findings::VERIFY => role == Role::Validation,
        _ => true,
    }
}

/// Whether a pipeline role may see a finding tool at all: some requested state
/// must be allowed. Derived from [`pipeline_finding_allowed`] so the tool list
/// and the dispatch check cannot disagree.
#[cfg(all(feature = "graphs", feature = "findings"))]
pub(crate) fn pipeline_finding_visible(role: beale_graphs::Role, name: &str) -> bool {
    [
        None,
        Some("finding"),
        Some("candidate"),
        Some("validated"),
        Some("screened"),
        Some("reported"),
    ]
    .into_iter()
    .any(|state| {
        let args = state.map_or_else(|| json!({}), |state| json!({"state": state}));
        pipeline_finding_allowed(role, name, &args)
    })
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
    /// Research Pipeline role that owns finding actions, when one applies.
    #[cfg(feature = "graphs")]
    pub pipeline_role: Option<beale_graphs::Role>,
}

impl ToolRuntime<'_> {
    pub(crate) fn available(&self, name: &str) -> bool {
        self.definitions.iter().any(|tool| tool["name"] == name)
    }

    async fn review_call(&self, name: &str, arguments: &Value) -> ReviewDecision {
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
        if !self.available(name) || !requires_call_review(name) {
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
    #[cfg_attr(not(feature = "graphs"), allow(dead_code))]
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
            Some(lock) if takes_mutation_lock(name) => Some(lock.lock().await),
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
        stop: &mut Option<watch::Receiver<bool>>,
    ) -> ToolResult {
        let _ = (call_id, reviewed, pending);
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
            return self.run_plugin(arguments, stop).await;
        }
        #[cfg(feature = "runbooks")]
        if name == beale_runbooks::RUN {
            return self.run_runbook(arguments, stop).await;
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
        if matches!(name, "read" | "write" | "edit" | "bash") {
            return match until_stopped(
                execute(
                    name,
                    &arguments,
                    self.workspace,
                    self.state.workspaces.root(),
                ),
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
            let decision = self.review_call("plugin_call", &review_args).await;
            if !decision.approved {
                if review_failed(&decision) {
                    review_unavailable = true;
                    anyhow::bail!("{}", decision.reason);
                }
                anyhow::bail!("plugin tool denied by review: {}", decision.reason);
            }
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
                    let decision = self.review_call("bash", &review_args).await;
                    if !decision.approved {
                        if review_failed(&decision) {
                            review_unavailable = true;
                            anyhow::bail!("{}", decision.reason);
                        }
                        anyhow::bail!("runbook cell denied by review: {}", decision.reason);
                    }
                }
            }
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
        #[cfg(feature = "graphs")]
        let evidence_role = match self.pipeline_role {
            Some(role) if !pipeline_finding_allowed(role, name, &arguments) => {
                return ToolResult::error("This finding action belongs to another Research Pipeline agent. Hand off the finding and evidence to that role.".to_owned());
            }
            Some(_) => Some(self.agent_role),
            None => None,
        };
        #[cfg(not(feature = "graphs"))]
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
                beale_tools::run_bash(&request.command, self.workspace, request.timeout_seconds),
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
        for name in ["read", "write", "edit", "bash"] {
            assert!(requires_call_review(name), "{name}");
        }
        assert!(!requires_call_review("plugin_call"));
        #[cfg(feature = "graphs")]
        for name in [
            beale_graphs::SPAWN_TOOL,
            beale_graphs::LIST_TOOL,
            beale_graphs::INSPECT_TOOL,
            beale_graphs::MESSAGE_TOOL,
            beale_graphs::KILL_TOOL,
            beale_graphs::HANDOFF_TOOL,
        ] {
            assert!(!requires_call_review(name), "{name}");
        }
        #[cfg(feature = "findings")]
        assert!(requires_call_review(beale_findings::VERIFY));
    }

    #[test]
    fn only_reads_and_webfetch_skip_the_subagent_mutation_lock() {
        assert!(!takes_mutation_lock("read"));
        for name in ["write", "edit", "bash", "plugin_call"] {
            assert!(takes_mutation_lock(name), "{name}");
        }
        #[cfg(feature = "webfetch")]
        assert!(!takes_mutation_lock(beale_webfetch::TOOL_NAME));
    }

    #[cfg(all(feature = "graphs", feature = "findings"))]
    #[test]
    fn pipeline_tool_visibility_matches_dispatch_permissions() {
        use beale_graphs::Role;
        for role in [
            Role::AuditTargets,
            Role::Discovery,
            Role::Validation,
            Role::Reporter,
            Role::PatchGeneration,
        ] {
            for tool in beale_findings::definitions() {
                let name = tool["name"].as_str().unwrap();
                let any_state = ["finding", "candidate", "validated", "screened", "reported"]
                    .iter()
                    .any(|state| pipeline_finding_allowed(role, name, &json!({"state": state})))
                    || pipeline_finding_allowed(role, name, &json!({}));
                assert_eq!(
                    pipeline_finding_visible(role, name),
                    any_state,
                    "{role:?} {name}"
                );
            }
        }
        assert!(pipeline_finding_visible(
            Role::Validation,
            beale_findings::VERIFY
        ));
        assert!(!pipeline_finding_visible(
            Role::Discovery,
            beale_findings::VERIFY
        ));
        assert!(!pipeline_finding_visible(
            Role::AuditTargets,
            "findings_update"
        ));
    }
}
