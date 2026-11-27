//! Path-free storage and simulated-transfer contracts. Paths below are snapshot-relative.
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;
pub const SNAPSHOT_CHUNK_BYTES: usize = 128 * 1024;
pub const SNAPSHOT_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;

/// Bounded reads of immutable initial snapshot objects, scoped by worker identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotRead {
    pub snapshot: String,
    /// None reads the manifest; Some reads a blob referenced by that manifest.
    pub blob: Option<String>,
    pub offset: u64,
}
impl SnapshotRead {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !digest(&self.snapshot)
            || self.blob.as_ref().is_some_and(|b| !digest(b))
            || self.offset > i64::MAX as u64
        {
            return Err("invalid snapshot read");
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotChunk {
    pub request: SnapshotRead,
    pub total: u64,
    pub bytes: Vec<u8>,
    pub sha256: String,
}
impl SnapshotChunk {
    pub fn validate_for(&self, request: &SnapshotRead) -> Result<(), &'static str> {
        request.validate()?;
        if self.request != *request
            || !digest(&self.sha256)
            || self.total > i64::MAX as u64
            || request.offset > self.total
            || request.blob.is_none() && self.total > SNAPSHOT_MANIFEST_BYTES
            || self.bytes.len() as u64
                != (self.total - request.offset).min(SNAPSHOT_CHUNK_BYTES as u64)
        {
            return Err("invalid snapshot chunk");
        }
        Ok(())
    }
}

/// Worker-supplied byte metadata. Ownership and assignment provenance are
/// derived by the leader from the authenticated execution, never supplied here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactUpload {
    pub id: String,
    pub source_path: String,
    pub sha256: String,
    pub size: u64,
    pub executable: Option<bool>,
    pub revision: u64,
    pub previous: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u32,
    pub entries: Vec<Entry>,
    pub repositories: Vec<Repository>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub path: String,
    pub kind: EntryKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EntryKind {
    Directory {
        mode: Option<u32>,
    },
    File {
        size: u64,
        sha256: String,
        executable: Option<bool>,
        mode: Option<u32>,
    },
    Symlink {
        target: String,
        directory: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Repository {
    pub path: String,
    pub bundle: String,
    pub size: u64,
    pub head: String,
    pub branch: Option<String>,
    pub executable_files: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub id: String,
    pub operation: String,
    pub workspace: String,
    pub scan: String,
    pub producer: String,
    /// Coordinators can publish managerial artifacts without a child assignment.
    pub assignment: Option<String>,
    pub attempt: String,
    pub generation: u64,
    pub revision: u64,
    pub source_path: String,
    pub sha256: String,
    pub size: u64,
    pub executable: Option<bool>,
    pub previous: Option<String>,
}

impl Artifact {
    pub fn validate(&self) -> Result<(), &'static str> {
        use crate::swarms::valid_id;
        if [
            &self.id,
            &self.operation,
            &self.workspace,
            &self.scan,
            &self.producer,
            &self.attempt,
        ]
        .iter()
        .any(|id| !valid_id(id))
            || self.assignment.as_ref().is_some_and(|id| !valid_id(id))
            || self.previous.as_ref().is_some_and(|id| !valid_id(id))
            || self.generation == 0
            || self.revision == 0
            || self.size > i64::MAX as u64
            || !relative_path(&self.source_path)
            || !digest(&self.sha256)
        {
            return Err("invalid artifact metadata");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Environment {
    pub version: u32,
    pub scan: String,
    pub agent: String,
    pub snapshot: String,
    pub checkpoint: Option<String>,
    pub retained_components: Vec<String>,
    pub pending_publications: Vec<String>,
    pub compatibility: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterializeArtifact {
    pub artifact: String,
    pub destination: String,
}

/// Portable boundary used before touching a destination. Native adapters may be stricter.
pub fn relative_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4096
        && !path.contains(['\\', ':', '\0'])
        && path
            .split('/')
            .all(|p| !p.is_empty() && p != "." && p != ".." && !p.chars().any(char::is_control))
}

pub fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
