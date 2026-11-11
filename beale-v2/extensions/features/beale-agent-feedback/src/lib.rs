//! Optional agent reports about Beale harness and runtime behavior.

use std::{
    fs::OpenOptions,
    io::Write,
    path::Path,
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use beale_storage::HostDatabase;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

pub const FEATURE_ID: &str = "beale-agent-feedback";
pub const FEATURE_NAME: &str = "Agent Feedback";
pub const FEATURE_DESCRIPTION: &str =
    "Record observed Beale harness or agent runtime issues for development review.";
pub const HAS_PROMPT_EXTENSION: bool = true;
pub const TOOL_COUNT: u32 = 1;
pub const TOOL_NAME: &str = "agent_feedback";
pub const PROMPT_EXTENSION: &str = "## Agent Feedback\n\nWhen you encounter a concrete Beale harness or agent runtime flaw, quirk, or limitation, call agent_feedback when observed and continue the task. Describe the behavior and its effect, plus any workaround. Do not invent issues or include target findings, credentials, or research details.";
pub const FILE_NAME: &str = "agent-feedback.jsonl";

static WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Flaw,
    Quirk,
    Limitation,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FeedbackInput {
    kind: Kind,
    observation: String,
    #[serde(default)]
    workaround: String,
}

#[derive(Serialize)]
struct FeedbackRecord<'a> {
    id: String,
    recorded_at_ms: u64,
    workspace_id: &'a str,
    session_id: &'a str,
    agent_id: &'a str,
    kind: &'a str,
    observation: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    workaround: &'a str,
}

pub fn definition() -> Value {
    json!({"type":"function","name":TOOL_NAME,
        "description":"Record a concrete Beale harness or agent runtime flaw, quirk, or limitation encountered in this session. Do not include target findings.",
        "parameters":{"type":"object","properties":{
            "kind":{"type":"string","enum":["flaw","quirk","limitation"]},
            "observation":{"type":"string","description":"Observed Beale behavior and its effect on the work.","minLength":1,"maxLength":4000},
            "workaround":{"type":"string","description":"What you did instead, if anything.","maxLength":4000}
        },"required":["kind","observation"],"additionalProperties":false}})
}

fn validate(input: &FeedbackInput) -> Result<()> {
    ensure!(
        !input.observation.trim().is_empty()
            && input.observation.len() <= 4_000
            && !input.observation.contains('\0')
            && input.workaround.len() <= 4_000
            && !input.workaround.contains('\0'),
        "feedback observation must be 1–4,000 bytes and workaround at most 4,000 bytes"
    );
    Ok(())
}

pub fn execute(
    root: &Path,
    workspace_id: &str,
    session_id: &str,
    agent_id: &str,
    args: &Value,
) -> Result<String> {
    ensure!(
        !workspace_id.is_empty()
            && workspace_id.len() <= 128
            && !session_id.is_empty()
            && session_id.len() <= 128
            && !agent_id.is_empty()
            && agent_id.len() <= 128,
        "invalid feedback session"
    );
    let input: FeedbackInput = serde_json::from_value(args.clone()).context("invalid feedback")?;
    validate(&input)?;
    let kind = match input.kind {
        Kind::Flaw => "flaw",
        Kind::Quirk => "quirk",
        Kind::Limitation => "limitation",
    };
    let recorded_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time is before Unix epoch")?
        .as_millis()
        .min(u64::MAX as u128) as u64;
    let record = FeedbackRecord {
        id: Uuid::new_v4().to_string(),
        recorded_at_ms,
        workspace_id,
        session_id,
        agent_id,
        kind,
        observation: input.observation.trim(),
        workaround: input.workaround.trim(),
    };
    let mut line = serde_json::to_vec(&record)?;
    line.push(b'\n');
    let _guard = WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("feedback write lock poisoned"))?;
    let path = root.join(FILE_NAME);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .context("open feedback file")?;
    HostDatabase::protect_database_file(root, &path)?;
    file.write_all(&line).context("write feedback")?;
    file.sync_data().context("flush feedback")?;
    Ok("Feedback recorded for development review.".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_feedback_is_readable_and_invalid_feedback_is_not_written() {
        let root = std::env::temp_dir().join(format!("beale-feedback-example-{}", Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        let args = json!({"kind":"quirk","observation":"Example agent tool result was delayed.","workaround":"Retried the example call."});
        assert!(execute(&root, "workspace-example", "session-example", "main", &args).is_ok());
        assert!(
            execute(
                &root,
                "workspace-example",
                "session-example",
                "subagent-example",
                &args
            )
            .is_ok()
        );
        assert!(
            execute(
                &root,
                "workspace-example",
                "session-example",
                "main",
                &json!({"kind":"flaw","observation":"  "})
            )
            .is_err()
        );
        let lines = std::fs::read_to_string(root.join(FILE_NAME)).unwrap();
        let rows = lines
            .lines()
            .map(serde_json::from_str::<Value>)
            .collect::<serde_json::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["kind"], "quirk");
        assert_eq!(
            rows[0]["observation"],
            "Example agent tool result was delayed."
        );
        assert_eq!(rows[0]["workspace_id"], "workspace-example");
        assert_eq!(rows[0]["session_id"], "session-example");
        assert_eq!(rows[0]["agent_id"], "main");
        assert_eq!(rows[1]["agent_id"], "subagent-example");
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
}
