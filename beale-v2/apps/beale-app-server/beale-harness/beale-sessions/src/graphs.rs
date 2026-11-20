#[cfg(test)]
use std::future::Future;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicI64, AtomicU32, Ordering},
    },
};

use beale_contracts::v1::{
    AuthMethod, GraphNodeModel, ProviderId, SessionGraphConfig, SessionStreamEvent,
    SessionStreamKind, SessionSubagentSummary, SessionToolDetailPage, SessionToolSummary,
};
use beale_graphs::{Assignment, Role};
use beale_providers::{
    AuthMode, OpenAiConnection, Provider, TextMessage, TextRole, ToolStepRequest, input_messages,
    provider_error_message,
};
use beale_storage::{ChildTurnRow, HostDatabase, ModelUsageRow, ToolCallRow};
use beale_tools::{AgentFiles, Redactor, definitions, page};
#[cfg(test)]
use futures_util::future::join_all;
use serde_json::{Value, json};
use tokio::{
    sync::{Mutex, Semaphore, mpsc},
    task::JoinHandle,
};
use uuid::Uuid;

use super::{
    SessionState, append_tool_result, model_tool_preview,
    tool_runtime::{ReviewTarget, ToolRuntime, unix_ms},
    tool_summary as summary, usage_update,
};

pub(super) struct SubagentOutcome {
    pub tools: Vec<ToolCallRow>,
    pub child_turns: Vec<ChildTurnRow>,
}

pub(super) struct ReceiveContext<'a> {
    pub agent_role: Role,
    pub subagent_id: &'a str,
    pub turn_number: i64,
    pub ordinal: i64,
    pub progress: Option<&'a mpsc::Sender<SessionStreamEvent>>,
}

pub(super) async fn received_message(
    from_id: &str,
    from_role: Role,
    message: &str,
    context: ReceiveContext<'_>,
) -> ToolCallRow {
    let ReceiveContext {
        agent_role,
        subagent_id,
        turn_number,
        ordinal,
        progress,
    } = context;
    let id = Uuid::new_v4().to_string();
    let name = beale_graphs::RECEIVE_EVENT.to_owned();
    let summary = format!("Graph receive role: {}", from_role.label());
    let arguments = json!({"from":from_id,"role":from_role.id()}).to_string();
    if let Some(tx) = progress {
        for (kind, output) in [
            (SessionStreamKind::ToolStarted, ""),
            (SessionStreamKind::ToolFinished, message),
        ] {
            let _ = tx
                .send(SessionStreamEvent {
                    kind: kind as i32,
                    agent_role: agent_role.id().to_owned(),
                    subagent_id: subagent_id.to_owned(),
                    tool: Some(SessionToolSummary {
                        id: id.clone(),
                        name: name.clone(),
                        summary: summary.clone(),
                        is_error: false,
                        agent_role: agent_role.id().to_owned(),
                    }),
                    tool_detail: Some(SessionToolDetailPage {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: arguments.clone(),
                        output: output.to_owned(),
                        offset: 0,
                        next_offset: output.len() as u64,
                        total_bytes: output.len() as u64,
                        is_error: false,
                    }),
                    ..Default::default()
                })
                .await;
        }
    }
    ToolCallRow {
        id,
        turn_number,
        ordinal,
        name,
        summary,
        arguments,
        output: message.to_owned(),
        is_error: false,
        agent_role: agent_role.id().to_owned(),
        created_at_ms: unix_ms(),
    }
}

struct ChildEntry {
    role: Role,
    task: String,
    status: &'static str,
    last_message_at: std::time::Instant,
    inbox: Vec<(String, String)>,
    result: Option<String>,
    activity: Vec<Value>,
    activity_offset: usize,
}

const INSPECT_ACTIVITY_LIMIT: usize = 256;
const INSPECT_TEXT_CHARS: usize = 4_000;

pub(super) struct InspectRequest {
    id: String,
    offset: Option<usize>,
    limit: usize,
}

impl InspectRequest {
    pub fn parse(args: &Value) -> Result<Self, &'static str> {
        let object = args.as_object().ok_or("invalid inspect arguments")?;
        if object
            .keys()
            .any(|key| !matches!(key.as_str(), "subagent_id" | "offset" | "limit"))
        {
            return Err("invalid inspect arguments");
        }
        let id = object
            .get("subagent_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 64)
            .ok_or("invalid subagent ID")?
            .to_owned();
        let offset = object
            .get("offset")
            .map(|value| {
                value
                    .as_u64()
                    .and_then(|value| usize::try_from(value).ok())
                    .ok_or("invalid offset")
            })
            .transpose()?;
        let limit = object
            .get("limit")
            .map(|value| {
                value
                    .as_u64()
                    .and_then(|value| usize::try_from(value).ok())
                    .filter(|limit| (1..=40).contains(limit))
                    .ok_or("invalid limit")
            })
            .transpose()?
            .unwrap_or(20);
        Ok(Self { id, offset, limit })
    }
}

fn clipped(text: &str, limit: usize) -> (String, bool) {
    let mut chars = text.chars();
    let clipped: String = chars.by_ref().take(limit).collect();
    (clipped, chars.next().is_some())
}

fn push_activity(entry: &mut ChildEntry, item: Value) {
    entry.activity.push(item);
    if entry.activity.len() > INSPECT_ACTIVITY_LIMIT {
        entry.activity.remove(0);
        entry.activity_offset += 1;
    }
}

async fn record_activity(registry: &Arc<Mutex<Registry>>, id: &str, item: Value) {
    if let Some(entry) = registry.lock().await.entries.get_mut(id) {
        push_activity(entry, item);
    }
}

async fn complete_tool_activity(
    registry: &Arc<Mutex<Registry>>,
    agent_id: &str,
    tool: &ToolCallRow,
    preview: &str,
) {
    let mut state = registry.lock().await;
    if let Some(entry) = state.entries.get_mut(agent_id) {
        let completed = completed_tool_activity(tool, Some(preview));
        if let Some(item) = entry
            .activity
            .iter_mut()
            .rev()
            .find(|item| item["id"] == tool.id)
        {
            *item = completed;
        } else {
            push_activity(entry, completed);
        }
    }
}

async fn inspect_live(registry: &Arc<Mutex<Registry>>, request: &InspectRequest) -> Option<String> {
    let state = registry.lock().await;
    let entry = state.entries.get(&request.id)?;
    Some(inspection_page(
        InspectionView {
            id: &request.id,
            role: entry.role.id(),
            task: &entry.task,
            status: entry.status,
            result: entry.result.as_deref(),
            activity: &entry.activity,
            activity_offset: entry.activity_offset,
        },
        request,
    ))
}

struct InspectionView<'a> {
    id: &'a str,
    role: &'a str,
    task: &'a str,
    status: &'a str,
    result: Option<&'a str>,
    activity: &'a [Value],
    activity_offset: usize,
}

fn inspection_page(view: InspectionView<'_>, request: &InspectRequest) -> String {
    let InspectionView {
        id,
        role,
        task,
        status,
        result,
        activity,
        activity_offset,
    } = view;
    let (result, result_truncated) = result
        .map(|value| clipped(value, INSPECT_TEXT_CHARS))
        .map_or((None, false), |(value, truncated)| (Some(value), truncated));
    let total_items = activity_offset.saturating_add(activity.len());
    let first_offset = request
        .offset
        .unwrap_or(total_items.saturating_sub(request.limit))
        .clamp(activity_offset, total_items);
    let start = first_offset - activity_offset;
    let items: Vec<_> = activity
        .iter()
        .skip(start)
        .take(request.limit)
        .cloned()
        .collect();
    let next_offset = first_offset + items.len();
    json!({"subagent_id":id,"role":role,"task":task,"status":status,"result":result,
        "result_truncated":result_truncated,
        "items":items,"first_offset":first_offset,"next_offset":next_offset,
        "total_items":total_items,"retained_from":activity_offset,
        "has_more":next_offset<total_items})
    .to_string()
}

fn completed_tool_activity(tool: &ToolCallRow, preview: Option<&str>) -> Value {
    let (arguments, arguments_truncated) = clipped(&tool.arguments, 2_000);
    let (output, output_truncated) = clipped(&tool.output, INSPECT_TEXT_CHARS);
    json!({"kind":"tool","id":tool.id,"name":tool.name,
        "summary":clipped(&tool.summary, 512).0,
        "arguments":arguments,"arguments_truncated":arguments_truncated,
        "output":output,"output_truncated":output_truncated,
        "output_bytes":tool.output.len(),"preview":preview.map(|value| clipped(value, INSPECT_TEXT_CHARS).0),
        "status":"finished","is_error":tool.is_error})
}

pub(super) fn inspect_saved(
    db: &HostDatabase,
    session_id: &str,
    parent_role: Role,
    request: &InspectRequest,
) -> anyhow::Result<Option<String>> {
    let Some(child) = db.session_child(session_id, &request.id)? else {
        return Ok(None);
    };
    if child.parent_role != parent_role.id() {
        return Ok(None);
    }
    let messages: Vec<Value> = serde_json::from_str(&child.turn.messages_json)?;
    let mut activity = Vec::with_capacity(messages.len());
    for message in messages {
        match message.get("role").and_then(Value::as_str) {
            Some("assistant" | "reasoning" | "user") => {
                let kind = message["role"].as_str().unwrap_or_default();
                let text = message
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                activity.push(
                    json!({"kind":kind,"text":clipped(text, INSPECT_TEXT_CHARS).0,
                    "text_truncated":text.chars().count()>INSPECT_TEXT_CHARS}),
                );
            }
            Some("tool") => {
                if let Some(id) = message.get("tool_call_id").and_then(Value::as_str)
                    && let Some(tool) = db.tool_call(session_id, id)?
                {
                    activity.push(completed_tool_activity(&tool, None));
                }
            }
            Some("inspect_tool") => {
                if let Some(item) = message.get("activity") {
                    activity.push(item.clone());
                }
            }
            _ => {}
        }
    }
    let status = if child.turn.is_error {
        "failed"
    } else {
        "completed"
    };
    Ok(Some(inspection_page(
        InspectionView {
            id: &request.id,
            role: &child.turn.role,
            task: &child.turn.task,
            status,
            result: Some(&child.turn.result),
            activity: &activity,
            activity_offset: 0,
        },
        request,
    )))
}

#[derive(Default)]
struct Registry {
    entries: HashMap<String, ChildEntry>,
    parent_inbox: Vec<(String, String)>,
}

pub(super) struct Subagents {
    registry: Arc<Mutex<Registry>>,
    handles: Vec<(String, JoinHandle<(ChildOutcome, ChildTurnRow)>)>,
    limits: HashMap<Role, Arc<Semaphore>>,
    mutation_lock: Arc<Mutex<()>>,
    tool_sequence: Arc<AtomicI64>,
    progress: Option<mpsc::Sender<SessionStreamEvent>>,
    parent_role: Option<Role>,
}

impl Subagents {
    pub fn new(config: &SessionGraphConfig) -> Self {
        let limits = [Role::Scout, Role::Reviewer, Role::Prover]
            .into_iter()
            .map(|role| {
                let max = config
                    .concurrency
                    .iter()
                    .find(|item| item.node_id == role.id())
                    .map_or(beale_graphs::MAX_BATCH, |item| item.max_concurrent as usize)
                    .clamp(1, beale_graphs::MAX_BATCH);
                (role, Arc::new(Semaphore::new(max)))
            })
            .collect();
        Self {
            registry: Arc::new(Mutex::new(Registry::default())),
            handles: Vec::new(),
            limits,
            mutation_lock: Arc::new(Mutex::new(())),
            tool_sequence: Arc::new(AtomicI64::new(0)),
            progress: None,
            parent_role: None,
        }
    }

    pub async fn spawn(
        &mut self,
        assignments: Vec<Assignment>,
        context: ChildContext,
    ) -> Result<String, &'static str> {
        if self
            .registry
            .lock()
            .await
            .entries
            .len()
            .saturating_add(assignments.len())
            > beale_graphs::MAX_CONCURRENT as usize
        {
            return Err("subagent limit reached for this parent agent");
        }
        if let Some(tx) = context.progress.as_ref() {
            let _ = tx
                .send(SessionStreamEvent {
                    kind: SessionStreamKind::SubagentsSpawned as i32,
                    agent_role: context.parent_role.id().to_owned(),
                    ..Default::default()
                })
                .await;
        }
        self.progress = context.progress.clone();
        self.parent_role = Some(context.parent_role);
        let mut spawned = Vec::new();
        for assignment in assignments {
            let id = Uuid::new_v4().to_string();
            let Some(limit) = self.limits.get(&assignment.role).cloned() else {
                continue;
            };
            self.registry.lock().await.entries.insert(
                id.clone(),
                ChildEntry {
                    role: assignment.role,
                    task: assignment.task.clone(),
                    status: "queued",
                    last_message_at: std::time::Instant::now(),
                    inbox: Vec::new(),
                    result: None,
                    activity: Vec::new(),
                    activity_offset: 0,
                },
            );
            let registry = Arc::clone(&self.registry);
            let mutation_lock = Arc::clone(&self.mutation_lock);
            let tool_sequence = Arc::clone(&self.tool_sequence);
            let child_context = context.clone();
            let child_id = id.clone();
            let handle = tokio::spawn(async move {
                let _permit = limit.acquire().await.ok();
                if let Some(entry) = registry.lock().await.entries.get_mut(&child_id) {
                    entry.status = "active";
                }
                run_child(
                    assignment,
                    &child_context,
                    &mutation_lock,
                    &tool_sequence,
                    &child_id,
                    &registry,
                )
                .await
            });
            self.handles.push((id.clone(), handle));
            spawned.push(id);
        }
        Ok(format!(
            "Spawned subagents: {}. Use graph_subagents_list to check progress and results.",
            spawned.join(", ")
        ))
    }

    pub async fn list(&self) -> String {
        let registry = self.registry.lock().await;
        let mut items: Vec<_> = registry.entries.iter().map(|(id, entry)| json!({
            "id": id, "role": entry.role.id(), "task": entry.task,
            "status": entry.status,
            "since_last_message_ms": entry.last_message_at.elapsed().as_millis().min(u64::MAX as u128) as u64,
            "result": entry.result,
        })).collect();
        items.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        json!({"subagents": items, "parent_messages": registry.parent_inbox.iter().map(|(from, text)|
            json!({"from":from,"message":text})).collect::<Vec<_>>()}).to_string()
    }

    pub async fn role_of(&self, id: &str) -> Option<Role> {
        self.registry
            .lock()
            .await
            .entries
            .get(id)
            .map(|entry| entry.role)
    }

    pub async fn inspect(&self, request: &InspectRequest) -> Option<String> {
        inspect_live(&self.registry, request).await
    }

    pub async fn message(&self, sender: &str, args: &Value) -> Result<String, &'static str> {
        let recipient = args
            .get("recipient")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 64)
            .ok_or("invalid recipient")?;
        let message = args
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty() && value.len() <= 4096)
            .ok_or("invalid message")?;
        let mut registry = self.registry.lock().await;
        send_message(&mut registry, sender, recipient, message)
    }

    pub async fn kill(&mut self, args: &Value) -> Result<String, &'static str> {
        let id = args
            .get("subagent_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 64)
            .ok_or("invalid subagent ID")?;
        let mut registry = self.registry.lock().await;
        let entry = registry.entries.get_mut(id).ok_or("subagent not found")?;
        if !matches!(entry.status, "queued" | "active") {
            return Err("subagent is not active");
        }
        let (_, handle) = self
            .handles
            .iter()
            .find(|(agent_id, _)| agent_id == id)
            .ok_or("subagent task not found")?;
        handle.abort();
        entry.status = "killed";
        entry.result = Some("Stopped by parent agent.".to_owned());
        entry.last_message_at = std::time::Instant::now();
        let role = entry.role;
        let task = entry.task.clone();
        drop(registry);
        self.emit_stopped(id, role, task).await;
        Ok(format!("Stopped subagent {id}."))
    }

    pub async fn abort_all(&mut self) {
        for (id, handle) in &self.handles {
            if handle.is_finished() {
                continue;
            }
            handle.abort();
            if let Some(entry) = self.registry.lock().await.entries.get_mut(id) {
                entry.status = "killed";
                entry.result = Some("Stopped by user.".to_owned());
                entry.last_message_at = std::time::Instant::now();
                let role = entry.role;
                let task = entry.task.clone();
                self.emit_stopped(id, role, task).await;
            }
        }
    }

    async fn emit_stopped(&self, id: &str, role: Role, task: String) {
        if let (Some(tx), Some(parent)) = (&self.progress, self.parent_role) {
            let _ = tx
                .send(SessionStreamEvent {
                    kind: SessionStreamKind::SubagentCompleted as i32,
                    agent_role: parent.id().to_owned(),
                    subagent_id: id.to_owned(),
                    subagent: Some(SessionSubagentSummary {
                        id: id.to_owned(),
                        parent_role: parent.id().to_owned(),
                        role: role.id().to_owned(),
                        task,
                        status: "killed".to_owned(),
                        completed_at_ms: now_ms() as u64,
                    }),
                    ..Default::default()
                })
                .await;
        }
    }

    pub async fn parent_messages(&self) -> Vec<(String, String)> {
        std::mem::take(&mut self.registry.lock().await.parent_inbox)
    }

    pub async fn finish(mut self) -> SubagentOutcome {
        let mut tools = Vec::new();
        let mut child_turns = Vec::new();
        for (id, handle) in self.handles.drain(..) {
            match handle.await {
                Ok((outcome, transcript)) => {
                    tools.extend(outcome.tools);
                    child_turns.push(transcript);
                }
                Err(error) if error.is_cancelled() => {
                    let registry = self.registry.lock().await;
                    if let Some(entry) = registry.entries.get(&id) {
                        let messages: Vec<Value> = entry
                            .activity
                            .iter()
                            .map(|item| {
                                if item["kind"] == "tool" {
                                    json!({"role":"inspect_tool","activity":item})
                                } else {
                                    json!({"role":item["kind"],"text":item["text"]})
                                }
                            })
                            .collect();
                        let reasoning: Vec<_> = entry
                            .activity
                            .iter()
                            .filter(|item| item["kind"] == "reasoning")
                            .filter_map(|item| item["text"].as_str())
                            .collect();
                        child_turns.push(ChildTurnRow {
                            agent_id: id,
                            role: entry.role.id().to_owned(),
                            task: entry.task.clone(),
                            result: entry
                                .result
                                .clone()
                                .unwrap_or_else(|| "Stopped by parent agent.".to_owned()),
                            is_error: true,
                            messages_json: serde_json::to_string(&messages)
                                .unwrap_or_else(|_| "[]".to_owned()),
                            reasoning_json: serde_json::to_string(&reasoning)
                                .unwrap_or_else(|_| "[]".to_owned()),
                            completed_at_ms: now_ms(),
                        });
                    }
                }
                Err(error) => beale_host_log::error!("graph subagent task failed: {error}"),
            }
        }
        tools.sort_by_key(|tool| tool.ordinal);
        child_turns.sort_by_key(|child| child.completed_at_ms);
        SubagentOutcome { tools, child_turns }
    }
}

impl Drop for Subagents {
    fn drop(&mut self) {
        for (_, handle) in &self.handles {
            handle.abort();
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}

fn send_message(
    registry: &mut Registry,
    sender: &str,
    recipient: &str,
    message: &str,
) -> Result<String, &'static str> {
    if sender != "parent" && !registry.entries.contains_key(sender) {
        return Err("sender not found");
    }
    if sender == recipient {
        return Err("invalid recipient");
    }
    if recipient == "parent" {
        if sender == "parent" {
            return Err("invalid recipient");
        }
        if registry.parent_inbox.len() >= 128 {
            return Err("parent inbox is full");
        }
        registry
            .parent_inbox
            .push((sender.to_owned(), message.to_owned()));
    } else {
        let target = registry
            .entries
            .get_mut(recipient)
            .ok_or("recipient not found")?;
        if !matches!(target.status, "queued" | "active") {
            return Err("recipient is not active");
        }
        if target.inbox.len() >= 32 {
            return Err("subagent inbox is full");
        }
        target.inbox.push((sender.to_owned(), message.to_owned()));
    }
    if sender != "parent"
        && let Some(source) = registry.entries.get_mut(sender)
    {
        source.last_message_at = std::time::Instant::now();
    }
    Ok(format!("Message sent to {recipient}."))
}

pub(super) fn provider_for(model: &GraphNodeModel) -> Option<(Provider, AuthMode)> {
    let provider = match ProviderId::try_from(model.provider).ok()? {
        ProviderId::Openai => Provider::OpenAi,
        ProviderId::Xai => Provider::Xai,
        ProviderId::Anthropic => Provider::Anthropic,
        ProviderId::Zai => Provider::Zai,
        ProviderId::Openrouter => Provider::OpenRouter,
        ProviderId::Custom => Provider::Custom,
        ProviderId::Unspecified => return None,
    };
    let mode = match AuthMethod::try_from(model.method).ok()? {
        AuthMethod::Subscription => AuthMode::Subscription,
        AuthMethod::ApiKey => AuthMode::ApiKey,
        AuthMethod::Unspecified => return None,
    };
    if provider == Provider::Custom && mode != AuthMode::ApiKey {
        return None;
    }
    Some((provider, mode))
}

struct ChildOutcome {
    result: String,
    is_error: bool,
    tools: Vec<ToolCallRow>,
}

#[derive(Clone)]
pub(super) struct ChildContext {
    pub parent_role: Role,
    pub state: SessionState,
    pub workspace: PathBuf,
    pub agent_files: Arc<AgentFiles>,
    pub workspace_id: String,
    pub session_id: String,
    pub redactor: Arc<Redactor>,
    /// Definitions of enabled features whose descriptors allow subagents.
    pub feature_tools: Arc<Vec<Value>>,
    pub workspace_context: String,
    pub surfaces_context: String,
    pub base_system: String,
    pub reasoning_effort: Option<String>,
    pub turn_number: i64,
    pub turn_started: std::time::Instant,
    pub progress: Option<mpsc::Sender<SessionStreamEvent>>,
    pub message_sequence: Arc<AtomicU32>,
    pub model_usage: Arc<Mutex<Vec<ModelUsageRow>>>,
}

async fn run_child(
    assignment: Assignment,
    context: &ChildContext,
    mutation_lock: &Mutex<()>,
    tool_sequence: &AtomicI64,
    agent_id: &str,
    registry: &Arc<Mutex<Registry>>,
) -> (ChildOutcome, ChildTurnRow) {
    let role = assignment.role.id().to_owned();
    let task = if context.parent_role == Role::Main {
        assignment.task.clone()
    } else {
        format!("From {}: {}", context.parent_role.label(), assignment.task)
    };
    if let Some(tx) = context.progress.as_ref() {
        let _ = tx
            .send(SessionStreamEvent {
                kind: SessionStreamKind::SubagentStarted as i32,
                agent_role: context.parent_role.id().to_owned(),
                subagent_id: agent_id.to_owned(),
                subagent: Some(SessionSubagentSummary {
                    id: agent_id.to_owned(),
                    parent_role: context.parent_role.id().to_owned(),
                    role: role.clone(),
                    task: task.clone(),
                    status: "active".to_owned(),
                    completed_at_ms: 0,
                }),
                ..Default::default()
            })
            .await;
    }
    let mut messages = Vec::new();
    let mut reasoning = Vec::new();
    let outcome = run_child_inner(
        assignment,
        context,
        mutation_lock,
        tool_sequence,
        agent_id,
        registry,
        (&mut messages, &mut reasoning),
    )
    .await;
    let completed_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        });
    {
        let mut state = registry.lock().await;
        if let Some(entry) = state.entries.get_mut(agent_id) {
            entry.status = if outcome.is_error {
                "failed"
            } else {
                "completed"
            };
            entry.result = Some(outcome.result.clone());
            entry.last_message_at = std::time::Instant::now();
        }
        state.parent_inbox.push((
            agent_id.to_owned(),
            format!(
                "Subagent {}. Use graph_subagents_list to inspect its result.",
                if outcome.is_error {
                    "failed"
                } else {
                    "completed"
                }
            ),
        ));
    }
    if let Some(tx) = context.progress.as_ref() {
        let _ = tx
            .send(SessionStreamEvent {
                kind: SessionStreamKind::SubagentCompleted as i32,
                agent_role: context.parent_role.id().to_owned(),
                subagent_id: agent_id.to_owned(),
                subagent: Some(SessionSubagentSummary {
                    id: agent_id.to_owned(),
                    parent_role: context.parent_role.id().to_owned(),
                    role: role.clone(),
                    task: task.clone(),
                    status: if outcome.is_error {
                        "failed"
                    } else {
                        "completed"
                    }
                    .to_owned(),
                    completed_at_ms: completed_at_ms as u64,
                }),
                ..Default::default()
            })
            .await;
    }
    let transcript = ChildTurnRow {
        agent_id: agent_id.to_owned(),
        role,
        task,
        result: outcome.result.clone(),
        is_error: outcome.is_error,
        messages_json: serde_json::to_string(&messages).unwrap_or_else(|_| "[]".to_owned()),
        reasoning_json: serde_json::to_string(&reasoning).unwrap_or_else(|_| "[]".to_owned()),
        completed_at_ms,
    };
    (outcome, transcript)
}

fn child_tool_definitions(feature_tools: &[Value]) -> Vec<Value> {
    let mut tools = definitions();
    tools.push(beale_graphs::message_definition(true));
    tools.push(beale_graphs::inspect_definition(true));
    tools.extend(feature_tools.iter().cloned());
    tools
}

async fn run_child_inner(
    assignment: Assignment,
    context: &ChildContext,
    mutation_lock: &Mutex<()>,
    tool_sequence: &AtomicI64,
    agent_id: &str,
    registry: &Arc<Mutex<Registry>>,
    transcript: (&mut Vec<Value>, &mut Vec<String>),
) -> ChildOutcome {
    let (messages, reasoning) = transcript;
    let Some((provider, mode)) = provider_for(&assignment.model) else {
        return ChildOutcome {
            result: "Configured subagent provider is invalid.".to_owned(),
            is_error: true,
            tools: Vec::new(),
        };
    };
    let system = format!(
        "{}\n\n## {} subagent for {}\n\n{}\n\nYou have a fresh context. Work only on the assigned task. You cannot spawn or kill subagents. Use graph_subagents_message to contact your parent (`parent`) or a sibling ID, and graph_subagents_inspect to read a sibling's work. Return a concise result with evidence and uncertainties to your parent agent.",
        context.base_system,
        assignment.role.label(),
        context.parent_role.label(),
        assignment.role.instruction()
    );
    let task = format!(
        "Task: {}\n\nIf scope details are missing, state the gap in your result so the parent can resolve it. Stay within the recorded authorization boundary.\n\nRecorded surfaces at delegation:\n{}",
        assignment.task, context.surfaces_context
    );
    let mut input = input_messages(&[TextMessage {
        role: TextRole::User,
        text: task,
    }]);
    if provider != Provider::OpenAi {
        input.insert(0, json!({"role":"system","content":system}));
    }
    let mut connection = OpenAiConnection::default();
    let definitions = child_tool_definitions(&context.feature_tools);
    let no_plugins = HashSet::new();
    let mut tools = Vec::<ToolCallRow>::new();
    loop {
        let incoming = {
            let mut state = registry.lock().await;
            state
                .entries
                .get_mut(agent_id)
                .map(|entry| std::mem::take(&mut entry.inbox))
                .unwrap_or_default()
        };
        if !incoming.is_empty() {
            for (from, text) in &incoming {
                let from_role = if from == "parent" {
                    Some(context.parent_role)
                } else {
                    registry
                        .lock()
                        .await
                        .entries
                        .get(from)
                        .map(|entry| entry.role)
                };
                if let Some(from_role) = from_role {
                    let row = received_message(
                        from,
                        from_role,
                        text,
                        ReceiveContext {
                            agent_role: context.parent_role,
                            subagent_id: agent_id,
                            turn_number: context.turn_number,
                            ordinal: tool_sequence.fetch_add(1, Ordering::SeqCst),
                            progress: context.progress.as_ref(),
                        },
                    )
                    .await;
                    messages.push(json!({"role":"tool","tool_call_id":row.id,"name":row.name,
                        "summary":row.summary,"is_error":false}));
                    record_activity(registry, agent_id, completed_tool_activity(&row, None)).await;
                    tools.push(row);
                }
                record_activity(
                    registry,
                    agent_id,
                    json!({"kind":"user","from":from,"text":clipped(text, INSPECT_TEXT_CHARS).0}),
                )
                .await;
                messages.push(json!({"role":"user","text":format!("From {from}: {text}")}));
            }
            input.extend(input_messages(&[TextMessage {
                role: TextRole::User,
                text: format!(
                    "Messages from your graph group:\n{}",
                    incoming
                        .iter()
                        .map(|(from, text)| format!("From {from}: {text}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
            }]));
        }
        let request = ToolStepRequest {
            model: &assignment.model.model,
            input: &input,
            tools: &definitions,
            instructions: (provider == Provider::OpenAi).then_some(system.as_str()),
            reasoning_effort: context.reasoning_effort.as_deref(),
            workspace_root: Some(&context.workspace),
        };
        let call_started = std::time::Instant::now();
        let step = if provider == Provider::OpenAi {
            context
                .state
                .provider
                .openai_tool_step(
                    mode,
                    request,
                    &context.state.credentials,
                    None,
                    &mut connection,
                )
                .await
        } else {
            context
                .state
                .provider
                .tool_step(provider, mode, request, &context.state.credentials, None)
                .await
        };
        let step = match step {
            Ok(step) => step,
            Err(error) => {
                beale_host_log::error!(
                    "graph subagent provider failed for {} ({provider:?}, {}): {error:#}",
                    assignment.role.id(),
                    assignment.model.model
                );
                return ChildOutcome {
                    result: provider_error_message(provider, mode, &error),
                    is_error: true,
                    tools,
                };
            }
        };
        let usage = ModelUsageRow {
            agent_id: agent_id.to_owned(),
            role: assignment.role.id().to_owned(),
            provider: assignment.model.provider,
            method: assignment.model.method,
            model: assignment.model.model.clone(),
            input_tokens: step.usage.map(|usage| usage.input_tokens as i64),
            output_tokens: step.usage.map(|usage| usage.output_tokens as i64),
            cache_read_tokens: step
                .usage
                .and_then(|usage| usage.cache_read_tokens.map(|count| count as i64)),
            work_elapsed_ms: Some(
                context
                    .turn_started
                    .elapsed()
                    .as_millis()
                    .min(i64::MAX as u128) as i64,
            ),
            duration_ms: call_started.elapsed().as_millis().min(i64::MAX as u128) as i64,
            observed_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| {
                    duration.as_millis().min(i64::MAX as u128) as i64
                }),
        };
        if let Some(tx) = context.progress.as_ref() {
            let mut event = usage_update(&usage, context.parent_role.id());
            event.subagent_id = agent_id.to_owned();
            let _ = tx.send(event).await;
        }
        context.model_usage.lock().await.push(usage);
        for text in &step.reasoning_summaries {
            record_activity(
                registry,
                agent_id,
                json!({"kind":"reasoning",
                "text":clipped(text, INSPECT_TEXT_CHARS).0,
                "text_truncated":text.chars().count()>INSPECT_TEXT_CHARS}),
            )
            .await;
            messages.push(
                json!({"role":"reasoning","text":text.chars().take(8_000).collect::<String>()}),
            );
            if let Some(tx) = context.progress.as_ref() {
                let _ = tx
                    .send(SessionStreamEvent {
                        kind: SessionStreamKind::ReasoningDelta as i32,
                        text: text.clone(),
                        segment_index: context.message_sequence.fetch_add(1, Ordering::SeqCst),
                        agent_role: context.parent_role.id().to_owned(),
                        subagent_id: agent_id.to_owned(),
                        ..Default::default()
                    })
                    .await;
            }
        }
        if !step.text.trim().is_empty()
            && let Some(tx) = context.progress.as_ref()
        {
            let _ = tx
                .send(SessionStreamEvent {
                    kind: SessionStreamKind::MessageDelta as i32,
                    text: step.text.clone(),
                    segment_index: context.message_sequence.fetch_add(1, Ordering::SeqCst),
                    agent_role: context.parent_role.id().to_owned(),
                    subagent_id: agent_id.to_owned(),
                    ..Default::default()
                })
                .await;
        }
        if !step.text.is_empty() {
            if let Some(entry) = registry.lock().await.entries.get_mut(agent_id) {
                entry.last_message_at = std::time::Instant::now();
            }
            record_activity(
                registry,
                agent_id,
                json!({"kind":"assistant",
                "text":clipped(&step.text, INSPECT_TEXT_CHARS).0,
                "text_truncated":step.text.chars().count()>INSPECT_TEXT_CHARS}),
            )
            .await;
            messages.push(json!({"role":"assistant","text":step.text.chars().take(32_000).collect::<String>()}));
        }
        reasoning.extend(
            step.reasoning_summaries
                .iter()
                .map(|text| text.chars().take(8_000).collect()),
        );
        input.extend(step.continuation);
        if step.calls.is_empty() {
            if registry
                .lock()
                .await
                .entries
                .get(agent_id)
                .is_some_and(|entry| !entry.inbox.is_empty())
            {
                continue;
            }
            let result = step.text.trim();
            if result.is_empty() {
                return ChildOutcome {
                    result: "Subagent completed without a result.".to_owned(),
                    is_error: true,
                    tools,
                };
            }
            return ChildOutcome {
                result: result.to_owned(),
                is_error: false,
                tools,
            };
        }
        let mut anthropic_results = Vec::new();
        for call in step.calls {
            if call.id.is_empty()
                || call.id.len() > 256
                || call.name.len() > 128
                || call.arguments.len() > 65_536
            {
                return ChildOutcome {
                    result: "Subagent returned an invalid tool call.".to_owned(),
                    is_error: true,
                    tools,
                };
            }
            let id = Uuid::new_v4().to_string();
            let ordinal = tool_sequence.fetch_add(1, Ordering::SeqCst);
            let arguments: Result<Value, _> = serde_json::from_str(&call.arguments);
            let label = if matches!(
                call.name.as_str(),
                beale_graphs::INSPECT_TOOL | beale_graphs::MESSAGE_TOOL
            ) {
                let field = if call.name == beale_graphs::INSPECT_TOOL {
                    "subagent_id"
                } else {
                    "recipient"
                };
                let target = arguments
                    .as_ref()
                    .ok()
                    .and_then(|value| value.get(field))
                    .and_then(Value::as_str);
                let target_role = if target == Some("parent") {
                    Some(context.parent_role)
                } else {
                    match target {
                        Some(id) => registry
                            .lock()
                            .await
                            .entries
                            .get(id)
                            .map(|entry| entry.role),
                        None => None,
                    }
                };
                let action = if call.name == beale_graphs::INSPECT_TOOL {
                    "inspect"
                } else {
                    "message"
                };
                target_role.map_or_else(
                    || format!("Graph {action}"),
                    |role| format!("Graph {action} role: {}", role.label()),
                )
            } else {
                summary(&call.name, arguments.as_ref().unwrap_or(&Value::Null))
            };
            record_activity(
                registry,
                agent_id,
                json!({"kind":"tool","id":id,
                "name":call.name,"summary":clipped(&label,512).0,
                "arguments":clipped(&call.arguments,2_000).0,
                "arguments_truncated":call.arguments.chars().count()>2_000,
                "status":"running"}),
            )
            .await;
            if let Some(tx) = context.progress.as_ref() {
                let _ = tx
                    .send(SessionStreamEvent {
                        runbook_progress: None,
                        kind: SessionStreamKind::ToolStarted as i32,
                        text: String::new(),
                        tool: Some(SessionToolSummary {
                            id: id.clone(),
                            name: call.name.clone(),
                            summary: label.clone(),
                            is_error: false,
                            agent_role: context.parent_role.id().to_owned(),
                        }),
                        detail: None,
                        segment_index: 0,
                        agent_role: context.parent_role.id().to_owned(),
                        tool_detail: Some(beale_contracts::v1::SessionToolDetailPage {
                            id: id.clone(),
                            name: call.name.clone(),
                            arguments: call.arguments.clone(),
                            output: String::new(),
                            offset: 0,
                            next_offset: 0,
                            total_bytes: 0,
                            is_error: false,
                        }),
                        usage_delta: None,
                        subagent_id: agent_id.to_owned(),
                        subagent: None,
                    })
                    .await;
            }
            let runtime = ToolRuntime {
                state: &context.state,
                workspace_id: &context.workspace_id,
                session_id: &context.session_id,
                agent_id,
                agent_role: context.parent_role.id(),
                workspace: &context.workspace,
                review: ReviewTarget {
                    provider,
                    mode,
                    model: &assignment.model.model,
                    context: &context.workspace_context,
                },
                definitions: &definitions,
                callable_plugins: &no_plugins,
                redactor: &context.redactor,
                mutation_lock: Some(mutation_lock),
                progress: context.progress.as_ref(),
                pipeline_role: None,
            };
            let mut no_stop = None;
            let mut review_unavailable = false;
            let (output, is_error) = match arguments {
                Ok(arguments) if call.name == beale_graphs::INSPECT_TOOL => {
                    match InspectRequest::parse(&arguments) {
                        Ok(request) => match inspect_live(registry, &request).await {
                            Some(value) => (value, false),
                            None => ("Subagent not found in this parent group.".to_owned(), true),
                        },
                        Err(error) => (error.to_owned(), true),
                    }
                }
                Ok(arguments) if call.name == beale_graphs::MESSAGE_TOOL => {
                    let recipient = arguments.get("recipient").and_then(Value::as_str);
                    let message = arguments
                        .get("message")
                        .and_then(Value::as_str)
                        .map(str::trim);
                    match (recipient, message) {
                        (Some(recipient), Some(message))
                            if !message.is_empty()
                                && message.len() <= 4096
                                && !recipient.is_empty()
                                && recipient.len() <= 64 =>
                        {
                            let mut state = registry.lock().await;
                            match send_message(&mut state, agent_id, recipient, message) {
                                Ok(value) => (value, false),
                                Err(error) => (error.to_owned(), true),
                            }
                        }
                        _ => ("Invalid subagent message.".to_owned(), true),
                    }
                }
                arguments => {
                    let result = runtime
                        .dispatch(&id, &call.name, arguments, &tools, &mut no_stop)
                        .await;
                    review_unavailable = result.review_unavailable;
                    (result.output, result.is_error)
                }
            };
            let model_output = {
                if let Some(tx) = context.progress.as_ref() {
                    let _ = tx
                        .send(SessionStreamEvent {
                            runbook_progress: None,
                            kind: SessionStreamKind::ToolFinished as i32,
                            text: String::new(),
                            tool: Some(SessionToolSummary {
                                id: id.clone(),
                                name: call.name.clone(),
                                summary: label.clone(),
                                is_error,
                                agent_role: context.parent_role.id().to_owned(),
                            }),
                            detail: None,
                            segment_index: 0,
                            agent_role: context.parent_role.id().to_owned(),
                            tool_detail: Some(beale_contracts::v1::SessionToolDetailPage {
                                id: id.clone(),
                                name: call.name.clone(),
                                arguments: call.arguments.clone(),
                                output: page(&output, 0, super::TOOL_PAGE_BYTES)
                                    .map_or_else(|_| String::new(), |(first, _)| first),
                                offset: 0,
                                next_offset: page(&output, 0, super::TOOL_PAGE_BYTES)
                                    .map_or(0, |(_, next)| next as u64),
                                total_bytes: output.len() as u64,
                                is_error,
                            }),
                            usage_delta: None,
                            subagent_id: agent_id.to_owned(),
                            subagent: None,
                        })
                        .await;
                }
                messages.push(json!({"role":"tool","tool_call_id":id,"name":call.name,
                    "summary":label,"is_error":is_error}));
                tools.push(ToolCallRow {
                    id: id.clone(),
                    turn_number: context.turn_number,
                    ordinal,
                    name: call.name.clone(),
                    summary: label,
                    arguments: call.arguments,
                    output: output.clone(),
                    is_error,
                    agent_role: context.parent_role.id().to_owned(),
                    created_at_ms: unix_ms(),
                });
                model_tool_preview(&call.name, &id, &output, is_error, &context.agent_files)
            };
            if let Some(tool) = tools.last() {
                complete_tool_activity(registry, agent_id, tool, &model_output).await;
            }
            append_tool_result(
                &mut input,
                &mut anthropic_results,
                provider,
                mode,
                &call.id,
                &model_output,
            );
            if review_unavailable {
                return ChildOutcome {
                    result: "Auto-Review was unavailable; the tool was not run. Retry this subagent after the review service is available.".to_owned(),
                    is_error: true,
                    tools,
                };
            }
        }
        if !anthropic_results.is_empty() {
            input.push(json!({"role":"user","content":anthropic_results}));
        }
    }
}

#[cfg(test)]
async fn schedule<T, F, Fut>(
    assignments: Vec<Assignment>,
    config: &SessionGraphConfig,
    run: F,
) -> Vec<(Role, String, Option<T>)>
where
    F: Fn(Assignment) -> Fut,
    Fut: Future<Output = T>,
{
    let limits: Vec<_> = [Role::Scout, Role::Reviewer, Role::Prover]
        .into_iter()
        .map(|role| {
            let max = config
                .concurrency
                .iter()
                .find(|item| item.node_id == role.id())
                .map_or(beale_graphs::MAX_BATCH, |item| item.max_concurrent as usize)
                .clamp(1, beale_graphs::MAX_BATCH);
            Arc::new(Semaphore::new(max))
        })
        .collect();
    join_all(assignments.into_iter().map(|assignment| {
        let semaphore = if assignment.role == Role::Main {
            None
        } else {
            limits.get(assignment.role.index() - 1).cloned()
        };
        let run = &run;
        async move {
            let role = assignment.role;
            let task = assignment.task.clone();
            let outcome = match semaphore {
                Some(semaphore) => match semaphore.acquire().await {
                    Ok(_permit) => Some(run(assignment).await),
                    Err(_) => None,
                },
                None => None,
            };
            (role, task, outcome)
        }
    }))
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn received_graph_message_has_stream_and_saved_tool_activity() {
        let (tx, mut rx) = mpsc::channel(4);
        let row = received_message(
            "child-example",
            Role::Scout,
            "Check example evidence",
            ReceiveContext {
                agent_role: Role::Main,
                subagent_id: "",
                turn_number: 1,
                ordinal: 3,
                progress: Some(&tx),
            },
        )
        .await;
        assert_eq!(row.name, beale_graphs::RECEIVE_EVENT);
        assert_eq!(row.summary, "Graph receive role: Scout");
        assert_eq!(row.output, "Check example evidence");
        assert_eq!(
            rx.recv().await.unwrap().kind,
            SessionStreamKind::ToolStarted as i32
        );
        let finished = rx.recv().await.unwrap();
        assert_eq!(finished.kind, SessionStreamKind::ToolFinished as i32);
        assert_eq!(finished.tool.unwrap().id, row.id);
        assert_eq!(finished.tool_detail.unwrap().output, row.output);
    }

    fn example_entry(role: Role) -> ChildEntry {
        ChildEntry {
            role,
            task: "Inspect the example module".to_owned(),
            status: "active",
            last_message_at: std::time::Instant::now() - std::time::Duration::from_secs(2),
            inbox: Vec::new(),
            result: None,
            activity: Vec::new(),
            activity_offset: 0,
        }
    }

    #[tokio::test]
    async fn messages_stay_within_one_parent_group_and_status_reports_activity_age() {
        let manager = Subagents::new(&SessionGraphConfig::default());
        let mut group = manager.registry.lock().await;
        group
            .entries
            .insert("child-a".to_owned(), example_entry(Role::Scout));
        group
            .entries
            .insert("child-b".to_owned(), example_entry(Role::Reviewer));
        assert!(send_message(&mut group, "parent", "child-a", "Check boundary").is_ok());
        assert!(send_message(&mut group, "child-a", "child-b", "Review evidence").is_ok());
        assert!(send_message(&mut group, "child-b", "parent", "Evidence ready").is_ok());
        assert_eq!(
            send_message(&mut group, "child-a", "outside", "test"),
            Err("recipient not found")
        );
        assert_eq!(
            send_message(&mut group, "outside", "parent", "test"),
            Err("sender not found")
        );
        assert_eq!(group.entries["child-a"].inbox[0].1, "Check boundary");
        assert_eq!(group.entries["child-b"].inbox[0].0, "child-a");
        drop(group);
        let list: Value = serde_json::from_str(&manager.list().await).unwrap();
        assert_eq!(list["subagents"].as_array().unwrap().len(), 2);
        assert_eq!(list["parent_messages"][0]["message"], "Evidence ready");
        assert!(
            list["subagents"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["id"] == "child-b"
                    && item["since_last_message_ms"].as_u64().unwrap() < 1_000)
        );
    }

    #[tokio::test]
    async fn inspect_shows_live_tool_details_and_pages_within_the_parent_group() {
        let manager = Subagents::new(&SessionGraphConfig::default());
        manager
            .registry
            .lock()
            .await
            .entries
            .insert("child-a".to_owned(), example_entry(Role::Scout));
        record_activity(
            &manager.registry,
            "child-a",
            json!({"kind":"assistant","text":"Checking example input"}),
        )
        .await;
        let tool = ToolCallRow {
            id: "tool-example-a".to_owned(),
            turn_number: 1,
            ordinal: 0,
            name: "read".to_owned(),
            summary: "Read example.txt".to_owned(),
            arguments: json!({"path":"example.txt"}).to_string(),
            output: "Example file contents".to_owned(),
            is_error: false,
            agent_role: "main".to_owned(),
            created_at_ms: 0,
        };
        record_activity(
            &manager.registry,
            "child-a",
            json!({"kind":"tool","id":tool.id,
            "name":tool.name,"status":"running"}),
        )
        .await;
        let running = InspectRequest::parse(&json!({"subagent_id":"child-a"})).unwrap();
        let running_page: Value =
            serde_json::from_str(&manager.inspect(&running).await.unwrap()).unwrap();
        assert_eq!(running_page["items"][1]["status"], "running");
        complete_tool_activity(&manager.registry, "child-a", &tool, "Example file contents").await;
        let finished: Value =
            serde_json::from_str(&manager.inspect(&running).await.unwrap()).unwrap();
        assert_eq!(finished["items"][1]["status"], "finished");
        assert_eq!(finished["items"][1]["arguments"], tool.arguments);
        assert_eq!(finished["items"][1]["output"], tool.output);
        for index in 0..260 {
            record_activity(
                &manager.registry,
                "child-a",
                json!({"kind":"assistant","text":index}),
            )
            .await;
        }
        let first =
            InspectRequest::parse(&json!({"subagent_id":"child-a","offset":0,"limit":2})).unwrap();
        let page: Value = serde_json::from_str(&manager.inspect(&first).await.unwrap()).unwrap();
        assert_eq!(page["retained_from"], 6);
        assert_eq!(page["first_offset"], 6);
        assert_eq!(page["next_offset"], 8);
        let other = Subagents::new(&SessionGraphConfig::default());
        assert!(other.inspect(&running).await.is_none());
        assert!(InspectRequest::parse(&json!({"subagent_id":"child-a","limit":41})).is_err());
    }

    #[tokio::test]
    async fn kill_aborts_a_child_and_retains_a_failed_transcript() {
        let mut manager = Subagents::new(&SessionGraphConfig::default());
        manager
            .registry
            .lock()
            .await
            .entries
            .insert("child-a".to_owned(), example_entry(Role::Scout));
        record_activity(
            &manager.registry,
            "child-a",
            json!({"kind":"assistant","text":"Checking example input"}),
        )
        .await;
        record_activity(
            &manager.registry,
            "child-a",
            json!({"kind":"tool","id":"tool-example-a",
            "name":"read","status":"running"}),
        )
        .await;
        let handle =
            tokio::spawn(async { std::future::pending::<(ChildOutcome, ChildTurnRow)>().await });
        manager.handles.push(("child-a".to_owned(), handle));
        assert!(
            manager
                .kill(&json!({"subagent_id":"child-a"}))
                .await
                .is_ok()
        );
        assert_eq!(
            manager.kill(&json!({"subagent_id":"child-a"})).await,
            Err("subagent is not active")
        );
        let result = manager.finish().await;
        assert_eq!(result.child_turns.len(), 1);
        assert!(result.child_turns[0].is_error);
        assert_eq!(result.child_turns[0].agent_id, "child-a");
        assert!(result.child_turns[0].messages_json.contains("inspect_tool"));
        assert!(
            result.child_turns[0]
                .messages_json
                .contains("Checking example input")
        );
    }

    #[test]
    fn child_catalog_keeps_webfetch_optional() {
        let base = child_tool_definitions(&[]);
        assert_eq!(base.len(), 6);
        assert_eq!(base[4]["name"], beale_graphs::MESSAGE_TOOL);
        assert_eq!(base[5]["name"], beale_graphs::INSPECT_TOOL);
        assert!(
            base.iter()
                .all(|tool| tool["name"] != beale_graphs::SPAWN_TOOL
                    && tool["name"] != beale_graphs::LIST_TOOL
                    && tool["name"] != beale_graphs::KILL_TOOL)
        );
        assert!(base.iter().all(|tool| tool["name"] != "webfetch"));
        #[cfg(feature = "webfetch")]
        {
            let tools = child_tool_definitions(&[beale_webfetch::definition()]);
            assert_eq!(tools.len(), 7);
            assert_eq!(tools.last().unwrap(), &beale_webfetch::definition());
        }
    }

    #[test]
    fn subagent_features_come_from_descriptors() {
        for feature in crate::features::native_features() {
            assert_eq!(
                crate::features::subagents_receive(feature.id),
                matches!(feature.id, "webfetch" | "beale-agent-feedback" | "fleet"),
                "{}",
                feature.id
            );
        }
        for id in ["goal-mode", "graphs", "surfaces", "example-plugin"] {
            assert!(!crate::features::subagents_receive(id));
        }
    }

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn scheduler_runs_roles_concurrently_with_per_role_limits() {
        let model = GraphNodeModel {
            node_id: "scout".to_owned(),
            provider: ProviderId::Openai as i32,
            method: AuthMethod::ApiKey as i32,
            model: "example-model".to_owned(),
        };
        let assignments = [Role::Scout, Role::Scout, Role::Reviewer, Role::Reviewer]
            .into_iter()
            .enumerate()
            .map(|(index, role)| Assignment {
                role,
                task: format!("example-task-{index}"),
                model: model.clone(),
            })
            .collect();
        let config = SessionGraphConfig {
            models: Vec::new(),
            concurrency: vec![beale_contracts::v1::GraphNodeConcurrency {
                node_id: "scout".to_owned(),
                max_concurrent: 1,
            }],
            stage_duration_minutes: 0,
        };
        let active = Arc::new([AtomicUsize::new(0), AtomicUsize::new(0)]);
        let peak = Arc::new([AtomicUsize::new(0), AtomicUsize::new(0)]);
        let results = schedule(assignments, &config, |assignment| {
            let active = active.clone();
            let peak = peak.clone();
            async move {
                let index = if assignment.role == Role::Scout { 0 } else { 1 };
                let count = active[index].fetch_add(1, Ordering::SeqCst) + 1;
                peak[index].fetch_max(count, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(10)).await;
                active[index].fetch_sub(1, Ordering::SeqCst);
                assignment.task
            }
        })
        .await;
        assert_eq!(results.len(), 4);
        assert!(results.iter().all(|(_, _, result)| result.is_some()));
        assert_eq!(peak[0].load(Ordering::SeqCst), 1);
        assert_eq!(peak[1].load(Ordering::SeqCst), 2);
    }
}
