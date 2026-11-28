//! The resumable agent round loop. Each [`step`] makes one transition: a
//! provider attempt, a tool call, or ending the turn. Hosts supply provider
//! access, tool execution, persistence, and stop/yield signals through
//! [`AgentPorts`]; the loop owns the shared policy.
use crate::{answers, policy::*};
use beale_contracts::swarm_worker::{InferenceCall, InferenceUsage, MessageFormat};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, future::Future, time::Duration};
use uuid::Uuid;

/// A provider tool call awaiting execution, with stable identities for the
/// host's review and outcome records.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingCall {
    pub call: InferenceCall,
    pub review_id: String,
    pub publish_id: String,
}

impl PendingCall {
    fn new(call: InferenceCall) -> Self {
        Self {
            call,
            review_id: Uuid::new_v4().to_string(),
            publish_id: Uuid::new_v4().to_string(),
        }
    }
}

/// Why a turn ended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ending {
    /// The model gave its answer.
    #[default]
    Answer,
    Stopped,
    Safeguard,
    Yielded,
    ReviewUnavailable,
    /// The provider or its tool calls failed after tools ran.
    Interrupted,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub enum Phase {
    Infer {
        operation_id: String,
    },
    Tools {
        calls: Vec<PendingCall>,
        /// Batched Anthropic results not yet sent in this round.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        batch: Vec<Value>,
        /// Conversation length before this round's provider output, restored
        /// when a stop discards the unfinished round.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prior_input_len: Option<usize>,
    },
    Done {
        operation_id: String,
        answer: String,
        #[serde(default)]
        ending: Ending,
    },
}

/// The durable state of one turn. A host can serialize it after any
/// transition and continue later from exactly that point.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopState {
    pub format: MessageFormat,
    pub input: Vec<Value>,
    pub provider_failures: u32,
    pub executed_calls: u32,
    pub replayed_calls: u32,
    pub completed: BTreeMap<String, CompletedCall>,
    pub phase: Phase,
}

impl LoopState {
    pub fn new(format: MessageFormat, input: Vec<Value>) -> Self {
        Self {
            format,
            input,
            provider_failures: 0,
            executed_calls: 0,
            replayed_calls: 0,
            completed: BTreeMap::new(),
            phase: Phase::Infer {
                operation_id: Uuid::new_v4().to_string(),
            },
        }
    }

    fn finish(&mut self, ending: Ending, answer: String) {
        self.phase = Phase::Done {
            operation_id: Uuid::new_v4().to_string(),
            answer,
            ending,
        };
    }
}

/// Behavior that differs between hosts. The presets reproduce each host's
/// established behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoopConfig {
    pub retry: RetryPolicy,
    /// Apply the recovering retry limits once tools have run.
    pub recovery_after_activity: bool,
    /// Replay repeated provider call IDs instead of running them again.
    pub dedupe: bool,
    /// Pause the turn after this many executed and replayed calls.
    pub max_tool_activity: Option<u32>,
    /// Send Anthropic tool results as one message per round.
    pub batch_results: bool,
    /// Add the answer text to the conversation when the provider supplies
    /// no continuation items.
    pub assistant_text_fallback: bool,
    /// After tools ran, save an interrupted answer when the provider cannot
    /// finish, rather than failing the turn.
    pub interrupt_after_activity: bool,
    /// Treat a final step with neither text nor tool calls as a provider
    /// failure before tools ran, or as an interrupted answer after.
    pub reject_empty_answer: bool,
}

/// The per-turn tool activity safeguard shared by Swarm agents.
pub const SWARM_TOOL_ACTIVITY_LIMIT: u32 = 512;

impl LoopConfig {
    /// In-process session turns. Swarm turns add call replay, recovery
    /// retries, and the activity safeguard.
    pub fn session(swarm: bool, batch_results: bool) -> Self {
        Self {
            retry: RetryPolicy {
                attempts: 2,
                recovering_attempts: 3,
                retry_any_error_while_recovering: true,
                retry_empty_while_recovering: true,
                delay: Duration::from_millis(500),
            },
            recovery_after_activity: swarm,
            dedupe: swarm,
            max_tool_activity: swarm.then_some(SWARM_TOOL_ACTIVITY_LIMIT),
            batch_results,
            assistant_text_fallback: false,
            interrupt_after_activity: true,
            reject_empty_answer: true,
        }
    }

    /// Assigned workers, whose durable journal replays operations itself.
    /// Like Swarm session turns they reject empty answers and stop at the
    /// tool activity safeguard.
    pub fn worker() -> Self {
        Self {
            retry: RetryPolicy {
                attempts: 3,
                recovering_attempts: 3,
                retry_any_error_while_recovering: false,
                retry_empty_while_recovering: false,
                delay: Duration::ZERO,
            },
            recovery_after_activity: false,
            dedupe: false,
            max_tool_activity: Some(SWARM_TOOL_ACTIVITY_LIMIT),
            batch_results: false,
            assistant_text_fallback: true,
            interrupt_after_activity: false,
            reject_empty_answer: true,
        }
    }
}

/// One provider step.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelStep {
    pub text: String,
    pub calls: Vec<InferenceCall>,
    pub continuation: Vec<Value>,
    pub reasoning_summaries: Vec<String>,
    pub usage: Option<InferenceUsage>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InferOutcome {
    Completed(ModelStep),
    Failed { retryable: bool, message: String },
    Stopped,
}

/// The known result of one tool call.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolOutcome {
    /// The result text given back to the model.
    pub model_output: String,
    pub review_unavailable: bool,
    /// Host operation IDs to acknowledge once the result is durable.
    pub operations: Vec<String>,
}

/// The transition a host may persist.
#[derive(Debug)]
pub enum Checkpoint<'a> {
    /// A provider attempt was applied.
    Inference { operation_id: &'a str },
    /// A call was executed (`Some`) or replayed (`None`).
    Call {
        call: &'a PendingCall,
        outcome: Option<&'a ToolOutcome>,
    },
    /// The turn ended without executing a call or provider attempt.
    Transition,
}

/// Host capabilities used by the loop.
pub trait AgentPorts: Send {
    /// One provider attempt. `retry` is set when it repeats a failed or empty
    /// attempt of the same step.
    fn infer(
        &mut self,
        input: &[Value],
        operation_id: &str,
        retry: bool,
    ) -> impl Future<Output = anyhow::Result<InferOutcome>> + Send;

    /// Observes a completed step before it changes the conversation.
    fn observe_step(
        &mut self,
        state: &LoopState,
        step: &ModelStep,
    ) -> impl Future<Output = anyhow::Result<()>> + Send;

    /// Runs one call. An error means its outcome is uncertain; the loop stops
    /// without recording a result.
    fn run_tool(
        &mut self,
        state: &LoopState,
        call: &PendingCall,
        arguments: Option<&Value>,
    ) -> impl Future<Output = anyhow::Result<ToolOutcome>> + Send;

    /// Called after every transition is applied to `state`.
    fn checkpoint(
        &mut self,
        state: &LoopState,
        point: Checkpoint<'_>,
    ) -> impl Future<Output = anyhow::Result<()>> + Send;

    fn stop_requested(&mut self) -> bool;

    /// Whether the agent must end its turn so a child can use its slot.
    fn yield_requested(&mut self) -> anyhow::Result<bool>;
}

#[derive(Debug)]
pub enum LoopError {
    /// The provider failed before any tool ran.
    Provider { message: String },
    /// The provider failed after tools ran, and this host does not save an
    /// interrupted answer.
    ProviderExhausted { message: String },
    /// The provider returned an out-of-bounds tool call before any tool ran.
    InvalidCall,
    /// A host port failed. For a tool call the outcome is uncertain.
    Port(anyhow::Error),
}

impl std::fmt::Display for LoopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Provider { message } => write!(f, "provider step failed: {message}"),
            Self::ProviderExhausted { message } => {
                write!(f, "provider recovery exhausted: {message}")
            }
            Self::InvalidCall => f.write_str("provider returned an invalid tool call"),
            Self::Port(error) => write!(f, "{error:#}"),
        }
    }
}

impl std::error::Error for LoopError {}

impl From<anyhow::Error> for LoopError {
    fn from(error: anyhow::Error) -> Self {
        Self::Port(error)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finished {
    pub operation_id: String,
    pub answer: String,
    pub ending: Ending,
}

/// Runs steps until the turn is done.
pub async fn run<P: AgentPorts>(
    state: &mut LoopState,
    config: &LoopConfig,
    ports: &mut P,
) -> Result<Finished, LoopError> {
    loop {
        if let Phase::Done {
            operation_id,
            answer,
            ending,
        } = &state.phase
        {
            return Ok(Finished {
                operation_id: operation_id.clone(),
                answer: answer.clone(),
                ending: *ending,
            });
        }
        step(state, config, ports).await?;
    }
}

/// Makes one transition. Returns `true` once the turn is done.
pub async fn step<P: AgentPorts>(
    state: &mut LoopState,
    config: &LoopConfig,
    ports: &mut P,
) -> Result<bool, LoopError> {
    match &state.phase {
        Phase::Done { .. } => Ok(true),
        Phase::Infer { operation_id } => {
            let operation_id = operation_id.clone();
            infer_step(state, config, ports, &operation_id).await
        }
        Phase::Tools { .. } => tool_step(state, config, ports).await,
    }
}

async fn infer_step<P: AgentPorts>(
    state: &mut LoopState,
    config: &LoopConfig,
    ports: &mut P,
    operation_id: &str,
) -> Result<bool, LoopError> {
    if config
        .max_tool_activity
        .is_some_and(|max| state.executed_calls.saturating_add(state.replayed_calls) >= max)
    {
        state.finish(Ending::Safeguard, answers::SAFEGUARD_ANSWER.into());
        ports.checkpoint(state, Checkpoint::Transition).await?;
        return Ok(true);
    }
    if ports.stop_requested() {
        state.finish(Ending::Stopped, answers::STOPPED_ANSWER.into());
        ports.checkpoint(state, Checkpoint::Transition).await?;
        return Ok(true);
    }
    let recovering = config.recovery_after_activity && state.executed_calls > 0;
    let retry = state.provider_failures > 0;
    match ports.infer(&state.input, operation_id, retry).await? {
        InferOutcome::Stopped => {
            state.finish(Ending::Stopped, answers::STOPPED_ANSWER.into());
        }
        InferOutcome::Failed { retryable, message } => {
            state.provider_failures = state.provider_failures.saturating_add(1);
            if config
                .retry
                .retry_error(state.provider_failures, retryable, recovering)
            {
                return retry_step(state, config, ports, operation_id).await;
            }
            if state.executed_calls == 0 {
                return Err(LoopError::Provider { message });
            }
            if !config.interrupt_after_activity {
                return Err(LoopError::ProviderExhausted { message });
            }
            state.finish(Ending::Interrupted, answers::interrupted(&message));
        }
        InferOutcome::Completed(step) => {
            let empty = step.calls.is_empty() && step.text.trim().is_empty();
            if empty
                && config
                    .retry
                    .retry_empty(state.provider_failures.saturating_add(1), recovering)
            {
                state.provider_failures = state.provider_failures.saturating_add(1);
                return retry_step(state, config, ports, operation_id).await;
            }
            if ports.stop_requested() {
                state.finish(Ending::Stopped, answers::STOPPED_ANSWER.into());
            } else {
                state.provider_failures = 0;
                ports.observe_step(state, &step).await?;
                let prior_input_len = state.input.len();
                if config.assistant_text_fallback
                    && step.continuation.is_empty()
                    && !step.text.is_empty()
                {
                    state
                        .input
                        .push(json!({"role":"assistant","content":step.text}));
                } else {
                    state.input.extend(step.continuation);
                }
                if step.calls.is_empty() {
                    if config.reject_empty_answer && step.text.trim().is_empty() {
                        if state.executed_calls == 0 {
                            return Err(LoopError::Provider {
                                message: answers::EMPTY_STEP_MESSAGE.into(),
                            });
                        }
                        if !config.interrupt_after_activity {
                            return Err(LoopError::ProviderExhausted {
                                message: answers::EMPTY_ANSWER_DETAIL.into(),
                            });
                        }
                        state.finish(
                            Ending::Interrupted,
                            answers::interrupted(answers::EMPTY_ANSWER_DETAIL),
                        );
                    } else {
                        state.finish(Ending::Answer, step.text);
                    }
                } else {
                    state.phase = Phase::Tools {
                        calls: step.calls.into_iter().map(PendingCall::new).collect(),
                        batch: Vec::new(),
                        prior_input_len: Some(prior_input_len),
                    };
                }
            }
        }
    }
    ports
        .checkpoint(state, Checkpoint::Inference { operation_id })
        .await?;
    Ok(matches!(state.phase, Phase::Done { .. }))
}

async fn retry_step<P: AgentPorts>(
    state: &mut LoopState,
    config: &LoopConfig,
    ports: &mut P,
    operation_id: &str,
) -> Result<bool, LoopError> {
    state.phase = Phase::Infer {
        operation_id: Uuid::new_v4().to_string(),
    };
    ports
        .checkpoint(state, Checkpoint::Inference { operation_id })
        .await?;
    if !config.retry.delay.is_zero() {
        tokio::time::sleep(config.retry.delay).await;
    }
    Ok(false)
}

async fn tool_step<P: AgentPorts>(
    state: &mut LoopState,
    config: &LoopConfig,
    ports: &mut P,
) -> Result<bool, LoopError> {
    let Phase::Tools {
        calls,
        prior_input_len,
        ..
    } = &state.phase
    else {
        return Err(LoopError::Port(anyhow::anyhow!("not in a tool phase")));
    };
    let pending = calls
        .first()
        .cloned()
        .ok_or_else(|| LoopError::Port(anyhow::anyhow!("tool phase has no pending call")))?;
    let prior_input_len = *prior_input_len;
    if ports.stop_requested() {
        // A stop discards the unfinished round, including its provider output.
        if let Some(length) = prior_input_len {
            state.input.truncate(length);
        }
        state.finish(Ending::Stopped, answers::STOPPED_ANSWER.into());
        ports.checkpoint(state, Checkpoint::Transition).await?;
        return Ok(true);
    }
    if !call_within_bounds(&pending.call) {
        if state.executed_calls == 0 {
            return Err(LoopError::InvalidCall);
        }
        end_round(
            state,
            Ending::Interrupted,
            answers::interrupted(answers::INVALID_CALL_DETAIL),
        );
        ports.checkpoint(state, Checkpoint::Transition).await?;
        return Ok(true);
    }
    let (arguments, canonical) = canonical_arguments(&pending.call.arguments);
    if config.dedupe {
        match replay(&state.completed, &pending.call, &canonical) {
            Replay::Reuse(output) => {
                let output = output.to_owned();
                append_result(state, config, &pending.call.id, &output);
                state.replayed_calls = state.replayed_calls.saturating_add(1);
                advance(state);
                ports
                    .checkpoint(
                        state,
                        Checkpoint::Call {
                            call: &pending,
                            outcome: None,
                        },
                    )
                    .await?;
                return Ok(false);
            }
            Replay::Conflict => {
                end_round(
                    state,
                    Ending::Interrupted,
                    answers::interrupted(answers::REUSED_CALL_DETAIL),
                );
                ports.checkpoint(state, Checkpoint::Transition).await?;
                return Ok(true);
            }
            Replay::Execute => {}
        }
    }
    let outcome = ports.run_tool(state, &pending, arguments.as_ref()).await?;
    append_result(state, config, &pending.call.id, &outcome.model_output);
    state.executed_calls = state.executed_calls.saturating_add(1);
    if config.dedupe {
        state.completed.insert(
            pending.call.id.clone(),
            CompletedCall {
                name: pending.call.name.clone(),
                arguments: canonical,
                output: outcome.model_output.clone(),
            },
        );
    }
    if ports.yield_requested()? {
        end_round(state, Ending::Yielded, answers::DELEGATION_ANSWER.into());
    } else if outcome.review_unavailable {
        end_round(
            state,
            Ending::ReviewUnavailable,
            answers::REVIEW_UNAVAILABLE_ANSWER.into(),
        );
    } else {
        advance(state);
    }
    ports
        .checkpoint(
            state,
            Checkpoint::Call {
                call: &pending,
                outcome: Some(&outcome),
            },
        )
        .await?;
    Ok(matches!(state.phase, Phase::Done { .. }))
}

fn append_result(state: &mut LoopState, config: &LoopConfig, call_id: &str, output: &str) {
    let format = state.format;
    if let Phase::Tools { batch, .. } = &mut state.phase {
        append_tool_result(
            format,
            config.batch_results,
            &mut state.input,
            batch,
            call_id,
            output,
        );
    }
}

/// Removes the finished call; after the last one the round's batched results
/// are sent and the next provider step begins.
fn advance(state: &mut LoopState) {
    let finished_round = if let Phase::Tools { calls, batch, .. } = &mut state.phase {
        calls.remove(0);
        if calls.is_empty() {
            flush_tool_results(&mut state.input, batch);
            true
        } else {
            false
        }
    } else {
        false
    };
    if finished_round {
        state.phase = Phase::Infer {
            operation_id: Uuid::new_v4().to_string(),
        };
    }
}

/// Ends the turn mid-round. Results already given are kept, so the
/// conversation records every executed call's result.
fn end_round(state: &mut LoopState, ending: Ending, answer: String) {
    if let Phase::Tools { batch, .. } = &mut state.phase {
        flush_tool_results(&mut state.input, batch);
    }
    state.finish(ending, answer);
}

#[cfg(test)]
#[path = "machine_tests.rs"]
mod tests;
