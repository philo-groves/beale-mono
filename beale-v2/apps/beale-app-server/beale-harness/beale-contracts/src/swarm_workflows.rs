//! Declarative workflow contracts. Routes describe parent-mediated work, never execution.
use crate::swarms::{DefinitionSnapshot, ProfileRef, RecordRevision, Role, Task, valid_id};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const WORKFLOW_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDefinition {
    pub version: u32,
    pub id: String,
    pub revision: u32,
    pub name: String,
    pub description: String,
    pub nodes: Vec<WorkflowNode>,
    pub delegation: Vec<Delegation>,
    pub initial_deputies: Vec<String>,
    pub routes: Vec<HandoffRoute>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowNode {
    pub profile: ProfileRef,
    pub role: Role,
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delegation {
    pub parent: String,
    pub children: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PayloadKind {
    Task,
    Candidate,
    Assessment,
    RankedTargets,
    SupportedFinding,
    Remediation,
    Patch,
    PatchAssessment,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffRoute {
    pub id: String,
    pub from: String,
    pub to: String,
    pub payload: PayloadKind,
}

fn text(s: &str, max: usize) -> bool {
    !s.trim().is_empty() && s.len() <= max
}
fn ids(v: &[String]) -> bool {
    v.len() <= 256
        && v.iter().all(|s| valid_id(s))
        && v.iter().collect::<HashSet<_>>().len() == v.len()
}
fn texts(v: &[String]) -> bool {
    v.len() <= 32 && v.iter().all(|s| text(s, 4096))
}

impl WorkflowDefinition {
    pub fn decode(raw: &str) -> Result<Self, String> {
        if raw.len() > 65536 {
            return Err("swarm definition too large".into());
        }
        let definition: Self = serde_json::from_str(raw).map_err(|_| "invalid swarm definition")?;
        definition.validate().map_err(str::to_owned)?;
        Ok(definition)
    }
    pub fn node(&self, id: &str) -> Option<&WorkflowNode> {
        self.nodes.iter().find(|n| n.profile.id == id)
    }
    pub fn permits(&self, parent: &str, child: &str) -> bool {
        self.delegation
            .iter()
            .any(|d| d.parent == parent && d.children.iter().any(|c| c == child))
    }
    pub fn leader(&self) -> Option<&WorkflowNode> {
        self.nodes.iter().find(|n| n.role == Role::Leader)
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != WORKFLOW_VERSION
            || !valid_id(&self.id)
            || self.revision == 0
            || !text(&self.name, 128)
            || !text(&self.description, 4096)
            || self.nodes.len() > 64
            || self.nodes.iter().filter(|n| n.role == Role::Leader).count() != 1
            || !self.nodes.iter().any(|n| n.role == Role::Deputy)
            || !self.nodes.iter().any(|n| n.role == Role::Worker)
            || self.nodes.iter().any(|n| {
                !valid_id(&n.profile.id)
                    || n.profile.revision == 0
                    || n.x.abs_diff(0) > 10000
                    || n.y.abs_diff(0) > 10000
            })
            || self
                .nodes
                .iter()
                .map(|n| &n.profile.id)
                .collect::<HashSet<_>>()
                .len()
                != self.nodes.len()
        {
            return Err("invalid swarm metadata or roles");
        }
        if self.delegation.len() > 64
            || self
                .delegation
                .iter()
                .map(|d| &d.parent)
                .collect::<HashSet<_>>()
                .len()
                != self.delegation.len()
        {
            return Err("duplicate delegation parent");
        }
        for d in &self.delegation {
            let parent = self.node(&d.parent).ok_or("unknown delegation parent")?;
            if d.children.is_empty()
                || !ids(&d.children)
                || d.children.iter().any(|c| {
                    self.node(c)
                        .is_none_or(|n| parent.role.child() != Some(n.role))
                })
            {
                return Err("delegation must follow leader/deputy/worker hierarchy");
            }
        }
        if self.nodes.iter().any(|n| {
            n.role != Role::Leader
                && !self
                    .delegation
                    .iter()
                    .any(|d| d.children.contains(&n.profile.id))
        }) {
            return Err("unreachable swarm profile");
        }
        if !ids(&self.initial_deputies)
            || self
                .initial_deputies
                .iter()
                .any(|id| self.node(id).is_none_or(|n| n.role != Role::Deputy))
        {
            return Err("invalid initial deputies");
        }
        if self.routes.len() > 256
            || self
                .routes
                .iter()
                .map(|r| &r.id)
                .collect::<HashSet<_>>()
                .len()
                != self.routes.len()
        {
            return Err("duplicate handoff route");
        }
        for route in &self.routes {
            if !valid_id(&route.id)
                || [route.from.as_str(), route.to.as_str()]
                    .iter()
                    .any(|id| self.node(id).is_none_or(|n| n.role != Role::Deputy))
            {
                return Err("workflow routes must connect deputies through their leader");
            }
        }
        Ok(())
    }
}

impl DefinitionSnapshot {
    pub fn workflow(&self) -> Result<WorkflowDefinition, &'static str> {
        let d: WorkflowDefinition = serde_json::from_value(self.definition.clone())
            .map_err(|_| "invalid workflow snapshot")?;
        d.validate()?;
        if d.id != self.id
            || d.revision != self.revision
            || d.nodes.len() != self.profiles.len()
            || d.nodes.iter().any(|n| {
                !self
                    .profiles
                    .iter()
                    .any(|p| p.profile == n.profile && p.role == n.role)
            })
        {
            return Err("workflow and profile snapshots disagree");
        }
        Ok(d)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Assessment {
    Supported,
    Contradicted,
    Inconclusive,
    NotApplicable,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Completion {
    Complete,
    Incomplete,
    Blocked,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkResult {
    pub completion: Completion,
    pub assessment: Assessment,
    pub summary: String,
    pub observations: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub artifact_refs: Vec<String>,
    pub unresolved_questions: Vec<String>,
}
impl WorkResult {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !text(&self.summary, 4096)
            || !texts(&self.observations)
            || !texts(&self.unresolved_questions)
            || !ids(&self.evidence_refs)
            || !ids(&self.artifact_refs)
            || (matches!(
                self.assessment,
                Assessment::Supported | Assessment::Contradicted
            ) && self.evidence_refs.is_empty())
        {
            return Err("invalid result or missing evidence");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RankedTarget {
    pub rank: u32,
    pub target: String,
    pub rationale: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HandoffPayload {
    Task {},
    Candidate {
        claim: String,
    },
    Assessment {
        result: WorkResult,
    },
    RankedTargets {
        targets: Vec<RankedTarget>,
    },
    SupportedFinding {
        result: WorkResult,
    },
    Remediation {
        finding_refs: Vec<String>,
    },
    Patch {
        patch_refs: Vec<String>,
        test_refs: Vec<String>,
    },
    PatchAssessment {
        patch_refs: Vec<String>,
        test_refs: Vec<String>,
        result: WorkResult,
    },
}
impl HandoffPayload {
    pub fn kind(&self) -> PayloadKind {
        match self {
            Self::Task {} => PayloadKind::Task,
            Self::Candidate { .. } => PayloadKind::Candidate,
            Self::Assessment { .. } => PayloadKind::Assessment,
            Self::RankedTargets { .. } => PayloadKind::RankedTargets,
            Self::SupportedFinding { .. } => PayloadKind::SupportedFinding,
            Self::Remediation { .. } => PayloadKind::Remediation,
            Self::Patch { .. } => PayloadKind::Patch,
            Self::PatchAssessment { .. } => PayloadKind::PatchAssessment,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffRequest {
    pub id: String,
    pub from_agent: String,
    pub parent_id: String,
    pub route_id: String,
    pub destination_profile: ProfileRef,
    pub task: Task,
    pub scope: Vec<String>,
    pub record_revisions: Vec<RecordRevision>,
    pub artifact_refs: Vec<String>,
    pub observations: Vec<String>,
    pub unresolved_questions: Vec<String>,
    pub payload: HandoffPayload,
}
impl HandoffRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.task.validate()?;
        if ![
            &self.id,
            &self.from_agent,
            &self.parent_id,
            &self.route_id,
            &self.destination_profile.id,
        ]
        .iter()
        .all(|v| valid_id(v))
            || self.destination_profile.revision == 0
            || self.scope.is_empty()
            || !texts(&self.scope)
            || !texts(&self.observations)
            || !texts(&self.unresolved_questions)
            || !ids(&self.artifact_refs)
            || self.record_revisions.len() > 256
            || self
                .record_revisions
                .iter()
                .any(|r| !valid_id(&r.id) || r.revision == 0)
            || self
                .record_revisions
                .iter()
                .map(|r| &r.id)
                .collect::<HashSet<_>>()
                .len()
                != self.record_revisions.len()
            || serde_json::to_vec(self).map_or(true, |b| b.len() > 65536)
        {
            return Err("invalid handoff envelope");
        }
        match &self.payload {
            HandoffPayload::Task {} => {}
            HandoffPayload::Candidate { claim } => {
                if !text(claim, 4096) || self.task.evidence_refs.is_empty() {
                    return Err("candidate needs a labeled claim and evidence");
                }
            }
            HandoffPayload::Assessment { result } => result.validate()?,
            HandoffPayload::SupportedFinding { result } => {
                result.validate()?;
                if result.assessment != Assessment::Supported {
                    return Err("reporting requires a supported assessment");
                }
            }
            HandoffPayload::RankedTargets { targets } => {
                if targets.is_empty()
                    || targets.len() > 64
                    || targets.iter().enumerate().any(|(i, t)| {
                        t.rank as usize != i + 1
                            || !text(&t.target, 1024)
                            || !text(&t.rationale, 4096)
                    })
                    || targets
                        .iter()
                        .map(|t| &t.target)
                        .collect::<HashSet<_>>()
                        .len()
                        != targets.len()
                {
                    return Err("targets must have distinct ordered ranks");
                }
            }
            HandoffPayload::Remediation { finding_refs } => {
                if finding_refs.is_empty() || !ids(finding_refs) {
                    return Err("remediation needs finding references");
                }
            }
            HandoffPayload::Patch {
                patch_refs,
                test_refs,
            }
            | HandoffPayload::PatchAssessment {
                patch_refs,
                test_refs,
                ..
            } => {
                if patch_refs.is_empty()
                    || test_refs.is_empty()
                    || !ids(patch_refs)
                    || !ids(test_refs)
                {
                    return Err("patch handoff needs patch and test references");
                }
                if let HandoffPayload::PatchAssessment { result, .. } = &self.payload {
                    result.validate()?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffStatus {
    Pending,
    Assigned,
    Completed,
    Blocked,
    Canceled,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowHandoff {
    pub request: HandoffRequest,
    pub revision: u64,
    pub status: HandoffStatus,
    pub assignment_context: Option<String>,
    pub result: Option<WorkResult>,
}
