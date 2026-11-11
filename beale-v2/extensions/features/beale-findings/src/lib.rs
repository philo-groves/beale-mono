//! Independent, workspace-scoped finding lifecycle.

use anyhow::{Context, Result, bail, ensure};
use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    routing::post,
};
use beale_contracts::findings::{
    self, Candidate, Completion, EvidenceKind, Finding, FindingAttackPath, FindingRootCause,
    FindingState, GetRequest, QueryRequest, Rejected, Reported, Screened, Status, Summary,
    Validated,
};
use beale_storage::HostDatabase;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

pub const FEATURE_ID: &str = "beale-findings";
pub const FEATURE_NAME: &str = "Findings";
pub const FEATURE_DESCRIPTION: &str = "Independent, evidence-backed findings with explicit validation, screening, and reporting states.";
pub const HAS_PROMPT_EXTENSION: bool = true;
pub const TOOL_COUNT: u32 = 6;
pub const PROMPT_EXTENSION: &str = "## Findings\n\nUse Candidate for a specific hypothesis. A finding may cover multiple surfaces; keep its summary concise. Record an optional root_cause when supported, an attack_path when its reachability and steps are understood, and remediation when known. Complete Validated only after evidence establishes the behavior; name and describe the validation method, record its result and limitations, cite a successful workspace tool call in evidence_refs, and record counter_evidence when present. A local proof may help but is not required. Complete Screened only after checking validation quality, accepted risks or known issues, and scope/reportability. Prepare a triage-facing, machine-agnostic PoC and report only after Screened. Draft finding or state details with findings_update, then use findings_complete_state; it reports missing fields. Rejected records a reason without discarding earlier state. A state object is not a completed status until its completion passes the predecessor check.";

const QUERY: &str = "findings_query";
const READ: &str = "findings_read";
const CREATE: &str = "findings_create";
const UPDATE: &str = "findings_update";
const COMPLETE: &str = "findings_complete_state";
const REJECT: &str = "findings_reject";

pub fn definitions() -> Vec<Value> {
    let attack_path_schema = json!({
        "type":"object",
        "properties":{
            "summary":{"type":"string"},
            "assumptions":{"type":"array","items":{"type":"string"}},
            "evidence_refs":{"type":"array","items":{"type":"object","properties":{"kind":{"type":"string","enum":["tool_call","artifact","surface","document","url","other"]},"reference":{"type":"string"},"note":{"type":"string"}},"required":["kind","reference"],"additionalProperties":false}},
            "limitations":{"type":"array","items":{"type":"string"}},
            "preconditions":{"type":"array","items":{"type":"string"}},
            "likelihood_description":{"type":"string"},
            "impact_description":{"type":"string"},
            "steps":{"type":"array","items":{"type":"string"}},
            "reachability":{"type":"object","properties":{"summary":{"type":"string"},"attacker_role":{"type":"string"},"entrypoint":{"type":"string"},"source":{"type":"string"},"sink":{"type":"string"}},"required":["summary","attacker_role","entrypoint","source","sink"],"additionalProperties":false}
        },
        "required":["summary","assumptions","evidence_refs","limitations","preconditions","likelihood_description","impact_description","steps","reachability"],
        "additionalProperties":false
    });
    let evidence_refs_schema = json!({"type":"array","items":{"type":"object","properties":{"kind":{"type":"string","enum":["tool_call","artifact","surface","document","url","other"]},"reference":{"type":"string"},"note":{"type":"string"}},"required":["kind","reference"],"additionalProperties":false}});
    let root_cause_schema = json!({"type":"object","properties":{"summary":{"type":"string"},"evidence_refs":evidence_refs_schema.clone()},"required":["summary","evidence_refs"],"additionalProperties":false});
    let screening_check_schema = json!({"type":"object","properties":{"passed":{"type":"boolean"},"method":{"type":"string"},"conclusion":{"type":"string"}},"required":["passed","method","conclusion"],"additionalProperties":false});
    let patch_schema = json!({
        "type":"object",
        "description":"Set fields for the selected state. For finding, root_cause, attack_path, and remediation may be null to clear them.",
        "properties":{
            "title":{"type":"string"},"summary":{"type":"string"},"surface_ids":{"type":"array","items":{"type":"string"}},
            "root_cause":{"anyOf":[root_cause_schema.clone(),{"type":"null"}]},
            "attack_path":{"anyOf":[attack_path_schema.clone(),{"type":"null"}]},
            "remediation":{"type":["string","null"]},
            "hypothesis":{"type":"string"},
            "attacker_control":{"type":"string"},"violated_invariant":{"type":"string"},
            "observed_behavior":{"type":"string"},"impact":{"type":"string"},
            "validation_method":{"type":"string"},"validation_description":{"type":"string"},
            "validation_result":{"type":"string"},
            "validation_limitations":{"type":"array","items":{"type":"string"}},
            "counter_evidence":evidence_refs_schema.clone(),
            "validation_review":screening_check_schema.clone(),
            "known_issue_review":screening_check_schema.clone(),
            "scope_review":screening_check_schema,
            "poc_steps":{"type":"string"},"poc_result":{"type":"string"},
            "poc_portability_notes":{"type":"string"},
            "poc_artifact_refs":{"type":"array","items":{"type":"string"}},
            "report_reference":{"type":"string"},
            "evidence_refs":evidence_refs_schema
        },
        "additionalProperties":false
    });
    vec![
        json!({"type":"function","name":QUERY,"description":"Page through workspace findings by text, surfaces (all listed IDs), or derived status. Use next_offset for the next page.","parameters":{"type":"object","properties":{"text":{"type":"string"},"status":{"type":"string","enum":["candidate","validated","screened","reported","rejected"]},"surface_ids":{"type":"array","items":{"type":"string"}},"limit":{"type":"integer","minimum":1,"maximum":50},"offset":{"type":"integer","minimum":0}},"additionalProperties":false}}),
        json!({"type":"function","name":READ,"description":"Read a finding. Pass history_offset (starting at 0) to include one newest-first revision; omit it for the current record only.","parameters":{"type":"object","properties":{"id":{"type":"string"},"history_offset":{"type":"integer","minimum":0}},"required":["id"],"additionalProperties":false}}),
        json!({"type":"function","name":CREATE,"description":"Create a Candidate finding with a concise summary, concrete hypothesis, and optional surface IDs, root cause, attack path, and remediation.","parameters":{"type":"object","properties":{"title":{"type":"string"},"summary":{"type":"string"},"surface_ids":{"type":"array","items":{"type":"string"}},"root_cause":{"type":"object","properties":{"summary":{"type":"string"},"evidence_refs":{"type":"array","items":{"type":"object","properties":{"kind":{"type":"string","enum":["tool_call","artifact","surface","document","url","other"]},"reference":{"type":"string"},"note":{"type":"string"}},"required":["kind","reference"],"additionalProperties":false}}},"required":["summary","evidence_refs"],"additionalProperties":false},"attack_path":attack_path_schema,"remediation":{"type":"string"},"hypothesis":{"type":"string"}},"required":["title","summary","hypothesis"],"additionalProperties":false}}),
        json!({"type":"function","name":UPDATE,"description":"Revise finding metadata or one state object. Set state to finding, candidate, validated, screened, or reported. Supply a complete root_cause or attack_path object when setting it. State-specific fields are listed in patch; only fields belonging to the selected state are accepted. Reported PoC details require completed screening.","parameters":{"type":"object","properties":{"id":{"type":"string"},"expected_revision":{"type":"integer","minimum":1},"state":{"type":"string","enum":["finding","candidate","validated","screened","reported"]},"patch":patch_schema,"reason":{"type":"string"}},"required":["id","expected_revision","state","patch","reason"],"additionalProperties":false}}),
        json!({"type":"function","name":COMPLETE,"description":"Complete Validated, Screened, or Reported in order. Returns missing state details when incomplete. Validation evidence must cite durable successful read or bash calls in this workspace.","parameters":{"type":"object","properties":{"id":{"type":"string"},"expected_revision":{"type":"integer","minimum":1},"state":{"type":"string","enum":["validated","screened","reported"]}},"required":["id","expected_revision","state"],"additionalProperties":false}}),
        json!({"type":"function","name":REJECT,"description":"Reject a finding from its current state with a reason and optional typed evidence_refs, retaining prior state objects.","parameters":{"type":"object","properties":{"id":{"type":"string"},"expected_revision":{"type":"integer","minimum":1},"reason":{"type":"string"},"evidence_refs":{"type":"array","items":{"type":"object","properties":{"kind":{"type":"string","enum":["tool_call","artifact","surface","document","url","other"]},"reference":{"type":"string"},"note":{"type":"string"}},"required":["kind","reference"],"additionalProperties":false}}},"required":["id","expected_revision","reason"],"additionalProperties":false}}),
    ]
}

pub fn is_tool(name: &str) -> bool {
    matches!(name, QUERY | READ | CREATE | UPDATE | COMPLETE | REJECT)
}

pub struct PendingSource<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub output: &'a str,
    pub is_error: bool,
}

fn required<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("{key} is required"))
}
fn revision(args: &Value) -> Result<u64> {
    args.get("expected_revision")
        .and_then(Value::as_u64)
        .filter(|number| *number > 0)
        .context("expected_revision is required")
}
fn state_summary(finding: &Finding) -> Value {
    json!({"record":Summary::from(finding)})
}

fn optional_field<T: DeserializeOwned>(args: &Value, key: &str) -> Result<Option<T>> {
    args.get(key)
        .map(|value| serde_json::from_value(value.clone()).map_err(Into::into))
        .transpose()
}

fn patch_finding(finding: &mut Finding, patch: &Value) -> Result<()> {
    let fields = patch.as_object().context("patch must be an object")?;
    ensure!(
        !fields.is_empty() && fields.len() <= 6,
        "invalid finding patch"
    );
    let prior_surface_ids = finding.surface_ids.clone();
    for (key, value) in fields {
        match key.as_str() {
            "title" => finding.title = serde_json::from_value(value.clone())?,
            "summary" => finding.summary = serde_json::from_value(value.clone())?,
            "surface_ids" => finding.surface_ids = serde_json::from_value(value.clone())?,
            "root_cause" => finding.root_cause = serde_json::from_value(value.clone())?,
            "attack_path" => finding.attack_path = serde_json::from_value(value.clone())?,
            "remediation" => finding.remediation = serde_json::from_value(value.clone())?,
            _ => bail!("unknown finding field: {key}"),
        }
    }
    if finding.surface_ids != prior_surface_ids
        && let Some(screened) = &mut finding.state.screened
    {
        screened.completion = None;
        screened.revision += 1;
    }
    Ok(())
}

fn patch_state<T: serde::Serialize + DeserializeOwned>(state: &mut T, patch: &Value) -> Result<()> {
    let fields = patch.as_object().context("patch must be an object")?;
    ensure!(
        !fields.is_empty() && fields.len() <= 16,
        "patch is empty or too large"
    );
    let mut value = serde_json::to_value(&*state)?;
    let target = value.as_object_mut().context("invalid state object")?;
    for (key, value) in fields {
        ensure!(
            !matches!(key.as_str(), "revision" | "completion" | "completed_at_ms"),
            "state metadata is host-owned"
        );
        ensure!(target.contains_key(key), "unknown state field: {key}");
        target.insert(key.clone(), value.clone());
    }
    *state = serde_json::from_value(value)?;
    Ok(())
}

fn screening_missing(state: &Screened) -> Vec<String> {
    let mut missing = Vec::new();
    for (name, check) in [
        ("validation_review", &state.validation_review),
        ("known_issue_review", &state.known_issue_review),
        ("scope_review", &state.scope_review),
    ] {
        if !check.passed {
            missing.push(format!("{name}.passed"));
        }
        if check.method.trim().is_empty() {
            missing.push(format!("{name}.method"));
        }
        if check.conclusion.trim().is_empty() {
            missing.push(format!("{name}.conclusion"));
        }
    }
    missing
}

pub fn execute(
    db: &HostDatabase,
    workspace_id: &str,
    session_id: &str,
    name: &str,
    args: &Value,
    pending: &[PendingSource<'_>],
) -> Result<String> {
    ensure!(args.is_object(), "arguments must be an object");
    let result = match name {
        QUERY => {
            let status = args
                .get("status")
                .map(|value| serde_json::from_value::<Status>(value.clone()))
                .transpose()?;
            let request = QueryRequest {
                workspace_id: workspace_id.to_owned(),
                text: args
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                status,
                surface_ids: optional_field(args, "surface_ids")?.unwrap_or_default(),
                limit: args
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(20)
                    .try_into()?,
                offset: args
                    .get("offset")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .try_into()?,
            };
            json!(db.finding_query(&request)?)
        }
        READ => json!(
            db.finding_get_page(
                workspace_id,
                required(args, "id")?,
                args.get("history_offset")
                    .map(|value| -> Result<u32> {
                        Ok(value
                            .as_u64()
                            .context("invalid history_offset")?
                            .try_into()?)
                    })
                    .transpose()?,
            )?
            .context("finding not found")?
        ),
        CREATE => {
            let finding = Finding {
                id: String::new(),
                workspace_id: workspace_id.to_owned(),
                title: required(args, "title")?.to_owned(),
                summary: required(args, "summary")?.to_owned(),
                surface_ids: optional_field(args, "surface_ids")?.unwrap_or_default(),
                root_cause: optional_field::<FindingRootCause>(args, "root_cause")?,
                attack_path: optional_field::<FindingAttackPath>(args, "attack_path")?,
                remediation: optional_field::<String>(args, "remediation")?,
                state: FindingState {
                    candidate: Candidate {
                        hypothesis: required(args, "hypothesis")?.to_owned(),
                        evidence_refs: None,
                        revision: 0,
                        completed_at_ms: 0,
                    },
                    validated: None,
                    screened: None,
                    reported: None,
                    rejected: None,
                },
                created_at_ms: 0,
                updated_at_ms: 0,
                revision: 0,
                history: Vec::new(),
            };
            state_summary(&db.finding_create(finding, session_id)?)
        }
        UPDATE | COMPLETE | REJECT => {
            let mut finding = db
                .finding_get(workspace_id, required(args, "id")?)?
                .context("finding not found")?;
            let expected = revision(args)?;
            ensure!(finding.revision == expected, "finding revision conflict");
            ensure!(
                finding.state.rejected.is_none(),
                "rejected finding cannot be changed"
            );
            if name == REJECT {
                let reason = required(args, "reason")?;
                ensure!(!reason.trim().is_empty(), "rejection reason is required");
                finding.state.rejected = Some(Rejected {
                    reason: reason.to_owned(),
                    from: finding.status(),
                    at_ms: beale_storage::finding_now_ms()?,
                    session_id: session_id.to_owned(),
                    evidence_refs: optional_field(args, "evidence_refs")?,
                });
                state_summary(&db.finding_update(finding, expected, session_id, "rejected")?)
            } else {
                let state = required(args, "state")?;
                if name == UPDATE {
                    let patch = args.get("patch").context("patch is required")?;
                    let reason = required(args, "reason")?;
                    match state {
                        "finding" => patch_finding(&mut finding, patch)?,
                        "candidate" => {
                            patch_state(&mut finding.state.candidate, patch)?;
                            finding.state.candidate.revision += 1;
                        }
                        "validated" => {
                            let stage = finding.state.validated.get_or_insert_with(|| Validated {
                                revision: 1,
                                ..Default::default()
                            });
                            patch_state(stage, patch)?;
                            stage.revision += 1;
                            stage.completion = None;
                        }
                        "screened" => {
                            ensure!(
                                matches!(
                                    finding.status(),
                                    Status::Validated | Status::Screened | Status::Reported
                                ),
                                "complete validation before screening"
                            );
                            let stage = finding.state.screened.get_or_insert_with(|| Screened {
                                revision: 1,
                                ..Default::default()
                            });
                            patch_state(stage, patch)?;
                            stage.revision += 1;
                            stage.completion = None;
                        }
                        "reported" => {
                            ensure!(
                                matches!(finding.status(), Status::Screened | Status::Reported),
                                "complete screening before preparing a triage-facing PoC"
                            );
                            let stage = finding.state.reported.get_or_insert_with(|| Reported {
                                revision: 1,
                                ..Default::default()
                            });
                            patch_state(stage, patch)?;
                            stage.revision += 1;
                            stage.completion = None;
                        }
                        _ => bail!("invalid finding state"),
                    }
                    state_summary(&db.finding_update(finding, expected, session_id, reason)?)
                } else {
                    let mut missing = Vec::new();
                    match state {
                        "validated" => {
                            ensure!(
                                finding.status() == Status::Candidate,
                                "validation can only complete from Candidate"
                            );
                            let stage = finding.state.validated.get_or_insert_with(|| Validated {
                                revision: 1,
                                ..Default::default()
                            });
                            for (key, value) in [
                                ("attacker_control", &stage.attacker_control),
                                ("violated_invariant", &stage.violated_invariant),
                                ("observed_behavior", &stage.observed_behavior),
                                ("impact", &stage.impact),
                                ("validation_method", &stage.validation_method),
                                ("validation_description", &stage.validation_description),
                                ("validation_result", &stage.validation_result),
                            ] {
                                if value.trim().is_empty() {
                                    missing.push(key.to_owned());
                                }
                            }
                            let tool_ids: Vec<String> = stage
                                .evidence_refs
                                .as_ref()
                                .into_iter()
                                .flatten()
                                .filter(|evidence| evidence.kind == EvidenceKind::ToolCall)
                                .map(|evidence| evidence.reference.clone())
                                .collect();
                            if tool_ids.is_empty() {
                                missing.push("evidence_refs (tool_call)".to_owned());
                            }
                            if missing.is_empty() {
                                for id in &tool_ids {
                                    if let Some(source) =
                                        pending.iter().find(|source| source.id == id)
                                    {
                                        db.finding_record_tool_source(
                                            workspace_id,
                                            session_id,
                                            source.id,
                                            source.name,
                                            source.output,
                                            source.is_error,
                                        )?;
                                    }
                                }
                                db.finding_validate_tool_evidence(workspace_id, &tool_ids)?;
                                stage.completion = Some(Completion {
                                    at_ms: beale_storage::finding_now_ms()?,
                                    session_id: session_id.to_owned(),
                                    predecessor_revision: finding.state.candidate.revision,
                                });
                                stage.revision += 1;
                            }
                        }
                        "screened" => {
                            ensure!(
                                finding.status() == Status::Validated,
                                "complete validation before screening"
                            );
                            let predecessor = finding
                                .state
                                .validated
                                .as_ref()
                                .context("validation missing")?
                                .revision;
                            let stage = finding.state.screened.get_or_insert_with(|| Screened {
                                revision: 1,
                                ..Default::default()
                            });
                            missing = screening_missing(stage);
                            if missing.is_empty() {
                                stage.completion = Some(Completion {
                                    at_ms: beale_storage::finding_now_ms()?,
                                    session_id: session_id.to_owned(),
                                    predecessor_revision: predecessor,
                                });
                                stage.revision += 1;
                            }
                        }
                        "reported" => {
                            ensure!(
                                finding.status() == Status::Screened,
                                "complete screening before reporting"
                            );
                            let predecessor = finding
                                .state
                                .screened
                                .as_ref()
                                .context("screening missing")?
                                .revision;
                            let stage = finding.state.reported.get_or_insert_with(|| Reported {
                                revision: 1,
                                ..Default::default()
                            });
                            for (key, value) in [
                                ("poc_steps", &stage.poc_steps),
                                ("poc_result", &stage.poc_result),
                                ("poc_portability_notes", &stage.poc_portability_notes),
                                ("report_reference", &stage.report_reference),
                            ] {
                                if value.trim().is_empty() {
                                    missing.push(key.to_owned());
                                }
                            }
                            if missing.is_empty() {
                                stage.completion = Some(Completion {
                                    at_ms: beale_storage::finding_now_ms()?,
                                    session_id: session_id.to_owned(),
                                    predecessor_revision: predecessor,
                                });
                                stage.revision += 1;
                            }
                        }
                        _ => bail!("invalid finding completion state"),
                    }
                    if missing.is_empty() {
                        state_summary(&db.finding_update(
                            finding,
                            expected,
                            session_id,
                            "completed state",
                        )?)
                    } else {
                        json!({"completed":false,"status":finding.status(),"missing":missing})
                    }
                }
            }
        }
        _ => bail!("unknown Findings tool"),
    };
    let output = result.to_string();
    ensure!(output.len() <= 2_000_000, "Findings result is too large");
    Ok(output)
}

#[derive(Clone)]
struct HttpState {
    token: String,
    db: HostDatabase,
}
pub fn router(token: String, db: HostDatabase) -> Router {
    Router::new()
        .route(findings::QUERY_PATH, post(query))
        .route(findings::GET_PATH, post(get))
        .layer(axum::extract::DefaultBodyLimit::max(8_192))
        .with_state(HttpState { token, db })
}
fn authorized(headers: &HeaderMap, token: &str) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|value| {
            value.len() == token.len()
                && value
                    .as_bytes()
                    .iter()
                    .zip(token.as_bytes())
                    .fold(0_u8, |diff, (a, b)| diff | (a ^ b))
                    == 0
        })
}
fn decode<T: DeserializeOwned>(headers: &HeaderMap, body: &Bytes) -> Result<T, StatusCode> {
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some("application/json")
    {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    serde_json::from_slice(body).map_err(|_| StatusCode::BAD_REQUEST)
}
async fn query(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let mut request: QueryRequest = decode(&headers, &body)?;
    if !state
        .db
        .feature_enabled(FEATURE_ID)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        || !state
            .db
            .extension_applies_to("feature", FEATURE_ID, &request.workspace_id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        return Err(StatusCode::FORBIDDEN);
    }
    if request.limit == 0 {
        request.limit = 50;
    }
    let records = state
        .db
        .finding_query(&request)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok(Json(json!(records)))
}
async fn get(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let request: GetRequest = decode(&headers, &body)?;
    if !state
        .db
        .feature_enabled(FEATURE_ID)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        || !state
            .db
            .extension_applies_to("feature", FEATURE_ID, &request.workspace_id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        return Err(StatusCode::FORBIDDEN);
    }
    let page = state
        .db
        .finding_get_page(&request.workspace_id, &request.id, request.history_offset)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(json!(page)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use beale_storage::{SessionRow, ToolCallRow, TurnRow, WorkspaceRow};
    use tower::ServiceExt;

    fn invoke(db: &HostDatabase, name: &str, args: Value) -> Value {
        serde_json::from_str(
            &execute(db, "workspace-example", "session-example", name, &args, &[]).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn tool_contract_exposes_state_patch_fields() {
        let definitions = definitions();
        assert_eq!(definitions.len(), TOOL_COUNT as usize);
        let update = definitions
            .iter()
            .find(|tool| tool["name"] == UPDATE)
            .unwrap();
        let fields = &update["parameters"]["properties"]["patch"]["properties"];
        for name in [
            "title",
            "summary",
            "surface_ids",
            "root_cause",
            "attack_path",
            "remediation",
            "hypothesis",
            "attacker_control",
            "violated_invariant",
            "observed_behavior",
            "impact",
            "validation_method",
            "validation_description",
            "validation_result",
            "validation_limitations",
            "counter_evidence",
            "validation_review",
            "known_issue_review",
            "scope_review",
            "poc_steps",
            "poc_result",
            "poc_portability_notes",
            "poc_artifact_refs",
            "report_reference",
            "evidence_refs",
        ] {
            assert!(fields.get(name).is_some(), "missing update field {name}");
        }
        assert_eq!(
            fields["attack_path"]["anyOf"][0]["properties"]["reachability"]["properties"]["sink"]["type"],
            "string"
        );
        assert_eq!(
            fields["evidence_refs"]["items"]["properties"]["kind"]["enum"][0],
            "tool_call"
        );
    }

    #[test]
    fn finding_states_gate_reporting_and_recompute_after_earlier_edits() {
        let root =
            std::env::temp_dir().join(format!("beale-findings-example-{}", uuid::Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        db.replace_workspaces(&[WorkspaceRow {
            id: "workspace-example".to_owned(),
            name: "Example Workspace".to_owned(),
            directory: "example-directory".to_owned(),
        }])
        .unwrap();
        let created = invoke(
            &db,
            CREATE,
            json!({"title":"Example boundary finding","summary":"A synthetic boundary issue","surface_ids":["S1","S2"],"hypothesis":"An example.test input crosses a boundary"}),
        );
        let id = created["record"]["id"].as_str().unwrap();
        assert_eq!(created["record"]["status"], "candidate");
        assert!(execute(&db,"workspace-example","session-example",UPDATE,&json!({"id":id,"expected_revision":1,"state":"reported","patch":{"poc_steps":"Run an example"},"reason":"Draft"}),&[]).is_err());
        assert!(execute(&db,"workspace-example","session-example",UPDATE,&json!({"id":id,"expected_revision":1,"state":"screened","patch":{"scope_review":{"passed":true,"method":"Example review","conclusion":"In scope"}},"reason":"Draft"}),&[]).is_err());
        let validation = invoke(
            &db,
            UPDATE,
            json!({"id":id,"expected_revision":1,"state":"validated","patch":{"attacker_control":"Example input","violated_invariant":"Example authorization boundary","observed_behavior":"Synthetic response","impact":"Example data exposure"},"reason":"Document validation"}),
        );
        assert_eq!(validation["record"]["status"], "candidate");
        let missing = invoke(
            &db,
            COMPLETE,
            json!({"id":id,"expected_revision":2,"state":"validated"}),
        );
        assert_eq!(missing["completed"], false);
        assert_eq!(
            missing["missing"],
            json!([
                "validation_method",
                "validation_description",
                "validation_result",
                "evidence_refs (tool_call)"
            ])
        );
        let session = SessionRow {
            id: "session-example".to_owned(),
            workspace_id: "workspace-example".to_owned(),
            title: "Example".to_owned(),
            created_at_ms: 1,
            last_message_at_ms: 1,
        };
        let turn = TurnRow {
            number: 1,
            provider: 1,
            method: 1,
            model: "example-model".to_owned(),
            prompt: "Example prompt".to_owned(),
            answer: "Example answer".to_owned(),
            duration_ms: 1,
            internal_prompt: false,
        };
        let tool = ToolCallRow {
            id: "tool-example".to_owned(),
            turn_number: 1,
            ordinal: 0,
            name: "read".to_owned(),
            summary: "Read example".to_owned(),
            arguments: "{}".to_owned(),
            output: "Synthetic observed response".to_owned(),
            is_error: false,
        };
        db.append_turn_with_tools(&session, &turn, true, &[])
            .unwrap();
        assert!(
            db.finding_record_tool_source(
                "workspace-example",
                "session-example",
                "tool-rejected",
                "bash",
                "Synthetic error",
                true
            )
            .is_err()
        );
        assert!(
            db.finding_record_tool_source(
                "workspace-other",
                "session-example",
                "tool-other",
                "read",
                "Other data",
                false
            )
            .is_err()
        );
        let updated = invoke(
            &db,
            UPDATE,
            json!({"id":id,"expected_revision":2,"state":"validated","patch":{"validation_method":"Read response comparison","validation_description":"Compare a synthetic request with its expected authorization boundary","validation_result":"The synthetic response crossed the example boundary","validation_limitations":["Only the example configuration was checked"],"counter_evidence":[{"kind":"document","reference":"example-negative-case","note":"Alternate input did not cross the boundary"}],"evidence_refs":[{"kind":"tool_call","reference":"tool-example"}]},"reason":"Attach validation details"}),
        );
        assert_eq!(updated["record"]["revision"], 3);
        assert!(
            execute(
                &db,
                "workspace-example",
                "session-example",
                COMPLETE,
                &json!({"id":id,"expected_revision":3,"state":"validated"}),
                &[]
            )
            .is_err()
        );
        let validated: Value = serde_json::from_str(
            &execute(
                &db,
                "workspace-example",
                "session-example",
                COMPLETE,
                &json!({"id":id,"expected_revision":3,"state":"validated"}),
                &[PendingSource {
                    id: &tool.id,
                    name: &tool.name,
                    output: &tool.output,
                    is_error: tool.is_error,
                }],
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(validated["record"]["status"], "validated");
        assert!(execute(&db,"workspace-example","session-example",UPDATE,&json!({"id":id,"expected_revision":4,"state":"reported","patch":{"poc_steps":"Too early"},"reason":"Draft"}),&[]).is_err());
        let missing_screening = invoke(
            &db,
            COMPLETE,
            json!({"id":id,"expected_revision":4,"state":"screened"}),
        );
        assert_eq!(missing_screening["completed"], false);
        assert!(
            missing_screening["missing"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item == "scope_review.passed")
        );
        let check = json!({"passed":true,"method":"Review synthetic sources","conclusion":"No blocking issue found"});
        invoke(
            &db,
            UPDATE,
            json!({"id":id,"expected_revision":4,"state":"screened","patch":{"validation_review":check,"known_issue_review":check,"scope_review":check,"evidence_refs":[{"kind":"document","reference":"example-screening-note"}]},"reason":"Perform screening"}),
        );
        let screened = invoke(
            &db,
            COMPLETE,
            json!({"id":id,"expected_revision":5,"state":"screened"}),
        );
        assert_eq!(screened["record"]["status"], "screened");
        invoke(
            &db,
            UPDATE,
            json!({"id":id,"expected_revision":6,"state":"reported","patch":{"poc_steps":"Use synthetic input on any supported host","poc_result":"Synthetic response","poc_portability_notes":"No host-specific paths or binaries","report_reference":"report-example-001","evidence_refs":[{"kind":"artifact","reference":"example-report-artifact"}]},"reason":"Prepare triage report"}),
        );
        let reported = invoke(
            &db,
            COMPLETE,
            json!({"id":id,"expected_revision":7,"state":"reported"}),
        );
        assert_eq!(reported["record"]["status"], "reported");
        let rescoped = invoke(
            &db,
            UPDATE,
            json!({"id":id,"expected_revision":8,"state":"finding","patch":{"surface_ids":["S1","S2","S3"]},"reason":"Add affected surface"}),
        );
        assert_eq!(rescoped["record"]["status"], "validated");
        let rescoped_page = invoke(&db, READ, json!({"id":id}));
        assert!(rescoped_page["record"]["state"]["screened"]["completion"].is_null());
        assert!(rescoped_page["record"]["state"]["reported"].is_object());
        let reopened = invoke(
            &db,
            UPDATE,
            json!({"id":id,"expected_revision":9,"state":"candidate","patch":{"hypothesis":"Revised example boundary hypothesis","evidence_refs":[{"kind":"surface","reference":"S1"}]},"reason":"Correct premise"}),
        );
        assert_eq!(reopened["record"]["status"], "candidate");
        let queried = invoke(&db, QUERY, json!({"status":"candidate"}));
        assert_eq!(queried["records"][0]["id"], id);
        assert_eq!(
            queried["records"][0]["summary"],
            "A synthetic boundary issue"
        );
        assert_eq!(
            queried["records"][0]["surface_ids"],
            json!(["S1", "S2", "S3"])
        );
        assert_eq!(
            invoke(&db, QUERY, json!({"surface_ids":["S1","S2"]}))["records"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            invoke(&db, QUERY, json!({"surface_ids":["S1","S4"]}))["records"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        let rejected = invoke(
            &db,
            REJECT,
            json!({"id":id,"expected_revision":10,"reason":"Synthetic counterexample","evidence_refs":[{"kind":"document","reference":"example-counterexample"}]}),
        );
        assert_eq!(rejected["record"]["status"], "rejected");
        let page = invoke(&db, READ, json!({"id":id}));
        assert_eq!(
            page["record"]["state"]["candidate"]["evidence_refs"][0]["kind"],
            "surface"
        );
        assert_eq!(
            page["record"]["state"]["validated"]["evidence_refs"][0]["reference"],
            "tool-example"
        );
        assert_eq!(
            page["record"]["state"]["validated"]["validation_method"],
            "Read response comparison"
        );
        assert_eq!(
            page["record"]["state"]["validated"]["validation_limitations"],
            json!(["Only the example configuration was checked"])
        );
        assert_eq!(
            page["record"]["state"]["validated"]["counter_evidence"][0]["reference"],
            "example-negative-case"
        );
        assert_eq!(
            page["record"]["state"]["screened"]["evidence_refs"][0]["kind"],
            "document"
        );
        assert_eq!(
            page["record"]["state"]["reported"]["evidence_refs"][0]["kind"],
            "artifact"
        );
        assert_eq!(
            page["record"]["state"]["rejected"]["evidence_refs"][0]["reference"],
            "example-counterexample"
        );
        assert_eq!(page["history_total"], 11);
        assert_eq!(page["record"]["history"].as_array().unwrap().len(), 0);
        assert_eq!(page["next_history_offset"], 0);
        let history_page = invoke(&db, READ, json!({"id":id,"history_offset":0}));
        assert_eq!(
            history_page["record"]["history"].as_array().unwrap().len(),
            1
        );
        assert_eq!(history_page["next_history_offset"], 1);
        assert!(execute(&db,"workspace-example","session-example",UPDATE,&json!({"id":id,"expected_revision":11,"state":"candidate","patch":{"hypothesis":"Another"},"reason":"Edit"}),&[]).is_err());
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn finding_metadata_update_supports_multiple_surfaces() {
        let root = std::env::temp_dir().join(format!(
            "beale-findings-metadata-example-{}",
            uuid::Uuid::new_v4()
        ));
        let db = HostDatabase::open(&root).unwrap();
        db.replace_workspaces(&[WorkspaceRow {
            id: "workspace-example".to_owned(),
            name: "Example Workspace".to_owned(),
            directory: "example-directory".to_owned(),
        }])
        .unwrap();
        let created = invoke(
            &db,
            CREATE,
            json!({"title":"Example finding","summary":"Initial summary","surface_ids":["S1"],"root_cause":{"summary":"Initial synthetic cause","evidence_refs":[{"kind":"surface","reference":"S1"}]},"remediation":"Add an example boundary check","hypothesis":"Example hypothesis"}),
        );
        let id = created["record"]["id"].as_str().unwrap();
        let updated = invoke(
            &db,
            UPDATE,
            json!({"id":id,"expected_revision":1,"state":"finding","patch":{"summary":"Updated summary","surface_ids":["S1","S2"],"root_cause":{"summary":"Example trust boundary","evidence_refs":[{"kind":"surface","reference":"S1"},{"kind":"document","reference":"example-design-note"}]},"remediation":"Require authorization at the example boundary"},"reason":"Clarify affected surfaces and cause"}),
        );
        assert_eq!(updated["record"]["revision"], 2);
        let page = invoke(&db, READ, json!({"id":id}));
        assert_eq!(page["record"]["summary"], "Updated summary");
        assert_eq!(page["record"]["surface_ids"], json!(["S1", "S2"]));
        assert_eq!(
            page["record"]["remediation"],
            "Require authorization at the example boundary"
        );
        assert_eq!(
            page["record"]["root_cause"]["summary"],
            "Example trust boundary"
        );
        assert_eq!(
            page["record"]["root_cause"]["evidence_refs"][1]["reference"],
            "example-design-note"
        );
        assert_eq!(
            page["record"]["state"]["candidate"]["hypothesis"],
            "Example hypothesis"
        );
        assert_eq!(
            invoke(&db, QUERY, json!({"surface_ids":["S1","S2"]}))["records"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            invoke(&db, QUERY, json!({"surface_ids":["S1","S3"]}))["records"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            invoke(&db, QUERY, json!({"text":"trust boundary"}))["records"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            invoke(&db, QUERY, json!({"text":"Require authorization"}))["records"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(execute(&db,"workspace-example","session-example",UPDATE,&json!({"id":id,"expected_revision":2,"state":"finding","patch":{"root_cause":{"summary":" ","evidence_refs":[]}},"reason":"Invalid cause"}),&[]).is_err());
        assert!(execute(&db,"workspace-example","session-example",UPDATE,&json!({"id":id,"expected_revision":2,"state":"finding","patch":{"root_cause":{"summary":"Example cause","evidence_refs":[{"kind":"surface","reference":"S1"},{"kind":"surface","reference":"S1"}]}},"reason":"Duplicate evidence"}),&[]).is_err());
        assert!(execute(&db,"workspace-example","session-example",UPDATE,&json!({"id":id,"expected_revision":2,"state":"finding","patch":{"remediation":"  "},"reason":"Invalid remediation"}),&[]).is_err());
        let cleared = invoke(
            &db,
            UPDATE,
            json!({"id":id,"expected_revision":2,"state":"finding","patch":{"root_cause":null,"remediation":null},"reason":"Withdraw cause and remediation"}),
        );
        assert_eq!(cleared["record"]["revision"], 3);
        assert!(invoke(&db, READ, json!({"id":id}))["record"]["root_cause"].is_null());
        assert!(invoke(&db, READ, json!({"id":id}))["record"]["remediation"].is_null());
        invoke(
            &db,
            CREATE,
            json!({"title":"Second example finding","summary":"Another synthetic issue","hypothesis":"Another example boundary"}),
        );
        let first_page = invoke(&db, QUERY, json!({"limit":1}));
        assert_eq!(first_page["total"], 2);
        assert_eq!(first_page["next_offset"], 1);
        let second_page = invoke(&db, QUERY, json!({"limit":1,"offset":1}));
        assert_eq!(second_page["total"], 2);
        assert!(second_page["next_offset"].is_null());
        assert_ne!(
            first_page["records"][0]["id"],
            second_page["records"][0]["id"]
        );
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn finding_attack_path_round_trips_and_remains_optional() {
        let root = std::env::temp_dir().join(format!(
            "beale-findings-path-example-{}",
            uuid::Uuid::new_v4()
        ));
        let db = HostDatabase::open(&root).unwrap();
        db.replace_workspaces(&[WorkspaceRow {
            id: "workspace-example".to_owned(),
            name: "Example Workspace".to_owned(),
            directory: "example-directory".to_owned(),
        }])
        .unwrap();
        let path = json!({
            "summary":"Synthetic request path",
            "assumptions":["Example role has an account"],
            "evidence_refs":[{"kind":"document","reference":"example-flow-note"}],
            "limitations":["Only the example route was checked"],
            "preconditions":["Example endpoint is available"],
            "likelihood_description":"Reachable with a standard example account",
            "impact_description":"Could expose synthetic data",
            "steps":["Send an example request","Observe the synthetic response"],
            "reachability":{
                "summary":"Example user can reach the route",
                "attacker_role":"Example user",
                "entrypoint":"example.test/api/example",
                "source":"request parameter",
                "sink":"response body"
            }
        });
        let created = invoke(
            &db,
            CREATE,
            json!({"title":"Example path finding","summary":"Synthetic path issue","hypothesis":"Example boundary hypothesis","attack_path":path}),
        );
        let id = created["record"]["id"].as_str().unwrap();
        let page = invoke(&db, READ, json!({"id":id}));
        assert_eq!(
            page["record"]["attack_path"]["reachability"]["attacker_role"],
            "Example user"
        );
        assert_eq!(
            page["record"]["attack_path"]["steps"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            invoke(&db, QUERY, json!({"text":"Example user can reach"}))["records"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let mut invalid = page["record"]["attack_path"].clone();
        invalid["steps"] = json!([" "]);
        assert!(execute(&db,"workspace-example","session-example",UPDATE,&json!({"id":id,"expected_revision":1,"state":"finding","patch":{"attack_path":invalid},"reason":"Invalid step"}),&[]).is_err());
        let mut incomplete = page["record"]["attack_path"].clone();
        incomplete["reachability"]
            .as_object_mut()
            .unwrap()
            .remove("sink");
        assert!(execute(&db,"workspace-example","session-example",UPDATE,&json!({"id":id,"expected_revision":1,"state":"finding","patch":{"attack_path":incomplete},"reason":"Incomplete reachability"}),&[]).is_err());
        let mut revised = page["record"]["attack_path"].clone();
        revised["likelihood_description"] = json!("Requires a specific example account");
        let updated = invoke(
            &db,
            UPDATE,
            json!({"id":id,"expected_revision":1,"state":"finding","patch":{"attack_path":revised},"reason":"Refine likelihood"}),
        );
        assert_eq!(updated["record"]["status"], "candidate");
        assert_eq!(
            invoke(&db, READ, json!({"id":id}))["record"]["attack_path"]["likelihood_description"],
            "Requires a specific example account"
        );
        invoke(
            &db,
            UPDATE,
            json!({"id":id,"expected_revision":2,"state":"finding","patch":{"attack_path":null},"reason":"Withdraw attack path"}),
        );
        assert!(invoke(&db, READ, json!({"id":id}))["record"]["attack_path"].is_null());
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn finding_history_remains_editable_and_paged_after_256_revisions() {
        let root = std::env::temp_dir().join(format!(
            "beale-findings-history-example-{}",
            uuid::Uuid::new_v4()
        ));
        let db = HostDatabase::open(&root).unwrap();
        db.replace_workspaces(&[WorkspaceRow {
            id: "workspace-example".to_owned(),
            name: "Example Workspace".to_owned(),
            directory: "example-directory".to_owned(),
        }])
        .unwrap();
        let created = invoke(
            &db,
            CREATE,
            json!({"title":"Example long-running finding","summary":"Initial synthetic summary","hypothesis":"Example hypothesis"}),
        );
        let id = created["record"]["id"].as_str().unwrap();
        for revision in 1..=260 {
            invoke(
                &db,
                UPDATE,
                json!({"id":id,"expected_revision":revision,"state":"finding","patch":{"summary":format!("Synthetic summary {revision}")},"reason":"Refine description"}),
            );
        }
        drop(db);
        let db = HostDatabase::open(&root).unwrap();
        let current = invoke(&db, READ, json!({"id":id}));
        assert_eq!(current["record"]["revision"], 261);
        assert_eq!(current["history_total"], 261);
        assert_eq!(current["record"]["history"], json!([]));
        let oldest = invoke(&db, READ, json!({"id":id,"history_offset":260}));
        assert_eq!(oldest["record"]["history"][0]["number"], 1);
        assert!(oldest["next_history_offset"].is_null());
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn read_route_requires_authentication_and_feature_scope() {
        let root = std::env::temp_dir().join(format!(
            "beale-findings-route-example-{}",
            uuid::Uuid::new_v4()
        ));
        let db = HostDatabase::open(&root).unwrap();
        db.replace_workspaces(&[WorkspaceRow {
            id: "workspace-example".to_owned(),
            name: "Example Workspace".to_owned(),
            directory: "example-directory".to_owned(),
        }])
        .unwrap();
        invoke(
            &db,
            CREATE,
            json!({"title":"Example finding","summary":"A synthetic finding","hypothesis":"Example.test boundary hypothesis"}),
        );
        let app = router("example-token".to_owned(), db.clone());
        let body = json!({"workspace_id":"workspace-example","limit":10}).to_string();
        let request = |authorized: bool| {
            let mut builder = Request::builder()
                .uri(findings::QUERY_PATH)
                .method("POST")
                .header(header::CONTENT_TYPE, "application/json");
            if authorized {
                builder = builder.header(header::AUTHORIZATION, "Bearer example-token");
            }
            builder.body(Body::from(body.clone())).unwrap()
        };
        assert_eq!(
            app.clone().oneshot(request(false)).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            app.clone().oneshot(request(true)).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        db.set_feature_enabled(FEATURE_ID, true).unwrap();
        let response = app.clone().oneshot(request(true)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 16_384).await.unwrap()).unwrap();
        assert_eq!(value["records"][0]["status"], "candidate");
        assert_eq!(value["total"], 1);
        let id = value["records"][0]["id"].as_str().unwrap();
        let get_request = |workspace_id: &str, authorized: bool| {
            let mut builder = Request::builder()
                .uri(findings::GET_PATH)
                .method("POST")
                .header(header::CONTENT_TYPE, "application/json");
            if authorized {
                builder = builder.header(header::AUTHORIZATION, "Bearer example-token");
            }
            builder
                .body(Body::from(
                    json!({"workspace_id":workspace_id,"id":id}).to_string(),
                ))
                .unwrap()
        };
        assert_eq!(
            app.clone()
                .oneshot(get_request("workspace-example", false))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            app.clone()
                .oneshot(get_request("workspace-other", true))
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        let response = app
            .oneshot(get_request("workspace-example", true))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let detail: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 16_384).await.unwrap()).unwrap();
        assert_eq!(detail["record"]["id"], id);
        assert_eq!(detail["record"]["history"], json!([]));
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
}
