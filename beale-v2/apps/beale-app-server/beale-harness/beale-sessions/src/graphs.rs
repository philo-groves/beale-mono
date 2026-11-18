use std::{
    future::Future,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicI64, AtomicU32, Ordering},
    },
};

use beale_contracts::v1::{
    AuthMethod, GraphNodeModel, ProviderId, SessionGraphConfig, SessionStreamEvent,
    SessionStreamKind, SessionSubagentSummary, SessionToolSummary,
};
use beale_graphs::{Assignment, Role};
use beale_providers::{
    AuthMode, OpenAiConnection, Provider, TextMessage, TextRole, ToolStepRequest, input_messages,
    provider_error_message,
};
use beale_safety::ReviewRequest;
use beale_storage::{ChildTurnRow, ModelUsageRow, ToolCallRow};
use beale_tools::{AgentFiles, definitions, execute, page};
use futures_util::future::join_all;
use serde_json::{Value, json};
use tokio::sync::{Mutex, Semaphore, mpsc};
use uuid::Uuid;

use super::{
    SessionState, append_tool_result, model_tool_preview, tool_summary as summary, usage_update,
};

pub(super) struct DelegateOutcome {
    pub output: String,
    pub is_error: bool,
    pub tools: Vec<ToolCallRow>,
    pub child_turns: Vec<ChildTurnRow>,
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

pub(super) struct ChildContext<'a> {
    pub parent_role: Role,
    pub state: &'a SessionState,
    pub workspace: &'a Path,
    pub agent_files: &'a AgentFiles,
    #[cfg(feature = "webfetch")]
    pub webfetch_enabled: bool,
    #[cfg(any(feature = "agent-feedback", feature = "webfetch"))]
    pub workspace_id: &'a str,
    #[cfg(feature = "agent-feedback")]
    pub session_id: &'a str,
    #[cfg(feature = "agent-feedback")]
    pub feedback_enabled: bool,
    pub workspace_context: &'a str,
    pub surfaces_context: &'a str,
    pub base_system: &'a str,
    pub reasoning_effort: Option<&'a str>,
    pub turn_number: i64,
    pub turn_started: std::time::Instant,
    pub progress: Option<&'a mpsc::Sender<SessionStreamEvent>>,
    pub message_sequence: &'a AtomicU32,
    pub model_usage: &'a Mutex<Vec<ModelUsageRow>>,
}

async fn run_child(
    assignment: Assignment,
    context: &ChildContext<'_>,
    mutation_lock: &Mutex<()>,
    tool_sequence: &AtomicI64,
) -> (ChildOutcome, ChildTurnRow) {
    let agent_id = Uuid::new_v4().to_string();
    let role = assignment.role.id().to_owned();
    let task = if context.parent_role == Role::Main {
        assignment.task.clone()
    } else {
        format!("From {}: {}", context.parent_role.label(), assignment.task)
    };
    if let Some(tx) = context.progress {
        let _ = tx
            .send(SessionStreamEvent {
                kind: SessionStreamKind::SubagentStarted as i32,
                agent_role: context.parent_role.id().to_owned(),
                subagent_id: agent_id.clone(),
                subagent: Some(SessionSubagentSummary {
                    id: agent_id.clone(),
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
        &agent_id,
        &mut messages,
        &mut reasoning,
    )
    .await;
    let completed_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        });
    if let Some(tx) = context.progress {
        let _ = tx
            .send(SessionStreamEvent {
                kind: SessionStreamKind::SubagentCompleted as i32,
                agent_role: context.parent_role.id().to_owned(),
                subagent_id: agent_id.clone(),
                subagent: Some(SessionSubagentSummary {
                    id: agent_id.clone(),
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
        agent_id,
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

fn child_tool_definitions(feedback_enabled: bool, webfetch_enabled: bool) -> Vec<Value> {
    #[cfg(not(feature = "agent-feedback"))]
    let _ = feedback_enabled;
    #[cfg(not(feature = "webfetch"))]
    let _ = webfetch_enabled;
    #[allow(unused_mut)]
    let mut tools = definitions();
    #[cfg(feature = "webfetch")]
    if webfetch_enabled {
        tools.push(beale_webfetch::definition());
    }
    #[cfg(feature = "agent-feedback")]
    if feedback_enabled {
        tools.push(beale_agent_feedback::definition());
    }
    tools
}

pub(super) fn child_supports_feature(id: &str) -> bool {
    let _ = id;
    #[cfg(feature = "webfetch")]
    if id == beale_webfetch::FEATURE_ID {
        return true;
    }
    #[cfg(feature = "agent-feedback")]
    if id == beale_agent_feedback::FEATURE_ID {
        return true;
    }
    false
}

async fn run_child_inner(
    assignment: Assignment,
    context: &ChildContext<'_>,
    mutation_lock: &Mutex<()>,
    tool_sequence: &AtomicI64,
    agent_id: &str,
    messages: &mut Vec<Value>,
    reasoning: &mut Vec<String>,
) -> ChildOutcome {
    let Some((provider, mode)) = provider_for(&assignment.model) else {
        return ChildOutcome {
            result: "Configured subagent provider is invalid.".to_owned(),
            is_error: true,
            tools: Vec::new(),
        };
    };
    let system = format!(
        "{}\n\n## {} subagent for {}\n\n{}\n\nYou have a fresh context. Work only on the assigned task. You cannot delegate. Return a concise result with evidence and uncertainties to your parent agent.",
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
    #[cfg(feature = "webfetch")]
    let webfetch_enabled = context.webfetch_enabled;
    #[cfg(not(feature = "webfetch"))]
    let webfetch_enabled = false;
    #[cfg(feature = "agent-feedback")]
    let feedback_enabled = context.feedback_enabled;
    #[cfg(not(feature = "agent-feedback"))]
    let feedback_enabled = false;
    let definitions = child_tool_definitions(feedback_enabled, webfetch_enabled);
    let mut tools = Vec::<ToolCallRow>::new();
    loop {
        let request = ToolStepRequest {
            model: &assignment.model.model,
            input: &input,
            tools: &definitions,
            instructions: (provider == Provider::OpenAi).then_some(system.as_str()),
            reasoning_effort: context.reasoning_effort,
            workspace_root: Some(context.workspace),
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
        if let Some(tx) = context.progress {
            let mut event = usage_update(&usage, context.parent_role.id());
            event.subagent_id = agent_id.to_owned();
            let _ = tx.send(event).await;
        }
        context.model_usage.lock().await.push(usage);
        for text in &step.reasoning_summaries {
            messages.push(
                json!({"role":"reasoning","text":text.chars().take(8_000).collect::<String>()}),
            );
            if let Some(tx) = context.progress {
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
            && let Some(tx) = context.progress
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
            messages.push(json!({"role":"assistant","text":step.text.chars().take(32_000).collect::<String>()}));
        }
        reasoning.extend(
            step.reasoning_summaries
                .iter()
                .map(|text| text.chars().take(8_000).collect()),
        );
        input.extend(step.continuation);
        if step.calls.is_empty() {
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
            let label = summary(&call.name, arguments.as_ref().unwrap_or(&Value::Null));
            if let Some(tx) = context.progress {
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
            let mut review_unavailable = false;
            let (output, is_error) = match arguments {
                #[cfg(feature = "agent-feedback")]
                Ok(arguments)
                    if context.feedback_enabled && call.name == beale_agent_feedback::TOOL_NAME =>
                {
                    match beale_agent_feedback::execute(
                        context.state.workspaces.root(),
                        context.workspace_id,
                        context.session_id,
                        agent_id,
                        &arguments,
                    ) {
                        Ok(value) => (value, false),
                        Err(error) => (format!("Agent feedback error: {error}"), true),
                    }
                }
                Ok(arguments) if definitions.iter().any(|tool| tool["name"] == call.name) => {
                    let decision = beale_safety::review(
                        &context.state.db,
                        &context.state.provider,
                        &context.state.credentials,
                        ReviewRequest {
                            provider,
                            mode,
                            model: &assignment.model.model,
                            name: &call.name,
                            arguments: &arguments,
                            workspace_context: context.workspace_context,
                        },
                    )
                    .await;
                    review_unavailable = super::review_failed(&decision);
                    if !decision.approved {
                        (
                            if review_unavailable {
                                decision.reason
                            } else {
                                format!("Tool denied by review: {}", decision.reason)
                            },
                            true,
                        )
                    } else {
                        let _guard = if matches!(call.name.as_str(), "read" | "webfetch") {
                            None
                        } else {
                            Some(mutation_lock.lock().await)
                        };
                        let result = match call.name.as_str() {
                            #[cfg(feature = "webfetch")]
                            beale_webfetch::TOOL_NAME => {
                                super::execute_webfetch(
                                    &context.state.db,
                                    context.workspace_id,
                                    &arguments,
                                )
                                .await
                            }
                            _ => {
                                execute(
                                    &call.name,
                                    &arguments,
                                    context.workspace,
                                    context.state.workspaces.root(),
                                )
                                .await
                            }
                        };
                        match result {
                            Ok(value) => (value.output, value.is_error),
                            Err(error) => (format!("Tool error: {error:#}"), true),
                        }
                    }
                }
                Ok(_) => ("Tool error: unavailable in a subagent".to_owned(), true),
                Err(_) => ("Tool error: invalid JSON arguments".to_owned(), true),
            };
            let model_output = {
                if let Some(tx) = context.progress {
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
                });
                model_tool_preview(&call.name, &id, &output, is_error, context.agent_files)
            };
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

pub(super) async fn delegate(
    graph_id: &str,
    context: ChildContext<'_>,
    arguments: &Value,
    config: &SessionGraphConfig,
    fallback: &GraphNodeModel,
) -> DelegateOutcome {
    let Some(runtime) = beale_graphs::runtime(graph_id) else {
        return DelegateOutcome {
            output: "Graph runtime is unavailable.".to_owned(),
            is_error: true,
            tools: Vec::new(),
            child_turns: Vec::new(),
        };
    };
    let assignments = match runtime.assignments(arguments, config, fallback) {
        Ok(assignments) => assignments,
        Err(error) => {
            return DelegateOutcome {
                output: format!("Invalid delegation: {error}"),
                is_error: true,
                tools: Vec::new(),
                child_turns: Vec::new(),
            };
        }
    };
    if let Some(progress) = context.progress {
        let _ = progress
            .send(SessionStreamEvent {
                runbook_progress: None,
                kind: SessionStreamKind::SubagentsSpawned as i32,
                text: String::new(),
                tool: None,
                detail: None,
                segment_index: 0,
                agent_role: context.parent_role.id().to_owned(),
                tool_detail: None,
                usage_delta: None,
                subagent_id: String::new(),
                subagent: None,
            })
            .await;
    }
    let mutation_lock = Mutex::new(());
    let tool_sequence = AtomicI64::new(0);
    let results = schedule(assignments, config, |assignment| {
        run_child(assignment, &context, &mutation_lock, &tool_sequence)
    })
    .await;
    let mut output = String::new();
    let mut tools = Vec::new();
    let mut child_turns = Vec::new();
    let mut failed = false;
    for (index, (role, task, outcome)) in results.into_iter().enumerate() {
        let (outcome, transcript) = outcome.map_or_else(
            || {
                (
                    ChildOutcome {
                        result: "Subagent capacity is unavailable.".to_owned(),
                        is_error: true,
                        tools: Vec::new(),
                    },
                    None,
                )
            },
            |(outcome, transcript)| (outcome, Some(transcript)),
        );
        if let Some(transcript) = transcript {
            child_turns.push(transcript);
        }
        failed |= outcome.is_error;
        output.push_str(&format!(
            "## {} {}: {}\n{}\n\n",
            role.label(),
            index + 1,
            task,
            outcome.result
        ));
        tools.extend(outcome.tools);
    }
    tools.sort_by_key(|tool| tool.ordinal);
    child_turns.sort_by_key(|child| child.completed_at_ms);
    DelegateOutcome {
        output,
        is_error: failed,
        tools,
        child_turns,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_catalog_keeps_webfetch_optional() {
        let base = child_tool_definitions(false, false);
        assert_eq!(base.len(), 4);
        assert!(base.iter().all(|tool| tool["name"] != "webfetch"));
        let tools = child_tool_definitions(false, true);
        #[cfg(feature = "webfetch")]
        {
            assert_eq!(tools.len(), 5);
            assert_eq!(tools.last().unwrap(), &beale_webfetch::definition());
        }
        #[cfg(not(feature = "webfetch"))]
        assert_eq!(tools, base);
    }

    #[test]
    fn child_feature_catalog_matches_supported_tools() {
        let tools = child_tool_definitions(true, true);
        for (id, tool) in [
            ("webfetch", "webfetch"),
            ("beale-agent-feedback", "agent_feedback"),
        ] {
            assert_eq!(
                child_supports_feature(id),
                tools.iter().any(|item| item["name"] == tool)
            );
        }
        for id in ["goal-mode", "graphs", "surfaces", "example-plugin"] {
            assert!(!child_supports_feature(id));
        }
    }

    #[cfg(feature = "agent-feedback")]
    #[test]
    fn child_catalog_includes_feedback_only_when_enabled() {
        assert!(
            child_tool_definitions(false, false)
                .iter()
                .all(|tool| tool["name"] != beale_agent_feedback::TOOL_NAME)
        );
        assert!(
            child_tool_definitions(true, false)
                .iter()
                .any(|tool| tool["name"] == beale_agent_feedback::TOOL_NAME)
        );
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
