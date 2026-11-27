//! Heartbeats run independently of model turns and desktop connections. Only
//! the owning process supervisor can confirm managed execution termination.
use super::{Lease, LeaseState, ManagedWorker, TransportFailure, WorkerTransport};
use anyhow::Result;
use beale_contracts::{
    swarm_worker::{ExecutionIdentity, WorkerReply},
    swarms::{VERSION, WorkerOperation, WorkerRequest},
};
use std::{
    future::Future,
    time::{Duration, Instant},
};
use tokio::sync::watch;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    RootExited,
    Canceled,
    LeaseExpired,
    RecoveryExhausted,
    AuthorityRejected,
    InvalidReply,
    StartupFailed,
    ProcessObservationFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SupervisionReport {
    pub reason: StopReason,
    pub termination_confirmed: bool,
}

#[derive(Clone, Copy)]
struct Timing {
    heartbeat: Duration,
    request: Duration,
    retry: Duration,
}
const TIMING: Timing = Timing {
    heartbeat: Duration::from_secs(10),
    request: Duration::from_secs(5),
    retry: Duration::from_millis(250),
};

/// Own a prepared assignment through process termination and durable host
/// acceptance. Persistence retries never restart execution or repeat inference.
pub async fn supervise_assignment(
    process: &mut dyn ManagedWorker,
    launch: &super::WorkerLaunch,
    store: &beale_storage::swarm_state::SwarmStore,
    cancel: watch::Receiver<bool>,
) -> Result<beale_storage::swarm_state::WorkerReconciliation> {
    let mut observed = None;
    supervise_assignment_with_checkpoint(process, launch, store, cancel, &mut observed, |_| Ok(()))
        .await
}

/// The owner checkpoints its retained filesystem before the transaction that
/// releases execution. A failed checkpoint leaves the fenced attempt owned.
pub(super) async fn supervise_assignment_with_checkpoint(
    process: &mut dyn ManagedWorker,
    launch: &super::WorkerLaunch,
    store: &beale_storage::swarm_state::SwarmStore,
    cancel: watch::Receiver<bool>,
    observed: &mut Option<SupervisionReport>,
    mut checkpoint: impl FnMut(&beale_storage::swarm_state::WorkerProgress) -> Result<()>,
) -> Result<beale_storage::swarm_state::WorkerReconciliation> {
    use beale_storage::swarm_state::WorkerDisposition;
    anyhow::ensure!(
        store
            .worker_bootstrap(&launch.identity.attempt_id)?
            .identity
            == launch.identity,
        "prepared worker identity mismatch"
    );
    let transport = WorkerTransport::new(&launch.leader, &launch.bearer)?;
    let bytes = serde_json::to_vec(launch)?;
    let report = match supervise(process, launch.identity.clone(), &transport, &bytes, cancel).await
    {
        Ok(report) => report,
        Err(_) => SupervisionReport {
            reason: StopReason::StartupFailed,
            termination_confirmed: process.stop(Duration::from_secs(5)).await.unwrap_or(false),
        },
    };
    *observed = Some(report);
    let mut last = None;
    for retry in 0..3 {
        let mut reconcile = || {
            // Revoke first, including when later transcript persistence fails.
            store.fence_attempt(&launch.identity.scan_id, &launch.identity.attempt_id, false)?;
            let progress = store.worker_progress(&launch.identity.attempt_id)?;
            if report.termination_confirmed {
                checkpoint(&progress)?;
            }
            let disposition = match report.reason {
                StopReason::Canceled => WorkerDisposition::Canceled,
                StopReason::RootExited if progress.answer.is_some() => WorkerDisposition::Completed,
                _ => WorkerDisposition::Failed,
            };
            reconcile_owned_attempt(
                store,
                &launch.identity,
                report.termination_confirmed,
                disposition,
                crate::tool_runtime::unix_ms(),
            )
        };
        match reconcile() {
            Ok(result) => return Ok(result),
            Err(error) => last = Some(error),
        }
        if retry < 2 {
            tokio::time::sleep(Duration::from_millis(250) * (1 << retry)).await;
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("worker reconciliation failed")))
}

#[cfg(feature = "runbooks")]
struct RunbookReconciliation<'a>(&'a ExecutionIdentity);
#[cfg(feature = "runbooks")]
impl beale_storage::TurnExtension for RunbookReconciliation<'_> {
    fn commit(
        &self,
        tx: &beale_storage::rusqlite::Transaction<'_>,
        _: &beale_storage::CommittedTurn<'_>,
    ) -> Result<()> {
        beale_runbooks::assigned::interrupt_attempt(tx, self.0)
    }
}

#[cfg(feature = "findings")]
struct FindingsReconciliation<'a>(&'a ExecutionIdentity);
#[cfg(feature = "findings")]
impl beale_storage::TurnExtension for FindingsReconciliation<'_> {
    fn commit(
        &self,
        tx: &beale_storage::rusqlite::Transaction<'_>,
        _: &beale_storage::CommittedTurn<'_>,
    ) -> Result<()> {
        beale_findings::assigned::interrupt_attempt(tx, self.0)
    }
}

pub(super) fn reconcile_owned_attempt(
    store: &beale_storage::swarm_state::SwarmStore,
    identity: &ExecutionIdentity,
    termination_confirmed: bool,
    disposition: beale_storage::swarm_state::WorkerDisposition,
    now_ms: i64,
) -> Result<beale_storage::swarm_state::WorkerReconciliation> {
    anyhow::ensure!(
        store.worker_bootstrap(&identity.attempt_id)?.identity == *identity,
        "reconciliation identity mismatch"
    );
    #[cfg(feature = "runbooks")]
    let runbooks = RunbookReconciliation(identity);
    #[cfg(feature = "findings")]
    let findings = FindingsReconciliation(identity);
    let extensions: &[&dyn beale_storage::TurnExtension] = &[
        #[cfg(feature = "findings")]
        &findings,
        #[cfg(feature = "runbooks")]
        &runbooks,
    ];
    store.reconcile_worker_with_extensions(
        &identity.attempt_id,
        termination_confirmed,
        disposition,
        now_ms,
        extensions,
    )
}

/// Starts the trusted worker only after a fresh lease, then monitors it without
/// a model turn. The caller retains the process on an unconfirmed stop and must
/// persist fencing before it considers any replacement. Canceling this future
/// requests termination but does not manufacture a successful report.
pub async fn supervise(
    process: &mut dyn ManagedWorker,
    identity: ExecutionIdentity,
    transport: &WorkerTransport,
    bootstrap: &[u8],
    cancel: watch::Receiver<bool>,
) -> Result<SupervisionReport> {
    supervise_with_timing(process, identity, transport, bootstrap, cancel, TIMING).await
}

async fn supervise_with_timing(
    process: &mut dyn ManagedWorker,
    identity: ExecutionIdentity,
    transport: &WorkerTransport,
    bootstrap: &[u8],
    mut cancel: watch::Receiver<bool>,
    timing: Timing,
) -> Result<SupervisionReport> {
    // Establish cleanup before validating launch data, including early errors.
    let guard = SupervisionGuard(process);
    let mut lease = Lease::new(identity.clone())?;
    let reason = run(
        guard.0,
        &mut lease,
        &identity,
        transport,
        bootstrap,
        &mut cancel,
        timing,
    )
    .await;
    lease.stop();
    let termination_confirmed = guard.0.stop(Duration::from_secs(5)).await.unwrap_or(false);
    if termination_confirmed {
        lease.confirm_termination()?;
    }
    Ok(SupervisionReport {
        reason,
        termination_confirmed,
    })
}

struct SupervisionGuard<'a>(&'a mut dyn ManagedWorker);
impl Drop for SupervisionGuard<'_> {
    fn drop(&mut self) {
        self.0.request_stop();
    }
}

async fn run(
    process: &mut dyn ManagedWorker,
    lease: &mut Lease,
    identity: &ExecutionIdentity,
    transport: &WorkerTransport,
    bootstrap: &[u8],
    cancel: &mut watch::Receiver<bool>,
    timing: Timing,
) -> StopReason {
    let mut started = false;
    let mut failures = 0_u32;
    loop {
        let renewal = match lease.begin_renewal(Instant::now()) {
            Ok(renewal) => renewal,
            Err(_) => return StopReason::LeaseExpired,
        };
        let request = WorkerRequest {
            version: VERSION,
            workspace_id: identity.workspace_id.clone(),
            scan_id: identity.scan_id.clone(),
            agent_id: identity.agent_id.clone(),
            attempt_id: identity.attempt_id.clone(),
            generation: identity.generation,
            operation_id: renewal.request_id.clone(),
            operation: WorkerOperation::Heartbeat {},
        };
        let reply = match monitored(
            transport.send(&request, timing.request),
            lease,
            process,
            cancel,
        )
        .await
        {
            Ok(reply) => reply,
            Err(reason) => return reason,
        };
        let delay = match reply {
            Ok(WorkerReply::Lease { grant }) => {
                if lease.apply(renewal, &grant, Instant::now()).is_err() {
                    return StopReason::InvalidReply;
                }
                failures = 0;
                if !started {
                    let deadline = match lease.dispatch_deadline(Instant::now()) {
                        Ok(deadline) => deadline,
                        Err(_) => return StopReason::LeaseExpired,
                    };
                    if *cancel.borrow() {
                        return StopReason::Canceled;
                    }
                    let result = tokio::select! {
                        biased;
                        _ = cancel.changed() => return StopReason::Canceled,
                        result = tokio::time::timeout_at(deadline.into(), process.release_startup(bootstrap)) => result,
                    };
                    match result {
                        Ok(Ok(())) => started = true,
                        Ok(Err(_)) => return StopReason::StartupFailed,
                        Err(_) => return StopReason::LeaseExpired,
                    }
                }
                let now = Instant::now();
                match lease.dispatch_deadline(now) {
                    Ok(deadline) => timing
                        .heartbeat
                        .min(deadline.saturating_duration_since(now) / 3),
                    Err(_) => return StopReason::LeaseExpired,
                }
            }
            Ok(_) => return StopReason::InvalidReply,
            Err(error) => match error.kind {
                TransportFailure::Rejected => return StopReason::AuthorityRejected,
                TransportFailure::Protocol => return StopReason::InvalidReply,
                TransportFailure::Retryable => {
                    failures += 1;
                    if failures >= 3 {
                        return StopReason::RecoveryExhausted;
                    }
                    timing.retry * (1 << (failures - 1))
                }
            },
        };
        if let Err(reason) = monitored(tokio::time::sleep(delay), lease, process, cancel).await {
            return reason;
        }
    }
}

async fn monitored<T>(
    future: impl Future<Output = T>,
    lease: &mut Lease,
    process: &mut dyn ManagedWorker,
    cancel: &mut watch::Receiver<bool>,
) -> std::result::Result<T, StopReason> {
    tokio::pin!(future);
    loop {
        if *cancel.borrow() || cancel.has_changed().is_err() {
            return Err(StopReason::Canceled);
        }
        if lease.state(Instant::now()) == LeaseState::TerminationUnconfirmed {
            return Err(StopReason::LeaseExpired);
        }
        match process.root_exited() {
            Ok(true) => return Err(StopReason::RootExited),
            Err(_) => return Err(StopReason::ProcessObservationFailed),
            Ok(false) => (),
        }
        let tick = Instant::now() + Duration::from_millis(10);
        let wake = lease
            .dispatch_deadline(Instant::now())
            .unwrap_or(tick)
            .min(tick);
        tokio::select! {
            biased;
            _ = cancel.changed() => (),
            _ = tokio::time::sleep_until(wake.into()) => (),
            value = &mut future => return Ok(value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::distributed::WorkerProcess;
    use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::post};
    use beale_contracts::swarm_worker::{LeaseGrant, WORKER_PATH};
    use serde_json::json;
    use std::{
        ffi::OsString,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    #[derive(Clone, Copy)]
    enum Failure {
        Partition,
        Transient,
        Exhausted,
        Revoked,
        LateGrant,
    }

    async fn scenario(failure: Failure, expected: StopReason) {
        let identity = ExecutionIdentity {
            workspace_id: "workspace-example".into(),
            scan_id: "scan-example".into(),
            agent_id: "agent-example".into(),
            attempt_id: "attempt-example".into(),
            generation: 1,
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let root = std::env::temp_dir().join(format!(
            "beale-supervision-example-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        let marker = root.join("ready");
        let captured = calls.clone();
        let startup_marker = marker.clone();
        let assigned = identity.clone();
        let app = Router::new().route(
            WORKER_PATH,
            post(move |Json(request): Json<WorkerRequest>| {
                let identity = assigned.clone();
                let calls = captured.clone();
                let marker = startup_marker.clone();
                async move {
                    assert!(identity.matches(&request));
                    let count = calls.fetch_add(1, Ordering::SeqCst) + 1;
                    if matches!(failure, Failure::Revoked) {
                        return StatusCode::UNAUTHORIZED.into_response();
                    }
                    if matches!(failure, Failure::LateGrant) {
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                    if count > 1 {
                        match failure {
                            Failure::Partition => tokio::time::sleep(Duration::from_secs(10)).await,
                            Failure::Transient if count < 4 => {
                                return StatusCode::SERVICE_UNAVAILABLE.into_response();
                            }
                            Failure::Exhausted => {
                                // Failure injection follows child startup, not a race against
                                // Windows process loading. Runtime retry timing stays unchanged.
                                let deadline = Instant::now() + Duration::from_secs(3);
                                while !marker.exists() {
                                    assert!(Instant::now() < deadline, "worker did not start");
                                    tokio::time::sleep(Duration::from_millis(10)).await;
                                }
                                return StatusCode::SERVICE_UNAVAILABLE.into_response();
                            }
                            _ => (),
                        }
                    }
                    Json(WorkerReply::Lease {
                        grant: LeaseGrant {
                            identity,
                            request_id: request.operation_id,
                            valid_for_ms: if matches!(failure, Failure::Exhausted) {
                                5000
                            } else {
                                500
                            },
                        },
                    })
                    .into_response()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let transport = WorkerTransport::new(
            &format!("http://{}", listener.local_addr().unwrap()),
            &"a".repeat(64),
        )
        .unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let arguments: Vec<OsString> = [
            "--ignored",
            "--exact",
            "distributed::process::tests::worker_process_fixture",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        let mut process = WorkerProcess::spawn(
            &std::env::current_exe().unwrap(),
            &arguments,
            &std::env::temp_dir(),
        )
        .await
        .unwrap();
        let mut other_branch = if matches!(failure, Failure::Exhausted) {
            Some(
                WorkerProcess::spawn(
                    &std::env::current_exe().unwrap(),
                    &arguments,
                    &std::env::temp_dir(),
                )
                .await
                .unwrap(),
            )
        } else {
            None
        };
        let bootstrap = serde_json::to_vec(&json!({"ready":marker,"root_exit":false})).unwrap();
        let (cancel, receiver) = watch::channel(false);
        let monitor = if matches!(failure, Failure::Transient | Failure::LateGrant) {
            let calls = calls.clone();
            let marker = marker.clone();
            let cancel = cancel.clone();
            Some(tokio::spawn(async move {
                let deadline = Instant::now() + Duration::from_secs(4);
                let ready = || {
                    if matches!(failure, Failure::LateGrant) {
                        calls.load(Ordering::SeqCst) > 0
                    } else {
                        calls.load(Ordering::SeqCst) >= 4 && marker.exists()
                    }
                };
                while !ready() {
                    assert!(Instant::now() < deadline, "worker did not recover");
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                // Let the successful renewal finish before issuing cancellation.
                tokio::time::sleep(Duration::from_millis(20)).await;
                cancel.send(true).unwrap();
            }))
        } else {
            None
        };
        let report = tokio::time::timeout(
            Duration::from_secs(8),
            supervise_with_timing(
                &mut process,
                identity,
                &transport,
                &bootstrap,
                receiver,
                Timing {
                    heartbeat: Duration::from_millis(50),
                    request: Duration::from_secs(if matches!(failure, Failure::Exhausted) {
                        4
                    } else {
                        2
                    }),
                    retry: Duration::from_millis(10),
                },
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(report.reason, expected);
        assert!(report.termination_confirmed);
        assert!(process.confirm_stopped().unwrap());
        assert_eq!(
            marker.exists(),
            !matches!(failure, Failure::Revoked | Failure::LateGrant)
        );
        if matches!(failure, Failure::Exhausted) {
            assert_eq!(calls.load(Ordering::SeqCst), 4);
        }
        if let Some(branch) = &mut other_branch {
            assert!(
                !branch.root_exited().unwrap(),
                "another branch must retain its execution owner"
            );
            assert!(branch.stop(Duration::from_secs(5)).await.unwrap());
        }
        if let Some(monitor) = monitor {
            monitor.await.unwrap();
        }
        server.abort();
        let _ = server.await;
        drop(process);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn partition_during_execution_expires_the_lease_and_confirms_process_termination() {
        scenario(Failure::Partition, StopReason::LeaseExpired).await;
    }

    #[tokio::test]
    async fn temporary_heartbeat_failures_recover_before_cancellation() {
        scenario(Failure::Transient, StopReason::Canceled).await;
    }

    #[tokio::test]
    async fn exhausted_recovery_stops_only_the_owned_worker_after_three_failures() {
        scenario(Failure::Exhausted, StopReason::RecoveryExhausted).await;
    }

    #[tokio::test]
    async fn revoked_authority_never_releases_worker_startup() {
        scenario(Failure::Revoked, StopReason::AuthorityRejected).await;
    }

    #[tokio::test]
    async fn cancel_during_renewal_cannot_release_startup_from_a_late_grant() {
        scenario(Failure::LateGrant, StopReason::Canceled).await;
    }
}
