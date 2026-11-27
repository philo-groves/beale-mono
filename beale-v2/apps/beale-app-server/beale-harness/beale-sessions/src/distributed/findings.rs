//! Leader half of verification. Commands never execute in this dispatcher.
use super::gateway::{review, unavailable};
use crate::{SessionState, tool_runtime};
use axum::http::StatusCode;
use beale_contracts::{
    swarm_findings::{MAX_OBSERVATION_BYTES, Observation, Reply, Request, ResultKind},
    swarm_worker::ExecutionIdentity,
    swarms::{MutationAcknowledgement, WORKER_AUDIENCE, WorkerOperation, WorkerRequest},
    tool_error::ToolError,
};
use beale_findings::assigned;
use beale_storage::swarm_state::{CallerContext, SwarmStore};
use serde_json::json;

pub(super) async fn dispatch(
    state: &SessionState,
    store: &SwarmStore,
    bearer: &str,
    request: &WorkerRequest,
    caller: &CallerContext,
) -> Result<MutationAcknowledgement, StatusCode> {
    let WorkerOperation::Verify { request: operation } = &request.operation else {
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
    if let Request::Prepare { command, .. } = operation {
        let arguments = serde_json::to_value(command).map_err(|error| unavailable(error.into()))?;
        let encoded = arguments.to_string();
        if redactor.redact(&encoded) != encoded {
            return acknowledge_prepare(Err(ToolError::rejected("Verification command contains protected host data and cannot be sent to the worker.").into()));
        }
        let shell = store
            .worker_bootstrap(&request.attempt_id)
            .map_err(unavailable)?
            .shell;
        for (tool, args) in [
            (beale_findings::VERIFY, arguments),
            (
                shell.tool(),
                json!({"command":command.command,"timeout_seconds":command.timeout_seconds}),
            ),
        ] {
            match review(state, store, request, caller, tool, &args).await {
                Ok(()) => (),
                Err(StatusCode::FORBIDDEN) => {
                    denied = true;
                    break;
                }
                Err(error) => return Err(error),
            }
        }
    }
    let published = if let Request::Observe {
        run_id,
        index,
        artifact_id,
    } = operation
    {
        let files = crate::research_files::store(state).map_err(unavailable)?;
        let artifact = files
            .artifact(&request.workspace_id, artifact_id)
            .map_err(unavailable)?;
        if artifact.size > MAX_OBSERVATION_BYTES {
            return Err(StatusCode::BAD_REQUEST);
        }
        let bytes = files
            .read_blob(&artifact.sha256, artifact.size)
            .map_err(unavailable)?;
        let mut observation: Observation =
            serde_json::from_slice(&bytes).map_err(|_| StatusCode::BAD_REQUEST)?;
        observation
            .validate()
            .map_err(|_| StatusCode::BAD_REQUEST)?;
        if observation.identity != identity
            || observation.run_id != *run_id
            || observation.index != *index
        {
            return Err(StatusCode::BAD_REQUEST);
        }
        observation.stdout = redactor.redact(&observation.stdout).into_owned();
        observation.stderr = redactor.redact(&observation.stderr).into_owned();
        observation.error = observation
            .error
            .map(|error| redactor.redact(&error).into_owned());
        Some((artifact, observation))
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
                Request::Prepare {
                    tool_call_id,
                    command,
                } => ResultKind::Prepared {
                    run_id: assigned::prepare(
                        tx,
                        &identity,
                        &turn.session_id,
                        &turn.profile,
                        tool_call_id,
                        command,
                    )?,
                },
                Request::Observe {
                    run_id,
                    index,
                    artifact_id,
                } => {
                    // Acceptance must still exist in the same transaction as the
                    // verifier receipt, including if retention won the race.
                    beale_storage::swarm_files::validate_record_artifacts(
                        tx,
                        &identity.scan_id,
                        &identity.workspace_id,
                        &json!({"artifact_refs":[artifact_id]}),
                    )?;
                    let (artifact, observation) = published
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("verification artifact missing"))?;
                    assigned::observe(tx, &identity, artifact, observation)?;
                    ResultKind::Observed {
                        run_id: run_id.clone(),
                        index: *index,
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
