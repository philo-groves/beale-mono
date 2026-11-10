//! Optional model tools for workspace surface records.

use anyhow::{Context, Result, bail, ensure};
use beale_contracts::v1::{
    SurfaceDefinition, SurfaceEvidence, SurfaceLocator, SurfaceRelationship, SurfaceScope,
    SurfaceScopeState, WorkspaceSurface, WorkspaceSurfaceInput,
};
use beale_workspaces::WorkspaceStore;
use serde_json::{Value, json};

mod mapping;
mod threat_model;

pub const FEATURE_ID: &str = "surfaces";
pub const FEATURE_NAME: &str = "Surfaces";
pub const FEATURE_DESCRIPTION: &str = "Map local surface details, track relationships between surfaces, and promote independently researchable elements.";
pub const HAS_PROMPT_EXTENSION: bool = false;
pub const TOOL_COUNT: u32 = 6;
pub const PROMPT_EXTENSION: &str = "";

pub const ADD: &str = "surface_add";
pub const EDIT: &str = "surface_edit";
pub const GET: &str = "surface_get";
pub const QUERY: &str = "surface_query";
pub const MAPPING_GET: &str = "surface_mapping_get";
pub const MAPPING_EDIT: &str = "surface_mapping_edit";
pub const PROMOTE: &str = "surface_promote_element";
pub const THREAT_MODEL_GET: &str = "surface_threat_model_get";
pub const THREAT_MODEL_EDIT: &str = "surface_threat_model_edit";
pub const READ: &str = "surface_read";
pub const SAVE: &str = "surface_save";

pub fn definitions() -> Vec<Value> {
    let evidence = json!({"type":"object","properties":{"kind":{"type":"string"},"reference":{"type":"string"},"note":{"type":"string"}},"required":["kind","reference"],"additionalProperties":false});
    let locator = json!({"type":"object","properties":{"kind":{"type":"string"},"value":{"type":"string"}},"required":["kind","value"],"additionalProperties":false});
    let definition = json!({"type":"object","properties":{"type_id":{"type":"string"},"name":{"type":"string"},"locator":locator,"scope":{"type":"string","enum":["in_scope","out_of_scope"]},"description":{"type":"string"},"out_of_scope_reason":{"type":"string"},"evidence_refs":{"type":"array","items":evidence}},"additionalProperties":false});
    let mut promote_definition = definition.clone();
    promote_definition["required"] = json!(["type_id", "scope"]);
    let relationship = json!({"type":"object","properties":{"target_id":{"type":"string"},"label":{"type":"string"},"type_id":{"type":"string"},"source_element_id":{"type":"string"},"evidence_refs":{"type":"array","items":evidence}},"required":["target_id","label"],"additionalProperties":false});
    let map_element = json!({"type":"object","properties":{"id":{"type":"string"},"category":{"type":"string","enum":["component","interface","behavior","state","control"]},"type_id":{"type":"string"},"name":{"type":"string"},"locator":locator,"observation":{"type":"string"},"basis":{"type":"string","enum":["observed","reported","inferred"]},"observed_at_ms":{"type":"integer"},"evidence_refs":{"type":"array","items":evidence}},"required":["category","type_id","name","observation","basis","evidence_refs"],"additionalProperties":false});
    let map_flow = json!({"type":"object","properties":{"id":{"type":"string"},"from_element_id":{"type":"string"},"to_element_id":{"type":"string"},"kind":{"type":"string","enum":["call","data","control","event"]},"observation":{"type":"string"},"basis":{"type":"string","enum":["observed","reported","inferred"]},"evidence_refs":{"type":"array","items":evidence}},"required":["from_element_id","to_element_id","kind","observation","basis","evidence_refs"],"additionalProperties":false});
    let map_gap = json!({"type":"object","properties":{"id":{"type":"string"},"element_id":{"type":"string"},"question":{"type":"string"},"reason":{"type":"string","enum":["not_examined","unavailable","ambiguous"]},"evidence_refs":{"type":"array","items":evidence}},"required":["question","reason"],"additionalProperties":false});
    let map_ref = json!({"type":"object","properties":{"kind":{"type":"string","enum":["element","flow"]},"id":{"type":"string"}},"required":["kind","id"],"additionalProperties":false});
    let scenario = json!({"type":"object","properties":{"id":{"type":"string"},"title":{"type":"string"},"map_refs":{"type":"array","items":map_ref},"attacker_position":{"type":"string"},"preconditions":{"type":"array","items":{"type":"string"}},"suspected_failure":{"type":"string"},"invariant_at_risk":{"type":"string"},"potential_impact":{"type":"string"},"validation_question":{"type":"string"},"evidence_refs":{"type":"array","items":evidence},"disposition":{"type":"string","enum":["open","dismissed"]},"disposition_reason":{"type":"string"},"related_claim_ids":{"type":"array","items":{"type":"string"}}},"required":["title","map_refs","attacker_position","preconditions","suspected_failure","invariant_at_risk","potential_impact","validation_question"],"additionalProperties":false});
    vec![
        json!({"type":"function","name":QUERY,"description":"Query surfaces in this workspace by type, scope, or text. Returns at most 50 surfaces.","parameters":{"type":"object","properties":{"type_id":{"type":"string"},"scope":{"type":"string","enum":["in_scope","out_of_scope"]},"text":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":50}},"additionalProperties":false}}),
        json!({"type":"function","name":READ,"description":"Read one surface section: record and lineage, local mapping, or threat model. Choose mapping_section to page elements, flows, or gaps.","parameters":{"type":"object","properties":{"surface_id":{"type":"string"},"section":{"type":"string","enum":["record","mapping","threat_model"]},"mapping_section":{"type":"string","enum":["elements","flows","gaps"]},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":50}},"required":["surface_id","section"],"additionalProperties":false}}),
        json!({"type":"function","name":SAVE,"description":"Create or revise a surface definition and inter-surface relationships. Create requires type_id, locator, and scope; revise requires id and workspace_surfaces_revision from surface_read. Evidence references are candidate context, not proof of authorization.","parameters":{"type":"object","properties":{"operation":{"type":"string","enum":["create","revise"]},"id":{"type":"string"},"expected_revision":{"type":"integer","minimum":1},"definition":definition,"relationships":{"type":"array","items":relationship}},"required":["operation"],"additionalProperties":false}}),
        json!({"type":"function","name":MAPPING_EDIT,"description":"Add or replace one local element, flow, or gap, or remove one by ID. Supply one operation. Flows connect elements in this mapping only.","parameters":{"type":"object","properties":{"surface_id":{"type":"string"},"element":map_element,"flow":map_flow,"gap":map_gap,"remove":{"type":"object","properties":{"kind":{"type":"string","enum":["element","flow","gap"]},"id":{"type":"string"}},"required":["kind","id"],"additionalProperties":false}},"required":["surface_id"],"additionalProperties":false}}),
        json!({"type":"function","name":PROMOTE,"description":"Promote a mapped element when it needs its own scope or internal map. The host creates the surface and anchored relationship; the original element stays as a concise marker.","parameters":{"type":"object","properties":{"surface_id":{"type":"string"},"element_id":{"type":"string"},"definition":promote_definition},"required":["surface_id","element_id","definition"],"additionalProperties":false}}),
        json!({"type":"function","name":THREAT_MODEL_EDIT,"description":"Add or replace one plausible local failure, remove one, or mark the current map reviewed. Anchor scenarios to this surface's map; record investigation results as Knowledge claims.","parameters":{"type":"object","properties":{"surface_id":{"type":"string"},"scenario":scenario,"remove_id":{"type":"string"},"review_mapping":{"type":"boolean"}},"required":["surface_id"],"additionalProperties":false}}),
    ]
}

pub fn is_tool(name: &str) -> bool {
    matches!(
        name,
        QUERY | READ | SAVE | MAPPING_EDIT | PROMOTE | THREAT_MODEL_EDIT
    )
}

fn required<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("{key} is required"))
}

fn scope(value: &str) -> Result<i32> {
    Ok(match value {
        "in_scope" => SurfaceScope::InScope,
        "out_of_scope" => SurfaceScope::OutOfScope,
        _ => bail!("invalid surface scope"),
    } as i32)
}

fn evidence_refs(
    value: Option<&Value>,
    previous: Vec<SurfaceEvidence>,
) -> Result<Vec<SurfaceEvidence>> {
    let Some(value) = value else {
        return Ok(previous);
    };
    let items = value.as_array().context("evidence_refs must be an array")?;
    ensure!(items.len() <= 16, "too many evidence references");
    items
        .iter()
        .map(|item| {
            let object = item.as_object().context("invalid evidence reference")?;
            ensure!(
                object
                    .keys()
                    .all(|key| matches!(key.as_str(), "kind" | "reference" | "note")),
                "unknown evidence field"
            );
            Ok(SurfaceEvidence {
                kind: object
                    .get("kind")
                    .and_then(Value::as_str)
                    .context("evidence kind is required")?
                    .to_owned(),
                reference: object
                    .get("reference")
                    .and_then(Value::as_str)
                    .context("evidence reference is required")?
                    .to_owned(),
                note: object
                    .get("note")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            })
        })
        .collect()
}

fn definition(
    value: Option<&Value>,
    previous: Option<SurfaceDefinition>,
) -> Result<SurfaceDefinition> {
    let mut result = previous.unwrap_or_default();
    let Some(value) = value else {
        return Ok(result);
    };
    let fields = value.as_object().context("definition must be an object")?;
    for (key, value) in fields {
        match key.as_str() {
            "type_id" => result.type_id = value.as_str().context("invalid type_id")?.to_owned(),
            "name" => result.name = value.as_str().context("invalid name")?.to_owned(),
            "scope" => result.scope = scope(value.as_str().context("invalid scope")?)?,
            "description" => {
                result.description = value.as_str().context("invalid description")?.to_owned()
            }
            "out_of_scope_reason" => {
                result.out_of_scope_reason = value
                    .as_str()
                    .context("invalid out_of_scope_reason")?
                    .to_owned()
            }
            "evidence_refs" => result.evidence_refs = evidence_refs(Some(value), Vec::new())?,
            "locator" => {
                let locator = value.as_object().context("invalid locator")?;
                ensure!(
                    locator
                        .keys()
                        .all(|key| matches!(key.as_str(), "kind" | "value")),
                    "unknown locator field"
                );
                result.locator = Some(SurfaceLocator {
                    kind: locator
                        .get("kind")
                        .and_then(Value::as_str)
                        .context("locator kind is required")?
                        .to_owned(),
                    value: locator
                        .get("value")
                        .and_then(Value::as_str)
                        .context("locator value is required")?
                        .to_owned(),
                });
            }
            _ => bail!("unknown definition field"),
        }
    }
    Ok(result)
}

fn relationships(
    value: Option<&Value>,
    previous: Vec<SurfaceRelationship>,
) -> Result<Vec<SurfaceRelationship>> {
    let Some(value) = value else {
        return Ok(previous);
    };
    let items = value.as_array().context("relationships must be an array")?;
    ensure!(items.len() <= 64, "too many relationships");
    items
        .iter()
        .map(|item| {
            let item = item.as_object().context("invalid relationship")?;
            ensure!(
                item.keys().all(|key| matches!(
                    key.as_str(),
                    "target_id" | "label" | "type_id" | "evidence_refs" | "source_element_id"
                )),
                "unknown relationship field"
            );
            Ok(SurfaceRelationship {
                target_id: item
                    .get("target_id")
                    .and_then(Value::as_str)
                    .context("relationship target_id is required")?
                    .to_owned(),
                label: item
                    .get("label")
                    .and_then(Value::as_str)
                    .context("relationship label is required")?
                    .to_owned(),
                type_id: item
                    .get("type_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                evidence_refs: evidence_refs(item.get("evidence_refs"), Vec::new())?,
                source_element_id: item
                    .get("source_element_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            })
        })
        .collect()
}

fn input_from_surface(surface: WorkspaceSurface) -> WorkspaceSurfaceInput {
    WorkspaceSurfaceInput {
        id: surface.id,
        definition: surface.definition,
        relationships: surface.relationships,
    }
}

fn evidence_json(item: &SurfaceEvidence) -> Value {
    json!({"kind":item.kind,"reference":item.reference,"note":item.note})
}

fn surface_json(surface: &WorkspaceSurface, include_lineage: bool) -> Value {
    let definition = surface.definition.as_ref().map(|item| {
        let scope_name = if item.scope == SurfaceScope::InScope as i32 { "in_scope" } else { "out_of_scope" };
        let scope_state = match SurfaceScopeState::try_from(item.scope_state) { Ok(SurfaceScopeState::Approved) => "approved", Ok(SurfaceScopeState::Candidate) => "candidate", _ => "unknown" };
        json!({"type_id":item.type_id,"name":item.name,"locator":item.locator.as_ref().map(|locator| json!({"kind":locator.kind,"value":locator.value})),"scope":scope_name,"scope_state":scope_state,"description":item.description,"out_of_scope_reason":item.out_of_scope_reason,"evidence_refs":item.evidence_refs.iter().map(evidence_json).collect::<Vec<_>>()})
    });
    let lineage = if include_lineage {
        surface.lineage.iter().map(|event| json!({"number":event.number,"at_ms":event.at_ms,"actor":event.actor,"action":event.action,"changes":event.changes.iter().map(|change| json!({"field":change.field,"before":serde_json::from_str::<Value>(&change.before_json).unwrap_or(Value::Null),"after":serde_json::from_str::<Value>(&change.after_json).unwrap_or(Value::Null)})).collect::<Vec<_>>()})).collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    json!({"id":surface.id,"definition":definition,"relationships":surface.relationships.iter().map(|item| json!({"target_id":item.target_id,"label":item.label,"type_id":item.type_id,"source_element_id":item.source_element_id,"evidence_refs":item.evidence_refs.iter().map(evidence_json).collect::<Vec<_>>()})).collect::<Vec<_>>(),"lineage":lineage})
}

pub fn execute(
    store: &WorkspaceStore,
    workspace_id: &str,
    name: &str,
    args: &Value,
    review_approved: bool,
) -> Result<String> {
    ensure!(args.is_object(), "arguments must be an object");
    if name == READ {
        let fields = args.as_object().context("arguments must be an object")?;
        ensure!(
            fields.keys().all(|key| matches!(
                key.as_str(),
                "surface_id" | "section" | "mapping_section" | "offset" | "limit"
            )),
            "unknown surface_read field"
        );
        let surface_id = required(args, "surface_id")?;
        return match required(args, "section")? {
            "record" => {
                ensure!(
                    fields
                        .keys()
                        .all(|key| matches!(key.as_str(), "surface_id" | "section")),
                    "record read does not accept paging fields"
                );
                let mut result: Value = serde_json::from_str(&execute(
                    store,
                    workspace_id,
                    GET,
                    &json!({"id":surface_id}),
                    review_approved,
                )?)?;
                result["workspace_surfaces_revision"] = json!(
                    store
                        .surfaces_for_host(workspace_id)
                        .map_err(|status| anyhow::anyhow!("surface read failed: {status}"))?
                        .revision
                );
                Ok(result.to_string())
            }
            "mapping" => {
                let mut input = json!({"surface_id":surface_id});
                if let Some(section) = fields.get("mapping_section") {
                    input["section"] = section.clone();
                }
                if let Some(offset) = fields.get("offset") {
                    input["offset"] = offset.clone();
                }
                if let Some(limit) = fields.get("limit") {
                    input["limit"] = limit.clone();
                }
                mapping::execute(store, workspace_id, MAPPING_GET, &input, review_approved)
            }
            "threat_model" => {
                ensure!(
                    !fields.contains_key("mapping_section"),
                    "mapping_section applies only to mapping reads"
                );
                let mut input = json!({"surface_id":surface_id});
                if let Some(offset) = fields.get("offset") {
                    input["offset"] = offset.clone();
                }
                if let Some(limit) = fields.get("limit") {
                    input["limit"] = limit.clone();
                }
                threat_model::execute(store, workspace_id, THREAT_MODEL_GET, &input)
            }
            _ => bail!("invalid surface section"),
        };
    }
    if matches!(name, MAPPING_GET | MAPPING_EDIT | PROMOTE) {
        return mapping::execute(store, workspace_id, name, args, review_approved);
    }
    if matches!(name, THREAT_MODEL_GET | THREAT_MODEL_EDIT) {
        return threat_model::execute(store, workspace_id, name, args);
    }
    let current = store
        .surfaces_for_host(workspace_id)
        .map_err(|status| anyhow::anyhow!("surface read failed: {status}"))?;
    match name {
        GET => {
            let id = required(args, "id")?;
            let (surface, deleted) = store
                .surface_for_host(workspace_id, id)
                .map_err(|status| anyhow::anyhow!("surface read failed: {status}"))?
                .context("surface not found")?;
            Ok(json!({"surface":surface_json(&surface, true),"deleted":deleted}).to_string())
        }
        QUERY => {
            let type_id = args.get("type_id").and_then(Value::as_str);
            let filter_scope = args.get("scope").and_then(Value::as_str);
            let text = args
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_lowercase);
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(50);
            ensure!((1..=50).contains(&limit), "invalid limit");
            let matches = current
                .surfaces
                .iter()
                .filter(|surface| {
                    surface.definition.as_ref().is_some_and(|definition| {
                        type_id.is_none_or(|filter| definition.type_id == filter)
                            && filter_scope.is_none_or(|filter| {
                                (filter == "in_scope"
                                    && definition.scope == SurfaceScope::InScope as i32)
                                    || (filter == "out_of_scope"
                                        && definition.scope == SurfaceScope::OutOfScope as i32)
                            })
                            && text.as_ref().is_none_or(|needle| {
                                definition.name.to_lowercase().contains(needle)
                                    || definition.description.to_lowercase().contains(needle)
                                    || definition.locator.as_ref().is_some_and(|locator| {
                                        locator.value.to_lowercase().contains(needle)
                                    })
                            })
                    })
                })
                .take(limit as usize)
                .map(|surface| surface_json(surface, false))
                .collect::<Vec<_>>();
            Ok(json!({"revision":current.revision,"surfaces":matches}).to_string())
        }
        ADD | EDIT | SAVE => {
            let create = if name == SAVE {
                let fields = args.as_object().context("arguments must be an object")?;
                ensure!(
                    fields.keys().all(|key| matches!(
                        key.as_str(),
                        "operation" | "id" | "expected_revision" | "definition" | "relationships"
                    )),
                    "unknown surface_save field"
                );
                match required(args, "operation")? {
                    "create" => {
                        ensure!(
                            !fields.contains_key("id") && !fields.contains_key("expected_revision"),
                            "create does not accept id or expected_revision"
                        );
                        true
                    }
                    "revise" => {
                        required(args, "id")?;
                        let expected_revision = args
                            .get("expected_revision")
                            .and_then(Value::as_u64)
                            .context("expected_revision is required")?;
                        ensure!(
                            expected_revision == current.revision,
                            "surface revision conflict"
                        );
                        ensure!(
                            fields.contains_key("definition")
                                || fields.contains_key("relationships"),
                            "revise requires a change"
                        );
                        false
                    }
                    _ => bail!("invalid surface operation"),
                }
            } else {
                name == ADD
            };
            let mut inputs = current
                .surfaces
                .into_iter()
                .map(input_from_surface)
                .collect::<Vec<_>>();
            if create {
                let mut definition = definition(args.get("definition"), None)?;
                ensure!(
                    args.get("definition").is_some()
                        && !definition.type_id.is_empty()
                        && definition.locator.is_some()
                        && definition.scope != 0,
                    "surface definition requires type_id, locator, and scope"
                );
                definition.scope_state = if review_approved {
                    SurfaceScopeState::Approved
                } else {
                    SurfaceScopeState::Candidate
                } as i32;
                inputs.push(WorkspaceSurfaceInput {
                    id: String::new(),
                    definition: Some(definition),
                    relationships: relationships(args.get("relationships"), Vec::new())?,
                });
            } else {
                let id = required(args, "id")?;
                let item = inputs
                    .iter_mut()
                    .find(|item| item.id == id)
                    .context("surface not found")?;
                let mut updated = definition(args.get("definition"), item.definition.take())?;
                updated.scope_state = if review_approved {
                    SurfaceScopeState::Approved
                } else {
                    SurfaceScopeState::Candidate
                } as i32;
                item.definition = Some(updated);
                item.relationships = relationships(
                    args.get("relationships"),
                    std::mem::take(&mut item.relationships),
                )?;
            }
            let saved = store
                .update_surfaces_for_agent(workspace_id, current.revision, inputs)
                .map_err(|status| anyhow::anyhow!("surface update failed: {status}"))?;
            let id = if !create {
                required(args, "id")?.to_owned()
            } else {
                saved
                    .surfaces
                    .last()
                    .context("new surface missing")?
                    .id
                    .clone()
            };
            let surface = saved
                .surfaces
                .iter()
                .find(|item| item.id == id)
                .context("saved surface missing")?;
            let mut result = surface_json(surface, true);
            if name == SAVE {
                result["workspace_surfaces_revision"] = json!(saved.revision);
            }
            Ok(result.to_string())
        }
        _ => bail!("unknown surface tool"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use beale_contracts::{
        WORKSPACES_PATH,
        v1::{CreateWorkspaceRequest, WorkspaceSummary},
    };
    use prost::Message;
    use tower::ServiceExt;
    use uuid::Uuid;

    #[test]
    fn tool_contract_requires_definition_and_exposes_typed_evidence() {
        let tools = definitions();
        assert_eq!(tools[0]["name"], QUERY);
        assert_eq!(tools[1]["name"], READ);
        assert_eq!(tools[2]["name"], SAVE);
        assert_eq!(tools[2]["parameters"]["required"], json!(["operation"]));
        assert!(
            tools[2]["parameters"]["properties"]["definition"]["properties"]
                .get("parent_id")
                .is_none()
        );
        assert!(
            tools[2]["parameters"]["properties"]["definition"]["properties"]
                .get("evidence_refs")
                .is_some()
        );
        assert!(
            tools[2]["parameters"]["properties"]["relationships"]["items"]["properties"]
                .get("evidence_refs")
                .is_some()
        );
        assert_eq!(tools.len(), TOOL_COUNT as usize);
        assert_eq!(
            tools[4]["parameters"]["properties"]["definition"]["required"],
            json!(["type_id", "scope"])
        );
        assert_eq!(tools[5]["name"], THREAT_MODEL_EDIT);
        assert!(!is_tool(GET));
        assert!(!is_tool(MAPPING_GET));
        assert!(!is_tool(THREAT_MODEL_GET));
    }

    #[tokio::test]
    async fn threat_model_tools_track_local_scenarios_and_map_review() {
        let base =
            std::env::temp_dir().join(format!("beale-threat-tools-example-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();
        let store = WorkspaceStore::at(base.join("host")).unwrap();
        let token = "ab".repeat(32);
        let request = CreateWorkspaceRequest {
            name: "Example Workspace".to_owned(),
            directory: base.join("workspace").to_string_lossy().into_owned(),
            rules: vec!["Stay within recorded scope.".to_owned()],
            ..Default::default()
        };
        let response = beale_workspaces::router(token.clone(), store.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(WORKSPACES_PATH)
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/x-protobuf")
                    .body(Body::from(request.encode_to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let workspace =
            WorkspaceSummary::decode(to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        let added: Value = serde_json::from_str(&execute(&store, &workspace.id, SAVE,
            &json!({"operation":"create","definition":{"type_id":"repository","locator":{"kind":"freeform","value":"example repository"},"scope":"in_scope"}}), false).unwrap()).unwrap();
        assert_eq!(added["id"], "S1");
        assert_eq!(added["workspace_surfaces_revision"], 2);
        let mapped: Value = serde_json::from_str(&execute(&store, &workspace.id, MAPPING_EDIT,
            &json!({"surface_id":"S1","element":{"category":"interface","type_id":"web.http_route","name":"Example route","observation":"Accepts example requests.","basis":"observed","evidence_refs":[{"kind":"artifact","reference":"artifact-example-001"}]}}), false).unwrap()).unwrap();
        assert_eq!(mapped["changed_id"], "E1");
        let scenario = json!({"title":"Example authorization bypass","map_refs":[{"kind":"element","id":"E1"}],
            "attacker_position":"Unauthenticated client","preconditions":["Route is reachable"],
            "suspected_failure":"The route may omit authorization checks.",
            "invariant_at_risk":"Only authorized users may read records.",
            "potential_impact":"Unauthorized record disclosure.",
            "validation_question":"Does a request without a session return records?"});
        let saved: Value = serde_json::from_str(
            &execute(
                &store,
                &workspace.id,
                THREAT_MODEL_EDIT,
                &json!({"surface_id":"S1","scenario":scenario}),
                false,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(saved["changed_id"], "T1");
        let read: Value = serde_json::from_str(
            &execute(
                &store,
                &workspace.id,
                READ,
                &json!({"surface_id":"S1","section":"threat_model"}),
                false,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(read["scenarios"][0]["id"], "T1");
        assert_eq!(read["mapping_review_needed"], true);
        let reviewed: Value = serde_json::from_str(
            &execute(
                &store,
                &workspace.id,
                THREAT_MODEL_EDIT,
                &json!({"surface_id":"S1","review_mapping":true}),
                false,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(reviewed["reviewed_mapping_revision"], 2);
        execute(
            &store,
            &workspace.id,
            MAPPING_EDIT,
            &json!({"surface_id":"S1","remove":{"kind":"element","id":"E1"}}),
            false,
        )
        .unwrap();
        let stale: Value = serde_json::from_str(
            &execute(
                &store,
                &workspace.id,
                READ,
                &json!({"surface_id":"S1","section":"threat_model"}),
                false,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(stale["mapping_review_needed"], true);
        assert_eq!(stale["scenarios"][0]["stale_map_refs"][0]["id"], "E1");
        assert!(
            execute(
                &store,
                &workspace.id,
                THREAT_MODEL_EDIT,
                &json!({"surface_id":"S1","scenario":{"id":"T9"}}),
                false
            )
            .is_err()
        );
        drop(store);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[tokio::test]
    async fn tools_add_edit_get_and_query_workspace_surfaces() {
        let base =
            std::env::temp_dir().join(format!("beale-surface-tools-example-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();
        let store = WorkspaceStore::at(base.join("host")).unwrap();
        let token = "ab".repeat(32);
        let request = CreateWorkspaceRequest {
            name: "Example Workspace".to_owned(),
            directory: base.join("workspace").to_string_lossy().into_owned(),
            rules: vec!["Stay within the operator's recorded authorization.".to_owned()],
            ..Default::default()
        };
        let response = beale_workspaces::router(token.clone(), store.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(WORKSPACES_PATH)
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/x-protobuf")
                    .body(Body::from(request.encode_to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::CREATED);
        let workspace =
            WorkspaceSummary::decode(to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        let added: Value = serde_json::from_str(&execute(&store, &workspace.id, SAVE, &json!({"operation":"create","definition":{"type_id":"repository","name":"Example repository","locator":{"kind":"freeform","value":"example repository"},"scope":"in_scope","description":"Example research code.","evidence_refs":[{"kind":"artifact","reference":"artifact-example-001"}]} }), false).unwrap()).unwrap();
        assert_eq!(added["id"], "S1");
        assert_eq!(added["lineage"][0]["number"], 1);
        assert_eq!(added["definition"]["scope_state"], "candidate");
        assert_eq!(
            added["definition"]["evidence_refs"][0]["reference"],
            "artifact-example-001"
        );
        assert!(
            execute(
                &store,
                &workspace.id,
                SAVE,
                &json!({"operation":"revise","id":"S1","expected_revision":2,"definition":{"scope_state":"approved"}}),
                false
            )
            .is_err()
        );
        let edited: Value = serde_json::from_str(
            &execute(
                &store,
                &workspace.id,
                SAVE,
                &json!({"operation":"revise","id":"S1","expected_revision":2,"definition":{"description":"Example parser repository."}}),
                true,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(edited["lineage"][1]["number"], 2);
        assert_eq!(edited["definition"]["scope_state"], "approved");
        assert_eq!(edited["lineage"][1]["actor"], "agent");
        let fetched: Value = serde_json::from_str(
            &execute(
                &store,
                &workspace.id,
                READ,
                &json!({"surface_id":"S1","section":"record"}),
                false,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            fetched["surface"]["definition"]["description"],
            "Example parser repository."
        );
        assert_eq!(fetched["workspace_surfaces_revision"], 3);
        assert!(execute(&store, &workspace.id, SAVE, &json!({"operation":"revise","id":"S1","expected_revision":2,"definition":{"description":"Stale edit"}}), false).is_err());
        let queried: Value = serde_json::from_str(
            &execute(
                &store,
                &workspace.id,
                QUERY,
                &json!({"type_id":"repository","text":"parser"}),
                false,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(queried["surfaces"].as_array().unwrap().len(), 1);
        let mapped: Value = serde_json::from_str(&execute(&store, &workspace.id, MAPPING_EDIT, &json!({"surface_id":"S1","element":{"category":"interface","type_id":"code.export","name":"Parse input","locator":{"kind":"source_symbol","value":"src/parser.rs::parse_input"},"observation":"Accepts untrusted input.","basis":"observed","evidence_refs":[{"kind":"artifact","reference":"artifact-example-001"}]}}), false).unwrap()).unwrap();
        assert_eq!(mapped["changed_id"], "E1");
        assert_eq!(mapped["revision"], 2);
        let read: Value = serde_json::from_str(
            &execute(
                &store,
                &workspace.id,
                READ,
                &json!({"surface_id":"S1","section":"mapping","mapping_section":"elements"}),
                false,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(read["elements"][0]["basis"], "observed");
        assert!(execute(&store, &workspace.id, MAPPING_EDIT, &json!({"surface_id":"S1","flow":{"from_element_id":"E1","to_element_id":"E99","kind":"call","observation":"Calls a missing element.","basis":"inferred","evidence_refs":[{"kind":"artifact","reference":"artifact-example-001"}]}}), false).is_err());
        let promoted: Value = serde_json::from_str(&execute(&store, &workspace.id, PROMOTE, &json!({"surface_id":"S1","element_id":"E1","definition":{"type_id":"service","scope":"in_scope"}}), true).unwrap()).unwrap();
        assert_eq!(promoted["id"], "S2");
        assert_eq!(promoted["definition"]["scope_state"], "approved");
        let source: Value = serde_json::from_str(
            &execute(&store, &workspace.id, GET, &json!({"id":"S1"}), false).unwrap(),
        )
        .unwrap();
        assert_eq!(
            source["surface"]["relationships"][0]["source_element_id"],
            "E1"
        );
        assert!(execute(&store, &workspace.id, EDIT, &json!({"id":"S1","relationships":[{"target_id":"S2","label":"has part","type_id":"has_part","source_element_id":"E99"}]}), true).is_err());
        assert!(execute(&store, &workspace.id, PROMOTE, &json!({"surface_id":"S1","element_id":"E1","definition":{"type_id":"service","scope":"in_scope"}}), true).is_err());
        assert!(
            execute(
                &store,
                &workspace.id,
                EDIT,
                &json!({"id":"S1","definition":{"evidence_refs":[{"kind":"Invalid Kind","reference":"x"}]}}),
                false,
            )
            .is_err()
        );
        drop(store);
        std::fs::remove_dir_all(base).unwrap();
    }
}
