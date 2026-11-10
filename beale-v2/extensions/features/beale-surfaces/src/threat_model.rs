use super::{THREAT_MODEL_EDIT, THREAT_MODEL_GET, evidence_json, evidence_refs, required};
use anyhow::{Context, Result, bail, ensure};
use beale_contracts::v1::{
    SurfaceThreatDisposition, SurfaceThreatMapRef, SurfaceThreatMapRefKind, SurfaceThreatScenario,
    UpdateWorkspaceSurfaceThreatModelRequest, WorkspaceSurfaceMapping, WorkspaceSurfaceThreatModel,
};
use beale_workspaces::WorkspaceStore;
use serde_json::{Map, Value, json};

fn fields<'a>(value: &'a Value, allowed: &[&str]) -> Result<&'a Map<String, Value>> {
    let object = value.as_object().context("expected an object")?;
    ensure!(
        object.keys().all(|key| allowed.contains(&key.as_str())),
        "unknown threat model field"
    );
    Ok(object)
}

fn map_ref(value: &Value) -> Result<SurfaceThreatMapRef> {
    fields(value, &["kind", "id"])?;
    let kind = match required(value, "kind")? {
        "element" => SurfaceThreatMapRefKind::Element,
        "flow" => SurfaceThreatMapRefKind::Flow,
        _ => bail!("invalid map reference kind"),
    };
    Ok(SurfaceThreatMapRef {
        kind: kind as i32,
        id: required(value, "id")?.to_owned(),
    })
}

fn string_array(value: &Value, key: &str) -> Result<Vec<String>> {
    value
        .get(key)
        .and_then(Value::as_array)
        .with_context(|| format!("{key} must be an array"))?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .with_context(|| format!("invalid {key} item"))
        })
        .collect()
}

fn scenario(value: &Value, id: String) -> Result<SurfaceThreatScenario> {
    let object = fields(
        value,
        &[
            "id",
            "title",
            "map_refs",
            "attacker_position",
            "preconditions",
            "suspected_failure",
            "invariant_at_risk",
            "potential_impact",
            "validation_question",
            "evidence_refs",
            "disposition",
            "disposition_reason",
            "related_claim_ids",
        ],
    )?;
    let disposition = match object
        .get("disposition")
        .map(|value| value.as_str().context("invalid disposition"))
        .transpose()?
        .unwrap_or("open")
    {
        "open" => SurfaceThreatDisposition::Open,
        "dismissed" => SurfaceThreatDisposition::Dismissed,
        _ => bail!("invalid disposition"),
    };
    let map_refs = object
        .get("map_refs")
        .and_then(Value::as_array)
        .context("map_refs must be an array")?
        .iter()
        .map(map_ref)
        .collect::<Result<Vec<_>>>()?;
    Ok(SurfaceThreatScenario {
        id,
        title: required(value, "title")?.to_owned(),
        map_refs,
        attacker_position: required(value, "attacker_position")?.to_owned(),
        preconditions: string_array(value, "preconditions")?,
        suspected_failure: required(value, "suspected_failure")?.to_owned(),
        invariant_at_risk: required(value, "invariant_at_risk")?.to_owned(),
        potential_impact: required(value, "potential_impact")?.to_owned(),
        validation_question: required(value, "validation_question")?.to_owned(),
        evidence_refs: evidence_refs(object.get("evidence_refs"), Vec::new())?,
        disposition: disposition as i32,
        disposition_reason: object
            .get("disposition_reason")
            .map(|value| value.as_str().context("invalid disposition_reason"))
            .transpose()?
            .unwrap_or("")
            .to_owned(),
        related_claim_ids: object
            .get("related_claim_ids")
            .map(|_| string_array(value, "related_claim_ids"))
            .transpose()?
            .unwrap_or_default(),
    })
}

fn ref_json(reference: &SurfaceThreatMapRef) -> Value {
    let kind = if reference.kind == SurfaceThreatMapRefKind::Element as i32 {
        "element"
    } else {
        "flow"
    };
    json!({"kind":kind,"id":reference.id})
}

fn ref_exists(reference: &SurfaceThreatMapRef, mapping: &WorkspaceSurfaceMapping) -> bool {
    match SurfaceThreatMapRefKind::try_from(reference.kind) {
        Ok(SurfaceThreatMapRefKind::Element) => {
            mapping.elements.iter().any(|item| item.id == reference.id)
        }
        Ok(SurfaceThreatMapRefKind::Flow) => {
            mapping.flows.iter().any(|item| item.id == reference.id)
        }
        _ => false,
    }
}

fn page(
    model: &WorkspaceSurfaceThreatModel,
    mapping: &WorkspaceSurfaceMapping,
    args: &Value,
) -> Result<Value> {
    fields(args, &["surface_id", "offset", "limit"])?;
    let offset = args
        .get("offset")
        .map(|value| value.as_u64().context("invalid offset"))
        .transpose()?
        .unwrap_or(0);
    let offset = usize::try_from(offset).unwrap_or(usize::MAX);
    let limit = args
        .get("limit")
        .map(|value| value.as_u64().context("invalid limit"))
        .transpose()?
        .unwrap_or(20);
    ensure!((1..=50).contains(&limit), "invalid limit");
    let scenarios = model.scenarios.iter().skip(offset).take(limit as usize).map(|item| {
        let disposition = if item.disposition == SurfaceThreatDisposition::Dismissed as i32 { "dismissed" } else { "open" };
        json!({
            "id":item.id,"title":item.title,"map_refs":item.map_refs.iter().map(ref_json).collect::<Vec<_>>(),
            "stale_map_refs":item.map_refs.iter().filter(|reference| !ref_exists(reference, mapping)).map(ref_json).collect::<Vec<_>>(),
            "attacker_position":item.attacker_position,"preconditions":item.preconditions,
            "suspected_failure":item.suspected_failure,"invariant_at_risk":item.invariant_at_risk,
            "potential_impact":item.potential_impact,"validation_question":item.validation_question,
            "evidence_refs":item.evidence_refs.iter().map(evidence_json).collect::<Vec<_>>(),
            "disposition":disposition,"disposition_reason":item.disposition_reason,
            "related_claim_ids":item.related_claim_ids,
        })
    }).collect::<Vec<_>>();
    Ok(json!({
        "surface_id":model.surface_id,"revision":model.revision,
        "reviewed_mapping_revision":model.reviewed_mapping_revision,
        "current_mapping_revision":mapping.revision,
        "mapping_review_needed":model.reviewed_mapping_revision < mapping.revision,
        "total_scenarios":model.scenarios.len(),"scenarios":scenarios,
    }))
}

pub(super) fn execute(
    store: &WorkspaceStore,
    workspace_id: &str,
    name: &str,
    args: &Value,
) -> Result<String> {
    let surface_id = required(args, "surface_id")?;
    let mut model = store
        .threat_model_for_host(workspace_id, surface_id)
        .map_err(|status| anyhow::anyhow!("threat model read failed: {status}"))?;
    let mapping = store
        .mapping_for_host(workspace_id, surface_id)
        .map_err(|status| anyhow::anyhow!("mapping read failed: {status}"))?;
    match name {
        THREAT_MODEL_GET => Ok(page(&model, &mapping, args)?.to_string()),
        THREAT_MODEL_EDIT => {
            fields(
                args,
                &["surface_id", "scenario", "remove_id", "review_mapping"],
            )?;
            let review = args
                .get("review_mapping")
                .map(|value| value.as_bool().context("invalid review_mapping"))
                .transpose()?
                .unwrap_or(false);
            let count = usize::from(args.get("scenario").is_some())
                + usize::from(args.get("remove_id").is_some())
                + usize::from(review);
            ensure!(count == 1, "supply exactly one threat model operation");
            let changed_id = if let Some(value) = args.get("scenario") {
                let id = value
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("T{}", model.next_scenario_number));
                let item = scenario(value, id.clone())?;
                if let Some(previous) = model.scenarios.iter_mut().find(|item| item.id == id) {
                    *previous = item;
                } else {
                    ensure!(
                        value.get("id").is_none(),
                        "new scenario IDs are assigned by the host"
                    );
                    model.scenarios.push(item);
                }
                id
            } else if let Some(value) = args.get("remove_id") {
                let id = value.as_str().context("invalid remove_id")?;
                let before = model.scenarios.len();
                model.scenarios.retain(|item| item.id != id);
                ensure!(before != model.scenarios.len(), "scenario not found");
                id.to_owned()
            } else {
                model.reviewed_mapping_revision = mapping.revision;
                String::new()
            };
            let saved = store
                .update_threat_model_for_agent(
                    workspace_id,
                    surface_id,
                    UpdateWorkspaceSurfaceThreatModelRequest {
                        expected_revision: model.revision,
                        reviewed_mapping_revision: model.reviewed_mapping_revision,
                        scenarios: model.scenarios,
                    },
                )
                .map_err(|status| anyhow::anyhow!("threat model update failed: {status}"))?;
            Ok(json!({"surface_id":saved.surface_id,"revision":saved.revision,"changed_id":changed_id,
                "reviewed_mapping_revision":saved.reviewed_mapping_revision}).to_string())
        }
        _ => bail!("unknown threat model tool"),
    }
}
