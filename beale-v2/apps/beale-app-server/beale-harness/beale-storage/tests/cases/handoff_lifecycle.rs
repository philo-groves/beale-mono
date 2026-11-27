use super::*;

fn submit(f: &Fixture, source: &LocalTurn, leader: &LocalTurn, id: &str) -> HandoffRequest {
    let handoff = HandoffRequest {
        id: id.into(),
        from_agent: source.agent_id.clone(),
        parent_id: leader.agent_id.clone(),
        route_id: "research-request".into(),
        destination_profile: profile("research"),
        task: task(),
        scope: vec!["Synthetic example.test".into()],
        record_revisions: vec![],
        artifact_refs: vec![],
        observations: vec!["x".repeat(4096); 12],
        unresolved_questions: vec![],
        payload: HandoffPayload::Task {},
    };
    let command = SwarmCommand::MessageParent(Box::new(MessageParent {
        message: "Synthetic handoff".into(),
        handoff: Some(handoff.clone()),
    }));
    assert!(command.validate().is_ok());
    f.store
        .local_dispatch(source, id, command, Admission::Ready)
        .unwrap();
    handoff
}
fn action(
    f: &Fixture,
    turn: &LocalTurn,
    operation: &str,
    action: HandoffAction,
) -> serde_json::Value {
    f.store
        .local_dispatch(
            turn,
            operation,
            SwarmCommand::Handoff(Box::new(action)),
            Admission::Ready,
        )
        .unwrap()
}
fn result() -> WorkResult {
    WorkResult {
        completion: Completion::Complete,
        assessment: Assessment::Inconclusive,
        summary: "Synthetic work completed without a supported conclusion".into(),
        observations: vec![],
        evidence_refs: vec![],
        artifact_refs: vec![],
        unresolved_questions: vec!["Need evidence".into()],
    }
}

#[test]
fn canonical_handoff_forwarding_is_lossless_atomic_authorized_and_replayable() {
    let (f, leader) = setup();
    let source = spawn(&f, &leader, "source", "research", Admission::Ready, false);
    let dest = spawn(&f, &leader, "dest", "research", Admission::Ready, false);
    let source = claim(&f, &source);
    let dest = claim(&f, &dest);
    let expected = submit(&f, &source, &leader, "large-handoff");
    assert!(serde_json::to_vec(&expected).unwrap().len() > 48_000);
    let forward = HandoffAction::Forward {
        id: expected.id.clone(),
        to: dest.agent_id.clone(),
    };
    assert!(
        f.store
            .local_dispatch(
                &source,
                "sibling",
                SwarmCommand::Handoff(Box::new(forward.clone())),
                Admission::Ready
            )
            .is_err()
    );
    let reply = action(&f, &leader, "forward", forward.clone());
    assert_eq!(
        f.reopen()
            .local_dispatch(
                &leader,
                "forward",
                SwarmCommand::Handoff(Box::new(forward)),
                Admission::Ready
            )
            .unwrap(),
        reply
    );
    assert_eq!(
        f.store
            .local_state("scan-example")
            .unwrap()
            .inbox
            .iter()
            .filter(|m| m.id == "forward")
            .count(),
        1
    );
    assert_eq!(
        f.store
            .local_state("scan-example")
            .unwrap()
            .inbox
            .iter()
            .find(|m| m.id == "forward")
            .unwrap()
            .handoff
            .as_ref(),
        Some(&expected)
    );
    let spawn = SwarmCommand::Spawn(Spawn {
        handoff_id: Some(expected.id.clone()),
        profile: profile("investigator"),
        task: task(),
        independent: true,
    });
    let before = f.store.load("scan-example", true).unwrap();
    assert!(
        f.store
            .local_dispatch(&source, "steal-on-spawn", spawn.clone(), Admission::Ready)
            .is_err()
    );
    assert_eq!(f.store.load("scan-example", true).unwrap(), before);
    let queued = f
        .store
        .local_dispatch(&dest, "assigned-worker", spawn.clone(), Admission::Ready)
        .unwrap();
    assert_eq!(
        f.reopen()
            .local_dispatch(&dest, "assigned-worker", spawn.clone(), Admission::Ready)
            .unwrap(),
        queued
    );
    assert_eq!(f.store.load("scan-example", true).unwrap().agents.len(), 4);
    finish(&f, &source);
    let worker = claim(&f, queued["child_id"].as_str().unwrap());
    assert!(worker.prompt.contains(&"x".repeat(4096)));
    let mut invalid = result();
    invalid
        .artifact_refs
        .push(format!("artifact-{}", "a".repeat(64)));
    assert!(
        f.store
            .local_dispatch(
                &worker,
                "invalid-result",
                SwarmCommand::Handoff(Box::new(HandoffAction::Complete {
                    id: expected.id.clone(),
                    result: invalid
                })),
                Admission::Ready
            )
            .is_err()
    );
    assert!(
        f.store
            .local_dispatch(
                &leader,
                "non-direct-cancel",
                SwarmCommand::Handoff(Box::new(HandoffAction::Cancel {
                    id: expected.id.clone()
                })),
                Admission::Ready
            )
            .is_err()
    );
    action(
        &f,
        &worker,
        "block-active",
        HandoffAction::Block {
            id: expected.id.clone(),
        },
    );
    let before = f.store.load("scan-example", true).unwrap();
    assert!(
        f.store
            .local_dispatch(&dest, "replace-live-owner", spawn, Admission::Ready)
            .is_err()
    );
    assert_eq!(f.store.load("scan-example", true).unwrap(), before);
    let complete = HandoffAction::Complete {
        id: expected.id.clone(),
        result: result(),
    };
    let reply = action(&f, &worker, "complete", complete.clone());
    assert_eq!(
        f.reopen()
            .local_dispatch(
                &worker,
                "complete",
                SwarmCommand::Handoff(Box::new(complete)),
                Admission::Ready
            )
            .unwrap(),
        reply
    );
    finish(&f, &worker);
    let saved = f.reopen().load("scan-example", true).unwrap();
    assert_eq!(saved.handoffs[0].request, expected);
    assert_eq!(saved.handoffs[0].status, HandoffStatus::Completed);
    assert_eq!(saved.handoffs[0].result, Some(result()));
    assert_eq!(
        f.store
            .local_state("scan-example")
            .unwrap()
            .inbox
            .iter()
            .find(|m| m.id == "complete")
            .unwrap()
            .handoff_result,
        Some(result())
    );
}

#[test]
fn handoff_recovery_resume_stop_and_scan_end_preserve_terminal_results() {
    let (f, leader) = setup();
    let source = spawn(&f, &leader, "source", "research", Admission::Ready, false);
    let dest = spawn(&f, &leader, "dest", "research", Admission::Ready, false);
    let source = claim(&f, &source);
    let mut dest = claim(&f, &dest);
    for id in ["recover-handoff", "cancel-handoff"] {
        submit(&f, &source, &leader, id);
        action(
            &f,
            &leader,
            id,
            HandoffAction::Forward {
                id: id.into(),
                to: dest.agent_id.clone(),
            },
        );
    }
    // Temporary provider failures do not invalidate ownership or accepted input.
    f.store.local_failed(&dest).unwrap();
    assert_eq!(
        f.reopen().load("scan-example", true).unwrap().handoffs[0].status,
        HandoffStatus::Assigned
    );
    dest = claim(&f, &dest.agent_id);
    f.store.local_recovery_exhausted(&dest).unwrap();
    assert_eq!(
        f.reopen().load("scan-example", true).unwrap().handoffs[0].status,
        HandoffStatus::Blocked
    );
    f.store
        .local_dispatch(
            &leader,
            "retry",
            SwarmCommand::MessageChild(MessageChild {
                child_id: dest.agent_id.clone(),
                message: "Retry with retained evidence".into(),
            }),
            Admission::Ready,
        )
        .unwrap();
    dest = claim(&f, &dest.agent_id);
    assert_eq!(
        f.reopen().load("scan-example", true).unwrap().handoffs[0].status,
        HandoffStatus::Assigned
    );
    action(
        &f,
        &dest,
        "complete-recovered",
        HandoffAction::Complete {
            id: "recover-handoff".into(),
            result: result(),
        },
    );
    f.store.local_stop("scan-example", &dest.agent_id).unwrap();
    let saved = f.reopen().load("scan-example", true).unwrap();
    assert_eq!(saved.handoffs[0].status, HandoffStatus::Completed);
    assert_eq!(saved.handoffs[1].status, HandoffStatus::Canceled);
    submit(&f, &source, &leader, "pending-at-end");
    f.store.end_scan("scan-example", 100).unwrap();
    let saved = f.reopen().load("scan-example", true).unwrap();
    assert_eq!(saved.handoffs[0].result, Some(result()));
    assert_eq!(saved.handoffs[2].status, HandoffStatus::Canceled);
}

#[test]
fn unrelated_handoff_and_messages_do_not_reactivate_explicitly_blocked_item() {
    let (f, leader) = setup();
    let source = spawn(&f, &leader, "source", "research", Admission::Ready, false);
    let dest = spawn(&f, &leader, "dest", "research", Admission::Ready, false);
    let source = claim(&f, &source);
    submit(&f, &source, &leader, "item-a");
    action(
        &f,
        &leader,
        "route-a",
        HandoffAction::Forward {
            id: "item-a".into(),
            to: dest.clone(),
        },
    );
    let dest_turn = claim(&f, &dest);
    action(
        &f,
        &dest_turn,
        "block-a",
        HandoffAction::Block {
            id: "item-a".into(),
        },
    );
    finish(&f, &dest_turn);
    assert_eq!(
        f.store.load("scan-example", true).unwrap().handoffs[0].status,
        HandoffStatus::Blocked
    );
    submit(&f, &source, &leader, "item-b");
    action(
        &f,
        &leader,
        "route-b",
        HandoffAction::Forward {
            id: "item-b".into(),
            to: dest.clone(),
        },
    );
    let next = claim(&f, &dest);
    assert!(!next.prompt.contains("item-a"));
    let status = f.store.load("scan-example", true).unwrap().handoffs[0].status;
    assert_eq!(status, HandoffStatus::Blocked);
    f.store.local_recovery_exhausted(&next).unwrap();
    // Failure blocks B for executor recovery without turning explicitly blocked A
    // into retryable work. Both decisions survive reopening the database.
    let reopened = f.reopen();
    reopened
        .local_dispatch(
            &leader,
            "retry-b",
            SwarmCommand::MessageChild(MessageChild {
                child_id: dest.clone(),
                message: "Retry executor-blocked work".into(),
            }),
            Admission::Ready,
        )
        .unwrap();
    let retried = claim(&f, &dest);
    let saved = reopened.load("scan-example", true).unwrap();
    assert_eq!(saved.handoffs[0].status, HandoffStatus::Blocked);
    assert_eq!(saved.handoffs[1].status, HandoffStatus::Assigned);
    action(
        &f,
        &retried,
        "complete-b",
        HandoffAction::Complete {
            id: "item-b".into(),
            result: result(),
        },
    );
    assert_eq!(
        f.reopen().load("scan-example", true).unwrap().handoffs[0].status,
        HandoffStatus::Blocked
    );
    finish(&f, &retried);
    // An explicit per-item retry is still supported, even for a model block.
    action(
        &f,
        &leader,
        "retry-a-only",
        HandoffAction::Forward {
            id: "item-a".into(),
            to: dest.clone(),
        },
    );
    let resumed_a = claim(&f, &dest);
    assert!(resumed_a.prompt.contains("item-a"));
    let saved = f.reopen().load("scan-example", true).unwrap();
    assert_eq!(saved.handoffs[0].status, HandoffStatus::Assigned);
    assert_eq!(saved.handoffs[1].status, HandoffStatus::Completed);
}
#[test]
fn replacement_retires_unread_input_and_invalidates_retained_custody() {
    for acknowledged in [false, true] {
        replacement_custody(acknowledged);
    }
}
fn replacement_custody(acknowledged: bool) {
    let (f, leader) = setup();
    let source = spawn(&f, &leader, "source", "research", Admission::Ready, false);
    let dest = spawn(&f, &leader, "dest", "research", Admission::Ready, false);
    let source = claim(&f, &source);
    submit(&f, &source, &leader, "reassigned-item");
    action(
        &f,
        &leader,
        "route",
        HandoffAction::Forward {
            id: "reassigned-item".into(),
            to: dest.clone(),
        },
    );
    let old = claim(&f, &dest);
    if acknowledged {
        finish(&f, &old);
        assert_eq!(
            f.reopen()
                .load("scan-example", true)
                .unwrap()
                .agent(&dest)
                .unwrap()
                .checkpoint
                .context,
            json!({"saved":"synthetic context"})
        );
    } else {
        f.store.local_recovery_exhausted(&old).unwrap();
    }
    let queued = f
        .store
        .local_dispatch(
            &leader,
            "replacement",
            SwarmCommand::Spawn(Spawn {
                handoff_id: Some("reassigned-item".into()),
                profile: profile("research"),
                task: task(),
                independent: false,
            }),
            Admission::Ready,
        )
        .unwrap();
    let replacement = claim(&f, queued["child_id"].as_str().unwrap());
    action(
        &f,
        &replacement,
        "completed",
        HandoffAction::Complete {
            id: "reassigned-item".into(),
            result: result(),
        },
    );
    finish(&f, &replacement);
    // The custody change is durable, including the retired input classification.
    let reopened = f.reopen();
    assert!(
        reopened
            .local_state("scan-example")
            .unwrap()
            .controls
            .iter()
            .find(|c| c.agent_id == dest)
            .unwrap()
            .retired_handoffs
            .contains(&"reassigned-item".into())
    );
    reopened
        .local_dispatch(
            &leader,
            "unrelated-followup",
            SwarmCommand::MessageChild(MessageChild {
                child_id: dest.clone(),
                message: "Do unrelated synthetic work".into(),
            }),
            Admission::Ready,
        )
        .unwrap();
    let resumed = claim(&f, &dest);
    let h = &f.store.load("scan-example", true).unwrap().handoffs[0];
    assert_eq!(h.status, HandoffStatus::Completed);
    assert_eq!(
        h.assignment_context.as_deref(),
        Some(replacement.agent_id.as_str())
    );
    assert!(resumed.prompt.contains("reassigned-item"));
    assert!(resumed.prompt.contains("no longer assigned to you"));
    assert!(!resumed.prompt.contains("Assigned"));
    let local = f.reopen().local_state("scan-example").unwrap();
    assert!(!local.inbox.iter().any(|m| {
        m.to == dest
            && m.handoff
                .as_ref()
                .is_some_and(|h| h.id == "reassigned-item")
    }));
    // Keep the original context intact; the next turn still receives a custody
    // correction even after the unrelated follow-up has been acknowledged.
    finish(&f, &resumed);
    f.store
        .local_message(
            "scan-example",
            "operator-followup",
            &dest,
            "More unrelated synthetic work",
        )
        .unwrap();
    let again = claim(&f, &dest);
    assert!(
        again.operator_text.as_deref() == Some("More unrelated synthetic work"),
        "custody correction must not hide operator input"
    );
    assert!(again.prompt.contains("no longer assigned to you"));
    assert!(again.prompt.contains("reassigned-item"));
    assert!(again.prompt.contains("More unrelated synthetic work"));
}

#[test]
fn unrelated_delivery_does_not_retry_executor_blocked_work_but_operator_retry_does() {
    let (f, leader) = setup();
    let source = spawn(&f, &leader, "source", "research", Admission::Ready, false);
    let dest = spawn(&f, &leader, "dest", "research", Admission::Ready, false);
    let source = claim(&f, &source);
    submit(&f, &source, &leader, "recovery-a");
    action(
        &f,
        &leader,
        "route-a",
        HandoffAction::Forward {
            id: "recovery-a".into(),
            to: dest.clone(),
        },
    );
    let first = claim(&f, &dest);
    f.store.local_recovery_exhausted(&first).unwrap();
    submit(&f, &source, &leader, "unrelated-b");
    action(
        &f,
        &leader,
        "route-b",
        HandoffAction::Forward {
            id: "unrelated-b".into(),
            to: dest.clone(),
        },
    );
    let next = claim(&f, &dest);
    assert!(!next.prompt.contains("recovery-a"));
    assert_eq!(
        f.reopen().load("scan-example", true).unwrap().handoffs[0].status,
        HandoffStatus::Blocked
    );
    action(
        &f,
        &next,
        "complete-b",
        HandoffAction::Complete {
            id: "unrelated-b".into(),
            result: result(),
        },
    );
    finish(&f, &next);
    f.reopen()
        .local_message(
            "scan-example",
            "operator-retry",
            &dest,
            "Retry executor-blocked work",
        )
        .unwrap();
    let retry = claim(&f, &dest);
    assert!(retry.prompt.contains("recovery-a"));
    assert_eq!(
        f.reopen().load("scan-example", true).unwrap().handoffs[0].status,
        HandoffStatus::Assigned
    );
    action(
        &f,
        &retry,
        "complete-a",
        HandoffAction::Complete {
            id: "recovery-a".into(),
            result: result(),
        },
    );
    assert!(
        f.reopen()
            .load("scan-example", true)
            .unwrap()
            .handoffs
            .iter()
            .all(|h| h.status == HandoffStatus::Completed)
    );
}
