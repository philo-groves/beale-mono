//! Versioned JSON contract for the optional Findings feature.

use serde::{Deserialize, Serialize};

pub const QUERY_PATH: &str = "/v1/findings/query";
pub const GET_PATH: &str = "/v1/findings/get";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Candidate,
    Validated,
    Screened,
    Reported,
    Rejected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    ToolCall,
    Artifact,
    Surface,
    Document,
    Url,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRef {
    pub kind: EvidenceKind,
    pub reference: String,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Completion {
    pub at_ms: u64,
    pub session_id: String,
    pub predecessor_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub hypothesis: String,
    #[serde(default)]
    pub evidence_refs: Option<Vec<EvidenceRef>>,
    pub revision: u64,
    pub completed_at_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Validated {
    pub attacker_control: String,
    pub violated_invariant: String,
    pub observed_behavior: String,
    pub impact: String,
    #[serde(default)]
    pub validation_method: String,
    #[serde(default)]
    pub validation_description: String,
    #[serde(default)]
    pub validation_result: String,
    #[serde(default)]
    pub validation_limitations: Vec<String>,
    #[serde(default)]
    pub counter_evidence: Vec<EvidenceRef>,
    #[serde(default)]
    pub evidence_refs: Option<Vec<EvidenceRef>>,
    pub revision: u64,
    pub completion: Option<Completion>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreeningCheck {
    pub passed: bool,
    pub method: String,
    pub conclusion: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Screened {
    pub validation_review: ScreeningCheck,
    pub known_issue_review: ScreeningCheck,
    pub scope_review: ScreeningCheck,
    #[serde(default)]
    pub evidence_refs: Option<Vec<EvidenceRef>>,
    pub revision: u64,
    pub completion: Option<Completion>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reported {
    pub poc_steps: String,
    pub poc_result: String,
    pub poc_portability_notes: String,
    pub poc_artifact_refs: Vec<String>,
    pub report_reference: String,
    #[serde(default)]
    pub evidence_refs: Option<Vec<EvidenceRef>>,
    pub revision: u64,
    pub completion: Option<Completion>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rejected {
    pub reason: String,
    pub from: Status,
    pub at_ms: u64,
    pub session_id: String,
    #[serde(default)]
    pub evidence_refs: Option<Vec<EvidenceRef>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindingState {
    pub candidate: Candidate,
    pub validated: Option<Validated>,
    pub screened: Option<Screened>,
    pub reported: Option<Reported>,
    pub rejected: Option<Rejected>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindingRootCause {
    pub summary: String,
    pub evidence_refs: Vec<EvidenceRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindingReachability {
    pub summary: String,
    pub attacker_role: String,
    pub entrypoint: String,
    pub source: String,
    pub sink: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindingAttackPath {
    pub summary: String,
    pub assumptions: Vec<String>,
    pub evidence_refs: Vec<EvidenceRef>,
    pub limitations: Vec<String>,
    pub preconditions: Vec<String>,
    pub likelihood_description: String,
    pub impact_description: String,
    pub steps: Vec<String>,
    pub reachability: FindingReachability,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revision {
    pub number: u64,
    pub at_ms: u64,
    pub session_id: String,
    pub reason: String,
    pub before_json: String,
    pub after_json: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    pub summary: String,
    pub surface_ids: Vec<String>,
    #[serde(default)]
    pub root_cause: Option<FindingRootCause>,
    #[serde(default)]
    pub attack_path: Option<FindingAttackPath>,
    #[serde(default)]
    pub remediation: Option<String>,
    pub state: FindingState,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub revision: u64,
    pub history: Vec<Revision>,
}

impl Finding {
    pub fn status(&self) -> Status {
        if self.state.rejected.is_some() {
            return Status::Rejected;
        }
        let Some(validated) = &self.state.validated else {
            return Status::Candidate;
        };
        let Some(validation) = &validated.completion else {
            return Status::Candidate;
        };
        if validation.predecessor_revision != self.state.candidate.revision {
            return Status::Candidate;
        }
        let Some(screened) = &self.state.screened else {
            return Status::Validated;
        };
        let Some(screening) = &screened.completion else {
            return Status::Validated;
        };
        if screening.predecessor_revision != validated.revision {
            return Status::Validated;
        }
        let Some(reported) = &self.state.reported else {
            return Status::Screened;
        };
        let Some(report) = &reported.completion else {
            return Status::Screened;
        };
        if report.predecessor_revision != screened.revision {
            Status::Screened
        } else {
            Status::Reported
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryRequest {
    pub workspace_id: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub status: Option<Status>,
    #[serde(default)]
    pub surface_ids: Vec<String>,
    #[serde(default)]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueryPage {
    pub records: Vec<Summary>,
    pub total: usize,
    pub next_offset: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetRequest {
    pub workspace_id: String,
    pub id: String,
    #[serde(default)]
    pub history_offset: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetResponse {
    pub record: Finding,
    pub history_total: usize,
    pub next_history_offset: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Summary {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub surface_ids: Vec<String>,
    pub status: Status,
    pub updated_at_ms: u64,
    pub revision: u64,
}

impl From<&Finding> for Summary {
    fn from(value: &Finding) -> Self {
        Self {
            id: value.id.clone(),
            title: value.title.clone(),
            summary: value.summary.clone(),
            surface_ids: value.surface_ids.clone(),
            status: value.status(),
            updated_at_ms: value.updated_at_ms,
            revision: value.revision,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Validated;
    use serde_json::json;

    #[test]
    fn older_validation_records_default_new_details() {
        let record: Validated = serde_json::from_value(json!({
            "attacker_control":"Example input",
            "violated_invariant":"Example boundary",
            "observed_behavior":"Synthetic response",
            "impact":"Example impact",
            "revision":2,
            "completion":null
        }))
        .unwrap();
        assert!(record.validation_method.is_empty());
        assert!(record.validation_description.is_empty());
        assert!(record.validation_result.is_empty());
        assert!(record.validation_limitations.is_empty());
        assert!(record.counter_evidence.is_empty());
    }
}
