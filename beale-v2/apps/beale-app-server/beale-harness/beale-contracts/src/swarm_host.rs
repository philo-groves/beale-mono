//! Trusted host-to-host supervision, separate from the scoped worker audience.
//! Host paths, executable arguments and provider credentials are never accepted.
use crate::{swarm_worker::ExecutionIdentity, swarms::valid_id};
use serde::{Deserialize, Serialize};

pub const HOST_EXECUTION_PATH: &str = "/v1/fleet/worker-executions";
pub const HOST_EXECUTION_VERSION: u32 = 1;
pub const HOST_EXECUTION_MAX_BYTES: usize = 65_536;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionTarget {
    pub host_id: String,
    pub leader_id: String,
    pub configuration_id: String,
    pub configuration_revision: u64,
    pub instance_id: String,
    pub identity: ExecutionIdentity,
}
impl ExecutionTarget {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.identity.validate()?;
        if [
            &self.host_id,
            &self.leader_id,
            &self.configuration_id,
            &self.instance_id,
        ]
        .iter()
        .any(|id| !valid_id(id))
            || !(1..=i64::MAX as u64).contains(&self.configuration_revision)
            || self.identity.generation > i64::MAX as u64
        {
            return Err("invalid host execution target");
        }
        Ok(())
    }
}

/// Only a scoped worker credential crosses this boundary. Deliberately no Debug.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostWorkerLaunch {
    pub leader_origin: String,
    pub worker_bearer: String,
    /// A sealed initial snapshot for a newly allocated private workspace.
    /// None requires an already retained/prepared workspace, never recapture.
    pub snapshot: Option<String>,
}
impl HostWorkerLaunch {
    pub fn validate(&self) -> Result<(), &'static str> {
        validate_origin(&self.leader_origin)?;
        if self
            .snapshot
            .as_ref()
            .is_some_and(|s| !crate::swarm_files::digest(s))
        {
            return Err("invalid workspace snapshot");
        }
        if self.worker_bearer.len() != 64
            || !self.worker_bearer.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("invalid scoped worker credential");
        }
        Ok(())
    }
}

/// Name a local Docker container uses for its owning host. Docker Desktop
/// provides it and forwards it to the host's loopback services. On a native
/// engine the owning host maps it to its bridge gateway when it creates the
/// container and serves only the worker route there.
pub const CONTAINER_HOST_ALIAS: &str = "host.docker.internal";

fn loopback_host(origin: &url::Url) -> bool {
    origin.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    })
}

/// Plain HTTP is accepted only where traffic stays on the owning host: a
/// loopback address, or the container alias for the host's local bridge.
pub fn validate_origin(value: &str) -> Result<(), &'static str> {
    let origin = url::Url::parse(value).map_err(|_| "invalid host origin")?;
    let local = loopback_host(&origin) || origin.host_str() == Some(CONTAINER_HOST_ALIAS);
    if value.len() > 2048
        || value.trim() != value
        || value.chars().any(char::is_control)
        || origin.host_str().is_none()
        || !(origin.scheme() == "https" || origin.scheme() == "http" && local)
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
        || !origin.port_or_known_default().is_some_and(|port| port > 0)
    {
        return Err("invalid host origin");
    }
    Ok(())
}

/// The leader origin as seen from a container on the leader's own Docker
/// engine. Inside a container, loopback is the container itself, so a loopback
/// origin is addressed through [`CONTAINER_HOST_ALIAS`] on the same port. Other
/// origins, such as a paired Fleet HTTPS origin, are returned unchanged.
pub fn container_leader_origin(value: &str) -> Result<String, &'static str> {
    validate_origin(value)?;
    let mut origin = url::Url::parse(value).map_err(|_| "invalid host origin")?;
    if loopback_host(&origin) {
        origin
            .set_host(Some(CONTAINER_HOST_ALIAS))
            .map_err(|_| "invalid host origin")?;
    }
    let translated = origin.to_string();
    validate_origin(&translated)?;
    Ok(translated)
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HostExecutionOperation {
    Start { launch: HostWorkerLaunch },
    Stop {},
    Inspect {},
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostExecutionRequest {
    pub version: u32,
    pub operation_id: String,
    pub target: ExecutionTarget,
    pub operation: HostExecutionOperation,
}
impl HostExecutionRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.target.validate()?;
        if self.version != HOST_EXECUTION_VERSION
            || !valid_id(&self.operation_id)
            || !serde_json::to_vec(self).is_ok_and(|bytes| bytes.len() <= HOST_EXECUTION_MAX_BYTES)
        {
            return Err("invalid host execution request");
        }
        if let HostExecutionOperation::Start { launch } = &self.operation {
            launch.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostExecutionPhase {
    LaunchRequested,
    Running,
    StopRequested,
    TerminationUnconfirmed,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostExecutionReport {
    pub target: ExecutionTarget,
    pub phase: HostExecutionPhase,
    pub revision: u64,
    /// Sticky cancellation intent survives an uncertain backend observation.
    pub stop_requested: bool,
}
impl HostExecutionReport {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.target.validate()?;
        if !(1..=i64::MAX as u64).contains(&self.revision)
            || self.phase == HostExecutionPhase::StopRequested && !self.stop_requested
            || matches!(
                self.phase,
                HostExecutionPhase::LaunchRequested | HostExecutionPhase::Running
            ) && self.stop_requested
        {
            return Err("invalid host execution state");
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostExecutionReply {
    pub version: u32,
    pub operation_id: String,
    pub report: HostExecutionReport,
}
impl HostExecutionReply {
    pub fn validate_for(&self, request: &HostExecutionRequest) -> Result<(), &'static str> {
        self.report.validate()?;
        if self.version != HOST_EXECUTION_VERSION
            || self.operation_id != request.operation_id
            || self.report.target != request.target
            || !(1..=i64::MAX as u64).contains(&self.report.revision)
        {
            return Err("host execution acknowledgement mismatch");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostExecutionDescriptor {
    pub version: u32,
    pub host_id: String,
    pub execution_available: bool,
}
