//! Shared-loop ports for in-process turns: ordinary sessions and the Swarm
//! leader. Provider calls stream progress to the client, tools run through
//! [`tool_runtime::ToolRuntime`], and activity rows are held in memory until
//! the engine commits the turn in one transaction.
use super::*;
use beale_agent_loop::{
    AgentPorts, Checkpoint, InferOutcome, LoopState, ModelStep, PendingCall, ToolOutcome,
};
use beale_contracts::swarm_worker::{InferenceCall, InferenceUsage};
use beale_tools::Redactor;
use std::path::Path;

pub(super) struct EnginePorts<'a> {
    pub state: &'a SessionState,
    pub workspace_id: &'a str,
    pub session_id: &'a str,
    pub local: Option<&'a beale_storage::swarm_state::LocalTurn>,
    pub provider: Provider,
    pub mode: AuthMode,
    pub active_model: &'a SwarmNodeModel,
    pub definitions: &'a [Value],
    pub system: &'a str,
    pub reasoning_effort: Option<&'static str>,
    pub workspace: &'a Path,
    pub review_context: &'a str,
    pub callable_plugins: &'a HashSet<String>,
    pub redactor: &'a Redactor,
    pub agent_files: &'a AgentFiles,
    pub progress: Option<mpsc::Sender<SessionStreamEvent>>,
    pub stop: Option<watch::Receiver<bool>>,
    pub goal_active: bool,
    pub next_turn_number: i64,
    pub started: Instant,
    pub openai_connection: OpenAiConnection,
    pub goal_signal: Option<beale_goals::Signal>,
    pub tool_rows: Vec<ToolCallRow>,
    pub reasoning_rows: Vec<ReasoningSummaryRow>,
    pub commentary_rows: Vec<CommentaryRow>,
    pub model_usage: Vec<ModelUsageRow>,
    pub segment_index: u32,
    pub call_started: Instant,
}

fn unix_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}

fn elapsed_ms(since: Instant) -> i64 {
    since.elapsed().as_millis().min(i64::MAX as u128) as i64
}

impl EnginePorts<'_> {
    fn event(&self, kind: SessionStreamKind) -> SessionStreamEvent {
        SessionStreamEvent {
            turn_number: 0,
            runbook_progress: None,
            kind: kind as i32,
            text: String::new(),
            tool: None,
            detail: None,
            segment_index: 0,
            agent_role: self.active_model.node_id.clone(),
            tool_detail: None,
            usage_delta: None,
            subagent_id: String::new(),
            subagent: None,
        }
    }

    fn summary(&self, name: &str, args: &Value) -> String {
        if name == "goal_status" {
            return goal_status_summary(args);
        }
        if cfg!(feature = "agent-feedback") && name == "agent_feedback" {
            return "Record agent feedback".to_owned();
        }
        #[cfg(feature = "surfaces")]
        if name.starts_with("surface_") {
            return beale_surfaces::summary(&self.state.workspaces, self.workspace_id, name, args);
        }
        #[cfg(feature = "findings")]
        if name.starts_with("findings_") {
            return beale_findings::summary(&self.state.db, self.workspace_id, name, args);
        }
        #[cfg(feature = "memories")]
        if name.starts_with("memory_") {
            return beale_memories::summary(
                self.state.workspaces.root(),
                self.workspace_id,
                name,
                args,
            );
        }
        #[cfg(feature = "runbooks")]
        if name.starts_with("runbook_") {
            return beale_runbooks::summary(
                &self.state.db,
                self.workspace_id,
                self.session_id,
                name,
                args,
            );
        }
        tool_summary(name, args)
    }
}

impl AgentPorts for EnginePorts<'_> {
    async fn infer(
        &mut self,
        input: &[Value],
        _operation_id: &str,
        retry: bool,
    ) -> anyhow::Result<InferOutcome> {
        if retry {
            beale_host_log::error!(
                "session provider step requires bounded retry; retaining tool exchange"
            );
        } else {
            // Reserve one stream segment for the answer and one for each
            // reasoning part. A retried attempt reuses its round's segment.
            self.segment_index = self.segment_index.saturating_add(17);
        }
        let segment_index = self.segment_index;
        let (provider_tx, forward) = if let Some(client_tx) = self.progress.clone() {
            let (provider_tx, mut provider_rx) = mpsc::channel::<ProviderProgress>(32);
            let stream_role = self.active_model.node_id.clone();
            let forward = tokio::spawn(async move {
                let mut reasoning_segments = std::collections::BTreeMap::new();
                while let Some(update) = provider_rx.recv().await {
                    let (kind, text, item_segment) = match update {
                        ProviderProgress::MessageDelta(text) => {
                            (SessionStreamKind::MessageDelta, text, segment_index)
                        }
                        ProviderProgress::ReasoningDelta(text, key) => {
                            let next = reasoning_segments.len() as u32 + 1;
                            let part = *reasoning_segments.entry(key).or_insert(next);
                            (
                                SessionStreamKind::ReasoningDelta,
                                text,
                                segment_index.saturating_add(part),
                            )
                        }
                    };
                    if client_tx
                        .send(SessionStreamEvent {
                            turn_number: 0,
                            runbook_progress: None,
                            kind: kind as i32,
                            text,
                            tool: None,
                            detail: None,
                            segment_index: item_segment,
                            agent_role: stream_role.clone(),
                            tool_detail: None,
                            usage_delta: None,
                            subagent_id: String::new(),
                            subagent: None,
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });
            (Some(provider_tx), Some(forward))
        } else {
            (None, None)
        };
        self.call_started = Instant::now();
        let state = self.state;
        let provider = self.provider;
        let mode = self.mode;
        let local = self.local;
        let step_request = ToolStepRequest {
            model: &self.active_model.model,
            input,
            tools: self.definitions,
            instructions: (provider == Provider::OpenAi).then_some(self.system),
            reasoning_effort: self.reasoning_effort,
            workspace_root: Some(self.workspace),
        };
        let connection = &mut self.openai_connection;
        let call = async {
            if provider == Provider::OpenAi {
                state
                    .provider
                    .openai_tool_step(
                        mode,
                        step_request,
                        &state.credentials,
                        provider_tx.as_ref(),
                        connection,
                    )
                    .await
            } else {
                state
                    .provider
                    .tool_step(
                        provider,
                        mode,
                        step_request,
                        &state.credentials,
                        provider_tx.as_ref(),
                    )
                    .await
            }
        };
        let attributed = async {
            if let Some(local) = local {
                beale_providers::pacing::scope(
                    state.db.clone(),
                    &local.scan_id,
                    beale_contracts::pacing::CallClass::Research,
                    call,
                )
                .await
            } else {
                call.await
            }
        };
        let step = until_stopped(attributed, &mut self.stop).await;
        drop(provider_tx);
        if let Some(forward) = forward {
            let _ = forward.await;
        }
        Ok(match step {
            None => InferOutcome::Stopped,
            Some(Err(error)) => {
                beale_host_log::error!("session provider turn failed: {error:#}");
                InferOutcome::Failed {
                    retryable: provider_error_retryable(&error),
                    message: provider_error_message(provider, mode, &error),
                }
            }
            Some(Ok(step)) => InferOutcome::Completed(ModelStep {
                text: step.text,
                calls: step
                    .calls
                    .into_iter()
                    .map(|call| InferenceCall {
                        id: call.id,
                        name: call.name,
                        arguments: call.arguments,
                    })
                    .collect(),
                continuation: step.continuation,
                reasoning_summaries: step.reasoning_summaries,
                usage: step.usage.map(|usage| InferenceUsage {
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    cache_read_tokens: usage.cache_read_tokens,
                }),
            }),
        })
    }

    async fn observe_step(&mut self, _state: &LoopState, step: &ModelStep) -> anyhow::Result<()> {
        let node = self.active_model.node_id.clone();
        self.model_usage.push(ModelUsageRow {
            agent_id: node.clone(),
            role: node.clone(),
            provider: self.active_model.provider,
            method: self.active_model.method,
            model: self.active_model.model.clone(),
            input_tokens: step.usage.as_ref().map(|usage| usage.input_tokens as i64),
            output_tokens: step.usage.as_ref().map(|usage| usage.output_tokens as i64),
            cache_read_tokens: step
                .usage
                .as_ref()
                .and_then(|usage| usage.cache_read_tokens.map(|count| count as i64)),
            work_elapsed_ms: Some(elapsed_ms(self.started)),
            duration_ms: elapsed_ms(self.call_started),
            observed_at_ms: unix_now_ms(),
        });
        if let Some(tx) = &self.progress
            && let Some(item) = self.model_usage.last()
        {
            let _ = tx.send(usage_update(item, &node)).await;
        }
        for summary in &step.reasoning_summaries {
            if summary.trim().is_empty() {
                continue;
            }
            self.reasoning_rows.push(ReasoningSummaryRow {
                turn_number: self.next_turn_number,
                ordinal: self.reasoning_rows.len() as i64,
                after_tool_count: self.tool_rows.len() as i64,
                text: summary.clone(),
                agent_role: node.clone(),
            });
        }
        // Text alongside tool calls is commentary; without calls it is the answer.
        if !step.calls.is_empty() && !step.text.trim().is_empty() {
            self.commentary_rows.push(CommentaryRow {
                turn_number: self.next_turn_number,
                ordinal: self.commentary_rows.len() as i64,
                after_tool_count: self.tool_rows.len() as i64,
                text: step.text.clone(),
                agent_role: node,
            });
        }
        Ok(())
    }

    async fn run_tool(
        &mut self,
        _state: &LoopState,
        pending: &PendingCall,
        _arguments: Option<&Value>,
    ) -> anyhow::Result<ToolOutcome> {
        let call = &pending.call;
        let args: Result<Value, serde_json::Error> = serde_json::from_str(&call.arguments);
        let id = Uuid::new_v4().to_string();
        let summary = self.summary(&call.name, args.as_ref().unwrap_or(&Value::Null));
        if let Some(tx) = &self.progress {
            let mut event = self.event(SessionStreamKind::ToolStarted);
            event.tool = Some(SessionToolSummary {
                id: id.clone(),
                name: call.name.clone(),
                summary: summary.clone(),
                is_error: false,
                agent_role: self.active_model.node_id.clone(),
            });
            event.tool_detail = Some(SessionToolDetailPage {
                id: id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
                output: String::new(),
                offset: 0,
                next_offset: 0,
                total_bytes: 0,
                is_error: false,
            });
            let _ = tx.send(event).await;
        }
        let runtime = tool_runtime::ToolRuntime {
            local: self.local,
            state: self.state,
            workspace_id: self.workspace_id,
            session_id: self.session_id,
            agent_id: self
                .local
                .map_or(self.active_model.node_id.as_str(), |local| {
                    local.agent_id.as_str()
                }),
            agent_role: &self.active_model.node_id,
            workspace: self.workspace,
            review: tool_runtime::ReviewTarget {
                provider: self.provider,
                mode: self.mode,
                model: &self.active_model.model,
                context: self.review_context,
            },
            definitions: self.definitions,
            callable_plugins: self.callable_plugins,
            redactor: self.redactor,
            mutation_lock: None,
            progress: self.progress.as_ref(),
        };
        let gate = runtime
            .gate(&call.name, args.as_ref().ok(), &mut self.stop)
            .await;
        let mut review_unavailable = false;
        let (output, is_error) = match gate {
            tool_runtime::Gate::Run { reviewed } => match args {
                Ok(args) if call.name == beale_goals::TOOL_NAME && self.goal_active => {
                    if self.goal_signal.is_some() {
                        (
                            "Goal status was already recorded for this turn.".to_owned(),
                            true,
                        )
                    } else {
                        match beale_goals::parse_signal(&args) {
                            Ok(signal) => {
                                self.goal_signal = Some(signal);
                                ("Goal status recorded.".to_owned(), false)
                            }
                            Err(error) => (format!("Invalid goal status: {error}"), true),
                        }
                    }
                }
                args => {
                    let result = runtime
                        .execute(
                            &id,
                            &call.name,
                            args,
                            reviewed,
                            &self.tool_rows,
                            &mut self.stop,
                        )
                        .await;
                    review_unavailable = result.review_unavailable;
                    (result.output, result.is_error)
                }
            },
            blocked => {
                let result = blocked.blocked().unwrap_or_else(|| {
                    tool_runtime::ToolResult::error("Tool was not run.".to_owned())
                });
                review_unavailable = result.review_unavailable;
                (result.output, result.is_error)
            }
        };
        if let Some(tx) = &self.progress {
            let mut event = self.event(SessionStreamKind::ToolFinished);
            event.tool = Some(SessionToolSummary {
                id: id.clone(),
                name: call.name.clone(),
                summary: summary.clone(),
                is_error,
                agent_role: self.active_model.node_id.clone(),
            });
            let first_page = page(&output, 0, TOOL_PAGE_BYTES);
            event.tool_detail = Some(SessionToolDetailPage {
                id: id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
                output: first_page
                    .as_ref()
                    .map_or_else(|_| String::new(), |(first, _)| first.clone()),
                offset: 0,
                next_offset: first_page.as_ref().map_or(0, |(_, next)| *next as u64),
                total_bytes: output.len() as u64,
                is_error,
            });
            let _ = tx.send(event).await;
        }
        self.tool_rows.push(ToolCallRow {
            id: id.clone(),
            turn_number: self.next_turn_number,
            ordinal: self.tool_rows.len() as i64,
            name: call.name.clone(),
            summary,
            arguments: call.arguments.clone(),
            output: output.clone(),
            is_error,
            agent_role: self.active_model.node_id.clone(),
            created_at_ms: tool_runtime::unix_ms(),
        });
        Ok(ToolOutcome {
            model_output: model_tool_preview(&call.name, &id, &output, is_error, self.agent_files),
            review_unavailable,
            operations: Vec::new(),
        })
    }

    async fn checkpoint(
        &mut self,
        _state: &LoopState,
        _point: Checkpoint<'_>,
    ) -> anyhow::Result<()> {
        // In-process turns commit once, after the loop, in one transaction.
        Ok(())
    }

    fn stop_requested(&mut self) -> bool {
        stop_requested(&self.stop)
    }

    fn yield_requested(&mut self) -> anyhow::Result<bool> {
        match self.local {
            Some(local) => beale_storage::swarm_state::SwarmStore::open(self.state.db.clone())?
                .local_yield_requested(local),
            None => Ok(false),
        }
    }
}
