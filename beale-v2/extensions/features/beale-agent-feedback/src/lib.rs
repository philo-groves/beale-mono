//! Optional agent reports about Beale harness and runtime behavior.

use std::{
    fs::OpenOptions,
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::Path,
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use beale_contracts::tool_error::{ToolError, arguments};
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

/// Catalog and session descriptor for this feature.
pub const FEATURE: beale_feature_api::Feature = beale_feature_api::Feature {
    id: FEATURE_ID,
    name: FEATURE_NAME,
    description: FEATURE_DESCRIPTION,
    prompt_extension: PROMPT_EXTENSION,
    use_case: "Use to record an observed Beale harness or agent runtime issue for development review.",
    tool_names: &[TOOL_NAME],
    definitions: Some(feature_tools),
    review: beale_feature_api::ToolReview::Exempt,
    subagents: true,
    migrations: &[],
    startup: &[],
};

fn feature_tools() -> Vec<Value> {
    vec![definition()]
}
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
        ToolError::invalid(
            "feedback observation must be 1–4,000 bytes and workaround at most 4,000 bytes"
        )
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
    execute_once(
        root,
        &Uuid::new_v4().to_string(),
        workspace_id,
        session_id,
        agent_id,
        args,
    )
}

/// Host-owned operation identity allows exact retry after a lost receipt.
pub fn execute_once(
    root: &Path,
    operation: &str,
    workspace_id: &str,
    session_id: &str,
    agent_id: &str,
    args: &Value,
) -> Result<String> {
    ensure!(
        !operation.is_empty()
            && operation.len() <= 128
            && operation
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_:".contains(&b)),
        "invalid feedback operation"
    );
    ensure!(
        !workspace_id.is_empty()
            && workspace_id.len() <= 128
            && !session_id.is_empty()
            && session_id.len() <= 128
            && !agent_id.is_empty()
            && agent_id.len() <= 128,
        "invalid feedback session"
    );
    let input: FeedbackInput = arguments(args.clone())?;
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
        id: operation.to_owned(),
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
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .context("open feedback file")?;
    HostDatabase::protect_database_file(root, &path)?;
    file.try_lock()
        .context("feedback file is owned by another writer")?;
    let mut expected = serde_json::to_value(&record)?;
    expected
        .as_object_mut()
        .context("invalid feedback record")?
        .remove("recorded_at_ms");
    let mut accepted = false;
    let mut length = 0_u64;
    let mut incomplete_tail = false;
    let mut missing_newline = false;
    {
        let mut reader = BufReader::new(&mut file);
        loop {
            let mut bytes = Vec::new();
            let count = reader.by_ref().take(65_537).read_until(b'\n', &mut bytes)?;
            if count == 0 {
                break;
            }
            ensure!(count <= 65_536, "feedback record exceeds storage bound");
            let complete = bytes.last() == Some(&b'\n');
            let mut saved: Value = match serde_json::from_slice(&bytes) {
                Ok(value) => value,
                Err(_) if !complete => {
                    incomplete_tail = true;
                    break;
                }
                Err(error) => return Err(error).context("corrupt complete feedback record"),
            };
            ensure!(
                saved["id"].as_str().is_some() && saved["recorded_at_ms"].as_u64().is_some(),
                "invalid stored feedback identity"
            );
            if saved["id"] == operation {
                saved
                    .as_object_mut()
                    .context("invalid stored feedback")?
                    .remove("recorded_at_ms");
                ensure!(
                    saved == expected && !accepted,
                    "feedback operation reused with changed or duplicate content"
                );
                accepted = true;
            }
            length += count as u64;
            if !complete {
                missing_newline = true;
                break;
            }
        }
    }
    if incomplete_tail {
        file.set_len(length)?;
    }
    file.seek(SeekFrom::Start(length))?;
    if missing_newline {
        file.write_all(b"\n")?;
    }
    if !accepted {
        file.write_all(&line).context("write feedback")?;
    }
    file.sync_all().context("flush feedback")?;
    #[cfg(unix)]
    std::fs::File::open(root)?.sync_all()?;
    Ok("Feedback recorded for development review.".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_operations_recover_torn_appends_and_reject_changed_retries() {
        let root =
            std::env::temp_dir().join(format!("beale-feedback-retry-example-{}", Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        let args = json!({"kind":"quirk","observation":"Synthetic display delay"});
        let write = |id: &str, value: &Value| {
            execute_once(
                &root,
                id,
                "workspace-example",
                "session-example",
                "agent-example",
                value,
            )
        };
        write("operation-example", &args).unwrap();
        let path = root.join(FILE_NAME);
        let original = std::fs::read(&path).unwrap();
        write("operation-example", &args).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert!(
            write(
                "operation-example",
                &json!({"kind":"quirk","observation":"Changed synthetic observation"})
            )
            .is_err()
        );
        // A fully written record whose last newline was interrupted is retained.
        std::fs::write(&path, &original[..original.len() - 1]).unwrap();
        write("operation-example", &args).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let mut torn = original.clone();
        torn.extend_from_slice(b"{\"id\":\"unfinished");
        std::fs::write(&path, torn).unwrap();
        write("next-example", &args).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let rows: Vec<Value> = std::str::from_utf8(&bytes)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["id"], "operation-example");
        assert_eq!(rows[1]["id"], "next-example");
        let mut corrupt = bytes.clone();
        corrupt.extend_from_slice(b"broken complete record\n");
        std::fs::write(&path, &corrupt).unwrap();
        assert!(write("third-example", &args).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), corrupt);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

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
