use crate::{credentials::CredentialStore, oauth};
use anyhow::{Context, Result, bail, ensure};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_EVENT_BYTES: usize = 1_048_576;
const MAX_OUTPUT_BYTES: usize = 2_097_152;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Provider {
    OpenAi,
    Xai,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthMode {
    ApiKey,
    Subscription,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextMessage {
    pub role: TextRole,
    pub text: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextRole {
    System,
    User,
    Assistant,
}

#[derive(Clone, Debug)]
pub struct InferenceRequest {
    pub model: String,
    pub messages: Vec<TextMessage>,
}

#[derive(Clone, Copy)]
enum WireShape {
    Responses,
    Chat,
}

struct Route {
    endpoint: &'static str,
    shape: WireShape,
    oauth_name: &'static str,
    api_key_env: &'static str,
    proxy_header: bool,
}

fn route(provider: Provider, mode: AuthMode) -> Route {
    match (provider, mode) {
        (Provider::OpenAi, _) => Route {
            endpoint: "https://api.openai.com/v1/responses",
            shape: WireShape::Responses,
            oauth_name: "openai",
            api_key_env: "OPENAI_API_KEY",
            proxy_header: false,
        },
        (Provider::Xai, AuthMode::ApiKey) => Route {
            endpoint: "https://api.x.ai/v1/responses",
            shape: WireShape::Responses,
            oauth_name: "xai",
            api_key_env: "XAI_API_KEY",
            proxy_header: false,
        },
        (Provider::Xai, AuthMode::Subscription) => Route {
            endpoint: "https://cli-chat-proxy.grok.com/v1/chat/completions",
            shape: WireShape::Chat,
            oauth_name: "xai",
            api_key_env: "XAI_API_KEY",
            proxy_header: true,
        },
    }
}

pub struct ProviderClient {
    http: reqwest::Client,
}

impl ProviderClient {
    pub fn new() -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()?,
        })
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub async fn infer(
        &self,
        provider: Provider,
        mode: AuthMode,
        request: &InferenceRequest,
        store: &CredentialStore,
    ) -> Result<String> {
        ensure!(
            !request.model.trim().is_empty() && request.model.len() <= 128,
            "invalid model name"
        );
        ensure!(
            !request.messages.is_empty() && request.messages.len() <= 128,
            "invalid message count"
        );
        ensure!(
            request
                .messages
                .iter()
                .all(|m| !m.text.is_empty() && m.text.len() <= 1_048_576),
            "invalid message text"
        );
        let route = route(provider, mode);
        let token = match mode {
            AuthMode::ApiKey => std::env::var(route.api_key_env)
                .with_context(|| format!("{} is unavailable", route.api_key_env))?,
            AuthMode::Subscription => {
                let mut credential = store
                    .load(route.oauth_name)?
                    .context("subscription sign-in is required")?;
                let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
                if credential.expires_at <= now.saturating_add(300) {
                    let _lock = store.lock(route.oauth_name).await?;
                    credential = store
                        .load(route.oauth_name)?
                        .context("subscription sign-in is required")?;
                    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
                    if credential.expires_at <= now.saturating_add(300) {
                        credential =
                            oauth::refresh(&self.http, route.oauth_name, &credential).await?;
                        store.save(route.oauth_name, &credential)?;
                    }
                }
                credential.access_token
            }
        };
        ensure!(!token.trim().is_empty(), "provider credential is empty");
        let messages: Vec<Value> = request
            .messages
            .iter()
            .map(|message| json!({"role": message.role, "content": message.text}))
            .collect();
        let body = match route.shape {
            WireShape::Responses => json!({
                "model": request.model,
                "input": messages,
                "store": false,
                "stream": true,
            }),
            WireShape::Chat => json!({
                "model": request.model,
                "messages": messages,
                "stream": true,
            }),
        };
        let mut builder = self
            .http
            .post(route.endpoint)
            .bearer_auth(token)
            .json(&body);
        if route.proxy_header {
            builder = builder
                .header("X-XAI-Token-Auth", "xai-grok-cli")
                .header("x-grok-model-override", request.model.as_str());
        }
        let response = builder.send().await.context("provider request failed")?;
        ensure!(
            response.status().is_success(),
            "provider rejected request ({})",
            response.status()
        );
        read_stream(response, route.shape).await
    }
}

async fn read_stream(response: reqwest::Response, shape: WireShape) -> Result<String> {
    let mut stream = response.bytes_stream();
    let mut pending = Vec::new();
    let mut output = String::new();
    let mut completed = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("provider stream interrupted")?;
        ensure!(
            pending.len().saturating_add(chunk.len()) <= MAX_EVENT_BYTES,
            "provider event is too large"
        );
        pending.extend_from_slice(&chunk);
        process_frames(&mut pending, &mut output, &mut completed, shape)?;
        if completed {
            break;
        }
    }
    ensure!(completed, "provider stream ended without completion");
    Ok(output)
}

fn process_frames(
    pending: &mut Vec<u8>,
    output: &mut String,
    completed: &mut bool,
    shape: WireShape,
) -> Result<()> {
    while let Some(end) = frame_end(pending) {
        let frame: Vec<u8> = pending.drain(..end).collect();
        let text = std::str::from_utf8(&frame).context("invalid provider event encoding")?;
        let mut event_type = "";
        let mut data = "";
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if let Some(value) = line.strip_prefix("event: ") {
                event_type = value;
            }
            if let Some(value) = line.strip_prefix("data: ") {
                data = value;
            }
        }
        if data == "[DONE]" && matches!(shape, WireShape::Chat) {
            *completed = true;
            break;
        }
        if data.is_empty() {
            continue;
        }
        let event: Value = serde_json::from_str(data).context("invalid provider event JSON")?;
        match shape {
            WireShape::Responses => {
                let kind = event
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or(event_type);
                match kind {
                    "response.output_text.delta" => {
                        let delta = event
                            .get("delta")
                            .and_then(Value::as_str)
                            .context("missing text delta")?;
                        append(output, delta)?;
                    }
                    "response.completed" => {
                        *completed = true;
                        break;
                    }
                    "response.failed" | "response.incomplete" => {
                        bail!("provider response did not complete")
                    }
                    _ => {}
                }
            }
            WireShape::Chat => {
                if let Some(delta) = event
                    .pointer("/choices/0/delta/content")
                    .and_then(Value::as_str)
                {
                    append(output, delta)?;
                }
            }
        }
    }
    Ok(())
}

fn append(output: &mut String, delta: &str) -> Result<()> {
    if output.len().saturating_add(delta.len()) > MAX_OUTPUT_BYTES {
        bail!("provider output is too large");
    }
    output.push_str(delta);
    Ok(())
}

fn frame_end(pending: &[u8]) -> Option<usize> {
    let crlf = pending.windows(4).position(|part| part == b"\r\n\r\n");
    let lf = pending.windows(2).position(|part| part == b"\n\n");
    match (crlf, lf) {
        (Some(a), Some(b)) if a <= b => Some(a + 4),
        (Some(_), Some(b)) | (None, Some(b)) => Some(b + 2),
        (Some(a), None) => Some(a + 4),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_keep_subscription_paths_separate() {
        assert_eq!(
            route(Provider::OpenAi, AuthMode::Subscription).endpoint,
            "https://api.openai.com/v1/responses"
        );
        assert_eq!(
            route(Provider::Xai, AuthMode::Subscription).endpoint,
            "https://cli-chat-proxy.grok.com/v1/chat/completions"
        );
        assert_eq!(
            route(Provider::Xai, AuthMode::ApiKey).endpoint,
            "https://api.x.ai/v1/responses"
        );
    }

    #[test]
    fn accepts_both_sse_line_endings() {
        assert_eq!(frame_end(b"data: one\n\nnext"), Some(11));
        assert_eq!(frame_end(b"data: one\r\n\r\nnext"), Some(13));
        assert_eq!(frame_end(b"data: one"), None);
    }

    #[test]
    fn responses_stream_requires_completed_event() {
        let body = "event: response.output_text.delta\r\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\r\n\r\nevent: response.completed\r\ndata: {\"type\":\"response.completed\"}\r\n\r\n";
        let mut pending = body.as_bytes().to_vec();
        let mut output = String::new();
        let mut complete = false;
        process_frames(
            &mut pending,
            &mut output,
            &mut complete,
            WireShape::Responses,
        )
        .unwrap();
        assert_eq!(output, "hello");
        assert!(complete);
        let mut pending = b"data: {\"type\":\"response.incomplete\"}\n\n".to_vec();
        assert!(
            process_frames(
                &mut pending,
                &mut output,
                &mut complete,
                WireShape::Responses
            )
            .is_err()
        );
    }

    #[test]
    fn chat_stream_accepts_done_marker() {
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\ndata: [DONE]\n\n";
        let mut pending = body.as_bytes().to_vec();
        let mut output = String::new();
        let mut complete = false;
        process_frames(&mut pending, &mut output, &mut complete, WireShape::Chat).unwrap();
        assert_eq!(output, "hello");
        assert!(complete);
    }
}
