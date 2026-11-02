use crate::{AuthMode, CredentialStore, Provider, ProviderClient};
use anyhow::{Context, Result, bail, ensure};
use futures_util::StreamExt;
use serde_json::Value;
use std::time::Duration;

const MAX_CATALOG_BYTES: usize = 2_000_000;
const MAX_MODELS: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelEntry {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelCatalog {
    pub models: Vec<ModelEntry>,
    pub lead_model: String,
    pub small_model: String,
}

impl ProviderClient {
    pub async fn list_models(
        &self,
        provider: Provider,
        mode: AuthMode,
        store: &CredentialStore,
    ) -> Result<ModelCatalog> {
        let token = self.token(provider, mode, store).await?;
        let endpoint = match (provider, mode) {
            (Provider::OpenAi, _) => "https://api.openai.com/v1/models",
            (Provider::Xai, AuthMode::ApiKey) => "https://api.x.ai/v1/language-models",
            (Provider::Xai, AuthMode::Subscription) => "https://cli-chat-proxy.grok.com/v1/models",
        };
        let mut request = self
            .http()
            .get(endpoint)
            .bearer_auth(token)
            .timeout(Duration::from_secs(15));
        if provider == Provider::Xai && mode == AuthMode::Subscription {
            request = request.header("X-XAI-Token-Auth", "xai-grok-cli");
        }
        let response = request
            .send()
            .await
            .context("provider model request failed")?;
        ensure!(
            response.status().is_success(),
            "provider model request failed ({})",
            response.status()
        );
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("provider model stream failed")?;
            ensure!(
                bytes.len().saturating_add(chunk.len()) <= MAX_CATALOG_BYTES,
                "provider model list is too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes).context("invalid provider model list")?;
        let models = parse_models(provider, mode, &value)?;
        let saved = store.load_model_defaults(provider_name(provider), mode_name(mode))?;
        let lead_model = saved
            .as_ref()
            .map(|value| value.lead.as_str())
            .filter(|id| models.iter().any(|model| model.id == *id))
            .map(str::to_owned)
            .unwrap_or_else(|| recommended(provider, false, &models));
        let small_model = saved
            .as_ref()
            .map(|value| value.small.as_str())
            .filter(|id| models.iter().any(|model| model.id == *id))
            .map(str::to_owned)
            .unwrap_or_else(|| recommended(provider, true, &models));
        Ok(ModelCatalog {
            models,
            lead_model,
            small_model,
        })
    }
}

pub fn provider_name(provider: Provider) -> &'static str {
    match provider {
        Provider::OpenAi => "openai",
        Provider::Xai => "xai",
    }
}

pub fn mode_name(mode: AuthMode) -> &'static str {
    match mode {
        AuthMode::ApiKey => "api-key",
        AuthMode::Subscription => "subscription",
    }
}

fn parse_models(provider: Provider, mode: AuthMode, value: &Value) -> Result<Vec<ModelEntry>> {
    let items = match (provider, mode) {
        (Provider::OpenAi, AuthMode::Subscription) => value.get("models").and_then(Value::as_array),
        (Provider::Xai, _) => value
            .get("models")
            .or_else(|| value.get("data"))
            .and_then(Value::as_array),
        _ => value.get("data").and_then(Value::as_array),
    }
    .context("provider model list has no models")?;
    let mut models = Vec::new();
    for item in items {
        if provider == Provider::OpenAi
            && mode == AuthMode::Subscription
            && item.get("visibility").and_then(Value::as_str) != Some("list")
        {
            continue;
        }
        let id = if provider == Provider::OpenAi && mode == AuthMode::Subscription {
            item.get("slug")
        } else {
            item.get("id")
        }
        .and_then(Value::as_str)
        .unwrap_or("");
        if !valid_id(id) {
            continue;
        }
        if provider == Provider::OpenAi && mode == AuthMode::ApiKey && !openai_text_model(id) {
            continue;
        }
        if provider == Provider::Xai
            && mode == AuthMode::ApiKey
            && let Some(outputs) = item.get("output_modalities").and_then(Value::as_array)
            && !outputs.iter().any(|output| output.as_str() == Some("text"))
        {
            continue;
        }
        let name = item
            .get("display_name")
            .or_else(|| item.get("name"))
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty() && name.len() <= 128)
            .unwrap_or(id);
        push_model(&mut models, id, name);
        if provider == Provider::Xai
            && mode == AuthMode::ApiKey
            && let Some(aliases) = item.get("aliases").and_then(Value::as_array)
        {
            for alias in aliases.iter().filter_map(Value::as_str) {
                if valid_id(alias) {
                    push_model(&mut models, alias, alias);
                }
            }
        }
    }
    if models.is_empty() {
        bail!("provider returned no usable text models");
    }
    if models.len() > MAX_MODELS {
        let preferred = preferred_models(provider, false)
            .iter()
            .chain(preferred_models(provider, true));
        let mut selected = Vec::with_capacity(MAX_MODELS);
        for id in preferred {
            if let Some(model) = models.iter().find(|model| model.id == *id) {
                push_model(&mut selected, &model.id, &model.name);
            }
        }
        for model in models {
            if selected.len() == MAX_MODELS {
                break;
            }
            push_model(&mut selected, &model.id, &model.name);
        }
        return Ok(selected);
    }
    Ok(models)
}

fn push_model(models: &mut Vec<ModelEntry>, id: &str, name: &str) {
    if !models.iter().any(|model| model.id == id) {
        models.push(ModelEntry {
            id: id.to_owned(),
            name: name.to_owned(),
        });
    }
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.contains("..")
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn openai_text_model(id: &str) -> bool {
    let candidate = id.starts_with("gpt-")
        || id.starts_with("o1")
        || id.starts_with("o3")
        || id.starts_with("o4")
        || id.starts_with("ft:gpt-");
    candidate
        && !["image", "audio", "realtime", "transcribe", "tts", "search"]
            .iter()
            .any(|part| id.contains(part))
}

fn recommended(provider: Provider, small: bool, models: &[ModelEntry]) -> String {
    preferred_models(provider, small)
        .iter()
        .find_map(|id| {
            models
                .iter()
                .find(|model| model.id == *id)
                .map(|model| model.id.clone())
        })
        .unwrap_or_else(|| models[0].id.clone())
}

fn preferred_models(provider: Provider, small: bool) -> &'static [&'static str] {
    match (provider, small) {
        (Provider::OpenAi, false) => &["gpt-6.1-sol", "gpt-6-astra", "gpt-6-sol", "gpt-6.1-terra"],
        (Provider::OpenAi, true) => &["gpt-6-luna", "gpt-6.1-terra", "gpt-5.4-mini"],
        (Provider::Xai, false) => &["grok-4.7", "grok-4.6", "grok-build"],
        (Provider::Xai, true) => &[
            "grok-4.7-build-fast",
            "grok-4.6-build-fast",
            "grok-4.7",
            "grok-4.6",
            "grok-build",
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn openai_subscription_uses_visible_slugs_and_available_defaults() {
        let value = json!({"models": [
            {"slug":"gpt-6.1-sol","display_name":"Sol","visibility":"list"},
            {"slug":"gpt-6-luna","display_name":"Luna","visibility":"list"},
            {"slug":"hidden","visibility":"hidden"}
        ]});
        let models = parse_models(Provider::OpenAi, AuthMode::Subscription, &value).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(recommended(Provider::OpenAi, false, &models), "gpt-6.1-sol");
        assert_eq!(recommended(Provider::OpenAi, true, &models), "gpt-6-luna");
    }

    #[test]
    fn api_lists_drop_non_text_models_and_keep_xai_aliases() {
        let openai = json!({"data": [{"id":"gpt-6.1-sol"},{"id":"gpt-image-2"},{"id":"text-embedding-3-small"}]});
        assert_eq!(
            parse_models(Provider::OpenAi, AuthMode::ApiKey, &openai)
                .unwrap()
                .len(),
            1
        );
        let xai = json!({"models": [{"id":"grok-4.7-2026-09","aliases":["grok-4.7"],"output_modalities":["text"]},{"id":"grok-imagine","output_modalities":["image"]}]});
        let models = parse_models(Provider::Xai, AuthMode::ApiKey, &xai).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(recommended(Provider::Xai, false, &models), "grok-4.7");
        assert_eq!(recommended(Provider::Xai, true, &models), "grok-4.7");
    }

    #[test]
    fn unknown_catalog_shape_fails_closed() {
        assert!(
            parse_models(Provider::Xai, AuthMode::Subscription, &json!({"items": []})).is_err()
        );
        assert!(!valid_id("../private"));
    }

    #[test]
    fn large_catalog_keeps_available_recommendations() {
        let mut items: Vec<_> = (0..300)
            .map(|index| json!({"id": format!("gpt-example-{index}")}))
            .collect();
        items.push(json!({"id": "gpt-6.1-sol"}));
        items.push(json!({"id": "gpt-6-luna"}));
        let models =
            parse_models(Provider::OpenAi, AuthMode::ApiKey, &json!({"data": items})).unwrap();
        assert_eq!(models.len(), MAX_MODELS);
        assert_eq!(recommended(Provider::OpenAi, false, &models), "gpt-6.1-sol");
        assert_eq!(recommended(Provider::OpenAi, true, &models), "gpt-6-luna");
    }
}
