use super::*;
use beale_contracts::{swarm_host::*, swarm_local::*, swarm_worker::WorkerShell};

fn setup() -> (Fixture, String) {
    let f = Fixture::new();
    let state = f.scan();
    let leader = &state.agents[0].summary.id;
    f.store
        .peer_initialize(
            "scan-example",
            LocalSettings {
                models: ["coordinator", "research", "investigator"]
                    .into_iter()
                    .map(|id| LocalModel {
                        profile: id.into(),
                        provider: 1,
                        method: 1,
                        model: "model-example".into(),
                    })
                    .collect(),
                reasoning_effort: 0,
            },
            "Coordinate synthetic research",
        )
        .unwrap();
    let turn = f
        .store
        .local_claim("scan-example", leader, 1, 8)
        .unwrap()
        .unwrap();
    let result = f
        .store
        .local_dispatch(
            &turn,
            "spawn-peer",
            SwarmCommand::Spawn(Spawn {
                handoff_id: None,
                profile: profile("research"),
                task: task(),
                independent: false,
            }),
            Admission::Ready,
        )
        .unwrap();
    f.store
        .local_finish(&turn, json!({}), "Waiting for research", false)
        .unwrap();
    (f, result["child_id"].as_str().unwrap().into())
}
fn placement() -> PeerPlacement {
    PeerPlacement {
        leader_id: "leader-example".into(),
        host_id: "host-example".into(),
        configuration_id: "container-example".into(),
        configuration_revision: 1,
        instance_id: "instance-example".into(),
        shell: WorkerShell::native(),
    }
}
fn claim(f: &Fixture, agent: &str, key: Uuid) -> LocalTurn {
    f.store
        .peer_claim_owned("scan-example", agent, 2, 8, key, &placement())
        .unwrap()
        .unwrap()
}
fn assert_released(f: &Fixture, turn: &LocalTurn) {
    let saved = f.store.load("scan-example", true).unwrap();
    assert_eq!(
        saved
            .attempts
            .iter()
            .find(|a| a.id == turn.attempt_id)
            .unwrap()
            .state,
        ExecutionState::Stopped
    );
    assert_eq!(
        saved
            .instances
            .iter()
            .find(|i| i.attempt_id == turn.attempt_id)
            .unwrap()
            .state,
        InstanceState::Stopped
    );
}
#[test]
fn peer_claim_rolls_back_identity_capacity_and_inbox_on_failed_intent() {
    let (f, agent) = setup();
    let before = f.store.load("scan-example", true).unwrap();
    assert!(
        f.store
            .local_claim_owned("scan-example", &agent, 2, 8, Uuid::new_v4())
            .is_err()
    );
    let inbox = f.store.local_state("scan-example").unwrap();
    let mut wrong = placement();
    wrong.host_id = "unselected-example".into();
    assert!(
        f.store
            .peer_claim_owned("scan-example", &agent, 2, 8, Uuid::new_v4(), &wrong)
            .is_err()
    );
    f.db.with_connection(|c|{c.execute_batch("CREATE TRIGGER reject_peer BEFORE INSERT ON swarm_peer_workers BEGIN SELECT RAISE(ABORT,'synthetic intent failure'); END;")?;Ok(())}).unwrap();
    assert!(
        f.store
            .peer_claim_owned("scan-example", &agent, 2, 8, Uuid::new_v4(), &placement())
            .is_err()
    );
    assert_eq!(f.store.load("scan-example", true).unwrap(), before);
    assert_eq!(f.store.local_state("scan-example").unwrap(), inbox);
    f.db.with_connection(|c| {
        c.execute_batch("DROP TRIGGER reject_peer")?;
        Ok(())
    })
    .unwrap();
    let key = Uuid::new_v4();
    let turn = claim(&f, &agent, key);
    let record = f.reopen().peer_worker(&turn.attempt_id).unwrap();
    assert_eq!(record.owner_key, key);
    assert!(!record.dispatched);
    assert!(f.store.peer_dispatch(&turn.attempt_id, key).is_err()); // No bootstrap.
    assert!(
        f.store
            .abandon_unprepared_peer(&turn.attempt_id, Uuid::new_v4())
            .is_err()
    );
    assert!(!f.store.peer_worker(&turn.attempt_id).unwrap().complete);
    f.store
        .abandon_unprepared_peer(&turn.attempt_id, key)
        .unwrap();
    assert_released(&f, &turn);
    assert!(f.store.peer_recovery_page("").unwrap().is_empty());
}
#[test]
fn peer_reports_fence_without_release_and_reject_stale_identity_or_revived_execution() {
    let (f, agent) = setup();
    let key = Uuid::new_v4();
    let turn = claim(&f, &agent, key);
    let record = f.store.peer_worker(&turn.attempt_id).unwrap();
    let report = HostExecutionReport {
        target: record.target,
        phase: HostExecutionPhase::Running,
        revision: 2,
        stop_requested: false,
    };
    assert!(
        !f.store
            .worker_termination_confirmed(&report.target.identity)
            .unwrap()
    );
    f.store
        .peer_observe(&turn.attempt_id, key, &report)
        .unwrap();
    let mut stale = report.clone();
    stale.revision = 1;
    stale.phase = HostExecutionPhase::LaunchRequested;
    assert_eq!(
        f.store
            .peer_observe(&turn.attempt_id, key, &stale)
            .unwrap()
            .report,
        Some(report.clone())
    );
    stale.revision = 2;
    assert!(f.store.peer_observe(&turn.attempt_id, key, &stale).is_err());
    stale = report.clone();
    stale.target.identity.generation += 1;
    assert!(f.store.peer_observe(&turn.attempt_id, key, &stale).is_err());
    let stopped = HostExecutionReport {
        phase: HostExecutionPhase::Stopped,
        revision: 3,
        ..report.clone()
    };
    f.store
        .peer_observe(&turn.attempt_id, key, &stopped)
        .unwrap();
    assert!(
        f.store
            .worker_termination_confirmed(&stopped.target.identity)
            .unwrap()
    );
    let mut foreign = stopped.target.identity.clone();
    foreign.generation += 1;
    assert!(f.store.worker_termination_confirmed(&foreign).is_err());
    let saved = f.reopen().load("scan-example", true).unwrap();
    assert_eq!(
        saved
            .attempts
            .iter()
            .find(|a| a.id == turn.attempt_id)
            .unwrap()
            .state,
        ExecutionState::TerminationUnconfirmed
    );
    assert_eq!(
        saved.instances[0].state,
        InstanceState::TerminationUnconfirmed
    );
    assert!(
        f.store
            .peer_claim_owned("scan-example", &agent, 3, 8, Uuid::new_v4(), &placement())
            .unwrap()
            .is_none()
    );
    assert!(
        f.store
            .peer_observe(
                &turn.attempt_id,
                key,
                &HostExecutionReport {
                    revision: 4,
                    ..report
                }
            )
            .is_err()
    );
    assert_eq!(
        f.store
            .peer_disposition(&turn.attempt_id, key, WorkerDisposition::Completed)
            .unwrap(),
        WorkerDisposition::Completed
    );
    assert_eq!(
        f.reopen()
            .peer_disposition(&turn.attempt_id, key, WorkerDisposition::Failed)
            .unwrap(),
        WorkerDisposition::Completed
    );
    assert!(f.store.complete_peer_worker(&turn.attempt_id, key).is_err());
    assert!(!f.store.peer_worker(&turn.attempt_id).unwrap().complete);
}
#[test]
fn peer_unprepared_recovery_releases_ended_scans_and_requires_completed_instance_before_reuse() {
    let (f, agent) = setup();
    let key = Uuid::new_v4();
    let turn = claim(&f, &agent, key);
    f.store
        .abandon_unprepared_peer(&turn.attempt_id, key)
        .unwrap();
    assert_released(&f, &turn);
    f.store
        .local_message(
            "scan-example",
            "reuse-example",
            &agent,
            "Resume synthetic work",
        )
        .unwrap();
    let mut changed = placement();
    changed.configuration_revision += 1;
    assert!(
        f.store
            .peer_claim_owned("scan-example", &agent, 3, 8, Uuid::new_v4(), &changed)
            .is_err()
    );
    let next_key = Uuid::new_v4();
    let next = claim(&f, &agent, next_key);
    assert!(next.generation > turn.generation);
    assert_eq!(
        f.store.load("scan-example", true).unwrap().instances.len(),
        1
    );
    f.store.end_scan("scan-example", 10).unwrap();
    f.reopen()
        .abandon_unprepared_peer(&next.attempt_id, next_key)
        .unwrap();
    assert_released(&f, &next);
    assert!(f.store.peer_recovery_page("").unwrap().is_empty());
}
#[test]
fn peer_release_updates_instance_on_success_and_failure_and_corrupt_rows_remain_enumerable() {
    for fail in [false, true] {
        let (f, agent) = setup();
        let key = Uuid::new_v4();
        let turn = claim(&f, &agent, key);
        if fail {
            f.store.local_failed(&turn).unwrap();
        } else {
            f.store
                .local_finish(&turn, json!({}), "Synthetic result", false)
                .unwrap();
        }
        assert_released(&f, &turn);
        // Canonical release alone is insufficient to reuse an unacknowledged instance.
        f.store
            .local_message(
                "scan-example",
                "retry-example",
                &agent,
                "Continue synthetic work",
            )
            .unwrap();
        assert!(
            f.store
                .peer_claim_owned("scan-example", &agent, 3, 8, Uuid::new_v4(), &placement())
                .is_err()
        );
        f.db.with_connection(|c| {
            c.execute(
                "UPDATE swarm_peer_workers SET document='{' WHERE attempt=?1",
                [&turn.attempt_id],
            )?;
            Ok(())
        })
        .unwrap();
        assert!(f.reopen().peer_worker(&turn.attempt_id).is_err());
        assert_eq!(
            f.store.peer_recovery_page("").unwrap(),
            vec![turn.attempt_id.clone()]
        );
        assert!(
            f.store
                .peer_recovery_page(&turn.attempt_id)
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn peer_recovery_warning_is_owned_durable_bounded_and_atomic_on_storage_failure() {
    let (f, agent) = setup();
    let key = Uuid::new_v4();
    let turn = claim(&f, &agent, key);
    assert!(
        f.store
            .peer_recovery_warning(
                &turn.attempt_id,
                key,
                PeerRecoveryWarning::CheckpointPending
            )
            .is_err()
    );
    assert!(
        f.store
            .peer_recovery_warning(
                &turn.attempt_id,
                Uuid::new_v4(),
                PeerRecoveryWarning::PairingUnavailable
            )
            .is_err()
    );
    let before = f.store.load("scan-example", true).unwrap();
    f.db.with_connection(|c|{c.execute_batch("CREATE TRIGGER fail_warning BEFORE UPDATE ON swarm_scans BEGIN SELECT RAISE(ABORT,'synthetic warning failure'); END;")?;Ok(())}).unwrap();
    assert!(
        f.store
            .peer_recovery_warning(
                &turn.attempt_id,
                key,
                PeerRecoveryWarning::PairingUnavailable
            )
            .is_err()
    );
    assert_eq!(f.store.load("scan-example", true).unwrap(), before);
    f.db.with_connection(|c| {
        c.execute_batch("DROP TRIGGER fail_warning")?;
        Ok(())
    })
    .unwrap();
    for _ in 0..3 {
        f.reopen()
            .peer_recovery_warning(
                &turn.attempt_id,
                key,
                PeerRecoveryWarning::PairingUnavailable,
            )
            .unwrap();
    }
    let report = HostExecutionReport {
        target: f.store.peer_worker(&turn.attempt_id).unwrap().target,
        phase: HostExecutionPhase::Stopped,
        revision: 1,
        stop_requested: true,
    };
    f.store
        .peer_observe(&turn.attempt_id, key, &report)
        .unwrap();
    for _ in 0..3 {
        f.reopen()
            .peer_recovery_warning(
                &turn.attempt_id,
                key,
                PeerRecoveryWarning::CheckpointPending,
            )
            .unwrap();
    }
    let saved = f.store.load("scan-example", true).unwrap();
    assert_eq!(
        saved
            .events
            .iter()
            .filter(|e| e.kind == "peer-recovery-pending")
            .count(),
        2
    );
    for event in saved
        .events
        .iter()
        .filter(|e| e.kind == "peer-recovery-pending")
    {
        assert_eq!(event.payload["attempt_id"], turn.attempt_id);
        assert!(event.payload["warning"].as_str().unwrap().len() < 256);
        assert!(!event.payload.to_string().contains("host-example"));
    }
    assert!(!f.store.peer_worker(&turn.attempt_id).unwrap().complete);
}
#[test]
fn peer_recovery_lookup_preserves_disabled_pairing_and_ignores_unrelated_invalid_rows() {
    let f = Fixture::new();
    let peers = FleetCoordinationStore::open(f.db.clone()).unwrap();
    peers
        .save_peer(&FleetPeer {
            id: "peer-example".into(),
            endpoint: "https://example.tail-example.ts.net".into(),
            control_token: "a".repeat(64),
            enabled: false,
            protocol_major: beale_contracts::PROTOCOL_MAJOR,
        })
        .unwrap();
    f.db.with_connection(|c| {
        c.execute(
            "INSERT INTO fleet_coordination_peers VALUES('broken-example','invalid','',1,-1)",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    let peer = peers.peer("peer-example").unwrap().unwrap();
    assert!(!peer.enabled);
    assert_eq!(peer.control_token, "a".repeat(64));
    assert!(peers.peer("missing-example").unwrap().is_none());
    assert!(peers.peer("broken-example").is_err());
    assert!(peers.peers().is_err());
}
