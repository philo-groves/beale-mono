//! Quick Chats retain an internal session identity and may use a selected workspace as context.

use super::{SessionState, StoredTurn, detail, load_persisted, parse_id, system_prompt, title};
use axum::{
    body::Bytes,
    extract::{Path as RoutePath, State},
    http::{HeaderMap, StatusCode},
};
use beale_contracts::v1::{
    AuthMethod, ProviderId, QuickChatDetail, QuickChatMessage, QuickChatTurnRequest, SurfaceScope,
    WorkspaceDetails,
};
use beale_providers::{AuthMode, InferenceRequest, Provider, TextMessage, TextRole};
use beale_storage::{SessionRow, TurnRow};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use super::http::{authorized, checked_body, response};

pub(super) const WORKSPACE_ID: &str = "internal:quick-chats";
const MAX_PROMPT_BYTES: usize = 4_096;
const MAX_HISTORY_BYTES: usize = 131_072;
const MAX_ANSWER_BYTES: usize = 65_536;
const MAX_TURNS: usize = 32;
const MAX_WORKSPACE_CONTEXT_BYTES: usize = 24_000;
const QUICK_CHAT_CONTEXT: &str = "## Quick Chat\n\nThis is a Quick Chat without file tools. Answer directly from the conversation and any selected workspace context. Do not imply that workspace files or tools were inspected.";
const NO_WORKSPACE_CONTEXT: &str =
    "## Selected Workspace\n\nNone. This chat is general and has no research workspace context.";

pub(super) async fn details(
    State(state): State<SessionState>,
    RoutePath(id): RoutePath<String>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let id = parse_id(&id)?;
    let _guard = state.write_lock.lock().await;
    let (saved, turns) = load_persisted(&state.db, id)?;
    if saved.workspace_id != WORKSPACE_ID {
        return Err(StatusCode::NOT_FOUND);
    }
    let workspace_id = state
        .db
        .quick_chat_workspace(&saved.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let session_detail = detail(&state.db, &saved, &turns)?;
    Ok(response(QuickChatDetail {
        chat_id: saved.id,
        title: saved.title,
        workspace_id,
        messages: session_detail
            .messages
            .into_iter()
            .map(|message| QuickChatMessage {
                role: message.role,
                text: message.text,
            })
            .collect(),
    }))
}

fn workspace_context(details: WorkspaceDetails) -> Result<String, StatusCode> {
    let summary = details.summary.ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut context = format!(
        "## Selected Workspace\n\nName: {}\n\nGuidance:\n{}\n",
        summary.name, details.guidance
    );
    if !details.rules.is_empty() {
        context.push_str("\nRules:\n");
        for rule in details.rules {
            context.push_str("- ");
            context.push_str(&rule);
            context.push('\n');
        }
    }
    if !details.surfaces.is_empty() {
        context.push_str("\nSurfaces:\n");
        for surface in details.surfaces {
            let scope = match SurfaceScope::try_from(surface.scope) {
                Ok(SurfaceScope::InScope) => "In scope",
                Ok(SurfaceScope::OutOfScope) => "Out of scope",
                _ => return Err(StatusCode::INTERNAL_SERVER_ERROR),
            };
            context.push_str(&format!("- {scope}: {}", surface.value));
            if !surface.out_of_scope_reason.is_empty() {
                context.push_str(&format!(" ({})", surface.out_of_scope_reason));
            }
            context.push('\n');
        }
    }
    if context.len() > MAX_WORKSPACE_CONTEXT_BYTES {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    Ok(context)
}

fn auth_mode(subscription: bool, api_key: bool, priority: &str) -> Option<AuthMode> {
    match (subscription, api_key) {
        (true, true) if priority == "api-key" => Some(AuthMode::ApiKey),
        (true, _) => Some(AuthMode::Subscription),
        (_, true) => Some(AuthMode::ApiKey),
        _ => None,
    }
}

async fn default_model(state: &SessionState) -> Result<(Provider, AuthMode, String), StatusCode> {
    let preferred = state
        .credentials
        .load_lead_provider()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut names = vec!["openai", "xai", "anthropic", "zai", "openrouter"];
    let custom_providers = state
        .credentials
        .custom_providers()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if let Some(custom) = custom_providers
        .iter()
        .find(|custom| preferred.as_deref() == Some(custom.id.as_str()))
    {
        let model = beale_providers::qualified_model(&custom.id, &custom.lead_model)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        return Ok((Provider::Custom, AuthMode::ApiKey, model));
    }
    if let Some(preferred) = preferred.as_deref()
        && let Some(index) = names.iter().position(|name| *name == preferred)
    {
        names.swap(0, index);
    }
    for name in names {
        let subscription = if matches!(name, "anthropic" | "zai") {
            beale_providers::subscription::configured(if name == "anthropic" {
                Provider::Anthropic
            } else {
                Provider::Zai
            })
        } else if matches!(name, "openai" | "xai") {
            state
                .credentials
                .load(name)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                .is_some()
        } else {
            false
        };
        let api_key = state
            .credentials
            .api_key_configured(name)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let priority = state
            .credentials
            .source_priority(name)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let Some(mode) = auth_mode(subscription, api_key, priority) else {
            continue;
        };
        let provider = match name {
            "openai" => Provider::OpenAi,
            "xai" => Provider::Xai,
            "anthropic" => Provider::Anthropic,
            "zai" => Provider::Zai,
            "openrouter" => Provider::OpenRouter,
            _ => return Err(StatusCode::INTERNAL_SERVER_ERROR),
        };
        let catalog = state
            .provider
            .list_models(provider, mode, &state.credentials)
            .await
            .map_err(|error| {
                eprintln!("quick chat model selection failed: {error:#}");
                StatusCode::BAD_GATEWAY
            })?;
        return Ok((provider, mode, catalog.lead_model));
    }
    if let Some(custom) = custom_providers.into_iter().next() {
        let model = beale_providers::qualified_model(&custom.id, &custom.lead_model)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        return Ok((Provider::Custom, AuthMode::ApiKey, model));
    }
    Err(StatusCode::CONFLICT)
}

fn history(turns: &[StoredTurn]) -> Result<Vec<TextMessage>, StatusCode> {
    let mut messages = Vec::with_capacity(turns.len() * 2);
    let mut total = 0_usize;
    for turn in turns {
        total = total
            .saturating_add(turn.prompt.len())
            .saturating_add(turn.answer.len());
        if total > MAX_HISTORY_BYTES {
            return Err(StatusCode::CONFLICT);
        }
        messages.push(TextMessage {
            role: TextRole::User,
            text: turn.prompt.clone(),
        });
        messages.push(TextMessage {
            role: TextRole::Assistant,
            text: turn.answer.clone(),
        });
    }
    Ok(messages)
}

pub(super) async fn turn(
    State(state): State<SessionState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let request: QuickChatTurnRequest = checked_body(&headers, &body)?;
    let prompt = request.prompt.trim();
    if prompt.is_empty() || prompt.len() > MAX_PROMPT_BYTES || prompt.chars().any(|c| c == '\0') {
        return Err(StatusCode::BAD_REQUEST);
    }
    if !request.workspace_id.is_empty() && Uuid::parse_str(&request.workspace_id).is_err() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let _guard = state.write_lock.lock().await;
    let context = if request.workspace_id.is_empty() {
        NO_WORKSPACE_CONTEXT.to_owned()
    } else {
        workspace_context(state.workspaces.details_for_host(&request.workspace_id)?)?
    };
    let (id, chat_title, provider, mode, model, turns) = if request.chat_id.is_empty() {
        let (provider, mode, model) = default_model(&state).await?;
        (
            Uuid::new_v4(),
            title(prompt),
            provider,
            mode,
            model,
            Vec::new(),
        )
    } else {
        let id = parse_id(&request.chat_id)?;
        let (saved, turns) = load_persisted(&state.db, id)?;
        if saved.workspace_id != WORKSPACE_ID {
            return Err(StatusCode::NOT_FOUND);
        }
        let bound_workspace = state
            .db
            .quick_chat_workspace(&saved.id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
        if bound_workspace != request.workspace_id {
            return Err(StatusCode::CONFLICT);
        }
        let Some(latest) = turns.last() else {
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        };
        let provider = match ProviderId::try_from(latest.provider) {
            Ok(ProviderId::Openai) => Provider::OpenAi,
            Ok(ProviderId::Xai) => Provider::Xai,
            Ok(ProviderId::Anthropic) => Provider::Anthropic,
            Ok(ProviderId::Zai) => Provider::Zai,
            Ok(ProviderId::Openrouter) => Provider::OpenRouter,
            Ok(ProviderId::Custom) => Provider::Custom,
            _ => return Err(StatusCode::INTERNAL_SERVER_ERROR),
        };
        let mode = match AuthMethod::try_from(latest.method) {
            Ok(AuthMethod::Subscription) => AuthMode::Subscription,
            Ok(AuthMethod::ApiKey) => AuthMode::ApiKey,
            _ => return Err(StatusCode::INTERNAL_SERVER_ERROR),
        };
        (id, saved.title, provider, mode, latest.model.clone(), turns)
    };
    if turns.len() >= MAX_TURNS {
        return Err(StatusCode::CONFLICT);
    }
    let mut messages = history(&turns)?;
    let total = messages.iter().map(|m| m.text.len()).sum::<usize>();
    if total
        .saturating_add(prompt.len())
        .saturating_add(MAX_ANSWER_BYTES)
        > MAX_HISTORY_BYTES
    {
        return Err(StatusCode::CONFLICT);
    }
    let system = system_prompt::load(state.workspaces.root(), &[QUICK_CHAT_CONTEXT, &context])
        .map_err(|error| {
            eprintln!("quick chat system prompt failed: {error:#}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    messages.insert(
        0,
        TextMessage {
            role: TextRole::System,
            text: system,
        },
    );
    messages.push(TextMessage {
        role: TextRole::User,
        text: prompt.to_owned(),
    });
    let started = Instant::now();
    let answer = state
        .provider
        .infer(
            provider,
            mode,
            &InferenceRequest {
                model: model.clone(),
                messages,
            },
            &state.credentials,
        )
        .await
        .map_err(|error| {
            eprintln!("quick chat provider turn failed: {error:#}");
            StatusCode::BAD_GATEWAY
        })?;
    if answer.trim().is_empty() || answer.len() > MAX_ANSWER_BYTES {
        return Err(StatusCode::BAD_GATEWAY);
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let session = SessionRow {
        id: id.to_string(),
        workspace_id: WORKSPACE_ID.to_owned(),
        title: chat_title,
        created_at_ms: i64::try_from(now.as_millis())
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        last_message_at_ms: i64::try_from(now.as_millis())
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    };
    let turn = TurnRow {
        number: (turns.len() + 1) as i64,
        provider: match provider {
            Provider::OpenAi => ProviderId::Openai as i32,
            Provider::Xai => ProviderId::Xai as i32,
            Provider::Anthropic => ProviderId::Anthropic as i32,
            Provider::Zai => ProviderId::Zai as i32,
            Provider::OpenRouter => ProviderId::Openrouter as i32,
            Provider::Custom => ProviderId::Custom as i32,
        },
        method: if mode == AuthMode::Subscription {
            AuthMethod::Subscription as i32
        } else {
            AuthMethod::ApiKey as i32
        },
        model,
        prompt: prompt.to_owned(),
        answer,
        duration_ms: i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX),
    };
    if turns.is_empty() {
        state
            .db
            .append_new_quick_chat_turn(&session, &turn, &request.workspace_id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    } else {
        state
            .db
            .append_turn(&session, &turn, false)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }
    let (saved, saved_turns) = load_persisted(&state.db, id)?;
    let session_detail = detail(&state.db, &saved, &saved_turns)?;
    Ok(response(QuickChatDetail {
        chat_id: saved.id,
        title: saved.title,
        workspace_id: request.workspace_id,
        messages: session_detail
            .messages
            .into_iter()
            .map(|message| QuickChatMessage {
                role: message.role,
                text: message.text,
            })
            .collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use beale_contracts::v1::{WorkspaceSummary, WorkspaceSurface};

    #[test]
    fn selected_workspace_context_includes_guidance_and_scope() {
        let context = workspace_context(WorkspaceDetails {
            summary: Some(WorkspaceSummary {
                id: Uuid::new_v4().to_string(),
                name: "Example Workspace".to_owned(),
                directory: "example-directory".to_owned(),
            }),
            guidance: "Use synthetic examples.".to_owned(),
            surfaces: vec![WorkspaceSurface {
                kind: 1,
                scope: SurfaceScope::OutOfScope as i32,
                value: "example.test".to_owned(),
                created_by: 1,
                out_of_scope_reason: "Excluded by operator".to_owned(),
            }],
            rules: vec!["Use only authorized surfaces.".to_owned()],
        })
        .unwrap();
        assert!(context.contains("Example Workspace"));
        assert!(context.contains("Use synthetic examples."));
        assert!(context.contains("Out of scope: example.test (Excluded by operator)"));
        assert!(context.contains("Use only authorized surfaces."));
        assert!(!context.contains("example-directory"));
    }
    #[test]
    fn source_priority_uses_configured_default_method() {
        assert_eq!(
            auth_mode(true, true, "subscription"),
            Some(AuthMode::Subscription)
        );
        assert_eq!(auth_mode(true, true, "api-key"), Some(AuthMode::ApiKey));
        assert_eq!(
            auth_mode(false, true, "subscription"),
            Some(AuthMode::ApiKey)
        );
        assert_eq!(auth_mode(false, false, "subscription"), None);
    }
}
