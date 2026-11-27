//! Worker half of mixed Runbook execution. Only local recovery state is opened.
use super::*;
use beale_contracts::{
    runbooks::Cell,
    swarm_runbooks::{CellOutcome, Reply, Request, ResultKind},
};

const RUN_BUDGET_MS: u64 = 300_000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedCell {
    outcome: CellOutcome,
    elapsed_ms: u64,
}

impl Worker<'_> {
    fn runbook_operation(&self, call: &str, phase: &str) -> String {
        format!(
            "runbook-{}",
            beale_storage::swarm_files::hash(
                json!([self.launch.identity, call, phase])
                    .to_string()
                    .as_bytes()
            )
        )
    }

    async fn runbook_remote(&self, id: String, operation: Request) -> Result<Reply> {
        let value = self
            .remote(self.request(
                id,
                WorkerOperation::Runbook {
                    request: operation.clone(),
                },
            ))
            .await?;
        let reply: Reply = serde_json::from_value(value)?;
        reply
            .validate_for(&self.launch.identity, &operation)
            .map_err(anyhow::Error::msg)?;
        Ok(reply)
    }

    pub(super) async fn execute_runbook(
        &self,
        pending: &PendingCall,
        arguments: &Value,
    ) -> Result<CallOutcome> {
        let parsed: Arguments = match serde_json::from_value::<Arguments>(arguments.clone()) {
            Ok(parsed) if valid_id(&parsed.id) => parsed,
            _ => {
                return self
                    .reject_call(
                        pending,
                        "Runbook requires a valid id and no extra arguments.",
                    )
                    .await;
            }
        };
        let call = &pending.call;
        let prepare_id = self.runbook_operation(&call.id, "prepare");
        let prepared = self
            .runbook_remote(
                prepare_id.clone(),
                Request::Prepare {
                    runbook_id: parsed.id,
                },
            )
            .await?;
        let mut operations = vec![prepare_id];
        let run = match prepared.result {
            ResultKind::Prepared { run } => run,
            ResultKind::Rejected { error } => {
                let mut outcome = self.reject_call(pending, &error.message).await?;
                outcome.operations = operations;
                return Ok(outcome);
            }
            ResultKind::Denied {} => {
                let mut outcome = self
                    .reject_call(pending, "The leader did not approve this Runbook.")
                    .await?;
                outcome.operations = operations;
                return Ok(outcome);
            }
            _ => anyhow::bail!("invalid Runbook preparation"),
        };
        // Preflight every language before running any code, as in local runs.
        let preflight = beale_runbooks::validate_worker_languages(&run.document).err();
        let redactor = beale_tools::Redactor::new([self.launch.bearer.clone()]);
        let mut elapsed = 0_u64;
        for cell in &run.document.cells {
            let Cell::Code {
                id,
                language,
                source,
                ..
            } = cell
            else {
                continue;
            };
            self.deadline()?;
            let begin_id = self.runbook_operation(&call.id, &format!("begin:{id}"));
            self.runbook_remote(
                begin_id.clone(),
                Request::BeginCell {
                    run_id: run.run_id.clone(),
                    cell_id: id.clone(),
                },
            )
            .await?;
            operations.push(begin_id);
            let cell_id = self.runbook_operation(&call.id, &format!("execute:{id}"));
            let publication = self.runbook_operation(&call.id, &format!("result:{id}"));
            let invocation = ToolInvocation {
                id: cell_id.clone(),
                name: "runbook_cell".into(),
                arguments: json!({"run_id":run.run_id,"cell":cell}),
            };
            let saved: SavedCell = match self
                .spool
                .claim(&WorkRequest::Environment { call: invocation })?
            {
                Claim::Replay(value) => serde_json::from_value(value)?,
                Claim::Dispatch => {
                    let started = Instant::now();
                    let outcome = if preflight.is_some() || elapsed >= RUN_BUDGET_MS {
                        CellOutcome {
                            cell_id: id.clone(),
                            success: false,
                            exit_code: None,
                            stdout: String::new(),
                            stderr: if preflight.is_some() {
                                "Runbook contains an unsupported interpreter language."
                            } else {
                                "Runbook execution time budget exhausted."
                            }
                            .into(),
                        }
                    } else {
                        let result = beale_runbooks::run_worker_cell(
                            language,
                            source,
                            &self.launch.workspace,
                            Duration::from_millis((RUN_BUDGET_MS - elapsed).min(60_000)),
                        )
                        .await?;
                        CellOutcome {
                            cell_id: id.clone(),
                            success: result.success,
                            exit_code: result.exit_code,
                            stdout: redactor.redact(&result.stdout).into_owned(),
                            stderr: redactor.redact(&result.stderr).into_owned(),
                        }
                    };
                    outcome.validate().map_err(anyhow::Error::msg)?;
                    let saved = SavedCell {
                        outcome,
                        elapsed_ms: u64::try_from(started.elapsed().as_millis())?,
                    };
                    self.spool
                        .complete(&cell_id, &serde_json::to_value(&saved)?)?;
                    saved
                }
                Claim::OutcomeUnknown | Claim::InFlight => {
                    anyhow::bail!("Runbook cell outcome is unknown; parent reconciliation required")
                }
                Claim::RetryLeader => anyhow::bail!("invalid Runbook execution journal"),
            };
            saved.outcome.validate().map_err(anyhow::Error::msg)?;
            ensure!(
                saved.outcome.cell_id == *id,
                "Runbook journal cell mismatch"
            );
            elapsed = elapsed
                .checked_add(saved.elapsed_ms)
                .context("Runbook elapsed time overflow")?;
            let success = saved.outcome.success;
            self.runbook_remote(
                publication.clone(),
                Request::Cell {
                    run_id: run.run_id.clone(),
                    outcome: saved.outcome,
                },
            )
            .await?;
            operations.extend([cell_id, publication]);
            if !success {
                break;
            }
        }
        let result = self
            .runbook_remote(call.id.clone(), Request::Finish { run_id: run.run_id })
            .await?;
        let ResultKind::Finished { run } = result.result else {
            anyhow::bail!("invalid Runbook completion");
        };
        operations.push(call.id.clone());
        let result = json!({"output":json!({"run_id":run.run_id,"runbook_id":run.runbook_id,"status":if run.success {"completed"} else {"failed"},"output":run.output}).to_string(),"is_error":!run.success});
        let result = self.bounded_outcome(call, result).await?;
        let result = self.publish_outcome(call, result).await?;
        self.publish_tool_outcome(pending, arguments, &result)
            .await?;
        Ok(CallOutcome { result, operations })
    }
}
