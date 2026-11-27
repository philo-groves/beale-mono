//! Host-owned immutable inputs, artifact bytes and retained local workspaces.
//! These paths prevent harness aliasing; they are not an OS isolation boundary.
use crate::HostDatabase;
use anyhow::{Context, Result, bail, ensure};
use beale_contracts::swarm_files::*;
use rusqlite::{OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;

mod recovery;
mod snapshot;
mod transfer;
pub use recovery::{CapturedPublication, CapturedPublications, captured_worker_publications};
pub use snapshot::{Destination, SnapshotInput};
pub use transfer::SnapshotReceiver;

const SCHEMA: &str = "
CREATE TABLE swarm_file_snapshots(scan TEXT PRIMARY KEY, digest TEXT NOT NULL, document TEXT NOT NULL);
CREATE TABLE swarm_file_environments(scan TEXT NOT NULL, agent TEXT NOT NULL, location TEXT NOT NULL UNIQUE, document TEXT NOT NULL, PRIMARY KEY(scan,agent));
CREATE TABLE swarm_file_transfers(id TEXT PRIMARY KEY, document TEXT NOT NULL, offset INTEGER NOT NULL, accepted INTEGER NOT NULL);
CREATE TABLE swarm_file_checkpoints(operation TEXT PRIMARY KEY, document TEXT NOT NULL);
";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultPoint {
    CaptureVerify,
    ChunkWrite,
    ChunkCommit,
    ArtifactSeal,
    ArtifactCommit,
    EnvironmentCommit,
    CheckpointWrite,
    CheckpointCommit,
}

/// Test/adapter seam for storage exhaustion and interruption, never a model option.
pub trait StorageFaults: Send + Sync {
    fn check(&self, point: FaultPoint) -> Result<()>;
}
struct NoFaults;
impl StorageFaults for NoFaults {
    fn check(&self, _: FaultPoint) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct ResearchFiles {
    db: HostDatabase,
    host_root: PathBuf,
    root: PathBuf,
    faults: Arc<dyn StorageFaults>,
}

pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Worker-local immutable publication spool. This does not open host storage.
pub fn stage_worker_artifact(
    workspace: &Path,
    recovery: &Path,
    operation: &str,
    source_path: &str,
) -> Result<(ArtifactUpload, PathBuf)> {
    identity(operation)?;
    let key = hash(operation.as_bytes());
    let root = recovery.join("publications");
    fs::create_dir_all(&root)?;
    ensure!(
        fs::symlink_metadata(&root)?.file_type().is_dir(),
        "invalid publication spool"
    );
    let destination = root.join(&key);
    let load = || -> Result<(ArtifactUpload, PathBuf)> {
        ensure!(
            fs::symlink_metadata(&destination)?.file_type().is_dir(),
            "invalid publication directory"
        );
        let metadata = destination.join("metadata.json");
        ensure!(
            fs::symlink_metadata(&metadata)?.file_type().is_file()
                && fs::metadata(&metadata)?.len() <= 65_536,
            "invalid publication metadata"
        );
        let artifact: ArtifactUpload = serde_json::from_slice(&fs::read(metadata)?)?;
        let source = snapshot::safe_file(&destination, "content")?;
        ensure!(
            artifact.id == format!("artifact-{key}")
                && artifact.source_path == source_path
                && file_hash(&source)? == (artifact.sha256.clone(), artifact.size),
            "retained publication integrity mismatch"
        );
        Ok((artifact, source))
    };
    if destination.exists() {
        return load();
    }
    let source = snapshot::safe_file(workspace, source_path)?;
    let metadata = fs::metadata(&source)?;
    let staging = root.join(format!("staging-{}", Uuid::new_v4()));
    fs::create_dir(&staging)?;
    let content = staging.join("content");
    let mut input = fs::File::open(&source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&content)?;
    std::io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    drop(output);
    drop(input);
    let (sha256, size) = file_hash(&content)?;
    ensure!(
        metadata.len() == size
            && metadata.modified()? == fs::metadata(&source)?.modified()?
            && file_hash(&source)? == (sha256.clone(), size),
        "artifact changed during capture"
    );
    let artifact = ArtifactUpload {
        id: format!("artifact-{key}"),
        source_path: source_path.into(),
        sha256,
        size,
        executable: snapshot::publication_executable(workspace, &source, &metadata)?,
        revision: 1,
        previous: None,
    };
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(staging.join("metadata.json"))?;
    file.write_all(&serde_json::to_vec(&artifact)?)?;
    file.sync_all()?;
    drop(file);
    sync_directory(&staging)?;
    match fs::rename(&staging, &destination) {
        Ok(()) => (),
        Err(error) if destination.exists() => {
            let _ = error;
        }
        Err(error) => return Err(error.into()),
    }
    sync_directory(&root)?;
    load()
}

/// Explicit immutable handoff into a worker workspace; never overwrites a path.
pub fn materialize_worker_artifact(
    workspace: &Path,
    destination: &str,
    source: &Path,
    artifact: &Artifact,
) -> Result<()> {
    ensure!(
        !cfg!(windows) || artifact.executable != Some(true),
        "destination cannot preserve executable metadata"
    );
    ensure!(
        file_hash(source)? == (artifact.sha256.clone(), artifact.size),
        "downloaded artifact integrity mismatch"
    );
    let target = snapshot::safe_destination(workspace, destination)?;
    ensure!(
        !target.try_exists()?,
        "destination exists; use a separate version or patch path"
    );
    let parent = target.parent().context("destination parent missing")?;
    fs::create_dir_all(parent)?;
    let staging = parent.join(format!(".artifact-{}", Uuid::new_v4()));
    let mut input = fs::File::open(source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)?;
    std::io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    drop(output);
    drop(input);
    ensure!(
        file_hash(&staging)? == (artifact.sha256.clone(), artifact.size),
        "artifact changed during materialization"
    );
    snapshot::set_mode(&staging, artifact.executable, None)?;
    fs::hard_link(&staging, &target)?;
    fs::remove_file(staging)?;
    sync_directory(parent)
}

/// Install a captured plugin resource without overwriting workspace content.
/// Exact existing bytes and metadata permit recovery of an interrupted download.
pub fn materialize_plugin_resource(
    workspace: &Path,
    destination: &str,
    file: &beale_contracts::swarm_plugins::ResourceFile,
    bytes: &[u8],
) -> Result<()> {
    ensure!(
        beale_contracts::swarm_plugins::resource_path(&file.path)
            && bytes.len() as u64 == file.size
            && file.size <= beale_contracts::swarm_plugins::MAX_RESOURCE_BYTES
            && hash(bytes) == file.sha256,
        "invalid plugin resource content"
    );
    ensure!(
        !cfg!(windows) || !file.executable,
        "destination cannot preserve executable plugin metadata"
    );
    let target = snapshot::safe_destination(workspace, destination)?;
    if target.try_exists()? {
        let path = snapshot::safe_file(workspace, destination)?;
        ensure!(
            file_hash(&path)? == (file.sha256.clone(), file.size),
            "plugin destination contains different content"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            ensure!(
                (fs::metadata(path)?.permissions().mode() & 0o111 != 0) == file.executable,
                "plugin executable metadata changed"
            );
        }
        return Ok(());
    }
    let parent = target
        .parent()
        .context("plugin destination parent missing")?;
    fs::create_dir_all(parent)?;
    let staging = parent.join(format!(".plugin-resource-{}", Uuid::new_v4()));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)?;
    output.write_all(bytes)?;
    output.sync_all()?;
    drop(output);
    snapshot::set_mode(&staging, Some(file.executable), None)?;
    fs::hard_link(&staging, &target)?;
    fs::remove_file(staging)?;
    sync_directory(parent)
}

/// Retain full tool output before recording a bounded journal outcome.
pub fn retain_worker_output(workspace: &Path, operation: &str, output: &str) -> Result<String> {
    identity(operation)?;
    let relative = format!("artifacts/tool-output-{}.txt", hash(operation.as_bytes()));
    let target = snapshot::safe_destination(workspace, &relative)?;
    if target.try_exists()? {
        let saved = snapshot::safe_file(workspace, &relative)?;
        ensure!(
            file_hash(&saved)? == (hash(output.as_bytes()), output.len() as u64),
            "retained tool output changed"
        );
        return Ok(relative);
    }
    let parent = target.parent().context("output parent missing")?;
    fs::create_dir_all(parent)?;
    let staging = parent.join(format!(".tool-output-{}", Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)?;
    file.write_all(output.as_bytes())?;
    file.sync_all()?;
    drop(file);
    fs::hard_link(&staging, &target)?;
    fs::remove_file(staging)?;
    sync_directory(parent)?;
    Ok(relative)
}
fn file_hash(path: &Path) -> Result<(String, u64)> {
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut size = 0u64;
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
        size = size
            .checked_add(count as u64)
            .context("file size overflow")?;
    }
    Ok((format!("{:x}", digest.finalize()), size))
}
fn encode<T: Serialize>(value: &T) -> Result<String> {
    Ok(serde_json::to_string(value)?)
}
fn decode<T: DeserializeOwned>(value: String) -> Result<T> {
    Ok(serde_json::from_str(&value)?)
}
fn identity(value: &str) -> Result<()> {
    ensure!(
        beale_contracts::swarms::valid_id(value),
        "invalid storage identity"
    );
    Ok(())
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    std::fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Canonical-store guard, called inside the finding/handoff transaction. Managed
/// artifact references keep their byte/ownership requirements across session types;
/// ordinary non-managed evidence does not initialize Swarm storage.
pub fn validate_record_artifacts(
    db: &rusqlite::Connection,
    scope: &str,
    workspace: &str,
    record: &serde_json::Value,
) -> Result<()> {
    use beale_contracts::tool_error::ToolError;
    let installed: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='swarm_file_snapshots')",
        [],
        |r| r.get(0),
    )?;
    let scoped = installed && db.query_row("SELECT EXISTS(SELECT 1 FROM swarm_file_snapshots WHERE scan=?1) OR EXISTS(SELECT 1 FROM swarm_file_environments WHERE agent=?1)",[scope],|r|r.get::<_,bool>(0))?;
    fn refs<'a>(value: &'a serde_json::Value, output: &mut Vec<&'a str>) -> Result<()> {
        match value {
            serde_json::Value::Object(object) => {
                if object.get("kind").and_then(|v| v.as_str()) == Some("artifact") {
                    output.push(
                        object
                            .get("reference")
                            .and_then(|v| v.as_str())
                            .context(ToolError::invalid("artifact reference missing"))?,
                    );
                }
                for (key, value) in object {
                    if matches!(
                        key.as_str(),
                        "artifact_refs" | "poc_artifact_refs" | "patch_refs" | "test_refs"
                    ) {
                        for item in value
                            .as_array()
                            .context(ToolError::invalid("invalid artifact references"))?
                        {
                            output.push(
                                item.as_str()
                                    .context(ToolError::invalid("invalid artifact reference"))?,
                            );
                        }
                    } else {
                        refs(value, output)?;
                    }
                }
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    refs(value, output)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    let mut references = Vec::new();
    refs(record, &mut references)?;
    references.sort_unstable();
    references.dedup();
    let mut managed = Vec::new();
    for reference in references {
        let reserved = reference.strip_prefix("artifact-").is_some_and(digest);
        let known = installed
            && db.query_row(
                "SELECT EXISTS(SELECT 1 FROM swarm_file_transfers WHERE id=?1)",
                [reference],
                |r| r.get::<_, bool>(0),
            )?;
        if scoped || reserved || known {
            managed.push(reference);
        }
    }
    let references = managed;
    if references.is_empty() {
        return Ok(());
    }
    ensure!(
        installed,
        ToolError::rejected("artifact has not been accepted")
    );
    let database: String = db.query_row(
        "SELECT file FROM pragma_database_list WHERE name='main'",
        [],
        |r| r.get(0),
    )?;
    let root = Path::new(&database)
        .parent()
        .context("host database location unavailable")?
        .join("swarm-files/blobs");
    for reference in references {
        identity(reference).map_err(|_| ToolError::invalid("invalid artifact reference"))?;
        let artifact: Artifact = decode(
            db.query_row(
                "SELECT document FROM swarm_file_transfers WHERE id=?1 AND accepted=1",
                [reference],
                |r| r.get(0),
            )
            .optional()?
            .context(ToolError::rejected("artifact has not been accepted"))?,
        )?;
        ensure!(
            artifact.workspace == workspace,
            ToolError::rejected("artifact belongs to another workspace")
        );
        ensure!(
            digest(&artifact.sha256),
            "accepted artifact digest is invalid"
        );
        ensure!(
            file_hash(&root.join(&artifact.sha256))
                .context("accepted artifact bytes unavailable")?
                == (artifact.sha256, artifact.size),
            "accepted artifact integrity failure"
        );
    }
    Ok(())
}

impl ResearchFiles {
    fn retained_path(&self, location: &str) -> Result<PathBuf> {
        Uuid::parse_str(location).context("invalid retained environment location")?;
        let parent = self.root.join("environments");
        let path = parent.join(location);
        for component in [&parent, &path] {
            let metadata = fs::symlink_metadata(component)
                .context("retained environment is missing; reconstruction is unavailable")?;
            ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "retained environment must be a private directory, not a link"
            );
        }
        ensure!(
            path.canonicalize()? == path,
            "retained environment location changed"
        );
        Ok(path)
    }

    fn verify_checkpoint(&self, scan: &str, agent: &str, digest: &str) -> Result<()> {
        let size = fs::metadata(self.blob(digest)?)?.len();
        ensure!(
            size <= beale_contracts::swarms::MAX_DOCUMENT_BYTES as u64,
            "checkpoint is too large"
        );
        let context: serde_json::Value = serde_json::from_slice(&self.read_blob(digest, size)?)?;
        ensure!(
            context["scan"] == scan
                && context["agent"] == agent
                && context.get("context").is_some(),
            "checkpoint ownership mismatch"
        );
        Ok(())
    }

    pub fn open(db: HostDatabase, host_root: &Path) -> Result<Self> {
        db.migrate_schema("swarm-files", &[SCHEMA])?;
        let root = host_root.join("swarm-files");
        for child in ["blobs", "transfers", "environments", "staging"] {
            fs::create_dir_all(root.join(child))?;
        }
        Ok(Self {
            db,
            host_root: host_root.canonicalize()?,
            root: root.canonicalize()?,
            faults: Arc::new(NoFaults),
        })
    }

    pub fn with_faults(mut self, faults: Arc<dyn StorageFaults>) -> Self {
        self.faults = faults;
        self
    }

    fn blob(&self, digest: &str) -> Result<PathBuf> {
        ensure!(crate::swarm_files::digest(digest), "invalid content digest");
        Ok(self.root.join("blobs").join(digest))
    }

    pub fn read_blob(&self, digest: &str, size: u64) -> Result<Vec<u8>> {
        let bytes = fs::read(self.blob(digest)?)?;
        ensure!(
            bytes.len() as u64 == size && hash(&bytes) == digest,
            "content integrity failure"
        );
        Ok(bytes)
    }

    fn verify_blob(&self, digest: &str, size: u64) -> Result<PathBuf> {
        let path = self.blob(digest)?;
        ensure!(
            file_hash(&path)? == (digest.to_owned(), size),
            "content integrity failure"
        );
        Ok(path)
    }

    fn put_file_blob(&self, source: &Path) -> Result<(String, u64)> {
        let temporary = self.root.join("staging").join(Uuid::new_v4().to_string());
        let mut input = fs::File::open(source)?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        std::io::copy(&mut input, &mut output)?;
        output.sync_all()?;
        drop(output);
        let (digest, size) = file_hash(&temporary)?;
        let target = self.blob(&digest)?;
        if target.exists() {
            self.verify_blob(&digest, size)?;
            fs::remove_file(temporary)?;
        } else {
            fs::rename(temporary, target)?;
            sync_directory(&self.root.join("blobs"))?;
        }
        Ok((digest, size))
    }

    fn copy_blob(&self, digest: &str, size: u64, path: &Path) -> Result<()> {
        let mut source = fs::File::open(self.verify_blob(digest, size)?)?;
        let mut target = OpenOptions::new().write(true).create_new(true).open(path)?;
        std::io::copy(&mut source, &mut target)?;
        target.sync_all()?;
        drop(target);
        ensure!(
            file_hash(path)? == (digest.to_owned(), size),
            "materialized content integrity failure"
        );
        Ok(())
    }

    pub(crate) fn put_blob(&self, bytes: &[u8]) -> Result<String> {
        let digest = hash(bytes);
        let target = self.blob(&digest)?;
        if target.exists() {
            self.read_blob(&digest, bytes.len() as u64)?;
            return Ok(digest);
        }
        let temporary = self.root.join("staging").join(Uuid::new_v4().to_string());
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        // All callers serialize publication through the host database connection.
        fs::rename(&temporary, &target)?;
        sync_directory(&self.root.join("blobs"))?;
        Ok(digest)
    }

    pub fn snapshot(&self, scan: &str) -> Result<(String, Manifest)> {
        identity(scan)?;
        self.db.with_connection(|db| {
            let (digest, json): (String, String) = db.query_row(
                "SELECT digest,document FROM swarm_file_snapshots WHERE scan=?1",
                [scan],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            ensure!(
                hash(json.as_bytes()) == digest,
                "snapshot manifest integrity failure"
            );
            let manifest = decode(json)?;
            self.verify(&manifest)?;
            Ok((digest, manifest))
        })
    }

    /// Capture twice and verify metadata and bytes. This is bounded verification,
    /// not an atomic filesystem snapshot. A mutating source must be quiesced.
    pub fn capture(&self, scan: &str, inputs: &[SnapshotInput]) -> Result<String> {
        identity(scan)?;
        self.db.with_connection(|db| {
            if let Some(existing) = db
                .query_row(
                    "SELECT digest FROM swarm_file_snapshots WHERE scan=?1",
                    [scan],
                    |r| r.get(0),
                )
                .optional()?
            {
                return Ok(existing);
            }
            let mut last = None;
            for _ in 0..3 {
                let result = (|| {
                    let first = self.capture_inputs(inputs)?;
                    self.faults.check(FaultPoint::CaptureVerify)?;
                    let second = self.capture_inputs(inputs)?;
                    ensure!(first == second, "source changed during capture");
                    self.verify(&first)?;
                    let json = encode(&first)?;
                    let digest = hash(json.as_bytes());
                    db.execute(
                        "INSERT INTO swarm_file_snapshots VALUES(?1,?2,?3)",
                        params![scan, digest, json],
                    )?;
                    Ok(digest)
                })();
                match result {
                    Ok(id) => return Ok(id),
                    Err(error) => last = Some(error),
                }
            }
            bail!(
                "snapshot rejected; correct unsupported input or quiesce source: {}",
                last.context("capture did not run")?
            );
        })
    }

    pub fn verify(&self, manifest: &Manifest) -> Result<()> {
        snapshot::validate(manifest, &Destination::portable())?;
        for entry in &manifest.entries {
            if let EntryKind::File { size, sha256, .. } = &entry.kind {
                self.verify_blob(sha256, *size)?;
            }
        }
        for repository in &manifest.repositories {
            self.verify_blob(&repository.bundle, repository.size)?;
        }
        Ok(())
    }

    /// Used by simulated transfers now; no worker transport is introduced here.
    /// Every referenced blob must already have passed the transfer integrity gate.
    pub fn import_snapshot(&self, scan: &str, manifest: &Manifest) -> Result<String> {
        identity(scan)?;
        self.verify(manifest)?;
        let json = encode(manifest)?;
        let digest = hash(json.as_bytes());
        self.db.with_connection(|db| {
            if let Some(saved) = db
                .query_row(
                    "SELECT digest FROM swarm_file_snapshots WHERE scan=?1",
                    [scan],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
            {
                ensure!(saved == digest, "scan snapshot is immutable");
            } else {
                db.execute(
                    "INSERT INTO swarm_file_snapshots VALUES(?1,?2,?3)",
                    params![scan, digest, json],
                )?;
            }
            Ok(digest)
        })
    }

    /// Materializes in a new directory only. No overwrite or merging with existing edits.
    pub fn materialize_snapshot(&self, manifest: &Manifest, destination: &Path) -> Result<()> {
        self.verify(manifest)?;
        snapshot::validate(manifest, &Destination::native())?;
        ensure!(!destination.try_exists()?, "destination already exists");
        let parent = destination.parent().context("destination has no parent")?;
        fs::create_dir_all(parent)?;
        let staging = parent.join(format!(".snapshot-{}", Uuid::new_v4()));
        fs::create_dir(&staging)?;
        // Failed staging directories remain unacknowledged and can be reclaimed.
        self.write_manifest(manifest, &staging)?;
        fs::rename(staging, destination)?;
        sync_directory(parent)?;
        Ok(())
    }

    pub fn prepare_agent(&self, scan: &str, agent: &str) -> Result<PathBuf> {
        identity(agent)?;
        let (snapshot, manifest) = self.snapshot(scan)?;
        self.db.with_connection(|db| {
            let existing: Option<String> = db
                .query_row(
                    "SELECT location FROM swarm_file_environments WHERE scan=?1 AND agent=?2",
                    params![scan, agent],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(location) = existing {
                let environment: Environment = decode(db.query_row(
                    "SELECT document FROM swarm_file_environments WHERE scan=?1 AND agent=?2",
                    params![scan, agent],
                    |r| r.get(0),
                )?)?;
                ensure!(
                    environment.version == VERSION
                        && environment.scan == scan
                        && environment.agent == agent
                        && environment.snapshot == snapshot
                        && environment.compatibility
                            == format!("local-directory-v1-{}", std::env::consts::OS),
                    "retained environment is incompatible"
                );
                let path = self.retained_path(&location)?;
                if let Some(checkpoint) = &environment.checkpoint {
                    self.verify_checkpoint(scan, agent, checkpoint)?;
                }
                return Ok(path);
            }
            let location = Uuid::new_v4().to_string();
            let path = self.root.join("environments").join(&location);
            self.materialize_snapshot(&manifest, &path)?;
            let environment = Environment {
                version: VERSION,
                scan: scan.into(),
                agent: agent.into(),
                snapshot,
                checkpoint: None,
                retained_components: vec!["workspace".into()],
                pending_publications: vec![],
                compatibility: format!("local-directory-v1-{}", std::env::consts::OS),
            };
            self.faults.check(FaultPoint::EnvironmentCommit)?;
            db.execute(
                "INSERT INTO swarm_file_environments VALUES(?1,?2,?3,?4)",
                params![scan, agent, location, encode(&environment)?],
            )?;
            Ok(path)
        })
    }

    pub fn environment(&self, scan: &str, agent: &str) -> Result<Environment> {
        self.db.with_connection(|db| {
            decode(db.query_row(
                "SELECT document FROM swarm_file_environments WHERE scan=?1 AND agent=?2",
                params![scan, agent],
                |r| r.get(0),
            )?)
        })
    }
    /// Called only after an owning-host retention claim. Keep the metadata
    /// tombstone so a later direct resume fails instead of creating a clean copy.
    pub fn delete_private_environment(&self, scan: &str, agent: &str) -> Result<()> {
        let location: Option<String> = self.db.with_connection(|db| {
            Ok(db
                .query_row(
                    "SELECT location FROM swarm_file_environments WHERE scan=?1 AND agent=?2",
                    params![scan, agent],
                    |r| r.get(0),
                )
                .optional()?)
        })?;
        let Some(location) = location else {
            return Ok(());
        };
        Uuid::parse_str(&location)?;
        let path = self.root.join("environments").join(&location);
        if path.try_exists()? {
            let verified = self.retained_path(&location)?;
            fs::remove_dir_all(verified)?;
        }
        Ok(())
    }

    pub fn checkpoint(
        &self,
        scan: &str,
        agent: &str,
        operation: &str,
        context: &serde_json::Value,
    ) -> Result<String> {
        identity(operation)?;
        let document = encode(&serde_json::json!({"scan":scan,"agent":agent,"context":context}))?;
        ensure!(
            document.len() <= beale_contracts::swarms::MAX_DOCUMENT_BYTES,
            "checkpoint is too large"
        );
        self.db.with_connection(|db| {
            let tx = db.transaction()?;
            if let Some(saved) = tx
                .query_row(
                    "SELECT document FROM swarm_file_checkpoints WHERE operation=?1",
                    [operation],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
            {
                ensure!(
                    saved == document,
                    "checkpoint operation reused with changed content"
                );
                let digest = hash(document.as_bytes());
                self.verify_checkpoint(scan, agent, &digest)?;
                return Ok(digest);
            }
            let mut environment: Environment = decode(tx.query_row(
                "SELECT document FROM swarm_file_environments WHERE scan=?1 AND agent=?2",
                params![scan, agent],
                |r| r.get(0),
            )?)?;
            self.faults.check(FaultPoint::CheckpointWrite)?;
            let location: String = tx.query_row(
                "SELECT location FROM swarm_file_environments WHERE scan=?1 AND agent=?2",
                params![scan, agent],
                |r| r.get(0),
            )?;
            snapshot::flush(&self.retained_path(&location)?)?;
            let digest = self.put_blob(document.as_bytes())?;
            environment.checkpoint = Some(digest.clone());
            self.faults.check(FaultPoint::CheckpointCommit)?;
            tx.execute(
                "INSERT INTO swarm_file_checkpoints VALUES(?1,?2)",
                params![operation, document],
            )?;
            tx.execute(
                "UPDATE swarm_file_environments SET document=?3 WHERE scan=?1 AND agent=?2",
                params![scan, agent, encode(&environment)?],
            )?;
            tx.commit()?;
            Ok(digest)
        })
    }

    pub fn begin_transfer(&self, artifact: &Artifact) -> Result<u64> {
        self.db.with_connection(|db| {
            let tx = db.transaction()?;
            let offset = self.begin_transfer_in(&tx, artifact)?;
            tx.commit()?;
            Ok(offset)
        })
    }

    pub(crate) fn begin_transfer_in(
        &self,
        tx: &rusqlite::Transaction<'_>,
        artifact: &Artifact,
    ) -> Result<u64> {
        for id in [
            &artifact.id,
            &artifact.operation,
            &artifact.workspace,
            &artifact.scan,
            &artifact.producer,
            &artifact.attempt,
        ] {
            identity(id)?;
        }
        if let Some(assignment) = &artifact.assignment {
            identity(assignment)?;
        }
        ensure!(
            artifact.revision > 0
                && artifact.generation > 0
                && relative_path(&artifact.source_path)
                && digest(&artifact.sha256),
            "invalid artifact metadata"
        );
        ensure!(
            !snapshot::excluded_path(&artifact.source_path),
            "excluded artifact path"
        );
        ensure!(artifact.size <= i64::MAX as u64, "artifact too large");
        let json = encode(artifact)?;
        if let Some((saved, offset)) = tx
            .query_row(
                "SELECT document,offset FROM swarm_file_transfers WHERE id=?1",
                [&artifact.id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64)),
            )
            .optional()?
        {
            ensure!(
                saved == json,
                "transfer identity reused with changed content"
            );
            return Ok(offset);
        }
        let reused:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM swarm_file_transfers WHERE json_extract(document,'$.operation')=?1 AND json_extract(document,'$.workspace')=?2 AND json_extract(document,'$.producer')=?3)",params![artifact.operation,artifact.workspace,artifact.producer],|r|r.get(0))?;
        ensure!(!reused, "publication operation identity reused");
        if let Some(previous) = &artifact.previous {
            let saved: Artifact = decode(tx.query_row(
                "SELECT document FROM swarm_file_transfers WHERE id=?1 AND accepted=1",
                [previous],
                |r| r.get(0),
            )?)?;
            ensure!(
                saved.workspace == artifact.workspace
                    && Some(artifact.revision) == saved.revision.checked_add(1),
                "invalid artifact revision"
            );
        } else {
            ensure!(
                artifact.revision == 1,
                "initial artifact revision must be one"
            );
        }
        tx.execute(
            "INSERT INTO swarm_file_transfers VALUES(?1,?2,0,0)",
            params![artifact.id, json],
        )?;
        if let Some(raw) = tx
            .query_row(
                "SELECT document FROM swarm_file_environments WHERE scan=?1 AND agent=?2",
                params![artifact.scan, artifact.producer],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        {
            let mut env: Environment = decode(raw)?;
            env.pending_publications.push(artifact.id.clone());
            tx.execute(
                "UPDATE swarm_file_environments SET document=?3 WHERE scan=?1 AND agent=?2",
                params![artifact.scan, artifact.producer, encode(&env)?],
            )?;
        }
        Ok(0)
    }

    pub fn write_chunk(&self, id: &str, offset: u64, bytes: &[u8]) -> Result<u64> {
        self.db.with_connection(|db| {
            let tx = db.transaction()?;
            let next = self.write_chunk_in(&tx, id, offset, bytes)?;
            tx.commit()?;
            Ok(next)
        })
    }

    pub(crate) fn write_chunk_in(
        &self,
        db: &rusqlite::Transaction<'_>,
        id: &str,
        offset: u64,
        bytes: &[u8],
    ) -> Result<u64> {
        identity(id)?;
        ensure!(
            !bytes.is_empty() && bytes.len() <= 1024 * 1024,
            "chunk size must be 1..1048576"
        );
        let (json, received, accepted): (String, u64, bool) = db.query_row(
            "SELECT document,offset,accepted FROM swarm_file_transfers WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64, r.get(2)?)),
        )?;
        let artifact: Artifact = decode(json)?;
        let end = offset
            .checked_add(bytes.len() as u64)
            .context("chunk overflow")?;
        ensure!(
            end <= artifact.size && offset <= received,
            "invalid chunk offset"
        );
        let path = if accepted {
            self.blob(&artifact.sha256)?
        } else {
            self.root.join("transfers").join(hash(id.as_bytes()))
        };
        let mut file = OpenOptions::new()
            .read(true)
            .write(!accepted)
            .create(!accepted)
            .truncate(false)
            .open(path)?;
        if offset < received || accepted {
            ensure!(end <= received, "overlapping chunk");
            let mut saved = vec![0; bytes.len()];
            file.seek(SeekFrom::Start(offset))?;
            file.read_exact(&mut saved)?;
            ensure!(saved == bytes, "changed retry chunk");
            return Ok(received);
        }
        ensure!(
            file.metadata()?.len() >= received,
            "acknowledged transfer bytes missing"
        );
        file.set_len(received)?;
        file.seek(SeekFrom::Start(received))?;
        self.faults.check(FaultPoint::ChunkWrite)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        self.faults.check(FaultPoint::ChunkCommit)?;
        db.execute(
            "UPDATE swarm_file_transfers SET offset=?2 WHERE id=?1",
            params![id, end as i64],
        )?;
        Ok(end)
    }

    pub fn seal(&self, id: &str) -> Result<Artifact> {
        let artifact = self.db.with_connection(|db| {
            let tx = db.transaction()?;
            let artifact = self.seal_in(&tx, id)?;
            tx.commit()?;
            Ok(artifact)
        })?;
        self.cleanup_transfer(id);
        Ok(artifact)
    }

    /// Call only after the transaction accepting these bytes has committed.
    pub(crate) fn cleanup_transfer(&self, id: &str) {
        let _ = fs::remove_file(self.root.join("transfers").join(hash(id.as_bytes())));
    }

    pub(crate) fn seal_in(&self, tx: &rusqlite::Transaction<'_>, id: &str) -> Result<Artifact> {
        identity(id)?;
        let (json, offset, accepted): (String, u64, bool) = tx.query_row(
            "SELECT document,offset,accepted FROM swarm_file_transfers WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64, r.get(2)?)),
        )?;
        let artifact: Artifact = decode(json)?;
        if accepted {
            self.verify_blob(&artifact.sha256, artifact.size)?;
            return Ok(artifact);
        }
        ensure!(offset == artifact.size, "transfer incomplete");
        let path = self.root.join("transfers").join(hash(id.as_bytes()));
        if artifact.size == 0 && !path.exists() {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?
                .sync_all()?;
        }
        ensure!(
            file_hash(&path)? == (artifact.sha256.clone(), artifact.size),
            "transfer integrity failure"
        );
        self.faults.check(FaultPoint::ArtifactSeal)?;
        let copied = self.put_file_blob(&path)?;
        ensure!(
            copied == (artifact.sha256.clone(), artifact.size),
            "transfer changed during seal"
        );
        self.faults.check(FaultPoint::ArtifactCommit)?;
        tx.execute(
            "UPDATE swarm_file_transfers SET accepted=1 WHERE id=?1",
            [id],
        )?;
        if let Some(raw) = tx
            .query_row(
                "SELECT document FROM swarm_file_environments WHERE scan=?1 AND agent=?2",
                params![artifact.scan, artifact.producer],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        {
            let mut env: Environment = decode(raw)?;
            env.pending_publications.retain(|id| id != &artifact.id);
            tx.execute(
                "UPDATE swarm_file_environments SET document=?3 WHERE scan=?1 AND agent=?2",
                params![artifact.scan, artifact.producer, encode(&env)?],
            )?;
        }
        Ok(artifact)
    }

    pub fn artifact(&self, workspace: &str, id: &str) -> Result<Artifact> {
        identity(id)?;
        let artifact: Artifact = self.db.with_connection(|db| {
            decode(db.query_row(
                "SELECT document FROM swarm_file_transfers WHERE id=?1 AND accepted=1",
                [id],
                |r| r.get(0),
            )?)
        })?;
        ensure!(
            artifact.workspace == workspace,
            "artifact belongs to another workspace"
        );
        self.verify_blob(&artifact.sha256, artifact.size)?;
        Ok(artifact)
    }

    pub(crate) fn transfer_in(&self, db: &rusqlite::Connection, id: &str) -> Result<Artifact> {
        identity(id)?;
        decode(db.query_row(
            "SELECT document FROM swarm_file_transfers WHERE id=?1",
            [id],
            |r| r.get(0),
        )?)
    }

    pub(crate) fn artifact_chunk_in(
        &self,
        db: &rusqlite::Connection,
        workspace: &str,
        id: &str,
        offset: u64,
        length: u32,
    ) -> Result<beale_contracts::swarm_worker::ArtifactChunkResult> {
        ensure!(
            (1..=192 * 1024).contains(&length),
            "invalid artifact chunk length"
        );
        let artifact: Artifact = decode(db.query_row(
            "SELECT document FROM swarm_file_transfers WHERE id=?1 AND accepted=1",
            [id],
            |r| r.get(0),
        )?)?;
        ensure!(
            artifact.workspace == workspace && offset <= artifact.size,
            "artifact not available in this workspace"
        );
        let mut file = fs::File::open(self.verify_blob(&artifact.sha256, artifact.size)?)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0; usize::try_from((artifact.size - offset).min(u64::from(length)))?];
        file.read_exact(&mut bytes)?;
        Ok(beale_contracts::swarm_worker::ArtifactChunkResult {
            artifact,
            offset,
            sha256: hash(&bytes),
            bytes,
        })
    }

    pub fn publish_file(&self, workspace: &Path, mut artifact: Artifact) -> Result<Artifact> {
        let source = snapshot::safe_file(workspace, &artifact.source_path)?;
        let before = fs::metadata(&source)?;
        let (digest, size) = file_hash(&source)?;
        ensure!(
            before.len() == size && before.modified()? == fs::metadata(&source)?.modified()?,
            "attachment changed during publication"
        );
        artifact.sha256 = digest;
        artifact.size = size;
        artifact.executable = snapshot::publication_executable(workspace, &source, &before)?;
        let mut offset = self.begin_transfer(&artifact)?;
        let mut input = fs::File::open(&source)?;
        input.seek(SeekFrom::Start(offset))?;
        let mut buffer = vec![0; 1024 * 1024];
        loop {
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            offset = self.write_chunk(&artifact.id, offset, &buffer[..count])?;
        }
        self.seal(&artifact.id)
    }

    pub fn materialize_artifact(
        &self,
        workspace: &str,
        id: &str,
        recipient: &Path,
        destination: &str,
    ) -> Result<()> {
        let artifact = self.artifact(workspace, id)?;
        ensure!(
            !cfg!(windows) || artifact.executable != Some(true),
            "destination cannot preserve required executable metadata"
        );
        let target = snapshot::safe_destination(recipient, destination)?;
        ensure!(
            !target.try_exists()?,
            "destination exists; use a separate version or patch path"
        );
        let parent = target.parent().context("destination parent missing")?;
        fs::create_dir_all(parent)?;
        let staging = parent.join(format!(".artifact-{}", Uuid::new_v4()));
        self.copy_blob(&artifact.sha256, artifact.size, &staging)?;
        snapshot::set_mode(&staging, artifact.executable, None)?;
        // hard_link is atomic create-if-absent, then drop only the staging name.
        fs::hard_link(&staging, &target)?;
        fs::remove_file(staging)?;
        sync_directory(parent)?;
        Ok(())
    }

    /// Local `file:relative/path` attachments become central IDs before the caller
    /// commits a finding or handoff. Failure leaves bytes pending or accepted but
    /// unreferenced; it never returns a partially rewritten acknowledged record.
    pub fn resolve_attachments(
        &self,
        workspace: &Path,
        operation: &str,
        provenance: &Artifact,
        arguments: &mut serde_json::Value,
    ) -> Result<()> {
        fn visit(
            files: &ResearchFiles,
            workspace: &Path,
            operation: &str,
            base: &Artifact,
            value: &mut serde_json::Value,
            pointer: &str,
        ) -> Result<()> {
            match value {
                serde_json::Value::Object(object) => {
                    if object.get("kind").and_then(|v| v.as_str()) == Some("artifact") {
                        let reference = object
                            .get_mut("reference")
                            .context("artifact reference missing")?;
                        resolve(
                            files,
                            workspace,
                            operation,
                            base,
                            reference,
                            &format!("{pointer}/reference"),
                        )?;
                    }
                    for (key, value) in object.iter_mut() {
                        let path = format!("{pointer}/{key}");
                        if matches!(
                            key.as_str(),
                            "artifact_refs" | "poc_artifact_refs" | "patch_refs" | "test_refs"
                        ) {
                            let values = value
                                .as_array_mut()
                                .context("artifact refs must be an array")?;
                            for (index, value) in values.iter_mut().enumerate() {
                                resolve(
                                    files,
                                    workspace,
                                    operation,
                                    base,
                                    value,
                                    &format!("{path}/{index}"),
                                )?;
                            }
                        } else {
                            visit(files, workspace, operation, base, value, &path)?;
                        }
                    }
                }
                serde_json::Value::Array(values) => {
                    for (index, value) in values.iter_mut().enumerate() {
                        visit(
                            files,
                            workspace,
                            operation,
                            base,
                            value,
                            &format!("{pointer}/{index}"),
                        )?;
                    }
                }
                _ => {}
            }
            Ok(())
        }
        fn resolve(
            files: &ResearchFiles,
            workspace: &Path,
            operation: &str,
            base: &Artifact,
            value: &mut serde_json::Value,
            pointer: &str,
        ) -> Result<()> {
            let reference = value
                .as_str()
                .context("artifact reference must be a string")?;
            if let Some(path) = reference.strip_prefix("file:") {
                let mut artifact = base.clone();
                artifact.operation = hash(
                    format!("{}:{}:{operation}:{pointer}", base.scan, base.producer).as_bytes(),
                );
                artifact.id = format!("artifact-{}", artifact.operation);
                artifact.source_path = path.into();
                let artifact = files.publish_file(workspace, artifact)?;
                *value = artifact.id.into();
            } else {
                files.artifact(&base.workspace, reference)?;
            }
            Ok(())
        }
        let mut staged = arguments.clone();
        visit(self, workspace, operation, provenance, &mut staged, "")?;
        *arguments = staged;
        Ok(())
    }
}
