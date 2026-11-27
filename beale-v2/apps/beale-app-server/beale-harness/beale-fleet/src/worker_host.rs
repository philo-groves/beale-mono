//! Host-control execution routing. Worker tokens cannot reach this audience.
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::get,
};
use beale_contracts::swarm_host::*;
use beale_storage::{
    HostDatabase,
    fleet_executions::{BackendObservation, FleetExecutionStore},
};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions, TryLockError},
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    time::Duration,
};

mod client;
pub use client::{HostExecutionClient, PeerExecutionError, PeerFailure};

pub type BackendFuture<'a> =
    Pin<Box<dyn Future<Output = anyhow::Result<BackendObservation>> + Send + 'a>>;
#[derive(Clone, Copy, Debug)]
pub enum BackendRejection {
    Unsupported,
    Unavailable,
    Conflict,
}
fn storage_error(error: anyhow::Error) -> StatusCode {
    if error
        .downcast_ref::<beale_storage::fleet_executions::ExecutionConflict>()
        .is_some()
    {
        StatusCode::CONFLICT
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

/// The backend resolves IDs to host-owned paths and executable configuration.
/// It must persist ownership before releasing a worker's startup handshake.
/// `launch` is called once per durable claim; inspect/stop must safely reconcile
/// an interrupted launch without creating replacement research execution.
pub trait HostExecutionBackend: Send + Sync {
    fn available(&self) -> bool;
    fn validate(&self, target: &ExecutionTarget) -> Result<(), BackendRejection>;
    fn launch<'a>(
        &'a self,
        target: &'a ExecutionTarget,
        launch: &'a HostWorkerLaunch,
    ) -> BackendFuture<'a>;
    fn inspect<'a>(&'a self, target: &'a ExecutionTarget) -> BackendFuture<'a>;
    fn stop<'a>(&'a self, target: &'a ExecutionTarget) -> BackendFuture<'a>;
}

/// Until a destination backend is installed, selected placement fails explicitly.
/// There is deliberately no local-process fallback for selected environments.
pub struct UnavailableBackend;
impl HostExecutionBackend for UnavailableBackend {
    fn available(&self) -> bool {
        false
    }
    fn validate(&self, _: &ExecutionTarget) -> Result<(), BackendRejection> {
        Err(BackendRejection::Unsupported)
    }
    fn launch<'a>(&'a self, _: &'a ExecutionTarget, _: &'a HostWorkerLaunch) -> BackendFuture<'a> {
        Box::pin(async { anyhow::bail!("execution backend unavailable") })
    }
    fn inspect<'a>(&'a self, _: &'a ExecutionTarget) -> BackendFuture<'a> {
        Box::pin(async { Ok(BackendObservation::TerminationUnconfirmed) })
    }
    fn stop<'a>(&'a self, target: &'a ExecutionTarget) -> BackendFuture<'a> {
        self.inspect(target)
    }
}

#[derive(Clone)]
struct HostState {
    token: String,
    db: HostDatabase,
    root: PathBuf,
    backend: Arc<dyn HostExecutionBackend>,
}

pub fn router(
    token: String,
    db: HostDatabase,
    root: PathBuf,
    backend: Arc<dyn HostExecutionBackend>,
) -> Router {
    Router::new()
        .route(HOST_EXECUTION_PATH, get(descriptor).post(dispatch))
        .layer(DefaultBodyLimit::max(HOST_EXECUTION_MAX_BYTES))
        .with_state(HostState {
            token,
            db,
            root,
            backend,
        })
}

/// Resume observation after app-server restart without redispatching launch.
/// Backend handles and process evidence remain owned by the destination host.
pub fn start_recovery(
    db: HostDatabase,
    root: PathBuf,
    backend: Arc<dyn HostExecutionBackend>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut cursor = None;
        loop {
            if let Err(error) = recover_page(&db, &root, backend.clone(), &mut cursor).await {
                beale_host_log::error!("Fleet execution recovery remains pending: {error:#}");
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    })
}

pub async fn recover_page(
    db: &HostDatabase,
    root: &std::path::Path,
    backend: Arc<dyn HostExecutionBackend>,
    cursor: &mut Option<beale_storage::fleet_executions::ExecutionCursor>,
) -> anyhow::Result<()> {
    let store = FleetExecutionStore::open(db.clone())?;
    let pending = store.pending_page(cursor.as_ref())?;
    let state = HostState {
        token: String::new(),
        db: db.clone(),
        root: root.to_path_buf(),
        backend,
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    for key in &pending {
        if tokio::time::Instant::now() >= deadline {
            return Ok(());
        }
        *cursor = Some(key.clone());
        let report = match store.pending(key) {
            Ok(report) => report,
            Err(error) => {
                beale_host_log::error!("Fleet execution record cannot be recovered: {error:#}");
                continue;
            }
        };
        let _owner = match lock(&state, &report.target) {
            Ok(Some(owner)) => owner,
            Ok(None) => continue,
            Err(error) => {
                beale_host_log::error!("Fleet execution ownership unavailable: {error:#}");
                continue;
            }
        };
        if report.phase == HostExecutionPhase::Stopped {
            continue;
        }
        let task = if report.stop_requested {
            state.backend.stop(&report.target)
        } else {
            state.backend.inspect(&report.target)
        };
        let observation = match tokio::time::timeout_at(deadline, task).await {
            Ok(Ok(value)) => value,
            _ => BackendObservation::TerminationUnconfirmed,
        };
        if let Err(error) = store.observe(&report.target, report.revision, observation) {
            beale_host_log::error!("Fleet execution observation remains pending: {error:#}");
        }
    }
    if pending.len() < 64 {
        *cursor = None;
    }
    Ok(())
}

fn store(state: &HostState, headers: &HeaderMap) -> Result<FleetExecutionStore, StatusCode> {
    if !crate::http::authorized(headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    FleetExecutionStore::open(state.db.clone()).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn descriptor(
    State(state): State<HostState>,
    headers: HeaderMap,
) -> Result<Json<HostExecutionDescriptor>, StatusCode> {
    let store = store(&state, &headers)?;
    Ok(Json(HostExecutionDescriptor {
        version: HOST_EXECUTION_VERSION,
        host_id: store
            .host_id()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        execution_available: state.backend.available(),
    }))
}

pub struct ExecutionOwner(File);
impl Drop for ExecutionOwner {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// Held for the complete supervisor lifetime, separately from short control
/// request serialization. Recovery must not stop another live host's worker.
pub fn lock_worker_supervisor(
    root: &std::path::Path,
    target: &ExecutionTarget,
) -> anyhow::Result<Option<ExecutionOwner>> {
    lock_in(&root.join("fleet-worker-supervisors"), target)
}

fn lock(state: &HostState, target: &ExecutionTarget) -> anyhow::Result<Option<ExecutionOwner>> {
    lock_in(&state.root.join("fleet-execution-owners"), target)
}

fn lock_in(
    root: &std::path::Path,
    target: &ExecutionTarget,
) -> anyhow::Result<Option<ExecutionOwner>> {
    target.validate().map_err(anyhow::Error::msg)?;
    std::fs::create_dir_all(root)?;
    anyhow::ensure!(
        std::fs::symlink_metadata(root)?.file_type().is_dir(),
        "invalid execution ownership root"
    );
    let key = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(
            &target.leader_id,
            &target.identity.attempt_id
        ))?)
    );
    let path = root.join(format!("{key}.lock"));
    if path.try_exists()? {
        anyhow::ensure!(
            std::fs::symlink_metadata(&path)?.file_type().is_file(),
            "invalid execution ownership lock"
        );
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(ExecutionOwner(file))),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error.into()),
    }
}

async fn dispatch(
    State(state): State<HostState>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Result<Json<HostExecutionReply>, StatusCode> {
    let store = store(&state, &headers)?;
    if headers
        .get("content-type")
        .and_then(|s| s.to_str().ok())
        .and_then(|s| s.split(';').next())
        != Some("application/json")
    {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    let request: HostExecutionRequest =
        serde_json::from_slice(&bytes).map_err(|_| StatusCode::BAD_REQUEST)?;
    request.validate().map_err(|_| StatusCode::BAD_REQUEST)?;
    let existing = store.inspect(&request.target).map_err(storage_error)?;
    if existing.is_none() && matches!(request.operation, HostExecutionOperation::Inspect {}) {
        return Err(StatusCode::NOT_FOUND);
    }
    if matches!(request.operation, HostExecutionOperation::Start { .. }) && existing.is_none() {
        state
            .backend
            .validate(&request.target)
            .map_err(|error| match error {
                BackendRejection::Unsupported => StatusCode::NOT_IMPLEMENTED,
                BackendRejection::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
                BackendRejection::Conflict => StatusCode::CONFLICT,
            })?;
    }
    // A stop is durable even while another request owns backend startup. A
    // fresh start must own the lock before receiving its one-time launch grant.
    let stopping = matches!(request.operation, HostExecutionOperation::Stop {});
    let accepted_stop = if stopping {
        Some(store.accept(&request).map_err(storage_error)?)
    } else {
        None
    };
    let owner = lock(&state, &request.target).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let accepted = if let Some(accepted) = accepted_stop {
        accepted
    } else {
        if owner.is_none() {
            return Err(StatusCode::LOCKED);
        }
        store.accept(&request).map_err(storage_error)?
    };
    let mut report = accepted.report;
    if owner.is_some() && report.phase != HostExecutionPhase::Stopped {
        let task = if report.stop_requested {
            state.backend.stop(&request.target)
        } else if accepted.launch {
            let HostExecutionOperation::Start { launch } = &request.operation else {
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            };
            state.backend.launch(&request.target, launch)
        } else {
            state.backend.inspect(&request.target)
        };
        let observation = match tokio::time::timeout(Duration::from_secs(10), task).await {
            Ok(Ok(observation)) => observation,
            _ => BackendObservation::TerminationUnconfirmed,
        };
        report = store
            .observe(&request.target, report.revision, observation)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        // Stop may have committed while launch was in progress. Honor it before
        // returning; stale running observations cannot clear the stop intent.
        if report.stop_requested && report.phase != HostExecutionPhase::Stopped && !stopping {
            let observation = match tokio::time::timeout(
                Duration::from_secs(10),
                state.backend.stop(&request.target),
            )
            .await
            {
                Ok(Ok(value)) => value,
                _ => BackendObservation::TerminationUnconfirmed,
            };
            report = store
                .observe(&request.target, report.revision, observation)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
    }
    Ok(Json(HostExecutionReply {
        version: HOST_EXECUTION_VERSION,
        operation_id: request.operation_id,
        report,
    }))
}

#[cfg(test)]
mod tests;
