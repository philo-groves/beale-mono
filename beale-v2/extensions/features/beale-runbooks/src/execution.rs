use anyhow::{Context, Result, bail, ensure};
use beale_contracts::runbooks::{Cell, Document};
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
        "bash" | "sh" => Command::new(beale_tools::bash_executable()),
        "python" | "python3" | "py" => {
            Command::new(if cfg!(windows) { "python" } else { "python3" })
        }
        "javascript" | "js" | "node" => Command::new("node"),
        "powershell" | "ps1" | "pwsh" => Command::new(if cfg!(windows) {
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

async fn run_cell(language: &str, source: &str, workspace: &Path) -> Result<RunOutput> {
    let mut child = command(language)?
        .current_dir(workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("start {language} interpreter"))?;
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
            ensure!(
                out.len().saturating_add(err.len()) <= MAX_CELL_OUTPUT,
                "runbook cell output is too large"
            );
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
    timeout(CELL_TIMEOUT, execute)
        .await
        .context("runbook cell timed out")?
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
