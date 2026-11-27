//! Same-host placement uses the assigned worker protocol and a managed process.
//! It is available only when the scan has no selected Fleet destinations.
use super::{WorkerLaunch, WorkerProcess, prepare_worker, supervisor};
use crate::{SessionState, research_files, tool_runtime};
use anyhow::{Result, ensure};
use beale_contracts::{swarm_worker::WorkerShell, swarms::*};
use beale_storage::swarm_state::{LocalTurn, SwarmStore, WorkerDisposition, WorkerReconciliation};
use std::ffi::OsString;
use tokio::{net::TcpListener, sync::watch, task::JoinHandle};

// Retained by SessionState, beyond a single client request or queue-drain pass.
// Failed reconciliation retains the process-tree observation handle for retry.
pub(crate) struct WorkerRecovery {
    process: WorkerProcess,
    launch: WorkerLaunch,
    report: super::SupervisionReport,
    record: beale_storage::swarm_state::LocalWorkerRecord,
    _owner: std::fs::File,
}

pub(crate) struct LocalWorkerHost {
    state: SessionState,
    endpoint: String,
    server: JoinHandle<()>,
}

impl LocalWorkerHost {
    pub async fn start(state: SessionState) -> Result<Self> {
        // Only the scoped worker audience is exposed by this private listener.
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let app = super::gateway::router().with_state(state.clone());
        let server = tokio::spawn(async move {
            if let Err(error) = axum::serve(listener, app).await {
                beale_host_log::error!("Local worker gateway stopped: {error}");
            }
        });
        Ok(Self {
            state,
            endpoint,
            server,
        })
    }

    #[cfg(test)]
    pub async fn run(
        &self,
        turn: &LocalTurn,
        stop: Option<watch::Receiver<bool>>,
    ) -> Result<WorkerReconciliation> {
        let store = SwarmStore::open(self.state.db.clone())?;
        let saved = store.load(&turn.scan_id, true)?;
        ensure!(
            saved.config.destinations.is_empty(),
            "selected Fleet destinations cannot fall back to local execution"
        );
        ensure!(
            turn.role != Role::Leader,
            "leader uses the host coordinator"
        );
        let owner_key = uuid::Uuid::new_v4();
        let owner = super::restart::lock(&self.state, owner_key)?
            .ok_or_else(|| anyhow::anyhow!("local launch ownership unavailable"))?;
        store.register_local_worker(turn, owner_key)?;
        self.run_owned(turn, stop, owner_key, owner).await
    }

    pub(crate) async fn run_owned(
        &self,
        turn: &LocalTurn,
        stop: Option<watch::Receiver<bool>>,
        owner_key: uuid::Uuid,
        owner: std::fs::File,
    ) -> Result<WorkerReconciliation> {
        let store = SwarmStore::open(self.state.db.clone())?;
        let record = store.local_worker_record(&turn.attempt_id)?;
        ensure!(
            record.owner_key == owner_key
                && serde_json::to_value(&record.turn)? == serde_json::to_value(turn)?,
            "local launch does not own this claim"
        );
        ensure!(
            record.backend == beale_storage::swarm_state::local_worker_backend(),
            "local backend mismatch"
        );
        let saved = store.load(&turn.scan_id, true)?;
        ensure!(
            saved.config.destinations.is_empty() && turn.role != Role::Leader,
            "not a local subordinate launch"
        );
        #[cfg(not(test))]
        let arguments = vec![OsString::from("worker")];
        #[cfg(test)]
        let arguments = [
            "--ignored",
            "--exact",
            "distributed::gateway_tests::assigned_worker_process_fixture",
        ]
        .map(OsString::from)
        .to_vec();
        let program = std::env::current_exe()?;
        #[cfg(target_os = "linux")]
        let mut guardian = None;
        #[allow(unused_mut)]
        let mut preparation = || -> Result<WorkerLaunch> {
            let workspace =
                research_files::store(&self.state)?.prepare_agent(&turn.scan_id, &turn.agent_id)?;
            let root = self.state.workspaces.root();
            let recovery_root = root.join("swarm-worker-recovery");
            std::fs::create_dir_all(&recovery_root)?;
            #[cfg(target_os = "linux")]
            {
                // Persist the single-use supervisor identity before preparation
                // commits. A prepared attempt always has recoverable launch
                // metadata even if the host exits before spawning its helper.
                guardian = Some(beale_tools::linux_worker::LinuxWorker::prepare(
                    &root.join("swarm-linux-supervisors"),
                    owner_key,
                    &program,
                    &arguments,
                    &workspace,
                )?);
            }
            let registration = WorkerRegistration {
                version: VERSION,
                audience: WORKER_AUDIENCE.into(),
                capabilities: vec![
                    WorkerCapability::CanonicalRouting,
                    WorkerCapability::ArtifactTransfer,
                    WorkerCapability::DurableEvents,
                ],
            };
            let (bootstrap, credential) = prepare_worker(
                &self.state.db,
                turn,
                WorkerShell::native(),
                &registration,
                tool_runtime::unix_ms(),
            )?;
            Ok(WorkerLaunch {
                version: 1,
                identity: bootstrap.identity,
                leader: self.endpoint.clone(),
                bearer: credential.bearer().into(),
                workspace,
                protected_root: root.to_path_buf(),
                recovery: recovery_root.join(&turn.attempt_id),
            })
        };
        let launch = match preparation() {
            Ok(launch) => launch,
            Err(error) => {
                // Preparation commits atomically, and no process was started.
                // If preparation already existed, retain its ownership instead
                // of treating a duplicate launcher as proof of termination.
                if !store.worker_prepared(&turn.attempt_id)? {
                    store.reconcile_unlaunched_worker(&record)?;
                }
                return Err(error);
            }
        };
        #[cfg(not(target_os = "linux"))]
        let spawned =
            WorkerProcess::spawn_recoverable(&program, &arguments, &launch.workspace, owner_key)
                .await;
        #[cfg(target_os = "linux")]
        let spawned = guardian
            .ok_or_else(|| anyhow::anyhow!("missing Linux supervisor preparation"))
            .and_then(WorkerProcess::spawn_guarded);
        let mut process = match spawned {
            Ok(process) => process,
            Err(error) => {
                // The trusted stdin handshake has never been released.
                supervisor::reconcile_owned_attempt(
                    &store,
                    &launch.identity,
                    true,
                    WorkerDisposition::Failed,
                    tool_runtime::unix_ms(),
                )?;
                store.complete_local_worker_recovery(&record)?;
                return Err(error);
            }
        };
        let (_keep_alive, default_stop) = watch::channel(false);
        let mut observed = None;
        let mut result = supervisor::supervise_assignment_with_checkpoint(
            &mut process,
            &launch,
            &store,
            stop.unwrap_or(default_stop),
            &mut observed,
            |progress| research_files::checkpoint(&self.state, turn, &progress.context),
        )
        .await;
        if matches!(
            result,
            Ok(WorkerReconciliation::TurnAccepted | WorkerReconciliation::BranchStopped)
        ) && let Err(error) =
            super::recovery::publications(&self.state, &launch.identity, &launch.recovery)
        {
            result = Err(error);
        }
        if matches!(
            result,
            Ok(WorkerReconciliation::TurnAccepted | WorkerReconciliation::BranchStopped)
        ) && let Err(error) = store.complete_local_worker_recovery(&record)
        {
            result = Err(error);
        }
        if result.is_err() || matches!(result, Ok(WorkerReconciliation::TerminationUnconfirmed)) {
            let report = match observed {
                Some(report) => report,
                None => super::SupervisionReport {
                    reason: super::StopReason::StartupFailed,
                    termination_confirmed: process
                        .stop(std::time::Duration::from_secs(5))
                        .await
                        .unwrap_or(false),
                },
            };
            self.state
                .worker_recovery
                .lock()
                .map_err(|_| anyhow::anyhow!("worker recovery ownership lock failed"))?
                .push(WorkerRecovery {
                    process,
                    launch,
                    report,
                    record,
                    _owner: owner,
                });
        }
        result
    }

    pub fn recover(state: &SessionState, scan: &str) -> Result<()> {
        let mut coordinator_cursor = String::new();
        loop {
            coordinator_cursor =
                super::restart::recover_coordinators(state, Some(scan), &coordinator_cursor)?;
            if coordinator_cursor.is_empty() {
                break;
            }
        }
        Self::recover_retained(state, Some(scan))?;
        let mut cursor = String::new();
        loop {
            cursor = super::restart::recover_page(state, Some(scan), &cursor)?;
            if cursor.is_empty() {
                break;
            }
        }
        Ok(())
    }

    /// Retry a bounded batch of live retained owners, independently of model
    /// turns. Move handles out of the queue while doing IO; the owner lock stays
    /// held, so restart discovery and concurrent drains cannot claim them.
    pub(crate) fn recover_retained(state: &SessionState, scan: Option<&str>) -> Result<()> {
        let store = SwarmStore::open(state.db.clone())?;
        let batch = {
            let mut pending = state
                .worker_recovery
                .lock()
                .map_err(|_| anyhow::anyhow!("worker recovery ownership lock failed"))?;
            let mut batch = Vec::new();
            let mut index = 0;
            while index < pending.len() && batch.len() < 64 {
                if scan.is_none_or(|scan| pending[index].launch.identity.scan_id == scan) {
                    batch.push(pending.remove(index));
                } else {
                    index += 1;
                }
            }
            batch
        };
        let mut retained = Vec::new();
        for mut owner in batch {
            let mut reconcile = || -> Result<bool> {
                owner.process.request_stop();
                if !owner.process.confirm_stopped()? {
                    return Ok(false);
                }
                if !store.worker_reconciled(&owner.record.turn)? {
                    store.fence_attempt(
                        &owner.launch.identity.scan_id,
                        &owner.launch.identity.attempt_id,
                        false,
                    )?;
                    let progress = store.worker_progress(&owner.launch.identity.attempt_id)?;
                    research_files::checkpoint(state, &progress.turn, &progress.context)?;
                    let disposition = match owner.report.reason {
                        super::StopReason::Canceled => WorkerDisposition::Canceled,
                        super::StopReason::RootExited if progress.answer.is_some() => {
                            WorkerDisposition::Completed
                        }
                        _ => WorkerDisposition::Failed,
                    };
                    supervisor::reconcile_owned_attempt(
                        &store,
                        &owner.launch.identity,
                        true,
                        disposition,
                        tool_runtime::unix_ms(),
                    )?;
                }
                super::recovery::publications(
                    state,
                    &owner.launch.identity,
                    &owner.launch.recovery,
                )?;
                store.complete_local_worker_recovery(&owner.record)?;
                Ok(true)
            };
            match reconcile() {
                Ok(true) => (),
                outcome => {
                    if let Err(error) = outcome {
                        beale_host_log::error!("Retained worker reconciliation failed: {error:#}");
                    }
                    retained.push(owner);
                }
            }
        }
        // Failed owners rotate behind the next batch instead of starving it.
        state
            .worker_recovery
            .lock()
            .map_err(|_| anyhow::anyhow!("worker recovery ownership lock failed"))?
            .extend(retained);
        Ok(())
    }
}

impl Drop for LocalWorkerHost {
    fn drop(&mut self) {
        self.server.abort();
    }
}
