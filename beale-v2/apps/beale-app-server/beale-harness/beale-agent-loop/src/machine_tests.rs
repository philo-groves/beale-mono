use super::*;
use std::collections::VecDeque;

/// Scripted provider and tools. Each provider attempt pops the next outcome;
/// tools answer from `tool_output` and can mark review as unavailable.
#[derive(Default)]
struct Script {
    outcomes: VecDeque<InferOutcome>,
    attempts: Vec<(usize, bool)>,
    executed: Vec<String>,
    observed: usize,
    checkpoints: Vec<LoopState>,
    /// Provider outcomes consumed when each checkpoint was taken.
    consumed: Vec<usize>,
    stop_on_call: Option<usize>,
    stop_after_infer: bool,
    yield_after: Option<usize>,
    review_unavailable_on: Option<&'static str>,
    fail_tool: Option<&'static str>,
    stop_checks: usize,
}

impl AgentPorts for Script {
    fn infer(
        &mut self,
        input: &[Value],
        _operation_id: &str,
        retry: bool,
    ) -> impl Future<Output = anyhow::Result<InferOutcome>> + Send {
        self.attempts.push((input.len(), retry));
        let outcome = self
            .outcomes
            .pop_front()
            .ok_or_else(|| anyhow::anyhow!("unexpected provider attempt"));
        async move { outcome }
    }
    fn observe_step(
        &mut self,
        _state: &LoopState,
        _step: &ModelStep,
    ) -> impl Future<Output = anyhow::Result<()>> + Send {
        self.observed += 1;
        async { Ok(()) }
    }
    fn run_tool(
        &mut self,
        _state: &LoopState,
        call: &PendingCall,
        arguments: Option<&Value>,
    ) -> impl Future<Output = anyhow::Result<ToolOutcome>> + Send {
        self.executed.push(call.call.id.clone());
        let result = if self.fail_tool == Some(call.call.name.as_str()) {
            Err(anyhow::anyhow!("synthetic uncertain outcome"))
        } else {
            Ok(ToolOutcome {
                model_output: format!(
                    "{} output {}",
                    call.call.name,
                    arguments.map_or("invalid".into(), Value::to_string)
                ),
                review_unavailable: self.review_unavailable_on == Some(call.call.name.as_str()),
                operations: vec![format!("operation-{}", call.call.id)],
            })
        };
        async move { result }
    }
    fn checkpoint(
        &mut self,
        state: &LoopState,
        _point: Checkpoint<'_>,
    ) -> impl Future<Output = anyhow::Result<()>> + Send {
        self.checkpoints.push(state.clone());
        self.consumed.push(self.attempts.len());
        async { Ok(()) }
    }
    fn stop_requested(&mut self) -> bool {
        self.stop_checks += 1;
        let stop_for_call = self
            .stop_on_call
            .is_some_and(|call| self.executed.len() == call && !self.outcomes.is_empty());
        stop_for_call || (self.stop_after_infer && !self.attempts.is_empty())
    }
    fn yield_requested(&mut self) -> anyhow::Result<bool> {
        Ok(self.yield_after == Some(self.executed.len()))
    }
}

fn call(id: &str, name: &str, arguments: &str) -> InferenceCall {
    InferenceCall {
        id: id.into(),
        name: name.into(),
        arguments: arguments.into(),
    }
}

fn completed(text: &str, calls: Vec<InferenceCall>, continuation: Vec<Value>) -> InferOutcome {
    InferOutcome::Completed(ModelStep {
        text: text.into(),
        calls,
        continuation,
        ..Default::default()
    })
}

fn user() -> Vec<Value> {
    vec![json!({"role":"user","content":"Inspect the synthetic fixture"})]
}

async fn finish(script: &mut Script, state: &mut LoopState, config: LoopConfig) -> Finished {
    run(state, &config, script).await.unwrap()
}

#[tokio::test]
async fn answer_without_tools_ends_in_one_step() {
    let mut script = Script::default();
    script.outcomes.push_back(completed(
        "Synthetic answer",
        vec![],
        vec![json!({"role":"assistant","content":"Synthetic answer"})],
    ));
    let mut state = LoopState::new(MessageFormat::Chat, user());
    let done = finish(&mut script, &mut state, LoopConfig::session(false, false)).await;
    assert_eq!(done.ending, Ending::Answer);
    assert_eq!(done.answer, "Synthetic answer");
    assert_eq!(state.input.len(), 2);
    assert_eq!(script.observed, 1);
    assert_eq!(script.checkpoints.len(), 1);
}

#[tokio::test]
async fn tool_rounds_append_results_in_the_provider_format() {
    let mut script = Script::default();
    script.outcomes.push_back(completed(
        "Checking",
        vec![
            call("call-a", "read", "{\"path\":\"a.txt\"}"),
            call("call-b", "read", "{\"path\":\"b.txt\"}"),
        ],
        vec![json!({"type":"function_call","call_id":"call-a"})],
    ));
    script.outcomes.push_back(completed("Done", vec![], vec![]));
    let mut state = LoopState::new(MessageFormat::Responses, user());
    let done = finish(&mut script, &mut state, LoopConfig::session(false, false)).await;
    assert_eq!(done.answer, "Done");
    assert_eq!(script.executed, ["call-a", "call-b"]);
    assert_eq!(state.executed_calls, 2);
    assert_eq!(
        state.input[2],
        json!({"type":"function_call_output","call_id":"call-a","output":"read output {\"path\":\"a.txt\"}"})
    );
    assert_eq!(state.input[3]["call_id"], "call-b");
    // Inference, two calls, final inference.
    assert_eq!(script.checkpoints.len(), 4);
    assert_eq!(script.attempts, [(1, false), (4, false)]);
}

#[tokio::test]
async fn anthropic_results_batch_per_round_only_when_configured() {
    for (batch, messages) in [(true, 1), (false, 2)] {
        let mut script = Script::default();
        script.outcomes.push_back(completed(
            "",
            vec![call("call-a", "read", "{}"), call("call-b", "read", "{}")],
            vec![json!({"role":"assistant","content":[{"type":"tool_use","id":"call-a"}]})],
        ));
        script.outcomes.push_back(completed("Done", vec![], vec![]));
        let mut state = LoopState::new(MessageFormat::Anthropic, user());
        finish(&mut script, &mut state, LoopConfig::session(false, batch)).await;
        let results: Vec<_> = state.input.iter().skip(2).collect();
        assert_eq!(results.len(), messages, "batch={batch}");
        let blocks: usize = results
            .iter()
            .map(|m| m["content"].as_array().unwrap().len())
            .sum();
        assert_eq!(blocks, 2);
        // The batch is part of the durable state between the two calls.
        let between = &script.checkpoints[1];
        let Phase::Tools { batch: pending, .. } = &between.phase else {
            panic!("expected tool phase")
        };
        assert_eq!(pending.len(), usize::from(batch));
    }
}

#[tokio::test]
async fn session_retries_once_before_tools_and_fails_without_activity() {
    let mut script = Script::default();
    for _ in 0..2 {
        script.outcomes.push_back(InferOutcome::Failed {
            retryable: true,
            message: "Synthetic outage".into(),
        });
    }
    let mut state = LoopState::new(MessageFormat::Chat, user());
    let error = run(&mut state, &LoopConfig::session(true, false), &mut script)
        .await
        .unwrap_err();
    assert!(matches!(error, LoopError::Provider { ref message } if message == "Synthetic outage"));
    assert_eq!(script.attempts, [(1, false), (1, true)]);
    // A non-retryable error is not retried before tools run.
    let mut script = Script::default();
    script.outcomes.push_back(InferOutcome::Failed {
        retryable: false,
        message: "Synthetic rejection".into(),
    });
    let mut state = LoopState::new(MessageFormat::Chat, user());
    assert!(
        run(&mut state, &LoopConfig::session(true, false), &mut script)
            .await
            .is_err()
    );
    assert_eq!(script.attempts.len(), 1);
}

#[tokio::test]
async fn swarm_recovery_retries_any_error_three_times_then_saves_interruption() {
    let mut script = Script::default();
    script
        .outcomes
        .push_back(completed("", vec![call("call-a", "read", "{}")], vec![]));
    for _ in 0..3 {
        script.outcomes.push_back(InferOutcome::Failed {
            retryable: false,
            message: "Synthetic rejection".into(),
        });
    }
    let mut state = LoopState::new(MessageFormat::Chat, user());
    let done = finish(&mut script, &mut state, LoopConfig::session(true, false)).await;
    assert_eq!(done.ending, Ending::Interrupted);
    assert_eq!(done.answer, answers::interrupted("Synthetic rejection"));
    assert_eq!(script.attempts.len(), 4);
    // An ordinary session after activity retries only a retryable error once.
    let mut script = Script::default();
    script
        .outcomes
        .push_back(completed("", vec![call("call-a", "read", "{}")], vec![]));
    script.outcomes.push_back(InferOutcome::Failed {
        retryable: false,
        message: "Synthetic rejection".into(),
    });
    let mut state = LoopState::new(MessageFormat::Chat, user());
    let done = finish(&mut script, &mut state, LoopConfig::session(false, false)).await;
    assert_eq!(done.ending, Ending::Interrupted);
    assert_eq!(script.attempts.len(), 2);
}

#[tokio::test]
async fn empty_steps_retry_only_while_recovering() {
    let mut script = Script::default();
    script
        .outcomes
        .push_back(completed("", vec![call("call-a", "read", "{}")], vec![]));
    script.outcomes.push_back(completed("", vec![], vec![]));
    script
        .outcomes
        .push_back(completed("Recovered", vec![], vec![]));
    let mut state = LoopState::new(MessageFormat::Chat, user());
    let done = finish(&mut script, &mut state, LoopConfig::session(true, false)).await;
    assert_eq!(done.answer, "Recovered");
    assert_eq!(script.attempts.len(), 3);
    assert!(script.attempts[2].1);
    // Before any tool runs an empty step is not retried.
    let mut script = Script::default();
    script.outcomes.push_back(completed("", vec![], vec![]));
    let mut state = LoopState::new(MessageFormat::Chat, user());
    assert!(
        run(&mut state, &LoopConfig::session(true, false), &mut script)
            .await
            .is_err()
    );
    assert_eq!(script.attempts.len(), 1);
}

#[tokio::test]
async fn every_preset_rejects_an_empty_answer() {
    for config in [
        LoopConfig::session(false, false),
        LoopConfig::session(true, false),
        LoopConfig::worker(),
    ] {
        // Before tools ran the empty step is a provider failure.
        let mut script = Script::default();
        script
            .outcomes
            .push_back(completed(" ", vec![], vec![json!({"role":"assistant"})]));
        let mut state = LoopState::new(MessageFormat::Chat, user());
        let error = run(&mut state, &config, &mut script).await.unwrap_err();
        assert!(
            matches!(error, LoopError::Provider { ref message } if message == answers::EMPTY_STEP_MESSAGE)
        );
        // After tools ran, sessions save an interrupted answer and workers
        // fail the attempt, as they do for provider failures.
        let mut script = Script::default();
        script
            .outcomes
            .push_back(completed("", vec![call("call-a", "read", "{}")], vec![]));
        // Swarm sessions retry an empty step while recovering; give each
        // attempt the same empty answer.
        for _ in 0..3 {
            script.outcomes.push_back(completed("", vec![], vec![]));
        }
        let mut state = LoopState::new(MessageFormat::Chat, user());
        let result = run(&mut state, &config, &mut script).await;
        assert_eq!(script.executed, ["call-a"]);
        if config.interrupt_after_activity {
            let done = result.unwrap();
            assert_eq!(done.ending, Ending::Interrupted);
            assert_eq!(
                done.answer,
                answers::interrupted(answers::EMPTY_ANSWER_DETAIL)
            );
        } else {
            assert!(matches!(
                result,
                Err(LoopError::ProviderExhausted { ref message }) if message == answers::EMPTY_ANSWER_DETAIL
            ));
        }
    }
}

#[tokio::test]
async fn workers_and_swarm_sessions_share_the_activity_safeguard() {
    for (config, limited) in [
        (LoopConfig::worker(), true),
        (LoopConfig::session(true, false), true),
        (LoopConfig::session(false, false), false),
    ] {
        assert_eq!(config.max_tool_activity.is_some(), limited, "{config:?}");
        if !limited {
            continue;
        }
        let mut script = Script::default();
        let mut state = LoopState::new(MessageFormat::Chat, user());
        state.executed_calls = SWARM_TOOL_ACTIVITY_LIMIT;
        let done = finish(&mut script, &mut state, config).await;
        assert_eq!(done.ending, Ending::Safeguard);
        assert_eq!(done.answer, answers::SAFEGUARD_ANSWER);
        assert!(script.attempts.is_empty());
    }
}

/// With the same provider conversation, a session turn and a worker send the
/// provider the same input, as long as the provider returns continuation
/// items. Their remaining differences are deliberate: workers add answer text
/// without continuation, and only sessions batch Anthropic results.
#[tokio::test]
async fn session_and_worker_presets_send_the_same_provider_input() {
    let conversation = || {
        let mut outcomes = VecDeque::new();
        outcomes.push_back(completed(
            "Checking",
            vec![
                call("call-a", "read", "{\"path\":\"a.txt\"}"),
                call("call-b", "list", "{}"),
            ],
            vec![json!({"role":"assistant","content":"Checking"})],
        ));
        outcomes.push_back(InferOutcome::Failed {
            retryable: true,
            message: "Synthetic transient outage".into(),
        });
        outcomes.push_back(completed(
            "Synthetic answer",
            vec![],
            vec![json!({"role":"assistant","content":"Synthetic answer"})],
        ));
        outcomes
    };
    let mut inputs = Vec::new();
    for config in [LoopConfig::session(true, false), LoopConfig::worker()] {
        let mut script = Script {
            outcomes: conversation(),
            ..Default::default()
        };
        let mut state = LoopState::new(MessageFormat::Chat, user());
        let done = finish(
            &mut script,
            &mut state,
            LoopConfig {
                retry: RetryPolicy {
                    delay: Duration::ZERO,
                    ..config.retry
                },
                ..config
            },
        )
        .await;
        assert_eq!(done.answer, "Synthetic answer");
        assert_eq!(script.executed, ["call-a", "call-b"]);
        inputs.push((script.attempts, state.input));
    }
    assert_eq!(inputs[0], inputs[1]);
}

#[tokio::test]
async fn worker_preset_retries_retryable_errors_and_never_saves_interruptions() {
    let mut script = Script::default();
    script
        .outcomes
        .push_back(completed("", vec![call("call-a", "read", "{}")], vec![]));
    for _ in 0..3 {
        script.outcomes.push_back(InferOutcome::Failed {
            retryable: true,
            message: String::new(),
        });
    }
    let mut state = LoopState::new(MessageFormat::Chat, user());
    let error = run(&mut state, &LoopConfig::worker(), &mut script)
        .await
        .unwrap_err();
    assert!(matches!(error, LoopError::ProviderExhausted { .. }));
    assert_eq!(script.attempts.len(), 4);
    assert_eq!(state.provider_failures, 3);
}

#[tokio::test]
async fn assistant_text_fallback_keeps_answers_without_continuation() {
    for (config, expected) in [
        (LoopConfig::worker(), 2),
        (LoopConfig::session(false, false), 1),
    ] {
        let mut script = Script::default();
        script
            .outcomes
            .push_back(completed("Synthetic answer", vec![], vec![]));
        let mut state = LoopState::new(MessageFormat::Chat, user());
        finish(&mut script, &mut state, config).await;
        assert_eq!(state.input.len(), expected);
    }
}

#[tokio::test]
async fn stop_before_a_call_discards_the_unfinished_round() {
    let mut script = Script {
        stop_on_call: Some(1),
        ..Default::default()
    };
    script.outcomes.push_back(completed(
        "Checking",
        vec![call("call-a", "read", "{}"), call("call-b", "read", "{}")],
        vec![json!({"role":"assistant","content":"Checking"})],
    ));
    script
        .outcomes
        .push_back(completed("Unused", vec![], vec![]));
    let mut state = LoopState::new(MessageFormat::Chat, user());
    let done = finish(&mut script, &mut state, LoopConfig::session(false, false)).await;
    assert_eq!(done.ending, Ending::Stopped);
    assert_eq!(done.answer, answers::STOPPED_ANSWER);
    assert_eq!(script.executed, ["call-a"]);
    assert_eq!(state.input, user());
}

#[tokio::test]
async fn stop_after_a_provider_step_does_not_record_it() {
    let mut script = Script {
        stop_after_infer: true,
        ..Default::default()
    };
    script.outcomes.push_back(completed(
        "Checking",
        vec![call("call-a", "read", "{}")],
        vec![],
    ));
    let mut state = LoopState::new(MessageFormat::Chat, user());
    let done = finish(&mut script, &mut state, LoopConfig::session(false, false)).await;
    assert_eq!(done.ending, Ending::Stopped);
    assert_eq!(script.observed, 0);
    assert!(script.executed.is_empty());
    assert_eq!(state.input, user());
}

#[tokio::test]
async fn activity_safeguard_pauses_at_its_limit() {
    let mut script = Script::default();
    let mut state = LoopState::new(MessageFormat::Chat, user());
    state.executed_calls = 500;
    state.replayed_calls = 12;
    let done = finish(&mut script, &mut state, LoopConfig::session(true, false)).await;
    assert_eq!(done.ending, Ending::Safeguard);
    assert_eq!(done.answer, answers::SAFEGUARD_ANSWER);
    assert!(script.attempts.is_empty());
    // Ordinary sessions have no safeguard.
    script
        .outcomes
        .push_back(completed("Answer", vec![], vec![]));
    let mut state = LoopState::new(MessageFormat::Chat, user());
    state.executed_calls = 600;
    let done = finish(&mut script, &mut state, LoopConfig::session(false, false)).await;
    assert_eq!(done.ending, Ending::Answer);
}

#[tokio::test]
async fn yield_and_review_unavailability_end_the_round_after_recording_the_result() {
    let mut script = Script {
        yield_after: Some(1),
        ..Default::default()
    };
    script.outcomes.push_back(completed(
        "",
        vec![
            call("call-a", "swarm_spawn_subagent", "{}"),
            call("call-b", "read", "{}"),
        ],
        vec![],
    ));
    let mut state = LoopState::new(MessageFormat::Chat, user());
    let done = finish(&mut script, &mut state, LoopConfig::session(true, false)).await;
    assert_eq!(done.ending, Ending::Yielded);
    assert_eq!(done.answer, answers::DELEGATION_ANSWER);
    assert_eq!(script.executed, ["call-a"]);
    assert_eq!(state.input.last().unwrap()["tool_call_id"], "call-a");

    let mut script = Script {
        review_unavailable_on: Some("write"),
        ..Default::default()
    };
    script.outcomes.push_back(completed(
        "",
        vec![call("call-a", "write", "{}"), call("call-b", "read", "{}")],
        vec![],
    ));
    let mut state = LoopState::new(MessageFormat::Chat, user());
    let done = finish(&mut script, &mut state, LoopConfig::session(true, false)).await;
    assert_eq!(done.ending, Ending::ReviewUnavailable);
    assert_eq!(done.answer, answers::REVIEW_UNAVAILABLE_ANSWER);
    assert_eq!(script.executed, ["call-a"]);
}

#[tokio::test]
async fn repeated_call_ids_replay_or_interrupt_when_dedupe_applies() {
    let mut script = Script::default();
    script.outcomes.push_back(completed(
        "",
        vec![call("call-a", "read", "{\"path\":\"a.txt\"}")],
        vec![],
    ));
    script.outcomes.push_back(completed(
        "",
        vec![call("call-a", "read", "{ \"path\": \"a.txt\" }")],
        vec![],
    ));
    script.outcomes.push_back(completed(
        "",
        vec![call("call-a", "read", "{\"path\":\"b.txt\"}")],
        vec![],
    ));
    let mut state = LoopState::new(MessageFormat::Chat, user());
    let done = finish(&mut script, &mut state, LoopConfig::session(true, false)).await;
    assert_eq!(script.executed, ["call-a"]);
    assert_eq!(state.replayed_calls, 1);
    assert_eq!(state.input[1], state.input[2]);
    assert_eq!(done.ending, Ending::Interrupted);
    assert_eq!(
        done.answer,
        answers::interrupted(answers::REUSED_CALL_DETAIL)
    );
    // Without dedupe the host runs every call.
    let mut script = Script::default();
    for _ in 0..2 {
        script
            .outcomes
            .push_back(completed("", vec![call("call-a", "read", "{}")], vec![]));
    }
    script.outcomes.push_back(completed("Done", vec![], vec![]));
    let mut state = LoopState::new(MessageFormat::Chat, user());
    finish(&mut script, &mut state, LoopConfig::session(false, false)).await;
    assert_eq!(script.executed, ["call-a", "call-a"]);
}

#[tokio::test]
async fn invalid_calls_fail_before_activity_and_interrupt_after() {
    let mut script = Script::default();
    script
        .outcomes
        .push_back(completed("", vec![call("", "read", "{}")], vec![]));
    let mut state = LoopState::new(MessageFormat::Chat, user());
    assert!(matches!(
        run(&mut state, &LoopConfig::session(true, false), &mut script).await,
        Err(LoopError::InvalidCall)
    ));
    let mut script = Script::default();
    script.outcomes.push_back(completed(
        "",
        vec![
            call("call-a", "read", "{}"),
            call("call-b", &"n".repeat(129), "{}"),
        ],
        vec![],
    ));
    let mut state = LoopState::new(MessageFormat::Chat, user());
    let done = finish(&mut script, &mut state, LoopConfig::session(true, false)).await;
    assert_eq!(done.ending, Ending::Interrupted);
    assert_eq!(
        done.answer,
        answers::interrupted(answers::INVALID_CALL_DETAIL)
    );
}

#[tokio::test]
async fn uncertain_tool_outcomes_stop_without_recording_a_result() {
    let mut script = Script {
        fail_tool: Some("bash"),
        ..Default::default()
    };
    script
        .outcomes
        .push_back(completed("", vec![call("call-a", "bash", "{}")], vec![]));
    let mut state = LoopState::new(MessageFormat::Chat, user());
    assert!(matches!(
        run(&mut state, &LoopConfig::worker(), &mut script).await,
        Err(LoopError::Port(_))
    ));
    assert_eq!(state.executed_calls, 0);
    assert!(matches!(state.phase, Phase::Tools { ref calls, .. } if calls.len() == 1));
}

/// A durable host can resume from any checkpoint and reach the same end.
#[tokio::test]
async fn resuming_from_any_checkpoint_reaches_the_same_result() {
    let outcomes = || {
        VecDeque::from([
            completed(
                "Checking",
                vec![call("call-a", "read", "{}"), call("call-b", "write", "{}")],
                vec![json!({"role":"assistant","content":"Checking"})],
            ),
            InferOutcome::Failed {
                retryable: true,
                message: String::new(),
            },
            completed("", vec![call("call-c", "read", "{}")], vec![]),
            completed("Final", vec![], vec![]),
        ])
    };
    let mut script = Script {
        outcomes: outcomes(),
        ..Default::default()
    };
    let mut state = LoopState::new(MessageFormat::Anthropic, user());
    let expected = finish(&mut script, &mut state, LoopConfig::worker()).await;
    assert_eq!(script.checkpoints.len(), script.consumed.len());
    for (index, checkpoint) in script.checkpoints.iter().enumerate() {
        let mut resumed_state: LoopState =
            serde_json::from_value(serde_json::to_value(checkpoint).unwrap()).unwrap();
        // Provider attempts already applied at this checkpoint are not repeated.
        let mut remaining = outcomes();
        remaining.drain(..script.consumed[index]);
        let mut resumed = Script {
            outcomes: remaining,
            ..Default::default()
        };
        let done = finish(&mut resumed, &mut resumed_state, LoopConfig::worker()).await;
        assert_eq!(done.answer, expected.answer, "checkpoint {index}");
        assert_eq!(resumed_state.input, state.input, "checkpoint {index}");
        assert_eq!(resumed_state.executed_calls, state.executed_calls);
        assert!(resumed.outcomes.is_empty(), "checkpoint {index}");
    }
}

#[test]
fn version_2_worker_phases_still_deserialize() {
    let tools: Phase = serde_json::from_value(json!({"phase":"tools","calls":[{
        "call":{"id":"call-a","name":"read","arguments":"{}"},
        "review_id":"review-example","publish_id":"publish-example"}]}))
    .unwrap();
    assert!(
        matches!(tools, Phase::Tools { ref batch, prior_input_len: None, .. } if batch.is_empty())
    );
    let done: Phase = serde_json::from_value(
        json!({"phase":"done","operation_id":"operation-example","answer":"Synthetic"}),
    )
    .unwrap();
    assert_eq!(
        done,
        Phase::Done {
            operation_id: "operation-example".into(),
            answer: "Synthetic".into(),
            ending: Ending::Answer,
        }
    );
    // The leader's reconciliation reads these exact keys.
    let value = serde_json::to_value(&done).unwrap();
    assert_eq!(value["phase"], "done");
    assert_eq!(value["answer"], "Synthetic");
    assert!(
        serde_json::from_value::<Phase>(
            json!({"phase":"done","operation_id":"x","answer":"y","extra":1})
        )
        .is_err()
    );
}
