//! Owning-host resource admission. Measurements never authorize execution.
use crate::swarms::valid_id;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const PATH: &str = "/v1/fleet/allocations";
pub const VERSION: u32 = 1;
pub const RAM_FLOOR: u64 = 1_000_000_000;
pub const STORAGE_RESERVE: u64 = 5 * 1024 * 1024 * 1024;
pub const DEFAULT_MEMORY: u64 = 512 * 1024 * 1024;
pub const DEFAULT_STORAGE: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageDemand {
    /// Stable physical filesystem identity, shared by all paths on that volume.
    pub pool: String,
    /// Immutable shared allocations (image layers, snapshots) use the same key.
    pub key: String,
    pub bytes: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capacity {
    pub domain: String,
    pub observed_at_ms: i64,
    /// Effective available memory after applicable execution-domain limits.
    pub available_memory: u64,
    pub storage: BTreeMap<String, u64>,
    /// Only usage already reflected in available_memory belongs here.
    pub measured_memory: BTreeMap<String, u64>,
    /// Allocation keys already reflected in free storage, including caches.
    pub measured_storage: BTreeSet<String>,
}
impl Capacity {
    pub fn validate(&self, now: i64) -> Result<(), &'static str> {
        if !valid_id(&self.domain)
            || self.observed_at_ms < 0
            || now < self.observed_at_ms
            || now - self.observed_at_ms > 30_000
            || self.storage.is_empty()
            || self.storage.len() > 64
            || self.storage.keys().any(|id| !valid_id(id))
        {
            return Err(
                "Capacity is unknown or stale; check the execution host's RAM and storage measurements",
            );
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllocationRequest {
    pub version: u32,
    pub id: String,
    pub leader: String,
    pub scan: String,
    pub agent: String,
    pub branch: String,
    pub domain: String,
    pub configuration: String,
    pub configuration_revision: u64,
    pub instance: String,
    pub memory: u64,
    pub storage: Vec<StorageDemand>,
    pub storage_reserve: u64,
    /// Feasibility only; this allocation must be stopped before admission.
    pub release_parent: Option<String>,
    pub processing_result: bool,
}
impl AllocationRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != VERSION
            || [
                &self.id,
                &self.leader,
                &self.scan,
                &self.agent,
                &self.branch,
                &self.domain,
                &self.configuration,
                &self.instance,
            ]
            .iter()
            .any(|id| !valid_id(id))
            || self.configuration_revision == 0
            || self.memory == 0
            || self.memory > i64::MAX as u64
            || self.storage.is_empty()
            || self.storage.len() > 64
            || self
                .storage
                .iter()
                .any(|s| !valid_id(&s.pool) || !valid_id(&s.key) || s.bytes > i64::MAX as u64)
            || self
                .release_parent
                .as_ref()
                .is_some_and(|id| !valid_id(id) || id == &self.id)
            || self.storage_reserve > i64::MAX as u64
        {
            return Err("invalid allocation request");
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllocationPhase {
    Queued,
    Reserved,
    Started,
    Retained,
    Blocked,
    Canceled,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Allocation {
    pub request: AllocationRequest,
    pub phase: AllocationPhase,
    pub prepared: bool,
    pub execution: Option<crate::swarm_host::ExecutionTarget>,
    pub created_at_ms: i64,
    pub reservation_deadline_ms: i64,
    pub reason: Option<String>,
    pub sequence: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Feasibility {
    Ready,
    ReleaseParent { allocation: String },
    Denied { reason: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeletionStep {
    Retained,
    Claimed,
    ContainerRemoved,
    VolumeRemoved,
    Deleted,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedEnvironment {
    pub instance: String,
    pub leader: String,
    pub scan: String,
    pub agent: String,
    pub generation: u64,
    pub ended_at_ms: Option<i64>,
    pub deadline_ms: Option<i64>,
    pub keep: bool,
    pub references: BTreeSet<String>,
    pub pending_outputs: BTreeSet<String>,
    pub termination_confirmed: bool,
    pub checkpoint_confirmed: bool,
    pub deletion: DeletionStep,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum RetentionAction {
    Keep {
        value: bool,
    },
    Protect {
        reference: String,
    },
    Release {
        reference: String,
    },
    End {
        ended_at_ms: i64,
        retention_seconds: u64,
    },
    Reopen,
    Pending {
        operation: String,
    },
    Acknowledge {
        operation: String,
    },
    Checkpoint,
    ClaimDelete {
        now: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllocationSpec {
    pub id: String,
    pub leader: String,
    pub scan: String,
    pub agent: String,
    pub parent: String,
    pub configuration: String,
    pub input_bytes: u64,
    pub storage_reserve: u64,
    pub processing_result: bool,
    pub restore: Option<EnvironmentBundle>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum HostAllocationRequest {
    PreviewDiscard {
        instance: String,
        generation: u64,
    },
    ConfirmDiscard {
        preview: DiscardPreview,
    },
    ScanLifecycle {
        lifecycle: ScanLifecycle,
    },
    Feasibility {
        spec: AllocationSpec,
    },
    Enqueue {
        spec: AllocationSpec,
    },
    Inspect {
        id: String,
    },
    Cancel {
        id: String,
    },
    Checkpoint {
        target: crate::swarm_host::ExecutionTarget,
    },
    CheckpointAccepted {
        target: crate::swarm_host::ExecutionTarget,
        digest: String,
    },
    Export {
        target: crate::swarm_host::ExecutionTarget,
        transfer: String,
    },
    CancelExport {
        target: crate::swarm_host::ExecutionTarget,
        transfer: String,
    },
    ReadArchive {
        checkpoint: String,
        kind: ArchiveKind,
        offset: u64,
    },
    BeginRestore {
        allocation: String,
        bundle: EnvironmentBundle,
    },
    WriteArchive {
        allocation: String,
        kind: ArchiveKind,
        offset: u64,
        bytes: Vec<u8>,
    },
    FinishRestore {
        allocation: String,
    },
    Retention {
        instance: String,
        generation: u64,
        action: RetentionAction,
    },
    Environments,
    Inventory,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanLifecycle {
    pub leader: String,
    pub scan: String,
    pub revision: u64,
    pub ended_at_ms: Option<i64>,
    pub retention_seconds: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum HostAllocationReply {
    Inventory {
        allocations: Vec<Allocation>,
        environments: Vec<RetainedEnvironment>,
    },
    DiscardPreview {
        preview: DiscardPreview,
    },
    Feasibility {
        decision: Feasibility,
    },
    Allocation {
        allocation: Allocation,
    },
    Environment {
        environment: RetainedEnvironment,
    },
    Environments {
        environments: Vec<RetainedEnvironment>,
    },
    Bundle {
        bundle: EnvironmentBundle,
    },
    Archive {
        offset: u64,
        bytes: Vec<u8>,
    },
    Complete,
    Busy {},
    Rejected {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscardPreview {
    pub instance: String,
    pub generation: u64,
    pub operations: BTreeSet<String>,
    pub confirmation: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveKind {
    Workspace,
    State,
    Image,
}
impl ArchiveKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::State => "state",
            Self::Image => "image",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentArchive {
    pub kind: ArchiveKind,
    pub sha256: String,
    pub bytes: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentBundle {
    pub version: u32,
    pub target: crate::swarm_host::ExecutionTarget,
    pub configuration: crate::swarm_container::ImageConfiguration,
    pub image_digest: Option<String>,
    pub archives: Vec<EnvironmentArchive>,
}
impl EnvironmentBundle {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.target.validate()?;
        self.configuration.validate()?;
        if self.version != 1
            || self.archives.len() < 2
            || self.archives.len() > 3
            || self
                .archives
                .iter()
                .any(|a| a.bytes == 0 || !crate::swarm_files::digest(&a.sha256))
            || self
                .archives
                .iter()
                .map(|a| a.kind)
                .collect::<BTreeSet<_>>()
                .len()
                != self.archives.len()
            || ![ArchiveKind::Workspace, ArchiveKind::State]
                .iter()
                .all(|k| self.archives.iter().any(|a| a.kind == *k))
            || self.archives.iter().any(|a| a.kind == ArchiveKind::Image)
                != self.image_digest.is_some()
            || self.image_digest.as_ref().is_some_and(|s| {
                !s.strip_prefix("sha256:")
                    .is_some_and(crate::swarm_files::digest)
            })
        {
            return Err("invalid retained environment bundle");
        }
        Ok(())
    }
}
