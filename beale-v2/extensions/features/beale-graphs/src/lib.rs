//! Session graph definitions and delegation policy. Host execution stays in beale-sessions.

use beale_contracts::v1::{AuthMethod, GraphNodeModel, ProviderId, SessionGraphConfig};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{LazyLock, RwLock},
};
mod catalog;
mod tasks;
pub use tasks::{ContextPolicy, RESULT_INSTRUCTIONS, TaskContract, TaskResult};
pub const DEFAULT_MAX_CONCURRENT_WORKERS: u32 = 8;
pub fn worker_budget(config: &SessionGraphConfig) -> u32 {
    if config.max_concurrent_workers == 0 {
        DEFAULT_MAX_CONCURRENT_WORKERS
    } else {
        config.max_concurrent_workers.clamp(1, MAX_CONCURRENT)
    }
}
mod store;
pub use store::{GraphCommit, GraphHandoffRow, GraphStore, MIGRATIONS, migrate};

pub const FEATURE_ID: &str = "graphs";
pub const FEATURE_NAME: &str = "Graphs";
pub const FEATURE_DESCRIPTION: &str = "Coordinate peer research agents and focused Mapper, Investigator, Experimenter, Challenger, and Builder subagents.";
pub const HAS_PROMPT_EXTENSION: bool = true;
pub const PROMPT_EXTENSION: &str = "Spawn bounded Mapper, Investigator, Experimenter, Challenger, or Builder tasks, then continue your work while they run. Use graph_subagents_list to check status, graph_subagents_inspect to read a child's transcript and tool calls, graph_subagents_message to guide a child, and graph_subagents_kill to stop one. Workers with shared context may inspect or message siblings in their parent group. Evidence-only workers receive fresh context and may contact only their parent. Workers cannot spawn or kill. Check all spawned children before ending your turn and synthesize their results yourself.";
pub const TOOL_COUNT: u32 = 6;

/// Catalog and session descriptor for this feature.
pub const FEATURE: beale_feature_api::Feature = beale_feature_api::Feature {
    id: FEATURE_ID,
    name: FEATURE_NAME,
    description: FEATURE_DESCRIPTION,
    prompt_extension: PROMPT_EXTENSION,
    use_case: "Use to delegate bounded research tasks to graph subagents.",
    tool_names: &[
        SPAWN_TOOL,
        LIST_TOOL,
        INSPECT_TOOL,
        MESSAGE_TOOL,
        KILL_TOOL,
        HANDOFF_TOOL,
    ],
    definitions: None,
    review: beale_feature_api::ToolReview::Exempt,
    subagents: false,
    migrations: MIGRATIONS,
    startup: &[],
};
pub const MAX_CONCURRENT: u32 = 64;
pub const MAX_GRAPH_ID_BYTES: usize = 160;
pub const SPAWN_TOOL: &str = "graph_subagents_spawn";
pub const LIST_TOOL: &str = "graph_subagents_list";
pub const INSPECT_TOOL: &str = "graph_subagents_inspect";
pub const MESSAGE_TOOL: &str = "graph_subagents_message";
pub const KILL_TOOL: &str = "graph_subagents_kill";
/// Transcript-only activity generated when an agent receives a graph message.
pub const RECEIVE_EVENT: &str = "graph_subagents_receive";
pub const HANDOFF_TOOL: &str = "graph_handoff";
pub const MAX_BATCH: usize = 8;

#[derive(Clone, Debug)]
pub struct Assignment {
    pub role: ProfileId,
    pub task: String,
    pub contract: Option<TaskContract>,
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
pub static SEQUENTIAL_RUNTIME: FlowRuntime = FlowRuntime(SEQUENTIAL_ID);

struct RegisteredGraph {
    definition: &'static GraphDefinition,
    runtime: &'static FlowRuntime,
    snapshot: String,
}

static CUSTOM_GRAPHS: LazyLock<RwLock<HashMap<String, RegisteredGraph>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Register a bounded, validated template. Its content-derived ID pins the definition.
pub fn register_template(json: &str) -> Result<String, String> {
    let id = catalog::custom_id(json)?;
    let mut graphs = CUSTOM_GRAPHS
        .write()
        .map_err(|_| "graph registry unavailable")?;
    if graphs.contains_key(&id) {
        return Ok(id);
    }
    if graphs.len() >= 1_024 {
        return Err("too many graph template revisions are loaded".to_owned());
    }
    let definition = Box::leak(Box::new(catalog::load_custom(json, &id)?));
    let runtime = Box::leak(Box::new(FlowRuntime(definition.id)));
    graphs.insert(
        id.clone(),
        RegisteredGraph {
            definition,
            runtime,
            snapshot: json.to_owned(),
        },
    );
    Ok(id)
}

pub fn template_snapshot(id: &str) -> Option<String> {
    CUSTOM_GRAPHS
        .read()
        .ok()?
        .get(id)
        .map(|graph| graph.snapshot.clone())
}

pub fn subagent_tools() -> Vec<Value> {
    vec![
        json!({"type":"function","name":LIST_TOOL,
            "description":"List this top-level agent's subagents, status, time since their last message, and completed results.",
            "parameters":{"type":"object","properties":{},"additionalProperties":false}}),
        inspect_definition(false),
        message_definition(false),
        json!({"type":"function","name":KILL_TOOL,
            "description":"Immediately stop one of this top-level agent's active subagents.",
            "parameters":{"type":"object","properties":{"subagent_id":{"type":"string","minLength":1,"maxLength":64}},
                "required":["subagent_id"],"additionalProperties":false}}),
    ]
}

pub fn inspect_definition(for_child: bool) -> Value {
    let description = if for_child {
        "Inspect a sibling subagent's live transcript and tool calls in your parent group."
    } else {
        "Inspect one of your subagents' live or saved transcript and tool calls. Omit offset for the newest page."
    };
    json!({"type":"function","name":INSPECT_TOOL,"description":description,
        "parameters":{"type":"object","properties":{
            "subagent_id":{"type":"string","minLength":1,"maxLength":64},
            "offset":{"type":"integer","minimum":0},
            "limit":{"type":"integer","minimum":1,"maximum":40}},
            "required":["subagent_id"],"additionalProperties":false}})
}

pub fn message_definition(for_child: bool) -> Value {
    let description = if for_child {
        "Send a message to your parent (recipient: parent) or a sibling subagent in the same parent group."
    } else {
        "Send a message to one of your active subagents."
    };
    json!({"type":"function","name":MESSAGE_TOOL,"description":description,
        "parameters":{"type":"object","properties":{
            "recipient":{"type":"string","minLength":1,"maxLength":64},
            "message":{"type":"string","minLength":1,"maxLength":4096}},
            "required":["recipient","message"],"additionalProperties":false}})
}

pub fn runtime(id: &str) -> Option<&'static dyn GraphRuntime> {
    match id {
        SIMPLE_ID => Some(&SIMPLE_RUNTIME),
        CLOUDFLARE_ID => Some(&CLOUDFLARE_RUNTIME),
        OPENAI_ID => Some(&OPENAI_RUNTIME),
        SEQUENTIAL_ID => Some(&SEQUENTIAL_RUNTIME),
        _ => Some(CUSTOM_GRAPHS.read().ok()?.get(id)?.runtime),
    }
}

fn role_models(
    config: &SessionGraphConfig,
    role: ProfileId,
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
    for role in SIMPLE_GRAPH
        .nodes
        .iter()
        .copied()
        .filter(|role| !role.is_top_level())
    {
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
        section.push_str(&format!(
            "- {} (`{}`): {choices}. Spawn when: {}. Required context: {}\n",
            role.label(),
            role.id(),
            role.spawn_when(),
            role.required_context_policy()
                .map_or("either policy".to_owned(), |policy| serde_json::to_string(
                    &policy
                )
                .unwrap_or_default())
        ));
    }
}

pub fn top_level_model(
    config: &SessionGraphConfig,
    role: ProfileId,
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
        let roles: Vec<_> = SIMPLE_GRAPH
            .nodes
            .iter()
            .filter(|p| !p.is_top_level())
            .map(|p| p.id())
            .collect();
        json!({"type":"function","name":SPAWN_TOOL,
            "description":"Delegate an independent bounded task with a question, scope, evidence, deliverable, completion criteria, context policy, and budget. Tasks queue under the shared scan worker limit. Return IDs immediately.",
            "parameters":{"type":"object","properties":{"assignments":{"type":"array","minItems":1,"maxItems":MAX_BATCH,
                "items":{"type":"object","properties":{"role":{"type":"string","enum":roles},
                    "contract":tasks::contract_schema(),
                    "model_index":{"type":"integer","minimum":0,"maximum":15}},
                    "required":["role","contract"],"additionalProperties":false}}},
                "required":["assignments"],"additionalProperties":false}})
    }

    fn system_section(&self, config: &SessionGraphConfig, fallback: &GraphNodeModel) -> String {
        let mut section = format!(
            "## Simple Orchestra\n\n{PROMPT_EXTENSION}\n\nUse `{SPAWN_TOOL}` only when a specialist can help. Spawn at most {MAX_BATCH} tasks per batch. Model indices by role (omit `model_index` for the first):\n"
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
                .and_then(ProfileId::from_id)
                .ok_or("invalid subagent role")?;
            if role.is_top_level() {
                return Err("top-level agents require a handoff");
            }
            let contract = item
                .get("contract")
                .map(|value| {
                    serde_json::from_value::<TaskContract>(value.clone())
                        .map_err(|_| "invalid task contract")
                })
                .transpose()?;
            if let Some(contract) = &contract {
                contract.validate()?;
            }
            if !role.is_legacy() && contract.is_none() {
                return Err("task contract is required for this profile");
            }
            if role.required_context_policy().is_some_and(|required| {
                contract
                    .as_ref()
                    .is_none_or(|c| c.context_policy != required)
            }) {
                return Err("task context policy does not match profile requirement");
            }
            let task = if let Some(contract) = &contract {
                contract.question.clone()
            } else {
                item.get("task")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|task| !task.is_empty() && task.len() <= 4096)
                    .ok_or("invalid subagent task")?
                    .to_owned()
            };
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
                task,
                contract,
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
        if let Some(graph) = graph_definition(self.0) {
            let roles: Vec<_> = graph
                .nodes
                .iter()
                .filter(|role| !role.is_top_level())
                .map(|role| role.id())
                .collect();
            tool["parameters"]["properties"]["assignments"]["items"]["properties"]["role"]["enum"] =
                json!(roles);
        }
        tool["description"] = json!(
            "Start focused Mapper, Investigator, Experimenter, Challenger, or Builder tasks for the active top-level agent. Return their IDs immediately."
        );
        tool
    }

    fn system_section(&self, config: &SessionGraphConfig, fallback: &GraphNodeModel) -> String {
        let graph = graph_definition(self.0).expect("registered flow graph");
        let mut section = format!(
            "## {}\n\nYou are one of several top-level agents in this session. There is no central main agent. Your conversation history is separate from other top-level agents. Stay within your role even while a peer is idle; queue work for that peer instead of performing its stage. Use `{HANDOFF_TOOL}` to queue a concrete task and evidence references for an allowed peer. The recipient starts when capacity is available and remains idle otherwise. The host runs at most {} top-level roles simultaneously. You may continue your own role's work while the recipient runs. The recipient starts with the handoff task and its own history. {PROMPT_EXTENSION} Use `{SPAWN_TOOL}` within your role. Each top-level agent has its own children.\n\nTop-level roles and models:\n",
            graph.name, graph.max_active_top_level
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
        for role in graph.nodes.iter().filter(|role| !role.is_top_level()) {
            let models = role_models(config, *role, fallback);
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
            section.push_str(&format!(
                "- {} (`{}`): {choices}. Spawn when: {}. Required context: {}\n",
                role.label(),
                role.id(),
                role.spawn_when(),
                role.required_context_policy()
                    .map_or("either policy".to_owned(), |policy| serde_json::to_string(
                        &policy
                    )
                    .unwrap_or_default())
            ));
        }
        if is_pipeline_graph(self.0) {
            section.push_str("\nResearch Pipeline discovery has two peer stages. Audit Targets maps and ranks diverse, authorized codebase scopes; Discovery investigates those ranked targets for candidate vulnerabilities. The Audit Targets handoff requires an ordered `ranked_targets` array with target IDs or locators, ranking signals, rationale, and evidence references. State uncertainty in the task. Do not skip directly from Audit Targets to Dynamic Validation.\n");
        }
        if graph.mode == GraphMode::RotatingStages {
            let sequence = graph
                .stages
                .iter()
                .map(|stage| stage.role.label())
                .collect::<Vec<_>>()
                .join(", ");
            let early = graph
                .stages
                .iter()
                .filter(|stage| stage.when_empty == WhenEmpty::Advance)
                .map(|stage| stage.role.label())
                .collect::<Vec<_>>()
                .join(", ");
            section.push_str(&format!("\nSequential scheduling: only one top-level role runs at a time. The host cycles through {sequence}. Handoffs queue work for the recipient stage. At a stage deadline, finish the current action and yield to the next role; the host stops the turn {} minutes later if necessary. {early} advance early when their queues empty.\n", graph.grace_minutes));
        }
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

pub fn spawn_definition(id: &str, current: ProfileId) -> Option<Value> {
    let mut tool = runtime(id)?.tool_definition();
    let graph = graph_definition(id)?;
    if graph.mode != GraphMode::Single {
        let roles = graph
            .edges
            .iter()
            .filter(|edge| edge.from == current && !edge.to.is_top_level())
            .map(|edge| edge.to.id())
            .collect::<Vec<_>>();
        if roles.is_empty() {
            return None;
        }
        tool["parameters"]["properties"]["assignments"]["items"]["properties"]["role"]["enum"] =
            json!(roles);
    }
    Some(tool)
}

pub fn handoff_definition(id: &str, current: ProfileId) -> Option<Value> {
    let graph = graph_definition(id)?;
    let destinations: Vec<_> = graph
        .handoffs
        .iter()
        .filter(|handoff| handoff.from == current)
        .map(|handoff| handoff.to.id())
        .collect();
    if destinations.is_empty() {
        return None;
    }
    let ranked_destinations: Vec<_> = graph
        .handoffs
        .iter()
        .filter(|handoff| handoff.from == current && handoff.payload == "ranked_targets@1")
        .map(|handoff| handoff.to.id())
        .collect();
    if !ranked_destinations.is_empty() {
        return Some(json!({"type":"function","name":HANDOFF_TOOL,
            "description":"Queue a ranked batch of audit targets for Discovery. The array order is the priority order; include evidence for each signal. The recipient runs independently.",
            "parameters":{"type":"object","properties":{"role":{"type":"string","enum":ranked_destinations},
                "task":{"type":"string","minLength":1,"maxLength":1024},
                "ranked_targets":{"type":"array","minItems":1,"maxItems":8,"items":{
                    "type":"object","properties":{
                        "target":{"type":"string","minLength":1,"maxLength":256},
                        "signals":{"type":"array","minItems":1,"maxItems":6,"items":{"type":"string","enum":["reachability","sink","code_smell","bug_precedent","related_weakness","security_boundary"]}},
                        "rationale":{"type":"string","minLength":1,"maxLength":512},
                        "evidence_refs":{"type":"array","minItems":1,"maxItems":4,"items":{"type":"string","minLength":1,"maxLength":256}}
                    },"required":["target","signals","rationale","evidence_refs"],"additionalProperties":false}}},
                "required":["role","task","ranked_targets"],"additionalProperties":false}}));
    }
    Some(json!({"type":"function","name":HANDOFF_TOOL,
        "description":"Queue a task for a peer top-level agent. Include concrete instructions and evidence references. The recipient runs independently; your own work can continue.",
        "parameters":{"type":"object","properties":{"role":{"type":"string","enum":destinations},
            "task":{"type":"string","minLength":1,"maxLength":4096}},
            "required":["role","task"],"additionalProperties":false}}))
}

pub fn handoff_target(
    id: &str,
    current: ProfileId,
    arguments: &Value,
) -> Result<(ProfileId, String), &'static str> {
    let role = arguments
        .get("role")
        .and_then(Value::as_str)
        .and_then(ProfileId::from_id)
        .ok_or("invalid handoff role")?;
    let task = arguments
        .get("task")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|task| !task.is_empty() && task.len() <= 4096)
        .ok_or("invalid handoff task")?;
    let graph = graph_definition(id).ok_or("unknown graph")?;
    let handoff = graph
        .handoffs
        .iter()
        .find(|handoff| handoff.from == current && handoff.to == role)
        .ok_or("handoff is not allowed by this graph")?;
    if handoff.payload == "ranked_targets@1" {
        if task.len() > 1024 {
            return Err("invalid audit target handoff");
        }
        let targets = arguments
            .get("ranked_targets")
            .and_then(Value::as_array)
            .filter(|targets| !targets.is_empty() && targets.len() <= 8)
            .ok_or("ranked audit targets are required")?;
        for target in targets {
            let object = target.as_object().ok_or("invalid ranked audit target")?;
            if object.len() != 4
                || !object.keys().all(|key| {
                    matches!(
                        key.as_str(),
                        "target" | "signals" | "rationale" | "evidence_refs"
                    )
                })
            {
                return Err("invalid ranked audit target");
            }
            for (key, limit) in [("target", 256), ("rationale", 512)] {
                let value = target
                    .get(key)
                    .and_then(Value::as_str)
                    .ok_or("invalid ranked audit target")?;
                if value.trim().is_empty() || value.len() > limit {
                    return Err("invalid ranked audit target");
                }
            }
            let signals = target
                .get("signals")
                .and_then(Value::as_array)
                .ok_or("invalid ranking signals")?;
            if signals.is_empty()
                || signals.len() > 6
                || !signals.iter().all(|signal| {
                    matches!(
                        signal.as_str(),
                        Some(
                            "reachability"
                                | "sink"
                                | "code_smell"
                                | "bug_precedent"
                                | "related_weakness"
                                | "security_boundary"
                        )
                    )
                })
            {
                return Err("invalid ranking signals");
            }
            let evidence = target
                .get("evidence_refs")
                .and_then(Value::as_array)
                .ok_or("invalid ranking evidence")?;
            if evidence.is_empty()
                || evidence.len() > 4
                || !evidence.iter().all(|item| {
                    item.as_str()
                        .is_some_and(|value| !value.trim().is_empty() && value.len() <= 256)
                })
            {
                return Err("invalid ranking evidence");
            }
        }
        let queued = format!(
            "{task}\n\nRanked audit targets (highest first): {}",
            serde_json::to_string(targets).map_err(|_| "invalid ranked audit targets")?
        );
        if queued.len() > 4096 {
            return Err("ranked audit target handoff is too long");
        }
        return Ok((role, queued));
    }
    Ok((role, task.to_owned()))
}

pub fn entry_role(id: &str) -> Option<ProfileId> {
    Some(graph_definition(id)?.entry)
}

/// Earlier Research Pipeline sessions started in Discovery and have no Audit Targets model.
pub fn entry_role_for_config(id: &str, config: &SessionGraphConfig) -> Option<ProfileId> {
    if id == OPENAI_ID
        && !config
            .models
            .iter()
            .any(|model| model.node_id == ProfileId::AuditTargets.id())
        && config
            .models
            .iter()
            .any(|model| model.node_id == ProfileId::Discovery.id())
    {
        return Some(ProfileId::Discovery);
    }
    entry_role(id)
}

pub const SIMPLE_ID: &str = "simple";
pub const CLOUDFLARE_ID: &str = "cloudflare_harness";
pub const OPENAI_ID: &str = "openai_pipeline";
pub const SEQUENTIAL_ID: &str = "sequential_pipeline";

pub fn is_pipeline_graph(id: &str) -> bool {
    graph_definition(id).is_some_and(|graph| {
        graph
            .handoffs
            .iter()
            .any(|handoff| handoff.payload == "ranked_targets@1")
    })
}

pub fn is_peer_graph(id: &str) -> bool {
    graph_definition(id).is_some_and(|graph| graph.mode != GraphMode::Single)
}

pub fn is_rotating_graph(id: &str) -> bool {
    graph_definition(id).is_some_and(|graph| graph.mode == GraphMode::RotatingStages)
}
pub const SIMPLE_NAME: &str = "Simple Orchestra";
pub const SIMPLE_DESCRIPTION: &str = "One main agent with a single layer of Mapper, Investigator, Experimenter, Challenger, and Builder subagents.";

mod profiles;
pub use profiles::{ProfileId, task_profiles};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Edge {
    pub from: ProfileId,
    pub to: ProfileId,
    pub bidirectional: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeLayout {
    pub role: ProfileId,
    pub x: f32,
    pub y: f32,
}

pub struct GraphDefinition {
    pub id: &'static str,
    pub revision: u32,
    pub name: &'static str,
    pub description: &'static str,
    pub entry: ProfileId,
    pub mode: GraphMode,
    pub max_active_top_level: usize,
    pub default_stage_minutes: u32,
    pub grace_minutes: u32,
    pub stages: &'static [Stage],
    pub nodes: &'static [ProfileId],
    pub edges: &'static [Edge],
    pub handoffs: &'static [Handoff],
    pub layout: &'static [NodeLayout],
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GraphMode {
    Single,
    Concurrent,
    RotatingStages,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WhenEmpty {
    Continue,
    Advance,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Stage {
    pub role: ProfileId,
    pub when_empty: WhenEmpty,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Handoff {
    pub from: ProfileId,
    pub to: ProfileId,
    pub payload: &'static str,
}

pub static SIMPLE_GRAPH: LazyLock<GraphDefinition> = LazyLock::new(|| catalog::load(SIMPLE_ID));
pub static CLOUDFLARE_GRAPH: LazyLock<GraphDefinition> =
    LazyLock::new(|| catalog::load(CLOUDFLARE_ID));
pub static OPENAI_GRAPH: LazyLock<GraphDefinition> = LazyLock::new(|| catalog::load(OPENAI_ID));
pub static SEQUENTIAL_GRAPH: LazyLock<GraphDefinition> =
    LazyLock::new(|| catalog::load(SEQUENTIAL_ID));

pub fn graph_definition(id: &str) -> Option<&'static GraphDefinition> {
    match id {
        SIMPLE_ID => Some(&SIMPLE_GRAPH),
        CLOUDFLARE_ID => Some(&CLOUDFLARE_GRAPH),
        OPENAI_ID => Some(&OPENAI_GRAPH),
        SEQUENTIAL_ID => Some(&SEQUENTIAL_GRAPH),
        _ => Some(CUSTOM_GRAPHS.read().ok()?.get(id)?.definition),
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
        assert_eq!(SIMPLE_GRAPH.nodes.len(), 6);
        assert_eq!(SIMPLE_GRAPH.edges.len(), 5);
        assert_eq!(SIMPLE_GRAPH.layout.len(), SIMPLE_GRAPH.nodes.len());
        assert!(
            SIMPLE_GRAPH
                .edges
                .iter()
                .all(|edge| edge.from == ProfileId::Main && edge.bidirectional)
        );
        for role in [ProfileId::Scout, ProfileId::Reviewer, ProfileId::Prover] {
            assert_eq!(ProfileId::from_id(role.id()), Some(role));
        }
        assert_eq!(ProfileId::Main.default_max_concurrent(), Some(1));
        assert_eq!(ProfileId::Scout.default_max_concurrent(), None);
        assert_eq!(ProfileId::Main.max_models(), 1);
        assert_eq!(ProfileId::Scout.max_models(), 16);
        assert_eq!(ProfileId::Scout.runtime_max_concurrent(), 8);
        assert!(known_graph(SIMPLE_ID));
        assert!(!known_graph("unknown"));
    }

    #[test]
    fn custom_template_is_content_addressed_and_registered() {
        let catalog: Value = serde_json::from_str(include_str!("../graphs.json")).unwrap();
        let mut template = catalog["graphs"][1].clone();
        template["id"] = "example_flow".into();
        let first = serde_json::to_string(&template).unwrap();
        let id = register_template(&first).unwrap();
        assert!(id.starts_with("custom:example_flow:"));
        assert_eq!(template_snapshot(&id).as_deref(), Some(first.as_str()));
        assert_eq!(
            graph_definition(&id).unwrap().entry,
            ProfileId::DiscoveryHarness
        );
        assert_eq!(runtime(&id).unwrap().id(), id);
        assert!(is_peer_graph(&id));
        template["name"] = "Example revised flow".into();
        assert_ne!(register_template(&template.to_string()).unwrap(), id);
        template["scheduler"]["max_active_top_level"] = 1.into();
        let capped_id = register_template(&template.to_string()).unwrap();
        assert_eq!(
            graph_definition(&capped_id).unwrap().max_active_top_level,
            1
        );
        assert!(register_template("{}").is_err());
    }

    #[test]
    fn custom_spawn_tool_only_offers_declared_children_for_current_role() {
        let mut template: Value =
            serde_json::from_str(include_str!("../examples/example-review.json")).unwrap();
        template["nodes"].as_array_mut().unwrap().push(json!({
            "id":"builder","profile":"builder@1","x":150,"y":200
        }));
        template["delegation"] = json!([
            {"parents":["discovery_harness"],"children":["challenger"]},
            {"parents":["validation_system"],"children":["builder"]}
        ]);
        let id = register_template(&template.to_string()).unwrap();
        let discovery = spawn_definition(&id, ProfileId::DiscoveryHarness).unwrap();
        let validation = spawn_definition(&id, ProfileId::ValidationSystem).unwrap();
        let roles = |tool: Value| {
            tool["parameters"]["properties"]["assignments"]["items"]["properties"]["role"]["enum"]
                .clone()
        };
        assert_eq!(roles(discovery), json!(["challenger"]));
        assert_eq!(roles(validation), json!(["builder"]));
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
            stage_duration_minutes: 0,
            template_json: String::new(),
            max_concurrent_workers: 0,
        };
        let engine = runtime(SIMPLE_ID).unwrap();
        assert_eq!(engine.id(), SIMPLE_ID);
        assert_eq!(engine.tool_definition()["name"], SPAWN_TOOL);
        assert_eq!(subagent_tools().len(), 4);
        assert_eq!(subagent_tools()[0]["name"], LIST_TOOL);
        assert_eq!(subagent_tools()[1]["name"], INSPECT_TOOL);
        assert_eq!(subagent_tools()[2]["name"], MESSAGE_TOOL);
        assert_eq!(subagent_tools()[3]["name"], KILL_TOOL);
        assert_eq!(inspect_definition(true)["name"], INSPECT_TOOL);
        assert_eq!(message_definition(true)["name"], MESSAGE_TOOL);
        let section = engine.system_section(&config, &fallback);
        assert!(section.contains("Mapper"));
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
        assert_eq!(assignments[1].role, ProfileId::Reviewer);
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

    fn example_ranked_targets() -> Value {
        json!([{"target":"S1","signals":["reachability","sink"],
            "rationale":"Example entry reaches a parser sink.",
            "evidence_refs":["tool-example-001"]}])
    }

    #[test]
    fn flow_graphs_have_peer_handoffs_and_subagents() {
        for graph in [&CLOUDFLARE_GRAPH, &OPENAI_GRAPH, &SEQUENTIAL_GRAPH] {
            assert_eq!(entry_role(graph.id), Some(graph.nodes[0]));
            assert_eq!(graph.layout.len(), graph.nodes.len());
            assert!(
                graph
                    .nodes
                    .iter()
                    .all(|role| ProfileId::from_id(role.id()) == Some(*role))
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
                            .any(|edge| edge.from == *role && !edge.to.is_top_level())
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
            assert!(guidance.contains("- Investigator (`investigator`): 0:"));
            for edge in graph
                .edges
                .iter()
                .filter(|edge| edge.from.is_top_level() && edge.to.is_top_level())
            {
                let mut handoff = json!({"role":edge.to.id(),"task":"Review example evidence"});
                if edge.from == ProfileId::AuditTargets {
                    handoff["ranked_targets"] = example_ranked_targets();
                }
                assert_eq!(
                    handoff_target(graph.id, edge.from, &handoff).unwrap().0,
                    edge.to
                );
                assert!(handoff_definition(graph.id, edge.from).is_some());
                if edge.bidirectional {
                    let mut reverse = json!({"role":edge.from.id(),"task":"Recheck example claim"});
                    if edge.to == ProfileId::AuditTargets {
                        reverse["ranked_targets"] = example_ranked_targets();
                    }
                    assert_eq!(
                        handoff_target(graph.id, edge.to, &reverse).unwrap().0,
                        edge.from
                    );
                }
            }
        }
        assert_eq!(entry_role(OPENAI_ID), Some(ProfileId::AuditTargets));
        assert_eq!(entry_role(SEQUENTIAL_ID), Some(ProfileId::AuditTargets));
        assert!(
            handoff_target(
                SEQUENTIAL_ID,
                ProfileId::PatchGeneration,
                &json!({"role":"audit_targets","task":"Start the next scouting cycle"})
            )
            .is_ok()
        );
        assert!(
            handoff_target(
                OPENAI_ID,
                ProfileId::PatchGeneration,
                &json!({"role":"audit_targets","task":"Skip stages"})
            )
            .is_err()
        );
        assert_eq!(
            entry_role_for_config(
                OPENAI_ID,
                &SessionGraphConfig {
                    models: vec![GraphNodeModel {
                        node_id: "discovery".to_owned(),
                        ..Default::default()
                    }],
                    ..Default::default()
                }
            ),
            Some(ProfileId::Discovery)
        );
        assert!(
            handoff_target(
                OPENAI_ID,
                ProfileId::AuditTargets,
                &json!({"role":"discovery","task":"Review ranked example targets","ranked_targets":example_ranked_targets()})
            )
            .is_ok()
        );
        let (recipient, queued) = handoff_target(OPENAI_ID, ProfileId::AuditTargets,
            &json!({"role":"discovery","task":"Review example targets","ranked_targets":example_ranked_targets()})).unwrap();
        assert_eq!(recipient, ProfileId::Discovery);
        assert!(queued.contains("Ranked audit targets (highest first)"));
        assert!(queued.contains("tool-example-001"));
        for invalid in [
            json!({"role":"discovery","task":"Missing ranking"}),
            json!({"role":"discovery","task":"Empty ranking","ranked_targets":[]}),
            json!({"role":"discovery","task":"Unsupported signal","ranked_targets":[{"target":"S1","signals":["unknown"],"rationale":"Example","evidence_refs":["tool-example-001"]}]}),
        ] {
            assert!(handoff_target(OPENAI_ID, ProfileId::AuditTargets, &invalid).is_err());
        }
        assert!(
            handoff_target(
                OPENAI_ID,
                ProfileId::AuditTargets,
                &json!({"role":"validation","task":"Skip analysis"})
            )
            .is_err()
        );
        assert!(
            ProfileId::AuditTargets
                .instruction()
                .contains("historical bug precedent")
        );
        assert!(
            ProfileId::AuditTargets
                .instruction()
                .contains("dangerous sinks")
        );
        assert!(
            ProfileId::Discovery
                .instruction()
                .contains("ranked audit targets")
        );
        assert!(
            handoff_target(
                OPENAI_ID,
                ProfileId::Discovery,
                &json!({"role":"reporter","task":"Skip validation"})
            )
            .is_err()
        );
        assert!(
            handoff_target(
                CLOUDFLARE_ID,
                ProfileId::DiscoveryHarness,
                &json!({"role":"scout","task":"Not a peer"})
            )
            .is_err()
        );
        assert!(
            handoff_target(
                CLOUDFLARE_ID,
                ProfileId::DiscoveryHarness,
                &json!({"role":"validation_system","task":" "})
            )
            .is_err()
        );
        assert!(handoff_definition(SIMPLE_ID, ProfileId::Main).is_none());
    }
}
