//! Host-executed base tools and model-readable temporary context files.

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command, time::timeout};
mod agent_files;
mod webfetch;
pub use agent_files::AgentFiles;

pub const MAX_RESULT_BYTES: usize = 8_000_000;
const MAX_INPUT_BYTES: usize = 8_000_000;
const BASH_TIMEOUT: Duration = Duration::from_secs(60);

pub const WEBFETCH: &str = "webfetch";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginCallArgs {
    pub plugin: String,
    pub server: String,
    pub tool: String,
    pub arguments: Value,
}

pub fn plugin_call_definition() -> Value {
    json!({"type":"function","name":"plugin_call","description":"Call an available plugin tool. Read its PLUGIN.md for discovered server names, tool names, and argument schemas.","parameters":{"type":"object","properties":{"plugin":{"type":"string"},"server":{"type":"string"},"tool":{"type":"string"},"arguments":{"type":"object"}},"required":["plugin","server","tool","arguments"],"additionalProperties":false}})
}

pub struct ToolOutcome {
    pub output: String,
    pub is_error: bool,
}

fn success(output: String) -> ToolOutcome {
    ToolOutcome {
        output,
        is_error: false,
    }
}

pub fn definitions() -> Vec<Value> {
    vec![
        json!({"type":"function","name":"read","description":"Read a UTF-8 file. Use offset and limit to read a large file by line.","parameters":{"type":"object","properties":{"path":{"type":"string"},"offset":{"type":"integer","minimum":1},"limit":{"type":"integer","minimum":1,"maximum":2000}},"required":["path"],"additionalProperties":false}}),
        json!({"type":"function","name":"write","description":"Write a UTF-8 file, creating parent directories if needed.","parameters":{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false}}),
        json!({"type":"function","name":"edit","description":"Replace exactly one occurrence of old_text in a UTF-8 file.","parameters":{"type":"object","properties":{"path":{"type":"string"},"old_text":{"type":"string"},"new_text":{"type":"string"}},"required":["path","old_text","new_text"],"additionalProperties":false}}),
        json!({"type":"function","name":"bash","description":"Run a bash command in the workspace. This uses the current user's host privileges.","parameters":{"type":"object","properties":{"command":{"type":"string"}},"required":["command"],"additionalProperties":false}}),
        json!({"type":"function","name":WEBFETCH,"description":"Search the web with a query or fetch a page by URL. Returns readable text and links without executing JavaScript. Supply exactly one of query or url.","parameters":{"type":"object","properties":{"query":{"type":"string"},"url":{"type":"string"}},"additionalProperties":false}}),
    ]
}

pub fn summary(name: &str, args: &Value) -> String {
    let value = match name {
        "read" | "write" | "edit" => args.get("path").and_then(Value::as_str),
        "bash" => args.get("command").and_then(Value::as_str),
        _ => None,
    }
    .unwrap_or("");
    let short: String = value.chars().take(120).collect();
    match name {
        "read" => format!("Read {short}"),
        "write" => format!("Write {short}"),
        "edit" => format!("Edit {short}"),
        "bash" => format!("Run {short}"),
        WEBFETCH => format!(
            "Web {}",
            args.get("query")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .or_else(|| args.get("url").and_then(Value::as_str))
                .unwrap_or("")
                .chars()
                .take(120)
                .collect::<String>()
        ),
        "plugin_call" => format!(
            "Plugin {}: {}",
            args.get("plugin").and_then(Value::as_str).unwrap_or(""),
            args.get("tool").and_then(Value::as_str).unwrap_or("")
        ),
        _ => "Run tool".to_owned(),
    }
}

fn path(args: &Value, workspace: &Path, host_root: &Path) -> Result<PathBuf> {
    let value = args
        .get("path")
        .and_then(Value::as_str)
        .context("path is required")?;
    ensure!(
        !value.is_empty() && value.len() <= 4096 && !value.contains('\0'),
        "invalid path"
    );
    let input = Path::new(value);
    ensure!(
        !input
            .components()
            .any(|part| matches!(part, Component::ParentDir)),
        "parent traversal is unavailable"
    );
    let candidate = if input.is_absolute() {
        input.to_path_buf()
    } else {
        workspace.join(input)
    };
    let ancestor = candidate
        .ancestors()
        .find(|p| p.exists())
        .context("path has no existing ancestor")?;
    let resolved_ancestor = ancestor.canonicalize().context("resolve path")?;
    let suffix = candidate.strip_prefix(ancestor)?;
    let resolved = if suffix.as_os_str().is_empty() {
        resolved_ancestor
    } else {
        resolved_ancestor.join(suffix)
    };
    let private = host_root.canonicalize().context("resolve host data")?;
    ensure!(
        !resolved.starts_with(&private),
        "host state is unavailable to tools"
    );
    Ok(resolved)
}

fn required<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    let value = args
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("{key} is required"))?;
    ensure!(value.len() <= MAX_INPUT_BYTES, "{key} is too large");
    Ok(value)
}

pub fn bash_executable() -> PathBuf {
    #[cfg(windows)]
    {
        for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(root) = std::env::var_os(variable) {
                let candidate = PathBuf::from(root).join("Git").join("bin").join("bash.exe");
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
    }
    PathBuf::from("bash")
}

pub async fn execute(
    name: &str,
    args: &Value,
    workspace: &Path,
    host_root: &Path,
) -> Result<ToolOutcome> {
    match name {
        "read" => {
            let path = path(args, workspace, host_root)?;
            let size = tokio::fs::metadata(&path)
                .await
                .context("inspect file")?
                .len();
            ensure!(
                size <= MAX_INPUT_BYTES as u64,
                "file is too large; use bash to select a bounded range"
            );
            let content = tokio::fs::read_to_string(path).await.context("read file")?;
            let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(1);
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(2000);
            ensure!(
                (1..=1_000_000_000).contains(&offset) && (1..=2000).contains(&limit),
                "invalid line range"
            );
            let mut output = String::new();
            let total_lines = content.lines().count();
            for (index, line) in content
                .lines()
                .enumerate()
                .skip((offset - 1) as usize)
                .take(limit as usize)
            {
                let item = format!("{}\t{}\n", index + 1, line);
                ensure!(
                    output.len() + item.len() <= MAX_RESULT_BYTES,
                    "read result is too large; reduce limit"
                );
                output.push_str(&item);
            }
            if total_lines > (offset as usize - 1).saturating_add(limit as usize) {
                output.push_str(&format!(
                    "[more lines available; next offset={}]",
                    offset + limit
                ));
            }
            Ok(success(output))
        }
        "write" => {
            let path = path(args, workspace, host_root)?;
            let content = required(args, "content")?;
            let parent = path.parent().context("file has no parent")?;
            tokio::fs::create_dir_all(parent)
                .await
                .context("create parent directory")?;
            tokio::fs::write(&path, content)
                .await
                .context("write file")?;
            Ok(success(format!(
                "Wrote {} bytes to {}",
                content.len(),
                required(args, "path")?
            )))
        }
        "edit" => {
            let path = path(args, workspace, host_root)?;
            let old = required(args, "old_text")?;
            let new = required(args, "new_text")?;
            ensure!(!old.is_empty(), "old_text is empty");
            let content = tokio::fs::read_to_string(&path)
                .await
                .context("read file")?;
            ensure!(
                content.len() <= MAX_INPUT_BYTES,
                "file is too large to edit"
            );
            ensure!(
                content.matches(old).count() == 1,
                "old_text must match exactly once"
            );
            let updated = content.replacen(old, new, 1);
            ensure!(updated.len() <= MAX_INPUT_BYTES, "edited file is too large");
            tokio::fs::write(&path, updated)
                .await
                .context("edit file")?;
            Ok(success(format!("Edited {}", required(args, "path")?)))
        }
        "bash" => {
            let command = required(args, "command")?;
            ensure!(
                !command.trim().is_empty() && command.len() <= 16_384,
                "invalid command"
            );
            let mut child = Command::new(bash_executable())
                .arg("-lc")
                .arg(command)
                .current_dir(workspace)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .context("start bash")?;
            let stdout = child.stdout.take().context("bash stdout unavailable")?;
            let stderr = child.stderr.take().context("bash stderr unavailable")?;
            let collect = async {
                let mut out = Vec::new();
                let mut err = Vec::new();
                let mut stdout = stdout;
                let mut stderr = stderr;
                let mut out_done = false;
                let mut err_done = false;
                let mut out_buffer = [0_u8; 8192];
                let mut err_buffer = [0_u8; 8192];
                while !out_done || !err_done {
                    tokio::select! {
                        count = stdout.read(&mut out_buffer), if !out_done => {
                            let count = count.context("read bash stdout")?;
                            if count == 0 { out_done = true; } else { out.extend_from_slice(&out_buffer[..count]); }
                        }
                        count = stderr.read(&mut err_buffer), if !err_done => {
                            let count = count.context("read bash stderr")?;
                            if count == 0 { err_done = true; } else { err.extend_from_slice(&err_buffer[..count]); }
                        }
                    }
                    ensure!(
                        out.len().saturating_add(err.len()) <= MAX_RESULT_BYTES,
                        "bash output is too large"
                    );
                }
                let status = child.wait().await.context("wait for bash")?;
                let out = String::from_utf8_lossy(&out);
                let err = String::from_utf8_lossy(&err);
                Ok::<_, anyhow::Error>(ToolOutcome {
                    output: format!("exit status: {status}\nstdout:\n{out}\nstderr:\n{err}"),
                    is_error: !status.success(),
                })
            };
            timeout(BASH_TIMEOUT, collect)
                .await
                .context("bash timed out")?
        }
        WEBFETCH => webfetch::fetch(args).await,
        _ => bail!("unknown tool"),
    }
}

/// A UTF-8 safe byte page with an exact next offset.
pub fn page(value: &str, offset: usize, max_bytes: usize) -> Result<(String, usize)> {
    ensure!(
        offset <= value.len() && value.is_char_boundary(offset),
        "invalid page offset"
    );
    let mut end = offset.saturating_add(max_bytes).min(value.len());
    while end > offset && !value.is_char_boundary(end) {
        end -= 1;
    }
    ensure!(
        end > offset || offset == value.len(),
        "page size is too small"
    );
    Ok((value[offset..end].to_owned(), end))
}

pub fn model_preview(value: &str, files: &AgentFiles) -> String {
    const HALF: usize = 4_000;
    if value.len() <= HALF * 2 {
        return value.to_owned();
    }
    let (head, first) = page(value, 0, HALF).unwrap_or_default();
    let mut tail_start = value.len().saturating_sub(HALF);
    while tail_start < value.len() && !value.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    let location = match files.write(&format!("results/{}.txt", uuid::Uuid::new_v4()), value.as_bytes()) {
        Ok(path) => format!("Full output: {}. Read it with read (line offset/limit), or use bash to select a bounded range.", serde_json::to_string(&path.to_string_lossy()).unwrap_or_default()),
        Err(_) => "Could not save the full output to a temporary file. Full output remains in the session tool details.".to_owned(),
    };
    format!(
        "{head}\n[{} bytes omitted. {location}]\n{}",
        tail_start.saturating_sub(first),
        &value[tail_start..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn base_tools_and_extension_arguments_have_bounded_contracts() {
        let names = definitions()
            .into_iter()
            .filter_map(|item| item["name"].as_str().map(str::to_owned))
            .collect::<Vec<_>>();
        assert_eq!(names, ["read", "write", "edit", "bash", "webfetch"]);
        assert!(
            serde_json::from_value::<PluginCallArgs>(
                json!({"plugin":"example","server":"main","tool":"read","arguments":{}})
            )
            .is_ok()
        );
        assert!(serde_json::from_value::<PluginCallArgs>(json!({"plugin":"example","server":"main","tool":"read","arguments":{},"extra":true})).is_err());
        assert_eq!(plugin_call_definition()["name"], "plugin_call");
    }

    #[test]
    fn preview_and_pages_recover_middle_without_utf8_loss() {
        let value = format!("{}é{}", "a".repeat(12000), "z".repeat(12000));
        let files = AgentFiles::new().unwrap();
        let preview = model_preview(&value, &files);
        assert!(preview.contains("Full output:"));
        let result = std::fs::read_dir(files.root().join("results"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(std::fs::read_to_string(&result).unwrap(), value);
        assert!(preview.contains(&serde_json::to_string(&result.to_string_lossy()).unwrap()));
        assert_eq!(model_preview("small", &files), "small");
        std::fs::remove_dir_all(files.root()).unwrap();
        let (first, next) = page(&value, 12000, 8000).unwrap();
        assert!(first.starts_with('é'));
        assert!(next > 12000);
    }

    #[tokio::test]
    async fn spilled_output_can_be_read_by_line_without_exposing_private_host_data() {
        let root = std::env::temp_dir().join(format!("beale-tools-example-{}", Uuid::new_v4()));
        let workspace = root.join("workspace");
        let host = root.join("host");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&host).unwrap();
        std::fs::write(host.join("private.txt"), "Synthetic private host value").unwrap();
        let files = AgentFiles::new().unwrap();
        let output = (0..2000)
            .map(|i| format!("Example line {i}: é\n"))
            .collect::<String>();
        let preview = model_preview(&output, &files);
        assert!(preview.contains("bytes omitted"));
        let path = std::fs::read_dir(files.root().join("results"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let result = execute(
            "read",
            &json!({"path":path,"offset":1000,"limit":2}),
            &workspace,
            &host,
        )
        .await
        .unwrap();
        assert!(result.output.contains("1000\tExample line 999: é"));
        assert!(result.output.contains("1001\tExample line 1000: é"));
        assert!(!result.output.contains("Example line 1001: é"));
        assert!(
            execute(
                "read",
                &json!({"path":host.join("private.txt")}),
                &workspace,
                &host
            )
            .await
            .is_err()
        );
        std::fs::remove_dir_all(files.root().join("results")).unwrap();
        std::fs::write(files.root().join("results"), "Blocked destination").unwrap();
        let failed = model_preview(&output, &files);
        assert!(failed.contains("Could not save the full output"));
        assert!(!failed.contains("Full output:"));
        std::fs::remove_dir_all(files.root()).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn base_tools_execute_and_keep_host_state_private() {
        let root = std::env::temp_dir().join(format!("beale-tools-example-{}", Uuid::new_v4()));
        let workspace = root.join("workspace");
        let host = root.join("host");
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        tokio::fs::create_dir_all(&host).await.unwrap();
        execute(
            "write",
            &json!({"path":"notes/example.txt","content":"alpha\nbeta\n"}),
            &workspace,
            &host,
        )
        .await
        .unwrap();
        assert_eq!(
            execute(
                "read",
                &json!({"path":"notes/example.txt","offset":2,"limit":1}),
                &workspace,
                &host
            )
            .await
            .unwrap()
            .output,
            "2\tbeta\n"
        );
        execute(
            "edit",
            &json!({"path":"notes/example.txt","old_text":"beta","new_text":"gamma"}),
            &workspace,
            &host,
        )
        .await
        .unwrap();
        assert!(
            tokio::fs::read_to_string(workspace.join("notes/example.txt"))
                .await
                .unwrap()
                .contains("gamma")
        );
        assert!(
            execute(
                "read",
                &json!({"path":host.join("state.sqlite").to_string_lossy()}),
                &workspace,
                &host
            )
            .await
            .is_err()
        );
        assert!(
            execute(
                "write",
                &json!({"path":"../host/state.sqlite","content":"x"}),
                &workspace,
                &host
            )
            .await
            .is_err()
        );
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    #[cfg_attr(
        windows,
        ignore = "requires Windows process access outside restricted sandbox"
    )]
    async fn bash_executes_in_workspace() {
        let root = std::env::temp_dir().join(format!("beale-bash-example-{}", Uuid::new_v4()));
        let workspace = root.join("workspace");
        let host = root.join("host");
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        tokio::fs::create_dir_all(&host).await.unwrap();
        let outcome = execute(
            "bash",
            &json!({"command":"printf example"}),
            &workspace,
            &host,
        )
        .await
        .unwrap();
        assert!(!outcome.is_error, "{}", outcome.output);
        assert!(outcome.output.contains("example"));
        let failed = execute("bash", &json!({"command":"exit 7"}), &workspace, &host)
            .await
            .unwrap();
        assert!(failed.is_error);
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
