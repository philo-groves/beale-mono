//! Session graph definitions and delegation policy. Host execution stays in beale-sessions.

use beale_contracts::v1::{AuthMethod, GraphNodeModel, ProviderId, SessionGraphConfig};
use serde_json::{Value, json};

pub const FEATURE_ID: &str = "graphs";
pub const FEATURE_NAME: &str = "Graphs";
pub const FEATURE_DESCRIPTION: &str =
    "Coordinate research with Scout, Reviewer, and Prover subagents.";
pub const HAS_PROMPT_EXTENSION: bool = true;
pub const PROMPT_EXTENSION: &str = "The main agent may delegate bounded tasks to Scout, Reviewer, or Prover. Give each subagent a concrete task. Subagents use fresh context and cannot delegate further. Wait for their results and synthesize the final answer yourself.";
pub const TOOL_COUNT: u32 = 1;
pub const MAX_CONCURRENT: u32 = 64;
pub const DELEGATE_TOOL: &str = "graph_delegate";
pub const MAX_BATCH: usize = 8;
pub const MAX_ASSIGNMENTS_PER_TURN: usize = 16;

#[derive(Clone, Debug)]
pub struct Assignment {
    pub role: Role,
    pub task: String,
    pub model: GraphNodeModel,
}

/// A graph runtime owns the model-facing delegation policy; the host owns execution.
pub trait GraphRuntime: Sync {
    fn id(&self) -> &'static str;
    fn tool_definition(&self) -> Value;
    fn system_section(&self, config: &SessionGraphConfig, fallback: &GraphNodeModel) -> String;
    fn assignments(
        &self,
        arguments: &Value,
        config: &SessionGraphConfig,
        fallback: &GraphNodeModel,
    ) -> Result<Vec<Assignment>, &'static str>;
}

pub struct SimpleRuntime;
pub static SIMPLE_RUNTIME: SimpleRuntime = SimpleRuntime;

pub fn runtime(id: &str) -> Option<&'static dyn GraphRuntime> {
    match id {
        SIMPLE_ID => Some(&SIMPLE_RUNTIME),
        _ => None,
    }
}

fn role_models(
    config: &SessionGraphConfig,
    role: Role,
    fallback: &GraphNodeModel,
) -> Vec<GraphNodeModel> {
    let selected: Vec<_> = config
        .models
        .iter()
        .filter(|model| model.node_id == role.id())
        .cloned()
        .collect();
    if selected.is_empty() {
        vec![GraphNodeModel {
            node_id: role.id().to_owned(),
            ..fallback.clone()
        }]
    } else {
        selected
    }
}

impl GraphRuntime for SimpleRuntime {
    fn id(&self) -> &'static str {
        SIMPLE_ID
    }

    fn tool_definition(&self) -> Value {
        json!({"type":"function","name":DELEGATE_TOOL,
            "description":"Run one or more focused Scout, Reviewer, or Prover tasks in fresh subagent contexts. Results return to the main agent. Batch independent tasks for parallel execution.",
            "parameters":{"type":"object","properties":{"assignments":{"type":"array","minItems":1,"maxItems":MAX_BATCH,
                "items":{"type":"object","properties":{"role":{"type":"string","enum":["scout","reviewer","prover"]},
                    "task":{"type":"string","minLength":1,"maxLength":4096},
                    "model_index":{"type":"integer","minimum":0,"maximum":15}},
                    "required":["role","task"],"additionalProperties":false}}},
                "required":["assignments"],"additionalProperties":false}})
    }

    fn system_section(&self, config: &SessionGraphConfig, fallback: &GraphNodeModel) -> String {
        let mut section = format!(
            "## Simple Orchestra\n\n{PROMPT_EXTENSION}\n\nUse `{DELEGATE_TOOL}` only when a specialist can help. Delegate at most {MAX_BATCH} tasks per batch and {MAX_ASSIGNMENTS_PER_TURN} per turn. Model indices by role (omit `model_index` for the first):\n"
        );
        for role in [Role::Scout, Role::Reviewer, Role::Prover] {
            let models = role_models(config, role, fallback);
            let choices = models
                .iter()
                .enumerate()
                .map(|(index, model)| {
                    format!(
                        "{index}: provider={:?} method={:?} model={}",
                        ProviderId::try_from(model.provider).unwrap_or(ProviderId::Unspecified),
                        AuthMethod::try_from(model.method).unwrap_or(AuthMethod::Unspecified),
                        model.model
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            section.push_str(&format!("- {}: {choices}\n", role.label()));
        }
        section
    }

    fn assignments(
        &self,
        arguments: &Value,
        config: &SessionGraphConfig,
        fallback: &GraphNodeModel,
    ) -> Result<Vec<Assignment>, &'static str> {
        let items = arguments
            .get("assignments")
            .and_then(Value::as_array)
            .ok_or("assignments array is required")?;
        if items.is_empty() || items.len() > MAX_BATCH {
            return Err("invalid assignment count");
        }
        let mut assignments = Vec::with_capacity(items.len());
        for item in items {
            let role = item
                .get("role")
                .and_then(Value::as_str)
                .and_then(Role::from_id)
                .ok_or("invalid subagent role")?;
            if role == Role::Main {
                return Err("the main agent cannot be delegated");
            }
            let task = item
                .get("task")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|task| !task.is_empty() && task.len() <= 4096)
                .ok_or("invalid subagent task")?;
            let index = item
                .get("model_index")
                .map(|value| value.as_u64().ok_or("invalid model index"))
                .transpose()?
                .unwrap_or(0);
            let model = role_models(config, role, fallback)
                .get(usize::try_from(index).map_err(|_| "invalid model index")?)
                .cloned()
                .ok_or("model index is not configured for this role")?;
            assignments.push(Assignment {
                role,
                task: task.to_owned(),
                model,
            });
        }
        Ok(assignments)
    }
}

pub const SIMPLE_ID: &str = "simple";
pub const SIMPLE_NAME: &str = "Simple Orchestra";
pub const SIMPLE_DESCRIPTION: &str =
    "One main agent with a single layer of Scout, Reviewer, and Prover subagents.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    Main,
    Scout,
    Reviewer,
    Prover,
}

impl Role {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Scout => "scout",
            Self::Reviewer => "reviewer",
            Self::Prover => "prover",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Main => "Main Agent",
            Self::Scout => "Scout",
            Self::Reviewer => "Reviewer",
            Self::Prover => "Prover",
        }
    }

    pub const fn instruction(self) -> &'static str {
        match self {
            Self::Scout => {
                "Explore the assigned lead. Distinguish observations from hypotheses and report useful evidence references."
            }
            Self::Reviewer => {
                "Independently review the assigned claim or work. Identify unsupported assumptions, gaps, and concrete checks. Do not infer that a claim is verified."
            }
            Self::Prover => {
                "Seek reproducible evidence for the assigned claim within the workspace's authorized scope. Report the exact evidence and limitations."
            }
            Self::Main => "",
        }
    }

    pub const fn index(self) -> usize {
        match self {
            Self::Main => 0,
            Self::Scout => 1,
            Self::Reviewer => 2,
            Self::Prover => 3,
        }
    }

    pub const fn default_max_concurrent(self) -> Option<u32> {
        match self {
            Self::Main => Some(1),
            Self::Scout | Self::Reviewer | Self::Prover => None,
        }
    }

    pub const fn max_models(self) -> usize {
        match self {
            Self::Main => 1,
            Self::Scout | Self::Reviewer | Self::Prover => 16,
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "main" => Some(Self::Main),
            "scout" => Some(Self::Scout),
            "reviewer" => Some(Self::Reviewer),
            "prover" => Some(Self::Prover),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Edge {
    pub from: Role,
    pub to: Role,
    pub bidirectional: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeLayout {
    pub role: Role,
    pub x: f32,
    pub y: f32,
}

pub struct GraphDefinition {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub nodes: &'static [Role],
    pub edges: &'static [Edge],
    pub layout: &'static [NodeLayout],
}

pub const SIMPLE_NODES: [Role; 4] = [Role::Main, Role::Scout, Role::Reviewer, Role::Prover];
pub const SIMPLE_LAYOUT: [NodeLayout; 4] = [
    NodeLayout {
        role: Role::Main,
        x: 235.0,
        y: 15.0,
    },
    NodeLayout {
        role: Role::Scout,
        x: 25.0,
        y: 169.0,
    },
    NodeLayout {
        role: Role::Reviewer,
        x: 235.0,
        y: 169.0,
    },
    NodeLayout {
        role: Role::Prover,
        x: 445.0,
        y: 169.0,
    },
];
pub const SIMPLE_EDGES: [Edge; 3] = [
    Edge {
        from: Role::Main,
        to: Role::Scout,
        bidirectional: true,
    },
    Edge {
        from: Role::Main,
        to: Role::Reviewer,
        bidirectional: true,
    },
    Edge {
        from: Role::Main,
        to: Role::Prover,
        bidirectional: true,
    },
];

pub const SIMPLE_GRAPH: GraphDefinition = GraphDefinition {
    id: SIMPLE_ID,
    name: SIMPLE_NAME,
    description: SIMPLE_DESCRIPTION,
    nodes: &SIMPLE_NODES,
    edges: &SIMPLE_EDGES,
    layout: &SIMPLE_LAYOUT,
};

pub fn graph_definition(id: &str) -> Option<&'static GraphDefinition> {
    match id {
        SIMPLE_ID => Some(&SIMPLE_GRAPH),
        _ => None,
    }
}

pub fn known_graph(id: &str) -> bool {
    graph_definition(id).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_graph_has_only_one_subagent_layer() {
        assert_eq!(SIMPLE_GRAPH.nodes.len(), 4);
        assert_eq!(SIMPLE_GRAPH.edges.len(), 3);
        assert_eq!(SIMPLE_GRAPH.layout.len(), SIMPLE_GRAPH.nodes.len());
        assert!(
            SIMPLE_GRAPH
                .edges
                .iter()
                .all(|edge| edge.from == Role::Main && edge.bidirectional)
        );
        for role in [Role::Scout, Role::Reviewer, Role::Prover] {
            assert_eq!(Role::from_id(role.id()), Some(role));
        }
        assert_eq!(Role::Main.index(), 0);
        assert_eq!(Role::Main.default_max_concurrent(), Some(1));
        assert_eq!(Role::Scout.default_max_concurrent(), None);
        assert_eq!(Role::Main.max_models(), 1);
        assert_eq!(Role::Scout.max_models(), 16);
        assert!(known_graph(SIMPLE_ID));
        assert!(!known_graph("unknown"));
    }

    #[test]
    fn simple_runtime_resolves_role_models_and_rejects_invalid_delegation() {
        let fallback = GraphNodeModel {
            node_id: "main".to_owned(),
            provider: ProviderId::Openai as i32,
            method: AuthMethod::ApiKey as i32,
            model: "example-main".to_owned(),
        };
        let config = SessionGraphConfig {
            models: vec![
                GraphNodeModel {
                    node_id: "scout".to_owned(),
                    model: "example-scout-a".to_owned(),
                    ..fallback.clone()
                },
                GraphNodeModel {
                    node_id: "scout".to_owned(),
                    model: "example-scout-b".to_owned(),
                    ..fallback.clone()
                },
            ],
            concurrency: Vec::new(),
        };
        let engine = runtime(SIMPLE_ID).unwrap();
        assert_eq!(engine.id(), SIMPLE_ID);
        assert_eq!(engine.tool_definition()["name"], DELEGATE_TOOL);
        let section = engine.system_section(&config, &fallback);
        assert!(section.contains("example-scout-b"));
        let assignments = engine
            .assignments(
                &json!({"assignments":[
                    {"role":"scout","task":"Inspect the example module","model_index":1},
                    {"role":"reviewer","task":"Check the example evidence"}
                ]}),
                &config,
                &fallback,
            )
            .unwrap();
        assert_eq!(assignments[0].model.model, "example-scout-b");
        assert_eq!(assignments[1].model.model, "example-main");
        assert_eq!(assignments[1].role, Role::Reviewer);
        for invalid in [
            json!({"assignments":[]}),
            json!({"assignments":[{"role":"main","task":"Invalid"}]}),
            json!({"assignments":[{"role":"scout","task":"  "}]}),
            json!({"assignments":[{"role":"scout","task":"Valid","model_index":2}]}),
        ] {
            assert!(engine.assignments(&invalid, &config, &fallback).is_err());
        }
        assert!(runtime("").is_none());
    }
}
