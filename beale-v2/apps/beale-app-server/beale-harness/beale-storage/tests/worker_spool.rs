use beale_contracts::{
    swarm_worker::{ExecutionIdentity, ToolInvocation, WorkRequest},
    swarms::{MAX_DOCUMENT_BYTES, ToolOperationState, VERSION, WorkerOperation, WorkerRequest},
};
use beale_storage::worker_spool::*;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};
use uuid::Uuid;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("beale-worker-spool-example-{}", Uuid::new_v4())))
    }
    fn open(&self) -> WorkerSpool {
        WorkerSpool::open(&self.0, identity()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn identity() -> ExecutionIdentity {
    ExecutionIdentity {
        workspace_id: "workspace-example".into(),
        scan_id: "scan-example".into(),
        agent_id: "agent-example".into(),
        attempt_id: "attempt-example".into(),
        generation: 1,
    }
}
fn local(id: &str) -> WorkRequest {
    WorkRequest::Environment {
        call: ToolInvocation {
            id: id.into(),
            name: "write".into(),
            arguments: json!({"path":"candidate.txt","content":"synthetic candidate"}),
        },
    }
}
fn remote(id: &str) -> WorkRequest {
    let identity = identity();
    WorkRequest::Leader {
        request: WorkerRequest {
            version: VERSION,
            workspace_id: identity.workspace_id,
            scan_id: identity.scan_id,
            agent_id: identity.agent_id,
            attempt_id: identity.attempt_id,
            generation: identity.generation,
            operation_id: id.into(),
            operation: WorkerOperation::Canonical {
                tool: "findings_create".into(),
                arguments: json!({"title":"Synthetic candidate"}),
            },
        },
    }
}
struct Fail(SpoolWrite);
impl SpoolFaults for Fail {
    fn before_commit(&self, write: SpoolWrite) -> anyhow::Result<()> {
        anyhow::ensure!(write != self.0, "synthetic worker disk full");
        Ok(())
    }
}

#[test]
fn spool_ownership_and_identity_prevent_duplicate_workers_and_cross_attempt_reuse() {
    let f = Fixture::new();
    let spool = f.open();
    assert!(WorkerSpool::open(&f.0, identity()).is_err());
    assert_eq!(
        spool.claim(&local("call-example")).unwrap(),
        Claim::Dispatch
    );
    drop(spool);
    for field in 0..5 {
        let mut changed = identity();
        match field {
            0 => changed.workspace_id = "other-workspace".into(),
            1 => changed.scan_id = "other-scan".into(),
            2 => changed.agent_id = "other-agent".into(),
            3 => changed.attempt_id = "other-attempt".into(),
            _ => changed.generation += 1,
        }
        assert!(WorkerSpool::open(&f.0, changed).is_err());
    }
    let spool = f.open();
    assert_eq!(spool.identity(), &identity());
    assert_eq!(spool.pending(0, 10).unwrap().len(), 1);
    assert!(spool.require_known_outcomes().is_err());
}

#[test]
fn restart_never_replays_uncertain_local_effects_but_preserves_remote_request_identity() {
    let f = Fixture::new();
    let spool = f.open();
    assert_eq!(
        spool.claim(&local("local-example")).unwrap(),
        Claim::Dispatch
    );
    assert_eq!(
        spool.claim(&local("local-example")).unwrap(),
        Claim::InFlight
    );
    assert_eq!(
        spool.claim(&remote("remote-example")).unwrap(),
        Claim::Dispatch
    );
    assert!(spool.acknowledge("local-example").is_err());
    drop(spool);
    let spool = f.open();
    assert_eq!(
        spool.claim(&local("local-example")).unwrap(),
        Claim::OutcomeUnknown
    );
    assert_eq!(
        spool.claim(&remote("remote-example")).unwrap(),
        Claim::RetryLeader
    );
    assert!(
        spool
            .complete("local-example", &json!({"invented":"success"}))
            .is_err()
    );
    let pending = spool.pending(0, 10).unwrap();
    assert_eq!(pending[0].state, ToolOperationState::OutcomeUnknown);
    assert_eq!(pending[1].request, remote("remote-example"));
    spool.acknowledge("local-example").unwrap();
    assert!(spool.require_known_outcomes().is_err());
    assert_eq!(
        spool.claim(&local("local-example")).unwrap(),
        Claim::OutcomeUnknown
    );
}

#[test]
fn completed_null_results_replay_after_restart_without_dispatch_and_reject_changed_retries() {
    let f = Fixture::new();
    let spool = f.open();
    let request = local("call-example");
    spool.claim(&request).unwrap();
    spool.complete(request.id(), &Value::Null).unwrap();
    spool.complete(request.id(), &Value::Null).unwrap();
    assert!(spool.complete(request.id(), &json!(true)).is_err());
    drop(spool);
    let spool = f.open();
    assert_eq!(spool.claim(&request).unwrap(), Claim::Replay(Value::Null));
    assert!(spool.claim(&remote(request.id())).is_err());
    spool.acknowledge(request.id()).unwrap();
    spool.acknowledge(request.id()).unwrap();
    assert!(spool.pending(0, 10).unwrap().is_empty());
    spool.require_known_outcomes().unwrap();
    assert_eq!(spool.claim(&request).unwrap(), Claim::Replay(Value::Null));
}

#[test]
fn failed_journal_commits_never_permit_dispatch_or_acknowledge_missing_results() {
    let f = Fixture::new();
    let request = local("call-example");
    let spool = f.open().with_faults(Arc::new(Fail(SpoolWrite::Claim)));
    assert!(spool.claim(&request).is_err());
    assert!(spool.pending(0, 10).unwrap().is_empty());
    drop(spool);
    let spool = f.open().with_faults(Arc::new(Fail(SpoolWrite::Outcome)));
    assert_eq!(spool.claim(&request).unwrap(), Claim::Dispatch);
    assert!(
        spool
            .complete(request.id(), &json!({"output":"synthetic output"}))
            .is_err()
    );
    assert!(spool.acknowledge(request.id()).is_err());
    assert_eq!(
        spool.pending(0, 10).unwrap()[0].state,
        ToolOperationState::Running
    );
    drop(spool);
    let spool = f.open();
    assert_eq!(spool.claim(&request).unwrap(), Claim::OutcomeUnknown);
}

#[test]
fn failed_acknowledgements_retain_outputs_until_confirmed_by_leader() {
    let f = Fixture::new();
    let spool = f
        .open()
        .with_faults(Arc::new(Fail(SpoolWrite::Acknowledge)));
    spool.claim(&local("call-example")).unwrap();
    let output = json!({"output":"retained synthetic output"});
    spool.complete("call-example", &output).unwrap();
    assert!(spool.acknowledge("call-example").is_err());
    drop(spool);
    let spool = f.open();
    assert_eq!(spool.pending(0, 10).unwrap()[0].result, output);
    spool.acknowledge("call-example").unwrap();
    assert!(spool.pending(0, 10).unwrap().is_empty());
}

#[test]
fn checkpoint_exhaustion_preserves_last_durable_state_and_same_retry_identity() {
    let f = Fixture::new();
    let spool = f.open();
    let first = spool
        .checkpoint("checkpoint-one", 0, &json!({"cursor":1}))
        .unwrap();
    assert!(
        spool
            .checkpoint("checkpoint-stale", 0, &json!({"cursor":2}))
            .is_err()
    );
    assert!(
        spool
            .checkpoint("checkpoint-one", 1, &json!({"cursor":2}))
            .is_err()
    );
    drop(spool);
    let spool = f.open().with_faults(Arc::new(Fail(SpoolWrite::Checkpoint)));
    assert!(
        spool
            .checkpoint("checkpoint-two", 1, &json!({"cursor":2}))
            .is_err()
    );
    assert_eq!(spool.latest_checkpoint().unwrap(), Some(first));
    drop(spool);
    let spool = f.open();
    let second = spool
        .checkpoint("checkpoint-two", 1, &json!({"cursor":2}))
        .unwrap();
    assert_eq!(second.revision, 2);
    assert_eq!(
        spool
            .checkpoint("checkpoint-two", 1, &json!({"cursor":2}))
            .unwrap(),
        second
    );
    drop(spool);
    assert_eq!(f.open().latest_checkpoint().unwrap(), Some(second));
}

#[test]
fn recovery_pages_are_ordered_bounded_and_reject_invalid_or_foreign_documents() {
    let f = Fixture::new();
    let spool = f.open();
    for id in ["call-one", "call-two", "call-three"] {
        spool.claim(&local(id)).unwrap();
        spool.complete(id, &json!({"id":id})).unwrap();
    }
    spool.acknowledge("call-two").unwrap();
    let page = spool.pending(0, 1).unwrap();
    assert_eq!(page[0].request.id(), "call-one");
    assert_eq!(
        spool.pending(page[0].sequence, 1).unwrap()[0].request.id(),
        "call-three"
    );
    assert!(spool.pending(0, 0).is_err());
    assert!(spool.pending(0, 257).is_err());
    let WorkRequest::Leader { mut request } = remote("remote-example") else {
        unreachable!()
    };
    request.agent_id = "other-agent".into();
    assert!(spool.claim(&WorkRequest::Leader { request }).is_err());
    assert!(spool.claim(&local("")).is_err());
    assert!(
        spool
            .checkpoint("large-example", 0, &json!("x".repeat(MAX_DOCUMENT_BYTES)))
            .is_err()
    );
    assert!(serde_json::from_value::<WorkRequest>(json!({"location":"environment","call":{"id":"example","name":"write","arguments":{},"role":"leader"}})).is_err());
    assert_eq!(spool.pending(0, 10).unwrap().len(), 2);
}
