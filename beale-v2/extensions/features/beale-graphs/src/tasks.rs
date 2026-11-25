use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskContract {
    pub question: String,
    pub scope: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub deliverable: String,
    pub completion_criteria: Vec<String>,
    pub context_policy: ContextPolicy,
    pub budget: TaskBudget,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ContextPolicy {
    EvidenceOnly,
    Shared,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskBudget {
    pub max_model_turns: u32,
    pub max_duration_minutes: u32,
}

fn bounded(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max
}
fn list(values: &[String], max: usize, required: bool) -> bool {
    (!required || !values.is_empty())
        && values.len() <= max
        && values.iter().all(|s| bounded(s, 1024))
}

impl TaskContract {
    pub fn validate(&self) -> Result<(), &'static str> {
        if serde_json::to_vec(self).map_or(true, |bytes| bytes.len() > 16384)
            || !bounded(&self.question, 4096)
            || !bounded(&self.deliverable, 2048)
            || !list(&self.scope, 16, true)
            || !list(&self.evidence_refs, 32, false)
            || !list(&self.completion_criteria, 16, true)
            || !(1..=128).contains(&self.budget.max_model_turns)
            || !(1..=120).contains(&self.budget.max_duration_minutes)
        {
            return Err("invalid task contract or budget");
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskResult {
    pub completion: Completion,
    pub assessment: Assessment,
    pub summary: String,
    pub observations: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub artifacts: Vec<String>,
    pub uncertainties: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Completion {
    Complete,
    Incomplete,
    Blocked,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Assessment {
    Supported,
    Contradicted,
    Inconclusive,
    NotApplicable,
}

impl TaskResult {
    pub fn parse(text: &str) -> Result<Self, &'static str> {
        if text.len() > 32768 {
            return Err("worker result is too large");
        }
        let result: Self = serde_json::from_str(text)
            .map_err(|_| "worker result must match task_result@1 JSON")?;
        if !bounded(&result.summary, 4096)
            || !list(&result.observations, 32, false)
            || !list(&result.evidence_refs, 32, false)
            || !list(&result.artifacts, 32, false)
            || !list(&result.uncertainties, 32, false)
            || (matches!(
                result.assessment,
                Assessment::Supported | Assessment::Contradicted
            ) && result.evidence_refs.is_empty())
        {
            return Err("worker result lacks bounded fields or evidence references");
        }
        Ok(result)
    }
}

pub const RESULT_INSTRUCTIONS: &str = "Return only a task_result@1 JSON object: completion (complete, incomplete, blocked), assessment (supported, contradicted, inconclusive, not_applicable), summary (string), observations, evidence_refs, artifacts, uncertainties (arrays of strings). Supported and contradicted assessments require evidence references. Task completion does not validate a vulnerability or advance a finding's durable state.";

pub fn contract_schema() -> Value {
    let text = json!({"type":"string","minLength":1,"maxLength":1024});
    let array = |min, max| json!({"type":"array","minItems":min,"maxItems":max,"items":text});
    json!({"type":"object","properties":{
        "question":{"type":"string","minLength":1,"maxLength":4096},
        "scope":array(1,16),"evidence_refs":array(0,32),
        "deliverable":{"type":"string","minLength":1,"maxLength":2048},
        "completion_criteria":array(1,16),
        "context_policy":{"type":"string","enum":["evidence_only","shared"]},
        "budget":{"type":"object","properties":{
            "max_model_turns":{"type":"integer","minimum":1,"maximum":128},
            "max_duration_minutes":{"type":"integer","minimum":1,"maximum":120}},
            "required":["max_model_turns","max_duration_minutes"],"additionalProperties":false}
        },"required":["question","scope","evidence_refs","deliverable","completion_criteria","context_policy","budget"],"additionalProperties":false})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GraphRuntime, SIMPLE_RUNTIME};
    use beale_contracts::v1::{GraphNodeModel, SessionGraphConfig};

    fn contract() -> Value {
        json!({"question":"Inspect the example boundary","scope":["example module"],
            "evidence_refs":["tool-example-001"],"deliverable":"A trace of the entry path",
            "completion_criteria":["Identify the trust boundary"],"context_policy":"evidence_only",
            "budget":{"max_model_turns":4,"max_duration_minutes":5}})
    }

    #[test]
    fn five_profiles_require_bounded_contracts_and_challenger_independence() {
        let ids = crate::task_profiles()
            .into_iter()
            .filter(|id| !id.is_legacy())
            .collect::<Vec<_>>();
        assert_eq!(ids.len(), 5);
        for id in ids {
            let args = json!({"assignments":[{"role":id.id(),"contract":contract()}]});
            let assignments = SIMPLE_RUNTIME
                .assignments(
                    &args,
                    &SessionGraphConfig::default(),
                    &GraphNodeModel::default(),
                )
                .unwrap();
            assert_eq!(assignments[0].role, id);
            assert!(assignments[0].contract.is_some());
            assert!(
                SIMPLE_RUNTIME
                    .assignments(
                        &json!({"assignments":[{"role":id.id(),"task":"unbounded"}]}),
                        &SessionGraphConfig::default(),
                        &GraphNodeModel::default()
                    )
                    .is_err()
            );
        }
        let mut value = contract();
        value["budget"]["max_model_turns"] = 0.into();
        assert!(
            serde_json::from_value::<TaskContract>(value)
                .unwrap()
                .validate()
                .is_err()
        );
        let mut value = contract();
        value["context_policy"] = "shared".into();
        assert!(
            SIMPLE_RUNTIME
                .assignments(
                    &json!({"assignments":[{"role":"challenger","contract":value}]}),
                    &SessionGraphConfig::default(),
                    &GraphNodeModel::default()
                )
                .is_err()
        );
    }

    #[test]
    fn result_requires_evidence_for_conclusions_and_keeps_completion_separate() {
        let mut result = json!({"completion":"complete","assessment":"inconclusive","summary":"The example check did not resolve the question.","observations":[],"evidence_refs":[],"artifacts":[],"uncertainties":["Missing prerequisite"]});
        assert!(TaskResult::parse(&result.to_string()).is_ok());
        result["assessment"] = "supported".into();
        assert!(TaskResult::parse(&result.to_string()).is_err());
        result["evidence_refs"] = json!(["tool-example-001"]);
        assert!(TaskResult::parse(&result.to_string()).is_ok());
        result["finding_status"] = "validated".into();
        assert!(TaskResult::parse(&result.to_string()).is_err());
    }
}
