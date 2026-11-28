//! Pure decisions shared by every agent loop.
use beale_contracts::swarm_worker::{InferenceCall, MessageFormat};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Duration};

pub const MAX_CALL_ID_BYTES: usize = 256;
pub const MAX_CALL_NAME_BYTES: usize = 128;
pub const MAX_CALL_ARGUMENT_BYTES: usize = 65_536;

/// Whether a provider tool call fits the host's identity and argument bounds.
pub fn call_within_bounds(call: &InferenceCall) -> bool {
    !call.id.is_empty()
        && call.id.len() <= MAX_CALL_ID_BYTES
        && call.name.len() <= MAX_CALL_NAME_BYTES
        && call.arguments.len() <= MAX_CALL_ARGUMENT_BYTES
}

/// Parsed arguments and the canonical text used to compare repeated calls:
/// compact JSON when the arguments parse, otherwise the raw text.
pub fn canonical_arguments(raw: &str) -> (Option<Value>, String) {
    match serde_json::from_str::<Value>(raw) {
        Ok(value) => {
            let canonical = value.to_string();
            (Some(value), canonical)
        }
        Err(_) => (None, raw.to_owned()),
    }
}

/// The recorded result of a call identity within one execution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletedCall {
    pub name: String,
    pub arguments: String,
    pub output: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Replay<'a> {
    /// A new identity: run the tool.
    Execute,
    /// The same call was already completed: reuse its result.
    Reuse(&'a str),
    /// The identity was reused with a different tool or arguments.
    Conflict,
}

/// Provider call IDs identify results within one execution. A repeated call
/// replays its result instead of performing the tool operation again.
pub fn replay<'a>(
    completed: &'a BTreeMap<String, CompletedCall>,
    call: &InferenceCall,
    canonical: &str,
) -> Replay<'a> {
    match completed.get(&call.id) {
        None => Replay::Execute,
        Some(saved) if saved.name == call.name && saved.arguments == canonical => {
            Replay::Reuse(&saved.output)
        }
        Some(_) => Replay::Conflict,
    }
}

/// Bounded retry of a provider step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Total attempts for a step before tools have run.
    pub attempts: u32,
    /// Total attempts once tools have run, when recovery applies.
    pub recovering_attempts: u32,
    /// While recovering, retry errors the provider marked as not retryable.
    pub retry_any_error_while_recovering: bool,
    /// While recovering, retry a step with neither text nor tool calls.
    pub retry_empty_while_recovering: bool,
    /// Pause between attempts.
    pub delay: Duration,
}

impl RetryPolicy {
    fn allowed(&self, recovering: bool) -> u32 {
        if recovering {
            self.recovering_attempts
        } else {
            self.attempts
        }
    }

    /// Whether to retry after `failures` failed attempts of this step.
    pub fn retry_error(&self, failures: u32, retryable: bool, recovering: bool) -> bool {
        (retryable || (recovering && self.retry_any_error_while_recovering))
            && failures < self.allowed(recovering)
    }

    /// Whether to retry an empty step after `failures` unsuccessful attempts.
    pub fn retry_empty(&self, failures: u32, recovering: bool) -> bool {
        recovering && self.retry_empty_while_recovering && failures < self.allowed(recovering)
    }
}

/// Appends a tool result in the provider's message format. With batching,
/// Anthropic `tool_result` blocks are collected and sent as one user message
/// when the round ends.
pub fn append_tool_result(
    format: MessageFormat,
    batch_results: bool,
    input: &mut Vec<Value>,
    batch: &mut Vec<Value>,
    call_id: &str,
    output: &str,
) {
    let result = format.tool_output(call_id, output);
    if batch_results && format == MessageFormat::Anthropic {
        if let Some(blocks) = result.get("content").and_then(Value::as_array) {
            batch.extend(blocks.iter().cloned());
        }
    } else {
        input.push(result);
    }
}

/// Sends collected Anthropic `tool_result` blocks as one user message.
pub fn flush_tool_results(input: &mut Vec<Value>, batch: &mut Vec<Value>) {
    if !batch.is_empty() {
        input.push(json!({"role":"user","content":std::mem::take(batch)}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str, name: &str, arguments: &str) -> InferenceCall {
        InferenceCall {
            id: id.into(),
            name: name.into(),
            arguments: arguments.into(),
        }
    }

    #[test]
    fn bounds_match_the_host_limits() {
        assert!(call_within_bounds(&call("call-example", "read", "{}")));
        assert!(!call_within_bounds(&call("", "read", "{}")));
        assert!(!call_within_bounds(&call(&"i".repeat(257), "read", "{}")));
        assert!(call_within_bounds(&call(&"i".repeat(256), "read", "{}")));
        assert!(!call_within_bounds(&call("id", &"n".repeat(129), "{}")));
        assert!(!call_within_bounds(&call(
            "id",
            "read",
            &"a".repeat(65_537)
        )));
        assert!(call_within_bounds(&call("id", "read", &"a".repeat(65_536))));
    }

    #[test]
    fn canonical_arguments_ignore_formatting_but_keep_unparsed_text() {
        let (parsed, pretty) = canonical_arguments("{\n  \"path\": \"example.txt\"\n}");
        let (_, compact) = canonical_arguments("{\"path\":\"example.txt\"}");
        assert_eq!(pretty, compact);
        assert_eq!(parsed.unwrap()["path"], "example.txt");
        assert_eq!(canonical_arguments("{broken"), (None, "{broken".to_owned()));
    }

    #[test]
    fn replay_reuses_identical_calls_and_rejects_changed_content() {
        let mut completed = BTreeMap::new();
        completed.insert(
            "call-example".to_owned(),
            CompletedCall {
                name: "read".into(),
                arguments: "{\"path\":\"example.txt\"}".into(),
                output: "saved output".into(),
            },
        );
        let same = call("call-example", "read", "{\"path\":\"example.txt\"}");
        assert_eq!(
            replay(&completed, &same, "{\"path\":\"example.txt\"}"),
            Replay::Reuse("saved output")
        );
        assert_eq!(
            replay(&completed, &same, "{\"path\":\"other.txt\"}"),
            Replay::Conflict
        );
        let renamed = call("call-example", "write", "{\"path\":\"example.txt\"}");
        assert_eq!(
            replay(&completed, &renamed, "{\"path\":\"example.txt\"}"),
            Replay::Conflict
        );
        let fresh = call("call-new", "read", "{}");
        assert_eq!(replay(&completed, &fresh, "{}"), Replay::Execute);
    }

    #[test]
    fn retry_policy_reproduces_session_and_worker_limits() {
        // Sessions: one retry for a retryable error; three attempts for any
        // error or an empty step once tools ran in a Swarm turn.
        let session = RetryPolicy {
            attempts: 2,
            recovering_attempts: 3,
            retry_any_error_while_recovering: true,
            retry_empty_while_recovering: true,
            delay: Duration::from_millis(500),
        };
        assert!(session.retry_error(1, true, false));
        assert!(!session.retry_error(1, false, false));
        assert!(!session.retry_error(2, true, false));
        assert!(session.retry_error(1, false, true));
        assert!(session.retry_error(2, false, true));
        assert!(!session.retry_error(3, true, true));
        assert!(!session.retry_empty(1, false));
        assert!(session.retry_empty(2, true));
        assert!(!session.retry_empty(3, true));
        // Workers: three attempts for retryable errors only.
        let worker = RetryPolicy {
            attempts: 3,
            recovering_attempts: 3,
            retry_any_error_while_recovering: false,
            retry_empty_while_recovering: false,
            delay: Duration::ZERO,
        };
        assert!(worker.retry_error(2, true, false));
        assert!(!worker.retry_error(3, true, false));
        assert!(!worker.retry_error(1, false, true));
        assert!(!worker.retry_empty(1, true));
    }

    #[test]
    fn anthropic_results_batch_once_per_round_when_enabled() {
        let mut input = vec![];
        let mut batch = vec![];
        for (id, output) in [("call-a", "first"), ("call-b", "second")] {
            append_tool_result(
                MessageFormat::Anthropic,
                true,
                &mut input,
                &mut batch,
                id,
                output,
            );
        }
        assert!(input.is_empty());
        flush_tool_results(&mut input, &mut batch);
        assert_eq!(
            input,
            vec![json!({"role":"user","content":[
                {"type":"tool_result","tool_use_id":"call-a","content":"first"},
                {"type":"tool_result","tool_use_id":"call-b","content":"second"}
            ]})]
        );
        assert!(batch.is_empty());
        flush_tool_results(&mut input, &mut batch);
        assert_eq!(input.len(), 1);
        // Without batching each result is its own message; other formats
        // never batch.
        let mut unbatched = vec![];
        append_tool_result(
            MessageFormat::Anthropic,
            false,
            &mut unbatched,
            &mut batch,
            "call-a",
            "first",
        );
        append_tool_result(
            MessageFormat::Chat,
            true,
            &mut unbatched,
            &mut batch,
            "call-b",
            "second",
        );
        assert!(batch.is_empty());
        assert_eq!(
            unbatched,
            vec![
                json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"call-a","content":"first"}]}),
                json!({"role":"tool","tool_call_id":"call-b","content":"second"})
            ]
        );
    }
}
