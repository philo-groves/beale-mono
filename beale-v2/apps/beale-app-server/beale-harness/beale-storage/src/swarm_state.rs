//! Host-only Swarms persistence. No scheduler, worker transport, or Graph fallback.
//!
//! All changes use SQLite immediate transactions, including authority checks and
//! mutation acknowledgements. External effects must use the operation journal;
//! committing a database acknowledgement does not make shell execution exactly once.

use crate::HostDatabase;
use anyhow::{Context, Result, ensure};
use beale_contracts::swarm_workflows::*;
use beale_contracts::swarms::*;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use uuid::Uuid;

#[path = "swarm_local.rs"]
mod local;
pub use local::*;
#[path = "swarm_local_workers.rs"]
mod local_workers;
pub use local_workers::{LocalWorkerRecord, local_worker_backend};
#[path = "swarm_peer_workers.rs"]
mod peer_workers;
pub use peer_workers::{PeerPlacement, PeerRecoveryWarning, PeerWorkerRecord};
#[path = "swarm_coordinators.rs"]
mod coordinators;
pub use coordinators::CoordinatorTurnCommit;
#[path = "swarm_inference.rs"]
mod inference;
#[path = "swarm_plugin_resources.rs"]
mod plugin_resources;
#[path = "swarm_plugins.rs"]
mod plugins;
pub use inference::{InferenceClaim, InferenceOwner};
pub use plugin_resources::{PluginResourceInput, PreparedPluginBundle, plugin_resource_directory};
pub use plugins::{PluginClaim, PluginOwner};
#[path = "swarm_worker_runtime.rs"]
mod worker_runtime;
pub use worker_runtime::{WorkerProgress, WorkerProvenance};
#[path = "swarm_worker_activity.rs"]
mod worker_activity;
#[path = "swarm_worker_files.rs"]
mod worker_files;
#[path = "swarm_worker_reconcile.rs"]
mod worker_reconcile;
pub use worker_reconcile::{WorkerDisposition, WorkerReconciliation};

const SCHEMA: &str = "
CREATE TABLE swarm_scans(id TEXT PRIMARY KEY, document TEXT NOT NULL);
CREATE TABLE swarm_receipts(scan_id TEXT NOT NULL REFERENCES swarm_scans(id), actor TEXT NOT NULL,
 operation_id TEXT NOT NULL, content TEXT NOT NULL, result TEXT NOT NULL,
 PRIMARY KEY(scan_id, actor, operation_id));
CREATE TABLE swarm_credentials(digest TEXT PRIMARY KEY, scan_id TEXT NOT NULL REFERENCES swarm_scans(id),
 agent_id TEXT NOT NULL, attempt_id TEXT NOT NULL, generation INTEGER NOT NULL,
 audience TEXT NOT NULL, expires_at_ms INTEGER NOT NULL, revoked INTEGER NOT NULL DEFAULT 0);

";

// Claims survive a stopped execution because retained private instances remain
// owned by their agent. Releasing/reassigning physical instances belongs to Fleet.
const INSTANCE_OWNERSHIP_V2: &str = "
CREATE TABLE swarm_instance_owners (
    host_id TEXT NOT NULL,
    instance_id TEXT NOT NULL,
    scan_id TEXT NOT NULL REFERENCES swarm_scans(id),
    agent_id TEXT NOT NULL,
    PRIMARY KEY(host_id, instance_id)
);
INSERT INTO swarm_instance_owners(host_id,instance_id,scan_id,agent_id)
SELECT json_extract(i.value,'$.host_id'), json_extract(i.value,'$.id'), s.id,
       json_extract(i.value,'$.agent_id')
FROM swarm_scans s, json_each(s.document,'$.instances') i;
";

/// Trusted execution callback identity. The host supplies the current clock;
/// these fields are checked against durable state, never taken as authority alone.
pub struct ExecutionWriteContext {
    pub attempt_id: String,
    pub generation: u64,
    pub now_ms: i64,
}

/// Trusted host reconciliation only; deliberately absent from WorkerOperation.
/// Evidence references identify the host's durable basis for a known outcome.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum JournalRecovery {
    OutcomeUnknown,
    Canceled {
        evidence_refs: Vec<String>,
    },
    Completed {
        result: Value,
        evidence_refs: Vec<String>,
    },
}

#[derive(Clone)]
pub struct SwarmStore {
    db: HostDatabase,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRecord {
    pub summary: AgentSummary,
    pub checkpoint: AgentCheckpoint,
}

/// Durable host document, never a model/tool response. Conversation contents are
/// intentionally separate from the small, explicitly selected AgentSummary view.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanState {
    pub config: ScanConfig,
    pub revision: u64,
    pub ended_at_ms: Option<i64>,
    pub agents: Vec<AgentRecord>,
    pub assignments: Vec<Assignment>,
    pub handoffs: Vec<WorkflowHandoff>,
    pub attempts: Vec<ExecutionAttempt>,
    pub operations: Vec<ToolOperation>,
    pub events: Vec<LifecycleEvent>,
    pub environments: Vec<EnvironmentState>,
    pub instances: Vec<ExecutionInstance>,
    pub reservations: Vec<ResourceReservation>,
}

fn checked(result: ValidationResult) -> Result<()> {
    result.map_err(anyhow::Error::msg)
}
pub(super) fn unique<'a>(values: impl Iterator<Item = &'a str>) -> bool {
    let mut seen = HashSet::new();
    values.into_iter().all(|id| valid_id(id) && seen.insert(id))
}
pub(super) fn next(value: u64) -> Result<u64> {
    value.checked_add(1).context("generation exhausted")
}
fn new_id() -> String {
    Uuid::new_v4().to_string()
}

impl ScanState {
    /// Shared by the supervisor and durable coordinator recovery. A completed
    /// leader is terminal only after every execution and assignment has settled.
    pub fn ready_to_end(&self) -> Result<bool> {
        let leader = self
            .agents
            .iter()
            .find(|a| a.summary.role == Role::Leader)
            .context("missing coordinator")?;
        Ok(matches!(
            leader.summary.state,
            AgentState::Blocked | AgentState::Stopped
        ) || (leader.summary.state == AgentState::Completed
            && self
                .attempts
                .iter()
                .all(|a| a.state == ExecutionState::Stopped)
            && !self
                .assignments
                .iter()
                .any(|a| matches!(a.state, AssignmentState::Queued | AssignmentState::Assigned))))
    }

    pub fn agent(&self, id: &str) -> Result<&AgentRecord> {
        self.agents
            .iter()
            .find(|a| a.summary.id == id)
            .context("unknown agent")
    }

    pub fn validate(&self) -> Result<()> {
        checked(self.config.validate())?;
        ensure!(self.revision > 0, "invalid scan revision");
        ensure!(
            unique(self.agents.iter().map(|a| a.summary.id.as_str())),
            "duplicate agent"
        );
        let swarm = matches!(self.config.mode, ScanMode::Swarm { .. });
        ensure!(
            if swarm {
                self.agents
                    .iter()
                    .filter(|a| a.summary.role == Role::Leader)
                    .count()
                    == 1
            } else {
                self.agents.is_empty()
            },
            "invalid scan leadership"
        );
        for a in &self.agents {
            let s = &a.summary;
            ensure!(
                self.config
                    .profile(&s.profile)
                    .is_some_and(|p| p.role == s.role)
                    && s.revision > 0,
                "invalid agent profile"
            );
            match &s.parent_id {
                None => ensure!(s.role == Role::Leader, "missing parent"),
                Some(parent) => ensure!(
                    self.agent(parent)?.summary.role.child() == Some(s.role)
                        && self
                            .workflow()?
                            .permits(&self.agent(parent)?.summary.profile.id, &s.profile.id),
                    "invalid parent relationship"
                ),
            }
            ensure!(
                a.checkpoint.state == s.state && bounded_json(&a.checkpoint.context),
                "invalid checkpoint"
            );
            ensure!(
                unique(a.checkpoint.wake_reasons.iter().map(String::as_str)),
                "invalid wake reasons"
            );
            ensure!(
                unique(a.checkpoint.pending_assignments.iter().map(String::as_str)),
                "invalid pending assignments"
            );
            for id in &a.checkpoint.pending_assignments {
                ensure!(
                    self.assignments.iter().any(|v| &v.id == id
                        && v.parent_id == s.id
                        && matches!(v.state, AssignmentState::Queued | AssignmentState::Assigned)),
                    "unknown pending assignment"
                );
            }
            if let Some(id) = &a.checkpoint.environment_id {
                ensure!(
                    self.environments
                        .iter()
                        .any(|v| &v.id == id && v.agent_id == s.id),
                    "unknown environment"
                );
            }
        }
        ensure!(
            unique(self.assignments.iter().map(|v| v.id.as_str())),
            "duplicate assignment"
        );
        for a in &self.assignments {
            checked(a.task.validate())?;
            ensure!(
                self.agent(&a.child_id)?.summary.parent_id.as_ref() == Some(&a.parent_id),
                "invalid assignment parent"
            );
            ensure!(
                a.wait_reason.len() <= 4096
                    && (a.state != AssignmentState::Queued || !a.wait_reason.is_empty()),
                "invalid wait reason"
            );
        }
        ensure!(
            unique(self.handoffs.iter().map(|v| v.request.id.as_str())),
            "duplicate handoff"
        );
        for handoff in &self.handoffs {
            self.handoff_path(&handoff.request)?;
            ensure!(handoff.revision > 0, "invalid handoff revision");
            ensure!(
                (handoff.status == HandoffStatus::Completed) == handoff.result.is_some(),
                "invalid handoff outcome"
            );
            if let Some(result) = &handoff.result {
                checked(result.validate())?;
            }
            ensure!(
                handoff
                    .assignment_context
                    .as_ref()
                    .is_none_or(|v| !v.trim().is_empty() && v.len() <= 4096),
                "invalid assignment context"
            );
        }
        ensure!(
            unique(self.attempts.iter().map(|v| v.id.as_str())),
            "duplicate attempt"
        );
        let mut generations = HashSet::new();
        for a in &self.attempts {
            self.agent(&a.agent_id)?;
            ensure!(
                a.generation > 0
                    && a.expires_at_ms > 0
                    && generations.insert((&a.agent_id, a.generation)),
                "invalid execution generation"
            );
            if a.state != ExecutionState::Stopped {
                ensure!(
                    !self.attempts.iter().any(
                        |other| other.agent_id == a.agent_id && other.generation > a.generation
                    ),
                    "replacement before termination confirmation"
                );
            }
        }
        ensure!(
            unique(self.operations.iter().map(|v| v.id.as_str())),
            "duplicate tool operation"
        );
        for o in &self.operations {
            ensure!(
                o.revision > 0
                    && self
                        .attempts
                        .iter()
                        .any(|a| a.id == o.attempt_id && a.agent_id == o.agent_id),
                "invalid tool operation owner"
            );
            ensure!(
                (o.state == ToolOperationState::Completed || o.result.is_null())
                    && bounded_json(&o.result)
                    && o.request.is_object()
                    && bounded_json(&o.request),
                "invalid tool operation content"
            );
        }
        ensure!(
            unique(self.events.iter().map(|v| v.id.as_str())),
            "duplicate lifecycle event"
        );
        for (index, e) in self.events.iter().enumerate() {
            self.agent(&e.agent_id)?;
            ensure!(
                e.sequence == index as u64 + 1 && valid_id(&e.kind) && bounded_json(&e.payload),
                "invalid lifecycle event"
            );
        }
        ensure!(
            unique(self.environments.iter().map(|v| v.id.as_str())),
            "duplicate environment"
        );
        for e in &self.environments {
            self.agent(&e.agent_id)?;
            ensure!(
                e.generation > 0 && unique(e.pending_publications.iter().map(String::as_str)),
                "invalid environment"
            );
            ensure!(
                e.deletion == DeletionState::Retained
                    || (!e.keep
                        && e.active_references == 0
                        && e.pending_publications.is_empty()
                        && e.termination_confirmed),
                "deleted protected environment"
            );
            ensure!(
                !e.termination_confirmed
                    || (!self
                        .attempts
                        .iter()
                        .any(|a| a.agent_id == e.agent_id && a.state != ExecutionState::Stopped)
                        && !self.instances.iter().any(|i| i.agent_id == e.agent_id && i.state != InstanceState::Stopped)),
                "environment termination unconfirmed"
            );
        }
        ensure!(
            unique(self.instances.iter().map(|v| v.id.as_str())),
            "duplicate instance"
        );
        let mut active_agents = HashSet::new();
        for i in &self.instances {
            let attempt = self
                .attempts
                .iter()
                .find(|a| a.id == i.attempt_id && a.agent_id == i.agent_id)
                .context("invalid instance ownership")?;
            ensure!(
                valid_id(&i.configuration_id) && valid_id(&i.host_id),
                "invalid instance ownership"
            );
            ensure!(
                match i.state {
                    InstanceState::Starting | InstanceState::Running =>
                        self.ended_at_ms.is_none() && attempt.state == ExecutionState::Running,
                    InstanceState::TerminationUnconfirmed =>
                        attempt.state != ExecutionState::Stopped,
                    InstanceState::Stopped => true,
                },
                "instance state conflicts with execution authority"
            );
            ensure!(
                i.state == InstanceState::Stopped || active_agents.insert(&i.agent_id),
                "agent already owns an active instance"
            );
        }
        ensure!(
            unique(self.reservations.iter().map(|v| v.id.as_str())),
            "duplicate resource reservation"
        );
        for r in &self.reservations {
            self.agent(&r.agent_id)?;
            ensure!(
                valid_id(&r.host_id) && r.expires_at_ms > 0,
                "invalid reservation"
            );
        }
        Ok(())
    }
}

fn read_scan(db: &Connection, scan: &str) -> Result<ScanState> {
    let document: String = db.query_row(
        "SELECT document FROM swarm_scans WHERE id=?1",
        [scan],
        |r| r.get(0),
    )?;
    let state: ScanState = serde_json::from_str(&document)
        .context("incompatible saved swarm state; preserved unchanged")?;
    state.validate()?;
    ensure!(state.config.scan_id == scan, "saved scan identity mismatch");
    Ok(state)
}

fn write_scan(tx: &Transaction<'_>, state: &mut ScanState) -> Result<()> {
    state.revision = next(state.revision)?;
    state.validate()?;
    tx.execute(
        "UPDATE swarm_scans SET document=?2 WHERE id=?1",
        params![state.config.scan_id, serde_json::to_string(state)?],
    )?;
    Ok(())
}

impl SwarmStore {
    /// Explicit initialization: a host that does not load Swarms creates no Swarms tables.
    pub fn open(db: HostDatabase) -> Result<Self> {
        db.migrate_schema("swarms", &[SCHEMA, INSTANCE_OWNERSHIP_V2])?;
        db.migrate_schema("swarm-local", &[local::SCHEMA])?;
        db.migrate_schema("swarm-peer-workers", &[peer_workers::SCHEMA])?;
        db.migrate_schema(
            "swarm-local-workers",
            &[
                local_workers::SCHEMA,
                coordinators::SCHEMA,
                coordinators::TRANSCRIPT_SCHEMA,
            ],
        )?;
        db.migrate_schema("swarm-plugins", &[plugins::SCHEMA])?;
        db.migrate_schema("swarm-plugin-resources", &[plugin_resources::SCHEMA])?;
        db.migrate_schema(
            "swarm-inference",
            &[inference::SCHEMA, inference::ACTIVITY_SCHEMA],
        )?;
        db.migrate_schema(
            "swarm-worker-runtime",
            &[
                worker_runtime::SCHEMA,
                worker_runtime::ACTIVE_TIME_SCHEMA,
                worker_runtime::ASSIGNMENT_SCHEMA,
            ],
        )?;
        Ok(Self { db })
    }

    fn transaction<T>(&self, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        self.db.with_connection(|db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let result = f(&tx)?;
            tx.commit()?;
            Ok(result)
        })
    }

    /// Creation gates are supplied by the trusted host feature registry, never the worker.
    pub fn create_scan(
        &self,
        config: ScanConfig,
        enabled: bool,
        supported: bool,
    ) -> Result<ScanState> {
        checked(config.validate_creation(enabled, supported))?;
        let agents = match &config.mode {
            ScanMode::Local {} => vec![],
            ScanMode::Swarm { snapshot } => {
                let leader = snapshot
                    .profiles
                    .iter()
                    .find(|p| p.role == Role::Leader)
                    .context("missing leader")?;
                vec![AgentRecord {
                    summary: AgentSummary {
                        id: new_id(),
                        parent_id: None,
                        profile: leader.profile.clone(),
                        role: Role::Leader,
                        state: AgentState::Queued,
                        revision: 1,
                    },
                    checkpoint: AgentCheckpoint {
                        state: AgentState::Queued,
                        context: json!({}),
                        inbox_cursor: 0,
                        wake_reasons: vec![],
                        pending_assignments: vec![],
                        environment_id: None,
                    },
                }]
            }
        };
        let state = ScanState {
            config,
            revision: 1,
            ended_at_ms: None,
            agents,
            assignments: vec![],
            handoffs: vec![],
            attempts: vec![],
            operations: vec![],
            events: vec![],
            environments: vec![],
            instances: vec![],
            reservations: vec![],
        };
        state.validate()?;
        self.transaction(|tx| {
            tx.execute(
                "INSERT INTO swarm_scans(id,document) VALUES(?1,?2)",
                params![state.config.scan_id, serde_json::to_string(&state)?],
            )?;
            Ok(())
        })?;
        Ok(state)
    }

    pub fn load(&self, scan: &str, supported: bool) -> Result<ScanState> {
        self.db.with_connection(|db| {
            let state = read_scan(db, scan)?;
            checked(state.config.validate_resume(supported))?;
            Ok(state)
        })
    }

    pub fn summaries(&self, scan: &str) -> Result<Vec<AgentSummary>> {
        Ok(self
            .load(scan, true)?
            .agents
            .into_iter()
            .map(|a| a.summary)
            .collect())
    }

    /// Host acceptance only: allocation and admission are separate later-stage actions.
    pub fn queue_spawn(
        &self,
        scan: &str,
        parent: &str,
        operation_id: &str,
        profile: ProfileRef,
        task: Task,
        wait_reason: &str,
    ) -> Result<SpawnResult> {
        checked(task.validate())?;
        ensure!(
            valid_id(operation_id) && !wait_reason.is_empty() && wait_reason.len() <= 4096,
            "invalid spawn acceptance"
        );
        let content = json!({"kind":"spawn", "profile":profile, "task":task});
        self.transaction(|tx| {
            let mut state = read_scan(tx, scan)?;
            let parent_agent = state.agent(parent)?;
            let role = state
                .config
                .profile(&profile)
                .context("unknown profile")?
                .role;
            ensure!(
                parent_agent.summary.role.child() == Some(role)
                    && state
                        .workflow()?
                        .permits(&parent_agent.summary.profile.id, &profile.id),
                "invalid child role"
            );
            if let Some(result) = receipt(tx, scan, parent, operation_id, &content)? {
                return Ok(serde_json::from_value(result)?);
            }
            ensure!(state.ended_at_ms.is_none(), "scan has ended");
            let child_id = new_id();
            let assignment_id = new_id();
            let result = SpawnResult::Queued {
                child_id: child_id.clone(),
                assignment_id: assignment_id.clone(),
                wait_reason: wait_reason.to_owned(),
            };
            state.agents.push(AgentRecord {
                summary: AgentSummary {
                    id: child_id.clone(),
                    parent_id: Some(parent.to_owned()),
                    profile,
                    role,
                    state: AgentState::Queued,
                    revision: 1,
                },
                checkpoint: AgentCheckpoint {
                    state: AgentState::Queued,
                    context: json!({}),
                    inbox_cursor: 0,
                    wake_reasons: vec![],
                    pending_assignments: vec![],
                    environment_id: None,
                },
            });
            state.assignments.push(Assignment {
                id: assignment_id,
                parent_id: parent.to_owned(),
                child_id,
                task,
                state: AssignmentState::Queued,
                wait_reason: wait_reason.to_owned(),
            });
            write_scan(tx, &mut state)?;
            acknowledge(
                tx,
                scan,
                parent,
                operation_id,
                &content,
                &serde_json::to_value(&result)?,
            )?;
            Ok(result)
        })
    }

    /// Host initialization only, before this agent has any execution attempts.
    /// Once execution begins, use `checkpoint_execution` even after an attempt stops.
    pub fn checkpoint(
        &self,
        scan: &str,
        agent: &str,
        expected_revision: u64,
        checkpoint: AgentCheckpoint,
    ) -> Result<()> {
        self.save_checkpoint(scan, agent, expected_revision, checkpoint, None)
    }

    /// Execution-originated checkpoints must carry the attempt that produced them.
    pub fn checkpoint_execution(
        &self,
        scan: &str,
        agent: &str,
        expected_revision: u64,
        checkpoint: AgentCheckpoint,
        execution: &ExecutionWriteContext,
    ) -> Result<()> {
        self.save_checkpoint(scan, agent, expected_revision, checkpoint, Some(execution))
    }

    fn save_checkpoint(
        &self,
        scan: &str,
        agent: &str,
        expected_revision: u64,
        checkpoint: AgentCheckpoint,
        execution: Option<&ExecutionWriteContext>,
    ) -> Result<()> {
        self.transaction(|tx| {
            let mut state = read_scan(tx, scan)?;
            ensure!(state.ended_at_ms.is_none(), "scan has ended");
            if let Some(execution) = execution {
                ensure_execution(&state, agent, execution)?;
            } else {
                ensure!(
                    !state.attempts.iter().any(|a| a.agent_id == agent),
                    "execution checkpoint requires an attempt identity"
                );
            }
            let a = state
                .agents
                .iter_mut()
                .find(|a| a.summary.id == agent)
                .context("unknown agent")?;
            ensure!(
                a.summary.revision == expected_revision,
                "stale agent revision"
            );
            ensure!(
                checkpoint.inbox_cursor >= a.checkpoint.inbox_cursor,
                "inbox cursor cannot regress"
            );
            a.summary.revision = next(a.summary.revision)?;
            a.summary.state = checkpoint.state;
            a.checkpoint = checkpoint;
            write_scan(tx, &mut state)
        })
    }

    pub fn set_assignment_state(
        &self,
        scan: &str,
        assignment: &str,
        expected_scan_revision: u64,
        next_state: AssignmentState,
    ) -> Result<()> {
        self.transaction(|tx| {
            let mut state = read_scan(tx, scan)?;
            ensure!(
                state.revision == expected_scan_revision,
                "stale scan revision"
            );
            let a = state
                .assignments
                .iter_mut()
                .find(|a| a.id == assignment)
                .context("unknown assignment")?;
            ensure!(
                matches!(
                    (a.state, next_state),
                    (
                        AssignmentState::Queued,
                        AssignmentState::Assigned
                            | AssignmentState::Blocked
                            | AssignmentState::Canceled
                    ) | (
                        AssignmentState::Assigned,
                        AssignmentState::Completed
                            | AssignmentState::Blocked
                            | AssignmentState::Canceled
                    ) | (
                        AssignmentState::Blocked,
                        AssignmentState::Queued | AssignmentState::Canceled
                    )
                ),
                "invalid assignment transition"
            );
            a.state = next_state;
            if !matches!(
                next_state,
                AssignmentState::Queued | AssignmentState::Assigned
            ) {
                for parent in &mut state.agents {
                    if parent
                        .checkpoint
                        .pending_assignments
                        .iter()
                        .any(|id| id == assignment)
                    {
                        parent
                            .checkpoint
                            .pending_assignments
                            .retain(|id| id != assignment);
                        parent.summary.revision = next(parent.summary.revision)?;
                    }
                }
            }
            write_scan(tx, &mut state)
        })
    }

    /// Accept immutable observations for the immediate parent; never dispatch a sibling.
    pub fn add_handoff(&self, scan: &str, request: HandoffRequest) -> Result<WorkflowHandoff> {
        checked(request.validate())?;
        self.transaction(|tx| {
            let mut state = read_scan(tx, scan)?;
            state.handoff_path(&request)?;
            crate::swarm_files::validate_record_artifacts(
                tx,
                scan,
                &state.config.workspace_id,
                &serde_json::to_value(&request)?,
            )?;
            let content = serde_json::to_value(&request)?;
            if let Some(saved) = receipt(tx, scan, "host-handoff", &request.id, &content)? {
                return Ok(serde_json::from_value(saved)?);
            }
            ensure!(state.ended_at_ms.is_none(), "scan has ended");
            let handoff = WorkflowHandoff {
                request: request.clone(),
                revision: 1,
                status: HandoffStatus::Pending,
                assignment_context: None,
                result: None,
            };
            state.handoffs.push(handoff.clone());
            write_scan(tx, &mut state)?;
            acknowledge(
                tx,
                scan,
                "host-handoff",
                &request.id,
                &content,
                &serde_json::to_value(&handoff)?,
            )?;
            Ok(handoff)
        })
    }

    /// Host-only transition. Completion records an assessment, never changes a finding.
    pub fn change_handoff(
        &self,
        scan: &str,
        operation_id: &str,
        change: HandoffChange,
    ) -> Result<WorkflowHandoff> {
        ensure!(valid_id(operation_id), "invalid operation identity");
        let content = serde_json::to_value(&change)?;
        self.transaction(|tx| {
            let mut state = read_scan(tx, scan)?;
            crate::swarm_files::validate_record_artifacts(
                tx,
                scan,
                &state.config.workspace_id,
                &content,
            )?;
            if let Some(saved) =
                receipt(tx, scan, "host-handoff-transition", operation_id, &content)?
            {
                return Ok(serde_json::from_value(saved)?);
            }
            ensure!(state.ended_at_ms.is_none(), "scan has ended");
            let handoff = state
                .handoffs
                .iter_mut()
                .find(|h| h.request.id == change.id)
                .context("unknown handoff")?;
            ensure!(
                handoff.revision == change.expected_revision,
                "stale handoff revision"
            );
            ensure!(
                matches!(
                    (handoff.status, change.status),
                    (
                        HandoffStatus::Pending,
                        HandoffStatus::Assigned | HandoffStatus::Blocked | HandoffStatus::Canceled
                    ) | (
                        HandoffStatus::Assigned,
                        HandoffStatus::Completed | HandoffStatus::Blocked | HandoffStatus::Canceled
                    ) | (
                        HandoffStatus::Blocked,
                        HandoffStatus::Assigned | HandoffStatus::Canceled
                    )
                ),
                "invalid handoff transition"
            );
            ensure!(
                change.status != HandoffStatus::Assigned || change.assignment_context.is_some(),
                "assignment context required"
            );
            handoff.status = change.status;
            if change.assignment_context.is_some() {
                handoff.assignment_context = change.assignment_context;
            }
            handoff.result = change.result;
            handoff.revision = next(handoff.revision)?;
            let result = handoff.clone();
            write_scan(tx, &mut state)?;
            acknowledge(
                tx,
                scan,
                "host-handoff-transition",
                operation_id,
                &content,
                &serde_json::to_value(&result)?,
            )?;
            Ok(result)
        })
    }

    pub fn append_event(&self, scan: &str, event: LifecycleEvent) -> Result<()> {
        self.transaction(|tx| {
            let mut state = read_scan(tx, scan)?;
            if let Some(saved) = state.events.iter().find(|e| e.id == event.id) {
                ensure!(saved == &event, "event identity reused");
                return Ok(());
            }
            state.events.push(event);
            write_scan(tx, &mut state)
        })
    }

    pub fn put_environment(&self, scan: &str, mut environment: EnvironmentState) -> Result<()> {
        ensure!(
            environment.generation == 1 && environment.deletion == DeletionState::Retained,
            "invalid new environment"
        );
        self.transaction(|tx| {
            let mut state = read_scan(tx, scan)?;
            let deadline = state
                .ended_at_ms
                .map(|ended| retention_deadline(&state, ended))
                .transpose()?;
            ensure!(
                environment.retention_deadline_ms.is_none()
                    || environment.retention_deadline_ms == deadline,
                "environment deadline conflicts with scan retention"
            );
            environment.retention_deadline_ms = deadline;
            state.environments.push(environment);
            write_scan(tx, &mut state)
        })
    }

    pub fn reserve_resources(&self, scan: &str, reservation: ResourceReservation) -> Result<()> {
        self.transaction(|tx| {
            let mut state = read_scan(tx, scan)?;
            state.reservations.push(reservation);
            write_scan(tx, &mut state)
        })
    }

    pub fn register_instance(&self, scan: &str, instance: ExecutionInstance) -> Result<()> {
        self.transaction(|tx| {
            let mut state = read_scan(tx, scan)?;
            if let Some(saved) = state.instances.iter().find(|i| i.id == instance.id) {
                ensure!(saved == &instance, "instance identity reused with different content");
                return Ok(());
            }
            tx.execute("INSERT INTO swarm_instance_owners(host_id,instance_id,scan_id,agent_id) VALUES(?1,?2,?3,?4)", params![instance.host_id, instance.id, scan, instance.agent_id])
                .context("instance already owned or invalid ownership claim")?;
            state.instances.push(instance);
            write_scan(tx, &mut state)
        })
    }
}

fn receipt(
    tx: &Transaction<'_>,
    scan: &str,
    actor: &str,
    id: &str,
    content: &Value,
) -> Result<Option<Value>> {
    let claimed: Option<String> = tx.query_row("SELECT request FROM swarm_inference WHERE scan_id=?1 AND agent_id=?2 AND operation_id=?3",
        params![scan, actor, id], |r| r.get(0)).optional()?;
    if let Some(claimed) = claimed {
        let request: WorkerRequest = serde_json::from_str(&claimed)?;
        ensure!(
            serde_json::to_value(request.operation)? == *content,
            "operation ID is bound to a different inference request"
        );
    }
    let plugin: Option<String> = tx.query_row("SELECT request FROM swarm_plugin_calls WHERE scan_id=?1 AND agent_id=?2 AND operation_id=?3", params![scan,actor,id], |r|r.get(0)).optional()?;
    if let Some(plugin) = plugin {
        ensure!(
            serde_json::from_str::<Value>(&plugin)? == *content,
            "operation ID is bound to a different plugin request"
        );
    }
    let saved: Option<(String, String)> = tx.query_row("SELECT content,result FROM swarm_receipts WHERE scan_id=?1 AND actor=?2 AND operation_id=?3", params![scan, actor, id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    saved
        .map(|(bound, result)| {
            ensure!(
                serde_json::from_str::<Value>(&bound)? == *content,
                "operation ID reused with different content"
            );
            Ok(serde_json::from_str(&result)?)
        })
        .transpose()
}

fn acknowledge(
    tx: &Transaction<'_>,
    scan: &str,
    actor: &str,
    id: &str,
    content: &Value,
    result: &Value,
) -> Result<()> {
    ensure!(bounded_json(result), "mutation result too large");
    tx.execute(
        "INSERT INTO swarm_receipts VALUES(?1,?2,?3,?4,?5)",
        params![
            scan,
            actor,
            id,
            serde_json::to_string(content)?,
            serde_json::to_string(result)?
        ],
    )?;
    Ok(())
}

/// Not serializable or printable. Provision only through a trusted supervisor.
pub struct WorkerCredential(String);
impl WorkerCredential {
    pub fn bearer(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Debug for WorkerCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WorkerCredential([redacted])")
    }
}
fn digest(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallerContext {
    pub agent: AgentSummary,
    pub authorized_scope: String,
    pub enabled_tools: Vec<String>,
    pub provider_routes: Vec<ProviderRoute>,
    pub review_rule: Option<ReviewRuleSnapshot>,
}

impl SwarmStore {
    /// Requires trusted confirmation of prior termination; lease expiry alone is insufficient.
    pub fn start_attempt(
        &self,
        scan: &str,
        agent: &str,
        registration: &WorkerRegistration,
        now_ms: i64,
        expires_at_ms: i64,
    ) -> Result<(ExecutionAttempt, WorkerCredential)> {
        checked(registration.validate())?;
        validate_lease(now_ms, expires_at_ms)?;
        self.transaction(|tx| {
            let mut state = read_scan(tx, scan)?;
            state.agent(agent)?;
            ensure_worker_branch_open(tx, scan, agent)?;
            ensure!(state.ended_at_ms.is_none(), "ended scan must be reopened before execution");
            ensure!(state.attempts.iter().filter(|a| a.agent_id == agent).all(|a| a.state == ExecutionState::Stopped), "prior execution termination unconfirmed");
            ensure!(state.environments.iter().filter(|e| e.agent_id == agent).all(|e| e.deletion == DeletionState::Retained && e.active_references > 0), "environment must be protected before execution");
            let generation = next(state.attempts.iter().filter(|a| a.agent_id == agent).map(|a| a.generation).max().unwrap_or(0))?;
            let attempt = ExecutionAttempt { id: new_id(), agent_id: agent.to_owned(), generation, expires_at_ms, state: ExecutionState::Running };
            let credential = WorkerCredential(format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()));
            tx.execute("INSERT INTO swarm_credentials(digest,scan_id,agent_id,attempt_id,generation,audience,expires_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)", params![digest(credential.bearer()), scan, agent, attempt.id, i64::try_from(generation)?, WORKER_AUDIENCE, expires_at_ms])?;
            for e in state.environments.iter_mut().filter(|e| e.agent_id == agent) { e.termination_confirmed = false; e.generation = next(e.generation)?; }
            state.attempts.push(attempt.clone());
            write_scan(tx, &mut state)?;
            Ok((attempt, credential))
        })
    }

    /// Revocation does not assert process termination. Only a trusted supervisor may confirm it.
    pub fn fence_attempt(
        &self,
        scan: &str,
        attempt: &str,
        termination_confirmed: bool,
    ) -> Result<()> {
        self.transaction(|tx| fence_in_transaction(tx, scan, attempt, termination_confirmed))
    }

    pub fn authorize(
        &self,
        bearer: &str,
        audience: &str,
        request: &WorkerRequest,
        now_ms: i64,
    ) -> Result<CallerContext> {
        self.transaction(|tx| authorize(tx, bearer, audience, request, now_ms))
    }

    /// Replays an accepted response before invoking review or a provider again.
    /// Current execution authority is still required; a receipt is not a credential.
    pub fn replay_worker_mutation(
        &self,
        bearer: &str,
        request: &WorkerRequest,
        now_ms: i64,
    ) -> Result<Option<MutationAcknowledgement>> {
        self.transaction(|tx| {
            authorize(tx, bearer, WORKER_AUDIENCE, request, now_ms)?;
            Ok(receipt(
                tx,
                &request.scan_id,
                &request.agent_id,
                &request.operation_id,
                &worker_receipt_content(request)?,
            )?
            .map(|result| MutationAcknowledgement {
                version: VERSION,
                operation_id: request.operation_id.clone(),
                result,
            }))
        })
    }

    /// Worker events cannot claim lifecycle authority; their sequence and
    /// provenance are assigned centrally and committed with the acknowledgement.
    /// Records a worker event. `stored` is the payload the host keeps and
    /// projects, normally `payload` after host-secret redaction. The mutation
    /// receipt still binds the original request, so a replay matches its first
    /// delivery even if the host's secret set changed in between.
    pub fn worker_event(
        &self,
        bearer: &str,
        request: &WorkerRequest,
        stored: &Value,
        now_ms: i64,
    ) -> Result<MutationAcknowledgement> {
        let WorkerOperation::Event { event_id, .. } = &request.operation else {
            anyhow::bail!("not a worker event");
        };
        let payload = stored;
        self.dispatch_mutation(bearer, WORKER_AUDIENCE, request, now_ms, |tx, _| {
            let mut state = read_scan(tx, &request.scan_id)?;
            if let Some(existing) = state.events.iter().find(|event| &event.id == event_id) {
                ensure!(existing.agent_id == request.agent_id && existing.kind == "worker-event"
                    && existing.payload["attempt_id"] == request.attempt_id && existing.payload["generation"] == request.generation
                    && existing.payload["event"] == *payload,
                    "worker event identity changed");
                return Ok(json!({"sequence":existing.sequence}));
            }
            let payload = json!({"attempt_id":request.attempt_id,"generation":request.generation,"observed_at_ms":now_ms,"event":payload});
            let sequence = next(u64::try_from(state.events.len())?)?;
            state.events.push(LifecycleEvent { id: event_id.clone(), agent_id: request.agent_id.clone(), sequence,
                kind: "worker-event".into(), payload });
            write_scan(tx, &mut state)?;
            Ok(json!({"sequence":sequence}))
        })
    }

    pub fn renew(
        &self,
        bearer: &str,
        request: &WorkerRequest,
        now_ms: i64,
        expires_at_ms: i64,
    ) -> Result<()> {
        ensure!(
            now_ms >= 0 && expires_at_ms > now_ms,
            "invalid renewal time"
        );
        self.transaction(|tx| {
            authorize(tx, bearer, WORKER_AUDIENCE, request, now_ms)?;
            let mut state = read_scan(tx, &request.scan_id)?;
            let a = state
                .attempts
                .iter_mut()
                .find(|a| a.id == request.attempt_id)
                .context("unknown attempt")?;
            // A backward wall-clock step can put the previously authorized
            // deadline beyond the normal new-grant horizon. Keeping that exact
            // deadline does not extend authority. Any extension still validates
            // the bounded horizon, and expired/revoked identities already failed
            // authorization above. Worker lease expiry remains monotonic.
            if expires_at_ms != a.expires_at_ms {
                validate_lease(now_ms, expires_at_ms)?;
            }
            ensure!(expires_at_ms >= a.expires_at_ms, "lease cannot regress");
            a.expires_at_ms = expires_at_ms;
            tx.execute(
                "UPDATE swarm_credentials SET expires_at_ms=?2 WHERE digest=?1",
                params![digest(bearer), expires_at_ms],
            )?;
            write_scan(tx, &mut state)
        })
    }

    /// The handler is trusted host code for a synchronous canonical DB mutation.
    /// Never perform network/target execution here or hold this transaction while awaiting review.
    pub fn dispatch_mutation(
        &self,
        bearer: &str,
        audience: &str,
        request: &WorkerRequest,
        now_ms: i64,
        handler: impl FnOnce(&Transaction<'_>, &CallerContext) -> Result<Value>,
    ) -> Result<MutationAcknowledgement> {
        let result = self.transaction(|tx| {
            let caller = authorize(tx, bearer, audience, request, now_ms)?;
            ensure!(
                matches!(
                    request.operation,
                    WorkerOperation::PluginResources { .. }
                        | WorkerOperation::Canonical { .. }
                        | WorkerOperation::Runbook { .. }
                        | WorkerOperation::Verify { .. }
                        | WorkerOperation::Event { .. }
                        | WorkerOperation::Review { .. }
                        | WorkerOperation::ArtifactBegin { .. }
                        | WorkerOperation::ArtifactChunk { .. }
                        | WorkerOperation::ArtifactSeal { .. }
                        | WorkerOperation::ArtifactRead { .. }
                ),
                "operation requires its dedicated dispatcher"
            );
            let content = worker_receipt_content(request)?;
            if let Some(result) = receipt(
                tx,
                &request.scan_id,
                &request.agent_id,
                &request.operation_id,
                &content,
            )? {
                return Ok(result);
            }
            let result = handler(tx, &caller)?;
            acknowledge(
                tx,
                &request.scan_id,
                &request.agent_id,
                &request.operation_id,
                &content,
                &result,
            )?;
            Ok(result)
        })?;
        Ok(MutationAcknowledgement {
            version: VERSION,
            operation_id: request.operation_id.clone(),
            result,
        })
    }

    pub fn journal_operation(
        &self,
        scan: &str,
        operation: ToolOperation,
        expected_revision: Option<u64>,
        execution: &ExecutionWriteContext,
    ) -> Result<()> {
        self.transaction(|tx| {
            let mut state = read_scan(tx, scan)?;
            ensure!(
                operation.attempt_id == execution.attempt_id,
                "tool operation attempt mismatch"
            );
            ensure_execution(&state, &operation.agent_id, execution)?;
            if let Some(old) = state.operations.iter_mut().find(|o| o.id == operation.id) {
                ensure!(
                    expected_revision == Some(old.revision)
                        && operation.revision == next(old.revision)?,
                    "stale operation revision"
                );
                ensure!(
                    old.agent_id == operation.agent_id
                        && old.attempt_id == operation.attempt_id
                        && old.request == operation.request,
                    "operation identity changed"
                );
                ensure!(
                    matches!(
                        (old.state, operation.state),
                        (
                            ToolOperationState::NotStarted,
                            ToolOperationState::Running | ToolOperationState::Canceled
                        ) | (
                            ToolOperationState::Running,
                            ToolOperationState::Completed
                                | ToolOperationState::Canceled
                                | ToolOperationState::OutcomeUnknown
                        )
                    ),
                    "invalid tool operation transition; reconcile uncertain outcomes explicitly"
                );
                *old = operation;
            } else {
                ensure!(
                    expected_revision.is_none()
                        && operation.revision == 1
                        && operation.state == ToolOperationState::NotStarted,
                    "invalid initial journal state"
                );
                state.operations.push(operation);
            }
            write_scan(tx, &mut state)
        })
    }

    /// Resolve only an existing operation whose original attempt is confirmed stopped.
    /// This never dispatches work, alters assignments, or restores worker authority.
    /// Known outcomes require host evidence; termination alone only justifies uncertainty.
    pub fn recover_operation(
        &self,
        scan: &str,
        recovery_id: &str,
        operation_id: &str,
        expected_revision: u64,
        recovery: JournalRecovery,
    ) -> Result<ToolOperation> {
        ensure!(
            valid_id(recovery_id) && valid_id(operation_id),
            "invalid recovery identity"
        );
        let (target, result) = match &recovery {
            JournalRecovery::OutcomeUnknown => (ToolOperationState::OutcomeUnknown, Value::Null),
            JournalRecovery::Canceled { evidence_refs } => {
                validate_recovery_evidence(evidence_refs)?;
                (ToolOperationState::Canceled, Value::Null)
            }
            JournalRecovery::Completed {
                result,
                evidence_refs,
            } => {
                validate_recovery_evidence(evidence_refs)?;
                ensure!(bounded_json(result), "recovered result too large");
                (ToolOperationState::Completed, result.clone())
            }
        };
        let content = json!({"kind":"recover_tool_operation", "operation_id":operation_id, "expected_revision":expected_revision, "recovery":recovery});
        ensure!(bounded_json(&content), "recovery request too large");
        self.transaction(|tx| {
            let mut state = read_scan(tx, scan)?;
            if let Some(saved) = receipt(tx, scan, "host-journal-recovery", recovery_id, &content)? {
                return Ok(serde_json::from_value(saved)?);
            }
            let operation = state.operations.iter_mut().find(|o| o.id == operation_id)
                .context("unknown tool operation")?;
            ensure!(operation.revision == expected_revision, "stale operation revision");
            ensure!(state.attempts.iter().any(|a| a.id == operation.attempt_id && a.agent_id == operation.agent_id && a.state == ExecutionState::Stopped),
                "recovery requires confirmed execution termination");
            ensure!(matches!((operation.state, target),
                (ToolOperationState::NotStarted, ToolOperationState::Canceled) |
                (ToolOperationState::Running, ToolOperationState::OutcomeUnknown | ToolOperationState::Completed | ToolOperationState::Canceled) |
                (ToolOperationState::OutcomeUnknown, ToolOperationState::Completed | ToolOperationState::Canceled)),
                "invalid recovery transition; known outcomes are immutable");
            operation.state = target;
            operation.result = result;
            operation.revision = next(operation.revision)?;
            let recovered = operation.clone();
            state.events.push(LifecycleEvent {
                id: recovery_id.to_owned(), agent_id: recovered.agent_id.clone(),
                sequence: next(u64::try_from(state.events.len())?)?, kind: "tool-outcome-recovered".into(),
                payload: json!({"operation_id":operation_id, "attempt_id":recovered.attempt_id, "recovery":recovery}),
            });
            write_scan(tx, &mut state)?;
            acknowledge(tx, scan, "host-journal-recovery", recovery_id, &content, &serde_json::to_value(&recovered)?)?;
            Ok(recovered)
        })
    }
}

fn worker_receipt_content(request: &WorkerRequest) -> Result<Value> {
    if matches!(
        request.operation,
        WorkerOperation::Runbook { .. }
            | WorkerOperation::Verify { .. }
            | WorkerOperation::Plugin { .. }
            | WorkerOperation::PluginResources { .. }
    ) {
        // A prepared execution must never be rebound to a replacement attempt.
        Ok(serde_json::to_value(request)?)
    } else {
        Ok(serde_json::to_value(&request.operation)?)
    }
}

fn validate_recovery_evidence(evidence_refs: &[String]) -> Result<()> {
    ensure!(
        !evidence_refs.is_empty()
            && evidence_refs.len() <= 256
            && unique(evidence_refs.iter().map(String::as_str)),
        "known recovery outcome requires bounded, distinct evidence references"
    );
    Ok(())
}

fn ensure_execution(
    state: &ScanState,
    agent: &str,
    execution: &ExecutionWriteContext,
) -> Result<()> {
    ensure!(state.ended_at_ms.is_none(), "scan has ended");
    ensure!(
        execution.now_ms >= 0
            && state.attempts.iter().any(|a| a.agent_id == agent
                && a.id == execution.attempt_id
                && a.generation == execution.generation
                && a.state == ExecutionState::Running
                && a.expires_at_ms > execution.now_ms),
        "stale or expired execution write"
    );
    Ok(())
}

fn validate_lease(now: i64, expiry: i64) -> Result<()> {
    ensure!(
        now >= 0 && expiry > now && expiry.checked_sub(now).is_some_and(|v| v <= 15 * 60 * 1000),
        "lease must expire within fifteen minutes"
    );
    Ok(())
}

fn authorize(
    tx: &Transaction<'_>,
    bearer: &str,
    audience: &str,
    r: &WorkerRequest,
    now: i64,
) -> Result<CallerContext> {
    checked(r.validate())?;
    ensure!(
        audience == WORKER_AUDIENCE && now >= 0 && bearer.len() == 64,
        "invalid worker credential"
    );
    let valid: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM swarm_credentials WHERE digest=?1 AND scan_id=?2 AND agent_id=?3 AND attempt_id=?4 AND generation=?5 AND audience=?6 AND expires_at_ms>?7 AND revoked=0)", params![digest(bearer), r.scan_id, r.agent_id, r.attempt_id, i64::try_from(r.generation)?, audience, now], |row| row.get(0))?;
    ensure!(valid, "invalid or expired worker credential");
    let inference: Option<String> = tx.query_row(
        "SELECT request FROM swarm_inference WHERE scan_id=?1 AND agent_id=?2 AND operation_id=?3",
        params![r.scan_id, r.agent_id, r.operation_id], |row| row.get(0),
    ).optional()?;
    if let Some(inference) = inference {
        ensure!(
            serde_json::from_str::<WorkerRequest>(&inference)? == *r,
            "inference operation belongs to a different request or execution"
        );
    }
    let plugin: Option<String> = tx.query_row("SELECT request FROM swarm_plugin_calls WHERE scan_id=?1 AND agent_id=?2 AND operation_id=?3", params![r.scan_id,r.agent_id,r.operation_id], |row|row.get(0)).optional()?;
    if let Some(plugin) = plugin {
        ensure!(
            serde_json::from_str::<WorkerRequest>(&plugin)? == *r,
            "plugin operation belongs to a different request or execution"
        );
    }
    ensure_worker_branch_open(tx, &r.scan_id, &r.agent_id)?;
    let state = read_scan(tx, &r.scan_id)?;
    ensure!(state.ended_at_ms.is_none(), "scan has ended");
    ensure!(
        state.config.workspace_id == r.workspace_id,
        "wrong workspace"
    );
    ensure!(
        state.attempts.iter().any(|a| a.id == r.attempt_id
            && a.agent_id == r.agent_id
            && a.generation == r.generation
            && a.state == ExecutionState::Running
            && a.expires_at_ms > now),
        "stale execution attempt"
    );
    let agent = &state.agent(&r.agent_id)?.summary;
    // Prepared execution narrows the saved scan tools and selects its native
    // command implementation. Requests cannot supply or alter this capability.
    let prepared: Option<String> = tx
        .query_row(
            "SELECT bootstrap FROM swarm_worker_runtime WHERE attempt_id=?1",
            [&r.attempt_id],
            |row| row.get(0),
        )
        .optional()?;
    let enabled_tools = match prepared {
        Some(raw) => {
            serde_json::from_str::<beale_contracts::swarm_worker::WorkerBootstrap>(&raw)?
                .enabled_tools
        }
        None => state.config.enabled_tools.clone(),
    };
    if matches!(r.operation, WorkerOperation::PluginResources { .. }) {
        ensure!(
            enabled_tools
                .iter()
                .any(|name| matches!(name.as_str(), "read" | "plugin_call")),
            "plugin resources unavailable"
        );
    }
    let tool = match &r.operation {
        WorkerOperation::Review { call } => Some(call.name.as_str()),
        WorkerOperation::Canonical { tool, .. } => Some(tool.as_str()),
        WorkerOperation::Runbook { .. } => Some("runbook_run"),
        WorkerOperation::Verify { .. } => Some("findings_verify"),
        WorkerOperation::Plugin { .. } => Some("plugin_call"),
        WorkerOperation::Spawn { .. } => Some("swarm_spawn_subagent"),
        WorkerOperation::MessageParent { .. } => Some("swarm_message_parent"),
        WorkerOperation::MessageChild { .. } => Some("swarm_message_subagent"),
        WorkerOperation::StopChild { .. } => Some("swarm_stop_subagent"),
        _ => None,
    };
    if let Some(tool) = tool {
        ensure!(
            enabled_tools.iter().any(|enabled| enabled == tool),
            "tool unavailable"
        );
    }
    match &r.operation {
        WorkerOperation::Bootstrap {}
        | WorkerOperation::Review { .. }
        | WorkerOperation::Checkpoint { .. }
        | WorkerOperation::Finish { .. } => (),
        WorkerOperation::Canonical { .. }
        | WorkerOperation::Runbook { .. }
        | WorkerOperation::Verify { .. }
        | WorkerOperation::Plugin { .. }
        | WorkerOperation::PluginResources { .. } => (),
        WorkerOperation::Infer { route_id, .. } => ensure!(
            state
                .config
                .provider_routes
                .iter()
                .any(|p| &p.id == route_id),
            "provider route unavailable"
        ),
        WorkerOperation::MessageParent { .. } => {
            ensure!(agent.parent_id.is_some(), "leader has no parent")
        }
        WorkerOperation::MessageChild { child_id, .. }
        | WorkerOperation::StopChild { child_id } => ensure!(
            state.agent(child_id)?.summary.parent_id.as_ref() == Some(&r.agent_id),
            "not a direct child"
        ),
        WorkerOperation::Spawn { profile, .. } => ensure!(
            state
                .config
                .profile(profile)
                .is_some_and(|p| agent.role.child() == Some(p.role))
                && state.workflow()?.permits(&agent.profile.id, &profile.id),
            "invalid child profile"
        ),
        WorkerOperation::SnapshotRead { .. }
        | WorkerOperation::Heartbeat {}
        | WorkerOperation::Event { .. }
        | WorkerOperation::ArtifactBegin { .. }
        | WorkerOperation::ArtifactSeal { .. }
        | WorkerOperation::ArtifactRead { .. }
        | WorkerOperation::ArtifactChunk { .. } => (),
    }
    let rule = match &state.config.mode {
        ScanMode::Swarm { snapshot } if agent.role == Role::Leader => {
            Some(snapshot.leader_rule.clone())
        }
        _ => None,
    };
    Ok(CallerContext {
        agent: agent.clone(),
        authorized_scope: state.config.authorized_scope,
        enabled_tools,
        provider_routes: state.config.provider_routes,
        review_rule: rule,
    })
}

fn ensure_worker_branch_open(db: &Connection, scan: &str, agent: &str) -> Result<()> {
    let stopped: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM swarm_local, json_each(swarm_local.document,'$.controls') AS control WHERE scan_id=?1 AND json_extract(control.value,'$.agent_id')=?2 AND json_extract(control.value,'$.stop_requested')=1)",
        params![scan, agent], |r| r.get(0),
    )?;
    ensure!(!stopped, "worker branch has been intentionally stopped");
    Ok(())
}

#[derive(Clone, Debug)]
pub enum EnvironmentChange {
    Resume,
    Release,
    Keep(bool),
    Publish(String),
    AcknowledgePublication(String),
    ClaimCleanup,
    ConfirmDeleted,
}

impl SwarmStore {
    pub fn end_scan(&self, scan: &str, now_ms: i64) -> Result<()> {
        ensure!(now_ms >= 0, "invalid scan end time");
        self.transaction(|tx| {
            let mut state = read_scan(tx, scan)?;
            end_scan_in_transaction(tx, &mut state, now_ms)
        })
    }

    /// Clears the old end clock atomically; does not allocate an instance.
    pub fn reopen_scan(&self, scan: &str, supported: bool) -> Result<()> {
        self.transaction(|tx| {
            let mut state = read_scan(tx, scan)?;
            checked(state.config.validate_resume(supported))?;
            ensure!(
                state
                    .environments
                    .iter()
                    .all(|e| e.deletion == DeletionState::Retained),
                "environment deletion in progress or complete"
            );
            state.ended_at_ms = None;
            for e in &mut state.environments {
                e.retention_deadline_ms = None;
                e.generation = next(e.generation)?;
            }
            write_scan(tx, &mut state)
        })
    }

    pub fn change_environment(
        &self,
        scan: &str,
        id: &str,
        generation: u64,
        now_ms: i64,
        change: EnvironmentChange,
    ) -> Result<EnvironmentState> {
        ensure!(now_ms >= 0, "invalid lifecycle time");
        self.transaction(|tx| {
            let mut state = read_scan(tx, scan)?;
            let ended = state.ended_at_ms.is_some();
            if matches!(change, EnvironmentChange::Resume) {
                ensure!(
                    state
                        .environments
                        .iter()
                        .all(|e| e.deletion == DeletionState::Retained),
                    "environment deletion in progress or complete"
                );
                state.ended_at_ms = None;
                for other in &mut state.environments {
                    other.retention_deadline_ms = None;
                    if other.id != id {
                        other.generation = next(other.generation)?;
                    }
                }
            }
            let e = state
                .environments
                .iter_mut()
                .find(|e| e.id == id)
                .context("unknown environment")?;
            ensure!(e.generation == generation, "stale lifecycle generation");
            if matches!(change, EnvironmentChange::ConfirmDeleted) {
                ensure!(e.deletion == DeletionState::Claimed, "deletion not claimed");
                e.deletion = DeletionState::Deleted;
            } else {
                ensure!(
                    e.deletion == DeletionState::Retained,
                    "deletion in progress or complete"
                );
                match change {
                    EnvironmentChange::Resume => {
                        e.active_references = e
                            .active_references
                            .checked_add(1)
                            .context("reference overflow")?;
                    }
                    EnvironmentChange::Release => {
                        e.active_references = e
                            .active_references
                            .checked_sub(1)
                            .context("no active reference")?;
                    }
                    EnvironmentChange::Keep(keep) => e.keep = keep,
                    EnvironmentChange::Publish(id) => {
                        ensure!(
                            valid_id(&id) && !e.pending_publications.contains(&id),
                            "invalid publication"
                        );
                        e.pending_publications.push(id);
                    }
                    EnvironmentChange::AcknowledgePublication(id) => {
                        ensure!(e.pending_publications.contains(&id), "unknown publication");
                        e.pending_publications.retain(|p| p != &id);
                    }
                    EnvironmentChange::ClaimCleanup => {
                        ensure!(
                            ended
                                && e.retention_deadline_ms.is_some_and(|d| d <= now_ms)
                                && !e.keep
                                && e.active_references == 0
                                && e.pending_publications.is_empty()
                                && e.termination_confirmed,
                            "environment protected"
                        );
                        e.deletion = DeletionState::Claimed;
                    }
                    EnvironmentChange::ConfirmDeleted => anyhow::bail!("deletion must be claimed"),
                }
            }
            e.generation = next(e.generation)?;
            let result = e.clone();
            write_scan(tx, &mut state)?;
            Ok(result)
        })
    }
}

fn end_scan_in_transaction(tx: &Transaction<'_>, state: &mut ScanState, now_ms: i64) -> Result<()> {
    if state.ended_at_ms.is_some() {
        return Ok(());
    }
    let deadline = retention_deadline(state, now_ms)?;
    state.ended_at_ms = Some(now_ms);
    for handoff in &mut state.handoffs {
        if !matches!(
            handoff.status,
            HandoffStatus::Completed | HandoffStatus::Canceled
        ) {
            handoff.status = HandoffStatus::Canceled;
            handoff.revision = next(handoff.revision)?;
        }
    }
    // Revoke API authority atomically, without claiming managed processes stopped.
    for attempt in &mut state.attempts {
        if attempt.state != ExecutionState::Stopped {
            attempt.state = ExecutionState::TerminationUnconfirmed;
        }
    }
    for instance in &mut state.instances {
        if instance.state != InstanceState::Stopped {
            instance.state = InstanceState::TerminationUnconfirmed;
        }
    }
    tx.execute(
        "UPDATE swarm_credentials SET revoked=1 WHERE scan_id=?1",
        [&state.config.scan_id],
    )?;
    for e in &mut state.environments {
        e.retention_deadline_ms = Some(deadline);
        e.generation = next(e.generation)?;
    }
    write_scan(tx, state)
}

fn retention_deadline(state: &ScanState, ended_at_ms: i64) -> Result<i64> {
    ended_at_ms
        .checked_add(
            i64::try_from(state.config.retention.ended_scan_seconds)?
                .checked_mul(1000)
                .context("retention overflow")?,
        )
        .context("retention overflow")
}

#[derive(Clone, Debug, Serialize)]
pub struct HandoffChange {
    pub id: String,
    pub expected_revision: u64,
    pub status: HandoffStatus,
    pub assignment_context: Option<String>,
    pub result: Option<WorkResult>,
}
impl ScanState {
    fn workflow(&self) -> Result<WorkflowDefinition> {
        match &self.config.mode {
            ScanMode::Swarm { snapshot } => snapshot.workflow().map_err(anyhow::Error::msg),
            ScanMode::Local {} => anyhow::bail!("local scan has no workflow"),
        }
    }
    /// Ordered immediate parents on the way to a deputy destination. The leader
    /// resolves the destination instance later; a profile is never a sibling address.
    pub fn handoff_path(&self, request: &HandoffRequest) -> Result<Vec<String>> {
        checked(request.validate())?;
        let source = &self.agent(&request.from_agent)?.summary;
        ensure!(
            source.parent_id.as_ref() == Some(&request.parent_id),
            "handoff bypasses parent"
        );
        let parent = &self.agent(&request.parent_id)?.summary;
        let (deputy, path) = match source.role {
            Role::Deputy => (source, vec![parent.id.clone()]),
            Role::Worker => (
                parent,
                vec![
                    parent.id.clone(),
                    parent.parent_id.clone().context("deputy has no leader")?,
                ],
            ),
            Role::Leader => anyhow::bail!("leaders assign work rather than submit handoffs"),
        };
        let workflow = self.workflow()?;
        let route = workflow
            .routes
            .iter()
            .find(|r| r.id == request.route_id)
            .context(beale_contracts::tool_error::ToolError::missing(
                "unknown handoff route",
            ))?;
        ensure!(
            route.from == deputy.profile.id
                && route.to == request.destination_profile.id
                && route.payload == request.payload.kind(),
            beale_contracts::tool_error::ToolError::rejected(
                "handoff violates stage route or payload"
            )
        );
        ensure!(
            self.config
                .profile(&request.destination_profile)
                .is_some_and(|p| p.role == Role::Deputy),
            beale_contracts::tool_error::ToolError::missing("unknown destination profile")
        );
        Ok(path)
    }
}

fn fence_in_transaction(
    tx: &Transaction<'_>,
    scan: &str,
    attempt: &str,
    termination_confirmed: bool,
) -> Result<()> {
    let mut state = read_scan(tx, scan)?;
    let a = state
        .attempts
        .iter_mut()
        .find(|a| a.id == attempt)
        .context("unknown attempt")?;
    let target = if termination_confirmed || a.state == ExecutionState::Stopped {
        ExecutionState::Stopped
    } else {
        ExecutionState::TerminationUnconfirmed
    };
    let mut changed = a.state != target;
    a.state = target;
    let agent = a.agent_id.clone();
    for instance in state
        .instances
        .iter_mut()
        .filter(|i| i.attempt_id == attempt)
    {
        let target = if a.state == ExecutionState::Stopped {
            InstanceState::Stopped
        } else {
            InstanceState::TerminationUnconfirmed
        };
        changed |= instance.state != target;
        instance.state = target;
    }
    changed |= tx.execute(
        "UPDATE swarm_credentials SET revoked=1 WHERE scan_id=?1 AND attempt_id=?2 AND revoked=0",
        params![scan, attempt],
    )? > 0;
    if termination_confirmed
        && state
            .attempts
            .iter()
            .filter(|a| a.agent_id == agent)
            .all(|a| a.state == ExecutionState::Stopped)
    {
        for e in state
            .environments
            .iter_mut()
            .filter(|e| e.agent_id == agent && !e.termination_confirmed)
        {
            e.termination_confirmed = true;
            e.generation = next(e.generation)?;
            changed = true;
        }
    }
    if changed {
        write_scan(tx, &mut state)?;
    }
    Ok(())
}
