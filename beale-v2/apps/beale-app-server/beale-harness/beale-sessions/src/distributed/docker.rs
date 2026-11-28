//! Docker adapter for the existing destination supervisor. Placement is external.
use super::{
    ManagedWorker, ProcessFuture,
    peer_host::{PreparedEnvironments, PreparedWorker},
};
use anyhow::{Context, Result, ensure};
use beale_contracts::swarm_host::{ExecutionTarget, HostWorkerLaunch};
use beale_fleet::{
    docker::{ContainerStatus, DockerEnvironments, Instance},
    worker_host::{BackendFuture, BackendRejection},
};
use beale_storage::fleet_executions::BackendObservation;
use std::{fs::File, process::Stdio, time::Duration};
use tokio::{io::AsyncWriteExt, process::Child};

pub struct DockerBackend(pub DockerEnvironments);
/// Production entry requires an owning-host reservation; the raw prepared
/// adapter remains usable for isolated backend acceptance tests.
pub struct AllocatedDockerBackend(pub beale_fleet::allocation::Allocator);
impl AllocatedDockerBackend {
    fn reservation(
        &self,
        target: &ExecutionTarget,
    ) -> Result<beale_contracts::swarm_allocation::Allocation> {
        use beale_contracts::swarm_allocation::AllocationPhase;
        self.0
            .store
            .allocations()?
            .into_iter()
            .rev()
            .find(|a| {
                a.prepared
                    && matches!(
                        a.phase,
                        AllocationPhase::Reserved | AllocationPhase::Started
                    )
                    && a.request.instance == target.instance_id
                    && a.request.agent == target.identity.agent_id
                    && a.request.scan == target.identity.scan_id
                    && a.request.leader == target.leader_id
                    && a.request.configuration == target.configuration_id
                    && a.request.configuration_revision == target.configuration_revision
            })
            .context("execution has no owning-host resource reservation")
    }
}
impl PreparedEnvironments for AllocatedDockerBackend {
    fn available(&self) -> bool {
        self.0.environments.available()
    }
    fn validate(&self, target: &ExecutionTarget) -> Result<(), BackendRejection> {
        self.reservation(target)
            .map_err(|_| BackendRejection::Conflict)?;
        DockerBackend(self.0.environments.clone()).validate(target)
    }
    fn spawn<'a>(
        &'a self,
        target: &'a ExecutionTarget,
        launch: &'a HostWorkerLaunch,
    ) -> ProcessFuture<'a, PreparedWorker> {
        Box::pin(async move {
            let owner = self.0.environments.lock(target.instance_id.parse()?)?;
            let reservation = self.reservation(target)?;
            self.0
                .store
                .bind_execution(&reservation.request.id, target)?;
            self.0.store.started(&reservation.request.id)?;
            DockerBackend(self.0.environments.clone())
                .spawn_owned(target, launch, owner)
                .await
        })
    }
    fn recover<'a>(&'a self, target: &'a ExecutionTarget) -> BackendFuture<'a> {
        Box::pin(async move {
            let result = DockerBackend(self.0.environments.clone())
                .recover(target)
                .await?;
            if result == BackendObservation::Stopped
                && let Ok(reservation) = self.reservation(target)
            {
                self.0
                    .store
                    .confirm_stopped(&reservation.request.id, false)?;
            }
            Ok(result)
        })
    }
}
impl PreparedEnvironments for DockerBackend {
    fn available(&self) -> bool {
        self.0.available()
    }
    fn validate(&self, target: &ExecutionTarget) -> Result<(), BackendRejection> {
        self.0
            .store
            .validate_target(target)
            .map(|_| ())
            .map_err(|_| BackendRejection::Conflict)
    }
    fn spawn<'a>(
        &'a self,
        target: &'a ExecutionTarget,
        launch: &'a HostWorkerLaunch,
    ) -> ProcessFuture<'a, PreparedWorker> {
        Box::pin(async move {
            let owner = self.0.lock(target.instance_id.parse()?)?;
            self.spawn_owned(target, launch, owner).await
        })
    }
    fn recover<'a>(&'a self, target: &'a ExecutionTarget) -> BackendFuture<'a> {
        Box::pin(async move {
            let Ok(_owner) = self.0.lock(target.instance_id.parse()?) else {
                return Ok(BackendObservation::TerminationUnconfirmed);
            };
            let mut row = self.0.store.validate_target(target)?;
            // Cancellation can win before spawn is ever polled. Validation
            // permits a later attempt only after the prior owner was stopped;
            // claim that same logical agent's instance before fresh observation.
            if row.target.as_ref() != Some(target) {
                row.target = Some(target.clone());
                row.stopped = false;
                self.0.store.save_instance(&row)?;
            }
            let handle = row.handle.as_ref().context("retained container missing")?;
            if self.0.docker.stop(handle).await.unwrap_or(false) {
                row.stopped = true;
                self.0.store.save_instance(&row)?;
                Ok(BackendObservation::Stopped)
            } else {
                Ok(BackendObservation::TerminationUnconfirmed)
            }
        })
    }
}
impl DockerBackend {
    async fn spawn_owned(
        &self,
        target: &ExecutionTarget,
        launch: &HostWorkerLaunch,
        owner: File,
    ) -> Result<PreparedWorker> {
        let mut row = self.0.store.validate_target(target)?;
        ensure!(row.stopped, "container termination is unconfirmed");
        let handle = row.handle.clone().context("retained container missing")?;
        row.target = Some(target.clone());
        row.stopped = false;
        self.0.store.save_instance(&row)?;
        let readiness = self.0.store.begin_preflight(target)?;
        let workspace = "/beale-workspace/workspace".into();
        let protected_root =
            std::path::PathBuf::from(format!("{}/state", handle.runtime_directory));
        // These paths are in the Linux guest namespace even on Windows.
        let recovery = std::path::PathBuf::from(format!(
            "{}/state/{}",
            handle.runtime_directory, target.identity.attempt_id
        ));
        // Inside the container, loopback is the container itself; a loopback
        // leader is reached through the container host alias instead.
        let leader = beale_contracts::swarm_host::container_leader_origin(&launch.leader_origin)
            .map_err(anyhow::Error::msg)?;
        let bootstrap = ContainerPreparation {
            launch: super::WorkerLaunch {
                version: 1,
                identity: target.identity.clone(),
                leader,
                bearer: launch.worker_bearer.clone(),
                workspace,
                protected_root,
                recovery,
            },
            snapshot: launch.snapshot.clone(),
        };
        // Every readiness check shares the same guarded failure receipt,
        // including checks before the guest workspace can be reached.
        // Interrupted execution still requires stopping this exact instance.
        let preflight = async {
            ensure!(
                self.0.docker.inspect(&handle).await? == ContainerStatus::Stopped,
                "retained container is not stopped"
            );
            self.0.docker.start(&handle).await?;
            self.0.docker.probe(&handle).await?;
            self.0
                .docker
                .prepare_workspace(&handle, &serde_json::to_vec(&bootstrap)?)
                .await
        }
        .await;
        if let Err(error) = preflight {
            if let Some(observation) = &readiness {
                self.0
                    .store
                    .complete_preflight(observation, Some(&error.to_string()))?;
            }
            return Err(error);
        }
        if let Some(observation) = &readiness {
            self.0.store.complete_preflight(observation, None)?;
        }
        let child = self
            .0
            .docker
            .worker_command(&handle, "container-worker")?
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let cli_tree = beale_tools::process::ProcessTree::attach_required(&child)?;
        let process = DockerWorker {
            environments: self.0.clone(),
            row,
            child,
            released: false,
            stopping: false,
            owner: Some(owner),
            stop_task: None,
            _cli_tree: cli_tree,
        };
        Ok(PreparedWorker {
            process: Box::new(process),
            workspace: bootstrap.launch.workspace,
            protected_root: bootstrap.launch.protected_root,
            recovery: bootstrap.launch.recovery,
        })
    }
}
struct DockerWorker {
    environments: DockerEnvironments,
    row: Instance,
    child: Child,
    released: bool,
    stopping: bool,
    owner: Option<File>,
    stop_task: Option<tokio::task::JoinHandle<bool>>,
    _cli_tree: beale_tools::process::ProcessTree,
}
impl ManagedWorker for DockerWorker {
    fn release_startup<'a>(&'a mut self, bytes: &'a [u8]) -> ProcessFuture<'a, ()> {
        Box::pin(async move {
            ensure!(
                !self.released
                    && !self.stopping
                    && bytes.len() <= beale_contracts::swarms::MAX_DOCUMENT_BYTES,
                "invalid Docker startup release"
            );
            self.released = true;
            let mut stdin = self
                .child
                .stdin
                .take()
                .context("Docker worker stdin unavailable")?;
            tokio::time::timeout(Duration::from_secs(5), async {
                stdin.write_all(bytes).await?;
                stdin.shutdown().await
            })
            .await??;
            Ok(())
        })
    }
    fn root_exited(&mut self) -> Result<bool> {
        Ok(self.child.try_wait()?.is_some())
    }
    fn request_stop(&mut self) {
        self.stopping = true;
        self.child.stdin.take();
        let _ = self.child.start_kill();
    }
    fn stop(&mut self, timeout: Duration) -> ProcessFuture<'_, bool> {
        Box::pin(async move {
            self.request_stop();
            if self.row.stopped {
                return Ok(true);
            }
            if self.stop_task.is_none() {
                let handle = self
                    .row
                    .handle
                    .clone()
                    .context("retained container missing")?;
                let docker = self.environments.docker.clone();
                self.stop_task = Some(tokio::spawn(async move {
                    docker.stop(&handle).await.unwrap_or(false)
                }));
            }
            // Maintenance's short observation budget must not repeatedly
            // cancel a longer daemon stop/inspect operation before it finishes.
            let task = self
                .stop_task
                .as_mut()
                .context("Docker stop observation unavailable")?;
            let confirmed = match tokio::time::timeout(timeout, task).await {
                Ok(result) => {
                    self.stop_task = None;
                    result.unwrap_or(false)
                }
                Err(_) => return Ok(false),
            };
            if confirmed {
                let mut stopped = self.row.clone();
                stopped.stopped = true;
                self.environments.store.save_instance(&stopped)?;
                self.row = stopped;
            }
            Ok(confirmed)
        })
    }
}
impl Drop for DockerWorker {
    fn drop(&mut self) {
        self.request_stop();
        if self.row.stopped {
            return;
        }
        // Keep the instance lock until best-effort shutdown finishes. Durable
        // restart recovery still requires a fresh daemon observation.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let owner = self.owner.take();
            let environments = self.environments.clone();
            let mut row = self.row.clone();
            let stop_task = self.stop_task.take();
            runtime.spawn(async move {
                let _owner = owner;
                if let Some(task) = stop_task {
                    let _ = task.await;
                }
                if let Some(handle) = &row.handle
                    && environments.docker.stop(handle).await.unwrap_or(false)
                {
                    row.stopped = true;
                    let _ = environments.store.save_instance(&row);
                }
            });
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainerPreparation {
    pub launch: super::WorkerLaunch,
    pub snapshot: Option<String>,
}

/// Runs inside the Linux guest before any inference. The initial transfer is
/// verified in the destination filesystem, including executable/link metadata.
pub async fn prepare_container(input: ContainerPreparation) -> Result<()> {
    use beale_contracts::swarm_container::PreparationFailure;
    let launch = input.launch;
    ensure!(
        launch.version == 1,
        "unsupported container preparation version"
    );
    launch.identity.validate().map_err(anyhow::Error::msg)?;
    let runtime = std::env::current_exe()?
        .parent()
        .context("runtime directory unavailable")?
        .to_path_buf();
    ensure!(
        launch.workspace == std::path::Path::new("/beale-workspace/workspace")
            && launch.protected_root == runtime.join("state")
            && launch.recovery == launch.protected_root.join(&launch.identity.attempt_id),
        "invalid container storage layout"
    );
    std::fs::create_dir_all(&launch.protected_root)?;
    if input.snapshot.is_some() {
        super::peer_workspace::prepare_workspace(
            &HostWorkerLaunch {
                leader_origin: launch.leader.clone(),
                worker_bearer: launch.bearer.clone(),
                snapshot: input.snapshot,
            },
            &launch.identity,
            &launch.protected_root.join("initial-snapshot"),
            &launch.workspace,
        )
        .await
        .context(PreparationFailure::Snapshot)?;
    } else {
        beale_storage::swarm_files::SnapshotReceiver::open(
            &launch.protected_root.join("initial-snapshot"),
        )?
        .verify_retained(&launch.identity, &launch.workspace)
        .context(PreparationFailure::RetainedStorage)?;
    }
    std::fs::create_dir_all(&launch.recovery)?;
    super::worker::probe_artifact_transfer(&launch)
        .await
        .context(PreparationFailure::ArtifactTransfer)
}
