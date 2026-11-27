use beale_contracts::{swarm_allocation::*, v1::*};
use prost::Message;

#[test]
fn destination_and_retention_fields_round_trip_without_changing_empty_defaults() {
    let mut request = SubmitSessionTurnRequest::default();
    let old = SubmitSessionTurnRequest::decode(request.encode_to_vec().as_slice()).unwrap();
    assert!(old.swarm_destinations.is_empty() && old.swarm_retention.is_none());
    request.swarm_destinations.push(SwarmDestination {
        host_id: "host-example".into(),
        configuration_id: "image-example".into(),
    });
    request.swarm_retention = Some(SwarmRetentionPolicy {
        ended_scan_seconds: 604800,
        storage_reserve_bytes: STORAGE_RESERVE,
    });
    assert_eq!(
        SubmitSessionTurnRequest::decode(request.encode_to_vec().as_slice()).unwrap(),
        request
    );
}

#[test]
fn allocation_intent_preserves_all_fields_and_rejects_unknown_inputs() {
    let spec = AllocationSpec {
        id: "allocation-example".into(),
        leader: "leader-example".into(),
        scan: "scan-example".into(),
        agent: "agent-example".into(),
        parent: "parent-example".into(),
        configuration: "image-example".into(),
        input_bytes: 123,
        storage_reserve: STORAGE_RESERVE,
        processing_result: true,
        restore: None,
    };
    let mut json =
        serde_json::to_value(HostAllocationRequest::Enqueue { spec: spec.clone() }).unwrap();
    let HostAllocationRequest::Enqueue { spec: decoded } =
        serde_json::from_value(json.clone()).unwrap()
    else {
        panic!("wrong request")
    };
    assert_eq!(decoded, spec);
    json["spec"]["local_fallback"] = true.into();
    assert!(serde_json::from_value::<HostAllocationRequest>(json).is_err());
}

#[test]
fn export_cancellation_preserves_exact_execution_and_rejects_unknown_inputs() {
    let target = beale_contracts::swarm_host::ExecutionTarget {
        leader_id: "leader-example".into(),
        host_id: "host-example".into(),
        configuration_id: "image-example".into(),
        configuration_revision: 1,
        instance_id: "instance-example".into(),
        identity: beale_contracts::swarm_worker::ExecutionIdentity {
            workspace_id: "workspace-example".into(),
            scan_id: "scan-example".into(),
            agent_id: "agent-example".into(),
            attempt_id: "attempt-example".into(),
            generation: 2,
        },
    };
    let mut json = serde_json::to_value(HostAllocationRequest::CancelExport {
        target: target.clone(),
        transfer: "transfer-example".into(),
    })
    .unwrap();
    let HostAllocationRequest::CancelExport {
        target: decoded,
        transfer,
    } = serde_json::from_value(json.clone()).unwrap()
    else {
        panic!("wrong operation")
    };
    assert_eq!(decoded, target);
    assert_eq!(transfer, "transfer-example");
    json["discard_outputs"] = true.into();
    assert!(serde_json::from_value::<HostAllocationRequest>(json).is_err());
}

#[test]
fn busy_allocation_reply_is_distinct_from_terminal_rejection() {
    let json = serde_json::to_value(HostAllocationReply::Busy {}).unwrap();
    assert_eq!(json, serde_json::json!({"result":"busy"}));
    assert!(matches!(
        serde_json::from_value::<HostAllocationReply>(json).unwrap(),
        HostAllocationReply::Busy {}
    ));
    assert!(
        serde_json::from_value::<HostAllocationReply>(
            serde_json::json!({"result":"busy","reason":"unexpected"})
        )
        .is_err()
    );
}
