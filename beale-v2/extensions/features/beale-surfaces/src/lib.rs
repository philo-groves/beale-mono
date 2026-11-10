//! Optional model tools for workspace surface records.

use anyhow::{Context, Result, bail, ensure};
use beale_contracts::v1::{
    SurfaceDefinition, SurfaceEvidence, SurfaceLocator, SurfaceRelationship, SurfaceScope,
    SurfaceScopeState, WorkspaceSurface, WorkspaceSurfaceInput,
};
use beale_workspaces::WorkspaceStore;
use serde_json::{Value, json};

pub const FEATURE_ID: &str = "surfaces";
pub const FEATURE_NAME: &str = "Surfaces";
pub const FEATURE_DESCRIPTION: &str =
    "Track research surfaces, their relationships, and revision lineage in a workspace.";
pub const HAS_PROMPT_EXTENSION: bool = false;
pub const TOOL_COUNT: u32 = 4;
pub const PROMPT_EXTENSION: &str = "";

pub const ADD: &str = "surface_add";
pub const EDIT: &str = "surface_edit";
pub const GET: &str = "surface_get";
pub const QUERY: &str = "surface_query";

pub fn definitions() -> Vec<Value> {
    let evidence = json!({"type":"object","properties":{"kind":{"type":"string"},"reference":{"type":"string"},"note":{"type":"string"}},"required":["kind","reference"],"additionalProperties":false});
    let locator = json!({"type":"object","properties":{"kind":{"type":"string"},"value":{"type":"string"}},"required":["kind","value"],"additionalProperties":false});
    let definition = json!({"type":"object","properties":{"type_id":{"type":"string"},"name":{"type":"string"},"locator":locator,"scope":{"type":"string","enum":["in_scope","out_of_scope"]},"description":{"type":"string"},"out_of_scope_reason":{"type":"string"},"evidence_refs":{"type":"array","items":evidence}},"additionalProperties":false});
    let mut add_definition = definition.clone();
    add_definition["required"] = json!(["type_id", "locator", "scope"]);
    let relationship = json!({"type":"object","properties":{"target_id":{"type":"string"},"label":{"type":"string"},"type_id":{"type":"string"},"evidence_refs":{"type":"array","items":evidence}},"required":["target_id","label"],"additionalProperties":false});
    vec![
        json!({"type":"function","name":ADD,"description":"Add a surface to this workspace. Supply a definition with type_id, locator, and scope. The host assigns its short ID; evidence references are candidate context, not proof of authorization.","parameters":{"type":"object","properties":{"definition":add_definition,"relationships":{"type":"array","items":relationship}},"required":["definition"],"additionalProperties":false}}),
        json!({"type":"function","name":EDIT,"description":"Edit a surface by short ID. Supply only definition fields or relationships to change. The host records a new revision.","parameters":{"type":"object","properties":{"id":{"type":"string"},"definition":definition,"relationships":{"type":"array","items":relationship}},"required":["id"],"additionalProperties":false}}),
        json!({"type":"function","name":GET,"description":"Get one surface and its revision lineage by short ID.","parameters":{"type":"object","properties":{"id":{"type":"string"}},"required":["id"],"additionalProperties":false}}),
        json!({"type":"function","name":QUERY,"description":"Query surfaces in this workspace by type, scope, or text. Returns at most 50 surfaces.","parameters":{"type":"object","properties":{"type_id":{"type":"string"},"scope":{"type":"string","enum":["in_scope","out_of_scope"]},"text":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":50}},"additionalProperties":false}}),
    ]
}

pub fn is_tool(name: &str) -> bool {
    matches!(name, ADD | EDIT | GET | QUERY)
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
                    "target_id" | "label" | "type_id" | "evidence_refs"
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
    json!({"id":surface.id,"definition":definition,"relationships":surface.relationships.iter().map(|item| json!({"target_id":item.target_id,"label":item.label,"type_id":item.type_id,"evidence_refs":item.evidence_refs.iter().map(evidence_json).collect::<Vec<_>>()})).collect::<Vec<_>>(),"lineage":lineage})
}

pub fn execute(
    store: &WorkspaceStore,
    workspace_id: &str,
    name: &str,
    args: &Value,
    review_approved: bool,
) -> Result<String> {
    ensure!(args.is_object(), "arguments must be an object");
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
        ADD | EDIT => {
            let mut inputs = current
                .surfaces
                .into_iter()
                .map(input_from_surface)
                .collect::<Vec<_>>();
            if name == ADD {
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
            let id = if name == EDIT {
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
            Ok(surface_json(surface, true).to_string())
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
        assert_eq!(tools[0]["parameters"]["required"][0], "definition");
        assert_eq!(
            tools[0]["parameters"]["properties"]["definition"]["required"],
            json!(["type_id", "locator", "scope"])
        );
        assert!(
            tools[0]["parameters"]["properties"]["definition"]["properties"]
                .get("parent_id")
                .is_none()
        );
        assert!(
            tools[0]["parameters"]["properties"]["definition"]["properties"]
                .get("evidence_refs")
                .is_some()
        );
        assert!(
            tools[0]["parameters"]["properties"]["relationships"]["items"]["properties"]
                .get("evidence_refs")
                .is_some()
        );
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
        let added: Value = serde_json::from_str(&execute(&store, &workspace.id, ADD, &json!({"definition":{"type_id":"repository","name":"Example repository","locator":{"kind":"freeform","value":"example repository"},"scope":"in_scope","description":"Example research code.","evidence_refs":[{"kind":"artifact","reference":"artifact-example-001"}]} }), false).unwrap()).unwrap();
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
                EDIT,
                &json!({"id":"S1","definition":{"scope_state":"approved"}}),
                false
            )
            .is_err()
        );
        let edited: Value = serde_json::from_str(
            &execute(
                &store,
                &workspace.id,
                EDIT,
                &json!({"id":"S1","definition":{"description":"Example parser repository."}}),
                true,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(edited["lineage"][1]["number"], 2);
        assert_eq!(edited["definition"]["scope_state"], "approved");
        assert_eq!(edited["lineage"][1]["actor"], "agent");
        let fetched: Value = serde_json::from_str(
            &execute(&store, &workspace.id, GET, &json!({"id":"S1"}), false).unwrap(),
        )
        .unwrap();
        assert_eq!(
            fetched["surface"]["definition"]["description"],
            "Example parser repository."
        );
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
