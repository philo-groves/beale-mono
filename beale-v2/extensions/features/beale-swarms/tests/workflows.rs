use beale_contracts::{swarm_workflows::*, swarms::*};
use beale_storage::{
    HostDatabase,
    swarm_state::{HandoffChange, SwarmStore},
};
use beale_swarms::*;
use serde_json::json;

struct Fixture {
    root: std::path::PathBuf,
    db: HostDatabase,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "beale-swarm-workflow-test-{}",
            uuid::Uuid::new_v4()
        ));
        Self {
            db: HostDatabase::open(&root).unwrap(),
            root,
        }
    }
    fn scan(&self, id: &str) -> SwarmStore {
        create_scan(self.db.clone(), config(), id, true)
            .unwrap()
            .unwrap();
        SwarmStore::open(self.db.clone()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn config() -> ScanConfig {
    ScanConfig {
        version: VERSION,
        scan_id: "scan-example".into(),
        workspace_id: "workspace-example".into(),
        mode: ScanMode::Local {},
        authorized_scope: "Synthetic example.test fixtures".into(),
        enabled_tools: vec![],
        provider_routes: vec![],
        destinations: vec![],
        pacing: vec![],
        retention: RetentionPolicy::default(),
    }
}
fn task() -> Task {
    Task {
        question: "Inspect the example boundary".into(),
        completion_criteria: vec!["Return evidence and limitations".into()],
        evidence_refs: vec!["evidence-example".into()],
    }
}

#[test]
fn local_admission_requires_feasibility_after_the_identified_release() {
    use beale_contracts::swarm_local::{Admission, Spawn, SwarmCommand};
    use beale_storage::swarm_state::{LocalModel, LocalSettings};
    use beale_swarms::local::{AdmissionAdapter, LocalAdmission};
    let f = Fixture::new();
    let store = f.scan("basic");
    let state = store.load("scan-example", true).unwrap();
    let leader = state.agents[0].summary.id.clone();
    store
        .local_initialize(
            "scan-example",
            LocalSettings {
                models: vec![LocalModel {
                    profile: "leader".into(),
                    provider: 1,
                    method: 1,
                    model: "example-model".into(),
                }],
                reasoning_effort: 0,
            },
            "Coordinate",
        )
        .unwrap();
    let leader = store
        .local_claim("scan-example", &leader, 1, 4)
        .unwrap()
        .unwrap();
    let child = store
        .local_dispatch(
            &leader,
            "spawn",
            SwarmCommand::Spawn(Spawn {
                handoff_id: None,
                profile: ProfileRef {
                    id: "research_deputy".into(),
                    revision: 1,
                },
                task: task(),
                independent: false,
            }),
            Admission::Ready,
        )
        .unwrap()["child_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let deputy = store
        .local_claim("scan-example", &child, 1, 4)
        .unwrap()
        .unwrap();
    let state = store.load("scan-example", true).unwrap();
    assert!(matches!(
        LocalAdmission { capacity: 0 }.spawn(&state, &deputy),
        Admission::Denied { .. }
    ));
    assert!(matches!(
        LocalAdmission { capacity: 1 }.spawn(&state, &deputy),
        Admission::Denied { .. }
    ));
    assert!(matches!(
        LocalAdmission { capacity: 2 }.spawn(&state, &deputy),
        Admission::YieldParent { .. }
    ));
    assert!(matches!(
        LocalAdmission { capacity: 3 }.spawn(&state, &deputy),
        Admission::Ready
    ));
    assert!(matches!(
        LocalAdmission { capacity: 2 }.spawn(&state, &leader),
        Admission::Denied { .. }
    ));
}
fn spawn(store: &SwarmStore, parent: &str, profile: &str, key: &str) -> String {
    match store
        .queue_spawn(
            "scan-example",
            parent,
            key,
            ProfileRef {
                id: profile.into(),
                revision: 1,
            },
            task(),
            "Awaiting admission",
        )
        .unwrap()
    {
        SpawnResult::Queued { child_id, .. } => child_id,
        _ => panic!("definition acceptance must not execute"),
    }
}
fn result() -> WorkResult {
    WorkResult {
        completion: Completion::Complete,
        assessment: Assessment::Inconclusive,
        summary: "The example check remains inconclusive".into(),
        observations: vec!["A prerequisite was absent".into()],
        evidence_refs: vec!["evidence-example".into()],
        artifact_refs: vec![],
        unresolved_questions: vec!["Obtain the prerequisite".into()],
    }
}
fn payload(kind: PayloadKind) -> HandoffPayload {
    match kind {
        PayloadKind::Task => HandoffPayload::Task {},
        PayloadKind::Candidate => HandoffPayload::Candidate {
            claim: "An example candidate requiring validation".into(),
        },
        PayloadKind::Assessment => HandoffPayload::Assessment { result: result() },
        PayloadKind::RankedTargets => HandoffPayload::RankedTargets {
            targets: vec![RankedTarget {
                rank: 1,
                target: "example boundary".into(),
                rationale: "Explicit trust transition".into(),
            }],
        },
        PayloadKind::SupportedFinding => HandoffPayload::SupportedFinding {
            result: WorkResult {
                assessment: Assessment::Supported,
                ..result()
            },
        },
        PayloadKind::Remediation => HandoffPayload::Remediation {
            finding_refs: vec!["finding-example".into()],
        },
        PayloadKind::Patch => HandoffPayload::Patch {
            patch_refs: vec!["patch-example".into()],
            test_refs: vec!["test-example".into()],
        },
        PayloadKind::PatchAssessment => HandoffPayload::PatchAssessment {
            patch_refs: vec!["patch-example".into()],
            test_refs: vec!["test-example".into()],
            result: result(),
        },
    }
}
fn request(source: &str, parent: &str, route: &HandoffRoute) -> HandoffRequest {
    HandoffRequest {
        id: format!("handoff-{}", route.id),
        from_agent: source.into(),
        parent_id: parent.into(),
        route_id: route.id.clone(),
        destination_profile: ProfileRef {
            id: route.to.clone(),
            revision: 1,
        },
        task: task(),
        scope: vec!["Synthetic example scope".into()],
        record_revisions: vec![RecordRevision {
            id: "record-example".into(),
            revision: 2,
        }],
        artifact_refs: vec!["artifact-example".into()],
        observations: vec!["Original example observation".into()],
        unresolved_questions: vec!["Original example question".into()],
        payload: payload(route.payload),
    }
}
#[test]
fn bundled_structures_have_one_leader_and_exact_stage_delegation() {
    assert_eq!(
        bundled().iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
        [BASIC_ID, VDH_VVS_ID, OAI_948_ID]
    );
    let expected = [
        (
            BASIC_ID,
            "research_deputy",
            vec![
                "mapper",
                "investigator",
                "experimenter",
                "challenger",
                "builder",
            ],
        ),
        (
            VDH_VVS_ID,
            "discovery_harness",
            vec!["mapper", "investigator", "challenger"],
        ),
        (
            VDH_VVS_ID,
            "validation_system",
            vec!["builder", "experimenter", "challenger"],
        ),
        (OAI_948_ID, "audit_targets", vec!["mapper", "investigator"]),
        (
            OAI_948_ID,
            "discovery",
            vec!["mapper", "investigator", "challenger"],
        ),
        (
            OAI_948_ID,
            "validation",
            vec!["builder", "experimenter", "challenger"],
        ),
        (OAI_948_ID, "reporter", vec!["investigator", "challenger"]),
        (
            OAI_948_ID,
            "patch_generation",
            vec!["builder", "experimenter", "challenger"],
        ),
    ];
    for (id, parent, children) in expected {
        let d = swarm_definition(id).unwrap().snapshot.workflow().unwrap();
        assert_eq!(d.leader().unwrap().profile.id, "leader");
        assert!(d.permits("leader", parent));
        assert_eq!(
            d.delegation
                .iter()
                .find(|d| d.parent == parent)
                .unwrap()
                .children,
            children
        );
        assert!(
            d.routes
                .iter()
                .all(|r| r.from != "leader" && r.to != "leader")
        );
    }
    assert_eq!(
        swarm_definition(BASIC_ID)
            .unwrap()
            .snapshot
            .workflow()
            .unwrap()
            .initial_deputies,
        ["research_deputy"]
    );
    for old in [
        "simple",
        "cloudflare_harness",
        "openai_pipeline",
        "sequential_pipeline",
    ] {
        assert!(swarm_definition(old).is_none());
    }
    for old in ["scout", "reviewer", "prover", "main"] {
        assert!(ProfileId::from_id(old).is_none());
    }
}
#[test]
fn malformed_definitions_and_authority_shortcuts_are_rejected() {
    let base = swarm_definition(OAI_948_ID)
        .unwrap()
        .snapshot
        .workflow()
        .unwrap();
    let mut d = base.clone();
    d.delegation[0].children.push("builder".into());
    assert!(d.validate().is_err());
    let mut d = base.clone();
    d.delegation.push(d.delegation[0].clone());
    assert!(d.validate().is_err());
    let mut d = base.clone();
    d.nodes[0].role = Role::Deputy;
    assert!(d.validate().is_err());
    let mut d = base.clone();
    d.routes[0].to = "builder".into();
    assert!(d.validate().is_err());
    let mut d = base.clone();
    d.routes.push(d.routes[0].clone());
    assert!(d.validate().is_err());
    let mut d = base.clone();
    d.nodes[0].profile.revision = 2;
    assert!(register_template(&serde_json::to_string(&d).unwrap()).is_err());
    let mut value = serde_json::to_value(base).unwrap();
    value["scheduler"] = json!({"mode":"rotating_stages"});
    assert!(WorkflowDefinition::decode(&value.to_string()).is_err());
    assert!(WorkflowDefinition::decode(include_str!("../swarms.json")).is_err());
}
#[test]
fn basic_accepts_multiple_deputies_without_allocating_instances() {
    let f = Fixture::new();
    let store = f.scan(BASIC_ID);
    let leader = store.load("scan-example", true).unwrap().agents[0]
        .summary
        .id
        .clone();
    let a = spawn(&store, &leader, "research_deputy", "area-one");
    let mut second = task();
    second.question = "Inspect another example area".into();
    let b = store
        .queue_spawn(
            "scan-example",
            &leader,
            "area-two",
            ProfileRef {
                id: "research_deputy".into(),
                revision: 1,
            },
            second,
            "Awaiting capacity",
        )
        .unwrap();
    assert!(matches!(b,SpawnResult::Queued{child_id,..} if child_id!=a));
    let saved = store.load("scan-example", true).unwrap();
    assert_eq!(saved.agents.len(), 3);
    assert!(
        saved.instances.is_empty() && saved.reservations.is_empty() && saved.attempts.is_empty()
    );
    assert!(
        store
            .queue_spawn(
                "scan-example",
                &leader,
                "skip-deputy",
                ProfileRef {
                    id: "builder".into(),
                    revision: 1
                },
                task(),
                "Wait"
            )
            .is_err()
    );
}
#[test]
fn all_forward_return_and_independent_patch_routes_are_durable_and_parent_mediated() {
    let oai = swarm_definition(OAI_948_ID)
        .unwrap()
        .snapshot
        .workflow()
        .unwrap();
    let expected_routes = [
        ("audit_targets", "discovery", PayloadKind::RankedTargets),
        ("discovery", "audit_targets", PayloadKind::Task),
        ("discovery", "validation", PayloadKind::Candidate),
        ("validation", "discovery", PayloadKind::Assessment),
        ("validation", "discovery", PayloadKind::Task),
        ("validation", "reporter", PayloadKind::SupportedFinding),
        ("reporter", "validation", PayloadKind::Task),
        ("reporter", "patch_generation", PayloadKind::Remediation),
        ("patch_generation", "reporter", PayloadKind::Task),
        ("patch_generation", "validation", PayloadKind::Patch),
        (
            "validation",
            "patch_generation",
            PayloadKind::PatchAssessment,
        ),
        ("validation", "reporter", PayloadKind::PatchAssessment),
    ];
    for (from, to, payload) in expected_routes {
        assert!(
            oai.routes
                .iter()
                .any(|r| r.from == from && r.to == to && r.payload == payload)
        );
    }
    let vdh = swarm_definition(VDH_VVS_ID)
        .unwrap()
        .snapshot
        .workflow()
        .unwrap();
    for (from, to, payload) in [
        (
            "discovery_harness",
            "validation_system",
            PayloadKind::Candidate,
        ),
        (
            "validation_system",
            "discovery_harness",
            PayloadKind::Assessment,
        ),
        ("validation_system", "discovery_harness", PayloadKind::Task),
    ] {
        assert!(
            vdh.routes
                .iter()
                .any(|r| r.from == from && r.to == to && r.payload == payload)
        );
    }
    let expected = [(VDH_VVS_ID, 3), (OAI_948_ID, 12), (BASIC_ID, 1)];
    for (id, count) in expected {
        let f = Fixture::new();
        let store = f.scan(id);
        let initial = store.load("scan-example", true).unwrap();
        let leader = initial.agents[0].summary.id.clone();
        let d = swarm_definition(id).unwrap().snapshot.workflow().unwrap();
        assert_eq!(d.routes.len(), count);
        for route in &d.routes {
            let deputy = spawn(
                &store,
                &leader,
                &route.from,
                &format!("deputy-{}", route.id),
            );
            let worker_profile = &d
                .delegation
                .iter()
                .find(|p| p.parent == route.from)
                .unwrap()
                .children[0];
            let worker = spawn(
                &store,
                &deputy,
                worker_profile,
                &format!("worker-{}", route.id),
            );
            let handoff = request(&worker, &deputy, route);
            assert_eq!(
                store
                    .load("scan-example", true)
                    .unwrap()
                    .handoff_path(&handoff)
                    .unwrap(),
                [deputy.clone(), leader.clone()]
            );
            let accepted = store.add_handoff("scan-example", handoff.clone()).unwrap();
            assert_eq!(accepted.status, HandoffStatus::Pending);
            let reopened = SwarmStore::open(f.db.clone()).unwrap();
            assert_eq!(
                reopened
                    .add_handoff("scan-example", handoff.clone())
                    .unwrap(),
                accepted
            );
            let mut bypass = handoff.clone();
            bypass.id = format!("bypass-{}", route.id);
            bypass.parent_id = leader.clone();
            assert!(store.add_handoff("scan-example", bypass).is_err());
            let mut direct = request(&deputy, &leader, route);
            direct.id = format!("deputy-handoff-{}", route.id);
            assert_eq!(
                store
                    .load("scan-example", true)
                    .unwrap()
                    .handoff_path(&direct)
                    .unwrap(),
                std::slice::from_ref(&leader)
            );
            store.add_handoff("scan-example", direct).unwrap();
        }
        let saved = store.load("scan-example", true).unwrap();
        assert_eq!(saved.handoffs.len(), count * 2);
    }
    let d = swarm_definition(OAI_948_ID)
        .unwrap()
        .snapshot
        .workflow()
        .unwrap();
    for (from, to, payload) in [
        ("patch_generation", "validation", PayloadKind::Patch),
        (
            "validation",
            "patch_generation",
            PayloadKind::PatchAssessment,
        ),
        ("validation", "reporter", PayloadKind::PatchAssessment),
    ] {
        assert!(
            d.routes
                .iter()
                .any(|r| r.from == from && r.to == to && r.payload == payload)
        );
    }
}
#[test]
fn stage_workers_cannot_bypass_their_deputys_delegation_or_handoff_contract() {
    let f = Fixture::new();
    let store = f.scan(OAI_948_ID);
    let leader = store.load("scan-example", true).unwrap().agents[0]
        .summary
        .id
        .clone();
    let deputy = spawn(&store, &leader, "reporter", "reporter-example");
    assert!(
        store
            .queue_spawn(
                "scan-example",
                &deputy,
                "wrong-stage",
                ProfileRef {
                    id: "builder".into(),
                    revision: 1
                },
                task(),
                "Wait"
            )
            .is_err()
    );
    let d = swarm_definition(OAI_948_ID)
        .unwrap()
        .snapshot
        .workflow()
        .unwrap();
    let wrong = &d.routes[0];
    assert!(
        store
            .add_handoff("scan-example", request(&deputy, &leader, wrong))
            .is_err()
    );
}
#[test]
fn specialized_payloads_reject_missing_evidence_rankings_and_patch_references() {
    let d = swarm_definition(OAI_948_ID)
        .unwrap()
        .snapshot
        .workflow()
        .unwrap();
    for r in &d.routes {
        let mut h = request("deputy-example", "leader-example", r);
        assert!(h.validate().is_ok());
        match &mut h.payload {
            HandoffPayload::Candidate { claim } => claim.clear(),
            HandoffPayload::RankedTargets { targets } => targets[0].rank = 2,
            HandoffPayload::SupportedFinding { result } => result.evidence_refs.clear(),
            HandoffPayload::Remediation { finding_refs } => finding_refs.clear(),
            HandoffPayload::Patch { test_refs, .. }
            | HandoffPayload::PatchAssessment { test_refs, .. } => test_refs.clear(),
            _ => {
                h.scope.clear();
            }
        }
        assert!(h.validate().is_err());
    }
    let mut raw =
        serde_json::to_value(request("deputy-example", "leader-example", &d.routes[0])).unwrap();
    raw["conversation"] = json!(["Inherited discovery conversation"]);
    assert!(serde_json::from_value::<HandoffRequest>(raw).is_err());
}
#[test]
fn handoff_transitions_preserve_observations_and_separate_completion_from_assessment() {
    let f = Fixture::new();
    let store = f.scan(BASIC_ID);
    let leader = store.load("scan-example", true).unwrap().agents[0]
        .summary
        .id
        .clone();
    let deputy = spawn(&store, &leader, "research_deputy", "area-example");
    let d = swarm_definition(BASIC_ID)
        .unwrap()
        .snapshot
        .workflow()
        .unwrap();
    let request = request(&deputy, &leader, &d.routes[0]);
    let accepted = store.add_handoff("scan-example", request.clone()).unwrap();
    let change = |revision, status, result| HandoffChange {
        id: request.id.clone(),
        expected_revision: revision,
        status,
        assignment_context: Some("Assignment context added by the parent".into()),
        result,
    };
    store
        .change_handoff(
            "scan-example",
            "blocked",
            change(1, HandoffStatus::Blocked, None),
        )
        .unwrap();
    store
        .change_handoff(
            "scan-example",
            "assigned",
            change(2, HandoffStatus::Assigned, None),
        )
        .unwrap();
    let done = store
        .change_handoff(
            "scan-example",
            "completed",
            change(3, HandoffStatus::Completed, Some(result())),
        )
        .unwrap();
    assert_eq!(done.request, request);
    assert_eq!(
        done.result.as_ref().unwrap().assessment,
        Assessment::Inconclusive
    );
    assert_eq!(
        store
            .change_handoff(
                "scan-example",
                "completed",
                change(3, HandoffStatus::Completed, Some(result()))
            )
            .unwrap(),
        done
    );
    assert_eq!(
        store.add_handoff("scan-example", request.clone()).unwrap(),
        accepted
    );
    let before = store.load("scan-example", true).unwrap();
    assert!(
        store
            .change_handoff(
                "scan-example",
                "stale",
                change(2, HandoffStatus::Canceled, None)
            )
            .is_err()
    );
    assert!(
        store
            .change_handoff(
                "scan-example",
                "overwrite",
                change(4, HandoffStatus::Canceled, None)
            )
            .is_err()
    );
    let mut altered = request;
    altered.observations.push("Changed observation".into());
    assert!(store.add_handoff("scan-example", altered).is_err());
    assert_eq!(store.load("scan-example", true).unwrap(), before);
    let mut result_json = serde_json::to_value(result()).unwrap();
    result_json["finding_status"] = json!("validated");
    assert!(serde_json::from_value::<WorkResult>(result_json).is_err());
}
#[test]
fn canceled_handoffs_are_terminal_and_invalid_outcomes_roll_back() {
    let f = Fixture::new();
    let store = f.scan(BASIC_ID);
    let leader = store.load("scan-example", true).unwrap().agents[0]
        .summary
        .id
        .clone();
    let deputy = spawn(&store, &leader, "research_deputy", "area-example");
    let d = swarm_definition(BASIC_ID)
        .unwrap()
        .snapshot
        .workflow()
        .unwrap();
    let request = request(&deputy, &leader, &d.routes[0]);
    store.add_handoff("scan-example", request.clone()).unwrap();
    let before = store.load("scan-example", true).unwrap();
    let invalid = HandoffChange {
        id: request.id.clone(),
        expected_revision: 1,
        status: HandoffStatus::Assigned,
        assignment_context: Some("Example context".into()),
        result: Some(result()),
    };
    assert!(
        store
            .change_handoff("scan-example", "invalid", invalid)
            .is_err()
    );
    assert_eq!(store.load("scan-example", true).unwrap(), before);
    let canceled = store
        .change_handoff(
            "scan-example",
            "cancel",
            HandoffChange {
                id: request.id.clone(),
                expected_revision: 1,
                status: HandoffStatus::Canceled,
                assignment_context: None,
                result: None,
            },
        )
        .unwrap();
    assert_eq!(canceled.status, HandoffStatus::Canceled);
    assert!(
        store
            .change_handoff(
                "scan-example",
                "restart",
                HandoffChange {
                    id: request.id,
                    expected_revision: 2,
                    status: HandoffStatus::Assigned,
                    assignment_context: Some("Example".into()),
                    result: None
                }
            )
            .is_err()
    );
}
#[test]
fn templates_and_profile_instructions_are_snapshotted_and_toggle_only_gates_creation() {
    let f = Fixture::new();
    let raw = include_str!("../examples/example-review.json");
    let id = register_template(raw).unwrap();
    let saved = create_scan(f.db.clone(), config(), &id, true)
        .unwrap()
        .unwrap();
    let mut revised: serde_json::Value = serde_json::from_str(raw).unwrap();
    revised["name"] = json!("Revised Example");
    revised["revision"] = json!(2);
    let new_id = register_template(&revised.to_string()).unwrap();
    assert_ne!(id, new_id);
    let store = SwarmStore::open(f.db.clone()).unwrap();
    let restored = store.load("scan-example", true).unwrap();
    assert_eq!(restored, saved);
    assert!(restored.config.validate_resume(true).is_ok());
    assert!(restored.config.validate_resume(false).is_err());
    let snapshot = match restored.config.mode {
        ScanMode::Swarm { snapshot } => snapshot,
        _ => panic!(),
    };
    assert_eq!(
        validate_snapshot(&serde_json::to_string(&snapshot).unwrap()).unwrap(),
        *snapshot
    );
    assert!(create_scan(f.db.clone(), config(), &new_id, false).is_err());
    assert_eq!(store.load("scan-example", true).unwrap(), saved);
}
#[test]
fn local_selection_needs_no_swarm_tables_and_rejects_fleet_destinations() {
    let f = Fixture::new();
    assert!(
        create_scan(f.db.clone(), config(), "", false)
            .unwrap()
            .is_none()
    );
    assert!(
        create_scan(f.db.clone(), config(), "", true)
            .unwrap()
            .is_none()
    );
    assert_eq!(f.db.schema_version("swarms").unwrap(), 0);
    let mut remote = config();
    remote.destinations.push(Destination {
        host_id: "host-example".into(),
        configuration_id: "container-example".into(),
    });
    assert!(create_scan(f.db.clone(), remote, "", true).is_err());
    assert_eq!(f.db.schema_version("swarms").unwrap(), 0);
    assert_eq!(FEATURE.tool_names.len(), 7);
    assert!(FEATURE.prompt_extension.is_empty());
}

fn revised_snapshot(mut snapshot: DefinitionSnapshot) -> DefinitionSnapshot {
    let mut definition = snapshot.workflow().unwrap();
    definition.name = "Saved Example Workflow".into();
    definition.revision = 2;
    definition.nodes[0].x = 123;
    for node in &mut definition.nodes {
        node.profile.revision = 2;
    }
    snapshot.revision = 2;
    snapshot.definition = serde_json::to_value(definition).unwrap();
    for profile in &mut snapshot.profiles {
        profile.profile.revision = 2;
        profile.instruction = format!("Saved example instructions for {}", profile.profile.id);
    }
    snapshot.leader_rule.revision = 2;
    snapshot.leader_rule.instruction = "Saved example coordinator review rule".into();
    snapshot
}

#[test]
fn cold_saved_snapshots_preserve_profiles_rules_and_bundled_revisions() {
    let current = swarm_definition(BASIC_ID).unwrap().snapshot.clone();
    for id in [BASIC_ID, "custom:example-cold-snapshot"] {
        let f = Fixture::new();
        let mut snapshot = revised_snapshot(current.clone());
        snapshot.id = id.into();
        snapshot.definition["id"] = json!(id);
        let raw = serde_json::to_string(&snapshot).unwrap();
        // No custom registration or installed revision 2 exists in this process.
        let restored = restore_snapshot(&raw).unwrap();
        assert_eq!(restored.id, id);
        assert_eq!(restored.name, "Saved Example Workflow");
        assert_eq!(restored.layout[0].x, 123.0);
        assert_eq!(restored.snapshot, snapshot);
        assert_eq!(restore_snapshot(&raw).unwrap().snapshot, snapshot);

        let mut saved_config = config();
        saved_config.mode = ScanMode::Swarm {
            snapshot: Box::new(snapshot.clone()),
        };
        let store = SwarmStore::open(f.db.clone()).unwrap();
        store.create_scan(saved_config, true, true).unwrap();
        let reopened = SwarmStore::open(f.db.clone())
            .unwrap()
            .load("scan-example", true)
            .unwrap();
        let ScanMode::Swarm { snapshot: loaded } = reopened.config.mode else {
            panic!()
        };
        assert_eq!(*loaded, snapshot);
        assert_eq!(swarm_definition(BASIC_ID).unwrap().snapshot, current);
        if id.starts_with("custom:") {
            assert!(swarm_definition(id).is_none());
            assert!(create_scan(f.db.clone(), config(), id, true).is_err());
        }
    }
}

#[test]
fn saved_views_with_the_same_id_coexist_without_changing_the_catalog() {
    let custom = register_template(include_str!("../examples/example-review.json")).unwrap();
    for id in [BASIC_ID, custom.as_str()] {
        let current = swarm_definition(id).unwrap().snapshot.clone();
        let saved = revised_snapshot(current.clone());
        let first = restore_snapshot(&serde_json::to_string(&saved).unwrap()).unwrap();
        let mut second = saved.clone();
        second.definition["name"] = json!("Another Saved Example");
        second.leader_rule.instruction = "Another saved example coordinator rule".into();
        let next = restore_snapshot(&serde_json::to_string(&second).unwrap()).unwrap();
        assert_eq!(first.snapshot, saved);
        assert_eq!(next.snapshot, second);
        assert_eq!(swarm_definition(id).unwrap().snapshot, current);
        let f = Fixture::new();
        let created = create_scan(f.db.clone(), config(), id, true)
            .unwrap()
            .unwrap();
        assert_eq!(
            created.config.mode,
            ScanMode::Swarm {
                snapshot: Box::new(current)
            }
        );
    }
}

#[test]
fn workerless_definitions_are_rejected_before_catalog_registration() {
    let mut definition = swarm_definition(BASIC_ID)
        .unwrap()
        .snapshot
        .workflow()
        .unwrap();
    definition.id = "example-workerless".into();
    definition.nodes.retain(|node| node.role != Role::Worker);
    definition.delegation.retain(|edge| edge.parent == "leader");
    let raw = serde_json::to_string(&definition).unwrap();
    assert!(definition.validate().is_err());
    assert!(WorkflowDefinition::decode(&raw).is_err());
    assert!(register_template(&raw).is_err());
}
