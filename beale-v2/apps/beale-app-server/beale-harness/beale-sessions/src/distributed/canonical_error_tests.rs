use super::*;
use beale_contracts::tool_error::{ToolError, ToolErrorCode};
fn request(f: &Fixture, id: &str, tool: &str, arguments: Value) -> WorkerRequest {
    f.request(
        id,
        WorkerOperation::Canonical {
            tool: tool.into(),
            arguments,
        },
    )
}
async fn send(f: &Fixture, r: &WorkerRequest) -> Value {
    acknowledged(f.client().send(r, Duration::from_secs(5)).await.unwrap())
}
fn failure(result: &Value, code: ToolErrorCode) {
    assert_eq!(result["is_error"], true, "{result}");
    let error: ToolError = serde_json::from_value(result["error"].clone()).unwrap();
    error.validate().unwrap();
    assert_eq!(error.code, code);
    assert_eq!(result["output"], error.message);
}

#[tokio::test]
async fn hierarchy_rejections_preserve_branch_state_and_replay_without_authority_bypass() {
    let f = Fixture::new().await;
    f.allocation_snapshot();
    let profile = f.store.worker_progress(&f.attempt.id).unwrap().turn.profile;
    let spawn = |profile: &str| json!({"profile":{"id":profile,"revision":1},"task":task(),"independent":false});
    let handoff = json!({"to_profile":"missing-profile","task":task(),"scope":["Synthetic scope"],"payload":{"kind":"task"}});
    let mut spoofed = handoff.clone();
    spoofed["from_agent"] = json!(f.leader);
    let mut wrong_route = handoff.clone();
    wrong_route["to_profile"] = json!(profile);
    wrong_route["route_id"] = json!("missing-route");
    for (index, (tool, args, code)) in [
        (
            "swarm_spawn_subagent",
            json!({}),
            ToolErrorCode::InvalidArguments,
        ),
        (
            "swarm_spawn_subagent",
            spawn("missing-profile"),
            ToolErrorCode::NotFound,
        ),
        (
            "swarm_spawn_subagent",
            spawn(&profile),
            ToolErrorCode::Rejected,
        ),
        (
            "swarm_message_subagent",
            json!({"child_id":"missing-child","message":"Example"}),
            ToolErrorCode::NotFound,
        ),
        (
            "swarm_stop_subagent",
            json!({"child_id":f.leader}),
            ToolErrorCode::Rejected,
        ),
        (
            "swarm_message_parent",
            json!({"message":""}),
            ToolErrorCode::InvalidArguments,
        ),
        (
            "swarm_message_parent",
            json!({"message":"Example handoff","handoff":handoff}),
            ToolErrorCode::NotFound,
        ),
        // Host identity is not model input.
        (
            "swarm_message_parent",
            json!({"message":"Example handoff","handoff":spoofed}),
            ToolErrorCode::InvalidArguments,
        ),
        (
            "swarm_message_parent",
            json!({"message":"Example handoff","handoff":wrong_route}),
            ToolErrorCode::Rejected,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let r = request(&f, &format!("hierarchy-rejected-{index}"), tool, args);
        let result = send(&f, &r).await;
        failure(&result, code);
        assert_eq!(send(&f, &r).await, result);
        assert_eq!(f.store.summaries("scan-example").unwrap().len(), 2);
    }
    f.state.db.with_connection(|db| {
        let raw:String=db.query_row("SELECT document FROM swarm_local WHERE scan_id='scan-example'",[],|r|r.get(0))?;
        let mut local:Value=serde_json::from_str(&raw)?;
        local["inbox"]=json!((1..=1024).map(|sequence|json!({"sequence":sequence,"id":format!("message-{sequence}"),"from":f.deputy,"to":f.leader,"message":"Synthetic queued message","handoff":null})).collect::<Vec<_>>());
        local["sequence"]=json!(1024);
        db.execute("UPDATE swarm_local SET document=?1 WHERE scan_id='scan-example'",[local.to_string()])?;Ok(())
    }).unwrap();
    let full = request(
        &f,
        "full-parent-mailbox",
        "swarm_message_parent",
        json!({"message":"Synthetic new message"}),
    );
    failure(&send(&f, &full).await, ToolErrorCode::Rejected);
    f.state.db.with_connection(|db| {
        let count:i64=db.query_row("SELECT json_array_length(document,'$.inbox') FROM swarm_local WHERE scan_id='scan-example'",[],|r|r.get(0))?;assert_eq!(count,1024);
        db.execute_batch("CREATE TRIGGER fail_hierarchy_receipt BEFORE INSERT ON swarm_receipts BEGIN SELECT RAISE(ABORT,'synthetic hierarchy full'); END;")?;Ok(())
    }).unwrap();
    let good = request(
        &f,
        "corrected-spawn",
        "swarm_spawn_subagent",
        spawn("investigator"),
    );
    assert!(
        f.client()
            .send(&good, Duration::from_secs(5))
            .await
            .is_err()
    );
    assert_eq!(f.store.summaries("scan-example").unwrap().len(), 2);
    f.state
        .db
        .with_connection(|db| {
            db.execute_batch("DROP TRIGGER fail_hierarchy_receipt;")?;
            Ok(())
        })
        .unwrap();
    let accepted = send(&f, &good).await;
    let child = accepted["child_id"].as_str().unwrap();
    assert_eq!(send(&f, &good).await, accepted);
    assert_eq!(f.store.summaries("scan-example").unwrap().len(), 3);
    send(
        &f,
        &request(
            &f,
            "stop-new-child",
            "swarm_stop_subagent",
            json!({"child_id":child}),
        ),
    )
    .await;
    // A parent may explicitly resume a stopped child whose execution has
    // stopped. It runs as a new assignment; no earlier attempt is revived.
    let resume = request(
        &f,
        "resume-stopped-child",
        "swarm_message_subagent",
        json!({"child_id":child,"message":"Synthetic resume"}),
    );
    let resumed = send(&f, &resume).await;
    assert_eq!(resumed["queued"], true, "{resumed}");
    assert_eq!(send(&f, &resume).await, resumed);
    assert!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .assignments
            .iter()
            .any(|a| a.child_id == child
                && a.state == AssignmentState::Queued
                && a.task.question == "Synthetic resume")
    );
    f.store
        .fence_attempt("scan-example", &f.attempt.id, true)
        .unwrap();
    assert!(
        f.client()
            .send(&resume, Duration::from_secs(5))
            .await
            .is_err()
    );
    f.close().await;
}

#[cfg(feature = "surfaces")]
#[tokio::test]
async fn surface_rejections_preserve_files_and_do_not_mask_host_failures() {
    let workspace_id = "11111111-1111-4111-8111-111111111111";
    let f = Fixture::with_workspace(
        true,
        beale_contracts::swarm_worker::WorkerShell::native(),
        false,
        workspace_id,
    )
    .await;
    let workspace = f.root.join("workspace");
    std::fs::create_dir_all(workspace.join(".beale")).unwrap();
    let path = workspace.join("workspace.json");
    std::fs::write(&path,json!({"schemaVersion":7,"workspaceId":workspace_id,"name":"Example","directories":[],"surfaces":[],"rules":[],"foundationRevision":1,"surfacesRevision":1,"rulesRevision":1,"nextSurfaceNumber":1}).to_string()).unwrap();
    let create = |name: &str| json!({"operation":"create","definition":{"type_id":"repository","name":name,"locator":{"kind":"freeform","value":name},"scope":"in_scope"}});
    let r = request(
        &f,
        "valid-surface",
        "surface_save",
        create("Example repository"),
    );
    assert_eq!(send(&f, &r).await["id"], "S1");
    let original = std::fs::read(&path).unwrap();
    for (index,(tool,args,code)) in [
        ("surface_query",json!({"limit":"one"}),ToolErrorCode::InvalidArguments),
        ("surface_query",json!({"scope":"unknown"}),ToolErrorCode::InvalidArguments),
        ("surface_query",json!({"text":[]}),ToolErrorCode::InvalidArguments),
        ("surface_read",json!({"surface_id":"S999","section":"record"}),ToolErrorCode::NotFound),
        ("surface_save",json!({"operation":"revise","id":"S1","expected_revision":99,"definition":{"name":"Changed"}}),ToolErrorCode::Conflict),
        ("surface_save",json!({"operation":"revise","id":"S1","expected_revision":2,"relationships":[{"target_id":"S999","label":"uses"}]}),ToolErrorCode::InvalidArguments),
        ("surface_save",json!({"operation":"revise","id":"S1","expected_revision":2,"parent_id":"S999"}),ToolErrorCode::NotFound),
        ("surface_threat_model_edit",json!({"remove_id":"T1"}),ToolErrorCode::InvalidArguments),
        ("surface_threat_model_edit",json!({"surface_id":"../other","remove_id":"T1"}),ToolErrorCode::InvalidArguments),
        ("surface_threat_model_edit",json!({"surface_id":"S999","remove_id":"T1"}),ToolErrorCode::NotFound),
        ("surface_threat_model_edit",json!({"surface_id":"S1","remove_id":"T1"}),ToolErrorCode::NotFound),
    ].into_iter().enumerate() {
        let r=request(&f,&format!("rejected-surface-{index}"),tool,args);
        let result=send(&f,&r).await;
        failure(&result,code);
        assert_eq!(send(&f,&r).await,result);
        assert_eq!(std::fs::read(&path).unwrap(),original);
        assert!(!workspace.join(".beale/surface-threat-models/S1.json").exists());
    }
    let r = request(
        &f,
        "surface-storage-retry",
        "surface_save",
        create("Second example repository"),
    );
    let overflow = |field: &str, bytes: &[u8]| {
        let mut document: Value = serde_json::from_slice(bytes).unwrap();
        document[field] = json!(u64::MAX);
        document.to_string().into_bytes()
    };
    for broken in [
        b"invalid".to_vec(),
        {
            let mut document: Value = serde_json::from_slice(&original).unwrap();
            document["nextSurfaceNumber"] = json!(u64::MAX);
            document.to_string().into_bytes()
        },
        overflow("surfacesRevision", &original),
    ] {
        std::fs::write(&path, &broken).unwrap();
        assert!(f.client().send(&r, Duration::from_secs(5)).await.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), broken);
        f.state.db.with_connection(|db| {let count:i64=db.query_row("SELECT count(*) FROM swarm_receipts WHERE operation_id='surface-storage-retry'",[],|r|r.get(0))?;assert_eq!(count,0);Ok(())}).unwrap();
    }
    std::fs::write(&path, original).unwrap();
    assert_eq!(send(&f, &r).await["id"], "S2");
    let scenario = json!({"title":"Example scenario","attacker_position":"Synthetic client","preconditions":[],"suspected_failure":"Example boundary","invariant_at_risk":"Example isolation","potential_impact":"Example effect","validation_question":"Does the example hold?"});
    let r = request(
        &f,
        "valid-threat-model",
        "surface_threat_model_edit",
        json!({"surface_id":"S1","scenario":scenario}),
    );
    assert_eq!(send(&f, &r).await["changed_id"], "T1");
    let threat_path = workspace.join(".beale/surface-threat-models/S1.json");
    let original = std::fs::read(&threat_path).unwrap();
    let mut invalid = scenario.clone();
    invalid["title"] = json!("");
    let bad = request(
        &f,
        "invalid-threat-fields",
        "surface_threat_model_edit",
        json!({"surface_id":"S1","scenario":invalid}),
    );
    failure(&send(&f, &bad).await, ToolErrorCode::InvalidArguments);
    assert_eq!(std::fs::read(&threat_path).unwrap(), original);
    let retry = request(
        &f,
        "threat-storage-retry",
        "surface_threat_model_edit",
        json!({"surface_id":"S1","scenario":scenario}),
    );
    for broken in [
        b"invalid".to_vec(),
        overflow("revision", &original),
        overflow("nextScenarioNumber", &original),
    ] {
        std::fs::write(&threat_path, &broken).unwrap();
        assert!(
            f.client()
                .send(&retry, Duration::from_secs(5))
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&threat_path).unwrap(), broken);
        f.state
            .db
            .with_connection(|db| {
                let count: i64 = db.query_row(
                    "SELECT count(*) FROM swarm_receipts WHERE operation_id='threat-storage-retry'",
                    [],
                    |r| r.get(0),
                )?;
                assert_eq!(count, 0);
                Ok(())
            })
            .unwrap();
    }
    std::fs::write(&threat_path, original).unwrap();
    assert_eq!(send(&f, &retry).await["changed_id"], "T2");
    f.close().await;
}

#[cfg(feature = "memories")]
#[tokio::test]
async fn memory_rejections_replay_without_masking_storage_failures() {
    let f = Fixture::new().await;
    let invalid = [
        ("memory_query", json!({"text":4})),
        ("memory_query", json!({"kind":false})),
        ("memory_query", json!({"status":"unknown"})),
        ("memory_query", json!({"as_of_ms":-1})),
        ("memory_query", json!({"as_of_ms":"now"})),
        ("memory_query", json!({"limit":0})),
        ("memory_query", json!({"offset":10001})),
        ("memory_session_query", json!({"text":"x".repeat(257)})),
        ("memory_session_query", json!({"kind":"unknown"})),
        ("memory_read", json!({})),
        ("memory_read", json!({"id":5})),
        ("memory_read", json!({"id":"example","history_limit":0})),
        ("memory_graph", json!({"id":"example","depth":0})),
        (
            "memory_graph",
            json!({"id":"example","include_superseded":"false"}),
        ),
    ];
    for (index, (tool, args)) in invalid.into_iter().enumerate() {
        let r = request(&f, &format!("invalid-memory-{index}"), tool, args);
        let rejected = send(&f, &r).await;
        failure(&rejected, ToolErrorCode::InvalidArguments);
        assert_eq!(send(&f, &r).await, rejected);
    }
    for tool in ["memory_read", "memory_graph"] {
        let r = request(
            &f,
            &format!("missing-{tool}"),
            tool,
            json!({"id":"missing-example"}),
        );
        failure(&send(&f, &r).await, ToolErrorCode::NotFound);
    }
    let db = beale_storage::rusqlite::Connection::open(
        f.state.workspaces.root().join("memories.sqlite"),
    )
    .unwrap();
    db.execute_batch("ALTER TABLE entities RENAME TO unavailable_entities;")
        .unwrap();
    let r = request(&f, "retry-memory-storage", "memory_query", json!({}));
    assert!(f.client().send(&r, Duration::from_secs(5)).await.is_err());
    f.state
        .db
        .with_connection(|db| {
            let count: i64 = db.query_row(
                "SELECT count(*) FROM swarm_receipts WHERE operation_id='retry-memory-storage'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(count, 0);
            Ok(())
        })
        .unwrap();
    db.execute_batch("ALTER TABLE unavailable_entities RENAME TO entities;")
        .unwrap();
    let accepted = send(&f, &r).await;
    assert_eq!(accepted["items"], json!([]));
    db.execute_batch("ALTER TABLE entities RENAME TO unavailable_entities;")
        .unwrap();
    // An accepted snapshot remains replayable even while the source is damaged.
    assert_eq!(send(&f, &r).await, accepted);
    drop(db);
    f.close().await;
}

#[cfg(feature = "agent-feedback")]
#[tokio::test]
async fn feedback_rejections_are_durable_and_do_not_append_records() {
    let f = Fixture::new().await;
    let path = f
        .state
        .workspaces
        .root()
        .join(beale_agent_feedback::FILE_NAME);
    for (index, args) in [
        json!({}),
        json!({"kind":"unknown","observation":"Example"}),
        json!({"kind":"quirk","observation":""}),
        json!({"kind":"flaw","observation":"x".repeat(4001)}),
    ]
    .into_iter()
    .enumerate()
    {
        let r = request(
            &f,
            &format!("invalid-feedback-{index}"),
            "agent_feedback",
            args,
        );
        let result = send(&f, &r).await;
        failure(&result, ToolErrorCode::InvalidArguments);
        assert_eq!(send(&f, &r).await, result);
        assert!(!path.exists());
    }
    let bad = request(&f, "lost-feedback-error", "agent_feedback", json!({}));
    f.lose_response.store(true, Ordering::SeqCst);
    assert!(f.client().send(&bad, Duration::from_secs(5)).await.is_err());
    failure(&send(&f, &bad).await, ToolErrorCode::InvalidArguments);
    let corrected = request(
        &f,
        "corrected-feedback",
        "agent_feedback",
        json!({"kind":"quirk","observation":"Synthetic display delay"}),
    );
    assert_eq!(send(&f, &corrected).await["is_error"], false);
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes.iter().filter(|b| **b == b'\n').count(), 1);
    // Corrupt complete log entries are a host fault, not an input rejection.
    std::fs::write(&path, b"invalid\n").unwrap();
    let next = request(
        &f,
        "retry-feedback-storage",
        "agent_feedback",
        json!({"kind":"limitation","observation":"Synthetic tool delay"}),
    );
    assert!(
        f.client()
            .send(&next, Duration::from_secs(5))
            .await
            .is_err()
    );
    f.state
        .db
        .with_connection(|db| {
            let count: i64 = db.query_row(
                "SELECT count(*) FROM swarm_receipts WHERE operation_id='retry-feedback-storage'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(count, 0);
            Ok(())
        })
        .unwrap();
    std::fs::write(&path, bytes).unwrap();
    assert_eq!(send(&f, &next).await["is_error"], false);
    let accepted = std::fs::read(&path).unwrap();
    assert_eq!(accepted.iter().filter(|b| **b == b'\n').count(), 2);
    send(&f, &next).await;
    assert_eq!(std::fs::read(&path).unwrap(), accepted);
    f.close().await;
}
#[cfg(feature = "findings")]
#[tokio::test]
async fn findings_rejections_are_durable_and_correctable_without_consuming_record_ids() {
    let f = Fixture::new().await;
    for (id, args) in [
        (
            "wrong-type",
            json!({"title":[],"summary":"Example","hypothesis":"Example"}),
        ),
        (
            "invalid-title",
            json!({"title":"","summary":"Example","hypothesis":"Example"}),
        ),
    ] {
        let r = request(&f, id, "findings_create", args);
        let result = send(&f, &r).await;
        failure(&result, ToolErrorCode::InvalidArguments);
        assert_eq!(send(&f, &r).await, result);
    }
    failure(
        &send(
            &f,
            &request(&f, "missing-finding", "findings_read", json!({"id":"F999"})),
        )
        .await,
        ToolErrorCode::NotFound,
    );
    failure(
        &send(
            &f,
            &request(&f, "bad-limit", "findings_query", json!({"limit":-1})),
        )
        .await,
        ToolErrorCode::InvalidArguments,
    );
    let good = request(
        &f,
        "valid-finding",
        "findings_create",
        json!({"title":"Example","summary":"Synthetic summary","hypothesis":"Synthetic hypothesis"}),
    );
    let created = send(&f, &good).await;
    assert_eq!(created["record"]["id"], "F1");
    let stale = request(
        &f,
        "stale-finding",
        "findings_update",
        json!({"id":"F1","expected_revision":99,"state":"finding","reason":"Synthetic edit","patch":{"title":"Updated example"}}),
    );
    let error = send(&f, &stale).await;
    failure(&error, ToolErrorCode::Conflict);
    let corrected = request(
        &f,
        "corrected-finding",
        "findings_update",
        json!({"id":"F1","expected_revision":1,"state":"finding","reason":"Synthetic edit","patch":{"title":"Updated example"}}),
    );
    assert_eq!(send(&f, &corrected).await["record"]["revision"], 2);
    assert_eq!(send(&f, &stale).await, error);
    failure(
        &send(
            &f,
            &request(
                &f,
                "invalid-query-text",
                "findings_query",
                json!({"text":[]}),
            ),
        )
        .await,
        ToolErrorCode::InvalidArguments,
    );
    let premature = request(
        &f,
        "premature-report",
        "findings_update",
        json!({"id":"F1","expected_revision":2,"state":"reported","reason":"Synthetic edit","patch":{"report_reference":"example-report"}}),
    );
    let rejected = send(&f, &premature).await;
    failure(&rejected, ToolErrorCode::Rejected);
    assert_eq!(send(&f, &premature).await, rejected);
    assert_eq!(
        send(
            &f,
            &request(&f, "unchanged-finding", "findings_read", json!({"id":"F1"}))
        )
        .await["record"]["revision"],
        2
    );
    f.close().await;
}

#[cfg(feature = "runbooks")]
#[tokio::test]
async fn runbook_internal_counter_failure_is_not_a_durable_input_rejection() {
    let f = Fixture::new().await;
    send(
        &f,
        &request(
            &f,
            "create-overflow-fixture",
            "runbook_create",
            json!({"title":"Example runbook","purpose":"Synthetic purpose","cells":[]}),
        ),
    )
    .await;
    f.state
        .db
        .with_connection(|db| {
            let record: String = db.query_row(
                "SELECT document_json FROM runbook_documents WHERE id='R1'",
                [],
                |r| r.get(0),
            )?;
            let mut record: Value = serde_json::from_str(&record)?;
            record["next_cell_number"] = json!(u64::MAX);
            db.execute(
                "UPDATE runbook_documents SET document_json=?1 WHERE id='R1'",
                [record.to_string()],
            )?;
            Ok(())
        })
        .unwrap();
    let edit = request(
        &f,
        "overflow-edit",
        "runbook_edit",
        json!({"id":"R1","expected_revision":1,"reason":"Synthetic edit","cells":[{"kind":"markdown","id":"","source":"Example"}]}),
    );
    assert!(
        f.client()
            .send(&edit, Duration::from_secs(5))
            .await
            .is_err()
    );
    f.state
        .db
        .with_connection(|db| {
            let receipts: i64 = db.query_row(
                "SELECT count(*) FROM swarm_receipts WHERE operation_id='overflow-edit'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(receipts, 0);
            let record: String = db.query_row(
                "SELECT document_json FROM runbook_documents WHERE id='R1'",
                [],
                |r| r.get(0),
            )?;
            let mut record: Value = serde_json::from_str(&record)?;
            assert_eq!(record["revision"], 1);
            record["next_cell_number"] = json!(1);
            db.execute(
                "UPDATE runbook_documents SET document_json=?1 WHERE id='R1'",
                [record.to_string()],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(send(&f, &edit).await["runbook"]["revision"], 2);
    f.close().await;
}
#[cfg(feature = "runbooks")]
#[tokio::test]
async fn runbook_rejections_distinguish_validation_missing_records_and_conflicts() {
    let f = Fixture::new().await;
    for (id, args) in [
        (
            "wrong-runbook-type",
            json!({"title":[],"purpose":"Example","cells":[]}),
        ),
        (
            "invalid-runbook-title",
            json!({"title":"","purpose":"Example","cells":[]}),
        ),
    ] {
        failure(
            &send(&f, &request(&f, id, "runbook_create", args)).await,
            ToolErrorCode::InvalidArguments,
        );
    }
    failure(
        &send(
            &f,
            &request(&f, "missing-runbook", "runbook_get", json!({"id":"R999"})),
        )
        .await,
        ToolErrorCode::NotFound,
    );
    let created = send(
        &f,
        &request(
            &f,
            "valid-runbook",
            "runbook_create",
            json!({"title":"Example runbook","purpose":"Synthetic purpose","cells":[]}),
        ),
    )
    .await;
    assert_eq!(created["runbook"]["id"], "R1");
    failure(
        &send(
            &f,
            &request(
                &f,
                "duplicate-runbook",
                "runbook_create",
                json!({"title":"Example runbook","purpose":"Synthetic purpose","cells":[]}),
            ),
        )
        .await,
        ToolErrorCode::Conflict,
    );
    let stale = request(
        &f,
        "stale-runbook",
        "runbook_edit",
        json!({"id":"R1","expected_revision":99,"reason":"Synthetic edit","purpose":"Updated purpose"}),
    );
    let error = send(&f, &stale).await;
    failure(&error, ToolErrorCode::Conflict);
    assert_eq!(send(&f, &stale).await, error);
    f.close().await;
}
#[cfg(feature = "findings")]
#[tokio::test]
async fn failure_receipt_and_database_failures_remain_unacknowledged_until_recovery() {
    let f = Fixture::new().await;
    f.state.db.with_connection(|db|{db.execute_batch("CREATE TRIGGER error_receipt_full BEFORE INSERT ON swarm_receipts BEGIN SELECT RAISE(ABORT,'synthetic receipt full'); END;")?;Ok(())}).unwrap();
    let bad = request(&f, "durable-error", "findings_create", json!({}));
    assert!(f.client().send(&bad, Duration::from_secs(5)).await.is_err());
    f.state
        .db
        .with_connection(|db| {
            let receipts: i64 = db.query_row(
                "SELECT count(*) FROM swarm_receipts WHERE operation_id='durable-error'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(receipts, 0);
            db.execute_batch("DROP TRIGGER error_receipt_full;")?;
            Ok(())
        })
        .unwrap();
    f.lose_response.store(true, Ordering::SeqCst);
    assert!(f.client().send(&bad, Duration::from_secs(5)).await.is_err());
    failure(&send(&f, &bad).await, ToolErrorCode::InvalidArguments);
    f.state.db.with_connection(|db|{db.execute_batch("CREATE TRIGGER record_full BEFORE INSERT ON finding_revisions BEGIN SELECT RAISE(ABORT,'synthetic record full'); END;")?;Ok(())}).unwrap();
    let good = request(
        &f,
        "database-retry",
        "findings_create",
        json!({"title":"Example","summary":"Synthetic summary","hypothesis":"Synthetic hypothesis"}),
    );
    assert!(
        f.client()
            .send(&good, Duration::from_secs(5))
            .await
            .is_err()
    );
    f.state
        .db
        .with_connection(|db| {
            let records: i64 = db.query_row("SELECT count(*) FROM findings", [], |r| r.get(0))?;
            assert_eq!(records, 0);
            let receipts: i64 = db.query_row(
                "SELECT count(*) FROM swarm_receipts WHERE operation_id='database-retry'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(receipts, 0);
            db.execute_batch("DROP TRIGGER record_full;")?;
            Ok(())
        })
        .unwrap();
    assert_eq!(send(&f, &good).await["record"]["id"], "F1");
    f.state
        .db
        .with_connection(|db| {
            db.execute("UPDATE findings SET record_json='invalid'", [])?;
            Ok(())
        })
        .unwrap();
    let corrupt = request(&f, "corrupt-record", "findings_query", json!({}));
    assert!(
        f.client()
            .send(&corrupt, Duration::from_secs(5))
            .await
            .is_err()
    );
    f.close().await;
}
#[tokio::test]
async fn known_rejection_rolls_back_partial_writes_and_rechecks_receipts_and_authority() {
    let f = Fixture::new().await;
    let tool = "swarm_list_subagents";
    let r = request(&f, "rollback-example", tool, json!({}));
    f.state
        .db
        .with_connection(|db| {
            db.execute_batch("CREATE TABLE synthetic_mutation(value TEXT);")?;
            Ok(())
        })
        .unwrap();
    let result = f.store.dispatch_mutation(
        f.credential.bearer(),
        WORKER_AUDIENCE,
        &r,
        unix_ms(),
        |tx, _| {
            tx.execute("INSERT INTO synthetic_mutation VALUES ('partial')", [])?;
            Err(ToolError::invalid(format!(
                "Synthetic rejection host-control-example {}",
                f.credential.bearer()
            ))
            .into())
        },
    );
    let ack = super::super::tool_errors::acknowledge(
        &f.state,
        &f.store,
        f.credential.bearer(),
        &r,
        result,
    )
    .unwrap();
    failure(&ack.result, ToolErrorCode::InvalidArguments);
    assert!(!ack.result.to_string().contains("host-control-example"));
    assert!(!ack.result.to_string().contains(f.credential.bearer()));
    for (id, secret) in [
        ("bounded-host-error", "host-control-example"),
        ("bounded-bearer-error", f.credential.bearer()),
    ] {
        let prefix = "é".repeat(1018);
        let ack = super::super::tool_errors::acknowledge(
            &f.state,
            &f.store,
            f.credential.bearer(),
            &request(&f, id, tool, json!({})),
            Err(ToolError::invalid(format!("{prefix}{secret}{}", "é".repeat(100))).into()),
        )
        .unwrap();
        failure(&ack.result, ToolErrorCode::InvalidArguments);
        let output = ack.result["output"].as_str().unwrap();
        assert!(output.len() <= beale_contracts::tool_error::MAX_TOOL_ERROR_BYTES);
        assert!(
            output
                .strip_prefix(&prefix)
                .unwrap()
                .starts_with("[redacted")
        );
    }
    f.state
        .db
        .with_connection(|db| {
            let count: i64 =
                db.query_row("SELECT count(*) FROM synthetic_mutation", [], |r| r.get(0))?;
            assert_eq!(count, 0);
            Ok(())
        })
        .unwrap();
    let success = request(&f, "accepted-wins", tool, json!({}));
    let accepted = f
        .store
        .dispatch_mutation(
            f.credential.bearer(),
            WORKER_AUDIENCE,
            &success,
            unix_ms(),
            |_, _| Ok(json!({"accepted":true})),
        )
        .unwrap();
    let replay = super::super::tool_errors::acknowledge(
        &f.state,
        &f.store,
        f.credential.bearer(),
        &success,
        Err(ToolError::conflict("Late rejection").into()),
    )
    .unwrap();
    assert_eq!(replay, accepted);
    f.store
        .fence_attempt("scan-example", &f.attempt.id, true)
        .unwrap();
    assert!(
        super::super::tool_errors::acknowledge(
            &f.state,
            &f.store,
            f.credential.bearer(),
            &request(&f, "revoked-error", tool, json!({})),
            Err(ToolError::invalid("Synthetic rejection").into())
        )
        .is_err()
    );
    f.close().await;
}

#[cfg(any(feature = "findings", feature = "runbooks"))]
struct RejectCheckpoint {
    db: HostDatabase,
    fired: AtomicBool,
    preparation: bool,
}
#[cfg(any(feature = "findings", feature = "runbooks"))]
impl beale_storage::worker_spool::SpoolFaults for RejectCheckpoint {
    fn before_commit(&self, write: beale_storage::worker_spool::SpoolWrite) -> anyhow::Result<()> {
        if write == beale_storage::worker_spool::SpoolWrite::Checkpoint
            && !self.fired.load(Ordering::SeqCst)
        {
            let query = if self.preparation {
                "SELECT EXISTS(SELECT 1 FROM swarm_receipts WHERE json_extract(result,'$.result.status')='rejected')"
            } else {
                "SELECT EXISTS(SELECT 1 FROM swarm_receipts WHERE json_extract(result,'$.error.code')='invalid_arguments')"
            };
            let rejected = self
                .db
                .with_connection(|db| Ok(db.query_row(query, [], |r| r.get::<_, bool>(0))?))?;
            if rejected && !self.fired.swap(true, Ordering::SeqCst) {
                anyhow::bail!("synthetic rejection checkpoint failure");
            }
        }
        Ok(())
    }
}
#[cfg(feature = "findings")]
#[tokio::test]
async fn worker_corrects_a_rejected_call_after_checkpoint_recovery() {
    let f = Fixture::new().await;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = calls.clone();
    let app=axum::Router::new().route("/v1/chat/completions",axum::routing::post(move |axum::Json(body):axum::Json<Value>| {
        let count=count.clone();async move {
            let delta=match count.fetch_add(1,Ordering::SeqCst) {
                0=>json!({"tool_calls":[{"index":0,"id":"invalid-create","type":"function","function":{"name":"findings_create","arguments":"{}"}}]}),
                1=>{
                    let feedback=body["messages"].as_array().unwrap().iter().find(|m|m["tool_call_id"]=="invalid-create").unwrap();assert!(feedback["content"].as_str().unwrap().contains("is required"));
                    json!({"tool_calls":[{"index":0,"id":"corrected-create","type":"function","function":{"name":"findings_create","arguments":json!({"title":"Example","summary":"Synthetic summary","hypothesis":"Synthetic hypothesis"}).to_string()}}]})
                },
                2=>json!({"content":"Synthetic corrected research complete"}),
                _=>panic!("unexpected repeated inference"),
            };
            let chunk=json!({"choices":[{"delta":delta}]});([( "content-type","text/event-stream")],format!("data: {chunk}\n\ndata: [DONE]\n\n"))
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
    let workspace = f.root.join("worker-workspace");
    std::fs::create_dir(&workspace).unwrap();
    let launch = || WorkerLaunch {
        version: 1,
        identity: f.store.worker_bootstrap(&f.attempt.id).unwrap().identity,
        leader: f.url.clone(),
        bearer: f.credential.bearer().into(),
        workspace: workspace.clone(),
        protected_root: f.root.clone(),
        recovery: f.root.join("worker-recovery"),
    };
    let faults = Arc::new(RejectCheckpoint {
        db: f.state.db.clone(),
        fired: AtomicBool::new(false),
        preparation: false,
    });
    assert!(
        super::super::worker::run_with_faults(&mut launch(), Some(faults))
            .await
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    run_assigned(launch()).await.unwrap();
    run_assigned(launch()).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    f.state
        .db
        .with_connection(|db| {
            let count: i64 = db.query_row("SELECT count(*) FROM findings", [], |r| r.get(0))?;
            assert_eq!(count, 1);
            let id: String = db.query_row("SELECT id FROM findings", [], |r| r.get(0))?;
            assert_eq!(id, "F1");
            Ok(())
        })
        .unwrap();
    server.abort();
    let _ = server.await;
    f.close().await;
}

#[cfg(any(feature = "findings", feature = "runbooks"))]
#[tokio::test]
async fn worker_replays_preparation_rejections_after_checkpoint_recovery() {
    let cases = [
        #[cfg(feature = "findings")]
        (
            "findings_verify",
            json!({"id":"F999","expected_revision":1,"command":"echo synthetic-execution-must-not-start","oracle":{"kind":"exit_code","value":0},"timeout_seconds":1}),
        ),
        #[cfg(feature = "runbooks")]
        ("runbook_run", json!({"id":"R999"})),
    ];
    for (tool, arguments) in cases {
        let f = Fixture::new().await;
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = calls.clone();
        let app=axum::Router::new().route("/v1/chat/completions",axum::routing::post(move |axum::Json(body):axum::Json<Value>| {
        let count=count.clone(); let arguments=arguments.clone(); async move {
            let delta=match count.fetch_add(1,Ordering::SeqCst) {
                0=>json!({"tool_calls":[{"index":0,"id":"missing-record","type":"function","function":{"name":tool,"arguments":arguments.to_string()}}]}),
                1=>{
                    let feedback=body["messages"].as_array().unwrap().iter().find(|m|m["tool_call_id"]=="missing-record").unwrap();
                    assert!(!feedback["content"].as_str().unwrap().is_empty());
                    json!({"content":"Synthetic missing record acknowledged"})
                },
                _=>panic!("unexpected repeated inference"),
            };
            let chunk=json!({"choices":[{"delta":delta}]});([( "content-type","text/event-stream")],format!("data: {chunk}\n\ndata: [DONE]\n\n"))
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
        let workspace = f.root.join("worker-workspace");
        std::fs::create_dir(&workspace).unwrap();
        let launch = || WorkerLaunch {
            version: 1,
            identity: f.store.worker_bootstrap(&f.attempt.id).unwrap().identity,
            leader: f.url.clone(),
            bearer: f.credential.bearer().into(),
            workspace: workspace.clone(),
            protected_root: f.root.clone(),
            recovery: f.root.join("worker-recovery"),
        };
        let faults = Arc::new(RejectCheckpoint {
            db: f.state.db.clone(),
            fired: AtomicBool::new(false),
            preparation: true,
        });
        assert!(
            super::super::worker::run_with_faults(&mut launch(), Some(faults))
                .await
                .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        run_assigned(launch()).await.unwrap();
        run_assigned(launch()).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        f.state
        .db
        .with_connection(|db| {
            let rejected: i64 = db.query_row("SELECT count(*) FROM swarm_receipts WHERE json_extract(result,'$.result.status')='rejected' AND json_extract(result,'$.result.error.code')='not_found'", [], |r|r.get(0))?;
            assert_eq!(rejected,1);
            #[cfg(feature = "findings")]
            assert_eq!(db.query_row("SELECT count(*) FROM finding_worker_verifications",[],|r|r.get::<_,i64>(0))?,0);
            #[cfg(feature = "runbooks")]
            assert_eq!(db.query_row("SELECT count(*) FROM runbook_runs",[],|r|r.get::<_,i64>(0))?,0);
            Ok(())
        })
        .unwrap();
        server.abort();
        let _ = server.await;
        f.close().await;
    }
}
