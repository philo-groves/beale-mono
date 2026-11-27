//! Trusted local supervisor transactions. No worker transport or filesystem snapshots.
use super::*;
use beale_contracts::swarm_local::{Admission, InboxMessage, SwarmCommand};
#[path = "swarm_handoffs.rs"]
mod handoffs;

pub(super) const SCHEMA: &str = "CREATE TABLE swarm_local(scan_id TEXT PRIMARY KEY REFERENCES swarm_scans(id), document TEXT NOT NULL);";

pub(super) enum ClaimOwner<'a> {
    Local(Uuid),
    Peer(Uuid, &'a PeerPlacement),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalModel {
    pub profile: String,
    pub provider: i32,
    pub method: i32,
    pub model: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalSettings {
    pub models: Vec<LocalModel>,
    pub reasoning_effort: i32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalControl {
    pub agent_id: String,
    pub release_parent: Option<String>,
    pub yield_requested: bool,
    pub yield_committed: bool,
    pub stop_requested: bool,
    pub independent: bool,
    pub failures: u32,
    pub tool_results: Value,
    /// Executor-blocked work eligible for an explicit same-agent retry.
    pub recoverable_handoffs: Vec<String>,
    /// Custody revoked from retained context; never interpret it as active input.
    pub retired_handoffs: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalState {
    pub settings: LocalSettings,
    pub controls: Vec<LocalControl>,
    pub inbox: Vec<InboxMessage>,
    pub sequence: u64,
    /// Inbox sequences carrying executable custody, separate from result/status mail.
    pub handoff_inputs: Vec<u64>,
}
/// Host-issued identity, never decoded from tool arguments or client DTOs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalTurn {
    pub scan_id: String,
    pub agent_id: String,
    pub attempt_id: String,
    pub generation: u64,
    pub session_id: String,
    pub inbox_cursor: u64,
    pub prompt: String,
    /// Original user text, independent of model-only inbox/custody metadata.
    pub operator_text: Option<String>,
    pub profile: String,
    pub role: Role,
}

impl LocalTurn {
    pub fn transcript_prompt(&self) -> &str {
        self.operator_text.as_deref().unwrap_or(&self.prompt)
    }
}

fn control(agent: &str, independent: bool) -> LocalControl {
    LocalControl {
        agent_id: agent.into(),
        release_parent: None,
        yield_requested: false,
        yield_committed: false,
        stop_requested: false,
        independent,
        failures: 0,
        tool_results: json!({}),
        recoverable_handoffs: vec![],
        retired_handoffs: vec![],
    }
}
pub(super) fn read_local(db: &Connection, scan: &str) -> Result<LocalState> {
    let raw: String = db.query_row(
        "SELECT document FROM swarm_local WHERE scan_id=?1",
        [scan],
        |r| r.get(0),
    )?;
    Ok(serde_json::from_str(&raw)?)
}
pub(super) fn write_local(tx: &Transaction<'_>, scan: &str, local: &LocalState) -> Result<()> {
    tx.execute(
        "UPDATE swarm_local SET document=?2 WHERE scan_id=?1",
        params![scan, serde_json::to_string(local)?],
    )?;
    Ok(())
}
fn active(state: &ScanState, agent: &str) -> bool {
    state
        .attempts
        .iter()
        .any(|a| a.agent_id == agent && a.state != ExecutionState::Stopped)
}
pub(super) fn set_state(state: &mut ScanState, id: &str, status: AgentState) -> Result<()> {
    let agent = state
        .agents
        .iter_mut()
        .find(|a| a.summary.id == id)
        .context("unknown agent")?;
    agent.summary.state = status;
    agent.checkpoint.state = status;
    agent.summary.revision = next(agent.summary.revision)?;
    Ok(())
}
fn enqueue(
    state: &mut ScanState,
    local: &mut LocalState,
    id: &str,
    from: &str,
    to: &str,
    message: &str,
) -> Result<()> {
    enqueue_message(state, local, id, from, to, message, false)
}

// Lifecycle notifications belong to already accepted work. Ordinary mailbox
// admission must not prevent its completion, cancellation, or capacity release.
// These host-only messages share the durable sequence and acknowledgement cursor.
fn enqueue_lifecycle(
    state: &mut ScanState,
    local: &mut LocalState,
    id: &str,
    from: &str,
    to: &str,
    message: &str,
) -> Result<()> {
    enqueue_message(state, local, id, from, to, message, true)
}

fn enqueue_message(
    state: &mut ScanState,
    local: &mut LocalState,
    id: &str,
    from: &str,
    to: &str,
    message: &str,
    lifecycle: bool,
) -> Result<()> {
    ensure!(
        !message.trim().is_empty() && message.len() <= 16384,
        "invalid mailbox message"
    );
    state.agent(to)?;
    ensure!(
        lifecycle || local.inbox.iter().filter(|m| m.to == to).count() < 1024,
        beale_contracts::tool_error::ToolError::rejected("mailbox requires draining")
    );
    local.sequence = next(local.sequence)?;
    local.inbox.push(InboxMessage {
        sequence: local.sequence,
        id: id.into(),
        from: from.into(),
        to: to.into(),
        message: message.into(),
        handoff: None,
        handoff_result: None,
    });
    let agent = state
        .agents
        .iter_mut()
        .find(|a| a.summary.id == to)
        .context("unknown recipient")?;
    let reason = format!("inbox-{}", local.sequence);
    if !agent.checkpoint.wake_reasons.contains(&reason) {
        agent.checkpoint.wake_reasons.push(reason);
    }
    agent.summary.revision = next(agent.summary.revision)?;
    Ok(())
}
pub(super) fn prune_pending(state: &mut ScanState) {
    for a in &mut state.agents {
        a.checkpoint.pending_assignments.retain(|id| {
            state.assignments.iter().any(|v| {
                &v.id == id
                    && matches!(v.state, AssignmentState::Queued | AssignmentState::Assigned)
            })
        });
    }
}
pub(super) fn stop_branch(
    state: &mut ScanState,
    local: &mut LocalState,
    child: &str,
) -> Result<Vec<String>> {
    let mut ids = vec![child.to_owned()];
    let mut index = 0;
    while index < ids.len() {
        let parent = ids[index].clone();
        ids.extend(
            state
                .agents
                .iter()
                .filter(|a| a.summary.parent_id.as_ref() == Some(&parent))
                .map(|a| a.summary.id.clone()),
        );
        index += 1;
    }
    for id in &ids {
        handoffs::lifecycle(state, local, id, HandoffStatus::Canceled)?;
        local
            .controls
            .iter_mut()
            .find(|c| &c.agent_id == id)
            .context("missing local control")?
            .stop_requested = true;
        if !active(state, id) {
            let was_stopped = state.agent(id)?.summary.state == AgentState::Stopped;
            set_state(state, id, AgentState::Stopped)?;
            if !was_stopped && let Some(parent) = state.agent(id)?.summary.parent_id.clone() {
                enqueue_lifecycle(
                    state,
                    local,
                    &format!("cancel-{id}"),
                    id,
                    &parent,
                    "Child branch canceled; saved partial results remain available.",
                )?;
            }
        }
        for assignment in state.assignments.iter_mut().filter(|a| {
            &a.child_id == id
                && matches!(
                    a.state,
                    AssignmentState::Queued | AssignmentState::Assigned | AssignmentState::Blocked
                )
        }) {
            assignment.state = AssignmentState::Canceled;
        }
    }
    prune_pending(state);
    Ok(ids)
}

impl SwarmStore {
    pub fn allocation_blocked(&self, scan: &str, agent: &str, reason: &str) -> Result<()> {
        ensure!(
            !reason.is_empty() && reason.len() <= 2048,
            "invalid allocation diagnosis"
        );
        self.transaction(|tx| {
            let mut state = read_scan(tx, scan)?;
            let mut local = read_local(tx, scan)?;
            ensure!(
                !active(&state, agent),
                "running assignment cannot be allocation-blocked"
            );
            let control = local
                .controls
                .iter_mut()
                .find(|c| c.agent_id == agent)
                .context("unknown agent")?;
            if control.failures >= 3 || control.stop_requested {
                return Ok(());
            }
            control.failures = 3;
            set_state(&mut state, agent, AgentState::Blocked)?;
            handoffs::lifecycle(&mut state, &mut local, agent, HandoffStatus::Blocked)?;
            for assignment in state
                .assignments
                .iter_mut()
                .filter(|a| a.child_id == agent && a.state == AssignmentState::Queued)
            {
                assignment.state = AssignmentState::Blocked;
            }
            if let Some(parent) = state.agent(agent)?.summary.parent_id.clone() {
                enqueue_lifecycle(
                    &mut state,
                    &mut local,
                    &format!("allocation-blocked-{agent}"),
                    agent,
                    &parent,
                    reason,
                )?;
            }
            prune_pending(&mut state);
            write_local(tx, scan, &local)?;
            write_scan(tx, &mut state)
        })
    }
    pub fn local_initialize(
        &self,
        scan: &str,
        settings: LocalSettings,
        prompt: &str,
    ) -> Result<()> {
        self.initialize_hierarchy(scan, settings, prompt, false)
    }
    /// Initialize orchestration for explicitly selected execution destinations.
    /// This does not authorize a local subordinate executor.
    pub fn peer_initialize(&self, scan: &str, settings: LocalSettings, prompt: &str) -> Result<()> {
        self.initialize_hierarchy(scan, settings, prompt, true)
    }
    fn initialize_hierarchy(
        &self,
        scan: &str,
        settings: LocalSettings,
        prompt: &str,
        peer: bool,
    ) -> Result<()> {
        ensure!(
            !settings.models.is_empty() && settings.models.len() <= 64,
            "invalid model routes"
        );
        self.transaction(|tx| {
            let mut state = read_scan(tx, scan)?;
            ensure!(
                state.config.destinations.is_empty() != peer,
                "hierarchy execution selection mismatch"
            );
            let leader = state
                .agents
                .iter()
                .find(|a| a.summary.role == Role::Leader)
                .context("missing leader")?
                .summary
                .id
                .clone();
            for model in &settings.models {
                ensure!(
                    state.workflow()?.node(&model.profile).is_some()
                        && (1..=6).contains(&model.provider)
                        && (1..=2).contains(&model.method)
                        && !model.model.is_empty()
                        && model.model.len() <= 200,
                    "invalid model route"
                );
            }
            let mut local = LocalState {
                settings,
                controls: vec![control(&leader, false)],
                inbox: vec![],
                sequence: 0,
                handoff_inputs: vec![],
            };
            enqueue(
                &mut state, &mut local, "initial", "operator", &leader, prompt,
            )?;
            tx.execute(
                "INSERT INTO swarm_local(scan_id,document) VALUES(?1,?2)",
                params![scan, serde_json::to_string(&local)?],
            )?;
            write_scan(tx, &mut state)
        })
    }

    pub fn local_state(&self, scan: &str) -> Result<LocalState> {
        self.db.with_connection(|db| read_local(db, scan))
    }

    pub fn local_authorize(&self, turn: &LocalTurn) -> Result<()> {
        let state = self.load(&turn.scan_id, true)?;
        ensure_local_turn(&state, turn)?;
        ensure!(
            !self
                .local_state(&turn.scan_id)?
                .controls
                .iter()
                .any(|c| c.agent_id == turn.agent_id && c.stop_requested),
            "branch is stopping"
        );
        Ok(())
    }

    pub fn local_message(
        &self,
        scan: &str,
        operation: &str,
        agent: &str,
        message: &str,
    ) -> Result<()> {
        ensure!(valid_id(operation), "invalid operator message identity");
        self.transaction(|tx| {
            let content = json!({"agent":agent,"message":message});
            if receipt(tx, scan, "operator", operation, &content)?.is_some() {
                return Ok(());
            }
            let mut state = read_scan(tx, scan)?;
            ensure!(state.ended_at_ms.is_none(), "scan has ended");
            let mut local = read_local(tx, scan)?;
            let target = local
                .controls
                .iter_mut()
                .find(|c| c.agent_id == agent)
                .context("unknown agent")?;
            ensure!(!target.stop_requested, "agent intentionally stopped");
            target.failures = 0;
            handoffs::retry(&mut state, &mut local, agent)?;
            enqueue(
                &mut state, &mut local, operation, "operator", agent, message,
            )?;
            write_scan(tx, &mut state)?;
            write_local(tx, scan, &local)?;
            acknowledge(
                tx,
                scan,
                "operator",
                operation,
                &content,
                &json!({"queued":true}),
            )
        })
    }

    pub fn local_claim(
        &self,
        scan: &str,
        agent_id: &str,
        now: i64,
        capacity: usize,
    ) -> Result<Option<LocalTurn>> {
        self.local_claim_inner(scan, agent_id, now, capacity, None)
    }

    /// Production scheduling holds the owner key's OS lock before this call.
    /// The attempt and its recovery intent commit together, before any work.
    pub fn local_claim_owned(
        &self,
        scan: &str,
        agent_id: &str,
        now: i64,
        capacity: usize,
        owner_key: Uuid,
    ) -> Result<Option<LocalTurn>> {
        self.local_claim_inner(
            scan,
            agent_id,
            now,
            capacity,
            Some(ClaimOwner::Local(owner_key)),
        )
    }

    pub(super) fn local_claim_inner(
        &self,
        scan: &str,
        agent_id: &str,
        now: i64,
        capacity: usize,
        owner_key: Option<ClaimOwner<'_>>,
    ) -> Result<Option<LocalTurn>> {
        ensure!(now >= 0, "invalid clock");
        self.transaction(|tx| {
            let mut state = read_scan(tx, scan)?;
            let mut local = read_local(tx, scan)?;
            let c = local
                .controls
                .iter()
                .find(|c| c.agent_id == agent_id)
                .context("unknown local agent")?;
            if state.ended_at_ms.is_some()
                || c.stop_requested
                || c.failures >= 3
                || active(&state, agent_id)
                || state
                    .attempts
                    .iter()
                    .filter(|a| a.state != ExecutionState::Stopped)
                    .count()
                    >= capacity
            {
                return Ok(None);
            }
            if let Some(parent) = &c.release_parent
                && (active(&state, parent)
                    || !local
                        .controls
                        .iter()
                        .any(|c| &c.agent_id == parent && c.yield_committed))
            {
                return Ok(None);
            }
            let agent = state.agent(agent_id)?.clone();
            ensure!(
                agent.summary.role == Role::Leader
                    || state.config.destinations.is_empty()
                    || !matches!(owner_key, Some(ClaimOwner::Local(_))),
                "local subordinate cannot execute selected Fleet work"
            );
            let mut pending: Vec<_> = local
                .inbox
                .iter()
                .filter(|m| m.to == agent_id && m.sequence > agent.checkpoint.inbox_cursor)
                .take(3)
                .cloned()
                .collect();
            // Preserve each operator message as its own transcript turn. Earlier
            // internal deliveries may share its model input, but never swallow it.
            if let Some(index) = pending.iter().position(|m| m.from == "operator") {
                pending.truncate(index + 1);
            }
            if pending.is_empty() && agent.summary.state != AgentState::Queued {
                return Ok(None);
            }
            let assignment = state
                .assignments
                .iter()
                .rev()
                .find(|a| a.child_id == agent_id);
            // An unread follow-up supplements, rather than replaces, the first assignment.
            let initial =
                agent.summary.role != Role::Leader && agent.checkpoint.context == json!({});
            let render = |messages: &[InboxMessage]| -> Result<String> {
                let mut messages = messages.to_vec();
                if !c.retired_handoffs.is_empty() {
                    messages.insert(0, InboxMessage {
                        sequence: 0,
                        id: "handoff-custody".into(),
                        from: "harness".into(),
                        to: agent_id.into(),
                        message: format!("These handoffs are no longer assigned to you. Do not execute them from prior messages, assignment text, or retained context. Result and status notifications are informational. Retired handoffs: {}", serde_json::to_string(&c.retired_handoffs)?),
                        handoff: None,
                        handoff_result: None,
                    });
                }
                Ok(if messages.len() == 1 && messages[0].from == "operator" {
                    messages[0].message.clone()
                } else if initial {
                    serde_json::to_string(&json!({
                        "assignment": assignment.context("agent has no queued input")?.task,
                        "messages": messages,
                    }))?
                } else if messages.is_empty() {
                    serde_json::to_string(&assignment.context("agent has no queued input")?.task)?
                } else {
                    serde_json::to_string(&messages)?
                })
            };
            let mut prompt = render(&pending)?;
            // Whole typed handoffs can be much larger than plain messages. Leave excess
            // mail unread for the next wake instead of making the turn unexecutable.
            while prompt.len() > 262_144 && pending.len() > 1 {
                pending.pop();
                prompt = render(&pending)?;
            }
            ensure!(prompt.len() <= 262_144, "local prompt exceeds input limit");
            let attempt = ExecutionAttempt {
                id: new_id(),
                agent_id: agent_id.into(),
                generation: next(
                    state
                        .attempts
                        .iter()
                        .filter(|a| a.agent_id == agent_id)
                        .map(|a| a.generation)
                        .max()
                        .unwrap_or(0),
                )?,
                expires_at_ms: now.checked_add(900_000).context("clock overflow")?,
                state: ExecutionState::Running,
            };
            let turn = LocalTurn {
                scan_id: scan.into(),
                agent_id: agent_id.into(),
                attempt_id: attempt.id.clone(),
                generation: attempt.generation,
                session_id: if agent.summary.role == Role::Leader {
                    scan.into()
                } else {
                    agent_id.into()
                },
                inbox_cursor: pending
                    .last()
                    .map_or(agent.checkpoint.inbox_cursor, |m| m.sequence),
                prompt,
                operator_text: pending.iter().find(|m| m.from == "operator").map(|m| m.message.clone()),
                profile: agent.summary.profile.id,
                role: agent.summary.role,
            };
            state.attempts.push(attempt);
            set_state(&mut state, agent_id, AgentState::Running)?;
            for assignment in state
                .assignments
                .iter_mut()
                .filter(|a| a.child_id == agent_id && a.state == AssignmentState::Queued)
            {
                assignment.state = AssignmentState::Assigned;
            }
            let c = local
                .controls
                .iter_mut()
                .find(|c| c.agent_id == agent_id)
                .context("missing control")?;
            c.yield_committed = false;
            c.release_parent = None;
            write_scan(tx, &mut state)?;
            write_local(tx, scan, &local)?;
            match owner_key {
                Some(ClaimOwner::Local(key)) if turn.role == Role::Leader => {
                    coordinators::register_in(tx, &turn, key)?
                }
                Some(ClaimOwner::Local(key)) => local_workers::register_in(tx, &turn, key)?,
                Some(ClaimOwner::Peer(key, placement)) => {
                    peer_workers::register_in(tx, &turn, key, placement)?
                }
                None => (),
            }
            Ok(Some(turn))
        })
    }

    pub fn local_heartbeat(&self, turn: &LocalTurn, now: i64) -> Result<()> {
        self.transaction(|tx| {
            let mut state = read_scan(tx, &turn.scan_id)?;
            ensure_local_turn(&state, turn)?;
            state
                .attempts
                .iter_mut()
                .find(|a| a.id == turn.attempt_id)
                .context("unknown attempt")?
                .expires_at_ms = now.checked_add(900_000).context("clock overflow")?;
            write_scan(tx, &mut state)
        })
    }

    pub fn local_yield_requested(&self, turn: &LocalTurn) -> Result<bool> {
        let local = self.local_state(&turn.scan_id)?;
        Ok(local
            .controls
            .iter()
            .find(|c| c.agent_id == turn.agent_id)
            .context("missing control")?
            .yield_requested)
    }

    /// Only called once inference and all managed tool execution have finished.
    /// The transcript checkpoint and capacity release commit together.
    pub fn local_finish(
        &self,
        turn: &LocalTurn,
        context: Value,
        answer: &str,
        stopped: bool,
    ) -> Result<()> {
        self.transaction(|tx| finish_turn(tx, turn, context, answer, stopped, false))
    }

    /// A retryable executor failure preserves accepted results and unread mail.
    pub fn local_failed(&self, turn: &LocalTurn) -> Result<()> {
        self.local_failure(turn, None)
    }

    /// Exact checkpoint retries exhausted. Re-inference could repeat accepted side
    /// effects, so block this branch (or stop a broken leader) without replaying it.
    pub fn local_checkpoint_failed(&self, turn: &LocalTurn) -> Result<()> {
        self.local_failure(turn, Some("checkpoint"))
    }

    /// Provider recovery exhausted inside the same tool exchange. Do not start
    /// another execution that might repeat its already accepted side effects.
    pub fn local_recovery_exhausted(&self, turn: &LocalTurn) -> Result<()> {
        self.local_failure(turn, Some("provider"))
    }

    fn local_failure(&self, turn: &LocalTurn, exhausted_at: Option<&str>) -> Result<()> {
        self.transaction(|tx| fail_turn(tx, turn, exhausted_at, false))
    }

    pub fn local_stop(&self, scan: &str, root: &str) -> Result<Vec<String>> {
        self.transaction(|tx| {
            let mut state = read_scan(tx, scan)?;
            let mut local = read_local(tx, scan)?;
            let ids = stop_branch(&mut state, &mut local, root)?;
            write_scan(tx, &mut state)?;
            write_local(tx, scan, &local)?;
            Ok(ids)
        })
    }

    pub fn local_dispatch(
        &self,
        turn: &LocalTurn,
        operation: &str,
        command: SwarmCommand,
        admission: Admission,
    ) -> Result<Value> {
        self.transaction(|tx| dispatch_command(tx, turn, operation, command, admission))
    }

    /// Worker authority, hierarchy changes and both retry receipts share one
    /// transaction. Role and parentage are resolved from durable state.
    pub fn worker_dispatch(
        &self,
        bearer: &str,
        request: &WorkerRequest,
        now_ms: i64,
        admission: Admission,
    ) -> Result<MutationAcknowledgement> {
        let result = self.transaction(|tx| {
            let caller = authorize(tx, bearer, WORKER_AUDIENCE, request, now_ms)?;
            let content = serde_json::to_value(&request.operation)?;
            if let Some(saved) = receipt(
                tx,
                &request.scan_id,
                &request.agent_id,
                &request.operation_id,
                &content,
            )? {
                return Ok(saved);
            }
            let command = match &request.operation {
                WorkerOperation::Canonical { tool, arguments } => {
                    beale_contracts::tool_error::arguments(
                        json!({"tool":tool,"arguments":arguments}),
                    )?
                }
                WorkerOperation::Spawn { profile, task } => {
                    SwarmCommand::Spawn(beale_contracts::swarm_local::Spawn {
                        handoff_id: None,
                        profile: profile.clone(),
                        task: task.clone(),
                        independent: false,
                    })
                }
                WorkerOperation::MessageChild { child_id, message } => {
                    SwarmCommand::MessageChild(beale_contracts::swarm_local::MessageChild {
                        child_id: child_id.clone(),
                        message: message.clone(),
                    })
                }
                WorkerOperation::MessageParent { message } => SwarmCommand::MessageParent(
                    Box::new(beale_contracts::swarm_local::MessageParent {
                        message: message.clone(),
                        handoff: None,
                    }),
                ),
                WorkerOperation::StopChild { child_id } => {
                    SwarmCommand::Stop(beale_contracts::swarm_local::Stop {
                        child_id: child_id.clone(),
                    })
                }
                _ => anyhow::bail!("not a hierarchy operation"),
            };
            let turn = LocalTurn {
                scan_id: request.scan_id.clone(),
                agent_id: request.agent_id.clone(),
                attempt_id: request.attempt_id.clone(),
                generation: request.generation,
                session_id: if caller.agent.role == Role::Leader {
                    request.scan_id.clone()
                } else {
                    request.agent_id.clone()
                },
                inbox_cursor: 0,
                prompt: String::new(),
                operator_text: None,
                profile: caller.agent.profile.id,
                role: caller.agent.role,
            };
            let result = dispatch_command(tx, &turn, &request.operation_id, command, admission)?;
            acknowledge(
                tx,
                &request.scan_id,
                &request.agent_id,
                &request.operation_id,
                &content,
                &result,
            )?;
            Ok(result)
        })?;
        Ok(MutationAcknowledgement {
            version: VERSION,
            operation_id: request.operation_id.clone(),
            result,
        })
    }
}

fn dispatch_command(
    tx: &Transaction<'_>,
    turn: &LocalTurn,
    operation: &str,
    command: SwarmCommand,
    admission: Admission,
) -> Result<Value> {
    use beale_contracts::tool_error::ToolError;
    command.validate().map_err(ToolError::invalid)?;
    ensure!(valid_id(operation), "invalid tool operation identity");
    let mut state = read_scan(tx, &turn.scan_id)?;
    ensure_local_turn(&state, turn)?;
    let mut local = read_local(tx, &turn.scan_id)?;
    ensure!(
        !local
            .controls
            .iter()
            .any(|c| c.agent_id == turn.agent_id && c.stop_requested),
        "branch is stopping"
    );
    let actor = state.agent(&turn.agent_id)?.summary.clone();
    let content = serde_json::to_value(&command)?;
    let receipt_actor = format!("local-{}", turn.agent_id);
    if let Some(saved) = receipt(tx, &turn.scan_id, &receipt_actor, operation, &content)? {
        return Ok(saved);
    }
    let result = match command {
        SwarmCommand::Handoff(action) => {
            handoffs::dispatch(tx, &mut state, &mut local, &actor, operation, *action)?
        }
        SwarmCommand::List(_) => {
            ensure!(
                actor.role != Role::Worker,
                ToolError::rejected("workers cannot list descendants")
            );
            let mut ids = vec![actor.id.clone()];
            for _ in 0..2 {
                let next: Vec<_> = state
                    .agents
                    .iter()
                    .filter(|a| {
                        a.summary
                            .parent_id
                            .as_ref()
                            .is_some_and(|p| ids.contains(p))
                            && !ids.contains(&a.summary.id)
                    })
                    .map(|a| a.summary.id.clone())
                    .collect();
                ids.extend(next);
            }
            serde_json::to_value(
                state
                    .agents
                    .iter()
                    .filter(|a| a.summary.id != actor.id && ids.contains(&a.summary.id))
                    .map(|a| &a.summary)
                    .collect::<Vec<_>>(),
            )?
        }
        SwarmCommand::Spawn(spawn) => {
            let profile = state
                .config
                .profile(&spawn.profile)
                .context(ToolError::missing("unknown saved profile"))?;
            ensure!(
                actor.role.child() == Some(profile.role)
                    && state
                        .workflow()?
                        .permits(&actor.profile.id, &spawn.profile.id),
                ToolError::rejected("spawn is outside permitted delegation")
            );
            if let Admission::Denied { reason } | Admission::Waiting { reason } = admission {
                serde_json::to_value(SpawnResult::Denied { reason })?
            } else {
                ensure!(
                    state.agents.len() < 1024,
                    ToolError::rejected("scan agent safeguard reached")
                );
                let role = profile.role;
                let child = new_id();
                let assignment = new_id();
                let yielding = matches!(admission, Admission::YieldParent { .. });
                let wait_reason = match admission {
                    Admission::YieldParent { reason } => reason,
                    _ => "Awaiting local execution admission".into(),
                };
                ensure!(
                    !wait_reason.is_empty() && wait_reason.len() <= 4096,
                    "invalid admission reason"
                );
                let mut child_control = control(
                    &child,
                    spawn.independent || spawn.profile.id == "challenger",
                );
                if yielding {
                    ensure!(
                        actor.role == Role::Deputy,
                        "only a deputy may yield for its first child"
                    );
                    child_control.release_parent = Some(actor.id.clone());
                    local
                        .controls
                        .iter_mut()
                        .find(|c| c.agent_id == actor.id)
                        .context("missing parent control")?
                        .yield_requested = true;
                }
                local.controls.push(child_control);
                state.agents.push(AgentRecord {
                    summary: AgentSummary {
                        id: child.clone(),
                        parent_id: Some(actor.id.clone()),
                        profile: spawn.profile,
                        role,
                        state: AgentState::Queued,
                        revision: 1,
                    },
                    checkpoint: AgentCheckpoint {
                        state: AgentState::Queued,
                        context: json!({}),
                        inbox_cursor: 0,
                        wake_reasons: vec![],
                        pending_assignments: vec![],
                        environment_id: None,
                    },
                });
                state.assignments.push(Assignment {
                    id: assignment.clone(),
                    parent_id: actor.id.clone(),
                    child_id: child.clone(),
                    task: spawn.task,
                    state: AssignmentState::Queued,
                    wait_reason: wait_reason.clone(),
                });
                state
                    .agents
                    .iter_mut()
                    .find(|a| a.summary.id == actor.id)
                    .context("missing parent")?
                    .checkpoint
                    .pending_assignments
                    .push(assignment.clone());
                if let Some(id) = spawn.handoff_id {
                    handoffs::dispatch(
                        tx,
                        &mut state,
                        &mut local,
                        &actor,
                        operation,
                        beale_contracts::swarm_local::HandoffAction::Forward {
                            id,
                            to: child.clone(),
                        },
                    )?;
                }
                serde_json::to_value(SpawnResult::Queued {
                    child_id: child,
                    assignment_id: assignment,
                    wait_reason,
                })?
            }
        }
        SwarmCommand::MessageChild(message) => {
            let target = state
                .agents
                .iter()
                .find(|a| a.summary.id == message.child_id)
                .context(ToolError::missing("child not found"))?;
            ensure!(
                target.summary.parent_id.as_ref() == Some(&actor.id),
                ToolError::rejected("not a direct child")
            );
            let child = local
                .controls
                .iter_mut()
                .find(|c| c.agent_id == message.child_id)
                .context("missing child control")?;
            ensure!(
                !child.stop_requested,
                ToolError::rejected("child intentionally stopped")
            );
            ensure!(
                !child.independent
                    || state.agent(&message.child_id)?.summary.state != AgentState::Completed,
                ToolError::rejected("start a fresh independent reviewer")
            );
            child.failures = 0;
            handoffs::retry(&mut state, &mut local, &message.child_id)?;
            if matches!(
                state.agent(&message.child_id)?.summary.state,
                AgentState::Completed | AgentState::Blocked
            ) {
                let previous = state
                    .assignments
                    .iter()
                    .rev()
                    .find(|a| a.child_id == message.child_id)
                    .context("missing prior assignment")?;
                let mut task = previous.task.clone();
                task.question = message.message.clone();
                let id = new_id();
                state.assignments.push(Assignment {
                    id: id.clone(),
                    parent_id: actor.id.clone(),
                    child_id: message.child_id.clone(),
                    task,
                    state: AssignmentState::Queued,
                    wait_reason: "Awaiting resumed execution".into(),
                });
                state
                    .agents
                    .iter_mut()
                    .find(|a| a.summary.id == actor.id)
                    .context("missing parent")?
                    .checkpoint
                    .pending_assignments
                    .push(id);
            }
            enqueue(
                &mut state,
                &mut local,
                operation,
                &actor.id,
                &message.child_id,
                &message.message,
            )?;
            json!({"queued":true,"child_id":message.child_id})
        }
        SwarmCommand::Stop(stop) => {
            let target = state
                .agents
                .iter()
                .find(|a| a.summary.id == stop.child_id)
                .context(ToolError::missing("child not found"))?;
            ensure!(
                target.summary.parent_id.as_ref() == Some(&actor.id),
                ToolError::rejected("not a direct child")
            );
            let ids = stop_branch(&mut state, &mut local, &stop.child_id)?;
            json!({"stopping":ids})
        }
        SwarmCommand::MessageParent(message) => {
            let parent = actor
                .parent_id
                .context(ToolError::rejected("leader has no parent"))?;
            if let Some(handoff) = &message.handoff {
                ensure!(
                    handoff.from_agent == actor.id && handoff.parent_id == parent,
                    ToolError::rejected("handoff source mismatch")
                );
                state.handoff_path(handoff)?;
                crate::swarm_files::validate_record_artifacts(
                    tx,
                    &turn.scan_id,
                    &state.config.workspace_id,
                    &serde_json::to_value(handoff)?,
                )?;
                if let Some(existing) = state.handoffs.iter().find(|h| h.request.id == handoff.id) {
                    ensure!(
                        existing.request == *handoff,
                        ToolError::conflict("handoff identity reused")
                    );
                } else {
                    state.handoffs.push(WorkflowHandoff {
                        request: handoff.clone(),
                        revision: 1,
                        status: HandoffStatus::Pending,
                        assignment_context: None,
                        result: None,
                    });
                }
            }
            enqueue(
                &mut state,
                &mut local,
                operation,
                &actor.id,
                &parent,
                &message.message,
            )?;
            local
                .inbox
                .last_mut()
                .context("missing handoff message")?
                .handoff = message.handoff;
            if let Some(h) = local.inbox.last().and_then(|m| m.handoff.as_ref())
                && state.handoffs.iter().any(|saved| {
                    saved.request.id == h.id
                        && saved.assignment_context.is_none()
                        && saved.status == HandoffStatus::Pending
                })
            {
                local.handoff_inputs.push(local.sequence);
            }
            json!({"queued":true,"parent_id":parent})
        }
    };
    // The accepted result is durable before the executor reaches its safe yield checkpoint.
    local
        .controls
        .iter_mut()
        .find(|c| c.agent_id == actor.id)
        .context("missing control")?
        .tool_results[operation] = result.clone();
    write_scan(tx, &mut state)?;
    write_local(tx, &turn.scan_id, &local)?;
    acknowledge(
        tx,
        &turn.scan_id,
        &receipt_actor,
        operation,
        &content,
        &result,
    )?;
    Ok(result)
}

pub(super) fn ensure_local_turn(state: &ScanState, turn: &LocalTurn) -> Result<()> {
    ensure_execution_turn(state, turn, false)
}

fn ensure_execution_turn(state: &ScanState, turn: &LocalTurn, allow_fenced: bool) -> Result<()> {
    ensure!(
        state.ended_at_ms.is_none() && state.config.scan_id == turn.scan_id,
        "scan is not active"
    );
    ensure!(
        state.attempts.iter().any(|a| a.id == turn.attempt_id
            && a.agent_id == turn.agent_id
            && a.generation == turn.generation
            && (a.state == ExecutionState::Running
                || (allow_fenced && a.state == ExecutionState::TerminationUnconfirmed))),
        "stale local execution"
    );
    let agent = state.agent(&turn.agent_id)?;
    ensure!(
        agent.summary.role == turn.role && agent.summary.profile.id == turn.profile,
        "invalid local identity"
    );
    Ok(())
}

pub(super) fn finish_turn(
    tx: &Transaction<'_>,
    turn: &LocalTurn,
    context: Value,
    answer: &str,
    stopped: bool,
    allow_fenced: bool,
) -> Result<()> {
    ensure!(bounded_json(&context), "checkpoint too large");
    let mut state = read_scan(tx, &turn.scan_id)?;
    ensure_execution_turn(&state, turn, allow_fenced)?;
    let mut local = read_local(tx, &turn.scan_id)?;
    let control = local
        .controls
        .iter()
        .find(|c| c.agent_id == turn.agent_id)
        .context("missing control")?;
    let stop = stopped || control.stop_requested;
    let pending: Vec<_> = state
        .assignments
        .iter()
        .filter(|a| {
            a.parent_id == turn.agent_id
                && matches!(a.state, AssignmentState::Queued | AssignmentState::Assigned)
        })
        .map(|a| a.id.clone())
        .collect();
    let unread = local
        .inbox
        .iter()
        .any(|m| m.to == turn.agent_id && m.sequence > turn.inbox_cursor);
    let status = if stop {
        AgentState::Stopped
    } else if unread {
        AgentState::Queued
    } else if !pending.is_empty() || control.yield_requested {
        AgentState::Suspended
    } else {
        AgentState::Completed
    };
    let agent = state
        .agents
        .iter_mut()
        .find(|a| a.summary.id == turn.agent_id)
        .context("unknown agent")?;
    agent.checkpoint.context = context;
    agent.checkpoint.inbox_cursor = turn.inbox_cursor;
    agent.checkpoint.pending_assignments = pending;
    agent.checkpoint.wake_reasons = local
        .inbox
        .iter()
        .filter(|m| m.to == turn.agent_id && m.sequence > turn.inbox_cursor)
        .map(|m| format!("inbox-{}", m.sequence))
        .collect();
    set_state(&mut state, &turn.agent_id, status)?;
    state
        .attempts
        .iter_mut()
        .find(|a| a.id == turn.attempt_id)
        .context("unknown attempt")?
        .state = ExecutionState::Stopped;
    for instance in state
        .instances
        .iter_mut()
        .filter(|i| i.attempt_id == turn.attempt_id)
    {
        instance.state = InstanceState::Stopped;
    }
    local
        .inbox
        .retain(|m| m.to != turn.agent_id || m.sequence > turn.inbox_cursor);
    local
        .handoff_inputs
        .retain(|sequence| local.inbox.iter().any(|m| m.sequence == *sequence));
    let c = local
        .controls
        .iter_mut()
        .find(|c| c.agent_id == turn.agent_id)
        .context("missing control")?;
    c.yield_committed = !stop && c.yield_requested;
    let committed_yield = c.yield_committed;
    c.yield_requested = false;
    c.failures = 0;
    if committed_yield {
        for dependent in &mut local.controls {
            if dependent.release_parent.as_deref() == Some(turn.agent_id.as_str()) {
                dependent.release_parent = None;
            }
        }
    }
    if matches!(status, AgentState::Completed | AgentState::Stopped) {
        // Finishing inference is not a supported assessment. An unresolved owned
        // handoff remains recoverable instead of being falsely completed.
        handoffs::lifecycle(
            &mut state,
            &mut local,
            &turn.agent_id,
            if stop {
                HandoffStatus::Canceled
            } else {
                HandoffStatus::Blocked
            },
        )?;
        for assignment in state.assignments.iter_mut().filter(|a| {
            a.child_id == turn.agent_id
                && matches!(a.state, AssignmentState::Queued | AssignmentState::Assigned)
        }) {
            assignment.state = if stop {
                AssignmentState::Canceled
            } else {
                AssignmentState::Completed
            };
        }
        if let Some(parent) = state.agent(&turn.agent_id)?.summary.parent_id.clone() {
            let prefix = format!(
                "Child {} {}. Result: ",
                turn.agent_id,
                if stop { "stopped" } else { "completed" },
            );
            let end = answer.floor_char_boundary(16384 - prefix.len());
            let message = format!("{prefix}{}", &answer[..end]);
            enqueue_lifecycle(
                &mut state,
                &mut local,
                &format!("result-{}", turn.attempt_id),
                &turn.agent_id,
                &parent,
                &message,
            )?;
        }
    }
    prune_pending(&mut state);
    write_scan(tx, &mut state)?;
    write_local(tx, &turn.scan_id, &local)
}

pub(super) fn fail_turn(
    tx: &Transaction<'_>,
    turn: &LocalTurn,
    exhausted_at: Option<&str>,
    allow_fenced: bool,
) -> Result<()> {
    let mut state = read_scan(tx, &turn.scan_id)?;
    ensure_execution_turn(&state, turn, allow_fenced)?;
    let mut local = read_local(tx, &turn.scan_id)?;
    let c = local
        .controls
        .iter_mut()
        .find(|c| c.agent_id == turn.agent_id)
        .context("missing control")?;
    c.failures = if exhausted_at.is_some() {
        3
    } else {
        c.failures + 1
    };
    c.yield_committed = false;
    let exhausted = c.failures >= 3;
    state
        .attempts
        .iter_mut()
        .find(|a| a.id == turn.attempt_id)
        .context("unknown attempt")?
        .state = ExecutionState::Stopped;
    for instance in state
        .instances
        .iter_mut()
        .filter(|i| i.attempt_id == turn.attempt_id)
    {
        instance.state = InstanceState::Stopped;
    }
    set_state(
        &mut state,
        &turn.agent_id,
        if exhausted {
            AgentState::Blocked
        } else {
            AgentState::Queued
        },
    )?;
    let sequence = next(state.events.last().map_or(0, |e| e.sequence))?;
    state.events.push(LifecycleEvent { id: new_id(), agent_id: turn.agent_id.clone(), sequence, kind: "local-recovery".into(), payload: json!({"warning":"Local execution requires bounded recovery","attempts":c.failures,"checkpoint_exhausted":exhausted_at == Some("checkpoint"),"provider_exhausted":exhausted_at == Some("provider"),"worker_exhausted":exhausted_at == Some("worker")}) });
    if exhausted && turn.role == Role::Leader {
        stop_branch(&mut state, &mut local, &turn.agent_id)?;
    }
    if exhausted && turn.role != Role::Leader {
        handoffs::lifecycle(
            &mut state,
            &mut local,
            &turn.agent_id,
            HandoffStatus::Blocked,
        )?;
        for a in state.assignments.iter_mut().filter(|a| {
            a.child_id == turn.agent_id
                && matches!(a.state, AssignmentState::Assigned | AssignmentState::Queued)
        }) {
            a.state = AssignmentState::Blocked;
        }
        if let Some(parent) = state.agent(&turn.agent_id)?.summary.parent_id.clone() {
            enqueue_lifecycle(
                &mut state,
                &mut local,
                &format!("blocked-{}", turn.attempt_id),
                &turn.agent_id,
                &parent,
                if exhausted_at == Some("worker") {
                    "Assigned child execution stopped; partial results and recovery state are retained. Reconcile uncertain effects before retrying or replacing this branch."
                } else {
                    "Child recovery exhausted three attempts; partial results are retained. Decide whether to retry or stop this branch."
                },
            )?;
        }
        prune_pending(&mut state);
    }
    write_scan(tx, &mut state)?;
    write_local(tx, &turn.scan_id, &local)
}

/// A late artifact is candidate evidence for the original parent, not a worker
/// completion. Notification, artifact acceptance and recovery receipt are atomic.
pub(super) fn recovered_artifact_notice(
    tx: &Transaction<'_>,
    state: &mut ScanState,
    agent: &str,
    operation: &str,
    message: &str,
) -> Result<()> {
    if let Some(parent) = state.agent(agent)?.summary.parent_id.clone() {
        let scan = state.config.scan_id.clone();
        let mut local = read_local(tx, &scan)?;
        enqueue_lifecycle(state, &mut local, operation, agent, &parent, message)?;
        write_local(tx, &scan, &local)?;
    }
    Ok(())
}
