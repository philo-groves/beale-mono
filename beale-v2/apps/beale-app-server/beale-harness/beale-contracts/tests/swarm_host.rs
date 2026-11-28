use beale_contracts::{swarm_host::*, swarm_worker::ExecutionIdentity};

fn request() -> HostExecutionRequest {
    HostExecutionRequest {
        version: 1,
        operation_id: "operation-example".into(),
        target: ExecutionTarget {
            host_id: "host-example".into(),
            leader_id: "leader-example".into(),
            configuration_id: "configuration-example".into(),
            configuration_revision: 1,
            instance_id: "instance-example".into(),
            identity: ExecutionIdentity {
                workspace_id: "workspace-example".into(),
                scan_id: "scan-example".into(),
                agent_id: "agent-example".into(),
                attempt_id: "attempt-example".into(),
                generation: 1,
            },
        },
        operation: HostExecutionOperation::Start {
            launch: HostWorkerLaunch {
                snapshot: None,
                leader_origin: "https://leader.example.ts.net".into(),
                worker_bearer: "c".repeat(64),
            },
        },
    }
}

#[test]
fn host_execution_contracts_reject_unversioned_unscoped_and_host_path_inputs() {
    let request = request();
    request.validate().unwrap();
    let value = serde_json::to_value(&request).unwrap();
    assert!(serde_json::from_value::<HostExecutionRequest>(value.clone()).unwrap() == request);
    for field in [
        "provider_key",
        "host_token",
        "workspace_path",
        "command",
        "arguments",
    ] {
        let mut changed = value.clone();
        changed["operation"]["launch"][field] = "synthetic-private-value".into();
        assert!(serde_json::from_value::<HostExecutionRequest>(changed).is_err());
    }
    let mut changed = request.clone();
    changed.version = 2;
    assert!(changed.validate().is_err());
    changed = request.clone();
    changed.target.identity.generation = 0;
    assert!(changed.validate().is_err());
    changed = request.clone();
    changed.target.configuration_revision = u64::MAX;
    assert!(changed.validate().is_err());
    changed = request.clone();
    changed.operation_id.clear();
    assert!(changed.validate().is_err());
    changed = request.clone();
    if let HostExecutionOperation::Start { launch } = &mut changed.operation {
        launch.worker_bearer = "invalid".into();
    }
    assert!(changed.validate().is_err());
    let reply = HostExecutionReply {
        version: 1,
        operation_id: request.operation_id.clone(),
        report: HostExecutionReport {
            target: request.target.clone(),
            phase: HostExecutionPhase::Running,
            revision: 1,
            stop_requested: false,
        },
    };
    reply.validate_for(&request).unwrap();
    let mut changed = reply.clone();
    changed.report.target.identity.generation += 1;
    assert!(changed.validate_for(&request).is_err());
    let mut changed = reply.clone();
    changed.report.stop_requested = true;
    assert!(changed.validate_for(&request).is_err());
    changed.report.phase = HostExecutionPhase::TerminationUnconfirmed;
    changed.validate_for(&request).unwrap();
    changed.report.phase = HostExecutionPhase::StopRequested;
    changed.report.stop_requested = false;
    assert!(changed.validate_for(&request).is_err());
    let mut changed = reply;
    changed.report.revision = 0;
    assert!(changed.validate_for(&request).is_err());
}

#[test]
fn host_origins_never_carry_credentials_paths_or_cleartext_remote_authority() {
    for origin in [
        "https://host.example.ts.net",
        "http://127.0.0.1:12345",
        "http://[::1]:12345",
        "http://localhost:12345/",
    ] {
        validate_origin(origin).unwrap();
    }
    for origin in [
        "http://192.0.2.1",
        "https://user:secret@host.example.ts.net",
        "https://host.example.ts.net/path",
        "https://host.example.ts.net?token=example",
        "https://host.example.ts.net#example",
        "file:///tmp/example",
        "http://127.0.0.1:0",
    ] {
        assert!(validate_origin(origin).is_err(), "{origin}");
    }
}

#[test]
fn container_workers_reach_a_loopback_leader_through_the_host_alias() {
    for (leader, expected) in [
        (
            "http://127.0.0.1:43123",
            "http://host.docker.internal:43123/",
        ),
        (
            "http://localhost:43123/",
            "http://host.docker.internal:43123/",
        ),
        ("http://[::1]:43123", "http://host.docker.internal:43123/"),
        // A paired Fleet origin is already reachable and stays unchanged.
        (
            "https://leader.example.ts.net",
            "https://leader.example.ts.net/",
        ),
    ] {
        let translated = container_leader_origin(leader).unwrap();
        assert_eq!(translated, expected);
        validate_origin(&translated).unwrap();
    }
    // Plain HTTP stays limited to host-local names.
    assert!(validate_origin("http://host.docker.internal.example.test:43123").is_err());
    assert!(validate_origin("http://example.test:43123").is_err());
    assert!(container_leader_origin("http://192.0.2.1:43123").is_err());
    assert_eq!(CONTAINER_HOST_ALIAS, "host.docker.internal");
}
