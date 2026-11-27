use beale_contracts::swarms::*;
use serde_json::json;

#[test]
fn assigned_runbook_requests_bound_output_and_bind_typed_replies() {
    use beale_contracts::{swarm_runbooks::*, swarm_worker::*};
    let outcome = CellOutcome {
        cell_id: "L1".into(),
        success: true,
        exit_code: Some(0),
        stdout: "Example output".into(),
        stderr: String::new(),
    };
    let operation = Request::Cell {
        run_id: "run-example".into(),
        outcome: outcome.clone(),
    };
    let call = request(WorkerOperation::Runbook {
        request: operation.clone(),
    });
    assert_eq!(
        WorkerRequest::decode_json(&serde_json::to_vec(&call).unwrap()).unwrap(),
        call
    );
    let identity = ExecutionIdentity {
        workspace_id: call.workspace_id.clone(),
        scan_id: call.scan_id.clone(),
        agent_id: call.agent_id.clone(),
        attempt_id: call.attempt_id.clone(),
        generation: call.generation,
    };
    let reply = Reply {
        identity: identity.clone(),
        result: ResultKind::CellAccepted {
            run_id: "run-example".into(),
            cell_id: "L1".into(),
        },
    };
    assert!(reply.validate_for(&identity, &operation).is_ok());
    let mut foreign = identity.clone();
    foreign.generation += 1;
    assert!(reply.validate_for(&foreign, &operation).is_err());
    assert!(
        reply
            .validate_for(
                &identity,
                &Request::Finish {
                    run_id: "run-example".into()
                }
            )
            .is_err()
    );
    for change in 0..4 {
        let mut bad = outcome.clone();
        match change {
            0 => bad.stdout = "x".repeat(MAX_CELL_OUTPUT_BYTES + 1),
            1 => bad.cell_id.clear(),
            2 => bad.success = false,
            _ => bad.exit_code = None,
        }
        assert!(
            request(WorkerOperation::Runbook {
                request: Request::Cell {
                    run_id: "run-example".into(),
                    outcome: bad
                }
            })
            .validate()
            .is_err()
        );
    }
    let mut bad = serde_json::to_value(&call).unwrap();
    bad["operation"]["request"]["outcome"]["arbitrary"] = json!(true);
    assert!(WorkerRequest::decode_json(&serde_json::to_vec(&bad).unwrap()).is_err());
    assert!(
        Request::Prepare {
            runbook_id: String::new()
        }
        .validate()
        .is_err()
    );
}

#[test]
fn worker_review_replies_require_an_explicit_boolean_decision() {
    use beale_contracts::swarm_worker::*;
    let request = WorkerRequest {
        version: VERSION,
        workspace_id: "workspace-example".into(),
        scan_id: "scan-example".into(),
        agent_id: "agent-example".into(),
        attempt_id: "attempt-example".into(),
        generation: 1,
        operation_id: "review-example".into(),
        operation: WorkerOperation::Review {
            call: ToolInvocation {
                id: "call-example".into(),
                name: "read".into(),
                arguments: json!({"path":"example.txt"}),
            },
        },
    };
    let reply = |result| WorkerReply::Acknowledged {
        acknowledgement: MutationAcknowledgement {
            version: VERSION,
            operation_id: request.operation_id.clone(),
            result,
        },
    };
    for approved in [false, true] {
        assert!(
            reply(serde_json::to_value(WorkerReviewResult { approved }).unwrap())
                .validate_for(&request)
                .is_ok()
        );
    }
    for invalid in [
        json!({}),
        json!({"approved":"false"}),
        json!({"approved":null}),
        json!({"approved":true,"extra":false}),
    ] {
        assert!(reply(invalid).validate_for(&request).is_err());
    }
}

#[test]
fn worker_shell_mapping_preserves_saved_availability_without_cross_shell_authority() {
    use beale_contracts::swarm_worker::*;
    let saved = vec!["read".into(), "powershell".into(), "bash".into()];
    assert_eq!(WorkerShell::Bash.map_tools(&saved), vec!["read", "bash"]);
    assert_eq!(
        WorkerShell::Powershell.map_tools(&saved),
        vec!["read", "powershell"]
    );
    assert_eq!(WorkerShell::Bash.map_tools(&["read".into()]), vec!["read"]);
    assert!(serde_json::from_value::<WorkerShell>(json!("arbitrary_command")).is_err());
    let bootstrap = WorkerBootstrap {
        identity: ExecutionIdentity {
            workspace_id: "workspace-example".into(),
            scan_id: "scan-example".into(),
            agent_id: "agent-example".into(),
            attempt_id: "attempt-example".into(),
            generation: 1,
        },
        shell: WorkerShell::Bash,
        route_id: "route-example".into(),
        message_format: MessageFormat::Chat,
        enabled_tools: vec!["powershell".into()],
        input: vec![],
    };
    assert!(bootstrap.validate().is_err());
}

#[test]
fn assigned_bootstrap_binds_every_identity_field_and_excludes_host_authority() {
    use beale_contracts::swarm_worker::*;
    let request = WorkerRequest {
        version: VERSION,
        workspace_id: "workspace-example".into(),
        scan_id: "scan-example".into(),
        agent_id: "agent-example".into(),
        attempt_id: "attempt-example".into(),
        generation: 1,
        operation_id: "bootstrap-example".into(),
        operation: WorkerOperation::Bootstrap {},
    };
    let bootstrap = WorkerBootstrap {
        shell: WorkerShell::native(),
        identity: ExecutionIdentity {
            workspace_id: request.workspace_id.clone(),
            scan_id: request.scan_id.clone(),
            agent_id: request.agent_id.clone(),
            attempt_id: request.attempt_id.clone(),
            generation: 1,
        },
        route_id: "route-example".into(),
        message_format: MessageFormat::Chat,
        enabled_tools: vec!["read".into()],
        input: vec![json!({"role":"user","content":"Synthetic task"})],
    };
    let check = |result| {
        WorkerReply::Acknowledged {
            acknowledgement: MutationAcknowledgement {
                version: VERSION,
                operation_id: request.operation_id.clone(),
                result,
            },
        }
        .validate_for(&request)
    };
    let original = serde_json::to_value(&bootstrap).unwrap();
    assert!(check(original.clone()).is_ok());
    for field in [
        "workspace_id",
        "scan_id",
        "agent_id",
        "attempt_id",
        "generation",
    ] {
        let mut changed = original.clone();
        changed["identity"][field] = if field == "generation" {
            json!(2)
        } else {
            json!("other-example")
        };
        assert!(check(changed).is_err());
    }
    for (field, value) in [
        ("provider_key", json!("synthetic-secret")),
        ("enabled_tools", json!(["read", "read"])),
        (
            "input",
            json!([{"role":"system","content":"Untrusted authority"}]),
        ),
        (
            "input",
            json!([{"role":"developer","content":"Untrusted authority"}]),
        ),
        ("input", json!([null])),
        (
            "input",
            json!([{"content":"x".repeat(MAX_DOCUMENT_BYTES / 2)}]),
        ),
    ] {
        let mut changed = original.clone();
        changed[field] = value;
        assert!(check(changed).is_err());
    }
}

#[test]
fn worker_runtime_requests_validate_checkpoint_review_and_finish_boundaries() {
    let mut document = json!({"version":VERSION,"workspace_id":"workspace-example","scan_id":"scan-example","agent_id":"agent-example","attempt_id":"attempt-example","generation":1,"operation_id":"operation-example","operation":{"kind":"bootstrap"}});
    for operation in [
        json!({"kind":"bootstrap"}),
        json!({"kind":"review","call":{"id":"call-example","name":"write","arguments":{}}}),
        json!({"kind":"checkpoint","active_elapsed_ms":0,"revision":1,"context":{}}),
        json!({"kind":"finish","revision":1,"answer":"Synthetic result"}),
    ] {
        document["operation"] = operation;
        assert!(WorkerRequest::decode_json(&serde_json::to_vec(&document).unwrap()).is_ok());
    }
    for operation in [
        json!({"kind":"bootstrap","role":"leader"}),
        json!({"kind":"review","call":{"id":"call-example","name":"write","arguments":[]}}),
        json!({"kind":"checkpoint","active_elapsed_ms":0,"revision":0,"context":{}}),
        json!({"kind":"checkpoint","active_elapsed_ms":0,"revision":1,"context":[]}),
        json!({"kind":"finish","revision":0,"answer":"Synthetic result"}),
        json!({"kind":"finish","revision":1,"answer":"x".repeat(262145)}),
    ] {
        document["operation"] = operation;
        assert!(WorkerRequest::decode_json(&serde_json::to_vec(&document).unwrap()).is_err());
    }
}

#[test]
fn inference_acknowledgements_reject_untyped_results_and_invalid_provider_data() {
    use beale_contracts::swarm_worker::*;
    let request = WorkerRequest {
        version: VERSION,
        workspace_id: "workspace-example".into(),
        scan_id: "scan-example".into(),
        agent_id: "agent-example".into(),
        attempt_id: "attempt-example".into(),
        generation: 1,
        operation_id: "inference-example".into(),
        operation: WorkerOperation::Infer {
            route_id: "route-example".into(),
            messages: json!([]),
        },
    };
    let check = |result: serde_json::Value| {
        WorkerReply::Acknowledged {
            acknowledgement: MutationAcknowledgement {
                version: VERSION,
                operation_id: request.operation_id.clone(),
                result,
            },
        }
        .validate_for(&request)
    };
    assert!(check(json!({"status":"unavailable","retryable":true})).is_ok());
    assert!(
        check(json!({"status":"unavailable","retryable":true,"provider_secret":"synthetic"}))
            .is_err()
    );
    assert!(check(json!({"text":"untyped"})).is_err());
    let mut result = json!({"status":"completed","text":"Synthetic answer","calls":[{"id":"call-example","name":"read","arguments":"{invalid JSON"}],"reasoning_summaries":[],"continuation":[],"usage":{"input_tokens":100,"output_tokens":20,"cache_read_tokens":90},"response_id":"response-example"});
    assert!(
        check(result.clone()).is_ok(),
        "malformed call arguments are returned for ordinary tool error feedback"
    );
    let duplicate = result["calls"][0].clone();
    result["calls"].as_array_mut().unwrap().push(duplicate);
    assert!(check(result.clone()).is_err());
    result["calls"] = json!([]);
    result["usage"]["cache_read_tokens"] = json!(101);
    assert!(check(result.clone()).is_err());
    result["usage"] = json!(null);
    result["text"] = json!("x".repeat(MAX_DOCUMENT_BYTES / 2));
    assert!(check(result).is_err());
}

#[test]
fn tool_result_null_is_distinguished_by_completion_state_on_round_trip() {
    for state in [
        ToolOperationState::NotStarted,
        ToolOperationState::Running,
        ToolOperationState::Completed,
        ToolOperationState::Canceled,
        ToolOperationState::OutcomeUnknown,
    ] {
        let operation = ToolOperation {
            id: "operation-example".into(),
            agent_id: "agent-example".into(),
            attempt_id: "attempt-example".into(),
            request: json!({}),
            state,
            result: json!(null),
            revision: 1,
        };
        let encoded = serde_json::to_vec(&operation).unwrap();
        assert_eq!(
            serde_json::from_slice::<ToolOperation>(&encoded).unwrap(),
            operation
        );
    }
}

fn request(operation: WorkerOperation) -> WorkerRequest {
    WorkerRequest {
        version: VERSION,
        workspace_id: "workspace-example".into(),
        scan_id: "scan-example".into(),
        agent_id: "agent-example".into(),
        attempt_id: "attempt-example".into(),
        generation: 1,
        operation_id: "operation-example".into(),
        operation,
    }
}

#[test]
fn artifact_wire_contract_rejects_authority_injection_and_mismatched_replies() {
    use beale_contracts::{swarm_files::*, swarm_worker::*};
    let upload = ArtifactUpload {
        id: "artifact-example".into(),
        source_path: "proof/example.txt".into(),
        sha256: "a".repeat(64),
        size: 3,
        executable: Some(false),
        revision: 1,
        previous: None,
    };
    let begin = request(WorkerOperation::ArtifactBegin {
        artifact: upload.clone(),
    });
    let seal = request(WorkerOperation::ArtifactSeal {
        transfer_id: upload.id.clone(),
    });
    let read = request(WorkerOperation::ArtifactRead {
        artifact_id: upload.id.clone(),
        offset: 1,
        length: 2,
    });
    let chunk = request(WorkerOperation::ArtifactChunk {
        transfer_id: upload.id.clone(),
        offset: 0,
        bytes: vec![1, 2, 3],
        sha256: upload.sha256.clone(),
    });
    for value in [&begin, &seal, &read, &chunk] {
        assert_eq!(
            WorkerRequest::decode_json(&serde_json::to_vec(value).unwrap()).unwrap(),
            *value
        );
    }
    for field in [
        "workspace",
        "scan",
        "producer",
        "assignment",
        "attempt",
        "generation",
        "operation",
    ] {
        let mut value = serde_json::to_value(&begin).unwrap();
        value["operation"]["artifact"][field] = json!("foreign-example");
        assert!(WorkerRequest::decode_json(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    for (field, value) in [
        ("source_path", json!("../escape")),
        ("sha256", json!("not-a-digest")),
        ("revision", json!(0)),
        ("size", json!(u64::MAX)),
    ] {
        let mut document = serde_json::to_value(&begin).unwrap();
        document["operation"]["artifact"][field] = value;
        assert!(WorkerRequest::decode_json(&serde_json::to_vec(&document).unwrap()).is_err());
    }
    for bytes in [vec![], vec![1; 192 * 1024 + 1]] {
        assert!(
            request(WorkerOperation::ArtifactChunk {
                transfer_id: upload.id.clone(),
                offset: 0,
                bytes,
                sha256: upload.sha256.clone()
            })
            .validate()
            .is_err()
        );
    }
    for length in [0, 192 * 1024 + 1] {
        assert!(
            request(WorkerOperation::ArtifactRead {
                artifact_id: upload.id.clone(),
                offset: 0,
                length
            })
            .validate()
            .is_err()
        );
    }
    let check = |request: &WorkerRequest, result| {
        WorkerReply::Acknowledged {
            acknowledgement: MutationAcknowledgement {
                version: VERSION,
                operation_id: request.operation_id.clone(),
                result,
            },
        }
        .validate_for(request)
    };
    assert!(check(&begin, json!({"next_offset":3})).is_ok());
    assert!(check(&begin, json!({"next_offset":4})).is_err());
    let artifact = Artifact {
        id: upload.id,
        operation: begin.operation_id.clone(),
        workspace: begin.workspace_id.clone(),
        scan: begin.scan_id.clone(),
        producer: begin.agent_id.clone(),
        assignment: None,
        attempt: begin.attempt_id.clone(),
        generation: begin.generation,
        revision: 1,
        source_path: upload.source_path,
        sha256: upload.sha256,
        size: 3,
        executable: Some(false),
        previous: None,
    };
    assert!(check(&seal, serde_json::to_value(&artifact).unwrap()).is_ok());
    for field in ["id", "workspace", "scan", "producer", "attempt"] {
        let mut value = serde_json::to_value(&artifact).unwrap();
        value[field] = json!("foreign-example");
        assert!(check(&seal, value).is_err());
    }
    let reply = json!({"artifact":artifact,"offset":1,"bytes":[2,3],"sha256":"b".repeat(64)});
    assert!(check(&read, reply.clone()).is_ok());
    for (field, value) in [
        ("offset", json!(0)),
        ("bytes", json!([2])),
        ("sha256", json!("invalid")),
    ] {
        let mut changed = reply.clone();
        changed[field] = value;
        assert!(check(&read, changed).is_err());
    }
    let mut foreign = reply;
    foreign["artifact"]["workspace"] = json!("foreign-example");
    assert!(check(&read, foreign).is_err());
}

#[test]
fn worker_operations_round_trip_through_the_versioned_boundary() {
    let operations = vec![
        WorkerOperation::Infer {
            route_id: "route-example".into(),
            messages: json!([]),
        },
        WorkerOperation::Canonical {
            tool: "findings_create".into(),
            arguments: json!({}),
        },
        WorkerOperation::MessageParent {
            message: "Synthetic status".into(),
        },
        WorkerOperation::MessageChild {
            child_id: "child-example".into(),
            message: "Synthetic task".into(),
        },
        WorkerOperation::StopChild {
            child_id: "child-example".into(),
        },
        WorkerOperation::Spawn {
            profile: ProfileRef {
                id: "profile-example".into(),
                revision: 1,
            },
            task: Task {
                question: "Synthetic question".into(),
                completion_criteria: vec!["Synthetic criterion".into()],
                evidence_refs: vec![],
            },
        },
        WorkerOperation::Heartbeat {},
        WorkerOperation::Event {
            event_id: "event-example".into(),
            payload: json!({}),
        },
        WorkerOperation::ArtifactChunk {
            transfer_id: "transfer-example".into(),
            offset: 0,
            bytes: vec![0],
            sha256: "0".repeat(64),
        },
    ];
    for operation in operations {
        let req = request(operation);
        assert_eq!(
            WorkerRequest::decode_json(&serde_json::to_vec(&req).unwrap()).unwrap(),
            req
        );
    }
}

#[test]
fn decoder_rejects_version_changes_authority_fields_bad_shapes_and_oversized_documents() {
    let req = request(WorkerOperation::Heartbeat {});
    for (key, value) in [
        ("version", json!(999)),
        ("role", json!("leader")),
        ("authorized_scope", json!("expanded")),
        ("generation", json!(0)),
    ] {
        let mut v = serde_json::to_value(&req).unwrap();
        v[key] = value;
        assert!(WorkerRequest::decode_json(&serde_json::to_vec(&v).unwrap()).is_err());
    }
    for operation in [
        json!({"kind":"host_admin"}),
        json!({"kind":"heartbeat","role":"leader"}),
        json!({"kind":"canonical","tool":"findings_create","arguments":[]}),
    ] {
        let mut v = serde_json::to_value(&req).unwrap();
        v["operation"] = operation;
        assert!(WorkerRequest::decode_json(&serde_json::to_vec(&v).unwrap()).is_err());
    }
    assert!(WorkerRequest::decode_json(&vec![b' '; MAX_DOCUMENT_BYTES + 1]).is_err());
    let req = request(WorkerOperation::ArtifactChunk {
        transfer_id: "transfer-example".into(),
        offset: u64::MAX,
        bytes: vec![0],
        sha256: "0".repeat(64),
    });
    assert!(req.validate().is_err());
}

#[test]
fn local_scan_contract_rejects_selected_destinations_even_when_saved_as_a_draft() {
    let mut config = ScanConfig {
        version: VERSION,
        scan_id: "scan-example".into(),
        workspace_id: "workspace-example".into(),
        mode: ScanMode::Local {},
        authorized_scope: "Synthetic scope".into(),
        enabled_tools: vec![],
        provider_routes: vec![],
        destinations: vec![],
        pacing: vec![],
        retention: RetentionPolicy::default(),
    };
    assert_eq!(
        ScanConfig::decode_json(&serde_json::to_vec(&config).unwrap()).unwrap(),
        config
    );
    config.destinations.push(Destination {
        host_id: "host-example".into(),
        configuration_id: "config-example".into(),
    });
    assert!(ScanConfig::decode_json(&serde_json::to_vec(&config).unwrap()).is_err());
}

#[test]
fn worker_checkpoint_requires_a_bounded_active_duration() {
    use beale_contracts::swarm_worker::MAX_ACTIVE_ELAPSED_MS;
    let good = request(WorkerOperation::Checkpoint {
        revision: 1,
        active_elapsed_ms: MAX_ACTIVE_ELAPSED_MS,
        context: json!({}),
    });
    assert_eq!(
        WorkerRequest::decode_json(&serde_json::to_vec(&good).unwrap()).unwrap(),
        good
    );
    for value in [
        json!(-1),
        json!(MAX_ACTIVE_ELAPSED_MS + 1),
        json!(1.5),
        json!("1"),
        json!(null),
    ] {
        let mut bad = serde_json::to_value(&good).unwrap();
        bad["operation"]["active_elapsed_ms"] = value;
        assert!(WorkerRequest::decode_json(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
    let mut missing = serde_json::to_value(&good).unwrap();
    missing["operation"]
        .as_object_mut()
        .unwrap()
        .remove("active_elapsed_ms");
    assert!(WorkerRequest::decode_json(&serde_json::to_vec(&missing).unwrap()).is_err());
}
