use super::{
    MAPPING_EDIT, MAPPING_GET, PROMOTE, definition, evidence_json, evidence_refs, required,
    surface_json,
};
use anyhow::{Context, Result, bail, ensure};
use beale_contracts::v1::{
    SurfaceDefinition, SurfaceLocator, SurfaceMapBasis, SurfaceMapCategory, SurfaceMapElement,
    SurfaceMapFlow, SurfaceMapFlowKind, SurfaceMapGap, SurfaceMapGapReason, SurfaceScopeState,
    UpdateWorkspaceSurfaceMappingRequest, WorkspaceSurfaceMapping,
};
use beale_workspaces::WorkspaceStore;
use serde_json::{Map, Value, json};
use std::time::{SystemTime, UNIX_EPOCH};

fn fields<'a>(value: &'a Value, allowed: &[&str]) -> Result<&'a Map<String, Value>> {
    let object = value.as_object().context("expected an object")?;
    ensure!(
        object.keys().all(|key| allowed.contains(&key.as_str())),
        "unknown mapping field"
    );
    Ok(object)
}

fn category(value: &str) -> Result<i32> {
    Ok(match value {
        "component" => SurfaceMapCategory::Component,
        "interface" => SurfaceMapCategory::Interface,
        "behavior" => SurfaceMapCategory::Behavior,
        "state" => SurfaceMapCategory::State,
        "control" => SurfaceMapCategory::Control,
        _ => bail!("invalid element category"),
    } as i32)
}

fn basis(value: &str) -> Result<i32> {
    Ok(match value {
        "observed" => SurfaceMapBasis::Observed,
        "reported" => SurfaceMapBasis::Reported,
        "inferred" => SurfaceMapBasis::Inferred,
        _ => bail!("invalid mapping basis"),
    } as i32)
}

fn flow_kind(value: &str) -> Result<i32> {
    Ok(match value {
        "call" => SurfaceMapFlowKind::Call,
        "data" => SurfaceMapFlowKind::Data,
        "control" => SurfaceMapFlowKind::Control,
        "event" => SurfaceMapFlowKind::Event,
        _ => bail!("invalid flow kind"),
    } as i32)
}

fn gap_reason(value: &str) -> Result<i32> {
    Ok(match value {
        "not_examined" => SurfaceMapGapReason::NotExamined,
        "unavailable" => SurfaceMapGapReason::Unavailable,
        "ambiguous" => SurfaceMapGapReason::Ambiguous,
        _ => bail!("invalid gap reason"),
    } as i32)
}

fn now_ms() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("clock unavailable")?
        .as_millis()
        .min(u64::MAX as u128) as u64)
}

fn next_id(number: u64, prefix: char) -> Result<String> {
    ensure!(number > 0, "invalid mapping ID counter");
    Ok(format!("{prefix}{number}"))
}

fn locator(value: Option<&Value>) -> Result<Option<SurfaceLocator>> {
    value
        .map(|value| {
            let object = fields(value, &["kind", "value"])?;
            Ok(SurfaceLocator {
                kind: object
                    .get("kind")
                    .and_then(Value::as_str)
                    .context("locator kind is required")?
                    .to_owned(),
                value: object
                    .get("value")
                    .and_then(Value::as_str)
                    .context("locator value is required")?
                    .to_owned(),
            })
        })
        .transpose()
}

fn element(value: &Value, id: String) -> Result<SurfaceMapElement> {
    let object = fields(
        value,
        &[
            "id",
            "category",
            "type_id",
            "name",
            "locator",
            "observation",
            "basis",
            "observed_at_ms",
            "evidence_refs",
        ],
    )?;
    let basis = basis(required(value, "basis")?)?;
    Ok(SurfaceMapElement {
        id,
        category: category(required(value, "category")?)?,
        type_id: required(value, "type_id")?.to_owned(),
        name: required(value, "name")?.to_owned(),
        locator: locator(object.get("locator"))?,
        observation: required(value, "observation")?.to_owned(),
        basis,
        observed_at_ms: object
            .get("observed_at_ms")
            .map(|item| item.as_u64().context("invalid observed_at_ms"))
            .transpose()?
            .unwrap_or(if basis == SurfaceMapBasis::Observed as i32 {
                now_ms()?
            } else {
                0
            }),
        evidence_refs: evidence_refs(object.get("evidence_refs"), Vec::new())?,
    })
}

fn flow(value: &Value, id: String) -> Result<SurfaceMapFlow> {
    let object = fields(
        value,
        &[
            "id",
            "from_element_id",
            "to_element_id",
            "kind",
            "observation",
            "basis",
            "evidence_refs",
        ],
    )?;
    Ok(SurfaceMapFlow {
        id,
        from_element_id: required(value, "from_element_id")?.to_owned(),
        to_element_id: required(value, "to_element_id")?.to_owned(),
        kind: flow_kind(required(value, "kind")?)?,
        observation: required(value, "observation")?.to_owned(),
        basis: basis(required(value, "basis")?)?,
        evidence_refs: evidence_refs(object.get("evidence_refs"), Vec::new())?,
    })
}

fn gap(value: &Value, id: String) -> Result<SurfaceMapGap> {
    let object = fields(
        value,
        &["id", "element_id", "question", "reason", "evidence_refs"],
    )?;
    Ok(SurfaceMapGap {
        id,
        element_id: object
            .get("element_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        question: required(value, "question")?.to_owned(),
        reason: gap_reason(required(value, "reason")?)?,
        evidence_refs: evidence_refs(object.get("evidence_refs"), Vec::new())?,
    })
}

fn category_name(value: i32) -> &'static str {
    match SurfaceMapCategory::try_from(value) {
        Ok(SurfaceMapCategory::Component) => "component",
        Ok(SurfaceMapCategory::Interface) => "interface",
        Ok(SurfaceMapCategory::Behavior) => "behavior",
        Ok(SurfaceMapCategory::State) => "state",
        _ => "control",
    }
}

fn basis_name(value: i32) -> &'static str {
    match SurfaceMapBasis::try_from(value) {
        Ok(SurfaceMapBasis::Observed) => "observed",
        Ok(SurfaceMapBasis::Reported) => "reported",
        _ => "inferred",
    }
}

fn flow_kind_name(value: i32) -> &'static str {
    match SurfaceMapFlowKind::try_from(value) {
        Ok(SurfaceMapFlowKind::Call) => "call",
        Ok(SurfaceMapFlowKind::Data) => "data",
        Ok(SurfaceMapFlowKind::Control) => "control",
        _ => "event",
    }
}

fn gap_reason_name(value: i32) -> &'static str {
    match SurfaceMapGapReason::try_from(value) {
        Ok(SurfaceMapGapReason::NotExamined) => "not_examined",
        Ok(SurfaceMapGapReason::Unavailable) => "unavailable",
        _ => "ambiguous",
    }
}

fn mapping_json(mapping: &WorkspaceSurfaceMapping) -> Value {
    json!({
        "surface_id":mapping.surface_id,
        "revision":mapping.revision,
        "elements":mapping.elements.iter().map(|item| {
            let mut output = json!({
                "id":item.id,"category":category_name(item.category),"type_id":item.type_id,
                "name":item.name,"observation":item.observation,"basis":basis_name(item.basis),
                "observed_at_ms":item.observed_at_ms,
                "evidence_refs":item.evidence_refs.iter().map(evidence_json).collect::<Vec<_>>()
            });
            if let Some(locator) = &item.locator {
                output["locator"] = json!({"kind":locator.kind,"value":locator.value});
            }
            output
        }).collect::<Vec<_>>(),
        "flows":mapping.flows.iter().map(|item| json!({
            "id":item.id,"from_element_id":item.from_element_id,"to_element_id":item.to_element_id,
            "kind":flow_kind_name(item.kind),"observation":item.observation,"basis":basis_name(item.basis),
            "evidence_refs":item.evidence_refs.iter().map(evidence_json).collect::<Vec<_>>()
        })).collect::<Vec<_>>(),
        "gaps":mapping.gaps.iter().map(|item| json!({
            "id":item.id,"element_id":item.element_id,"question":item.question,"reason":gap_reason_name(item.reason),
            "evidence_refs":item.evidence_refs.iter().map(evidence_json).collect::<Vec<_>>()
        })).collect::<Vec<_>>()
    })
}

fn mapping_page(mapping: &WorkspaceSurfaceMapping, args: &Value) -> Result<Value> {
    fields(args, &["surface_id", "section", "offset", "limit"])?;
    let section = args
        .get("section")
        .map(|item| item.as_str().context("invalid section"))
        .transpose()?;
    ensure!(
        section.is_none_or(|item| matches!(item, "elements" | "flows" | "gaps")),
        "invalid section"
    );
    let offset = args
        .get("offset")
        .map(|item| item.as_u64().context("invalid offset"))
        .transpose()?
        .unwrap_or(0);
    let offset = usize::try_from(offset).unwrap_or(usize::MAX);
    let limit = args
        .get("limit")
        .map(|item| item.as_u64().context("invalid limit"))
        .transpose()?
        .unwrap_or(20);
    ensure!((1..=50).contains(&limit), "invalid limit");
    let mut result = mapping_json(mapping);
    result["total_elements"] = json!(mapping.elements.len());
    result["total_flows"] = json!(mapping.flows.len());
    result["total_gaps"] = json!(mapping.gaps.len());
    for key in ["elements", "flows", "gaps"] {
        let values = result[key].as_array().context("invalid mapping result")?;
        let selected = if section.is_none_or(|selected| selected == key) {
            values
                .iter()
                .skip(offset)
                .take(limit as usize)
                .cloned()
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        result[key] = json!(selected);
    }
    Ok(result)
}

fn edit(mapping: &mut WorkspaceSurfaceMapping, args: &Value) -> Result<(&'static str, String)> {
    fields(args, &["surface_id", "element", "flow", "gap", "remove"])?;
    let count = ["element", "flow", "gap", "remove"]
        .iter()
        .filter(|key| args.get(**key).is_some())
        .count();
    ensure!(count == 1, "supply exactly one mapping operation");
    let changed = if let Some(value) = args.get("element") {
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .map_or_else(|| next_id(mapping.next_element_number, 'E'), Ok)?;
        let item = element(value, id.clone())?;
        if let Some(previous) = mapping.elements.iter_mut().find(|item| item.id == id) {
            *previous = item;
        } else {
            ensure!(
                value.get("id").is_none(),
                "new element IDs are assigned by the host"
            );
            mapping.elements.push(item);
        }
        ("element", id)
    } else if let Some(value) = args.get("flow") {
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .map_or_else(|| next_id(mapping.next_flow_number, 'F'), Ok)?;
        let item = flow(value, id.clone())?;
        if let Some(previous) = mapping.flows.iter_mut().find(|item| item.id == id) {
            *previous = item;
        } else {
            ensure!(
                value.get("id").is_none(),
                "new flow IDs are assigned by the host"
            );
            mapping.flows.push(item);
        }
        ("flow", id)
    } else if let Some(value) = args.get("gap") {
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .map_or_else(|| next_id(mapping.next_gap_number, 'G'), Ok)?;
        let item = gap(value, id.clone())?;
        if let Some(previous) = mapping.gaps.iter_mut().find(|item| item.id == id) {
            *previous = item;
        } else {
            ensure!(
                value.get("id").is_none(),
                "new gap IDs are assigned by the host"
            );
            mapping.gaps.push(item);
        }
        ("gap", id)
    } else if let Some(value) = args.get("remove") {
        fields(value, &["kind", "id"])?;
        let id = required(value, "id")?;
        let kind = required(value, "kind")?;
        let removed = match kind {
            "element" => {
                let before = mapping.elements.len();
                mapping.elements.retain(|item| item.id != id);
                before != mapping.elements.len()
            }
            "flow" => {
                let before = mapping.flows.len();
                mapping.flows.retain(|item| item.id != id);
                before != mapping.flows.len()
            }
            "gap" => {
                let before = mapping.gaps.len();
                mapping.gaps.retain(|item| item.id != id);
                before != mapping.gaps.len()
            }
            _ => bail!("invalid mapping item kind"),
        };
        ensure!(removed, "mapping item not found");
        ("removed", id.to_owned())
    } else {
        bail!("missing mapping operation");
    };
    Ok(changed)
}

pub(super) fn execute(
    store: &WorkspaceStore,
    workspace_id: &str,
    name: &str,
    args: &Value,
    review_approved: bool,
) -> Result<String> {
    let surface_id = required(args, "surface_id")?;
    let mut mapping = store
        .mapping_for_host(workspace_id, surface_id)
        .map_err(|status| anyhow::anyhow!("mapping read failed: {status}"))?;
    match name {
        MAPPING_GET => Ok(mapping_page(&mapping, args)?.to_string()),
        MAPPING_EDIT => {
            let (kind, id) = edit(&mut mapping, args)?;
            let saved = store
                .update_mapping_for_agent(
                    workspace_id,
                    surface_id,
                    UpdateWorkspaceSurfaceMappingRequest {
                        expected_revision: mapping.revision,
                        elements: mapping.elements,
                        flows: mapping.flows,
                        gaps: mapping.gaps,
                    },
                )
                .map_err(|status| anyhow::anyhow!("mapping update failed: {status}"))?;
            Ok(json!({"surface_id":saved.surface_id,"revision":saved.revision,"changed_kind":kind,"changed_id":id}).to_string())
        }
        PROMOTE => {
            fields(args, &["surface_id", "element_id", "definition"])?;
            let element_id = required(args, "element_id")?;
            let element = mapping
                .elements
                .iter()
                .find(|item| item.id == element_id)
                .context("mapping element not found")?;
            let base = SurfaceDefinition {
                name: element.name.clone(),
                locator: Some(SurfaceLocator {
                    kind: "freeform".to_owned(),
                    value: element
                        .locator
                        .as_ref()
                        .map_or_else(|| element.name.clone(), |locator| locator.value.clone()),
                }),
                description: element.observation.clone(),
                evidence_refs: element.evidence_refs.clone(),
                ..Default::default()
            };
            let supplied = args.get("definition").context("definition is required")?;
            let mut promoted = definition(Some(supplied), Some(base))?;
            ensure!(
                supplied.get("type_id").is_some() && supplied.get("scope").is_some(),
                "promotion requires type_id and scope"
            );
            promoted.scope_state = if review_approved {
                SurfaceScopeState::Approved
            } else {
                SurfaceScopeState::Candidate
            } as i32;
            let surface = store
                .promote_element_for_agent(workspace_id, surface_id, element_id, promoted)
                .map_err(|status| anyhow::anyhow!("surface promotion failed: {status}"))?;
            Ok(surface_json(&surface, true).to_string())
        }
        _ => bail!("unknown surface mapping tool"),
    }
}
