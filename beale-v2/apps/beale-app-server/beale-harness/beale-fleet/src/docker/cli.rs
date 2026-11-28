use super::RuntimeBundle;
use anyhow::{Context, Result, ensure};
use beale_contracts::swarm_container::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
const MAX_OUTPUT: u64 = 2 * 1024 * 1024;
const WORKSPACE: &str = "/beale-workspace";
const MANAGED: &str = "io.beale.swarm.managed";
const OWNER: &str = "io.beale.swarm.host";
const INSTANCE: &str = "io.beale.swarm.instance";
#[derive(Debug)]
pub(super) struct ProbeFailure(pub &'static str);
impl std::fmt::Display for ProbeFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for ProbeFailure {}
#[derive(Clone)]
pub struct Docker {
    program: PathBuf,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedImage {
    pub requested: String,
    pub digest: String,
    pub architecture: LinuxArchitecture,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainerHandle {
    pub host: String,
    pub instance: uuid::Uuid,
    pub id: String,
    pub volume: String,
    pub runtime_directory: String,
    pub image: ResolvedImage,
}
/// How a container on this Docker engine reaches its owning host, addressed
/// as [`beale_contracts::swarm_host::CONTAINER_HOST_ALIAS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostAccess {
    /// Docker Desktop resolves the alias itself and forwards it to the host's
    /// loopback services.
    Desktop,
    /// A native engine. The alias is mapped to this bridge gateway address,
    /// where the owning host serves only the scoped worker route.
    Bridge(std::net::Ipv4Addr),
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct IpamConfig {
    #[serde(default)]
    gateway: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerStatus {
    Running,
    Stopped,
    Unconfirmed,
}
pub use beale_contracts::swarm_container::{ContainerOptions, StartupMode};
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ImageInfo {
    id: String,
    os: String,
    architecture: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ContainerInfo {
    id: String,
    image: String,
    state: ContainerState,
    config: ContainerConfig,
    mounts: Vec<Mount>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ContainerState {
    running: bool,
    paused: bool,
    restarting: bool,
    status: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ContainerConfig {
    labels: BTreeMap<String, String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Mount {
    #[serde(rename = "Type")]
    kind: String,
    name: Option<String>,
    destination: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct VolumeInfo {
    name: String,
    labels: BTreeMap<String, String>,
}
impl Docker {
    /// Stream guest archives into a private host file; never extract guest paths
    /// through the host Docker CLI. A failed write leaves the old checkpoint intact.
    pub(crate) async fn archive_to(&self, args: &[OsString], path: &Path) -> Result<()> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(path)?;
        let mut child = self
            .command()
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::from(file.try_clone()?))
            .stderr(Stdio::piped())
            .spawn()?;
        let tree = beale_tools::process::ProcessTree::attach_required(&child)?;
        let stderr = child
            .stderr
            .take()
            .context("Docker archive stderr missing")?;
        let result = tokio::time::timeout(Duration::from_secs(600), async {
            let (status, _) =
                tokio::try_join!(async { anyhow::Ok(child.wait().await?) }, bounded(stderr))?;
            ensure!(
                status.success(),
                "Docker retained archive failed; original environment remains protected"
            );
            file.sync_all()?;
            anyhow::Ok(())
        })
        .await
        .context("Docker archive timed out")?;
        drop(tree);
        result
    }
    pub(crate) async fn checkpoint_archive(
        &self,
        handle: &ContainerHandle,
        path: &Path,
        state: bool,
    ) -> Result<()> {
        ensure!(
            self.inspect(handle).await? == ContainerStatus::Stopped,
            "checkpoint execution is active"
        );
        // The runtime directory always exists, including failed preflight. Its
        // state subtree contains receipts, plugin bundles, spools and captures.
        let source = if state {
            format!("{}:{}/.", handle.id, handle.runtime_directory)
        } else {
            format!("{}:/beale-workspace/.", handle.id)
        };
        self.archive_to(&args(&["container", "cp", &source, "-"]), path)
            .await
    }
    pub(crate) async fn save_root(
        &self,
        handle: &ContainerHandle,
        tag: &str,
        path: &Path,
    ) -> Result<String> {
        ensure!(
            self.inspect(handle).await? == ContainerStatus::Stopped,
            "root checkpoint execution is active"
        );
        let digest = String::from_utf8(
            self.run(
                &args(&[
                    "container",
                    "commit",
                    "--pause=false",
                    "--",
                    &handle.id,
                    tag,
                ]),
                None,
                Duration::from_secs(600),
            )
            .await?,
        )?
        .trim()
        .to_owned();
        valid_digest(&digest)?;
        self.archive_to(&args(&["image", "save", "--", &digest]), path)
            .await?;
        Ok(digest)
    }
    pub(crate) async fn load_root(
        &self,
        digest: &str,
        path: &Path,
        architecture: LinuxArchitecture,
    ) -> Result<ResolvedImage> {
        valid_digest(digest)?;
        self.run(
            &args(&["image", "load"]),
            Some(path),
            Duration::from_secs(600),
        )
        .await?;
        let bytes = self
            .run(
                &args(&["image", "inspect", "--", digest]),
                None,
                Duration::from_secs(30),
            )
            .await?;
        let rows: Vec<ImageInfo> = serde_json::from_slice(&bytes)?;
        ensure!(
            rows.len() == 1
                && rows[0].id == digest
                && rows[0].os == "linux"
                && rows[0].architecture
                    == match architecture {
                        LinuxArchitecture::Amd64 => "amd64",
                        LinuxArchitecture::Arm64 => "arm64",
                    },
            "restored root filesystem is incompatible"
        );
        Ok(ResolvedImage {
            requested: digest.into(),
            digest: digest.into(),
            architecture,
        })
    }
    pub(crate) async fn restore_archive(
        &self,
        handle: &ContainerHandle,
        path: &Path,
        state: bool,
    ) -> Result<()> {
        ensure!(
            self.inspect(handle).await? == ContainerStatus::Stopped,
            "restore execution is active"
        );
        let destination = if state {
            format!("{}:{}", handle.id, handle.runtime_directory)
        } else {
            format!("{}:/beale-workspace", handle.id)
        };
        self.run(
            &args(&["container", "cp", "-", &destination]),
            Some(path),
            Duration::from_secs(600),
        )
        .await?;
        Ok(())
    }
    pub fn new(program: PathBuf) -> Self {
        Self { program }
    }
    pub fn command(&self) -> tokio::process::Command {
        let mut command = beale_tools::process::contained_command(&self.program);
        // Registry credentials and daemon selection stay on this host. These
        // values configure the CLI and are never injected into the container.
        for key in [
            "DOCKER_HOST",
            "DOCKER_CONTEXT",
            "DOCKER_CONFIG",
            "DOCKER_TLS_VERIFY",
            "DOCKER_CERT_PATH",
            "SSH_AUTH_SOCK",
            "XDG_RUNTIME_DIR",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command
    }
    pub(crate) async fn run(
        &self,
        args: &[OsString],
        input: Option<&Path>,
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        self.run_input(args, input, None, timeout).await
    }
    async fn run_input(
        &self,
        args: &[OsString],
        input: Option<&Path>,
        bytes: Option<&[u8]>,
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        let mut command = self.command();
        let mut child = command
            .args(args)
            .stdin(if input.is_some() || bytes.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Docker CLI could not start")?;
        let tree = beale_tools::process::ProcessTree::attach_required(&child)?;
        let stdout = child.stdout.take().context("Docker stdout unavailable")?;
        let stderr = child.stderr.take().context("Docker stderr unavailable")?;
        let stdin = child.stdin.take();
        let result=tokio::time::timeout(timeout,async {
            let write=async {
                if let Some(mut stdin)=stdin {
                    if let Some(path)=input {
                        let mut source=tokio::fs::File::open(path).await?;
                        tokio::io::copy(&mut source,&mut stdin).await?;
                    } else if let Some(bytes)=bytes { stdin.write_all(bytes).await?; }
                    stdin.shutdown().await?;
                }
                anyhow::Ok(())
            };
            let read_out=bounded(stdout);let read_err=bounded(stderr);
            let (_,out,_,status)=tokio::try_join!(write,read_out,read_err,async {Ok(child.wait().await?)})?;
            ensure!(status.success(),"Docker operation failed; check daemon availability, registry access, and the selected image");
            Ok(out)
        }).await.context("Docker operation timed out; its outcome requires inspection")?;
        // Stop only CLI helpers, never infer container termination from CLI exit.
        drop(tree);
        result
    }
    /// Detects how containers reach this host. Containers use the default
    /// bridge network, so a native engine maps the alias to its gateway.
    pub async fn host_access(&self) -> Result<HostAccess> {
        let system: String = serde_json::from_slice(
            &self
                .run(
                    &args(&["info", "--format", "{{json .OperatingSystem}}"]),
                    None,
                    Duration::from_secs(15),
                )
                .await?,
        )
        .context("Docker engine identity is unavailable")?;
        if system.contains("Docker Desktop") {
            return Ok(HostAccess::Desktop);
        }
        let configs: Vec<IpamConfig> = serde_json::from_slice(
            &self
                .run(
                    &args(&[
                        "network",
                        "inspect",
                        "--format",
                        "{{json .IPAM.Config}}",
                        "--",
                        "bridge",
                    ]),
                    None,
                    Duration::from_secs(15),
                )
                .await?,
        )
        .context("Docker bridge network configuration is unavailable")?;
        bridge_gateway(&configs).map(HostAccess::Bridge)
    }
    pub async fn resolve_image(
        &self,
        image: &str,
        architecture: LinuxArchitecture,
    ) -> Result<ResolvedImage> {
        ensure!(
            !image.is_empty()
                && image.len() <= 1024
                && !image.starts_with('-')
                && !image.chars().any(|c| c.is_whitespace() || c.is_control()),
            "invalid Docker image name"
        );
        self.run(
            &args(&[
                "image",
                "pull",
                "--platform",
                architecture.docker_platform(),
                "--",
                image,
            ]),
            None,
            Duration::from_secs(600),
        )
        .await?;
        let bytes = self
            .run(
                &args(&["image", "inspect", "--", image]),
                None,
                Duration::from_secs(15),
            )
            .await?;
        let mut images: Vec<ImageInfo> = serde_json::from_slice(&bytes)?;
        ensure!(images.len() == 1, "ambiguous Docker image");
        let image_info = images.remove(0);
        ensure!(
            image_info.os == "linux"
                && image_info.architecture
                    == match architecture {
                        LinuxArchitecture::Amd64 => "amd64",
                        LinuxArchitecture::Arm64 => "arm64",
                    },
            "Docker image platform is incompatible"
        );
        valid_digest(&image_info.id)?;
        Ok(ResolvedImage {
            requested: image.into(),
            digest: image_info.id,
            architecture,
        })
    }
    /// The caller must persist its instance intent and hold its owner lock before
    /// creating resources. Partial failures retain named resources for inspection.
    pub async fn prepare_new(
        &self,
        host: &str,
        instance: uuid::Uuid,
        image: ResolvedImage,
        bundle: &RuntimeBundle,
        archive: &Path,
        options: &ContainerOptions,
    ) -> Result<ContainerHandle> {
        ensure!(
            beale_contracts::swarms::valid_id(host),
            "invalid Docker owner"
        );
        valid_digest(&image.digest)?;
        ensure!(
            image.architecture == bundle.manifest().architecture,
            "runtime platform mismatch"
        );
        let directory = {
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(archive)?;
            bundle.write_archive(&mut file, instance)?
        };
        let volume = format!("beale-swarm-workspace-{instance}");
        let name = format!("beale-swarm-{instance}");
        let labels = [
            format!("{MANAGED}=true"),
            format!("{OWNER}={host}"),
            format!("{INSTANCE}={instance}"),
        ];
        let mut create_volume = args(&["volume", "create"]);
        for label in &labels {
            create_volume.extend(args(&["--label", label]));
        }
        create_volume.extend(args(&["--", &volume]));
        self.run(&create_volume, None, Duration::from_secs(30))
            .await?;
        self.check_volume(&volume, host, instance).await?;
        let mut create = args(&[
            "container",
            "create",
            "--name",
            &name,
            "--restart",
            "no",
            "--platform",
            image.architecture.docker_platform(),
            "--mount",
            &format!("type=volume,source={volume},target={WORKSPACE},volume-nocopy"),
        ]);
        for label in &labels {
            create.extend(args(&["--label", label]));
        }
        // Give the worker a stable name for its owning host. Docker Desktop
        // provides the alias; a native engine maps it to the bridge gateway,
        // where the host serves the scoped worker route.
        if let HostAccess::Bridge(gateway) = self.host_access().await? {
            create.extend(args(&[
                "--add-host",
                &format!(
                    "{}:{gateway}",
                    beale_contracts::swarm_host::CONTAINER_HOST_ALIAS
                ),
            ]));
        }
        ensure!(
            options.environment.len() <= 64,
            "too many container environment values"
        );
        for (key, value) in &options.environment {
            ensure!(
                !key.is_empty()
                    && key.len() <= 128
                    && key.bytes().enumerate().all(|(i, c)| c.is_ascii_alphabetic()
                        || c == b'_'
                        || i > 0 && c.is_ascii_digit())
                    && !key.starts_with("BEALE_")
                    && value.len() <= 16384
                    && !value.contains('\0'),
                "invalid container environment override"
            );
            create.extend(args(&["--env", &format!("{key}={value}")]));
        }
        if let Some(user) = &options.user {
            ensure!(
                !user.is_empty() && user.len() <= 128 && !user.chars().any(char::is_control),
                "invalid container user"
            );
            create.extend(args(&["--user", user]));
        }
        if matches!(options.startup, StartupMode::WorkerIdle) {
            create.extend(args(&[
                "--entrypoint",
                &format!("{directory}/beale-worker"),
            ]));
        }
        create.extend(args(&["--", &image.digest]));
        if matches!(options.startup, StartupMode::WorkerIdle) {
            create.push("container-idle".into());
        } else {
            create.extend(options.command.iter().map(OsString::from));
        }
        let output = self.run(&create, None, Duration::from_secs(30)).await?;
        let id = std::str::from_utf8(&output)?.trim().to_owned();
        valid_id(&id)?;
        let handle = ContainerHandle {
            host: host.into(),
            instance,
            id,
            volume,
            runtime_directory: directory,
            image,
        };
        self.inspect(&handle).await?;
        self.run(
            &args(&["container", "cp", "-", &format!("{}:/", handle.id)]),
            Some(archive),
            Duration::from_secs(120),
        )
        .await?;
        self.start(&handle).await?;
        self.probe(&handle).await?;
        Ok(handle)
    }
    async fn check_volume(&self, name: &str, host: &str, instance: uuid::Uuid) -> Result<()> {
        let bytes = self
            .run(
                &args(&["volume", "inspect", "--", name]),
                None,
                Duration::from_secs(15),
            )
            .await?;
        let volumes: Vec<VolumeInfo> = serde_json::from_slice(&bytes)?;
        ensure!(
            volumes.len() == 1 && volumes[0].name == name,
            "volume identity changed"
        );
        labels(&volumes[0].labels, host, instance)
    }
    pub async fn inspect(&self, handle: &ContainerHandle) -> Result<ContainerStatus> {
        valid_id(&handle.id)?;
        valid_digest(&handle.image.digest)?;
        ensure!(
            handle.volume == format!("beale-swarm-workspace-{}", handle.instance)
                && handle.runtime_directory == format!("/beale-runtime-{}", handle.instance),
            "invalid retained Docker identity"
        );
        let bytes = self
            .run(
                &args(&["container", "inspect", "--", &handle.id]),
                None,
                Duration::from_secs(15),
            )
            .await?;
        let containers: Vec<ContainerInfo> = serde_json::from_slice(&bytes)?;
        ensure!(containers.len() == 1, "container identity unavailable");
        let c = &containers[0];
        ensure!(
            c.id == handle.id && c.image == handle.image.digest,
            "retained container changed"
        );
        labels(&c.config.labels, &handle.host, handle.instance)?;
        ensure!(
            c.mounts.iter().any(|m| m.kind == "volume"
                && m.name.as_deref() == Some(&handle.volume)
                && m.destination == WORKSPACE),
            "retained workspace volume changed"
        );
        self.check_volume(&handle.volume, &handle.host, handle.instance)
            .await?;
        Ok(if c.state.restarting || c.state.paused {
            ContainerStatus::Unconfirmed
        } else if c.state.running {
            ContainerStatus::Running
        } else if matches!(c.state.status.as_str(), "exited" | "created") {
            ContainerStatus::Stopped
        } else {
            ContainerStatus::Unconfirmed
        })
    }
    /// Reconcile an interrupted create by its durable, unique instance name.
    /// Absence or conflicting labels is an error, never permission to recreate.
    pub async fn discover(
        &self,
        host: &str,
        instance: uuid::Uuid,
        image: &ResolvedImage,
    ) -> Result<ContainerHandle> {
        let bytes = self
            .run(
                &args(&[
                    "container",
                    "inspect",
                    "--",
                    &format!("beale-swarm-{instance}"),
                ]),
                None,
                Duration::from_secs(15),
            )
            .await?;
        let found: Vec<ContainerInfo> = serde_json::from_slice(&bytes)?;
        ensure!(found.len() == 1, "retained container missing");
        let handle = ContainerHandle {
            host: host.into(),
            instance,
            id: found[0].id.clone(),
            volume: format!("beale-swarm-workspace-{instance}"),
            runtime_directory: format!("/beale-runtime-{instance}"),
            image: image.clone(),
        };
        self.inspect(&handle).await?;
        Ok(handle)
    }
    pub fn worker_command(
        &self,
        handle: &ContainerHandle,
        mode: &str,
    ) -> Result<tokio::process::Command> {
        valid_id(&handle.id)?;
        ensure!(
            matches!(mode, "container-prepare" | "container-worker"),
            "invalid container worker mode"
        );
        let mut command = self.command();
        command.args(self.exec_arguments(handle, mode));
        Ok(command)
    }
    fn exec_arguments(&self, handle: &ContainerHandle, mode: &str) -> Vec<OsString> {
        args(&[
            "container",
            "exec",
            "--interactive",
            "--workdir",
            WORKSPACE,
            "--",
            &handle.id,
            &format!("{}/bash", handle.runtime_directory),
            "--noprofile",
            "--norc",
            "-c",
            // The injected shell expands the guest's effective PATH, including
            // image defaults and configured overrides. All data is positional.
            "export PATH=\"$1${PATH:+:$PATH}\"; shift; exec \"$@\"",
            "beale-container",
            &handle.runtime_directory,
            &format!("{}/beale-worker", handle.runtime_directory),
            mode,
        ])
    }
    /// Stdin carries only the scoped assignment credential, never host credentials.
    pub async fn prepare_workspace(
        &self,
        handle: &ContainerHandle,
        bootstrap: &[u8],
    ) -> Result<()> {
        ensure!(
            bootstrap.len() <= beale_contracts::swarms::MAX_DOCUMENT_BYTES,
            "container bootstrap too large"
        );
        ensure!(
            self.inspect(handle).await? == ContainerStatus::Running,
            "container is not running"
        );
        let bytes=self.run_input(&self.exec_arguments(handle,"container-prepare"),None,Some(bootstrap),Duration::from_secs(240)).await
            .context("container workspace/leader preflight failed; check reachability, snapshot metadata support, Git availability for repositories, and writable storage")?;
        let report: PreparationReport =
            serde_json::from_slice(&bytes).context("Invalid container preparation report")?;
        ensure!(
            report.version == 1,
            "Unsupported container preparation report"
        );
        if let Some(reason) = report.failure {
            return Err(reason.into());
        }
        Ok(())
    }
    pub async fn start(&self, handle: &ContainerHandle) -> Result<()> {
        ensure!(
            self.inspect(handle).await? == ContainerStatus::Stopped,
            "container is not safely stopped"
        );
        self.run(
            &args(&["container", "start", "--", &handle.id]),
            None,
            Duration::from_secs(30),
        )
        .await?;
        ensure!(
            self.inspect(handle).await? == ContainerStatus::Running,
            "container did not remain running"
        );
        Ok(())
    }
    pub async fn stop(&self, handle: &ContainerHandle) -> Result<bool> {
        if self.inspect(handle).await? == ContainerStatus::Stopped {
            return Ok(true);
        }
        self.run(
            &args(&["container", "stop", "--time", "5", "--", &handle.id]),
            None,
            Duration::from_secs(20),
        )
        .await?;
        Ok(self.inspect(handle).await? == ContainerStatus::Stopped)
    }
    pub async fn probe(&self, handle: &ContainerHandle) -> Result<RuntimeProbe> {
        ensure!(
            self.inspect(handle).await? == ContainerStatus::Running,
            "container is unavailable for probing"
        );
        let bytes = self
            .run(
                &self.exec_arguments(handle, "container-probe"),
                None,
                Duration::from_secs(15),
            )
            .await?;
        let probe: RuntimeProbe = serde_json::from_slice(&bytes)?;
        probe
            .validate(handle.image.architecture)
            .map_err(ProbeFailure)?;
        Ok(probe)
    }
    /// Only a stopped, still-owned instance may be physically removed. The
    /// caller additionally needs the durable retention/deletion claim.
    pub async fn delete(&self, handle: &ContainerHandle) -> Result<()> {
        ensure!(
            self.inspect(handle).await? == ContainerStatus::Stopped,
            "active container cannot be deleted"
        );
        self.run(
            &args(&["container", "rm", "--volumes", "--", &handle.id]),
            None,
            Duration::from_secs(30),
        )
        .await?;
        self.check_volume(&handle.volume, &handle.host, handle.instance)
            .await?;
        self.run(
            &args(&["volume", "rm", "--", &handle.volume]),
            None,
            Duration::from_secs(30),
        )
        .await?;
        Ok(())
    }
    /// Deletion resumes only under a durable generation claim. A successful
    /// listing distinguishes absent resources from an unavailable daemon.
    pub async fn delete_container_step(&self, handle: &ContainerHandle) -> Result<()> {
        valid_id(&handle.id)?;
        let ids = self
            .run(
                &args(&[
                    "container",
                    "ls",
                    "--all",
                    "--no-trunc",
                    "--filter",
                    &format!("id={}", handle.id),
                    "--format",
                    "{{.ID}}",
                ]),
                None,
                Duration::from_secs(15),
            )
            .await?;
        if std::str::from_utf8(&ids)?.lines().all(|id| id != handle.id) {
            return Ok(());
        }
        ensure!(
            self.inspect(handle).await? == ContainerStatus::Stopped,
            "active container cannot be deleted"
        );
        self.run(
            &args(&["container", "rm", "--volumes", "--", &handle.id]),
            None,
            Duration::from_secs(30),
        )
        .await?;
        Ok(())
    }
    pub async fn delete_volume_step(&self, handle: &ContainerHandle) -> Result<()> {
        ensure!(
            handle.volume == format!("beale-swarm-workspace-{}", handle.instance),
            "invalid retained volume identity"
        );
        let names = self
            .run(
                &args(&[
                    "volume",
                    "ls",
                    "--filter",
                    &format!("name=^{}$", handle.volume),
                    "--format",
                    "{{.Name}}",
                ]),
                None,
                Duration::from_secs(15),
            )
            .await?;
        if std::str::from_utf8(&names)?
            .lines()
            .all(|name| name != handle.volume)
        {
            return Ok(());
        }
        self.check_volume(&handle.volume, &handle.host, handle.instance)
            .await?;
        self.run(
            &args(&["volume", "rm", "--", &handle.volume]),
            None,
            Duration::from_secs(30),
        )
        .await?;
        Ok(())
    }
}
async fn bounded(reader: impl tokio::io::AsyncRead + Unpin) -> Result<Vec<u8>> {
    let mut bytes = vec![];
    reader.take(MAX_OUTPUT + 1).read_to_end(&mut bytes).await?;
    ensure!(
        bytes.len() as u64 <= MAX_OUTPUT,
        "Docker output exceeded its bound"
    );
    Ok(bytes)
}
fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
fn bridge_gateway(configs: &[IpamConfig]) -> Result<std::net::Ipv4Addr> {
    let gateway = configs
        .iter()
        .find_map(|config| config.gateway.parse::<std::net::Ipv4Addr>().ok())
        .context("Docker bridge gateway is unavailable")?;
    ensure!(
        !gateway.is_unspecified() && !gateway.is_loopback() && !gateway.is_multicast(),
        "invalid Docker bridge gateway"
    );
    Ok(gateway)
}
fn valid_id(value: &str) -> Result<()> {
    ensure!(
        value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit()),
        "invalid Docker container ID"
    );
    Ok(())
}
fn valid_digest(value: &str) -> Result<()> {
    ensure!(
        value
            .strip_prefix("sha256:")
            .is_some_and(beale_contracts::swarm_files::digest),
        "Docker image digest unavailable"
    );
    Ok(())
}
fn labels(values: &BTreeMap<String, String>, host: &str, instance: uuid::Uuid) -> Result<()> {
    ensure!(
        values.get(MANAGED).is_some_and(|v| v == "true")
            && values.get(OWNER).is_some_and(|v| v == host)
            && values
                .get(INSTANCE)
                .is_some_and(|v| v == &instance.to_string()),
        "Docker resource belongs to another owner"
    );
    Ok(())
}

#[cfg(test)]
mod host_access_tests {
    use super::*;

    #[test]
    fn bridge_gateway_comes_from_docker_ipam_output() {
        let configs: Vec<IpamConfig> = serde_json::from_str(
            r#"[{"Subnet":"fd00:db8::/64","Gateway":"fd00:db8::1"},{"Subnet":"192.0.2.0/24","Gateway":"192.0.2.1","IPRange":""}]"#,
        )
        .unwrap();
        assert_eq!(
            bridge_gateway(&configs).unwrap(),
            "192.0.2.1".parse::<std::net::Ipv4Addr>().unwrap()
        );
        for invalid in [
            r#"[]"#,
            r#"[{"Subnet":"192.0.2.0/24"}]"#,
            r#"[{"Gateway":"0.0.0.0"}]"#,
            r#"[{"Gateway":"127.0.0.1"}]"#,
        ] {
            let configs: Vec<IpamConfig> = serde_json::from_str(invalid).unwrap();
            assert!(bridge_gateway(&configs).is_err(), "{invalid}");
        }
    }
}
