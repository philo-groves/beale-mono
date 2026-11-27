use super::*;
use beale_contracts::swarm_worker::ExecutionIdentity;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Control {
    root: PathBuf,
    spawns: AtomicUsize,
    releases: AtomicUsize,
    stops: AtomicUsize,
    recovers: AtomicUsize,
    confirm: AtomicBool,
    blocked: AtomicBool,
    fail: AtomicBool,
    recover_running: AtomicBool,
    slow_recovery: AtomicBool,
    entered: tokio::sync::Notify,
}
impl Default for Control {
    fn default() -> Self {
        Self {
            root: std::env::temp_dir().join(format!(
                "beale-peer-runtime-example-{}",
                uuid::Uuid::new_v4()
            )),
            spawns: AtomicUsize::new(0),
            releases: AtomicUsize::new(0),
            stops: AtomicUsize::new(0),
            recovers: AtomicUsize::new(0),
            confirm: AtomicBool::new(false),
            blocked: AtomicBool::new(false),
            fail: AtomicBool::new(false),
            recover_running: AtomicBool::new(false),
            slow_recovery: AtomicBool::new(false),
            entered: tokio::sync::Notify::new(),
        }
    }
}
impl Drop for Control {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
struct Environments(Arc<Control>);
impl PreparedEnvironments for Environments {
    fn available(&self) -> bool {
        true
    }
    fn validate(&self, target: &ExecutionTarget) -> Result<(), BackendRejection> {
        if target == &target_example() {
            Ok(())
        } else {
            Err(BackendRejection::Conflict)
        }
    }
    fn spawn<'a>(
        &'a self,
        _: &'a ExecutionTarget,
        _: &'a HostWorkerLaunch,
    ) -> ProcessFuture<'a, PreparedWorker> {
        Box::pin(async move {
            self.0.spawns.fetch_add(1, Ordering::SeqCst);
            self.0.entered.notify_one();
            if self.0.blocked.load(Ordering::SeqCst) {
                std::future::pending::<()>().await;
            }
            ensure!(
                !self.0.fail.load(Ordering::SeqCst),
                "synthetic preparation failure"
            );
            Ok(PreparedWorker {
                process: Box::new(Process(self.0.clone())),
                workspace: PathBuf::from("example-workspace"),
                protected_root: PathBuf::from("example-host"),
                recovery: PathBuf::from("example-recovery"),
            })
        })
    }
    fn recover<'a>(&'a self, _: &'a ExecutionTarget) -> BackendFuture<'a> {
        Box::pin(async {
            self.0.recovers.fetch_add(1, Ordering::SeqCst);
            if self.0.slow_recovery.load(Ordering::SeqCst) {
                self.0.entered.notify_one();
                tokio::time::sleep(Duration::from_secs(6)).await;
            }
            Ok(if self.0.confirm.load(Ordering::SeqCst) {
                BackendObservation::Stopped
            } else if self.0.recover_running.load(Ordering::SeqCst) {
                BackendObservation::Running
            } else {
                BackendObservation::TerminationUnconfirmed
            })
        })
    }
}
struct Process(Arc<Control>);
impl ManagedWorker for Process {
    fn release_startup<'a>(&'a mut self, _: &'a [u8]) -> ProcessFuture<'a, ()> {
        self.0.releases.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
    fn root_exited(&mut self) -> Result<bool> {
        Ok(true)
    }
    fn request_stop(&mut self) {
        self.0.stops.fetch_add(1, Ordering::SeqCst);
    }
    fn stop(&mut self, _: Duration) -> ProcessFuture<'_, bool> {
        self.request_stop();
        Box::pin(async { Ok(self.0.confirm.load(Ordering::SeqCst)) })
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.request_stop();
    }
}
fn target_example() -> ExecutionTarget {
    ExecutionTarget {
        host_id: "host-example".into(),
        leader_id: "leader-example".into(),
        configuration_id: "configuration-example".into(),
        configuration_revision: 1,
        instance_id: "instance-example".into(),
        identity: ExecutionIdentity {
            workspace_id: "workspace-example".into(),
            scan_id: "scan-example".into(),
            agent_id: "agent-example".into(),
            attempt_id: "attempt-example".into(),
            generation: 1,
        },
    }
}
fn launch_example() -> HostWorkerLaunch {
    HostWorkerLaunch {
        snapshot: None,
        leader_origin: "http://127.0.0.1:1".into(),
        worker_bearer: "c".repeat(64),
    }
}
fn backend(control: &Arc<Control>) -> AssignedWorkerBackend {
    AssignedWorkerBackend::new(
        control.root.clone(),
        Arc::new(Environments(control.clone())),
    )
}
async fn finished(backend: &AssignedWorkerBackend, target: &ExecutionTarget) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let active = backend.active(target).unwrap().unwrap();
            if active.task.lock().unwrap().as_ref().unwrap().is_finished() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn peer_supervisor_retains_uncertain_execution_and_never_relaunches() {
    let control = Arc::new(Control::default());
    let backend = backend(&control);
    let target = target_example();
    backend.launch(&target, &launch_example()).await.unwrap();
    finished(&backend, &target).await;
    for _ in 0..3 {
        assert_eq!(
            backend.inspect(&target).await.unwrap(),
            BackendObservation::TerminationUnconfirmed
        );
        backend.launch(&target, &launch_example()).await.unwrap();
    }
    assert_eq!(control.spawns.load(Ordering::SeqCst), 1);
    assert_eq!(control.releases.load(Ordering::SeqCst), 0);
    assert_eq!(
        control.recovers.load(Ordering::SeqCst),
        0,
        "retains the live process handle"
    );
    let mut rebound = target.clone();
    rebound.identity.generation += 1;
    assert!(backend.inspect(&rebound).await.is_err());
    assert!(backend.launch(&rebound, &launch_example()).await.is_err());
    control.confirm.store(true, Ordering::SeqCst);
    assert_eq!(
        backend.stop(&target).await.unwrap(),
        BackendObservation::Stopped
    );
}

#[tokio::test]
async fn peer_stop_interrupts_pending_preparation_and_recovers_without_releasing_startup() {
    let control = Arc::new(Control::default());
    control.blocked.store(true, Ordering::SeqCst);
    let backend = backend(&control);
    let target = target_example();
    // Launch returns while preparation is blocked: request lifetime is not the
    // supervisor lifetime, and repeating it cannot create another task.
    backend.launch(&target, &launch_example()).await.unwrap();
    control.entered.notified().await;
    backend.launch(&target, &launch_example()).await.unwrap();
    backend.stop(&target).await.unwrap();
    finished(&backend, &target).await;
    assert_eq!(control.spawns.load(Ordering::SeqCst), 1);
    assert_eq!(control.releases.load(Ordering::SeqCst), 0);
    assert!(control.recovers.load(Ordering::SeqCst) > 0);
    assert_eq!(
        backend.inspect(&target).await.unwrap(),
        BackendObservation::TerminationUnconfirmed
    );
    control.confirm.store(true, Ordering::SeqCst);
    assert_eq!(
        backend.inspect(&target).await.unwrap(),
        BackendObservation::Stopped
    );
}

#[tokio::test]
async fn peer_restart_and_failed_preparation_require_backend_termination_proof() {
    let control = Arc::new(Control::default());
    control.fail.store(true, Ordering::SeqCst);
    control.recover_running.store(true, Ordering::SeqCst);
    let original = backend(&control);
    let target = target_example();
    original.launch(&target, &launch_example()).await.unwrap();
    finished(&original, &target).await;
    assert_eq!(
        original.inspect(&target).await.unwrap(),
        BackendObservation::TerminationUnconfirmed
    );
    drop(original);
    let reopened = backend(&control);
    assert_eq!(
        reopened.inspect(&target).await.unwrap(),
        BackendObservation::TerminationUnconfirmed
    );
    control.confirm.store(true, Ordering::SeqCst);
    assert_eq!(
        reopened.stop(&target).await.unwrap(),
        BackendObservation::Stopped
    );
    assert_eq!(control.spawns.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn peer_backend_shutdown_cancels_detached_preparation() {
    let control = Arc::new(Control::default());
    control.blocked.store(true, Ordering::SeqCst);
    let backend = backend(&control);
    backend
        .launch(&target_example(), &launch_example())
        .await
        .unwrap();
    control.entered.notified().await;
    let active = backend.active(&target_example()).unwrap().unwrap();
    drop(backend);
    tokio::time::timeout(Duration::from_secs(5), async {
        while !active.task.lock().unwrap().as_ref().unwrap().is_finished() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(control.spawns.load(Ordering::SeqCst), 1);
    assert_eq!(control.releases.load(Ordering::SeqCst), 0);
    assert!(control.recovers.load(Ordering::SeqCst) > 0);
}

#[tokio::test]
async fn peer_recovery_skips_another_live_supervisor_until_ownership_releases() {
    let control = Arc::new(Control::default());
    control.blocked.store(true, Ordering::SeqCst);
    let original = backend(&control);
    let contender = backend(&control);
    let target = target_example();
    original.launch(&target, &launch_example()).await.unwrap();
    control.entered.notified().await;
    assert!(contender.launch(&target, &launch_example()).await.is_err());
    assert_eq!(
        contender.inspect(&target).await.unwrap(),
        BackendObservation::TerminationUnconfirmed
    );
    assert_eq!(
        contender.stop(&target).await.unwrap(),
        BackendObservation::TerminationUnconfirmed
    );
    assert_eq!(control.recovers.load(Ordering::SeqCst), 0);
    assert_eq!(control.spawns.load(Ordering::SeqCst), 1);
    original.stop(&target).await.unwrap();
    finished(&original, &target).await;
    control.confirm.store(true, Ordering::SeqCst);
    drop(original);
    assert_eq!(
        contender.inspect(&target).await.unwrap(),
        BackendObservation::Stopped
    );
    assert_eq!(control.spawns.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn peer_restart_recovery_survives_normal_background_maintenance_deadlines() {
    use beale_contracts::swarm_host::{
        HostExecutionOperation, HostExecutionPhase, HostExecutionRequest,
    };
    use beale_storage::{HostDatabase, fleet_executions::FleetExecutionStore};
    let control = Arc::new(Control::default());
    control.slow_recovery.store(true, Ordering::SeqCst);
    control.confirm.store(true, Ordering::SeqCst);
    let db = HostDatabase::open(&control.root).unwrap();
    let store = FleetExecutionStore::open(db.clone()).unwrap();
    let mut target = target_example();
    target.host_id = store.host_id().unwrap();
    let request = HostExecutionRequest {
        version: 1,
        operation_id: "start-example".into(),
        target: target.clone(),
        operation: HostExecutionOperation::Start {
            launch: launch_example(),
        },
    };
    assert!(store.accept(&request).unwrap().launch);
    // A newly opened supervisor has no live ManagedWorker. Exercise the real
    // five-second maintenance budget against a six-second backend inspection.
    let recovered = Arc::new(backend(&control));
    let maintenance = beale_fleet::worker_host::start_recovery(
        db.clone(),
        control.root.clone(),
        recovered.clone(),
    );
    tokio::time::timeout(Duration::from_secs(3), control.entered.notified())
        .await
        .unwrap();
    let contender = backend(&control);
    assert_eq!(
        contender.stop(&target).await.unwrap(),
        BackendObservation::TerminationUnconfirmed
    );
    tokio::time::sleep(Duration::from_millis(5200)).await;
    assert_ne!(
        store.inspect(&target).unwrap().unwrap().phase,
        HostExecutionPhase::Stopped
    );
    let mut next = request.clone();
    next.operation_id = "next-example".into();
    next.target.identity.attempt_id = "next-attempt-example".into();
    next.target.identity.generation += 1;
    assert!(store.accept(&next).is_err(), "replacement remains fenced");
    tokio::time::timeout(Duration::from_secs(17), async {
        while store.inspect(&target).unwrap().unwrap().phase != HostExecutionPhase::Stopped {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        control.recovers.load(Ordering::SeqCst),
        1,
        "polls share one retained recovery operation"
    );
    assert_eq!(control.spawns.load(Ordering::SeqCst), 0);
    assert!(store.accept(&next).unwrap().launch);
    maintenance.abort();
    let _ = maintenance.await;
}
