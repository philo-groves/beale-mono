//! The worker audience is distinct from host control. Every operation rechecks
//! its lease and saved caller identity; errors never return host diagnostics.
use crate::{SessionState, engine, tool_runtime};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use beale_contracts::{
    swarm_local::Admission,
    swarm_worker::{ExecutionIdentity, LeaseGrant, WORKER_PATH, WorkerReply, WorkerReviewResult},
    swarms::{MAX_DOCUMENT_BYTES, Role, WORKER_AUDIENCE, WorkerOperation, WorkerRequest},
    v1::SubmitSessionTurnRequest,
};
use beale_storage::swarm_state::{CallerContext, LocalModel, SwarmStore};
use serde_json::{Value, json};

const LEASE_MILLIS: i64 = 30_000;

pub(crate) fn router() -> Router<SessionState> {
    Router::new()
        .route(WORKER_PATH, post(dispatch))
        .layer(DefaultBodyLimit::max(MAX_DOCUMENT_BYTES))
}

pub(super) fn unavailable(error: anyhow::Error) -> StatusCode {
    beale_host_log::error!("worker operation was not acknowledged: {error:#}");
    StatusCode::SERVICE_UNAVAILABLE
}

pub(super) fn selected_model(
    store: &SwarmStore,
    request: &WorkerRequest,
    caller: &CallerContext,
) -> anyhow::Result<LocalModel> {
    model_for_agent(
        store,
        &request.scan_id,
        &request.agent_id,
        &caller.agent.profile.id,
    )
}

pub(super) fn model_for_agent(
    store: &SwarmStore,
    scan: &str,
    agent: &str,
    profile: &str,
) -> anyhow::Result<LocalModel> {
    use anyhow::Context;
    let state = store.load(scan, true)?;
    let settings = store.local_state(scan)?.settings;
    let choices: Vec<_> = settings
        .models
        .iter()
        .filter(|model| model.profile == profile)
        .collect();
    let ordinal = state
        .agents
        .iter()
        .filter(|a| a.summary.profile.id == profile)
        .position(|a| a.summary.id == agent)
        .context("worker model identity unavailable")?;
    let index = ordinal
        .checked_rem(choices.len())
        .context("worker model route unavailable")?;
    Ok((*choices[index]).clone())
}

pub(super) async fn review(
    state: &SessionState,
    store: &SwarmStore,
    request: &WorkerRequest,
    caller: &CallerContext,
    tool: &str,
    arguments: &Value,
) -> Result<(), StatusCode> {
    let leader_reviews =
        caller.agent.role == Role::Leader && !beale_swarms::coordination_tool(tool);
    if !leader_reviews
        && !matches!(tool, "bash" | "powershell" | "plugin_call")
        && !tool_runtime::requires_call_review(tool)
    {
        return Ok(());
    }
    let model = selected_model(store, request, caller).map_err(unavailable)?;
    let validated = engine::validate_request(&SubmitSessionTurnRequest {
        provider: model.provider,
        method: model.method,
        model: model.model.clone(),
        prompt: "Review assigned worker operation".into(),
        ..Default::default()
    })
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let call = beale_safety::review_agent(
        &state.db,
        &state.provider,
        &state.credentials,
        beale_safety::ReviewRequest {
            provider: validated.provider,
            mode: validated.mode,
            model: &model.model,
            name: tool,
            arguments,
            workspace_context: &caller.authorized_scope,
        },
        &request.scan_id,
        &request.agent_id,
    );
    let decision = tokio::select! {
        decision=call=>decision,
        _=async {loop {tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            let live=store.load(&request.scan_id,true).is_ok_and(|scan|scan.ended_at_ms.is_none() && scan.attempts.iter().any(|a|a.id==request.attempt_id && a.generation==request.generation && a.state==beale_contracts::swarms::ExecutionState::Running && a.expires_at_ms>tool_runtime::unix_ms()));
            if !live {break}
        }}=>return Err(StatusCode::UNAUTHORIZED),
    };
    if decision.approved {
        Ok(())
    } else if decision.reviewed {
        Err(StatusCode::FORBIDDEN)
    } else {
        Err(StatusCode::SERVICE_UNAVAILABLE)
    }
}

async fn dispatch(
    State(state): State<SessionState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<WorkerReply>, StatusCode> {
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| v.len() == 64)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let request = WorkerRequest::decode_json(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let store = SwarmStore::open(state.db.clone()).map_err(unavailable)?;
    let now = tool_runtime::unix_ms();
    let caller = store
        .authorize(bearer, WORKER_AUDIENCE, &request, now)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    if matches!(request.operation, WorkerOperation::Heartbeat {}) {
        let current = store
            .load(&request.scan_id, true)
            .map_err(unavailable)?
            .attempts
            .into_iter()
            .find(|attempt| attempt.id == request.attempt_id)
            .ok_or(StatusCode::UNAUTHORIZED)?
            .expires_at_ms;
        let expiry = now
            .checked_add(LEASE_MILLIS)
            .ok_or(StatusCode::SERVICE_UNAVAILABLE)?
            .max(current);
        store
            .renew(bearer, &request, now, expiry)
            .map_err(unavailable)?;
        return Ok(Json(WorkerReply::Lease {
            grant: LeaseGrant {
                identity: ExecutionIdentity {
                    workspace_id: request.workspace_id,
                    scan_id: request.scan_id,
                    agent_id: request.agent_id,
                    attempt_id: request.attempt_id,
                    generation: request.generation,
                },
                request_id: request.operation_id,
                valid_for_ms: LEASE_MILLIS as u64,
            },
        }));
    }
    if let WorkerOperation::SnapshotRead { request: read } = &request.operation {
        let files = crate::research_files::store(&state).map_err(unavailable)?;
        let chunk = files
            .snapshot_chunk(&request.scan_id, read)
            .map_err(unavailable)?;
        store
            .authorize(bearer, WORKER_AUDIENCE, &request, tool_runtime::unix_ms())
            .map_err(|_| StatusCode::UNAUTHORIZED)?;
        return Ok(Json(WorkerReply::Acknowledged {
            acknowledgement: beale_contracts::swarms::MutationAcknowledgement {
                version: beale_contracts::swarms::VERSION,
                operation_id: request.operation_id,
                result: serde_json::to_value(chunk)
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
            },
        }));
    }
    if !matches!(request.operation, WorkerOperation::ArtifactSeal { .. })
        && let Some(acknowledgement) = store
            .replay_worker_mutation(bearer, &request, now)
            .map_err(unavailable)?
    {
        return Ok(Json(WorkerReply::Acknowledged { acknowledgement }));
    }
    let acknowledgement = match &request.operation {
        WorkerOperation::SnapshotRead { .. } => return Err(StatusCode::BAD_REQUEST),
        WorkerOperation::PluginResources { .. } => {
            super::plugin_resources::dispatch(&state, &store, bearer, &request).await?
        }
        WorkerOperation::ArtifactBegin { .. }
        | WorkerOperation::ArtifactChunk { .. }
        | WorkerOperation::ArtifactSeal { .. }
        | WorkerOperation::ArtifactRead { .. } => {
            let files = crate::research_files::store(&state).map_err(unavailable)?;
            let result =
                store.worker_artifact_request(&files, bearer, &request, tool_runtime::unix_ms());
            if matches!(request.operation, WorkerOperation::ArtifactRead { .. }) {
                // An unknown or foreign artifact ID comes from the model. Record
                // the typed rejection so the worker can report it and continue.
                super::tool_errors::acknowledge(&state, &store, bearer, &request, result)?
            } else {
                result.map_err(unavailable)?
            }
        }
        WorkerOperation::Bootstrap {}
        | WorkerOperation::Checkpoint { .. }
        | WorkerOperation::Finish { .. } => store
            .worker_runtime_request(bearer, &request, tool_runtime::unix_ms())
            .map_err(unavailable)?,
        WorkerOperation::Review { call } => {
            let approved = match review(
                &state,
                &store,
                &request,
                &caller,
                &call.name,
                &call.arguments,
            )
            .await
            {
                Ok(()) => true,
                Err(StatusCode::FORBIDDEN) => false,
                Err(error) => return Err(error),
            };
            store
                .dispatch_mutation(
                    bearer,
                    WORKER_AUDIENCE,
                    &request,
                    tool_runtime::unix_ms(),
                    |_, _| Ok(serde_json::to_value(WorkerReviewResult { approved })?),
                )
                .map_err(unavailable)?
        }
        WorkerOperation::Runbook { .. } => {
            #[cfg(feature = "runbooks")]
            {
                super::runbooks::dispatch(&state, &store, bearer, &request, &caller).await?
            }
            #[cfg(not(feature = "runbooks"))]
            {
                return Err(StatusCode::NOT_IMPLEMENTED);
            }
        }
        WorkerOperation::Plugin { .. } => {
            super::plugins::dispatch(&state, &store, bearer, &request, &caller).await?
        }
        WorkerOperation::Verify { .. } => {
            #[cfg(feature = "findings")]
            {
                super::findings::dispatch(&state, &store, bearer, &request, &caller).await?
            }
            #[cfg(not(feature = "findings"))]
            {
                return Err(StatusCode::NOT_IMPLEMENTED);
            }
        }
        WorkerOperation::Infer { .. } => {
            super::inference::dispatch(&state, &store, bearer, &request, &caller).await?
        }
        WorkerOperation::Event { payload, .. } => {
            // Tool-outcome events carry local tool output; keep and project
            // them with the same host-secret redaction as leader tool results.
            let stored = tool_runtime::host_redactor(&state).redact_json(payload.clone());
            store
                .worker_event(bearer, &request, &stored, tool_runtime::unix_ms())
                .map_err(unavailable)?
        }
        WorkerOperation::Canonical { tool, arguments } => {
            match review(&state, &store, &request, &caller, tool, arguments).await {
                Ok(()) => (),
                Err(StatusCode::FORBIDDEN) => {
                    let acknowledgement = store.dispatch_mutation(
                        bearer, WORKER_AUDIENCE, &request, tool_runtime::unix_ms(),
                        |_, _| Ok(json!({"output":"The leader did not approve this tool call.","is_error":true})),
                    ).map_err(unavailable)?;
                    return Ok(Json(WorkerReply::Acknowledged { acknowledgement }));
                }
                Err(error) => return Err(error),
            }

            #[cfg(feature = "surfaces")]
            if matches!(
                tool.as_str(),
                beale_surfaces::SAVE | beale_surfaces::THREAT_MODEL_EDIT
            ) {
                let outcome =
                    (|| -> anyhow::Result<_> {
                        let target = if tool == beale_surfaces::SAVE {
                            beale_workspaces::SurfaceTarget::Manifest
                        } else {
                            beale_workspaces::SurfaceTarget::threat_model(
                                arguments["surface_id"].as_str().ok_or_else(|| {
                                    beale_contracts::tool_error::ToolError::invalid(
                                        "surface_id is required",
                                    )
                                })?,
                            )?
                        };
                        let operation = format!(
                            "worker-{}",
                            beale_storage::swarm_files::hash(
                                json!([
                                    request.workspace_id,
                                    request.scan_id,
                                    request.agent_id,
                                    request.attempt_id,
                                    request.generation,
                                    request.operation_id
                                ])
                                .to_string()
                                .as_bytes()
                            )
                        );
                        let request_hash = beale_storage::swarm_files::hash(
                            serde_json::to_string(&request)?.as_bytes(),
                        );
                        let redactor = tool_runtime::host_redactor(&state);
                        let arguments =
                            serde_json::from_str(&redactor.redact(&arguments.to_string()))?;
                        state
                    .workspaces
                    .with_surface_writer(&request.workspace_id, |writer| {
                        let acknowledgement = store.dispatch_mutation(
                            bearer,
                            WORKER_AUDIENCE,
                            &request,
                            tool_runtime::unix_ms(),
                            |tx, _| {
                                beale_storage::swarm_files::validate_record_artifacts(
                                    tx,
                                    &request.scan_id,
                                    &request.workspace_id,
                                    &arguments,
                                )?;
                                let output =
                                    writer.execute(&target, &operation, &request_hash, |view| {
                                        beale_surfaces::execute(
                                            view,
                                            &request.workspace_id,
                                            tool,
                                            &arguments,
                                            true,
                                        )
                                    })?;
                                Ok(serde_json::from_str(&redactor.redact(&output))?)
                            },
                        )?;
                        if let Err(error) = writer.forget(&target, &operation) {
                            beale_host_log::error!(
                                "accepted Surface operation retained recovery metadata: {error}"
                            );
                        }
                        Ok(acknowledgement)
                    })
                    })();
                let acknowledgement =
                    super::tool_errors::acknowledge(&state, &store, bearer, &request, outcome)?;
                return Ok(Json(WorkerReply::Acknowledged { acknowledgement }));
            }
            #[cfg(feature = "surfaces")]
            if matches!(tool.as_str(), beale_surfaces::QUERY | beale_surfaces::READ) {
                let outcome = beale_surfaces::execute(
                    &state.workspaces,
                    &request.workspace_id,
                    tool,
                    arguments,
                    true,
                )
                .and_then(|output| {
                    let redactor = tool_runtime::host_redactor(&state);
                    let output = redactor.redact(&output);
                    let result = serde_json::from_str(&output)?;
                    store.dispatch_mutation(
                        bearer,
                        WORKER_AUDIENCE,
                        &request,
                        tool_runtime::unix_ms(),
                        |_, _| Ok(result),
                    )
                });
                let acknowledgement =
                    super::tool_errors::acknowledge(&state, &store, bearer, &request, outcome)?;
                return Ok(Json(WorkerReply::Acknowledged { acknowledgement }));
            }
            #[cfg(feature = "agent-feedback")]
            if tool == beale_agent_feedback::TOOL_NAME {
                let turn = store
                    .worker_progress(&request.attempt_id)
                    .map_err(unavailable)?
                    .turn;
                let operation = format!(
                    "worker-{}",
                    beale_storage::swarm_files::hash(
                        json!([
                            request.workspace_id,
                            request.scan_id,
                            request.agent_id,
                            request.attempt_id,
                            request.generation,
                            request.operation_id
                        ])
                        .to_string()
                        .as_bytes(),
                    )
                );
                let arguments = serde_json::from_str(
                    &tool_runtime::host_redactor(&state).redact(&arguments.to_string()),
                )
                .map_err(|_| StatusCode::BAD_REQUEST)?;
                let outcome = store.dispatch_mutation(
                    bearer,
                    WORKER_AUDIENCE,
                    &request,
                    tool_runtime::unix_ms(),
                    |_, _| {
                        let output = beale_agent_feedback::execute_once(
                            state.workspaces.root(),
                            &operation,
                            &request.workspace_id,
                            &turn.session_id,
                            &request.agent_id,
                            &arguments,
                        )?;
                        Ok(json!({"output":output,"is_error":false}))
                    },
                );
                let acknowledgement =
                    super::tool_errors::acknowledge(&state, &store, bearer, &request, outcome)?;
                return Ok(Json(WorkerReply::Acknowledged { acknowledgement }));
            }
            #[cfg(feature = "memories")]
            if beale_memories::is_tool(tool) {
                let turn = store
                    .worker_progress(&request.attempt_id)
                    .map_err(unavailable)?
                    .turn;
                // Memory tools are read-only. Finish the read before acquiring the
                // receipt transaction: session queries use the host DB as well.
                let memory = beale_memories::MemoryStore::open(state.workspaces.root())
                    .map_err(unavailable)?;
                let outcome = beale_memories::execute(
                    &memory,
                    &state.db,
                    &request.workspace_id,
                    &turn.session_id,
                    tool,
                    arguments,
                )
                .and_then(|output| {
                    let redactor = tool_runtime::host_redactor(&state);
                    let output = redactor.redact(&output);
                    let result = serde_json::from_str(&output)?;
                    store.dispatch_mutation(
                        bearer,
                        WORKER_AUDIENCE,
                        &request,
                        tool_runtime::unix_ms(),
                        |_, _| Ok(result),
                    )
                });
                let acknowledgement =
                    super::tool_errors::acknowledge(&state, &store, bearer, &request, outcome)?;
                return Ok(Json(WorkerReply::Acknowledged { acknowledgement }));
            }
            #[cfg(feature = "runbooks")]
            if beale_runbooks::is_tool(tool) && tool != beale_runbooks::RUN {
                let redactor = tool_runtime::host_redactor(&state);
                let turn = store
                    .worker_progress(&request.attempt_id)
                    .map_err(unavailable)?
                    .turn;
                let outcome = store.dispatch_mutation(
                    bearer,
                    WORKER_AUDIENCE,
                    &request,
                    tool_runtime::unix_ms(),
                    |tx, _| {
                        let output = beale_runbooks::execute_saved(
                            &beale_runbooks::transaction_store(tx),
                            &request.workspace_id,
                            &turn.session_id,
                            tool,
                            arguments,
                        )?;
                        Ok(serde_json::from_str(&redactor.redact(&output))?)
                    },
                );
                let acknowledgement =
                    super::tool_errors::acknowledge(&state, &store, bearer, &request, outcome)?;
                return Ok(Json(WorkerReply::Acknowledged { acknowledgement }));
            }
            if tool.starts_with("swarm_") && tool != "swarm_materialize_artifact" {
                let admission = if tool == "swarm_spawn_subagent" {
                    let turn = store
                        .worker_progress(&request.attempt_id)
                        .map_err(unavailable)?
                        .turn;
                    super::placement::admission(&state, &turn).await
                } else {
                    Admission::Ready
                };
                let outcome =
                    store.worker_dispatch(bearer, &request, tool_runtime::unix_ms(), admission);
                super::tool_errors::acknowledge(&state, &store, bearer, &request, outcome)?
            } else {
                #[cfg(feature = "findings")]
                {
                    if !beale_findings::is_tool(tool) || tool == beale_findings::VERIFY {
                        return Err(StatusCode::NOT_IMPLEMENTED);
                    }

                    let redactor = tool_runtime::host_redactor(&state);
                    let outcome = store.dispatch_mutation(
                        bearer,
                        WORKER_AUDIENCE,
                        &request,
                        tool_runtime::unix_ms(),
                        |tx, _| {
                            let output = beale_findings::execute(
                                &beale_findings::transaction_store(tx),
                                &beale_findings::ToolContext {
                                    workspace_id: &request.workspace_id,
                                    session_id: &request.agent_id,
                                    evidence_role: None,
                                    pending: &[],
                                },
                                tool,
                                arguments,
                            )?;
                            Ok(serde_json::from_str(&redactor.redact(&output))?)
                        },
                    );
                    super::tool_errors::acknowledge(&state, &store, bearer, &request, outcome)?
                }
                #[cfg(not(feature = "findings"))]
                {
                    return Err(StatusCode::NOT_IMPLEMENTED);
                }
            }
        }
        WorkerOperation::Spawn { profile, task } => {
            review(
                &state,
                &store,
                &request,
                &caller,
                "swarm_spawn_subagent",
                &json!({"profile":profile,"task":task,"independent":false}),
            )
            .await?;
            let turn = store
                .worker_progress(&request.attempt_id)
                .map_err(unavailable)?
                .turn;
            let admission = super::placement::admission(&state, &turn).await;
            store
                .worker_dispatch(bearer, &request, tool_runtime::unix_ms(), admission)
                .map_err(unavailable)?
        }
        WorkerOperation::MessageParent { message } => {
            review(
                &state,
                &store,
                &request,
                &caller,
                "swarm_message_parent",
                &json!({"message":message,"handoff":null}),
            )
            .await?;
            store
                .worker_dispatch(bearer, &request, tool_runtime::unix_ms(), Admission::Ready)
                .map_err(unavailable)?
        }
        WorkerOperation::MessageChild { child_id, message } => {
            review(
                &state,
                &store,
                &request,
                &caller,
                "swarm_message_subagent",
                &json!({"child_id":child_id,"message":message}),
            )
            .await?;
            store
                .worker_dispatch(bearer, &request, tool_runtime::unix_ms(), Admission::Ready)
                .map_err(unavailable)?
        }
        WorkerOperation::StopChild { child_id } => {
            review(
                &state,
                &store,
                &request,
                &caller,
                "swarm_stop_subagent",
                &json!({"child_id":child_id}),
            )
            .await?;
            store
                .worker_dispatch(bearer, &request, tool_runtime::unix_ms(), Admission::Ready)
                .map_err(unavailable)?
        }
        _ => return Err(StatusCode::NOT_IMPLEMENTED),
    };
    let reply = WorkerReply::Acknowledged { acknowledgement };
    reply
        .validate_for(&request)
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    // Worker mutations can make agents eligible or change live status.
    crate::hierarchy::scheduler::wake(&state, &request.scan_id);
    Ok(Json(reply))
}
