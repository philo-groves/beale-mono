//! Session graph definitions and delegation policy. Host execution stays in beale-sessions.

use beale_contracts::v1::{AuthMethod, GraphNodeModel, ProviderId, SessionGraphConfig};
use serde_json::{Value, json};

pub const FEATURE_ID: &str = "graphs";
pub const FEATURE_NAME: &str = "Graphs";
pub const FEATURE_DESCRIPTION: &str =
    "Coordinate peer research agents and focused Scout, Reviewer, and Prover subagents.";
pub const HAS_PROMPT_EXTENSION: bool = true;
pub const PROMPT_EXTENSION: &str = "The main agent may delegate bounded tasks to Scout, Reviewer, or Prover. Give each subagent a concrete task. Subagents use fresh context and cannot delegate further. Wait for their results and synthesize the final answer yourself.";
pub const TOOL_COUNT: u32 = 2;
pub const MAX_CONCURRENT: u32 = 64;
pub const DELEGATE_TOOL: &str = "graph_delegate";
pub const HANDOFF_TOOL: &str = "graph_handoff";
pub const MAX_BATCH: usize = 8;

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
pub struct FlowRuntime(&'static str);
pub static CLOUDFLARE_RUNTIME: FlowRuntime = FlowRuntime(CLOUDFLARE_ID);
pub static OPENAI_RUNTIME: FlowRuntime = FlowRuntime(OPENAI_ID);

pub fn runtime(id: &str) -> Option<&'static dyn GraphRuntime> {
    match id {
        SIMPLE_ID => Some(&SIMPLE_RUNTIME),
        CLOUDFLARE_ID => Some(&CLOUDFLARE_RUNTIME),
        OPENAI_ID => Some(&OPENAI_RUNTIME),
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

fn append_subagent_models(
    section: &mut String,
    config: &SessionGraphConfig,
    fallback: &GraphNodeModel,
) {
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
}

pub fn top_level_model(
    config: &SessionGraphConfig,
    role: Role,
    fallback: &GraphNodeModel,
) -> GraphNodeModel {
    role_models(config, role, fallback)
        .into_iter()
        .next()
        .unwrap_or_else(|| fallback.clone())
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
            "## Simple Orchestra\n\n{PROMPT_EXTENSION}\n\nUse `{DELEGATE_TOOL}` only when a specialist can help. Delegate at most {MAX_BATCH} tasks per batch. Model indices by role (omit `model_index` for the first):\n"
        );
        append_subagent_models(&mut section, config, fallback);
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
            if role.is_top_level() {
                return Err("top-level agents require a handoff");
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

impl GraphRuntime for FlowRuntime {
    fn id(&self) -> &'static str {
        self.0
    }

    fn tool_definition(&self) -> Value {
        let mut tool = SIMPLE_RUNTIME.tool_definition();
        tool["description"] = json!(
            "Run focused Scout, Reviewer, or Prover tasks for the active top-level agent. Results return to that agent."
        );
        tool
    }

    fn system_section(&self, config: &SessionGraphConfig, fallback: &GraphNodeModel) -> String {
        let graph = graph_definition(self.0).expect("registered flow graph");
        let mut section = format!(
            "## {}\n\nYou are one of several top-level agents in this session. There is no central main agent. Your conversation history is separate from other top-level agents. Stay within your role even while a peer is idle; queue work for that peer instead of performing its stage. Use `{HANDOFF_TOOL}` to queue a concrete task and evidence references for an allowed peer. The recipient starts when work is queued and remains idle otherwise. You may continue your own role's work while the recipient runs concurrently. The recipient starts with the handoff task and its own history. Use `{DELEGATE_TOOL}` for focused Scout, Reviewer, or Prover subagent tasks within your role. Each top-level agent may use its own subagents.\n\nTop-level roles and models:\n",
            graph.name
        );
        for role in graph.nodes.iter().filter(|role| role.is_top_level()) {
            let models = role_models(config, *role, fallback);
            if let Some(model) = models.first() {
                section.push_str(&format!(
                    "- {} (`{}`): provider={:?} method={:?} model={}\n",
                    role.label(),
                    role.id(),
                    ProviderId::try_from(model.provider).unwrap_or(ProviderId::Unspecified),
                    AuthMethod::try_from(model.method).unwrap_or(AuthMethod::Unspecified),
                    model.model
                ));
            }
        }
        section.push_str(&format!(
            "\nSubagent model indices (omit `model_index` for the first). Delegate at most {MAX_BATCH} tasks per batch:\n"
        ));
        append_subagent_models(&mut section, config, fallback);
        section
    }

    fn assignments(
        &self,
        arguments: &Value,
        config: &SessionGraphConfig,
        fallback: &GraphNodeModel,
    ) -> Result<Vec<Assignment>, &'static str> {
        SIMPLE_RUNTIME.assignments(arguments, config, fallback)
    }
}

pub fn handoff_definition(id: &str, current: Role) -> Option<Value> {
    let graph = graph_definition(id)?;
    let destinations: Vec<_> = graph
        .edges
        .iter()
        .filter_map(|edge| {
            if edge.from == current && edge.to.is_top_level() {
                Some(edge.to.id())
            } else if edge.bidirectional && edge.to == current && edge.from.is_top_level() {
                Some(edge.from.id())
            } else {
                None
            }
        })
        .collect();
    if destinations.is_empty() {
        return None;
    }
    Some(json!({"type":"function","name":HANDOFF_TOOL,
        "description":"Queue a task for a peer top-level agent. Include concrete instructions and evidence references. The recipient runs independently; your own work can continue.",
        "parameters":{"type":"object","properties":{"role":{"type":"string","enum":destinations},
            "task":{"type":"string","minLength":1,"maxLength":4096}},
            "required":["role","task"],"additionalProperties":false}}))
}

pub fn handoff_target(
    id: &str,
    current: Role,
    arguments: &Value,
) -> Result<(Role, String), &'static str> {
    let role = arguments
        .get("role")
        .and_then(Value::as_str)
        .and_then(Role::from_id)
        .ok_or("invalid handoff role")?;
    let task = arguments
        .get("task")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|task| !task.is_empty() && task.len() <= 4096)
        .ok_or("invalid handoff task")?;
    let graph = graph_definition(id).ok_or("unknown graph")?;
    if !graph.edges.iter().any(|edge| {
        (edge.from == current && edge.to == role
            || edge.bidirectional && edge.to == current && edge.from == role)
            && role.is_top_level()
    }) {
        return Err("handoff is not allowed by this graph");
    }
    Ok((role, task.to_owned()))
}

pub fn entry_role(id: &str) -> Option<Role> {
    graph_definition(id)?
        .nodes
        .iter()
        .copied()
        .find(|role| role.is_top_level())
}

pub const SIMPLE_ID: &str = "simple";
pub const CLOUDFLARE_ID: &str = "cloudflare_harness";
pub const OPENAI_ID: &str = "openai_pipeline";
pub const SIMPLE_NAME: &str = "Simple Orchestra";
pub const SIMPLE_DESCRIPTION: &str =
    "One main agent with a single layer of Scout, Reviewer, and Prover subagents.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    Main,
    Scout,
    Reviewer,
    Prover,
    DiscoveryHarness,
    ValidationSystem,
    Discovery,
    Validation,
    Reporter,
    PatchGeneration,
}

impl Role {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Scout => "scout",
            Self::Reviewer => "reviewer",
            Self::Prover => "prover",
            Self::DiscoveryHarness => "discovery_harness",
            Self::ValidationSystem => "validation_system",
            Self::Discovery => "discovery",
            Self::Validation => "validation",
            Self::Reporter => "reporter",
            Self::PatchGeneration => "patch_generation",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Main => "Main Agent",
            Self::Scout => "Scout",
            Self::Reviewer => "Reviewer",
            Self::Prover => "Prover",
            Self::DiscoveryHarness => "Discovery Harness",
            Self::ValidationSystem => "Validation System",
            Self::Discovery => "Discovery",
            Self::Validation => "Dynamic Validation",
            Self::Reporter => "Reporter",
            Self::PatchGeneration => "Patch Generation",
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
            Self::DiscoveryHarness => {
                "Explore the authorized codebase for candidate vulnerabilities. Hand off each concrete candidate and its evidence to the Validation System. Keep incomplete leads in Discovery; do not claim validation yourself."
            }
            Self::ValidationSystem => {
                "Independently validate and deduplicate handed-off candidates. Report evidence and uncertainty; hand unresolved gaps back to the Discovery Harness."
            }
            Self::Discovery => {
                "Discover candidate vulnerabilities within the authorized scope. Start with the in-scope Surfaces inventory when enabled; update a surface's definition or mapping as its boundaries become known, and mark a mapping reviewed only after its evidence supports review. Create or revise Candidate findings for concrete hypotheses and hand off the finding ID, observations, and evidence references to Dynamic Validation. Stop at a testable hypothesis: do not build or run a PoC, complete validation, screen, or report. If no candidate is ready, continue discovery and record coverage in Surfaces."
            }
            Self::Validation => {
                "Dynamically validate handed-off Candidates with reproducible evidence; build or run a focused PoC here when authorized and useful. Distinguish failed checks from confirmed behavior. Complete Validated only when supported, then hand off the finding ID and evidence to Reporter. Hand unresolved gaps back to Discovery. Do not screen or report."
            }
            Self::Reporter => {
                "Screen supported validated findings for impact and reachability, then prepare clear reports with evidence references. Hand actionable report and remediation context to Patch Generation. Return evidence gaps to Dynamic Validation."
            }
            Self::PatchGeneration => {
                "Propose and verify narrow patches for confirmed findings, preserving the evidence trail. Hand failed or incomplete patches back to Reporter with concrete gaps."
            }
        }
    }

    pub const fn index(self) -> usize {
        match self {
            Self::Main => 0,
            Self::Scout => 1,
            Self::Reviewer => 2,
            Self::Prover => 3,
            Self::DiscoveryHarness => 4,
            Self::ValidationSystem => 5,
            Self::Discovery => 6,
            Self::Validation => 7,
            Self::Reporter => 8,
            Self::PatchGeneration => 9,
        }
    }

    pub const fn default_max_concurrent(self) -> Option<u32> {
        match self {
            Self::Main
            | Self::DiscoveryHarness
            | Self::ValidationSystem
            | Self::Discovery
            | Self::Validation
            | Self::Reporter
            | Self::PatchGeneration => Some(1),
            Self::Scout | Self::Reviewer | Self::Prover => None,
        }
    }

    pub const fn max_models(self) -> usize {
        match self {
            Self::Main
            | Self::DiscoveryHarness
            | Self::ValidationSystem
            | Self::Discovery
            | Self::Validation
            | Self::Reporter
            | Self::PatchGeneration => 1,
            Self::Scout | Self::Reviewer | Self::Prover => 16,
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "main" => Some(Self::Main),
            "scout" => Some(Self::Scout),
            "reviewer" => Some(Self::Reviewer),
            "prover" => Some(Self::Prover),
            "discovery_harness" => Some(Self::DiscoveryHarness),
            "validation_system" => Some(Self::ValidationSystem),
            "discovery" => Some(Self::Discovery),
            "validation" => Some(Self::Validation),
            "reporter" => Some(Self::Reporter),
            "patch_generation" => Some(Self::PatchGeneration),
            _ => None,
        }
    }

    pub const fn is_top_level(self) -> bool {
        !matches!(self, Self::Scout | Self::Reviewer | Self::Prover)
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

pub const CLOUDFLARE_NODES: [Role; 5] = [
    Role::DiscoveryHarness,
    Role::ValidationSystem,
    Role::Scout,
    Role::Reviewer,
    Role::Prover,
];
pub const CLOUDFLARE_EDGES: [Edge; 7] = [
    Edge {
        from: Role::DiscoveryHarness,
        to: Role::ValidationSystem,
        bidirectional: true,
    },
    Edge {
        from: Role::DiscoveryHarness,
        to: Role::Scout,
        bidirectional: true,
    },
    Edge {
        from: Role::DiscoveryHarness,
        to: Role::Reviewer,
        bidirectional: true,
    },
    Edge {
        from: Role::DiscoveryHarness,
        to: Role::Prover,
        bidirectional: true,
    },
    Edge {
        from: Role::ValidationSystem,
        to: Role::Scout,
        bidirectional: true,
    },
    Edge {
        from: Role::ValidationSystem,
        to: Role::Reviewer,
        bidirectional: true,
    },
    Edge {
        from: Role::ValidationSystem,
        to: Role::Prover,
        bidirectional: true,
    },
];
pub const CLOUDFLARE_LAYOUT: [NodeLayout; 5] = [
    NodeLayout {
        role: Role::DiscoveryHarness,
        x: 75.0,
        y: 15.0,
    },
    NodeLayout {
        role: Role::ValidationSystem,
        x: 335.0,
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
pub const CLOUDFLARE_GRAPH: GraphDefinition = GraphDefinition {
    id: CLOUDFLARE_ID,
    name: "Discovery and Validation",
    description: "Two peer agents for vulnerability discovery and independent validation, each with subagents.",
    nodes: &CLOUDFLARE_NODES,
    edges: &CLOUDFLARE_EDGES,
    layout: &CLOUDFLARE_LAYOUT,
};

pub const OPENAI_NODES: [Role; 7] = [
    Role::Discovery,
    Role::Validation,
    Role::Reporter,
    Role::PatchGeneration,
    Role::Scout,
    Role::Reviewer,
    Role::Prover,
];
pub const OPENAI_EDGES: [Edge; 15] = [
    Edge {
        from: Role::Discovery,
        to: Role::Validation,
        bidirectional: true,
    },
    Edge {
        from: Role::Validation,
        to: Role::Reporter,
        bidirectional: true,
    },
    Edge {
        from: Role::Reporter,
        to: Role::PatchGeneration,
        bidirectional: true,
    },
    Edge {
        from: Role::Discovery,
        to: Role::Scout,
        bidirectional: true,
    },
    Edge {
        from: Role::Discovery,
        to: Role::Reviewer,
        bidirectional: true,
    },
    Edge {
        from: Role::Discovery,
        to: Role::Prover,
        bidirectional: true,
    },
    Edge {
        from: Role::Validation,
        to: Role::Scout,
        bidirectional: true,
    },
    Edge {
        from: Role::Validation,
        to: Role::Reviewer,
        bidirectional: true,
    },
    Edge {
        from: Role::Validation,
        to: Role::Prover,
        bidirectional: true,
    },
    Edge {
        from: Role::Reporter,
        to: Role::Scout,
        bidirectional: true,
    },
    Edge {
        from: Role::Reporter,
        to: Role::Reviewer,
        bidirectional: true,
    },
    Edge {
        from: Role::Reporter,
        to: Role::Prover,
        bidirectional: true,
    },
    Edge {
        from: Role::PatchGeneration,
        to: Role::Scout,
        bidirectional: true,
    },
    Edge {
        from: Role::PatchGeneration,
        to: Role::Reviewer,
        bidirectional: true,
    },
    Edge {
        from: Role::PatchGeneration,
        to: Role::Prover,
        bidirectional: true,
    },
];
pub const OPENAI_LAYOUT: [NodeLayout; 7] = [
    NodeLayout {
        role: Role::Discovery,
        x: 15.0,
        y: 15.0,
    },
    NodeLayout {
        role: Role::Validation,
        x: 175.0,
        y: 15.0,
    },
    NodeLayout {
        role: Role::Reporter,
        x: 335.0,
        y: 15.0,
    },
    NodeLayout {
        role: Role::PatchGeneration,
        x: 495.0,
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
pub const OPENAI_GRAPH: GraphDefinition = GraphDefinition {
    id: OPENAI_ID,
    name: "Research Pipeline",
    description: "Discovery, dynamic validation, reporting, and patch generation as peer agents with subagents.",
    nodes: &OPENAI_NODES,
    edges: &OPENAI_EDGES,
    layout: &OPENAI_LAYOUT,
};

pub fn graph_definition(id: &str) -> Option<&'static GraphDefinition> {
    match id {
        SIMPLE_ID => Some(&SIMPLE_GRAPH),
        CLOUDFLARE_ID => Some(&CLOUDFLARE_GRAPH),
        OPENAI_ID => Some(&OPENAI_GRAPH),
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

    #[test]
    fn flow_graphs_have_peer_handoffs_and_subagents() {
        for graph in [&CLOUDFLARE_GRAPH, &OPENAI_GRAPH] {
            assert_eq!(entry_role(graph.id), Some(graph.nodes[0]));
            assert_eq!(graph.layout.len(), graph.nodes.len());
            assert!(
                graph
                    .nodes
                    .iter()
                    .all(|role| Role::from_id(role.id()) == Some(*role))
            );
            assert!(
                graph
                    .nodes
                    .iter()
                    .filter(|role| role.is_top_level())
                    .all(|role| {
                        graph
                            .edges
                            .iter()
                            .any(|edge| edge.from == *role && edge.to == Role::Scout)
                    })
            );
            assert_eq!(
                runtime(graph.id).map(|runtime| runtime.id()),
                Some(graph.id)
            );
            let guidance = runtime(graph.id).unwrap().system_section(
                &SessionGraphConfig::default(),
                &GraphNodeModel {
                    node_id: graph.nodes[0].id().to_owned(),
                    provider: ProviderId::Openai as i32,
                    method: AuthMethod::ApiKey as i32,
                    model: "example-model".to_owned(),
                },
            );
            assert!(guidance.contains("Subagent model indices"));
            assert!(guidance.contains("- Scout: 0:"));
            for edge in graph
                .edges
                .iter()
                .filter(|edge| edge.from.is_top_level() && edge.to.is_top_level())
            {
                let handoff = json!({"role":edge.to.id(),"task":"Review example evidence"});
                assert_eq!(
                    handoff_target(graph.id, edge.from, &handoff).unwrap().0,
                    edge.to
                );
                assert!(handoff_definition(graph.id, edge.from).is_some());
                if edge.bidirectional {
                    let reverse = json!({"role":edge.from.id(),"task":"Recheck example claim"});
                    assert_eq!(
                        handoff_target(graph.id, edge.to, &reverse).unwrap().0,
                        edge.from
                    );
                }
            }
        }
        assert!(
            handoff_target(
                OPENAI_ID,
                Role::Discovery,
                &json!({"role":"reporter","task":"Skip validation"})
            )
            .is_err()
        );
        assert!(
            handoff_target(
                CLOUDFLARE_ID,
                Role::DiscoveryHarness,
                &json!({"role":"scout","task":"Not a peer"})
            )
            .is_err()
        );
        assert!(
            handoff_target(
                CLOUDFLARE_ID,
                Role::DiscoveryHarness,
                &json!({"role":"validation_system","task":" "})
            )
            .is_err()
        );
        assert!(handoff_definition(SIMPLE_ID, Role::Main).is_none());
    }
}
