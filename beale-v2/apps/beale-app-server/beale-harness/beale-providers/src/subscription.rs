//! Official CLI subscription boundaries. Beale never reads or replays Claude credentials.

use crate::{
    CredentialStore, ModelCatalog, ModelEntry, Provider, ProviderStep, ToolCall, ToolStepRequest,
    mode_name, provider_name,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};
use tokio::{io::AsyncWriteExt, process::Command as AsyncCommand};

const STATUS_TIMEOUT: Duration = Duration::from_secs(10);
static CLAUDE_STATUS: LazyLock<Mutex<Option<(Instant, bool)>>> = LazyLock::new(|| Mutex::new(None));
mod zcode;

fn command(program: &str) -> Command {
    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("cmd.exe");
        command.args(["/d", "/s", "/c", program]);
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        command
    };
    #[cfg(not(windows))]
    let mut command = Command::new(program);
    command.stdin(Stdio::null()).stderr(Stdio::null());
    command
}

fn zcode_command() -> Result<Command> {
    #[cfg(windows)]
    {
        let base = std::env::var_os("LOCALAPPDATA").context("ZCode is not installed")?;
        let script = PathBuf::from(base).join("Programs/ZCode/resources/glm/zcode.cjs");
        anyhow::ensure!(script.is_file(), "ZCode CLI is not installed");
        let mut command = Command::new("node");
        command.arg(script);
        command.stdin(Stdio::null()).stderr(Stdio::null());
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
        Ok(command)
    }
    #[cfg(target_os = "macos")]
    {
        let script = PathBuf::from("/Applications/ZCode.app/Contents/Resources/glm/zcode.cjs");
        anyhow::ensure!(script.is_file(), "ZCode CLI is not installed");
        let mut command = Command::new("node");
        command
            .arg(script)
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        Ok(command)
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        Ok(command("zcode"))
    }
}

fn bounded_output(mut command: Command) -> Result<Vec<u8>> {
    command.stdout(Stdio::piped());
    let mut child = command
        .spawn()
        .context("official provider CLI is unavailable")?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                bail!("official provider CLI rejected the request");
            }
            let mut bytes = Vec::new();
            child
                .stdout
                .take()
                .context("CLI output is unavailable")?
                .take(64 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            anyhow::ensure!(bytes.len() <= 64 * 1024, "CLI output is too large");
            return Ok(bytes);
        }
        if started.elapsed() >= STATUS_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            bail!("official provider CLI timed out");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub fn configured(provider: Provider) -> bool {
    match provider {
        Provider::Anthropic => {
            if let Ok(cache) = CLAUDE_STATUS.lock()
                && let Some((checked, logged_in)) = *cache
                && checked.elapsed() < Duration::from_secs(5)
            {
                return logged_in;
            }
            let mut command = command("claude");
            command.args(["auth", "status", "--json"]);
            let logged_in = bounded_output(command)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .is_some_and(|value| value["loggedIn"] == true);
            if let Ok(mut cache) = CLAUDE_STATUS.lock() {
                *cache = Some((Instant::now(), logged_in));
            }
            logged_in
        }
        Provider::Zai => zcode_credentials_path()
            .and_then(|path| std::fs::read(path).ok())
            .filter(|bytes| bytes.len() <= 64 * 1024)
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .is_some_and(|value| {
                value["oauth:zai:access_token"]
                    .as_str()
                    .is_some_and(|token| !token.trim().is_empty())
            }),
        _ => false,
    }
}

pub fn logout(provider: Provider) -> Result<()> {
    let mut command = match provider {
        Provider::Anthropic => command("claude"),
        Provider::Zai => zcode_command()?,
        _ => bail!("this provider has no official CLI subscription"),
    };
    match provider {
        Provider::Anthropic => {
            command.args(["auth", "logout"]);
        }
        Provider::Zai => {
            command.arg("logout");
        }
        _ => unreachable!(),
    }
    bounded_output(command).map(|_| {
        if provider == Provider::Anthropic
            && let Ok(mut cache) = CLAUDE_STATUS.lock()
        {
            *cache = None;
        }
    })
}

fn zcode_credentials_path() -> Option<PathBuf> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
    Some(
        PathBuf::from(home)
            .join(".zcode")
            .join("v2")
            .join("credentials.json"),
    )
}

pub fn catalog(provider: Provider, store: &CredentialStore) -> Result<ModelCatalog> {
    anyhow::ensure!(
        configured(provider),
        "official provider subscription sign-in is required"
    );
    let models = match provider {
        Provider::Anthropic => vec![
            ModelEntry {
                id: "claude-opus-5".into(),
                name: "Claude Opus 5".into(),
            },
            ModelEntry {
                id: "claude-sonnet-5".into(),
                name: "Claude Sonnet 5".into(),
            },
        ],
        Provider::Zai => vec![
            ModelEntry {
                id: "glm-5.3".into(),
                name: "GLM-5.3".into(),
            },
            ModelEntry {
                id: "glm-5-turbo".into(),
                name: "GLM-5-Turbo".into(),
            },
        ],
        _ => bail!("this provider has no official CLI subscription"),
    };
    let saved = store.load_model_defaults(
        provider_name(provider),
        mode_name(crate::AuthMode::Subscription),
    )?;
    let preferred = &models[0].id;
    let fallback_small = models
        .last()
        .context("empty subscription model catalog")?
        .id
        .as_str();
    Ok(ModelCatalog {
        lead_model: selected(saved.as_ref().map(|v| v.lead.as_str()), preferred, &models),
        small_model: selected(
            saved.as_ref().map(|v| v.small.as_str()),
            fallback_small,
            &models,
        ),
        models,
    })
}

fn selected(saved: Option<&str>, fallback: &str, models: &[ModelEntry]) -> String {
    saved
        .filter(|id| models.iter().any(|entry| entry.id == *id))
        .unwrap_or(fallback)
        .to_owned()
}

const RESPONSE_SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"properties":{"text":{"type":"string"},"calls":{"type":"array","maxItems":16,"items":{"type":"object","additionalProperties":false,"properties":{"name":{"type":"string"},"arguments":{"type":"object"}},"required":["name","arguments"]}}},"required":["text","calls"]}"#;

fn step_prompt(request: &ToolStepRequest<'_>) -> Result<String> {
    anyhow::ensure!(
        request.input.len() <= 192 && request.tools.len() <= 64,
        "subscription input is too large"
    );
    Ok(format!(
        "Conversation and available Beale tools (JSON):\n{}\n\nReturn a JSON object with `text` and `calls`. Each call needs a tool name and object arguments matching its listed schema. Return `calls: []` for a final answer. Beale executes calls and sends results in the next step. Do not execute tools yourself.",
        serde_json::to_string(&json!({"conversation":request.input,"tools":request.tools}))?
    ))
}

fn parse_step(value: &Value, tools: &[Value]) -> Result<ProviderStep> {
    let text = value["text"]
        .as_str()
        .context("subscription output has no text")?;
    anyhow::ensure!(text.len() <= 2_097_152, "subscription output is too large");
    let raw_calls = value["calls"]
        .as_array()
        .context("subscription output has no calls array")?;
    anyhow::ensure!(raw_calls.len() <= 16, "too many subscription tool calls");
    let mut calls = Vec::new();
    let mut replay_calls = Vec::new();
    for item in raw_calls {
        let name = item["name"]
            .as_str()
            .context("subscription tool call has no name")?;
        anyhow::ensure!(
            tools.iter().any(|tool| tool["name"] == name),
            "subscription requested an unavailable tool"
        );
        let arguments = item["arguments"]
            .as_object()
            .context("subscription tool arguments must be an object")?;
        let id = format!("subscription-tool-{}", uuid::Uuid::new_v4());
        let arguments = Value::Object(arguments.clone());
        replay_calls.push(json!({"id":id,"name":name,"arguments":arguments}));
        calls.push(ToolCall {
            id,
            name: name.to_owned(),
            arguments: serde_json::to_string(&arguments)?,
        });
    }
    let continuation = if calls.is_empty() {
        Vec::new()
    } else {
        vec![
            json!({"role":"assistant","content":json!({"text":text,"calls":replay_calls}).to_string()}),
        ]
    };
    Ok(ProviderStep {
        response_id: None,
        text: text.to_owned(),
        calls,
        reasoning_summaries: Vec::new(),
        continuation,
    })
}

pub async fn claude_step(request: ToolStepRequest<'_>) -> Result<ProviderStep> {
    anyhow::ensure!(
        crate::models::valid_id(request.model),
        "invalid Claude model"
    );
    let prompt = step_prompt(&request)?;
    let mut command = AsyncCommand::new(if cfg!(windows) {
        "claude.cmd"
    } else {
        "claude"
    });
    command.args([
        "--print", "--output-format", "json", "--json-schema", RESPONSE_SCHEMA,
        "--model", request.model, "--tools", "", "--strict-mcp-config",
        "--mcp-config", "{\"mcpServers\":{}}", "--setting-sources", "",
        "--disable-slash-commands", "--no-chrome", "--no-session-persistence",
        "--permission-mode", "dontAsk", "--system-prompt",
        "You are the model behind Beale's trusted Rust harness. Follow the system messages inside the supplied conversation. You have no direct tools; ask Beale to call only tools from the supplied list. Return the specified JSON object.",
    ]);
    if let Some(effort) = request.reasoning_effort {
        let level = match effort {
            "none" | "minimal" | "low" => "low",
            "medium" => "medium",
            "high" => "high",
            "xhigh" | "max" => "max",
            _ => bail!("invalid Claude effort"),
        };
        command.args(["--effort", level]);
    }
    command
        .env_remove("ANTHROPIC_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        command.creation_flags(0x0800_0000);
    }
    let mut child = command
        .spawn()
        .context("official Claude CLI is unavailable")?;
    child
        .stdin
        .take()
        .context("Claude stdin is unavailable")?
        .write_all(prompt.as_bytes())
        .await?;
    let result = tokio::time::timeout(Duration::from_secs(180), child.wait_with_output())
        .await
        .context("Claude subscription turn timed out")??;
    anyhow::ensure!(result.status.success(), "Claude subscription turn failed");
    anyhow::ensure!(
        result.stdout.len() <= 2_097_152,
        "Claude subscription output is too large"
    );
    let value: Value =
        serde_json::from_slice(&result.stdout).context("invalid Claude subscription output")?;
    anyhow::ensure!(value["is_error"] != true, "Claude subscription turn failed");
    let structured = value
        .get("structured_output")
        .filter(|value| value.is_object())
        .cloned()
        .or_else(|| {
            value["result"]
                .as_str()
                .and_then(|text| serde_json::from_str(text).ok())
        })
        .context("Claude did not return structured output")?;
    parse_step(&structured, request.tools)
}

pub async fn zcode_step(request: ToolStepRequest<'_>) -> Result<ProviderStep> {
    zcode::step(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscription_catalog_keeps_models_distinct() {
        let models = vec![ModelEntry {
            id: "example-a".into(),
            name: "A".into(),
        }];
        assert_eq!(
            selected(Some("example-b"), "example-a", &models),
            "example-a"
        );
        assert_eq!(
            selected(Some("example-a"), "example-a", &models),
            "example-a"
        );
    }

    #[test]
    fn structured_subscription_calls_are_allowlisted() {
        let tools = vec![json!({"name":"read"})];
        let step = parse_step(&json!({"text":"Checking example file","calls":[{"name":"read","arguments":{"path":"example.txt"}}]}), &tools).unwrap();
        assert_eq!(step.calls[0].name, "read");
        assert_eq!(step.continuation.len(), 1);
        assert!(
            parse_step(
                &json!({"text":"","calls":[{"name":"bash","arguments":{}}]}),
                &tools
            )
            .is_err()
        );
    }
}
