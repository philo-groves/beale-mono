use anyhow::{Context, Result, ensure};
use beale_contracts::swarm_container::*;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};

/// Verified immutable package inputs. Archive creation rechecks bytes against
/// the manifest so editing a package cannot silently change an admitted runtime.
pub struct RuntimeBundle {
    root: PathBuf,
    manifest: RuntimeManifest,
}
impl RuntimeBundle {
    pub fn open(root: &Path) -> Result<Self> {
        ensure!(
            std::fs::symlink_metadata(root)?.file_type().is_dir(),
            "runtime package is not a directory"
        );
        let path = root.join("manifest.json");
        ensure!(
            std::fs::symlink_metadata(&path)?.file_type().is_file(),
            "invalid runtime manifest file"
        );
        let mut bytes = vec![];
        File::open(path)?
            .take(64 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 64 * 1024, "runtime manifest too large");
        let manifest: RuntimeManifest = serde_json::from_slice(&bytes)?;
        manifest.validate().map_err(anyhow::Error::msg)?;
        let bundle = Self {
            root: root.canonicalize()?,
            manifest,
        };
        bundle.validate_files()?;
        Ok(bundle)
    }
    pub fn manifest(&self) -> &RuntimeManifest {
        &self.manifest
    }
    fn entries(&self) -> [(&str, &RuntimeArtifact, bool); 3] {
        [
            ("beale-worker", &self.manifest.worker, true),
            ("bash", &self.manifest.shell, true),
            ("BASH-COPYRIGHT", &self.manifest.shell_license, false),
        ]
    }
    fn read(&self, name: &str, artifact: &RuntimeArtifact, executable: bool) -> Result<Vec<u8>> {
        let path = self.root.join(name);
        let metadata = std::fs::symlink_metadata(&path)?;
        ensure!(
            metadata.file_type().is_file() && metadata.len() == artifact.bytes,
            "runtime artifact size changed"
        );
        let mut bytes = vec![];
        File::open(path)?
            .take(artifact.bytes + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 == artifact.bytes
                && format!("{:x}", Sha256::digest(&bytes)) == artifact.sha256,
            "runtime artifact digest changed"
        );
        if executable {
            let elf = goblin::elf::Elf::parse(&bytes)
                .context("runtime artifact is not an ELF executable")?;
            ensure!(
                elf.is_64
                    && elf.little_endian
                    && elf.header.e_machine == self.manifest.architecture.elf_machine()
                    && matches!(
                        elf.header.e_type,
                        goblin::elf::header::ET_EXEC | goblin::elf::header::ET_DYN
                    )
                    && elf.interpreter.is_none()
                    && elf.libraries.is_empty(),
                "runtime must be a static executable for the selected architecture"
            );
        }
        Ok(bytes)
    }
    fn validate_files(&self) -> Result<()> {
        for (name, artifact, executable) in self.entries() {
            self.read(name, artifact, executable)?;
        }
        Ok(())
    }
    pub fn write_archive(&self, file: &mut File, instance: uuid::Uuid) -> Result<String> {
        let directory = format!("beale-runtime-{instance}");
        let mut archive = tar::Builder::new(file);
        for (name, artifact, executable) in self.entries() {
            let bytes = self.read(name, artifact, executable)?;
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(if executable { 0o755 } else { 0o644 });
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            header.set_cksum();
            archive.append_data(&mut header, format!("{directory}/{name}"), bytes.as_slice())?;
        }
        archive.finish()?;
        let file = archive.into_inner()?;
        file.flush()?;
        file.sync_all()?;
        Ok(format!("/{directory}"))
    }
}
