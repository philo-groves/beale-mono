//! Host supervisor acceptance. Worker finish messages carry no release authority.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerDisposition {
    Completed,
    Canceled,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerReconciliation {
    TerminationUnconfirmed,
    TurnAccepted,
    BranchStopped,
}

struct AcceptWorker<'a> {
    turn: &'a LocalTurn,
    disposition: WorkerDisposition,
    revision: u64,
    activity_stamp: worker_activity::Stamp,
    event_count: usize,
    answer: &'a str,
}

impl crate::TurnExtension for AcceptWorker<'_> {
    fn commit(&self, tx: &Transaction<'_>, _: &crate::CommittedTurn<'_>) -> Result<()> {
        let current: i64 = tx.query_row(
            "SELECT revision FROM swarm_worker_runtime WHERE attempt_id=?1",
            [&self.turn.attempt_id],
            |r| r.get(0),
        )?;
        ensure!(
            u64::try_from(current)? == self.revision,
            "worker candidate changed during reconciliation"
        );
        ensure!(
            worker_activity::stamp(tx, self.turn)? == self.activity_stamp,
            "worker inference changed during reconciliation"
        );
        let state = read_scan(tx, &self.turn.scan_id)?;
        ensure!(
            state
                .events
                .iter()
                .filter(|event| event.agent_id == self.turn.agent_id
                    && event.kind == "worker-event"
                    && event.payload["attempt_id"] == self.turn.attempt_id)
                .count()
                == self.event_count,
            "worker events changed during reconciliation"
        );
        let stopped = local::read_local(tx, &self.turn.scan_id)?
            .controls
            .iter()
            .any(|control| control.agent_id == self.turn.agent_id && control.stop_requested);
        match if stopped {
            WorkerDisposition::Canceled
        } else {
            self.disposition
        } {
            WorkerDisposition::Completed | WorkerDisposition::Canceled => local::finish_turn(
                tx,
                self.turn,
                json!({"session_id":self.turn.session_id,"saved":true}),
                self.answer,
                self.disposition == WorkerDisposition::Canceled,
                true,
            )?,
            WorkerDisposition::Failed => local::fail_turn(tx, self.turn, Some("worker"), true)?,
        }
        fence_in_transaction(tx, &self.turn.scan_id, &self.turn.attempt_id, true)?;
        acknowledge(
            tx,
            &self.turn.scan_id,
            "supervisor",
            &format!("reconcile:{}", self.turn.attempt_id),
            &serde_json::to_value(self.disposition)?,
            &serde_json::to_value(if self.disposition == WorkerDisposition::Completed {
                WorkerReconciliation::TurnAccepted
            } else {
                WorkerReconciliation::BranchStopped
            })?,
        )
    }
}

impl SwarmStore {
    /// Call only with termination evidence from the owning process/backend.
    /// Fencing alone cannot accept candidate completion or release capacity.
    pub fn reconcile_worker(
        &self,
        attempt: &str,
        termination_confirmed: bool,
        disposition: WorkerDisposition,
        now_ms: i64,
    ) -> Result<WorkerReconciliation> {
        self.reconcile_worker_with_extensions(
            attempt,
            termination_confirmed,
            disposition,
            now_ms,
            &[],
        )
    }

    /// Optional feature-owned SQL joins the same transcript/release transaction.
    /// Extensions run only after confirmed termination and never on receipt replay.
    pub fn reconcile_worker_with_extensions(
        &self,
        attempt: &str,
        termination_confirmed: bool,
        disposition: WorkerDisposition,
        now_ms: i64,
        extensions: &[&dyn crate::TurnExtension],
    ) -> Result<WorkerReconciliation> {
        ensure!(now_ms >= 0, "invalid reconciliation time");
        let progress = self.worker_progress(attempt)?;
        let turn = &progress.turn;
        let operation = format!("reconcile:{attempt}");
        let content = serde_json::to_value(disposition)?;
        let replay = || {
            self.transaction(|tx| {
                receipt(tx, &turn.scan_id, "supervisor", &operation, &content)?
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(Into::into)
            })
        };
        if let Some(result) = replay()? {
            return Ok(result);
        }
        if !termination_confirmed {
            self.transaction(|tx| {
                fence_in_transaction(tx, &turn.scan_id, attempt, false)?;
                let mut state = read_scan(tx, &turn.scan_id)?;
                let id = format!("termination:{attempt}");
                if !state.events.iter().any(|e| e.id == id) {
                    let sequence = next(state.events.last().map_or(0, |event| event.sequence))?;
                    state.events.push(LifecycleEvent { id, agent_id: turn.agent_id.clone(), sequence, kind: "worker-termination-unconfirmed".into(), payload: json!({"attempt_id":attempt,"generation":turn.generation,"warning":"Managed execution termination is unconfirmed; replacement research remains blocked."}) });
                    write_scan(tx, &mut state)?;
                }
                Ok(())
            })?;
            return Ok(WorkerReconciliation::TerminationUnconfirmed);
        }
        let saved = self.load(&turn.scan_id, true)?;
        let bootstrap = self.worker_bootstrap(attempt)?;
        let settings = self.local_state(&turn.scan_id)?.settings;
        let index = saved
            .config
            .provider_routes
            .iter()
            .position(|route| route.id == bootstrap.route_id)
            .context("missing prepared worker route")?;
        let model = settings
            .models
            .get(index)
            .context("missing prepared worker model")?;
        let completed = disposition == WorkerDisposition::Completed;
        let answer = match disposition {
            WorkerDisposition::Completed => {
                let answer = progress
                    .answer
                    .as_deref()
                    .context("worker has no final candidate")?;
                ensure!(
                    progress.revision > 0
                        && progress.context["phase"]["phase"] == "done"
                        && progress.context["phase"]["answer"] == answer,
                    "worker candidate is not a completed checkpoint"
                );
                answer
            }
            WorkerDisposition::Canceled => {
                "Assigned execution stopped by request. Accepted activity and candidate recovery state are retained."
            }
            WorkerDisposition::Failed => {
                "Assigned execution recovery exhausted. Partial results and uncertain effects require parent reconciliation before new research."
            }
        };
        let retained = if completed {
            let input = progress.context["input"]
                .as_array()
                .context("worker conversation missing")?;
            ensure!(
                input.iter().all(|item| item.is_object()
                    && !matches!(item["role"].as_str(), Some("system" | "developer"))),
                "invalid worker conversation"
            );
            Some(serde_json::to_string(input)?)
        } else {
            None
        };
        let history = self.db.session(&turn.session_id)?;
        if let Some(history) = &history {
            ensure!(
                history.session.workspace_id == saved.config.workspace_id,
                "worker session belongs to another workspace"
            );
        }
        let number = history
            .as_ref()
            .and_then(|history| history.turns.last())
            .map_or(Ok(1), |last| {
                last.number.checked_add(1).context("turn number exhausted")
            })?;
        let session = crate::SessionRow {
            id: turn.session_id.clone(),
            workspace_id: saved.config.workspace_id,
            title: "Swarm research".into(),
            created_at_ms: history
                .as_ref()
                .map_or(now_ms, |history| history.session.created_at_ms),
            last_message_at_ms: now_ms,
        };
        let mut tools = Vec::new();
        for event in &saved.events {
            if event.agent_id != turn.agent_id
                || event.kind != "worker-event"
                || event.payload["attempt_id"] != attempt
                || event.payload["event"]["kind"] != "tool_outcome"
            {
                continue;
            }
            let payload = &event.payload["event"];
            let name = payload["tool"]
                .as_str()
                .context("invalid worker tool event")?;
            let result = &payload["result"];
            tools.push(crate::ToolCallRow {
                id: new_id(),
                turn_number: number,
                ordinal: i64::try_from(tools.len())?,
                name: name.into(),
                summary: "Assigned worker tool outcome".into(),
                arguments: serde_json::to_string(&payload["arguments"])?,
                output: result["output"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| result.to_string()),
                is_error: result["is_error"].as_bool().unwrap_or(false),
                agent_role: turn.profile.clone(),
                created_at_ms: event.payload["observed_at_ms"]
                    .as_i64()
                    .context("worker event observation time missing")?,
            });
        }
        let activity = self.transaction(|tx| {
            worker_activity::project(tx, turn, model, &bootstrap.route_id, number, tools.len())
        })?;
        let extension = AcceptWorker {
            turn,
            disposition,
            revision: progress.revision,
            activity_stamp: activity.stamp,
            event_count: saved
                .events
                .iter()
                .filter(|event| {
                    event.agent_id == turn.agent_id
                        && event.kind == "worker-event"
                        && event.payload["attempt_id"] == attempt
                })
                .count(),
            answer,
        };
        let mut commits: Vec<&dyn crate::TurnExtension> = vec![&extension];
        commits.extend_from_slice(extensions);
        let result = self.db.append_turn_with_metadata(
            &session,
            &crate::TurnRow {
                number,
                provider: model.provider,
                method: model.method,
                model: model.model.clone(),
                prompt: turn.transcript_prompt().to_owned(),
                answer: answer.into(),
                duration_ms: i64::try_from(progress.active_elapsed_ms)?,
                internal_prompt: turn.operator_text.is_none(),
            },
            history.is_none(),
            crate::TurnActivity {
                tools: &tools,
                reasoning: &activity.reasoning,
                commentary: &activity.commentary,
            },
            None,
            crate::TurnMetadata {
                model_prompt: turn.operator_text.as_ref().map(|_| turn.prompt.as_str()),
                provider_context_json: retained.as_deref(),
                agent_role: Some(&turn.profile),
                model_usage: Some(&activity.usage),
                extensions: &commits,
                ..Default::default()
            },
        );
        match result {
            Ok(()) => replay()?.context("worker reconciliation receipt missing"),
            // A concurrent owner can have committed this exact finalization.
            Err(error) => match replay()? {
                Some(result) => Ok(result),
                None => Err(error),
            },
        }
    }
}
