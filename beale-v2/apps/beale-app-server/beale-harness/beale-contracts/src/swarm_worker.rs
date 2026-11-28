//! Worker execution identities and journal requests. No provider credentials or host paths.
/// Largest elapsed duration representable by canonical SQLite storage.
pub const MAX_ACTIVE_ELAPSED_MS: u64 = i64::MAX as u64;

use crate::swarms::{
    MAX_DOCUMENT_BYTES, MutationAcknowledgement, VERSION, WorkerOperation, WorkerRequest, valid_id,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_LEASE_MILLIS: u64 = 15 * 60 * 1000;
pub const WORKER_PATH: &str = "/v1/swarm-worker/dispatch";

/// A completed policy decision. Transport/authentication failures are not denials.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerReviewResult {
    pub approved: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferProgress {
    pub next_offset: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactChunkResult {
    pub artifact: crate::swarm_files::Artifact,
    pub offset: u64,
    pub bytes: Vec<u8>,
    pub sha256: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerShell {
    Bash,
    Powershell,
}

impl WorkerShell {
    pub const fn native() -> Self {
        if cfg!(windows) {
            Self::Powershell
        } else {
            Self::Bash
        }
    }
    pub const fn tool(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Powershell => "powershell",
        }
    }
    /// The scan saves command availability; the assigned environment chooses
    /// its implementation. This never enables a command when neither was saved.
    pub fn map_tools(self, saved: &[String]) -> Vec<String> {
        let mut tools = Vec::new();
        for name in saved {
            let name = if matches!(name.as_str(), "bash" | "powershell") {
                self.tool()
            } else {
                name.as_str()
            };
            if !tools.iter().any(|old| old == name) {
                tools.push(name.to_owned());
            }
        }
        tools
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageFormat {
    Responses,
    Chat,
    Anthropic,
}

impl MessageFormat {
    pub fn tool_output(self, id: &str, output: &str) -> Value {
        match self {
            Self::Responses => {
                serde_json::json!({"type":"function_call_output","call_id":id,"output":output})
            }
            Self::Chat => serde_json::json!({"role":"tool","tool_call_id":id,"content":output}),
            Self::Anthropic => {
                serde_json::json!({"role":"user","content":[{"type":"tool_result","tool_use_id":id,"content":output}]})
            }
        }
    }
}

/// Assigned conversation data only. Launch paths and scoped credentials travel
/// in the supervisor's private stdin configuration, never in this response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerBootstrap {
    pub identity: ExecutionIdentity,
    pub shell: WorkerShell,
    pub route_id: String,
    pub message_format: MessageFormat,
    pub enabled_tools: Vec<String>,
    pub input: Vec<Value>,
}

impl WorkerBootstrap {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.identity.validate()?;
        let mut names = std::collections::HashSet::new();
        if !valid_id(&self.route_id)
            || self.enabled_tools.len() > 256
            || self.enabled_tools.iter().any(|name| {
                !valid_id(name)
                    || !names.insert(name)
                    || (matches!(name.as_str(), "bash" | "powershell") && name != self.shell.tool())
            })
            || self.input.iter().any(|item| {
                !item.is_object() || matches!(item["role"].as_str(), Some("system" | "developer"))
            })
            || !serde_json::to_vec(self).is_ok_and(|bytes| bytes.len() <= MAX_DOCUMENT_BYTES / 2)
        {
            return Err("invalid worker bootstrap");
        }
        Ok(())
    }
}

/// The worker receives provider output, never provider endpoints or credentials.
/// Continuation items retain the provider's opaque conversation format.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum InferenceResult {
    Completed {
        text: String,
        calls: Vec<InferenceCall>,
        reasoning_summaries: Vec<String>,
        continuation: Vec<Value>,
        usage: Option<InferenceUsage>,
        response_id: Option<String>,
    },
    Unavailable {
        retryable: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InferenceCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InferenceUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: Option<u64>,
}

impl InferenceResult {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !serde_json::to_vec(self).is_ok_and(|bytes| bytes.len() <= MAX_DOCUMENT_BYTES / 2) {
            return Err("inference result too large");
        }
        if let Self::Completed {
            calls,
            usage,
            response_id,
            ..
        } = self
        {
            let mut ids = std::collections::HashSet::new();
            if calls.len() > 256
                || calls.iter().any(|call| {
                    !valid_id(&call.id) || !valid_id(&call.name) || !ids.insert(&call.id)
                })
            {
                return Err("invalid inference tool call identity");
            }
            if response_id.as_ref().is_some_and(|id| !valid_id(id)) {
                return Err("invalid inference response identity");
            }
            if usage.as_ref().is_some_and(|usage| {
                usage.input_tokens > 1_000_000_000_000
                    || usage.output_tokens > 1_000_000_000_000
                    || usage
                        .cache_read_tokens
                        .is_some_and(|cache| cache > usage.input_tokens)
            }) {
                return Err("invalid inference usage");
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerReply {
    Acknowledged {
        acknowledgement: MutationAcknowledgement,
    },
    Lease {
        grant: LeaseGrant,
    },
}

impl WorkerReply {
    pub fn validate_for(&self, request: &WorkerRequest) -> Result<(), &'static str> {
        if !serde_json::to_vec(self).is_ok_and(|bytes| bytes.len() <= MAX_DOCUMENT_BYTES) {
            return Err("worker reply too large");
        }
        match (self, &request.operation) {
            (Self::Lease { grant }, WorkerOperation::Heartbeat {}) => {
                grant.validate()?;
                if !grant.identity.matches(request) || grant.request_id != request.operation_id {
                    return Err("lease reply identity mismatch");
                }
            }
            (Self::Acknowledged { acknowledgement }, operation)
                if !matches!(operation, WorkerOperation::Heartbeat {}) =>
            {
                if acknowledgement.version != VERSION
                    || acknowledgement.operation_id != request.operation_id
                {
                    return Err("worker acknowledgement identity mismatch");
                }
                if matches!(operation, WorkerOperation::Infer { .. }) {
                    serde_json::from_value::<InferenceResult>(acknowledgement.result.clone())
                        .map_err(|_| "invalid inference acknowledgement")?
                        .validate()?;
                }
                if matches!(operation, WorkerOperation::Bootstrap {}) {
                    let bootstrap: WorkerBootstrap =
                        serde_json::from_value(acknowledgement.result.clone())
                            .map_err(|_| "invalid worker bootstrap reply")?;
                    bootstrap.validate()?;
                    if !bootstrap.identity.matches(request) {
                        return Err("worker bootstrap identity mismatch");
                    }
                }
                match operation {
                    WorkerOperation::SnapshotRead { request } => {
                        let chunk: crate::swarm_files::SnapshotChunk =
                            serde_json::from_value(acknowledgement.result.clone())
                                .map_err(|_| "invalid snapshot acknowledgement")?;
                        chunk.validate_for(request)?;
                    }
                    // A canonical tool or an artifact read can be acknowledged
                    // with a typed rejection of model-supplied input.
                    WorkerOperation::Canonical { .. } | WorkerOperation::ArtifactRead { .. }
                        if acknowledgement.result.get("error").is_some() =>
                    {
                        let error: crate::tool_error::ToolError =
                            serde_json::from_value(acknowledgement.result["error"].clone())
                                .map_err(|_| "invalid canonical tool error")?;
                        error.validate()?;
                        if acknowledgement.result["is_error"] != true
                            || acknowledgement.result["output"].as_str() != Some(&error.message)
                        {
                            return Err("canonical error feedback mismatch");
                        }
                    }
                    WorkerOperation::Runbook { request: runbook } => {
                        let reply: crate::swarm_runbooks::Reply =
                            serde_json::from_value(acknowledgement.result.clone())
                                .map_err(|_| "invalid Runbook acknowledgement")?;
                        reply.validate_for(
                            &ExecutionIdentity {
                                workspace_id: request.workspace_id.clone(),
                                scan_id: request.scan_id.clone(),
                                agent_id: request.agent_id.clone(),
                                attempt_id: request.attempt_id.clone(),
                                generation: request.generation,
                            },
                            runbook,
                        )?;
                    }
                    WorkerOperation::Verify {
                        request: verification,
                    } => {
                        let reply: crate::swarm_findings::Reply =
                            serde_json::from_value(acknowledgement.result.clone())
                                .map_err(|_| "invalid verification acknowledgement")?;
                        reply.validate_for(
                            &ExecutionIdentity {
                                workspace_id: request.workspace_id.clone(),
                                scan_id: request.scan_id.clone(),
                                agent_id: request.agent_id.clone(),
                                attempt_id: request.attempt_id.clone(),
                                generation: request.generation,
                            },
                            verification,
                        )?;
                    }
                    WorkerOperation::PluginResources { request: resource } => {
                        let reply: crate::swarm_plugins::ResourceReply =
                            serde_json::from_value(acknowledgement.result.clone())
                                .map_err(|_| "invalid plugin resource acknowledgement")?;
                        reply.validate_for(
                            &ExecutionIdentity {
                                workspace_id: request.workspace_id.clone(),
                                scan_id: request.scan_id.clone(),
                                agent_id: request.agent_id.clone(),
                                attempt_id: request.attempt_id.clone(),
                                generation: request.generation,
                            },
                            resource,
                        )?;
                    }
                    WorkerOperation::Plugin { .. } => {
                        let reply: crate::swarm_plugins::Reply =
                            serde_json::from_value(acknowledgement.result.clone())
                                .map_err(|_| "invalid plugin acknowledgement")?;
                        reply.validate_for(&ExecutionIdentity {
                            workspace_id: request.workspace_id.clone(),
                            scan_id: request.scan_id.clone(),
                            agent_id: request.agent_id.clone(),
                            attempt_id: request.attempt_id.clone(),
                            generation: request.generation,
                        })?;
                    }
                    WorkerOperation::Review { .. } => {
                        serde_json::from_value::<WorkerReviewResult>(
                            acknowledgement.result.clone(),
                        )
                        .map_err(|_| "invalid worker review decision")?;
                    }
                    WorkerOperation::ArtifactBegin { artifact } => {
                        let progress: TransferProgress =
                            serde_json::from_value(acknowledgement.result.clone())
                                .map_err(|_| "invalid transfer progress")?;
                        if progress.next_offset > artifact.size {
                            return Err("invalid transfer offset");
                        }
                    }
                    WorkerOperation::ArtifactChunk { .. } => {
                        let progress: TransferProgress =
                            serde_json::from_value(acknowledgement.result.clone())
                                .map_err(|_| "invalid transfer progress")?;
                        if progress.next_offset > i64::MAX as u64 {
                            return Err("invalid transfer offset");
                        }
                    }
                    WorkerOperation::ArtifactSeal { transfer_id } => {
                        let artifact: crate::swarm_files::Artifact =
                            serde_json::from_value(acknowledgement.result.clone())
                                .map_err(|_| "invalid accepted artifact")?;
                        artifact.validate()?;
                        if artifact.id != *transfer_id
                            || artifact.workspace != request.workspace_id
                            || artifact.scan != request.scan_id
                            || artifact.producer != request.agent_id
                            || artifact.attempt != request.attempt_id
                            || artifact.generation != request.generation
                            || !crate::swarm_files::digest(&artifact.sha256)
                        {
                            return Err("accepted artifact identity mismatch");
                        }
                    }
                    WorkerOperation::ArtifactRead {
                        artifact_id,
                        offset,
                        length,
                    } => {
                        let chunk: ArtifactChunkResult =
                            serde_json::from_value(acknowledgement.result.clone())
                                .map_err(|_| "invalid artifact chunk reply")?;
                        chunk.artifact.validate()?;
                        if chunk.artifact.id != *artifact_id
                            || chunk.artifact.workspace != request.workspace_id
                            || chunk.offset != *offset
                            || *offset > chunk.artifact.size
                            || chunk.bytes.len() as u64
                                != (chunk.artifact.size - offset).min(u64::from(*length))
                            || !crate::swarm_files::digest(&chunk.sha256)
                            || !crate::swarm_files::digest(&chunk.artifact.sha256)
                        {
                            return Err("artifact chunk reply mismatch");
                        }
                    }
                    _ => (),
                }
            }
            _ => return Err("unexpected worker reply kind"),
        }
        Ok(())
    }
}

/// The remaining grant is measured from the worker's request send time, not
/// from its response arrival time. No synchronized wall clocks are required.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseGrant {
    pub identity: ExecutionIdentity,
    pub request_id: String,
    pub valid_for_ms: u64,
}

impl LeaseGrant {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.identity.validate()?;
        if !valid_id(&self.request_id) || !(1..=MAX_LEASE_MILLIS).contains(&self.valid_for_ms) {
            return Err("invalid worker lease grant");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionIdentity {
    pub workspace_id: String,
    pub scan_id: String,
    pub agent_id: String,
    pub attempt_id: String,
    pub generation: u64,
}

impl ExecutionIdentity {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.generation == 0
            || [
                &self.workspace_id,
                &self.scan_id,
                &self.agent_id,
                &self.attempt_id,
            ]
            .iter()
            .any(|id| !valid_id(id))
        {
            return Err("invalid execution identity");
        }
        Ok(())
    }

    pub fn matches(&self, request: &WorkerRequest) -> bool {
        self.workspace_id == request.workspace_id
            && self.scan_id == request.scan_id
            && self.agent_id == request.agent_id
            && self.attempt_id == request.attempt_id
            && self.generation == request.generation
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolInvocation {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// Remote requests can be retransmitted only with their original identity.
/// A local operation with an uncertain outcome must never be automatically rerun.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "location", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkRequest {
    Leader { request: WorkerRequest },
    Environment { call: ToolInvocation },
}

impl WorkRequest {
    pub fn id(&self) -> &str {
        match self {
            Self::Leader { request } => &request.operation_id,
            Self::Environment { call } => &call.id,
        }
    }

    pub fn validate(&self, identity: &ExecutionIdentity) -> Result<(), &'static str> {
        identity.validate()?;
        if !serde_json::to_vec(self).is_ok_and(|bytes| bytes.len() <= MAX_DOCUMENT_BYTES) {
            return Err("worker journal request too large");
        }
        match self {
            Self::Leader { request } => {
                request.validate()?;
                if !identity.matches(request) {
                    return Err("worker journal identity mismatch");
                }
            }
            Self::Environment { call } => {
                if !valid_id(&call.id) || !valid_id(&call.name) || !call.arguments.is_object() {
                    return Err("invalid environment tool invocation");
                }
            }
        }
        Ok(())
    }
}
