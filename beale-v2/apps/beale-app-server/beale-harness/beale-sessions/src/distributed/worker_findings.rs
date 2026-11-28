//! Worker half of verification. Every fresh command has an independent durable
//! claim; transcript publication and final canonical attachment can be replayed.
use super::*;
use beale_contracts::swarm_findings::{
    Command, Observation, Reply, Request, ResultKind, VERIFY_RUNS,
};

impl Worker<'_> {
    fn verification_operation(&self, call: &str, phase: &str) -> String {
        format!(
            "verify-{}",
            beale_storage::swarm_files::hash(
                json!([self.launch.identity, call, phase])
                    .to_string()
                    .as_bytes()
            )
        )
    }
    async fn verification_remote(&self, id: String, operation: Request) -> Result<Reply> {
        let value = self
            .remote(self.request(
                id,
                WorkerOperation::Verify {
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
    pub(super) async fn execute_verification(
        &self,
        pending: &PendingCall,
        arguments: &Value,
    ) -> Result<CallOutcome> {
        let command: Command = match serde_json::from_value::<Command>(arguments.clone()) {
            Ok(command) if command.validate().is_ok() => command,
            _ => {
                return self
                    .reject_call(
                        pending,
                        "Invalid verification command, finding revision, oracle, or timeout.",
                    )
                    .await;
            }
        };
        let call = &pending.call;
        let prepare = self.verification_operation(&call.id, "prepare");
        let reply = self
            .verification_remote(
                prepare.clone(),
                Request::Prepare {
                    tool_call_id: call.id.clone(),
                    command: command.clone(),
                },
            )
            .await?;
        let mut operations = vec![prepare];
        let run_id = match reply.result {
            ResultKind::Prepared { run_id } => run_id,
            ResultKind::Rejected { error } => {
                let mut outcome = self.reject_call(pending, &error.message).await?;
                outcome.operations = operations;
                return Ok(outcome);
            }
            ResultKind::Denied {} => {
                let mut result = self
                    .reject_call(pending, "The leader did not approve this verification.")
                    .await?;
                result.operations = operations;
                return Ok(result);
            }
            _ => anyhow::bail!("invalid verification preparation"),
        };
        for index in 0..VERIFY_RUNS {
            self.deadline()?;
            let execute = self.verification_operation(&call.id, &format!("execute:{index}"));
            let publication = self.verification_operation(&call.id, &format!("artifact:{index}"));
            let acceptance = self.verification_operation(&call.id, &format!("observe:{index}"));
            let invocation = ToolInvocation {
                id: execute.clone(),
                name: "verification_run".into(),
                arguments: json!({"run_id":run_id,"index":index,"command":command}),
            };
            let source = match self
                .spool
                .claim(&WorkRequest::Environment { call: invocation })?
            {
                Claim::Replay(value) => value
                    .as_str()
                    .context("invalid verification transcript journal")?
                    .to_owned(),
                Claim::Dispatch => {
                    let started = Instant::now();
                    // A time or output limit ends the command before it is
                    // reported and becomes a failed run, as on the leader. Any
                    // other collection error can leave effects or descendants:
                    // let the supervisor prove termination; never repeat it.
                    let result = beale_tools::run_worker_command(
                        &command.command,
                        &self.launch.workspace,
                        command.timeout_seconds,
                    )
                    .await;
                    let duration_ms = i64::try_from(started.elapsed().as_millis())?;
                    let redactor = beale_tools::Redactor::new([self.launch.bearer.clone()]);
                    let observation = match result {
                        Ok(output) => Observation {
                            identity: self.launch.identity.clone(),
                            run_id: run_id.clone(),
                            index,
                            exit_code: output.exit_code,
                            stdout: redactor.redact(&output.stdout).into_owned(),
                            stderr: redactor.redact(&output.stderr).into_owned(),
                            duration_ms,
                            error: None,
                        },
                        Err(error) => {
                            let Some(limit) =
                                error.downcast_ref::<beale_tools::process::LimitExceeded>()
                            else {
                                return Err(error);
                            };
                            Observation {
                                identity: self.launch.identity.clone(),
                                run_id: run_id.clone(),
                                index,
                                exit_code: None,
                                stdout: String::new(),
                                stderr: String::new(),
                                duration_ms,
                                error: Some(limit.to_string()),
                            }
                        }
                    };
                    observation.validate().map_err(anyhow::Error::msg)?;
                    let serialized = serde_json::to_string(&observation)?;
                    let workspace = self.launch.workspace.clone();
                    let recovery = self.launch.recovery.clone();
                    let publication = publication.clone();
                    let source = tokio::task::spawn_blocking(move || -> Result<String> {
                        let source = beale_storage::swarm_files::retain_worker_output(
                            &workspace,
                            &publication,
                            &serialized,
                        )?;
                        // Capture immutable upload bytes before acknowledging
                        // the local effect; replay cannot pick up a later edit.
                        beale_storage::swarm_files::stage_worker_artifact(
                            &workspace,
                            &recovery,
                            &publication,
                            &source,
                        )?;
                        Ok(source)
                    })
                    .await??;
                    self.spool.complete(&execute, &json!(source))?;
                    source
                }
                Claim::OutcomeUnknown | Claim::InFlight => anyhow::bail!(
                    "verification command outcome is unknown; parent reconciliation required"
                ),
                Claim::RetryLeader => anyhow::bail!("invalid verification execution journal"),
            };
            let artifact_id = self.publish_artifact(&publication, &source).await?;
            self.verification_remote(
                acceptance.clone(),
                Request::Observe {
                    run_id: run_id.clone(),
                    index,
                    artifact_id,
                },
            )
            .await?;
            operations.extend([execute, acceptance]);
        }
        let reply = self
            .verification_remote(call.id.clone(), Request::Finish { run_id })
            .await?;
        let ResultKind::Finished { run } = reply.result else {
            anyhow::bail!("invalid verification completion");
        };
        operations.push(call.id.clone());
        let result = json!({"output":run.output,"is_error":!run.recorded || !run.verified});
        self.publish_tool_outcome(pending, arguments, &result)
            .await?;
        Ok(CallOutcome { result, operations })
    }
}
