use crate::{AuthMode, CredentialStore, Provider, ProviderClient};
use anyhow::{Context, Result, bail, ensure};
use futures_util::StreamExt;
use serde_json::Value;
use std::time::Duration;

const MAX_CATALOG_BYTES: usize = 8_000_000;
const MAX_MODELS: usize = 1024;

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
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
        if mode == AuthMode::Subscription && matches!(provider, Provider::Anthropic | Provider::Zai)
        {
            return crate::subscription::catalog(provider, store);
        }
        let token = self.token(provider, mode, store).await?;
        let endpoint = match (provider, mode) {
            (Provider::OpenAi, _) => "https://api.openai.com/v1/models",
            (Provider::Xai, AuthMode::ApiKey) => "https://api.x.ai/v1/language-models",
            (Provider::Xai, AuthMode::Subscription) => "https://cli-chat-proxy.grok.com/v1/models",
            (Provider::Anthropic, _) => "https://api.anthropic.com/v1/models?limit=256",
            (Provider::Zai, _) => "https://api.z.ai/api/paas/v4/models",
            (Provider::OpenRouter, _) => "https://openrouter.ai/api/v1/models",
            (Provider::Custom, _) => bail!("custom provider models come from its configuration"),
        };
        let mut request = self.http().get(endpoint).timeout(Duration::from_secs(15));
        request = request.bearer_auth(token);
        if provider == Provider::Xai && mode == AuthMode::Subscription {
            request = request.header("X-XAI-Token-Auth", "xai-grok-cli");
        }
        if provider == Provider::Anthropic {
            request = request.header("anthropic-version", "2026-10-21");
            if let Ok(workspace) = std::env::var("ANTHROPIC_WORKSPACE_ID") {
                request = request.header("anthropic-workspace-id", workspace);
            }
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
        let models = filter_for_daybreak(
            parse_models(provider, mode, &value)?,
            provider,
            if provider == Provider::OpenAi {
                Some(store.daybreak_mode()?)
            } else {
                None
            },
        );
        ensure!(
            !models.is_empty(),
            "no models available for the selected Daybreak mode"
        );
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

fn filter_for_daybreak(
    models: Vec<ModelEntry>,
    provider: Provider,
    daybreak_mode: Option<&str>,
) -> Vec<ModelEntry> {
    if provider == Provider::OpenAi && daybreak_mode == Some("blue") {
        models
            .into_iter()
            .filter(|model| !crate::wire::openai_requires_daybreak_red(&model.id))
            .collect()
    } else {
        models
    }
}

pub fn provider_name(provider: Provider) -> &'static str {
    match provider {
        Provider::OpenAi => "openai",
        Provider::Xai => "xai",
        Provider::Anthropic => "anthropic",
        Provider::Zai => "zai",
        Provider::OpenRouter => "openrouter",
        Provider::Custom => "custom",
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
        if provider == Provider::OpenRouter {
            let supports_tools = item
                .get("supported_parameters")
                .and_then(Value::as_array)
                .is_some_and(|params| params.iter().any(|param| param.as_str() == Some("tools")));
            if !supports_tools {
                continue;
            }
            if item
                .pointer("/architecture/output_modalities")
                .and_then(Value::as_array)
                .is_some_and(|outputs| !outputs.iter().any(|value| value.as_str() == Some("text")))
            {
                continue;
            }
        }
        let name = item
            .get("display_name")
            .or_else(|| item.get("name"))
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty() && name.len() <= 128)
            .unwrap_or(id);
        if provider == Provider::OpenAi {
            push_model(&mut models, id, &name.replace('-', " "));
        } else {
            push_model(&mut models, id, name);
        }
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
    if provider == Provider::OpenRouter && !models.iter().any(|model| model.id == "openrouter/auto")
    {
        models.insert(
            0,
            ModelEntry {
                id: "openrouter/auto".to_owned(),
                name: "Auto Router".to_owned(),
            },
        );
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
        && id.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
        })
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
        (Provider::OpenAi, false) => &[
            "gpt-5.6-sol",
            "gpt-6-sol",
            "gpt-6.1-sol",
            "gpt-6-astra",
            "gpt-6.1-terra",
        ],
        (Provider::OpenAi, true) => &["gpt-6-luna", "gpt-6.1-terra", "gpt-5.4-mini"],
        (Provider::Xai, false) => &["grok-4.7", "grok-4.6", "grok-build"],
        (Provider::Xai, true) => &[
            "grok-4.7-build-fast",
            "grok-4.6-build-fast",
            "grok-4.7",
            "grok-4.6",
            "grok-build",
        ],
        (Provider::Anthropic, false) => &["claude-opus-5-5", "claude-sonnet-5-5", "claude-fable-5"],
        (Provider::Anthropic, true) => &["claude-haiku-5-5", "claude-haiku-4-5"],
        (Provider::Zai, false) => &["glm-5.3", "glm-5-turbo", "glm-5.2"],
        (Provider::Zai, true) => &["glm-5-turbo", "glm-4.7-flash", "glm-4.5-flash"],
        (Provider::OpenRouter, false) => &["openrouter/auto", "auto"],
        (Provider::OpenRouter, true) => &["openrouter/auto", "auto"],
        (Provider::Custom, _) => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn openai_subscription_uses_visible_slugs_and_available_defaults() {
        let value = json!({"models": [
            {"slug":"gpt-5.6-sol","display_name":"GPT-5.6-Sol","visibility":"list"},
            {"slug":"gpt-6.1-sol","display_name":"GPT-6.1-Sol","visibility":"list"},
            {"slug":"gpt-6-luna","display_name":"Luna","visibility":"list"},
            {"slug":"hidden","visibility":"hidden"}
        ]});
        let models = parse_models(Provider::OpenAi, AuthMode::Subscription, &value).unwrap();
        assert_eq!(models.len(), 3);
        assert_eq!(models[0].name, "GPT 5.6 Sol");
        assert_eq!(models[0].id, "gpt-5.6-sol");
        assert_eq!(recommended(Provider::OpenAi, false, &models), "gpt-5.6-sol");
        assert_eq!(recommended(Provider::OpenAi, true, &models), "gpt-6-luna");
        let blue = filter_for_daybreak(models.clone(), Provider::OpenAi, Some("blue"));
        assert_eq!(blue.len(), 2);
        assert!(blue.iter().all(|model| model.id != "gpt-6.1-sol"));
        assert_eq!(
            filter_for_daybreak(models.clone(), Provider::OpenAi, Some("red")),
            models
        );
    }

    #[test]
    fn api_lists_drop_non_text_models_and_keep_xai_aliases() {
        let openai = json!({"data": [{"id":"gpt-6.1-sol"},{"id":"gpt-5.6-sol"},{"id":"gpt-image-2"},{"id":"text-embedding-3-small"}]});
        let openai_models = parse_models(Provider::OpenAi, AuthMode::ApiKey, &openai).unwrap();
        assert_eq!(
            recommended(Provider::OpenAi, false, &openai_models),
            "gpt-5.6-sol"
        );
        assert_eq!(
            openai_models
                .iter()
                .map(|model| (model.id.as_str(), model.name.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("gpt-6.1-sol", "gpt 6.1 sol"),
                ("gpt-5.6-sol", "gpt 5.6 sol")
            ]
        );
        let xai = json!({"models": [{"id":"grok-4.7-2026-09","aliases":["grok-4.7"],"output_modalities":["text"]},{"id":"grok-imagine","output_modalities":["image"]}]});
        let models = parse_models(Provider::Xai, AuthMode::ApiKey, &xai).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].name, "grok-4.7-2026-09");
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
        let mut items: Vec<_> = (0..1100)
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

    #[test]
    fn new_api_catalogs_preserve_provider_model_ids() {
        let anthropic = parse_models(
            Provider::Anthropic,
            AuthMode::ApiKey,
            &json!({"data":[{"id":"claude-example","display_name":"Claude Example"}]}),
        )
        .unwrap();
        assert_eq!(anthropic[0].name, "Claude Example");
        let zai = parse_models(
            Provider::Zai,
            AuthMode::ApiKey,
            &json!({"data":[{"id":"glm-example","name":"GLM Example"}]}),
        )
        .unwrap();
        assert_eq!(zai[0].id, "glm-example");
        let router = parse_models(Provider::OpenRouter, AuthMode::ApiKey,
            &json!({"data":[
                {"id":"example-org/tool-model","name":"Tool Model","supported_parameters":["tools"],"architecture":{"output_modalities":["text"]}},
                {"id":"example-org/image-model","supported_parameters":["tools"],"architecture":{"output_modalities":["image"]}},
                {"id":"example-org/no-tools","supported_parameters":[]}
            ]})).unwrap();
        assert!(router.iter().any(|model| model.id == "openrouter/auto"));
        assert!(
            router
                .iter()
                .any(|model| model.id == "example-org/tool-model")
        );
        assert!(!router.iter().any(
            |model| model.id == "example-org/image-model" || model.id == "example-org/no-tools"
        ));
        assert!(valid_id("example-org/tool-model"));
    }
}
