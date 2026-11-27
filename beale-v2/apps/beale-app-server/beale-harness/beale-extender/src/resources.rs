//! Immutable, portable model resources shared by local publication and workers.
use super::*;
use beale_contracts::swarm_plugins::{
    MAX_BUNDLE_BYTES, MAX_BUNDLE_FILES, MAX_RESOURCE_BYTES, ResourceFile, ResourceManifest,
    resource_path,
};
use beale_storage::swarm_files::hash;
use std::io::Read;

/// Bytes are captured once. Consumers must verify the manifest when receiving a bundle.
pub struct PluginResource {
    pub file: ResourceFile,
    pub bytes: Vec<u8>,
}
pub struct PluginSnapshot {
    pub manifest: ResourceManifest,
    pub resources: Vec<PluginResource>,
}
impl PluginSnapshot {
    pub fn digest(&self) -> Result<String> {
        self.validate()?;
        Ok(hash(&serde_json::to_vec(&self.manifest)?))
    }
    pub fn validate(&self) -> Result<()> {
        self.manifest.validate().map_err(anyhow::Error::msg)?;
        ensure!(
            self.resources.len() == self.manifest.files.len(),
            "plugin resource count changed"
        );
        for (resource, metadata) in self.resources.iter().zip(&self.manifest.files) {
            ensure!(
                resource.file == *metadata
                    && resource.bytes.len() as u64 == metadata.size
                    && hash(&resource.bytes) == metadata.sha256,
                "plugin resource content changed"
            );
        }
        Ok(())
    }
}

fn add(
    resources: &mut Vec<PluginResource>,
    path: String,
    bytes: Vec<u8>,
    executable: bool,
) -> Result<()> {
    ensure!(resource_path(&path), "plugin resource path is not portable");
    ensure!(
        resources.len() < MAX_BUNDLE_FILES && bytes.len() as u64 <= MAX_RESOURCE_BYTES,
        "plugin resources exceed transfer limits"
    );
    let total = resources.iter().map(|r| r.file.size).sum::<u64>() + bytes.len() as u64;
    ensure!(total <= MAX_BUNDLE_BYTES, "plugin bundle is too large");
    resources.push(PluginResource {
        file: ResourceFile {
            path,
            sha256: hash(&bytes),
            size: bytes.len() as u64,
            executable,
        },
        bytes,
    });
    Ok(())
}

/// Check every component, including the selected skill directory. Resolving a
/// link before checking its type would hide a model-resource escape.
fn regular_path(root: &Path, path: &Path, directory: bool) -> Result<std::fs::Metadata> {
    let relative = path.strip_prefix(root)?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        ensure!(
            matches!(component, std::path::Component::Normal(_)),
            "invalid plugin resource path"
        );
        current.push(component);
        ensure!(
            !std::fs::symlink_metadata(&current)?
                .file_type()
                .is_symlink(),
            "plugin resource cannot be a link"
        );
    }
    let metadata = std::fs::symlink_metadata(path)?;
    ensure!(
        if directory {
            metadata.is_dir()
        } else {
            metadata.is_file()
        },
        "invalid plugin resource type"
    );
    ensure!(
        path.canonicalize()?.starts_with(root),
        "plugin resource escapes package"
    );
    Ok(metadata)
}

pub(super) fn skill_instructions(root: &Path, path: &Path) -> Result<String> {
    regular_path(root, path, false)?;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(64_001)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 64_000,
        "plugin skill is too large for model context"
    );
    String::from_utf8(bytes).context("plugin skill is not UTF-8")
}

fn capture(root: &Path, path: &Path) -> Result<(Vec<u8>, bool)> {
    let metadata = regular_path(root, path, false)?;
    ensure!(
        metadata.len() <= MAX_RESOURCE_BYTES,
        "plugin resource is too large"
    );
    let file = std::fs::File::open(path)?;
    ensure!(file.metadata()?.is_file(), "invalid plugin resource file");
    let mut bytes = Vec::new();
    file.take(MAX_RESOURCE_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_RESOURCE_BYTES,
        "plugin resource grew beyond limit"
    );
    #[cfg(unix)]
    let executable = {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    };
    #[cfg(not(unix))]
    let executable = false;
    Ok((bytes, executable))
}

impl PluginRuntime {
    /// Export only declared model-facing skills and discovered schemas. The
    /// returned bytes have no dependency on the package or host paths afterward.
    pub async fn snapshot(&self, name: &str, workspace_id: &str) -> Result<PluginSnapshot> {
        let details = self.details(name, workspace_id).await?;
        let root = self
            .catalog
            .package(name)?
            .context("plugin package unavailable")?
            .canonicalize()?;
        let mut resources = Vec::new();
        let mut skills = details["skills"]
            .as_array()
            .context("invalid plugin skills")?
            .clone();
        skills.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        let mut document = format!(
            "# {}\n\n{}\n\nAll resource paths below are relative to the directory containing this PLUGIN.md.\n\n## Skills\n",
            name,
            details["description"].as_str().unwrap_or_default()
        );
        let mut visited = 0usize;
        for skill in skills {
            let skill_name = skill["name"].as_str().context("invalid skill name")?;
            let relative = format!("skills/{skill_name}/SKILL.md");
            regular_path(&root, &root.join(&relative), false)?;
            let instructions = skill["instructions"]
                .as_str()
                .context("invalid skill instructions")?;
            add(
                &mut resources,
                relative.clone(),
                instructions.as_bytes().to_vec(),
                false,
            )?;
            document.push_str(&format!(
                "\n- {}: {}",
                skill_name,
                serde_json::to_string(&relative)?
            ));
            for kind in ["scripts", "references", "assets"] {
                let directory = root.join("skills").join(skill_name).join(kind);
                match std::fs::symlink_metadata(&directory) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    other => {
                        other?;
                    }
                }
                regular_path(&root, &directory, true)?;
                let mut pending = vec![directory];
                while let Some(directory) = pending.pop() {
                    for entry in std::fs::read_dir(directory)? {
                        let path = entry?.path();
                        visited += 1;
                        ensure!(visited <= 8192, "too many plugin resource entries");
                        let metadata = std::fs::symlink_metadata(&path)?;
                        ensure!(
                            !metadata.file_type().is_symlink(),
                            "plugin resource cannot be a link"
                        );
                        if metadata.is_dir() {
                            regular_path(&root, &path, true)?;
                            pending.push(path);
                        } else {
                            let (bytes, executable) = capture(&root, &path)?;
                            let relative = path
                                .strip_prefix(&root)?
                                .components()
                                .map(|part| {
                                    part.as_os_str()
                                        .to_str()
                                        .context("plugin resource path is not UTF-8")
                                })
                                .collect::<Result<Vec<_>>>()?
                                .join("/");
                            add(&mut resources, relative, bytes, executable)?;
                        }
                    }
                }
            }
        }
        document.push_str("\n\n## Tools\n\nCall these with plugin_call using this plugin name, the server, tool name, and arguments matching its input schema.\n\n```json\n");
        document.push_str(&serde_json::to_string_pretty(&details["tools"])?);
        document.push_str("\n```\n");
        add(
            &mut resources,
            "PLUGIN.md".into(),
            document.into_bytes(),
            false,
        )?;
        resources.sort_by(|a, b| a.file.path.cmp(&b.file.path));
        // Scope can change while MCP metadata is being discovered.
        self.require_available(name, workspace_id)?;
        let snapshot = PluginSnapshot {
            manifest: ResourceManifest {
                plugin: name.into(),
                has_tools: details["tools"].as_array().is_some_and(|v| !v.is_empty()),
                files: resources.iter().map(|r| r.file.clone()).collect(),
            },
            resources,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    /// Local and distributed publication share the same resource selection.
    /// Only this local adapter returns a host path, never the portable manifest.
    pub async fn publish(
        &self,
        name: &str,
        workspace_id: &str,
        files: &beale_tools::AgentFiles,
    ) -> Result<(PathBuf, bool)> {
        let snapshot = self.snapshot(name, workspace_id).await?;
        let prefix = format!("plugins/{name}");
        let mut document = None;
        for resource in &snapshot.resources {
            let path = files.write(&format!("{prefix}/{}", resource.file.path), &resource.bytes)?;
            #[cfg(unix)]
            if resource.file.executable {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
            }
            if resource.file.path == "PLUGIN.md" {
                document = Some(path);
            }
        }
        Ok((
            document.context("plugin document missing")?,
            snapshot.manifest.has_tools,
        ))
    }
}

#[cfg(test)]
#[path = "resources_tests.rs"]
mod tests;
