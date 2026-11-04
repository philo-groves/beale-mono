use std::{
    collections::{HashMap, HashSet},
    net::IpAddr,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use reqwest::{
    Client, Url,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};

const MODERN_VERSION: &str = "2026-07-28";
const LEGACY_VERSION: &str = "2025-11-25";
const MAX_MESSAGE_BYTES: usize = 1_048_576;
const MAX_TOOLS: usize = 4_096;

pub(super) struct RunningServer {
    transport: Transport,
    pub(super) tool_count: u32,
}

enum Transport {
    Stdio {
        child: Child,
        stdin: ChildStdin,
        stdout: BufReader<ChildStdout>,
    },
    Http {
        client: Client,
        url: Url,
        headers: HeaderMap,
        session_id: Option<HeaderValue>,
        legacy: bool,
    },
}

impl RunningServer {
    pub(super) fn alive(&mut self) -> bool {
        match &mut self.transport {
            Transport::Stdio { child, .. } => matches!(child.try_wait(), Ok(None)),
            Transport::Http { .. } => true,
        }
    }

    pub(super) async fn launch(root: &Path, data_root: &Path, config: &Value) -> Result<Self> {
        let transport = match config.get("type").and_then(Value::as_str) {
            Some("stdio") => stdio(root, data_root, config).await?,
            Some("streamable-http") => http(config)?,
            _ => bail!("unsupported MCP transport"),
        };
        let mut server = Self {
            transport,
            tool_count: 0,
        };
        let tool_count = timeout(Duration::from_secs(12), server.discover_tools())
            .await
            .context("MCP discovery timed out")??;
        server.tool_count = tool_count;
        Ok(server)
    }

    async fn discover_tools(&mut self) -> Result<u32> {
        let discover = self.request(1, "server/discover", modern_params()).await;
        let modern = discover
            .as_ref()
            .ok()
            .and_then(|response| response.get("result"))
            .and_then(|result| result.get("supportedVersions"))
            .and_then(Value::as_array)
            .is_some_and(|versions| {
                versions
                    .iter()
                    .any(|version| version.as_str() == Some(MODERN_VERSION))
            });
        if !modern {
            let initialized = self
                .request(
                    2,
                    "initialize",
                    json!({
                        "protocolVersion": LEGACY_VERSION,
                        "capabilities": {},
                        "clientInfo": {"name":"beale", "version":"2"}
                    }),
                )
                .await?;
            ensure!(
                initialized.get("result").is_some(),
                "MCP initialization failed"
            );
            self.notify("notifications/initialized", json!({})).await?;
        }
        let mut names = HashSet::new();
        let mut cursors = HashSet::new();
        let mut cursor: Option<String> = None;
        for id in 3..67 {
            let mut params = if modern { modern_params() } else { json!({}) };
            if let Some(value) = &cursor {
                params["cursor"] = json!(value);
            }
            let response = self.request(id, "tools/list", params).await?;
            let result = response.get("result").context("MCP tools/list failed")?;
            let tools = result
                .get("tools")
                .and_then(Value::as_array)
                .context("invalid MCP tool list")?;
            for tool in tools {
                let name = tool
                    .get("name")
                    .and_then(Value::as_str)
                    .context("MCP tool has no name")?;
                ensure!(
                    !name.is_empty() && names.insert(name.to_owned()),
                    "invalid or duplicate MCP tool"
                );
                ensure!(names.len() <= MAX_TOOLS, "too many MCP tools");
            }
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
            match &cursor {
                Some(value) => ensure!(cursors.insert(value.clone()), "repeated MCP tool cursor"),
                None => return u32::try_from(names.len()).context("too many MCP tools"),
            }
        }
        bail!("too many MCP tool pages")
    }

    async fn request(&mut self, id: i64, method: &str, params: Value) -> Result<Value> {
        let message = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        match &mut self.transport {
            Transport::Stdio { stdin, stdout, .. } => {
                write_line(stdin, &message).await?;
                for _ in 0..64 {
                    let value = read_line(stdout).await?;
                    if value.get("id").and_then(Value::as_i64) == Some(id) {
                        ensure!(
                            value.get("jsonrpc").and_then(Value::as_str) == Some("2.0"),
                            "invalid MCP response"
                        );
                        return Ok(value);
                    }
                }
                bail!("MCP response not received")
            }
            Transport::Http {
                client,
                url,
                headers,
                session_id,
                legacy,
            } => {
                let bytes = serde_json::to_vec(&message)?;
                let mut request = client
                    .post(url.clone())
                    .headers(headers.clone())
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .header(
                        "mcp-protocol-version",
                        if method == "initialize" || *legacy {
                            LEGACY_VERSION
                        } else {
                            MODERN_VERSION
                        },
                    )
                    .header("mcp-method", method)
                    .body(bytes);
                if let Some(id) = session_id.as_ref() {
                    request = request.header("mcp-session-id", id);
                }
                let response = request.send().await.context("connect to MCP server")?;
                ensure!(response.status().is_success(), "MCP request failed");
                if method == "initialize" {
                    *session_id = response.headers().get("mcp-session-id").cloned();
                    *legacy = true;
                }
                let content_type = response
                    .headers()
                    .get("content-type")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_owned();
                let mut response = response;
                let mut bytes = Vec::new();
                let streaming = content_type.starts_with("text/event-stream");
                let value = 'found: loop {
                    match response.chunk().await? {
                        Some(chunk) => {
                            ensure!(
                                bytes.len() + chunk.len() <= MAX_MESSAGE_BYTES,
                                "MCP response is too large"
                            );
                            bytes.extend_from_slice(&chunk);
                            if streaming {
                                let text = std::str::from_utf8(&bytes)
                                    .context("invalid MCP event stream")?;
                                let normalized = text.replace("\r\n", "\n");
                                for event in normalized.split("\n\n") {
                                    let data = event
                                        .lines()
                                        .filter_map(|line| line.strip_prefix("data:"))
                                        .map(str::trim_start)
                                        .collect::<Vec<_>>()
                                        .join("\n");
                                    if let Ok(value) = serde_json::from_str::<Value>(&data)
                                        && value.get("id").and_then(Value::as_i64) == Some(id)
                                    {
                                        break 'found value;
                                    }
                                }
                            }
                        }
                        None => {
                            ensure!(!streaming, "MCP event response not received");
                            break serde_json::from_slice(&bytes)
                                .context("invalid MCP response")?;
                        }
                    }
                };
                ensure!(
                    value.get("id").and_then(Value::as_i64) == Some(id),
                    "unexpected MCP response id"
                );
                Ok(value)
            }
        }
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        let message = json!({"jsonrpc":"2.0","method":method,"params":params});
        match &mut self.transport {
            Transport::Stdio { stdin, .. } => write_line(stdin, &message).await?,
            Transport::Http {
                client,
                url,
                headers,
                session_id,
                ..
            } => {
                let mut request = client
                    .post(url.clone())
                    .headers(headers.clone())
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .header("mcp-protocol-version", LEGACY_VERSION)
                    .body(serde_json::to_vec(&message)?);
                if let Some(id) = session_id.as_ref() {
                    request = request.header("mcp-session-id", id);
                }
                ensure!(
                    request.send().await?.status().is_success(),
                    "MCP notification failed"
                );
            }
        }
        Ok(())
    }
}

fn modern_params() -> Value {
    json!({"_meta":{
        "io.modelcontextprotocol/protocolVersion": MODERN_VERSION,
        "io.modelcontextprotocol/clientInfo":{"name":"beale","version":"2"},
        "io.modelcontextprotocol/clientCapabilities":{}
    }})
}

async fn write_line(stdin: &mut ChildStdin, value: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    ensure!(bytes.len() <= MAX_MESSAGE_BYTES, "MCP request is too large");
    bytes.push(b'\n');
    stdin.write_all(&bytes).await?;
    stdin.flush().await?;
    Ok(())
}

async fn read_line(stdout: &mut BufReader<ChildStdout>) -> Result<Value> {
    let mut line = Vec::new();
    loop {
        let chunk = stdout.fill_buf().await?;
        ensure!(!chunk.is_empty(), "MCP server closed stdout");
        let end = chunk.iter().position(|byte| *byte == b'\n');
        let length = end.map_or(chunk.len(), |at| at + 1);
        ensure!(
            line.len() + length <= MAX_MESSAGE_BYTES,
            "MCP response is too large"
        );
        line.extend_from_slice(&chunk[..length]);
        stdout.consume(length);
        if end.is_some() {
            break;
        }
    }
    serde_json::from_slice(&line).context("invalid MCP response")
}

async fn stdio(root: &Path, data_root: &Path, config: &Value) -> Result<Transport> {
    let command = config
        .get("command")
        .and_then(Value::as_str)
        .context("MCP command is missing")?;
    ensure!(
        !command.is_empty() && command.len() <= 1_024,
        "invalid MCP command"
    );
    ensure!(
        !command.chars().any(char::is_whitespace),
        "MCP command must be one token"
    );
    let executable = if command.starts_with("./") {
        let path = root
            .join(command)
            .canonicalize()
            .context("locate MCP executable")?;
        ensure!(
            path.starts_with(root) && path.is_file(),
            "MCP executable escapes plugin root"
        );
        path.into_os_string()
    } else {
        ensure!(
            !command.contains('/') && !command.contains('\\'),
            "invalid MCP executable token"
        );
        command.into()
    };
    let mut process = Command::new(executable);
    process.kill_on_drop(true);
    let args = config
        .get("args")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    ensure!(args.len() <= 64, "too many MCP arguments");
    for arg in args {
        let arg = arg.as_str().context("invalid MCP argument")?;
        ensure!(arg.len() <= 4_096, "MCP argument is too large");
        process.arg(expand(arg, root, data_root)?);
    }
    let cwd = config
        .get("cwd")
        .and_then(Value::as_str)
        .unwrap_or("${PLUGIN_ROOT}");
    let (cwd, boundary) = if cwd.starts_with("./") {
        (root.join(cwd), root)
    } else if cwd == "${PLUGIN_ROOT}" || cwd.starts_with("${PLUGIN_ROOT}/") {
        (PathBuf::from(expand(cwd, root, data_root)?), root)
    } else if cwd == "${PLUGIN_DATA}" || cwd.starts_with("${PLUGIN_DATA}/") {
        (PathBuf::from(expand(cwd, root, data_root)?), data_root)
    } else {
        bail!("invalid MCP working directory")
    };
    let cwd = cwd.canonicalize()?;
    ensure!(
        cwd.starts_with(boundary),
        "MCP working directory escapes its root"
    );
    process.current_dir(platform_path(&cwd));
    if let Some(env) = config.get("env") {
        let env = env.as_object().context("invalid MCP environment")?;
        ensure!(env.len() <= 64, "too many MCP environment variables");
        for (key, value) in env {
            ensure!(
                !key.eq_ignore_ascii_case("PLUGIN_ROOT")
                    && !key.eq_ignore_ascii_case("PLUGIN_DATA"),
                "reserved MCP environment variable"
            );
            let value = value.as_str().context("invalid MCP environment value")?;
            process.env(key, expand(value, root, data_root)?);
        }
    }
    process
        .env("PLUGIN_ROOT", platform_path(root))
        .env("PLUGIN_DATA", platform_path(data_root));
    process
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = process.spawn().context("launch MCP server")?;
    let stdin = child.stdin.take().context("MCP stdin unavailable")?;
    let stdout = child.stdout.take().context("MCP stdout unavailable")?;
    Ok(Transport::Stdio {
        child,
        stdin,
        stdout: BufReader::new(stdout),
    })
}

fn http(config: &Value) -> Result<Transport> {
    let url = config
        .get("url")
        .and_then(Value::as_str)
        .context("MCP URL is missing")?;
    let url = Url::parse(url).context("invalid MCP URL")?;
    ensure!(
        matches!(url.scheme(), "https" | "http"),
        "unsupported MCP URL scheme"
    );
    ensure!(
        url.username().is_empty() && url.password().is_none() && url.fragment().is_none(),
        "invalid MCP URL components"
    );
    if url.scheme() == "http" {
        let host = url.host_str().context("MCP URL is missing a host")?;
        ensure!(
            host.eq_ignore_ascii_case("localhost")
                || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback()),
            "remote MCP requires HTTPS"
        );
    }
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(12))
        .build()?;
    let mut headers = HeaderMap::new();
    if let Some(configured) = config.get("headers") {
        let configured = configured.as_object().context("invalid MCP headers")?;
        ensure!(configured.len() <= 64, "too many MCP headers");
        let mut names = HashSet::new();
        for (key, value) in configured {
            ensure!(
                names.insert(key.to_ascii_lowercase()),
                "duplicate MCP header"
            );
            let key = HeaderName::from_bytes(key.as_bytes())?;
            let value = HeaderValue::from_str(value.as_str().context("invalid MCP header value")?)?;
            headers.insert(key, value);
        }
    }
    Ok(Transport::Http {
        client,
        url,
        headers,
        session_id: None,
        legacy: false,
    })
}

fn expand(value: &str, root: &Path, data_root: &Path) -> Result<String> {
    let expanded = value
        .replace("${PLUGIN_ROOT}", &platform_path(root))
        .replace("${PLUGIN_DATA}", &platform_path(data_root));
    Ok(expanded)
}

fn platform_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    #[cfg(windows)]
    {
        if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{rest}");
        }
        if let Some(rest) = value.strip_prefix(r"\\?\") {
            return rest.to_owned();
        }
    }
    value.into_owned()
}

pub(super) fn configurations(root: &Path) -> Result<HashMap<String, Value>> {
    let path = root.join("mcp.json");
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let path = path.canonicalize()?;
    ensure!(
        path.starts_with(root) && path.is_file(),
        "MCP configuration escapes plugin root"
    );
    ensure!(
        path.metadata()?.len() <= 64_000,
        "MCP configuration is too large"
    );
    let data: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let object = data.as_object().context("invalid MCP configuration")?;
    ensure!(
        object.len() == 2 && object.contains_key("$schema") && object.contains_key("mcpServers"),
        "invalid MCP configuration fields"
    );
    ensure!(
        data.get("$schema").and_then(Value::as_str) == Some(super::MCP_SCHEMA),
        "unsupported MCP schema"
    );
    let servers = data
        .get("mcpServers")
        .and_then(Value::as_object)
        .context("invalid MCP servers")?;
    ensure!(servers.len() <= 128, "too many MCP servers");
    Ok(servers
        .iter()
        .map(|(name, config)| (name.clone(), config.clone()))
        .collect())
}
