//! Leader-owned Runbook snapshots and result publication for assigned workers.
use super::gateway::{review, unavailable};
use crate::{SessionState, tool_runtime};
use axum::http::StatusCode;
use beale_contracts::{
    runbooks::Cell,
    swarm_runbooks::{Reply, Request, ResultKind},
    swarm_worker::ExecutionIdentity,
    swarms::{MutationAcknowledgement, WORKER_AUDIENCE, WorkerOperation, WorkerRequest},
    tool_error::ToolError,
};
use beale_runbooks::{RunbookStore, assigned};
use beale_storage::swarm_state::{CallerContext, SwarmStore};
use serde_json::json;

pub(super) async fn dispatch(
    state: &SessionState,
    store: &SwarmStore,
    bearer: &str,
    request: &WorkerRequest,
    caller: &CallerContext,
) -> Result<MutationAcknowledgement, StatusCode> {
    let WorkerOperation::Runbook { request: operation } = &request.operation else {
        return Err(StatusCode::BAD_REQUEST);
    };
    let identity = ExecutionIdentity {
        workspace_id: request.workspace_id.clone(),
        scan_id: request.scan_id.clone(),
        agent_id: request.agent_id.clone(),
        attempt_id: request.attempt_id.clone(),
        generation: request.generation,
    };
    let turn = store
        .worker_progress(&request.attempt_id)
        .map_err(unavailable)?
        .turn;
    let redactor = tool_runtime::host_redactor(state);
    let acknowledge_prepare = |outcome| {
        super::tool_errors::acknowledge_with(state, store, bearer, request, outcome, |error| {
            Ok(serde_json::to_value(Reply {
                identity: identity.clone(),
                result: ResultKind::Rejected { error },
            })?)
        })
    };
    let mut denied = false;
    let reviewed = if let Request::Prepare { runbook_id } = operation {
        let document = state
            .db
            .runbook_get(&request.workspace_id, runbook_id)
            .map_err(unavailable)?;
        let Some(document) = document else {
            return acknowledge_prepare(Err(ToolError::missing("Runbook not found").into()));
        };
        let serialized =
            serde_json::to_string(&document).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        // Do not send host-held secrets to workers or silently change code that
        // will be reviewed/executed. The canonical document remains unchanged.
        if redactor.redact(&serialized) != serialized {
            return acknowledge_prepare(Err(ToolError::rejected(
                "Runbook contains protected host data and cannot be sent to the worker.",
            )
            .into()));
        }
        let shell = store
            .worker_bootstrap(&request.attempt_id)
            .map_err(unavailable)?
            .shell;
        let mut reviews = vec![(beale_runbooks::RUN, json!({"id":runbook_id}))];
        for cell in &document.cells {
            if let Cell::Code {
                language, source, ..
            } = cell
            {
                reviews.push((
                    shell.tool(),
                    json!({"command":format!("# {language} runbook cell\n{source}")}),
                ));
            }
        }
        for (tool, args) in reviews {
            match review(state, store, request, caller, tool, &args).await {
                Ok(()) => (),
                Err(StatusCode::FORBIDDEN) => {
                    denied = true;
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        Some(document)
    } else {
        None
    };
    let outcome = store.dispatch_mutation(
        bearer,
        WORKER_AUDIENCE,
        request,
        tool_runtime::unix_ms(),
        |tx, _| {
            let result = match operation {
                Request::Prepare { .. } if denied => ResultKind::Denied {},
                Request::Prepare { .. } => ResultKind::Prepared {
                    run: assigned::prepare(
                        tx,
                        &identity,
                        &turn.session_id,
                        reviewed
                            .as_ref()
                            .ok_or_else(|| anyhow::anyhow!("Runbook review missing"))?,
                    )?,
                },
                Request::BeginCell { run_id, cell_id } => {
                    assigned::begin_cell(tx, &identity, run_id, cell_id)?;
                    ResultKind::CellReady {
                        run_id: run_id.clone(),
                        cell_id: cell_id.clone(),
                    }
                }
                Request::Cell { run_id, outcome } => {
                    let mut outcome = outcome.clone();
                    outcome.stdout = redactor.redact(&outcome.stdout).into_owned();
                    outcome.stderr = redactor.redact(&outcome.stderr).into_owned();
                    assigned::accept_cell(tx, &identity, run_id, &outcome)?;
                    ResultKind::CellAccepted {
                        run_id: run_id.clone(),
                        cell_id: outcome.cell_id,
                    }
                }
                Request::Finish { run_id } => ResultKind::Finished {
                    run: assigned::finish(tx, &identity, run_id)?,
                },
            };
            let reply = Reply {
                identity: identity.clone(),
                result,
            };
            reply
                .validate_for(&identity, operation)
                .map_err(anyhow::Error::msg)?;
            Ok(serde_json::to_value(reply)?)
        },
    );
    if matches!(operation, Request::Prepare { .. }) {
        acknowledge_prepare(outcome)
    } else {
        outcome.map_err(unavailable)
    }
}
