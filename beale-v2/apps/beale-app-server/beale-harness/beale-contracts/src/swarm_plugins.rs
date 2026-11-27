//! Scoped plugin calls use leader-owned configuration, never worker launch data.
use crate::{
    swarm_worker::ExecutionIdentity,
    swarms::{MAX_DOCUMENT_BYTES, valid_id},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invocation {
    pub plugin: String,
    pub server: String,
    pub tool: String,
    pub arguments: Value,
}
impl Invocation {
    pub fn validate(&self) -> Result<(), &'static str> {
        if [&self.plugin, &self.server, &self.tool].iter().any(|name| {
            name.trim().is_empty() || name.len() > 128 || name.chars().any(char::is_control)
        }) || !self.arguments.is_object()
            || serde_json::to_vec(self).map_or(true, |v| v.len() > MAX_DOCUMENT_BYTES / 2)
        {
            return Err("invalid plugin invocation");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum Outcome {
    Completed {
        output: String,
        is_error: bool,
        artifact_refs: Vec<String>,
    },
    /// No conclusive result was durably captured. Never automatically retry.
    OutcomeUnknown {},
}
impl Outcome {
    pub fn validate(&self) -> Result<(), &'static str> {
        if serde_json::to_vec(self).map_or(true, |bytes| bytes.len() > MAX_DOCUMENT_BYTES / 4) {
            return Err("encoded plugin outcome is too large");
        }
        if let Self::Completed {
            output,
            artifact_refs,
            ..
        } = self
            && (output.len() > MAX_DOCUMENT_BYTES / 4
                || artifact_refs.len() > 32
                || artifact_refs.iter().any(|id| !valid_id(id))
                || artifact_refs
                    .iter()
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    != artifact_refs.len())
        {
            return Err("invalid plugin outcome");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub identity: ExecutionIdentity,
    pub outcome: Outcome,
}
impl Reply {
    pub fn validate_for(&self, identity: &ExecutionIdentity) -> Result<(), &'static str> {
        self.identity.validate()?;
        self.outcome.validate()?;
        if self.identity != *identity {
            return Err("plugin reply execution mismatch");
        }
        Ok(())
    }
}

/// Portable model-readable plugin files. MCP launch configuration is excluded.
pub const MAX_RESOURCE_BYTES: u64 = 8_000_000;
pub const MAX_BUNDLE_BYTES: u64 = 64_000_000;
pub const MAX_BUNDLE_FILES: usize = 4_096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceFile {
    pub path: String,
    pub sha256: String,
    pub size: u64,
    pub executable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceManifest {
    pub plugin: String,
    pub has_tools: bool,
    pub files: Vec<ResourceFile>,
}

/// Reject aliases and unsupported names before a bundle reaches any filesystem.
pub fn resource_path(path: &str) -> bool {
    if path.len() > 512 || !crate::swarm_files::relative_path(path) {
        return false;
    }
    let parts: Vec<_> = path.split('/').collect();
    if parts.iter().any(|part| !portable_component(part)) {
        return false;
    }
    path == "PLUGIN.md"
        || (parts.len() >= 3
            && parts[0] == "skills"
            && ((parts.len() == 3 && parts[2] == "SKILL.md")
                || (parts.len() >= 4 && matches!(parts[2], "scripts" | "references" | "assets"))))
}
fn portable_component(part: &str) -> bool {
    let stem = part
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    !(part.ends_with([' ', '.'])
        || part.contains(['<', '>', '|', '"', '*', '?'])
        || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(
                stem.get(3..),
                Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
            )))
}
impl ResourceManifest {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.plugin.is_empty()
            || self.plugin.len() > 64
            || !self
                .plugin
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'.'))
            || !self.plugin.as_bytes()[0].is_ascii_alphanumeric()
            || !self.plugin.as_bytes()[self.plugin.len() - 1].is_ascii_alphanumeric()
            || self.plugin.contains("..")
            || self.plugin.contains("--")
            || !portable_component(&self.plugin)
            || self.files.is_empty()
            || self.files.len() > MAX_BUNDLE_FILES
            || serde_json::to_vec(self).map_or(true, |v| v.len() > MAX_DOCUMENT_BYTES / 2)
        {
            return Err("invalid plugin resource manifest");
        }
        let mut paths = std::collections::HashSet::new();
        let mut previous = "";
        let mut prefixes = std::collections::HashMap::new();
        let mut total = 0u64;
        for file in &self.files {
            if !resource_path(&file.path)
                || !crate::swarm_files::digest(&file.sha256)
                || file.size > MAX_RESOURCE_BYTES
                || file.path.as_str() <= previous
                || !paths.insert(file.path.to_lowercase())
            {
                return Err("invalid plugin resource file");
            }
            let mut prefix = String::new();
            for part in file.path.split('/') {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(part);
                if let Some(old) = prefixes.insert(prefix.to_lowercase(), prefix.clone())
                    && old != prefix
                {
                    return Err("plugin resource directory alias");
                }
            }
            previous = &file.path;
            total = total
                .checked_add(file.size)
                .ok_or("plugin bundle size overflow")?;
        }
        if total > MAX_BUNDLE_BYTES
            || !paths.contains("plugin.md")
            || paths.iter().filter(|p| p.starts_with("skills/")).any(|p| {
                let skill = p.split('/').nth(1).unwrap_or_default();
                !paths.contains(&format!("skills/{skill}/skill.md"))
            })
            || paths.iter().any(|path| {
                path.split('/')
                    .scan(String::new(), |prefix, part| {
                        if !prefix.is_empty() {
                            prefix.push('/');
                        }
                        prefix.push_str(part);
                        Some(prefix.clone())
                    })
                    .any(|prefix| prefix != *path && paths.contains(&prefix))
            })
        {
            return Err("invalid plugin resource bundle");
        }
        Ok(())
    }
}

pub const RESOURCE_CHUNK_BYTES: usize = 192 * 1024;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceRequest {
    Catalog {},
    Prepare {
        plugin: String,
    },
    Read {
        plugin: String,
        bundle: String,
        path: String,
        offset: u64,
    },
}
fn plugin_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'.'))
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name.as_bytes()[name.len() - 1].is_ascii_alphanumeric()
        && !name.contains("..")
        && !name.contains("--")
}
impl ResourceRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Catalog {} => Ok(()),
            Self::Prepare { plugin } if plugin_name(plugin) => Ok(()),
            Self::Read {
                plugin,
                bundle,
                path,
                offset,
            } if plugin_name(plugin)
                && crate::swarm_files::digest(bundle)
                && resource_path(path)
                && *offset <= MAX_RESOURCE_BYTES =>
            {
                Ok(())
            }
            _ => Err("invalid plugin resource request"),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceResult {
    Catalog {
        plugins: Vec<String>,
    },
    Prepared {
        bundle: String,
        manifest: ResourceManifest,
    },
    Chunk {
        bundle: String,
        file: ResourceFile,
        offset: u64,
        bytes: Vec<u8>,
        sha256: String,
    },
    Unavailable {
        message: String,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceReply {
    pub identity: ExecutionIdentity,
    pub result: ResourceResult,
}
impl ResourceReply {
    pub fn validate_for(
        &self,
        identity: &ExecutionIdentity,
        request: &ResourceRequest,
    ) -> Result<(), &'static str> {
        self.identity.validate()?;
        request.validate()?;
        if self.identity != *identity
            || serde_json::to_vec(self).map_or(true, |v| v.len() > MAX_DOCUMENT_BYTES - 4096)
        {
            return Err("invalid plugin resource reply");
        }
        match (&self.result, request) {
            (ResourceResult::Unavailable { message }, _)
                if !message.is_empty() && message.len() <= 2048 =>
            {
                Ok(())
            }
            (ResourceResult::Catalog { plugins }, ResourceRequest::Catalog {})
                if plugins.len() <= 128
                    && plugins.iter().all(|name| plugin_name(name))
                    && plugins.windows(2).all(|p| p[0] < p[1]) =>
            {
                Ok(())
            }
            (
                ResourceResult::Prepared { bundle, manifest },
                ResourceRequest::Prepare { plugin },
            ) if manifest.plugin == *plugin && crate::swarm_files::digest(bundle) => {
                manifest.validate()
            }
            (
                ResourceResult::Chunk {
                    bundle,
                    file,
                    offset,
                    bytes,
                    sha256,
                },
                ResourceRequest::Read {
                    bundle: expected,
                    path,
                    offset: start,
                    ..
                },
            ) if bundle == expected
                && file.path == *path
                && *offset == *start
                && resource_path(&file.path)
                && crate::swarm_files::digest(&file.sha256)
                && crate::swarm_files::digest(sha256)
                && file.size <= MAX_RESOURCE_BYTES
                && bytes.len() <= RESOURCE_CHUNK_BYTES
                && offset
                    .checked_add(bytes.len() as u64)
                    .is_some_and(|end| end <= file.size)
                && (!bytes.is_empty() || *offset == file.size) =>
            {
                Ok(())
            }
            _ => Err("plugin resource reply does not match request"),
        }
    }
}
