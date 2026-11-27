//! Destination-host supervision over a prepared, backend-owned environment.
//! No host paths or executable configuration are taken from peer requests.
use super::{ManagedWorker, ProcessFuture, WorkerLaunch, WorkerTransport, supervise};
use anyhow::{Result, ensure};
use beale_contracts::swarm_host::{ExecutionTarget, HostWorkerLaunch};
use beale_fleet::worker_host::{
    BackendFuture, BackendRejection, ExecutionOwner, HostExecutionBackend, lock_worker_supervisor,
};
use beale_storage::fleet_executions::BackendObservation;
use std::{collections::HashMap, path::PathBuf, sync::Arc, sync::Mutex, time::Duration};
use tokio::{sync::watch, task::JoinHandle};

/// These paths are selected by the destination backend, in the worker's
/// filesystem namespace. They never travel in the host-control request.
pub struct PreparedWorker {
    pub process: Box<dyn ManagedWorker>,
    pub workspace: PathBuf,
    pub protected_root: PathBuf,
    pub recovery: PathBuf,
}

/// Resolves an already allocated instance and exact configuration revision.
/// Implementations must durably bind the complete target and process ownership
/// before returning a startup-gated worker. `spawn` must be cancellation safe:
/// its interrupted outcome is reconciled through `recover`, never relaunched.
/// `recover` fences execution and returns Stopped only with durable proof that
/// all managed execution has ended. Missing/corrupt ownership remains unknown.
/// This interface has no default local fallback for a selected container/VM.
pub trait PreparedEnvironments: Send + Sync {
    fn available(&self) -> bool;
    fn validate(&self, target: &ExecutionTarget) -> Result<(), BackendRejection>;
    fn spawn<'a>(
        &'a self,
        target: &'a ExecutionTarget,
        launch: &'a HostWorkerLaunch,
    ) -> ProcessFuture<'a, PreparedWorker>;
    fn recover<'a>(&'a self, target: &'a ExecutionTarget) -> BackendFuture<'a>;
}

struct Active {
    target: ExecutionTarget,
    cancel: watch::Sender<bool>,
    observation: watch::Sender<BackendObservation>,
    retained: tokio::sync::Mutex<Option<Box<dyn ManagedWorker>>>,
    task: Mutex<Option<JoinHandle<()>>>,
    _owner: ExecutionOwner,
}
struct Recovery {
    target: ExecutionTarget,
    result: watch::Receiver<Option<BackendObservation>>,
}

/// The host retains this backend independently of each HTTP request. The Fleet
/// store grants launch once; retries and restart maintenance only inspect/stop.
/// Production installs it only with an actual destination environment backend.
pub struct AssignedWorkerBackend {
    environments: Arc<dyn PreparedEnvironments>,
    root: PathBuf,
    active: Mutex<HashMap<(String, String), Arc<Active>>>,
    recovering: Mutex<HashMap<(String, String), Arc<Recovery>>>,
}

impl AssignedWorkerBackend {
    pub fn new(root: PathBuf, environments: Arc<dyn PreparedEnvironments>) -> Self {
        Self {
            environments,
            root,
            active: Mutex::new(HashMap::new()),
            recovering: Mutex::new(HashMap::new()),
        }
    }

    fn active(&self, target: &ExecutionTarget) -> Result<Option<Arc<Active>>> {
        target.validate().map_err(anyhow::Error::msg)?;
        let active = self
            .active
            .lock()
            .map_err(|_| anyhow::anyhow!("worker ownership unavailable"))?
            .get(&(target.leader_id.clone(), target.identity.attempt_id.clone()))
            .cloned();
        if let Some(active) = &active {
            ensure!(active.target == *target, "worker ownership mismatch");
        }
        Ok(active)
    }

    async fn observe(&self, target: &ExecutionTarget, stop: bool) -> Result<BackendObservation> {
        let Some(active) = self.active(target)? else {
            // A new host never resumes an old loop or infers termination from
            // absence in memory. The backend must reconcile its durable owner.
            return self.recover_persistently(target, None).await;
        };
        if stop {
            active.cancel.send_replace(true);
        }
        let observation = *active.observation.borrow();
        if observation == BackendObservation::Stopped {
            return Ok(observation);
        }
        let finished = active
            .task
            .lock()
            .map_err(|_| anyhow::anyhow!("worker supervision unavailable"))?
            .as_ref()
            .is_some_and(JoinHandle::is_finished);
        if !finished {
            return Ok(if stop {
                BackendObservation::TerminationUnconfirmed
            } else {
                observation
            });
        }
        // The task may have panicked or lost process observation. Keep the
        // handle while confirmation is unavailable; do not authorize reuse.
        let mut retained = active.retained.lock().await;
        let observation = if let Some(process) = retained.as_mut() {
            if process.stop(Duration::from_millis(250)).await? {
                *retained = None;
                BackendObservation::Stopped
            } else {
                BackendObservation::TerminationUnconfirmed
            }
        } else {
            self.recover_persistently(target, Some(active.clone()))
                .await?
        };
        active.observation.send_replace(observation);
        Ok(observation)
    }
    async fn recover_persistently(
        &self,
        target: &ExecutionTarget,
        active: Option<Arc<Active>>,
    ) -> Result<BackendObservation> {
        let key = (target.leader_id.clone(), target.identity.attempt_id.clone());
        let recovery = {
            let mut tasks = self
                .recovering
                .lock()
                .map_err(|_| anyhow::anyhow!("worker recovery unavailable"))?;
            if let Some(task) = tasks.get(&key) {
                ensure!(task.target == *target, "worker recovery ownership mismatch");
                task.clone()
            } else {
                let owner = if active.is_none() {
                    let Some(owner) = lock_worker_supervisor(&self.root, target)? else {
                        return Ok(BackendObservation::TerminationUnconfirmed);
                    };
                    Some(owner)
                } else {
                    None
                };
                let (result, receiver) = watch::channel(None);
                let task = Arc::new(Recovery {
                    target: target.clone(),
                    result: receiver,
                });
                tasks.insert(key.clone(), task.clone());
                let environments = self.environments.clone();
                let target = target.clone();
                // The bounded backend operation and its owner outlive short
                // HTTP/maintenance polls. Dropping a poll cannot cancel stop.
                tokio::spawn(async move {
                    let _owner = owner;
                    let _active = active;
                    let observation = recover(environments.as_ref(), &target)
                        .await
                        .unwrap_or(BackendObservation::TerminationUnconfirmed);
                    result.send_replace(Some(observation));
                });
                task
            }
        };
        let mut receiver = recovery.result.clone();
        let observation = loop {
            if let Some(observation) = *receiver.borrow_and_update() {
                break observation;
            }
            if receiver.changed().await.is_err() {
                break BackendObservation::TerminationUnconfirmed;
            }
        };
        let mut tasks = self
            .recovering
            .lock()
            .map_err(|_| anyhow::anyhow!("worker recovery unavailable"))?;
        if tasks
            .get(&key)
            .is_some_and(|task| Arc::ptr_eq(task, &recovery))
        {
            tasks.remove(&key);
        }
        Ok(observation)
    }
}

async fn recover(
    environments: &dyn PreparedEnvironments,
    target: &ExecutionTarget,
) -> Result<BackendObservation> {
    // An unsupervised process being alive cannot be reported as a healthy run.
    Ok(match environments.recover(target).await? {
        BackendObservation::Stopped => BackendObservation::Stopped,
        _ => BackendObservation::TerminationUnconfirmed,
    })
}

impl HostExecutionBackend for AssignedWorkerBackend {
    fn available(&self) -> bool {
        self.environments.available()
    }
    fn validate(&self, target: &ExecutionTarget) -> Result<(), BackendRejection> {
        target.validate().map_err(|_| BackendRejection::Conflict)?;
        self.environments.validate(target)
    }
    fn launch<'a>(
        &'a self,
        target: &'a ExecutionTarget,
        launch: &'a HostWorkerLaunch,
    ) -> BackendFuture<'a> {
        Box::pin(async move {
            target.validate().map_err(anyhow::Error::msg)?;
            launch.validate().map_err(anyhow::Error::msg)?;
            self.environments
                .validate(target)
                .map_err(|_| anyhow::anyhow!("prepared environment unavailable"))?;
            let mut entries = self
                .active
                .lock()
                .map_err(|_| anyhow::anyhow!("worker ownership unavailable"))?;
            let key = (target.leader_id.clone(), target.identity.attempt_id.clone());
            if let Some(active) = entries.get(&key) {
                ensure!(active.target == *target, "worker ownership mismatch");
                return Ok(*active.observation.borrow());
            }
            let owner = lock_worker_supervisor(&self.root, target)?
                .ok_or_else(|| anyhow::anyhow!("worker already owned by another supervisor"))?;
            let (cancel, canceled) = watch::channel(false);
            let (observation, _) = watch::channel(BackendObservation::TerminationUnconfirmed);
            let active = Arc::new(Active {
                target: target.clone(),
                cancel,
                observation,
                retained: tokio::sync::Mutex::new(None),
                task: Mutex::new(None),
                _owner: owner,
            });
            entries.insert(key, active.clone());
            let owned = active.clone();
            let environments = self.environments.clone();
            let launch = launch.clone();
            // No await between registering the owner and creating the task.
            // Dropped HTTP futures cannot lose supervision or cause relaunch.
            let task = tokio::spawn(async move {
                run(environments, owned, launch, canceled).await;
            });
            *active
                .task
                .lock()
                .map_err(|_| anyhow::anyhow!("worker supervision unavailable"))? = Some(task);
            Ok(*active.observation.borrow())
        })
    }
    fn inspect<'a>(&'a self, target: &'a ExecutionTarget) -> BackendFuture<'a> {
        Box::pin(self.observe(target, false))
    }
    fn stop<'a>(&'a self, target: &'a ExecutionTarget) -> BackendFuture<'a> {
        Box::pin(self.observe(target, true))
    }
}

impl Drop for AssignedWorkerBackend {
    fn drop(&mut self) {
        // Detached tasks finish bounded cleanup; dropping their JoinHandles
        // must not abort process-tree confirmation halfway through.
        let entries = self.active.get_mut().unwrap_or_else(|e| e.into_inner());
        for active in entries.values() {
            active.cancel.send_replace(true);
        }
    }
}

async fn run(
    environments: Arc<dyn PreparedEnvironments>,
    active: Arc<Active>,
    launch: HostWorkerLaunch,
    mut cancel: watch::Receiver<bool>,
) {
    let target = &active.target;
    let spawned = if *cancel.borrow() {
        Err(anyhow::anyhow!("worker canceled before preparation"))
    } else {
        tokio::select! {
            biased;
            _ = cancel.changed() => Err(anyhow::anyhow!("worker preparation canceled")),
            result = tokio::time::timeout(Duration::from_secs(300), environments.spawn(target, &launch)) => {
                result.unwrap_or_else(|_| Err(anyhow::anyhow!("worker preparation timed out")))
            }
        }
    };
    let mut worker = match spawned {
        Ok(worker) => worker,
        Err(_) => {
            let observation = tokio::time::timeout(
                Duration::from_secs(5),
                recover(environments.as_ref(), target),
            )
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or(BackendObservation::TerminationUnconfirmed);
            active.observation.send_replace(observation);
            return;
        }
    };
    let launch = WorkerLaunch {
        version: 1,
        identity: target.identity.clone(),
        leader: launch.leader_origin,
        bearer: launch.worker_bearer,
        workspace: worker.workspace,
        protected_root: worker.protected_root,
        recovery: worker.recovery,
    };
    let prepared = WorkerTransport::new(&launch.leader, &launch.bearer)
        .map_err(anyhow::Error::from)
        .and_then(|transport| Ok((transport, serde_json::to_vec(&launch)?)));
    let confirmed = match prepared {
        Ok((transport, bytes)) => {
            active.observation.send_replace(BackendObservation::Running);
            supervise(
                worker.process.as_mut(),
                target.identity.clone(),
                &transport,
                &bytes,
                cancel,
            )
            .await
            .is_ok_and(|report| report.termination_confirmed)
        }
        Err(_) => worker
            .process
            .stop(Duration::from_secs(5))
            .await
            .unwrap_or(false),
    };
    if confirmed {
        active.observation.send_replace(BackendObservation::Stopped);
    } else {
        *active.retained.lock().await = Some(worker.process);
        active
            .observation
            .send_replace(BackendObservation::TerminationUnconfirmed);
    }
}

#[cfg(test)]
#[path = "peer_backend_tests.rs"]
mod tests;
