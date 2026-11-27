//! A feature rejection is acknowledged only after its failed mutation rolled back.
use super::gateway::unavailable;
use crate::{SessionState, tool_runtime};
use axum::http::StatusCode;
use beale_contracts::{
    swarms::{MutationAcknowledgement, WORKER_AUDIENCE, WorkerRequest},
    tool_error::{MAX_TOOL_ERROR_BYTES, ToolError},
};
use beale_storage::swarm_state::SwarmStore;

pub(super) fn acknowledge(
    state: &SessionState,
    store: &SwarmStore,
    bearer: &str,
    request: &WorkerRequest,
    result: anyhow::Result<MutationAcknowledgement>,
) -> Result<MutationAcknowledgement, StatusCode> {
    acknowledge_with(state, store, bearer, request, result, |error| {
        Ok(error.feedback())
    })
}

pub(super) fn acknowledge_with(
    state: &SessionState,
    store: &SwarmStore,
    bearer: &str,
    request: &WorkerRequest,
    result: anyhow::Result<MutationAcknowledgement>,
    feedback: impl FnOnce(ToolError) -> anyhow::Result<serde_json::Value>,
) -> Result<MutationAcknowledgement, StatusCode> {
    match result {
        Ok(ack) => Ok(ack),
        Err(error) => {
            let Some(rejection) = error.downcast_ref::<ToolError>() else {
                return Err(unavailable(error));
            };
            let mut rejection = rejection.clone();
            rejection.message = tool_runtime::host_redactor(state)
                .redact(&rejection.message)
                .into_owned();
            rejection.message = beale_tools::Redactor::new([bearer.to_owned()])
                .redact(&rejection.message)
                .into_owned();
            rejection
                .message
                .truncate(rejection.message.floor_char_boundary(MAX_TOOL_ERROR_BYTES));
            rejection
                .validate()
                .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
            // Recheck the current lease/branch and the original receipt. A concurrent
            // success wins; a fenced worker cannot turn an old error into acceptance.
            store
                .dispatch_mutation(
                    bearer,
                    WORKER_AUDIENCE,
                    request,
                    tool_runtime::unix_ms(),
                    |_, _| feedback(rejection),
                )
                .map_err(unavailable)
        }
    }
}
