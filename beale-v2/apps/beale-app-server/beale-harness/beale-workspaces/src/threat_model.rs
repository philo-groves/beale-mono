use super::{
    StoredEvidence, WorkspaceError, evidence_dto, valid_text, validate_evidence_refs, write_new,
};
use beale_contracts::v1::{
    SurfaceThreatDisposition, SurfaceThreatMapRef, SurfaceThreatMapRefKind, SurfaceThreatScenario,
    UpdateWorkspaceSurfaceThreatModelRequest, WorkspaceSurfaceMapping, WorkspaceSurfaceThreatModel,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs, path::Path};
use uuid::Uuid;

const MAX_BYTES: usize = 4_000_000;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StoredThreatModel {
    schema_version: u32,
    pub surface_id: String,
    pub revision: u64,
    pub reviewed_mapping_revision: u64,
    scenarios: Vec<StoredScenario>,
    pub next_scenario_number: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredScenario {
    id: String,
    title: String,
    map_refs: Vec<StoredMapRef>,
    attacker_position: String,
    preconditions: Vec<String>,
    suspected_failure: String,
    invariant_at_risk: String,
    potential_impact: String,
    validation_question: String,
    evidence_refs: Vec<StoredEvidence>,
    disposition: i32,
    disposition_reason: String,
    related_claim_ids: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredMapRef {
    kind: i32,
    id: String,
}

pub(super) fn dto(model: &StoredThreatModel) -> WorkspaceSurfaceThreatModel {
    WorkspaceSurfaceThreatModel {
        surface_id: model.surface_id.clone(),
        revision: model.revision,
        reviewed_mapping_revision: model.reviewed_mapping_revision,
        scenarios: model
            .scenarios
            .iter()
            .map(|item| SurfaceThreatScenario {
                id: item.id.clone(),
                title: item.title.clone(),
                map_refs: item
                    .map_refs
                    .iter()
                    .map(|reference| SurfaceThreatMapRef {
                        kind: reference.kind,
                        id: reference.id.clone(),
                    })
                    .collect(),
                attacker_position: item.attacker_position.clone(),
                preconditions: item.preconditions.clone(),
                suspected_failure: item.suspected_failure.clone(),
                invariant_at_risk: item.invariant_at_risk.clone(),
                potential_impact: item.potential_impact.clone(),
                validation_question: item.validation_question.clone(),
                evidence_refs: item
                    .evidence_refs
                    .iter()
                    .cloned()
                    .map(evidence_dto)
                    .collect(),
                disposition: item.disposition,
                disposition_reason: item.disposition_reason.clone(),
                related_claim_ids: item.related_claim_ids.clone(),
            })
            .collect(),
        next_scenario_number: model.next_scenario_number,
    }
}

fn valid_id(id: &str, prefix: char) -> bool {
    id.strip_prefix(prefix).is_some_and(|suffix| {
        !suffix.starts_with('0')
            && !suffix.is_empty()
            && suffix.bytes().all(|byte| byte.is_ascii_digit())
            && suffix.parse::<u64>().is_ok_and(|number| number > 0)
    })
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

pub(super) fn validate(
    surface_id: &str,
    revision: u64,
    request: UpdateWorkspaceSurfaceThreatModelRequest,
    previous: Option<&StoredThreatModel>,
    mapping: &WorkspaceSurfaceMapping,
) -> Result<StoredThreatModel, WorkspaceError> {
    if revision < 2
        || request.reviewed_mapping_revision > mapping.revision
        || request.scenarios.len() > 256
    {
        return Err(WorkspaceError::Invalid);
    }
    let mut ids = HashSet::new();
    let scenarios = request
        .scenarios
        .into_iter()
        .map(|item| {
            if !valid_id(&item.id, 'T')
                || !ids.insert(item.id.clone())
                || !valid_text(&item.title, 200, false)
                || item.map_refs.is_empty()
                || item.map_refs.len() > 32
                || !valid_text(&item.attacker_position, 1_000, true)
                || item.preconditions.len() > 16
                || item
                    .preconditions
                    .iter()
                    .any(|value| !valid_text(value, 1_000, true))
                || !valid_text(&item.suspected_failure, 2_000, true)
                || !valid_text(&item.invariant_at_risk, 2_000, true)
                || !valid_text(&item.potential_impact, 2_000, true)
                || !valid_text(&item.validation_question, 2_000, true)
                || item.related_claim_ids.len() > 32
                || item.related_claim_ids.iter().any(|id| !valid_id(id, 'C'))
                || item.related_claim_ids.iter().collect::<HashSet<_>>().len()
                    != item.related_claim_ids.len()
            {
                return Err(WorkspaceError::Invalid);
            }
            let dismissed = match SurfaceThreatDisposition::try_from(item.disposition) {
                Ok(SurfaceThreatDisposition::Open) => false,
                Ok(SurfaceThreatDisposition::Dismissed) => true,
                _ => return Err(WorkspaceError::Invalid),
            };
            if (dismissed && !valid_text(&item.disposition_reason, 2_000, true))
                || (!dismissed && !item.disposition_reason.is_empty())
            {
                return Err(WorkspaceError::Invalid);
            }
            let prior = previous.and_then(|model| {
                model
                    .scenarios
                    .iter()
                    .find(|scenario| scenario.id == item.id)
            });
            let mut refs = HashSet::new();
            for reference in &item.map_refs {
                let prefix = match SurfaceThreatMapRefKind::try_from(reference.kind) {
                    Ok(SurfaceThreatMapRefKind::Element) => 'E',
                    Ok(SurfaceThreatMapRefKind::Flow) => 'F',
                    _ => return Err(WorkspaceError::Invalid),
                };
                if !valid_id(&reference.id, prefix)
                    || !refs.insert((reference.kind, reference.id.as_str()))
                    || (!ref_exists(reference, mapping)
                        && !prior.is_some_and(|scenario| {
                            scenario
                                .map_refs
                                .iter()
                                .any(|old| old.kind == reference.kind && old.id == reference.id)
                        }))
                {
                    return Err(WorkspaceError::Invalid);
                }
            }
            Ok(StoredScenario {
                id: item.id,
                title: item.title.trim().to_owned(),
                map_refs: item
                    .map_refs
                    .into_iter()
                    .map(|reference| StoredMapRef {
                        kind: reference.kind,
                        id: reference.id,
                    })
                    .collect(),
                attacker_position: item.attacker_position.trim().to_owned(),
                preconditions: item
                    .preconditions
                    .into_iter()
                    .map(|value| value.trim().to_owned())
                    .collect(),
                suspected_failure: item.suspected_failure.trim().to_owned(),
                invariant_at_risk: item.invariant_at_risk.trim().to_owned(),
                potential_impact: item.potential_impact.trim().to_owned(),
                validation_question: item.validation_question.trim().to_owned(),
                evidence_refs: validate_evidence_refs(item.evidence_refs)?,
                disposition: item.disposition,
                disposition_reason: item.disposition_reason.trim().to_owned(),
                related_claim_ids: item.related_claim_ids,
            })
        })
        .collect::<Result<Vec<_>, WorkspaceError>>()?;
    let maximum = ids
        .iter()
        .filter_map(|id| id[1..].parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    let next_scenario_number = previous
        .map_or(1, |item| item.next_scenario_number)
        .max(maximum.checked_add(1).ok_or(WorkspaceError::Invalid)?);
    Ok(StoredThreatModel {
        schema_version: 1,
        surface_id: surface_id.to_owned(),
        revision,
        reviewed_mapping_revision: request.reviewed_mapping_revision,
        scenarios,
        next_scenario_number,
    })
}

fn model_path(directory: &Path, surface_id: &str) -> Result<std::path::PathBuf, WorkspaceError> {
    if super::surface_number(surface_id).is_none() {
        return Err(WorkspaceError::Invalid);
    }
    Ok(directory
        .join(".beale")
        .join("surface-threat-models")
        .join(format!("{surface_id}.json")))
}

pub(super) fn read(
    directory: &Path,
    surface_id: &str,
    mapping: &WorkspaceSurfaceMapping,
) -> Result<Option<StoredThreatModel>, WorkspaceError> {
    let path = model_path(directory, surface_id)?;
    let metadata = fs::symlink_metadata(directory.join(".beale"))
        .map_err(|_| WorkspaceError::Internal("inspect workspace metadata directory"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(WorkspaceError::Invalid);
    }
    let parent = path.parent().ok_or(WorkspaceError::Invalid)?;
    match fs::symlink_metadata(parent) {
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
            return Err(WorkspaceError::Invalid);
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(WorkspaceError::Internal("inspect threat model directory")),
    }
    match fs::symlink_metadata(&path) {
        Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
            return Err(WorkspaceError::Invalid);
        }
        Ok(metadata) if metadata.len() > MAX_BYTES as u64 => {
            return Err(WorkspaceError::Internal("threat model exceeds size limit"));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(WorkspaceError::Internal("inspect threat model file")),
    }
    let bytes = fs::read(path).map_err(|_| WorkspaceError::Internal("read threat model"))?;
    if bytes.len() > MAX_BYTES {
        return Err(WorkspaceError::Internal("threat model exceeds size limit"));
    }
    let model: StoredThreatModel = serde_json::from_slice(&bytes)
        .map_err(|_| WorkspaceError::Internal("decode threat model"))?;
    if model.schema_version != 1
        || model.surface_id != surface_id
        || model.revision < 2
        || model.next_scenario_number == 0
    {
        return Err(WorkspaceError::Internal("invalid threat model header"));
    }
    let loaded = dto(&model);
    let validated = validate(
        surface_id,
        model.revision,
        UpdateWorkspaceSurfaceThreatModelRequest {
            expected_revision: model.revision - 1,
            reviewed_mapping_revision: loaded.reviewed_mapping_revision,
            scenarios: loaded.scenarios,
        },
        Some(&model),
        mapping,
    )
    .map_err(|_| WorkspaceError::Internal("invalid threat model"))?;
    if model.next_scenario_number != validated.next_scenario_number {
        return Err(WorkspaceError::Internal("invalid threat model counter"));
    }
    Ok(Some(model))
}

pub(super) fn write(directory: &Path, model: &StoredThreatModel) -> Result<(), WorkspaceError> {
    let path = model_path(directory, &model.surface_id)?;
    let metadata_dir = directory.join(".beale");
    let metadata = fs::symlink_metadata(&metadata_dir)
        .map_err(|_| WorkspaceError::Internal("inspect workspace metadata directory"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(WorkspaceError::Invalid);
    }
    let parent = path.parent().ok_or(WorkspaceError::Invalid)?;
    match fs::symlink_metadata(parent) {
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
            return Err(WorkspaceError::Invalid);
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir(parent)
            .map_err(|_| WorkspaceError::Internal("create threat model directory"))?,
        Err(_) => return Err(WorkspaceError::Internal("inspect threat model directory")),
    }
    let mut bytes = serde_json::to_vec_pretty(model)
        .map_err(|_| WorkspaceError::Internal("encode threat model"))?;
    bytes.push(b'\n');
    if bytes.len() > MAX_BYTES {
        return Err(WorkspaceError::Invalid);
    }
    let temp = parent.join(format!("update-{}.tmp", Uuid::new_v4()));
    write_new(&temp, &bytes)?;
    if fs::rename(&temp, &path).is_err() {
        let _ = fs::remove_file(&temp);
        return Err(WorkspaceError::Internal("replace threat model"));
    }
    Ok(())
}
