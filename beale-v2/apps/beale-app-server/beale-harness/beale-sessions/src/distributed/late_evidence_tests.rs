use super::*;
use beale_storage::{
    swarm_files::{hash, stage_worker_artifact},
    worker_spool::WorkerSpool,
};

#[tokio::test]
async fn late_capture_preserves_original_assignment_across_replacement_and_receipt_failure() {
    let f = Fixture::new().await;
    let identity = f.store.worker_bootstrap(&f.attempt.id).unwrap().identity;
    let original = f.store.worker_provenance(&f.attempt.id).unwrap();
    let workspace = f.root.join("workspace");
    let recovery = f.root.join("late-capture");
    std::fs::create_dir_all(&workspace).unwrap();
    let bytes = vec![b'e'; 400_000];
    std::fs::write(workspace.join("candidate.txt"), &bytes).unwrap();
    let spool = WorkerSpool::open(&recovery, identity.clone()).unwrap();
    spool
        .claim(&beale_contracts::swarm_worker::WorkRequest::Environment {
            call: beale_contracts::swarm_worker::ToolInvocation {
                id: "unresolved-command".into(),
                name: "write".into(),
                arguments: json!({"path":"pending.txt","content":"Synthetic unresolved effect"}),
            },
        })
        .unwrap();
    drop(spool);
    let (upload, captured) = stage_worker_artifact(
        &workspace,
        &recovery,
        "publication-example",
        "candidate.txt",
    )
    .unwrap();
    let key = hash(b"publication-example");
    let begin = f.request(
        &format!("artifact-begin:{key}"),
        WorkerOperation::ArtifactBegin {
            artifact: upload.clone(),
        },
    );
    f.client()
        .send(&begin, Duration::from_secs(5))
        .await
        .unwrap();
    f.client()
        .send(
            &f.request(
                "first-chunk",
                WorkerOperation::ArtifactChunk {
                    transfer_id: upload.id.clone(),
                    offset: 0,
                    bytes: bytes[..1000].to_vec(),
                    sha256: hash(&bytes[..1000]),
                },
            ),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert!(super::super::recovery::publications(&f.state, &identity, &recovery).is_err());
    f.store
        .fence_attempt("scan-example", &f.attempt.id, false)
        .unwrap();
    assert!(super::super::recovery::publications(&f.state, &identity, &recovery).is_err());
    super::super::supervisor::reconcile_owned_attempt(
        &f.store,
        &identity,
        true,
        WorkerDisposition::Failed,
        unix_ms(),
    )
    .unwrap();

    let parent = f
        .store
        .local_claim("scan-example", &f.leader, unix_ms(), 64)
        .unwrap()
        .unwrap();
    f.store
        .local_dispatch(
            &parent,
            "resume-after-reconciliation",
            SwarmCommand::MessageChild(MessageChild {
                child_id: f.deputy.clone(),
                message: "Continue reconciled synthetic research".into(),
            }),
            Admission::Ready,
        )
        .unwrap();
    f.store
        .local_finish(&parent, json!({}), "Awaiting replacement", false)
        .unwrap();
    let replacement = f
        .store
        .local_claim("scan-example", &f.deputy, unix_ms(), 64)
        .unwrap()
        .unwrap();
    prepare_worker(
        &f.state.db,
        &replacement,
        beale_contracts::swarm_worker::WorkerShell::native(),
        &registration(),
        unix_ms(),
    )
    .unwrap();
    let current = f.store.worker_provenance(&replacement.attempt_id).unwrap();
    assert_ne!(original.assignment, current.assignment);
    assert_eq!(f.store.worker_provenance(&f.attempt.id).unwrap(), original);
    std::fs::write(
        workspace.join("candidate.txt"),
        "Synthetic newer workspace edit",
    )
    .unwrap();
    let before = f.store.load("scan-example", true).unwrap();
    let files = crate::research_files::store(&f.state).unwrap();
    f.state.db.with_connection(|db| {db.execute_batch("CREATE TRIGGER fail_recovered_seal BEFORE INSERT ON swarm_receipts WHEN NEW.actor LIKE 'artifact-recovery:%' AND json_extract(NEW.content,'$.operation.kind')='artifact_seal' BEGIN SELECT RAISE(ABORT,'synthetic recovered receipt full'); END;")?;Ok(())}).unwrap();
    assert!(super::super::recovery::publications(&f.state, &identity, &recovery).is_err());
    assert!(files.artifact(&f.workspace_id, &upload.id).is_err());
    assert!(captured.exists());
    assert_eq!(f.store.load("scan-example", true).unwrap(), before);
    f.state
        .db
        .with_connection(|db| {
            db.execute_batch("DROP TRIGGER fail_recovered_seal;")?;
            Ok(())
        })
        .unwrap();
    super::super::recovery::publications(&f.state, &identity, &recovery).unwrap();
    let accepted = files.artifact(&f.workspace_id, &upload.id).unwrap();
    assert_eq!(accepted.assignment, original.assignment);
    assert_eq!(accepted.attempt, identity.attempt_id);
    assert_eq!(accepted.generation, identity.generation);
    assert_eq!(
        files.read_blob(&accepted.sha256, accepted.size).unwrap(),
        bytes
    );
    let after = f.store.load("scan-example", true).unwrap();
    assert_eq!(after.assignments, before.assignments);
    assert_eq!(after.attempts, before.attempts);
    assert_eq!(
        after.agent(&f.deputy).unwrap(),
        before.agent(&f.deputy).unwrap()
    );
    let events = after
        .events
        .iter()
        .filter(|e| e.kind == "worker-recovery-artifact")
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].payload["candidate"], true);
    assert_eq!(
        events[0].payload["assignment"],
        original.assignment.unwrap()
    );
    assert_eq!(
        f.store
            .local_state("scan-example")
            .unwrap()
            .inbox
            .iter()
            .filter(|m| m.message.contains(&accepted.id))
            .count(),
        1
    );
    super::super::recovery::publications(&f.state, &identity, &recovery).unwrap();
    assert_eq!(f.store.load("scan-example", true).unwrap(), after);
    assert!(
        f.client()
            .send(
                &f.request(
                    "stale-seal",
                    WorkerOperation::ArtifactSeal {
                        transfer_id: upload.id.clone()
                    }
                ),
                Duration::from_secs(5)
            )
            .await
            .is_err()
    );
    assert!(
        f.store
            .recover_worker_artifact_request(
                &files,
                &f.request(
                    "invalid-recovery",
                    WorkerOperation::Finish {
                        revision: 1,
                        answer: "Synthetic stale completion".into()
                    }
                )
            )
            .is_err()
    );
    assert_eq!(f.store.load("scan-example", true).unwrap(), after);
    assert!(captured.exists());
    let spool = WorkerSpool::open_existing(&recovery, identity.clone()).unwrap();
    assert!(spool.require_known_outcomes().is_err());
    assert!(
        spool
            .pending(0, 256)
            .unwrap()
            .iter()
            .any(|operation| operation.request.id() == "unresolved-command"
                && operation.state == ToolOperationState::OutcomeUnknown)
    );
    drop(spool);
    drop(files);
    f.close().await;
}

#[tokio::test]
async fn late_capture_requires_original_spool_identity_and_intact_provenance() {
    let f = Fixture::new().await;
    let identity = f.store.worker_bootstrap(&f.attempt.id).unwrap().identity;
    let workspace = f.root.join("workspace");
    let recovery = f.root.join("late-capture");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("candidate.txt"), "Synthetic candidate").unwrap();
    let spool = WorkerSpool::open(&recovery, identity.clone()).unwrap();
    let (upload, captured) = stage_worker_artifact(
        &workspace,
        &recovery,
        "publication-example",
        "candidate.txt",
    )
    .unwrap();
    super::super::supervisor::reconcile_owned_attempt(
        &f.store,
        &identity,
        true,
        WorkerDisposition::Failed,
        unix_ms(),
    )
    .unwrap();
    assert!(super::super::recovery::publications(&f.state, &identity, &recovery).is_err());
    drop(spool);
    let binding = f.store.worker_provenance(&f.attempt.id).unwrap().assignment;
    f.state
        .db
        .with_connection(|db| {
            db.execute(
                "UPDATE swarm_worker_runtime SET assignment_id=NULL WHERE attempt_id=?1",
                [&f.attempt.id],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(f.store.worker_provenance(&f.attempt.id).is_err());
    assert!(super::super::recovery::publications(&f.state, &identity, &recovery).is_err());
    f.state
        .db
        .with_connection(|db| {
            db.execute(
                "UPDATE swarm_worker_runtime SET assignment_id=?2 WHERE attempt_id=?1",
                beale_storage::rusqlite::params![f.attempt.id, binding],
            )?;
            Ok(())
        })
        .unwrap();
    let other = f.root.join("unbound-capture");
    stage_worker_artifact(&workspace, &other, "publication-example", "candidate.txt").unwrap();
    assert!(super::super::recovery::publications(&f.state, &identity, &other).is_err());
    assert!(!other.join("recovery.sqlite").exists());
    let mut foreign = identity.clone();
    foreign.attempt_id = "attempt-other".into();
    drop(WorkerSpool::open(&other, foreign).unwrap());
    assert!(super::super::recovery::publications(&f.state, &identity, &other).is_err());
    std::fs::write(&captured, "Synthetic changed capture").unwrap();
    let before = f.store.load("scan-example", true).unwrap();
    assert!(super::super::recovery::publications(&f.state, &identity, &recovery).is_err());
    assert!(
        crate::research_files::store(&f.state)
            .unwrap()
            .artifact(&f.workspace_id, &upload.id)
            .is_err()
    );
    assert_eq!(f.store.load("scan-example", true).unwrap(), before);
    f.close().await;
}

#[tokio::test]
async fn local_supervisor_recovers_captured_output_after_publication_storage_failure() {
    recover_large_output(false).await;
}

#[tokio::test]
async fn restarted_host_discovers_captured_output_and_respects_live_owner() {
    recover_large_output(true).await;
}

async fn recover_large_output(restart: bool) {
    let f = Fixture::new().await;
    let workspace = f.root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let original = "Synthetic retained output ".repeat(20_000);
    std::fs::write(workspace.join("large.txt"), &original).unwrap();
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
            "resume-publication",
            &f.deputy,
            "Read the synthetic output",
        )
        .unwrap();
    let turn = f
        .store
        .local_claim("scan-example", &f.deputy, unix_ms(), 64)
        .unwrap()
        .unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = calls.clone();
    let provider=axum::Router::new().route("/v1/chat/completions",axum::routing::post(move || {
        let count=count.clone(); async move {
            assert_eq!(count.fetch_add(1,Ordering::SeqCst),0,"recovery must not repeat inference");
            let chunk=json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"large-output","type":"function","function":{"name":"read","arguments":json!({"path":"large.txt"}).to_string()}}]}}]});
            ([("content-type","text/event-stream")],format!("data: {chunk}\n\ndata: [DONE]\n\n"))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    f.state
        .credentials
        .save_custom_provider(beale_providers::CustomProvider {
            id: "custom-example".into(),
            name: "Example".into(),
            base_url: format!("http://{}/v1", listener.local_addr().unwrap()),
            models: vec![beale_providers::ModelEntry {
                id: "example".into(),
                name: "Example".into(),
            }],
            lead_model: "example".into(),
            small_model: "example".into(),
            reasoning: false,
        })
        .unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, provider).await.unwrap();
    });
    f.state.db.with_connection(|db| {db.execute_batch("CREATE TRIGGER fail_late_output BEFORE UPDATE OF accepted ON swarm_file_transfers WHEN NEW.accepted=1 BEGIN SELECT RAISE(ABORT,'synthetic publication full'); END;")?;Ok(())}).unwrap();
    let host = LocalWorkerHost::start(f.state.clone()).await.unwrap();
    assert!(host.run(&turn, None).await.is_err());
    drop(host);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.state.worker_recovery.lock().unwrap().len(), 1);
    let files = crate::research_files::store(&f.state).unwrap();
    let private = files.prepare_agent("scan-example", &f.deputy).unwrap();
    std::fs::write(private.join("large.txt"), "Synthetic later edit").unwrap();
    let before = f.store.load("scan-example", true).unwrap();
    f.state
        .db
        .with_connection(|db| {
            db.execute_batch("DROP TRIGGER fail_late_output;")?;
            Ok(())
        })
        .unwrap();
    if restart {
        let fresh =
            crate::build_state("control-example".into(), f.state.workspaces.clone()).unwrap();
        assert_eq!(f.store.local_worker_records(None, "").unwrap().len(), 1);
        super::super::restart::recover_page(&fresh, None, "").unwrap();
        assert_eq!(
            f.store.local_worker_records(None, "").unwrap().len(),
            1,
            "live owner must not be reclaimed"
        );
        assert!(
            f.store
                .load("scan-example", true)
                .unwrap()
                .events
                .iter()
                .all(|e| e.kind != "worker-recovery-artifact")
        );
        // Lose every retained process/launch handle. Only disk records remain.
        f.state.worker_recovery.lock().unwrap().clear();
        super::super::restart::recover_page(&fresh, None, "").unwrap();
        super::super::restart::recover_page(&fresh, None, "").unwrap();
        assert!(f.store.local_worker_records(None, "").unwrap().is_empty());
    } else {
        LocalWorkerHost::recover(&f.state, "scan-example").unwrap();
        LocalWorkerHost::recover(&f.state, "scan-example").unwrap();
    }
    assert!(f.state.worker_recovery.lock().unwrap().is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let saved = f.store.load("scan-example", true).unwrap();
    assert_eq!(saved.assignments, before.assignments);
    assert_eq!(
        saved.agent(&f.deputy).unwrap().summary.state,
        AgentState::Blocked
    );
    let events = saved
        .events
        .iter()
        .filter(|e| e.kind == "worker-recovery-artifact")
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 1);
    let artifact = files
        .artifact(
            &f.workspace_id,
            events[0].payload["artifact"].as_str().unwrap(),
        )
        .unwrap();
    assert_eq!(artifact.attempt, turn.attempt_id);
    assert_eq!(artifact.sha256, hash(format!("1\t{original}\n").as_bytes()));
    drop(files);
    server.abort();
    let _ = server.await;
    f.close().await;
}

#[tokio::test]
async fn late_capture_of_published_bytes_does_not_duplicate_evidence_or_parent_notice() {
    let f = Fixture::new().await;
    let identity = f.store.worker_bootstrap(&f.attempt.id).unwrap().identity;
    let workspace = f.root.join("workspace");
    let recovery = f.root.join("accepted-capture");
    std::fs::create_dir_all(&workspace).unwrap();
    let bytes = b"Synthetic previously accepted evidence";
    std::fs::write(workspace.join("candidate.txt"), bytes).unwrap();
    drop(WorkerSpool::open(&recovery, identity.clone()).unwrap());
    let (upload, _) =
        stage_worker_artifact(&workspace, &recovery, "already-published", "candidate.txt").unwrap();
    let key = hash(b"already-published");
    for (operation_id, operation) in [
        (
            format!("artifact-begin:{key}"),
            WorkerOperation::ArtifactBegin {
                artifact: upload.clone(),
            },
        ),
        (
            format!("artifact-chunk:{key}:0"),
            WorkerOperation::ArtifactChunk {
                transfer_id: upload.id.clone(),
                offset: 0,
                bytes: bytes.to_vec(),
                sha256: hash(bytes),
            },
        ),
        (
            format!("artifact-seal:{key}"),
            WorkerOperation::ArtifactSeal {
                transfer_id: upload.id.clone(),
            },
        ),
    ] {
        f.client()
            .send(&f.request(&operation_id, operation), Duration::from_secs(5))
            .await
            .unwrap();
    }
    super::super::supervisor::reconcile_owned_attempt(
        &f.store,
        &identity,
        true,
        WorkerDisposition::Failed,
        unix_ms(),
    )
    .unwrap();
    let before = f.store.load("scan-example", true).unwrap();
    let mailbox = f.store.local_state("scan-example").unwrap().inbox;
    super::super::recovery::publications(&f.state, &identity, &recovery).unwrap();
    super::super::recovery::publications(&f.state, &identity, &recovery).unwrap();
    assert_eq!(f.store.load("scan-example", true).unwrap(), before);
    assert_eq!(f.store.local_state("scan-example").unwrap().inbox, mailbox);
    let files = crate::research_files::store(&f.state).unwrap();
    let saved = files.artifact(&f.workspace_id, &upload.id).unwrap();
    assert_eq!(files.read_blob(&saved.sha256, saved.size).unwrap(), bytes);
    // Even a receipt replay must first establish the original execution identity.
    let original = f.request(
        &format!("artifact-seal:{key}"),
        WorkerOperation::ArtifactSeal {
            transfer_id: upload.id,
        },
    );
    for field in 0..5 {
        let mut foreign = original.clone();
        match field {
            0 => foreign.workspace_id = "workspace-other".into(),
            1 => foreign.scan_id = "scan-other".into(),
            2 => foreign.agent_id = "agent-other".into(),
            3 => foreign.attempt_id = "attempt-other".into(),
            _ => foreign.generation += 1,
        }
        assert!(
            f.store
                .recover_worker_artifact_request(&files, &foreign)
                .is_err()
        );
    }
    assert_eq!(f.store.load("scan-example", true).unwrap(), before);
    drop(files);
    f.close().await;
}
