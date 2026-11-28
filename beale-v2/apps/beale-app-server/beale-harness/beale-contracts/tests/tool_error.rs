use beale_contracts::{swarm_worker::WorkerReply, swarms::*, tool_error::*};
use serde_json::json;

#[test]
fn mixed_operation_rejections_are_bound_to_preparation_and_execution_identity() {
    use beale_contracts::{
        swarm_findings as v, swarm_runbooks as r, swarm_worker::ExecutionIdentity,
    };
    let identity = ExecutionIdentity {
        workspace_id: "workspace-example".into(),
        scan_id: "scan-example".into(),
        agent_id: "agent-example".into(),
        attempt_id: "attempt-example".into(),
        generation: 1,
    };
    let runbook = r::Reply {
        identity: identity.clone(),
        result: r::ResultKind::Rejected {
            error: ToolError::missing("Example Runbook missing"),
        },
    };
    let prepare = r::Request::Prepare {
        runbook_id: "R1".into(),
    };
    let roundtrip: r::Reply =
        serde_json::from_value(serde_json::to_value(&runbook).unwrap()).unwrap();
    roundtrip.validate_for(&identity, &prepare).unwrap();
    assert!(
        runbook
            .validate_for(
                &identity,
                &r::Request::Finish {
                    run_id: "run-example".into()
                }
            )
            .is_err()
    );
    let mut foreign = identity.clone();
    foreign.generation += 1;
    assert!(runbook.validate_for(&foreign, &prepare).is_err());
    let verification = v::Reply {
        identity: identity.clone(),
        result: v::ResultKind::Rejected {
            error: ToolError::conflict("Example revision changed"),
        },
    };
    let prepare = v::Request::Prepare {
        tool_call_id: "call-example".into(),
        command: v::Command {
            id: "F1".into(),
            expected_revision: 1,
            command: "./example-proof".into(),
            oracle: beale_contracts::findings::Oracle::ExitCode { value: 0 },
            timeout_seconds: 10,
        },
    };
    let roundtrip: v::Reply =
        serde_json::from_value(serde_json::to_value(&verification).unwrap()).unwrap();
    roundtrip.validate_for(&identity, &prepare).unwrap();
    assert!(verification.validate_for(&foreign, &prepare).is_err());
    assert!(
        verification
            .validate_for(
                &identity,
                &v::Request::Observe {
                    run_id: "run-example".into(),
                    index: 0,
                    artifact_id: "artifact-example".into()
                }
            )
            .is_err()
    );
    for bad in [
        json!({"code":"conflict","message":""}),
        json!({"code":"conflict","message":"x".repeat(2049)}),
    ] {
        let mut value = serde_json::to_value(&verification).unwrap();
        value["result"]["error"] = bad;
        assert!(
            serde_json::from_value::<v::Reply>(value)
                .unwrap()
                .validate_for(&identity, &prepare)
                .is_err()
        );
    }
    let mut value = serde_json::to_value(&verification).unwrap();
    value["result"]["error"]["host_path"] = json!("synthetic");
    assert!(serde_json::from_value::<v::Reply>(value).is_err());
}
#[test]
fn typed_tool_rejections_round_trip_with_bounded_matching_feedback() {
    let request = WorkerRequest {
        version: 1,
        workspace_id: "workspace-example".into(),
        scan_id: "scan-example".into(),
        agent_id: "agent-example".into(),
        attempt_id: "attempt-example".into(),
        generation: 1,
        operation_id: "operation-example".into(),
        operation: WorkerOperation::Canonical {
            tool: "findings_create".into(),
            arguments: json!({}),
        },
    };
    for error in [
        ToolError::invalid("Example arguments"),
        ToolError::missing("Example record"),
        ToolError::conflict("Example revision"),
        ToolError::rejected("Example evidence"),
    ] {
        error.validate().unwrap();
        let feedback = error.feedback();
        let response = |result| WorkerReply::Acknowledged {
            acknowledgement: MutationAcknowledgement {
                version: 1,
                operation_id: request.operation_id.clone(),
                result,
            },
        };
        response(feedback.clone()).validate_for(&request).unwrap();
        let mut changed = feedback.clone();
        changed["output"] = json!("Other text");
        assert!(response(changed).validate_for(&request).is_err());
        let mut changed = feedback.clone();
        changed["is_error"] = json!(false);
        assert!(response(changed).validate_for(&request).is_err());
        let mut changed = feedback;
        changed["error"]["host_path"] = json!("synthetic");
        assert!(response(changed).validate_for(&request).is_err());
    }
    ToolError::invalid("é".repeat(MAX_TOOL_ERROR_BYTES / 2))
        .validate()
        .unwrap();
    assert!(
        ToolError::invalid("é".repeat(MAX_TOOL_ERROR_BYTES / 2 + 1))
            .validate()
            .is_err()
    );
    assert!(ToolError::invalid("").validate().is_err());
    assert!(
        serde_json::from_value::<ToolError>(json!({"code":"database_failure","message":"example"}))
            .is_err()
    );
    assert_eq!(
        arguments::<u32>(json!(-1)).unwrap_err().code,
        ToolErrorCode::InvalidArguments
    );
}

#[test]
fn artifact_reads_accept_typed_rejections_and_model_text_is_bounded() {
    let request = WorkerRequest {
        version: 1,
        workspace_id: "workspace-example".into(),
        scan_id: "scan-example".into(),
        agent_id: "agent-example".into(),
        attempt_id: "attempt-example".into(),
        generation: 1,
        operation_id: "operation-example".into(),
        operation: WorkerOperation::ArtifactRead {
            artifact_id: "artifact-missing-example".into(),
            offset: 0,
            length: 1024,
        },
    };
    let response = |result| WorkerReply::Acknowledged {
        acknowledgement: MutationAcknowledgement {
            version: 1,
            operation_id: request.operation_id.clone(),
            result,
        },
    };
    let error = ToolError::missing("artifact has not been accepted");
    response(error.feedback()).validate_for(&request).unwrap();
    let mut changed = error.feedback();
    changed["output"] = json!("Other text");
    assert!(response(changed).validate_for(&request).is_err());
    // Without typed feedback an artifact read still requires a matching chunk.
    assert!(
        response(json!({"output":"artifact has not been accepted","is_error":true}))
            .validate_for(&request)
            .is_err()
    );
    assert_eq!(
        error.model_text("Artifact operation"),
        "Artifact operation rejected (not_found): artifact has not been accepted"
    );
    let long = ToolError::invalid("é".repeat(MAX_TOOL_ERROR_BYTES));
    let text = long.model_text("Swarm request");
    let message = text
        .strip_prefix("Swarm request rejected (invalid_arguments): ")
        .unwrap();
    assert!(message.len() <= MAX_TOOL_ERROR_BYTES);
    assert!(message.chars().all(|c| c == 'é'));
    for code in [
        ToolErrorCode::InvalidArguments,
        ToolErrorCode::NotFound,
        ToolErrorCode::Conflict,
        ToolErrorCode::Rejected,
    ] {
        assert_eq!(serde_json::to_value(code).unwrap(), json!(code.as_str()));
    }
}
