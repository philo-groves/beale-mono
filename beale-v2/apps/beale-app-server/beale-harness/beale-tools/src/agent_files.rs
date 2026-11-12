use anyhow::{Context, Result, ensure};
use std::{
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
};

/// A per-turn snapshot of model-readable files, separate from private host state.
/// Kept after the turn because persisted provider context can still reference it.
pub struct AgentFiles {
    root: PathBuf,
}

impl AgentFiles {
    pub fn new() -> Result<Self> {
        let root = std::env::temp_dir().join(format!("beale-agent-{}", uuid::Uuid::new_v4()));
        #[allow(unused_mut)]
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&root)
            .context("create agent temporary directory")?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn write(&self, relative: &str, bytes: &[u8]) -> Result<PathBuf> {
        let relative = Path::new(relative);
        ensure!(
            !relative.as_os_str().is_empty()
                && relative
                    .components()
                    .all(|c| matches!(c, Component::Normal(_))),
            "invalid agent file path"
        );
        let path = relative
            .components()
            .fold(self.root.clone(), |mut path, part| {
                path.push(part.as_os_str());
                path
            });
        let parent = path.parent().context("agent file has no parent")?;
        fs::create_dir_all(parent).context("create agent file directory")?;
        ensure!(
            parent
                .canonicalize()?
                .starts_with(self.root.canonicalize()?),
            "agent file escapes temporary directory"
        );
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path).context("create agent file")?;
        file.write_all(bytes).context("write agent file")?;
        file.sync_all().context("sync agent file")?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_unique_and_cannot_replace_or_escape() {
        let files = AgentFiles::new().unwrap();
        let other = AgentFiles::new().unwrap();
        assert_ne!(files.root(), other.root());
        let path = files
            .write("features/example/FEATURE.md", b"Example guidance")
            .unwrap();
        assert_eq!(fs::read(path).unwrap(), b"Example guidance");
        assert!(
            files
                .write("features/example/FEATURE.md", b"overwrite")
                .is_err()
        );
        assert!(files.write("../escape", b"escape").is_err());
        assert!(files.write("/escape", b"escape").is_err());
        fs::remove_dir_all(files.root()).unwrap();
        fs::remove_dir_all(other.root()).unwrap();
    }
}
