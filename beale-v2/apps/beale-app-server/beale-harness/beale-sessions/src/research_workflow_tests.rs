use super::*;
use beale_contracts::swarm_workflows::*;
use beale_storage::swarm_state::HandoffChange;

/// The host assigns handoff IDs; fixtures label their envelopes in the scope.
fn label(request: &HandoffRequest) -> &str {
    request
        .scope
        .iter()
        .find_map(|s| s.strip_prefix("fixture-label:"))
        .expect("fixture handoff label")
}

// Component composition: tool routing, private files, publication and durable transitions.
// Scheduler/inbox delivery is independently covered in hierarchy_workflow_tests.rs.
async fn handoff(
    f: &Fixture,
    source: &(LocalTurn, PathBuf),
    destination: &str,
    id: &str,
    payload: HandoffPayload,
) -> HandoffRequest {
    let saved = f.swarm.load("scan-example", true).unwrap();
    let parent = saved
        .agent(&source.0.agent_id)
        .unwrap()
        .summary
        .parent_id
        .as_ref()
        .unwrap();
    let profile = &saved.agent(parent).unwrap().summary.profile.id;
    let ScanMode::Swarm { snapshot } = &saved.config.mode else {
        panic!("expected swarm")
    };
    // The fixture computes the expected route only to check the host's choice.
    let workflow = snapshot.workflow().unwrap();
    let route = workflow
        .routes
        .iter()
        .find(|r| r.from == *profile && r.to == destination && r.payload == payload.kind())
        .unwrap();
    let write = f
        .call(
            &source.0,
            &source.1,
            &format!("write-{id}"),
            "write",
            json!({"path":format!("{id}.txt"),"content":format!("Synthetic evidence for {id}")}),
        )
        .await;
    assert!(!write.is_error, "{}", write.output);
    let request = HandoffDraft {
        to_profile: destination.into(),
        route_id: None,
        task: Task {
            question: format!("Continue synthetic {id}"),
            completion_criteria: vec!["Return evidence and limitations".into()],
            evidence_refs: vec!["evidence-example".into()],
        },
        scope: vec![
            "Synthetic example.test fixture".into(),
            format!("fixture-label:{id}"),
        ],
        record_revisions: vec![],
        artifact_refs: vec![format!("file:{id}.txt")],
        observations: vec![format!("Original observation for {id}")],
        unresolved_questions: vec!["Check the synthetic prerequisite".into()],
        payload,
    };
    let reply = f
        .call(
            &source.0,
            &source.1,
            id,
            "swarm_message_parent",
            json!({"message":"Synthetic evidence handoff","handoff":request}),
        )
        .await;
    assert!(!reply.is_error, "{}", reply.output);
    let handoff_id = serde_json::from_str::<Value>(&reply.output).unwrap()["handoff_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let saved = f.swarm.load("scan-example", true).unwrap();
    let accepted = saved
        .handoffs
        .iter()
        .find(|h| h.request.id == handoff_id)
        .unwrap()
        .request
        .clone();
    // The host completed the envelope from durable identity and saved state.
    assert_eq!(accepted.from_agent, source.0.agent_id);
    assert_eq!(&accepted.parent_id, parent);
    assert_eq!(accepted.route_id, route.id);
    assert_eq!(
        accepted.destination_profile,
        ProfileRef {
            id: destination.into(),
            revision: 1,
        }
    );
    assert_eq!(label(&accepted), id);
    assert_eq!(accepted.observations, request.observations);
    assert!(!accepted.artifact_refs[0].starts_with("file:"));
    let artifact = store(&f.state)
        .unwrap()
        .artifact("workspace-example", &accepted.artifact_refs[0])
        .unwrap();
    assert_eq!(artifact.producer, source.0.agent_id);
    assert_eq!(artifact.attempt, source.0.attempt_id);
    assert_eq!(artifact.generation, source.0.generation);
    accepted
}

async fn receive(f: &Fixture, destination: &(LocalTurn, PathBuf), request: &HandoffRequest) {
    let name = format!("received-{}.txt", request.id);
    assert!(!destination.1.join(&name).exists());
    let reply = f
        .call(
            &destination.0,
            &destination.1,
            &format!("receive-{}", request.id),
            "swarm_materialize_artifact",
            json!({"artifact":request.artifact_refs[0],"destination":name}),
        )
        .await;
    assert!(!reply.is_error, "{}", reply.output);
    assert_eq!(
        std::fs::read_to_string(destination.1.join(name)).unwrap(),
        format!("Synthetic evidence for {}", label(request))
    );
}

fn assessment(request: &HandoffRequest) -> WorkResult {
    WorkResult {
        completion: Completion::Complete,
        assessment: Assessment::Supported,
        summary: "Synthetic fixture supports the recorded observation".into(),
        observations: request.observations.clone(),
        evidence_refs: request.artifact_refs.clone(),
        artifact_refs: request.artifact_refs.clone(),
        unresolved_questions: vec![],
    }
}

fn transition(f: &Fixture, request: &HandoffRequest, status: HandoffStatus) {
    let current = f
        .swarm
        .load("scan-example", true)
        .unwrap()
        .handoffs
        .into_iter()
        .find(|h| h.request.id == request.id)
        .unwrap();
    let result = (status == HandoffStatus::Completed).then(|| assessment(request));
    f.swarm
        .change_handoff(
            "scan-example",
            &format!("transition-{}-{}", request.id, current.revision),
            HandoffChange {
                id: request.id.clone(),
                expected_revision: current.revision,
                status,
                assignment_context: Some("Parent assigned synthetic work".into()),
                result,
            },
        )
        .unwrap();
}

#[tokio::test]
async fn basic_two_deputies_handoff_artifacts_and_resume_completed_worker() {
    let f = Fixture::new();
    let first = f.child("area-one");
    let second = f.child("area-two");
    let a = f.spawn(&first.0, "investigator", "worker-one", false);
    let b = f.spawn(&second.0, "investigator", "worker-two", false);
    let request = handoff(
        &f,
        &a,
        "research_deputy",
        "basic-evidence",
        HandoffPayload::Task {},
    )
    .await;
    assert!(!b.1.join("basic-evidence.txt").exists());
    // The envelope goes to its parent, never directly to the sibling worker.
    let state = f.swarm.load("scan-example", true).unwrap();
    assert_eq!(
        state.handoff_path(&request).unwrap(),
        vec![first.0.agent_id.clone(), f.leader.agent_id.clone()]
    );
    receive(&f, &b, &request).await;
    transition(&f, &request, HandoffStatus::Assigned);
    transition(&f, &request, HandoffStatus::Completed);
    checkpoint(&f.state, &a.0, &json!({"saved":"synthetic context"})).unwrap();
    f.swarm
        .local_finish(
            &a.0,
            json!({"saved":"synthetic context"}),
            "Synthetic completed result",
            false,
        )
        .unwrap();
    let reply = f
        .call(
            &first.0,
            &first.1,
            "resume-worker",
            "swarm_message_subagent",
            json!({"child_id":a.0.agent_id,"message":"Related synthetic follow-up"}),
        )
        .await;
    assert!(!reply.is_error, "{}", reply.output);
    let resumed = f
        .swarm
        .local_claim("scan-example", &a.0.agent_id, 3, 64)
        .unwrap()
        .unwrap();
    assert!(resumed.generation > a.0.generation);
    assert_eq!(
        directory(&f.state, &resumed, f.workspace.clone()).unwrap(),
        a.1
    );
    assert!(a.1.join("basic-evidence.txt").exists());
    assert_eq!(
        std::fs::read_to_string(f.workspace.join("shared.txt")).unwrap(),
        "initial"
    );
    assert_eq!(f.swarm.load("scan-example", true).unwrap().agents.len(), 5);
}

#[tokio::test]
async fn vdh_missing_evidence_returns_and_preserves_provenance_after_reopen() {
    let f = Fixture::with_definition("vdh_vvs");
    let discovery = f.spawn(&f.leader, "discovery_harness", "discovery", false);
    let validation = f.spawn(&f.leader, "validation_system", "validation", false);
    let source = f.spawn(&discovery.0, "investigator", "candidate-worker", false);
    let reviewer = f.spawn(&validation.0, "challenger", "independent-review", true);
    assert_ne!(source.0.agent_id, reviewer.0.agent_id);
    assert!(!reviewer.0.prompt.contains("candidate-worker"));
    let initial = handoff(
        &f,
        &source,
        "validation_system",
        "candidate-initial",
        HandoffPayload::Candidate {
            claim: "Synthetic boundary candidate".into(),
        },
    )
    .await;
    receive(&f, &reviewer, &initial).await;
    transition(&f, &initial, HandoffStatus::Blocked);
    let missing = handoff(
        &f,
        &reviewer,
        "discovery_harness",
        "missing-evidence",
        HandoffPayload::Task {},
    )
    .await;
    receive(&f, &source, &missing).await;
    let extra = handoff(
        &f,
        &source,
        "validation_system",
        "candidate-evidence",
        HandoffPayload::Candidate {
            claim: "Synthetic boundary candidate with prerequisite".into(),
        },
    )
    .await;
    receive(&f, &reviewer, &extra).await;
    transition(&f, &initial, HandoffStatus::Assigned);
    transition(&f, &initial, HandoffStatus::Completed);
    let outcome = handoff(
        &f,
        &reviewer,
        "discovery_harness",
        "validation-result",
        HandoffPayload::Assessment {
            result: assessment(&extra),
        },
    )
    .await;
    receive(&f, &source, &outcome).await;
    let reopened = HostDatabase::open(&f.root.join("host")).unwrap();
    let saved = SwarmStore::open(reopened)
        .unwrap()
        .load("scan-example", true)
        .unwrap();
    let original = saved
        .handoffs
        .iter()
        .find(|h| h.request.id == initial.id)
        .unwrap();
    assert_eq!(original.request, initial);
    assert_eq!(original.status, HandoffStatus::Completed);
    assert_eq!(saved.handoffs.len(), 4);
    assert!(source.1.join("candidate-initial.txt").exists());
}

#[cfg(feature = "findings")]
#[tokio::test]
async fn oai_concurrent_items_reach_independent_patch_review_without_stage_barrier() {
    let f = Fixture::with_definition("oai_948");
    let mut workers = std::collections::BTreeMap::new();
    for (stage, profile) in [
        ("audit_targets", "investigator"),
        ("discovery", "investigator"),
        ("validation", "challenger"),
        ("reporter", "investigator"),
        ("patch_generation", "builder"),
    ] {
        let deputy = f.spawn(&f.leader, stage, stage, false);
        workers.insert(
            stage,
            f.spawn(
                &deputy.0,
                profile,
                &format!("{stage}-worker"),
                stage == "validation",
            ),
        );
    }
    let ranked = |item: &str| HandoffPayload::RankedTargets {
        targets: vec![RankedTarget {
            rank: 1,
            target: format!("Synthetic {item} boundary"),
            rationale: "Explicit synthetic transition".into(),
        }],
    };
    let a = handoff(
        &f,
        &workers["audit_targets"],
        "discovery",
        "item-a-ranked",
        ranked("a"),
    )
    .await;
    transition(&f, &a, HandoffStatus::Blocked);
    let b = handoff(
        &f,
        &workers["audit_targets"],
        "discovery",
        "item-b-ranked",
        ranked("b"),
    )
    .await;
    receive(&f, &workers["discovery"], &b).await;
    let candidate = handoff(
        &f,
        &workers["discovery"],
        "validation",
        "item-b-candidate",
        HandoffPayload::Candidate {
            claim: "Synthetic candidate B".into(),
        },
    )
    .await;
    receive(&f, &workers["validation"], &candidate).await;
    let supported = handoff(
        &f,
        &workers["validation"],
        "reporter",
        "item-b-supported",
        HandoffPayload::SupportedFinding {
            result: assessment(&candidate),
        },
    )
    .await;
    receive(&f, &workers["reporter"], &supported).await;
    beale_findings::migrate(&f.state.db).unwrap();
    let reporter = &workers["reporter"];
    let created = f.call(&reporter.0, &reporter.1, "record-item-b", "findings_create", json!({
        "title":"Synthetic candidate B", "summary":"Synthetic evidence", "hypothesis":"Example boundary",
        "root_cause":{"summary":"Candidate explanation", "evidence_refs":[{"kind":"artifact","reference":candidate.artifact_refs[0]}]}
    })).await;
    assert!(!created.is_error, "{}", created.output);
    let record: Value = serde_json::from_str(&created.output).unwrap();
    let finding = record["record"]["id"].as_str().unwrap();
    let read = f
        .call(
            &reporter.0,
            &reporter.1,
            "read-item-b",
            "findings_read",
            json!({"id":finding}),
        )
        .await;
    assert!(!read.is_error, "{}", read.output);
    let canonical: Value = serde_json::from_str(&read.output).unwrap();
    assert_eq!(canonical["record"]["id"], finding);
    let remediation = handoff(
        &f,
        &workers["reporter"],
        "patch_generation",
        "item-b-remediation",
        HandoffPayload::Remediation {
            finding_refs: vec![finding.into()],
        },
    )
    .await;
    receive(&f, &workers["patch_generation"], &remediation).await;
    let builder = &workers["patch_generation"];
    let (patch_name, patch_script, test_name, test_script, check_command) = if cfg!(windows) {
        (
            "patch.ps1",
            "(Get-Content input.txt -Raw).Replace('value=old','value=new') | Set-Content input.txt",
            "check.ps1",
            "if ((Get-Content input.txt -Raw).Trim() -ne 'value=new') { throw 'patch missing' }; Write-Output 'Synthetic patch verified'",
            "& ./patch.ps1; & ./check.ps1",
        )
    } else {
        (
            "patch.sh",
            "sed 's/value=old/value=new/' input.txt > patched.txt && mv patched.txt input.txt",
            "check.sh",
            r#"test "$(cat input.txt)" = value=new && printf 'Synthetic patch verified'"#,
            "bash patch.sh && bash check.sh",
        )
    };
    for (name, content) in [(patch_name, patch_script), (test_name, test_script)] {
        let written = f
            .call(
                &builder.0,
                &builder.1,
                &format!("write-{name}"),
                "write",
                json!({"path":name,"content":content}),
            )
            .await;
        assert!(!written.is_error, "{}", written.output);
    }
    let patch = handoff(
        &f,
        &workers["patch_generation"],
        "validation",
        "item-b-patch",
        HandoffPayload::Patch {
            patch_refs: vec![format!("file:{patch_name}")],
            test_refs: vec![format!("file:{test_name}")],
        },
    )
    .await;
    receive(&f, &workers["validation"], &patch).await;
    let HandoffPayload::Patch {
        patch_refs,
        test_refs,
    } = &patch.payload
    else {
        panic!("expected patch")
    };
    let checker = &workers["validation"];
    for (reference, name) in [(&patch_refs[0], patch_name), (&test_refs[0], test_name)] {
        assert!(!reference.starts_with("file:"));
        let metadata = store(&f.state)
            .unwrap()
            .artifact("workspace-example", reference)
            .unwrap();
        assert_eq!(metadata.producer, builder.0.agent_id);
        let materialized = f
            .call(
                &checker.0,
                &checker.1,
                &format!("receive-{name}"),
                "swarm_materialize_artifact",
                json!({"artifact":reference,"destination":name}),
            )
            .await;
        assert!(!materialized.is_error, "{}", materialized.output);
    }
    let input = f
        .call(
            &checker.0,
            &checker.1,
            "write-patch-input",
            "write",
            json!({"path":"input.txt","content":"value=old"}),
        )
        .await;
    assert!(!input.is_error, "{}", input.output);
    // The test must reject the unpatched input before accepting the applied patch.
    let before_command = if cfg!(windows) {
        "& ./check.ps1"
    } else {
        "bash check.sh"
    };
    let before = f
        .call(
            &checker.0,
            &checker.1,
            "check-before-patch",
            beale_tools::COMMAND_TOOL_NAME,
            json!({"command":before_command,"timeout_seconds":10}),
        )
        .await;
    assert!(
        !before.output.contains("Synthetic patch verified"),
        "{}",
        before.output
    );
    let checked = f
        .call(
            &checker.0,
            &checker.1,
            "apply-and-check-patch",
            beale_tools::COMMAND_TOOL_NAME,
            json!({"command":check_command,"timeout_seconds":10}),
        )
        .await;
    assert!(
        !checked.is_error && checked.output.contains("Synthetic patch verified"),
        "{}",
        checked.output
    );
    assert_eq!(
        std::fs::read_to_string(checker.1.join("input.txt"))
            .unwrap()
            .trim(),
        "value=new"
    );
    assert_ne!(
        workers["validation"].0.agent_id,
        workers["patch_generation"].0.agent_id
    );
    let check_evidence =
        serde_json::to_string(&json!({"before":before.output,"after":checked.output})).unwrap();
    let written = f
        .call(
            &checker.0,
            &checker.1,
            "retain-patch-check",
            "write",
            json!({"path":"patch-check-result.txt","content":check_evidence}),
        )
        .await;
    assert!(!written.is_error, "{}", written.output);
    let mut patch_assessment = assessment(&patch);
    patch_assessment.summary =
        "Independent check rejects the baseline and passes after applying the submitted patch"
            .into();
    patch_assessment.evidence_refs = patch_refs.iter().chain(test_refs).cloned().collect();
    patch_assessment.artifact_refs = vec!["file:patch-check-result.txt".into()];
    for (destination, id) in [
        ("patch_generation", "item-b-patch-result"),
        ("reporter", "item-b-report-result"),
    ] {
        let result = handoff(
            &f,
            &workers["validation"],
            destination,
            id,
            HandoffPayload::PatchAssessment {
                patch_refs: patch_refs.clone(),
                test_refs: test_refs.clone(),
                result: patch_assessment.clone(),
            },
        )
        .await;
        let HandoffPayload::PatchAssessment {
            patch_refs: assessed_patch,
            test_refs: assessed_test,
            result: checked_result,
        } = &result.payload
        else {
            panic!("expected assessment")
        };
        assert_eq!(assessed_patch, patch_refs);
        assert_eq!(assessed_test, test_refs);
        let evidence = store(&f.state)
            .unwrap()
            .artifact("workspace-example", &checked_result.artifact_refs[0])
            .unwrap();
        assert_eq!(evidence.producer, checker.0.agent_id);
        assert_eq!(
            store(&f.state)
                .unwrap()
                .read_blob(&evidence.sha256, evidence.size)
                .unwrap(),
            check_evidence.as_bytes()
        );
        receive(&f, &workers[destination], &result).await;
        transition(&f, &result, HandoffStatus::Assigned);
        transition(&f, &result, HandoffStatus::Completed);
    }
    let saved = f.swarm.load("scan-example", true).unwrap();
    assert_eq!(
        saved
            .handoffs
            .iter()
            .find(|h| h.request.id == a.id)
            .unwrap()
            .status,
        HandoffStatus::Blocked
    );
    assert_eq!(saved.handoffs.len(), 8);
    receive(&f, &workers["discovery"], &a).await;
    transition(&f, &a, HandoffStatus::Assigned);
}
