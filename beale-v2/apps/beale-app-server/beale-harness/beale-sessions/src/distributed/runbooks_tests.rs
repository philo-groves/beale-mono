use super::*;
use beale_contracts::{
    runbooks::{Cell, Document},
    swarm_runbooks::{CellOutcome, Reply, Request as RunRequest, ResultKind},
};
use beale_runbooks::RunbookStore;

#[cfg(windows)]
#[path = "worker_runbooks_tests.rs"]
mod worker_execution;

fn document(f: &Fixture) -> Document {
    f.state
        .db
        .runbook_create(
            Document {
                id: String::new(),
                workspace_id: f.workspace_id.clone(),
                title: "Assigned example".into(),
                purpose: "Synthetic procedure".into(),
                cells: vec![Cell::Code {
                    id: String::new(),
                    name: "Example cell".into(),
                    language: "python".into(),
                    source: "print('example')".into(),
                }],
                next_cell_number: 1,
                created_at_ms: 0,
                updated_at_ms: 0,
                revision: 0,
            },
            "agent",
            &f.deputy,
        )
        .unwrap()
}
fn decoded(reply: WorkerReply) -> Reply {
    serde_json::from_value(acknowledged(reply)).unwrap()
}

#[tokio::test]
async fn preparation_errors_replay_after_record_changes_and_receipt_recovery() {
    use beale_contracts::tool_error::ToolErrorCode;
    let f = Fixture::new().await;
    let request = |id: &str| {
        f.request(
            id,
            WorkerOperation::Runbook {
                request: RunRequest::Prepare {
                    runbook_id: "R1".into(),
                },
            },
        )
    };
    let missing = request("missing-runbook-prepare");
    f.state.db.with_connection(|db| {db.execute_batch("CREATE TRIGGER fail_rejection_receipt BEFORE INSERT ON swarm_receipts BEGIN SELECT RAISE(ABORT,'synthetic rejection full'); END;")?;Ok(())}).unwrap();
    assert!(
        f.client()
            .send(&missing, Duration::from_secs(5))
            .await
            .is_err()
    );
    f.state
        .db
        .with_connection(|db| {
            db.execute_batch("DROP TRIGGER fail_rejection_receipt;")?;
            Ok(())
        })
        .unwrap();
    f.lose_response.store(true, Ordering::SeqCst);
    assert!(
        f.client()
            .send(&missing, Duration::from_secs(5))
            .await
            .is_err()
    );
    let result = decoded(
        f.client()
            .send(&missing, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    let ResultKind::Rejected { error } = &result.result else {
        panic!("expected missing record")
    };
    assert_eq!(error.code, ToolErrorCode::NotFound);
    let mut doc = document(&f);
    assert_eq!(
        decoded(
            f.client()
                .send(&missing, Duration::from_secs(5))
                .await
                .unwrap()
        ),
        result
    );
    let mut original = doc.clone();
    for cell in &mut original.cells {
        cell.id_mut().clear();
    }
    doc.cells.clear();
    let empty = f
        .state
        .db
        .runbook_edit(doc, 1, "agent", &f.deputy, "Synthetic empty procedure")
        .unwrap();
    let empty_request = request("empty-runbook-prepare");
    let ResultKind::Rejected { error } = decoded(
        f.client()
            .send(&empty_request, Duration::from_secs(5))
            .await
            .unwrap(),
    )
    .result
    else {
        panic!("expected no code rejection")
    };
    assert_eq!(error.code, ToolErrorCode::Rejected);
    f.state
        .db
        .with_connection(|db| {
            let count: i64 = db.query_row("SELECT count(*) FROM runbook_runs", [], |r| r.get(0))?;
            assert_eq!(count, 0);
            Ok(())
        })
        .unwrap();
    f.state
        .db
        .runbook_edit(
            original,
            empty.revision,
            "agent",
            &f.deputy,
            "Restore synthetic code",
        )
        .unwrap();
    let fresh = request("corrected-runbook-prepare");
    let accepted = decoded(
        f.client()
            .send(&fresh, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    assert!(matches!(&accepted.result, ResultKind::Prepared { .. }));
    assert_eq!(
        decoded(
            f.client()
                .send(&fresh, Duration::from_secs(5))
                .await
                .unwrap()
        ),
        accepted
    );
    f.state
        .db
        .with_connection(|db| {
            let count: i64 = db.query_row("SELECT count(*) FROM runbook_runs", [], |r| r.get(0))?;
            assert_eq!(count, 1);
            Ok(())
        })
        .unwrap();
    f.close().await;
}

#[tokio::test]
async fn supervisor_runbook_interruption_is_atomic_and_waits_for_termination() {
    let f = Fixture::new().await;
    let doc = document(&f);
    let prepare = f.request(
        "supervised-run-example",
        WorkerOperation::Runbook {
            request: RunRequest::Prepare {
                runbook_id: doc.id.clone(),
            },
        },
    );
    let ResultKind::Prepared { run } = decoded(
        f.client()
            .send(&prepare, Duration::from_secs(5))
            .await
            .unwrap(),
    )
    .result
    else {
        panic!("expected prepared run");
    };
    let identity = f.store.worker_bootstrap(&f.attempt.id).unwrap().identity;
    let reconcile = |confirmed| {
        super::super::supervisor::reconcile_owned_attempt(
            &f.store,
            &identity,
            confirmed,
            WorkerDisposition::Failed,
            unix_ms(),
        )
    };
    assert_eq!(
        reconcile(false).unwrap(),
        WorkerReconciliation::TerminationUnconfirmed
    );
    assert_eq!(
        f.state
            .db
            .runbook_run_status(&f.deputy, &run.run_id)
            .unwrap()
            .unwrap()
            .status,
        "running"
    );
    f.state.db.with_connection(|db| {db.execute_batch("CREATE TRIGGER fail_run_interruption BEFORE UPDATE ON runbook_runs WHEN NEW.status='interrupted' BEGIN SELECT RAISE(ABORT, 'synthetic interruption full'); END;")?;Ok(())}).unwrap();
    assert!(reconcile(true).is_err());
    assert!(f.state.db.session(&f.deputy).unwrap().is_none());
    assert_eq!(
        f.state
            .db
            .runbook_run_status(&f.deputy, &run.run_id)
            .unwrap()
            .unwrap()
            .status,
        "running"
    );
    assert_eq!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .attempts
            .iter()
            .find(|a| a.id == f.attempt.id)
            .unwrap()
            .state,
        ExecutionState::TerminationUnconfirmed
    );
    f.state
        .db
        .with_connection(|db| {
            db.execute_batch("DROP TRIGGER fail_run_interruption;")?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        reconcile(true).unwrap(),
        WorkerReconciliation::BranchStopped
    );
    assert_eq!(
        f.state
            .db
            .runbook_run_status(&f.deputy, &run.run_id)
            .unwrap()
            .unwrap()
            .status,
        "interrupted"
    );
    assert!(f.state.db.session(&f.deputy).unwrap().is_some());
    assert_eq!(
        reconcile(true).unwrap(),
        WorkerReconciliation::BranchStopped
    );
    f.close().await;
}

#[tokio::test]
async fn runbook_preparation_rejects_host_secrets_and_cancel_during_review() {
    let f = Fixture::new().await;
    let original = document(&f);
    let mut changed = original.clone();
    if let Cell::Code { source, .. } = &mut changed.cells[0] {
        *source = "print('host-control-example')".into();
    }
    let saved = f
        .state
        .db
        .runbook_edit(
            changed,
            original.revision,
            "agent",
            &f.deputy,
            "Synthetic secret boundary",
        )
        .unwrap();
    let request = f.request(
        "secret-run-example",
        WorkerOperation::Runbook {
            request: RunRequest::Prepare {
                runbook_id: original.id.clone(),
            },
        },
    );
    let result = decoded(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    let ResultKind::Rejected { error } = &result.result else {
        panic!("expected preflight rejection")
    };
    assert_eq!(
        error.code,
        beale_contracts::tool_error::ToolErrorCode::Rejected
    );
    assert!(!error.message.contains("host-control-example"));
    assert_eq!(
        decoded(
            f.client()
                .send(&request, Duration::from_secs(5))
                .await
                .unwrap()
        ),
        result
    );
    assert!(
        f.store
            .replay_worker_mutation(f.credential.bearer(), &request, unix_ms())
            .unwrap()
            .is_some()
    );
    f.state
        .db
        .runbook_edit(
            original.clone(),
            saved.revision,
            "agent",
            &f.deputy,
            "Restore synthetic procedure",
        )
        .unwrap();
    for rule in beale_safety::rules(&f.state.db).unwrap() {
        f.state
            .db
            .save_permission_rule(&rule.id, &rule.title, &rule.instruction, true)
            .unwrap();
    }
    let server = reviewer(&f, "deputy", true).await;
    let request = f.request(
        "canceled-run-example",
        WorkerOperation::Runbook {
            request: RunRequest::Prepare {
                runbook_id: original.id.clone(),
            },
        },
    );
    assert!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .is_err()
    );
    assert!(
        f.state
            .db
            .session_runbook_runs(&f.workspace_id, &f.deputy, &original.id)
            .unwrap()
            .is_empty()
    );
    server.abort();
    let _ = server.await;
    f.close().await;
}

#[tokio::test]
async fn routed_runbook_results_share_receipts_and_never_rebind_to_replacement() {
    let f = Fixture::new().await;
    let document = document(&f);
    let prepare = f.request(
        "prepare-run-example",
        WorkerOperation::Runbook {
            request: RunRequest::Prepare {
                runbook_id: document.id.clone(),
            },
        },
    );
    f.lose_response.store(true, Ordering::SeqCst);
    assert_eq!(
        f.client()
            .send(&prepare, Duration::from_secs(5))
            .await
            .unwrap_err()
            .kind,
        TransportFailure::Retryable
    );
    let prepared = f
        .client()
        .send(&prepare, Duration::from_secs(5))
        .await
        .unwrap();
    let ResultKind::Prepared { run } = decoded(prepared.clone()).result else {
        panic!("expected preparation")
    };
    assert_eq!(run.document, document);
    assert_eq!(
        f.state
            .db
            .session_runbook_runs(&f.workspace_id, &f.deputy, &document.id)
            .unwrap()
            .len(),
        1
    );
    let finish = f.request(
        "finish-run-example",
        WorkerOperation::Runbook {
            request: RunRequest::Finish {
                run_id: run.run_id.clone(),
            },
        },
    );
    assert_eq!(
        f.client()
            .send(&finish, Duration::from_secs(5))
            .await
            .unwrap_err()
            .kind,
        TransportFailure::Retryable
    );
    let outcome = CellOutcome {
        cell_id: document.cells[0].id().into(),
        success: true,
        exit_code: Some(0),
        stdout: "Synthetic cell output host-control-example".into(),
        stderr: String::new(),
    };
    let cell = f.request(
        "cell-run-example",
        WorkerOperation::Runbook {
            request: RunRequest::Cell {
                run_id: run.run_id.clone(),
                outcome: outcome.clone(),
            },
        },
    );
    f.state.db.with_connection(|db| {db.execute_batch("CREATE TRIGGER fail_run_cell_receipt BEFORE INSERT ON swarm_receipts WHEN NEW.operation_id='cell-run-example' BEGIN SELECT RAISE(ABORT, 'synthetic cell receipt full'); END;")?;Ok(())}).unwrap();
    assert_eq!(
        f.client()
            .send(&cell, Duration::from_secs(5))
            .await
            .unwrap_err()
            .kind,
        TransportFailure::Retryable
    );
    assert!(
        f.state
            .db
            .session_runbook_cell_results(&f.workspace_id, &f.deputy, &document.id)
            .unwrap()
            .is_empty()
    );
    f.state
        .db
        .with_connection(|db| {
            db.execute_batch("DROP TRIGGER fail_run_cell_receipt;")?;
            Ok(())
        })
        .unwrap();
    let accepted = f
        .client()
        .send(&cell, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(
        f.client()
            .send(&cell, Duration::from_secs(5))
            .await
            .unwrap(),
        accepted
    );
    let mut changed = cell.clone();
    if let WorkerOperation::Runbook {
        request: RunRequest::Cell { outcome, .. },
    } = &mut changed.operation
    {
        outcome.stdout = "Changed result".into();
    }
    assert!(
        f.client()
            .send(&changed, Duration::from_secs(5))
            .await
            .is_err()
    );
    let result = f
        .client()
        .send(&finish, Duration::from_secs(5))
        .await
        .unwrap();
    assert!(
        matches!(decoded(result.clone()).result,ResultKind::Finished {run} if run.success && run.output.contains("Synthetic cell output"))
    );
    assert!(
        !serde_json::to_string(&result)
            .unwrap()
            .contains("host-control-example")
    );
    let reopened = SwarmStore::open(HostDatabase::open(&f.root.join("host")).unwrap()).unwrap();
    assert_eq!(
        reopened
            .replay_worker_mutation(f.credential.bearer(), &finish, unix_ms())
            .unwrap()
            .unwrap()
            .result,
        acknowledged(result)
    );
    drop(reopened);
    f.store
        .fence_attempt("scan-example", &f.attempt.id, true)
        .unwrap();
    let (attempt, credential) = f
        .store
        .start_attempt(
            "scan-example",
            &f.deputy,
            &registration(),
            unix_ms(),
            unix_ms() + 60_000,
        )
        .unwrap();
    for old in [&prepare, &cell, &finish] {
        let mut rebound = old.clone();
        rebound.attempt_id = attempt.id.clone();
        rebound.generation = attempt.generation;
        assert!(
            f.store
                .replay_worker_mutation(credential.bearer(), &rebound, unix_ms())
                .is_err()
        );
    }
    f.close().await;
}

#[tokio::test]
async fn runbook_preparation_denial_is_durable_and_disabled_policy_cannot_dispatch() {
    let f = Fixture::new().await;
    let doc = document(&f);
    for rule in beale_safety::rules(&f.state.db).unwrap() {
        f.state
            .db
            .save_permission_rule(&rule.id, &rule.title, &rule.instruction, true)
            .unwrap();
    }
    let server = reviewer(&f, "deputy", false).await;
    let request = f.request(
        "denied-run-example",
        WorkerOperation::Runbook {
            request: RunRequest::Prepare {
                runbook_id: doc.id.clone(),
            },
        },
    );
    let denied = f
        .client()
        .send(&request, Duration::from_secs(5))
        .await
        .unwrap();
    assert!(matches!(
        decoded(denied.clone()).result,
        ResultKind::Denied {}
    ));
    assert!(
        f.state
            .db
            .session_runbook_runs(&f.workspace_id, &f.deputy, &doc.id)
            .unwrap()
            .is_empty()
    );
    server.abort();
    let _ = server.await;
    assert_eq!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap(),
        denied
    );
    f.close().await;
    let f = Fixture::with_tools(false).await;
    let request = f.request(
        "unavailable-run-example",
        WorkerOperation::Runbook {
            request: RunRequest::Prepare {
                runbook_id: "R1".into(),
            },
        },
    );
    assert_eq!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap_err()
            .kind,
        TransportFailure::Rejected
    );
    f.close().await;
}

#[tokio::test]
async fn preparation_conflict_after_review_is_durable_and_requires_fresh_review() {
    let f = Fixture::new().await;
    let document = document(&f);
    for rule in beale_safety::rules(&f.state.db).unwrap() {
        f.state
            .db
            .save_permission_rule(&rule.id, &rule.title, &rule.instruction, true)
            .unwrap();
    }
    let db = f.state.db.clone();
    let edited = document.clone();
    let deputy = f.deputy.clone();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = calls.clone();
    let app=axum::Router::new().route("/v1/chat/completions",axum::routing::post(move || {
        let db=db.clone(); let mut edited=edited.clone(); let deputy=deputy.clone(); let counter=counter.clone();
        async move {
            if counter.fetch_add(1,Ordering::SeqCst)==0 {
                edited.purpose="Synthetic concurrent edit".into();
                db.runbook_edit(edited,1,"agent",&deputy,"Synthetic review race").unwrap();
            }
            let chunk=json!({"choices":[{"delta":{"content":json!({"decision":"approved","reason":"Synthetic review"}).to_string()}}]});
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
        axum::serve(listener, app).await.unwrap();
    });
    let request = |id: &str| {
        f.request(
            id,
            WorkerOperation::Runbook {
                request: RunRequest::Prepare {
                    runbook_id: document.id.clone(),
                },
            },
        )
    };
    let stale = request("review-race");
    let reply = decoded(
        f.client()
            .send(&stale, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    let ResultKind::Rejected { error } = &reply.result else {
        panic!("expected review conflict")
    };
    assert_eq!(
        error.code,
        beale_contracts::tool_error::ToolErrorCode::Conflict
    );
    assert!(
        f.state
            .db
            .session_runbook_runs(&f.workspace_id, &f.deputy, &document.id)
            .unwrap()
            .is_empty()
    );
    let reviewed = calls.load(Ordering::SeqCst);
    assert!(reviewed > 0);
    assert_eq!(
        decoded(
            f.client()
                .send(&stale, Duration::from_secs(5))
                .await
                .unwrap()
        ),
        reply
    );
    assert_eq!(calls.load(Ordering::SeqCst), reviewed);
    let reply = decoded(
        f.client()
            .send(&request("fresh-review"), Duration::from_secs(5))
            .await
            .unwrap(),
    );
    let ResultKind::Prepared { run } = reply.result else {
        panic!("expected fresh preparation")
    };
    assert_eq!(run.document.revision, 2);
    assert_eq!(run.document.purpose, "Synthetic concurrent edit");
    assert!(calls.load(Ordering::SeqCst) > reviewed);
    server.abort();
    let _ = server.await;
    f.close().await;
}
