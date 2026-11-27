//! Assigned agent execution. This module opens only the worker spool and local
//! workspace; it never initializes host storage or a provider credential store.
use super::{Lease, TransportError, TransportFailure, WorkerTransport};
use anyhow::{Context, Result, ensure};
use beale_contracts::{swarm_worker::*, swarms::*};
use beale_storage::worker_spool::{Claim, WorkerSpool};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};
use uuid::Uuid;
#[path = "worker_artifacts.rs"]
mod artifacts;
#[cfg(feature = "findings")]
#[path = "worker_findings.rs"]
mod findings;
#[path = "worker_plugin_resources.rs"]
mod plugin_resources;
#[cfg(feature = "runbooks")]
#[path = "worker_runbooks.rs"]
mod runbooks;

/// Private supervisor configuration delivered on stdin, not a model-visible
/// DTO. Deliberately has no Debug implementation and no provider configuration.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerLaunch {
    pub version: u32,
    pub identity: ExecutionIdentity,
    pub leader: String,
    pub bearer: String,
    pub workspace: PathBuf,
    pub protected_root: PathBuf,
    pub recovery: PathBuf,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingCall {
    call: InferenceCall,
    review_id: String,
    publish_id: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
enum Phase {
    Infer {
        operation_id: String,
    },
    Tools {
        calls: Vec<PendingCall>,
    },
    Done {
        operation_id: String,
        answer: String,
    },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LoopState {
    version: u32,
    shell: WorkerShell,
    route_id: String,
    format: MessageFormat,
    enabled_tools: Vec<String>,
    input: Vec<Value>,
    provider_failures: u32,
    active_elapsed_ms: u64,
    phase: Phase,
}

/// Running-process elapsed time. A new process resumes only the last durable
/// counter; downtime and uncertain time after the last checkpoint are excluded.
struct ActiveClock {
    base_ms: u64,
    started: Instant,
}
impl ActiveClock {
    fn new(base_ms: u64) -> Result<Self> {
        ensure!(
            base_ms <= MAX_ACTIVE_ELAPSED_MS,
            "worker active duration exceeds storage bound"
        );
        Ok(Self {
            base_ms,
            started: Instant::now(),
        })
    }
    fn elapsed_at(&self, now: Instant) -> Result<u64> {
        let elapsed = now
            .checked_duration_since(self.started)
            .context("worker monotonic clock regressed")?;
        let total = self
            .base_ms
            .checked_add(u64::try_from(elapsed.as_millis())?)
            .context("worker active duration overflow")?;
        ensure!(
            total <= MAX_ACTIVE_ELAPSED_MS,
            "worker active duration exceeds storage bound"
        );
        Ok(total)
    }
}

struct Worker<'a> {
    launch: &'a WorkerLaunch,
    transport: WorkerTransport,
    spool: WorkerSpool,
    lease: Mutex<Lease>,
}

struct CallOutcome {
    result: Value,
    /// Rejected calls have no environment or canonical operation to acknowledge.
    operations: Vec<String>,
}

fn invalid_native_arguments(name: &str, arguments: &Value) -> Option<&'static str> {
    if (matches!(
        name,
        "read" | "write" | "edit" | "swarm_materialize_artifact"
    ) || name == beale_tools::COMMAND_TOOL_NAME)
        && arguments.get("machine").is_some()
    {
        return Some("Assigned tools cannot select another machine. Remove the machine argument.");
    }
    let text = |key: &str| arguments.get(key).and_then(Value::as_str);
    let range = |key: &str, maximum: u64| {
        arguments
            .get(key)
            .is_none_or(|value| value.as_u64().is_some_and(|v| (1..=maximum).contains(&v)))
    };
    let valid = match name {
        "read" => text("path").is_some() && range("offset", 1_000_000_000) && range("limit", 2000),
        "write" => text("path").is_some() && text("content").is_some(),
        "edit" => {
            text("path").is_some()
                && text("old_text").is_some_and(|s| !s.is_empty())
                && text("new_text").is_some()
        }
        "swarm_materialize_artifact" => serde_json::from_value::<
            beale_contracts::swarm_files::MaterializeArtifact,
        >(arguments.clone())
        .is_ok_and(|a| {
            valid_id(&a.artifact) && beale_contracts::swarm_files::relative_path(&a.destination)
        }),
        name if name == beale_tools::COMMAND_TOOL_NAME => {
            arguments.get("machine").is_none()
                && text("command").is_some_and(|command| {
                    !command.trim().is_empty() && command.len() <= beale_tools::MAX_COMMAND_BYTES
                })
                && range("timeout_seconds", beale_tools::MAX_COMMAND_TIMEOUT_SECONDS)
        }
        _ => true,
    };
    (!valid).then_some("Invalid tool arguments. Check the supplied tool schema and correct the required fields or ranges.")
}

impl Worker<'_> {
    fn request(&self, id: String, operation: WorkerOperation) -> WorkerRequest {
        let identity = &self.launch.identity;
        WorkerRequest {
            version: VERSION,
            workspace_id: identity.workspace_id.clone(),
            scan_id: identity.scan_id.clone(),
            agent_id: identity.agent_id.clone(),
            attempt_id: identity.attempt_id.clone(),
            generation: identity.generation,
            operation_id: id,
            operation,
        }
    }
    fn deadline(&self) -> Result<Instant> {
        self.lease
            .lock()
            .map_err(|_| anyhow::anyhow!("worker lease unavailable"))?
            .dispatch_deadline(Instant::now())
    }
    async fn renew(&self) -> Result<()> {
        let renewal = self
            .lease
            .lock()
            .map_err(|_| anyhow::anyhow!("worker lease unavailable"))?
            .begin_renewal(Instant::now())?;
        let request = self.request(renewal.request_id.clone(), WorkerOperation::Heartbeat {});
        let timeout = self
            .deadline()
            .map(|deadline| {
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_secs(5))
            })
            .unwrap_or(Duration::from_secs(5));
        let reply = self.transport.send(&request, timeout).await?;
        let WorkerReply::Lease { grant } = reply else {
            anyhow::bail!("invalid worker lease response")
        };
        self.lease
            .lock()
            .map_err(|_| anyhow::anyhow!("worker lease unavailable"))?
            .apply(renewal, &grant, Instant::now())
    }
    async fn keep_alive(&self) -> Result<()> {
        let mut failures = 0_u32;
        loop {
            let remaining = self.deadline()?.saturating_duration_since(Instant::now());
            let delay = if failures == 0 {
                Duration::from_secs(10).min(remaining / 3)
            } else {
                Duration::from_millis(250) * (1 << (failures - 1))
            };
            tokio::time::sleep(delay.min(remaining)).await;
            self.deadline()?;
            match self.renew().await {
                Ok(()) => failures = 0,
                Err(error) => {
                    failures += 1;
                    if failures >= 3
                        || !error
                            .downcast_ref::<TransportError>()
                            .is_some_and(|e| e.kind == TransportFailure::Retryable)
                    {
                        return Err(error);
                    }
                }
            }
        }
    }
    async fn remote(&self, request: WorkerRequest) -> Result<Value> {
        self.deadline()?;
        let work = WorkRequest::Leader {
            request: request.clone(),
        };
        match self.spool.claim(&work)? {
            Claim::Replay(value) => return Ok(value),
            Claim::Dispatch | Claim::RetryLeader => (),
            _ => anyhow::bail!("worker remote operation has an unresolved outcome"),
        }
        for attempt in 0..3 {
            self.deadline()?;
            match self.transport.send_waiting(&request).await {
                Ok(WorkerReply::Acknowledged { acknowledgement }) => {
                    self.deadline()?;
                    self.spool
                        .complete(&request.operation_id, &acknowledgement.result)?;
                    return Ok(acknowledgement.result);
                }
                Ok(_) => anyhow::bail!("unexpected worker operation response"),
                Err(error) => {
                    if error.kind != TransportFailure::Retryable || attempt == 2 {
                        return Err(error.into());
                    }
                    tokio::time::sleep(Duration::from_millis(250) * (1 << attempt)).await;
                }
            }
        }
        unreachable!("bounded retry returns on its final attempt")
    }
    async fn publish_checkpoint(&self) -> Result<u64> {
        let saved = self
            .spool
            .latest_checkpoint()?
            .context("worker checkpoint unavailable")?;
        let request = self.request(
            format!("checkpoint:{}", saved.operation),
            WorkerOperation::Checkpoint {
                revision: saved.revision,
                active_elapsed_ms: saved.context["active_elapsed_ms"]
                    .as_u64()
                    .context("worker duration missing from checkpoint")?,
                context: saved.context,
            },
        );
        let id = request.operation_id.clone();
        let result = self.remote(request).await?;
        ensure!(
            result["revision"].as_u64() == Some(saved.revision),
            "worker checkpoint acknowledgement mismatch"
        );
        self.spool.acknowledge(&id)?;
        Ok(saved.revision)
    }
    async fn save(&self, state: &LoopState, clock: &ActiveClock) -> Result<u64> {
        let mut state = state.clone();
        state.active_elapsed_ms = clock.elapsed_at(Instant::now())?;
        self.deadline()?;
        let previous = self.spool.latest_checkpoint()?.map_or(0, |c| c.revision);
        self.spool.checkpoint(
            &Uuid::new_v4().to_string(),
            previous,
            &serde_json::to_value(&state)?,
        )?;
        self.publish_checkpoint().await
    }
    async fn reject_call(&self, pending: &PendingCall, reason: &str) -> Result<CallOutcome> {
        let result = json!({"output":reason,"is_error":true});
        self.publish_tool_outcome(pending, &json!(pending.call.arguments), &result)
            .await?;
        Ok(CallOutcome {
            result,
            operations: Vec::new(),
        })
    }

    async fn publish_tool_outcome(
        &self,
        pending: &PendingCall,
        arguments: &Value,
        result: &Value,
    ) -> Result<()> {
        let call = &pending.call;
        let payload = json!({"kind":"tool_outcome","call_id":call.id,"tool":call.name,"arguments":arguments,"result":result});
        let redactor = beale_tools::Redactor::new([self.launch.bearer.clone()]);
        let payload = serde_json::from_str(&redactor.redact(&serde_json::to_string(&payload)?))?;
        self.remote(self.request(
            pending.publish_id.clone(),
            WorkerOperation::Event {
                event_id: pending.publish_id.clone(),
                payload,
            },
        ))
        .await?;
        Ok(())
    }

    fn call_operation(&self, call: &str) -> Result<String> {
        Ok(format!(
            "call:{}",
            beale_storage::swarm_files::hash(&serde_json::to_vec(&(&self.launch.identity, call))?)
        ))
    }

    async fn execute_call(&self, state: &LoopState, pending: &PendingCall) -> Result<CallOutcome> {
        let call = &pending.call;
        let mut arguments: Value = match serde_json::from_str(&call.arguments) {
            Ok(value) if matches!(value, Value::Object(_)) => value,
            _ => return self.reject_call(pending, "Tool arguments must be a valid JSON object. Correct the arguments and try again.").await,
        };
        if !state.enabled_tools.contains(&call.name) {
            return self.reject_call(pending, "This tool is unavailable to the assigned worker. Use one of the supplied tools.").await;
        }
        if let Some(reason) = invalid_native_arguments(&call.name, &arguments) {
            return self.reject_call(pending, reason).await;
        }
        #[cfg(feature = "webfetch")]
        if call.name == beale_webfetch::TOOL_NAME
            && let Err(error) = beale_webfetch::validate_arguments(&arguments)
        {
            return self
                .reject_call(pending, &format!("Webfetch failed: {error}"))
                .await;
        }
        #[cfg(feature = "runbooks")]
        if call.name == beale_runbooks::RUN {
            return self.execute_runbook(pending, &arguments).await;
        }
        #[cfg(feature = "findings")]
        if call.name == beale_findings::VERIFY {
            return self.execute_verification(pending, &arguments).await;
        }
        if call.name == "plugin_call" {
            let invocation:beale_contracts::swarm_plugins::Invocation=match serde_json::from_value::<beale_contracts::swarm_plugins::Invocation>(arguments.clone()) {
                Ok(invocation) if invocation.validate().is_ok()=>invocation,
                _=>return self.reject_call(pending,"Plugin call requires valid plugin, server, tool, and object arguments.").await,
            };
            let operation = self.call_operation(&call.id)?;
            let request = self.request(operation.clone(), WorkerOperation::Plugin { invocation });
            let reply: beale_contracts::swarm_plugins::Reply =
                serde_json::from_value(self.remote(request).await?)?;
            reply
                .validate_for(&self.launch.identity)
                .map_err(anyhow::Error::msg)?;
            let result = match reply.outcome {
                beale_contracts::swarm_plugins::Outcome::Completed {
                    output,
                    is_error,
                    artifact_refs,
                } => json!({"output":output,"is_error":is_error,"artifact_refs":artifact_refs}),
                beale_contracts::swarm_plugins::Outcome::OutcomeUnknown {} => {
                    anyhow::bail!("plugin outcome is unknown; parent reconciliation required")
                }
            };
            self.publish_tool_outcome(pending, &arguments, &result)
                .await?;
            return Ok(CallOutcome {
                result,
                operations: vec![operation],
            });
        }
        let local = matches!(
            call.name.as_str(),
            "read" | "write" | "edit" | "swarm_materialize_artifact"
        ) || call.name == beale_tools::COMMAND_TOOL_NAME
            || (cfg!(feature = "webfetch") && call.name == "webfetch");
        let operation = if local {
            call.id.clone()
        } else {
            self.call_operation(&call.id)?
        };
        let result = if local {
            let invocation = ToolInvocation {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: arguments.clone(),
            };
            match self.spool.claim(&WorkRequest::Environment {
                call: invocation.clone(),
            })? {
                Claim::Replay(value) => value,
                Claim::OutcomeUnknown | Claim::InFlight => {
                    anyhow::bail!("local tool outcome is unknown; parent reconciliation required")
                }
                Claim::RetryLeader => anyhow::bail!("invalid local tool journal state"),
                Claim::Dispatch => {
                    let approval = self
                        .remote(self.request(
                            pending.review_id.clone(),
                            WorkerOperation::Review { call: invocation },
                        ))
                        .await;
                    let result = match approval {
                        Ok(value)
                            if serde_json::from_value::<WorkerReviewResult>(value.clone())?
                                .approved =>
                        {
                            self.deadline()?;
                            ensure!(
                                arguments.get("machine").is_none(),
                                "assigned commands cannot select another machine"
                            );
                            let output = if call.name == "swarm_materialize_artifact" {
                                let result = self.materialize(call, &arguments).await?;
                                Ok(beale_tools::ToolOutcome {
                                    output: result.to_string(),
                                    is_error: false,
                                })
                            } else if call.name == beale_tools::COMMAND_TOOL_NAME {
                                let command = arguments["command"]
                                    .as_str()
                                    .context("command is required")?;
                                let timeout = arguments
                                    .get("timeout_seconds")
                                    .and_then(Value::as_u64)
                                    .unwrap_or(beale_tools::DEFAULT_COMMAND_TIMEOUT_SECONDS);
                                let output = beale_tools::run_worker_command(
                                    command,
                                    &self.launch.workspace,
                                    timeout,
                                )
                                .await?;
                                Ok(beale_tools::ToolOutcome {
                                    output: output.transcript(),
                                    is_error: !output.success,
                                })
                            } else {
                                self.execute_builtin(&call.name, &arguments).await
                            };
                            match output {
                                Ok(output) => {
                                    json!({"output":output.output,"is_error":output.is_error})
                                }
                                Err(_) => {
                                    json!({"output":"Assigned environment tool failed; inspect the retained workspace.","is_error":true})
                                }
                            }
                        }
                        Ok(value)
                            if !serde_json::from_value::<WorkerReviewResult>(value.clone())?
                                .approved =>
                        {
                            json!({"output":"The leader did not approve this tool call.","is_error":true})
                        }
                        Err(error) => return Err(error),
                        Ok(_) => anyhow::bail!("invalid tool approval response"),
                    };
                    let redactor = beale_tools::Redactor::new([self.launch.bearer.clone()]);
                    let result: Value =
                        serde_json::from_str(&redactor.redact(&serde_json::to_string(&result)?))?;
                    let result = self.bounded_outcome(call, result).await?;
                    self.spool.complete(&call.id, &result)?;
                    result
                }
            }
        } else {
            let approval = self
                .remote(self.request(
                    pending.review_id.clone(),
                    WorkerOperation::Review {
                        call: ToolInvocation {
                            id: call.id.clone(),
                            name: call.name.clone(),
                            arguments: arguments.clone(),
                        },
                    },
                ))
                .await?;
            if !serde_json::from_value::<WorkerReviewResult>(approval)?.approved {
                return self
                    .reject_call(pending, "The leader did not approve this tool call.")
                    .await;
            }
            self.attachments(call, &mut arguments).await?;
            self.remote(self.request(
                operation.clone(),
                WorkerOperation::Canonical {
                    tool: call.name.clone(),
                    arguments: arguments.clone(),
                },
            ))
            .await?
        };
        let result = self.publish_outcome(call, result).await?;
        self.publish_tool_outcome(pending, &arguments, &result)
            .await?;
        Ok(CallOutcome {
            result,
            operations: vec![operation],
        })
    }

    async fn execute_builtin(
        &self,
        name: &str,
        arguments: &Value,
    ) -> Result<beale_tools::ToolOutcome> {
        #[cfg(feature = "webfetch")]
        if name == beale_webfetch::TOOL_NAME {
            // Saved tools and leader review authorize this request. Fetch from
            // the assigned environment without opening host settings/storage.
            return Ok(match beale_webfetch::execute(arguments).await {
                Ok(output) => beale_tools::ToolOutcome {
                    output,
                    is_error: false,
                },
                Err(error) => beale_tools::ToolOutcome {
                    output: format!("Webfetch failed: {error}"),
                    is_error: true,
                },
            });
        }
        if self
            .launch
            .workspace
            .starts_with(&self.launch.protected_root)
        {
            beale_tools::execute_in_environment(
                name,
                arguments,
                &self.launch.workspace,
                &self.launch.protected_root,
            )
            .await
        } else {
            beale_tools::execute(
                name,
                arguments,
                &self.launch.workspace,
                &self.launch.protected_root,
            )
            .await
        }
    }
    async fn agent_loop(&self) -> Result<()> {
        self.spool.require_known_outcomes()?;
        let checkpoint = self.spool.latest_checkpoint()?;
        let base_ms = checkpoint
            .as_ref()
            .map(|saved| {
                saved.context["active_elapsed_ms"]
                    .as_u64()
                    .context("worker duration missing from checkpoint")
            })
            .transpose()?
            .unwrap_or(0);
        let clock = ActiveClock::new(base_ms)?;
        let mut state = match checkpoint {
            Some(checkpoint) => {
                let state: LoopState = serde_json::from_value(checkpoint.context)?;
                ensure!(state.version == 2, "unsupported worker checkpoint version");
                ensure!(
                    state.shell == WorkerShell::native(),
                    "retained shell does not match this worker runtime"
                );
                self.publish_checkpoint().await?;
                state
            }
            None => {
                let id = format!("bootstrap:{}", self.launch.identity.attempt_id);
                let bootstrap: WorkerBootstrap = serde_json::from_value(
                    self.remote(self.request(id.clone(), WorkerOperation::Bootstrap {}))
                        .await?,
                )?;
                bootstrap.validate().map_err(anyhow::Error::msg)?;
                ensure!(
                    bootstrap.shell == WorkerShell::native(),
                    "assigned shell does not match this worker runtime"
                );
                ensure!(
                    bootstrap.identity == self.launch.identity,
                    "worker bootstrap belongs to another execution"
                );
                self.prepare_plugin_resources(&bootstrap.enabled_tools)
                    .await?;
                let state = LoopState {
                    version: 2,
                    active_elapsed_ms: 0,
                    shell: bootstrap.shell,
                    route_id: bootstrap.route_id,
                    format: bootstrap.message_format,
                    enabled_tools: bootstrap.enabled_tools,
                    input: bootstrap.input,
                    provider_failures: 0,
                    phase: Phase::Infer {
                        operation_id: Uuid::new_v4().to_string(),
                    },
                };
                self.save(&state, &clock).await?;
                self.spool.acknowledge(&id)?;
                state
            }
        };
        loop {
            self.deadline()?;
            match state.phase.clone() {
                Phase::Infer { operation_id } => {
                    let result: InferenceResult = serde_json::from_value(
                        self.remote(self.request(
                            operation_id.clone(),
                            WorkerOperation::Infer {
                                route_id: state.route_id.clone(),
                                messages: json!(state.input),
                            },
                        ))
                        .await?,
                    )?;
                    result.validate().map_err(anyhow::Error::msg)?;
                    match result {
                        InferenceResult::Unavailable { retryable } => {
                            state.provider_failures += 1;
                            ensure!(
                                retryable && state.provider_failures < 3,
                                "assigned inference recovery exhausted"
                            );
                            state.phase = Phase::Infer {
                                operation_id: Uuid::new_v4().to_string(),
                            };
                        }
                        InferenceResult::Completed {
                            text,
                            calls,
                            continuation,
                            ..
                        } => {
                            state.provider_failures = 0;
                            if continuation.is_empty() && !text.is_empty() {
                                state.input.push(json!({"role":"assistant","content":text}));
                            } else {
                                state.input.extend(continuation);
                            }
                            state.phase = if calls.is_empty() {
                                Phase::Done {
                                    operation_id: Uuid::new_v4().to_string(),
                                    answer: text,
                                }
                            } else {
                                Phase::Tools {
                                    calls: calls
                                        .into_iter()
                                        .map(|call| PendingCall {
                                            call,
                                            review_id: Uuid::new_v4().to_string(),
                                            publish_id: Uuid::new_v4().to_string(),
                                        })
                                        .collect(),
                                }
                            };
                        }
                    }
                    self.save(&state, &clock).await?;
                    self.spool.acknowledge(&operation_id)?;
                }
                Phase::Tools { mut calls } => {
                    let Some(pending) = calls.first() else {
                        anyhow::bail!("worker checkpoint has an empty tool phase")
                    };
                    let outcome = self.execute_call(&state, pending).await?;
                    let output = outcome
                        .result
                        .get("output")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .unwrap_or_else(|| outcome.result.to_string());
                    state
                        .input
                        .push(state.format.tool_output(&pending.call.id, &output));
                    let mut acknowledged = outcome.operations;
                    acknowledged.push(pending.publish_id.clone());
                    calls.remove(0);
                    state.phase = if calls.is_empty() {
                        Phase::Infer {
                            operation_id: Uuid::new_v4().to_string(),
                        }
                    } else {
                        Phase::Tools { calls }
                    };
                    self.save(&state, &clock).await?;
                    for id in acknowledged {
                        self.spool.acknowledge(&id)?;
                    }
                }
                Phase::Done {
                    operation_id,
                    answer,
                } => {
                    let revision = self.publish_checkpoint().await?;
                    self.remote(self.request(
                        operation_id.clone(),
                        WorkerOperation::Finish { revision, answer },
                    ))
                    .await?;
                    self.spool.acknowledge(&operation_id)?;
                    return Ok(());
                }
            }
        }
    }
}

pub async fn run_assigned(mut launch: WorkerLaunch) -> Result<()> {
    run_with_faults(&mut launch, None).await
}

/// Exercise the real scoped upload and download path before Docker research.
/// A small candidate artifact records readiness; it is not a research finding.
pub(super) async fn probe_artifact_transfer(launch: &WorkerLaunch) -> Result<()> {
    let worker = Worker {
        transport: WorkerTransport::new(&launch.leader, &launch.bearer)?,
        spool: WorkerSpool::open(&launch.recovery, launch.identity.clone())?,
        lease: Mutex::new(Lease::new(launch.identity.clone())?),
        launch,
    };
    worker.renew().await?;
    let name = format!(".container-readiness-{}", launch.identity.attempt_id);
    let path = launch.workspace.join(&name);
    let bytes = b"Beale container artifact readiness v1\n";
    if !path.try_exists()? {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    ensure!(
        std::fs::symlink_metadata(&path)?.file_type().is_file() && std::fs::read(&path)? == bytes,
        "container readiness artifact changed"
    );
    let artifact_id = worker
        .publish_artifact(
            &format!("container-readiness:{}", launch.identity.attempt_id),
            &name,
        )
        .await?;
    let chunk: ArtifactChunkResult = serde_json::from_value(
        worker
            .remote(worker.request(
                format!("container-readiness-read:{}", launch.identity.attempt_id),
                WorkerOperation::ArtifactRead {
                    artifact_id,
                    offset: 0,
                    length: bytes.len() as u32,
                },
            ))
            .await?,
    )?;
    ensure!(
        chunk.bytes == bytes && chunk.sha256 == beale_storage::swarm_files::hash(bytes),
        "container artifact return failed verification"
    );
    std::fs::remove_file(path)?;
    Ok(())
}

pub(super) async fn run_with_faults(
    launch: &mut WorkerLaunch,
    faults: Option<std::sync::Arc<dyn beale_storage::worker_spool::SpoolFaults>>,
) -> Result<()> {
    ensure!(launch.version == 1, "unsupported worker launch version");
    launch.identity.validate().map_err(anyhow::Error::msg)?;
    launch.workspace = launch
        .workspace
        .canonicalize()
        .context("worker workspace unavailable")?;
    launch.protected_root = launch
        .protected_root
        .canonicalize()
        .context("worker private root unavailable")?;
    // Resolve the existing parent before creating anything. Recovery is one
    // supervisor-selected child, never a recursively created arbitrary path.
    let name = launch
        .recovery
        .file_name()
        .context("invalid worker recovery name")?;
    let parent = launch
        .recovery
        .parent()
        .context("missing worker recovery parent")?
        .canonicalize()?;
    let recovery = parent.join(name);
    ensure!(
        parent.starts_with(&launch.protected_root) && !recovery.starts_with(&launch.workspace),
        "invalid worker recovery boundary"
    );
    if !recovery.exists() {
        std::fs::create_dir(&recovery)?;
    }
    ensure!(
        !std::fs::symlink_metadata(&recovery)?
            .file_type()
            .is_symlink(),
        "worker recovery must not be a link"
    );
    launch.recovery = recovery.canonicalize()?;
    ensure!(
        launch.workspace.is_dir()
            && launch.protected_root.is_dir()
            && launch.recovery.starts_with(&launch.protected_root)
            && !launch.recovery.starts_with(&launch.workspace),
        "invalid worker storage boundaries"
    );
    let spool = WorkerSpool::open(&launch.recovery, launch.identity.clone())?;
    let spool = if let Some(faults) = faults {
        spool.with_faults(faults)
    } else {
        spool
    };
    let worker = Worker {
        transport: WorkerTransport::new(&launch.leader, &launch.bearer)?,
        spool,
        lease: Mutex::new(Lease::new(launch.identity.clone())?),
        launch,
    };
    for attempt in 0..3 {
        match worker.renew().await {
            Ok(()) => break,
            Err(error) => {
                if attempt == 2
                    || !error
                        .downcast_ref::<TransportError>()
                        .is_some_and(|e| e.kind == TransportFailure::Retryable)
                {
                    return Err(error);
                }
                tokio::time::sleep(Duration::from_millis(250) * (1 << attempt)).await;
            }
        }
    }
    let result = tokio::select! {
        result = worker.agent_loop() => result,
        failure = worker.keep_alive() => failure,
    };
    if result.is_err() {
        let id = Uuid::new_v4().to_string();
        let warning = worker.request(id.clone(), WorkerOperation::Event { event_id: id,
            payload: json!({"kind":"worker_failure","message":"Worker execution stopped; retained recovery state requires reconciliation."}) });
        // Error reporting must not delay process termination indefinitely. A
        // failed warning never changes the last durable execution/checkpoint.
        let _ = tokio::time::timeout(Duration::from_secs(3), worker.remote(warning)).await;
    }
    result
}

/// The executable calls this before initializing any host services. Unix workers
/// end their whole managed group even after an error; no background command is
/// left running merely because the agent loop or root process exited.
pub async fn worker_entry(launch: WorkerLaunch) -> Result<()> {
    beale_tools::process::require_worker_group()?;
    let result = run_assigned(launch).await;
    beale_tools::process::terminate_worker_group()?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_clock_resumes_only_durable_time_and_checks_its_storage_bound() {
        let start = Instant::now();
        let first = ActiveClock {
            base_ms: 0,
            started: start,
        };
        let retained = first
            .elapsed_at(start + Duration::from_millis(250))
            .unwrap();
        let resumed_start = start + Duration::from_secs(30 * 24 * 60 * 60);
        let resumed = ActiveClock {
            base_ms: retained,
            started: resumed_start,
        };
        assert_eq!(
            resumed
                .elapsed_at(resumed_start + Duration::from_millis(75))
                .unwrap(),
            325
        );
        assert!(resumed.elapsed_at(start).is_err());
        let boundary = ActiveClock {
            base_ms: MAX_ACTIVE_ELAPSED_MS - 1,
            started: start,
        };
        assert_eq!(
            boundary
                .elapsed_at(start + Duration::from_millis(1))
                .unwrap(),
            MAX_ACTIVE_ELAPSED_MS
        );
        assert!(
            boundary
                .elapsed_at(start + Duration::from_millis(2))
                .is_err()
        );
        assert!(ActiveClock::new(MAX_ACTIVE_ELAPSED_MS + 1).is_err());
    }

    #[test]
    fn native_argument_errors_are_detected_before_claiming_execution() {
        for (name, arguments) in [
            ("read", json!({"path":"example.txt","offset":0})),
            ("read", json!({"path":"example.txt","limit":2001})),
            ("write", json!({"path":"example.txt","content":false})),
            (
                "write",
                json!({"path":"example.txt","content":"example","machine":"other-example"}),
            ),
            (
                "edit",
                json!({"path":"example.txt","old_text":"","new_text":"example"}),
            ),
            (
                "swarm_materialize_artifact",
                json!({"artifact":"artifact-example","destination":"../outside"}),
            ),
            (beale_tools::COMMAND_TOOL_NAME, json!({"command":""})),
            (
                beale_tools::COMMAND_TOOL_NAME,
                json!({"command":"example","timeout_seconds":"10"}),
            ),
            (
                beale_tools::COMMAND_TOOL_NAME,
                json!({"command":"example","timeout_seconds":601}),
            ),
            (
                beale_tools::COMMAND_TOOL_NAME,
                json!({"command":"example","machine":"other-example"}),
            ),
        ] {
            assert!(
                invalid_native_arguments(name, &arguments).is_some(),
                "{name}: {arguments}"
            );
        }
        for (name, arguments) in [
            ("read", json!({"path":"example.txt"})),
            ("write", json!({"path":"example.txt","content":""})),
            (
                "edit",
                json!({"path":"example.txt","old_text":"example","new_text":""}),
            ),
            (
                "swarm_materialize_artifact",
                json!({"artifact":"artifact-example","destination":"received/example.txt"}),
            ),
            (
                beale_tools::COMMAND_TOOL_NAME,
                json!({"command":"example","timeout_seconds":600}),
            ),
        ] {
            assert!(
                invalid_native_arguments(name, &arguments).is_none(),
                "{name}: {arguments}"
            );
        }
    }
}
