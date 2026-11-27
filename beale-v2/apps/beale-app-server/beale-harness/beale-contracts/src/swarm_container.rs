//! Versioned portable runtime metadata. No credentials or host paths.
use serde::{Deserialize, Serialize};
pub const RUNTIME_VERSION: u32 = 1;
pub const CONFIGURATIONS_PATH: &str = "/v1/fleet/container-configurations";

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreparationFailure {
    Runtime,
    Snapshot,
    RetainedStorage,
    ArtifactTransfer,
}
impl std::fmt::Display for PreparationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {Self::Runtime=>"Container runtime or private storage unavailable",Self::Snapshot=>"Snapshot transfer failed; check leader connectivity, Git availability, and filesystem metadata compatibility",Self::RetainedStorage=>"Retained workspace integrity check failed; restore retained state before reuse",Self::ArtifactTransfer=>"Artifact transfer preflight failed; check leader connectivity and publication storage"})
    }
}
impl std::error::Error for PreparationFailure {}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparationReport {
    pub version: u32,
    pub failure: Option<PreparationFailure>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupMode {
    #[default]
    WorkerIdle,
    ImageDefault,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainerOptions {
    #[serde(default)]
    pub startup: StartupMode,
    #[serde(default)]
    pub environment: std::collections::BTreeMap<String, String>,
    pub user: Option<String>,
    /// An explicit command override; empty preserves the image's command.
    #[serde(default)]
    pub command: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageConfiguration {
    pub version: u32,
    pub id: String,
    pub revision: u64,
    pub image: String,
    pub architecture: LinuxArchitecture,
    #[serde(default)]
    pub options: ContainerOptions,
}
impl ImageConfiguration {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != 1
            || !crate::swarms::valid_id(&self.id)
            || !(1..=i64::MAX as u64).contains(&self.revision)
            || self.image.is_empty()
            || self.image.len() > 1024
            || self.image.starts_with('-')
            || self
                .image
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
        {
            return Err("invalid container image configuration");
        }
        let o = &self.options;
        if o.environment.len() > 64
            || o.environment.iter().any(|(k, v)| {
                k.is_empty()
                    || k.len() > 128
                    || k.starts_with("BEALE_")
                    || !k.bytes().enumerate().all(|(i, c)| {
                        c.is_ascii_alphabetic() || c == b'_' || i > 0 && c.is_ascii_digit()
                    })
                    || v.len() > 8192
                    || v.contains('\0')
            })
            || o.user.as_ref().is_some_and(|s| {
                s.is_empty()
                    || s.len() > 128
                    || s.starts_with('-')
                    || s.chars().any(char::is_control)
            })
            || o.command.len() > 64
            || o.command.iter().any(|s| s.len() > 8192 || s.contains('\0'))
            || o.startup == StartupMode::WorkerIdle && !o.command.is_empty()
        {
            return Err("invalid container startup options");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Readiness {
    Unprepared,
    Preparing,
    Ready,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationReadiness {
    pub configuration: ImageConfiguration,
    pub host_id: String,
    pub state: Readiness,
    pub reason: Option<String>,
    pub image_digest: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinuxArchitecture {
    Amd64,
    Arm64,
}
impl LinuxArchitecture {
    pub fn docker_platform(self) -> &'static str {
        match self {
            Self::Amd64 => "linux/amd64",
            Self::Arm64 => "linux/arm64",
        }
    }
    pub fn elf_machine(self) -> u16 {
        match self {
            Self::Amd64 => 62,
            Self::Arm64 => 183,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeArtifact {
    pub sha256: String,
    pub bytes: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeManifest {
    pub version: u32,
    pub worker_protocol: u32,
    pub architecture: LinuxArchitecture,
    pub worker: RuntimeArtifact,
    pub shell: RuntimeArtifact,
    pub shell_license: RuntimeArtifact,
}
impl RuntimeManifest {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != RUNTIME_VERSION || self.worker_protocol != crate::swarms::VERSION {
            return Err("unsupported container runtime version");
        }
        for artifact in [&self.worker, &self.shell, &self.shell_license] {
            if !crate::swarm_files::digest(&artifact.sha256)
                || artifact.bytes == 0
                || artifact.bytes > 512 * 1024 * 1024
            {
                return Err("invalid runtime artifact");
            }
        }
        if self.shell_license.bytes > 1024 * 1024 {
            return Err("runtime license too large");
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeProbe {
    pub version: u32,
    pub worker_protocol: u32,
    pub architecture: LinuxArchitecture,
    pub shell_available: bool,
    pub workspace_writable: bool,
}
impl RuntimeProbe {
    pub fn validate(&self, architecture: LinuxArchitecture) -> Result<(), &'static str> {
        if self.version != RUNTIME_VERSION
            || self.worker_protocol != crate::swarms::VERSION
            || self.architecture != architecture
        {
            return Err("incompatible container worker runtime");
        }
        if !self.shell_available {
            return Err("container shell did not start");
        }
        if !self.workspace_writable {
            return Err("container workspace is not writable");
        }
        Ok(())
    }
}
