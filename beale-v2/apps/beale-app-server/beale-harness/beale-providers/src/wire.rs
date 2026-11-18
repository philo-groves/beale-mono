use crate::{credentials::CredentialStore, oauth};
use anyhow::{Context, Result, bail, ensure};
use futures_util::{SinkExt, Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    fmt,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest, http::HeaderValue},
};

mod anthropic;

const MAX_EVENT_BYTES: usize = 1_048_576;
const MAX_OUTPUT_BYTES: usize = 2_097_152;
const MAX_REASONING_SUMMARIES: usize = 16;
const MAX_REASONING_SUMMARY_BYTES: usize = 16_384;

#[derive(Clone, Copy, Debug)]
enum FailureKind {
    Http(u16),
    Failed,
    Incomplete,
    StreamEnded,
}

#[derive(Debug)]
struct ProviderFailure {
    kind: FailureKind,
    code: Option<String>,
    param: Option<String>,
}

impl fmt::Display for ProviderFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "provider {:?}", self.kind)?;
        if let Some(code) = &self.code {
            write!(formatter, " ({code})")?;
        }
        if let Some(param) = &self.param {
            write!(formatter, " [param: {param}]")?;
        }
        Ok(())
    }
}

impl std::error::Error for ProviderFailure {}

#[derive(Debug)]
struct DaybreakAccessError;

#[derive(Debug)]
struct DaybreakSelectionError {
    requested: String,
    selected: String,
}

impl fmt::Display for DaybreakSelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "OpenAI selected {} instead of requested {}",
            self.selected, self.requested
        )
    }
}

impl std::error::Error for DaybreakSelectionError {}

impl fmt::Display for DaybreakAccessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("selected OpenAI model requires Daybreak Red access")
    }
}

impl std::error::Error for DaybreakAccessError {}

fn safe_code(value: Option<&str>) -> Option<String> {
    value
        .filter(|code| {
            !code.is_empty()
                && code.len() <= 80
                && code
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
        .map(str::to_ascii_lowercase)
}

fn safe_param(value: Option<&str>) -> Option<String> {
    value
        .filter(|param| {
            !param.is_empty()
                && param.len() <= 80
                && param.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-' | b'[' | b']')
                })
        })
        .map(str::to_owned)
}

fn terminal_failure(event: &Value, kind: FailureKind) -> ProviderFailure {
    let code = match kind {
        FailureKind::Incomplete => event
            .pointer("/response/incomplete_details/reason")
            .and_then(Value::as_str),
        FailureKind::Failed => event
            .pointer("/response/error/code")
            .or_else(|| event.pointer("/error/code"))
            .or_else(|| event.get("code"))
            .and_then(Value::as_str),
        FailureKind::Http(_) | FailureKind::StreamEnded => None,
    };
    ProviderFailure {
        kind,
        code: safe_code(code),
        param: safe_param(
            event
                .pointer("/response/error/param")
                .or_else(|| event.pointer("/error/param"))
                .or_else(|| event.get("param"))
                .and_then(Value::as_str),
        ),
    }
}

fn rejected_failure(status: u16, body: &[u8]) -> ProviderFailure {
    let detail = (body.len() <= 8_192)
        .then(|| serde_json::from_slice::<Value>(body).ok())
        .flatten();
    let code = detail.as_ref().and_then(|value| {
        safe_code(
            value
                .pointer("/error/code")
                .or_else(|| value.get("code"))
                .and_then(Value::as_str),
        )
    });
    let param = detail.as_ref().and_then(|value| {
        safe_param(
            value
                .pointer("/error/param")
                .or_else(|| value.get("param"))
                .and_then(Value::as_str),
        )
    });
    ProviderFailure {
        kind: FailureKind::Http(status),
        code,
        param,
    }
}

async fn rejected_response(mut response: reqwest::Response) -> ProviderFailure {
    let status = response.status().as_u16();
    let mut body = Vec::new();
    if response
        .content_length()
        .is_none_or(|length| length <= 8_192)
    {
        while body.len() < 8_192 {
            match response.chunk().await {
                Ok(Some(chunk)) if body.len() + chunk.len() <= 8_192 => {
                    body.extend_from_slice(&chunk)
                }
                _ => break,
            }
        }
    }
    rejected_failure(status, &body)
}

pub fn provider_error_retryable(error: &anyhow::Error) -> bool {
    if matches!(
        error.downcast_ref::<tokio_tungstenite::tungstenite::Error>(),
        Some(
            tokio_tungstenite::tungstenite::Error::Io(_)
                | tokio_tungstenite::tungstenite::Error::ConnectionClosed
                | tokio_tungstenite::tungstenite::Error::AlreadyClosed
        )
    ) {
        return true;
    }
    let websocket_failure = error
        .downcast_ref::<tokio_tungstenite::tungstenite::Error>()
        .and_then(|error| match error {
            tokio_tungstenite::tungstenite::Error::Http(response) => Some(rejected_failure(
                response.status().as_u16(),
                response.body().as_deref().unwrap_or_default(),
            )),
            _ => None,
        });
    if let Some(failure) = error
        .downcast_ref::<ProviderFailure>()
        .or(websocket_failure.as_ref())
    {
        return matches!(
            failure.kind,
            FailureKind::Http(408 | 429 | 500..=599) | FailureKind::StreamEnded
        ) || matches!(
            failure.code.as_deref(),
            Some("SERVER_ERROR" | "server_error" | "rate_limit_exceeded")
        );
    }
    if let Some(network) = error.downcast_ref::<reqwest::Error>() {
        return network.is_timeout()
            || network.is_connect()
            || network.is_body()
            || network.is_decode();
    }
    error.chain().any(|cause| {
        matches!(
            cause.to_string().as_str(),
            "OpenAI WebSocket connection timed out"
                | "OpenAI response timed out"
                | "OpenAI WebSocket is unavailable"
                | "OpenAI WebSocket recovery failed"
                | "OpenAI WebSocket send failed"
        )
    })
}

pub fn provider_error_message(provider: Provider, mode: AuthMode, error: &anyhow::Error) -> String {
    if provider == Provider::OpenAi
        && mode == AuthMode::Subscription
        && error
            .downcast_ref::<crate::codex::CodexSignInError>()
            .is_some()
    {
        return "Sign in to Codex on this host, then retry.".to_owned();
    }
    if let Some(selection) = error.downcast_ref::<DaybreakSelectionError>() {
        return format!(
            "OpenAI selected {} instead of {} for this request. Check Codex account access and the Daybreak setting.",
            selection.selected, selection.requested
        );
    }
    if error.downcast_ref::<DaybreakAccessError>().is_some() {
        return "This OpenAI model requires Daybreak Red. Choose another model or switch Daybreak Mode in Providers.".to_owned();
    }
    let websocket_failure = error
        .downcast_ref::<tokio_tungstenite::tungstenite::Error>()
        .and_then(|error| match error {
            tokio_tungstenite::tungstenite::Error::Http(response) => Some(rejected_failure(
                response.status().as_u16(),
                response.body().as_deref().unwrap_or_default(),
            )),
            _ => None,
        });
    let Some(failure) = error
        .downcast_ref::<ProviderFailure>()
        .or(websocket_failure.as_ref())
    else {
        if let Some(network) = error.downcast_ref::<reqwest::Error>() {
            return if network.is_timeout() {
                "The provider request timed out. Try again.".to_owned()
            } else {
                "The provider connection was interrupted. Try again.".to_owned()
            };
        }
        let has_cause = |cause: &str| error.chain().any(|item| item.to_string() == cause);
        if has_cause("OpenAI WebSocket connection timed out")
            || has_cause("OpenAI response timed out")
        {
            return "OpenAI timed out before completing this turn.".to_owned();
        }
        if has_cause("OpenAI WebSocket is unavailable")
            || has_cause("OpenAI WebSocket recovery failed")
            || has_cause("OpenAI WebSocket send failed")
        {
            return "The OpenAI connection failed during this turn.".to_owned();
        }
        if has_cause("provider output is too large") {
            return "The provider response exceeded the supported output size.".to_owned();
        }
        if has_cause("too many provider output items") || has_cause("too many provider tool calls")
        {
            return "The provider returned more items than the current protocol supports."
                .to_owned();
        }
        if has_cause("invalid provider event JSON") || has_cause("invalid provider event encoding")
        {
            return "The provider returned an invalid response event.".to_owned();
        }
        return "The provider could not complete this turn. Try again.".to_owned();
    };
    if provider == Provider::OpenAi
        && mode == AuthMode::Subscription
        && failure.code.as_deref() == Some("access_program_not_enabled")
    {
        return "Daybreak access is not enabled for this Codex account and selected model."
            .to_owned();
    }
    match failure.kind {
        FailureKind::Http(401) if provider == Provider::OpenAi && mode == AuthMode::Subscription => {
            "Codex sign-in was rejected. Sign in to Codex again, then retry.".to_owned()
        }
        FailureKind::Http(401) => "The provider sign-in was rejected. Reconnect this provider.".to_owned(),
        FailureKind::Http(403) if provider == Provider::OpenAi && mode == AuthMode::Subscription => {
            "OpenAI denied this Codex request. Check the Codex account, model, and Daybreak access.".to_owned()
        }
        FailureKind::Http(403) => {
            "The provider denied this request for the selected account or model.".to_owned()
        }
        FailureKind::Http(429) => {
            "The provider rate limit was reached. Try again shortly.".to_owned()
        }
        FailureKind::Http(status) if status >= 500 => {
            format!("The provider had a server error (HTTP {status}). Try again.")
        }
        FailureKind::Http(status) => format!("The provider rejected this request (HTTP {status})."),
        FailureKind::Failed => match failure.code.as_deref() {
            Some("server_error") => "The provider had a server error. Try again.".to_owned(),
            None if provider == Provider::OpenAi && failure.param.as_deref() == Some("input") => {
                "OpenAI rejected this session's conversation input. Retry the turn; if it persists, start a new session.".to_owned()
            }
            Some(code) => format!("The provider ended the response with error code {code}."),
            None => "The provider ended the response with an error.".to_owned(),
        },
        FailureKind::Incomplete => match failure.code.as_deref() {
            Some("max_output_tokens") => "The provider stopped at its output limit.".to_owned(),
            Some(code) => format!("The provider stopped before completion ({code})."),
            None => "The provider stopped before completing this turn.".to_owned(),
        },
        FailureKind::StreamEnded => {
            "The provider stream closed before completing the turn. Try again.".to_owned()
        }
    }
}

pub(crate) fn openai_requires_daybreak_red(model: &str) -> bool {
    matches!(
        model,
        "gpt-5.5-cyber"
            | "gpt-5.6-cyber"
            | "gpt-daybreak-red-latest"
            | "gpt-6.1-sol"
            | "gpt-6-astra"
    )
}

fn openai_cyber_program(model: &str, mode: &str) -> Result<&'static str> {
    ensure!(matches!(mode, "blue" | "red"), "invalid Daybreak mode");
    let red_program = matches!(
        model,
        "gpt-5.5-cyber" | "gpt-5.6-cyber" | "gpt-daybreak-red-latest"
    );
    if mode != "red" && openai_requires_daybreak_red(model) {
        return Err(DaybreakAccessError.into());
    }
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
    Anthropic,
    Zai,
    OpenRouter,
    Custom,
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
    pub instructions: Option<&'a str>,
    pub reasoning_effort: Option<&'a str>,
    pub workspace_root: Option<&'a std::path::Path>,
}

#[derive(Clone, Copy)]
enum WireShape {
    Responses,
    Chat,
    Anthropic,
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
    pub usage: Option<TokenUsage>,
    /// Provider output items to replay while `store` is false.
    pub continuation: Vec<Value>,
    pub response_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: Option<u64>,
}

fn token_usage(value: &Value, input: &str, output: &str) -> Option<TokenUsage> {
    let input_tokens = value.get(input)?.as_u64()?;
    let output_tokens = value.get(output)?.as_u64()?;
    (input_tokens <= 1_000_000_000_000 && output_tokens <= 1_000_000_000_000).then_some(
        TokenUsage {
            input_tokens,
            output_tokens,
            cache_read_tokens: None,
        },
    )
}

fn token_usage_with_cache(
    value: &Value,
    input: &str,
    output: &str,
    cached_path: &str,
) -> Option<TokenUsage> {
    let mut usage = token_usage(value, input, output)?;
    usage.cache_read_tokens = value
        .pointer(cached_path)
        .and_then(Value::as_u64)
        .filter(|&cached| cached <= usage.input_tokens);
    Some(usage)
}

#[derive(Clone, Debug)]
pub enum ProviderProgress {
    MessageDelta(String),
    ReasoningDelta(String, (u64, u64)),
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
        WireShape::Anthropic => {
            json!({"role":"user","content":[{"type":"tool_result","tool_use_id":id,"content":output}]})
        }
    }
}

struct RequestOptions<'a> {
    instructions: Option<&'a str>,
    reasoning_effort: Option<&'a str>,
    cyber_program: Option<&'a str>,
}

fn tool_request_body(
    provider: Provider,
    shape: WireShape,
    model: &str,
    input: &[Value],
    tools: &[Value],
    options: RequestOptions<'_>,
) -> Value {
    let RequestOptions {
        instructions,
        reasoning_effort,
        cyber_program,
    } = options;
    match shape {
        WireShape::Responses => {
            let mut body = json!({
            "model":model,"input":input,"tools":tools,"store":false,"stream":true,
            "include":["reasoning.encrypted_content"]
            });
            if provider == Provider::OpenAi {
                if !tools.is_empty() {
                    let tools = tools
                        .iter()
                        .map(|tool| {
                            let mut tool = tool.clone();
                            tool["strict"] = json!(false);
                            tool
                        })
                        .collect::<Vec<_>>();
                    body["tools"] = json!([{"type":"namespace","name":"beale","description":"Beale host tools for authorized research work.","tools":tools}]);
                }
                body["reasoning"] = json!({"summary":"auto"});
                body["context_management"] =
                    json!([{"type":"compaction","compact_threshold":100_000}]);
                if let Some(instructions) = instructions {
                    body["instructions"] = json!(instructions);
                }
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
            let mut body = json!({"model":model,"messages":input,"tools":chat_tools,"stream":true});
            if provider != Provider::Custom {
                body["stream_options"] = json!({"include_usage":true});
            }
            if provider == Provider::Custom
                && let Some(effort) = reasoning_effort
            {
                body["reasoning_effort"] = json!(effort);
            }
            body
        }
        WireShape::Anthropic => {
            let system = input
                .iter()
                .filter(|message| message["role"] == "system")
                .filter_map(|message| message["content"].as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            let messages: Vec<_> = input
                .iter()
                .filter(|message| message["role"] != "system")
                .cloned()
                .collect();
            let native_tools: Vec<Value> = tools.iter().map(|tool| json!({
                "name":tool["name"],"description":tool["description"],"input_schema":tool["parameters"]
            })).collect();
            json!({"model":model,"system":system,"messages":messages,"tools":native_tools,"max_tokens":8192,"stream":true})
        }
    }
}

fn route(provider: Provider, mode: AuthMode) -> Route {
    match (provider, mode) {
        (Provider::OpenAi, AuthMode::ApiKey) => Route {
            endpoint: "https://api.openai.com/v1/responses",
            shape: WireShape::Responses,
            oauth_name: "openai",
            api_key_env: "OPENAI_API_KEY",
            proxy_header: false,
        },
        (Provider::OpenAi, AuthMode::Subscription) => Route {
            endpoint: "https://chatgpt.com/backend-api/codex/responses",
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
        (Provider::Anthropic, _) => Route {
            endpoint: "https://api.anthropic.com/v1/messages",
            shape: WireShape::Anthropic,
            oauth_name: "anthropic",
            api_key_env: "ANTHROPIC_API_KEY",
            proxy_header: false,
        },
        (Provider::Zai, _) => Route {
            endpoint: "https://api.z.ai/api/paas/v4/chat/completions",
            shape: WireShape::Chat,
            oauth_name: "zai",
            api_key_env: "ZAI_API_KEY",
            proxy_header: false,
        },
        (Provider::OpenRouter, _) => Route {
            endpoint: "https://openrouter.ai/api/v1/chat/completions",
            shape: WireShape::Chat,
            oauth_name: "openrouter",
            api_key_env: "OPENROUTER_API_KEY",
            proxy_header: false,
        },
        (Provider::Custom, _) => Route {
            endpoint: "",
            shape: WireShape::Chat,
            oauth_name: "custom",
            api_key_env: "",
            proxy_header: false,
        },
    }
}

pub struct ProviderClient {
    http: reqwest::Client,
}

/// Connection-local Responses continuation. A disconnected socket cannot reuse a
/// response ID with `store: false`, so the caller must keep the complete input.
#[derive(Default)]
pub struct OpenAiConnection {
    socket: Option<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    previous_response_id: Option<String>,
    known_input_len: usize,
}

fn websocket_body(mut body: Value, input: &[Value], previous_response_id: Option<&str>) -> Value {
    body["type"] = json!("response.create");
    if let Some(fields) = body.as_object_mut() {
        fields.remove("stream");
    }
    if let Some(id) = previous_response_id {
        body["previous_response_id"] = json!(id);
    }
    body["input"] = json!(input);
    body
}

fn openai_socket_request(token: &str) -> Result<tokio_tungstenite::tungstenite::http::Request<()>> {
    let mut request = "wss://api.openai.com/v1/responses".into_client_request()?;
    request.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}"))?,
    );
    Ok(request)
}

fn recoverable_socket_error(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<ProviderFailure>()
        .is_some_and(|failure| {
            failure.code.as_deref() == Some("previous_response_not_found")
                || matches!(failure.kind, FailureKind::StreamEnded)
        })
        || error
            .downcast_ref::<tokio_tungstenite::tungstenite::Error>()
            .is_some()
}

async fn read_openai_socket<S>(
    socket: &mut WebSocketStream<S>,
    progress: Option<&mpsc::Sender<ProviderProgress>>,
    requested_cyber_program: Option<&str>,
) -> Result<ProviderStep>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let events = futures_util::stream::unfold(
        (socket, requested_cyber_program),
        |(socket, requested_cyber_program)| async move {
            loop {
                let event = socket.next().await?;
                match event {
                    Ok(Message::Text(value)) => {
                        if let Ok(event) = serde_json::from_str::<Value>(&value)
                            && matches!(
                                event["type"].as_str(),
                                Some("response.created" | "response.completed")
                            )
                            && let Some(selected) =
                                event["response"]["access_programs"]["cyber"].as_str()
                            && Some(selected) != requested_cyber_program
                        {
                            return Some((
                                Err(DaybreakSelectionError {
                                    requested: requested_cyber_program.unwrap_or("none").to_owned(),
                                    selected: safe_code(Some(selected))
                                        .unwrap_or_else(|| "unknown".to_owned()),
                                }
                                .into()),
                                (socket, requested_cyber_program),
                            ));
                        }
                        return Some((
                            Ok(format!("data: {value}\n\n").into_bytes()),
                            (socket, requested_cyber_program),
                        ));
                    }
                    Ok(Message::Close(_)) => return None,
                    Ok(_) => continue,
                    Err(error) => {
                        return Some((
                            Err(anyhow::Error::from(error)),
                            (socket, requested_cyber_program),
                        ));
                    }
                }
            }
        },
    );
    read_tool_events(events, WireShape::Responses, progress).await
}

fn codex_request_body(request: ToolStepRequest<'_>, cyber_program: Option<&str>) -> Value {
    let mut body = tool_request_body(
        Provider::OpenAi,
        WireShape::Responses,
        request.model,
        request.input,
        request.tools,
        RequestOptions {
            instructions: request.instructions,
            reasoning_effort: request.reasoning_effort,
            cyber_program,
        },
    );
    // Match v1's Codex Responses wire shape. The Codex backend expects plain
    // function tools and a complete input on every request.
    body["tools"] = json!(
        request
            .tools
            .iter()
            .map(|tool| {
                let mut tool = tool.clone();
                tool["strict"] = Value::Null;
                tool
            })
            .collect::<Vec<_>>()
    );
    body["instructions"] = json!(
        request
            .instructions
            .unwrap_or("You are a helpful assistant.")
    );
    body["text"] = json!({"verbosity":"low"});
    body["tool_choice"] = json!("auto");
    body["parallel_tool_calls"] = json!(true);
    if let Some(fields) = body.as_object_mut() {
        fields.remove("context_management");
    }
    body
}

impl ProviderClient {
    pub async fn openai_tool_step(
        &self,
        mode: AuthMode,
        request: ToolStepRequest<'_>,
        store: &CredentialStore,
        progress: Option<&mpsc::Sender<ProviderProgress>>,
        connection: &mut OpenAiConnection,
    ) -> Result<ProviderStep> {
        ensure!(
            !request.model.is_empty() && request.model.len() <= 200,
            "invalid model name"
        );
        ensure!(!request.input.is_empty(), "empty provider input");
        if mode == AuthMode::Subscription {
            return self.codex_tool_step(request, store, progress).await;
        }
        let token = self.token(Provider::OpenAi, mode, store).await?;
        let cyber_program = request_cyber_program(Provider::OpenAi, request.model, store)?;
        for attempt in 0..2 {
            if connection.socket.is_none() {
                let handshake = openai_socket_request(&token)?;
                let (socket, _) = tokio::time::timeout(
                    std::time::Duration::from_secs(15),
                    connect_async(handshake),
                )
                .await
                .context("OpenAI WebSocket connection timed out")?
                .map_err(|error| match error {
                    tokio_tungstenite::tungstenite::Error::Http(response) => {
                        anyhow::Error::new(rejected_failure(
                            response.status().as_u16(),
                            response.body().as_deref().unwrap_or_default(),
                        ))
                    }
                    other => other.into(),
                })?;
                connection.socket = Some(socket);
                connection.previous_response_id = None;
                connection.known_input_len = 0;
            }
            let incremental = connection.previous_response_id.is_some()
                && connection.known_input_len <= request.input.len();
            let new_input = if incremental {
                &request.input[connection.known_input_len..]
            } else {
                request.input
            };
            let body = tool_request_body(
                Provider::OpenAi,
                WireShape::Responses,
                request.model,
                new_input,
                request.tools,
                RequestOptions {
                    instructions: request.instructions,
                    reasoning_effort: request.reasoning_effort,
                    cyber_program,
                },
            );
            let payload = websocket_body(
                body,
                new_input,
                if incremental {
                    connection.previous_response_id.as_deref()
                } else {
                    None
                },
            );
            let socket = connection
                .socket
                .as_mut()
                .context("OpenAI WebSocket is unavailable")?;
            if socket
                .send(Message::Text(payload.to_string().into()))
                .await
                .is_err()
            {
                connection.socket = None;
                connection.previous_response_id = None;
                if attempt == 0 {
                    continue;
                }
                bail!("OpenAI WebSocket send failed");
            }
            let step = tokio::time::timeout(
                std::time::Duration::from_secs(120),
                read_openai_socket(socket, progress, cyber_program),
            )
            .await
            .context("OpenAI response timed out")?;
            match step {
                Ok(step) => {
                    connection.known_input_len = request.input.len() + step.continuation.len();
                    connection.previous_response_id = step.response_id.clone();
                    return Ok(step);
                }
                Err(error) => {
                    let recoverable = recoverable_socket_error(&error);
                    connection.socket = None;
                    connection.previous_response_id = None;
                    if recoverable && attempt == 0 {
                        continue;
                    }
                    return Err(error);
                }
            }
        }
        bail!("OpenAI WebSocket recovery failed")
    }

    async fn codex_tool_step(
        &self,
        request: ToolStepRequest<'_>,
        store: &CredentialStore,
        progress: Option<&mpsc::Sender<ProviderProgress>>,
    ) -> Result<ProviderStep> {
        let credential = crate::codex::credential()?;
        let cyber_program = request_cyber_program(Provider::OpenAi, request.model, store)?;
        let body = codex_request_body(request, cyber_program);
        let response = self
            .http
            .post(route(Provider::OpenAi, AuthMode::Subscription).endpoint)
            .bearer_auth(&credential.access_token)
            .header("ChatGPT-Account-Id", &credential.account_id)
            .header("originator", "pi")
            .header("OpenAI-Beta", "responses=experimental")
            .header("Accept", "text/event-stream")
            .json(&body)
            .send()
            .await
            .context("Codex provider request failed")?;
        if !response.status().is_success() {
            return Err(rejected_response(response).await.into());
        }
        read_tool_stream_with_program(response, WireShape::Responses, progress, cyber_program).await
    }

    fn custom_request(
        &self,
        model: &str,
        store: &CredentialStore,
    ) -> Result<(String, String, Option<String>)> {
        let (id, model_id) = crate::split_qualified_model(model)?;
        let definition = store
            .custom_provider(id)?
            .context("unknown custom provider")?;
        ensure!(
            definition.models.iter().any(|entry| entry.id == model_id),
            "unknown custom provider model"
        );
        let token = store.load_api_key(id)?;
        Ok((
            format!("{}/chat/completions", definition.base_url),
            model_id.to_owned(),
            token,
        ))
    }

    pub fn new() -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .redirect(reqwest::redirect::Policy::none())
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
        ensure!(
            mode == AuthMode::ApiKey || matches!(provider, Provider::OpenAi | Provider::Xai),
            "this provider supports API keys only"
        );
        ensure!(
            provider != Provider::Custom,
            "custom providers need model-scoped credentials"
        );
        let route = route(provider, mode);
        let token = match mode {
            AuthMode::ApiKey => store
                .load_api_key(route.oauth_name)?
                .or_else(|| std::env::var(route.api_key_env).ok())
                .with_context(|| format!("{} is unavailable", route.api_key_env))?,
            AuthMode::Subscription => {
                if provider == Provider::OpenAi {
                    bail!("OpenAI subscription requires the Codex adapter");
                }
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
        if matches!(provider, Provider::Anthropic | Provider::Zai) && mode == AuthMode::Subscription
        {
            let input = input_messages(&request.messages);
            let step = ToolStepRequest {
                model: &request.model,
                input: &input,
                tools: &[],
                instructions: None,
                reasoning_effort: None,
                workspace_root: None,
            };
            return Ok(if provider == Provider::Anthropic {
                crate::subscription::claude_step(step).await?.text
            } else {
                crate::subscription::zcode_step(step).await?.text
            });
        }
        ensure!(
            !request.model.trim().is_empty() && request.model.len() <= 200,
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
        if provider == Provider::OpenAi {
            let instructions = request
                .messages
                .iter()
                .filter(|message| matches!(message.role, TextRole::System))
                .map(|message| message.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            let input = request
                .messages
                .iter()
                .filter(|message| !matches!(message.role, TextRole::System))
                .map(|message| json!({"role":message.role,"content":message.text}))
                .collect::<Vec<_>>();
            let mut connection = OpenAiConnection::default();
            return Ok(self
                .openai_tool_step(
                    mode,
                    ToolStepRequest {
                        model: &request.model,
                        input: &input,
                        tools: &[],
                        instructions: (!instructions.is_empty()).then_some(&instructions),
                        reasoning_effort: None,
                        workspace_root: None,
                    },
                    store,
                    None,
                    &mut connection,
                )
                .await?
                .text);
        }
        let route = route(provider, mode);
        let (endpoint, model, token) = if provider == Provider::Custom {
            ensure!(
                mode == AuthMode::ApiKey,
                "custom providers support API keys only"
            );
            self.custom_request(&request.model, store)?
        } else {
            (
                route.endpoint.to_owned(),
                request.model.clone(),
                Some(self.token(provider, mode, store).await?),
            )
        };
        let messages: Vec<Value> = request
            .messages
            .iter()
            .map(|message| json!({"role": message.role, "content": message.text}))
            .collect();
        let cyber_program = request_cyber_program(provider, &request.model, store)?;
        let mut body = match route.shape {
            WireShape::Responses => json!({
                "model": model,
                "input": messages,
                "store": false,
                "stream": true,
            }),
            WireShape::Chat => json!({
                "model": model,
                "messages": messages,
                "stream": true,
            }),
            WireShape::Anthropic => {
                let system = messages
                    .iter()
                    .filter(|message| message["role"] == "system")
                    .filter_map(|message| message["content"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let messages: Vec<_> = messages
                    .into_iter()
                    .filter(|message| message["role"] != "system")
                    .collect();
                json!({"model":model,"system":system,"messages":messages,"max_tokens":8192,"stream":true})
            }
        };
        add_cyber_access(&mut body, cyber_program);
        let mut builder = self.http.post(endpoint).json(&body);
        if let Some(token) = token {
            builder = builder.bearer_auth(token);
        }
        if route.proxy_header {
            builder = builder
                .header("X-XAI-Token-Auth", "xai-grok-cli")
                .header("x-grok-client-version", "1.0.13")
                .header("x-grok-client-identifier", "beale")
                .header("x-grok-client-mode", "headless")
                .header("x-grok-model-override", request.model.as_str());
        }
        if provider == Provider::Anthropic {
            builder = builder.header("anthropic-version", "2026-10-21");
            if let Ok(workspace) = std::env::var("ANTHROPIC_WORKSPACE_ID") {
                builder = builder.header("anthropic-workspace-id", workspace);
            }
        }
        let response = builder.send().await.context("provider request failed")?;
        if !response.status().is_success() {
            return Err(rejected_response(response).await.into());
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
        if provider == Provider::OpenAi {
            return self
                .openai_tool_step(
                    mode,
                    request,
                    store,
                    progress,
                    &mut OpenAiConnection::default(),
                )
                .await;
        }
        if mode == AuthMode::Subscription {
            match provider {
                Provider::Anthropic => return crate::subscription::claude_step(request).await,
                Provider::Zai => return crate::subscription::zcode_step(request).await,
                _ => {}
            }
        }
        ensure!(
            !request.model.is_empty() && request.model.len() <= 200,
            "invalid model name"
        );
        ensure!(!request.input.is_empty(), "empty provider input");
        let route = route(provider, mode);
        let (endpoint, model, token) = if provider == Provider::Custom {
            ensure!(
                mode == AuthMode::ApiKey,
                "custom providers support API keys only"
            );
            self.custom_request(request.model, store)?
        } else {
            (
                route.endpoint.to_owned(),
                request.model.to_owned(),
                Some(self.token(provider, mode, store).await?),
            )
        };
        let cyber_program = request_cyber_program(provider, request.model, store)?;
        let custom_reasoning = if provider == Provider::Custom {
            let (id, _) = crate::split_qualified_model(request.model)?;
            store
                .custom_provider(id)?
                .is_some_and(|definition| definition.reasoning)
        } else {
            false
        };
        let body = tool_request_body(
            provider,
            route.shape,
            &model,
            request.input,
            request.tools,
            RequestOptions {
                instructions: request.instructions,
                reasoning_effort: if provider == Provider::Custom && !custom_reasoning {
                    None
                } else {
                    request.reasoning_effort
                },
                cyber_program,
            },
        );
        let mut builder = self.http.post(endpoint).json(&body);
        if let Some(token) = token {
            builder = builder.bearer_auth(token);
        }
        if route.proxy_header {
            builder = builder
                .header("X-XAI-Token-Auth", "xai-grok-cli")
                .header("x-grok-client-version", "1.0.13")
                .header("x-grok-client-identifier", "beale")
                .header("x-grok-client-mode", "headless")
                .header("x-grok-model-override", request.model);
        }
        if provider == Provider::Anthropic {
            builder = builder.header("anthropic-version", "2026-10-21");
            if let Ok(workspace) = std::env::var("ANTHROPIC_WORKSPACE_ID") {
                builder = builder.header("anthropic-workspace-id", workspace);
            }
        }
        let response = builder.send().await.context("provider request failed")?;
        if !response.status().is_success() {
            return Err(rejected_response(response).await.into());
        }
        read_tool_stream(response, route.shape, progress).await
    }
}

async fn read_tool_stream(
    response: reqwest::Response,
    shape: WireShape,
    progress: Option<&mpsc::Sender<ProviderProgress>>,
) -> Result<ProviderStep> {
    read_tool_stream_with_program(response, shape, progress, None).await
}

async fn read_tool_stream_with_program(
    response: reqwest::Response,
    shape: WireShape,
    progress: Option<&mpsc::Sender<ProviderProgress>>,
    requested_cyber_program: Option<&str>,
) -> Result<ProviderStep> {
    if matches!(shape, WireShape::Anthropic) {
        return anthropic::read_stream(response, progress).await;
    }
    let stream = response.bytes_stream().map(|item| {
        item.map(|bytes| bytes.to_vec())
            .map_err(anyhow::Error::from)
    });
    read_tool_events_with_program(stream, shape, progress, requested_cyber_program).await
}

async fn read_tool_events<S>(
    stream: S,
    shape: WireShape,
    progress: Option<&mpsc::Sender<ProviderProgress>>,
) -> Result<ProviderStep>
where
    S: Stream<Item = Result<Vec<u8>>>,
{
    read_tool_events_with_program(stream, shape, progress, None).await
}

async fn read_tool_events_with_program<S>(
    stream: S,
    shape: WireShape,
    progress: Option<&mpsc::Sender<ProviderProgress>>,
    requested_cyber_program: Option<&str>,
) -> Result<ProviderStep>
where
    S: Stream<Item = Result<Vec<u8>>>,
{
    futures_util::pin_mut!(stream);
    let mut pending = Vec::new();
    let mut text = String::new();
    let mut completed = false;
    let mut response_output: Option<Vec<Value>> = None;
    let mut response_id = None;
    let mut response_items = BTreeMap::<usize, Value>::new();
    let mut chat_calls = BTreeMap::<usize, PendingCall>::new();
    let mut reasoning_parts = BTreeMap::<(u64, u64), String>::new();
    let mut sent_reasoning = HashSet::<(u64, u64)>::new();
    let mut sent_text = false;
    let mut usage = None;
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
            let mut event_type = "";
            let mut data = "";
            for line in frame.lines() {
                let line = line.trim_end_matches('\r');
                if let Some(value) = line.strip_prefix("event: ") {
                    event_type = value;
                }
                if let Some(value) = line.strip_prefix("data: ") {
                    data = value;
                }
            }
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
                    if matches!(
                        event["type"].as_str(),
                        Some("response.created" | "response.completed" | "response.done")
                    ) && let Some(selected) = event
                        .pointer("/response/access_programs/cyber")
                        .and_then(Value::as_str)
                        && Some(selected) != requested_cyber_program
                    {
                        return Err(DaybreakSelectionError {
                            requested: requested_cyber_program.unwrap_or("none").to_owned(),
                            selected: safe_code(Some(selected))
                                .unwrap_or_else(|| "unknown".to_owned()),
                        }
                        .into());
                    }
                    match event
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or(event_type)
                    {
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
                            if reasoning_parts.contains_key(&key)
                                || reasoning_parts.len() < MAX_REASONING_SUMMARIES
                            {
                                let part = reasoning_parts.entry(key).or_default();
                                if part.len().saturating_add(delta.len())
                                    <= MAX_REASONING_SUMMARY_BYTES
                                {
                                    part.push_str(delta);
                                    if !delta.is_empty() {
                                        sent_reasoning.insert(key);
                                    }
                                    if !delta.is_empty()
                                        && let Some(tx) = progress
                                    {
                                        let _ = tx
                                            .send(ProviderProgress::ReasoningDelta(
                                                delta.to_owned(),
                                                key,
                                            ))
                                            .await;
                                    }
                                }
                            }
                        }
                        "response.reasoning_summary_text.done"
                        | "response.reasoning_summary_part.done" => {
                            let key = summary_key(&event)?;
                            if let Some(value) = event
                                .get("text")
                                .or_else(|| event.pointer("/part/text"))
                                .and_then(Value::as_str)
                                && value.len() <= MAX_REASONING_SUMMARY_BYTES
                                && (reasoning_parts.contains_key(&key)
                                    || reasoning_parts.len() < MAX_REASONING_SUMMARIES)
                            {
                                reasoning_parts.insert(key, value.to_owned());
                                if !value.is_empty()
                                    && sent_reasoning.insert(key)
                                    && let Some(tx) = progress
                                {
                                    let _ = tx
                                        .send(ProviderProgress::ReasoningDelta(
                                            value.to_owned(),
                                            key,
                                        ))
                                        .await;
                                }
                            }
                        }
                        "response.output_item.done" => {
                            if let Some(item) = event.get("item") {
                                let index = event
                                    .get("output_index")
                                    .and_then(Value::as_u64)
                                    .context("missing provider output index")?;
                                let index = usize::try_from(index)
                                    .context("invalid provider output index")?;
                                ensure!(
                                    index < 32
                                        && (response_items.contains_key(&index)
                                            || response_items.len() < 32),
                                    "too many provider output items"
                                );
                                response_items.insert(index, item.clone());
                            }
                        }
                        "response.completed" | "response.done" => {
                            match event.pointer("/response/status").and_then(Value::as_str) {
                                Some("failed") => {
                                    return Err(
                                        terminal_failure(&event, FailureKind::Failed).into()
                                    );
                                }
                                Some("incomplete") => {
                                    return Err(
                                        terminal_failure(&event, FailureKind::Incomplete).into()
                                    );
                                }
                                _ => {}
                            }
                            usage = event.pointer("/response/usage").and_then(|value| {
                                token_usage_with_cache(
                                    value,
                                    "input_tokens",
                                    "output_tokens",
                                    "/input_tokens_details/cached_tokens",
                                )
                            });
                            response_id = event
                                .pointer("/response/id")
                                .and_then(Value::as_str)
                                .filter(|id| id.len() <= 256 && id.is_ascii())
                                .map(str::to_owned);
                            response_output = event
                                .pointer("/response/output")
                                .and_then(Value::as_array)
                                .cloned();
                            completed = true;
                            break;
                        }
                        "response.failed" | "error" => {
                            return Err(terminal_failure(&event, FailureKind::Failed).into());
                        }
                        "response.incomplete" => {
                            return Err(terminal_failure(&event, FailureKind::Incomplete).into());
                        }
                        _ => {}
                    }
                }
                WireShape::Chat => {
                    if let Some(value) = event.get("usage") {
                        usage = token_usage_with_cache(
                            value,
                            "prompt_tokens",
                            "completion_tokens",
                            "/prompt_tokens_details/cached_tokens",
                        )
                        .or(usage);
                    }
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
                WireShape::Anthropic => unreachable!(),
            }
        }
        if completed {
            break;
        }
    }
    if !completed {
        return Err(ProviderFailure {
            kind: FailureKind::StreamEnded,
            code: None,
            param: None,
        }
        .into());
    }
    match shape {
        WireShape::Responses => {
            let output = merge_response_output(response_output, response_items)?;
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
                    for (index, summary) in reasoning_summaries.iter().enumerate() {
                        let _ = tx
                            .send(ProviderProgress::ReasoningDelta(
                                summary.clone(),
                                (0, index as u64),
                            ))
                            .await;
                    }
                }
            }
            Ok(ProviderStep {
                text,
                calls,
                reasoning_summaries,
                usage,
                continuation: output,
                response_id,
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
                usage,
                continuation,
                response_id: None,
            })
        }
        WireShape::Anthropic => unreachable!(),
    }
}

fn merge_response_output(
    completed: Option<Vec<Value>>,
    mut finished: BTreeMap<usize, Value>,
) -> Result<Vec<Value>> {
    let completed = completed.unwrap_or_default();
    let count = completed
        .len()
        .max(finished.last_key_value().map_or(0, |(index, _)| index + 1));
    ensure!(count <= 32, "too many provider output items");
    (0..count)
        .map(|index| {
            finished
                .remove(&index)
                .or_else(|| completed.get(index).cloned())
                .context("missing provider output item")
        })
        .collect()
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
                    let key = (output_index as u64, summary_index as u64);
                    if value.len() <= MAX_REASONING_SUMMARY_BYTES
                        && (streamed.contains_key(&key) || streamed.len() < MAX_REASONING_SUMMARIES)
                    {
                        streamed.insert(key, value.to_owned());
                    }
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
            ensure!(
                item.get("namespace")
                    .and_then(Value::as_str)
                    .is_none_or(|value| value == "beale"),
                "unexpected provider tool namespace"
            );
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
    if matches!(shape, WireShape::Anthropic) {
        return Ok(anthropic::read_stream(response, None).await?.text);
    }
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
    if !completed {
        return Err(ProviderFailure {
            kind: FailureKind::StreamEnded,
            code: None,
            param: None,
        }
        .into());
    }
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
                    "response.failed" | "error" => {
                        return Err(terminal_failure(&event, FailureKind::Failed).into());
                    }
                    "response.incomplete" => {
                        return Err(terminal_failure(&event, FailureKind::Incomplete).into());
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
            WireShape::Anthropic => unreachable!(),
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
    fn token_usage_accepts_measured_counts_only() {
        assert_eq!(
            token_usage(
                &json!({"input_tokens": 120, "output_tokens": 30}),
                "input_tokens",
                "output_tokens"
            ),
            Some(TokenUsage {
                input_tokens: 120,
                output_tokens: 30,
                cache_read_tokens: None,
            })
        );
        assert!(
            token_usage(
                &json!({"input_tokens": 120}),
                "input_tokens",
                "output_tokens"
            )
            .is_none()
        );
        assert!(
            token_usage(
                &json!({"input_tokens": -1, "output_tokens": 30}),
                "input_tokens",
                "output_tokens"
            )
            .is_none()
        );
        assert!(
            token_usage(
                &json!({"input_tokens": 1_000_000_000_001_u64, "output_tokens": 30}),
                "input_tokens",
                "output_tokens"
            )
            .is_none()
        );
    }

    #[test]
    fn cache_usage_requires_a_reported_count_within_input_tokens() {
        let usage = json!({
            "input_tokens": 120,
            "output_tokens": 30,
            "input_tokens_details": {"cached_tokens": 41}
        });
        assert_eq!(
            token_usage_with_cache(
                &usage,
                "input_tokens",
                "output_tokens",
                "/input_tokens_details/cached_tokens"
            )
            .unwrap()
            .cache_read_tokens,
            Some(41)
        );
        let invalid = json!({
            "prompt_tokens": 120,
            "completion_tokens": 30,
            "prompt_tokens_details": {"cached_tokens": 121}
        });
        assert_eq!(
            token_usage_with_cache(
                &invalid,
                "prompt_tokens",
                "completion_tokens",
                "/prompt_tokens_details/cached_tokens"
            )
            .unwrap()
            .cache_read_tokens,
            None
        );
    }

    #[test]
    fn routes_keep_subscription_paths_separate() {
        assert_eq!(
            route(Provider::OpenAi, AuthMode::Subscription).endpoint,
            "https://chatgpt.com/backend-api/codex/responses"
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
    fn reviewer_retries_transient_provider_failures_only() {
        for status in [408, 429, 500, 503] {
            let error: anyhow::Error = ProviderFailure {
                kind: FailureKind::Http(status),
                code: None,
                param: None,
            }
            .into();
            assert!(provider_error_retryable(&error));
        }
        for status in [400, 401, 403] {
            let error: anyhow::Error = ProviderFailure {
                kind: FailureKind::Http(status),
                code: None,
                param: None,
            }
            .into();
            assert!(!provider_error_retryable(&error));
        }
        let websocket_closed: anyhow::Error =
            tokio_tungstenite::tungstenite::Error::ConnectionClosed.into();
        assert!(provider_error_retryable(&websocket_closed));
        let stream_ended: anyhow::Error = ProviderFailure {
            kind: FailureKind::StreamEnded,
            code: None,
            param: None,
        }
        .into();
        assert!(provider_error_retryable(&stream_ended));
    }

    #[test]
    fn responses_failures_keep_safe_codes_without_upstream_messages() {
        let mut output = String::new();
        let mut completed = false;
        let mut pending = b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"usage_limit_exceeded\",\"message\":\"private upstream detail\"}}}\n\n".to_vec();
        let error = process_frames(
            &mut pending,
            &mut output,
            &mut completed,
            WireShape::Responses,
        )
        .unwrap_err();
        assert_eq!(
            provider_error_message(Provider::OpenAi, AuthMode::Subscription, &error),
            "The provider ended the response with error code usage_limit_exceeded."
        );
        assert!(!error.to_string().contains("private upstream detail"));

        let mut pending = b"data: {\"type\":\"error\",\"code\":\"SERVER_ERROR\",\"message\":\"private upstream detail\"}\n\n".to_vec();
        let error = process_frames(
            &mut pending,
            &mut output,
            &mut completed,
            WireShape::Responses,
        )
        .unwrap_err();
        assert_eq!(
            provider_error_message(Provider::OpenAi, AuthMode::Subscription, &error),
            "The provider had a server error. Try again."
        );
        let error: anyhow::Error = terminal_failure(
            &json!({"type":"response.failed","response":{"error":{
                "code":"unsupported_capability",
                "param":"tools[0].type"
            }}}),
            FailureKind::Failed,
        )
        .into();
        assert_eq!(
            provider_error_message(Provider::OpenAi, AuthMode::Subscription, &error),
            "The provider ended the response with error code unsupported_capability."
        );
        let mut pending = b"event: response.incomplete\ndata: {\"response\":{\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n".to_vec();
        let error = process_frames(
            &mut pending,
            &mut output,
            &mut completed,
            WireShape::Responses,
        )
        .unwrap_err();
        assert_eq!(
            provider_error_message(Provider::OpenAi, AuthMode::Subscription, &error),
            "The provider stopped at its output limit."
        );
        let error: anyhow::Error = ProviderFailure {
            kind: FailureKind::Http(429),
            code: None,
            param: None,
        }
        .into();
        assert_eq!(
            provider_error_message(Provider::OpenAi, AuthMode::ApiKey, &error),
            "The provider rate limit was reached. Try again shortly."
        );
        assert!(safe_param(Some("private user prompt\nsecret")).is_none());
    }

    #[test]
    fn provider_protocol_errors_have_specific_sanitized_messages() {
        let missing: anyhow::Error = crate::codex::CodexSignInError.into();
        assert_eq!(
            provider_error_message(Provider::OpenAi, AuthMode::Subscription, &missing),
            "Sign in to Codex on this host, then retry."
        );
        let response = tokio_tungstenite::tungstenite::http::Response::builder()
            .status(403)
            .body(Some(Vec::new()))
            .unwrap();
        let error: anyhow::Error =
            tokio_tungstenite::tungstenite::Error::Http(Box::new(response)).into();
        assert_eq!(
            provider_error_message(Provider::OpenAi, AuthMode::Subscription, &error),
            "OpenAI denied this Codex request. Check the Codex account, model, and Daybreak access."
        );
        let denial = rejected_failure(
            403,
            br#"{"error":{"code":"access_program_not_enabled","message":"private upstream detail"}}"#,
        );
        assert_eq!(denial.code.as_deref(), Some("access_program_not_enabled"));
        assert!(!denial.to_string().contains("private upstream detail"));
        assert_eq!(
            provider_error_message(
                Provider::OpenAi,
                AuthMode::Subscription,
                &anyhow::Error::new(denial)
            ),
            "Daybreak access is not enabled for this Codex account and selected model."
        );
        let error = anyhow::anyhow!("too many provider output items");
        assert_eq!(
            provider_error_message(Provider::OpenAi, AuthMode::ApiKey, &error),
            "The provider returned more items than the current protocol supports."
        );
        let error = anyhow::anyhow!("OpenAI response timed out");
        assert_eq!(
            provider_error_message(Provider::OpenAi, AuthMode::Subscription, &error),
            "OpenAI timed out before completing this turn."
        );
        let error = anyhow::anyhow!("invalid provider event JSON");
        assert_eq!(
            provider_error_message(Provider::OpenAi, AuthMode::ApiKey, &error),
            "The provider returned an invalid response event."
        );
        let input_error: anyhow::Error = terminal_failure(
            &json!({"type":"response.failed","response":{"error":{"param":"input","message":"private upstream detail"}}}),
            FailureKind::Failed,
        )
        .into();
        assert_eq!(
            provider_error_message(Provider::OpenAi, AuthMode::Subscription, &input_error),
            "OpenAI rejected this session's conversation input. Retry the turn; if it persists, start a new session."
        );
        assert!(!input_error.to_string().contains("private upstream detail"));
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
            RequestOptions {
                instructions: None,
                reasoning_effort: Some("high"),
                cyber_program: Some("daybreak_blue"),
            },
        );
        assert_eq!(responses["tools"][0]["name"], "beale");
        assert_eq!(responses["tools"][0]["tools"][0]["name"], "read");
        assert_eq!(responses["tools"][0]["tools"][0]["strict"], false);
        assert_eq!(responses["store"], false);
        assert_eq!(responses["input"], json!(input));
        assert_eq!(responses["reasoning"]["summary"], "auto");
        assert_eq!(responses["reasoning"]["effort"], "high");
        assert_eq!(responses["context_management"][0]["type"], "compaction");
        assert_eq!(
            responses["context_management"][0]["compact_threshold"],
            100_000
        );
        assert_eq!(responses["access_programs"]["cyber"], "daybreak_blue");
        let xai = tool_request_body(
            Provider::Xai,
            WireShape::Responses,
            "model-example",
            &input,
            &tools,
            RequestOptions {
                instructions: None,
                reasoning_effort: Some("high"),
                cyber_program: None,
            },
        );
        assert!(xai.get("reasoning").is_none());
        assert!(xai["tools"][0].get("strict").is_none());
        assert!(xai.get("access_programs").is_none());
        let chat = tool_request_body(
            Provider::Xai,
            WireShape::Chat,
            "model-example",
            &input,
            &tools,
            RequestOptions {
                instructions: None,
                reasoning_effort: Some("high"),
                cyber_program: None,
            },
        );
        assert_eq!(chat["tools"][0]["function"]["name"], "read");
        assert_eq!(chat["messages"], json!(input));
        assert!(chat.get("access_programs").is_none());
        let custom = tool_request_body(
            Provider::Custom,
            WireShape::Chat,
            "model-example",
            &input,
            &tools,
            RequestOptions {
                instructions: None,
                reasoning_effort: Some("high"),
                cyber_program: None,
            },
        );
        assert_eq!(custom["reasoning_effort"], "high");
        let anthropic = tool_request_body(
            Provider::Anthropic,
            WireShape::Anthropic,
            "claude-example",
            &[
                json!({"role":"system","content":"Example instruction"}),
                json!({"role":"user","content":"Read example.txt"}),
            ],
            &tools,
            RequestOptions {
                instructions: None,
                reasoning_effort: Some("high"),
                cyber_program: None,
            },
        );
        assert_eq!(anthropic["system"], "Example instruction");
        assert_eq!(anthropic["messages"].as_array().unwrap().len(), 1);
        assert_eq!(
            anthropic["tools"][0]["input_schema"],
            json!({"type":"object"})
        );
        assert_eq!(
            tool_output(
                Provider::Anthropic,
                AuthMode::ApiKey,
                "tool-example",
                "Example result"
            ),
            json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"tool-example","content":"Example result"}]})
        );
    }

    #[test]
    fn websocket_continuation_uses_only_new_items_and_preserves_store_false() {
        let base = json!({"model":"model-example","input":[{"role":"user","content":"First"}],"store":false,"stream":true});
        let full = websocket_body(
            base.clone(),
            &[json!({"role":"user","content":"First"})],
            None,
        );
        assert_eq!(full["type"], "response.create");
        assert_eq!(full["store"], false);
        assert!(full.get("stream").is_none());
        assert!(full.get("previous_response_id").is_none());
        let next = websocket_body(
            base,
            &[json!({"role":"user","content":"Second"})],
            Some("resp-example"),
        );
        assert_eq!(next["previous_response_id"], "resp-example");
        assert_eq!(next["input"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn openai_subscription_socket_uses_public_responses_route() {
        let request = openai_socket_request("synthetic-token").unwrap();
        assert_eq!(
            request.uri().to_string(),
            "wss://api.openai.com/v1/responses"
        );
        assert_eq!(request.headers()["Authorization"], "Bearer synthetic-token");
        assert!(!request.headers().contains_key("chatgpt-account-id"));
        assert!(!request.headers().contains_key("originator"));
    }

    #[test]
    fn codex_request_uses_stateless_input_and_plain_tools() {
        let input = [json!({"role":"user","content":"Synthetic research question"})];
        let tools = [
            json!({"type":"function","name":"read_example","description":"Read synthetic data","parameters":{"type":"object","properties":{}}}),
        ];
        let body = codex_request_body(
            ToolStepRequest {
                model: "gpt-6-sol",
                input: &input,
                tools: &tools,
                instructions: Some("Synthetic instructions"),
                reasoning_effort: Some("high"),
                workspace_root: None,
            },
            Some("daybreak_blue"),
        );
        assert_eq!(body["input"], json!(input));
        assert_eq!(body["tools"][0]["name"], "read_example");
        assert!(body["tools"][0].get("namespace").is_none());
        assert!(body.get("context_management").is_none());
        assert_eq!(body["store"], false);
        assert_eq!(body["access_programs"]["cyber"], "daybreak_blue");
    }

    #[tokio::test]
    async fn codex_stream_rejects_a_downgraded_program() {
        let events = futures_util::stream::iter([Ok(b"data: {\"type\":\"response.done\",\"response\":{\"status\":\"completed\",\"access_programs\":{\"cyber\":\"standard\"},\"output\":[]}}\n\n".to_vec())]);
        let error = read_tool_events_with_program(
            events,
            WireShape::Responses,
            None,
            Some("daybreak_blue"),
        )
        .await
        .unwrap_err();
        assert!(error.downcast_ref::<DaybreakSelectionError>().is_some());
    }

    #[tokio::test]
    async fn codex_done_with_failed_status_is_not_a_success() {
        let events = futures_util::stream::iter([Ok(b"data: {\"type\":\"response.done\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"server_error\"}}}\n\n".to_vec())]);
        let error = read_tool_events_with_program(events, WireShape::Responses, None, None)
            .await
            .unwrap_err();
        assert!(error.downcast_ref::<ProviderFailure>().is_some());
    }

    #[test]
    fn openai_daybreak_blue_marker_reaches_each_websocket_request() {
        let input = vec![json!({"role":"user","content":"Review example.txt"})];
        let body = tool_request_body(
            Provider::OpenAi,
            WireShape::Responses,
            "gpt-5.6-sol",
            &input,
            &[],
            RequestOptions {
                instructions: None,
                reasoning_effort: None,
                cyber_program: Some(openai_cyber_program("gpt-5.6-sol", "blue").unwrap()),
            },
        );
        for previous_response_id in [None, Some("resp-example")] {
            let payload = websocket_body(body.clone(), &input, previous_response_id);
            let serialized: Value = serde_json::from_str(&payload.to_string()).unwrap();
            assert_eq!(serialized["type"], "response.create");
            assert_eq!(
                serialized["access_programs"],
                json!({"cyber":"daybreak_blue"})
            );
            assert_eq!(
                serialized
                    .get("previous_response_id")
                    .and_then(Value::as_str),
                previous_response_id
            );
        }
    }

    #[test]
    fn websocket_retries_transport_loss_but_not_provider_rejection() {
        let disconnected: anyhow::Error = ProviderFailure {
            kind: FailureKind::StreamEnded,
            code: None,
            param: None,
        }
        .into();
        assert!(recoverable_socket_error(&disconnected));
        let evicted: anyhow::Error = terminal_failure(
            &json!({"type":"error","error":{"code":"previous_response_not_found"}}),
            FailureKind::Failed,
        )
        .into();
        assert!(recoverable_socket_error(&evicted));
        let rejected: anyhow::Error = ProviderFailure {
            kind: FailureKind::Failed,
            code: Some("invalid_request_error".to_owned()),
            param: None,
        }
        .into();
        assert!(!recoverable_socket_error(&rejected));
    }

    #[tokio::test]
    async fn websocket_frames_stream_tool_calls_and_response_id() {
        let (client_io, server_io) = tokio::io::duplex(8192);
        let server = tokio::spawn(async move {
            let mut socket = WebSocketStream::from_raw_socket(
                server_io,
                tokio_tungstenite::tungstenite::protocol::Role::Server,
                None,
            )
            .await;
            socket
                .send(Message::Text(
                    json!({
                        "type":"response.output_item.done","output_index":0,
                        "item":{"type":"compaction","encrypted_content":"opaque-example"}
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            socket.send(Message::Text(json!({
                "type":"response.output_item.done","output_index":1,
                "item":{"type":"function_call","namespace":"beale","call_id":"call-example","name":"read","arguments":"{}"}
            }).to_string().into())).await.unwrap();
            socket
                .send(Message::Text(
                    json!({
                        "type":"response.completed","response":{"id":"resp-example","output":[]}
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
        });
        let mut client = WebSocketStream::from_raw_socket(
            client_io,
            tokio_tungstenite::tungstenite::protocol::Role::Client,
            None,
        )
        .await;
        let step = read_openai_socket(&mut client, None, None).await.unwrap();
        assert_eq!(step.response_id.as_deref(), Some("resp-example"));
        assert_eq!(step.calls[0].name, "read");
        assert_eq!(step.continuation.len(), 2);
        assert_eq!(step.continuation[0]["type"], "compaction");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn websocket_rejects_a_downgraded_daybreak_response() {
        let (client_io, server_io) = tokio::io::duplex(4096);
        let server = tokio::spawn(async move {
            let mut socket = WebSocketStream::from_raw_socket(
                server_io,
                tokio_tungstenite::tungstenite::protocol::Role::Server,
                None,
            )
            .await;
            socket
                .send(Message::Text(
                    json!({"type":"response.created","response":{"access_programs":{"cyber":"standard"}}})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
        });
        let mut client = WebSocketStream::from_raw_socket(
            client_io,
            tokio_tungstenite::tungstenite::protocol::Role::Client,
            None,
        )
        .await;
        let error = read_openai_socket(&mut client, None, Some("daybreak_blue"))
            .await
            .unwrap_err();
        assert!(error.downcast_ref::<DaybreakSelectionError>().is_some());
        assert!(
            provider_error_message(Provider::OpenAi, AuthMode::Subscription, &error)
                .contains("selected standard instead of daybreak_blue")
        );
        let interrupted = error.context("provider stream interrupted");
        assert!(
            provider_error_message(Provider::OpenAi, AuthMode::Subscription, &interrupted)
                .contains("selected standard instead of daybreak_blue")
        );
        server.await.unwrap();
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
        let error = openai_cyber_program("gpt-6.1-sol", "blue").unwrap_err();
        assert_eq!(
            provider_error_message(Provider::OpenAi, AuthMode::Subscription, &error),
            "This OpenAI model requires Daybreak Red. Choose another model or switch Daybreak Mode in Providers."
        );
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

    #[test]
    fn excessive_reasoning_parts_do_not_discard_a_valid_response() {
        let output = vec![json!({
            "type": "reasoning",
            "summary": (0..20)
                .map(|index| json!({"type": "summary_text", "text": format!("Example summary {index}")}))
                .collect::<Vec<_>>()
        })];
        let summaries = summaries_from_response(&output, BTreeMap::new()).unwrap();
        assert_eq!(summaries.len(), MAX_REASONING_SUMMARIES);
        assert_eq!(summaries[0], "Example summary 0");
    }

    #[tokio::test]
    async fn streamed_reasoning_beyond_display_limit_keeps_tool_response() {
        use axum::{body::Bytes, http::Response};
        use futures_util::stream;
        use std::convert::Infallible;

        let mut chunks = (0..20)
            .map(|index| {
                Ok::<_, Infallible>(Bytes::from(format!(
                    "data: {}\n\n",
                    json!({"type":"response.reasoning_summary_text.delta","output_index":0,
                        "summary_index":index,"delta":format!("Example summary {index}")})
                )))
            })
            .collect::<Vec<_>>();
        chunks.push(Ok(Bytes::from(format!(
            "data: {}\n\n",
            json!({"type":"response.completed","response":{"output":[
                {"type":"reasoning","summary":[]},
                {"type":"function_call","call_id":"call-example","name":"read",
                    "arguments":"{}"}
            ]}})
        ))));
        let response: reqwest::Response =
            Response::new(reqwest::Body::wrap_stream(stream::iter(chunks))).into();
        let step = read_tool_stream(response, WireShape::Responses, None)
            .await
            .unwrap();
        assert_eq!(step.reasoning_summaries.len(), MAX_REASONING_SUMMARIES);
        assert_eq!(step.calls.len(), 1);
    }

    #[tokio::test]
    async fn response_keeps_commentary_before_a_tool_call() {
        use axum::{body::Bytes, http::Response};
        use futures_util::stream;
        use std::convert::Infallible;

        let body = format!(
            "data: {}\n\ndata: {}\n\n",
            json!({"type":"response.output_text.delta","delta":"Checking the example file."}),
            json!({"type":"response.completed","response":{"output":[
                {"type":"message","content":[{"type":"output_text","text":"Checking the example file."}]},
                {"type":"function_call","call_id":"call-example","name":"read","arguments":"{}"}
            ]}}),
        );
        let chunks = vec![Ok::<Bytes, Infallible>(Bytes::from(body))];
        let response: reqwest::Response =
            Response::new(reqwest::Body::wrap_stream(stream::iter(chunks))).into();
        let step = read_tool_stream(response, WireShape::Responses, None)
            .await
            .unwrap();
        assert_eq!(step.text, "Checking the example file.");
        assert_eq!(step.calls.len(), 1);
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
            matches!(first, ProviderProgress::ReasoningDelta(value, _) if value == "Checking example")
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
    async fn reasoning_parts_keep_distinct_stream_keys() {
        use axum::{body::Bytes, http::Response};
        use futures_util::stream;
        use std::convert::Infallible;

        let chunks = stream::iter([
            Ok::<_, Infallible>(Bytes::from_static(b"data: {\"type\":\"response.reasoning_summary_text.delta\",\"output_index\":0,\"summary_index\":0,\"delta\":\"First \"}\n\n")),
            Ok(Bytes::from_static(b"data: {\"type\":\"response.reasoning_summary_text.delta\",\"output_index\":0,\"summary_index\":0,\"delta\":\"summary\"}\n\n")),
            Ok(Bytes::from_static(b"data: {\"type\":\"response.reasoning_summary_text.delta\",\"output_index\":0,\"summary_index\":1,\"delta\":\"Second summary\"}\n\n")),
            Ok(Bytes::from_static(b"data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"reasoning\",\"summary\":[{\"text\":\"First summary\"},{\"text\":\"Second summary\"}]}]}}\n\n")),
        ]);
        let response: reqwest::Response = Response::new(reqwest::Body::wrap_stream(chunks)).into();
        let (tx, mut rx) = mpsc::channel(8);
        let step = read_tool_stream(response, WireShape::Responses, Some(&tx))
            .await
            .unwrap();
        assert_eq!(
            step.reasoning_summaries,
            ["First summary", "Second summary"]
        );
        assert!(
            matches!(rx.recv().await, Some(ProviderProgress::ReasoningDelta(text, (0, 0))) if text == "First ")
        );
        assert!(
            matches!(rx.recv().await, Some(ProviderProgress::ReasoningDelta(text, (0, 0))) if text == "summary")
        );
        assert!(
            matches!(rx.recv().await, Some(ProviderProgress::ReasoningDelta(text, (0, 1))) if text == "Second summary")
        );
    }

    #[tokio::test]
    async fn streamed_reasoning_can_be_followed_by_a_specific_failure() {
        use axum::{body::Bytes, http::Response};
        use futures_util::stream;
        use std::convert::Infallible;

        let chunks = stream::iter([
            Ok::<_, Infallible>(Bytes::from_static(
                b"data: {\"type\":\"response.reasoning_summary_text.delta\",\"output_index\":0,\"summary_index\":0,\"delta\":\"Checking example\"}\n\n",
            )),
            Ok(Bytes::from_static(
                b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"server_error\",\"message\":\"private upstream detail\"}}}\n\n",
            )),
        ]);
        let response: reqwest::Response = Response::new(reqwest::Body::wrap_stream(chunks)).into();
        let (tx, mut rx) = mpsc::channel(8);
        let result = read_tool_stream(response, WireShape::Responses, Some(&tx)).await;
        assert!(
            matches!(rx.recv().await, Some(ProviderProgress::ReasoningDelta(value, _)) if value == "Checking example")
        );
        let error = result.unwrap_err();
        assert_eq!(
            provider_error_message(Provider::OpenAi, AuthMode::Subscription, &error),
            "The provider had a server error. Try again."
        );
        assert!(!error.to_string().contains("private upstream detail"));
    }

    #[tokio::test]
    async fn completed_response_uses_finished_items_when_output_is_empty() {
        use axum::{body::Bytes, http::Response};
        use futures_util::stream;
        use std::convert::Infallible;

        let chunks = stream::iter([
            Ok::<_, Infallible>(Bytes::from_static(
                b"data: {\"type\":\"response.reasoning_summary_text.delta\",\"output_index\":0,\"summary_index\":0,\"delta\":\"Planning example steps\"}\n\n",
            )),
            Ok(Bytes::from_static(
                b"data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"reasoning\",\"summary\":[{\"text\":\"Planning example steps\"}]}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"data: {\"type\":\"response.output_item.done\",\"output_index\":1,\"item\":{\"type\":\"message\",\"phase\":\"commentary\",\"content\":[{\"type\":\"output_text\",\"text\":\"Checking the example file.\"}]}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"data: {\"type\":\"response.output_item.done\",\"output_index\":2,\"item\":{\"type\":\"function_call\",\"call_id\":\"call-example\",\"name\":\"write\",\"arguments\":\"{\\\"path\\\":\\\"example.txt\\\"}\"}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"data: {\"type\":\"response.completed\",\"response\":{\"output\":[]}}\n\n",
            )),
        ]);
        let response: reqwest::Response = Response::new(reqwest::Body::wrap_stream(chunks)).into();
        let step = read_tool_stream(response, WireShape::Responses, None)
            .await
            .unwrap();
        assert_eq!(step.reasoning_summaries, ["Planning example steps"]);
        assert_eq!(step.text, "Checking the example file.");
        assert_eq!(step.calls.len(), 1);
        assert_eq!(step.calls[0].name, "write");
        assert_eq!(step.calls[0].id, "call-example");
        assert_eq!(step.continuation.len(), 3);
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

    #[tokio::test]
    #[ignore = "requires a signed-in Codex account and live network access"]
    async fn codex_subscription_selects_daybreak_blue_with_tools() {
        let store = CredentialStore::new().unwrap();
        assert!(crate::codex::configured());
        assert_eq!(store.daybreak_mode().unwrap(), "blue");
        let client = ProviderClient::new().unwrap();
        let input = vec![json!({"role":"user","content":"Reply with OK without using tools."})];
        let tools = vec![json!({
            "type":"function",
            "name":"read",
            "description":"Read a synthetic example file.",
            "parameters":{"type":"object","properties":{},"additionalProperties":false}
        })];
        let step = client
            .openai_tool_step(
                AuthMode::Subscription,
                ToolStepRequest {
                    model: "gpt-5.6-sol",
                    input: &input,
                    tools: &tools,
                    instructions: Some("This is a synthetic transport check."),
                    reasoning_effort: None,
                    workspace_root: None,
                },
                &store,
                None,
                &mut OpenAiConnection::default(),
            )
            .await
            .unwrap();
        assert!(!step.text.trim().is_empty());
    }
}
