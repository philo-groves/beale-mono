use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path as RoutePath, State},
    http::{HeaderMap, StatusCode},
    routing::get,
};
use beale_contracts::{
    WORKSPACE_CAMPAIGN_PATH, WORKSPACE_DETAILS_PATH, WORKSPACE_FOUNDATION_PATH,
    WORKSPACE_RULES_PATH, WORKSPACE_SURFACES_PATH, WORKSPACES_PATH,
    v1::{
        CreateWorkspaceRequest, ListWorkspacesResponse, SurfaceChange, SurfaceDefinition,
        SurfaceEvidence, SurfaceLocator, SurfaceRelationship, SurfaceRevision, SurfaceScope,
        SurfaceScopeState, UpdateWorkspaceFoundationRequest, UpdateWorkspaceRulesRequest,
        UpdateWorkspaceSurfacesRequest, WorkspaceCampaign, WorkspaceDetails, WorkspaceFoundation,
        WorkspaceRules, WorkspaceSummary, WorkspaceSurface, WorkspaceSurfaceInput,
        WorkspaceSurfaces,
    },
};
use beale_storage::{HostDatabase, WorkspaceRow};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;
use uuid::Uuid;

mod http;
mod import;
use http::{authorized, checked_body, response};

const DIRECTORIES: [&str; 10] = [
    "investigations",
    "runbooks",
    "reports",
    "evidence",
    "references",
    "memories",
    "claims",
    "traces",
    "scratch",
    "cache",
];
const DEFAULT_GUIDANCE: &str = "# Beale research workspace\n\nFollow the workspace scope and rules recorded in workspace.json.\n";
const MAX_MANIFEST_BYTES: usize = 8_000_000;

#[derive(Clone)]
pub struct WorkspaceStore {
    root: PathBuf,
    db: HostDatabase,
    write_lock: Arc<std::sync::Mutex<()>>,
}

struct Registry {
    workspaces: Vec<RegistryEntry>,
}

#[derive(Clone)]
struct RegistryEntry {
    id: String,
    name: String,
    directory: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceManifest {
    schema_version: u32,
    workspace_id: String,
    name: String,
    directories: &'static [&'static str],
    surfaces: Vec<StoredSurface>,
    retired_surfaces: Vec<StoredSurface>,
    next_surface_number: u64,
    rules: Vec<String>,
    foundation_revision: u64,
    surfaces_revision: u64,
    rules_revision: u64,
}

fn initial_revision() -> u64 {
    1
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredManifest {
    schema_version: u32,
    workspace_id: String,
    name: String,
    directories: Vec<String>,
    surfaces: Vec<StoredSurface>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    retired_surfaces: Vec<StoredSurface>,
    #[serde(default = "initial_revision")]
    next_surface_number: u64,
    rules: Vec<String>,
    #[serde(default = "initial_revision")]
    foundation_revision: u64,
    #[serde(default = "initial_revision")]
    surfaces_revision: u64,
    #[serde(default = "initial_revision")]
    rules_revision: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredSurface {
    #[serde(default)]
    id: String,
    #[serde(default, skip_serializing)]
    kind: String,
    #[serde(default, skip_serializing)]
    scope: String,
    #[serde(default = "unknown_scope_state", skip_serializing)]
    scope_state: String,
    #[serde(default, skip_serializing)]
    value: String,
    #[serde(default, skip_serializing)]
    created_by: String,
    #[serde(default, skip_serializing)]
    out_of_scope_reason: Option<String>,
    #[serde(default)]
    relationships: Vec<StoredRelationship>,
    #[serde(default)]
    definition: Option<StoredDefinition>,
    #[serde(default, skip_serializing)]
    legacy_ids: Vec<String>,
    #[serde(default, skip_serializing)]
    created_at_ms: u64,
    #[serde(default, skip_serializing)]
    updated_at_ms: u64,
    #[serde(default, skip_serializing)]
    revision: u64,
    #[serde(default)]
    lineage: Vec<StoredRevision>,
}

fn unknown_scope_state() -> String {
    "unknown".to_owned()
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredRevision {
    number: u64,
    at_ms: u64,
    actor: String,
    action: String,
    changes: Vec<StoredChange>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredChange {
    field: String,
    before_json: String,
    after_json: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredRelationship {
    target_id: String,
    label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    type_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    evidence_refs: Vec<StoredEvidence>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredEvidence {
    kind: String,
    reference: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    note: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredDefinition {
    type_id: String,
    name: String,
    locator: StoredLocator,
    #[serde(default)]
    description: String,
    #[serde(default)]
    scope: String,
    #[serde(default = "unknown_scope_state")]
    scope_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    out_of_scope_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    evidence_refs: Vec<StoredEvidence>,
    #[serde(default, skip_serializing)]
    parent_id: String,
    #[serde(default, skip_serializing)]
    summary: String,
    #[serde(default, skip_serializing)]
    context: String,
    #[serde(default, skip_serializing)]
    scope_basis: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredLocator {
    kind: String,
    value: String,
}

struct ValidatedWorkspace {
    name: String,
    directory: PathBuf,
    guidance: String,
    surfaces: Vec<StoredSurface>,
    next_surface_number: u64,
    rules: Vec<String>,
}

#[derive(Debug)]
enum WorkspaceError {
    Invalid,
    Conflict,
    NotFound,
    Internal(&'static str),
}

impl WorkspaceError {
    fn status(&self) -> StatusCode {
        match self {
            Self::Invalid => StatusCode::BAD_REQUEST,
            Self::Conflict => StatusCode::CONFLICT,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn report(&self) {
        if let Self::Internal(stage) = self {
            eprintln!("workspace operation failed: {stage}");
        }
    }
}

impl WorkspaceStore {
    pub fn new() -> anyhow::Result<Self> {
        let root = if let Some(path) = std::env::var_os("BEALE_V2_HOME") {
            PathBuf::from(path)
        } else {
            let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .ok_or_else(|| anyhow::anyhow!("home directory is unavailable"))?;
            PathBuf::from(home).join(".beale").join("v2")
        };
        let root = if root.is_absolute() {
            root
        } else {
            std::env::current_dir()?.join(root)
        };
        Self::at(root)
    }

    pub fn at(root: PathBuf) -> anyhow::Result<Self> {
        let db = HostDatabase::open(&root)?;
        Ok(Self {
            root,
            db,
            write_lock: Arc::new(std::sync::Mutex::new(())),
        })
    }

    fn list(&self) -> Result<Vec<WorkspaceSummary>, WorkspaceError> {
        Ok(self
            .read_registry()?
            .workspaces
            .into_iter()
            .map(|entry| WorkspaceSummary {
                id: entry.id,
                name: entry.name,
                directory: entry.directory,
            })
            .collect())
    }

    pub fn contains(&self, id: &str) -> Result<bool, StatusCode> {
        Ok(self
            .read_registry()
            .map_err(|error| error.status())?
            .workspaces
            .iter()
            .any(|workspace| workspace.id == id))
    }

    pub fn details_for_host(&self, id: &str) -> Result<WorkspaceDetails, StatusCode> {
        self.details(id).map_err(|error| {
            error.report();
            error.status()
        })
    }

    pub fn surfaces_for_host(&self, id: &str) -> Result<WorkspaceSurfaces, StatusCode> {
        self.surfaces(id).map_err(|error| error.status())
    }

    pub fn surface_for_host(
        &self,
        id: &str,
        surface_id: &str,
    ) -> Result<Option<(WorkspaceSurface, bool)>, StatusCode> {
        let entry = self.entry(id).map_err(|error| error.status())?;
        let manifest = Self::manifest(&entry).map_err(|error| error.status())?;
        manifest
            .surfaces
            .iter()
            .find(|item| item.id == surface_id)
            .map(|item| surface_dto(item.clone()).map(|surface| (surface, false)))
            .or_else(|| {
                manifest
                    .retired_surfaces
                    .iter()
                    .find(|item| item.id == surface_id)
                    .map(|item| surface_dto(item.clone()).map(|surface| (surface, true)))
            })
            .transpose()
            .map_err(|error| error.status())
    }

    pub fn update_surfaces_for_agent(
        &self,
        id: &str,
        expected_revision: u64,
        surfaces: Vec<WorkspaceSurfaceInput>,
    ) -> Result<WorkspaceSurfaces, StatusCode> {
        self.update_surfaces_with_actor(
            id,
            UpdateWorkspaceSurfacesRequest {
                expected_revision,
                surfaces,
            },
            "agent",
        )
        .map_err(|error| error.status())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn directory(&self, id: &str) -> Result<PathBuf, StatusCode> {
        let entry = self
            .read_registry()
            .map_err(|error| error.status())?
            .workspaces
            .into_iter()
            .find(|item| item.id == id)
            .ok_or(StatusCode::NOT_FOUND)?;
        Ok(PathBuf::from(entry.directory))
    }

    fn details(&self, id: &str) -> Result<WorkspaceDetails, WorkspaceError> {
        Uuid::parse_str(id).map_err(|_| WorkspaceError::Invalid)?;
        let entry = self
            .read_registry()?
            .workspaces
            .into_iter()
            .find(|entry| entry.id == id)
            .ok_or(WorkspaceError::NotFound)?;
        let directory = Path::new(&entry.directory);
        let manifest_bytes = fs::read(directory.join("workspace.json"))
            .map_err(|_| WorkspaceError::Internal("read workspace manifest"))?;
        if manifest_bytes.len() > MAX_MANIFEST_BYTES {
            return Err(WorkspaceError::Internal(
                "workspace manifest exceeds size limit",
            ));
        }
        let manifest: StoredManifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|_| WorkspaceError::Internal("decode workspace manifest"))?;
        let manifest = normalize_manifest(manifest)?;
        if manifest.schema_version != 6
            || manifest.workspace_id != entry.id
            || manifest.name != entry.name
            || manifest.surfaces.len() > 256
            || manifest.rules.len() > 128
        {
            return Err(WorkspaceError::Internal("invalid workspace manifest"));
        }
        let guidance = fs::read(directory.join("AGENTS.md"))
            .map_err(|_| WorkspaceError::Internal("read workspace guidance"))?;
        if guidance.len() > 20_001 {
            return Err(WorkspaceError::Internal(
                "workspace guidance exceeds size limit",
            ));
        }
        let guidance = String::from_utf8(guidance)
            .map_err(|_| WorkspaceError::Internal("decode workspace guidance"))?;
        let surfaces = manifest
            .surfaces
            .into_iter()
            .map(surface_dto)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(WorkspaceDetails {
            summary: Some(WorkspaceSummary {
                id: entry.id,
                name: entry.name,
                directory: entry.directory,
            }),
            guidance,
            surfaces,
            rules: manifest.rules,
        })
    }

    fn entry(&self, id: &str) -> Result<RegistryEntry, WorkspaceError> {
        Uuid::parse_str(id).map_err(|_| WorkspaceError::Invalid)?;
        self.read_registry()?
            .workspaces
            .into_iter()
            .find(|entry| entry.id == id)
            .ok_or(WorkspaceError::NotFound)
    }

    fn manifest(entry: &RegistryEntry) -> Result<StoredManifest, WorkspaceError> {
        let bytes = fs::read(Path::new(&entry.directory).join("workspace.json"))
            .map_err(|_| WorkspaceError::Internal("read workspace manifest"))?;
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(WorkspaceError::Internal(
                "workspace manifest exceeds size limit",
            ));
        }
        let manifest: StoredManifest = serde_json::from_slice(&bytes)
            .map_err(|_| WorkspaceError::Internal("decode workspace manifest"))?;
        let manifest = normalize_manifest(manifest)?;
        if manifest.schema_version != 6
            || manifest.workspace_id != entry.id
            || manifest.name != entry.name
            || manifest.surfaces.len() > 256
            || manifest.rules.len() > 128
            || manifest.foundation_revision == 0
            || manifest.surfaces_revision == 0
            || manifest.rules_revision == 0
            || manifest.next_surface_number == 0
        {
            return Err(WorkspaceError::Internal("invalid workspace manifest"));
        }
        for surface in manifest.surfaces.iter().chain(&manifest.retired_surfaces) {
            surface_dto(surface.clone())?;
        }
        Ok(manifest)
    }

    fn guidance(entry: &RegistryEntry) -> Result<String, WorkspaceError> {
        let bytes = fs::read(Path::new(&entry.directory).join("AGENTS.md"))
            .map_err(|_| WorkspaceError::Internal("read workspace guidance"))?;
        if bytes.len() > 20_001 {
            return Err(WorkspaceError::Internal(
                "workspace guidance exceeds size limit",
            ));
        }
        String::from_utf8(bytes).map_err(|_| WorkspaceError::Internal("decode workspace guidance"))
    }

    fn foundation(&self, id: &str) -> Result<WorkspaceFoundation, WorkspaceError> {
        let entry = self.entry(id)?;
        let manifest = Self::manifest(&entry)?;
        let guidance = Self::guidance(&entry)?;
        Ok(WorkspaceFoundation {
            summary: Some(summary(&entry)),
            guidance,
            revision: manifest.foundation_revision,
        })
    }

    fn surfaces(&self, id: &str) -> Result<WorkspaceSurfaces, WorkspaceError> {
        let entry = self.entry(id)?;
        let manifest = Self::manifest(&entry)?;
        Ok(WorkspaceSurfaces {
            surfaces: manifest
                .surfaces
                .into_iter()
                .map(surface_dto)
                .collect::<Result<Vec<_>, _>>()?,
            revision: manifest.surfaces_revision,
        })
    }

    fn rules(&self, id: &str) -> Result<WorkspaceRules, WorkspaceError> {
        let entry = self.entry(id)?;
        let manifest = Self::manifest(&entry)?;
        Ok(WorkspaceRules {
            rules: manifest.rules,
            revision: manifest.rules_revision,
        })
    }

    fn update_foundation(
        &self,
        id: &str,
        request: UpdateWorkspaceFoundationRequest,
    ) -> Result<WorkspaceFoundation, WorkspaceError> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| WorkspaceError::Internal("workspace write lock"))?;
        let name = request.name.trim();
        let guidance = request.guidance.replace("\r\n", "\n").replace('\r', "\n");
        if !valid_text(name, 200, false)
            || guidance.len() > 20_000
            || guidance
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
        {
            return Err(WorkspaceError::Invalid);
        }
        let mut registry = self.read_registry()?;
        Uuid::parse_str(id).map_err(|_| WorkspaceError::Invalid)?;
        let entry = registry
            .workspaces
            .iter_mut()
            .find(|entry| entry.id == id)
            .ok_or(WorkspaceError::NotFound)?;
        let mut manifest = Self::manifest(entry)?;
        if request.expected_revision != manifest.foundation_revision {
            return Err(WorkspaceError::Conflict);
        }
        let previous_manifest = manifest.clone();
        let previous_guidance = Self::guidance(entry)?;
        let guidance = if guidance.trim().is_empty() {
            DEFAULT_GUIDANCE.to_owned()
        } else {
            format!("{}\n", guidance.trim())
        };
        manifest.name = name.to_owned();
        manifest.foundation_revision = manifest
            .foundation_revision
            .checked_add(1)
            .ok_or(WorkspaceError::Invalid)?;
        let directory = PathBuf::from(&entry.directory);
        write_replace(&directory.join("AGENTS.md"), guidance.as_bytes())?;
        if let Err(error) = write_manifest(&directory, &manifest) {
            let _ = write_replace(&directory.join("AGENTS.md"), previous_guidance.as_bytes());
            return Err(error);
        }
        entry.name = name.to_owned();
        let saved_summary = summary(entry);
        if let Err(error) = self.write_registry(&registry) {
            let _ = write_manifest(&directory, &previous_manifest);
            let _ = write_replace(&directory.join("AGENTS.md"), previous_guidance.as_bytes());
            return Err(error);
        }
        Ok(WorkspaceFoundation {
            summary: Some(saved_summary),
            guidance,
            revision: manifest.foundation_revision,
        })
    }

    fn update_surfaces(
        &self,
        id: &str,
        request: UpdateWorkspaceSurfacesRequest,
    ) -> Result<WorkspaceSurfaces, WorkspaceError> {
        self.update_surfaces_with_actor(id, request, "operator")
    }

    fn update_surfaces_with_actor(
        &self,
        id: &str,
        request: UpdateWorkspaceSurfacesRequest,
        actor: &str,
    ) -> Result<WorkspaceSurfaces, WorkspaceError> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| WorkspaceError::Internal("workspace write lock"))?;
        let entry = self.entry(id)?;
        let mut manifest = Self::manifest(&entry)?;
        if request.expected_revision != manifest.surfaces_revision {
            return Err(WorkspaceError::Conflict);
        }
        let (mut surfaces, next_surface_number) = validate_surface_update_with_counter(
            request.surfaces,
            &manifest.surfaces,
            manifest.next_surface_number,
        )?;
        if manifest.rules.is_empty() && !surfaces.iter().any(|surface| surface.scope == "in_scope")
        {
            return Err(WorkspaceError::Invalid);
        }
        stamp_lineage(&mut surfaces, &manifest.surfaces, actor)?;
        for old in &manifest.surfaces {
            if !surfaces.iter().any(|item| item.id == old.id) {
                manifest.retired_surfaces.push(retire_surface(old, actor)?);
            }
        }
        manifest.surfaces = surfaces;
        manifest.next_surface_number = next_surface_number;
        manifest.surfaces_revision = manifest
            .surfaces_revision
            .checked_add(1)
            .ok_or(WorkspaceError::Invalid)?;
        write_manifest(Path::new(&entry.directory), &manifest)?;
        Ok(WorkspaceSurfaces {
            surfaces: manifest
                .surfaces
                .into_iter()
                .map(surface_dto)
                .collect::<Result<Vec<_>, _>>()?,
            revision: manifest.surfaces_revision,
        })
    }

    fn update_rules(
        &self,
        id: &str,
        request: UpdateWorkspaceRulesRequest,
    ) -> Result<WorkspaceRules, WorkspaceError> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| WorkspaceError::Internal("workspace write lock"))?;
        let entry = self.entry(id)?;
        let mut manifest = Self::manifest(&entry)?;
        if request.expected_revision != manifest.rules_revision {
            return Err(WorkspaceError::Conflict);
        }
        manifest.rules = validate_rules(request.rules)?;
        if manifest.rules.is_empty()
            && !manifest
                .surfaces
                .iter()
                .any(|surface| surface.scope == "in_scope")
        {
            return Err(WorkspaceError::Invalid);
        }
        manifest.rules_revision = manifest
            .rules_revision
            .checked_add(1)
            .ok_or(WorkspaceError::Invalid)?;
        write_manifest(Path::new(&entry.directory), &manifest)?;
        Ok(WorkspaceRules {
            rules: manifest.rules,
            revision: manifest.rules_revision,
        })
    }

    fn create(&self, request: CreateWorkspaceRequest) -> Result<WorkspaceSummary, WorkspaceError> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| WorkspaceError::Internal("workspace write lock"))?;
        let input = validate(request)?;
        let host_root = normalize_directory(&self.root.to_string_lossy())
            .map_err(|_| WorkspaceError::Internal("resolve host data directory"))?;
        if input.directory.starts_with(&host_root) || host_root.starts_with(&input.directory) {
            return Err(WorkspaceError::Invalid);
        }
        let mut registry = self.read_registry()?;
        let directory = input.directory.to_string_lossy().into_owned();
        if registry
            .workspaces
            .iter()
            .any(|entry| entry.directory == directory)
        {
            return Err(WorkspaceError::Conflict);
        }
        match fs::symlink_metadata(&input.directory) {
            Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
                return Err(WorkspaceError::Conflict);
            }
            Ok(_) => {
                if fs::read_dir(&input.directory)
                    .map_err(|_| WorkspaceError::Internal("inspect workspace directory"))?
                    .next()
                    .is_some()
                {
                    return Err(WorkspaceError::Conflict);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&input.directory)
                    .map_err(|_| WorkspaceError::Internal("create workspace directory"))?;
            }
            Err(_) => return Err(WorkspaceError::Internal("inspect workspace directory")),
        }
        let id = Uuid::new_v4().to_string();
        let manifest = WorkspaceManifest {
            schema_version: 6,
            workspace_id: id.clone(),
            name: input.name.clone(),
            directories: &DIRECTORIES,
            surfaces: input.surfaces,
            retired_surfaces: Vec::new(),
            next_surface_number: input.next_surface_number,
            rules: input.rules,
            foundation_revision: 1,
            surfaces_revision: 1,
            rules_revision: 1,
        };
        for directory_name in DIRECTORIES {
            fs::create_dir(input.directory.join(directory_name))
                .map_err(|_| WorkspaceError::Internal("create research directory"))?;
        }
        fs::create_dir(input.directory.join(".beale"))
            .map_err(|_| WorkspaceError::Internal("create workspace metadata directory"))?;
        let guidance = if input.guidance.is_empty() {
            DEFAULT_GUIDANCE.to_owned()
        } else {
            format!("{}\n", input.guidance)
        };
        write_new(&input.directory.join("AGENTS.md"), guidance.as_bytes())?;
        let mut manifest_bytes = serde_json::to_vec_pretty(&manifest)
            .map_err(|_| WorkspaceError::Internal("encode workspace manifest"))?;
        manifest_bytes.push(b'\n');
        write_new(&input.directory.join("workspace.json"), &manifest_bytes)?;
        let summary = WorkspaceSummary {
            id: id.clone(),
            name: input.name.clone(),
            directory: directory.clone(),
        };
        registry.workspaces.push(RegistryEntry {
            id,
            name: input.name,
            directory,
        });
        self.write_registry(&registry)?;
        Ok(summary)
    }

    fn read_registry(&self) -> Result<Registry, WorkspaceError> {
        let workspaces = self
            .db
            .workspaces()
            .map_err(|_| WorkspaceError::Internal("read workspace registry"))?
            .into_iter()
            .map(|row| RegistryEntry {
                id: row.id,
                name: row.name,
                directory: row.directory,
            })
            .collect();
        Ok(Registry { workspaces })
    }

    fn write_registry(&self, registry: &Registry) -> Result<(), WorkspaceError> {
        let rows: Vec<_> = registry
            .workspaces
            .iter()
            .map(|entry| WorkspaceRow {
                id: entry.id.clone(),
                name: entry.name.clone(),
                directory: entry.directory.clone(),
            })
            .collect();
        self.db
            .replace_workspaces(&rows)
            .map_err(|_| WorkspaceError::Internal("write workspace registry"))
    }
}

fn summary(entry: &RegistryEntry) -> WorkspaceSummary {
    WorkspaceSummary {
        id: entry.id.clone(),
        name: entry.name.clone(),
        directory: entry.directory.clone(),
    }
}

fn surface_dto(surface: StoredSurface) -> Result<WorkspaceSurface, WorkspaceError> {
    let definition = surface
        .definition
        .ok_or(WorkspaceError::Internal("missing surface definition"))?;
    Ok(WorkspaceSurface {
        id: surface.id,
        definition: Some(definition_dto(definition)?),
        relationships: surface
            .relationships
            .into_iter()
            .map(|relationship| SurfaceRelationship {
                target_id: relationship.target_id,
                label: relationship.label,
                type_id: relationship.type_id,
                evidence_refs: relationship
                    .evidence_refs
                    .into_iter()
                    .map(evidence_dto)
                    .collect(),
            })
            .collect(),
        lineage: surface
            .lineage
            .into_iter()
            .map(|event| SurfaceRevision {
                number: event.number,
                at_ms: event.at_ms,
                actor: event.actor,
                action: event.action,
                changes: event
                    .changes
                    .into_iter()
                    .map(|change| SurfaceChange {
                        field: change.field,
                        before_json: change.before_json,
                        after_json: change.after_json,
                    })
                    .collect(),
            })
            .collect(),
    })
}

fn surface_snapshot(
    surface: &StoredSurface,
) -> Result<serde_json::Map<String, serde_json::Value>, WorkspaceError> {
    let serde_json::Value::Object(mut fields) = serde_json::to_value(surface)
        .map_err(|_| WorkspaceError::Internal("encode surface lineage"))?
    else {
        return Err(WorkspaceError::Internal("invalid surface lineage"));
    };
    for key in [
        "createdAtMs",
        "updatedAtMs",
        "revision",
        "lineage",
        "legacyIds",
        "mappingProgress",
        "threatModelingProgress",
        "analysisProgress",
    ] {
        fields.remove(key);
    }
    Ok(fields)
}

fn stamp_lineage(
    surfaces: &mut [StoredSurface],
    previous: &[StoredSurface],
    actor: &str,
) -> Result<(), WorkspaceError> {
    let at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| WorkspaceError::Internal("clock unavailable"))?
        .as_millis()
        .min(u64::MAX as u128) as u64;
    for surface in surfaces {
        let prior = previous.iter().find(|item| item.id == surface.id);
        let current = surface_snapshot(surface)?;
        let changes = if let Some(prior) = prior {
            let before = surface_snapshot(prior)?;
            current
                .iter()
                .filter_map(|(field, after)| {
                    let old = before.get(field).unwrap_or(&serde_json::Value::Null);
                    (old != after).then(|| StoredChange {
                        field: field.clone(),
                        before_json: old.to_string(),
                        after_json: after.to_string(),
                    })
                })
                .collect::<Vec<_>>()
        } else {
            vec![StoredChange {
                field: "surface".to_owned(),
                before_json: "null".to_owned(),
                after_json: serde_json::Value::Object(current).to_string(),
            }]
        };
        if let Some(prior) = prior {
            surface.created_at_ms = prior.created_at_ms;
            surface.updated_at_ms = prior.updated_at_ms;
            surface.revision = prior.revision;
            surface.lineage = prior.lineage.clone();
        }
        if !changes.is_empty() {
            surface.revision = surface
                .revision
                .checked_add(1)
                .ok_or(WorkspaceError::Invalid)?;
            surface.updated_at_ms = at_ms.max(surface.updated_at_ms.saturating_add(1));
            if prior.is_none() {
                surface.created_at_ms = surface.updated_at_ms;
            }
            surface.lineage.push(StoredRevision {
                number: surface.revision,
                at_ms: surface.updated_at_ms,
                actor: if prior.is_none()
                    && surface.scope_state == "candidate"
                    && actor == "operator"
                {
                    "ai_import"
                } else {
                    actor
                }
                .to_owned(),
                action: if prior.is_none() {
                    "created"
                } else {
                    "updated"
                }
                .to_owned(),
                changes,
            });
        }
    }
    Ok(())
}

fn retire_surface(surface: &StoredSurface, actor: &str) -> Result<StoredSurface, WorkspaceError> {
    let mut retired = surface.clone();
    let before = serde_json::Value::Object(surface_snapshot(surface)?).to_string();
    retired.revision = retired
        .revision
        .checked_add(1)
        .ok_or(WorkspaceError::Invalid)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| WorkspaceError::Internal("clock unavailable"))?
        .as_millis()
        .min(u64::MAX as u128) as u64;
    retired.updated_at_ms = now.max(retired.updated_at_ms.saturating_add(1));
    if retired.created_at_ms == 0 {
        retired.created_at_ms = retired.updated_at_ms;
    }
    retired.lineage.push(StoredRevision {
        number: retired.revision,
        at_ms: retired.updated_at_ms,
        actor: actor.to_owned(),
        action: "deleted".to_owned(),
        changes: vec![StoredChange {
            field: "surface".to_owned(),
            before_json: before,
            after_json: "null".to_owned(),
        }],
    });
    Ok(retired)
}

fn legacy_definition(kind: &str, value: &str) -> StoredDefinition {
    StoredDefinition {
        type_id: kind.to_owned(),
        name: default_surface_name(value),
        locator: StoredLocator {
            kind: "freeform".to_owned(),
            value: value.to_owned(),
        },
        description: String::new(),
        scope: String::new(),
        scope_state: unknown_scope_state(),
        out_of_scope_reason: None,
        evidence_refs: Vec::new(),
        parent_id: String::new(),
        summary: String::new(),
        context: String::new(),
        scope_basis: String::new(),
    }
}

fn default_surface_name(value: &str) -> String {
    let mut name = String::new();
    for character in value.chars() {
        if name.len() + character.len_utf8() > 200 {
            break;
        }
        name.push(character);
    }
    name
}

fn definition_dto(definition: StoredDefinition) -> Result<SurfaceDefinition, WorkspaceError> {
    let scope = match definition.scope.as_str() {
        "in_scope" => SurfaceScope::InScope,
        "out_of_scope" => SurfaceScope::OutOfScope,
        _ => return Err(WorkspaceError::Internal("invalid workspace surface scope")),
    };
    let scope_state = match definition.scope_state.as_str() {
        "unknown" => SurfaceScopeState::Unknown,
        "candidate" => SurfaceScopeState::Candidate,
        "approved" => SurfaceScopeState::Approved,
        _ => {
            return Err(WorkspaceError::Internal(
                "invalid workspace surface scope state",
            ));
        }
    };
    Ok(SurfaceDefinition {
        type_id: definition.type_id,
        name: definition.name,
        locator: Some(SurfaceLocator {
            kind: definition.locator.kind,
            value: definition.locator.value,
        }),
        scope: scope as i32,
        scope_state: scope_state as i32,
        description: definition.description,
        out_of_scope_reason: definition.out_of_scope_reason.unwrap_or_default(),
        evidence_refs: definition
            .evidence_refs
            .into_iter()
            .map(evidence_dto)
            .collect(),
    })
}

fn evidence_dto(evidence: StoredEvidence) -> SurfaceEvidence {
    SurfaceEvidence {
        kind: evidence.kind,
        reference: evidence.reference,
        note: evidence.note,
    }
}

fn valid_optional_text(value: &str, max: usize, multiline: bool) -> bool {
    value.is_empty() || valid_text(value, max, multiline)
}

fn valid_repository_path(value: &str) -> bool {
    let normalized = value.replace('\\', "/");
    !normalized.starts_with('/')
        && !normalized.contains(':')
        && normalized
            .split('/')
            .all(|part| !matches!(part, "" | "." | ".."))
}

fn validate_evidence_refs(
    input: Vec<SurfaceEvidence>,
) -> Result<Vec<StoredEvidence>, WorkspaceError> {
    if input.len() > 16 {
        return Err(WorkspaceError::Invalid);
    }
    input
        .into_iter()
        .map(|item| {
            if item.kind.is_empty()
                || item.kind.len() > 64
                || !item.kind.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'.' | b'_' | b'-')
                })
                || !valid_text(&item.reference, 2_048, false)
                || !valid_optional_text(&item.note, 500, true)
            {
                return Err(WorkspaceError::Invalid);
            }
            Ok(StoredEvidence {
                kind: item.kind,
                reference: item.reference.trim().to_owned(),
                note: item.note.trim().to_owned(),
            })
        })
        .collect()
}

fn validate_definition(input: SurfaceDefinition) -> Result<StoredDefinition, WorkspaceError> {
    let type_id = input.type_id.as_str();
    let known = matches!(
        type_id,
        "repository"
            | "code.module"
            | "code.package"
            | "code.file"
            | "documentation"
            | "binary"
            | "service"
            | "api.endpoint"
            | "domain"
            | "host"
            | "container"
            | "configuration"
            | "other"
    );
    let extension = (type_id.starts_with("kit.") || type_id.starts_with("custom."))
        && type_id.split('.').all(|part| !part.is_empty());
    if type_id.len() > 64
        || !(known || extension)
        || !type_id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'.' || byte == b'_'
        })
    {
        return Err(WorkspaceError::Invalid);
    }
    let locator = input.locator.ok_or(WorkspaceError::Invalid)?;
    let value = locator.value.trim();
    let name = if input.name.trim().is_empty() {
        default_surface_name(value)
    } else {
        input.name.trim().to_owned()
    };
    let locator_allowed = match type_id {
        "repository" => matches!(locator.kind.as_str(), "freeform" | "git_url" | "url"),
        "code.module" | "code.package" | "code.file" => {
            matches!(locator.kind.as_str(), "freeform" | "repository_path")
        }
        "domain" => matches!(locator.kind.as_str(), "freeform" | "domain"),
        "service" | "api.endpoint" => {
            matches!(locator.kind.as_str(), "freeform" | "url" | "domain")
        }
        "documentation" => matches!(
            locator.kind.as_str(),
            "freeform" | "url" | "repository_path"
        ),
        "binary" | "configuration" => matches!(
            locator.kind.as_str(),
            "freeform" | "artifact" | "repository_path"
        ),
        _ => matches!(
            locator.kind.as_str(),
            "freeform" | "git_url" | "repository_path" | "url" | "domain" | "artifact"
        ),
    };
    if !valid_text(&name, 200, false)
        || !matches!(
            locator.kind.as_str(),
            "freeform" | "git_url" | "repository_path" | "url" | "domain" | "artifact"
        )
        || !locator_allowed
        || !valid_text(&locator.value, 2_048, false)
        || (locator.kind == "repository_path" && !valid_repository_path(&locator.value))
        || !valid_optional_text(&input.description, 10_000, true)
        || !valid_optional_text(&input.out_of_scope_reason, 2_000, true)
    {
        return Err(WorkspaceError::Invalid);
    }
    let scope = match SurfaceScope::try_from(input.scope) {
        Ok(SurfaceScope::InScope) => "in_scope",
        Ok(SurfaceScope::OutOfScope) => "out_of_scope",
        _ => return Err(WorkspaceError::Invalid),
    };
    let scope_state = match SurfaceScopeState::try_from(input.scope_state) {
        Ok(SurfaceScopeState::Unknown) => "unknown",
        Ok(SurfaceScopeState::Candidate) => "candidate",
        Ok(SurfaceScopeState::Approved) => "approved",
        Err(_) => return Err(WorkspaceError::Invalid),
    };
    if scope == "in_scope" && !input.out_of_scope_reason.is_empty() {
        return Err(WorkspaceError::Invalid);
    }
    Ok(StoredDefinition {
        type_id: type_id.to_owned(),
        name,
        locator: StoredLocator {
            kind: locator.kind,
            value: locator.value.trim().to_owned(),
        },
        description: input.description.trim().to_owned(),
        scope: scope.to_owned(),
        scope_state: scope_state.to_owned(),
        out_of_scope_reason: (!input.out_of_scope_reason.trim().is_empty())
            .then(|| input.out_of_scope_reason.trim().to_owned()),
        evidence_refs: validate_evidence_refs(input.evidence_refs)?,
        parent_id: String::new(),
        summary: String::new(),
        context: String::new(),
        scope_basis: String::new(),
    })
}

fn legacy_surface_id(kind: &str, value: &str) -> String {
    format!("legacy:{kind}:{}", value.to_lowercase())
}

fn surface_number(id: &str) -> Option<u64> {
    id.strip_prefix('S').and_then(|number| {
        if number.starts_with('0')
            || number.is_empty()
            || !number.bytes().all(|byte| byte.is_ascii_digit())
        {
            None
        } else {
            number.parse::<u64>().ok().filter(|value| *value > 0)
        }
    })
}

fn normalize_manifest(mut manifest: StoredManifest) -> Result<StoredManifest, WorkspaceError> {
    if manifest.schema_version == 3 {
        let mut renamed = std::collections::HashMap::new();
        for (index, surface) in manifest.surfaces.iter_mut().enumerate() {
            let old = if surface.id.is_empty() {
                legacy_surface_id(&surface.kind, &surface.value)
            } else {
                surface.id.clone()
            };
            let short = format!("S{}", index + 1);
            if renamed.insert(old.clone(), short.clone()).is_some() {
                return Err(WorkspaceError::Internal("duplicate legacy surface id"));
            }
            surface.legacy_ids.push(old);
            surface.id = short;
        }
        for surface in &mut manifest.surfaces {
            if let Some(definition) = &mut surface.definition
                && let Some(parent) = renamed.get(&definition.parent_id)
            {
                definition.parent_id = parent.clone();
            }
            for relationship in &mut surface.relationships {
                if let Some(target) = renamed.get(&relationship.target_id) {
                    relationship.target_id = target.clone();
                }
            }
        }
        manifest.next_surface_number = manifest.surfaces.len() as u64 + 1;
        manifest.schema_version = 4;
    }
    if manifest.schema_version == 4 {
        manifest.schema_version = 5;
    }
    if manifest.schema_version == 5 {
        let mut parent_links = Vec::new();
        let active_ids: HashSet<_> = manifest
            .surfaces
            .iter()
            .map(|surface| surface.id.clone())
            .collect();
        for surface in manifest
            .surfaces
            .iter_mut()
            .chain(manifest.retired_surfaces.iter_mut())
        {
            let definition = surface
                .definition
                .get_or_insert_with(|| legacy_definition(&surface.kind, &surface.value));
            if definition.scope.is_empty() {
                definition.scope = surface.scope.clone();
            }
            definition.scope_state.clone_from(&surface.scope_state);
            if definition.out_of_scope_reason.is_none() {
                definition
                    .out_of_scope_reason
                    .clone_from(&surface.out_of_scope_reason);
            }
            if definition.description.is_empty() {
                let mut parts = Vec::new();
                if !definition.summary.is_empty() {
                    parts.push(definition.summary.clone());
                }
                if !definition.context.is_empty() {
                    parts.push(definition.context.clone());
                }
                if !definition.scope_basis.is_empty() {
                    parts.push(format!("Authorization basis: {}", definition.scope_basis));
                }
                definition.description = parts.join("\n\n");
            }
            if !definition.parent_id.is_empty() && active_ids.contains(&surface.id) {
                parent_links.push((definition.parent_id.clone(), surface.id.clone()));
            }
        }
        for (parent, child) in parent_links {
            let surface = manifest
                .surfaces
                .iter_mut()
                .find(|item| item.id == parent)
                .ok_or(WorkspaceError::Internal("missing surface parent"))?;
            if parent == child {
                return Err(WorkspaceError::Internal("surface parent cycle"));
            }
            if !surface
                .relationships
                .iter()
                .any(|item| item.target_id == child)
            {
                surface.relationships.push(StoredRelationship {
                    target_id: child,
                    label: "contains".to_owned(),
                    type_id: "contains".to_owned(),
                    evidence_refs: Vec::new(),
                });
            }
        }
        manifest.schema_version = 6;
    }
    if manifest.schema_version != 6 {
        return Err(WorkspaceError::Internal(
            "invalid workspace manifest version",
        ));
    }
    let mut ids = HashSet::new();
    for surface in manifest
        .surfaces
        .iter_mut()
        .chain(manifest.retired_surfaces.iter_mut())
    {
        let definition = surface
            .definition
            .as_ref()
            .ok_or(WorkspaceError::Internal("missing surface definition"))?;
        let validated = validate_definition(definition_dto(definition.clone())?)
            .map_err(|_| WorkspaceError::Internal("invalid surface definition"))?;
        surface.kind = match validated.type_id.as_str() {
            "repository" | "code.module" | "code.package" | "code.file" => "repository",
            "documentation" => "documentation",
            "binary" | "configuration" => "binary",
            "service" | "api.endpoint" => "service",
            "domain" | "host" => "domain",
            _ => "other",
        }
        .to_owned();
        surface.scope.clone_from(&validated.scope);
        surface.scope_state.clone_from(&validated.scope_state);
        surface.value.clone_from(&validated.locator.value);
        surface
            .out_of_scope_reason
            .clone_from(&validated.out_of_scope_reason);
        surface.created_by = surface
            .lineage
            .first()
            .map_or("user", |event| {
                if matches!(event.actor.as_str(), "agent" | "ai_import") {
                    "agent"
                } else {
                    "user"
                }
            })
            .to_owned();
        surface.revision = surface.lineage.last().map_or(0, |event| event.number);
        surface.created_at_ms = surface.lineage.first().map_or(0, |event| event.at_ms);
        surface.updated_at_ms = surface.lineage.last().map_or(0, |event| event.at_ms);
        if surface_number(&surface.id).is_none() || !ids.insert(surface.id.clone()) {
            return Err(WorkspaceError::Internal("invalid surface id"));
        }
        if surface
            .lineage
            .iter()
            .enumerate()
            .any(|(index, event)| event.number != index as u64 + 1 || event.at_ms == 0)
        {
            return Err(WorkspaceError::Internal("invalid surface lineage"));
        }
    }
    let active_ids: HashSet<_> = manifest
        .surfaces
        .iter()
        .map(|surface| surface.id.as_str())
        .collect();
    if manifest.surfaces.iter().any(|surface| {
        surface_number(&surface.id).is_some_and(|number| number >= manifest.next_surface_number)
    }) {
        return Err(WorkspaceError::Internal("invalid surface counter"));
    }
    for surface in &manifest.surfaces {
        let mut targets = HashSet::new();
        if surface.relationships.iter().any(|relationship| {
            relationship.target_id == surface.id
                || !active_ids.contains(relationship.target_id.as_str())
                || !targets.insert(relationship.target_id.as_str())
                || !valid_text(&relationship.label, 80, false)
                || !valid_optional_text(&relationship.type_id, 64, false)
                || !relationship.type_id.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'.' | b'_')
                })
                || validate_evidence_refs(
                    relationship
                        .evidence_refs
                        .clone()
                        .into_iter()
                        .map(evidence_dto)
                        .collect(),
                )
                .is_err()
        }) {
            return Err(WorkspaceError::Internal("invalid surface relationship"));
        }
    }
    for surface in &manifest.retired_surfaces {
        if surface
            .lineage
            .last()
            .is_none_or(|event| event.action != "deleted")
        {
            return Err(WorkspaceError::Internal("invalid retired surface lineage"));
        }
    }
    Ok(manifest)
}

fn validate_surface_update_with_counter(
    inputs: Vec<WorkspaceSurfaceInput>,
    existing: &[StoredSurface],
    mut next_surface_number: u64,
) -> Result<(Vec<StoredSurface>, u64), WorkspaceError> {
    if inputs.len() > 256 {
        return Err(WorkspaceError::Invalid);
    }
    let mut seen = HashSet::new();
    let mut ids = HashSet::new();
    let mut remap = std::collections::HashMap::new();
    let mut result = Vec::with_capacity(inputs.len());
    for input in inputs {
        let mut definition = validate_definition(input.definition.ok_or(WorkspaceError::Invalid)?)?;
        let kind = match definition.type_id.as_str() {
            "repository" | "code.module" | "code.package" | "code.file" => "repository",
            "documentation" => "documentation",
            "binary" | "configuration" => "binary",
            "service" | "api.endpoint" => "service",
            "domain" | "host" => "domain",
            _ => "other",
        };
        let value = definition.locator.value.clone();
        if !seen.insert((definition.type_id.clone(), value.to_lowercase())) {
            return Err(WorkspaceError::Invalid);
        }
        let previous = existing
            .iter()
            .find(|item| item.id == input.id || item.legacy_ids.contains(&input.id));
        if previous.is_none()
            && !input.id.is_empty()
            && (surface_number(&input.id).is_some()
                || input.id.len() > 128
                || input.id.chars().any(char::is_control))
        {
            return Err(WorkspaceError::Invalid);
        }
        let id = if let Some(prior) = previous {
            prior.id.clone()
        } else {
            let id = format!("S{next_surface_number}");
            next_surface_number = next_surface_number
                .checked_add(1)
                .ok_or(WorkspaceError::Invalid)?;
            id
        };
        if !input.id.is_empty() && remap.insert(input.id, id.clone()).is_some() {
            return Err(WorkspaceError::Invalid);
        }
        if !ids.insert(id.clone()) || input.relationships.len() > 64 {
            return Err(WorkspaceError::Invalid);
        }
        if definition.scope_state == "unknown"
            && let Some(prior) = previous
        {
            definition.scope_state.clone_from(&prior.scope_state);
        }
        if definition.scope == "out_of_scope" && definition.out_of_scope_reason.is_none() {
            definition.out_of_scope_reason = Some("Set in workspace".to_owned());
        }
        let unchanged = previous.is_some_and(|prior| {
            prior.kind == kind
                && prior.scope == definition.scope
                && prior.value.eq_ignore_ascii_case(&value)
        });
        let created_by = if unchanged {
            previous.map_or("user", |prior| prior.created_by.as_str())
        } else if definition.scope_state == "candidate" {
            "agent"
        } else {
            "user"
        };
        let relationships = input
            .relationships
            .into_iter()
            .map(|relationship| {
                Ok(StoredRelationship {
                    target_id: relationship.target_id,
                    label: relationship.label.trim().to_owned(),
                    type_id: relationship.type_id.trim().to_owned(),
                    evidence_refs: validate_evidence_refs(relationship.evidence_refs)?,
                })
            })
            .collect::<Result<Vec<_>, WorkspaceError>>()?;
        result.push(StoredSurface {
            id,
            kind: kind.to_owned(),
            scope: definition.scope.clone(),
            scope_state: definition.scope_state.clone(),
            value,
            created_by: created_by.to_owned(),
            out_of_scope_reason: definition.out_of_scope_reason.clone(),
            relationships,
            definition: Some(definition),
            legacy_ids: previous.map_or_else(Vec::new, |prior| prior.legacy_ids.clone()),
            created_at_ms: 0,
            updated_at_ms: 0,
            revision: 0,
            lineage: Vec::new(),
        });
    }
    for surface in &mut result {
        for relationship in &mut surface.relationships {
            if let Some(target) = remap.get(&relationship.target_id) {
                relationship.target_id = target.clone();
            }
        }
        let mut targets = HashSet::new();
        for relationship in &surface.relationships {
            if relationship.target_id == surface.id
                || !ids.contains(&relationship.target_id)
                || !targets.insert(&relationship.target_id)
                || !valid_text(&relationship.label, 80, false)
                || !valid_optional_text(&relationship.type_id, 64, false)
                || !relationship.type_id.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'.' | b'_')
                })
            {
                return Err(WorkspaceError::Invalid);
            }
        }
    }
    Ok((result, next_surface_number))
}

#[cfg(test)]
fn validate_surface_update(
    inputs: Vec<beale_contracts::v1::WorkspaceSurfaceInput>,
    existing: &[StoredSurface],
) -> Result<Vec<StoredSurface>, WorkspaceError> {
    let next = existing
        .iter()
        .filter_map(|surface| surface_number(&surface.id))
        .max()
        .unwrap_or(0)
        + 1;
    validate_surface_update_with_counter(inputs, existing, next).map(|(surfaces, _)| surfaces)
}

fn validate_rules(inputs: Vec<String>) -> Result<Vec<String>, WorkspaceError> {
    if inputs.len() > 128 {
        return Err(WorkspaceError::Invalid);
    }
    let mut seen = HashSet::new();
    let mut result = Vec::with_capacity(inputs.len());
    for input in inputs {
        let rule = input.trim();
        if !valid_text(rule, 2_000, true) || !seen.insert(rule.to_owned()) {
            return Err(WorkspaceError::Invalid);
        }
        result.push(rule.to_owned());
    }
    Ok(result)
}

fn write_manifest(directory: &Path, manifest: &StoredManifest) -> Result<(), WorkspaceError> {
    let mut bytes = serde_json::to_vec_pretty(manifest)
        .map_err(|_| WorkspaceError::Internal("encode workspace manifest"))?;
    bytes.push(b'\n');
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(WorkspaceError::Invalid);
    }
    write_replace(&directory.join("workspace.json"), &bytes)
}

fn write_replace(path: &Path, bytes: &[u8]) -> Result<(), WorkspaceError> {
    let parent = path.parent().ok_or(WorkspaceError::Invalid)?;
    let temp = parent
        .join(".beale")
        .join(format!("update-{}.tmp", Uuid::new_v4()));
    write_new(&temp, bytes)?;
    if fs::rename(&temp, path).is_err() {
        let _ = fs::remove_file(&temp);
        return Err(WorkspaceError::Internal("replace workspace file"));
    }
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), WorkspaceError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| WorkspaceError::Internal("create workspace file"))?;
    file.write_all(bytes)
        .map_err(|_| WorkspaceError::Internal("write workspace file"))?;
    file.sync_all()
        .map_err(|_| WorkspaceError::Internal("sync workspace file"))
}

fn valid_text(value: &str, max: usize, multiline: bool) -> bool {
    !value.trim().is_empty()
        && value.len() <= max
        && !value.chars().any(|character| {
            character.is_control() && !(multiline && matches!(character, '\n' | '\t'))
        })
}

fn validate(request: CreateWorkspaceRequest) -> Result<ValidatedWorkspace, WorkspaceError> {
    let name = request.name.trim();
    let guidance = request.guidance.replace("\r\n", "\n").replace('\r', "\n");
    if !valid_text(name, 200, false)
        || guidance.len() > 20_000
        || guidance
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
        || request.surfaces.len() > 256
        || request.rules.len() > 128
    {
        return Err(WorkspaceError::Invalid);
    }
    let directory = normalize_directory(&request.directory)?;
    let (mut surfaces, next_surface_number) =
        validate_surface_update_with_counter(request.surfaces, &[], 1)?;
    for surface in &mut surfaces {
        if surface.scope == "out_of_scope"
            && let Some(definition) = &mut surface.definition
            && definition.out_of_scope_reason.is_none()
        {
            definition.out_of_scope_reason = Some("Set at workspace creation".to_owned());
            surface
                .out_of_scope_reason
                .clone_from(&definition.out_of_scope_reason);
        }
    }
    stamp_lineage(&mut surfaces, &[], "operator")?;
    let mut rules = Vec::with_capacity(request.rules.len());
    let mut seen_rules = HashSet::new();
    for rule in request.rules {
        let rule = rule.trim();
        if !valid_text(rule, 2_000, true) || !seen_rules.insert(rule.to_owned()) {
            return Err(WorkspaceError::Invalid);
        }
        rules.push(rule.to_owned());
    }
    if rules.is_empty() && !surfaces.iter().any(|surface| surface.scope == "in_scope") {
        return Err(WorkspaceError::Invalid);
    }
    Ok(ValidatedWorkspace {
        name: name.to_owned(),
        directory,
        guidance: guidance.trim().to_owned(),
        surfaces,
        next_surface_number,
        rules,
    })
}

fn normalize_directory(value: &str) -> Result<PathBuf, WorkspaceError> {
    if !valid_text(value, 4_096, false) {
        return Err(WorkspaceError::Invalid);
    }
    let path = Path::new(value.trim());
    if !path.is_absolute() {
        return Err(WorkspaceError::Invalid);
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(WorkspaceError::Conflict)
        }
        Ok(_) => fs::canonicalize(path)
            .map_err(|_| WorkspaceError::Internal("canonicalize workspace directory")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or(WorkspaceError::Invalid)?;
            let name = path.file_name().ok_or(WorkspaceError::Invalid)?;
            let parent = fs::canonicalize(parent).map_err(|_| WorkspaceError::Invalid)?;
            Ok(parent.join(name))
        }
        Err(_) => Err(WorkspaceError::Internal("inspect workspace directory")),
    }
}

#[derive(Clone)]
struct WorkspaceState {
    token: String,
    store: WorkspaceStore,
    write_lock: Arc<Mutex<()>>,
}

pub fn router(token: String, store: WorkspaceStore) -> Router {
    Router::new()
        .route(WORKSPACES_PATH, get(list).post(create))
        .route(
            beale_contracts::WORKSPACE_IMPORT_STREAM_PATH,
            axum::routing::post(import::stream),
        )
        .route(WORKSPACE_DETAILS_PATH, get(details))
        .route(WORKSPACE_CAMPAIGN_PATH, get(campaign))
        .route(
            WORKSPACE_FOUNDATION_PATH,
            get(foundation).put(update_foundation),
        )
        .route(WORKSPACE_SURFACES_PATH, get(surfaces).put(update_surfaces))
        .route(WORKSPACE_RULES_PATH, get(rules).put(update_rules))
        .layer(DefaultBodyLimit::max(1_048_576))
        .with_state(WorkspaceState {
            token,
            store,
            write_lock: Arc::new(Mutex::new(())),
        })
}

async fn list(
    State(state): State<WorkspaceState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let workspaces = state.store.list().map_err(|error| {
        error.report();
        error.status()
    })?;
    Ok(response(ListWorkspacesResponse { workspaces }))
}

async fn create(
    State(state): State<WorkspaceState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let request: CreateWorkspaceRequest = checked_body(&headers, &body)?;
    let _guard = state.write_lock.lock().await;
    let created = state.store.create(request).map_err(|error| {
        error.report();
        error.status()
    })?;
    Ok((StatusCode::CREATED, response(created)))
}

async fn details(
    State(state): State<WorkspaceState>,
    RoutePath(id): RoutePath<String>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let details = state.store.details(&id).map_err(|error| {
        error.report();
        error.status()
    })?;
    Ok(response(details))
}

async fn campaign(
    State(state): State<WorkspaceState>,
    RoutePath(id): RoutePath<String>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    state.store.entry(&id).map_err(|error| error.status())?;
    Ok(response(WorkspaceCampaign {
        enabled_feature_ids: Vec::new(),
    }))
}

async fn foundation(
    State(state): State<WorkspaceState>,
    RoutePath(id): RoutePath<String>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let result = state.store.foundation(&id).map_err(|error| {
        error.report();
        error.status()
    })?;
    Ok(response(result))
}

async fn update_foundation(
    State(state): State<WorkspaceState>,
    RoutePath(id): RoutePath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let request: UpdateWorkspaceFoundationRequest = checked_body(&headers, &body)?;
    let _guard = state.write_lock.lock().await;
    let result = state
        .store
        .update_foundation(&id, request)
        .map_err(|error| {
            error.report();
            error.status()
        })?;
    Ok(response(result))
}

async fn surfaces(
    State(state): State<WorkspaceState>,
    RoutePath(id): RoutePath<String>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let result = state.store.surfaces(&id).map_err(|error| {
        error.report();
        error.status()
    })?;
    Ok(response(result))
}

async fn update_surfaces(
    State(state): State<WorkspaceState>,
    RoutePath(id): RoutePath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let request: UpdateWorkspaceSurfacesRequest = checked_body(&headers, &body)?;
    let _guard = state.write_lock.lock().await;
    let result = state.store.update_surfaces(&id, request).map_err(|error| {
        error.report();
        error.status()
    })?;
    Ok(response(result))
}

async fn rules(
    State(state): State<WorkspaceState>,
    RoutePath(id): RoutePath<String>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let result = state.store.rules(&id).map_err(|error| {
        error.report();
        error.status()
    })?;
    Ok(response(result))
}

async fn update_rules(
    State(state): State<WorkspaceState>,
    RoutePath(id): RoutePath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let request: UpdateWorkspaceRulesRequest = checked_body(&headers, &body)?;
    let _guard = state.write_lock.lock().await;
    let result = state.store.update_rules(&id, request).map_err(|error| {
        error.report();
        error.status()
    })?;
    Ok(response(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use beale_contracts::{PROTOBUF_CONTENT_TYPE, WORKSPACES_PATH};
    use prost::Message;
    use tower::ServiceExt;

    fn example_input(value: &str, scope: SurfaceScope) -> WorkspaceSurfaceInput {
        WorkspaceSurfaceInput {
            id: String::new(),
            definition: Some(SurfaceDefinition {
                type_id: "domain".to_owned(),
                name: value.to_owned(),
                locator: Some(SurfaceLocator {
                    kind: "domain".to_owned(),
                    value: value.to_owned(),
                }),
                scope: scope as i32,
                scope_state: SurfaceScopeState::Approved as i32,
                out_of_scope_reason: if scope == SurfaceScope::OutOfScope {
                    "Excluded by operator.".to_owned()
                } else {
                    String::new()
                },
                ..Default::default()
            }),
            relationships: Vec::new(),
        }
    }

    fn example_request(directory: &Path) -> CreateWorkspaceRequest {
        CreateWorkspaceRequest {
            name: "Example Workspace".to_owned(),
            directory: directory.to_string_lossy().into_owned(),
            guidance: "Stay within recorded scope.".to_owned(),
            surfaces: vec![
                example_input("example.test", SurfaceScope::InScope),
                example_input("excluded.example.test", SurfaceScope::OutOfScope),
            ],
            rules: vec!["Do not contact excluded third parties.".to_owned()],
        }
    }

    #[test]
    fn required_definition_and_evidence_refs_are_validated() {
        let mut input = example_input("example.test", SurfaceScope::InScope);
        assert!(validate_surface_update(vec![WorkspaceSurfaceInput::default()], &[]).is_err());
        input
            .definition
            .as_mut()
            .unwrap()
            .evidence_refs
            .push(SurfaceEvidence {
                kind: "tool_result".to_owned(),
                reference: "result-example-001".to_owned(),
                note: "Observed host response".to_owned(),
            });
        let saved = validate_surface_update(vec![input.clone()], &[]).unwrap();
        assert_eq!(
            saved[0].definition.as_ref().unwrap().evidence_refs[0].reference,
            "result-example-001"
        );
        input.definition.as_mut().unwrap().evidence_refs[0].kind = "Invalid Kind".to_owned();
        assert!(validate_surface_update(vec![input], &[]).is_err());
    }

    #[test]
    fn relationships_remap_draft_ids_and_preserve_evidence() {
        let mut source = example_input("example.test", SurfaceScope::InScope);
        source.id = "draft-source".to_owned();
        source.relationships.push(SurfaceRelationship {
            target_id: "draft-target".to_owned(),
            label: "calls".to_owned(),
            type_id: "calls".to_owned(),
            evidence_refs: vec![SurfaceEvidence {
                kind: "artifact".to_owned(),
                reference: "artifact-example-001".to_owned(),
                note: String::new(),
            }],
        });
        let mut target = example_input("api.example.test", SurfaceScope::InScope);
        target.id = "draft-target".to_owned();
        let saved = validate_surface_update(vec![source.clone(), target], &[]).unwrap();
        assert_eq!(saved[0].relationships[0].target_id, "S2");
        assert_eq!(
            saved[0].relationships[0].evidence_refs[0].reference,
            "artifact-example-001"
        );
        assert!(validate_surface_update(vec![source], &[]).is_err());
    }

    #[test]
    fn v5_manifest_migrates_parent_and_description_without_progress_fields() {
        let value = serde_json::json!({
            "schemaVersion":5,"workspaceId":"workspace-example","name":"Example Workspace","directories":[],"nextSurfaceNumber":3,"rules":[],
            "surfaces":[
                {"id":"S1","kind":"repository","scope":"in_scope","scopeState":"approved","value":"example repository","createdBy":"user","mappingProgress":75,"relationships":[],"definition":{"typeId":"repository","name":"Example repository","locator":{"kind":"freeform","value":"example repository"},"summary":"Research code","source":"operator"}},
                {"id":"S2","kind":"other","scope":"in_scope","value":"src/parser","createdBy":"user","relationships":[],"definition":{"typeId":"code.module","name":"Parser","locator":{"kind":"repository_path","value":"src/parser"},"parentId":"S1","context":"Parses inputs","source":"operator"}}
            ]
        });
        let manifest: StoredManifest = serde_json::from_value(value).unwrap();
        let migrated = normalize_manifest(manifest).unwrap();
        assert_eq!(migrated.schema_version, 6);
        assert_eq!(migrated.surfaces[0].relationships[0].target_id, "S2");
        assert_eq!(
            migrated.surfaces[1]
                .definition
                .as_ref()
                .unwrap()
                .description,
            "Parses inputs"
        );
        let serialized = serde_json::to_value(&migrated).unwrap();
        let surface = serialized["surfaces"][0].as_object().unwrap();
        assert_eq!(surface.len(), 4);
        for key in ["id", "definition", "relationships", "lineage"] {
            assert!(surface.contains_key(key));
        }
        assert!(
            serialized["surfaces"][1]["definition"]
                .get("parentId")
                .is_none()
        );
        assert!(serialized["surfaces"][0].get("mappingProgress").is_none());
    }

    #[test]
    fn v3_manifest_rewrites_legacy_parent_ids_as_relationships() {
        let value = serde_json::json!({
            "schemaVersion":3,"workspaceId":"workspace-example","name":"Example Workspace","directories":[],"rules":[],
            "surfaces":[
                {"id":"legacy-repo","kind":"repository","scope":"in_scope","value":"example repository","createdBy":"user","definition":{"typeId":"repository","name":"Example repository","locator":{"kind":"freeform","value":"example repository"}}},
                {"id":"legacy-module","kind":"other","scope":"in_scope","value":"src/parser","createdBy":"user","definition":{"typeId":"code.module","name":"Parser","locator":{"kind":"repository_path","value":"src/parser"},"parentId":"legacy-repo"}}
            ]
        });
        let manifest: StoredManifest = serde_json::from_value(value).unwrap();
        let migrated = normalize_manifest(manifest).unwrap();
        assert_eq!(migrated.schema_version, 6);
        assert_eq!(migrated.surfaces[0].id, "S1");
        assert_eq!(migrated.surfaces[0].relationships[0].target_id, "S2");
        assert_eq!(
            migrated.surfaces[1].definition.as_ref().unwrap().scope,
            "in_scope"
        );
    }

    #[test]
    fn lineage_and_manifest_survive_edits_and_retirement() {
        let base = std::env::temp_dir().join(format!("beale-surface-example-{}", Uuid::new_v4()));
        fs::create_dir_all(&base).unwrap();
        let directory = base.join("workspace");
        let store = WorkspaceStore::at(base.join("host")).unwrap();
        let summary = store.create(example_request(&directory)).unwrap();
        let original = store.surfaces(&summary.id).unwrap();
        assert_eq!(original.surfaces[0].lineage[0].action, "created");
        let mut inputs = original
            .surfaces
            .into_iter()
            .map(|surface| WorkspaceSurfaceInput {
                id: surface.id,
                definition: surface.definition,
                relationships: surface.relationships,
            })
            .collect::<Vec<_>>();
        inputs[0].definition.as_mut().unwrap().description =
            "Updated example description".to_owned();
        let updated = store
            .update_surfaces(
                &summary.id,
                UpdateWorkspaceSurfacesRequest {
                    expected_revision: original.revision,
                    surfaces: inputs,
                },
            )
            .unwrap();
        assert_eq!(updated.surfaces[0].lineage.len(), 2);
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(directory.join("workspace.json")).unwrap()).unwrap();
        assert_eq!(manifest["schemaVersion"], 6);
        assert_eq!(
            manifest["surfaces"][0]["definition"]["description"],
            "Updated example description"
        );
        drop(store);
        let reopened = WorkspaceStore::at(base.join("host")).unwrap();
        assert_eq!(
            reopened.surfaces(&summary.id).unwrap().surfaces[0]
                .lineage
                .len(),
            2
        );
        reopened
            .update_surfaces(
                &summary.id,
                UpdateWorkspaceSurfacesRequest {
                    expected_revision: updated.revision,
                    surfaces: Vec::new(),
                },
            )
            .unwrap();
        let (retired, deleted) = reopened
            .surface_for_host(&summary.id, "S1")
            .unwrap()
            .unwrap();
        assert!(deleted);
        assert_eq!(retired.lineage.last().unwrap().action, "deleted");
        drop(reopened);
        fs::remove_dir_all(base).unwrap();
    }

    #[tokio::test]
    async fn create_route_requires_authentication_and_returns_new_contract() {
        let base = std::env::temp_dir().join(format!("beale-route-example-{}", Uuid::new_v4()));
        fs::create_dir_all(&base).unwrap();
        let directory = base.join("workspace");
        let token = "ab".repeat(32);
        let app = router(
            token.clone(),
            WorkspaceStore::at(base.join("host")).unwrap(),
        );
        let payload = example_request(&directory).encode_to_vec();
        let request = |authorized: bool| {
            let mut builder = Request::builder()
                .method("POST")
                .uri(WORKSPACES_PATH)
                .header(axum::http::header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE);
            if authorized {
                builder =
                    builder.header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"));
            }
            builder.body(Body::from(payload.clone())).unwrap()
        };
        assert_eq!(
            app.clone().oneshot(request(false)).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let result = app.oneshot(request(true)).await.unwrap();
        assert_eq!(result.status(), StatusCode::CREATED);
        let summary =
            WorkspaceSummary::decode(to_bytes(result.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(summary.name, "Example Workspace");
        fs::remove_dir_all(base).unwrap();
    }
}
