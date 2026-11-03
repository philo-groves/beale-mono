use crate::{credentials::CredentialStore, oauth};
use anyhow::{Context, Result, bail, ensure};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;

const MAX_EVENT_BYTES: usize = 1_048_576;
const MAX_OUTPUT_BYTES: usize = 2_097_152;
const MAX_REASONING_SUMMARIES: usize = 16;
const MAX_REASONING_SUMMARY_BYTES: usize = 16_384;

fn openai_cyber_program(model: &str, mode: &str) -> Result<&'static str> {
    ensure!(matches!(mode, "blue" | "red"), "invalid Daybreak mode");
    let red_program = matches!(
        model,
        "gpt-5.5-cyber" | "gpt-5.6-cyber" | "gpt-daybreak-red-latest"
    );
    let red_approval = red_program || matches!(model, "gpt-6.1-sol" | "gpt-6-astra");
    ensure!(
        mode == "red" || !red_approval,
        "selected OpenAI model requires Daybreak Red access"
    );
    Ok(if red_program {
        "daybreak_red"
    } else {
        "daybreak_blue"
    })
}

fn request_cyber_program(
    provider: Provider,
    model: &str,
    store: &CredentialStore,
) -> Result<Option<&'static str>> {
    if provider == Provider::OpenAi {
        Ok(Some(openai_cyber_program(model, store.daybreak_mode()?)?))
    } else {
        Ok(None)
    }
}

fn add_cyber_access(body: &mut Value, program: Option<&str>) {
    if let Some(program) = program {
        body["access_programs"] = json!({"cyber":program});
    }
}

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

pub struct ToolStepRequest<'a> {
    pub model: &'a str,
    pub input: &'a [Value],
    pub tools: &'a [Value],
    pub reasoning_effort: Option<&'a str>,
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

#[derive(Clone, Debug)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug)]
pub struct ProviderStep {
    pub text: String,
    pub calls: Vec<ToolCall>,
    pub reasoning_summaries: Vec<String>,
    /// Provider output items to replay while `store` is false.
    pub continuation: Vec<Value>,
}

#[derive(Clone, Debug)]
pub enum ProviderProgress {
    MessageDelta(String),
    ReasoningDelta(String),
}

#[derive(Default)]
struct PendingCall {
    id: String,
    name: String,
    arguments: String,
}

pub fn input_messages(messages: &[TextMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|message| json!({"role":message.role,"content":message.text}))
        .collect()
}

pub fn tool_output(provider: Provider, mode: AuthMode, id: &str, output: &str) -> Value {
    match route(provider, mode).shape {
        WireShape::Responses => json!({"type":"function_call_output","call_id":id,"output":output}),
        WireShape::Chat => json!({"role":"tool","tool_call_id":id,"content":output}),
    }
}

fn tool_request_body(
    provider: Provider,
    shape: WireShape,
    model: &str,
    input: &[Value],
    tools: &[Value],
    reasoning_effort: Option<&str>,
    cyber_program: Option<&str>,
) -> Value {
    match shape {
        WireShape::Responses => {
            let mut body = json!({
            "model":model,"input":input,"tools":tools,"store":false,"stream":true,
            "include":["reasoning.encrypted_content"]
            });
            if provider == Provider::OpenAi {
                body["reasoning"] = json!({"summary":"auto"});
                if let Some(effort) = reasoning_effort {
                    body["reasoning"]["effort"] = json!(effort);
                }
            }
            add_cyber_access(&mut body, cyber_program);
            body
        }
        WireShape::Chat => {
            let chat_tools: Vec<Value> = tools.iter().map(|tool| json!({
                "type":"function",
                "function":{"name":tool["name"],"description":tool["description"],"parameters":tool["parameters"]}
            })).collect();
            json!({"model":model,"messages":input,"tools":chat_tools,"stream":true})
        }
    }
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

    pub(crate) async fn token(
        &self,
        provider: Provider,
        mode: AuthMode,
        store: &CredentialStore,
    ) -> Result<String> {
        let route = route(provider, mode);
        let token = match mode {
            AuthMode::ApiKey => store
                .load_api_key(route.oauth_name)?
                .or_else(|| std::env::var(route.api_key_env).ok())
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
        Ok(token)
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
        let token = self.token(provider, mode, store).await?;
        let messages: Vec<Value> = request
            .messages
            .iter()
            .map(|message| json!({"role": message.role, "content": message.text}))
            .collect();
        let cyber_program = request_cyber_program(provider, &request.model, store)?;
        let mut body = match route.shape {
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
        add_cyber_access(&mut body, cyber_program);
        let mut builder = self
            .http
            .post(route.endpoint)
            .bearer_auth(token)
            .json(&body);
        if route.proxy_header {
            builder = builder
                .header("X-XAI-Token-Auth", "xai-grok-cli")
                .header("x-grok-client-version", "1.0.13")
                .header("x-grok-client-identifier", "beale")
                .header("x-grok-client-mode", "headless")
                .header("x-grok-model-override", request.model.as_str());
        }
        let response = builder.send().await.context("provider request failed")?;
        if !response.status().is_success() {
            let status = response.status();
            bail!("provider rejected request ({status})");
        }
        read_stream(response, route.shape).await
    }

    pub async fn tool_step(
        &self,
        provider: Provider,
        mode: AuthMode,
        request: ToolStepRequest<'_>,
        store: &CredentialStore,
        progress: Option<&mpsc::Sender<ProviderProgress>>,
    ) -> Result<ProviderStep> {
        ensure!(
            !request.model.is_empty() && request.model.len() <= 128,
            "invalid model name"
        );
        ensure!(
            !request.input.is_empty() && request.input.len() <= 192,
            "invalid provider input count"
        );
        let route = route(provider, mode);
        let token = self.token(provider, mode, store).await?;
        let cyber_program = request_cyber_program(provider, request.model, store)?;
        let body = tool_request_body(
            provider,
            route.shape,
            request.model,
            request.input,
            request.tools,
            request.reasoning_effort,
            cyber_program,
        );
        let mut builder = self
            .http
            .post(route.endpoint)
            .bearer_auth(token)
            .json(&body);
        if route.proxy_header {
            builder = builder
                .header("X-XAI-Token-Auth", "xai-grok-cli")
                .header("x-grok-client-version", "1.0.13")
                .header("x-grok-client-identifier", "beale")
                .header("x-grok-client-mode", "headless")
                .header("x-grok-model-override", request.model);
        }
        let response = builder.send().await.context("provider request failed")?;
        if !response.status().is_success() {
            bail!("provider rejected request ({})", response.status());
        }
        read_tool_stream(response, route.shape, progress).await
    }
}

async fn read_tool_stream(
    response: reqwest::Response,
    shape: WireShape,
    progress: Option<&mpsc::Sender<ProviderProgress>>,
) -> Result<ProviderStep> {
    let mut stream = response.bytes_stream();
    let mut pending = Vec::new();
    let mut text = String::new();
    let mut completed = false;
    let mut response_output: Option<Vec<Value>> = None;
    let mut response_items = Vec::<Value>::new();
    let mut chat_calls = BTreeMap::<usize, PendingCall>::new();
    let mut reasoning_parts = BTreeMap::<(u64, u64), String>::new();
    let mut sent_reasoning = HashSet::<(u64, u64)>::new();
    let mut sent_text = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("provider stream interrupted")?;
        ensure!(
            pending.len().saturating_add(chunk.len()) <= MAX_EVENT_BYTES,
            "provider event is too large"
        );
        pending.extend_from_slice(&chunk);
        while let Some(end) = frame_end(&pending) {
            let frame: Vec<u8> = pending.drain(..end).collect();
            let frame = std::str::from_utf8(&frame).context("invalid provider event encoding")?;
            let data = frame
                .lines()
                .find_map(|line| line.trim_end_matches('\r').strip_prefix("data: "))
                .unwrap_or("");
            if data == "[DONE]" && matches!(shape, WireShape::Chat) {
                completed = true;
                break;
            }
            if data.is_empty() {
                continue;
            }
            let event: Value = serde_json::from_str(data).context("invalid provider event JSON")?;
            match shape {
                WireShape::Responses => {
                    match event.get("type").and_then(Value::as_str).unwrap_or("") {
                        "response.output_text.delta" => {
                            let delta = event
                                .get("delta")
                                .and_then(Value::as_str)
                                .context("missing text delta")?;
                            append(&mut text, delta)?;
                            sent_text |= !delta.is_empty();
                            if !delta.is_empty()
                                && let Some(tx) = progress
                            {
                                let _ = tx
                                    .send(ProviderProgress::MessageDelta(delta.to_owned()))
                                    .await;
                            }
                        }
                        "response.reasoning_summary_text.delta" => {
                            let key = summary_key(&event)?;
                            let delta = event
                                .get("delta")
                                .and_then(Value::as_str)
                                .context("missing reasoning summary delta")?;
                            ensure!(
                                reasoning_parts.contains_key(&key)
                                    || reasoning_parts.len() < MAX_REASONING_SUMMARIES,
                                "too many reasoning summaries"
                            );
                            let part = reasoning_parts.entry(key).or_default();
                            ensure!(
                                part.len().saturating_add(delta.len())
                                    <= MAX_REASONING_SUMMARY_BYTES,
                                "reasoning summary is too large"
                            );
                            part.push_str(delta);
                            if !delta.is_empty() {
                                sent_reasoning.insert(key);
                            }
                            if !delta.is_empty()
                                && let Some(tx) = progress
                            {
                                let _ = tx
                                    .send(ProviderProgress::ReasoningDelta(delta.to_owned()))
                                    .await;
                            }
                        }
                        "response.reasoning_summary_text.done"
                        | "response.reasoning_summary_part.done" => {
                            let key = summary_key(&event)?;
                            if let Some(value) = event
                                .get("text")
                                .or_else(|| event.pointer("/part/text"))
                                .and_then(Value::as_str)
                            {
                                ensure!(
                                    value.len() <= MAX_REASONING_SUMMARY_BYTES,
                                    "reasoning summary is too large"
                                );
                                reasoning_parts.insert(key, value.to_owned());
                                if !value.is_empty()
                                    && sent_reasoning.insert(key)
                                    && let Some(tx) = progress
                                {
                                    let _ = tx
                                        .send(ProviderProgress::ReasoningDelta(value.to_owned()))
                                        .await;
                                }
                                ensure!(
                                    reasoning_parts.len() <= MAX_REASONING_SUMMARIES,
                                    "too many reasoning summaries"
                                );
                            }
                        }
                        "response.output_item.done" => {
                            if let Some(item) = event.get("item") {
                                ensure!(
                                    response_items.len() < 32,
                                    "too many provider output items"
                                );
                                response_items.push(item.clone());
                            }
                        }
                        "response.completed" | "response.done" => {
                            response_output = event
                                .pointer("/response/output")
                                .and_then(Value::as_array)
                                .cloned();
                            completed = true;
                            break;
                        }
                        "response.failed" | "response.incomplete" => {
                            bail!("provider response did not complete")
                        }
                        _ => {}
                    }
                }
                WireShape::Chat => {
                    let before = text.len();
                    apply_chat_event(&event, &mut text, &mut chat_calls)?;
                    if let Some(tx) = progress
                        && text.len() > before
                    {
                        let _ = tx
                            .send(ProviderProgress::MessageDelta(text[before..].to_owned()))
                            .await;
                    }
                }
            }
        }
        if completed {
            break;
        }
    }
    ensure!(completed, "provider stream ended without completion");
    match shape {
        WireShape::Responses => {
            let output = response_output.unwrap_or(response_items);
            if text.is_empty() {
                for item in &output {
                    if let Some(content) = item.get("content").and_then(Value::as_array) {
                        for part in content {
                            if let Some(value) = part.get("text").and_then(Value::as_str) {
                                append(&mut text, value)?;
                            }
                        }
                    }
                }
            }
            let calls = calls_from_response(&output)?;
            let reasoning_summaries = summaries_from_response(&output, reasoning_parts)?;
            if let Some(tx) = progress {
                if !sent_text && !text.is_empty() {
                    let _ = tx.send(ProviderProgress::MessageDelta(text.clone())).await;
                }
                if sent_reasoning.is_empty() {
                    for summary in &reasoning_summaries {
                        let _ = tx
                            .send(ProviderProgress::ReasoningDelta(summary.clone()))
                            .await;
                    }
                }
            }
            Ok(ProviderStep {
                text,
                calls,
                reasoning_summaries,
                continuation: output,
            })
        }
        WireShape::Chat => {
            let calls: Vec<ToolCall> = chat_calls
                .into_values()
                .map(|call| ToolCall {
                    id: call.id,
                    name: call.name,
                    arguments: call.arguments,
                })
                .collect();
            let continuation = if calls.is_empty() {
                Vec::new()
            } else {
                vec![
                    json!({"role":"assistant","content":text,"tool_calls":calls.iter().map(|call| json!({"id":call.id,"type":"function","function":{"name":call.name,"arguments":call.arguments}})).collect::<Vec<_>>()}),
                ]
            };
            Ok(ProviderStep {
                text,
                calls,
                reasoning_summaries: Vec::new(),
                continuation,
            })
        }
    }
}

fn summary_key(event: &Value) -> Result<(u64, u64)> {
    Ok((
        event
            .get("output_index")
            .and_then(Value::as_u64)
            .context("missing reasoning output index")?,
        event
            .get("summary_index")
            .and_then(Value::as_u64)
            .context("missing reasoning summary index")?,
    ))
}

fn summaries_from_response(
    output: &[Value],
    mut streamed: BTreeMap<(u64, u64), String>,
) -> Result<Vec<String>> {
    for (output_index, item) in output.iter().enumerate() {
        if item.get("type").and_then(Value::as_str) != Some("reasoning") {
            continue;
        }
        if let Some(parts) = item.get("summary").and_then(Value::as_array) {
            for (summary_index, part) in parts.iter().enumerate() {
                if let Some(value) = part.get("text").and_then(Value::as_str) {
                    ensure!(
                        value.len() <= MAX_REASONING_SUMMARY_BYTES,
                        "reasoning summary is too large"
                    );
                    streamed.insert(
                        (output_index as u64, summary_index as u64),
                        value.to_owned(),
                    );
                    ensure!(
                        streamed.len() <= MAX_REASONING_SUMMARIES,
                        "too many reasoning summaries"
                    );
                }
            }
        }
    }
    Ok(streamed
        .into_values()
        .filter(|value| !value.trim().is_empty())
        .collect())
}

fn calls_from_response(output: &[Value]) -> Result<Vec<ToolCall>> {
    let mut calls = Vec::new();
    for item in output {
        if item.get("type").and_then(Value::as_str) == Some("function_call") {
            calls.push(ToolCall {
                id: item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .context("missing call ID")?
                    .to_owned(),
                name: item
                    .get("name")
                    .and_then(Value::as_str)
                    .context("missing tool name")?
                    .to_owned(),
                arguments: item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .context("missing tool arguments")?
                    .to_owned(),
            });
        }
    }
    ensure!(calls.len() <= 16, "too many provider tool calls");
    Ok(calls)
}

fn apply_chat_event(
    event: &Value,
    text: &mut String,
    calls: &mut BTreeMap<usize, PendingCall>,
) -> Result<()> {
    if let Some(delta) = event
        .pointer("/choices/0/delta/content")
        .and_then(Value::as_str)
    {
        append(text, delta)?;
    }
    if let Some(items) = event
        .pointer("/choices/0/delta/tool_calls")
        .and_then(Value::as_array)
    {
        for item in items {
            let index = item
                .get("index")
                .and_then(Value::as_u64)
                .context("missing tool call index")? as usize;
            ensure!(index < 16, "too many provider tool calls");
            let pending = calls.entry(index).or_default();
            if let Some(id) = item.get("id").and_then(Value::as_str) {
                pending.id.push_str(id);
            }
            if let Some(name) = item.pointer("/function/name").and_then(Value::as_str) {
                pending.name.push_str(name);
            }
            if let Some(args) = item.pointer("/function/arguments").and_then(Value::as_str) {
                pending.arguments.push_str(args);
            }
            ensure!(
                pending.arguments.len() <= 65_536,
                "tool arguments are too large"
            );
        }
    }
    Ok(())
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
                    "response.completed" | "response.done" => {
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

    #[test]
    fn responses_tool_calls_replay_output_items_and_match_call_ids() {
        let output = vec![
            json!({"type":"reasoning","encrypted_content":"example-encrypted"}),
            json!({"type":"function_call","call_id":"call-example","name":"read","arguments":"{\"path\":\"example.txt\"}"}),
        ];
        let calls = calls_from_response(&output).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read");
        assert_eq!(
            tool_output(Provider::OpenAi, AuthMode::ApiKey, &calls[0].id, "example"),
            json!({"type":"function_call_output","call_id":"call-example","output":"example"})
        );
        assert_eq!(
            tool_output(
                Provider::Xai,
                AuthMode::Subscription,
                &calls[0].id,
                "example"
            ),
            json!({"role":"tool","tool_call_id":"call-example","content":"example"})
        );
    }

    #[test]
    fn tool_request_bodies_follow_each_provider_wire_shape() {
        let input = vec![json!({"role":"user","content":"Read example.txt"})];
        let tools = vec![
            json!({"type":"function","name":"read","description":"Read file","parameters":{"type":"object"}}),
        ];
        let responses = tool_request_body(
            Provider::OpenAi,
            WireShape::Responses,
            "model-example",
            &input,
            &tools,
            Some("high"),
            Some("daybreak_blue"),
        );
        assert_eq!(responses["tools"][0]["name"], "read");
        assert_eq!(responses["store"], false);
        assert_eq!(responses["input"], json!(input));
        assert_eq!(responses["reasoning"]["summary"], "auto");
        assert_eq!(responses["reasoning"]["effort"], "high");
        assert_eq!(responses["access_programs"]["cyber"], "daybreak_blue");
        let xai = tool_request_body(
            Provider::Xai,
            WireShape::Responses,
            "model-example",
            &input,
            &tools,
            Some("high"),
            None,
        );
        assert!(xai.get("reasoning").is_none());
        assert!(xai.get("access_programs").is_none());
        let chat = tool_request_body(
            Provider::Xai,
            WireShape::Chat,
            "model-example",
            &input,
            &tools,
            Some("high"),
            None,
        );
        assert_eq!(chat["tools"][0]["function"]["name"], "read");
        assert_eq!(chat["messages"], json!(input));
        assert!(chat.get("access_programs").is_none());
    }

    #[test]
    fn openai_cyber_program_matches_model_and_approval() {
        for mode in ["blue", "red"] {
            assert_eq!(
                openai_cyber_program("gpt-5.6-sol", mode).unwrap(),
                "daybreak_blue"
            );
            assert_eq!(
                openai_cyber_program("gpt-6-sol", mode).unwrap(),
                "daybreak_blue"
            );
        }
        assert_eq!(
            openai_cyber_program("gpt-6.1-sol", "red").unwrap(),
            "daybreak_blue"
        );
        assert_eq!(
            openai_cyber_program("gpt-6-astra", "red").unwrap(),
            "daybreak_blue"
        );
        assert_eq!(
            openai_cyber_program("gpt-5.6-cyber", "red").unwrap(),
            "daybreak_red"
        );
        assert!(openai_cyber_program("gpt-6.1-sol", "blue").is_err());
        assert!(openai_cyber_program("gpt-5.6-cyber", "blue").is_err());
        assert!(openai_cyber_program("gpt-5.6-sol", "other").is_err());
        assert_eq!(
            openai_cyber_program("gpt-example-model", "blue").unwrap(),
            "daybreak_blue"
        );
    }

    #[test]
    fn request_cyber_program_reads_the_host_setting_only_for_openai() {
        let root = std::env::temp_dir().join(format!(
            "beale-cyber-routing-example-{}",
            uuid::Uuid::new_v4()
        ));
        let store = CredentialStore::at(root.clone());
        assert_eq!(
            request_cyber_program(Provider::OpenAi, "gpt-5.6-sol", &store).unwrap(),
            Some("daybreak_blue")
        );
        store.save_daybreak_mode("red").unwrap();
        assert_eq!(
            request_cyber_program(Provider::OpenAi, "gpt-5.6-cyber", &store).unwrap(),
            Some("daybreak_red")
        );
        assert_eq!(
            request_cyber_program(Provider::Xai, "grok-4.7", &store).unwrap(),
            None
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_only_provider_reasoning_summaries() {
        let output = vec![json!({
            "type":"reasoning",
            "reasoning_text":"private trace",
            "summary":[{"type":"summary_text","text":"Checked the example input."}]
        })];
        let mut streamed = BTreeMap::new();
        streamed.insert((0, 0), "partial".to_owned());
        let summaries = summaries_from_response(&output, streamed).unwrap();
        assert_eq!(summaries, vec!["Checked the example input."]);
        assert!(summaries[0].find("private trace").is_none());
    }

    #[tokio::test]
    async fn provider_deltas_arrive_before_the_response_completes() {
        use axum::{body::Bytes, http::Response};
        use futures_util::stream;
        use std::convert::Infallible;

        let chunks = stream::unfold(0, |index| async move {
            let body = match index {
                0 => {
                    "data: {\"type\":\"response.reasoning_summary_text.delta\",\"output_index\":0,\"summary_index\":0,\"delta\":\"Checking example\"}\n\n"
                }
                1 => {
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Done example\"}\n\n"
                }
                2 => {
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"reasoning\",\"summary\":[{\"text\":\"Checking example\"}]},{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"Done example\"}]}]}}\n\n"
                }
                _ => return None,
            };
            Some((
                Ok::<_, Infallible>(Bytes::from_static(body.as_bytes())),
                index + 1,
            ))
        });
        let response: reqwest::Response = Response::new(reqwest::Body::wrap_stream(chunks)).into();
        let (tx, mut rx) = mpsc::channel(8);
        let reader = tokio::spawn(async move {
            read_tool_stream(response, WireShape::Responses, Some(&tx)).await
        });
        let first = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(first, ProviderProgress::ReasoningDelta(value) if value == "Checking example")
        );
        assert!(!reader.is_finished());
        let second = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(second, ProviderProgress::MessageDelta(value) if value == "Done example"));
        assert!(!reader.is_finished());
        let result = reader.await.unwrap().unwrap();
        assert_eq!(result.text, "Done example");
        assert_eq!(result.reasoning_summaries, ["Checking example"]);
    }

    #[tokio::test]
    async fn chat_text_arrives_before_the_done_marker() {
        use axum::{body::Bytes, http::Response};
        use futures_util::stream;
        use std::convert::Infallible;

        let chunks = stream::unfold(0, |index| async move {
            let body = match index {
                0 => "data: {\"choices\":[{\"delta\":{\"content\":\"Example reply\"}}]}\n\n",
                1 => {
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    "data: [DONE]\n\n"
                }
                _ => return None,
            };
            Some((
                Ok::<_, Infallible>(Bytes::from_static(body.as_bytes())),
                index + 1,
            ))
        });
        let response: reqwest::Response = Response::new(reqwest::Body::wrap_stream(chunks)).into();
        let (tx, mut rx) = mpsc::channel(8);
        let reader =
            tokio::spawn(
                async move { read_tool_stream(response, WireShape::Chat, Some(&tx)).await },
            );
        let first = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(first, ProviderProgress::MessageDelta(value) if value == "Example reply"));
        assert!(!reader.is_finished());
        assert_eq!(reader.await.unwrap().unwrap().text, "Example reply");
    }

    #[test]
    fn chat_tool_call_arguments_reassemble_across_deltas() {
        let mut text = String::new();
        let mut calls = BTreeMap::new();
        apply_chat_event(
            &json!({"choices":[{"delta":{"reasoning_content":"private trace"}}]}),
            &mut text,
            &mut calls,
        )
        .unwrap();
        assert!(text.is_empty());
        apply_chat_event(&json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-example","function":{"name":"bash","arguments":"{\"command\":"}}]}}]}), &mut text, &mut calls).unwrap();
        apply_chat_event(&json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"printf example\"}"}}]}}]}), &mut text, &mut calls).unwrap();
        let call = calls.get(&0).unwrap();
        assert_eq!(call.name, "bash");
        assert_eq!(call.arguments, "{\"command\":\"printf example\"}");
    }

    #[tokio::test]
    #[ignore = "requires signed-in provider credentials and live network access"]
    async fn signed_in_providers_complete_a_text_turn() {
        let store = CredentialStore::new().unwrap();
        let client = ProviderClient::new().unwrap();
        for provider in [Provider::OpenAi, Provider::Xai] {
            let catalog = client
                .list_models(provider, AuthMode::Subscription, &store)
                .await
                .unwrap();
            let answer = client
                .infer(
                    provider,
                    AuthMode::Subscription,
                    &InferenceRequest {
                        model: catalog.lead_model,
                        messages: vec![TextMessage {
                            role: TextRole::User,
                            text: "Reply with a short echo of: Beale example message.".to_owned(),
                        }],
                    },
                    &store,
                )
                .await
                .unwrap();
            assert!(!answer.trim().is_empty());
        }
    }
}
