// Included by swarm_foundation to reuse its synthetic workflow fixture.
use super::*;
use beale_contracts::swarm_local::*;
#[path = "handoff_lifecycle.rs"]
mod handoff_lifecycle;

fn setup() -> (Fixture, LocalTurn) {
    let f = Fixture::new();
    let mut c = config();
    c.destinations.clear();
    let state = f.store.create_scan(c, true, true).unwrap();
    let leader = state.agents[0].summary.id.clone();
    f.store
        .local_initialize(
            "scan-example",
            LocalSettings {
                models: ["coordinator", "research", "investigator"]
                    .iter()
                    .map(|id| LocalModel {
                        profile: (*id).into(),
                        provider: 1,
                        method: 1,
                        model: "model-example".into(),
                    })
                    .collect(),
                reasoning_effort: 0,
            },
            "Coordinate synthetic research",
        )
        .unwrap();
    let turn = f
        .store
        .local_claim("scan-example", &leader, 1, 4)
        .unwrap()
        .unwrap();
    (f, turn)
}
fn spawn(
    f: &Fixture,
    parent: &LocalTurn,
    id: &str,
    role: &str,
    admission: Admission,
    independent: bool,
) -> String {
    let result = f
        .store
        .local_dispatch(
            parent,
            id,
            SwarmCommand::Spawn(Spawn {
                handoff_id: None,
                profile: profile(role),
                task: task(),
                independent,
            }),
            admission,
        )
        .unwrap();
    result["child_id"].as_str().unwrap().into()
}
fn finish(f: &Fixture, turn: &LocalTurn) {
    f.store
        .local_finish(
            turn,
            json!({"saved":"synthetic context"}),
            "Synthetic evidence result",
            false,
        )
        .unwrap();
}
fn claim(f: &Fixture, id: &str) -> LocalTurn {
    f.store
        .local_claim("scan-example", id, 2, 4)
        .unwrap()
        .unwrap()
}

#[test]
fn direct_child_authority_is_enforced_in_transactions() {
    let (f, leader) = setup();
    let deputy = spawn(
        &f,
        &leader,
        "spawn-deputy",
        "research",
        Admission::Ready,
        false,
    );
    let other = spawn(
        &f,
        &leader,
        "spawn-other",
        "research",
        Admission::Ready,
        false,
    );
    let deputy = claim(&f, &deputy);
    let worker = spawn(
        &f,
        &deputy,
        "spawn-worker",
        "investigator",
        Admission::Ready,
        false,
    );
    let worker_turn = claim(&f, &worker);
    for command in [
        SwarmCommand::Stop(Stop {
            child_id: worker.clone(),
        }),
        SwarmCommand::MessageChild(MessageChild {
            child_id: worker.clone(),
            message: "hello".into(),
        }),
    ] {
        assert!(
            f.store
                .local_dispatch(&leader, "forbidden", command, Admission::Ready)
                .is_err()
        );
    }
    assert!(
        f.store
            .local_dispatch(
                &deputy,
                "sibling",
                SwarmCommand::MessageChild(MessageChild {
                    child_id: other,
                    message: "hello".into()
                }),
                Admission::Ready
            )
            .is_err()
    );
    assert!(
        f.store
            .local_dispatch(
                &worker_turn,
                "worker-spawn",
                SwarmCommand::Spawn(Spawn {
                    handoff_id: None,
                    profile: profile("investigator"),
                    task: task(),
                    independent: false
                }),
                Admission::Ready
            )
            .is_err()
    );
    assert!(
        f.store
            .local_dispatch(
                &worker_turn,
                "list",
                SwarmCommand::List(Empty {}),
                Admission::Ready
            )
            .is_err()
    );
    let list = f
        .store
        .local_dispatch(
            &leader,
            "list",
            SwarmCommand::List(Empty {}),
            Admission::Ready,
        )
        .unwrap();
    assert!(list.as_array().unwrap().iter().any(|a| a["id"] == worker));
    assert!(serde_json::from_value::<SwarmCommand>(json!({"tool":"swarm_spawn_subagent","arguments":{"profile":profile("research"),"task":task(),"independent":false,"role":"leader","max_turns":10}})).is_err());
}

#[test]
fn queued_result_is_durable_before_release_and_wakes_replay_only_unread() {
    let (f, leader) = setup();
    let dep = spawn(&f, &leader, "spawn", "research", Admission::Ready, false);
    finish(&f, &leader);
    let deputy = claim(&f, &dep);
    let worker = spawn(
        &f,
        &deputy,
        "queued-worker",
        "investigator",
        Admission::YieldParent {
            reason: "Release deputy slot".into(),
        },
        false,
    );
    let reopened = f.reopen();
    assert_eq!(
        reopened
            .local_state("scan-example")
            .unwrap()
            .controls
            .iter()
            .find(|c| c.agent_id == dep)
            .unwrap()
            .tool_results["queued-worker"]["child_id"],
        worker
    );
    assert!(
        reopened
            .local_claim("scan-example", &worker, 3, 4)
            .unwrap()
            .is_none()
    );
    finish(&f, &deputy);
    assert_eq!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .agent(&dep)
            .unwrap()
            .summary
            .state,
        AgentState::Suspended
    );
    assert!(
        reopened
            .local_claim("scan-example", &worker, 4, 0)
            .unwrap()
            .is_none()
    );
    let child = claim(&f, &worker);
    finish(&f, &child);
    assert!(
        reopened
            .local_claim("scan-example", &dep, 4, 0)
            .unwrap()
            .is_none()
    );
    let resumed = claim(&f, &dep);
    assert!(resumed.prompt.contains("Synthetic evidence result"));
    assert!(resumed.inbox_cursor > deputy.inbox_cursor);
    finish(&f, &resumed);
    assert!(
        reopened
            .local_claim("scan-example", &dep, 5, 4)
            .unwrap()
            .is_none()
    );
    let leader2 = claim(&f, &leader.agent_id);
    let command = SwarmCommand::MessageChild(MessageChild {
        child_id: dep.clone(),
        message: "Related follow-up".into(),
    });
    let first = f
        .store
        .local_dispatch(&leader2, "follow-up", command.clone(), Admission::Ready)
        .unwrap();
    assert_eq!(
        first,
        f.store
            .local_dispatch(&leader2, "follow-up", command, Admission::Ready)
            .unwrap()
    );
    assert_eq!(
        f.store
            .local_state("scan-example")
            .unwrap()
            .inbox
            .iter()
            .filter(|m| m.to == dep)
            .count(),
        1
    );
    let followup = claim(&f, &dep);
    assert_eq!(followup.session_id, deputy.session_id);
    assert_eq!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .agent(&dep)
            .unwrap()
            .checkpoint
            .context,
        json!({"saved":"synthetic context"})
    );
    assert!(followup.prompt.contains("Related follow-up"));
    assert!(!followup.prompt.contains("Synthetic evidence result"));
}

#[test]
fn checkpoint_failure_blocks_child_until_success_and_recovery_is_bounded() {
    let (f, leader) = setup();
    let dep = spawn(&f, &leader, "spawn", "research", Admission::Ready, false);
    finish(&f, &leader);
    let deputy = claim(&f, &dep);
    let worker = spawn(
        &f,
        &deputy,
        "spawn-worker",
        "investigator",
        Admission::YieldParent {
            reason: "Yield slot".into(),
        },
        false,
    );
    assert!(
        f.store
            .local_finish(&deputy, json!("x".repeat(1_048_577)), "", false)
            .is_err()
    );
    assert!(
        f.store
            .local_claim("scan-example", &worker, 2, 4)
            .unwrap()
            .is_none()
    );
    f.store.local_failed(&deputy).unwrap();
    for _ in 0..2 {
        let retry = claim(&f, &dep);
        f.store.local_failed(&retry).unwrap();
    }
    assert!(
        f.store
            .local_claim("scan-example", &dep, 3, 4)
            .unwrap()
            .is_none()
    );
    assert!(
        f.store
            .local_claim("scan-example", &worker, 3, 4)
            .unwrap()
            .is_none()
    );
    let saved = f.store.load("scan-example", true).unwrap();
    assert_eq!(
        saved.agent(&dep).unwrap().summary.state,
        AgentState::Blocked
    );
    assert_ne!(
        saved.agent(&leader.agent_id).unwrap().summary.state,
        AgentState::Stopped
    );
    assert!(
        claim(&f, &leader.agent_id)
            .prompt
            .contains("recovery exhausted")
    );
}

#[test]
fn cancellation_keeps_capacity_until_ack_preserves_partial_and_unrelated_branches() {
    let (f, leader) = setup();
    let dep = spawn(&f, &leader, "spawn", "research", Admission::Ready, false);
    let other = spawn(&f, &leader, "other", "research", Admission::Ready, false);
    let deputy = claim(&f, &dep);
    let child = spawn(
        &f,
        &deputy,
        "worker",
        "investigator",
        Admission::Ready,
        false,
    );
    finish(&f, &deputy);
    let worker = claim(&f, &child);
    f.store
        .local_dispatch(
            &leader,
            "stop",
            SwarmCommand::Stop(Stop {
                child_id: dep.clone(),
            }),
            Admission::Ready,
        )
        .unwrap();
    assert!(f.store.local_authorize(&worker).is_err());
    assert!(
        f.store
            .local_claim("scan-example", &other, 3, 2)
            .unwrap()
            .is_none()
    );
    f.store
        .local_finish(
            &worker,
            json!({"partial":"saved evidence"}),
            "Partial result",
            true,
        )
        .unwrap();
    assert!(
        f.store
            .local_claim("scan-example", &other, 3, 2)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .agent(&child)
            .unwrap()
            .checkpoint
            .context["partial"],
        "saved evidence"
    );
    assert!(
        f.store
            .local_state("scan-example")
            .unwrap()
            .inbox
            .iter()
            .any(|m| m.to == leader.agent_id && m.message.contains("canceled"))
    );
}

#[test]
fn independent_review_has_new_identity_and_no_participant_context() {
    let (f, leader) = setup();
    let first = spawn(
        &f,
        &leader,
        "review-one",
        "research",
        Admission::Ready,
        true,
    );
    let reviewer = claim(&f, &first);
    finish(&f, &reviewer);
    assert!(
        f.store
            .local_dispatch(
                &leader,
                "reuse",
                SwarmCommand::MessageChild(MessageChild {
                    child_id: first.clone(),
                    message: "Review independently".into()
                }),
                Admission::Ready
            )
            .is_err()
    );
    let fresh = spawn(
        &f,
        &leader,
        "review-two",
        "research",
        Admission::Ready,
        true,
    );
    assert_ne!(fresh, first);
    assert_eq!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .agent(&fresh)
            .unwrap()
            .checkpoint
            .context,
        json!({})
    );
    let fresh = claim(&f, &fresh);
    assert_ne!(fresh.session_id, reviewer.session_id);
    assert!(!fresh.prompt.contains("synthetic context"));
}

#[test]
fn fleet_destinations_cannot_silently_execute_locally() {
    let f = Fixture::new();
    f.scan();
    assert!(
        f.store
            .local_initialize(
                "scan-example",
                LocalSettings {
                    models: vec![LocalModel {
                        profile: "coordinator".into(),
                        provider: 1,
                        method: 1,
                        model: "model-example".into()
                    }],
                    reasoning_effort: 0
                },
                "Research"
            )
            .is_err()
    );
}

#[test]
fn parent_messages_wake_suspended_deputy_once_without_invalidating_child_release() {
    let (f, leader) = setup();
    let dep = spawn(&f, &leader, "deputy", "research", Admission::Ready, false);
    let deputy = claim(&f, &dep);
    let worker = spawn(
        &f,
        &deputy,
        "child",
        "investigator",
        Admission::YieldParent {
            reason: "Yield capacity".into(),
        },
        false,
    );
    finish(&f, &deputy);
    f.db.set_feature_enabled("swarms", false).unwrap();
    let command = SwarmCommand::MessageChild(MessageChild {
        child_id: dep.clone(),
        message: "Clarify synthetic scope".into(),
    });
    for _ in 0..2 {
        f.store
            .local_dispatch(&leader, "clarify", command.clone(), Admission::Ready)
            .unwrap();
    }
    let resumed = claim(&f, &dep);
    let inbox: Vec<InboxMessage> = serde_json::from_str(&resumed.prompt).unwrap();
    assert_eq!(inbox.len(), 1);
    finish(&f, &resumed);
    assert!(
        f.store
            .local_claim("scan-example", &dep, 3, 4)
            .unwrap()
            .is_none()
    );
    let worker = claim(&f, &worker);
    finish(&f, &worker);
    assert!(claim(&f, &dep).prompt.contains("Synthetic evidence result"));
}

#[test]
fn infeasible_spawn_leaves_no_backlog_and_leader_failure_stops_the_tree() {
    let (f, leader) = setup();
    let command = SwarmCommand::Spawn(Spawn {
        handoff_id: None,
        profile: profile("research"),
        task: task(),
        independent: false,
    });
    let result = f
        .store
        .local_dispatch(
            &leader,
            "denied",
            command,
            Admission::Denied {
                reason: "No eligible capacity".into(),
            },
        )
        .unwrap();
    assert_eq!(result["status"], "denied");
    assert!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .assignments
            .is_empty()
    );
    let dep = spawn(&f, &leader, "deputy", "research", Admission::Ready, false);
    f.store.local_failed(&leader).unwrap();
    for _ in 0..2 {
        let retry = claim(&f, &leader.agent_id);
        f.store.local_failed(&retry).unwrap();
    }
    assert!(
        f.store
            .local_claim("scan-example", &dep, 3, 4)
            .unwrap()
            .is_none()
    );
    assert!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .agents
            .iter()
            .all(|a| a.summary.state == AgentState::Stopped)
    );
}

#[test]
fn parent_handoffs_are_durable_content_bound_and_reject_spoofed_sources() {
    let (f, leader) = setup();
    let dep = spawn(&f, &leader, "deputy", "research", Admission::Ready, false);
    let deputy = claim(&f, &dep);
    let worker = spawn(
        &f,
        &deputy,
        "worker",
        "investigator",
        Admission::Ready,
        false,
    );
    let worker = claim(&f, &worker);
    let handoff = HandoffRequest {
        id: "handoff-example".into(),
        from_agent: worker.agent_id.clone(),
        parent_id: dep.clone(),
        route_id: "research-request".into(),
        destination_profile: profile("research"),
        task: task(),
        artifact_refs: vec!["artifact-example".into()],
        record_revisions: vec![RecordRevision {
            id: "record-example".into(),
            revision: 2,
        }],
        scope: vec!["Synthetic scope".into()],
        observations: vec!["Synthetic observation".into()],
        unresolved_questions: vec![],
        payload: HandoffPayload::Task {},
    };
    let command = SwarmCommand::MessageParent(Box::new(MessageParent {
        message: "Evidence handoff".into(),
        handoff: Some(handoff.clone()),
    }));
    for _ in 0..2 {
        f.store
            .local_dispatch(&worker, "handoff", command.clone(), Admission::Ready)
            .unwrap();
    }
    assert_eq!(
        f.reopen()
            .load("scan-example", true)
            .unwrap()
            .handoffs
            .len(),
        1
    );
    assert_eq!(
        f.store
            .local_state("scan-example")
            .unwrap()
            .inbox
            .iter()
            .filter(|m| m.to == dep)
            .count(),
        1
    );
    // Delivery is the same typed envelope, not just the message's prose.
    finish(&f, &deputy);
    let resumed = claim(&f, &dep);
    let inbox: Vec<InboxMessage> = serde_json::from_str(&resumed.prompt).unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].handoff.as_ref(), Some(&handoff));
    finish(&f, &resumed);
    assert!(
        f.reopen()
            .local_claim("scan-example", &dep, 3, 4)
            .unwrap()
            .is_none()
    );
    let mut spoofed = handoff;
    spoofed.from_agent = leader.agent_id.clone();
    assert!(
        f.store
            .local_dispatch(
                &worker,
                "spoof",
                SwarmCommand::MessageParent(Box::new(MessageParent {
                    message: "Handoff".into(),
                    handoff: Some(spoofed)
                })),
                Admission::Ready
            )
            .is_err()
    );
    finish(&f, &worker);
    assert!(
        f.store
            .local_dispatch(&worker, "stale", command, Admission::Ready)
            .is_err()
    );
}

#[test]
fn queued_followup_preserves_initial_assignment_and_evidence() {
    let (f, leader) = setup();
    let mut assigned = task();
    assigned.evidence_refs.push("evidence-example".into());
    let result = f
        .store
        .local_dispatch(
            &leader,
            "spawn",
            SwarmCommand::Spawn(Spawn {
                handoff_id: None,
                profile: profile("research"),
                task: assigned.clone(),
                independent: false,
            }),
            Admission::Ready,
        )
        .unwrap();
    let child = result["child_id"].as_str().unwrap();
    let followup = SwarmCommand::MessageChild(MessageChild {
        child_id: child.into(),
        message: "Also check the synthetic boundary".into(),
    });
    f.store
        .local_dispatch(&leader, "followup", followup.clone(), Admission::Ready)
        .unwrap();
    f.store
        .local_dispatch(&leader, "followup", followup, Admission::Ready)
        .unwrap();
    let child = claim(&f, child);
    let prompt: serde_json::Value = serde_json::from_str(&child.prompt).unwrap();
    assert_eq!(
        prompt["assignment"],
        serde_json::to_value(&assigned).unwrap()
    );
    assert_eq!(prompt["messages"].as_array().unwrap().len(), 1);
    assert_eq!(
        prompt["messages"][0]["message"],
        "Also check the synthetic boundary"
    );
    f.store.local_failed(&child).unwrap();
    let retry = claim(&f, &child.agent_id);
    assert_eq!(retry.prompt, child.prompt);
    finish(&f, &retry);
    assert!(
        f.store
            .local_claim("scan-example", &child.agent_id, 3, 4)
            .unwrap()
            .is_none()
    );
}

#[test]
fn multibyte_completion_and_stop_fit_mailbox_and_commit_once() {
    for stop in [false, true] {
        let (f, leader) = setup();
        let dep = spawn(&f, &leader, "deputy", "research", Admission::Ready, false);
        let deputy = claim(&f, &dep);
        let answer = "a例\u{20000}".repeat(6000);
        f.store
            .local_finish(&deputy, json!({"saved":true}), &answer, stop)
            .unwrap();
        let saved = f.reopen().load("scan-example", true).unwrap();
        assert_eq!(
            saved.agent(&dep).unwrap().summary.state,
            if stop {
                AgentState::Stopped
            } else {
                AgentState::Completed
            }
        );
        assert!(
            saved
                .attempts
                .iter()
                .filter(|a| a.agent_id == dep)
                .all(|a| a.state == ExecutionState::Stopped)
        );
        let inbox = f.store.local_state("scan-example").unwrap().inbox;
        let result = inbox.iter().find(|m| m.from == dep).unwrap();
        assert!(result.message.len() <= 16384);
        assert!(result.message.len() >= 16380);
        let text = result.message.split_once(". Result: ").unwrap().1;
        assert!(answer.starts_with(text));
        assert!(
            f.store
                .local_finish(&deputy, json!({}), &answer, stop)
                .is_err()
        );
        assert_eq!(f.store.local_state("scan-example").unwrap().inbox, inbox);
        finish(&f, &leader);
        assert!(claim(&f, &leader.agent_id).prompt.contains(text));
    }
}

#[test]
fn exhausted_checkpoint_blocks_branch_without_replaying_or_releasing_child() {
    let (f, leader) = setup();
    let dep = spawn(&f, &leader, "deputy", "research", Admission::Ready, false);
    let deputy = claim(&f, &dep);
    let worker = spawn(
        &f,
        &deputy,
        "worker",
        "investigator",
        Admission::YieldParent {
            reason: "Yield capacity".into(),
        },
        false,
    );
    let accepted = f
        .store
        .local_state("scan-example")
        .unwrap()
        .controls
        .into_iter()
        .find(|c| c.agent_id == dep)
        .unwrap()
        .tool_results;
    f.store.local_checkpoint_failed(&deputy).unwrap();
    let saved = f.reopen().load("scan-example", true).unwrap();
    assert_eq!(
        saved.agent(&dep).unwrap().summary.state,
        AgentState::Blocked
    );
    assert_eq!(
        saved.agent(&leader.agent_id).unwrap().summary.state,
        AgentState::Running
    );
    assert!(
        f.store
            .local_claim("scan-example", &dep, 3, 4)
            .unwrap()
            .is_none()
    );
    assert!(
        f.store
            .local_claim("scan-example", &worker, 3, 4)
            .unwrap()
            .is_none()
    );
    let local = f.store.local_state("scan-example").unwrap();
    assert_eq!(
        local
            .controls
            .iter()
            .find(|c| c.agent_id == dep)
            .unwrap()
            .tool_results,
        accepted
    );
    assert!(
        local
            .inbox
            .iter()
            .any(|m| m.to == leader.agent_id && m.from == dep)
    );
}

#[test]
fn oversized_commands_are_rejected_before_creating_work() {
    let (f, leader) = setup();
    let mut assigned = task();
    assigned.completion_criteria = vec!["x".repeat(4096); 32];
    assigned.validate().unwrap();
    let command = SwarmCommand::Spawn(Spawn {
        handoff_id: None,
        profile: profile("research"),
        task: assigned,
        independent: false,
    });
    assert!(
        f.store
            .local_dispatch(&leader, "oversized", command, Admission::Ready)
            .is_err()
    );
    assert!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .assignments
            .is_empty()
    );
}

#[test]
fn full_handoffs_are_batched_without_losing_unread_payloads() {
    let (f, leader) = setup();
    let mut assigned = task();
    assigned.question = "x".repeat(4000);
    assigned.completion_criteria = vec!["x".repeat(4096); 14];
    assigned.completion_criteria.push("x".into());
    let mut spawn_command = SwarmCommand::Spawn(Spawn {
        handoff_id: None,
        profile: profile("research"),
        task: assigned,
        independent: false,
    });
    let extra = 65536 - serde_json::to_vec(&spawn_command).unwrap().len();
    if let SwarmCommand::Spawn(s) = &mut spawn_command {
        s.task
            .completion_criteria
            .last_mut()
            .unwrap()
            .push_str(&"x".repeat(extra));
    }
    spawn_command.validate().unwrap();
    let result = f
        .store
        .local_dispatch(&leader, "large-task", spawn_command, Admission::Ready)
        .unwrap();
    let dep = result["child_id"].as_str().unwrap();
    let deputy = claim(&f, dep);
    let worker = spawn(
        &f,
        &deputy,
        "worker",
        "investigator",
        Admission::Ready,
        false,
    );
    let worker = claim(&f, &worker);
    let mut expected = vec![];
    for n in 0..3 {
        let mut observations = vec!["x".repeat(4096); 15];
        observations.push("x".into());
        let mut command = SwarmCommand::MessageParent(Box::new(MessageParent {
            message: "Evidence handoff".into(),
            handoff: Some(HandoffRequest {
                id: format!("handoff-{n}"),
                from_agent: worker.agent_id.clone(),
                parent_id: dep.into(),
                route_id: "research-request".into(),
                destination_profile: profile("research"),
                task: task(),
                scope: vec!["Synthetic scope".into()],
                record_revisions: vec![],
                artifact_refs: vec!["artifact-example".into()],
                observations,
                unresolved_questions: vec![],
                payload: HandoffPayload::Task {},
            }),
        }));
        let extra = 65536 - serde_json::to_vec(&command).unwrap().len();
        if let SwarmCommand::MessageParent(m) = &mut command {
            let h = m.handoff.as_mut().unwrap();
            h.observations
                .last_mut()
                .unwrap()
                .push_str(&"x".repeat(extra));
            expected.push(h.clone());
        }
        command.validate().unwrap();
        f.store
            .local_dispatch(&worker, &format!("message-{n}"), command, Admission::Ready)
            .unwrap();
    }
    // The initial assignment remains required after a failed first execution.
    f.store.local_failed(&deputy).unwrap();
    let resumed = claim(&f, dep);
    assert!(resumed.prompt.len() <= 262_144);
    let prompt: serde_json::Value = serde_json::from_str(&resumed.prompt).unwrap();
    assert!(prompt["assignment"]["question"].as_str().unwrap().len() >= 4000);
    let mut delivered: Vec<InboxMessage> =
        serde_json::from_value(prompt["messages"].clone()).unwrap();
    assert_eq!(delivered.len(), 2, "Large third message must remain unread");
    finish(&f, &resumed);
    let next = claim(&f, dep);
    delivered.extend(serde_json::from_str::<Vec<InboxMessage>>(&next.prompt).unwrap());
    assert_eq!(
        delivered
            .into_iter()
            .map(|m| m.handoff.unwrap())
            .collect::<Vec<_>>(),
        expected
    );
    finish(&f, &next);
    assert!(
        f.store
            .local_claim("scan-example", dep, 3, 4)
            .unwrap()
            .is_none()
    );
}

#[test]
fn full_mailbox_cannot_block_lifecycle_or_lose_notifications() {
    let (f, leader) = setup();
    let ids = (0..5)
        .map(|n| {
            spawn(
                &f,
                &leader,
                &format!("spawn-{n}"),
                "research",
                Admission::Ready,
                false,
            )
        })
        .collect::<Vec<_>>();
    let completed = claim(&f, &ids[0]);
    // The initial operator message occupies the first slot.
    for n in 0..1023 {
        f.store
            .local_dispatch(
                &completed,
                &format!("update-{n}"),
                SwarmCommand::MessageParent(Box::new(MessageParent {
                    message: "Synthetic update".into(),
                    handoff: None,
                })),
                Admission::Ready,
            )
            .unwrap();
    }
    assert_eq!(
        f.store.local_state("scan-example").unwrap().inbox.len(),
        1024
    );
    assert!(
        f.store
            .local_dispatch(
                &completed,
                "overflow",
                SwarmCommand::MessageParent(Box::new(MessageParent {
                    message: "Ordinary messages still have backpressure".into(),
                    handoff: None,
                })),
                Admission::Ready
            )
            .is_err()
    );
    f.store.local_stop("scan-example", &ids[1]).unwrap();
    f.store.local_stop("scan-example", &ids[1]).unwrap();
    finish(&f, &completed);
    let stopped = claim(&f, &ids[2]);
    f.store.local_stop("scan-example", &ids[2]).unwrap();
    f.store
        .local_finish(
            &stopped,
            json!({"partial":"retained"}),
            "Synthetic partial result",
            true,
        )
        .unwrap();
    let checkpoint = claim(&f, &ids[3]);
    f.store.local_checkpoint_failed(&checkpoint).unwrap();
    let provider = claim(&f, &ids[4]);
    f.store.local_recovery_exhausted(&provider).unwrap();
    let store = f.reopen();
    let state = store.load("scan-example", true).unwrap();
    assert_eq!(
        state.agent(&ids[0]).unwrap().summary.state,
        AgentState::Completed
    );
    for id in &ids[1..3] {
        assert_eq!(state.agent(id).unwrap().summary.state, AgentState::Stopped);
    }
    for id in &ids[3..] {
        assert_eq!(state.agent(id).unwrap().summary.state, AgentState::Blocked);
    }
    assert_eq!(
        state.agent(&leader.agent_id).unwrap().summary.state,
        AgentState::Running
    );
    assert!(
        state
            .attempts
            .iter()
            .filter(|a| a.agent_id != leader.agent_id)
            .all(|a| a.state == ExecutionState::Stopped)
    );
    assert_eq!(
        state.agent(&ids[2]).unwrap().checkpoint.context,
        json!({"partial":"retained"})
    );
    let expected = store.local_state("scan-example").unwrap().inbox;
    assert_eq!(
        state.events.iter().map(|e| e.sequence).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(expected.len(), 1029); // Five lifecycle notices, one per child.
    assert_eq!(expected.iter().filter(|m| m.from == ids[1]).count(), 1);
    finish(&f, &leader);
    let mut delivered = vec![];
    while let Some(turn) = store
        .local_claim("scan-example", &leader.agent_id, 4, 4)
        .unwrap()
    {
        delivered.extend(serde_json::from_str::<Vec<InboxMessage>>(&turn.prompt).unwrap());
        store
            .local_finish(
                &turn,
                json!({"saved":true}),
                "Acknowledged synthetic updates",
                false,
            )
            .unwrap();
    }
    assert_eq!(
        delivered,
        expected
            .into_iter()
            .filter(|m| m.from != "operator")
            .collect::<Vec<_>>()
    );
    assert!(store.local_state("scan-example").unwrap().inbox.is_empty());
    assert!(
        store
            .local_claim("scan-example", &leader.agent_id, 5, 4)
            .unwrap()
            .is_none()
    );
}
