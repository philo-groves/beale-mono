//! Cross-feature acceptance for atomic canonical record/receipt transactions.
use beale_contracts::swarms::*;
use beale_findings::{FindingStore, ToolContext};
use beale_storage::{HostDatabase, WorkspaceRow, swarm_state::SwarmStore};
use serde_json::json;
use uuid::Uuid;

#[test]
fn canonical_finding_and_receipt_commit_together_and_replay_after_leader_reopen() {
    let root =
        std::env::temp_dir().join(format!("beale-routed-finding-example-{}", Uuid::new_v4()));
    let db = HostDatabase::open(&root).unwrap();
    beale_findings::migrate(&db).unwrap();
    db.replace_workspaces(&[WorkspaceRow {
        id: "workspace-example".into(),
        name: "Example".into(),
        directory: root.join("workspace").to_string_lossy().into(),
    }])
    .unwrap();
    let store = SwarmStore::open(db.clone()).unwrap();
    let state = store
        .create_scan(
            ScanConfig {
                version: VERSION,
                scan_id: "scan-example".into(),
                workspace_id: "workspace-example".into(),
                mode: ScanMode::Swarm {
                    snapshot: Box::new(beale_swarms::BASIC_SWARM.snapshot.clone()),
                },
                authorized_scope: "Synthetic example.test fixtures only".into(),
                enabled_tools: vec!["findings_create".into()],
                provider_routes: vec![],
                destinations: vec![],
                pacing: vec![],
                retention: RetentionPolicy::default(),
            },
            true,
            true,
        )
        .unwrap();
    let leader = state.agents[0].summary.id.clone();
    let profile = state.config.mode;
    let ScanMode::Swarm { snapshot } = profile else {
        unreachable!()
    };
    let deputy = snapshot
        .profiles
        .iter()
        .find(|p| p.role == Role::Deputy)
        .unwrap();
    let SpawnResult::Queued { child_id, .. } = store
        .queue_spawn(
            "scan-example",
            &leader,
            "spawn-example",
            deputy.profile.clone(),
            Task {
                question: "Inspect the synthetic fixture".into(),
                completion_criteria: vec!["Return candidate evidence".into()],
                evidence_refs: vec![],
            },
            "awaiting execution",
        )
        .unwrap()
    else {
        unreachable!()
    };
    let (attempt, credential) = store
        .start_attempt(
            "scan-example",
            &child_id,
            &WorkerRegistration {
                version: VERSION,
                audience: WORKER_AUDIENCE.into(),
                capabilities: vec![
                    WorkerCapability::CanonicalRouting,
                    WorkerCapability::ArtifactTransfer,
                    WorkerCapability::DurableEvents,
                ],
            },
            100,
            10000,
        )
        .unwrap();
    let request = WorkerRequest {
        version: VERSION,
        workspace_id: "workspace-example".into(),
        scan_id: "scan-example".into(),
        agent_id: child_id,
        attempt_id: attempt.id,
        generation: attempt.generation,
        operation_id: "create-example".into(),
        operation: WorkerOperation::Canonical {
            tool: "findings_create".into(),
            arguments: json!({"title":"Synthetic candidate","summary":"Unverified example observation","hypothesis":"A synthetic input may violate an example invariant"}),
        },
    };
    // Fail the receipt after the feature has executed its create. The record,
    // revision and counter must all roll back, leaving the same operation retryable.
    db.with_connection(|c| { c.execute_batch("CREATE TRIGGER fail_worker_receipt BEFORE INSERT ON swarm_receipts BEGIN SELECT RAISE(ABORT, 'synthetic receipt disk full'); END;")?; Ok(()) }).unwrap();
    let dispatch = |store: &SwarmStore| {
        store.dispatch_mutation(
            credential.bearer(),
            WORKER_AUDIENCE,
            &request,
            101,
            |tx, caller| {
                assert_eq!(caller.agent.role, Role::Deputy);
                let WorkerOperation::Canonical { tool, arguments } = &request.operation else {
                    unreachable!()
                };
                let output = beale_findings::execute(
                    &beale_findings::transaction_store(tx),
                    &ToolContext {
                        workspace_id: &request.workspace_id,
                        session_id: &request.agent_id,
                        evidence_role: None,
                        pending: &[],
                    },
                    tool,
                    arguments,
                )?;
                Ok(serde_json::from_str(&output)?)
            },
        )
    };
    assert!(dispatch(&store).is_err());
    assert!(db.finding_get("workspace-example", "F1").unwrap().is_none());
    db.with_connection(|c| {
        c.execute_batch("DROP TRIGGER fail_worker_receipt;")?;
        Ok(())
    })
    .unwrap();
    let accepted = dispatch(&store).unwrap();
    assert!(db.finding_get("workspace-example", "F1").unwrap().is_some());
    let reopened = SwarmStore::open(HostDatabase::open(&root).unwrap()).unwrap();
    let replayed = reopened
        .dispatch_mutation(
            credential.bearer(),
            WORKER_AUDIENCE,
            &request,
            102,
            |_, _| panic!("acknowledged finding must not execute again"),
        )
        .unwrap();
    assert_eq!(accepted, replayed);
    assert!(db.finding_get("workspace-example", "F2").unwrap().is_none());
    let mut changed = request.clone();
    changed.operation = WorkerOperation::Canonical {
        tool: "findings_create".into(),
        arguments: json!({"title":"Changed"}),
    };
    assert!(
        reopened
            .dispatch_mutation(
                credential.bearer(),
                WORKER_AUDIENCE,
                &changed,
                103,
                |_, _| panic!("changed retry must not execute")
            )
            .is_err()
    );
    reopened
        .fence_attempt("scan-example", &request.attempt_id, false)
        .unwrap();
    assert!(
        reopened
            .dispatch_mutation(
                credential.bearer(),
                WORKER_AUDIENCE,
                &request,
                104,
                |_, _| panic!("revoked request must not execute")
            )
            .is_err()
    );
    drop(reopened);
    drop(store);
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}
