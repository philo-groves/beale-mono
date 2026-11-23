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
pub mod process;
pub mod redact;
pub use agent_files::AgentFiles;
pub use redact::Redactor;

pub const MAX_RESULT_BYTES: usize = 8_000_000;
const MAX_INPUT_BYTES: usize = 8_000_000;
pub const DEFAULT_COMMAND_TIMEOUT_SECONDS: u64 = 120;
pub const MAX_COMMAND_TIMEOUT_SECONDS: u64 = 600;
pub const MAX_COMMAND_BYTES: usize = 16_384;
pub const COMMAND_TOOL_NAME: &str = if cfg!(windows) { "powershell" } else { "bash" };
const COMMAND_DESCRIPTION: &str = if cfg!(windows) {
    "Run a PowerShell command on this host, or on an approved Hyper-V guest when machine is set to its Fleet VM ID. Guest commands start in the SSH user's directory. Set timeout_seconds for long operations; default 120 seconds."
} else {
    "Run a Bash command in the workspace with the current user's host privileges. Set timeout_seconds for long operations; default 120 seconds."
};

/// The bounded result of a contained shell command.
pub struct CommandOutput {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub status: String,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    pub fn transcript(&self) -> String {
        format!(
            "exit status: {}\nstdout:\n{}\nstderr:\n{}",
            self.status, self.stdout, self.stderr
        )
    }
}

/// Runs the host platform's command shell in the workspace. A timeout,
/// oversized output, or dropped future ends the command's whole process tree;
/// background processes started by a command that exits normally keep running.
pub async fn run_command(
    command: &str,
    workspace: &Path,
    timeout_seconds: u64,
) -> Result<CommandOutput> {
    ensure!(
        (1..=MAX_COMMAND_TIMEOUT_SECONDS).contains(&timeout_seconds),
        "invalid command timeout_seconds"
    );
    ensure!(
        !command.trim().is_empty() && command.len() <= MAX_COMMAND_BYTES,
        "invalid command"
    );
    #[cfg(windows)]
    let process = {
        let system_root =
            std::env::var_os("SystemRoot").context("Windows system root unavailable")?;
        let executable = PathBuf::from(system_root)
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        ensure!(executable.is_file(), "Windows PowerShell unavailable");
        let mut process = process::contained_command(executable);
        process.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]);
        process.arg(powershell_script(command));
        process
    };
    #[cfg(not(windows))]
    let process = {
        let mut process = process::contained_command(bash_executable());
        process.arg("-lc").arg(command);
        process
    };
    run_contained_process(process, workspace, timeout_seconds).await
}

/// Script wrapper shared with the Windows guest command path.
pub fn powershell_script(command: &str) -> String {
    format!(
        "$ErrorActionPreference = 'Stop'; [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false);\n{command}\nif ($null -ne $LASTEXITCODE) {{ exit $LASTEXITCODE }}"
    )
}

/// Collect bounded output from a process made with `contained_command`.
pub async fn run_contained_process(
    mut process: Command,
    workspace: &Path,
    timeout_seconds: u64,
) -> Result<CommandOutput> {
    ensure!(
        (1..=MAX_COMMAND_TIMEOUT_SECONDS).contains(&timeout_seconds),
        "invalid command timeout_seconds"
    );
    process
        .current_dir(workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    enable_git_long_paths(&mut process);
    let mut child = process.spawn().context("start command")?;
    let tree = process::ProcessTree::attach(&child);
    let stdout = child.stdout.take().context("command stdout unavailable")?;
    let stderr = child.stderr.take().context("command stderr unavailable")?;
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
                    let count = count.context("read command stdout")?;
                    if count == 0 { out_done = true; } else { out.extend_from_slice(&out_buffer[..count]); }
                }
                count = stderr.read(&mut err_buffer), if !err_done => {
                    let count = count.context("read command stderr")?;
                    if count == 0 { err_done = true; } else { err.extend_from_slice(&err_buffer[..count]); }
                }
            }
            ensure!(
                out.len().saturating_add(err.len()) <= MAX_RESULT_BYTES,
                "command output is too large"
            );
        }
        let status = child.wait().await.context("wait for command")?;
        Ok::<_, anyhow::Error>(CommandOutput {
            success: status.success(),
            exit_code: status.code(),
            status: status.to_string(),
            stdout: String::from_utf8_lossy(&out).into_owned(),
            stderr: String::from_utf8_lossy(&err).into_owned(),
        })
    };
    let output = timeout(Duration::from_secs(timeout_seconds), collect)
        .await
        .with_context(|| format!("command timed out after {timeout_seconds} seconds"))??;
    tree.release();
    Ok(output)
}

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
        json!({"type":"function","name":COMMAND_TOOL_NAME,"description":COMMAND_DESCRIPTION,"parameters":{"type":"object","properties":{"command":{"type":"string"},"timeout_seconds":{"type":"integer","minimum":1,"maximum":600},"machine":{"type":"string","description":"Optional approved Fleet VM ID on Windows. Omit to run on this host."}},"required":["command"],"additionalProperties":false}}),
    ]
}

pub fn summary(name: &str, args: &Value) -> String {
    let value = match name {
        "read" | "write" | "edit" => args.get("path").and_then(Value::as_str),
        name if name == COMMAND_TOOL_NAME => args.get("command").and_then(Value::as_str),
        _ => None,
    }
    .unwrap_or("");
    let short: String = value.chars().take(120).collect();
    match name {
        "read" => format!("Read {short}"),
        "write" => format!("Write {short}"),
        "edit" => format!("Edit {short}"),
        name if name == COMMAND_TOOL_NAME => {
            args.get("machine").and_then(Value::as_str).map_or_else(
                || format!("Run {short}"),
                |machine| format!("Run on {machine}: {short}"),
            )
        }
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

#[cfg(windows)]
fn enable_git_long_paths(command: &mut Command) {
    let index = match std::env::var("GIT_CONFIG_COUNT") {
        Ok(value) => match value.parse::<usize>() {
            Ok(index) if index < 32 => index,
            _ => return,
        },
        Err(std::env::VarError::NotPresent) => 0,
        Err(std::env::VarError::NotUnicode(_)) => return,
    };
    command
        .env("GIT_CONFIG_COUNT", (index + 1).to_string())
        .env(format!("GIT_CONFIG_KEY_{index}"), "core.longpaths")
        .env(format!("GIT_CONFIG_VALUE_{index}"), "true");
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
                "file is too large; use the command tool to select a bounded range"
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
        name if name == COMMAND_TOOL_NAME => {
            ensure!(
                args.get("machine").is_none(),
                "machine requires Fleet dispatch"
            );
            let command = required(args, "command")?;
            let timeout_seconds = args
                .get("timeout_seconds")
                .map_or(Ok(DEFAULT_COMMAND_TIMEOUT_SECONDS), |value| {
                    value.as_u64().context("invalid command timeout_seconds")
                })?;
            let output = run_command(command, workspace, timeout_seconds).await?;
            Ok(ToolOutcome {
                output: output.transcript(),
                is_error: !output.success,
            })
        }
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
        Ok(path) => format!("Full output: {}. Read it with read (line offset/limit), or use {COMMAND_TOOL_NAME} to select a bounded range.", serde_json::to_string(&path.to_string_lossy()).unwrap_or_default()),
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

    #[tokio::test]
    async fn webfetch_is_not_a_base_execution_tool() {
        let result = execute("webfetch", &json!({}), Path::new("."), Path::new(".")).await;
        assert!(result.err().unwrap().to_string().contains("unknown tool"));
    }

    #[tokio::test]
    async fn command_timeout_is_bounded_before_process_start() {
        let definition = definitions().pop().unwrap();
        assert_eq!(
            definition["parameters"]["properties"]["timeout_seconds"]["maximum"],
            600
        );
        for seconds in [0, 601] {
            let error = execute(
                COMMAND_TOOL_NAME,
                &json!({"command":"printf example","timeout_seconds":seconds}),
                Path::new("."),
                Path::new("."),
            )
            .await
            .err()
            .unwrap();
            assert!(error.to_string().contains("timeout_seconds"));
        }
    }

    #[test]
    fn base_tools_and_extension_arguments_have_bounded_contracts() {
        let tools = definitions();
        let names = tools
            .iter()
            .filter_map(|item| item["name"].as_str().map(str::to_owned))
            .collect::<Vec<_>>();
        assert_eq!(names, ["read", "write", "edit", COMMAND_TOOL_NAME]);
        assert_eq!(
            COMMAND_TOOL_NAME,
            if cfg!(windows) { "powershell" } else { "bash" }
        );
        assert!(
            tools[3]["description"]
                .as_str()
                .unwrap()
                .contains(if cfg!(windows) { "PowerShell" } else { "Bash" })
        );
        assert_eq!(
            tools[3]["parameters"]["properties"]["machine"]["type"],
            "string"
        );
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
    async fn command_executes_in_workspace() {
        let root = std::env::temp_dir().join(format!("beale-command-example-{}", Uuid::new_v4()));
        let workspace = root.join("workspace");
        let host = root.join("host");
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        tokio::fs::create_dir_all(&host).await.unwrap();
        let outcome = execute(
            COMMAND_TOOL_NAME,
            &json!({"command":if cfg!(windows) { "Write-Output example" } else { "printf example" }}),
            &workspace,
            &host,
        )
        .await
        .unwrap();
        assert!(!outcome.is_error, "{}", outcome.output);
        assert!(outcome.output.contains("example"));
        let failed = execute(
            COMMAND_TOOL_NAME,
            &json!({"command":"exit 7"}),
            &workspace,
            &host,
        )
        .await
        .unwrap();
        assert!(failed.is_error);
        #[cfg(windows)]
        {
            let failed_native = execute(
                COMMAND_TOOL_NAME,
                &json!({"command":"cmd.exe /c exit 7"}),
                &workspace,
                &host,
            )
            .await
            .unwrap();
            assert!(failed_native.is_error);
        }
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn machine_argument_cannot_fall_back_to_host_execution() {
        let result = execute(
            COMMAND_TOOL_NAME,
            &json!({"command":"exit 0","machine":"hyper-v:00000000-0000-4000-8000-000000000001"}),
            Path::new("."),
            Path::new("."),
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn command_does_not_inherit_credential_variables() {
        let root = std::env::temp_dir().join(format!("beale-command-env-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let command = if cfg!(windows) {
            r#"Get-ChildItem Env: | ForEach-Object { "$($_.Name)=$($_.Value)" }"#
        } else {
            "env"
        };
        let output = run_command(command, &root, 30).await.unwrap();
        assert!(output.success, "{}", output.stderr);
        for line in output.stdout.lines() {
            let name = line.split('=').next().unwrap_or_default();
            assert!(
                !process::secret_like_env_name(name),
                "tool environment exposed {name}"
            );
        }
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn bash_timeout_ends_background_descendants() {
        let root = std::env::temp_dir().join(format!("beale-bash-tree-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let timed_out = run_command(
            "sleep 30 >/dev/null 2>&1 & echo $! > child.pid; wait",
            &root,
            2,
        )
        .await;
        assert!(timed_out.is_err());
        tokio::time::sleep(Duration::from_millis(500)).await;
        let pid = tokio::fs::read_to_string(root.join("child.pid"))
            .await
            .unwrap();
        let check = run_command(
            &format!(
                "kill -0 {} 2>/dev/null && echo alive || echo gone",
                pid.trim()
            ),
            &root,
            10,
        )
        .await
        .unwrap();
        assert_eq!(check.stdout.trim(), "gone");
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn normal_exit_keeps_background_processes() {
        let root = std::env::temp_dir().join(format!("beale-bash-release-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let started = run_command("sleep 30 >/dev/null 2>&1 & echo $!", &root, 10)
            .await
            .unwrap();
        let pid = started.stdout.trim().to_owned();
        let check = run_command(
            &format!(
                "kill -0 {pid} 2>/dev/null && echo alive || echo gone; kill {pid} 2>/dev/null; true"
            ),
            &root,
            10,
        )
        .await
        .unwrap();
        assert_eq!(check.stdout.lines().next(), Some("alive"));
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
