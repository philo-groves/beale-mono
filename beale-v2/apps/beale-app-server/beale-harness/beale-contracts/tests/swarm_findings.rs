use beale_contracts::{
    findings::Oracle,
    swarm_findings::{Command, FinishedRun, Observation, Reply, Request, ResultKind},
    swarm_worker::ExecutionIdentity,
};
use serde_json::{Value, json};
fn identity() -> ExecutionIdentity {
    ExecutionIdentity {
        workspace_id: "workspace-example".into(),
        scan_id: "scan-example".into(),
        agent_id: "agent-example".into(),
        attempt_id: "attempt-example".into(),
        generation: 1,
    }
}
fn command() -> Command {
    Command {
        id: "F1".into(),
        expected_revision: 1,
        command: "./example-proof".into(),
        oracle: Oracle::OutputContains {
            value: "example-observation".into(),
        },
        timeout_seconds: 120,
    }
}

#[test]
fn verifier_contract_rejects_forged_verdicts_and_invalid_execution_inputs() {
    let valid = command();
    valid.validate().unwrap();
    let mut default = serde_json::to_value(&valid).unwrap();
    default.as_object_mut().unwrap().remove("timeout_seconds");
    assert_eq!(serde_json::from_value::<Command>(default).unwrap(), valid);
    for (key, value) in [
        ("id", json!("../other")),
        ("expected_revision", json!(0)),
        ("expected_revision", json!(u64::MAX)),
        ("command", json!(" ")),
        ("command", json!("x".repeat(16_385))),
        ("timeout_seconds", json!(0)),
        ("timeout_seconds", json!(601)),
        (
            "oracle",
            json!({"kind":"stdout_contains","value":"example-proof"}),
        ),
        ("oracle", json!({"kind":"stdout_contains","value":" "})),
    ] {
        let mut value_command = serde_json::to_value(&valid).unwrap();
        value_command[key] = value;
        assert!(
            serde_json::from_value::<Command>(value_command)
                .unwrap()
                .validate()
                .is_err()
        );
    }
    let observation = Observation {
        identity: identity(),
        run_id: "run-example".into(),
        index: 0,
        exit_code: Some(0),
        stdout: "example-observation".into(),
        stderr: String::new(),
        duration_ms: 1,
        error: None,
    };
    observation.validate().unwrap();
    for field in ["verified", "satisfied", "output_sha256", "agent_role"] {
        let mut value = serde_json::to_value(&observation).unwrap();
        value[field] = json!(true);
        assert!(serde_json::from_value::<Observation>(value).is_err());
    }
    for (key, value) in [
        ("index", json!(2)),
        ("duration_ms", json!(-1)),
        ("error", json!("failure")),
        ("run_id", json!("")),
    ] {
        let mut value_observation = serde_json::to_value(&observation).unwrap();
        value_observation[key] = value;
        assert!(
            serde_json::from_value::<Observation>(value_observation)
                .unwrap()
                .validate()
                .is_err()
        );
    }
    let mut failure = observation;
    failure.exit_code = None;
    failure.stdout.clear();
    failure.error = Some("Synthetic failure".into());
    failure.validate().unwrap();
    let mut request = serde_json::to_value(Request::Prepare {
        tool_call_id: "call-example".into(),
        command: valid,
    })
    .unwrap();
    request["role"] = json!("leader");
    assert!(serde_json::from_value::<Request>(request).is_err());
}

#[test]
fn verifier_replies_are_bound_to_full_execution_and_request_phase() {
    let request = Request::Observe {
        run_id: "run-example".into(),
        index: 1,
        artifact_id: "artifact-example".into(),
    };
    let reply = Reply {
        identity: identity(),
        result: ResultKind::Observed {
            run_id: "run-example".into(),
            index: 1,
        },
    };
    reply.validate_for(&identity(), &request).unwrap();
    for field in [
        "workspace_id",
        "scan_id",
        "agent_id",
        "attempt_id",
        "generation",
    ] {
        let mut value = serde_json::to_value(&reply).unwrap();
        value["identity"][field] = if field == "generation" {
            json!(2)
        } else {
            json!("foreign-example")
        };
        assert!(
            serde_json::from_value::<Reply>(value)
                .unwrap()
                .validate_for(&identity(), &request)
                .is_err()
        );
    }
    for request in [
        Request::Observe {
            run_id: "other-run".into(),
            index: 1,
            artifact_id: "artifact-example".into(),
        },
        Request::Observe {
            run_id: "run-example".into(),
            index: 0,
            artifact_id: "artifact-example".into(),
        },
        Request::Finish {
            run_id: "run-example".into(),
        },
    ] {
        assert!(reply.validate_for(&identity(), &request).is_err());
    }
    let finished = Reply {
        identity: identity(),
        result: ResultKind::Finished {
            run: FinishedRun {
                run_id: "run-example".into(),
                recorded: false,
                verified: true,
                output: "invalid success".into(),
            },
        },
    };
    assert!(
        finished
            .validate_for(
                &identity(),
                &Request::Finish {
                    run_id: "run-example".into()
                }
            )
            .is_err()
    );
    let mut unknown = serde_json::to_value(reply).unwrap();
    unknown["result"]["untrusted"] = Value::Bool(true);
    assert!(serde_json::from_value::<Reply>(unknown).is_err());
}
