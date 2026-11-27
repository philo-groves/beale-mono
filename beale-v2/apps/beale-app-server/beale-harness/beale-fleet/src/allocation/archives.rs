//! Immutable, host-authenticated environment handoffs. Guest tar paths are never
//! extracted on the owning host; workspace/root restoration is delegated to Docker.
use super::*;
use anyhow::bail;
use beale_contracts::swarm_host::ExecutionTarget;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
pub const CHUNK: usize = 192 * 1024;
pub fn bundle_id(bundle: &EnvironmentBundle) -> Result<String> {
    Ok(beale_storage::swarm_files::hash(&serde_json::to_vec(&(
        &bundle.target,
        bundle.image_digest.is_some(),
    ))?))
}
pub fn bundle_digest(bundle: &EnvironmentBundle) -> Result<String> {
    Ok(beale_storage::swarm_files::hash(&serde_json::to_vec(
        bundle,
    )?))
}
pub fn file_digest(path: &Path) -> Result<(String, u64)> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut size = 0u64;
    let mut bytes = [0; 65536];
    loop {
        let n = file.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
        size = size
            .checked_add(n as u64)
            .context("archive size overflow")?;
    }
    Ok((format!("{:x}", hash.finalize()), size))
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.try_exists()? {
        ensure!(
            fs::read(path)? == bytes,
            "immutable checkpoint identity changed"
        );
        return Ok(());
    }
    let temporary = path.with_extension("part");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(temporary, path)?;
    #[cfg(unix)]
    File::open(path.parent().context("checkpoint parent missing")?)?.sync_all()?;
    Ok(())
}
impl Allocator {
    pub(super) fn delete_archives(&self, instance: &str) -> Result<()> {
        let id: uuid::Uuid = instance.parse()?;
        let restore = self
            .environments
            .root()
            .join("allocation-restores")
            .join(id.to_string());
        if restore.try_exists()? {
            ensure!(
                fs::symlink_metadata(&restore)?.file_type().is_dir(),
                "restore directory changed"
            );
            fs::remove_dir_all(restore)?;
        }
        let root = self.environments.root().join("allocation-archives");
        for id in self.store.archive_ids(instance)? {
            ensure!(
                beale_contracts::swarm_files::digest(&id),
                "invalid owned archive identity"
            );
            let path = root.join(id);
            if path.try_exists()? {
                ensure!(
                    fs::symlink_metadata(&path)?.file_type().is_dir(),
                    "owned archive directory changed"
                );
                fs::remove_dir_all(path)?;
            }
        }
        Ok(())
    }
    fn archive_root(&self, id: &str) -> Result<PathBuf> {
        ensure!(
            beale_contracts::swarm_files::digest(id),
            "invalid checkpoint identity"
        );
        let root = self
            .environments
            .root()
            .join("allocation-archives")
            .join(id);
        fs::create_dir_all(&root)?;
        ensure!(
            fs::symlink_metadata(&root)?.is_dir(),
            "checkpoint directory changed"
        );
        Ok(root)
    }
    fn read_bundle(&self, id: &str) -> Result<EnvironmentBundle> {
        let bundle: EnvironmentBundle =
            serde_json::from_slice(&fs::read(self.archive_root(id)?.join("manifest.json"))?)?;
        bundle.validate().map_err(anyhow::Error::msg)?;
        ensure!(bundle_id(&bundle)? == id, "checkpoint identity mismatch");
        Ok(bundle)
    }
    pub async fn capture(&self, target: &ExecutionTarget, full: bool) -> Result<EnvironmentBundle> {
        self.capture_inner(target, full, &target.identity.attempt_id)
            .await
    }
    pub async fn capture_export(
        &self,
        target: &ExecutionTarget,
        transfer: &str,
    ) -> Result<EnvironmentBundle> {
        self.capture_inner(target, true, transfer).await
    }
    async fn capture_inner(
        &self,
        target: &ExecutionTarget,
        full: bool,
        transfer: &str,
    ) -> Result<EnvironmentBundle> {
        ensure!(
            beale_contracts::swarms::valid_id(transfer),
            "invalid transfer identity"
        );
        target.validate().map_err(anyhow::Error::msg)?;
        let _owner = self.environments.lock(target.instance_id.parse()?)?;
        let instance = self.environments.store.validate_target(target)?;
        ensure!(
            instance.target.as_ref() == Some(target) && instance.stopped,
            "checkpoint requires the exact stopped execution"
        );
        let handle = instance.handle.context("retained container missing")?;
        ensure!(
            self.environments.docker.inspect(&handle).await? == ContainerStatus::Stopped,
            "checkpoint execution active"
        );
        let mut environment = self.store.environment(&target.instance_id)?;
        if full {
            ensure!(
                !self.store.export_canceled(target, transfer)?,
                "export was canceled"
            );
            ensure!(
                environment.checkpoint_confirmed
                    && environment.pending_outputs.is_empty()
                    && environment
                        .references
                        .iter()
                        .all(|r| r == &format!("restore-{transfer}")),
                "restore requires an accepted checkpoint"
            );
            environment = self.store.retention(
                &target.instance_id,
                environment.generation,
                RetentionAction::Protect {
                    reference: format!("restore-{transfer}"),
                },
                Self::now(),
            )?;
        } else if !environment
            .pending_outputs
            .contains(&format!("checkpoint-{}", target.identity.attempt_id))
            && !environment.checkpoint_confirmed
        {
            environment = self.store.retention(
                &target.instance_id,
                environment.generation,
                RetentionAction::Pending {
                    operation: format!("checkpoint-{}", target.identity.attempt_id),
                },
                Self::now(),
            )?;
        }
        ensure!(
            environment.deletion == DeletionStep::Retained,
            "environment deletion won"
        );
        let mut bundle = EnvironmentBundle {
            version: 1,
            target: target.clone(),
            configuration: instance.configuration,
            image_digest: full.then(|| String::from("pending")),
            archives: vec![],
        };
        let id = bundle_id(&bundle)?;
        // Ownership is durable before any archive bytes, including partial
        // exports that never reach manifest publication.
        self.store.bind_archive(&id, &target.instance_id)?;
        let root = self.archive_root(&id)?;
        if root.join("manifest.json").try_exists()? {
            return self.read_bundle(&id);
        }
        for kind in [ArchiveKind::Workspace, ArchiveKind::State] {
            let final_path = root.join(format!("{}.tar", kind.name()));
            let path = root.join(format!("{}.part", kind.name()));
            self.environments
                .docker
                .checkpoint_archive(&handle, &path, kind == ArchiveKind::State)
                .await?;
            let (sha256, bytes) = file_digest(&path)?;
            fs::rename(&path, &final_path)?;
            bundle.archives.push(EnvironmentArchive {
                kind,
                sha256,
                bytes,
            });
        }
        if full {
            let path = root.join("image.part");
            let tag = format!(
                "beale-retained:{}-{}",
                target.instance_id, target.identity.generation
            );
            bundle.image_digest = Some(
                self.environments
                    .docker
                    .save_root(&handle, &tag, &path)
                    .await?,
            );
            let (sha256, bytes) = file_digest(&path)?;
            fs::rename(&path, root.join("image.tar"))?;
            bundle.archives.push(EnvironmentArchive {
                kind: ArchiveKind::Image,
                sha256,
                bytes,
            });
        }
        bundle.validate().map_err(anyhow::Error::msg)?;
        write_new(&root.join("manifest.json"), &serde_json::to_vec(&bundle)?)?;
        Ok(bundle)
    }
    pub fn read_archive(&self, id: &str, kind: ArchiveKind, offset: u64) -> Result<Vec<u8>> {
        let bundle = self.read_bundle(id)?;
        let environment = self.store.environment(&bundle.target.instance_id)?;
        ensure!(
            environment.deletion == DeletionStep::Retained,
            "checkpoint is being deleted"
        );
        let entry = bundle
            .archives
            .iter()
            .find(|a| a.kind == kind)
            .context("checkpoint component missing")?;
        ensure!(offset <= entry.bytes, "archive offset invalid");
        let mut file = File::open(self.archive_root(id)?.join(format!("{}.tar", kind.name())))?;
        ensure!(
            file.metadata()?.len() == entry.bytes,
            "checkpoint archive changed"
        );
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0; usize::try_from((entry.bytes - offset).min(CHUNK as u64))?];
        file.read_exact(&mut bytes)?;
        Ok(bytes)
    }
    pub async fn accept_checkpoint(&self, target: &ExecutionTarget, digest: &str) -> Result<()> {
        // Completed acknowledgements are immutable receipts. Replaying one
        // must not create pending output on a reused instance.
        let allocations = self.store.allocations()?;
        if allocations.iter().any(|a| {
            a.execution.as_ref() == Some(target)
                && a.phase == AllocationPhase::Retained
                && self.store.environment(&target.instance_id).is_ok_and(|e| {
                    !e.references.contains(&a.request.id)
                        && !e
                            .pending_outputs
                            .contains(&format!("checkpoint-{}", target.identity.attempt_id))
                        && (e.checkpoint_confirmed
                            || allocations.iter().any(|other| {
                                other.request.instance == target.instance_id
                                    && other.sequence > a.sequence
                            }))
                })
        }) {
            let id = beale_storage::swarm_files::hash(&serde_json::to_vec(&(target, false))?);
            let bundle = self.read_bundle(&id)?;
            ensure!(
                bundle.target == *target && bundle_digest(&bundle)? == digest,
                "checkpoint acknowledgement mismatch"
            );
            return Ok(());
        }
        let bundle = self.capture(target, false).await?;
        ensure!(
            bundle_digest(&bundle)? == digest,
            "checkpoint acknowledgement mismatch"
        );
        let _owner = self.environments.lock(target.instance_id.parse()?)?;
        let mut environment = self.store.environment(&target.instance_id)?;
        let operation = format!("checkpoint-{}", target.identity.attempt_id);
        if environment.pending_outputs.contains(&operation) {
            environment = self.store.retention(
                &target.instance_id,
                environment.generation,
                RetentionAction::Acknowledge { operation },
                Self::now(),
            )?;
        }
        ensure!(
            environment.pending_outputs.is_empty(),
            "other publications remain pending"
        );
        drop(_owner);
        self.checkpoint(target).await
    }
    pub fn cancel_export(&self, target: &ExecutionTarget, transfer: &str) -> Result<()> {
        ensure!(
            beale_contracts::swarms::valid_id(transfer),
            "invalid transfer identity"
        );
        let _owner = self.environments.lock(target.instance_id.parse()?)?;
        let environment = self.store.environment(&target.instance_id)?;
        ensure!(
            target.host_id == self.environments.store.host()
                && target.leader_id == environment.leader
                && target.identity.scan_id == environment.scan
                && target.identity.agent_id == environment.agent,
            "export cancellation identity mismatch"
        );
        // The tombstone precedes release, so delayed Export requests cannot
        // reacquire a pin after leader cleanup has completed.
        self.store.cancel_export(target, transfer)?;
        let reference = format!("restore-{transfer}");
        if environment.references.contains(&reference) {
            self.store.retention(
                &target.instance_id,
                environment.generation,
                RetentionAction::Release { reference },
                Self::now(),
            )?;
        }
        Ok(())
    }
    fn restore_root(&self, allocation: &str) -> Result<PathBuf> {
        let a = self.store.allocation(allocation)?;
        ensure!(
            a.phase == AllocationPhase::Reserved,
            "restore allocation is not reserved"
        );
        let root = self
            .environments
            .root()
            .join("allocation-restores")
            .join(a.request.instance);
        fs::create_dir_all(&root)?;
        Ok(root)
    }
    pub fn begin_restore(&self, allocation: &str, bundle: &EnvironmentBundle) -> Result<()> {
        bundle.validate().map_err(anyhow::Error::msg)?;
        ensure!(
            self.store.restore_bundle(allocation)?.as_ref() == Some(bundle),
            "restore differs from reserved intent"
        );
        let a = self.store.allocation(allocation)?;
        let _owner = self.environments.lock(a.request.instance.parse()?)?;
        self.store.renew_transfer(allocation, Self::now())?;
        ensure!(
            bundle.target.leader_id == a.request.leader
                && bundle.target.identity.scan_id == a.request.scan
                && bundle.target.identity.agent_id == a.request.agent,
            "restore identity mismatch"
        );
        write_new(
            &self.restore_root(allocation)?.join("manifest.json"),
            &serde_json::to_vec(bundle)?,
        )
    }
    pub fn write_archive(
        &self,
        allocation: &str,
        kind: ArchiveKind,
        offset: u64,
        bytes: &[u8],
    ) -> Result<u64> {
        ensure!(
            !bytes.is_empty() && bytes.len() <= CHUNK,
            "invalid restore chunk"
        );
        let a = self.store.allocation(allocation)?;
        let _owner = self.environments.lock(a.request.instance.parse()?)?;
        let bundle = self
            .store
            .restore_bundle(allocation)?
            .context("restore intent missing")?;
        let entry = bundle
            .archives
            .iter()
            .find(|a| a.kind == kind)
            .context("restore component missing")?;
        let next = offset
            .checked_add(bytes.len() as u64)
            .context("restore offset overflow")?;
        ensure!(next <= entry.bytes, "restore exceeds reserved content");
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(
                self.restore_root(allocation)?
                    .join(format!("{}.tar", kind.name())),
            )?;
        let length = file.metadata()?.len();
        ensure!(offset <= length, "restore chunk gap");
        file.seek(SeekFrom::Start(offset))?;
        if next <= length {
            let mut previous = vec![0; bytes.len()];
            file.read_exact(&mut previous)?;
            ensure!(previous == bytes, "restore retry changed bytes");
            return Ok(next);
        }
        // A short interrupted write is overwritten under the same identity.
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(next)
    }
    pub async fn finish_restore(&self, allocation: &str) -> Result<()> {
        let row = self.store.allocation(allocation)?;
        if row.prepared {
            return Ok(());
        }
        let _owner = self.environments.lock(row.request.instance.parse()?)?;
        let bundle = self
            .store
            .restore_bundle(allocation)?
            .context("restore intent missing")?;
        let root = self.restore_root(allocation)?;
        for entry in &bundle.archives {
            ensure!(
                file_digest(&root.join(format!("{}.tar", entry.kind.name())))?
                    == (entry.sha256.clone(), entry.bytes),
                "restore archive incomplete or corrupted"
            );
        }
        let config = self
            .environments
            .store
            .configuration(&row.request.configuration)?
            .configuration;
        ensure!(
            config.revision == row.request.configuration_revision
                && config.architecture == bundle.configuration.architecture
                && config.options == bundle.configuration.options,
            "selected destination is incompatible with retained tooling/startup"
        );
        let image = self
            .environments
            .docker
            .load_root(
                bundle
                    .image_digest
                    .as_deref()
                    .context("root filesystem is missing; clean substitution is forbidden")?,
                &root.join("image.tar"),
                config.architecture,
            )
            .await?;
        let mut instance = match self
            .environments
            .store
            .instance_if_present(row.request.instance.parse()?)?
        {
            Some(row) => row,
            None => {
                let row = crate::docker::Instance {
                    id: row.request.instance.parse()?,
                    configuration: config,
                    image,
                    handle: None,
                    target: None,
                    stopped: false,
                    prepared: false,
                };
                self.environments.store.insert(&row)?;
                row
            }
        };
        if instance.handle.is_none() {
            // A crash after create is recovered by its exact labels, never by a
            // clean second instance. A new creation uses the reserved identity.
            let handle = match self
                .environments
                .docker
                .discover(self.environments.store.host(), instance.id, &instance.image)
                .await
            {
                Ok(handle) => handle,
                Err(_) => self.environments.prepare_restored(&instance).await?,
            };
            instance.handle = Some(handle);
            self.environments.store.save_instance(&instance)?;
        }
        let handle = instance
            .handle
            .as_ref()
            .context("restore container missing")?;
        ensure!(
            self.environments.docker.stop(handle).await?,
            "restore termination unconfirmed"
        );
        for kind in [ArchiveKind::Workspace, ArchiveKind::State] {
            self.environments
                .docker
                .restore_archive(
                    handle,
                    &root.join(format!("{}.tar", kind.name())),
                    kind == ArchiveKind::State,
                )
                .await?;
        }
        // Fresh preflight still runs through the normal backend before research.
        instance.prepared = true;
        instance.stopped = true;
        self.environments.store.save_instance(&instance)?;
        self.store.prepared(allocation)
    }
}

/// Extract only regular recovery files and directories. Reject links, devices,
/// traversal and duplicate entries; no host tool executes retained guest data.
pub fn extract_recovery(archive: &Path, root: &Path, attempt: &str) -> Result<()> {
    ensure!(
        beale_contracts::swarms::valid_id(attempt),
        "invalid recovery attempt"
    );
    let prefix = Path::new("state").join(attempt);
    fs::create_dir_all(root)?;
    let mut seen = std::collections::BTreeSet::new();
    for entry in tar::Archive::new(File::open(archive)?).entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        ensure!(
            path.components().all(|c| matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )),
            "unsafe recovery archive path"
        );
        let relative = path.strip_prefix(".").unwrap_or(&path);
        if !relative.starts_with(&prefix) {
            continue;
        }
        ensure!(seen.insert(path.clone()), "duplicate recovery archive path");
        let target = root.join(&path);
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            fs::create_dir_all(&target)?;
        } else if kind.is_file() {
            fs::create_dir_all(target.parent().context("recovery path has no parent")?)?;
            let mut out = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&target)?;
            std::io::copy(&mut entry, &mut out)?;
            out.sync_all()?;
        } else {
            bail!("unsupported recovery archive entry");
        }
    }
    Ok(())
}
