use anyhow::{Context, Result, bail, ensure};
use beale_contracts::runbooks::{Cell, Document};
use beale_tools::process::{LimitExceeded, ProcessTree, contained_command};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    time::timeout,
};

const CELL_TIMEOUT: Duration = Duration::from_secs(60);
const RUN_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_CELL_OUTPUT: usize = 16_000;
const MAX_RUN_OUTPUT: usize = 64_000;

pub struct RunOutput {
    pub success: bool,
    pub output: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl RunOutput {
    fn aggregate(success: bool, output: String) -> Self {
        Self {
            success,
            output,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
        }
    }
}

fn command(language: &str) -> Result<Command> {
    let mut command = match language {
        "bash" | "sh" => contained_command(beale_tools::bash_executable()),
        "python" | "python3" | "py" => {
            contained_command(if cfg!(windows) { "python" } else { "python3" })
        }
        "javascript" | "js" | "node" => contained_command("node"),
        "powershell" | "ps1" | "pwsh" => contained_command(if cfg!(windows) {
            "powershell.exe"
        } else {
            "pwsh"
        }),
        _ => bail!("unsupported runbook language: {language}"),
    };
    match language {
        "bash" | "sh" => {
            command.arg("-s");
        }
        "python" | "python3" | "py" | "javascript" | "js" | "node" => {
            command.arg("-");
        }
        _ => {
            command.args(["-NoProfile", "-NonInteractive", "-Command", "-"]);
        }
    }
    Ok(command)
}

pub fn validate_worker_languages(document: &Document) -> Result<()> {
    for cell in &document.cells {
        if let Cell::Code { language, .. } = cell {
            command(language)?;
        }
    }
    Ok(())
}

/// Executes one already reviewed cell inside the dedicated worker's execution
/// tree. A time or output limit returns [`LimitExceeded`] only after the cell
/// was ended and may be recorded as a failed cell. Any other error after
/// dispatch remains uncertain and stops worker recovery.
pub async fn run_worker_cell(
    language: &str,
    source: &str,
    workspace: &Path,
    budget: Duration,
) -> Result<RunOutput> {
    beale_tools::process::require_worker_group()?;
    ensure!(
        !budget.is_zero() && budget <= CELL_TIMEOUT,
        "invalid Runbook cell budget"
    );
    run_cell_policy(language, source, workspace, budget, true).await
}

async fn run_cell(language: &str, source: &str, workspace: &Path) -> Result<RunOutput> {
    run_cell_policy(language, source, workspace, CELL_TIMEOUT, false).await
}

async fn run_cell_policy(
    language: &str,
    source: &str,
    workspace: &Path,
    budget: Duration,
    worker: bool,
) -> Result<RunOutput> {
    let mut process = command(language)?;
    if worker {
        beale_tools::process::retain_worker_group(&mut process)?;
    }
    let mut child = process
        .current_dir(workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("start {language} interpreter"))?;
    let tree = if worker && !beale_tools::process::worker_commands_are_isolated() {
        None
    } else {
        Some(ProcessTree::attach(&child))
    };
    let mut stdin = child.stdin.take().context("runbook stdin unavailable")?;
    let mut stdout = child.stdout.take().context("runbook stdout unavailable")?;
    let mut stderr = child.stderr.take().context("runbook stderr unavailable")?;
    let execute = async {
        stdin
            .write_all(source.as_bytes())
            .await
            .context("write runbook code")?;
        drop(stdin);
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut out_done = false;
        let mut err_done = false;
        let mut out_buffer = [0_u8; 4096];
        let mut err_buffer = [0_u8; 4096];
        while !out_done || !err_done {
            tokio::select! {
                read = stdout.read(&mut out_buffer), if !out_done => {
                    let count = read.context("read runbook stdout")?;
                    if count == 0 { out_done = true; } else { out.extend_from_slice(&out_buffer[..count]); }
                }
                read = stderr.read(&mut err_buffer), if !err_done => {
                    let count = read.context("read runbook stderr")?;
                    if count == 0 { err_done = true; } else { err.extend_from_slice(&err_buffer[..count]); }
                }
            }
            if out.len().saturating_add(err.len()) > MAX_CELL_OUTPUT {
                return Err(LimitExceeded::output("runbook cell").into());
            }
        }
        let status = child.wait().await.context("wait for runbook cell")?;
        let stdout = String::from_utf8_lossy(&out).into_owned();
        let stderr = String::from_utf8_lossy(&err).into_owned();
        Ok::<_, anyhow::Error>(RunOutput {
            success: status.success(),
            output: format!("exit status: {status}\nstdout:\n{stdout}\nstderr:\n{stderr}"),
            exit_code: status.code(),
            stdout,
            stderr,
        })
    };
    let error = match timeout(budget, execute).await {
        Ok(Ok(output)) => {
            if let Some(tree) = tree {
                tree.release();
            }
            return Ok(output);
        }
        Ok(Err(error)) => error,
        Err(_) => LimitExceeded::timed_out("runbook cell", budget).into(),
    };
    if error.downcast_ref::<LimitExceeded>().is_some() {
        // The interpreter child is still owned here; end it before reporting.
        let _ = child.start_kill();
        beale_tools::process::end_limited(tree, worker)
            .context("runbook cell termination was not confirmed")?;
    }
    Err(error)
}

pub async fn run_document(
    document: &Document,
    workspace: &Path,
    mut on_cell: impl FnMut(u32) -> Result<()>,
    mut on_result: impl FnMut(&str, bool, Option<i32>, &str, &str) -> Result<()>,
) -> Result<RunOutput> {
    ensure!(workspace.is_dir(), "workspace directory is unavailable");
    for cell in &document.cells {
        if let Cell::Code { language, .. } = cell {
            command(language)?;
        }
    }
    let run = async {
        let mut output = String::new();
        let mut count = 0;
        for (index, cell) in document.cells.iter().enumerate() {
            let Cell::Code {
                id,
                language,
                source,
                ..
            } = cell
            else {
                continue;
            };
            count += 1;
            on_cell(u32::try_from(index + 1)?)?;
            let result = run_cell(language, source, workspace).await;
            match &result {
                Ok(cell) => {
                    on_result(id, cell.success, cell.exit_code, &cell.stdout, &cell.stderr)?
                }
                Err(error) => on_result(
                    id,
                    false,
                    None,
                    "",
                    &format!("Runbook execution error: {error}"),
                )?,
            }
            let result = result?;
            output.push_str(&format!("Cell {id} ({language}):\n{}\n", result.output));
            ensure!(
                output.len() <= MAX_RUN_OUTPUT,
                "runbook output is too large"
            );
            if !result.success {
                return Ok(RunOutput::aggregate(false, output));
            }
        }
        if count == 0 {
            output.push_str("No executable code cells.");
        }
        Ok(RunOutput::aggregate(true, output))
    };
    timeout(RUN_TIMEOUT, run)
        .await
        .context("runbook timed out")?
}

pub async fn run_selected_cell(
    document: &Document,
    cell_id: &str,
    workspace: &Path,
) -> Result<RunOutput> {
    ensure!(workspace.is_dir(), "workspace directory is unavailable");
    let cell = document
        .cells
        .iter()
        .find(|cell| cell.id() == cell_id)
        .context("runbook cell not found")?;
    let Cell::Code {
        id,
        language,
        source,
        ..
    } = cell
    else {
        bail!("Markdown cells cannot be run");
    };
    let result = run_cell(language, source, workspace).await?;
    Ok(RunOutput {
        success: result.success,
        output: format!("Cell {id} ({language}):\n{}\n", result.output),
        exit_code: result.exit_code,
        stdout: result.stdout,
        stderr: result.stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[tokio::test]
    async fn worker_cell_timeout_terminates_managed_descendants() {
        let workspace = std::env::temp_dir().join(format!(
            "beale-worker-cell-example-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&workspace).unwrap();
        let child = "from pathlib import Path; import time; Path('ready.txt').write_text('ready'); time.sleep(2); Path('late.txt').write_text('must not execute')";
        let source = format!(
            "import subprocess,sys,time\nsubprocess.Popen([sys.executable,'-c',{}])\ntime.sleep(30)\n",
            serde_json::to_string(child).unwrap()
        );
        let error = run_worker_cell("python", &source, &workspace, Duration::from_secs(1))
            .await
            .err()
            .expect("cell must reach its time limit");
        // A limit is reported only after the managed tree was terminated, so
        // the worker can record a failed cell instead of an unknown outcome.
        assert_eq!(
            error.downcast_ref::<LimitExceeded>(),
            Some(&LimitExceeded::timed_out(
                "runbook cell",
                Duration::from_secs(1)
            ))
        );
        assert_eq!(error.to_string(), "runbook cell timed out after 1 second");
        assert!(
            workspace.join("ready.txt").exists(),
            "descendant must start before timeout"
        );
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(!workspace.join("late.txt").exists());
        std::fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn only_declared_interpreters_are_accepted() {
        for language in ["bash", "python", "javascript", "powershell"] {
            assert!(command(language).is_ok());
        }
        assert!(command("rust").is_err());
    }

    #[tokio::test]
    async fn executes_code_cells_in_order_and_stops_after_failure() {
        if std::process::Command::new("node")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let workspace = std::env::temp_dir().join(format!(
            "beale-runbook-execution-example-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&workspace).unwrap();
        let document = Document {
            id: "R1".into(),
            workspace_id: "workspace-example".into(),
            title: "Example".into(),
            purpose: "Synthetic test".into(),
            cells: vec![
                Cell::Markdown {
                    id: "L1".into(),
                    name: String::new(),
                    source: "Example guidance".into(),
                },
                Cell::Code {
                    id: "L2".into(),
                    name: String::new(),
                    language: "javascript".into(),
                    source: "console.log('example')\n".into(),
                },
                Cell::Code {
                    id: "L3".into(),
                    name: String::new(),
                    language: "javascript".into(),
                    source: "process.exit(3)\n".into(),
                },
                Cell::Code {
                    id: "L4".into(),
                    name: String::new(),
                    language: "javascript".into(),
                    source: "console.log('skipped')\n".into(),
                },
            ],
            next_cell_number: 5,
            created_at_ms: 0,
            updated_at_ms: 0,
            revision: 1,
        };
        let mut started = Vec::new();
        let mut finished = Vec::new();
        let result = run_document(
            &document,
            &workspace,
            |number| {
                started.push(number);
                Ok(())
            },
            |id, success, exit_code, stdout, stderr| {
                assert!(exit_code.is_some());
                assert!(stdout.contains("example") || !success);
                assert!(stderr.is_empty() || !success);
                finished.push((id.to_owned(), success));
                Ok(())
            },
        )
        .await
        .unwrap();
        assert_eq!(started, [2, 3]);
        assert_eq!(
            finished,
            [("L2".to_owned(), true), ("L3".to_owned(), false)]
        );
        assert!(!result.success);
        assert!(result.output.contains("Cell L2 (javascript)"));
        assert!(result.output.contains("example"), "{}", result.output);
        assert!(result.output.contains("Cell L3 (javascript)"));
        assert!(!result.output.contains("Cell L4"));
        let selected = run_selected_cell(&document, "L4", &workspace)
            .await
            .unwrap();
        assert!(selected.success);
        assert!(selected.output.contains("skipped"));
        assert_eq!(selected.exit_code, Some(0));
        assert!(selected.stdout.contains("skipped"));
        assert!(!selected.output.contains("Cell L2"));
        assert!(
            run_selected_cell(&document, "L1", &workspace)
                .await
                .is_err()
        );
        let mut successful = document.clone();
        successful.cells.truncate(2);
        let result = run_document(&successful, &workspace, |_| Ok(()), |_, _, _, _, _| Ok(()))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("example"));
        std::fs::remove_dir_all(workspace).unwrap();
    }
}
