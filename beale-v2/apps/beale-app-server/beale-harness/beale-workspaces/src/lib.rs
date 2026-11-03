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
        CreateWorkspaceRequest, ListWorkspacesResponse, SurfaceCreator, SurfaceKind, SurfaceScope,
        UpdateWorkspaceFoundationRequest, UpdateWorkspaceRulesRequest,
        UpdateWorkspaceSurfacesRequest, WorkspaceCampaign, WorkspaceDetails, WorkspaceFoundation,
        WorkspaceRules, WorkspaceSummary, WorkspaceSurface, WorkspaceSurfaces,
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
};
use tokio::sync::Mutex;
use uuid::Uuid;

mod http;
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

#[derive(Clone)]
pub struct WorkspaceStore {
    root: PathBuf,
    db: HostDatabase,
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
    surfaces: Vec<SurfaceRecord>,
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
    kind: String,
    scope: String,
    value: String,
    created_by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    out_of_scope_reason: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SurfaceRecord {
    kind: &'static str,
    scope: &'static str,
    value: String,
    created_by: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    out_of_scope_reason: Option<&'static str>,
}

struct ValidatedWorkspace {
    name: String,
    directory: PathBuf,
    guidance: String,
    surfaces: Vec<SurfaceRecord>,
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
        Ok(Self { root, db })
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

    pub fn root(&self) -> &Path {
        &self.root
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
        if manifest_bytes.len() > 1_048_576 {
            return Err(WorkspaceError::Internal(
                "workspace manifest exceeds size limit",
            ));
        }
        let manifest: StoredManifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|_| WorkspaceError::Internal("decode workspace manifest"))?;
        if manifest.schema_version != 3
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
            .map(|surface| {
                let kind = match surface.kind.as_str() {
                    "repository" => SurfaceKind::Repository,
                    "documentation" => SurfaceKind::Documentation,
                    "binary" => SurfaceKind::Binary,
                    "service" => SurfaceKind::Service,
                    "domain" => SurfaceKind::Domain,
                    "other" => SurfaceKind::Other,
                    _ => return Err(WorkspaceError::Internal("invalid workspace surface kind")),
                };
                let scope = match surface.scope.as_str() {
                    "in_scope" => SurfaceScope::InScope,
                    "out_of_scope" => SurfaceScope::OutOfScope,
                    _ => return Err(WorkspaceError::Internal("invalid workspace surface scope")),
                };
                let created_by = match surface.created_by.as_str() {
                    "user" => SurfaceCreator::User,
                    "agent" => SurfaceCreator::Agent,
                    _ => {
                        return Err(WorkspaceError::Internal(
                            "invalid workspace surface creator",
                        ));
                    }
                };
                Ok(WorkspaceSurface {
                    kind: kind as i32,
                    scope: scope as i32,
                    value: surface.value,
                    created_by: created_by as i32,
                    out_of_scope_reason: surface.out_of_scope_reason.unwrap_or_default(),
                })
            })
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
        if bytes.len() > 1_048_576 {
            return Err(WorkspaceError::Internal(
                "workspace manifest exceeds size limit",
            ));
        }
        let manifest: StoredManifest = serde_json::from_slice(&bytes)
            .map_err(|_| WorkspaceError::Internal("decode workspace manifest"))?;
        if manifest.schema_version != 3
            || manifest.workspace_id != entry.id
            || manifest.name != entry.name
            || manifest.surfaces.len() > 256
            || manifest.rules.len() > 128
            || manifest.foundation_revision == 0
            || manifest.surfaces_revision == 0
            || manifest.rules_revision == 0
        {
            return Err(WorkspaceError::Internal("invalid workspace manifest"));
        }
        for surface in &manifest.surfaces {
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
        let entry = self.entry(id)?;
        let mut manifest = Self::manifest(&entry)?;
        if request.expected_revision != manifest.surfaces_revision {
            return Err(WorkspaceError::Conflict);
        }
        manifest.surfaces = validate_surface_update(request.surfaces, &manifest.surfaces)?;
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
        let entry = self.entry(id)?;
        let mut manifest = Self::manifest(&entry)?;
        if request.expected_revision != manifest.rules_revision {
            return Err(WorkspaceError::Conflict);
        }
        manifest.rules = validate_rules(request.rules)?;
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
            schema_version: 3,
            workspace_id: id.clone(),
            name: input.name.clone(),
            directories: &DIRECTORIES,
            surfaces: input.surfaces,
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
    let kind = match surface.kind.as_str() {
        "repository" => SurfaceKind::Repository,
        "documentation" => SurfaceKind::Documentation,
        "binary" => SurfaceKind::Binary,
        "service" => SurfaceKind::Service,
        "domain" => SurfaceKind::Domain,
        "other" => SurfaceKind::Other,
        _ => return Err(WorkspaceError::Internal("invalid workspace surface kind")),
    };
    let scope = match surface.scope.as_str() {
        "in_scope" => SurfaceScope::InScope,
        "out_of_scope" => SurfaceScope::OutOfScope,
        _ => return Err(WorkspaceError::Internal("invalid workspace surface scope")),
    };
    let created_by = match surface.created_by.as_str() {
        "user" => SurfaceCreator::User,
        "agent" => SurfaceCreator::Agent,
        _ => {
            return Err(WorkspaceError::Internal(
                "invalid workspace surface creator",
            ));
        }
    };
    Ok(WorkspaceSurface {
        kind: kind as i32,
        scope: scope as i32,
        value: surface.value,
        created_by: created_by as i32,
        out_of_scope_reason: surface.out_of_scope_reason.unwrap_or_default(),
    })
}

fn validate_surface_update(
    inputs: Vec<beale_contracts::v1::WorkspaceSurfaceInput>,
    existing: &[StoredSurface],
) -> Result<Vec<StoredSurface>, WorkspaceError> {
    if inputs.is_empty() || inputs.len() > 256 {
        return Err(WorkspaceError::Invalid);
    }
    let mut seen = HashSet::new();
    let mut has_in_scope = false;
    let mut result = Vec::with_capacity(inputs.len());
    for input in inputs {
        let kind = match SurfaceKind::try_from(input.kind) {
            Ok(SurfaceKind::Repository) => "repository",
            Ok(SurfaceKind::Documentation) => "documentation",
            Ok(SurfaceKind::Binary) => "binary",
            Ok(SurfaceKind::Service) => "service",
            Ok(SurfaceKind::Domain) => "domain",
            Ok(SurfaceKind::Other) => "other",
            _ => return Err(WorkspaceError::Invalid),
        };
        let scope = match SurfaceScope::try_from(input.scope) {
            Ok(SurfaceScope::InScope) => {
                has_in_scope = true;
                "in_scope"
            }
            Ok(SurfaceScope::OutOfScope) => "out_of_scope",
            _ => return Err(WorkspaceError::Invalid),
        };
        let value = input.value.trim();
        if !valid_text(value, 2_048, false) || !seen.insert((kind, value.to_lowercase())) {
            return Err(WorkspaceError::Invalid);
        }
        let previous = existing
            .iter()
            .find(|item| item.kind == kind && item.value.eq_ignore_ascii_case(value));
        let created_by = previous.map_or("user", |item| item.created_by.as_str());
        let reason = if scope == "out_of_scope" {
            previous
                .filter(|item| item.scope == "out_of_scope")
                .and_then(|item| item.out_of_scope_reason.clone())
                .or_else(|| Some("Set in workspace".to_owned()))
        } else {
            None
        };
        result.push(StoredSurface {
            kind: kind.to_owned(),
            scope: scope.to_owned(),
            value: value.to_owned(),
            created_by: created_by.to_owned(),
            out_of_scope_reason: reason,
        });
    }
    if !has_in_scope {
        return Err(WorkspaceError::Invalid);
    }
    Ok(result)
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
        || request.surfaces.is_empty()
        || request.surfaces.len() > 256
        || request.rules.len() > 128
    {
        return Err(WorkspaceError::Invalid);
    }
    let directory = normalize_directory(&request.directory)?;
    let mut surfaces = Vec::with_capacity(request.surfaces.len());
    let mut seen = HashSet::new();
    let mut has_in_scope = false;
    for surface in request.surfaces {
        let kind = match SurfaceKind::try_from(surface.kind) {
            Ok(SurfaceKind::Repository) => "repository",
            Ok(SurfaceKind::Documentation) => "documentation",
            Ok(SurfaceKind::Binary) => "binary",
            Ok(SurfaceKind::Service) => "service",
            Ok(SurfaceKind::Domain) => "domain",
            Ok(SurfaceKind::Other) => "other",
            _ => return Err(WorkspaceError::Invalid),
        };
        let (scope, reason) = match SurfaceScope::try_from(surface.scope) {
            Ok(SurfaceScope::InScope) => {
                has_in_scope = true;
                ("in_scope", None)
            }
            Ok(SurfaceScope::OutOfScope) => ("out_of_scope", Some("Set at workspace creation")),
            _ => return Err(WorkspaceError::Invalid),
        };
        let value = surface.value.trim();
        if !valid_text(value, 2_048, false) || !seen.insert((kind, value.to_lowercase())) {
            return Err(WorkspaceError::Invalid);
        }
        surfaces.push(SurfaceRecord {
            kind,
            scope,
            value: value.to_owned(),
            created_by: "user",
            out_of_scope_reason: reason,
        });
    }
    if !has_in_scope {
        return Err(WorkspaceError::Invalid);
    }
    let mut rules = Vec::with_capacity(request.rules.len());
    let mut seen_rules = HashSet::new();
    for rule in request.rules {
        let rule = rule.trim();
        if !valid_text(rule, 2_000, true) || !seen_rules.insert(rule.to_owned()) {
            return Err(WorkspaceError::Invalid);
        }
        rules.push(rule.to_owned());
    }
    Ok(ValidatedWorkspace {
        name: name.to_owned(),
        directory,
        guidance: guidance.trim().to_owned(),
        surfaces,
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
        http::{Request, header},
    };
    use beale_contracts::PROTOBUF_CONTENT_TYPE;
    use prost::Message;
    use tower::ServiceExt;

    fn example_request(directory: &Path) -> CreateWorkspaceRequest {
        CreateWorkspaceRequest {
            name: "Example Workspace".to_owned(),
            directory: directory.to_string_lossy().into_owned(),
            guidance: "# Example guidance\n\nStay within scope.\n".to_owned(),
            surfaces: vec![
                beale_contracts::v1::WorkspaceSurfaceInput {
                    kind: SurfaceKind::Domain as i32,
                    scope: SurfaceScope::InScope as i32,
                    value: "example.test".to_owned(),
                },
                beale_contracts::v1::WorkspaceSurfaceInput {
                    kind: SurfaceKind::Domain as i32,
                    scope: SurfaceScope::OutOfScope as i32,
                    value: "excluded.example.test".to_owned(),
                },
            ],
            rules: vec!["Do not contact excluded third parties.".to_owned()],
        }
    }

    fn post(token: Option<&str>, body: Vec<u8>) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri(WORKSPACES_PATH)
            .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE);
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        builder.body(Body::from(body)).unwrap()
    }

    #[tokio::test]
    async fn create_and_list_preserve_scope_and_require_authentication() {
        let base = std::env::temp_dir().join(format!("beale-workspace-example-{}", Uuid::new_v4()));
        let directory = base.join("workspace");
        fs::create_dir_all(&directory).unwrap();
        let token = "ab".repeat(32);
        let app = router(
            token.clone(),
            WorkspaceStore::at(base.join("host")).unwrap(),
        );
        let payload = example_request(&directory).encode_to_vec();
        assert_eq!(
            app.clone()
                .oneshot(post(None, payload.clone()))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED,
        );
        assert!(fs::read_dir(&directory).unwrap().next().is_none());
        let result = app
            .clone()
            .oneshot(post(Some(&token), payload.clone()))
            .await
            .unwrap();
        assert_eq!(result.status(), StatusCode::CREATED);
        assert_eq!(
            result.headers()[header::CONTENT_TYPE],
            PROTOBUF_CONTENT_TYPE
        );
        let bytes = to_bytes(result.into_body(), 4096).await.unwrap();
        let summary = WorkspaceSummary::decode(bytes).unwrap();
        assert_eq!(summary.name, "Example Workspace");
        assert_eq!(
            summary.directory,
            fs::canonicalize(&directory).unwrap().to_string_lossy()
        );
        assert!(Uuid::parse_str(&summary.id).is_ok());
        assert!(directory.join("AGENTS.md").is_file());
        assert_eq!(
            fs::read_to_string(directory.join("AGENTS.md")).unwrap(),
            "# Example guidance\n\nStay within scope.\n"
        );
        assert!(directory.join(".beale").is_dir());
        for name in DIRECTORIES {
            assert!(directory.join(name).is_dir(), "{name}");
        }
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(directory.join("workspace.json")).unwrap()).unwrap();
        assert_eq!(manifest["schemaVersion"], 3);
        assert_eq!(manifest["workspaceId"], summary.id);
        assert_eq!(manifest["surfaces"][0]["scope"], "in_scope");
        assert_eq!(manifest["surfaces"][0]["createdBy"], "user");
        assert_eq!(manifest["surfaces"][1]["scope"], "out_of_scope");
        assert_eq!(
            manifest["surfaces"][1]["outOfScopeReason"],
            "Set at workspace creation"
        );
        let detail_path = format!("{WORKSPACES_PATH}/{}", summary.id);
        let unauthorized_details = Request::builder()
            .uri(&detail_path)
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone()
                .oneshot(unauthorized_details)
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let details_request = Request::builder()
            .uri(&detail_path)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let detail_response = app.clone().oneshot(details_request).await.unwrap();
        assert_eq!(detail_response.status(), StatusCode::OK);
        let detail_bytes = to_bytes(detail_response.into_body(), 65_536).await.unwrap();
        let details = WorkspaceDetails::decode(detail_bytes).unwrap();
        assert_eq!(details.summary, Some(summary.clone()));
        assert_eq!(
            details.guidance,
            "# Example guidance\n\nStay within scope.\n"
        );
        assert_eq!(details.surfaces[0].kind, SurfaceKind::Domain as i32);
        assert_eq!(details.surfaces[0].scope, SurfaceScope::InScope as i32);
        assert_eq!(details.surfaces[0].created_by, SurfaceCreator::User as i32);
        assert_eq!(details.surfaces[1].scope, SurfaceScope::OutOfScope as i32);
        assert_eq!(
            details.surfaces[1].out_of_scope_reason,
            "Set at workspace creation"
        );
        assert_eq!(details.rules, ["Do not contact excluded third parties."]);
        assert_eq!(
            app.clone()
                .oneshot(post(Some(&token), payload))
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT,
        );
        let second_directory = base.join("workspace-two");
        let mut second_input = example_request(&second_directory);
        second_input.name = "Second Example Workspace".to_owned();
        let created_second = app
            .clone()
            .oneshot(post(Some(&token), second_input.encode_to_vec()))
            .await
            .unwrap();
        assert_eq!(created_second.status(), StatusCode::CREATED);
        let bytes = to_bytes(created_second.into_body(), 4096).await.unwrap();
        let second_summary = WorkspaceSummary::decode(bytes).unwrap();
        let unauthorized = Request::builder()
            .uri(WORKSPACES_PATH)
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(unauthorized).await.unwrap().status(),
            StatusCode::UNAUTHORIZED,
        );
        let list_request = Request::builder()
            .uri(WORKSPACES_PATH)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let listed = app.oneshot(list_request).await.unwrap();
        assert_eq!(listed.status(), StatusCode::OK);
        let bytes = to_bytes(listed.into_body(), 4096).await.unwrap();
        let list = ListWorkspacesResponse::decode(bytes).unwrap();
        assert_eq!(list.workspaces, vec![summary, second_summary]);
        assert_eq!(
            WorkspaceStore::at(base.join("host"))
                .unwrap()
                .list()
                .unwrap()
                .len(),
            2
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[tokio::test]
    async fn workspace_sections_save_independently_with_revision_checks() {
        let base = std::env::temp_dir().join(format!("beale-section-example-{}", Uuid::new_v4()));
        fs::create_dir_all(&base).unwrap();
        let store = WorkspaceStore::at(base.join("host")).unwrap();
        let summary = store
            .create(example_request(&base.join("workspace")))
            .unwrap();
        let token = "ef".repeat(32);
        let app = router(token.clone(), store.clone());
        let path = |section: &str| format!("{WORKSPACES_PATH}/{}/{section}", summary.id);
        let get = |section: &str| {
            Request::builder()
                .uri(path(section))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap()
        };
        let put = |section: &str, body: Vec<u8>| {
            Request::builder()
                .method("PUT")
                .uri(path(section))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
                .body(Body::from(body))
                .unwrap()
        };
        let campaign = app.clone().oneshot(get("campaign")).await.unwrap();
        assert_eq!(campaign.status(), StatusCode::OK);
        assert!(
            WorkspaceCampaign::decode(to_bytes(campaign.into_body(), 4096).await.unwrap())
                .unwrap()
                .enabled_feature_ids
                .is_empty()
        );
        let foundation = app.clone().oneshot(get("foundation")).await.unwrap();
        let foundation =
            WorkspaceFoundation::decode(to_bytes(foundation.into_body(), 25_000).await.unwrap())
                .unwrap();
        assert_eq!(foundation.revision, 1);
        let changed = UpdateWorkspaceFoundationRequest {
            expected_revision: 1,
            name: "Renamed Example Workspace".to_owned(),
            guidance: "Use only example.test.".to_owned(),
        };
        let saved = app
            .clone()
            .oneshot(put("foundation", changed.encode_to_vec()))
            .await
            .unwrap();
        assert_eq!(saved.status(), StatusCode::OK);
        let saved = WorkspaceFoundation::decode(to_bytes(saved.into_body(), 25_000).await.unwrap())
            .unwrap();
        assert_eq!(saved.revision, 2);
        assert_eq!(saved.summary.unwrap().name, "Renamed Example Workspace");
        assert_eq!(
            fs::read_to_string(base.join("workspace/AGENTS.md")).unwrap(),
            "Use only example.test.\n"
        );
        assert_eq!(store.list().unwrap()[0].name, "Renamed Example Workspace");
        let stale = UpdateWorkspaceFoundationRequest {
            expected_revision: 1,
            name: "Stale Example".to_owned(),
            guidance: String::new(),
        };
        assert_eq!(
            app.clone()
                .oneshot(put("foundation", stale.encode_to_vec()))
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
        let surface_update = UpdateWorkspaceSurfacesRequest {
            expected_revision: 1,
            surfaces: vec![
                beale_contracts::v1::WorkspaceSurfaceInput {
                    kind: SurfaceKind::Domain as i32,
                    scope: SurfaceScope::InScope as i32,
                    value: "example.test".to_owned(),
                },
                beale_contracts::v1::WorkspaceSurfaceInput {
                    kind: SurfaceKind::Documentation as i32,
                    scope: SurfaceScope::OutOfScope as i32,
                    value: "excluded.example.test".to_owned(),
                },
            ],
        };
        let saved = app
            .clone()
            .oneshot(put("surfaces", surface_update.encode_to_vec()))
            .await
            .unwrap();
        assert_eq!(saved.status(), StatusCode::OK);
        let saved =
            WorkspaceSurfaces::decode(to_bytes(saved.into_body(), 65_536).await.unwrap()).unwrap();
        assert_eq!(saved.revision, 2);
        assert_eq!(saved.surfaces[1].created_by, SurfaceCreator::User as i32);
        assert_eq!(saved.surfaces[1].out_of_scope_reason, "Set in workspace");
        let rule_update = UpdateWorkspaceRulesRequest {
            expected_revision: 1,
            rules: vec!["Stay within scope.".to_owned()],
        };
        let saved = app
            .clone()
            .oneshot(put("rules", rule_update.encode_to_vec()))
            .await
            .unwrap();
        assert_eq!(saved.status(), StatusCode::OK);
        let saved =
            WorkspaceRules::decode(to_bytes(saved.into_body(), 65_536).await.unwrap()).unwrap();
        assert_eq!(saved.revision, 2);
        assert_eq!(saved.rules, ["Stay within scope."]);
        assert_eq!(store.foundation(&summary.id).unwrap().revision, 2);
        assert_eq!(store.surfaces(&summary.id).unwrap().revision, 2);
        assert_eq!(store.rules(&summary.id).unwrap().revision, 2);
        drop(app);
        drop(store);
        fs::remove_dir_all(base).unwrap();
    }

    #[tokio::test]
    async fn invalid_scope_and_nonempty_directory_do_not_create_workspaces() {
        let base = std::env::temp_dir().join(format!(
            "beale-workspace-invalid-example-{}",
            Uuid::new_v4()
        ));
        let directory = base.join("workspace");
        fs::create_dir_all(&directory).unwrap();
        let token = "cd".repeat(32);
        let app = router(
            token.clone(),
            WorkspaceStore::at(base.join("host")).unwrap(),
        );
        let mut input = example_request(&directory);
        input.surfaces[0].scope = SurfaceScope::Unspecified as i32;
        assert_eq!(
            app.clone()
                .oneshot(post(Some(&token), input.encode_to_vec()))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST,
        );
        let mut all_excluded = example_request(&directory);
        all_excluded.surfaces[0].scope = SurfaceScope::OutOfScope as i32;
        assert_eq!(
            app.clone()
                .oneshot(post(Some(&token), all_excluded.encode_to_vec()))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST,
        );
        assert!(fs::read_dir(&directory).unwrap().next().is_none());
        let host_directory = base.join("host");
        assert_eq!(
            app.clone()
                .oneshot(post(
                    Some(&token),
                    example_request(&host_directory).encode_to_vec()
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST,
        );
        assert!(
            WorkspaceStore::at(host_directory)
                .unwrap()
                .list()
                .unwrap()
                .is_empty()
        );
        fs::write(directory.join("existing.txt"), b"existing material").unwrap();
        assert_eq!(
            app.oneshot(post(
                Some(&token),
                example_request(&directory).encode_to_vec()
            ))
            .await
            .unwrap()
            .status(),
            StatusCode::CONFLICT,
        );
        assert_eq!(
            fs::read(directory.join("existing.txt")).unwrap(),
            b"existing material"
        );
        assert!(!directory.join("workspace.json").exists());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn database_registry_ignores_old_file_and_survives_restart() {
        let root = std::env::temp_dir().join(format!(
            "beale-workspace-migration-example-{}",
            Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let id = Uuid::new_v4().to_string();
        fs::write(root.join("workspaces.json"), b"stale registry file").unwrap();
        let store = WorkspaceStore::at(root.clone()).unwrap();
        assert!(store.list().unwrap().is_empty());
        let saved = Registry {
            workspaces: vec![RegistryEntry {
                id: id.clone(),
                name: "Example Workspace".to_owned(),
                directory: "example-directory".to_owned(),
            }],
        };
        store.write_registry(&saved).unwrap();
        assert_eq!(store.list().unwrap()[0].id, id);
        drop(store);
        let reopened = WorkspaceStore::at(root.clone()).unwrap();
        assert_eq!(reopened.list().unwrap()[0].name, "Example Workspace");
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }
}
