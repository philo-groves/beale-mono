//! Local admission seam and model-facing hierarchy tools. Fleet owns physical placement later.
use beale_contracts::{
    swarm_local::{Admission, SwarmCommand},
    swarms::{ExecutionState, Role},
};
use beale_storage::swarm_state::{LocalTurn, ScanState, SwarmStore};
use serde_json::{Value, json};

pub trait AdmissionAdapter {
    fn capacity(&self) -> usize;
    fn spawn(&self, state: &ScanState, caller: &LocalTurn) -> Admission;
}
pub struct LocalAdmission {
    pub capacity: usize,
}
impl AdmissionAdapter for LocalAdmission {
    fn capacity(&self) -> usize {
        self.capacity
    }
    fn spawn(&self, state: &ScanState, caller: &LocalTurn) -> Admission {
        if self.capacity == 0 {
            return Admission::Denied {
                reason: "No eligible local capacity".into(),
            };
        }
        let occupied = state
            .attempts
            .iter()
            .filter(|a| a.state != ExecutionState::Stopped)
            .count();
        if occupied < self.capacity {
            Admission::Ready
        } else if caller.role == Role::Deputy
            && occupied == self.capacity
            && state
                .attempts
                .iter()
                .any(|a| a.agent_id == caller.agent_id && a.state == ExecutionState::Running)
        {
            Admission::YieldParent {
                reason:
                    "Queued until the deputy commits a safe checkpoint and yields local capacity"
                        .into(),
            }
        } else {
            Admission::Denied {
                reason: "Local capacity is unavailable; defer until a child result arrives".into(),
            }
        }
    }
}
pub fn dispatch(
    store: &SwarmStore,
    caller: &LocalTurn,
    operation: &str,
    name: &str,
    arguments: Value,
    admission: &dyn AdmissionAdapter,
) -> anyhow::Result<Value> {
    let command: SwarmCommand = serde_json::from_value(json!({"tool":name,"arguments":arguments}))?;
    let decision = if matches!(command, SwarmCommand::Spawn(_)) {
        admission.spawn(&store.load(&caller.scan_id, true)?, caller)
    } else {
        Admission::Ready
    };
    store.local_dispatch(caller, operation, command, decision)
}
pub fn definitions(role: Role) -> Vec<Value> {
    let string = json!({"type":"string"});
    let strings = json!({"type":"array","items":{"type":"string"}});
    let mut definitions = Vec::new();
    let mut tool = |name: &str, description: &str, properties: Value, required: &[&str]| {
        definitions.push(json!({"type":"function","name":name,"description":description,
            "parameters":{"type":"object","properties":properties,"required":required,"additionalProperties":false}}));
    };
    tool(
        "swarm_handoff",
        "Operate on an accepted handoff by ID, preserving its full canonical envelope. Forward to your immediate parent on the source side, or to a direct child on the saved destination route. Spawn a child first when needed; use a fresh independent reviewer. Complete only your owned handoff with a typed WorkResult and accepted evidence. Block preserves incomplete work; cancel ends it. Never copy a full envelope into a message or task question.",
        json!({"action":{"type":"string","enum":["forward","complete","block","cancel"]},"id":string,"to":{"type":"string","description":"Required only for forward: immediate parent or direct child ID."},"result":{"type":"object","description":"Required only for complete: WorkResult with completion=complete, assessment (supported/contradicted/inconclusive/not_applicable), summary, observations[], evidence_refs[], artifact_refs[], unresolved_questions[]."}}),
        &["action", "id"],
    );
    tool(
        "swarm_materialize_artifact",
        "Copy an accepted central artifact into a new workspace-relative path. Never overwrites existing files. Attach local files to findings or handoff artifact fields using file:relative/path; acceptance stores bytes and provenance, not vulnerability verification.",
        json!({"artifact":string,"destination":string}),
        &["artifact", "destination"],
    );
    if role != Role::Worker {
        tool(
            "swarm_list_subagents",
            "List descendant identities and status. Only direct children can be controlled.",
            json!({}),
            &[],
        );
        tool(
            "swarm_spawn_subagent",
            "Queue a direct child using a saved permitted profile. Returns immediately; do not poll while waiting. Independent work starts fresh; no parent conversation is copied.",
            json!({
                "profile":{"type":"object","properties":{"id":string,"revision":{"type":"integer"}},"required":["id","revision"],"additionalProperties":false},
                "task":{"type":"object","properties":{"question":string,"completion_criteria":strings,"evidence_refs":strings},"required":["question","completion_criteria","evidence_refs"],"additionalProperties":false},
                "independent":{"type":"boolean"},
                "handoff_id":{"type":["string","null"],"description":"Optional owned canonical handoff to assign atomically before the child starts. Prefer this for fresh independent recipients."}
            }),
            &["profile", "task", "independent"],
        );
        tool(
            "swarm_stop_subagent",
            "Intentionally stop a direct child's branch while retaining partial results.",
            json!({"child_id":string}),
            &["child_id"],
        );
        tool(
            "swarm_message_subagent",
            "Queue related follow-up work for a direct child, preserving its identity and context. Use a fresh spawn for independent review. Results wake you automatically.",
            json!({"child_id":string,"message":string}),
            &["child_id", "message"],
        );
    }
    if role != Role::Leader {
        tool(
            "swarm_message_parent",
            "Send observations or requests to your immediate parent. Optional handoff follows a saved route; never address siblings directly.",
            json!({"message":string,"handoff":{"type":["object","null"],"description":"Optional HandoffRequest: id, from_agent, parent_id, route_id, destination_profile {id,revision}, task {question,completion_criteria,evidence_refs}, scope[], record_revisions[{id,revision}], artifact_refs[], observations[], unresolved_questions[], payload {kind,...route-specific fields}. Use null for an ordinary parent message."}}),
            &["message", "handoff"],
        );
    }
    definitions
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tools_follow_host_roles_without_legacy_budgets() {
        assert_eq!(definitions(Role::Leader).len(), 6);
        assert_eq!(definitions(Role::Deputy).len(), 7);
        assert_eq!(definitions(Role::Worker).len(), 3);
        for role in [Role::Leader, Role::Deputy, Role::Worker] {
            let tools = definitions(role);
            assert_eq!(
                tools.iter().any(|t| t["name"] == "swarm_message_parent"),
                role != Role::Leader
            );
            assert_eq!(
                tools.iter().any(|t| t["name"] == "swarm_spawn_subagent"),
                role != Role::Worker
            );
            let wire = serde_json::to_string(&tools).unwrap();
            assert!(!wire.contains("max_turns") && !wire.contains("token_budget"));
        }
    }
}
