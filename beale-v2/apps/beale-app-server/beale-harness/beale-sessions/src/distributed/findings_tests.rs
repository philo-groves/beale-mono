use super::*;
use beale_contracts::swarm_findings::{
    Command, Observation, Reply, Request as VerifyRequest, ResultKind,
};
use beale_findings::FindingStore;

#[cfg(windows)]
#[path = "worker_findings_tests.rs"]
mod worker_execution;

fn candidate(f: &Fixture) -> Command {
    let output=beale_findings::execute(&f.state.db,&beale_findings::ToolContext {workspace_id:&f.workspace_id,session_id:&f.deputy,evidence_role:None,pending:&[]},"findings_create",&json!({"title":"Synthetic candidate","summary":"Example observation","hypothesis":"Example hypothesis"})).unwrap();
    let output: Value = serde_json::from_str(&output).unwrap();
    Command {
        id: output["record"]["id"].as_str().unwrap().into(),
        expected_revision: 1,
        command: "python proof.py".into(),
        oracle: beale_contracts::findings::Oracle::StdoutContains {
            value: "example-observation".into(),
        },
        timeout_seconds: 10,
    }
}
fn decoded(reply: WorkerReply) -> Reply {
    serde_json::from_value(acknowledged(reply)).unwrap()
}

#[tokio::test]
async fn verifier_preparation_rejections_do_not_start_runs_or_hide_storage_failures() {
    use beale_contracts::tool_error::ToolErrorCode;
    let f = Fixture::new().await;
    let command = candidate(&f);
    let request = |id: &str, command: Command| {
        f.request(
            id,
            WorkerOperation::Verify {
                request: VerifyRequest::Prepare {
                    tool_call_id: id.into(),
                    command,
                },
            },
        )
    };
    let mut missing = command.clone();
    missing.id = "F999".into();
    let missing = request("missing-verification", missing);
    let mut stale = command.clone();
    stale.expected_revision = 2;
    let stale = request("stale-verification", stale);
    for (request, code) in [
        (&missing, ToolErrorCode::NotFound),
        (&stale, ToolErrorCode::Conflict),
    ] {
        f.lose_response.store(true, Ordering::SeqCst);
        assert!(
            f.client()
                .send(request, Duration::from_secs(5))
                .await
                .is_err()
        );
        let result = decoded(
            f.client()
                .send(request, Duration::from_secs(5))
                .await
                .unwrap(),
        );
        let ResultKind::Rejected { error } = &result.result else {
            panic!("expected preparation rejection")
        };
        assert_eq!(error.code, code);
        assert_eq!(
            decoded(
                f.client()
                    .send(request, Duration::from_secs(5))
                    .await
                    .unwrap()
            ),
            result
        );
    }
    f.state.db.with_connection(|db| {let count:i64=db.query_row("SELECT count(*) FROM finding_worker_verifications",[],|r|r.get(0))?;assert_eq!(count,0);db.execute_batch("CREATE TRIGGER fail_verification_prepare BEFORE INSERT ON finding_worker_verifications BEGIN SELECT RAISE(ABORT,'synthetic verification full'); END;")?;Ok(())}).unwrap();
    let fresh = request("corrected-verification", command);
    assert!(
        f.client()
            .send(&fresh, Duration::from_secs(5))
            .await
            .is_err()
    );
    f.state
        .db
        .with_connection(|db| {
            let count: i64 = db.query_row(
                "SELECT count(*) FROM swarm_receipts WHERE operation_id='corrected-verification'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(count, 0);
            db.execute_batch("DROP TRIGGER fail_verification_prepare;")?;
            Ok(())
        })
        .unwrap();
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
    f.close().await;
}
async fn prepare(f: &Fixture, command: &Command) -> String {
    let request = f.request(
        "prepare-example",
        WorkerOperation::Verify {
            request: VerifyRequest::Prepare {
                tool_call_id: "verify-example".into(),
                command: command.clone(),
            },
        },
    );
    f.lose_response.store(true, Ordering::SeqCst);
    assert!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .is_err()
    );
    let first = decoded(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    assert_eq!(
        first,
        decoded(
            f.client()
                .send(&request, Duration::from_secs(5))
                .await
                .unwrap()
        )
    );
    let ResultKind::Prepared { run_id } = first.result else {
        panic!("expected prepared verifier");
    };
    run_id
}
async fn accept(f: &Fixture, run: &str, index: u8) -> WorkerRequest {
    let identity = f.store.worker_bootstrap(&f.attempt.id).unwrap().identity;
    let observation = Observation {
        identity,
        run_id: run.into(),
        index,
        exit_code: Some(0),
        stdout: "example-observation".into(),
        stderr: String::new(),
        duration_ms: 15,
        error: None,
    };
    let workspace = f.root.join("artifact-workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let source = format!("observation-{index}.json");
    std::fs::write(
        workspace.join(&source),
        serde_json::to_vec(&observation).unwrap(),
    )
    .unwrap();
    let artifact = crate::research_files::store(&f.state)
        .unwrap()
        .publish_file(
            &workspace,
            beale_contracts::swarm_files::Artifact {
                id: format!("artifact-observation-{index}"),
                operation: format!("publish-observation-{index}"),
                workspace: f.workspace_id.clone(),
                scan: "scan-example".into(),
                producer: f.deputy.clone(),
                assignment: None,
                attempt: f.attempt.id.clone(),
                generation: f.attempt.generation,
                revision: 1,
                source_path: source,
                sha256: String::new(),
                size: 0,
                executable: None,
                previous: None,
            },
        )
        .unwrap();
    let request = f.request(
        &format!("observe-{index}"),
        WorkerOperation::Verify {
            request: VerifyRequest::Observe {
                run_id: run.into(),
                index,
                artifact_id: artifact.id,
            },
        },
    );
    f.client()
        .send(&request, Duration::from_secs(5))
        .await
        .unwrap();
    request
}

#[tokio::test]
async fn verifier_gateway_attaches_once_and_rolls_back_with_receipt() {
    let f = Fixture::new().await;
    let command = candidate(&f);
    let run = prepare(&f, &command).await;
    let mut wrong_ordinal = accept(&f, &run, 0).await;
    wrong_ordinal.operation_id = "wrong-ordinal".into();
    if let WorkerOperation::Verify {
        request: VerifyRequest::Observe { index, .. },
    } = &mut wrong_ordinal.operation
    {
        *index = 1;
    }
    assert!(
        f.client()
            .send(&wrong_ordinal, Duration::from_secs(1))
            .await
            .is_err()
    );
    let replay = accept(&f, &run, 1).await;
    f.client()
        .send(&replay, Duration::from_secs(5))
        .await
        .unwrap();
    f.state.db.with_connection(|db|{db.execute_batch("CREATE TRIGGER fail_verifier_finish BEFORE UPDATE ON finding_worker_verifications WHEN NEW.state='completed' BEGIN SELECT RAISE(ABORT,'synthetic receipt failure'); END;")?;Ok(())}).unwrap();
    let request = f.request(
        "finish-example",
        WorkerOperation::Verify {
            request: VerifyRequest::Finish {
                run_id: run.clone(),
            },
        },
    );
    assert!(
        f.client()
            .send(&request, Duration::from_secs(2))
            .await
            .is_err()
    );
    assert_eq!(
        f.state
            .db
            .finding_get(&f.workspace_id, &command.id)
            .unwrap()
            .unwrap()
            .revision,
        1
    );
    f.state
        .db
        .with_connection(|db| {
            db.execute_batch("DROP TRIGGER fail_verifier_finish;")?;
            Ok(())
        })
        .unwrap();
    f.lose_response.store(true, Ordering::SeqCst);
    assert!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .is_err()
    );
    let reply = decoded(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    let ResultKind::Finished { run } = reply.result else {
        panic!("expected verification");
    };
    assert!(run.recorded && run.verified);
    f.client()
        .send(&request, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(
        f.state
            .db
            .finding_get(&f.workspace_id, &command.id)
            .unwrap()
            .unwrap()
            .revision,
        2
    );
    // Changing any envelope field cannot reuse an earlier run receipt.
    let mut replaced = request;
    replaced.generation += 1;
    assert!(
        f.client()
            .send(&replaced, Duration::from_secs(1))
            .await
            .is_err()
    );
    f.close().await;
}

#[tokio::test]
async fn verifier_preparation_enforces_saved_tools_and_rejects_host_secrets() {
    for enabled in [false, true] {
        let f = Fixture::with_tools(enabled).await;
        let mut command = candidate(&f);
        if enabled {
            command.command = "echo host-control-example".into();
        }
        let request = f.request(
            "invalid-preparation",
            WorkerOperation::Verify {
                request: VerifyRequest::Prepare {
                    tool_call_id: "call-example".into(),
                    command,
                },
            },
        );
        let response = f.client().send(&request, Duration::from_secs(5)).await;
        if enabled {
            let result = decoded(response.unwrap());
            let ResultKind::Rejected { error } = result.result else {
                panic!("expected preflight rejection")
            };
            assert_eq!(
                error.code,
                beale_contracts::tool_error::ToolErrorCode::Rejected
            );
            assert!(!error.message.contains("host-control-example"));
        } else {
            assert!(response.is_err());
        }
        f.state
            .db
            .with_connection(|db| {
                assert_eq!(
                    db.query_row(
                        "SELECT COUNT(*) FROM finding_worker_verifications",
                        [],
                        |r| r.get::<_, u32>(0)
                    )?,
                    0
                );
                Ok(())
            })
            .unwrap();
        f.close().await;
    }
}

#[tokio::test]
async fn verifier_interruption_waits_for_termination_and_commits_atomically() {
    let f = Fixture::new().await;
    let command = candidate(&f);
    let run = prepare(&f, &command).await;
    accept(&f, &run, 0).await;
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
    f.state.db.with_connection(|db|{db.execute_batch("CREATE TRIGGER fail_verifier_interrupt BEFORE UPDATE ON finding_worker_verifications WHEN NEW.state='interrupted' BEGIN SELECT RAISE(ABORT,'synthetic interruption failure'); END;")?;Ok(())}).unwrap();
    assert!(reconcile(true).is_err());
    assert!(f.state.db.session(&f.deputy).unwrap().is_none());
    f.state
        .db
        .with_connection(|db| {
            assert_eq!(
                db.query_row(
                    "SELECT state FROM finding_worker_verifications WHERE id=?1",
                    [&run],
                    |r| r.get::<_, String>(0)
                )?,
                "running"
            );
            db.execute_batch("DROP TRIGGER fail_verifier_interrupt;")?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        reconcile(true).unwrap(),
        WorkerReconciliation::BranchStopped
    );
    f.state
        .db
        .with_connection(|db| {
            assert_eq!(
                db.query_row(
                    "SELECT state FROM finding_worker_verifications WHERE id=?1",
                    [&run],
                    |r| r.get::<_, String>(0)
                )?,
                "interrupted"
            );
            assert_eq!(
                db.query_row(
                    "SELECT COUNT(*) FROM finding_worker_observations WHERE run_id=?1",
                    [&run],
                    |r| r.get::<_, u32>(0)
                )?,
                1
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(
        f.state
            .db
            .finding_get(&f.workspace_id, &command.id)
            .unwrap()
            .unwrap()
            .revision,
        1
    );
    f.close().await;
}
