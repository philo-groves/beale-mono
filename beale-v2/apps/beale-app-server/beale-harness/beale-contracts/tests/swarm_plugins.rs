use beale_contracts::{
    swarm_plugins::{Invocation, Outcome, Reply},
    swarm_worker::ExecutionIdentity,
};
use serde_json::json;

#[test]
fn plugin_contracts_do_not_accept_host_configuration_or_unbounded_arguments() {
    let invocation = Invocation {
        plugin: "example-plugin".into(),
        server: "main".into(),
        tool: "lookup".into(),
        arguments: json!({"query":"example"}),
    };
    invocation.validate().unwrap();
    for field in ["headers", "token", "env", "command", "workspace_id", "role"] {
        let mut value = serde_json::to_value(&invocation).unwrap();
        value[field] = json!("synthetic");
        assert!(serde_json::from_value::<Invocation>(value).is_err());
    }
    for bad in [json!(null), json!([]), json!("example")] {
        let mut changed = invocation.clone();
        changed.arguments = bad;
        assert!(changed.validate().is_err());
    }
    for name in [
        String::new(),
        " ".into(),
        "x".repeat(129),
        "invalid\nname".into(),
    ] {
        let mut changed = invocation.clone();
        changed.plugin = name;
        assert!(changed.validate().is_err());
    }
    let mut large = invocation;
    large.arguments = json!({"value":"x".repeat(524_288)});
    assert!(large.validate().is_err());
}

#[test]
fn plugin_results_bind_execution_and_keep_unknown_distinct_from_known_failure() {
    let identity = ExecutionIdentity {
        workspace_id: "workspace-example".into(),
        scan_id: "scan-example".into(),
        agent_id: "agent-example".into(),
        attempt_id: "attempt-example".into(),
        generation: 1,
    };
    for outcome in [
        Outcome::Completed {
            output: "null".into(),
            is_error: false,
            artifact_refs: vec![],
        },
        Outcome::Completed {
            output: "Synthetic tool error".into(),
            is_error: true,
            artifact_refs: vec!["artifact-example".into()],
        },
        Outcome::OutcomeUnknown {},
    ] {
        let reply = Reply {
            identity: identity.clone(),
            outcome,
        };
        reply.validate_for(&identity).unwrap();
        let request = beale_contracts::swarms::WorkerRequest {
            version: 1,
            workspace_id: identity.workspace_id.clone(),
            scan_id: identity.scan_id.clone(),
            agent_id: identity.agent_id.clone(),
            attempt_id: identity.attempt_id.clone(),
            generation: identity.generation,
            operation_id: "call-example".into(),
            operation: beale_contracts::swarms::WorkerOperation::Plugin {
                invocation: Invocation {
                    plugin: "example-plugin".into(),
                    server: "main".into(),
                    tool: "lookup".into(),
                    arguments: json!({}),
                },
            },
        };
        assert_eq!(
            beale_contracts::swarms::WorkerRequest::decode_json(
                &serde_json::to_vec(&request).unwrap()
            )
            .unwrap(),
            request
        );
        beale_contracts::swarm_worker::WorkerReply::Acknowledged {
            acknowledgement: beale_contracts::swarms::MutationAcknowledgement {
                version: 1,
                operation_id: request.operation_id.clone(),
                result: serde_json::to_value(&reply).unwrap(),
            },
        }
        .validate_for(&request)
        .unwrap();
        let encoded = serde_json::to_value(&reply).unwrap();
        assert_eq!(
            serde_json::from_value::<Reply>(encoded.clone()).unwrap(),
            reply
        );
        for field in [
            "workspace_id",
            "scan_id",
            "agent_id",
            "attempt_id",
            "generation",
        ] {
            let mut value = encoded.clone();
            value["identity"][field] = if field == "generation" {
                json!(2)
            } else {
                json!("different-example")
            };
            assert!(
                serde_json::from_value::<Reply>(value)
                    .unwrap()
                    .validate_for(&identity)
                    .is_err()
            );
        }
    }
    assert!(
        Outcome::Completed {
            output: "x".repeat(262_145),
            is_error: false,
            artifact_refs: vec![]
        }
        .validate()
        .is_err()
    );
    assert!(
        Outcome::Completed {
            output: "\0".repeat(100_000),
            is_error: false,
            artifact_refs: vec![]
        }
        .validate()
        .is_err()
    );
    assert!(
        Outcome::Completed {
            output: "example".into(),
            is_error: false,
            artifact_refs: vec!["artifact-example".into(), "artifact-example".into()]
        }
        .validate()
        .is_err()
    );
    assert!(
        serde_json::from_value::<Outcome>(json!({"status":"outcome_unknown","retryable":true}))
            .is_err()
    );
}

#[test]
fn plugin_resource_manifest_rejects_escapes_aliases_and_configuration_files() {
    use beale_contracts::swarm_plugins::{ResourceFile, ResourceManifest, resource_path};
    let file = |path: &str| ResourceFile {
        path: path.into(),
        sha256: "a".repeat(64),
        size: 3,
        executable: false,
    };
    let valid = ResourceManifest {
        plugin: "example-plugin".into(),
        has_tools: false,
        files: vec![
            file("PLUGIN.md"),
            file("skills/example/assets/data.bin"),
            file("skills/example/SKILL.md"),
        ],
    };
    let mut valid = valid;
    valid.files.sort_by(|a, b| a.path.cmp(&b.path));
    valid.validate().unwrap();
    for path in [
        "../mcp.json",
        "mcp.json",
        "plugin.json",
        "/absolute",
        "skills/../SKILL.md",
        "skills/example/assets/CON.txt",
        "skills/example/assets/a.",
        "skills/example/assets/a ",
        "skills/example/assets/a:b",
        "skills/example/assets/a?b",
        "skills/example/assets/LPT1",
        "skills/example/assets/COM¹",
        "skills/example/assets/a\\b",
    ] {
        assert!(!resource_path(path), "{path}");
    }
    for extra in [
        "skills/example/assets/DATA.bin",
        "skills/example/assets/data.bin/child",
    ] {
        let mut bad = valid.clone();
        bad.files.push(file(extra));
        bad.files.sort_by(|a, b| a.path.cmp(&b.path));
        assert!(bad.validate().is_err());
    }
    let mut bad = valid.clone();
    bad.files[0].size = 8_000_001;
    assert!(bad.validate().is_err());
    let mut bad = valid.clone();
    bad.files[0].sha256 = "invalid".into();
    assert!(bad.validate().is_err());
    let mut bad = valid.clone();
    bad.files.reverse();
    assert!(bad.validate().is_err());
    let mut bad = valid.clone();
    bad.files.remove(0);
    assert!(bad.validate().is_err());
    let mut bad = valid.clone();
    bad.plugin = "../example".into();
    assert!(bad.validate().is_err());
    let mut value = serde_json::to_value(valid).unwrap();
    value["host_path"] = json!("synthetic");
    assert!(serde_json::from_value::<ResourceManifest>(value).is_err());
}

#[test]
fn plugin_resources_enforce_aggregate_and_declared_skill_boundaries() {
    use beale_contracts::swarm_plugins::{ResourceFile, ResourceManifest};
    let file = |path: &str, size| ResourceFile {
        path: path.into(),
        sha256: "a".repeat(64),
        size,
        executable: false,
    };
    let valid = ResourceManifest {
        plugin: "example-plugin".into(),
        has_tools: true,
        files: vec![file("PLUGIN.md", 1), file("skills/example/SKILL.md", 1)],
    };
    for name in ["con", "aux.txt", "lpt1", "nul"] {
        let mut bad = valid.clone();
        bad.plugin = name.into();
        assert!(bad.validate().is_err());
    }
    let mut undeclared = valid.clone();
    undeclared
        .files
        .push(file("skills/other/assets/example.bin", 1));
    undeclared.files.sort_by(|a, b| a.path.cmp(&b.path));
    assert!(undeclared.validate().is_err());
    let mut aliased = valid.clone();
    aliased
        .files
        .push(file("skills/Example/assets/example.bin", 1));
    aliased.files.sort_by(|a, b| a.path.cmp(&b.path));
    assert!(aliased.validate().is_err());
    let mut huge = valid.clone();
    for n in 0..8 {
        huge.files
            .push(file(&format!("skills/example/assets/{n}.bin"), 8_000_000));
    }
    huge.files.sort_by(|a, b| a.path.cmp(&b.path));
    assert!(huge.validate().is_err());
    let mut many = valid;
    for n in 0..4096 {
        many.files
            .push(file(&format!("skills/example/assets/{n}.bin"), 1));
    }
    many.files.sort_by(|a, b| a.path.cmp(&b.path));
    assert!(many.validate().is_err());
}

#[test]
fn resource_transport_binds_identity_requests_and_bounded_chunks() {
    use beale_contracts::swarm_plugins::*;
    use beale_contracts::swarms::*;
    let identity = ExecutionIdentity {
        workspace_id: "workspace-example".into(),
        scan_id: "scan-example".into(),
        agent_id: "agent-example".into(),
        attempt_id: "attempt-example".into(),
        generation: 1,
    };
    let manifest = ResourceManifest {
        plugin: "example-plugin".into(),
        has_tools: false,
        files: vec![ResourceFile {
            path: "PLUGIN.md".into(),
            sha256: "a".repeat(64),
            size: 3,
            executable: false,
        }],
    };
    let cases = vec![
        (
            ResourceRequest::Catalog {},
            ResourceResult::Catalog {
                plugins: vec!["example-plugin".into()],
            },
        ),
        (
            ResourceRequest::Prepare {
                plugin: manifest.plugin.clone(),
            },
            ResourceResult::Prepared {
                bundle: "a".repeat(64),
                manifest: manifest.clone(),
            },
        ),
        (
            ResourceRequest::Read {
                plugin: manifest.plugin.clone(),
                bundle: "a".repeat(64),
                path: "PLUGIN.md".into(),
                offset: 0,
            },
            ResourceResult::Chunk {
                bundle: "a".repeat(64),
                file: manifest.files[0].clone(),
                offset: 0,
                bytes: vec![0, 1, 2],
                sha256: "b".repeat(64),
            },
        ),
    ];
    for (operation, result) in cases {
        let reply = ResourceReply {
            identity: identity.clone(),
            result,
        };
        reply.validate_for(&identity, &operation).unwrap();
        let request = WorkerRequest {
            version: 1,
            workspace_id: identity.workspace_id.clone(),
            scan_id: identity.scan_id.clone(),
            agent_id: identity.agent_id.clone(),
            attempt_id: identity.attempt_id.clone(),
            generation: 1,
            operation_id: "resources-example".into(),
            operation: WorkerOperation::PluginResources {
                request: operation.clone(),
            },
        };
        assert_eq!(
            WorkerRequest::decode_json(&serde_json::to_vec(&request).unwrap()).unwrap(),
            request
        );
        beale_contracts::swarm_worker::WorkerReply::Acknowledged {
            acknowledgement: MutationAcknowledgement {
                version: 1,
                operation_id: request.operation_id.clone(),
                result: serde_json::to_value(&reply).unwrap(),
            },
        }
        .validate_for(&request)
        .unwrap();
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
                json!("changed-example")
            };
            assert!(
                serde_json::from_value::<ResourceReply>(value)
                    .unwrap()
                    .validate_for(&identity, &operation)
                    .is_err()
            );
        }
        let changed = ResourceRequest::Read {
            plugin: manifest.plugin.clone(),
            bundle: "c".repeat(64),
            path: "PLUGIN.md".into(),
            offset: 1,
        };
        assert!(reply.validate_for(&identity, &changed).is_err());
    }
    for path in [
        "../mcp.json",
        "mcp.json",
        "/absolute",
        "skills/example/assets/a:b",
    ] {
        assert!(
            ResourceRequest::Read {
                plugin: "example-plugin".into(),
                bundle: "a".repeat(64),
                path: path.into(),
                offset: 0
            }
            .validate()
            .is_err()
        );
    }
    let request = ResourceRequest::Catalog {};
    for plugins in [
        vec!["z".into(), "a".into()],
        vec!["example".into(), "example".into()],
        vec!["example".into(); 129],
    ] {
        assert!(
            ResourceReply {
                identity: identity.clone(),
                result: ResourceResult::Catalog { plugins }
            }
            .validate_for(&identity, &request)
            .is_err()
        );
    }
    let mut injected = serde_json::to_value(ResourceRequest::Prepare {
        plugin: "example-plugin".into(),
    })
    .unwrap();
    injected["host_path"] = json!("synthetic");
    assert!(serde_json::from_value::<ResourceRequest>(injected).is_err());
    let request = ResourceRequest::Read {
        plugin: "example-plugin".into(),
        bundle: "a".repeat(64),
        path: "PLUGIN.md".into(),
        offset: 0,
    };
    let result = ResourceResult::Chunk {
        bundle: "a".repeat(64),
        file: ResourceFile {
            size: MAX_RESOURCE_BYTES,
            ..manifest.files[0].clone()
        },
        offset: 0,
        bytes: vec![0; RESOURCE_CHUNK_BYTES + 1],
        sha256: "b".repeat(64),
    };
    assert!(
        ResourceReply { identity, result }
            .validate_for(&request_identity(), &request)
            .is_err()
    );
    fn request_identity() -> ExecutionIdentity {
        ExecutionIdentity {
            workspace_id: "workspace-example".into(),
            scan_id: "scan-example".into(),
            agent_id: "agent-example".into(),
            attempt_id: "attempt-example".into(),
            generation: 1,
        }
    }
}
