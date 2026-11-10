use super::{
    StoredEvidence, StoredLocator, StoredSurface, WorkspaceError, evidence_dto, valid_text,
    validate_evidence_refs, write_new,
};
use beale_contracts::v1::{
    SurfaceLocator, SurfaceMapBasis, SurfaceMapCategory, SurfaceMapElement, SurfaceMapFlow,
    SurfaceMapFlowKind, SurfaceMapGap, SurfaceMapGapReason, UpdateWorkspaceSurfaceMappingRequest,
    WorkspaceSurfaceMapping,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs, path::Path};
use uuid::Uuid;

const MAX_MAPPING_BYTES: usize = 4_000_000;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StoredMapping {
    schema_version: u32,
    pub surface_id: String,
    pub revision: u64,
    pub elements: Vec<StoredElement>,
    pub flows: Vec<StoredFlow>,
    pub gaps: Vec<StoredGap>,
    pub next_element_number: u64,
    pub next_flow_number: u64,
    pub next_gap_number: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StoredElement {
    pub(super) id: String,
    category: i32,
    type_id: String,
    name: String,
    locator: Option<StoredLocator>,
    observation: String,
    basis: i32,
    observed_at_ms: u64,
    pub(super) evidence_refs: Vec<StoredEvidence>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StoredFlow {
    id: String,
    from_element_id: String,
    to_element_id: String,
    kind: i32,
    observation: String,
    basis: i32,
    evidence_refs: Vec<StoredEvidence>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StoredGap {
    id: String,
    element_id: String,
    question: String,
    reason: i32,
    evidence_refs: Vec<StoredEvidence>,
}

pub(super) fn mapping_dto(mapping: &StoredMapping) -> WorkspaceSurfaceMapping {
    WorkspaceSurfaceMapping {
        surface_id: mapping.surface_id.clone(),
        revision: mapping.revision,
        elements: mapping
            .elements
            .iter()
            .map(|item| SurfaceMapElement {
                id: item.id.clone(),
                category: item.category,
                type_id: item.type_id.clone(),
                name: item.name.clone(),
                locator: item.locator.as_ref().map(|locator| SurfaceLocator {
                    kind: locator.kind.clone(),
                    value: locator.value.clone(),
                }),
                observation: item.observation.clone(),
                basis: item.basis,
                observed_at_ms: item.observed_at_ms,
                evidence_refs: item
                    .evidence_refs
                    .iter()
                    .cloned()
                    .map(evidence_dto)
                    .collect(),
            })
            .collect(),
        flows: mapping
            .flows
            .iter()
            .map(|item| SurfaceMapFlow {
                id: item.id.clone(),
                from_element_id: item.from_element_id.clone(),
                to_element_id: item.to_element_id.clone(),
                kind: item.kind,
                observation: item.observation.clone(),
                basis: item.basis,
                evidence_refs: item
                    .evidence_refs
                    .iter()
                    .cloned()
                    .map(evidence_dto)
                    .collect(),
            })
            .collect(),
        gaps: mapping
            .gaps
            .iter()
            .map(|item| SurfaceMapGap {
                id: item.id.clone(),
                element_id: item.element_id.clone(),
                question: item.question.clone(),
                reason: item.reason,
                evidence_refs: item
                    .evidence_refs
                    .iter()
                    .cloned()
                    .map(evidence_dto)
                    .collect(),
            })
            .collect(),
        next_element_number: mapping.next_element_number,
        next_flow_number: mapping.next_flow_number,
        next_gap_number: mapping.next_gap_number,
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

fn valid_type_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn valid_basis(value: i32, evidence_count: usize, observed_at_ms: Option<u64>) -> bool {
    match SurfaceMapBasis::try_from(value) {
        Ok(SurfaceMapBasis::Observed) => {
            evidence_count > 0 && observed_at_ms.is_none_or(|timestamp| timestamp > 0)
        }
        Ok(SurfaceMapBasis::Reported | SurfaceMapBasis::Inferred) => evidence_count > 0,
        _ => false,
    }
}

pub(super) fn validate_mapping(
    surface_id: &str,
    revision: u64,
    request: UpdateWorkspaceSurfaceMappingRequest,
    previous: Option<&StoredMapping>,
) -> Result<StoredMapping, WorkspaceError> {
    if revision == 0
        || request.elements.len() > 512
        || request.flows.len() > 1024
        || request.gaps.len() > 256
    {
        return Err(WorkspaceError::Invalid);
    }
    let mut element_ids = HashSet::new();
    let elements = request
        .elements
        .into_iter()
        .map(|item| {
            if !valid_id(&item.id, 'E')
                || !element_ids.insert(item.id.clone())
                || !matches!(
                    SurfaceMapCategory::try_from(item.category),
                    Ok(SurfaceMapCategory::Component
                        | SurfaceMapCategory::Interface
                        | SurfaceMapCategory::Behavior
                        | SurfaceMapCategory::State
                        | SurfaceMapCategory::Control)
                )
                || !valid_type_id(&item.type_id)
                || !valid_text(&item.name, 200, false)
                || !valid_text(&item.observation, 4_000, true)
            {
                return Err(WorkspaceError::Invalid);
            }
            let locator = item
                .locator
                .map(|locator| {
                    if !valid_type_id(&locator.kind) || !valid_text(&locator.value, 2_048, false) {
                        return Err(WorkspaceError::Invalid);
                    }
                    Ok(StoredLocator {
                        kind: locator.kind,
                        value: locator.value.trim().to_owned(),
                    })
                })
                .transpose()?;
            let evidence_refs = validate_evidence_refs(item.evidence_refs)?;
            if !valid_basis(item.basis, evidence_refs.len(), Some(item.observed_at_ms)) {
                return Err(WorkspaceError::Invalid);
            }
            Ok(StoredElement {
                id: item.id,
                category: item.category,
                type_id: item.type_id,
                name: item.name.trim().to_owned(),
                locator,
                observation: item.observation.trim().to_owned(),
                basis: item.basis,
                observed_at_ms: item.observed_at_ms,
                evidence_refs,
            })
        })
        .collect::<Result<Vec<_>, WorkspaceError>>()?;
    let mut flow_ids = HashSet::new();
    let flows = request
        .flows
        .into_iter()
        .map(|item| {
            if !valid_id(&item.id, 'F')
                || !flow_ids.insert(item.id.clone())
                || item.from_element_id == item.to_element_id
                || !element_ids.contains(&item.from_element_id)
                || !element_ids.contains(&item.to_element_id)
                || !matches!(
                    SurfaceMapFlowKind::try_from(item.kind),
                    Ok(SurfaceMapFlowKind::Call
                        | SurfaceMapFlowKind::Data
                        | SurfaceMapFlowKind::Control
                        | SurfaceMapFlowKind::Event)
                )
                || !valid_text(&item.observation, 4_000, true)
            {
                return Err(WorkspaceError::Invalid);
            }
            let evidence_refs = validate_evidence_refs(item.evidence_refs)?;
            if !valid_basis(item.basis, evidence_refs.len(), None) {
                return Err(WorkspaceError::Invalid);
            }
            Ok(StoredFlow {
                id: item.id,
                from_element_id: item.from_element_id,
                to_element_id: item.to_element_id,
                kind: item.kind,
                observation: item.observation.trim().to_owned(),
                basis: item.basis,
                evidence_refs,
            })
        })
        .collect::<Result<Vec<_>, WorkspaceError>>()?;
    let mut gap_ids = HashSet::new();
    let gaps = request
        .gaps
        .into_iter()
        .map(|item| {
            if !valid_id(&item.id, 'G')
                || !gap_ids.insert(item.id.clone())
                || (!item.element_id.is_empty() && !element_ids.contains(&item.element_id))
                || !valid_text(&item.question, 2_000, true)
                || !matches!(
                    SurfaceMapGapReason::try_from(item.reason),
                    Ok(SurfaceMapGapReason::NotExamined
                        | SurfaceMapGapReason::Unavailable
                        | SurfaceMapGapReason::Ambiguous)
                )
            {
                return Err(WorkspaceError::Invalid);
            }
            Ok(StoredGap {
                id: item.id,
                element_id: item.element_id,
                question: item.question.trim().to_owned(),
                reason: item.reason,
                evidence_refs: validate_evidence_refs(item.evidence_refs)?,
            })
        })
        .collect::<Result<Vec<_>, WorkspaceError>>()?;
    let next_number =
        |prefix: char, ids: &HashSet<String>, prior: u64| -> Result<u64, WorkspaceError> {
            let maximum = ids
                .iter()
                .filter_map(|id| id.strip_prefix(prefix)?.parse::<u64>().ok())
                .max()
                .unwrap_or(0);
            Ok(prior
                .max(maximum.checked_add(1).ok_or(WorkspaceError::Invalid)?)
                .max(1))
        };
    let next_element_number = next_number(
        'E',
        &element_ids,
        previous.map_or(1, |item| item.next_element_number),
    )?;
    let next_flow_number = next_number(
        'F',
        &flow_ids,
        previous.map_or(1, |item| item.next_flow_number),
    )?;
    let next_gap_number = next_number(
        'G',
        &gap_ids,
        previous.map_or(1, |item| item.next_gap_number),
    )?;
    Ok(StoredMapping {
        schema_version: 1,
        surface_id: surface_id.to_owned(),
        revision,
        elements,
        flows,
        gaps,
        next_element_number,
        next_flow_number,
        next_gap_number,
    })
}

pub(super) fn validate_surface_anchors(
    surfaces: &[StoredSurface],
    directory: &Path,
) -> Result<(), WorkspaceError> {
    for surface in surfaces {
        for relationship in &surface.relationships {
            if !relationship.source_element_id.is_empty() {
                let mapping =
                    read_mapping(directory, &surface.id)?.ok_or(WorkspaceError::Invalid)?;
                if !mapping
                    .elements
                    .iter()
                    .any(|item| item.id == relationship.source_element_id)
                {
                    return Err(WorkspaceError::Invalid);
                }
            }
        }
    }
    Ok(())
}

pub(super) fn validate_relationship_anchors(
    surfaces: &[StoredSurface],
    mapping: &StoredMapping,
) -> Result<(), WorkspaceError> {
    if surfaces
        .iter()
        .filter(|surface| surface.id == mapping.surface_id)
        .flat_map(|surface| &surface.relationships)
        .any(|relationship| {
            !relationship.source_element_id.is_empty()
                && !mapping
                    .elements
                    .iter()
                    .any(|item| item.id == relationship.source_element_id)
        })
    {
        return Err(WorkspaceError::Invalid);
    }
    Ok(())
}

fn mapping_path(directory: &Path, surface_id: &str) -> Result<std::path::PathBuf, WorkspaceError> {
    if super::surface_number(surface_id).is_none() {
        return Err(WorkspaceError::Invalid);
    }
    Ok(directory
        .join(".beale")
        .join("surface-mappings")
        .join(format!("{surface_id}.json")))
}

pub(super) fn read_mapping(
    directory: &Path,
    surface_id: &str,
) -> Result<Option<StoredMapping>, WorkspaceError> {
    let path = mapping_path(directory, surface_id)?;
    let metadata_dir = directory.join(".beale");
    let metadata = fs::symlink_metadata(metadata_dir)
        .map_err(|_| WorkspaceError::Internal("inspect workspace metadata directory"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(WorkspaceError::Invalid);
    }
    let mapping_dir = path.parent().ok_or(WorkspaceError::Invalid)?;
    match fs::symlink_metadata(mapping_dir) {
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
            return Err(WorkspaceError::Invalid);
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(WorkspaceError::Internal(
                "inspect surface mapping directory",
            ));
        }
    }
    match fs::symlink_metadata(&path) {
        Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
            return Err(WorkspaceError::Invalid);
        }
        Ok(metadata) if metadata.len() > MAX_MAPPING_BYTES as u64 => {
            return Err(WorkspaceError::Internal(
                "surface mapping exceeds size limit",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(WorkspaceError::Internal("inspect surface mapping file")),
    }
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(WorkspaceError::Internal("read surface mapping")),
    };
    if bytes.len() > MAX_MAPPING_BYTES {
        return Err(WorkspaceError::Internal(
            "surface mapping exceeds size limit",
        ));
    }
    let mapping: StoredMapping = serde_json::from_slice(&bytes)
        .map_err(|_| WorkspaceError::Internal("decode surface mapping"))?;
    if mapping.schema_version != 1 || mapping.surface_id != surface_id || mapping.revision < 2 {
        return Err(WorkspaceError::Internal("invalid surface mapping header"));
    }
    let dto = mapping_dto(&mapping);
    let validated = validate_mapping(
        surface_id,
        mapping.revision,
        UpdateWorkspaceSurfaceMappingRequest {
            expected_revision: mapping.revision - 1,
            elements: dto.elements,
            flows: dto.flows,
            gaps: dto.gaps,
        },
        Some(&mapping),
    )
    .map_err(|_| WorkspaceError::Internal("invalid surface mapping"))?;
    if mapping.next_element_number == 0
        || mapping.next_flow_number == 0
        || mapping.next_gap_number == 0
        || mapping.next_element_number != validated.next_element_number
        || mapping.next_flow_number != validated.next_flow_number
        || mapping.next_gap_number != validated.next_gap_number
    {
        return Err(WorkspaceError::Internal("invalid surface mapping counters"));
    }
    Ok(Some(mapping))
}

pub(super) fn write_mapping(
    directory: &Path,
    mapping: &StoredMapping,
) -> Result<(), WorkspaceError> {
    let path = mapping_path(directory, &mapping.surface_id)?;
    let metadata_dir = directory.join(".beale");
    let metadata = fs::symlink_metadata(&metadata_dir)
        .map_err(|_| WorkspaceError::Internal("inspect workspace metadata directory"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(WorkspaceError::Invalid);
    }
    let mapping_dir = metadata_dir.join("surface-mappings");
    match fs::symlink_metadata(&mapping_dir) {
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
            return Err(WorkspaceError::Invalid);
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&mapping_dir)
                .map_err(|_| WorkspaceError::Internal("create surface mapping directory"))?;
        }
        Err(_) => {
            return Err(WorkspaceError::Internal(
                "inspect surface mapping directory",
            ));
        }
    }
    let mut bytes = serde_json::to_vec_pretty(mapping)
        .map_err(|_| WorkspaceError::Internal("encode surface mapping"))?;
    bytes.push(b'\n');
    if bytes.len() > MAX_MAPPING_BYTES {
        return Err(WorkspaceError::Invalid);
    }
    let temp = mapping_dir.join(format!("update-{}.tmp", Uuid::new_v4()));
    write_new(&temp, &bytes)?;
    if fs::rename(&temp, &path).is_err() {
        let _ = fs::remove_file(&temp);
        return Err(WorkspaceError::Internal("replace surface mapping"));
    }
    Ok(())
}
