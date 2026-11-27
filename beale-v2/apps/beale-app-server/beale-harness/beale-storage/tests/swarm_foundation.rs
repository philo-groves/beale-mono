use beale_contracts::swarm_workflows::*;
use beale_contracts::swarms::*;
use beale_storage::{
    HostDatabase,
    fleet_coordination::{FleetCoordinationStore, FleetPeer},
    swarm_state::*,
};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::{Arc, Barrier},
};
use uuid::Uuid;

struct Fixture {
    db: HostDatabase,
    store: SwarmStore,
    root: TemporaryDirectory,
}
struct TemporaryDirectory(PathBuf);
impl std::ops::Deref for TemporaryDirectory {
    type Target = std::path::Path;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("beale-swarm-test-{}", Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        let store = SwarmStore::open(db.clone()).unwrap();
        Self {
            db,
            store,
            root: TemporaryDirectory(root),
        }
    }
    fn scan(&self) -> ScanState {
        self.store.create_scan(config(), true, true).unwrap()
    }
    fn reopen(&self) -> SwarmStore {
        SwarmStore::open(HostDatabase::open(&self.root).unwrap()).unwrap()
    }
}

fn profile(id: &str) -> ProfileRef {
    ProfileRef {
        id: id.into(),
        revision: 1,
    }
}
fn config() -> ScanConfig {
    ScanConfig {
        version: VERSION,
        scan_id: "scan-example".into(),
        workspace_id: "workspace-example".into(),
        mode: ScanMode::Swarm {
            snapshot: Box::new(DefinitionSnapshot {
                id: "swarm-example".into(),
                revision: 1,
                definition: json!({"version":1,"id":"swarm-example","revision":1,"name":"Example Swarm","description":"Synthetic workflow",
                    "nodes":[{"profile":{"id":"coordinator","revision":1},"role":"leader","x":0,"y":0},{"profile":{"id":"research","revision":1},"role":"deputy","x":0,"y":1},{"profile":{"id":"investigator","revision":1},"role":"worker","x":0,"y":2}],
                    "delegation":[{"parent":"coordinator","children":["research"]},{"parent":"research","children":["investigator"]}],"initial_deputies":["research"],
                    "routes":[{"id":"research-request","from":"research","to":"research","payload":"task"}]}),
                profiles: [
                    ("coordinator", Role::Leader),
                    ("research", Role::Deputy),
                    ("investigator", Role::Worker),
                ]
                .into_iter()
                .map(|(id, role)| ProfileSnapshot {
                    profile: profile(id),
                    role,
                    instruction: "Work within the synthetic example scope.".into(),
                })
                .collect(),
                leader_rule: ReviewRuleSnapshot {
                    id: "coordinator-rule".into(),
                    revision: 3,
                    applicability: ReviewApplicability::Leader,
                    instruction: "Coordinate research and delegate dedicated research.".into(),
                },
            }),
        },
        authorized_scope: "Synthetic example.test fixtures only".into(),
        enabled_tools: vec!["findings_create".into(), "swarm_stop_subagent".into()],
        provider_routes: vec![ProviderRoute {
            id: "route-example".into(),
            provider_id: "provider-example".into(),
            model: "model-example".into(),
            budget_group_id: "budget-example".into(),
        }],
        destinations: vec![Destination {
            host_id: "host-example".into(),
            configuration_id: "container-example".into(),
        }],
        pacing: vec![PacingTarget {
            budget_group_id: "budget-example".into(),
            interval_seconds: 60,
            amount: PacingAmount::Subscription {
                window_id: "window-example".into(),
                basis_points: 100,
            },
        }],
        retention: RetentionPolicy::default(),
    }
}
fn task() -> Task {
    Task {
        question: "Inspect the synthetic fixture".into(),
        completion_criteria: vec!["Return evidence references".into()],
        evidence_refs: vec![],
    }
}
fn registration() -> WorkerRegistration {
    WorkerRegistration {
        version: VERSION,
        audience: WORKER_AUDIENCE.into(),
        capabilities: vec![
            WorkerCapability::CanonicalRouting,
            WorkerCapability::ArtifactTransfer,
            WorkerCapability::DurableEvents,
        ],
    }
}
fn request(agent: &str, attempt: &ExecutionAttempt) -> WorkerRequest {
    WorkerRequest {
        version: VERSION,
        workspace_id: "workspace-example".into(),
        scan_id: "scan-example".into(),
        agent_id: agent.into(),
        attempt_id: attempt.id.clone(),
        generation: attempt.generation,
        operation_id: "operation-example".into(),
        operation: WorkerOperation::Canonical {
            tool: "findings_create".into(),
            arguments: json!({"title":"Synthetic observation"}),
        },
    }
}
fn child(store: &SwarmStore, parent: &str, operation: &str, p: &str) -> String {
    match store
        .queue_spawn(
            "scan-example",
            parent,
            operation,
            profile(p),
            task(),
            "capacity pending",
        )
        .unwrap()
    {
        SpawnResult::Queued { child_id, .. } => child_id,
        _ => panic!("expected queued"),
    }
}
fn environment(agent: &str) -> EnvironmentState {
    EnvironmentState {
        id: "environment-example".into(),
        agent_id: agent.into(),
        generation: 1,
        retention_deadline_ms: None,
        keep: false,
        active_references: 0,
        pending_publications: vec![],
        termination_confirmed: true,
        deletion: DeletionState::Retained,
    }
}

fn execution(attempt: &ExecutionAttempt, now_ms: i64) -> ExecutionWriteContext {
    ExecutionWriteContext {
        attempt_id: attempt.id.clone(),
        generation: attempt.generation,
        now_ms,
    }
}

fn tool_operation(agent: &str, attempt: &ExecutionAttempt) -> ToolOperation {
    ToolOperation {
        id: "operation-regression".into(),
        agent_id: agent.into(),
        attempt_id: attempt.id.clone(),
        request: json!({"command":"synthetic-command"}),
        state: ToolOperationState::NotStarted,
        result: json!(null),
        revision: 1,
    }
}

fn instance_for(agent: &str, attempt: &ExecutionAttempt) -> ExecutionInstance {
    ExecutionInstance {
        id: "instance-followup".into(),
        configuration_id: "container-example".into(),
        host_id: "host-example".into(),
        agent_id: agent.into(),
        attempt_id: attempt.id.clone(),
        state: InstanceState::Running,
    }
}

fn start_journal(f: &Fixture, agent: &str, attempt: &ExecutionAttempt) -> ToolOperation {
    let mut op = tool_operation(agent, attempt);
    f.store
        .journal_operation("scan-example", op.clone(), None, &execution(attempt, 1000))
        .unwrap();
    op.state = ToolOperationState::Running;
    op.revision = 2;
    f.store
        .journal_operation(
            "scan-example",
            op.clone(),
            Some(1),
            &execution(attempt, 1000),
        )
        .unwrap();
    op
}

#[test]
fn late_instance_registration_cannot_bypass_termination_and_cleanup_guards() {
    let f = Fixture::new();
    let state = f.scan();
    let agent = &state.agents[0].summary.id;
    let (attempt, _) = f
        .store
        .start_attempt("scan-example", agent, &registration(), 1000, 2000)
        .unwrap();
    f.store
        .fence_attempt("scan-example", &attempt.id, false)
        .unwrap();
    let fenced = f.store.load("scan-example", true).unwrap();
    for status in [InstanceState::Starting, InstanceState::Running] {
        let mut instance = instance_for(agent, &attempt);
        instance.state = status;
        assert!(f.store.register_instance("scan-example", instance).is_err());
        assert_eq!(f.store.load("scan-example", true).unwrap(), fenced);
    }
    f.store
        .fence_attempt("scan-example", &attempt.id, true)
        .unwrap();
    f.store
        .put_environment("scan-example", environment(agent))
        .unwrap();
    f.store.end_scan("scan-example", 1000).unwrap();
    let before = f.store.load("scan-example", true).unwrap();
    for status in [
        InstanceState::Starting,
        InstanceState::Running,
        InstanceState::TerminationUnconfirmed,
    ] {
        let mut instance = instance_for(agent, &attempt);
        instance.state = status;
        assert!(f.store.register_instance("scan-example", instance).is_err());
        assert_eq!(f.reopen().load("scan-example", true).unwrap(), before);
    }
    let claims =
        f.db.with_connection(|db| {
            Ok(
                db.query_row("SELECT COUNT(*) FROM swarm_instance_owners", [], |r| {
                    r.get::<_, u32>(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!(claims, 0);
    // A factual stopped-instance inventory record is allowed and is cleanup-safe.
    let mut stopped = instance_for(agent, &attempt);
    stopped.state = InstanceState::Stopped;
    f.store.register_instance("scan-example", stopped).unwrap();
    f.store
        .change_environment(
            "scan-example",
            "environment-example",
            2,
            604801000,
            EnvironmentChange::ClaimCleanup,
        )
        .unwrap();
    let mut invalid = f.store.load("scan-example", true).unwrap();
    invalid.instances[0].state = InstanceState::Running;
    assert!(invalid.validate().is_err());
}

#[test]
fn racing_instance_registration_and_stop_confirmation_never_leave_live_metadata() {
    let f = Fixture::new();
    let state = f.scan();
    let agent = &state.agents[0].summary.id;
    let (attempt, _) = f
        .store
        .start_attempt("scan-example", agent, &registration(), 1000, 2000)
        .unwrap();
    let instance = instance_for(agent, &attempt);
    let registering = f.reopen();
    let stopping = f.reopen();
    let barrier = Arc::new(Barrier::new(2));
    let other = barrier.clone();
    let register = std::thread::spawn(move || {
        barrier.wait();
        registering.register_instance("scan-example", instance)
    });
    let stop = std::thread::spawn(move || {
        other.wait();
        stopping.fence_attempt("scan-example", &attempt.id, true)
    });
    let registered = register.join().unwrap();
    stop.join().unwrap().unwrap();
    let saved = f.reopen().load("scan-example", true).unwrap();
    assert_eq!(saved.attempts[0].state, ExecutionState::Stopped);
    assert_eq!(saved.instances.len(), usize::from(registered.is_ok()));
    assert!(
        saved
            .instances
            .iter()
            .all(|i| i.state == InstanceState::Stopped)
    );
    let claims =
        f.db.with_connection(|db| {
            Ok(
                db.query_row("SELECT COUNT(*) FROM swarm_instance_owners", [], |r| {
                    r.get::<_, u32>(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!(claims, u32::try_from(saved.instances.len()).unwrap());
}

#[test]
fn duplicate_stop_confirmation_preserves_cleanup_claim_and_deleted_generation() {
    let f = Fixture::new();
    let state = f.scan();
    let agent = &state.agents[0].summary.id;
    f.store
        .put_environment("scan-example", environment(agent))
        .unwrap();
    f.store
        .change_environment(
            "scan-example",
            "environment-example",
            1,
            1000,
            EnvironmentChange::Resume,
        )
        .unwrap();
    let (attempt, _) = f
        .store
        .start_attempt("scan-example", agent, &registration(), 1000, 2000)
        .unwrap();
    f.store
        .register_instance("scan-example", instance_for(agent, &attempt))
        .unwrap();
    f.store
        .fence_attempt("scan-example", &attempt.id, true)
        .unwrap();
    let env = f
        .store
        .load("scan-example", true)
        .unwrap()
        .environments
        .remove(0);
    assert!(env.termination_confirmed);
    f.store
        .change_environment(
            "scan-example",
            &env.id,
            env.generation,
            1000,
            EnvironmentChange::Release,
        )
        .unwrap();
    f.store.end_scan("scan-example", 1000).unwrap();
    let env = f
        .store
        .load("scan-example", true)
        .unwrap()
        .environments
        .remove(0);
    let claimed = f
        .store
        .change_environment(
            "scan-example",
            &env.id,
            env.generation,
            604801000,
            EnvironmentChange::ClaimCleanup,
        )
        .unwrap();
    let before = f.store.load("scan-example", true).unwrap();
    for confirmed in [true, false, true] {
        f.reopen()
            .fence_attempt("scan-example", &attempt.id, confirmed)
            .unwrap();
    }
    assert_eq!(f.store.load("scan-example", true).unwrap(), before);
    let deleted = f
        .store
        .change_environment(
            "scan-example",
            &claimed.id,
            claimed.generation,
            604801001,
            EnvironmentChange::ConfirmDeleted,
        )
        .unwrap();
    let before = f.store.load("scan-example", true).unwrap();
    f.store
        .fence_attempt("scan-example", &attempt.id, true)
        .unwrap();
    assert_eq!(f.reopen().load("scan-example", true).unwrap(), before);
    assert_eq!(before.environments[0], deleted);
}

#[test]
fn stopped_journals_can_be_recovered_after_expiry_or_scan_end_without_worker_authority() {
    for end_scan in [false, true] {
        let f = Fixture::new();
        let state = f.scan();
        let agent = &state.agents[0].summary.id;
        let (attempt, token) = f
            .store
            .start_attempt("scan-example", agent, &registration(), 1000, 2000)
            .unwrap();
        let operation = start_journal(&f, agent, &attempt);
        if end_scan {
            f.store.end_scan("scan-example", 1100).unwrap();
        } else {
            assert!(
                f.store
                    .authorize(
                        token.bearer(),
                        WORKER_AUDIENCE,
                        &request(agent, &attempt),
                        2000
                    )
                    .is_err()
            );
        }
        let before = f.store.load("scan-example", true).unwrap();
        assert!(
            f.store
                .recover_operation(
                    "scan-example",
                    "recovery-unknown",
                    &operation.id,
                    2,
                    JournalRecovery::OutcomeUnknown
                )
                .is_err()
        );
        assert_eq!(f.store.load("scan-example", true).unwrap(), before);
        f.store
            .fence_attempt("scan-example", &attempt.id, true)
            .unwrap();
        let unknown = f
            .store
            .recover_operation(
                "scan-example",
                "recovery-unknown",
                &operation.id,
                2,
                JournalRecovery::OutcomeUnknown,
            )
            .unwrap();
        assert_eq!(unknown.state, ToolOperationState::OutcomeUnknown);
        assert_eq!(unknown.attempt_id, attempt.id);
        let before = f.reopen().load("scan-example", true).unwrap();
        assert_eq!(
            f.store
                .recover_operation(
                    "scan-example",
                    "recovery-unknown",
                    &operation.id,
                    2,
                    JournalRecovery::OutcomeUnknown
                )
                .unwrap(),
            unknown
        );
        assert_eq!(f.store.load("scan-example", true).unwrap(), before);
        assert!(
            f.store
                .authorize(
                    token.bearer(),
                    WORKER_AUDIENCE,
                    &request(agent, &attempt),
                    1200
                )
                .is_err()
        );
        assert!(
            f.store
                .renew(token.bearer(), &request(agent, &attempt), 1200, 3000)
                .is_err()
        );
        let mut forged = unknown.clone();
        forged.state = ToolOperationState::Completed;
        forged.revision = 4;
        assert!(
            f.store
                .journal_operation("scan-example", forged, Some(3), &execution(&attempt, 1200))
                .is_err()
        );
        let completed = f
            .reopen()
            .recover_operation(
                "scan-example",
                "recovery-completed",
                &operation.id,
                3,
                JournalRecovery::Completed {
                    result: json!(null),
                    evidence_refs: vec!["evidence-host-example".into()],
                },
            )
            .unwrap();
        assert_eq!(completed.result, json!(null));
        assert_eq!(completed.state, ToolOperationState::Completed);
        let recovered = f.store.load("scan-example", true).unwrap();
        assert_eq!(recovered.events.len(), 2);
        assert_eq!(recovered.events[1].payload["attempt_id"], attempt.id);
        assert_eq!(
            recovered.events[1].payload["recovery"]["evidence_refs"],
            json!(["evidence-host-example"])
        );
        assert_eq!(recovered.assignments, before.assignments);
        assert_eq!(recovered.agents, before.agents);
        assert_eq!(recovered.attempts, before.attempts);
    }
    assert!(
        serde_json::from_value::<WorkerOperation>(json!({"kind":"recover_operation"})).is_err()
    );
}

#[test]
fn journal_recovery_is_content_bound_and_cannot_overwrite_known_outcomes() {
    let f = Fixture::new();
    let state = f.scan();
    let agent = &state.agents[0].summary.id;
    let (attempt, _) = f
        .store
        .start_attempt("scan-example", agent, &registration(), 1000, 2000)
        .unwrap();
    let operation = start_journal(&f, agent, &attempt);
    f.store
        .fence_attempt("scan-example", &attempt.id, true)
        .unwrap();
    let (replacement, _) = f
        .store
        .start_attempt("scan-example", agent, &registration(), 1100, 3000)
        .unwrap();
    let replacement_before = f.store.load("scan-example", true).unwrap();
    for evidence_refs in [
        vec![],
        vec!["duplicate-example".into(), "duplicate-example".into()],
        vec!["invalid/reference".into()],
    ] {
        assert!(
            f.store
                .recover_operation(
                    "scan-example",
                    "invalid-evidence",
                    &operation.id,
                    2,
                    JournalRecovery::Completed {
                        result: json!({}),
                        evidence_refs
                    }
                )
                .is_err()
        );
        assert_eq!(
            f.store.load("scan-example", true).unwrap(),
            replacement_before
        );
    }
    assert!(
        f.store
            .recover_operation(
                "scan-example",
                "wrong-revision",
                &operation.id,
                1,
                JournalRecovery::OutcomeUnknown
            )
            .is_err()
    );
    assert!(
        f.store
            .recover_operation(
                "scan-example",
                "missing-operation",
                "missing-example",
                2,
                JournalRecovery::OutcomeUnknown
            )
            .is_err()
    );
    let outcome = JournalRecovery::Canceled {
        evidence_refs: vec!["supervisor-cancellation-example".into()],
    };
    let canceled = f
        .store
        .recover_operation(
            "scan-example",
            "recovery-canceled",
            &operation.id,
            2,
            outcome.clone(),
        )
        .unwrap();
    assert_eq!(canceled.state, ToolOperationState::Canceled);
    assert_eq!(canceled.attempt_id, attempt.id);
    let before = f.reopen().load("scan-example", true).unwrap();
    assert_eq!(before.attempts, replacement_before.attempts);
    assert_eq!(before.attempts.last().unwrap().id, replacement.id);
    assert_eq!(
        f.store
            .recover_operation(
                "scan-example",
                "recovery-canceled",
                &operation.id,
                2,
                outcome
            )
            .unwrap(),
        canceled
    );
    assert!(
        f.store
            .recover_operation(
                "scan-example",
                "recovery-canceled",
                &operation.id,
                2,
                JournalRecovery::OutcomeUnknown
            )
            .is_err()
    );
    assert!(
        f.store
            .recover_operation(
                "scan-example",
                "overwrite-canceled",
                &operation.id,
                3,
                JournalRecovery::Completed {
                    result: json!({}),
                    evidence_refs: vec!["evidence-example".into()]
                }
            )
            .is_err()
    );
    assert_eq!(f.store.load("scan-example", true).unwrap(), before);
}

#[test]
fn failed_journal_recovery_rolls_back_outcome_event_and_receipt_together() {
    let f = Fixture::new();
    let state = f.scan();
    let agent = &state.agents[0].summary.id;
    let (attempt, _) = f
        .store
        .start_attempt("scan-example", agent, &registration(), 1000, 2000)
        .unwrap();
    let operation = start_journal(&f, agent, &attempt);
    f.store
        .fence_attempt("scan-example", &attempt.id, true)
        .unwrap();
    f.db.with_connection(|db| {db.execute_batch("CREATE TRIGGER reject_recovery_receipt BEFORE INSERT ON swarm_receipts WHEN NEW.actor='host-journal-recovery' BEGIN SELECT RAISE(ABORT,'synthetic receipt failure'); END;")?;Ok(())}).unwrap();
    let before = f.store.load("scan-example", true).unwrap();
    assert!(
        f.store
            .recover_operation(
                "scan-example",
                "recovery-retry",
                &operation.id,
                2,
                JournalRecovery::OutcomeUnknown
            )
            .is_err()
    );
    assert_eq!(f.reopen().load("scan-example", true).unwrap(), before);
    let receipts =
        f.db.with_connection(|db| {
            Ok(db.query_row(
                "SELECT COUNT(*) FROM swarm_receipts WHERE actor='host-journal-recovery'",
                [],
                |r| r.get::<_, u32>(0),
            )?)
        })
        .unwrap();
    assert_eq!(receipts, 0);
    f.db.with_connection(|db| {
        db.execute_batch("DROP TRIGGER reject_recovery_receipt;")?;
        Ok(())
    })
    .unwrap();
    f.store
        .recover_operation(
            "scan-example",
            "recovery-retry",
            &operation.id,
            2,
            JournalRecovery::OutcomeUnknown,
        )
        .unwrap();
    let saved = f.reopen().load("scan-example", true).unwrap();
    assert_eq!(saved.events.len(), 1);
    assert_eq!(
        saved.operations[0].state,
        ToolOperationState::OutcomeUnknown
    );
}

#[test]
fn completed_tool_results_including_null_survive_reopen_and_further_writes() {
    for result in [
        json!(null),
        json!(false),
        json!(42),
        json!("synthetic"),
        json!([]),
        json!({"evidence":"synthetic"}),
    ] {
        let f = Fixture::new();
        let state = f.scan();
        let agent = &state.agents[0].summary.id;
        let (attempt, _) = f
            .store
            .start_attempt("scan-example", agent, &registration(), 1000, 2000)
            .unwrap();
        let mut op = tool_operation(agent, &attempt);
        let context = execution(&attempt, 1100);
        f.store
            .journal_operation("scan-example", op.clone(), None, &context)
            .unwrap();
        op.state = ToolOperationState::Running;
        op.revision = 2;
        f.store
            .journal_operation("scan-example", op.clone(), Some(1), &context)
            .unwrap();
        op.state = ToolOperationState::Completed;
        op.result = result.clone();
        op.revision = 3;
        f.store
            .journal_operation("scan-example", op.clone(), Some(2), &context)
            .unwrap();
        let reopened = f.reopen();
        let saved = reopened.load("scan-example", true).unwrap();
        assert_eq!(saved.operations, vec![op]);
        reopened
            .checkpoint_execution(
                "scan-example",
                agent,
                1,
                state.agents[0].checkpoint.clone(),
                &context,
            )
            .unwrap();
        reopened
            .fence_attempt("scan-example", &attempt.id, true)
            .unwrap();
        assert_eq!(
            reopened.load("scan-example", true).unwrap().operations[0].result,
            result
        );
    }
}

#[test]
fn checkpoint_and_journal_writes_are_fenced_by_attempt_generation_and_expiry() {
    let f = Fixture::new();
    let state = f.scan();
    let agent = &state.agents[0].summary.id;
    let (old, _) = f
        .store
        .start_attempt("scan-example", agent, &registration(), 1000, 2000)
        .unwrap();
    let mut checkpoint = state.agents[0].checkpoint.clone();
    checkpoint.context = json!({"summary":"old synthetic context"});
    let mut op = tool_operation(agent, &old);
    f.store
        .journal_operation("scan-example", op.clone(), None, &execution(&old, 1000))
        .unwrap();
    for context in [
        execution(&old, 2000),
        ExecutionWriteContext {
            generation: old.generation + 1,
            ..execution(&old, 1000)
        },
    ] {
        let before = f.store.load("scan-example", true).unwrap();
        assert!(
            f.store
                .checkpoint_execution("scan-example", agent, 1, checkpoint.clone(), &context)
                .is_err()
        );
        op.state = ToolOperationState::Running;
        op.revision = 2;
        assert!(
            f.store
                .journal_operation("scan-example", op.clone(), Some(1), &context)
                .is_err()
        );
        assert_eq!(f.store.load("scan-example", true).unwrap(), before);
    }
    f.store
        .fence_attempt("scan-example", &old.id, true)
        .unwrap();
    let (new, _) = f
        .store
        .start_attempt("scan-example", agent, &registration(), 1000, 2000)
        .unwrap();
    let before = f.store.load("scan-example", true).unwrap();
    assert!(
        f.store
            .checkpoint_execution(
                "scan-example",
                agent,
                1,
                checkpoint.clone(),
                &execution(&old, 1100)
            )
            .is_err()
    );
    assert!(
        f.store
            .checkpoint("scan-example", agent, 1, checkpoint.clone())
            .is_err()
    );
    assert!(
        f.store
            .journal_operation("scan-example", op.clone(), Some(1), &execution(&old, 1100))
            .is_err()
    );
    assert!(
        f.store
            .journal_operation("scan-example", op, Some(1), &execution(&new, 1100))
            .is_err()
    );
    assert_eq!(f.reopen().load("scan-example", true).unwrap(), before);
    checkpoint.context = json!({"summary":"current synthetic context"});
    f.store
        .checkpoint_execution(
            "scan-example",
            agent,
            1,
            checkpoint.clone(),
            &execution(&new, 1100),
        )
        .unwrap();
    assert_eq!(
        f.reopen()
            .load("scan-example", true)
            .unwrap()
            .agent(agent)
            .unwrap()
            .checkpoint,
        checkpoint
    );
}

#[test]
fn scan_end_revokes_research_without_claiming_process_termination() {
    let f = Fixture::new();
    let state = f.scan();
    let agent = &state.agents[0].summary.id;
    let deputy = child(&f.store, agent, "spawn-regression", "research");
    f.store
        .put_environment("scan-example", environment(agent))
        .unwrap();
    f.store
        .change_environment(
            "scan-example",
            "environment-example",
            1,
            1000,
            EnvironmentChange::Resume,
        )
        .unwrap();
    let (attempt, token) = f
        .store
        .start_attempt("scan-example", agent, &registration(), 1000, 2000)
        .unwrap();
    let (child_attempt, child_token) = f
        .store
        .start_attempt("scan-example", &deputy, &registration(), 1000, 2000)
        .unwrap();
    f.store
        .register_instance(
            "scan-example",
            ExecutionInstance {
                id: "instance-regression".into(),
                configuration_id: "container-example".into(),
                host_id: "host-example".into(),
                agent_id: agent.clone(),
                attempt_id: attempt.id.clone(),
                state: InstanceState::Running,
            },
        )
        .unwrap();
    f.store.end_scan("scan-example", 1100).unwrap();
    let before = f.store.load("scan-example", true).unwrap();
    f.store.end_scan("scan-example", 1200).unwrap();
    assert_eq!(f.store.load("scan-example", true).unwrap(), before);
    assert!(
        before
            .attempts
            .iter()
            .all(|a| a.state == ExecutionState::TerminationUnconfirmed)
    );
    assert_eq!(
        before.instances[0].state,
        InstanceState::TerminationUnconfirmed
    );
    assert!(!before.environments[0].termination_confirmed);
    for (actor, a, t) in [
        (agent.as_str(), &attempt, &token),
        (deputy.as_str(), &child_attempt, &child_token),
    ] {
        let mut req = request(actor, a);
        assert!(
            f.store
                .dispatch_mutation(t.bearer(), WORKER_AUDIENCE, &req, 1200, |_, _| panic!(
                    "ended scan dispatched"
                ))
                .is_err()
        );
        assert!(f.store.renew(t.bearer(), &req, 1200, 3000).is_err());
        req.operation = WorkerOperation::Infer {
            route_id: "route-example".into(),
            messages: json!([]),
        };
        assert!(
            f.store
                .authorize(t.bearer(), WORKER_AUDIENCE, &req, 1200)
                .is_err()
        );
    }
    assert!(
        f.store
            .queue_spawn(
                "scan-example",
                agent,
                "spawn-after-end",
                profile("research"),
                task(),
                "waiting"
            )
            .is_err()
    );
    assert_eq!(f.store.load("scan-example", true).unwrap(), before);
    f.store.reopen_scan("scan-example", true).unwrap();
    assert!(
        f.store
            .authorize(
                token.bearer(),
                WORKER_AUDIENCE,
                &request(agent, &attempt),
                1200
            )
            .is_err()
    );
    assert!(
        f.store
            .start_attempt("scan-example", agent, &registration(), 1200, 3000)
            .is_err()
    );
    // Trusted supervisor confirmation remains available after end; old bearers stay revoked.
    f.store
        .fence_attempt("scan-example", &attempt.id, true)
        .unwrap();
    f.store
        .start_attempt("scan-example", agent, &registration(), 1200, 3000)
        .unwrap();
}

#[test]
fn independent_scans_cannot_claim_the_same_host_instance() {
    let f = Fixture::new();
    let first = f.scan();
    let mut config = config();
    config.scan_id = "scan-other-example".into();
    let second = f.store.create_scan(config, true, true).unwrap();
    let mut claims = vec![];
    for scan in [&first, &second] {
        let agent = &scan.agents[0].summary.id;
        let (attempt, _) = f
            .store
            .start_attempt(&scan.config.scan_id, agent, &registration(), 1000, 2000)
            .unwrap();
        claims.push((
            scan.config.scan_id.clone(),
            ExecutionInstance {
                id: "shared-instance-example".into(),
                configuration_id: "container-example".into(),
                host_id: "host-example".into(),
                agent_id: agent.clone(),
                attempt_id: attempt.id,
                state: InstanceState::Running,
            },
        ));
    }
    let barrier = Arc::new(Barrier::new(2));
    let handles = claims
        .iter()
        .cloned()
        .map(|(scan, instance)| {
            let store = f.reopen();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store.register_instance(&scan, instance)
            })
        })
        .collect::<Vec<_>>();
    let results = handles
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    for ((scan, instance), result) in claims.iter().zip(&results) {
        let saved = f.reopen().load(scan, true).unwrap();
        assert_eq!(saved.instances.len(), usize::from(result.is_ok()));
        if result.is_ok() {
            f.store.register_instance(scan, instance.clone()).unwrap();
        } else {
            assert!(
                f.reopen()
                    .register_instance(scan, instance.clone())
                    .is_err()
            );
            let mut different_host = instance.clone();
            different_host.host_id = "host-other-example".into();
            f.store.register_instance(scan, different_host).unwrap();
        }
    }
}

#[test]
fn environment_registration_and_scan_end_share_the_authoritative_deadline() {
    let f = Fixture::new();
    let state = f.scan();
    let agent = &state.agents[0].summary.id;
    let store = f.reopen();
    let end_store = f.store.clone();
    let environment = environment(agent);
    let barrier = Arc::new(Barrier::new(2));
    let other_barrier = barrier.clone();
    let registration = std::thread::spawn(move || {
        barrier.wait();
        store.put_environment("scan-example", environment)
    });
    let end = std::thread::spawn(move || {
        other_barrier.wait();
        end_store.end_scan("scan-example", 1000)
    });
    registration.join().unwrap().unwrap();
    end.join().unwrap().unwrap();
    let saved = f.reopen().load("scan-example", true).unwrap();
    assert_eq!(saved.environments[0].retention_deadline_ms, Some(604801000));
    let mut late = saved.environments[0].clone();
    late.id = "late-environment-example".into();
    late.generation = 1;
    late.retention_deadline_ms = None;
    f.store
        .put_environment("scan-example", late.clone())
        .unwrap();
    late.id = "invalid-deadline-example".into();
    late.retention_deadline_ms = Some(0);
    let before = f.store.load("scan-example", true).unwrap();
    assert!(f.store.put_environment("scan-example", late).is_err());
    assert_eq!(f.store.load("scan-example", true).unwrap(), before);
    let saved = f.reopen().load("scan-example", true).unwrap();
    for env in saved.environments {
        assert_eq!(env.retention_deadline_ms, Some(604801000));
        assert!(
            f.store
                .change_environment(
                    "scan-example",
                    &env.id,
                    env.generation,
                    604800999,
                    EnvironmentChange::ClaimCleanup
                )
                .is_err()
        );
        f.store
            .change_environment(
                "scan-example",
                &env.id,
                env.generation,
                604801000,
                EnvironmentChange::ClaimCleanup,
            )
            .unwrap();
    }
}

#[test]
fn pacing_replay_survives_handover_without_admitting_stale_work() {
    let f = Fixture::new();
    let fleet = FleetCoordinationStore::open(f.db.clone()).unwrap();
    let mut group = fleet
        .account_group("provider-example", "account-example", "host-example")
        .unwrap();
    group.participants = vec!["leader-example".into()];
    group.owner_generation = 2;
    fleet.update_group(&group, 1).unwrap();
    let reservation = PacingReservation {
        id: "reservation-regression".into(),
        group_id: group.id.clone(),
        owner_generation: 2,
        participant_id: "leader-example".into(),
        estimated_micros: 10,
    };
    fleet.record_pacing_reservation(&reservation).unwrap();
    group.owner_host_id = "host-other-example".into();
    group.owner_generation = 3;
    fleet.update_group(&group, 2).unwrap();
    let reopened = FleetCoordinationStore::open(HostDatabase::open(&f.root).unwrap()).unwrap();
    reopened.record_pacing_reservation(&reservation).unwrap();
    let mut changed = reservation.clone();
    changed.estimated_micros += 1;
    assert!(reopened.record_pacing_reservation(&changed).is_err());
    changed = reservation.clone();
    changed.owner_generation = 3;
    assert!(reopened.record_pacing_reservation(&changed).is_err());
    let mut fresh = reservation.clone();
    fresh.id = "new-reservation-example".into();
    assert!(reopened.record_pacing_reservation(&fresh).is_err());
    assert_eq!(
        reopened.pacing_reservations(&group.id).unwrap(),
        vec![reservation.clone()]
    );
    fresh.owner_generation = 3;
    reopened.record_pacing_reservation(&fresh).unwrap();
    group.participants.clear();
    group.owner_generation = 4;
    fleet.update_group(&group, 3).unwrap();
    assert!(reopened.record_pacing_reservation(&reservation).is_err());
    assert_eq!(reopened.pacing_reservations(&group.id).unwrap().len(), 2);
}

// Recreate the previous owner schema on an isolated fixture to exercise upgrade,
// without a compatibility reader or changes to the persisted scan documents.
fn restore_v1_instance_schema(db: &HostDatabase) {
    db.with_connection(|db| {db.execute_batch("DROP TABLE swarm_instance_owners; UPDATE schema_versions SET version=1 WHERE owner='swarms';")?;Ok(())}).unwrap();
}

#[test]
fn instance_ownership_upgrade_preserves_existing_claims() {
    let f = Fixture::new();
    let state = f.scan();
    let agent = &state.agents[0].summary.id;
    let (attempt, _) = f
        .store
        .start_attempt("scan-example", agent, &registration(), 1000, 2000)
        .unwrap();
    f.store
        .register_instance(
            "scan-example",
            ExecutionInstance {
                id: "instance-upgrade".into(),
                configuration_id: "container-example".into(),
                host_id: "host-example".into(),
                agent_id: agent.clone(),
                attempt_id: attempt.id,
                state: InstanceState::Running,
            },
        )
        .unwrap();
    let before = f.store.load("scan-example", true).unwrap();
    restore_v1_instance_schema(&f.db);
    let reopened = f.reopen();
    assert_eq!(f.db.schema_version("swarms").unwrap(), 2);
    assert_eq!(reopened.load("scan-example", true).unwrap(), before);
    let owner =
        f.db.with_connection(|db| {
            Ok(db.query_row(
                "SELECT scan_id,agent_id FROM swarm_instance_owners",
                [],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )?)
        })
        .unwrap();
    assert_eq!(owner, ("scan-example".into(), agent.clone()));
}

#[test]
fn conflicting_instance_ownership_aborts_upgrade_without_deleting_state() {
    let f = Fixture::new();
    let state = f.scan();
    let agent = &state.agents[0].summary.id;
    let (attempt, _) = f
        .store
        .start_attempt("scan-example", agent, &registration(), 1000, 2000)
        .unwrap();
    f.store
        .register_instance(
            "scan-example",
            ExecutionInstance {
                id: "instance-upgrade".into(),
                configuration_id: "container-example".into(),
                host_id: "host-example".into(),
                agent_id: agent.clone(),
                attempt_id: attempt.id,
                state: InstanceState::Running,
            },
        )
        .unwrap();
    restore_v1_instance_schema(&f.db);
    f.db.with_connection(|db| {db.execute("INSERT INTO swarm_scans SELECT 'scan-conflict-example',json_set(document,'$.config.scan_id','scan-conflict-example') FROM swarm_scans WHERE id='scan-example'",[])?;Ok(())}).unwrap();
    assert!(SwarmStore::open(f.db.clone()).is_err());
    assert_eq!(f.db.schema_version("swarms").unwrap(), 1);
    let counts =
        f.db.with_connection(|db| {
            Ok((
                db.query_row("SELECT COUNT(*) FROM swarm_scans", [], |r| {
                    r.get::<_, u32>(0)
                })?,
                db.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name='swarm_instance_owners'",
                    [],
                    |r| r.get::<_, u32>(0),
                )?,
            ))
        })
        .unwrap();
    assert_eq!(counts, (2, 0));
}

#[test]
fn contracts_reject_malformed_roles_states_versions_and_parent_relationships() {
    let f = Fixture::new();
    let state = f.scan();
    assert_eq!(
        serde_json::from_value::<ScanState>(serde_json::to_value(&state).unwrap()).unwrap(),
        state
    );
    for field in ["role", "state"] {
        let mut value = serde_json::to_value(&state).unwrap();
        value["agents"][0]["summary"][field] = json!("invented");
        assert!(serde_json::from_value::<ScanState>(value).is_err());
    }
    let mut bad = state.clone();
    bad.agents[0].summary.parent_id = Some(bad.agents[0].summary.id.clone());
    assert!(bad.validate().is_err());
    let mut bad = config();
    bad.version += 1;
    assert!(bad.validate().is_err());
    let mut reg = registration();
    reg.version += 1;
    assert!(reg.validate().is_err());
    let mut reg = registration();
    reg.capabilities.pop();
    assert!(reg.validate().is_err());
    let mut reg = registration();
    reg.capabilities[0] = WorkerCapability::DurableEvents;
    assert!(reg.validate().is_err());
    assert!(
        serde_json::from_value::<WorkerOperation>(
            json!({"kind":"host_admin","command":"shutdown"})
        )
        .is_err()
    );
}

#[test]
fn feature_toggle_only_gates_new_scans_and_never_downgrades_saved_configuration() {
    let f = Fixture::new();
    let state = f.scan();
    assert!(f.store.create_scan(config(), false, true).is_err());
    assert!(f.store.create_scan(config(), true, false).is_err());
    assert!(f.store.load("scan-example", false).is_err());
    assert_eq!(f.reopen().load("scan-example", true).unwrap(), state);
    let mut local = config();
    local.mode = ScanMode::Local {};
    assert!(local.validate_creation(false, true).is_err());
    local.destinations.clear();
    local.scan_id = "local-example".into();
    let local_state = f.store.create_scan(local, false, false).unwrap();
    assert!(local_state.agents.is_empty());
    assert_eq!(f.store.load("local-example", false).unwrap(), local_state);
    assert_eq!(state.config.retention.ended_scan_seconds, 604800);
    assert_eq!(
        state.config.retention.storage_reserve_bytes,
        5 * 1024 * 1024 * 1024
    );
}

#[test]
fn queued_spawn_is_idempotent_and_does_not_claim_admission() {
    let f = Fixture::new();
    let state = f.scan();
    let leader = &state.agents[0].summary.id;
    let first = child(&f.store, leader, "spawn-example", "research");
    assert_eq!(first, child(&f.store, leader, "spawn-example", "research"));
    let mut changed = task();
    changed.question = "Different task".into();
    assert!(
        f.store
            .queue_spawn(
                "scan-example",
                leader,
                "spawn-example",
                profile("research"),
                changed,
                "capacity pending"
            )
            .is_err()
    );
    assert!(
        f.store
            .queue_spawn(
                "scan-example",
                leader,
                "wrong-role",
                profile("investigator"),
                task(),
                "capacity pending"
            )
            .is_err()
    );
    let saved = f.reopen().load("scan-example", true).unwrap();
    assert_eq!(saved.agents.len(), 2);
    assert_eq!(saved.assignments.len(), 1);
    assert!(
        saved.reservations.is_empty() && saved.attempts.is_empty() && saved.instances.is_empty()
    );
    assert_eq!(
        saved.agent(&first).unwrap().summary.state,
        AgentState::Queued
    );
}

#[test]
fn authoritative_dispatch_rejects_impersonation_expiry_revocation_and_bypassed_parent() {
    let f = Fixture::new();
    let state = f.scan();
    let leader = &state.agents[0].summary.id;
    let deputy = child(&f.store, leader, "spawn-deputy", "research");
    let worker = child(&f.store, &deputy, "spawn-worker", "investigator");
    let (attempt, token) = f
        .store
        .start_attempt("scan-example", &deputy, &registration(), 1000, 2000)
        .unwrap();
    let req = request(&deputy, &attempt);
    let context = f
        .store
        .authorize(token.bearer(), WORKER_AUDIENCE, &req, 1000)
        .unwrap();
    assert_eq!(context.agent.role, Role::Deputy);
    assert!(context.review_rule.is_none());
    assert_eq!(context.authorized_scope, state.config.authorized_scope);
    assert_eq!(context.provider_routes, state.config.provider_routes);
    for field in ["agent_id", "workspace_id", "scan_id", "attempt_id"] {
        let mut value = serde_json::to_value(&req).unwrap();
        value[field] = json!("other-example");
        assert!(
            f.store
                .authorize(
                    token.bearer(),
                    WORKER_AUDIENCE,
                    &serde_json::from_value(value).unwrap(),
                    1000
                )
                .is_err()
        );
    }
    assert!(
        f.store
            .authorize(token.bearer(), "host-control", &req, 1000)
            .is_err()
    );
    assert!(
        f.store
            .authorize(token.bearer(), WORKER_AUDIENCE, &req, 2000)
            .is_err()
    );
    f.store.renew(token.bearer(), &req, 1500, 3000).unwrap();
    f.store
        .authorize(token.bearer(), WORKER_AUDIENCE, &req, 2000)
        .unwrap();
    let mut invalid = req.clone();
    invalid.operation = WorkerOperation::Canonical {
        tool: "host_shutdown".into(),
        arguments: json!({}),
    };
    assert!(
        f.store
            .authorize(token.bearer(), WORKER_AUDIENCE, &invalid, 2000)
            .is_err()
    );
    let (leader_attempt, leader_token) = f
        .store
        .start_attempt("scan-example", leader, &registration(), 1000, 3000)
        .unwrap();
    let mut leader_request = request(leader, &leader_attempt);
    let leader_context = f
        .store
        .authorize(
            leader_token.bearer(),
            WORKER_AUDIENCE,
            &leader_request,
            2000,
        )
        .unwrap();
    assert_eq!(leader_context.review_rule.unwrap().revision, 3);
    leader_request.operation = WorkerOperation::StopChild { child_id: worker };
    assert!(
        f.store
            .authorize(
                leader_token.bearer(),
                WORKER_AUDIENCE,
                &leader_request,
                2000
            )
            .is_err()
    );
    leader_request.operation = WorkerOperation::StopChild {
        child_id: deputy.clone(),
    };
    f.store
        .authorize(
            leader_token.bearer(),
            WORKER_AUDIENCE,
            &leader_request,
            2000,
        )
        .unwrap();
    f.store
        .fence_attempt("scan-example", &attempt.id, false)
        .unwrap();
    assert!(f.store.renew(token.bearer(), &req, 2000, 4000).is_err());
    assert!(
        f.store
            .start_attempt("scan-example", &deputy, &registration(), 2000, 4000)
            .is_err()
    );
    f.store
        .fence_attempt("scan-example", &attempt.id, true)
        .unwrap();
    let (replacement, _) = f
        .store
        .start_attempt("scan-example", &deputy, &registration(), 2000, 4000)
        .unwrap();
    assert_eq!(replacement.generation, 2);
    assert!(
        f.store
            .authorize(token.bearer(), WORKER_AUDIENCE, &req, 2000)
            .is_err()
    );
}

#[test]
fn acknowledged_database_mutations_replay_across_reopen_and_fence_old_attempts() {
    let f = Fixture::new();
    let state = f.scan();
    let leader = &state.agents[0].summary.id;
    let (a, t) = f
        .store
        .start_attempt("scan-example", leader, &registration(), 1000, 3000)
        .unwrap();
    let mut req = request(leader, &a);
    f.db.with_connection(|db| {
        db.execute("CREATE TABLE example_records(id TEXT PRIMARY KEY)", [])?;
        Ok(())
    })
    .unwrap();
    let result = f
        .store
        .dispatch_mutation(t.bearer(), WORKER_AUDIENCE, &req, 1000, |tx, _| {
            tx.execute("INSERT INTO example_records VALUES('record-example')", [])?;
            Ok(json!({"record_id":"record-example"}))
        })
        .unwrap();
    assert_eq!(
        f.reopen()
            .dispatch_mutation(t.bearer(), WORKER_AUDIENCE, &req, 1000, |_, _| panic!(
                "replayed handler"
            ))
            .unwrap(),
        result
    );
    let mut changed = req.clone();
    changed.operation = WorkerOperation::Canonical {
        tool: "findings_create".into(),
        arguments: json!({"different":true}),
    };
    assert!(
        f.store
            .dispatch_mutation(t.bearer(), WORKER_AUDIENCE, &changed, 1000, |_, _| panic!(
                "conflicting replay"
            ))
            .is_err()
    );
    f.store.fence_attempt("scan-example", &a.id, true).unwrap();
    assert!(
        f.store
            .dispatch_mutation(t.bearer(), WORKER_AUDIENCE, &req, 1000, |_, _| panic!(
                "fenced handler"
            ))
            .is_err()
    );
    let (new_a, new_t) = f
        .store
        .start_attempt("scan-example", leader, &registration(), 1000, 3000)
        .unwrap();
    req.attempt_id = new_a.id;
    req.generation = new_a.generation;
    assert_eq!(
        f.store
            .dispatch_mutation(new_t.bearer(), WORKER_AUDIENCE, &req, 1000, |_, _| panic!(
                "replacement duplicated mutation"
            ))
            .unwrap(),
        result
    );
    let count =
        f.db.with_connection(|db| {
            Ok(
                db.query_row("SELECT COUNT(*) FROM example_records", [], |r| {
                    r.get::<_, u32>(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!(count, 1);
    req.operation_id = "failed-operation".into();
    assert!(
        f.store
            .dispatch_mutation(new_t.bearer(), WORKER_AUDIENCE, &req, 1000, |tx, _| {
                tx.execute("INSERT INTO example_records VALUES('rolled-back')", [])?;
                anyhow::bail!("synthetic commit failure")
            })
            .is_err()
    );
    let count =
        f.db.with_connection(|db| {
            Ok(
                db.query_row("SELECT COUNT(*) FROM example_records", [], |r| {
                    r.get::<_, u32>(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!(count, 1);
    f.store
        .dispatch_mutation(new_t.bearer(), WORKER_AUDIENCE, &req, 1000, |_, _| {
            Ok(json!({"retried":true}))
        })
        .unwrap();
}

#[test]
fn reopen_reconstructs_checkpoints_handoffs_journal_attempts_and_cleanup_claims() {
    let f = Fixture::new();
    let state = f.scan();
    let leader = &state.agents[0].summary.id;
    let deputy = child(&f.store, leader, "spawn-deputy", "research");
    let worker = child(&f.store, &deputy, "spawn-worker", "investigator");
    let state = f.store.load("scan-example", true).unwrap();
    let assignment = state
        .assignments
        .iter()
        .find(|a| a.child_id == worker)
        .unwrap()
        .id
        .clone();
    f.store
        .checkpoint(
            "scan-example",
            &deputy,
            1,
            AgentCheckpoint {
                state: AgentState::Suspended,
                context: json!({"summary":"Synthetic checkpoint"}),
                inbox_cursor: 12,
                wake_reasons: vec!["child-result".into()],
                pending_assignments: vec![assignment],
                environment_id: None,
            },
        )
        .unwrap();
    f.store
        .add_handoff(
            "scan-example",
            HandoffRequest {
                id: "handoff-example".into(),
                from_agent: worker.clone(),
                parent_id: deputy,
                destination_profile: profile("research"),
                task: task(),
                artifact_refs: vec!["artifact-example".into()],
                record_revisions: vec![RecordRevision {
                    id: "record-example".into(),
                    revision: 2,
                }],
                route_id: "research-request".into(),
                scope: vec!["example scope".into()],
                observations: vec![],
                unresolved_questions: vec![],
                payload: HandoffPayload::Task {},
            },
        )
        .unwrap();
    let (a, _) = f
        .store
        .start_attempt("scan-example", &worker, &registration(), 1000, 2000)
        .unwrap();
    let mut op = ToolOperation {
        id: "tool-example".into(),
        agent_id: worker.clone(),
        attempt_id: a.id.clone(),
        request: json!({"command":"synthetic-command"}),
        state: ToolOperationState::NotStarted,
        result: json!(null),
        revision: 1,
    };
    f.store
        .journal_operation("scan-example", op.clone(), None, &execution(&a, 1000))
        .unwrap();
    op.state = ToolOperationState::Running;
    op.revision = 2;
    f.store
        .journal_operation("scan-example", op.clone(), Some(1), &execution(&a, 1000))
        .unwrap();
    op.state = ToolOperationState::OutcomeUnknown;
    op.revision = 3;
    f.store
        .journal_operation("scan-example", op.clone(), Some(2), &execution(&a, 1000))
        .unwrap();
    op.state = ToolOperationState::Running;
    op.revision = 4;
    assert!(
        f.store
            .journal_operation("scan-example", op, Some(3), &execution(&a, 1000))
            .is_err()
    );
    f.store
        .append_event(
            "scan-example",
            LifecycleEvent {
                id: "event-example".into(),
                agent_id: worker.clone(),
                sequence: 1,
                kind: "uncertain-outcome".into(),
                payload: json!({"operation_id":"tool-example"}),
            },
        )
        .unwrap();
    f.store.fence_attempt("scan-example", &a.id, true).unwrap();
    f.store
        .put_environment("scan-example", environment(&worker))
        .unwrap();
    f.store
        .reserve_resources(
            "scan-example",
            ResourceReservation {
                id: "reservation-example".into(),
                host_id: "host-example".into(),
                agent_id: worker,
                memory_bytes: 1_000_000,
                storage_bytes: 2_000_000,
                expires_at_ms: 2000,
            },
        )
        .unwrap();
    f.store.end_scan("scan-example", 1000).unwrap();
    f.store
        .change_environment(
            "scan-example",
            "environment-example",
            2,
            604801000,
            EnvironmentChange::ClaimCleanup,
        )
        .unwrap();
    let saved = f.store.load("scan-example", true).unwrap();
    assert_eq!(saved, f.reopen().load("scan-example", true).unwrap());
    assert_eq!(saved.environments[0].deletion, DeletionState::Claimed);
    assert_eq!(
        saved.operations[0].state,
        ToolOperationState::OutcomeUnknown
    );
}

#[test]
fn cleanup_resume_and_keep_are_serialized_across_independent_connections() {
    for contender in [EnvironmentChange::Resume, EnvironmentChange::Keep(true)] {
        let f = Fixture::new();
        let state = f.scan();
        f.store
            .put_environment("scan-example", environment(&state.agents[0].summary.id))
            .unwrap();
        f.store.end_scan("scan-example", 1000).unwrap();
        let other = f.reopen();
        let first = f.store.clone();
        let barrier = Arc::new(Barrier::new(2));
        let second_barrier = barrier.clone();
        let cleanup = std::thread::spawn(move || {
            barrier.wait();
            first.change_environment(
                "scan-example",
                "environment-example",
                2,
                604801000,
                EnvironmentChange::ClaimCleanup,
            )
        });
        let protect = std::thread::spawn(move || {
            second_barrier.wait();
            other.change_environment(
                "scan-example",
                "environment-example",
                2,
                604801000,
                contender,
            )
        });
        let a = cleanup.join().unwrap();
        let b = protect.join().unwrap();
        assert_ne!(a.is_ok(), b.is_ok());
        let saved = f.reopen().load("scan-example", true).unwrap();
        assert_eq!(saved.environments[0].generation, 3);
        if a.is_ok() {
            assert!(
                f.store
                    .change_environment(
                        "scan-example",
                        "environment-example",
                        3,
                        604801000,
                        EnvironmentChange::Resume
                    )
                    .is_err()
            );
            assert!(
                f.store
                    .change_environment(
                        "scan-example",
                        "environment-example",
                        3,
                        604801000,
                        EnvironmentChange::Keep(true)
                    )
                    .is_err()
            );
        } else if saved.environments[0].active_references > 0 {
            assert!(saved.ended_at_ms.is_none());
            assert!(saved.environments[0].retention_deadline_ms.is_none());
        }
    }
}

#[test]
fn publication_and_unconfirmed_execution_protect_retained_state() {
    let f = Fixture::new();
    let state = f.scan();
    let leader = &state.agents[0].summary.id;
    f.store
        .put_environment("scan-example", environment(leader))
        .unwrap();
    let e = f
        .store
        .change_environment(
            "scan-example",
            "environment-example",
            1,
            1000,
            EnvironmentChange::Publish("artifact-example".into()),
        )
        .unwrap();
    f.store.end_scan("scan-example", 1000).unwrap();
    assert!(
        f.store
            .change_environment(
                "scan-example",
                &e.id,
                3,
                604801000,
                EnvironmentChange::ClaimCleanup
            )
            .is_err()
    );
    let e = f
        .store
        .change_environment(
            "scan-example",
            &e.id,
            3,
            604801000,
            EnvironmentChange::AcknowledgePublication("artifact-example".into()),
        )
        .unwrap();
    f.store
        .change_environment(
            "scan-example",
            &e.id,
            e.generation,
            604801000,
            EnvironmentChange::Resume,
        )
        .unwrap();
    let (a, _) = f
        .store
        .start_attempt("scan-example", leader, &registration(), 1000, 2000)
        .unwrap();
    f.store.fence_attempt("scan-example", &a.id, false).unwrap();
    let e = f
        .store
        .load("scan-example", true)
        .unwrap()
        .environments
        .remove(0);
    let e = f
        .store
        .change_environment(
            "scan-example",
            &e.id,
            e.generation,
            2000,
            EnvironmentChange::Release,
        )
        .unwrap();
    f.store.end_scan("scan-example", 2000).unwrap();
    assert!(
        f.store
            .change_environment(
                "scan-example",
                &e.id,
                e.generation + 1,
                604802000,
                EnvironmentChange::ClaimCleanup
            )
            .is_err()
    );
}

#[test]
fn host_fleet_state_is_independent_of_desktop_and_model_projection_is_small() {
    let f = Fixture::new();
    let state = f.scan();
    let fleet = FleetCoordinationStore::open(f.db.clone()).unwrap();
    let token = "a".repeat(64);
    fleet
        .save_peer(&FleetPeer {
            id: "peer-example".into(),
            endpoint: "https://example-peer.example-tailnet.ts.net/".into(),
            control_token: token.clone(),
            enabled: true,
            protocol_major: beale_contracts::PROTOCOL_MAJOR,
        })
        .unwrap();
    fleet
        .register_container(&ContainerConfiguration {
            id: "container-example".into(),
            host_id: "peer-example".into(),
            image: "ubuntu:24.04".into(),
            revision: 1,
        })
        .unwrap();
    let reopened = FleetCoordinationStore::open(HostDatabase::open(&f.root).unwrap()).unwrap();
    assert_eq!(reopened.peers().unwrap()[0].control_token, token);
    assert_eq!(reopened.containers().unwrap().len(), 1);
    let (a, credential) = f
        .store
        .start_attempt(
            "scan-example",
            &state.agents[0].summary.id,
            &registration(),
            1000,
            2000,
        )
        .unwrap();
    let projection = serde_json::to_value(f.store.summaries("scan-example").unwrap()).unwrap();
    let keys = projection[0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    assert_eq!(
        keys,
        vec!["id", "parent_id", "profile", "revision", "role", "state"]
    );
    let text = projection.to_string();
    assert!(!text.contains(&token) && !text.contains(credential.bearer()) && !text.contains(&a.id));
    assert_eq!(format!("{credential:?}"), "WorkerCredential([redacted])");
    let root = std::env::temp_dir().join(format!("beale-fleet-only-test-{}", Uuid::new_v4()));
    let db = HostDatabase::open(&root).unwrap();
    FleetCoordinationStore::open(db.clone()).unwrap();
    assert_eq!(db.schema_version("swarms").unwrap(), 0);
    drop(db);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn account_identity_survives_refresh_and_accounting_records_replay_without_merging_accounts() {
    let f = Fixture::new();
    let fleet = FleetCoordinationStore::open(f.db.clone()).unwrap();
    let mut group = fleet
        .account_group("provider-example", "account-example-a", "host-example")
        .unwrap();
    let other = fleet
        .account_group("provider-example", "account-example-b", "host-example")
        .unwrap();
    assert_ne!(group.id, other.id);
    // Refreshing provider credentials has no input into or write path to account identity.
    let reopened = FleetCoordinationStore::open(HostDatabase::open(&f.root).unwrap()).unwrap();
    assert_eq!(
        group.id,
        reopened
            .account_group("provider-example", "account-example-a", "host-other")
            .unwrap()
            .id
    );
    group.participants.push("leader-example".into());
    group.owner_generation = 2;
    fleet.update_group(&group, 1).unwrap();
    assert!(fleet.update_group(&group, 1).is_err());
    let usage = UsageEvent {
        id: "usage-example".into(),
        group_id: group.id.clone(),
        participant_id: "leader-example".into(),
        attempt_id: "provider-attempt-example".into(),
        measured_micros: None,
    };
    fleet.record_usage(&usage).unwrap();
    fleet.record_usage(&usage).unwrap();
    let mut changed = usage.clone();
    changed.measured_micros = Some(1);
    assert!(fleet.record_usage(&changed).is_err());
    assert_eq!(reopened.usage(&group.id).unwrap(), vec![usage]);
    let mut reservation = PacingReservation {
        id: "pacing-example".into(),
        group_id: group.id.clone(),
        owner_generation: 1,
        participant_id: "leader-example".into(),
        estimated_micros: 10,
    };
    assert!(fleet.record_pacing_reservation(&reservation).is_err());
    reservation.owner_generation = 2;
    fleet.record_pacing_reservation(&reservation).unwrap();
    assert_eq!(
        reopened.pacing_reservations(&group.id).unwrap(),
        vec![reservation]
    );
}

#[test]
fn incompatible_durable_state_is_rejected_without_reset_or_fallback() {
    let f = Fixture::new();
    f.scan();
    f.db.with_connection(|db| {
        db.execute(
            "UPDATE swarm_scans SET document=json_set(document,'$.config.version',999)",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(f.reopen().load("scan-example", true).is_err());
    let version =
        f.db.with_connection(|db| {
            Ok(db.query_row(
                "SELECT json_extract(document,'$.config.version') FROM swarm_scans",
                [],
                |r| r.get::<_, u32>(0),
            )?)
        })
        .unwrap();
    assert_eq!(version, 999);
    f.db.with_connection(|db| {
        db.execute(
            "UPDATE schema_versions SET version=999 WHERE owner='swarms'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(SwarmStore::open(f.db.clone()).is_err());
    assert_eq!(f.db.schema_version("swarms").unwrap(), 999);
}

#[test]
fn assignment_completion_does_not_destroy_agent_context_and_instances_have_one_owner() {
    let f = Fixture::new();
    let initial = f.scan();
    let leader = &initial.agents[0].summary.id;
    let deputy = child(&f.store, leader, "spawn-deputy", "research");
    let replay = f
        .store
        .queue_spawn(
            "scan-example",
            leader,
            "spawn-deputy",
            profile("research"),
            task(),
            "changed host capacity",
        )
        .unwrap();
    assert!(
        matches!(replay,SpawnResult::Queued{wait_reason,..} if wait_reason=="capacity pending")
    );
    let state = f.store.load("scan-example", true).unwrap();
    let assignment = state.assignments[0].id.clone();
    f.store
        .set_assignment_state(
            "scan-example",
            &assignment,
            state.revision,
            AssignmentState::Assigned,
        )
        .unwrap();
    assert!(
        f.store
            .set_assignment_state(
                "scan-example",
                &assignment,
                state.revision,
                AssignmentState::Completed
            )
            .is_err()
    );
    let mut checkpoint = state.agent(&deputy).unwrap().checkpoint.clone();
    checkpoint.context = json!({"summary":"Retained synthetic context"});
    checkpoint.state = AgentState::Waiting;
    f.store
        .checkpoint("scan-example", &deputy, 1, checkpoint.clone())
        .unwrap();
    let state = f.store.load("scan-example", true).unwrap();
    f.store
        .set_assignment_state(
            "scan-example",
            &assignment,
            state.revision,
            AssignmentState::Completed,
        )
        .unwrap();
    let (attempt, _) = f
        .store
        .start_attempt("scan-example", &deputy, &registration(), 1000, 2000)
        .unwrap();
    let instance = ExecutionInstance {
        id: "instance-example".into(),
        configuration_id: "container-example".into(),
        host_id: "host-example".into(),
        agent_id: deputy.clone(),
        attempt_id: attempt.id.clone(),
        state: InstanceState::Running,
    };
    f.store
        .register_instance("scan-example", instance.clone())
        .unwrap();
    let mut duplicate = instance;
    duplicate.id = "instance-duplicate".into();
    assert!(
        f.store
            .register_instance("scan-example", duplicate)
            .is_err()
    );
    assert!(
        f.store
            .start_attempt("scan-example", &deputy, &registration(), 2000, 3000)
            .is_err()
    );
    f.store
        .fence_attempt("scan-example", &attempt.id, true)
        .unwrap();
    let state = f.reopen().load("scan-example", true).unwrap();
    assert_eq!(state.agent(&deputy).unwrap().checkpoint, checkpoint);
    assert_eq!(state.assignments[0].state, AssignmentState::Completed);
    assert_eq!(state.instances[0].state, InstanceState::Stopped);
}

#[path = "cases/local_hierarchy.rs"]
mod local_hierarchy_cases;

#[path = "cases/plugin_calls.rs"]
mod plugin_calls;

#[test]
fn lease_renewal_retains_existing_deadline_after_wall_clock_rollback_without_extending_authority() {
    let f = Fixture::new();
    let state = f.scan();
    let leader = &state.agents[0].summary.id;
    let issued_at = 1_000_000;
    let deadline = issued_at + 900_000;
    let (attempt, token) = f
        .store
        .start_attempt("scan-example", leader, &registration(), issued_at, deadline)
        .unwrap();
    let mut heartbeat = request(leader, &attempt);
    heartbeat.operation = WorkerOperation::Heartbeat {};
    let rolled_back = issued_at - 100_000;
    f.store
        .renew(token.bearer(), &heartbeat, rolled_back, deadline)
        .unwrap();
    let saved = f.store.load("scan-example", true).unwrap();
    assert_eq!(
        saved
            .attempts
            .iter()
            .find(|a| a.id == attempt.id)
            .unwrap()
            .expires_at_ms,
        deadline
    );
    assert!(
        f.store
            .renew(token.bearer(), &heartbeat, rolled_back, deadline + 1)
            .is_err()
    );
    assert!(
        f.store
            .renew(token.bearer(), &heartbeat, rolled_back, deadline - 100_000)
            .is_err()
    );
    assert!(
        f.store
            .renew(token.bearer(), &heartbeat, -1, deadline)
            .is_err()
    );
    assert!(
        f.store
            .renew(token.bearer(), &heartbeat, deadline, deadline)
            .is_err()
    );
    assert!(
        f.store
            .renew(token.bearer(), &heartbeat, deadline + 1, deadline + 1_000)
            .is_err()
    );
    assert_eq!(f.store.load("scan-example", true).unwrap(), saved);
    // Forward progress can extend the deadline within the normal grant bound.
    f.store
        .renew(
            token.bearer(),
            &heartbeat,
            issued_at + 1_000,
            deadline + 1_000,
        )
        .unwrap();
    f.store
        .fence_attempt("scan-example", &attempt.id, false)
        .unwrap();
    let fenced = f.store.load("scan-example", true).unwrap();
    assert!(
        f.store
            .renew(token.bearer(), &heartbeat, rolled_back, deadline + 1_000)
            .is_err()
    );
    assert_eq!(f.store.load("scan-example", true).unwrap(), fenced);
}

#[path = "cases/peer_workers.rs"]
mod peer_workers;
