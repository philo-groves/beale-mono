use super::{parse_step, step_prompt};
use crate::{ProviderStep, ToolStepRequest};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

struct Client {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    completed: Option<String>,
}

impl Client {
    fn spawn(workspace: &Path) -> Result<Self> {
        let mut command = if cfg!(windows) {
            let base = std::env::var_os("LOCALAPPDATA").context("ZCode is not installed")?;
            let script = Path::new(&base).join("Programs/ZCode/resources/glm/zcode.cjs");
            ensure!(script.is_file(), "ZCode CLI is not installed");
            let mut command = Command::new("node");
            command.arg(script);
            command
        } else if cfg!(target_os = "macos") {
            let script = Path::new("/Applications/ZCode.app/Contents/Resources/glm/zcode.cjs");
            ensure!(script.is_file(), "ZCode CLI is not installed");
            let mut command = Command::new("node");
            command.arg(script);
            command
        } else {
            Command::new("zcode")
        };
        command
            .args(["app-server", "--stdio"])
            .current_dir(workspace)
            .env("NO_COLOR", "1")
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
            .context("official ZCode CLI is unavailable")?;
        let stdin = child.stdin.take().context("ZCode stdin is unavailable")?;
        let stdout = BufReader::new(child.stdout.take().context("ZCode stdout is unavailable")?);
        Ok(Self {
            child,
            stdin,
            stdout,
            next_id: 1,
            completed: None,
        })
    }

    async fn write(&mut self, value: &Value) -> Result<()> {
        let mut line = serde_json::to_vec(value)?;
        ensure!(line.len() <= 1_048_576, "ZCode request is too large");
        line.push(b'\n');
        self.stdin
            .write_all(&line)
            .await
            .context("ZCode protocol write failed")
    }

    async fn read(&mut self) -> Result<Value> {
        let mut bytes = Vec::new();
        loop {
            let available = self
                .stdout
                .fill_buf()
                .await
                .context("ZCode protocol read failed")?;
            ensure!(!available.is_empty(), "ZCode protocol ended unexpectedly");
            let count = available
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(available.len(), |index| index + 1);
            ensure!(
                bytes.len().saturating_add(count) <= 1_048_576,
                "ZCode protocol message is too large"
            );
            bytes.extend_from_slice(&available[..count]);
            self.stdout.consume(count);
            if bytes.last() == Some(&b'\n') {
                break;
            }
        }
        serde_json::from_slice(&bytes).context("invalid ZCode protocol message")
    }

    async fn receive(&mut self) -> Result<Option<Value>> {
        let value = self.read().await?;
        if let (Some(id), Some(method)) = (value.get("id"), value["method"].as_str()) {
            let result = match method {
                "session/requestRuntimePreferences" => {
                    json!({"nativeSearchEnhancementsEnabled":false})
                }
                "interaction/requestPermission" => {
                    json!({"decision":"deny","reason":"Beale executes only its governed tools."})
                }
                _ => {
                    self.write(&json!({"id":id,"error":{"code":-32601,"message":"Unsupported client request"}})).await?;
                    return Ok(None);
                }
            };
            self.write(&json!({"id":id,"result":result})).await?;
            return Ok(None);
        }
        if value["method"] == "session/event" {
            let event = value.pointer("/params/event").unwrap_or(&value["params"]);
            match event["type"].as_str() {
                Some("turn.completed") => {
                    self.completed = Some(event["response"].as_str().unwrap_or("").to_owned())
                }
                Some("turn.failed") => bail!("ZCode subscription turn failed"),
                _ => {}
            }
            return Ok(None);
        }
        Ok(Some(value))
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&json!({"id":id,"method":method,"params":params}))
            .await?;
        loop {
            if let Some(value) = self.receive().await? {
                if value["id"].as_u64() != Some(id) {
                    continue;
                }
                ensure!(
                    value.get("error").is_none(),
                    "ZCode rejected a protocol request"
                );
                return Ok(value["result"].clone());
            }
        }
    }
}

fn final_text(snapshot: &Value) -> Option<String> {
    snapshot["messages"]
        .as_array()?
        .iter()
        .rev()
        .find_map(|message| {
            if message.pointer("/info/role").and_then(Value::as_str) != Some("assistant") {
                return None;
            }
            let text = message["parts"]
                .as_array()?
                .iter()
                .filter_map(|part| {
                    (part["type"] == "text")
                        .then(|| part["text"].as_str())
                        .flatten()
                })
                .collect::<String>();
            (!text.trim().is_empty()).then_some(text)
        })
}

fn model_reference(session: &Value, requested: &str) -> Value {
    if let Some(models) = session
        .pointer("/settings/model/available")
        .and_then(Value::as_array)
    {
        for model in models {
            if model
                .pointer("/ref/modelId")
                .and_then(Value::as_str)
                .is_some_and(|id| id.eq_ignore_ascii_case(requested))
            {
                return model["ref"].clone();
            }
        }
    }
    json!({"providerId":"builtin:zai-coding-plan","modelId":requested})
}

fn session_id(value: &Value) -> Option<&str> {
    value["sessionId"]
        .as_str()
        .or_else(|| value.pointer("/session/sessionId").and_then(Value::as_str))
}

fn structured_text(text: &str) -> Result<Value> {
    let trimmed = text.trim();
    let body = trimmed
        .strip_prefix("```json\n")
        .or_else(|| trimmed.strip_prefix("```\n"))
        .and_then(|value| value.strip_suffix("```"))
        .map(str::trim)
        .unwrap_or(trimmed);
    serde_json::from_str(body).context("ZCode did not return a structured tool decision")
}

pub(super) async fn step(request: ToolStepRequest<'_>) -> Result<ProviderStep> {
    ensure!(
        crate::models::valid_id(request.model),
        "invalid ZCode model"
    );
    let workspace = request
        .workspace_root
        .map(Path::to_path_buf)
        .unwrap_or(std::env::current_dir()?);
    let prompt = step_prompt(&request)?;
    tokio::time::timeout(Duration::from_secs(180), async {
        let mut client = Client::spawn(&workspace)?;
        let key = format!(
            "beale-{}",
            &format!(
                "{:x}",
                Sha256::digest(workspace.to_string_lossy().as_bytes())
            )[..24]
        );
        let session = client
            .request(
                "session/create",
                json!({
                    "workspace":{"workspacePath":workspace,"workspaceKey":key},
                    "mode":"yolo","mcpServers":[],"toolAllowlist":[]
                }),
            )
            .await?;
        let id = session_id(&session)
            .context("ZCode did not return a session ID")?
            .to_owned();
        client
            .request(
                "session/setModel",
                json!({"sessionId":id,"model":model_reference(&session,request.model)}),
            )
            .await?;
        if let Some(effort) = request.reasoning_effort {
            let thought = match effort {
                "none" | "minimal" => "nothink",
                "xhigh" | "max" => "max",
                _ => "high",
            };
            client
                .request(
                    "session/setThoughtLevel",
                    json!({"sessionId":id,"thoughtLevel":thought}),
                )
                .await?;
        }
        client
            .request(
                "session/subscribe",
                json!({"sessionId":id,"deliveryKind":"desktop-continuous"}),
            )
            .await?;
        client
            .request("session/send", json!({"sessionId":id,"content":prompt}))
            .await?;
        while client.completed.is_none() {
            client.receive().await?;
        }
        let text = client.completed.take().unwrap_or_default();
        let text = if text.trim().is_empty() {
            let snapshot = client
                .request("session/read", json!({"sessionId":id}))
                .await?;
            final_text(&snapshot).context("ZCode completed without an assistant response")?
        } else {
            text
        };
        let value = structured_text(&text)?;
        let result = parse_step(&value, request.tools)?;
        let _ = client.child.kill().await;
        Ok::<_, anyhow::Error>(result)
    })
    .await
    .context("ZCode subscription turn timed out")?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_selected_model_in_official_catalog() {
        let session = json!({"settings":{"model":{"available":[{"ref":{"providerId":"builtin:zai-coding-plan","modelId":"glm-example"}}]}}});
        assert_eq!(
            model_reference(&session, "glm-example")["modelId"],
            "glm-example"
        );
    }

    #[test]
    fn accepts_single_json_code_fence() {
        assert_eq!(
            structured_text("```json\n{\"text\":\"Example\",\"calls\":[]}\n```").unwrap()["text"],
            "Example"
        );
    }
}
