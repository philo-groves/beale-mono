//! Canonical handoff custody and lifecycle, in the same transaction as inbox delivery.
use super::*;
use beale_contracts::{swarm_local::HandoffAction, tool_error::ToolError};

fn owner(h: &WorkflowHandoff) -> &str {
    h.assignment_context
        .as_deref()
        .unwrap_or(&h.request.parent_id)
}

pub(super) fn lifecycle(
    state: &mut ScanState,
    local: &mut LocalState,
    agent: &str,
    status: HandoffStatus,
) -> Result<()> {
    let control = local
        .controls
        .iter_mut()
        .find(|c| c.agent_id == agent)
        .context("missing control")?;
    let mut blocked = vec![];
    for h in &mut state.handoffs {
        if owner(h) == agent
            && !matches!(h.status, HandoffStatus::Completed | HandoffStatus::Canceled)
            && h.status != status
        {
            // An explicit model block is already Blocked and must never become
            // retryable merely because its executor later finishes or fails.
            if status == HandoffStatus::Blocked {
                control.recoverable_handoffs.push(h.request.id.clone());
                blocked.push(h.request.id.clone());
            } else {
                control
                    .recoverable_handoffs
                    .retain(|id| id != &h.request.id);
            }
            h.status = status;
            h.revision = next(h.revision)?;
        }
    }
    for id in blocked {
        drop_inputs(local, agent, &id);
    }
    Ok(())
}

/// Only an explicit parent/operator message retries executor-blocked work.
/// Claiming unrelated inbox work is not a retry decision.
pub(super) fn retry(state: &mut ScanState, local: &mut LocalState, agent: &str) -> Result<()> {
    let control = local
        .controls
        .iter_mut()
        .find(|c| c.agent_id == agent)
        .context("missing control")?;
    let ids = std::mem::take(&mut control.recoverable_handoffs);
    for id in ids {
        if let Some(h) = state.handoffs.iter_mut().find(|h| h.request.id == id)
            && owner(h) == agent
            && h.status == HandoffStatus::Blocked
        {
            h.status = HandoffStatus::Assigned;
            h.revision = next(h.revision)?;
            let request = h.request.clone();
            let operation = format!("handoff-retry-{}-{}", request.id, h.revision);
            drop_inputs(local, agent, &id);
            enqueue_lifecycle(
                state,
                local,
                &operation,
                "harness",
                agent,
                "Retry executor-blocked handoff with retained evidence",
            )?;
            let mail = local.inbox.last_mut().context("missing retry mail")?;
            mail.handoff = Some(request);
            local.handoff_inputs.push(mail.sequence);
        }
    }
    Ok(())
}

fn drop_inputs(local: &mut LocalState, agent: &str, id: &str) {
    // Retire only executable deliveries. A result or status notification can
    // legitimately reach a former owner after it delegates to a child.
    local.inbox.retain(|m| {
        !(m.to == agent
            && local.handoff_inputs.contains(&m.sequence)
            && m.handoff.as_ref().is_some_and(|h| h.id == id))
    });
    local
        .handoff_inputs
        .retain(|sequence| local.inbox.iter().any(|m| m.sequence == *sequence));
}

fn retire(local: &mut LocalState, agent: &str, id: &str) -> Result<()> {
    drop_inputs(local, agent, id);
    let control = local
        .controls
        .iter_mut()
        .find(|c| c.agent_id == agent)
        .context("missing control")?;
    control.recoverable_handoffs.retain(|item| item != id);
    if !control.retired_handoffs.iter().any(|item| item == id) {
        control.retired_handoffs.push(id.into());
    }
    Ok(())
}

pub(super) fn dispatch(
    tx: &Transaction<'_>,
    state: &mut ScanState,
    local: &mut LocalState,
    actor: &AgentSummary,
    operation: &str,
    action: HandoffAction,
) -> Result<Value> {
    let index = state
        .handoffs
        .iter()
        .position(|h| h.request.id == action.id())
        .context(ToolError::missing("unknown handoff"))?;
    let saved = state.handoffs[index].clone();
    ensure!(
        !matches!(
            saved.status,
            HandoffStatus::Completed | HandoffStatus::Canceled
        ),
        ToolError::conflict("handoff is terminal")
    );
    let owner_id = owner(&saved).to_owned();
    let owns = owner_id == actor.id;
    let parent_controls = state.agent(&owner_id)?.summary.parent_id.as_ref() == Some(&actor.id);
    let mut outcome = None;
    let (status, recipient) = match action {
        HandoffAction::Forward { to, .. } => {
            ensure!(
                owns || (parent_controls
                    && saved.status == HandoffStatus::Blocked
                    && !active(state, &owner_id)),
                ToolError::rejected("handoff belongs to another agent")
            );
            let target = state.agent(&to)?.summary.clone();
            let upstream = saved.assignment_context.is_none()
                && actor.role == Role::Deputy
                && actor.parent_id.as_ref() == Some(&to);
            let downstream = target.parent_id.as_ref() == Some(&actor.id)
                && match actor.role {
                    Role::Leader => target.profile == saved.request.destination_profile,
                    Role::Deputy => {
                        actor.profile == saved.request.destination_profile
                            && target.role == Role::Worker
                    }
                    Role::Worker => false,
                };
            ensure!(
                upstream || downstream,
                ToolError::rejected(
                    "handoff must follow its saved route through immediate parents and direct children"
                )
            );
            let target_control = local
                .controls
                .iter()
                .find(|c| c.agent_id == to)
                .context("missing recipient control")?;
            ensure!(
                !target_control.stop_requested,
                ToolError::rejected("handoff recipient is stopped")
            );
            ensure!(
                !target_control.independent || target.state != AgentState::Completed,
                ToolError::rejected("start a fresh independent reviewer")
            );
            // Forwarding to a completed reusable deputy creates normal resumed work.
            // The envelope travels separately, so even a 64 KiB handoff fits intact.
            if matches!(target.state, AgentState::Completed | AgentState::Blocked) {
                let id = new_id();
                state.assignments.push(Assignment {
                    id: id.clone(),
                    parent_id: target
                        .parent_id
                        .clone()
                        .context("cannot resume leader as child")?,
                    child_id: to.clone(),
                    task: saved.request.task.clone(),
                    state: AssignmentState::Queued,
                    wait_reason: "Awaiting handoff execution".into(),
                });
                state
                    .agents
                    .iter_mut()
                    .find(|a| Some(&a.summary.id) == target.parent_id.as_ref())
                    .context("missing recipient parent")?
                    .checkpoint
                    .pending_assignments
                    .push(id);
            }
            local
                .controls
                .iter_mut()
                .find(|c| c.agent_id == to)
                .context("missing recipient control")?
                .failures = 0;
            state.handoffs[index].assignment_context = Some(to.clone());
            if owner_id != to {
                retire(local, &owner_id, &saved.request.id)?;
            }
            let target_control = local
                .controls
                .iter_mut()
                .find(|c| c.agent_id == to)
                .context("missing recipient control")?;
            target_control
                .retired_handoffs
                .retain(|id| id != &saved.request.id);
            target_control
                .recoverable_handoffs
                .retain(|id| id != &saved.request.id);
            (HandoffStatus::Assigned, Some(to))
        }
        HandoffAction::Complete { result, .. } => {
            ensure!(
                owns,
                ToolError::rejected("only the handoff owner can complete it")
            );
            ensure!(
                result.completion == Completion::Complete,
                ToolError::invalid("use block for incomplete work")
            );
            crate::swarm_files::validate_record_artifacts(
                tx,
                &state.config.scan_id,
                &state.config.workspace_id,
                &serde_json::to_value(&result)?,
            )?;
            outcome = Some(result);
            (HandoffStatus::Completed, actor.parent_id.clone())
        }
        HandoffAction::Block { .. } => {
            ensure!(
                owns,
                ToolError::rejected("only the handoff owner can block it")
            );
            (HandoffStatus::Blocked, actor.parent_id.clone())
        }
        HandoffAction::Cancel { .. } => {
            ensure!(
                owns || parent_controls,
                ToolError::rejected("handoff cancellation requires owner or immediate parent")
            );
            (
                HandoffStatus::Canceled,
                if owns {
                    actor.parent_id.clone()
                } else {
                    Some(owner_id.clone())
                },
            )
        }
    };
    if status == HandoffStatus::Blocked {
        drop_inputs(local, &owner_id, &saved.request.id);
    } else if matches!(status, HandoffStatus::Completed | HandoffStatus::Canceled) {
        retire(local, &owner_id, &saved.request.id)?;
    }
    if status != HandoffStatus::Assigned {
        local
            .controls
            .iter_mut()
            .find(|c| c.agent_id == owner_id)
            .context("missing owner control")?
            .recoverable_handoffs
            .retain(|id| id != &saved.request.id);
    }
    let h = &mut state.handoffs[index];
    h.status = status;
    h.result = outcome.clone();
    h.revision = next(h.revision)?;
    let reply = json!({"id":h.request.id,"revision":h.revision,"status":h.status});
    if let Some(to) = recipient {
        enqueue(
            state,
            local,
            operation,
            &actor.id,
            &to,
            &format!("Handoff {}: {status:?}", saved.request.id),
        )?;
        let mail = local.inbox.last_mut().context("missing handoff mail")?;
        mail.handoff = Some(saved.request);
        mail.handoff_result = outcome;
        if status == HandoffStatus::Assigned {
            local.handoff_inputs.push(mail.sequence);
        }
    }
    Ok(reply)
}
