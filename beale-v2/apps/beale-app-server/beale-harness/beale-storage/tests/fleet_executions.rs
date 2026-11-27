use beale_contracts::{swarm_host::*, swarm_worker::ExecutionIdentity};
use beale_storage::{HostDatabase, fleet_executions::*};
use std::{
    path::PathBuf,
    sync::{Arc, Barrier},
};

struct Fixture {
    root: PathBuf,
    db: HostDatabase,
    store: FleetExecutionStore,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "beale-host-execution-example-{}",
            uuid::Uuid::new_v4()
        ));
        let db = HostDatabase::open(&root).unwrap();
        let store = FleetExecutionStore::open(db.clone()).unwrap();
        Self { root, db, store }
    }
    fn request(&self) -> HostExecutionRequest {
        HostExecutionRequest {
            version: 1,
            operation_id: "start-example".into(),
            target: ExecutionTarget {
                host_id: self.store.host_id().unwrap(),
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
            },
            operation: HostExecutionOperation::Start {
                launch: HostWorkerLaunch {
                    snapshot: None,
                    leader_origin: "https://leader.example.ts.net".into(),
                    worker_bearer: "c".repeat(64),
                },
            },
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn host_launch_claims_replay_after_reopen_without_persisting_credentials() {
    let f = Fixture::new();
    let request = f.request();
    assert!(f.store.accept(&request).unwrap().launch);
    let reopened = FleetExecutionStore::open(HostDatabase::open(&f.root).unwrap()).unwrap();
    assert_eq!(reopened.host_id().unwrap(), request.target.host_id);
    assert!(!reopened.accept(&request).unwrap().launch);
    let mut replay = request.clone();
    replay.operation_id = "new-call-same-launch".into();
    assert!(!reopened.accept(&replay).unwrap().launch);
    if let HostExecutionOperation::Start { launch } = &mut replay.operation {
        launch.worker_bearer = "d".repeat(64);
    }
    assert!(reopened.accept(&replay).is_err());
    f.db.with_connection(|db| {
        let saved: String = db.query_row(
            "SELECT target || launch_hash FROM fleet_executions",
            [],
            |row| row.get(0),
        )?;
        assert!(!saved.contains(&"c".repeat(64)) && !saved.contains("leader_origin"));
        Ok(())
    })
    .unwrap();
    assert_eq!(f.db.schema_version("swarms").unwrap(), 0);
}

#[test]
fn cancellation_tombstones_and_stale_observations_cannot_revive_execution() {
    let f = Fixture::new();
    let start = f.request();
    let mut stop = start.clone();
    stop.operation_id = "stop-before-start".into();
    stop.operation = HostExecutionOperation::Stop {};
    assert_eq!(
        f.store.accept(&stop).unwrap().report.phase,
        HostExecutionPhase::Stopped
    );
    let canceled = f.store.accept(&start).unwrap();
    assert!(!canceled.launch);
    assert_eq!(canceled.report.phase, HostExecutionPhase::Stopped);
    let mut next = start.clone();
    next.operation_id = "start-next".into();
    next.target.identity.attempt_id = "attempt-next".into();
    next.target.identity.generation = 2;
    let running = f.store.accept(&next).unwrap();
    assert!(running.launch);
    stop = next.clone();
    stop.operation_id = "stop-next".into();
    stop.operation = HostExecutionOperation::Stop {};
    let stopping = f.store.accept(&stop).unwrap().report;
    assert_eq!(
        f.store
            .observe(
                &next.target,
                running.report.revision,
                BackendObservation::Running
            )
            .unwrap(),
        stopping
    );
    let uncertain = f
        .store
        .observe(
            &next.target,
            stopping.revision,
            BackendObservation::TerminationUnconfirmed,
        )
        .unwrap();
    assert_eq!(uncertain.phase, HostExecutionPhase::TerminationUnconfirmed);
    assert!(uncertain.stop_requested);
    let stopped = f
        .store
        .observe(
            &next.target,
            uncertain.revision,
            BackendObservation::Stopped,
        )
        .unwrap();
    assert_eq!(stopped.phase, HostExecutionPhase::Stopped);
    assert_eq!(
        f.store
            .observe(&next.target, stopped.revision, BackendObservation::Running)
            .unwrap(),
        stopped
    );
}

#[test]
fn private_instance_ownership_and_full_execution_identity_are_enforced() {
    let f = Fixture::new();
    let request = f.request();
    let first = f.store.accept(&request).unwrap().report;
    let mut changed = request.clone();
    changed.operation_id = "changed".into();
    changed.target.host_id = "other-host".into();
    assert!(f.store.accept(&changed).is_err());
    changed = request.clone();
    changed.target.configuration_revision += 1;
    assert!(f.store.accept(&changed).is_err());
    changed = request.clone();
    changed.target.identity.workspace_id = "workspace-other".into();
    assert!(f.store.accept(&changed).is_err());
    changed = request.clone();
    changed.target.instance_id = "instance-other".into();
    assert!(f.store.accept(&changed).is_err());
    changed = request.clone();
    changed.operation_id = "alias".into();
    changed.target.identity.attempt_id = "attempt-alias".into();
    assert!(f.store.accept(&changed).is_err());
    changed.target.identity.generation = 2;
    assert!(f.store.accept(&changed).is_err());
    let unknown = f
        .store
        .observe(
            &request.target,
            first.revision,
            BackendObservation::TerminationUnconfirmed,
        )
        .unwrap();
    assert!(f.store.accept(&changed).is_err());
    f.store
        .observe(
            &request.target,
            unknown.revision,
            BackendObservation::Stopped,
        )
        .unwrap();
    changed.target.identity.agent_id = "agent-other".into();
    assert!(f.store.accept(&changed).is_err());
    changed.target.identity.agent_id = request.target.identity.agent_id.clone();
    changed.target.leader_id = "leader-other".into();
    assert!(f.store.accept(&changed).is_err());
    changed.target.leader_id = request.target.leader_id.clone();
    assert!(f.store.accept(&changed).unwrap().launch);
}

#[test]
fn failed_receipt_and_observation_writes_retain_prior_ownership_atomically() {
    let f = Fixture::new();
    let request = f.request();
    f.db.with_connection(|db| { db.execute_batch("CREATE TRIGGER fail_receipt BEFORE INSERT ON fleet_execution_operations BEGIN SELECT RAISE(ABORT,'synthetic receipt full'); END;")?; Ok(()) }).unwrap();
    assert!(f.store.accept(&request).is_err());
    assert!(f.store.inspect(&request.target).unwrap().is_none());
    f.db.with_connection(|db| {
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM fleet_execution_instances",
                [],
                |row| row.get::<_, i64>(0)
            )?,
            0
        );
        db.execute_batch("DROP TRIGGER fail_receipt;")?;
        Ok(())
    })
    .unwrap();
    let accepted = f.store.accept(&request).unwrap();
    assert!(accepted.launch);
    f.db.with_connection(|db| { db.execute_batch("CREATE TRIGGER fail_observation BEFORE UPDATE ON fleet_executions BEGIN SELECT RAISE(ABORT,'synthetic status full'); END;")?; Ok(()) }).unwrap();
    assert!(
        f.store
            .observe(
                &request.target,
                accepted.report.revision,
                BackendObservation::Stopped
            )
            .is_err()
    );
    assert_eq!(
        f.store.inspect(&request.target).unwrap().unwrap(),
        accepted.report
    );
    assert!(!f.store.accept(&request).unwrap().launch);
}

#[test]
fn concurrent_host_connections_grant_exactly_one_launch() {
    let f = Fixture::new();
    let request = f.request();
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let store = FleetExecutionStore::open(HostDatabase::open(&f.root).unwrap()).unwrap();
            let barrier = barrier.clone();
            let request = request.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store.accept(&request).unwrap().launch
            })
        })
        .collect();
    assert_eq!(
        threads
            .into_iter()
            .map(|thread| usize::from(thread.join().unwrap()))
            .sum::<usize>(),
        1
    );
}

#[test]
fn recovery_pages_are_bounded_and_corrupt_records_do_not_hide_later_identities() {
    let f = Fixture::new();
    for index in 0..66 {
        let mut request = f.request();
        request.operation_id = format!("operation-{index:03}");
        request.target.identity.attempt_id = format!("attempt-{index:03}");
        request.target.identity.agent_id = format!("agent-{index:03}");
        request.target.instance_id = format!("instance-{index:03}");
        f.store.accept(&request).unwrap();
    }
    f.db.with_connection(|db| {
        db.execute(
            "UPDATE fleet_executions SET target='null' WHERE attempt='attempt-000'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    let first = f.store.pending_page(None).unwrap();
    assert_eq!(first.len(), 64);
    assert!(f.store.pending(&first[0]).is_err());
    assert!(f.store.pending(&first[1]).is_ok());
    let second = f.store.pending_page(first.last()).unwrap();
    assert_eq!(second.len(), 2);
    assert_eq!(second[0].attempt, "attempt-064");
    assert!(f.store.pending_page(second.last()).unwrap().is_empty());
}
