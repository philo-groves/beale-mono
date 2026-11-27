//! Local hierarchy tool and admission contracts. Host identity is never supplied by a model.
use crate::{
    swarm_workflows::{HandoffRequest, WorkResult},
    swarms::*,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tool", content = "arguments", deny_unknown_fields)]
pub enum SwarmCommand {
    #[serde(rename = "swarm_list_subagents")]
    List(Empty),
    #[serde(rename = "swarm_spawn_subagent")]
    Spawn(Spawn),
    #[serde(rename = "swarm_message_subagent")]
    MessageChild(MessageChild),
    #[serde(rename = "swarm_stop_subagent")]
    Stop(Stop),
    #[serde(rename = "swarm_message_parent")]
    MessageParent(Box<MessageParent>),
    #[serde(rename = "swarm_handoff")]
    Handoff(Box<HandoffAction>),
}
/// Forward the canonical envelope by identity; never embed it in a chat message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum HandoffAction {
    Forward { id: String, to: String },
    Complete { id: String, result: WorkResult },
    Block { id: String },
    Cancel { id: String },
}
impl HandoffAction {
    pub fn id(&self) -> &str {
        match self {
            Self::Forward { id, .. }
            | Self::Complete { id, .. }
            | Self::Block { id }
            | Self::Cancel { id } => id,
        }
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if !valid_id(self.id()) {
            return Err("invalid handoff identity");
        }
        match self {
            Self::Forward { to, .. } if !valid_id(to) => Err("invalid handoff recipient"),
            Self::Complete { result, .. } => {
                if result.completion != crate::swarm_workflows::Completion::Complete {
                    return Err("use block for incomplete handoff work");
                }
                result.validate()
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod handoff_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn handoff_commands_reject_ambiguous_actions_and_invalid_completion() {
        for arguments in [
            json!({"action":"forward","id":"handoff-example"}),
            json!({"action":"cancel","id":"handoff-example","to":"child-example"}),
            json!({"action":"complete","id":"handoff-example"}),
            json!({"action":"assign","id":"handoff-example","to":"child-example"}),
        ] {
            assert!(
                serde_json::from_value::<SwarmCommand>(
                    json!({"tool":"swarm_handoff","arguments":arguments})
                )
                .is_err()
            );
        }
        for action in ["forward", "block", "cancel"] {
            let mut arguments = json!({"action":action,"id":"handoff-example"});
            if action == "forward" {
                arguments["to"] = json!("child-example");
            }
            let command: SwarmCommand =
                serde_json::from_value(json!({"tool":"swarm_handoff","arguments":arguments}))
                    .unwrap();
            assert!(command.validate().is_ok());
        }
        let result = json!({"completion":"blocked","assessment":"inconclusive","summary":"Synthetic incomplete work","observations":[],"evidence_refs":[],"artifact_refs":[],"unresolved_questions":[]});
        let command:SwarmCommand=serde_json::from_value(json!({"tool":"swarm_handoff","arguments":{"action":"complete","id":"handoff-example","result":result}})).unwrap();
        assert!(command.validate().is_err());
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Empty {}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spawn {
    /// Assign an owned canonical handoff atomically with child creation.
    pub handoff_id: Option<String>,
    pub profile: ProfileRef,
    pub task: Task,
    /// Independent work always creates a new identity with evidence-only context.
    pub independent: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageChild {
    pub child_id: String,
    pub message: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stop {
    pub child_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageParent {
    pub message: String,
    pub handoff: Option<HandoffRequest>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "admission", rename_all = "snake_case", deny_unknown_fields)]
pub enum Admission {
    Ready,
    YieldParent { reason: String },
    Waiting { reason: String },
    Denied { reason: String },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboxMessage {
    pub sequence: u64,
    pub id: String,
    pub from: String,
    pub to: String,
    pub message: String,
    pub handoff: Option<HandoffRequest>,
    pub handoff_result: Option<WorkResult>,
}
impl SwarmCommand {
    pub fn validate(&self) -> Result<(), &'static str> {
        if serde_json::to_vec(self).map_or(true, |bytes| bytes.len() > 65536) {
            return Err("swarm command exceeds input limit");
        }
        let message = |s: &str| !s.trim().is_empty() && s.len() <= 16384;
        match self {
            Self::List(_) => Ok(()),
            Self::Handoff(action) => action.validate(),
            Self::Spawn(s) => {
                s.task.validate()?;
                if s.handoff_id.as_ref().is_some_and(|id| !valid_id(id)) {
                    return Err("invalid handoff identity");
                }
                if !valid_id(&s.profile.id) || s.profile.revision == 0 {
                    return Err("invalid profile");
                }
                Ok(())
            }
            Self::MessageChild(m) if valid_id(&m.child_id) && message(&m.message) => Ok(()),
            Self::Stop(s) if valid_id(&s.child_id) => Ok(()),
            Self::MessageParent(m) if message(&m.message) => {
                if let Some(h) = &m.handoff {
                    h.validate()?;
                }
                Ok(())
            }
            _ => Err("invalid swarm tool arguments"),
        }
    }
}
