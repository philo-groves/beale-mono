//! Swarms JSON contracts. Secrets and host storage stay outside these documents.
//! Secrets travel outside these documents; host paths and database types never do.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

pub const VERSION: u32 = 1;
pub const MAX_ID_BYTES: usize = 128;
pub const WORKER_AUDIENCE: &str = "beale.swarm-worker.v1";
pub const MAX_DOCUMENT_BYTES: usize = 1_048_576;

pub type ValidationResult = Result<(), &'static str>;

pub fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID_BYTES
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._:-".contains(&c))
}

fn require(condition: bool, reason: &'static str) -> ValidationResult {
    if condition { Ok(()) } else { Err(reason) }
}

fn ids(values: &[String]) -> bool {
    values.len() <= 256
        && values.iter().all(|v| valid_id(v))
        && values.iter().collect::<HashSet<_>>().len() == values.len()
}

pub fn bounded_json(value: &Value) -> bool {
    serde_json::to_vec(value).is_ok_and(|v| v.len() <= MAX_DOCUMENT_BYTES)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Leader,
    Deputy,
    Worker,
}

impl Role {
    pub fn child(self) -> Option<Self> {
        match self {
            Self::Leader => Some(Self::Deputy),
            Self::Deputy => Some(Self::Worker),
            Self::Worker => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileRef {
    pub id: String,
    pub revision: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileSnapshot {
    pub profile: ProfileRef,
    pub role: Role,
    pub instruction: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewApplicability {
    Leader,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewRuleSnapshot {
    pub id: String,
    pub revision: u32,
    pub applicability: ReviewApplicability,
    pub instruction: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefinitionSnapshot {
    pub id: String,
    pub revision: u32,
    /// Exact declarative workflow, interpreted by `swarm_workflows`.
    pub definition: Value,
    pub profiles: Vec<ProfileSnapshot>,
    pub leader_rule: ReviewRuleSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScanMode {
    Local {},
    Swarm { snapshot: Box<DefinitionSnapshot> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Destination {
    pub host_id: String,
    pub configuration_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderRoute {
    pub id: String,
    pub provider_id: String,
    pub model: String,
    pub budget_group_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "unit", rename_all = "snake_case", deny_unknown_fields)]
pub enum PacingAmount {
    Subscription {
        window_id: String,
        basis_points: u32,
    },
    Currency {
        currency: String,
        micros: u64,
    },
    Credits {
        micros: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PacingTarget {
    pub budget_group_id: String,
    pub interval_seconds: u32,
    pub amount: PacingAmount,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionPolicy {
    pub ended_scan_seconds: u64,
    pub storage_reserve_bytes: u64,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            ended_scan_seconds: 7 * 86_400,
            storage_reserve_bytes: 5 * 1024 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanConfig {
    pub version: u32,
    pub scan_id: String,
    pub workspace_id: String,
    pub mode: ScanMode,
    pub authorized_scope: String,
    pub enabled_tools: Vec<String>,
    pub provider_routes: Vec<ProviderRoute>,
    pub destinations: Vec<Destination>,
    pub pacing: Vec<PacingTarget>,
    pub retention: RetentionPolicy,
}

impl ScanConfig {
    pub fn decode_json(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_DOCUMENT_BYTES {
            return Err("scan config too large".into());
        }
        let config: Self = serde_json::from_slice(bytes).map_err(|_| "invalid scan config JSON")?;
        config.validate().map_err(str::to_owned)?;
        Ok(config)
    }

    pub fn validate(&self) -> ValidationResult {
        require(
            self.version == VERSION,
            "unsupported swarm contract version",
        )?;
        require(
            valid_id(&self.scan_id) && valid_id(&self.workspace_id),
            "invalid scan identity",
        )?;
        require(
            !self.authorized_scope.trim().is_empty() && self.authorized_scope.len() <= 32_768,
            "invalid authorized scope",
        )?;
        require(ids(&self.enabled_tools), "invalid enabled tools")?;
        require(
            self.destinations.len() <= 128
                && self
                    .destinations
                    .iter()
                    .all(|v| valid_id(&v.host_id) && valid_id(&v.configuration_id)),
            "invalid destinations",
        )?;
        require(
            self.destinations
                .iter()
                .map(|d| (&d.host_id, &d.configuration_id))
                .collect::<HashSet<_>>()
                .len()
                == self.destinations.len(),
            "duplicate destination",
        )?;
        require(
            self.provider_routes.len() <= 64
                && self.provider_routes.iter().all(|v| {
                    valid_id(&v.id)
                        && valid_id(&v.provider_id)
                        && valid_id(&v.budget_group_id)
                        && !v.model.trim().is_empty()
                        && v.model.len() <= 256
                }),
            "invalid provider routes",
        )?;
        require(
            self.provider_routes
                .iter()
                .map(|v| &v.id)
                .collect::<HashSet<_>>()
                .len()
                == self.provider_routes.len(),
            "duplicate provider route",
        )?;
        require(
            self.retention.ended_scan_seconds <= 365 * 86_400,
            "invalid retention period",
        )?;
        require(self.pacing.len() <= 64, "too many pacing targets")?;
        let mut groups = HashSet::new();
        for target in &self.pacing {
            require(
                valid_id(&target.budget_group_id)
                    && groups.insert(&target.budget_group_id)
                    && (1..=604_800).contains(&target.interval_seconds),
                "invalid pacing target",
            )?;
            require(
                self.provider_routes
                    .iter()
                    .any(|r| r.budget_group_id == target.budget_group_id),
                "unknown pacing account",
            )?;
            require(
                match &target.amount {
                    PacingAmount::Subscription {
                        window_id,
                        basis_points,
                    } => valid_id(window_id) && (1..=10_000).contains(basis_points),
                    PacingAmount::Currency { currency, micros } => {
                        currency.len() == 3
                            && currency.bytes().all(|c| c.is_ascii_uppercase())
                            && *micros > 0
                    }
                    PacingAmount::Credits { micros } => *micros > 0,
                },
                "invalid pacing amount",
            )?;
        }
        match &self.mode {
            ScanMode::Local {} => require(
                self.destinations.is_empty(),
                "local scans cannot select Fleet destinations",
            )?,
            ScanMode::Swarm { snapshot } => {
                snapshot.workflow()?;
                require(
                    valid_id(&snapshot.id)
                        && snapshot.revision > 0
                        && snapshot.definition.is_object()
                        && bounded_json(&snapshot.definition),
                    "invalid definition snapshot",
                )?;
                require(
                    !snapshot.profiles.is_empty() && snapshot.profiles.len() <= 128,
                    "invalid profiles",
                )?;
                let mut names = HashSet::new();
                for p in &snapshot.profiles {
                    require(
                        valid_id(&p.profile.id)
                            && p.profile.revision > 0
                            && names.insert(&p.profile.id)
                            && !p.instruction.trim().is_empty()
                            && p.instruction.len() <= 32_768,
                        "invalid profile snapshot",
                    )?;
                }
                require(
                    snapshot
                        .profiles
                        .iter()
                        .filter(|p| p.role == Role::Leader)
                        .count()
                        == 1
                        && snapshot.profiles.iter().any(|p| p.role == Role::Deputy)
                        && snapshot.profiles.iter().any(|p| p.role == Role::Worker),
                    "swarm requires leader, deputy, and worker profiles",
                )?;
                let rule = &snapshot.leader_rule;
                require(
                    valid_id(&rule.id)
                        && rule.revision > 0
                        && !rule.instruction.trim().is_empty()
                        && rule.instruction.len() <= 16_384,
                    "invalid leader rule",
                )?;
            }
        }
        require(
            serde_json::to_vec(self).is_ok_and(|v| v.len() <= MAX_DOCUMENT_BYTES),
            "scan config too large",
        )
    }

    pub fn validate_creation(&self, enabled: bool, supported: bool) -> ValidationResult {
        self.validate()?;
        require(
            matches!(self.mode, ScanMode::Local {}) || (enabled && supported),
            "Swarms unavailable for new scans",
        )
    }

    pub fn validate_resume(&self, supported: bool) -> ValidationResult {
        self.validate()?;
        require(
            matches!(self.mode, ScanMode::Local {}) || supported,
            "saved swarm is unsupported by this build",
        )
    }

    pub fn profile(&self, profile: &ProfileRef) -> Option<&ProfileSnapshot> {
        match &self.mode {
            ScanMode::Local {} => None,
            ScanMode::Swarm { snapshot } => {
                snapshot.profiles.iter().find(|p| &p.profile == profile)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Queued,
    Running,
    Waiting,
    Suspended,
    Completed,
    Blocked,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSummary {
    pub id: String,
    pub parent_id: Option<String>,
    pub profile: ProfileRef,
    pub role: Role,
    pub state: AgentState,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCheckpoint {
    pub state: AgentState,
    pub context: Value,
    pub inbox_cursor: u64,
    pub wake_reasons: Vec<String>,
    pub pending_assignments: Vec<String>,
    pub environment_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub question: String,
    pub completion_criteria: Vec<String>,
    pub evidence_refs: Vec<String>,
}

impl Task {
    pub fn validate(&self) -> ValidationResult {
        require(
            !self.question.trim().is_empty()
                && self.question.len() <= 16_384
                && !self.completion_criteria.is_empty()
                && self.completion_criteria.len() <= 32
                && self
                    .completion_criteria
                    .iter()
                    .all(|v| !v.trim().is_empty() && v.len() <= 4096)
                && ids(&self.evidence_refs),
            "invalid task",
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentState {
    Queued,
    Assigned,
    Completed,
    Blocked,
    Canceled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assignment {
    pub id: String,
    pub parent_id: String,
    pub child_id: String,
    pub task: Task,
    pub state: AssignmentState,
    pub wait_reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum SpawnResult {
    Queued {
        child_id: String,
        assignment_id: String,
        wait_reason: String,
    },
    Starting {
        child_id: String,
        assignment_id: String,
        attempt_id: String,
    },
    Running {
        child_id: String,
        assignment_id: String,
        attempt_id: String,
    },
    Denied {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordRevision {
    pub id: String,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerCapability {
    CanonicalRouting,
    ArtifactTransfer,
    DurableEvents,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerRegistration {
    pub version: u32,
    pub audience: String,
    pub capabilities: Vec<WorkerCapability>,
}

impl WorkerRegistration {
    pub fn validate(&self) -> ValidationResult {
        require(
            self.version == VERSION && self.audience == WORKER_AUDIENCE,
            "incompatible worker protocol",
        )?;
        require(
            self.capabilities.len() == 3
                && self.capabilities.iter().collect::<HashSet<_>>().len() == 3,
            "missing or duplicate worker capabilities",
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerOperation {
    SnapshotRead {
        request: crate::swarm_files::SnapshotRead,
    },
    Bootstrap {},
    Review {
        call: crate::swarm_worker::ToolInvocation,
    },
    Checkpoint {
        revision: u64,
        active_elapsed_ms: u64,
        context: Value,
    },
    Finish {
        revision: u64,
        answer: String,
    },
    Infer {
        route_id: String,
        messages: Value,
    },
    Canonical {
        tool: String,
        arguments: Value,
    },
    PluginResources {
        request: crate::swarm_plugins::ResourceRequest,
    },
    Plugin {
        invocation: crate::swarm_plugins::Invocation,
    },
    Verify {
        request: crate::swarm_findings::Request,
    },
    Runbook {
        request: crate::swarm_runbooks::Request,
    },
    MessageParent {
        message: String,
    },
    MessageChild {
        child_id: String,
        message: String,
    },
    StopChild {
        child_id: String,
    },
    Spawn {
        profile: ProfileRef,
        task: Task,
    },
    Heartbeat {},
    Event {
        event_id: String,
        payload: Value,
    },
    ArtifactChunk {
        transfer_id: String,
        offset: u64,
        bytes: Vec<u8>,
        sha256: String,
    },
    ArtifactBegin {
        artifact: crate::swarm_files::ArtifactUpload,
    },
    ArtifactSeal {
        transfer_id: String,
    },
    ArtifactRead {
        artifact_id: String,
        offset: u64,
        length: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerRequest {
    pub version: u32,
    pub workspace_id: String,
    pub scan_id: String,
    pub agent_id: String,
    pub attempt_id: String,
    pub generation: u64,
    pub operation_id: String,
    pub operation: WorkerOperation,
}

impl WorkerRequest {
    pub fn decode_json(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_DOCUMENT_BYTES {
            return Err("worker request too large".into());
        }
        let request: Self =
            serde_json::from_slice(bytes).map_err(|_| "invalid worker request JSON")?;
        request.validate().map_err(str::to_owned)?;
        Ok(request)
    }

    pub fn validate(&self) -> ValidationResult {
        require(
            self.version == VERSION,
            "unsupported worker request version",
        )?;
        require(
            [
                &self.workspace_id,
                &self.scan_id,
                &self.agent_id,
                &self.attempt_id,
                &self.operation_id,
            ]
            .iter()
            .all(|v| valid_id(v))
                && self.generation > 0,
            "invalid worker request identity",
        )?;
        require(
            serde_json::to_vec(self).is_ok_and(|v| v.len() <= MAX_DOCUMENT_BYTES),
            "worker request too large",
        )?;
        match &self.operation {
            WorkerOperation::SnapshotRead { request } => request.validate(),
            WorkerOperation::Bootstrap {} => Ok(()),
            WorkerOperation::Review { call } => require(
                valid_id(&call.id) && valid_id(&call.name) && call.arguments.is_object(),
                "invalid local tool review",
            ),
            WorkerOperation::Checkpoint {
                revision,
                active_elapsed_ms,
                context,
            } => require(
                *revision > 0
                    && *active_elapsed_ms <= crate::swarm_worker::MAX_ACTIVE_ELAPSED_MS
                    && context.is_object(),
                "invalid worker checkpoint",
            ),
            WorkerOperation::Finish { revision, answer } => require(
                *revision > 0 && answer.len() <= 262_144,
                "invalid worker finish",
            ),
            WorkerOperation::Infer { route_id, messages } => require(
                valid_id(route_id) && messages.is_array(),
                "invalid inference request",
            ),
            WorkerOperation::Canonical { tool, arguments } => require(
                valid_id(tool) && arguments.is_object(),
                "invalid canonical request",
            ),
            WorkerOperation::Runbook { request } => request.validate(),
            WorkerOperation::Verify { request } => request.validate(),
            WorkerOperation::Plugin { invocation } => invocation.validate(),
            WorkerOperation::PluginResources { request } => request.validate(),
            WorkerOperation::MessageParent { message } => require(
                !message.trim().is_empty() && message.len() <= 32_768,
                "invalid parent message",
            ),
            WorkerOperation::MessageChild { child_id, message } => require(
                valid_id(child_id) && !message.trim().is_empty() && message.len() <= 32_768,
                "invalid child message",
            ),
            WorkerOperation::StopChild { child_id } => {
                require(valid_id(child_id), "invalid child identity")
            }
            WorkerOperation::Spawn { profile, task } => {
                require(
                    valid_id(&profile.id) && profile.revision > 0,
                    "invalid profile",
                )?;
                task.validate()
            }
            WorkerOperation::Heartbeat {} => Ok(()),
            WorkerOperation::Event { event_id, .. } => {
                require(valid_id(event_id), "invalid event identity")
            }
            WorkerOperation::ArtifactChunk {
                transfer_id,
                offset,
                bytes,
                sha256,
            } => require(
                valid_id(transfer_id)
                    && !bytes.is_empty()
                    && bytes.len() <= 192 * 1024
                    && offset.checked_add(bytes.len() as u64).is_some()
                    && sha256.len() == 64
                    && sha256.bytes().all(|c| c.is_ascii_hexdigit()),
                "invalid artifact chunk",
            ),
            WorkerOperation::ArtifactBegin { artifact } => require(
                valid_id(&artifact.id)
                    && artifact.previous.as_ref().is_none_or(|id| valid_id(id))
                    && artifact.revision > 0
                    && crate::swarm_files::relative_path(&artifact.source_path)
                    && crate::swarm_files::digest(&artifact.sha256)
                    && artifact.size <= i64::MAX as u64,
                "invalid artifact metadata",
            ),
            WorkerOperation::ArtifactSeal { transfer_id } => {
                require(valid_id(transfer_id), "invalid transfer identity")
            }
            WorkerOperation::ArtifactRead {
                artifact_id,
                offset,
                length,
            } => require(
                valid_id(artifact_id)
                    && (1..=192 * 1024).contains(length)
                    && offset.checked_add(u64::from(*length)).is_some(),
                "invalid artifact read",
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionState {
    Running,
    Revoked,
    TerminationUnconfirmed,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionAttempt {
    pub id: String,
    pub agent_id: String,
    pub generation: u64,
    pub expires_at_ms: i64,
    pub state: ExecutionState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolOperationState {
    NotStarted,
    Running,
    Completed,
    Canceled,
    OutcomeUnknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolOperation {
    pub id: String,
    pub agent_id: String,
    pub attempt_id: String,
    pub request: Value,
    pub state: ToolOperationState,
    /// Completion is indicated by `state`, so a completed JSON null is preserved.
    /// Non-completed operations carry null rather than a result.
    pub result: Value,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleEvent {
    pub id: String,
    pub agent_id: String,
    pub sequence: u64,
    pub kind: String,
    pub payload: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeletionState {
    Retained,
    Claimed,
    Deleted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentState {
    pub id: String,
    pub agent_id: String,
    pub generation: u64,
    pub retention_deadline_ms: Option<i64>,
    pub keep: bool,
    pub active_references: u32,
    pub pending_publications: Vec<String>,
    pub termination_confirmed: bool,
    pub deletion: DeletionState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainerConfiguration {
    pub id: String,
    pub host_id: String,
    pub image: String,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceState {
    Starting,
    Running,
    Stopped,
    TerminationUnconfirmed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionInstance {
    pub id: String,
    pub configuration_id: String,
    pub host_id: String,
    pub agent_id: String,
    pub attempt_id: String,
    pub state: InstanceState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MutationAcknowledgement {
    pub version: u32,
    pub operation_id: String,
    pub result: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceReservation {
    pub id: String,
    pub host_id: String,
    pub agent_id: String,
    pub memory_bytes: u64,
    pub storage_bytes: u64,
    pub expires_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetGroup {
    pub id: String,
    pub provider_id: String,
    pub owner_host_id: String,
    pub owner_generation: u64,
    pub participants: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageEvent {
    pub id: String,
    pub group_id: String,
    pub participant_id: String,
    pub attempt_id: String,
    pub measured_micros: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PacingReservation {
    pub id: String,
    pub group_id: String,
    pub owner_generation: u64,
    pub participant_id: String,
    pub estimated_micros: u64,
}
