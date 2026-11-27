use super::*;
use beale_contracts::swarm_plugins::{Invocation, Outcome, Reply};

fn setup() -> (Fixture, WorkerRequest, WorkerCredential) {
    let f = Fixture::new();
    let mut cfg = config();
    cfg.enabled_tools.push("plugin_call".into());
    let state = f.store.create_scan(cfg, true, true).unwrap();
    let agent = &state.agents[0].summary.id;
    let (attempt, credential) = f
        .store
        .start_attempt("scan-example", agent, &registration(), 1000, 100_000)
        .unwrap();
    let mut r = request(agent, &attempt);
    r.operation = WorkerOperation::Plugin {
        invocation: Invocation {
            plugin: "example-plugin".into(),
            server: "example-server".into(),
            tool: "example-tool".into(),
            arguments: json!({"key":"synthetic"}),
        },
    };
    (f, r, credential)
}
fn result() -> Outcome {
    Outcome::Completed {
        output: "Synthetic plugin result".into(),
        is_error: false,
        artifact_refs: vec![],
    }
}
fn own(f: &Fixture, r: &WorkerRequest, c: &WorkerCredential) -> PluginOwner {
    f.store
        .try_own_plugin(c.bearer(), r, 1001)
        .unwrap()
        .unwrap()
}
fn claim(owner: &PluginOwner, c: &WorkerCredential) -> String {
    match owner.claim(c.bearer(), 1001).unwrap() {
        PluginClaim::Dispatch(claim) => claim,
        _ => panic!("expected fresh claim"),
    }
}

#[test]
fn plugin_claims_are_exclusive_and_results_replay_after_reopen_and_receipt_failure() {
    let (f, r, c) = setup();
    let owner = own(&f, &r, &c);
    let token = claim(&owner, &c);
    assert!(
        f.reopen()
            .try_own_plugin(c.bearer(), &r, 1001)
            .unwrap()
            .is_none()
    );
    owner.stage_result(&token, result()).unwrap();
    owner.stage_result(&token, result()).unwrap();
    assert!(owner.stage_result("wrong-claim", result()).is_err());
    assert!(
        owner
            .stage_result(
                &token,
                Outcome::Completed {
                    output: "Changed result".into(),
                    is_error: false,
                    artifact_refs: vec![]
                }
            )
            .is_err()
    );
    f.db.with_connection(|db|{db.execute_batch("CREATE TRIGGER plugin_receipt_full BEFORE INSERT ON swarm_receipts BEGIN SELECT RAISE(ABORT,'synthetic plugin receipt failure'); END;")?;Ok(())}).unwrap();
    assert!(owner.recover(c.bearer(), 1002).is_err());
    assert_eq!(
        f.store.load("scan-example", true).unwrap().operations[0].state,
        ToolOperationState::Running
    );
    drop(owner);
    f.db.with_connection(|db| {
        db.execute_batch("DROP TRIGGER plugin_receipt_full;")?;
        Ok(())
    })
    .unwrap();
    let reopened = f.reopen();
    let owner = reopened
        .try_own_plugin(c.bearer(), &r, 1003)
        .unwrap()
        .unwrap();
    assert!(matches!(
        owner.claim(c.bearer(), 1003).unwrap(),
        PluginClaim::Pending
    ));
    let ack = owner.recover(c.bearer(), 1003).unwrap();
    let reply: Reply = serde_json::from_value(ack.result.clone()).unwrap();
    assert_eq!(reply.outcome, result());
    assert_eq!(owner.recover(c.bearer(), 1004).unwrap(), ack);
    assert!(matches!(
        owner.claim(c.bearer(), 1004).unwrap(),
        PluginClaim::Replay(_)
    ));
    let state = reopened.load("scan-example", true).unwrap();
    assert_eq!(state.operations.len(), 1);
    assert_eq!(state.operations[0].state, ToolOperationState::Completed);
    // Completed JSON "null" is still a known outcome and permits new work.
    drop(owner);
    let mut next = r.clone();
    next.operation_id = "next-call".into();
    let owner = own(&f, &next, &c);
    let token = claim(&owner, &c);
    owner
        .stage_result(
            &token,
            Outcome::Completed {
                output: "null".into(),
                is_error: false,
                artifact_refs: vec![],
            },
        )
        .unwrap();
    owner.recover(c.bearer(), 1005).unwrap();
}

#[test]
fn abandoned_plugin_effect_is_unknown_and_new_ids_cannot_evade_reconciliation() {
    let (f, r, c) = setup();
    let owner = own(&f, &r, &c);
    claim(&owner, &c);
    drop(owner);
    let reopened = f.reopen();
    let owner = reopened
        .try_own_plugin(c.bearer(), &r, 1002)
        .unwrap()
        .unwrap();
    assert!(matches!(
        owner.claim(c.bearer(), 1002).unwrap(),
        PluginClaim::Pending
    ));
    f.db.with_connection(|db|{db.execute_batch("CREATE TRIGGER plugin_unknown_full BEFORE INSERT ON swarm_receipts BEGIN SELECT RAISE(ABORT,'synthetic uncertainty failure'); END;")?;Ok(())}).unwrap();
    assert!(owner.recover(c.bearer(), 1003).is_err());
    let state = f.store.load("scan-example", true).unwrap();
    assert_eq!(state.operations[0].state, ToolOperationState::Running);
    assert!(
        !state
            .events
            .iter()
            .any(|e| e.kind == "plugin-outcome-unknown")
    );
    f.db.with_connection(|db| {
        db.execute_batch("DROP TRIGGER plugin_unknown_full;")?;
        Ok(())
    })
    .unwrap();
    let ack = owner.recover(c.bearer(), 1004).unwrap();
    let reply: Reply = serde_json::from_value(ack.result).unwrap();
    assert!(matches!(reply.outcome, Outcome::OutcomeUnknown {}));
    owner.recover(c.bearer(), 1005).unwrap();
    drop(owner);
    let state = f.store.load("scan-example", true).unwrap();
    assert_eq!(
        state.operations[0].state,
        ToolOperationState::OutcomeUnknown
    );
    assert_eq!(
        state
            .events
            .iter()
            .filter(|e| e.kind == "plugin-outcome-unknown")
            .count(),
        1
    );
    let mut next = r.clone();
    next.operation_id = "new-call-id".into();
    let owner = own(&f, &next, &c);
    assert!(owner.claim(c.bearer(), 1005).is_err());
    drop(owner);
    // A different agent can still make progress.
    let deputy = child(&f.store, &r.agent_id, "spawn-example", "research");
    let (attempt, other) = f
        .store
        .start_attempt("scan-example", &deputy, &registration(), 1005, 100_000)
        .unwrap();
    next.agent_id = deputy;
    next.attempt_id = attempt.id;
    next.generation = attempt.generation;
    let owner = own(&f, &next, &other);
    claim(&owner, &other);
}

#[test]
fn plugin_candidate_survives_revocation_without_mutating_canonical_state() {
    let (f, r, c) = setup();
    let owner = own(&f, &r, &c);
    let token = claim(&owner, &c);
    f.store
        .fence_attempt("scan-example", &r.attempt_id, true)
        .unwrap();
    let stopped = f.store.load("scan-example", true).unwrap();
    owner.stage_result(&token, result()).unwrap();
    assert!(owner.recover(c.bearer(), 1002).is_err());
    assert!(owner.claim(c.bearer(), 1002).is_err());
    assert_eq!(f.store.load("scan-example", true).unwrap(), stopped);
    f.db.with_connection(|db| {
        let raw: String = db.query_row("SELECT result FROM swarm_plugin_calls", [], |row| {
            row.get(0)
        })?;
        assert_eq!(serde_json::from_str::<Reply>(&raw)?.outcome, result());
        Ok(())
    })
    .unwrap();
    drop(owner);
    let (replacement, new_credential) = f
        .store
        .start_attempt("scan-example", &r.agent_id, &registration(), 1003, 100_000)
        .unwrap();
    let mut next = r.clone();
    next.attempt_id = replacement.id;
    next.generation = replacement.generation;
    assert!(
        f.store
            .try_own_plugin(new_credential.bearer(), &next, 1004)
            .is_err()
    );
    next.operation_id = "new-operation".into();
    let owner = f
        .store
        .try_own_plugin(new_credential.bearer(), &next, 1004)
        .unwrap()
        .unwrap();
    assert!(owner.claim(new_credential.bearer(), 1004).is_err());
    drop(owner);
    let journal = &stopped.operations[0];
    f.store
        .recover_operation(
            "scan-example",
            "reconcile-example",
            &journal.id,
            journal.revision,
            JournalRecovery::Completed {
                result: json!({"output":"Synthetic plugin result"}),
                evidence_refs: vec!["artifact:example-result".into()],
            },
        )
        .unwrap();
    let owner = f
        .store
        .try_own_plugin(new_credential.bearer(), &next, 1005)
        .unwrap()
        .unwrap();
    assert!(matches!(
        owner.claim(new_credential.bearer(), 1005).unwrap(),
        PluginClaim::Dispatch(_)
    ));
}

#[test]
#[ignore = "subprocess fixture invoked by plugin_process_death_preserves_uncertainty"]
fn plugin_lock_subprocess() {
    let Ok(root) = std::env::var("BEALE_PLUGIN_TEST_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let data: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("plugin-child.json")).unwrap()).unwrap();
    let request: WorkerRequest = serde_json::from_value(data["request"].clone()).unwrap();
    let store = SwarmStore::open(HostDatabase::open(&root).unwrap()).unwrap();
    let owner = store
        .try_own_plugin(data["bearer"].as_str().unwrap(), &request, 1001)
        .unwrap()
        .unwrap();
    assert!(matches!(
        owner.claim(data["bearer"].as_str().unwrap(), 1001).unwrap(),
        PluginClaim::Dispatch(_)
    ));
    std::fs::write(root.join("plugin-child-ready"), b"ready").unwrap();
    loop {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[test]
fn plugin_process_death_preserves_uncertainty() {
    struct OwnedChild(std::process::Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let (f, r, c) = setup();
    std::fs::write(
        f.root.join("plugin-child.json"),
        serde_json::to_vec(&json!({"request":r,"bearer":c.bearer()})).unwrap(),
    )
    .unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--ignored",
            "--exact",
            "plugin_calls::plugin_lock_subprocess",
        ])
        .env("BEALE_PLUGIN_TEST_ROOT", &f.root.0)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = OwnedChild(command.spawn().unwrap());
    let start = std::time::Instant::now();
    while !f.root.join("plugin-child-ready").exists()
        && start.elapsed() < std::time::Duration::from_secs(10)
    {
        if child.0.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let ready = f.root.join("plugin-child-ready").exists();
    if ready {
        assert!(
            f.store
                .try_own_plugin(c.bearer(), &r, 1002)
                .unwrap()
                .is_none()
        );
    }
    let _ = child.0.kill();
    child.0.wait().unwrap();
    assert!(ready, "plugin subprocess did not acquire its durable claim");
    let owner = own(&f, &r, &c);
    assert!(matches!(
        owner.claim(c.bearer(), 1003).unwrap(),
        PluginClaim::Pending
    ));
    let reply: Reply =
        serde_json::from_value(owner.recover(c.bearer(), 1003).unwrap().result).unwrap();
    assert!(matches!(reply.outcome, Outcome::OutcomeUnknown {}));
}

#[test]
fn plugin_claim_transaction_and_operation_namespace_are_fenced() {
    let (f, r, c) = setup();
    let owner = own(&f, &r, &c);
    f.db.with_connection(|db|{db.execute_batch("CREATE TRIGGER plugin_claim_full BEFORE INSERT ON swarm_plugin_calls BEGIN SELECT RAISE(ABORT,'synthetic claim failure'); END;")?;Ok(())}).unwrap();
    assert!(owner.claim(c.bearer(), 1001).is_err());
    assert!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .operations
            .is_empty()
    );
    f.db.with_connection(|db| {
        db.execute_batch("DROP TRIGGER plugin_claim_full;")?;
        Ok(())
    })
    .unwrap();
    claim(&owner, &c);
    let mut changed = r.clone();
    changed.operation = WorkerOperation::Canonical {
        tool: "findings_create".into(),
        arguments: json!({"title":"Synthetic record"}),
    };
    assert!(
        f.store
            .dispatch_mutation(c.bearer(), WORKER_AUDIENCE, &changed, 1002, |_, _| Ok(
                json!({})
            ))
            .is_err()
    );
    assert!(
        f.store
            .replay_worker_mutation(c.bearer(), &changed, 1002)
            .is_err()
    );
    let mut changed = r.clone();
    if let WorkerOperation::Plugin { invocation } = &mut changed.operation {
        invocation.tool = "other-tool".into();
    }
    assert!(f.store.try_own_plugin(c.bearer(), &changed, 1002).is_err());
    assert!(f.store.try_own_plugin(c.bearer(), &r, 100_001).is_err());
}

#[test]
fn plugin_result_storage_failure_retains_the_original_claim() {
    let (f, r, c) = setup();
    let owner = own(&f, &r, &c);
    let token = claim(&owner, &c);
    f.db.with_connection(|db|{db.execute_batch("CREATE TRIGGER plugin_result_full BEFORE UPDATE ON swarm_plugin_calls BEGIN SELECT RAISE(ABORT,'synthetic result failure'); END;")?;Ok(())}).unwrap();
    assert!(owner.stage_result(&token, result()).is_err());
    assert!(matches!(
        owner.claim(c.bearer(), 1002).unwrap(),
        PluginClaim::Pending
    ));
    f.db.with_connection(|db| {
        db.execute_batch("DROP TRIGGER plugin_result_full;")?;
        Ok(())
    })
    .unwrap();
    owner.stage_result(&token, result()).unwrap();
    let ack = owner.recover(c.bearer(), 1003).unwrap();
    assert_eq!(
        serde_json::from_value::<Reply>(ack.result).unwrap().outcome,
        result()
    );
    assert_eq!(
        f.store.load("scan-example", true).unwrap().operations.len(),
        1
    );
}
