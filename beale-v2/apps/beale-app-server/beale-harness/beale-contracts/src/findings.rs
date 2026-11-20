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

/// SHA-256 of a tool result cited when validation completed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceDigest {
    pub reference: String,
    pub sha256: String,
}

/// A deterministic check the host applies to a proof command's result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Oracle {
    /// The command exits with exactly this code.
    ExitCode {
        value: i32,
    },
    /// The command exits unsuccessfully or is ended by a signal.
    ExitNonzero,
    StdoutContains {
        value: String,
    },
    StderrContains {
        value: String,
    },
    /// Standard output or standard error contains the text.
    OutputContains {
        value: String,
    },
}

/// One fresh-process execution of a verification command.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationRun {
    pub exit_code: Option<i32>,
    pub satisfied: bool,
    /// SHA-256 of the redacted stdout and stderr transcript.
    pub output_sha256: String,
    pub duration_ms: i64,
    /// Why the command could not complete, such as a timeout.
    #[serde(default)]
    pub error: Option<String>,
}

/// A host-run reproduction of a declared proof command. `verified` is true
/// only when every run satisfied the oracle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verification {
    pub command: String,
    pub oracle: Oracle,
    pub runs: Vec<VerificationRun>,
    pub verified: bool,
    pub at_ms: u64,
    pub session_id: String,
    /// Stored role of the agent that requested verification.
    pub agent_role: String,
    /// Tool call that ran the verification; citable as validation evidence when verified.
    pub tool_call_id: String,
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
    /// Latest host verification; host-owned and absent on older records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<Verification>,
    /// Digests of cited tool results, recorded by the host at completion.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_digests: Vec<EvidenceDigest>,
}

impl Validated {
    /// Whether the latest host verification reproduced the oracle on every run.
    pub fn verified(&self) -> bool {
        self.verification
            .as_ref()
            .is_some_and(|verification| verification.verified)
    }
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
    /// Screening assessments on a 0–100 scale; absent on legacy records and incomplete drafts.
    #[serde(default)]
    pub impact_score: Option<u8>,
    #[serde(default)]
    pub reachability_score: Option<u8>,
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
    #[serde(default)]
    pub impact_score: Option<u8>,
    #[serde(default)]
    pub reachability_score: Option<u8>,
    pub id: String,
    pub title: String,
    pub summary: String,
    pub surface_ids: Vec<String>,
    pub status: Status,
    pub updated_at_ms: u64,
    pub revision: u64,
    /// Distinct supporting evidence references across the record; counter-evidence excluded.
    #[serde(default)]
    pub evidence_count: u32,
    /// Whether host verification reproduced the validation oracle.
    #[serde(default)]
    pub verified: bool,
}

impl Finding {
    /// Distinct supporting evidence references (by kind and reference) cited by any stage,
    /// the root cause, or the attack path. Counter-evidence is not counted.
    pub fn evidence_count(&self) -> u32 {
        let state = &self.state;
        let stages = [
            state.candidate.evidence_refs.as_deref(),
            state
                .validated
                .as_ref()
                .and_then(|stage| stage.evidence_refs.as_deref()),
            state
                .screened
                .as_ref()
                .and_then(|stage| stage.evidence_refs.as_deref()),
            state
                .reported
                .as_ref()
                .and_then(|stage| stage.evidence_refs.as_deref()),
            state
                .rejected
                .as_ref()
                .and_then(|stage| stage.evidence_refs.as_deref()),
            self.root_cause
                .as_ref()
                .map(|cause| cause.evidence_refs.as_slice()),
            self.attack_path
                .as_ref()
                .map(|path| path.evidence_refs.as_slice()),
        ];
        let distinct: std::collections::BTreeSet<(String, &str)> = stages
            .into_iter()
            .flatten()
            .flatten()
            .map(|item| (format!("{:?}", item.kind), item.reference.as_str()))
            .collect();
        u32::try_from(distinct.len()).unwrap_or(u32::MAX)
    }
}

impl From<&Finding> for Summary {
    fn from(value: &Finding) -> Self {
        Self {
            impact_score: value
                .state
                .screened
                .as_ref()
                .and_then(|stage| stage.impact_score),
            reachability_score: value
                .state
                .screened
                .as_ref()
                .and_then(|stage| stage.reachability_score),
            id: value.id.clone(),
            title: value.title.clone(),
            summary: value.summary.clone(),
            surface_ids: value.surface_ids.clone(),
            status: value.status(),
            updated_at_ms: value.updated_at_ms,
            revision: value.revision,
            evidence_count: value.evidence_count(),
            verified: value
                .state
                .validated
                .as_ref()
                .is_some_and(Validated::verified),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(record.verification.is_none());
        assert!(record.evidence_digests.is_empty());
        assert!(!record.verified());
        let wire = serde_json::to_value(&record).unwrap();
        assert!(wire.get("verification").is_none());
        assert!(wire.get("evidence_digests").is_none());
    }

    #[test]
    fn verification_round_trips_with_tagged_oracles() {
        let verification = Verification {
            command: "./example-proof".to_owned(),
            oracle: Oracle::StderrContains {
                value: "example-sanitizer-report".to_owned(),
            },
            runs: vec![VerificationRun {
                exit_code: Some(1),
                satisfied: true,
                output_sha256: "0".repeat(64),
                duration_ms: 5,
                error: None,
            }],
            verified: true,
            at_ms: 1,
            session_id: "session-example".to_owned(),
            agent_role: "validation".to_owned(),
            tool_call_id: "tool-example".to_owned(),
        };
        let wire = serde_json::to_value(&verification).unwrap();
        assert_eq!(wire["oracle"]["kind"], "stderr_contains");
        assert_eq!(
            serde_json::from_value::<Verification>(wire).unwrap(),
            verification
        );
        assert_eq!(
            serde_json::from_value::<Oracle>(json!({"kind":"exit_nonzero"})).unwrap(),
            Oracle::ExitNonzero
        );
        assert!(serde_json::from_value::<Oracle>(json!({"kind":"regex","value":"x"})).is_err());
    }
    #[test]
    fn legacy_screening_and_summary_keep_missing_scores_distinct_from_zero() {
        let check = json!({"passed":true,"method":"Example review","conclusion":"Supported"});
        let legacy: Screened = serde_json::from_value(json!({
            "validation_review":check,"known_issue_review":check,"scope_review":check,
            "revision":3,"completion":{"at_ms":1,"session_id":"session-example","predecessor_revision":2}
        })).unwrap();
        assert_eq!(legacy.impact_score, None);
        assert_eq!(legacy.reachability_score, None);
        assert!(legacy.completion.is_some());
        let mut summary: Summary = serde_json::from_value(json!({
            "id":"F1","title":"Example finding","summary":"Synthetic summary","surface_ids":[],
            "status":"screened","updated_at_ms":1,"revision":3
        }))
        .unwrap();
        assert_eq!(summary.impact_score, None);
        assert_eq!(summary.reachability_score, None);
        summary.impact_score = Some(0);
        summary.reachability_score = Some(100);
        let wire = serde_json::to_value(&summary).unwrap();
        assert_eq!(wire["impact_score"], 0);
        assert_eq!(wire["reachability_score"], 100);
        let decoded: Summary = serde_json::from_value(wire).unwrap();
        assert_eq!(decoded.impact_score, Some(0));
        assert_eq!(decoded.reachability_score, Some(100));
    }
}
