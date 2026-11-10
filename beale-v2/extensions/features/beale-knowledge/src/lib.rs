//! Optional workspace Knowledge Base: memories and evidence-gated claims.

use anyhow::{Context, Result, bail, ensure};
use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    routing::post,
};
use beale_contracts::knowledge::{
    self, Data, GetRequest, GetResponse, Kind, Link, QueryRequest, QueryResponse, Record, Summary,
};
use beale_storage::{EvidenceInput, HostDatabase};
use serde_json::{Value, json};

pub const FEATURE_ID: &str = "beale-knowledge";
pub const FEATURE_NAME: &str = "Knowledge Base";
pub const FEATURE_DESCRIPTION: &str =
    "Workspace findings and reusable memories in one searchable Knowledge Base.";
pub const HAS_PROMPT_EXTENSION: bool = true;
pub const TOOL_COUNT: u32 = 6;
pub const PROMPT_EXTENSION: &str = "## Knowledge Base\n\nSearch the workspace Knowledge Base before creating a memory or security claim. Record reusable system facts, sources, sinks, trust boundaries, controls, methods, prior art, and negative results as memories. A sink or sensitive operation alone is not a security finding. Use a lead for a specific unobserved vulnerability hypothesis. Create or promote an observed finding only with a durable read or bash tool result that directly supports the claimed behavior. Link memories to claims as context, support, or contradiction; a link never advances claim maturity. Reproduction and verification remain unavailable until Knowledge can validate them independently. Keep records concise and use revisions to correct them rather than creating a duplicate.";

const QUERY: &str = "knowledge_query";
const GET: &str = "knowledge_get";
const MEMORY_CREATE: &str = "memory_create";
const MEMORY_EDIT: &str = "memory_edit";
const LEAD_CREATE: &str = "lead_create";
const FINDING_CREATE: &str = "finding_create";
const FINDING_REVISE: &str = "finding_revise";
const FINDING_TRANSITION: &str = "finding_transition";
const MEMORY_SAVE: &str = "memory_save";
const CLAIM_CREATE: &str = "claim_create";

pub fn definitions() -> Vec<Value> {
    let evidence = json!({"type":"object","properties":{"tool_call_id":{"type":"string"},"summary":{"type":"string"}},"required":["tool_call_id","summary"],"additionalProperties":false});
    let link = json!({"type":"object","properties":{"target_id":{"type":"string"},"relation":{"type":"string"},"note":{"type":"string"}},"required":["target_id","relation"],"additionalProperties":false});
    let query = json!({"type":"object","properties":{"text":{"type":"string"},"kind":{"type":"string","enum":["memory","claim"]},"category":{"type":"string"},"role":{"type":"string"},"status":{"type":"string"},"surface_id":{"type":"string"},"include_duplicates":{"type":"boolean"},"limit":{"type":"integer","minimum":1,"maximum":50}},"additionalProperties":false});
    let common = json!({"title":{"type":"string"},"summary":{"type":"string"},"body":{"type":"string"},"surface_id":{"type":"string"},"links":{"type":"array","items":link},"evidence":{"type":"array","items":evidence}});
    let mut memory = common.clone();
    memory["category"] = json!({"type":"string","enum":["observation","system_model","method","prior_art","negative_result"]});
    memory["role"] =
        json!({"type":"string","enum":["source","sink","invariant","control","trust_boundary"]});
    memory["support"] = json!({"type":"string","enum":["proposed","supported","disputed","stale"]});
    let mut claim = common;
    claim["rating"] =
        json!({"type":"string","enum":["informational","low","medium","high","critical"]});
    claim["impact"] = json!({"type":"string"});
    claim["attacker_control"] = json!({"type":"string"});
    claim["violated_invariant"] = json!({"type":"string"});
    claim["observed_behavior"] = json!({"type":"string"});
    claim["reproduction_steps"] = json!({"type":"string"});
    claim["source_revision"] = json!({"type":"string"});
    claim["environment"] = json!({"type":"string"});
    let mut memory_edit = memory.clone();
    memory_edit["id"] = json!({"type":"string"});
    memory_edit["expected_revision"] = json!({"type":"integer","minimum":1});
    memory_edit["reason"] = json!({"type":"string"});
    memory_edit["duplicate_of"] = json!({"type":"string"});
    memory_edit["operation"] = json!({"type":"string","enum":["create","revise"]});
    let mut finding_revise = claim.clone();
    finding_revise["id"] = json!({"type":"string"});
    finding_revise["expected_revision"] = json!({"type":"integer","minimum":1});
    finding_revise["reason"] = json!({"type":"string"});
    finding_revise["duplicate_of"] = json!({"type":"string"});
    claim["kind"] = json!({"type":"string","enum":["lead","observed"]});
    let transition = json!({"type":"object","properties":{"id":{"type":"string"},"expected_revision":{"type":"integer","minimum":1},"to_stage":{"type":"string","enum":["observed"]},"disposition":{"type":"string","enum":["active","refuted","stale","duplicate"]},"duplicate_of":{"type":"string"},"reason":{"type":"string"},"attacker_control":{"type":"string"},"violated_invariant":{"type":"string"},"observed_behavior":{"type":"string"},"impact":{"type":"string"},"evidence":{"type":"array","items":evidence}},"required":["id","expected_revision","reason"],"additionalProperties":false});
    vec![
        json!({"type":"function","name":QUERY,"description":"Search workspace memories and claims together. A sink is a memory role, never a finding.","parameters":query}),
        json!({"type":"function","name":GET,"description":"Read a memory or claim with evidence, links, and a page of newest-first revision history by short ID.","parameters":{"type":"object","properties":{"id":{"type":"string"},"history_offset":{"type":"integer","minimum":0},"history_limit":{"type":"integer","minimum":1,"maximum":5}},"required":["id"],"additionalProperties":false}}),
        json!({"type":"function","name":MEMORY_SAVE,"description":"Create or revise a reusable memory. Create requires title and category; revise requires id, expected_revision, and reason. Supplied links replace links; evidence appends.","parameters":{"type":"object","properties":memory_edit,"required":["operation"],"additionalProperties":false}}),
        json!({"type":"function","name":CLAIM_CREATE,"description":"Create a lead or observed claim. An observed claim requires attacker_control, violated_invariant, observed_behavior, impact, and durable read or bash evidence. A sink alone is not a claim.","parameters":{"type":"object","properties":claim,"required":["kind","title","summary"],"additionalProperties":false}}),
        json!({"type":"function","name":FINDING_REVISE,"description":"Revise claim details without changing evidence maturity; append evidence or set a canonical duplicate.","parameters":{"type":"object","properties":finding_revise,"required":["id","expected_revision","reason"],"additionalProperties":false}}),
        json!({"type":"function","name":FINDING_TRANSITION,"description":"Promote a lead to observed with durable evidence, or mark a claim refuted, stale, duplicate, or active with a reason. Reproduction and verification await Knowledge-owned validation.","parameters":transition}),
    ]
}

pub fn is_tool(name: &str) -> bool {
    matches!(
        name,
        QUERY | GET | MEMORY_SAVE | CLAIM_CREATE | FINDING_REVISE | FINDING_TRANSITION
    )
}

fn required<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("{key} is required"))
}
fn text(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::to_owned)
}
fn number(args: &Value, key: &str) -> Result<u64> {
    args.get(key)
        .and_then(Value::as_u64)
        .with_context(|| format!("{key} is required"))
}
fn evidence(args: &Value) -> Result<Vec<EvidenceInput>> {
    args.get("evidence").map_or(Ok(Vec::new()), |value| {
        value
            .as_array()
            .context("evidence must be an array")?
            .iter()
            .map(|item| {
                Ok(EvidenceInput {
                    tool_call_id: required(item, "tool_call_id")?.to_owned(),
                    summary: required(item, "summary")?.to_owned(),
                })
            })
            .collect()
    })
}

pub struct PendingSource<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub output: &'a str,
    pub is_error: bool,
}

fn write_evidence(
    db: &HostDatabase,
    workspace_id: &str,
    session_id: &str,
    args: &Value,
    pending: &[PendingSource<'_>],
) -> Result<Vec<EvidenceInput>> {
    let inputs = evidence(args)?;
    for input in &inputs {
        if let Some(source) = pending
            .iter()
            .find(|source| source.id == input.tool_call_id)
        {
            db.record_knowledge_tool_source(
                workspace_id,
                session_id,
                source.id,
                source.name,
                source.output,
                source.is_error,
            )?;
        }
    }
    Ok(inputs)
}
fn links(args: &Value) -> Result<Option<Vec<Link>>> {
    args.get("links")
        .map(|value| {
            value
                .as_array()
                .context("links must be an array")?
                .iter()
                .map(|item| {
                    Ok(Link {
                        target_id: required(item, "target_id")?.to_owned(),
                        relation: required(item, "relation")?.to_owned(),
                        note: text(item, "note").unwrap_or_default(),
                    })
                })
                .collect()
        })
        .transpose()
}
fn empty_record(workspace_id: &str, title: String, data: Data) -> Record {
    Record {
        id: String::new(),
        workspace_id: workspace_id.to_owned(),
        title,
        summary: String::new(),
        body: String::new(),
        surface_id: String::new(),
        data,
        evidence: Vec::new(),
        links: Vec::new(),
        duplicate_of: String::new(),
        created_at_ms: 0,
        updated_at_ms: 0,
        revision: 0,
        history: Vec::new(),
    }
}
fn apply_common(record: &mut Record, args: &Value) -> Result<()> {
    for (key, target) in [
        ("title", &mut record.title),
        ("summary", &mut record.summary),
        ("body", &mut record.body),
        ("surface_id", &mut record.surface_id),
    ] {
        if let Some(value) = text(args, key) {
            *target = value;
        }
    }
    if let Some(value) = links(args)? {
        record.links = value;
    }
    if let Some(value) = text(args, "duplicate_of") {
        record.duplicate_of = value.clone();
        if let Data::Claim { disposition, .. } = &mut record.data {
            *disposition = if value.is_empty() {
                "active"
            } else {
                "duplicate"
            }
            .to_owned();
        }
    }
    Ok(())
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
    let action = match name {
        MEMORY_SAVE => match required(args, "operation")? {
            "create" => {
                ensure!(
                    args.get("id").is_none()
                        && args.get("expected_revision").is_none()
                        && args.get("reason").is_none(),
                    "memory create does not accept revision fields"
                );
                required(args, "title")?;
                required(args, "category")?;
                MEMORY_CREATE
            }
            "revise" => {
                required(args, "id")?;
                number(args, "expected_revision")?;
                required(args, "reason")?;
                MEMORY_EDIT
            }
            _ => bail!("invalid memory operation"),
        },
        CLAIM_CREATE => match required(args, "kind")? {
            "lead" => {
                required(args, "title")?;
                required(args, "summary")?;
                LEAD_CREATE
            }
            "observed" => {
                for key in [
                    "title",
                    "summary",
                    "attacker_control",
                    "violated_invariant",
                    "observed_behavior",
                    "impact",
                ] {
                    required(args, key)?;
                }
                ensure!(
                    args.get("evidence")
                        .and_then(Value::as_array)
                        .is_some_and(|items| !items.is_empty()),
                    "observed claim requires evidence"
                );
                FINDING_CREATE
            }
            _ => bail!("invalid claim kind"),
        },
        _ => name,
    };
    let result = match action {
        QUERY => {
            let kind = match text(args, "kind").as_deref() {
                Some("memory") => Some(Kind::Memory),
                Some("claim") => Some(Kind::Claim),
                None => None,
                _ => bail!("invalid kind"),
            };
            let query = QueryRequest {
                workspace_id: workspace_id.to_owned(),
                text: text(args, "text").unwrap_or_default(),
                kind,
                category: text(args, "category").unwrap_or_default(),
                role: text(args, "role").unwrap_or_default(),
                status: text(args, "status").unwrap_or_default(),
                surface_id: text(args, "surface_id").unwrap_or_default(),
                include_duplicates: args
                    .get("include_duplicates")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                limit: args
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(20)
                    .try_into()
                    .context("invalid limit")?,
            };
            json!(QueryResponse {
                records: db.knowledge_query(&query)?
            })
        }
        GET => json!(knowledge::page_record(
            db.knowledge_get(workspace_id, required(args, "id")?)?
                .context("knowledge record not found")?,
            args.get("history_offset")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .try_into()
                .context("invalid history offset")?,
            args.get("history_limit")
                .and_then(Value::as_u64)
                .unwrap_or(5)
                .try_into()
                .context("invalid history limit")?,
        )),
        MEMORY_CREATE | LEAD_CREATE | FINDING_CREATE => {
            let data = if action == MEMORY_CREATE {
                Data::Memory {
                    category: required(args, "category")?.to_owned(),
                    role: text(args, "role").unwrap_or_default(),
                    support: text(args, "support").unwrap_or_else(|| "proposed".to_owned()),
                }
            } else {
                Data::Claim {
                    stage: if action == LEAD_CREATE {
                        "lead"
                    } else {
                        "observed"
                    }
                    .to_owned(),
                    disposition: "active".to_owned(),
                    rating: text(args, "rating").unwrap_or_else(|| "informational".to_owned()),
                    attacker_control: text(args, "attacker_control").unwrap_or_default(),
                    violated_invariant: text(args, "violated_invariant").unwrap_or_default(),
                    observed_behavior: text(args, "observed_behavior").unwrap_or_default(),
                    impact: text(args, "impact").unwrap_or_default(),
                    reproduction_steps: text(args, "reproduction_steps").unwrap_or_default(),
                    source_revision: text(args, "source_revision").unwrap_or_default(),
                    environment: text(args, "environment").unwrap_or_default(),
                }
            };
            let mut record = empty_record(workspace_id, required(args, "title")?.to_owned(), data);
            apply_common(&mut record, args)?;
            let saved = db.knowledge_create(
                workspace_id,
                record,
                &write_evidence(db, workspace_id, session_id, args, pending)?,
                "agent",
                session_id,
            )?;
            json!({"record":Summary::from(&saved)})
        }
        MEMORY_EDIT | FINDING_REVISE | FINDING_TRANSITION => {
            let id = required(args, "id")?;
            let mut record = db
                .knowledge_get(workspace_id, id)?
                .context("knowledge record not found")?;
            ensure!(
                (action == MEMORY_EDIT) == (record.data.kind() == Kind::Memory)
                    || (action != MEMORY_EDIT && record.data.kind() == Kind::Claim),
                "wrong knowledge record kind"
            );
            let expected = number(args, "expected_revision")?;
            if action == FINDING_TRANSITION {
                if let Some(stage) = text(args, "to_stage") {
                    ensure!(
                        stage == "observed"
                            && matches!(&record.data,Data::Claim{stage: current,..} if current=="lead"),
                        "invalid claim transition"
                    );
                    if let Data::Claim { stage: current, .. } = &mut record.data {
                        *current = stage;
                    }
                }
                if let Some(value) = text(args, "disposition")
                    && let Data::Claim { disposition, .. } = &mut record.data
                {
                    *disposition = value;
                }
                if let Some(value) = text(args, "duplicate_of") {
                    record.duplicate_of = value.clone();
                    if let Data::Claim { disposition, .. } = &mut record.data {
                        *disposition = if value.is_empty() {
                            "active"
                        } else {
                            "duplicate"
                        }
                        .to_owned();
                    }
                }
                if let Data::Claim {
                    attacker_control,
                    violated_invariant,
                    observed_behavior,
                    impact,
                    ..
                } = &mut record.data
                {
                    for (key, target) in [
                        ("attacker_control", attacker_control),
                        ("violated_invariant", violated_invariant),
                        ("observed_behavior", observed_behavior),
                        ("impact", impact),
                    ] {
                        if let Some(value) = text(args, key) {
                            *target = value;
                        }
                    }
                }
            } else {
                apply_common(&mut record, args)?;
                match &mut record.data {
                    Data::Memory {
                        category,
                        role,
                        support,
                    } => {
                        for (key, target) in
                            [("category", category), ("role", role), ("support", support)]
                        {
                            if let Some(value) = text(args, key) {
                                *target = value;
                            }
                        }
                    }
                    Data::Claim {
                        rating,
                        attacker_control,
                        violated_invariant,
                        observed_behavior,
                        impact,
                        reproduction_steps,
                        source_revision,
                        environment,
                        ..
                    } => {
                        for (key, target) in [
                            ("rating", rating),
                            ("attacker_control", attacker_control),
                            ("violated_invariant", violated_invariant),
                            ("observed_behavior", observed_behavior),
                            ("impact", impact),
                            ("reproduction_steps", reproduction_steps),
                            ("source_revision", source_revision),
                            ("environment", environment),
                        ] {
                            if let Some(value) = text(args, key) {
                                *target = value;
                            }
                        }
                    }
                }
            }
            let saved = db.knowledge_update(
                record,
                expected,
                &write_evidence(db, workspace_id, session_id, args, pending)?,
                "agent",
                session_id,
                required(args, "reason")?,
            )?;
            json!({"record":Summary::from(&saved)})
        }
        _ => bail!("unknown knowledge tool"),
    };
    let output = result.to_string();
    ensure!(output.len() <= 128_000, "knowledge result is too large");
    Ok(output)
}

#[derive(Clone)]
struct HttpState {
    token: String,
    db: HostDatabase,
}
pub fn router(token: String, db: HostDatabase) -> Router {
    Router::new()
        .route(knowledge::QUERY_PATH, post(query))
        .route(knowledge::GET_PATH, post(get))
        .with_state(HttpState { token, db })
        .layer(axum::extract::DefaultBodyLimit::max(8_192))
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
fn available(state: &HttpState, workspace_id: &str) -> Result<bool> {
    Ok(state.db.feature_enabled(FEATURE_ID)?
        && state
            .db
            .extension_applies_to("feature", FEATURE_ID, workspace_id)?)
}
async fn query(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<QueryResponse>, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let mut input: QueryRequest = decode_json(&headers, &body)?;
    if !available(&state, &input.workspace_id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)? {
        return Err(StatusCode::FORBIDDEN);
    }
    if input.limit == 0 {
        input.limit = 50;
    }
    let records = state
        .db
        .knowledge_query(&input)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok(Json(QueryResponse { records }))
}
async fn get(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<GetResponse>, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let input: GetRequest = decode_json(&headers, &body)?;
    if !available(&state, &input.workspace_id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)? {
        return Err(StatusCode::FORBIDDEN);
    }
    let record = state
        .db
        .knowledge_get(&input.workspace_id, &input.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(knowledge::page_record(
        record,
        input.history_offset,
        input.history_limit,
    )))
}

fn decode_json<T: serde::de::DeserializeOwned>(
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<T, StatusCode> {
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some("application/json")
    {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    serde_json::from_slice(body).map_err(|_| StatusCode::BAD_REQUEST)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use beale_storage::WorkspaceRow;
    use tower::ServiceExt;

    #[test]
    fn tools_keep_sinks_in_memory_and_claims_in_their_own_ledger() {
        let root = std::env::temp_dir().join(format!(
            "beale-knowledge-feature-example-{}",
            uuid::Uuid::new_v4()
        ));
        let db = HostDatabase::open(&root).unwrap();
        db.replace_workspaces(&[WorkspaceRow {
            id: "workspace-example".to_owned(),
            name: "Example".to_owned(),
            directory: "example-directory".to_owned(),
        }])
        .unwrap();
        assert_eq!(definitions().len(), TOOL_COUNT as usize);
        assert!(!is_tool(MEMORY_CREATE));
        assert!(!is_tool(LEAD_CREATE));
        let memory: Value = serde_json::from_str(&execute(&db,"workspace-example","session-example",MEMORY_SAVE,&json!({"operation":"create","title":"Example sensitive sink","category":"system_model","role":"sink","summary":"An operation that can change trust state"}),&[]).unwrap()).unwrap();
        assert_eq!(memory["record"]["id"], "M1");
        assert_eq!(memory["record"]["kind"], "memory");
        let revised: Value = serde_json::from_str(&execute(&db,"workspace-example","session-example",MEMORY_SAVE,&json!({"operation":"revise","id":"M1","expected_revision":1,"reason":"Clarify behavior","summary":"An example operation changes trust state"}),&[]).unwrap()).unwrap();
        assert_eq!(revised["record"]["revision"], 2);
        assert!(execute(&db,"workspace-example","session-example",MEMORY_SAVE,&json!({"operation":"revise","id":"M1","expected_revision":1,"reason":"Stale edit","summary":"Outdated text"}),&[]).is_err());
        let lead: Value = serde_json::from_str(&execute(&db,"workspace-example","session-example",CLAIM_CREATE,&json!({"kind":"lead","title":"Example authorization candidate","summary":"A boundary may be crossed"}),&[]).unwrap()).unwrap();
        assert_eq!(lead["record"]["id"], "C1");
        assert_eq!(lead["record"]["status"], "lead");
        assert!(execute(&db,"workspace-example","session-example",CLAIM_CREATE,&json!({"kind":"observed","title":"Example finding","summary":"Observed behavior","attacker_control":"Input","violated_invariant":"Boundary","observed_behavior":"Response","impact":"Disclosure"}),&[]).is_err());
        let query: Value = serde_json::from_str(
            &execute(
                &db,
                "workspace-example",
                "session-example",
                QUERY,
                &json!({"role":"sink"}),
                &[],
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(query["records"].as_array().unwrap().len(), 1);
        assert_eq!(query["records"][0]["kind"], "memory");
        assert!(execute(&db,"workspace-example","session-example",FINDING_TRANSITION,&json!({"id":"C1","expected_revision":1,"to_stage":"observed","reason":"Direct observation"}),&[]).is_err());
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn json_reads_require_auth_and_enabled_workspace_scope() {
        let root = std::env::temp_dir().join(format!(
            "beale-knowledge-route-example-{}",
            uuid::Uuid::new_v4()
        ));
        let db = HostDatabase::open(&root).unwrap();
        db.replace_workspaces(&[WorkspaceRow {
            id: "workspace-example".to_owned(),
            name: "Example".to_owned(),
            directory: "example-directory".to_owned(),
        }])
        .unwrap();
        let request = || {
            Request::builder()
                .method("POST")
                .uri(knowledge::QUERY_PATH)
                .header("content-type", "application/json")
                .header("authorization", "Bearer test-token")
                .body(Body::from(
                    serde_json::to_vec(&QueryRequest {
                        workspace_id: "workspace-example".to_owned(),
                        limit: 50,
                        ..Default::default()
                    })
                    .unwrap(),
                ))
                .unwrap()
        };
        let disabled = router("test-token".to_owned(), db.clone())
            .oneshot(request())
            .await
            .unwrap();
        assert_eq!(disabled.status(), StatusCode::FORBIDDEN);
        db.set_feature_enabled(FEATURE_ID, true).unwrap();
        let enabled = router("test-token".to_owned(), db.clone())
            .oneshot(request())
            .await
            .unwrap();
        assert_eq!(enabled.status(), StatusCode::OK);
        let unauthorized = router("test-token".to_owned(), db.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(knowledge::QUERY_PATH)
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
}
