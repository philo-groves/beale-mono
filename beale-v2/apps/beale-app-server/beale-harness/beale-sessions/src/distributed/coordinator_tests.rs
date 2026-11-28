use super::super::restart;
use super::*;

async fn leader_turn(f: &Fixture, operation: &str, key: Uuid) -> LocalTurn {
    f.store
        .local_message(
            "scan-example",
            operation,
            &f.leader,
            "Coordinate the synthetic assignment",
        )
        .unwrap();
    f.store
        .local_claim_owned("scan-example", &f.leader, unix_ms(), 64, key)
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn coordinator_restart_respects_live_owner_and_atomically_stops_lost_hierarchy() {
    let f = Fixture::new().await;
    let key = Uuid::new_v4();
    let owner = restart::lock(&f.state, key).unwrap().unwrap();
    let turn = leader_turn(&f, "coordinate-example", key).await;
    let fresh = crate::build_state("control-example".into(), f.state.workspaces.clone()).unwrap();
    let before = f.store.load("scan-example", true).unwrap();
    f.state.db.with_connection(|db| {db.execute("UPDATE swarm_local_coordinators SET turn=json_set(turn,'$.profile','profile-other') WHERE attempt_id=?1",[&turn.attempt_id])?;Ok(())}).unwrap();
    assert!(f.store.coordinator_owner(&turn.attempt_id).is_err());
    f.state
        .db
        .with_connection(|db| {
            db.execute(
                "UPDATE swarm_local_coordinators SET turn=?2 WHERE attempt_id=?1",
                beale_storage::rusqlite::params![
                    turn.attempt_id,
                    serde_json::to_string(&turn).unwrap()
                ],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(
        f.store
            .complete_coordinator(&turn.attempt_id, key, unix_ms())
            .is_err()
    );
    assert!(
        f.store
            .interrupt_coordinator(&turn.attempt_id, Uuid::new_v4(), unix_ms())
            .is_err()
    );
    restart::recover_coordinators(&fresh, None, "").unwrap();
    assert_eq!(f.store.load("scan-example", true).unwrap(), before);
    drop(owner);
    f.state.db.with_connection(|db| {db.execute_batch("CREATE TRIGGER fail_coordinator_fence BEFORE UPDATE OF revoked ON swarm_credentials BEGIN SELECT RAISE(ABORT,'synthetic fence storage failure'); END;")?;Ok(())}).unwrap();
    restart::recover_coordinators(&fresh, None, "").unwrap();
    assert_eq!(f.store.load("scan-example", true).unwrap(), before);
    assert_eq!(
        f.store.coordinator_recovery_page(None, "").unwrap(),
        vec![turn.attempt_id.clone()]
    );
    f.state
        .db
        .with_connection(|db| {
            db.execute_batch("DROP TRIGGER fail_coordinator_fence;")?;
            Ok(())
        })
        .unwrap();
    restart::recover_coordinators(&fresh, None, "").unwrap();
    let after = f.store.load("scan-example", true).unwrap();
    let transcript = f.state.db.session("scan-example").unwrap().unwrap();
    assert_eq!(transcript.turns.len(), 1);
    assert!(transcript.turns[0].answer.starts_with("Host recovery:"));
    assert_eq!(transcript.turns[0].duration_ms, 0);
    assert_eq!(
        after.agent(&f.leader).unwrap().summary.state,
        AgentState::Blocked
    );
    assert!(
        after
            .attempts
            .iter()
            .filter(|a| a.id == turn.attempt_id || a.id == f.attempt.id)
            .all(|a| a.state == ExecutionState::TerminationUnconfirmed)
    );
    assert!(
        f.store
            .local_state("scan-example")
            .unwrap()
            .controls
            .iter()
            .all(|c| c.stop_requested)
    );
    assert_eq!(
        after
            .events
            .iter()
            .filter(|e| e.kind == "coordinator-interrupted")
            .count(),
        1
    );
    assert!(
        f.store
            .local_claim("scan-example", &f.leader, unix_ms(), 64)
            .unwrap()
            .is_none()
    );
    assert!(
        f.store
            .local_message("scan-example", "no-resurrection", &f.leader, "Continue")
            .is_err()
    );
    assert!(
        f.client()
            .send(
                &f.request("stale-after-coordinator", WorkerOperation::Bootstrap {}),
                Duration::from_secs(5)
            )
            .await
            .is_err()
    );
    restart::recover_coordinators(&fresh, None, "").unwrap();
    assert_eq!(f.store.load("scan-example", true).unwrap(), after);
    assert_eq!(
        f.state
            .db
            .session("scan-example")
            .unwrap()
            .unwrap()
            .turns
            .len(),
        1
    );
    drop(fresh);
    f.close().await;
}

#[tokio::test]
async fn coordinator_recovery_of_completed_turn_cannot_stop_replacement() {
    let f = Fixture::new().await;
    let key = Uuid::new_v4();
    let turn = leader_turn(&f, "first-coordinator", key).await;
    f.store
        .local_finish(&turn, json!({}), "Waiting for synthetic work", false)
        .unwrap();
    let next_key = Uuid::new_v4();
    let owner = restart::lock(&f.state, next_key).unwrap().unwrap();
    let next = leader_turn(&f, "next-coordinator", next_key).await;
    let before = f.store.load("scan-example", true).unwrap();
    restart::recover_coordinators(&f.state, None, "").unwrap();
    assert_eq!(f.store.load("scan-example", true).unwrap(), before);
    f.store
        .complete_coordinator(&turn.attempt_id, key, unix_ms())
        .unwrap();
    assert_eq!(f.store.load("scan-example", true).unwrap(), before);
    assert!(before.ended_at_ms.is_none());
    assert_eq!(
        f.store.coordinator_recovery_page(None, "").unwrap(),
        vec![next.attempt_id.clone()]
    );
    assert!(
        !f.store
            .local_state("scan-example")
            .unwrap()
            .controls
            .iter()
            .any(|c| c.stop_requested)
    );
    f.store
        .local_finish(&next, json!({}), "Waiting for synthetic work", false)
        .unwrap();
    f.store
        .complete_coordinator(&next.attempt_id, next_key, unix_ms())
        .unwrap();
    drop(owner);
    f.close().await;
}

#[tokio::test]
async fn coordinator_and_worker_owned_claims_roll_back_when_intent_storage_fails() {
    let f = Fixture::new().await;
    f.store
        .local_message("scan-example", "queue-coordinator", &f.leader, "Coordinate")
        .unwrap();
    let before = f.store.load("scan-example", true).unwrap();
    f.state.db.with_connection(|db|{db.execute_batch("CREATE TRIGGER fail_coordinator_intent BEFORE INSERT ON swarm_local_coordinators BEGIN SELECT RAISE(ABORT,'synthetic intent full'); END;")?;Ok(())}).unwrap();
    assert!(
        f.store
            .local_claim_owned("scan-example", &f.leader, unix_ms(), 64, Uuid::new_v4())
            .is_err()
    );
    assert_eq!(f.store.load("scan-example", true).unwrap(), before);
    f.state
        .db
        .with_connection(|db| {
            db.execute_batch("DROP TRIGGER fail_coordinator_intent;")?;
            Ok(())
        })
        .unwrap();
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
            "queue-worker",
            &f.deputy,
            "Resume synthetic work",
        )
        .unwrap();
    let before = f.store.load("scan-example", true).unwrap();
    f.state.db.with_connection(|db|{db.execute_batch("CREATE TRIGGER fail_worker_intent BEFORE INSERT ON swarm_local_workers BEGIN SELECT RAISE(ABORT,'synthetic intent full'); END;")?;Ok(())}).unwrap();
    assert!(
        f.store
            .local_claim_owned("scan-example", &f.deputy, unix_ms(), 64, Uuid::new_v4())
            .is_err()
    );
    assert_eq!(f.store.load("scan-example", true).unwrap(), before);
    f.state
        .db
        .with_connection(|db| {
            db.execute_batch("DROP TRIGGER fail_worker_intent;")?;
            Ok(())
        })
        .unwrap();
    let key = Uuid::new_v4();
    let turn = f
        .store
        .local_claim_owned("scan-example", &f.deputy, unix_ms(), 64, key)
        .unwrap()
        .unwrap();
    assert_eq!(
        f.store
            .local_worker_record(&turn.attempt_id)
            .unwrap()
            .owner_key,
        key
    );
    // Coordinator recovery may fence this claim before worker preparation.
    f.store.local_stop("scan-example", &f.leader).unwrap();
    f.store
        .fence_attempt("scan-example", &turn.attempt_id, false)
        .unwrap();
    restart::recover_page(&f.state, None, "").unwrap();
    assert!(
        f.store
            .local_worker_recovery_page(None, "")
            .unwrap()
            .is_empty()
    );
    let saved = f.store.load("scan-example", true).unwrap();
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
    f.close().await;
}

#[tokio::test]
async fn terminal_coordinator_keeps_the_scan_open_while_a_subagent_runs() {
    let f = Fixture::new().await;
    let key = Uuid::new_v4();
    let turn = leader_turn(&f, "terminal-with-running-child", key).await;
    f.store.local_checkpoint_failed(&turn).unwrap();
    f.store
        .complete_coordinator(&turn.attempt_id, key, 1000)
        .unwrap();
    // The running subagent's result can still be reconciled; its branch is
    // stopped and the scheduler ends the scan once it finishes.
    let saved = f.store.load("scan-example", true).unwrap();
    assert!(saved.ended_at_ms.is_none());
    assert!(
        saved
            .attempts
            .iter()
            .any(|a| a.id == f.attempt.id && a.state == ExecutionState::Running)
    );
    assert!(
        f.store
            .local_state("scan-example")
            .unwrap()
            .controls
            .iter()
            .any(|c| c.agent_id == f.deputy && c.stop_requested)
    );
    assert!(
        f.store
            .coordinator_recovery_page(None, "")
            .unwrap()
            .is_empty()
    );
    f.close().await;
}

#[tokio::test]
async fn coordinator_terminal_completion_rolls_back_and_cannot_end_a_reopened_scan() {
    let f = Fixture::new().await;
    let key = Uuid::new_v4();
    // No subagent is running, so terminal completion ends the scan.
    f.store
        .fence_attempt("scan-example", &f.attempt.id, true)
        .unwrap();
    let turn = leader_turn(&f, "terminal-coordinator", key).await;
    f.store.local_checkpoint_failed(&turn).unwrap();
    let before = f.store.load("scan-example", true).unwrap();
    let transcript = f.state.db.session("scan-example").unwrap().map(|s| s.turns);
    f.state.db.with_connection(|db| {
        db.execute_batch("CREATE TRIGGER reject_scan_end BEFORE UPDATE OF document ON swarm_scans WHEN json_extract(NEW.document, '$.ended_at_ms') IS NOT NULL BEGIN SELECT RAISE(ABORT,'synthetic scan end failure'); END;")?;
        Ok(())
    }).unwrap();
    assert!(
        f.store
            .complete_coordinator(&turn.attempt_id, key, 1000)
            .is_err()
    );
    assert_eq!(f.store.load("scan-example", true).unwrap(), before);
    assert_eq!(
        f.state.db.session("scan-example").unwrap().map(|s| s.turns),
        transcript
    );
    assert_eq!(
        f.store.coordinator_recovery_page(None, "").unwrap(),
        vec![turn.attempt_id.clone()]
    );
    f.state
        .db
        .with_connection(|db| {
            db.execute_batch("DROP TRIGGER reject_scan_end;")?;
            Ok(())
        })
        .unwrap();
    f.store
        .complete_coordinator(&turn.attempt_id, key, 2000)
        .unwrap();
    let ended = f.store.load("scan-example", true).unwrap();
    assert_eq!(ended.ended_at_ms, Some(2000));
    assert!(
        f.store
            .coordinator_recovery_page(None, "")
            .unwrap()
            .is_empty()
    );
    f.store
        .complete_coordinator(&turn.attempt_id, key, 3000)
        .unwrap();
    f.store
        .interrupt_coordinator(&turn.attempt_id, key, 3000)
        .unwrap();
    assert_eq!(f.store.load("scan-example", true).unwrap(), ended);
    // Reopening clears the end clock. An already-completed owner record must
    // remain a no-op even if the stopped leader has no newer attempt yet.
    f.store.reopen_scan("scan-example", true).unwrap();
    let reopened = f.store.load("scan-example", true).unwrap();
    assert!(reopened.ended_at_ms.is_none());
    f.store
        .complete_coordinator(&turn.attempt_id, key, 4000)
        .unwrap();
    f.store
        .interrupt_coordinator(&turn.attempt_id, key, 4000)
        .unwrap();
    assert_eq!(f.store.load("scan-example", true).unwrap(), reopened);
    f.close().await;
}
