//! Optional model tools for workspace surface records.

use anyhow::{Context, Result, bail, ensure};
use beale_contracts::v1::{
    SurfaceDefinition, SurfaceKind, SurfaceLocator, SurfaceRelationship, SurfaceScope,
    WorkspaceSurface, WorkspaceSurfaceInput,
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
    let definition = json!({"type":"object","properties":{
        "type_id":{"type":"string"},"name":{"type":"string"},
        "locator":{"type":"object","properties":{"kind":{"type":"string"},"value":{"type":"string"}},"required":["kind","value"],"additionalProperties":false},
        "parent_id":{"type":"string"},"summary":{"type":"string"},"context":{"type":"string"},
        "scope_basis":{"type":"string"},"scope_descendants":{"type":"boolean"}
    },"additionalProperties":false});
    let relationship = json!({"type":"object","properties":{"target_id":{"type":"string"},"label":{"type":"string"},"type_id":{"type":"string"}},"required":["target_id","label"],"additionalProperties":false});
    vec![
        json!({"type":"function","name":ADD,"description":"Add a surface to this workspace. The host assigns its short ID. A surface record is context, not proof of authorization.","parameters":{"type":"object","properties":{"kind":{"type":"string","enum":["repository","documentation","binary","service","domain","other"]},"scope":{"type":"string","enum":["in_scope","out_of_scope"]},"value":{"type":"string"},"definition":definition,"relationships":{"type":"array","items":relationship}},"required":["kind","scope","value"],"additionalProperties":false}}),
        json!({"type":"function","name":EDIT,"description":"Edit a surface by short ID. Supply only fields to change. The host records a new revision with before and after values.","parameters":{"type":"object","properties":{"id":{"type":"string"},"kind":{"type":"string","enum":["repository","documentation","binary","service","domain","other"]},"scope":{"type":"string","enum":["in_scope","out_of_scope"]},"value":{"type":"string"},"definition":definition,"relationships":{"type":"array","items":relationship}},"required":["id"],"additionalProperties":false}}),
        json!({"type":"function","name":GET,"description":"Get one surface and its revision lineage by short ID.","parameters":{"type":"object","properties":{"id":{"type":"string"}},"required":["id"],"additionalProperties":false}}),
        json!({"type":"function","name":QUERY,"description":"Query surfaces in this workspace by type, scope, parent, or text. Returns at most 50 summaries.","parameters":{"type":"object","properties":{"type_id":{"type":"string"},"scope":{"type":"string","enum":["in_scope","out_of_scope"]},"parent_id":{"type":"string"},"text":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":50}},"additionalProperties":false}}),
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

fn kind(value: &str) -> Result<i32> {
    Ok(match value {
        "repository" => SurfaceKind::Repository,
        "documentation" => SurfaceKind::Documentation,
        "binary" => SurfaceKind::Binary,
        "service" => SurfaceKind::Service,
        "domain" => SurfaceKind::Domain,
        "other" => SurfaceKind::Other,
        _ => bail!("invalid surface kind"),
    } as i32)
}

fn scope(value: &str) -> Result<i32> {
    Ok(match value {
        "in_scope" => SurfaceScope::InScope,
        "out_of_scope" => SurfaceScope::OutOfScope,
        _ => bail!("invalid surface scope"),
    } as i32)
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
            "parent_id" => {
                result.parent_id = value.as_str().context("invalid parent_id")?.to_owned()
            }
            "summary" => result.summary = value.as_str().context("invalid summary")?.to_owned(),
            "context" => result.context = value.as_str().context("invalid context")?.to_owned(),
            "scope_basis" => {
                result.scope_basis = value.as_str().context("invalid scope_basis")?.to_owned()
            }
            "scope_descendants" => {
                result.scope_descendants = value.as_bool().context("invalid scope_descendants")?
            }
            "locator" => {
                let locator = value.as_object().context("invalid locator")?;
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
            })
        })
        .collect()
}

fn input_from_surface(surface: WorkspaceSurface) -> WorkspaceSurfaceInput {
    WorkspaceSurfaceInput {
        kind: surface.kind,
        scope: surface.scope,
        value: surface.value,
        id: surface.id,
        relationships: surface.relationships,
        definition: surface.definition,
        ..Default::default()
    }
}

fn surface_json(surface: &WorkspaceSurface, include_lineage: bool) -> Value {
    let definition = surface.definition.as_ref();
    let kind_name = match SurfaceKind::try_from(surface.kind) {
        Ok(SurfaceKind::Repository) => "repository",
        Ok(SurfaceKind::Documentation) => "documentation",
        Ok(SurfaceKind::Binary) => "binary",
        Ok(SurfaceKind::Service) => "service",
        Ok(SurfaceKind::Domain) => "domain",
        _ => "other",
    };
    let scope_name = if surface.scope == SurfaceScope::InScope as i32 {
        "in_scope"
    } else {
        "out_of_scope"
    };
    let mut value = json!({
        "id":surface.id,"kind":kind_name,
        "scope":scope_name,
        "value":surface.value,"created_by":surface.created_by,"out_of_scope_reason":surface.out_of_scope_reason,
        "created_at_ms":surface.created_at_ms,"updated_at_ms":surface.updated_at_ms,"revision":surface.revision,
        "definition":definition.map(|item| json!({"type_id":item.type_id,"name":item.name,"locator":item.locator.as_ref().map(|locator| json!({"kind":locator.kind,"value":locator.value})),"parent_id":item.parent_id,"summary":item.summary,"context":item.context,"scope_basis":item.scope_basis,"scope_descendants":item.scope_descendants,"source":item.source})),
        "relationships":surface.relationships.iter().map(|item| json!({"target_id":item.target_id,"label":item.label,"type_id":item.type_id})).collect::<Vec<_>>()
    });
    if include_lineage {
        value["lineage"] = json!(surface.lineage.iter().map(|event| json!({
            "number":event.number,"at_ms":event.at_ms,"actor":event.actor,"action":event.action,
            "changes":event.changes.iter().map(|change| json!({"field":change.field,"before":serde_json::from_str::<Value>(&change.before_json).unwrap_or(Value::Null),"after":serde_json::from_str::<Value>(&change.after_json).unwrap_or(Value::Null)})).collect::<Vec<_>>()
        })).collect::<Vec<_>>());
    }
    value
}

pub fn execute(
    store: &WorkspaceStore,
    workspace_id: &str,
    name: &str,
    args: &Value,
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
            let mut value = surface_json(&surface, true);
            value["deleted"] = json!(deleted);
            Ok(value.to_string())
        }
        QUERY => {
            let type_id = args.get("type_id").and_then(Value::as_str);
            let filter_scope = args.get("scope").and_then(Value::as_str);
            let parent_id = args.get("parent_id").and_then(Value::as_str);
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
                    let definition = surface.definition.as_ref();
                    type_id.is_none_or(|type_id| {
                        definition.is_some_and(|item| item.type_id == type_id)
                    }) && filter_scope.is_none_or(|scope| {
                        (scope == "in_scope" && surface.scope == SurfaceScope::InScope as i32)
                            || (scope == "out_of_scope"
                                && surface.scope == SurfaceScope::OutOfScope as i32)
                    }) && parent_id
                        .is_none_or(|id| definition.is_some_and(|item| item.parent_id == id))
                        && text.as_ref().is_none_or(|needle| {
                            surface.value.to_lowercase().contains(needle)
                                || definition.is_some_and(|item| {
                                    item.name.to_lowercase().contains(needle)
                                        || item.summary.to_lowercase().contains(needle)
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
                let mut item = WorkspaceSurfaceInput {
                    kind: kind(required(args, "kind")?)?,
                    scope: scope(required(args, "scope")?)?,
                    value: required(args, "value")?.to_owned(),
                    definition: Some(definition(args.get("definition"), None)?),
                    relationships: relationships(args.get("relationships"), Vec::new())?,
                    ..Default::default()
                };
                if let Some(definition) = &mut item.definition {
                    definition.source = "agent".to_owned();
                }
                inputs.push(item);
            } else {
                let id = required(args, "id")?;
                let item = inputs
                    .iter_mut()
                    .find(|item| item.id == id)
                    .context("surface not found")?;
                if let Some(value) = args.get("kind") {
                    item.kind = kind(value.as_str().context("invalid kind")?)?;
                }
                if let Some(value) = args.get("scope") {
                    item.scope = scope(value.as_str().context("invalid scope")?)?;
                }
                if let Some(value) = args.get("value") {
                    item.value = value.as_str().context("invalid value")?.to_owned();
                }
                if args.get("definition").is_some() {
                    item.definition =
                        Some(definition(args.get("definition"), item.definition.take())?);
                }
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
        let added: Value = serde_json::from_str(&execute(&store, &workspace.id, ADD, &json!({"kind":"repository","scope":"in_scope","value":"example repository","definition":{"name":"Example repository","scope_basis":"Operator supplied authorization."}})).unwrap()).unwrap();
        assert_eq!(added["id"], "S1");
        assert_eq!(added["revision"], 1);
        let edited: Value = serde_json::from_str(
            &execute(
                &store,
                &workspace.id,
                EDIT,
                &json!({"id":"S1","definition":{"summary":"Example parser repository."}}),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(edited["revision"], 2);
        assert_eq!(edited["lineage"][1]["actor"], "agent");
        let fetched: Value = serde_json::from_str(
            &execute(&store, &workspace.id, GET, &json!({"id":"S1"})).unwrap(),
        )
        .unwrap();
        assert_eq!(
            fetched["definition"]["summary"],
            "Example parser repository."
        );
        let queried: Value = serde_json::from_str(
            &execute(
                &store,
                &workspace.id,
                QUERY,
                &json!({"type_id":"repository","text":"parser"}),
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
                &json!({"id":"S1","definition":{"parent_id":"S1"}})
            )
            .is_err()
        );
        drop(store);
        std::fs::remove_dir_all(base).unwrap();
    }
}
