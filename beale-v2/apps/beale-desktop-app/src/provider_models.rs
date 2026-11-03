use beale_contracts::{
    MODELS_DEFAULTS_PATH, MODELS_LIST_PATH, PROTOBUF_CONTENT_TYPE,
    v1::{
        AuthMethod, ListModelsRequest, ModelCatalogResponse, ProviderId, SetModelDefaultsRequest,
    },
};
use prost::Message;
use reqwest::{Client, StatusCode, header::CONTENT_TYPE};
use std::{net::SocketAddr, time::Duration};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModelEntry {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModelCatalog {
    pub models: Vec<ModelEntry>,
    pub lead: String,
    pub small: String,
}

pub(crate) fn client() -> Result<Client, String> {
    Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(20))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not initialize model connection".to_owned())
}

fn url(addr: SocketAddr, path: &str) -> String {
    format!("http://{addr}{path}")
}

pub(crate) async fn list(
    client: &Client,
    addr: SocketAddr,
    provider: ProviderId,
    method: AuthMethod,
) -> Result<ModelCatalog, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let response = client
        .post(url(addr, MODELS_LIST_PATH))
        .bearer_auth(token)
        .header(CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
        .body(
            ListModelsRequest {
                provider: provider as i32,
                method: method as i32,
            }
            .encode_to_vec(),
        )
        .send()
        .await
        .map_err(|_| "Could not fetch provider models".to_owned())?;
    if response.status() != StatusCode::OK
        || response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            != Some(PROTOBUF_CONTENT_TYPE)
    {
        return Err(format!(
            "Provider model list is unavailable ({})",
            response.status()
        ));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "Could not read provider models".to_owned())?;
    if bytes.len() > 131_072 {
        return Err("Provider model list is too large".to_owned());
    }
    decode_catalog(provider, method, &bytes)
}

fn decode_catalog(
    provider: ProviderId,
    method: AuthMethod,
    bytes: &[u8],
) -> Result<ModelCatalog, String> {
    let reply = ModelCatalogResponse::decode(bytes)
        .map_err(|_| "Invalid provider model list".to_owned())?;
    if reply.provider != provider as i32
        || reply.method != method as i32
        || reply.models.is_empty()
        || reply.models.len() > 256
    {
        return Err("Invalid provider model list".to_owned());
    }
    let models: Vec<_> = reply
        .models
        .into_iter()
        .map(|model| ModelEntry {
            id: model.id,
            name: if provider == ProviderId::Openai {
                model.display_name.replace('-', " ")
            } else {
                model.display_name
            },
        })
        .collect();
    if models
        .iter()
        .any(|model| !valid_id(&model.id) || model.name.is_empty() || model.name.len() > 128)
        || !models.iter().any(|model| model.id == reply.lead_model)
        || !models.iter().any(|model| model.id == reply.small_model)
    {
        return Err("Invalid provider model list".to_owned());
    }
    Ok(ModelCatalog {
        models,
        lead: reply.lead_model,
        small: reply.small_model,
    })
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.contains("..")
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

pub(crate) async fn save(
    client: &Client,
    addr: SocketAddr,
    provider: ProviderId,
    method: AuthMethod,
    lead: String,
    small: String,
) -> Result<(), String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let response = client
        .post(url(addr, MODELS_DEFAULTS_PATH))
        .bearer_auth(token)
        .header(CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
        .body(
            SetModelDefaultsRequest {
                provider: provider as i32,
                method: method as i32,
                lead_model: lead,
                small_model: small,
            }
            .encode_to_vec(),
        )
        .send()
        .await
        .map_err(|_| "Could not save model defaults".to_owned())?;
    if response.status() != StatusCode::NO_CONTENT {
        return Err(format!(
            "Could not save model defaults ({})",
            response.status()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use beale_contracts::v1::ModelEntry as WireEntry;

    #[test]
    fn catalog_must_match_request_and_selected_models() {
        let reply = ModelCatalogResponse {
            provider: ProviderId::Openai as i32,
            method: AuthMethod::Subscription as i32,
            models: vec![WireEntry {
                id: "example-model".to_owned(),
                display_name: "Example Model".to_owned(),
            }],
            lead_model: "example-model".to_owned(),
            small_model: "example-model".to_owned(),
        }
        .encode_to_vec();
        assert!(decode_catalog(ProviderId::Openai, AuthMethod::Subscription, &reply).is_ok());
        assert!(decode_catalog(ProviderId::Xai, AuthMethod::Subscription, &reply).is_err());
    }

    #[test]
    fn openai_catalog_labels_are_normalized_even_if_host_sends_dashes() {
        let reply = |provider| {
            ModelCatalogResponse {
                provider: provider as i32,
                method: AuthMethod::ApiKey as i32,
                models: vec![WireEntry {
                    id: "example-model-id".to_owned(),
                    display_name: "Example-Model-Name".to_owned(),
                }],
                lead_model: "example-model-id".to_owned(),
                small_model: "example-model-id".to_owned(),
            }
            .encode_to_vec()
        };
        let openai = decode_catalog(
            ProviderId::Openai,
            AuthMethod::ApiKey,
            &reply(ProviderId::Openai),
        )
        .unwrap();
        assert_eq!(openai.models[0].id, "example-model-id");
        assert_eq!(openai.models[0].name, "Example Model Name");
        let xai =
            decode_catalog(ProviderId::Xai, AuthMethod::ApiKey, &reply(ProviderId::Xai)).unwrap();
        assert_eq!(xai.models[0].name, "Example-Model-Name");
    }
}
