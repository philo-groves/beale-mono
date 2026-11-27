//! Trusted host construction of an assigned turn. Guest data cannot select the
//! model, another agent's conversation, or tools outside the saved scan policy.
use super::{gateway::model_for_agent, inference};
use crate::engine;
use anyhow::{Context, Result, ensure};
use beale_contracts::{swarm_worker::*, swarms::*, v1::SubmitSessionTurnRequest};
use beale_storage::{
    HostDatabase,
    swarm_state::{LocalTurn, SwarmStore, WorkerCredential},
};
use serde_json::{Value, json};

pub fn prepare_worker(
    db: &HostDatabase,
    turn: &LocalTurn,
    shell: WorkerShell,
    registration: &WorkerRegistration,
    now_ms: i64,
) -> Result<(WorkerBootstrap, WorkerCredential)> {
    let store = SwarmStore::open(db.clone())?;
    let saved = store.load(&turn.scan_id, true)?;
    let agent = &saved.agent(&turn.agent_id)?.summary;
    let session = if agent.role == Role::Leader {
        &turn.scan_id
    } else {
        &turn.agent_id
    };
    ensure!(
        turn.session_id == *session && turn.role == agent.role && turn.profile == agent.profile.id,
        "prepared turn identity mismatch"
    );
    let model = model_for_agent(&store, &turn.scan_id, &turn.agent_id, &agent.profile.id)?;
    let settings = store.local_state(&turn.scan_id)?.settings;
    let index = settings
        .models
        .iter()
        .position(|candidate| candidate == &model)
        .context("worker model route unavailable")?;
    let route = saved
        .config
        .provider_routes
        .get(index)
        .context("worker model route unavailable")?;
    ensure!(
        route.model == model.model && route.provider_id == format!("provider-{}", model.provider),
        "saved worker route mismatch"
    );
    let validated = engine::validate_request(&SubmitSessionTurnRequest {
        provider: model.provider,
        method: model.method,
        model: model.model.clone(),
        prompt: "Prepare assigned research".into(),
        reasoning_effort: settings.reasoning_effort,
        ..Default::default()
    })
    .map_err(|_| anyhow::anyhow!("saved worker model unavailable"))?;
    let definitions = inference::tools(
        &shell.map_tools(&saved.config.enabled_tools),
        agent.role,
        shell,
    )?;
    let enabled_tools = definitions
        .iter()
        .map(|definition| {
            definition["name"]
                .as_str()
                .map(str::to_owned)
                .context("invalid saved tool definition")
        })
        .collect::<Result<Vec<_>>>()?;
    let mut input = Vec::new();
    if let Some(history) = db.session(session)? {
        ensure!(
            history.session.workspace_id == saved.config.workspace_id,
            "worker conversation belongs to another workspace"
        );
        let same_route = history.turns.last().is_some_and(|prior| {
            prior.provider == model.provider
                && prior.method == model.method
                && prior.model == model.model
        });
        let retained = if same_route {
            engine::retained_context(db, session, Some(&turn.profile))?
        } else {
            None
        };
        if let Some(retained) = retained {
            input = serde_json::from_str::<Vec<Value>>(&retained)
                .context("invalid retained worker conversation")?;
            // Runtime policy is rebuilt on the leader for every inference.
            input.retain(|item| !matches!(item["role"].as_str(), Some("system" | "developer")));
        } else {
            let model_prompts = db.session_model_prompts(session)?;
            for prior in history.turns {
                input.push(json!({"role":"user","content":model_prompts.get(&prior.number).unwrap_or(&prior.prompt)}));
                input.push(json!({"role":"assistant","content":prior.answer}));
            }
        }
    }
    input.push(json!({"role":"user","content":turn.prompt}));
    let bootstrap = WorkerBootstrap {
        identity: ExecutionIdentity {
            workspace_id: saved.config.workspace_id,
            scan_id: turn.scan_id.clone(),
            agent_id: turn.agent_id.clone(),
            attempt_id: turn.attempt_id.clone(),
            generation: turn.generation,
        },
        shell,
        route_id: route.id.clone(),
        message_format: beale_providers::message_format(validated.provider, validated.mode),
        enabled_tools,
        input,
    };
    bootstrap.validate().map_err(anyhow::Error::msg)?;
    let credential = store.prepare_worker(turn, &bootstrap, registration, now_ms)?;
    Ok((bootstrap, credential))
}
