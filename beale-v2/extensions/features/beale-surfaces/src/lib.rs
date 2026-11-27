//! Optional model tools for workspace surface records.

use anyhow::{Context, Result, bail, ensure};
use beale_contracts::tool_error::ToolError;
use beale_contracts::v1::{
    SurfaceDefinition, SurfaceEvidence, SurfaceLocator, SurfaceRelationship, SurfaceScope,
    SurfaceScopeState, WorkspaceSurface, WorkspaceSurfaceInput,
};
use beale_workspaces::WorkspaceStore;
use serde_json::{Value, json};

mod threat_model;

pub const FEATURE_ID: &str = "surfaces";
pub const FEATURE_NAME: &str = "Surfaces";
pub const FEATURE_DESCRIPTION: &str =
    "Record surfaces and their parent-child and other relationships.";
pub const HAS_PROMPT_EXTENSION: bool = false;
pub const TOOL_COUNT: u32 = 4;

/// Catalog and session descriptor for this feature.
pub const FEATURE: beale_feature_api::Feature = beale_feature_api::Feature {
    id: FEATURE_ID,
    name: FEATURE_NAME,
    description: FEATURE_DESCRIPTION,
    prompt_extension: PROMPT_EXTENSION,
    use_case: "Use to inspect or maintain the workspace surface map and relationships.",
    tool_names: &[QUERY, READ, SAVE, THREAT_MODEL_EDIT],
    definitions: Some(definitions),
    review: beale_feature_api::ToolReview::Required,
    subagents: false,
    migrations: &[],
    startup: &[],
};
pub const PROMPT_EXTENSION: &str = "";

pub const ADD: &str = "surface_add";
pub const EDIT: &str = "surface_edit";
pub const GET: &str = "surface_get";
pub const QUERY: &str = "surface_query";
pub const THREAT_MODEL_GET: &str = "surface_threat_model_get";
pub const THREAT_MODEL_EDIT: &str = "surface_threat_model_edit";
pub const READ: &str = "surface_read";
pub const SAVE: &str = "surface_save";

pub fn definitions() -> Vec<Value> {
    let evidence = json!({"type":"object","properties":{"kind":{"type":"string"},"reference":{"type":"string"},"note":{"type":"string"}},"required":["kind","reference"],"additionalProperties":false});
    let locator = json!({"type":"object","properties":{"kind":{"type":"string","description":"Use repository_path or freeform for code.module, code.package, and code.file; use url, git_url, or freeform for repository. Other surface types accept their matching locator kinds."},"value":{"type":"string"}},"required":["kind","value"],"additionalProperties":false});
    let definition = json!({"type":"object","properties":{"type_id":{"type":"string","description":"Built-in types: repository, code.module, code.package, code.file, documentation, binary, service, api.endpoint, domain, host, container, configuration, other. Extensions use kit.* or custom.*. Use code.module for a module, not module."},"name":{"type":"string"},"locator":locator,"scope":{"type":"string","enum":["in_scope","out_of_scope"]},"description":{"type":"string"},"out_of_scope_reason":{"type":"string"},"evidence_refs":{"type":"array","items":evidence}},"additionalProperties":false});
    let relationship = json!({"type":"object","properties":{"target_id":{"type":"string"},"label":{"type":"string"},"type_id":{"type":"string"},"evidence_refs":{"type":"array","items":evidence}},"required":["target_id","label"],"additionalProperties":false});
    let scenario = json!({"type":"object","properties":{"id":{"type":"string"},"title":{"type":"string"},"attacker_position":{"type":"string"},"preconditions":{"type":"array","items":{"type":"string"}},"suspected_failure":{"type":"string"},"invariant_at_risk":{"type":"string"},"potential_impact":{"type":"string"},"validation_question":{"type":"string"},"evidence_refs":{"type":"array","items":evidence},"disposition":{"type":"string","enum":["open","dismissed"]},"disposition_reason":{"type":"string"},"related_claim_ids":{"type":"array","items":{"type":"string"}}},"required":["title","attacker_position","preconditions","suspected_failure","invariant_at_risk","potential_impact","validation_question"],"additionalProperties":false});
    vec![
        json!({"type":"function","name":QUERY,"description":"Query surfaces in this workspace by type, scope, or text. Returns at most 50 surfaces.","parameters":{"type":"object","properties":{"type_id":{"type":"string"},"scope":{"type":"string","enum":["in_scope","out_of_scope"]},"text":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":50}},"additionalProperties":false}}),
        json!({"type":"function","name":READ,"description":"Read one surface record and lineage, or its threat scenarios.","parameters":{"type":"object","properties":{"surface_id":{"type":"string"},"section":{"type":"string","enum":["record","threat_model"]},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":50}},"required":["surface_id","section"],"additionalProperties":false}}),
        json!({"type":"function","name":SAVE,"description":"Create or revise a surface and its relationships. Set parent_id to place a child beneath exactly one parent; omit it for a top-level surface. Create requires type_id, locator, and scope; revise requires id and workspace_surfaces_revision from surface_read.","parameters":{"type":"object","properties":{"operation":{"type":"string","enum":["create","revise"]},"id":{"type":"string"},"expected_revision":{"type":"integer","minimum":1},"parent_id":{"type":"string"},"definition":definition,"relationships":{"type":"array","items":relationship}},"required":["operation"],"additionalProperties":false}}),
        json!({"type":"function","name":THREAT_MODEL_EDIT,"description":"Add or replace a plausible failure for this surface, or remove a scenario. Record investigation results as Knowledge claims.","parameters":{"type":"object","properties":{"surface_id":{"type":"string"},"scenario":scenario,"remove_id":{"type":"string"}},"required":["surface_id"],"additionalProperties":false}}),
    ]
}

pub fn is_tool(name: &str) -> bool {
    matches!(name, QUERY | READ | SAVE | THREAT_MODEL_EDIT)
}

fn summary_value(value: &str) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut result = String::new();
    for character in normalized.chars() {
        if result.len() + character.len_utf8() > 320 {
            break;
        }
        result.push(character);
    }
    result
}

pub fn summary(store: &WorkspaceStore, workspace_id: &str, name: &str, args: &Value) -> String {
    if name == QUERY {
        return args
            .get("text")
            .and_then(Value::as_str)
            .map(summary_value)
            .filter(|value| !value.is_empty())
            .map_or_else(
                || "Surface query".to_owned(),
                |value| format!("Surface query: {value}"),
            );
    }
    let selected_name = (name == SAVE)
        .then(|| args.get("definition")?.get("name")?.as_str())
        .flatten()
        .map(summary_value)
        .filter(|value| !value.is_empty());
    let id = match name {
        SAVE => args.get("id"),
        READ | THREAT_MODEL_EDIT => args.get("surface_id"),
        _ => None,
    }
    .and_then(Value::as_str);
    selected_name
        .or_else(|| {
            id.and_then(|id| store.surface_for_host(workspace_id, id).ok().flatten())
                .and_then(|(surface, _)| surface.definition)
                .map(|definition| summary_value(&definition.name))
                .filter(|value| !value.is_empty())
        })
        .map_or_else(
            || "Surface".to_owned(),
            |value| format!("Surface name: {value}"),
        )
}

fn required<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .with_context(|| ToolError::invalid(format!("{key} is required")))
}

fn optional_text<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>> {
    args.get(key)
        .map(|value| {
            value
                .as_str()
                .with_context(|| ToolError::invalid(format!("{key} must be a string")))
        })
        .transpose()
}

fn scope(value: &str) -> Result<i32> {
    Ok(match value {
        "in_scope" => SurfaceScope::InScope,
        "out_of_scope" => SurfaceScope::OutOfScope,
        _ => bail!(ToolError::invalid("invalid surface scope")),
    } as i32)
}

fn evidence_refs(
    value: Option<&Value>,
    previous: Vec<SurfaceEvidence>,
) -> Result<Vec<SurfaceEvidence>> {
    let Some(value) = value else {
        return Ok(previous);
    };
    let items = value
        .as_array()
        .context(ToolError::invalid("evidence_refs must be an array"))?;
    ensure!(
        items.len() <= 16,
        ToolError::invalid("too many evidence references")
    );
    items
        .iter()
        .map(|item| {
            let object = item
                .as_object()
                .context(ToolError::invalid("invalid evidence reference"))?;
            ensure!(
                object
                    .keys()
                    .all(|key| matches!(key.as_str(), "kind" | "reference" | "note")),
                ToolError::invalid("unknown evidence field")
            );
            Ok(SurfaceEvidence {
                kind: object
                    .get("kind")
                    .and_then(Value::as_str)
                    .context(ToolError::invalid("evidence kind is required"))?
                    .to_owned(),
                reference: object
                    .get("reference")
                    .and_then(Value::as_str)
                    .context(ToolError::invalid("evidence reference is required"))?
                    .to_owned(),
                note: optional_text(item, "note")?.unwrap_or("").to_owned(),
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
    let fields = value
        .as_object()
        .context(ToolError::invalid("definition must be an object"))?;
    for (key, value) in fields {
        match key.as_str() {
            "type_id" => {
                result.type_id = value
                    .as_str()
                    .context(ToolError::invalid("invalid type_id"))?
                    .to_owned()
            }
            "name" => {
                result.name = value
                    .as_str()
                    .context(ToolError::invalid("invalid name"))?
                    .to_owned()
            }
            "scope" => {
                result.scope = scope(
                    value
                        .as_str()
                        .context(ToolError::invalid("invalid scope"))?,
                )?
            }
            "description" => {
                result.description = value
                    .as_str()
                    .context(ToolError::invalid("invalid description"))?
                    .to_owned()
            }
            "out_of_scope_reason" => {
                result.out_of_scope_reason = value
                    .as_str()
                    .context(ToolError::invalid("invalid out_of_scope_reason"))?
                    .to_owned()
            }
            "evidence_refs" => result.evidence_refs = evidence_refs(Some(value), Vec::new())?,
            "locator" => {
                let locator = value
                    .as_object()
                    .context(ToolError::invalid("invalid locator"))?;
                ensure!(
                    locator
                        .keys()
                        .all(|key| matches!(key.as_str(), "kind" | "value")),
                    ToolError::invalid("unknown locator field")
                );
                result.locator = Some(SurfaceLocator {
                    kind: locator
                        .get("kind")
                        .and_then(Value::as_str)
                        .context(ToolError::invalid("locator kind is required"))?
                        .to_owned(),
                    value: locator
                        .get("value")
                        .and_then(Value::as_str)
                        .context(ToolError::invalid("locator value is required"))?
                        .to_owned(),
                });
            }
            _ => bail!(ToolError::invalid("unknown definition field")),
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
    let items = value
        .as_array()
        .context(ToolError::invalid("relationships must be an array"))?;
    ensure!(
        items.len() <= 64,
        ToolError::invalid("too many relationships")
    );
    items
        .iter()
        .map(|item| {
            let item = item
                .as_object()
                .context(ToolError::invalid("invalid relationship"))?;
            ensure!(
                item.keys().all(|key| matches!(
                    key.as_str(),
                    "target_id" | "label" | "type_id" | "evidence_refs"
                )),
                ToolError::invalid("unknown relationship field")
            );
            Ok(SurfaceRelationship {
                target_id: item
                    .get("target_id")
                    .and_then(Value::as_str)
                    .context(ToolError::invalid("relationship target_id is required"))?
                    .to_owned(),
                label: item
                    .get("label")
                    .and_then(Value::as_str)
                    .context(ToolError::invalid("relationship label is required"))?
                    .to_owned(),
                type_id: item
                    .get("type_id")
                    .map(|value| {
                        value
                            .as_str()
                            .context(ToolError::invalid("type_id must be a string"))
                    })
                    .transpose()?
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
    ensure!(
        args.is_object(),
        ToolError::invalid("arguments must be an object")
    );
    if name == READ {
        let fields = args
            .as_object()
            .context(ToolError::invalid("arguments must be an object"))?;
        ensure!(
            fields
                .keys()
                .all(|key| matches!(key.as_str(), "surface_id" | "section" | "offset" | "limit")),
            ToolError::invalid("unknown surface_read field")
        );
        let surface_id = required(args, "surface_id")?;
        return match required(args, "section")? {
            "record" => {
                ensure!(
                    fields
                        .keys()
                        .all(|key| matches!(key.as_str(), "surface_id" | "section")),
                    ToolError::invalid("record read does not accept paging fields")
                );
                let (surface, deleted, revision) = store
                    .surface_with_revision_for_host(workspace_id, surface_id)
                    .map_err(|status| anyhow::anyhow!("surface read failed: {status}"))?
                    .context(ToolError::missing("surface not found"))?;
                Ok(json!({"surface":surface_json(&surface, true),"deleted":deleted,"workspace_surfaces_revision":revision}).to_string())
            }
            "threat_model" => {
                let mut input = json!({"surface_id":surface_id});
                if let Some(offset) = fields.get("offset") {
                    input["offset"] = offset.clone();
                }
                if let Some(limit) = fields.get("limit") {
                    input["limit"] = limit.clone();
                }
                threat_model::execute(store, workspace_id, THREAT_MODEL_GET, &input)
            }
            _ => bail!(ToolError::invalid("invalid surface section")),
        };
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
                .context(ToolError::missing("surface not found"))?;
            Ok(json!({"surface":surface_json(&surface, true),"deleted":deleted}).to_string())
        }
        QUERY => {
            let type_id = optional_text(args, "type_id")?;
            let filter_scope = optional_text(args, "scope")?;
            if let Some(filter) = filter_scope {
                scope(filter)?;
            }
            let text = optional_text(args, "text")?.map(str::to_lowercase);
            let limit = args
                .get("limit")
                .map(|value| beale_contracts::tool_error::arguments::<u64>(value.clone()))
                .transpose()?
                .unwrap_or(50);
            ensure!(
                (1..=50).contains(&limit),
                ToolError::invalid("invalid limit")
            );
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
            let requested_parent = args
                .get("parent_id")
                .map(|value| {
                    value
                        .as_str()
                        .context(ToolError::invalid("parent_id must be a string"))
                })
                .transpose()?;
            let create = if name == SAVE {
                let fields = args
                    .as_object()
                    .context(ToolError::invalid("arguments must be an object"))?;
                ensure!(
                    fields.keys().all(|key| matches!(
                        key.as_str(),
                        "operation"
                            | "id"
                            | "expected_revision"
                            | "parent_id"
                            | "definition"
                            | "relationships"
                    )),
                    ToolError::invalid("unknown surface_save field")
                );
                match required(args, "operation")? {
                    "create" => {
                        ensure!(
                            !fields.contains_key("id") && !fields.contains_key("expected_revision"),
                            ToolError::invalid("create does not accept id or expected_revision")
                        );
                        true
                    }
                    "revise" => {
                        required(args, "id")?;
                        let expected_revision = args
                            .get("expected_revision")
                            .and_then(Value::as_u64)
                            .context(ToolError::invalid("expected_revision is required"))?;
                        ensure!(
                            expected_revision == current.revision,
                            ToolError::conflict("surface revision conflict")
                        );
                        ensure!(
                            fields.contains_key("definition")
                                || fields.contains_key("relationships")
                                || fields.contains_key("parent_id"),
                            ToolError::invalid("revise requires a change")
                        );
                        false
                    }
                    _ => bail!(ToolError::invalid("invalid surface operation")),
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
                    ToolError::invalid("surface definition requires type_id, locator, and scope")
                );
                definition.scope_state = if review_approved {
                    SurfaceScopeState::Approved
                } else {
                    SurfaceScopeState::Candidate
                } as i32;
                if let Some(hint) = beale_workspaces::surface_definition_hint(&definition) {
                    bail!(ToolError::invalid(hint));
                }
                let new_id = "draft-new-surface";
                if let Some(parent_id) = requested_parent.filter(|id| !id.is_empty()) {
                    let parent = inputs
                        .iter_mut()
                        .find(|item| item.id == parent_id)
                        .context(ToolError::missing("parent surface not found"))?;
                    parent.relationships.push(SurfaceRelationship {
                        target_id: new_id.to_owned(),
                        label: "contains".to_owned(),
                        type_id: "contains".to_owned(),
                        ..Default::default()
                    });
                }
                inputs.push(WorkspaceSurfaceInput {
                    id: new_id.to_owned(),
                    definition: Some(definition),
                    relationships: relationships(args.get("relationships"), Vec::new())?,
                });
            } else {
                let id = required(args, "id")?;
                let item = inputs
                    .iter_mut()
                    .find(|item| item.id == id)
                    .context(ToolError::missing("surface not found"))?;
                let mut updated = definition(args.get("definition"), item.definition.take())?;
                updated.scope_state = if review_approved {
                    SurfaceScopeState::Approved
                } else {
                    SurfaceScopeState::Candidate
                } as i32;
                if let Some(hint) = beale_workspaces::surface_definition_hint(&updated) {
                    bail!(ToolError::invalid(hint));
                }
                item.definition = Some(updated);
                item.relationships = relationships(
                    args.get("relationships"),
                    std::mem::take(&mut item.relationships),
                )?;
                if let Some(parent_id) = requested_parent {
                    ensure!(
                        parent_id != id,
                        ToolError::invalid("a surface cannot parent itself")
                    );
                    for surface in &mut inputs {
                        surface
                            .relationships
                            .retain(|link| link.type_id != "contains" || link.target_id != id);
                    }
                    if !parent_id.is_empty() {
                        let parent = inputs
                            .iter_mut()
                            .find(|item| item.id == parent_id)
                            .context(ToolError::missing("parent surface not found"))?;
                        parent.relationships.push(SurfaceRelationship {
                            target_id: id.to_owned(),
                            label: "contains".to_owned(),
                            type_id: "contains".to_owned(),
                            ..Default::default()
                        });
                    }
                }
            }
            let saved = store.update_surfaces_for_agent(workspace_id, current.revision, inputs)?;
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
            tools[2]["parameters"]["properties"]
                .get("parent_id")
                .is_some()
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
        assert_eq!(tools[3]["name"], THREAT_MODEL_EDIT);
        assert!(!is_tool(GET));
        assert!(!is_tool(THREAT_MODEL_GET));
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
        let invalid_type = execute(
            &store,
            &workspace.id,
            SAVE,
            &json!({"operation":"create","definition":{"type_id":"module","locator":{"kind":"url","value":"https://example.test/module"},"scope":"in_scope"}}),
            false,
        )
        .unwrap_err();
        assert!(
            invalid_type
                .to_string()
                .contains("use repository, code.module")
        );
        let invalid_locator = execute(
            &store,
            &workspace.id,
            SAVE,
            &json!({"operation":"create","definition":{"type_id":"code.module","locator":{"kind":"url","value":"https://example.test/module"},"scope":"in_scope"}}),
            false,
        )
        .unwrap_err();
        assert!(
            invalid_locator
                .to_string()
                .contains("locator kind is incompatible")
        );
        let added: Value = serde_json::from_str(&execute(&store, &workspace.id, SAVE, &json!({"operation":"create","definition":{"type_id":"repository","name":"Example repository","locator":{"kind":"freeform","value":"example repository"},"scope":"in_scope","description":"Example research code.","evidence_refs":[{"kind":"artifact","reference":"artifact-example-001"}]} }), false).unwrap()).unwrap();
        assert_eq!(
            summary(&store, &workspace.id, QUERY, &json!({"text":"parser"})),
            "Surface query: parser"
        );
        assert_eq!(
            summary(&store, &workspace.id, QUERY, &json!({"scope":"in_scope"})),
            "Surface query"
        );
        assert_eq!(
            summary(&store, &workspace.id, READ, &json!({"surface_id":"S1"})),
            "Surface name: Example repository"
        );
        assert_eq!(
            summary(
                &store,
                &workspace.id,
                SAVE,
                &json!({"id":"S1","operation":"revise"})
            ),
            "Surface name: Example repository"
        );
        assert_eq!(
            summary(
                &store,
                &workspace.id,
                THREAT_MODEL_EDIT,
                &json!({"surface_id":"S1"})
            ),
            "Surface name: Example repository"
        );
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
        let child: Value = serde_json::from_str(&execute(
            &store, &workspace.id, SAVE,
            &json!({"operation":"create","parent_id":"S1","definition":{"type_id":"repository","name":"Example child","locator":{"kind":"freeform","value":"example child"},"scope":"in_scope"}}),
            false,
        ).unwrap()).unwrap();
        assert_eq!(child["id"], "S2");
        let parent: Value = serde_json::from_str(
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
        assert_eq!(parent["surface"]["relationships"][0]["target_id"], "S2");
        assert_eq!(parent["surface"]["relationships"][0]["type_id"], "contains");
        let scenario: Value = serde_json::from_str(&execute(
            &store, &workspace.id, THREAT_MODEL_EDIT,
            &json!({"surface_id":"S2","scenario":{"title":"Example trust-boundary failure","attacker_position":"Remote client","preconditions":[],"suspected_failure":"Example validation gap","invariant_at_risk":"Example invariant","potential_impact":"Example impact","validation_question":"Can the boundary be bypassed?"}}), false,
        ).unwrap()).unwrap();
        assert_eq!(scenario["changed_id"], "T1");
        let model: Value = serde_json::from_str(
            &execute(
                &store,
                &workspace.id,
                READ,
                &json!({"surface_id":"S2","section":"threat_model"}),
                false,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            model["scenarios"][0]["title"],
            "Example trust-boundary failure"
        );
        drop(store);
        std::fs::remove_dir_all(base).unwrap();
    }
}
