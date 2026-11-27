//! Leader-owned MCP dispatch with durable effect claims and private captures.
use super::gateway::{review, unavailable};
use crate::{SessionState, tool_runtime};
use anyhow::{Context, Result, ensure};
use axum::http::StatusCode;
use beale_contracts::{
    swarm_files::ArtifactUpload,
    swarm_plugins::{Outcome, Reply},
    swarm_worker::{ExecutionIdentity, TransferProgress},
    swarms::{MutationAcknowledgement, WorkerOperation, WorkerRequest},
};
use beale_storage::{
    swarm_files::{hash, retain_worker_output, stage_worker_artifact},
    swarm_state::{CallerContext, PluginClaim, PluginOwner, SwarmStore},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{io::Read, path::PathBuf};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Capture {
    identity: ExecutionIdentity,
    operation_id: String,
    output: String,
    is_error: bool,
}
struct Saved {
    upload: ArtifactUpload,
    source: PathBuf,
    outcome: Outcome,
}
fn identity(r: &WorkerRequest) -> ExecutionIdentity {
    ExecutionIdentity {
        workspace_id: r.workspace_id.clone(),
        scan_id: r.scan_id.clone(),
        agent_id: r.agent_id.clone(),
        attempt_id: r.attempt_id.clone(),
        generation: r.generation,
    }
}
fn key(r: &WorkerRequest) -> Result<String> {
    Ok(hash(serde_json::to_string(r)?.as_bytes()))
}
fn location(state: &SessionState, r: &WorkerRequest) -> Result<PathBuf> {
    Ok(state
        .workspaces
        .root()
        .join("swarm-plugin-results")
        .join(key(r)?))
}

/// The immutable capture precedes result-table acknowledgement. Reopening it
/// cannot consult current plugin configuration or send another MCP request.
fn load_capture(state: &SessionState, r: &WorkerRequest) -> Result<Option<Saved>> {
    let root = location(state, r)?;
    if !root.try_exists()? {
        return Ok(None);
    }
    ensure!(
        std::fs::symlink_metadata(&root)?.file_type().is_dir(),
        "invalid plugin capture directory"
    );
    let operation = key(r)?;
    let relative = format!("artifacts/tool-output-{}.txt", hash(operation.as_bytes()));
    let (upload, source) =
        stage_worker_artifact(&root, &root.join("recovery"), &operation, &relative)?;
    ensure!(upload.size <= 64_000_000, "plugin capture is too large");
    let capture: Capture = serde_json::from_slice(&std::fs::read(&source)?)?;
    ensure!(
        capture.identity == identity(r) && capture.operation_id == r.operation_id,
        "plugin capture identity mismatch"
    );
    let mut outcome = Outcome::Completed {
        output: capture.output.clone(),
        is_error: capture.is_error,
        artifact_refs: vec![],
    };
    if outcome.validate().is_err() {
        let preview = &capture.output[..capture.output.floor_char_boundary(32_768)];
        outcome = Outcome::Completed {
            output: format!(
                "{preview}\n\nFull plugin response retained in artifact {} (execution identity and output envelope).",
                upload.id
            ),
            is_error: capture.is_error,
            artifact_refs: vec![upload.id.clone()],
        };
    }
    outcome.validate().map_err(anyhow::Error::msg)?;
    Ok(Some(Saved {
        upload,
        source,
        outcome,
    }))
}
fn capture(state: &SessionState, r: &WorkerRequest, bearer: &str, value: &Value) -> Result<Saved> {
    let root = location(state, r)?;
    std::fs::create_dir_all(&root)?;
    ensure!(
        std::fs::symlink_metadata(&root)?.file_type().is_dir(),
        "invalid plugin capture directory"
    );
    let redactor = tool_runtime::host_redactor(state);
    let redacted = redactor.redact(&value.to_string()).into_owned();
    let output = beale_tools::Redactor::new([bearer.to_owned()])
        .redact(&redacted)
        .into_owned();
    let value = Capture {
        identity: identity(r),
        operation_id: r.operation_id.clone(),
        output,
        is_error: value["isError"].as_bool().unwrap_or(false),
    };
    retain_worker_output(&root, &key(r)?, &serde_json::to_string(&value)?)?;
    load_capture(state, r)?.context("plugin capture missing")
}

fn publish(
    state: &SessionState,
    store: &SwarmStore,
    bearer: &str,
    r: &WorkerRequest,
    saved: &Saved,
) -> Result<()> {
    let Outcome::Completed { artifact_refs, .. } = &saved.outcome else {
        return Ok(());
    };
    if artifact_refs.is_empty() {
        return Ok(());
    }
    let files = crate::research_files::store(state)?;
    let key = key(r)?;
    let operation = |phase: &str, operation| {
        let mut request = r.clone();
        request.operation_id = format!(
            "plugin-output-{}",
            hash(format!("{key}:{phase}").as_bytes())
        );
        request.operation = operation;
        request
    };
    let call = |request: &WorkerRequest| {
        store.worker_artifact_request(&files, bearer, request, tool_runtime::unix_ms())
    };
    let begin = call(&operation(
        "begin",
        WorkerOperation::ArtifactBegin {
            artifact: saved.upload.clone(),
        },
    ))?;
    let progress: TransferProgress = serde_json::from_value(begin.result)?;
    ensure!(
        progress.next_offset <= saved.upload.size,
        "invalid plugin artifact progress"
    );
    let mut file = std::fs::File::open(&saved.source)?;
    use std::io::Seek;
    file.seek(std::io::SeekFrom::Start(progress.next_offset))?;
    let mut offset = progress.next_offset;
    while offset < saved.upload.size {
        let mut bytes = vec![0; usize::try_from((saved.upload.size - offset).min(192 * 1024))?];
        file.read_exact(&mut bytes)?;
        let next = offset + bytes.len() as u64;
        let chunk = call(&operation(
            &format!("chunk:{offset}"),
            WorkerOperation::ArtifactChunk {
                transfer_id: saved.upload.id.clone(),
                offset,
                sha256: hash(&bytes),
                bytes,
            },
        ))?;
        ensure!(
            serde_json::from_value::<TransferProgress>(chunk.result)?.next_offset == next,
            "plugin artifact offset changed"
        );
        offset = next;
    }
    call(&operation(
        "seal",
        WorkerOperation::ArtifactSeal {
            transfer_id: saved.upload.id.clone(),
        },
    ))?;
    Ok(())
}

fn recover(
    state: &SessionState,
    store: &SwarmStore,
    owner: &PluginOwner,
    bearer: &str,
    r: &WorkerRequest,
) -> Result<MutationAcknowledgement> {
    let mut staged = owner.staged()?;
    if staged.is_none()
        && let Some(saved) = load_capture(state, r)?
    {
        owner.restore_captured(saved.outcome)?;
        staged = owner.staged()?;
    }
    if let Some(Reply {
        outcome: Outcome::Completed { artifact_refs, .. },
        ..
    }) = &staged
        && !artifact_refs.is_empty()
    {
        let saved = load_capture(state, r)?.context("plugin artifact capture unavailable")?;
        ensure!(
            Some(&saved.outcome) == staged.as_ref().map(|reply| &reply.outcome),
            "plugin capture result changed"
        );
        publish(state, store, bearer, r, &saved)?;
    }
    owner.recover(bearer, tool_runtime::unix_ms())
}

pub(super) async fn dispatch(
    state: &SessionState,
    store: &SwarmStore,
    bearer: &str,
    r: &WorkerRequest,
    caller: &CallerContext,
) -> std::result::Result<MutationAcknowledgement, StatusCode> {
    let WorkerOperation::Plugin { invocation } = &r.operation else {
        return Err(StatusCode::BAD_REQUEST);
    };
    let owner = store
        .try_own_plugin(bearer, r, tool_runtime::unix_ms())
        .map_err(unavailable)?
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    match owner
        .prior(bearer, tool_runtime::unix_ms())
        .map_err(unavailable)?
    {
        Some(PluginClaim::Replay(ack)) => return Ok(ack),
        Some(PluginClaim::Pending) => {
            return recover(state, store, &owner, bearer, r).map_err(unavailable);
        }
        Some(PluginClaim::Dispatch(_)) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
        None => (),
    }
    let reject = |message: &str| {
        owner
            .reject_without_dispatch(bearer, message, tool_runtime::unix_ms())
            .map_err(unavailable)
    };
    if state
        .plugins
        .details(&invocation.plugin, &r.workspace_id)
        .await
        .is_err()
    {
        return reject(
            "Plugin is unavailable in this workspace or its servers could not be prepared.",
        );
    }
    match review(
        state,
        store,
        r,
        caller,
        "plugin_call",
        &serde_json::to_value(invocation).map_err(|_| StatusCode::BAD_REQUEST)?,
    )
    .await
    {
        Ok(()) => (),
        Err(StatusCode::FORBIDDEN) => {
            return reject("The leader did not approve this plugin call.");
        }
        Err(error) => return Err(error),
    }
    let mut claim = None;
    let result = state
        .plugins
        .call_tool_guarded(invocation, &r.workspace_id, || {
            match owner.claim(bearer, tool_runtime::unix_ms())? {
                PluginClaim::Dispatch(token) => {
                    claim = Some(token);
                    Ok(())
                }
                _ => anyhow::bail!("plugin operation changed during preflight"),
            }
        })
        .await;
    let Some(claim) = claim else {
        return reject(
            "Plugin call could not be dispatched. Check the configured server, tool, arguments, and unresolved prior plugin effects.",
        );
    };
    let outcome = match result {
        Ok(beale_extender::GuardedToolResult::Completed(value)) => {
            capture(state, r, bearer, &value)
                .map_err(unavailable)?
                .outcome
        }
        Ok(beale_extender::GuardedToolResult::OutcomeUnknown(_)) | Err(_) => {
            Outcome::OutcomeUnknown {}
        }
    };
    owner.stage_result(&claim, outcome).map_err(unavailable)?;
    recover(state, store, &owner, bearer, r).map_err(unavailable)
}
