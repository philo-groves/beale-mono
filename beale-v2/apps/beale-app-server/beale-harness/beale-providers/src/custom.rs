use crate::{CredentialStore, ModelEntry, ProviderClient, models::valid_id};
use anyhow::{Context, Result, bail, ensure};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::time::Duration;
use url::Url;

const SETTING: &str = "custom-providers";
const MAX_PROVIDERS: usize = 32;
const MAX_MODELS: usize = 128;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CustomProvider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub models: Vec<ModelEntry>,
    pub lead_model: String,
    pub small_model: String,
    pub reasoning: bool,
}

pub fn valid_custom_id(id: &str) -> bool {
    id.strip_prefix("custom-").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix.len() <= 64
            && suffix
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
            && suffix
                .bytes()
                .last()
                .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            && !suffix.contains("--")
    })
}

pub fn qualified_model(id: &str, model: &str) -> Result<String> {
    ensure!(
        valid_custom_id(id) && valid_id(model) && model.len() <= 128,
        "invalid custom model identity"
    );
    let qualified = format!("{id}::{model}");
    ensure!(qualified.len() <= 200, "custom model identity is too long");
    Ok(qualified)
}

pub fn split_qualified_model(value: &str) -> Result<(&str, &str)> {
    let (id, model) = value
        .split_once("::")
        .context("custom model needs a provider ID")?;
    ensure!(
        valid_custom_id(id) && valid_id(model) && model.len() <= 128,
        "invalid custom model identity"
    );
    Ok((id, model))
}

pub fn validate(mut provider: CustomProvider) -> Result<CustomProvider> {
    ensure!(valid_custom_id(&provider.id), "invalid custom provider ID");
    provider.name = provider.name.trim().to_owned();
    ensure!(
        !provider.name.is_empty()
            && provider.name.len() <= 100
            && !provider.name.chars().any(char::is_control),
        "invalid custom provider name"
    );
    provider.base_url = validate_base_url(&provider.base_url)?;
    ensure!(
        !provider.models.is_empty() && provider.models.len() <= MAX_MODELS,
        "invalid custom provider model count"
    );
    let mut seen = HashSet::new();
    for model in &mut provider.models {
        model.id = model.id.trim().to_owned();
        model.name = model.name.trim().to_owned();
        ensure!(
            valid_id(&model.id)
                && !model.name.is_empty()
                && model.name.len() <= 128
                && !model.name.chars().any(char::is_control),
            "invalid custom provider model"
        );
        ensure!(
            seen.insert(model.id.clone()),
            "duplicate custom provider model"
        );
        qualified_model(&provider.id, &model.id)?;
    }
    ensure!(
        seen.contains(&provider.lead_model) && seen.contains(&provider.small_model),
        "custom defaults must select listed models"
    );
    Ok(provider)
}

pub fn validate_base_url(base_url: &str) -> Result<String> {
    ensure!(base_url.len() <= 2048, "custom provider URL is too long");
    let url = Url::parse(base_url).context("invalid custom provider URL")?;
    ensure!(
        matches!(url.scheme(), "https" | "http")
            && url.host().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "invalid custom provider URL"
    );
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

pub async fn discover_models(
    client: &ProviderClient,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<Vec<ModelEntry>> {
    let base_url = validate_base_url(base_url)?;
    let mut request = client
        .http()
        .get(format!("{base_url}/models"))
        .timeout(Duration::from_secs(15));
    if let Some(key) = api_key.filter(|key| !key.is_empty()) {
        request = request.bearer_auth(key);
    }
    let response = request
        .send()
        .await
        .context("custom model discovery failed")?;
    ensure!(
        response.status().is_success(),
        "custom model discovery failed ({})",
        response.status()
    );
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("custom model discovery stream failed")?;
        ensure!(
            bytes.len().saturating_add(chunk.len()) <= 2_000_000,
            "custom model list is too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    let body: Value = serde_json::from_slice(&bytes).context("invalid custom model list")?;
    parse_discovered_models(&body)
}

fn parse_discovered_models(body: &Value) -> Result<Vec<ModelEntry>> {
    let entries = body
        .get("data")
        .and_then(Value::as_array)
        .context("custom API has no model list")?;
    let mut models = Vec::new();
    let mut seen = HashSet::new();
    for entry in entries {
        let Some(id) = entry
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| valid_id(id))
        else {
            continue;
        };
        if !seen.insert(id) {
            continue;
        }
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty() && name.len() <= 128)
            .unwrap_or(id);
        models.push(ModelEntry {
            id: id.to_owned(),
            name: name.to_owned(),
        });
        if models.len() == MAX_MODELS {
            break;
        }
    }
    ensure!(!models.is_empty(), "custom API returned no usable models");
    Ok(models)
}

impl CredentialStore {
    pub fn custom_providers(&self) -> Result<Vec<CustomProvider>> {
        let Some(value) = self.db()?.setting(SETTING)? else {
            return Ok(Vec::new());
        };
        let providers: Vec<CustomProvider> =
            serde_json::from_str(&value).context("invalid saved custom providers")?;
        ensure!(
            providers.len() <= MAX_PROVIDERS,
            "too many saved custom providers"
        );
        providers.into_iter().map(validate).collect()
    }

    pub fn custom_provider(&self, id: &str) -> Result<Option<CustomProvider>> {
        ensure!(valid_custom_id(id), "invalid custom provider ID");
        Ok(self
            .custom_providers()?
            .into_iter()
            .find(|provider| provider.id == id))
    }

    pub fn save_custom_provider(&self, provider: CustomProvider) -> Result<()> {
        let provider = validate(provider)?;
        let mut providers = self.custom_providers()?;
        if let Some(existing) = providers.iter_mut().find(|item| item.id == provider.id) {
            *existing = provider;
        } else {
            ensure!(providers.len() < MAX_PROVIDERS, "too many custom providers");
            providers.push(provider);
        }
        self.db()?
            .save_setting(SETTING, Some(&serde_json::to_string(&providers)?))
    }

    pub fn remove_custom_provider(&self, id: &str) -> Result<()> {
        ensure!(valid_custom_id(id), "invalid custom provider ID");
        let mut providers = self.custom_providers()?;
        if !providers.iter().any(|provider| provider.id == id) {
            bail!("unknown custom provider");
        }
        self.remove_api_key(id)?;
        providers.retain(|provider| provider.id != id);
        self.db()?
            .save_setting(SETTING, Some(&serde_json::to_string(&providers)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn example() -> CustomProvider {
        CustomProvider {
            id: "custom-example".to_owned(),
            name: "Example API".to_owned(),
            base_url: "http://127.0.0.1:12345/v1/".to_owned(),
            models: vec![ModelEntry {
                id: "example-model".to_owned(),
                name: "Example Model".to_owned(),
            }],
            lead_model: "example-model".to_owned(),
            small_model: "example-model".to_owned(),
            reasoning: false,
        }
    }

    #[test]
    fn validates_and_persists_custom_provider_without_key_in_registry() {
        let root = std::env::temp_dir().join(format!("beale-custom-example-{}", Uuid::new_v4()));
        let store = CredentialStore::at(root.clone());
        store.save_custom_provider(example()).unwrap();
        store
            .save_api_key("custom-example", "example-secret")
            .unwrap();
        drop(store);
        let reopened = CredentialStore::at(root.clone());
        assert_eq!(
            reopened.custom_providers().unwrap()[0].base_url,
            "http://127.0.0.1:12345/v1"
        );
        assert!(reopened.custom_providers().unwrap().iter().all(|provider| {
            !serde_json::to_string(provider)
                .unwrap()
                .contains("example-secret")
        }));
        reopened.remove_custom_provider("custom-example").unwrap();
        assert!(reopened.custom_providers().unwrap().is_empty());
        assert!(!reopened.api_key_configured("custom-example").unwrap());
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_invalid_custom_endpoints_and_model_identity() {
        let mut provider = example();
        provider.base_url = "https://user:pass@example.test/v1".to_owned();
        assert!(validate(provider).is_err());
        let mut provider = example();
        provider.models[0].id = "../bad".to_owned();
        assert!(validate(provider).is_err());
        assert!(split_qualified_model("custom-example::example-model").is_ok());
        assert!(split_qualified_model("../example::example-model").is_err());
    }

    #[test]
    fn discovers_models_from_openai_compatible_catalog_shape() {
        let models = parse_discovered_models(&serde_json::json!({"data":[
            {"id":"example-model","name":"Example Model"},
            {"id":"example-model"},
            {"id":"../invalid"}
        ]}))
        .unwrap();
        assert_eq!(
            models,
            vec![ModelEntry {
                id: "example-model".to_owned(),
                name: "Example Model".to_owned()
            }]
        );
        assert!(parse_discovered_models(&serde_json::json!({"items":[]})).is_err());
    }
}
