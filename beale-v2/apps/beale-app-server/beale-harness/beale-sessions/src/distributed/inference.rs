//! Inference always uses leader-owned provider settings, credentials and tool
//! policy. Worker conversation items carry data, never provider configuration.
use super::gateway::{selected_model, unavailable};
use crate::{SessionState, engine, features, hierarchy, system_prompt, tool_runtime};
use anyhow::{Result, ensure};
use axum::http::StatusCode;
use beale_contracts::{
    swarm_worker::{InferenceCall, InferenceResult, InferenceUsage, WorkerReply},
    swarms::{MutationAcknowledgement, WorkerOperation, WorkerRequest},
    v1::SubmitSessionTurnRequest,
};
use beale_providers::{Provider, ToolStepRequest};
use beale_storage::swarm_state::{CallerContext, InferenceClaim, SwarmStore};
use serde_json::{Value, json};

pub(super) fn tools(
    enabled: &[String],
    role: beale_contracts::swarms::Role,
    shell: beale_contracts::swarm_worker::WorkerShell,
) -> Result<Vec<Value>> {
    let mut available = beale_tools::worker_definitions(shell.tool())?;
    for feature in features::native_features() {
        // Goal Mode owns a separate session continuation loop. Swarm scheduling
        // does not compose that loop, including for a remotely assigned leader.
        // Reject a saved goal tool below instead of exposing an unhandled tool.
        if feature.id == beale_goals::FEATURE_ID {
            continue;
        }
        if let Some(definitions) = feature.definitions {
            available.extend(definitions());
        }
    }
    available.push(crate::plugin_call_definition());
    available.extend(beale_swarms::local::definitions(role));
    let hierarchy_names = beale_swarms::FEATURE.tool_names;
    let mut selected = Vec::new();
    for name in enabled {
        if let Some(definition) = available.iter().find(|d| d["name"].as_str() == Some(name)) {
            selected.push(definition.clone());
        } else {
            // Role-specific omissions are intentional; missing compiled feature
            // implementations are not silently replaced with different tools.
            ensure!(
                hierarchy_names.contains(&name.as_str()),
                "saved tool implementation unavailable"
            );
        }
    }
    Ok(selected)
}

fn instructions(
    state: &SessionState,
    store: &SwarmStore,
    request: &WorkerRequest,
    caller: &CallerContext,
) -> Result<String> {
    let saved = store.load(&request.scan_id, true)?;
    let identity = hierarchy::instructions(&saved, &request.agent_id)?;
    let mut sections = vec![
        identity,
        format!(
            "Recorded authorization boundary:\n{}",
            caller.authorized_scope
        ),
    ];
    // Remote instructions contain guidance itself, never host AgentFiles paths.
    for feature in features::native_features() {
        if feature
            .tool_names
            .iter()
            .any(|name| caller.enabled_tools.iter().any(|enabled| enabled == name))
            && !feature.prompt_extension.is_empty()
        {
            sections.push(feature.prompt_extension.to_owned());
        }
    }
    let bootstrap = store.worker_bootstrap(&request.attempt_id)?;
    let plugins = store.worker_plugin_guidance(&bootstrap.identity)?;
    if !plugins.is_empty() {
        sections.push(format!("Installed plugin resources in your assigned workspace:\n{plugins}Read the plugin document before using its skills or tools. MCP calls execute through the leader; scripts use your assigned environment."));
    }
    {
        // Managed remote destinations currently execute Linux containers. Native
        // copies use the local execution host, including macOS Bash workers.
        let os = if bootstrap.shell == beale_contracts::swarm_worker::WorkerShell::Powershell {
            "windows"
        } else if !saved.config.destinations.is_empty() || cfg!(windows) {
            "linux"
        } else {
            std::env::consts::OS
        };
        let preferences = beale_fleet::allowed_tools(&state.db)
            .map_err(|_| anyhow::anyhow!("research-tool preferences unavailable"))?;
        if let Some(note) = beale_fleet::prompt_note_for_os(&preferences, os) {
            sections.push(note);
        }
    }
    let refs: Vec<_> = sections.iter().map(String::as_str).collect();
    system_prompt::load_for_worker(state.workspaces.root(), &refs, bootstrap.shell)
}

pub(super) async fn dispatch(
    state: &SessionState,
    store: &SwarmStore,
    bearer: &str,
    request: &WorkerRequest,
    caller: &CallerContext,
) -> std::result::Result<MutationAcknowledgement, StatusCode> {
    let WorkerOperation::Infer { route_id, messages } = &request.operation else {
        return Err(StatusCode::BAD_REQUEST);
    };
    let model = selected_model(store, request, caller).map_err(unavailable)?;
    let settings = store
        .local_state(&request.scan_id)
        .map_err(unavailable)?
        .settings;
    let routes = &caller.provider_routes;
    let expected = settings
        .models
        .iter()
        .position(|m| m == &model)
        .and_then(|index| routes.get(index));
    if !expected.is_some_and(|route| {
        route.id == *route_id
            && route.model == model.model
            && route.provider_id == format!("provider-{}", model.provider)
    }) {
        return Err(StatusCode::FORBIDDEN);
    }
    let validated = engine::validate_request(&SubmitSessionTurnRequest {
        provider: model.provider,
        method: model.method,
        model: model.model.clone(),
        reasoning_effort: settings.reasoning_effort,
        prompt: "Execute assigned research".into(),
        ..Default::default()
    })
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let bootstrap = store
        .worker_bootstrap(&request.attempt_id)
        .map_err(unavailable)?;
    let definitions =
        tools(&caller.enabled_tools, caller.agent.role, bootstrap.shell).map_err(unavailable)?;
    let system = instructions(state, store, request, caller).map_err(unavailable)?;
    let input = messages.as_array().ok_or(StatusCode::BAD_REQUEST)?;
    // System/developer instructions are supplied only by the leader. Other
    // provider continuation items are opaque data, bounded by WorkerRequest.
    if input.iter().any(|item| {
        !item.is_object() || matches!(item["role"].as_str(), Some("system" | "developer"))
    }) {
        return Err(StatusCode::BAD_REQUEST);
    }
    // Worker tool results reach this boundary as conversation items. Apply the
    // same host-secret redaction the leader applies to its own tool results
    // before any of them is sent to a provider. The idempotency claim below
    // still binds the original request content.
    let redactor = tool_runtime::host_redactor(state);
    let mut input: Vec<Value> = input
        .iter()
        .cloned()
        .map(|item| redactor.redact_json(item))
        .collect();
    if validated.provider != Provider::OpenAi {
        input.insert(0, json!({"role":"system","content":system}));
    }
    let owner = store
        .try_own_inference(bearer, request, tool_runtime::unix_ms())
        .map_err(unavailable)?
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let claim = match owner
        .claim(bearer, tool_runtime::unix_ms())
        .map_err(unavailable)?
    {
        InferenceClaim::Replay(saved) => return Ok(saved),
        InferenceClaim::Pending => {
            return owner
                .recover(bearer, tool_runtime::unix_ms())
                .map_err(unavailable);
        }
        InferenceClaim::Dispatch(claim) => claim,
    };
    let call = beale_providers::pacing::scope(
        state.db.clone(),
        &request.scan_id,
        beale_contracts::pacing::CallClass::Research,
        state.provider.tool_step(
            validated.provider,
            validated.mode,
            ToolStepRequest {
                model: &model.model,
                input: &input,
                tools: &definitions,
                instructions: (validated.provider == Provider::OpenAi).then_some(system.as_str()),
                reasoning_effort: validated.reasoning_effort,
                workspace_root: None,
            },
            &state.credentials,
            None,
        ),
    );
    let result = tokio::select! {
        result=call=>result,
        _=async {loop {tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            if store.authorize(bearer,beale_contracts::swarms::WORKER_AUDIENCE,request,tool_runtime::unix_ms()).is_err(){break}
        }}=>return Err(StatusCode::UNAUTHORIZED),
    };
    let output = match result {
        Ok(step) => InferenceResult::Completed {
            text: step.text,
            calls: step
                .calls
                .into_iter()
                .map(|call| InferenceCall {
                    id: call.id,
                    name: call.name,
                    arguments: call.arguments,
                })
                .collect(),
            reasoning_summaries: step.reasoning_summaries,
            continuation: step.continuation,
            response_id: step.response_id,
            usage: step.usage.map(|usage| InferenceUsage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_read_tokens: usage.cache_read_tokens,
            }),
        },
        Err(error) => InferenceResult::Unavailable {
            retryable: beale_providers::provider_error_retryable(&error),
        },
    };
    // Redaction covers all strings in continuation items and returned tool calls,
    // not just the displayed answer. Opaque non-secret content is retained.
    let output = if output.validate().is_ok() {
        output
    } else {
        InferenceResult::Unavailable { retryable: false }
    };
    let output = serde_json::to_value(output).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let output = redactor.redact_json(output);
    let output = match serde_json::from_value::<InferenceResult>(output.clone()) {
        Ok(parsed) if parsed.validate().is_ok() => output,
        _ => json!({"status":"unavailable","retryable":false}),
    };
    let acknowledgement = owner
        .complete(bearer, &claim, output, tool_runtime::unix_ms())
        .map_err(unavailable)?;
    WorkerReply::Acknowledged {
        acknowledgement: acknowledgement.clone(),
    }
    .validate_for(request)
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(acknowledgement)
}

#[cfg(test)]
mod tests {
    use super::*;
    use beale_contracts::{swarm_worker::WorkerShell, swarms::Role};

    #[test]
    fn assigned_roles_reject_goal_continuation_tools_without_losing_hierarchy_tools() {
        for role in [Role::Leader, Role::Deputy, Role::Worker] {
            for shell in [WorkerShell::Bash, WorkerShell::Powershell] {
                let mut saved = vec!["read".into(), shell.tool().into()];
                saved.extend(beale_swarms::FEATURE.tool_names.iter().map(|s| (*s).into()));
                let definitions = tools(&saved, role, shell).unwrap();
                assert!(definitions.iter().any(|d| d["name"] == shell.tool()));
                for expected in beale_swarms::local::definitions(role) {
                    assert!(definitions.contains(&expected));
                }
                saved.push(beale_goals::TOOL_NAME.into());
                assert_eq!(
                    tools(&saved, role, shell).unwrap_err().to_string(),
                    "saved tool implementation unavailable"
                );
            }
        }
    }
}
