//! Read only fully committed, immutable worker publication captures. Partial
//! staging directories remain on disk for diagnosis and are never publications.
use super::*;

pub struct CapturedPublication {
    key: String,
    upload: ArtifactUpload,
    content: PathBuf,
}

impl CapturedPublication {
    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn upload(&self) -> &ArtifactUpload {
        &self.upload
    }
    pub fn open(&self) -> Result<fs::File> {
        ensure!(
            fs::symlink_metadata(&self.content)?.file_type().is_file(),
            "invalid captured content"
        );
        Ok(fs::File::open(&self.content)?)
    }
}

pub struct CapturedPublications {
    entries: Option<fs::ReadDir>,
}

pub fn captured_worker_publications(recovery: &Path) -> Result<CapturedPublications> {
    let directory = |path: &Path| -> Result<bool> {
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                ensure!(
                    metadata.file_type().is_dir(),
                    "invalid publication recovery directory"
                );
                Ok(true)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    };
    let root = recovery.join("publications");
    let entries = if directory(recovery)? && directory(&root)? {
        Some(fs::read_dir(root)?)
    } else {
        None
    };
    Ok(CapturedPublications { entries })
}

impl Iterator for CapturedPublications {
    type Item = Result<CapturedPublication>;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let entry = match self.entries.as_mut()?.next()? {
                Ok(entry) => entry,
                Err(error) => return Some(Err(error.into())),
            };
            let load = || -> Result<Option<CapturedPublication>> {
                ensure!(
                    entry.file_type()?.is_dir(),
                    "invalid publication capture entry"
                );
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("invalid capture name"))?;
                if let Some(staging) = name.strip_prefix("staging-") {
                    ensure!(
                        Uuid::parse_str(staging).is_ok(),
                        "invalid staging capture name"
                    );
                    return Ok(None);
                }
                ensure!(digest(&name), "invalid capture identity");
                let directory = entry.path();
                let metadata = snapshot::safe_file(&directory, "metadata.json")?;
                ensure!(
                    fs::metadata(&metadata)?.len() <= 65_536,
                    "capture metadata too large"
                );
                let upload: ArtifactUpload = serde_json::from_slice(&fs::read(metadata)?)?;
                let content = snapshot::safe_file(&directory, "content")?;
                ensure!(
                    upload.id == format!("artifact-{name}")
                        && relative_path(&upload.source_path)
                        && !snapshot::excluded_path(&upload.source_path)
                        && upload.revision == 1
                        && upload.previous.is_none()
                        && file_hash(&content)? == (upload.sha256.clone(), upload.size),
                    "captured publication integrity mismatch"
                );
                Ok(Some(CapturedPublication {
                    key: name,
                    upload,
                    content,
                }))
            };
            match load() {
                Ok(None) => continue,
                Ok(Some(capture)) => return Some(Ok(capture)),
                Err(error) => return Some(Err(error)),
            }
        }
    }
}
