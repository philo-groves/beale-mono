//! Initial workspace transfer. The receiver uses files only, never HostDatabase.
use super::*;
use beale_contracts::swarm_worker::ExecutionIdentity;

impl ResearchFiles {
    pub fn snapshot_chunk(&self, scan: &str, request: &SnapshotRead) -> Result<SnapshotChunk> {
        request.validate().map_err(anyhow::Error::msg)?;
        let (digest, manifest) = self.snapshot(scan)?;
        ensure!(digest == request.snapshot, "snapshot identity mismatch");
        let (total, bytes) = if let Some(blob) = &request.blob {
            let size = manifest
                .entries
                .iter()
                .find_map(|e| match &e.kind {
                    EntryKind::File { sha256, size, .. } if sha256 == blob => Some(*size),
                    _ => None,
                })
                .or_else(|| {
                    manifest
                        .repositories
                        .iter()
                        .find(|r| &r.bundle == blob)
                        .map(|r| r.size)
                })
                .context("blob is not in the initial snapshot")?;
            let path = self.blob(blob)?;
            ensure!(
                fs::symlink_metadata(&path)?.file_type().is_file(),
                "invalid snapshot blob"
            );
            let mut file = fs::File::open(path)?;
            ensure!(
                file.metadata()?.len() == size && request.offset <= size,
                "invalid snapshot blob size or offset"
            );
            file.seek(SeekFrom::Start(request.offset))?;
            let mut bytes =
                vec![0; (size - request.offset).min(SNAPSHOT_CHUNK_BYTES as u64) as usize];
            file.read_exact(&mut bytes)?;
            (size, bytes)
        } else {
            let bytes = serde_json::to_vec(&manifest)?;
            let total = bytes.len() as u64;
            ensure!(
                total <= SNAPSHOT_MANIFEST_BYTES && request.offset <= total,
                "snapshot manifest or offset exceeds transfer bound"
            );
            let offset = request.offset as usize;
            (
                total,
                bytes[offset..bytes.len().min(offset + SNAPSHOT_CHUNK_BYTES)].to_vec(),
            )
        };
        let chunk = SnapshotChunk {
            request: request.clone(),
            total,
            sha256: hash(&bytes),
            bytes,
        };
        chunk.validate_for(request).map_err(anyhow::Error::msg)?;
        Ok(chunk)
    }
}

/// One receiver owns its private staging directory across download/materialize.
/// Keep the receiver alive in any blocking filesystem task after async cancel.
pub struct SnapshotReceiver {
    root: PathBuf,
    _owner: ReceiverOwner,
}
struct ReceiverOwner(fs::File);
impl Drop for ReceiverOwner {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}
#[derive(Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Installation {
    snapshot: String,
    identity: ExecutionIdentity,
    destination: PathBuf,
    marker: String,
    staging: String,
}

impl SnapshotReceiver {
    pub fn open(root: &Path) -> Result<Self> {
        fs::create_dir_all(root)?;
        ensure!(
            fs::symlink_metadata(root)?.file_type().is_dir(),
            "invalid snapshot cache"
        );
        let lock = root.join("owner.lock");
        if lock.try_exists()? {
            ensure!(
                fs::symlink_metadata(&lock)?.file_type().is_file(),
                "invalid snapshot cache owner"
            );
        }
        let owner = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock)?;
        owner
            .try_lock()
            .context("snapshot receiver already owned")?;
        Ok(Self {
            root: root.canonicalize()?,
            _owner: ReceiverOwner(owner),
        })
    }
    fn object(&self, digest: &str, part: bool) -> Result<PathBuf> {
        ensure!(super::digest(digest), "invalid snapshot object digest");
        let path = self.root.join(if part {
            format!("{digest}.part")
        } else {
            digest.to_owned()
        });
        if path.try_exists()? {
            ensure!(
                fs::symlink_metadata(&path)?.file_type().is_file(),
                "invalid snapshot object"
            );
        }
        Ok(path)
    }
    pub fn offset(&self, digest: &str) -> Result<u64> {
        let part = self.object(digest, true)?;
        Ok(if part.try_exists()? {
            fs::metadata(part)?.len()
        } else {
            0
        })
    }
    pub fn complete(&self, digest: &str, expected: Option<u64>) -> Result<bool> {
        let path = self.object(digest, false)?;
        if !path.try_exists()? {
            return Ok(false);
        }
        let (actual, size) = file_hash(&path)?;
        ensure!(
            actual == digest && expected.is_none_or(|e| e == size),
            "snapshot object integrity failure"
        );
        Ok(true)
    }
    /// Flush before exposing the next offset. A full object is published only
    /// after its complete digest passes, including bytes from earlier attempts.
    pub fn append(&self, request: &SnapshotRead, chunk: &SnapshotChunk) -> Result<()> {
        chunk.validate_for(request).map_err(anyhow::Error::msg)?;
        ensure!(
            hash(&chunk.bytes) == chunk.sha256,
            "snapshot chunk integrity failure"
        );
        let digest = request.blob.as_ref().unwrap_or(&request.snapshot);
        let path = self.object(digest, true)?;
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(&path)?;
        ensure!(
            file.metadata()?.len() == request.offset,
            "snapshot chunk offset mismatch"
        );
        file.write_all(&chunk.bytes)?;
        file.sync_all()?;
        let length = file.metadata()?.len();
        drop(file);
        if length == chunk.total {
            ensure!(
                file_hash(&path)? == (digest.clone(), chunk.total),
                "snapshot object integrity failure"
            );
            let target = self.object(digest, false)?;
            if target.try_exists()? {
                ensure!(
                    self.complete(digest, Some(chunk.total))?,
                    "missing completed object"
                );
            } else {
                fs::rename(path, target)?;
                sync_directory(&self.root)?;
            }
        }
        Ok(())
    }
    pub fn manifest(&self, digest: &str) -> Result<Manifest> {
        let path = self.object(digest, false)?;
        ensure!(
            fs::metadata(&path)?.len() <= SNAPSHOT_MANIFEST_BYTES,
            "snapshot manifest exceeds transfer bound"
        );
        ensure!(self.complete(digest, None)?, "snapshot manifest incomplete");
        let manifest: Manifest = serde_json::from_slice(&fs::read(path)?)?;
        Destination::native().validate(&manifest)?;
        Ok(manifest)
    }
    pub fn installed(
        &self,
        digest: &str,
        identity: &ExecutionIdentity,
        destination: &Path,
    ) -> Result<bool> {
        let receipt = self.root.join("installation.json");
        if !receipt.try_exists()? {
            ensure!(
                !self.root.join("installed.json").try_exists()?,
                "snapshot installation receipt disappeared"
            );
            return Ok(false);
        }
        ensure!(
            fs::symlink_metadata(&receipt)?.file_type().is_file()
                && fs::metadata(&receipt)?.len() <= 32_768,
            "invalid snapshot installation receipt"
        );
        let saved: Installation = serde_json::from_slice(&fs::read(receipt)?)?;
        ensure!(
            saved.snapshot == digest
                && saved.identity == *identity
                && saved.destination == destination,
            "snapshot installation identity mismatch"
        );
        if !destination.try_exists()? {
            ensure!(
                !self.root.join("installed.json").try_exists()?,
                "retained workspace disappeared"
            );
            let staging = self.staging(&saved)?;
            ensure!(
                fs::symlink_metadata(&staging)?.file_type().is_dir(),
                "snapshot publication lost both destination and staging"
            );
            self.verify_publication(&saved, &staging)?;
            return Ok(false);
        }
        self.verify_publication(&saved, destination)?;
        self.mark_installed(&saved)?;
        Ok(true)
    }
    /// Same logical agent, later execution: validate retained publication
    /// receipts without restoring the initial bytes over private edits.
    pub fn verify_retained(&self, identity: &ExecutionIdentity, destination: &Path) -> Result<()> {
        identity.validate().map_err(anyhow::Error::msg)?;
        let receipt = self.root.join("installed.json");
        ensure!(
            fs::symlink_metadata(&receipt)?.file_type().is_file()
                && fs::metadata(&receipt)?.len() <= 32_768,
            "retained snapshot receipt unavailable"
        );
        let saved: Installation = serde_json::from_slice(&fs::read(receipt)?)?;
        ensure!(
            saved.identity.workspace_id == identity.workspace_id
                && saved.identity.scan_id == identity.scan_id
                && saved.identity.agent_id == identity.agent_id
                && identity.generation >= saved.identity.generation
                && saved.destination == destination,
            "retained workspace belongs to another agent"
        );
        self.verify_publication(&saved, destination)?;
        self.mark_installed(&saved)
    }
    fn verify_publication(&self, saved: &Installation, destination: &Path) -> Result<()> {
        ensure!(
            fs::symlink_metadata(destination)?.file_type().is_dir(),
            "invalid prepared workspace"
        );
        let marker = destination.join(".beale/initial-snapshot");
        ensure!(
            fs::symlink_metadata(destination.join(".beale"))?
                .file_type()
                .is_dir()
                && fs::symlink_metadata(&marker)?.file_type().is_file()
                && fs::metadata(&marker)?.len() <= 128
                && fs::read_to_string(marker)? == saved.marker,
            "workspace publication is unconfirmed"
        );
        Ok(())
    }
    fn staging(&self, saved: &Installation) -> Result<PathBuf> {
        let suffix = saved
            .staging
            .strip_prefix(".snapshot-")
            .context("invalid snapshot staging name")?;
        ensure!(
            Uuid::parse_str(suffix)?.to_string() == suffix,
            "invalid snapshot staging name"
        );
        Ok(saved
            .destination
            .parent()
            .context("missing snapshot parent")?
            .join(&saved.staging))
    }
    fn mark_installed(&self, saved: &Installation) -> Result<()> {
        let completed = self.root.join("installed.json");
        if completed.try_exists()? {
            ensure!(
                fs::symlink_metadata(&completed)?.file_type().is_file()
                    && fs::metadata(&completed)?.len() <= 32_768,
                "invalid completed snapshot receipt"
            );
            ensure!(
                serde_json::from_slice::<Installation>(&fs::read(completed)?)? == *saved,
                "completed snapshot receipt changed"
            );
        } else {
            fs::hard_link(self.root.join("installation.json"), completed)?;
        }
        sync_directory(&self.root)?;
        Ok(())
    }
    /// Never merge into an existing directory. The receipt and private marker
    /// identify a completed rename after a lost acknowledgement; edits survive.
    pub fn install(
        &self,
        digest: &str,
        identity: &ExecutionIdentity,
        destination: &Path,
    ) -> Result<()> {
        identity.validate().map_err(anyhow::Error::msg)?;
        ensure!(
            super::digest(digest) && destination.is_absolute(),
            "invalid snapshot destination"
        );
        if self.installed(digest, identity, destination)? {
            return Ok(());
        }
        ensure!(
            !destination.try_exists()?,
            "workspace already exists without a publication receipt"
        );
        let parent = destination
            .parent()
            .context("snapshot destination parent missing")?;
        ensure!(
            fs::symlink_metadata(parent)?.file_type().is_dir(),
            "invalid snapshot destination parent"
        );
        let receipt = self.root.join("installation.json");
        if receipt.try_exists()? {
            // installed() already checked identity and rejected lost storage.
            let saved: Installation = serde_json::from_slice(&fs::read(&receipt)?)?;
            fs::rename(self.staging(&saved)?, destination)?;
            sync_directory(parent)?;
            return self.mark_installed(&saved);
        }
        let manifest = self.manifest(digest)?;
        let staging_name = format!(".snapshot-{}", Uuid::new_v4());
        let staging = parent.join(&staging_name);
        fs::create_dir(&staging)?;
        snapshot::render_manifest(&manifest, &staging, |digest, size| {
            ensure!(
                self.complete(digest, Some(size))?,
                "snapshot content incomplete"
            );
            self.object(digest, false)
        })?;
        let saved = Installation {
            snapshot: digest.into(),
            identity: identity.clone(),
            destination: destination.into(),
            marker: Uuid::new_v4().to_string(),
            staging: staging_name,
        };
        fs::create_dir(staging.join(".beale"))?;
        let mut marker = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(staging.join(".beale/initial-snapshot"))?;
        marker.write_all(saved.marker.as_bytes())?;
        marker.sync_all()?;
        drop(marker);
        sync_directory(&staging.join(".beale"))?;
        sync_directory(&staging)?;
        let temporary = self.root.join(format!("receipt-{}", Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec(&saved)?)?;
        file.sync_all()?;
        drop(file);
        // Create-only publication; a prior intent is never replaced.
        fs::hard_link(&temporary, &receipt)?;
        fs::remove_file(temporary)?;
        sync_directory(&self.root)?;
        ensure!(
            !destination.try_exists()?,
            "workspace appeared during preparation"
        );
        fs::rename(staging, destination)?;
        sync_directory(parent)?;
        self.mark_installed(&saved)
    }
}
