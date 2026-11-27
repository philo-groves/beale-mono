use super::super::restart;
use super::*;

async fn unprepared() -> (Fixture, LocalTurn) {
    let f = Fixture::new().await;
    std::fs::create_dir_all(f.root.join("workspace")).unwrap();
    crate::research_files::capture(&f.state, "scan-example", &f.workspace_id).unwrap();
    let identity = f.store.worker_bootstrap(&f.attempt.id).unwrap().identity;
    super::super::supervisor::reconcile_owned_attempt(
        &f.store,
        &identity,
        true,
        WorkerDisposition::Failed,
        unix_ms(),
    )
    .unwrap();
    f.store
        .local_message(
            "scan-example",
            "resume-example",
            &f.deputy,
            "Resume synthetic assignment",
        )
        .unwrap();
    let turn = f
        .store
        .local_claim("scan-example", &f.deputy, unix_ms(), 64)
        .unwrap()
        .unwrap();
    (f, turn)
}

#[tokio::test]
async fn restart_discovers_intent_before_preparation_without_claiming_live_launch() {
    let (f, turn) = unprepared().await;
    let key = Uuid::new_v4();
    let owner = restart::lock(&f.state, key).unwrap().unwrap();
    f.store.register_local_worker(&turn, key).unwrap();
    assert!(
        f.store
            .register_local_worker(&turn, Uuid::new_v4())
            .is_err()
    );
    let before = f.store.load("scan-example", true).unwrap();
    restart::recover_page(&f.state, None, "").unwrap();
    assert_eq!(f.store.load("scan-example", true).unwrap(), before);
    let record = f.store.local_worker_records(None, "").unwrap().remove(0);
    assert!(f.store.complete_local_worker_recovery(&record).is_err());
    drop(owner);
    let fresh = crate::build_state("control-example".into(), f.state.workspaces.clone()).unwrap();
    restart::recover_page(&fresh, None, "").unwrap();
    assert!(f.store.local_worker_records(None, "").unwrap().is_empty());
    assert!(!f.store.worker_prepared(&turn.attempt_id).unwrap());
    assert!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .attempts
            .iter()
            .any(|a| a.id == turn.attempt_id && a.state == ExecutionState::Stopped)
    );
    drop(fresh);
    f.close().await;
}

#[cfg(any(windows, target_os = "linux"))]
#[tokio::test]
async fn restart_retains_prepared_attempt_across_checkpoint_failure_and_reconciles_once() {
    let (f, turn) = unprepared().await;
    let key = Uuid::new_v4();
    f.store.register_local_worker(&turn, key).unwrap();
    let files = crate::research_files::store(&f.state).unwrap();
    let workspace = files.prepare_agent("scan-example", &f.deputy).unwrap();
    #[cfg(target_os = "linux")]
    let _guardian = beale_tools::linux_worker::LinuxWorker::prepare(
        &f.state.workspaces.root().join("swarm-linux-supervisors"),
        key,
        &std::env::current_exe().unwrap(),
        &[],
        &workspace,
    )
    .unwrap();
    #[cfg(not(target_os = "linux"))]
    let _ = workspace;
    let (bootstrap, credential) = prepare_worker(
        &f.state.db,
        &turn,
        beale_contracts::swarm_worker::WorkerShell::native(),
        &registration(),
        unix_ms(),
    )
    .unwrap();
    f.state
        .db
        .with_connection(|db| {
            db.execute(
                "UPDATE swarm_local_workers SET backend='unix_group' WHERE attempt_id=?1",
                [&turn.attempt_id],
            )?;
            Ok(())
        })
        .unwrap();
    restart::recover_page(&f.state, None, "").unwrap();
    assert!(
        !f.store.worker_reconciled(&turn).unwrap(),
        "another backend must not use native supervisor evidence as termination proof"
    );
    assert!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .events
            .iter()
            .any(|e| e.kind == "worker-termination-unconfirmed")
    );
    f.state
        .db
        .with_connection(|db| {
            db.execute(
                "UPDATE swarm_local_workers SET backend=?2 WHERE attempt_id=?1",
                [
                    &turn.attempt_id,
                    beale_storage::swarm_state::local_worker_backend(),
                ],
            )?;
            Ok(())
        })
        .unwrap();
    f.state.db.with_connection(|db| { db.execute_batch("CREATE TRIGGER fail_restart_checkpoint BEFORE INSERT ON swarm_file_checkpoints BEGIN SELECT RAISE(ABORT,'synthetic checkpoint full'); END;")?; Ok(()) }).unwrap();
    let fresh = crate::build_state("control-example".into(), f.state.workspaces.clone()).unwrap();
    restart::recover_page(&fresh, None, "").unwrap();
    assert!(!f.store.worker_reconciled(&turn).unwrap());
    assert_eq!(f.store.local_worker_records(None, "").unwrap().len(), 1);
    assert!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .attempts
            .iter()
            .any(|a| a.id == turn.attempt_id && a.state == ExecutionState::TerminationUnconfirmed)
    );
    // Fencing survives the checkpoint failure; the old credential cannot renew.
    let client = WorkerTransport::new(&f.url, credential.bearer()).unwrap();
    let request = WorkerRequest {
        version: VERSION,
        workspace_id: bootstrap.identity.workspace_id,
        scan_id: turn.scan_id.clone(),
        agent_id: turn.agent_id.clone(),
        attempt_id: turn.attempt_id.clone(),
        generation: turn.generation,
        operation_id: "stale-renew".into(),
        operation: WorkerOperation::Bootstrap {},
    };
    assert!(client.send(&request, Duration::from_secs(5)).await.is_err());
    f.state
        .db
        .with_connection(|db| {
            db.execute_batch("DROP TRIGGER fail_restart_checkpoint;")?;
            Ok(())
        })
        .unwrap();
    restart::recover_page(&fresh, None, "").unwrap();
    assert!(f.store.worker_reconciled(&turn).unwrap());
    assert!(f.store.local_worker_records(None, "").unwrap().is_empty());
    let saved = f.store.load("scan-example", true).unwrap();
    restart::recover_page(&fresh, None, "").unwrap();
    assert_eq!(f.store.load("scan-example", true).unwrap(), saved);
    f.state.db.with_connection(|db|{ db.execute("UPDATE swarm_receipts SET result='null' WHERE scan_id=?1 AND actor='supervisor' AND operation_id=?2",beale_storage::rusqlite::params![turn.scan_id,format!("reconcile:{}",turn.attempt_id)])?;Ok(()) }).unwrap();
    assert!(f.store.worker_reconciled(&turn).is_err());
    drop(files);
    drop(fresh);
    f.close().await;
}

#[tokio::test]
async fn restart_corrupt_record_does_not_starve_a_healthy_launch_intent() {
    let (f, turn) = unprepared().await;
    f.store
        .register_local_worker(&turn, Uuid::new_v4())
        .unwrap();
    f.state.db.with_connection(|db| {
        db.execute("INSERT INTO swarm_local_workers(attempt_id,scan_id,owner_key,backend,turn) VALUES('!corrupt-example','scan-example',?1,'windows_job','null')",[Uuid::new_v4().to_string()])?;
        Ok(())
    }).unwrap();
    assert!(f.store.local_worker_record("!corrupt-example").is_err());
    restart::recover_page(&f.state, None, "").unwrap();
    assert_eq!(
        f.store.local_worker_recovery_page(None, "").unwrap(),
        vec!["!corrupt-example"]
    );
    assert!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .attempts
            .iter()
            .any(|a| a.id == turn.attempt_id && a.state == ExecutionState::Stopped)
    );
    f.close().await;
}

#[tokio::test]
async fn restart_ended_scan_releases_only_never_prepared_attempt() {
    let (f, turn) = unprepared().await;
    f.store
        .register_local_worker(&turn, Uuid::new_v4())
        .unwrap();
    f.store.end_scan("scan-example", unix_ms()).unwrap();
    restart::recover_page(&f.state, None, "").unwrap();
    assert!(
        f.store
            .local_worker_recovery_page(None, "")
            .unwrap()
            .is_empty()
    );
    let saved = f.store.load("scan-example", true).unwrap();
    assert!(saved.ended_at_ms.is_some());
    assert_eq!(
        saved.agent(&f.deputy).unwrap().summary.state,
        AgentState::Stopped
    );
    assert!(
        saved
            .attempts
            .iter()
            .any(|a| a.id == turn.attempt_id && a.state == ExecutionState::Stopped)
    );
    assert!(!f.store.worker_prepared(&turn.attempt_id).unwrap());
    f.close().await;
}
