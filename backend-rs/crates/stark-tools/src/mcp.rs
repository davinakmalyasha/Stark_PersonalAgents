//! MCP client on the official `rmcp` SDK: stdio and Streamable-HTTP.
//!
//! Config is the existing `.deep_agents/mcp_servers.json` format. Tools are
//! exposed dynamically as `mcp__<server>__<tool>` and appended to the normal
//! per-scope tool list. Calls are serialized per server and bounded.
//!
//! Layering (Codex `codex-mcp` pattern): `rmcp` owns the protocol (framing,
//! initialize handshake, pagination, sessions, notifications). Everything
//! Stark stays above it: server config, approvals, scopes, enabled/disabled
//! lists, per-run `active_mcp` deferral, catalog revision + refresh, tool
//! naming, output truncation. No `rmcp::` types leak past `McpServer`.

use std::collections::{BTreeMap, HashMap};
use std::process::Stdio;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::truncate::truncate_tool_output;
use crate::{QuestionBroker, ToolContext, ToolErrorClass, ToolOutcome};

const CLIENT_NAME: &str = "stark-rs";
const CLIENT_VERSION: &str = "2.0.0";
const MAX_TOOLS_PER_SERVER: usize = 500;
/// Pagination guard for `tools/list` (Codex `pagination.rs` class): servers
/// must terminate, and the catalog stays bounded for provider specs.
const MAX_LIST_PAGES: usize = 100;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ServerConfig {
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub env_vars: Vec<String>,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub bearer_token_env_var: String,
    #[serde(default)]
    pub http_headers: BTreeMap<String, String>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub enabled_tools: Vec<String>,
    #[serde(default)]
    pub disabled_tools: Vec<String>,
    #[serde(default = "default_approval")]
    pub approval: String,
    #[serde(default)]
    pub instructions: String,
    #[serde(default = "default_startup_timeout")]
    pub startup_timeout_sec: f64,
    #[serde(default = "default_tool_timeout")]
    pub tool_timeout_sec: f64,
}

fn default_approval() -> String {
    "auto".to_string()
}
fn default_startup_timeout() -> f64 {
    10.0
}
fn default_tool_timeout() -> f64 {
    60.0
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct McpConfig {
    #[serde(default)]
    servers: BTreeMap<String, ServerConfig>,
}

struct McpServer {
    name: String,
    config: ServerConfig,
    status: RwLock<String>,
    error: RwLock<String>,
    tools: RwLock<Vec<Value>>,
    session: Mutex<Option<RmcpSession>>,
    /// Set by the rmcp `notifications/tools/list_changed` handler (or an
    /// "unknown tool" call failure). The background poller refreshes promptly
    /// instead of waiting for the periodic interval.
    list_changed: Arc<AtomicBool>,
    /// Elicitation bridge for the in-flight call, if any. Set while holding
    /// the session lock so a server input request is always attributed to
    /// the call that triggered it.
    elicit: Arc<Mutex<Option<ElicitBridge>>>,
    /// Last `WWW-Authenticate` challenge from a 401, reused to seed OAuth
    /// scope selection.
    auth_challenge: Mutex<Option<String>>,
    /// Catalog revision, bumped whenever the cached tool set actually
    /// changes (Codex parity with `tool_list_changes`: the revision lets
    /// the agent loop know the provider spec list is stale).
    revision: AtomicU64,
    /// Set when a `notifications/tools/list_changed` arrived (or a call
    /// failed with "unknown tool"); the background poller refreshes promptly
    /// instead of waiting for the periodic interval.
    needs_refresh: AtomicBool,
    last_refresh: Mutex<Instant>,
}

impl McpServer {
    fn new(name: String, config: ServerConfig) -> Self {
        Self {
            name,
            config,
            status: RwLock::new("starting".to_string()),
            error: RwLock::new(String::new()),
            tools: RwLock::new(Vec::new()),
            session: Mutex::new(None),
            list_changed: Arc::new(AtomicBool::new(false)),
            elicit: Arc::new(Mutex::new(None)),
            auth_challenge: Mutex::new(None),
            revision: AtomicU64::new(0),
            needs_refresh: AtomicBool::new(false),
            last_refresh: Mutex::new(Instant::now()),
        }
    }

    fn take_changed(&self) -> bool {
        self.list_changed.swap(false, Ordering::AcqRel)
    }

    fn clear_bridge(&self) -> impl Future<Output = ()> {
        let elicit = self.elicit.clone();
        async move {
            *elicit.lock().await = None;
        }
    }

    fn bump_revision(&self) {
        self.revision.fetch_add(1, Ordering::AcqRel);
    }

    fn mark_refresh_needed(&self) {
        self.needs_refresh.store(true, Ordering::Release);
    }

    fn status(&self) -> String {
        self.status
            .read()
            .map(|value| value.clone())
            .unwrap_or_else(|_| "failed".to_string())
    }

    fn set_status(&self, status: &str, error: &str) {
        if let Ok(mut value) = self.status.write() {
            *value = status.to_string();
        }
        if let Ok(mut value) = self.error.write() {
            *value = error.to_string();
        }
    }

    fn tool_count(&self) -> usize {
        self.tools.read().map(|tools| tools.len()).unwrap_or(0)
    }
}

/// Bridge between an in-flight MCP call and the elicitation handler: the
/// handler answers server input requests through the normal question broker
/// (same UX as approvals), attributed to the originating chat/scope.
/// Non-interactive agents (subagents) always decline instead of blocking.
#[derive(Clone)]
struct ElicitBridge {
    broker: Arc<QuestionBroker>,
    chat_id: String,
    server: String,
    tool: String,
    interactive: bool,
}

/// rmcp client handler: advertises our identity + form elicitation, and
/// records catalog invalidations. Stateless besides shared flags; all Stark
/// policy lives above.
#[derive(Clone)]
struct StarkMcpHandler {
    changed: Arc<AtomicBool>,
    elicit: Arc<Mutex<Option<ElicitBridge>>>,
}

impl rmcp::ClientHandler for StarkMcpHandler {
    fn get_info(&self) -> rmcp::model::ClientConfig {
        let mut capabilities = rmcp::model::ClientCapabilities::builder();
        capabilities.elicitation = Some(
            rmcp::model::ElicitationCapability::new()
                .with_form(rmcp::model::FormElicitationCapability::new()),
        );
        rmcp::model::ClientConfig::new(
            capabilities.build(),
            rmcp::model::Implementation::new(CLIENT_NAME, CLIENT_VERSION),
        )
    }

    fn on_tool_list_changed(
        &self,
        _context: rmcp::service::NotificationContext<rmcp::service::RoleClient>,
    ) -> impl Future<Output = ()> + rmcp::service::MaybeSendFuture + '_ {
        self.changed.store(true, Ordering::Release);
        std::future::ready(())
    }

    async fn create_elicitation(
        &self,
        request: rmcp::model::ElicitRequestParams,
        _context: rmcp::service::RequestContext<rmcp::service::RoleClient>,
    ) -> Result<rmcp::model::ElicitResult, rmcp::model::ErrorData> {
        use rmcp::model::ElicitRequestParams::*;
        let bridge = self.elicit.lock().await.clone();
        let Some(bridge) = bridge else {
            // No call in flight to attribute the request to: decline so the
            // server continues without input (rmcp's safe default).
            return Ok(rmcp::model::ElicitResult::new(
                rmcp::model::ElicitationAction::Decline,
            ));
        };
        match request {
            FormElicitationParams {
                message,
                requested_schema,
                ..
            } => {
                if !bridge.interactive {
                    // Subagents run non-interactively: decline immediately
                    // instead of blocking on a question nobody answers.
                    return Ok(rmcp::model::ElicitResult::new(
                        rmcp::model::ElicitationAction::Decline,
                    ));
                }
                let schema =
                    serde_json::to_value(&requested_schema).unwrap_or(serde_json::Value::Null);
                let question = json!({
                    "id": "mcp-elicit",
                    "question": message,
                    "header": "Input needed",
                    "detail": format!(
                        "MCP server '{}' needs input for '{}'. Reply with a `values` object matching the schema.",
                        bridge.server, bridge.tool
                    ),
                    "schema": schema,
                });
                let answers = match bridge
                    .broker
                    .ask(&bridge.chat_id, vec![question], 180)
                    .await
                {
                    Ok(answers) => answers,
                    // Timeout or cancellation: decline so the tool call can
                    // proceed without input instead of failing.
                    Err(_) => {
                        return Ok(rmcp::model::ElicitResult::new(
                            rmcp::model::ElicitationAction::Decline,
                        ));
                    }
                };
                let values = answers.iter().find_map(|answer| {
                    let values = answer.get("values")?;
                    values.is_object().then(|| values.clone())
                });
                match values {
                    Some(values) => Ok(rmcp::model::ElicitResult::new(
                        rmcp::model::ElicitationAction::Accept,
                    )
                    .with_content(values)),
                    None => Ok(rmcp::model::ElicitResult::new(
                        rmcp::model::ElicitationAction::Decline,
                    )),
                }
            }
            // URL mode needs a browser handoff the agent loop does not have;
            // decline so the server falls back instead of hanging.
            _ => Ok(rmcp::model::ElicitResult::new(
                rmcp::model::ElicitationAction::Decline,
            )),
        }
    }
}

/// One live rmcp connection. Stored in `McpServer`; dropping it tears down
/// the transport (stdio child included) via rmcp's own cleanup, which
/// SIGTERMs before killing, matching the old graceful shutdown.
struct RmcpSession {
    service: rmcp::service::RunningService<rmcp::service::RoleClient, StarkMcpHandler>,
}

/// Connect outcome: either a live session or a discovered OAuth challenge.
/// HTTP 401 surfaces as `AuthRequired` (reactive discovery, matching the
/// official SDK examples); stdio servers never produce it.
enum ConnectError {
    Failed(String),
    AuthRequired { challenge: Option<String> },
}

impl ConnectError {
    fn message(&self) -> String {
        match self {
            ConnectError::Failed(error) => error.clone(),
            ConnectError::AuthRequired { .. } => {
                "MCP server requires OAuth authorization (HTTP 401)".to_string()
            }
        }
    }
}

/// Classify an rmcp serve error: `StreamableHttpError::AuthRequired`
/// displays as `authorization required: <WWW-Authenticate>`.
fn classify_connect_error(error: String) -> ConnectError {
    if let Some(challenge) = error.strip_prefix("authorization required: ") {
        let challenge = challenge.trim();
        return ConnectError::AuthRequired {
            challenge: (!challenge.is_empty()).then(|| challenge.to_string()),
        };
    }
    ConnectError::Failed(error)
}

/// Connect a server with the rmcp SDK (pure replacement: stdio child process
/// or Streamable HTTP with sessions). The initialize handshake runs inside
/// `serve`. Stark specifics preserved: `cmd /C` shim, secrets scrubbing,
/// cwd, env allowlist, bearer token, custom headers.
async fn connect_session(
    config: &ServerConfig,
    changed: Arc<AtomicBool>,
    elicit: Arc<Mutex<Option<ElicitBridge>>>,
) -> Result<RmcpSession, ConnectError> {
    use rmcp::ServiceExt as _;
    let handler = StarkMcpHandler { changed, elicit };
    if !config.url.trim().is_empty() {
        let mut http_config =
            rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(
                config.url.clone(),
            );
        if !config.bearer_token_env_var.trim().is_empty() {
            let token = std::env::var(config.bearer_token_env_var.trim()).unwrap_or_default();
            if !token.is_empty() {
                http_config = http_config.auth_header(token);
            }
        }
        if !config.http_headers.is_empty() {
            let mut headers = HashMap::new();
            for (name, value) in &config.http_headers {
                let name =
                    http::HeaderName::from_str(&name.to_ascii_lowercase()).map_err(|error| {
                        ConnectError::Failed(format!("invalid MCP header '{name}': {error}"))
                    })?;
                let value = http::HeaderValue::from_str(value).map_err(|error| {
                    ConnectError::Failed(format!("invalid MCP header value for '{name}': {error}"))
                })?;
                headers.insert(name, value);
            }
            http_config = http_config.custom_headers(headers);
        }
        let transport = rmcp::transport::StreamableHttpClientTransport::from_config(http_config);
        let service = handler
            .serve(transport)
            .await
            .map_err(|error| classify_connect_error(error.to_string()))?;
        return Ok(RmcpSession { service });
    }
    if config.command.trim().is_empty() {
        return Err(ConnectError::Failed(
            "stdio MCP server requires 'command'".to_string(),
        ));
    }
    let mut command = if needs_cmd_shim(&config.command) {
        let mut command = Command::new("cmd");
        command.arg("/C").arg(&config.command);
        command
    } else {
        Command::new(&config.command)
    };
    command.args(&config.args);
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.kill_on_drop(true);
    // MCP servers inherit the parent env by default. Strip secrets first;
    // only explicitly configured env (config.env / config.env_vars) is
    // re-added below, so a server only gets what its config allows.
    stark_sandbox::secrets::scrub_tokio_command(&mut command);
    if !config.cwd.trim().is_empty() {
        command.current_dir(&config.cwd);
    }
    for (key, value) in &config.env {
        command.env(key, value);
    }
    for key in &config.env_vars {
        if let Ok(value) = std::env::var(key) {
            command.env(key, value);
        }
    }
    let (transport, stderr) = rmcp::transport::TokioChildProcess::builder(command)
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| ConnectError::Failed(error.to_string()))?;
    if let Some(stderr) = stderr {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!("[mcp] {line}");
            }
        });
    }
    let service = handler
        .serve(transport)
        .await
        .map_err(|error| ConnectError::Failed(error.to_string()))?;
    Ok(RmcpSession { service })
}

/// Install a connected session: list tools (paginated), refresh the cache,
/// bump the revision only when the name set changed. Shared by initial
/// connect and the OAuth authorize path.
async fn install_session(server: &Arc<McpServer>, session: RmcpSession) -> Result<bool, String> {
    let tools = list_all_tools(&session.service).await?;
    {
        let mut guard = server.session.lock().await;
        *guard = Some(session);
    }
    let changed = {
        let current = server
            .tools
            .read()
            .map(|tools| tools.clone())
            .unwrap_or_default();
        tool_names_changed(&current, &tools)
    };
    if changed {
        if let Ok(mut current) = server.tools.write() {
            *current = tools.into_iter().take(MAX_TOOLS_PER_SERVER).collect();
        }
        server.bump_revision();
    }
    {
        let mut last = server.last_refresh.lock().await;
        *last = Instant::now();
    }
    if server.take_changed() {
        server.mark_refresh_needed();
    }
    server.set_status("connected", "");
    Ok(changed)
}

/// Paginated `tools/list` (Codex `pagination.rs` class): cursor loop with
/// page and item caps so a hostile or huge catalog cannot grow the provider
/// spec list without bound. Returns raw tool definitions; Stark naming,
/// filtering, and truncation stay downstream.
async fn list_all_tools(
    service: &rmcp::service::RunningService<rmcp::service::RoleClient, StarkMcpHandler>,
) -> Result<Vec<Value>, String> {
    let mut tools = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_LIST_PAGES {
        let mut params = rmcp::model::PaginatedRequestParams::default();
        params.cursor = cursor;
        let page = service
            .list_tools(Some(params))
            .await
            .map_err(|error| error.to_string())?;
        for tool in page.tools {
            if tools.len() >= MAX_TOOLS_PER_SERVER {
                break;
            }
            tools.push(serde_json::to_value(&tool).map_err(|error| error.to_string())?);
        }
        cursor = page.next_cursor;
        if cursor.is_none() || tools.len() >= MAX_TOOLS_PER_SERVER {
            break;
        }
    }
    Ok(tools)
}

/// `tools/call` with Stark's existing result shaping: text blocks joined as
/// before, `structuredContent` fallback, `isError` gate. The JSON shape is
/// identical to the hand-rolled client's, so truncation and dispatch output
/// do not change.
async fn call_tool(
    session: &RmcpSession,
    tool: &str,
    args: Value,
    timeout: Duration,
) -> Result<String, String> {
    let arguments = args.as_object().cloned().unwrap_or_default();
    let params =
        rmcp::model::CallToolRequestParams::new(tool.to_string()).with_arguments(arguments);
    let result = tokio::time::timeout(timeout, session.service.call_tool(params))
        .await
        .map_err(|_| format!("MCP tool {tool} timed out"))?
        .map_err(|error| error.to_string())?;
    let value = serde_json::to_value(&result).map_err(|error| error.to_string())?;
    let multiline = |value: &Value| {
        let text = content_text(value);
        if text.is_empty()
            && let Some(structured) = value.get("structuredContent")
        {
            return structured.to_string();
        }
        text
    };
    if value.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(multiline(&value));
    }
    let text = multiline(&value);
    if text.is_empty() {
        Ok(value.to_string())
    } else {
        Ok(text)
    }
}

async fn close_session(session: RmcpSession) {
    let _ = session.service.cancel().await;
    // Dropping the service tears down the transport; the stdio child is
    // SIGTERMed before being killed by rmcp's own cleanup.
}

/// Attribute an in-flight call to the elicitation bridge so a server input
/// request is answered in the right chat. Called while holding the session
/// lock; every `call_tool` below is wrapped the same way.
async fn set_bridge(server: &Arc<McpServer>, ctx: &ToolContext, tool: &str) {
    *server.elicit.lock().await = Some(ElicitBridge {
        broker: ctx.questions.clone(),
        chat_id: ctx.chat_id.clone(),
        server: server.name.clone(),
        tool: tool.to_string(),
        interactive: ctx.depth == 0,
    });
}

/// Stored OAuth credentials per server: client id plus the token response.
/// Refresh tokens survive restarts; the browser flow only runs when both the
/// stored set and a refresh are rejected.
#[derive(Serialize, Deserialize)]
struct StoredOAuthCredentials {
    client_id: String,
    token_response: Value,
}

fn oauth_token_path(root: &std::path::Path, server: &str) -> std::path::PathBuf {
    root.join(".deep_agents")
        .join("mcp_oauth")
        .join(format!("{}.json", sanitize(server)))
}

/// True when a call failure means the session lost authorization: 401
/// challenges, scope upgrades, and rejected/failed refreshes. Plain tool
/// errors never match.
fn is_auth_error(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("authorization required")
        || error.contains("insufficient scope")
        || error.contains("refresh token was rejected")
        || error.contains("token refresh failed")
}

/// Minimal percent-decoder for OAuth callback query strings (`%XX` + `+`).
fn url_decode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = &value[index + 1..index + 3];
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte as char);
                    index += 3;
                    continue;
                }
                out.push('%');
                index += 1;
            }
            b'+' => {
                out.push(' ');
                index += 1;
            }
            byte => {
                out.push(byte as char);
                index += 1;
            }
        }
    }
    out
}

struct OAuthCallback {
    code: String,
    state: String,
    iss: Option<String>,
}

/// Parse `GET /callback?code=..&state=..[&iss=..] HTTP/1.1` request lines.
fn parse_oauth_callback(request_line: &str) -> Option<OAuthCallback> {
    let path = request_line.split_whitespace().nth(1)?;
    let (_, query) = path.split_once('?')?;
    let mut code = None;
    let mut state = None;
    let mut iss = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=')?;
        match key {
            "code" => code = Some(url_decode(value)),
            "state" => state = Some(url_decode(value)),
            "iss" => iss = Some(url_decode(value)),
            _ => {}
        }
    }
    Some(OAuthCallback {
        code: code?,
        state: state?,
        iss,
    })
}

/// Bind a loopback listener for the OAuth redirect. The port is chosen by
/// the OS so no fixed callback port is needed; the redirect URI handed to
/// the provider embeds it.
async fn oauth_callback_listener() -> Result<(u16, tokio::net::TcpListener), String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|error| format!("MCP OAuth callback listener failed: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("MCP OAuth callback listener failed: {error}"))?
        .port();
    Ok((port, listener))
}

/// Wait for the provider's browser redirect, then answer with a plain page
/// so the user can close the tab.
async fn wait_for_oauth_callback(
    listener: tokio::net::TcpListener,
    timeout: Duration,
) -> Result<OAuthCallback, String> {
    let (mut stream, _) = tokio::time::timeout(timeout, listener.accept())
        .await
        .map_err(|_| "MCP OAuth timed out waiting for the browser callback".to_string())?
        .map_err(|error| format!("MCP OAuth callback accept failed: {error}"))?;
    let mut raw = Vec::new();
    let mut chunk = [0u8; 1024];
    use tokio::io::AsyncReadExt as _;
    loop {
        let read = tokio::time::timeout(Duration::from_secs(30), stream.read(&mut chunk))
            .await
            .map_err(|_| "MCP OAuth timed out reading the browser callback".to_string())?
            .map_err(|error| format!("MCP OAuth callback read failed: {error}"))?;
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&chunk[..read]);
        if raw.len() > 65536 || raw.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8_lossy(&raw);
    let request_line = head.lines().next().unwrap_or("");
    let callback = parse_oauth_callback(request_line)
        .ok_or_else(|| "MCP OAuth callback did not carry a code".to_string())?;
    let body = "<html><body><h1>Authorized</h1><p>You can close this tab and return to Stark.</p></body></html>";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    use tokio::io::AsyncWriteExt as _;
    let _ = stream.write_all(response.as_bytes()).await;
    Ok(callback)
}

/// On Windows, bare command names may resolve to an npm `.cmd` shim rather
/// than an `.exe`; `CreateProcess` only appends `.exe`, so run those through
/// `cmd /C`. Absolute `.exe` paths and extension-qualified commands stay
/// direct.
fn needs_cmd_shim(command: &str) -> bool {
    if !cfg!(windows) {
        return false;
    }
    let lower = command.to_ascii_lowercase();
    if lower.ends_with(".cmd") || lower.ends_with(".bat") {
        return true;
    }
    let path = std::path::Path::new(command);
    path.extension().is_none() && !path.is_absolute()
}

fn content_text(result: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(content) = result.get("content").and_then(Value::as_array) {
        for item in content {
            if let Some(text) = item.get("text").and_then(Value::as_str) {
                parts.push(text.to_string());
            } else if item.is_string() {
                parts.push(item.as_str().unwrap_or("").to_string());
            }
        }
    }
    if parts.is_empty()
        && let Some(structured) = result.get("structuredContent")
    {
        parts.push(structured.to_string());
    }
    parts.join("\n")
}

pub struct McpManager {
    root: std::path::PathBuf,
    servers: RwLock<BTreeMap<String, Arc<McpServer>>>,
}

static MANAGER: OnceLock<Arc<McpManager>> = OnceLock::new();

impl McpManager {
    pub fn init(root: impl Into<std::path::PathBuf>) -> Arc<Self> {
        MANAGER
            .get_or_init(|| {
                Arc::new(Self {
                    root: root.into(),
                    servers: RwLock::new(BTreeMap::new()),
                })
            })
            .clone()
    }

    pub fn global() -> Option<Arc<Self>> {
        MANAGER.get().cloned()
    }

    fn config_path(&self) -> std::path::PathBuf {
        self.root.join(".deep_agents").join("mcp_servers.json")
    }

    fn load_config(&self) -> McpConfig {
        std::fs::read_to_string(self.config_path())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn save_config(&self, config: &McpConfig) -> Result<(), String> {
        let path = self.config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let raw = serde_json::to_string_pretty(config).map_err(|error| error.to_string())?;
        std::fs::write(path, raw).map_err(|error| error.to_string())
    }

    pub async fn start_all(self: Arc<Self>) {
        let config = self.load_config();
        {
            let mut servers = match self.servers.write() {
                Ok(servers) => servers,
                Err(_) => return,
            };
            for (name, cfg) in &config.servers {
                servers.insert(
                    name.clone(),
                    Arc::new(McpServer::new(name.clone(), cfg.clone())),
                );
            }
        }
        let servers = self
            .servers
            .read()
            .map(|servers| servers.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for server in servers {
            let manager = self.clone();
            tokio::spawn(async move {
                manager.connect_server(server).await;
            });
        }
        // Freshness poller: notification-flagged servers refresh promptly;
        // the rest re-list at most once a minute so catalog drift is picked
        // up even on HTTP transports that cannot receive push notifications.
        let poller = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(15)).await;
                poller.poll_refresh().await;
            }
        });
    }

    async fn connect_server(&self, server: Arc<McpServer>) {
        let startup = Duration::from_secs_f64(server.config.startup_timeout_sec.max(1.0));
        let result: Result<bool, ConnectError> = tokio::time::timeout(startup, async {
            let session = connect_session(
                &server.config,
                server.list_changed.clone(),
                server.elicit.clone(),
            )
            .await?;
            install_session(&server, session)
                .await
                .map_err(ConnectError::Failed)
        })
        .await
        .map_err(|_| {
            ConnectError::Failed(format!(
                "MCP server '{}' startup timed out after {startup:?}",
                server.name
            ))
        })
        .and_then(|inner| inner);
        match result {
            Ok(_) => {}
            Err(ConnectError::AuthRequired { challenge }) => {
                *server.auth_challenge.lock().await = challenge;
                server.set_status(
                    "auth_required",
                    "MCP server requires OAuth authorization; it completes on the next tool call.",
                );
            }
            Err(other) => server.set_status("failed", &other.message()),
        }
    }

    /// Re-list a connected server's tools and update the cache when the set
    /// changed. Returns true when the catalog changed (revision bumped).
    async fn refresh_server(&self, name: &str) -> Result<bool, String> {
        let server = self
            .servers
            .read()
            .ok()
            .and_then(|servers| servers.get(name).cloned())
            .ok_or_else(|| format!("server '{name}' not configured"))?;
        if server.status() != "connected" {
            return Err(format!(
                "MCP server '{name}' is not connected (status: {})",
                server.status()
            ));
        }
        let tools = {
            let guard = server.session.lock().await;
            let session = guard
                .as_ref()
                .ok_or_else(|| format!("MCP server '{name}' has no session"))?;
            list_all_tools(&session.service).await?
        };
        let changed = {
            let current = server
                .tools
                .read()
                .map(|tools| tools.clone())
                .unwrap_or_default();
            tool_names_changed(&current, &tools)
        };
        if changed {
            if let Ok(mut current) = server.tools.write() {
                *current = tools.into_iter().take(MAX_TOOLS_PER_SERVER).collect();
            }
            server.bump_revision();
        }
        {
            let mut last = server.last_refresh.lock().await;
            *last = Instant::now();
        }
        server.needs_refresh.store(false, Ordering::Release);
        Ok(changed)
    }

    async fn poll_refresh(&self) {
        let servers = self
            .servers
            .read()
            .map(|servers| servers.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for server in servers {
            if server.status() != "connected" {
                continue;
            }
            let flagged = server.needs_refresh.load(Ordering::Acquire);
            let stale = server.last_refresh.lock().await.elapsed() >= Duration::from_secs(60);
            if (flagged || stale)
                && let Err(error) = self.refresh_server(&server.name).await
            {
                tracing::debug!("[mcp] refresh of '{}' failed: {error}", server.name);
            }
        }
    }

    /// Current catalog revision for a server (0 = never loaded). Used by
    /// diagnostics and tests to observe refreshes; the HTTP API is unchanged.
    pub fn tool_revision(&self, server: &str) -> Option<u64> {
        self.servers.read().ok().and_then(|servers| {
            servers
                .get(server)
                .map(|server| server.revision.load(Ordering::Acquire))
        })
    }

    pub fn tool_specs_for_scope(&self, scope: &str) -> Vec<Value> {
        let servers = match self.servers.read() {
            Ok(servers) => servers.values().cloned().collect::<Vec<_>>(),
            Err(_) => return Vec::new(),
        };
        let mut specs = Vec::new();
        for server in servers {
            if server.status() != "connected" || !scope_matches(&server.config.scopes, scope) {
                continue;
            }
            let enabled: Vec<&str> = server
                .config
                .enabled_tools
                .iter()
                .map(String::as_str)
                .collect();
            let disabled: Vec<&str> = server
                .config
                .disabled_tools
                .iter()
                .map(String::as_str)
                .collect();
            let tools = match server.tools.read() {
                Ok(tools) => tools.clone(),
                Err(_) => continue,
            };
            for tool in tools {
                let name = tool.get("name").and_then(Value::as_str).unwrap_or("");
                if name.is_empty()
                    || (!enabled.is_empty() && !enabled.contains(&name))
                    || disabled.contains(&name)
                {
                    continue;
                }
                specs.push(tool_spec(&server.name, name, &tool));
            }
        }
        // Codex parity (PR #2611 class): MCP tools must surface in a stable
        // order. Discovery order can shift mid-conversation via
        // notifications/tools/list_changed, and any order change busts the
        // provider prompt cache for the rest of a long turn sequence.
        specs.sort_by(|left, right| {
            let left_name = left
                .get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let right_name = right
                .get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("");
            left_name.cmp(right_name)
        });
        specs
    }

    /// Rank MCP specs for `tool_search`. Pure matching so it can be tested
    /// without a live server.
    pub fn search(&self, scope: &str, query: &str, limit: usize) -> Vec<Value> {
        rank_tools(&self.tool_specs_for_scope(scope), query, limit)
    }

    pub fn status_snapshot(&self) -> Vec<Value> {
        let servers = match self.servers.read() {
            Ok(servers) => servers.values().cloned().collect::<Vec<_>>(),
            Err(_) => return Vec::new(),
        };
        servers
            .iter()
            .map(|server| {
                json!({
                    "name": server.name,
                    "status": server.status(),
                    "transport": if server.config.url.is_empty() { "stdio" } else { "http" },
                    "error": server.error.read().map(|value| value.clone()).unwrap_or_default(),
                    "tool_count": server.tool_count(),
                    "scopes": server.config.scopes,
                    "instructions": server.config.instructions,
                })
            })
            .collect()
    }

    pub async fn call(
        &self,
        fq_name: &str,
        args: Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let (server_name, tool_name) = parse_fq(fq_name)?;
        let server = self
            .servers
            .read()
            .ok()
            .and_then(|servers| servers.get(&server_name).cloned())
            .ok_or_else(|| format!("unknown MCP server '{server_name}'"))?;
        if server.status() == "auth_required" {
            self.authorize_interactive(&server, ctx).await?;
        }
        if server.status() != "connected" {
            return Err(format!(
                "MCP server '{server_name}' is not connected (status: {})",
                server.status()
            ));
        }
        if !scope_matches(&server.config.scopes, &ctx.scope) {
            return Err(format!(
                "MCP tool '{fq_name}' is not exposed to scope '{}'",
                ctx.scope
            ));
        }
        let actual_tool = server
            .tools
            .read()
            .ok()
            .map(|tools| resolve_cached_tool(&tools, &tool_name))
            .unwrap_or_else(|| tool_name.clone());
        if server.config.approval == "prompt" {
            if stark_sandbox::SandboxProfile::from_env(&ctx.workspace).approval
                == stark_sandbox::ApprovalPolicy::Never
            {
                return Err(format!(
                    "MCP tool call {server_name}/{actual_tool} requires approval, but approval policy is 'never'"
                ));
            }
            if ctx.depth > 0 {
                return Err(format!(
                    "MCP tool call {server_name}/{actual_tool} requires approval; subagents run non-interactively"
                ));
            }
            let question = json!({
                "id": "mcp-approval",
                "question": format!("Approve MCP tool call {server_name}/{actual_tool} with {}?", truncate(&args.to_string(), 400)),
                "options": ["Approve", "Deny"],
            });
            let answers = ctx
                .questions
                .ask(&ctx.chat_id, vec![question], 180)
                .await
                .map_err(|error| error.to_string())?;
            let approved = answers.iter().any(|answer| {
                answer
                    .get("selected")
                    .and_then(Value::as_array)
                    .is_some_and(|selected| {
                        selected.iter().any(|item| {
                            item.as_str()
                                .is_some_and(|value| value.eq_ignore_ascii_case("approve"))
                        })
                    })
            });
            if !approved {
                return Err(format!(
                    "MCP tool call {server_name}/{actual_tool} was not approved"
                ));
            }
        }
        let timeout = Duration::from_secs_f64(server.config.tool_timeout_sec.max(1.0));
        let (first, saw_change) = {
            let guard = server.session.lock().await;
            let session = guard
                .as_ref()
                .ok_or_else(|| format!("MCP server '{server_name}' has no session"))?;
            set_bridge(&server, ctx, &actual_tool).await;
            let result = call_tool(session, &actual_tool, args.clone(), timeout).await;
            server.clear_bridge().await;
            let saw_change = server.take_changed();
            (result, saw_change)
        };
        if saw_change {
            server.mark_refresh_needed();
            // Refresh promptly so the next provider request advertises the
            // new catalog instead of waiting for the background poll.
            if let Err(error) = self.refresh_server(&server_name).await {
                tracing::debug!("[mcp] refresh after list-changed failed: {error}");
            }
        }
        let error = match first {
            Ok(text) => return Ok(text),
            Err(error) => error,
        };
        // A session that lost authorization mid-life is re-authorized in
        // place (user context is available here) and the call retried once.
        if is_auth_error(&error) {
            self.authorize_interactive(&server, ctx).await?;
            let guard = server.session.lock().await;
            let session = guard.as_ref().ok_or_else(|| {
                format!("MCP server '{server_name}' has no session after authorization")
            })?;
            set_bridge(&server, ctx, &actual_tool).await;
            let retry = call_tool(session, &actual_tool, args, timeout).await;
            server.clear_bridge().await;
            return retry;
        }
        // A stale catalog surfaces as "unknown tool": refresh once,
        // re-resolve the name against the new cache, and retry once before
        // giving up. This covers tools added or renamed mid-run.
        if is_unknown_tool_error(&error) && self.refresh_server(&server_name).await.is_ok() {
            let retry_tool = server
                .tools
                .read()
                .ok()
                .map(|tools| resolve_cached_tool(&tools, &tool_name))
                .unwrap_or_else(|| actual_tool.clone());
            let retry = {
                let guard = server.session.lock().await;
                let session = guard.as_ref().ok_or_else(|| {
                    format!("MCP server '{server_name}' has no session after refresh")
                })?;
                set_bridge(&server, ctx, &retry_tool).await;
                let out = call_tool(session, &retry_tool, args, timeout).await;
                server.clear_bridge().await;
                out
            };
            return retry;
        }
        // A dead stdio pipe or a hung HTTP transport is recovered by one
        // restart + retry; a normal tool error is returned unchanged.
        if !transport_failure(&error) {
            return Err(error);
        }
        self.restart_server(&server_name).await?;
        let server = self
            .servers
            .read()
            .ok()
            .and_then(|servers| servers.get(&server_name).cloned())
            .ok_or_else(|| format!("MCP server '{server_name}' disappeared during restart"))?;
        let guard = server.session.lock().await;
        let session = guard
            .as_ref()
            .ok_or_else(|| format!("MCP server '{server_name}' has no session after restart"))?;
        set_bridge(&server, ctx, &actual_tool).await;
        let out = call_tool(session, &actual_tool, args, timeout).await;
        server.clear_bridge().await;
        out
    }

    /// Install an already-authorized rmcp transport (OAuth): serve, list,
    /// and adopt the session through the standard install path.
    async fn install_authorized(
        &self,
        server: &Arc<McpServer>,
        transport: rmcp::transport::StreamableHttpClientTransport<
            rmcp::transport::auth::AuthClient<reqwest013::Client>,
        >,
    ) -> Result<(), String> {
        use rmcp::ServiceExt as _;
        let handler = StarkMcpHandler {
            changed: server.list_changed.clone(),
            elicit: server.elicit.clone(),
        };
        let service = handler
            .serve(transport)
            .await
            .map_err(|error| format!("MCP OAuth session failed for '{}': {error}", server.name))?;
        install_session(server, RmcpSession { service })
            .await
            .map(|_| ())
    }

    /// Silent restore: trade stored refresh credentials for a live session.
    /// Fails (→ browser flow) when nothing is stored, the file is corrupt,
    /// or the provider rejected the refresh token.
    async fn authorize_with_stored(&self, server: &Arc<McpServer>) -> Result<(), String> {
        let raw = tokio::fs::read_to_string(oauth_token_path(&self.root, &server.name))
            .await
            .map_err(|error| format!("no stored MCP OAuth credentials: {error}"))?;
        let stored: StoredOAuthCredentials =
            serde_json::from_str(&raw).map_err(|error| format!("bad stored MCP OAuth: {error}"))?;
        let token_response: rmcp::transport::auth::OAuthTokenResponse =
            serde_json::from_value(stored.token_response)
                .map_err(|error| format!("bad stored MCP OAuth token: {error}"))?;
        let mut oauth = rmcp::transport::auth::OAuthState::new(&server.config.url, None)
            .await
            .map_err(|error| format!("MCP OAuth discovery failed: {error}"))?;
        oauth
            .set_credentials(&stored.client_id, token_response)
            .await
            .map_err(|error| format!("stored MCP OAuth rejected: {error}"))?;
        let manager = oauth
            .into_authorization_manager()
            .ok_or_else(|| "stored MCP OAuth credentials were rejected".to_string())?;
        let auth_client =
            rmcp::transport::auth::AuthClient::new(reqwest013::Client::default(), manager);
        let transport = rmcp::transport::StreamableHttpClientTransport::with_client(
            auth_client,
            rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(
                server.config.url.clone(),
            ),
        );
        self.install_authorized(server, transport).await
    }

    /// Full OAuth login for an `auth_required` server: silent restore first,
    /// otherwise a browser flow (localhost callback, question-broker approval)
    /// whose tokens are persisted for the next restart.
    async fn authorize_interactive(
        &self,
        server: &Arc<McpServer>,
        ctx: &ToolContext,
    ) -> Result<(), String> {
        if self.authorize_with_stored(server).await.is_ok() {
            return Ok(());
        }
        let challenge = server.auth_challenge.lock().await.clone();
        let (port, listener) = oauth_callback_listener().await?;
        let redirect_uri = format!("http://127.0.0.1:{port}/callback");
        let mut oauth = rmcp::transport::auth::OAuthState::new(&server.config.url, None)
            .await
            .map_err(|error| {
                format!("MCP OAuth discovery failed for '{}': {error}", server.name)
            })?;
        {
            let mut request =
                rmcp::transport::auth::AuthorizationRequest::new(redirect_uri.clone())
                    .with_client_name("stark-rs");
            if let Some(challenge) = challenge.as_deref() {
                request = request.with_challenge(challenge);
            }
            oauth.start_authorization(request).await.map_err(|error| {
                format!("MCP OAuth start failed for '{}': {error}", server.name)
            })?;
        }
        let url = oauth
            .get_authorization_url()
            .await
            .map_err(|error| format!("MCP OAuth URL failed for '{}': {error}", server.name))?;
        let question = json!({
            "id": "mcp-oauth",
            "question": format!("Authorize MCP server '{}'?\n\nOpen this URL in your browser to approve access:\n\n{}", server.name, url),
            "header": "Authorization needed",
            "detail": "A temporary localhost callback completes the login automatically once you approve. Deny to skip MCP tools from this server.",
            "options": ["Approve", "Deny"],
        });
        let answers = ctx
            .questions
            .ask(&ctx.chat_id, vec![question], 300)
            .await
            .map_err(|error| format!("MCP OAuth for '{}' not approved: {error}", server.name))?;
        let approved = answers.iter().any(|answer| {
            answer
                .get("selected")
                .and_then(Value::as_array)
                .is_some_and(|selected| {
                    selected.iter().any(|item| {
                        item.as_str()
                            .is_some_and(|value| value.eq_ignore_ascii_case("approve"))
                    })
                })
        });
        if !approved {
            return Err(format!("MCP OAuth for '{}' was not approved", server.name));
        }
        let callback = wait_for_oauth_callback(listener, Duration::from_secs(300)).await?;
        oauth
            .handle_callback_with_issuer(&callback.code, &callback.state, callback.iss.as_deref())
            .await
            .map_err(|error| format!("MCP OAuth callback failed for '{}': {error}", server.name))?;
        // Persist refresh credentials so the next restart reconnects silently.
        match oauth.get_credentials().await {
            Ok((client_id, Some(token_response))) => {
                let stored = StoredOAuthCredentials {
                    client_id,
                    token_response: serde_json::to_value(&token_response).unwrap_or(Value::Null),
                };
                let path = oauth_token_path(&self.root, &server.name);
                if let Some(parent) = path.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                if let Ok(raw) = serde_json::to_string_pretty(&stored) {
                    let _ = tokio::fs::write(&path, raw).await;
                }
            }
            Ok((_, None)) => {}
            Err(error) => {
                tracing::debug!("[mcp] OAuth credential snapshot failed: {error}");
            }
        }
        let manager = oauth.into_authorization_manager().ok_or_else(|| {
            format!(
                "MCP OAuth for '{}' did not yield an authorization manager",
                server.name
            )
        })?;
        let auth_client =
            rmcp::transport::auth::AuthClient::new(reqwest013::Client::default(), manager);
        let transport = rmcp::transport::StreamableHttpClientTransport::with_client(
            auth_client,
            rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(
                server.config.url.clone(),
            ),
        );
        self.install_authorized(server, transport).await
    }

    pub async fn upsert_server(
        &self,
        name: &str,
        config: ServerConfig,
    ) -> Result<Vec<Value>, String> {
        if !valid_name(name) {
            return Err("server name must contain only letters, digits, _ or -".to_string());
        }
        let mut cfg = self.load_config();
        cfg.servers.insert(name.to_string(), config.clone());
        self.save_config(&cfg)?;
        self.replace_server(name, config).await;
        Ok(self.status_snapshot())
    }

    pub async fn remove_server(&self, name: &str) -> Result<Vec<Value>, String> {
        let mut cfg = self.load_config();
        cfg.servers.remove(name);
        self.save_config(&cfg)?;
        let old = self
            .servers
            .write()
            .ok()
            .and_then(|mut servers| servers.remove(name));
        if let Some(server) = old
            && let Some(session) = server.session.lock().await.take()
        {
            close_session(session).await;
        }
        Ok(self.status_snapshot())
    }

    pub async fn restart_server(&self, name: &str) -> Result<Vec<Value>, String> {
        let config = self
            .load_config()
            .servers
            .get(name)
            .cloned()
            .ok_or_else(|| format!("server '{name}' not configured"))?;
        self.replace_server(name, config).await;
        Ok(self.status_snapshot())
    }

    async fn replace_server(&self, name: &str, config: ServerConfig) {
        let old = self.servers.write().ok().and_then(|mut servers| {
            let old = servers.remove(name);
            servers.insert(
                name.to_string(),
                Arc::new(McpServer::new(name.to_string(), config)),
            );
            old
        });
        if let Some(old) = old
            && let Some(session) = old.session.lock().await.take()
        {
            close_session(session).await;
        }
        let server = self
            .servers
            .read()
            .ok()
            .and_then(|servers| servers.get(name).cloned());
        if let Some(server) = server {
            self.connect_server(server).await;
        }
    }
}

pub fn init(root: impl Into<std::path::PathBuf>) -> Arc<McpManager> {
    McpManager::init(root)
}

pub fn tool_specs_for_scope(scope: &str) -> Vec<Value> {
    McpManager::global()
        .map(|manager| manager.tool_specs_for_scope(scope))
        .unwrap_or_default()
}

/// Rank specs by case-insensitive token match on name + description.
pub(crate) fn rank_tools(specs: &[Value], query: &str, limit: usize) -> Vec<Value> {
    let limit = limit.clamp(1, 20);
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|term| term.to_ascii_lowercase())
        .filter(|term| !term.is_empty())
        .collect();
    let mut scored: Vec<(usize, &Value)> = specs
        .iter()
        .map(|spec| {
            let function = spec.get("function").unwrap_or(&Value::Null);
            let name = function
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase();
            let description = function
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase();
            let score = if terms.is_empty() {
                1
            } else {
                terms
                    .iter()
                    .map(|term| {
                        let mut score = 0;
                        if name.contains(term) {
                            score += 3;
                        }
                        if description.contains(term) {
                            score += 1;
                        }
                        score
                    })
                    .sum()
            };
            (score, spec)
        })
        .filter(|(score, _)| *score > 0)
        .collect();
    scored.sort_by_key(|left| std::cmp::Reverse(left.0));
    scored
        .into_iter()
        .take(limit)
        .map(|(_, spec)| spec.clone())
        .collect()
}

/// `tool_search`: find MCP tools by keyword, return their schemas, and enable
/// them for the next provider request.
pub async fn search_tools(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(8)
        .clamp(1, 20) as usize;
    let Some(manager) = McpManager::global() else {
        return ToolOutcome::ok("[MCP] no MCP servers are configured".to_string());
    };
    let specs = manager.search(&ctx.scope, &query, limit);
    if specs.is_empty() {
        return ToolOutcome::ok(format!(
            "[MCP] no tools matched '{query}'. Try a broader keyword."
        ));
    }
    if let Ok(mut active) = ctx.active_mcp.lock() {
        for spec in &specs {
            if let Some(name) = spec
                .get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
            {
                active.insert(name.to_string());
            }
        }
    }
    let functions: Vec<Value> = specs
        .iter()
        .filter_map(|spec| spec.get("function").cloned())
        .collect();
    ToolOutcome::ok(format!(
        "[MCP TOOLS] {} tool(s) enabled for the next request. Call them by the full name.\n{}",
        functions.len(),
        serde_json::to_string_pretty(&functions).unwrap_or_else(|_| "[]".to_string())
    ))
}

pub async fn call(fq_name: &str, args: &Value, ctx: &ToolContext) -> ToolOutcome {
    let Some(manager) = McpManager::global() else {
        return ToolOutcome::err_class(ToolErrorClass::Transport, "Error: MCP is not initialized");
    };
    match manager.call(fq_name, args.clone(), ctx).await {
        // Pi-class budget (50KB / 2000 lines, line-aware head + tail with
        // omitted counts) instead of the old 20k head-only cut: truncation
        // must never hide what the agent needs.
        Ok(text) => ToolOutcome::ok(truncate_tool_output(&text).text),
        Err(error) => {
            // Transport faults and auth challenges are not the tool's answer;
            // everything else is a normal tool error the model should read.
            let class = if is_auth_error(&error) {
                ToolErrorClass::Blocked
            } else if transport_failure(&error) {
                ToolErrorClass::Transport
            } else {
                ToolErrorClass::Tool
            };
            ToolOutcome::err_class(
                class,
                format!(
                    "Error: MCP call failed: {error}. Check the server status; restart or reconnect the MCP server, then retry."
                ),
            )
        }
    }
}

fn transport_failure(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("broken pipe")
        || error.contains("connection")
        || error.contains("channel closed")
        || error.contains("closed")
        || error.contains("eof")
        || error.contains("timed out")
        || error.contains("timeout")
}

/// True when the server rejected the call because the tool is not in its
/// current catalog: our cache is stale and a refresh may fix it.
fn is_unknown_tool_error(error: &str) -> bool {
    error.to_ascii_lowercase().contains("unknown tool")
}

/// True when the tool-name sets differ (order-insensitive). Additions and
/// removals both invalidate the cached provider specs.
fn tool_names_changed(old: &[Value], new: &[Value]) -> bool {
    if old.len() != new.len() {
        return true;
    }
    let mut old_names: Vec<&str> = old
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .collect();
    let mut new_names: Vec<&str> = new
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .collect();
    old_names.sort_unstable();
    new_names.sort_unstable();
    old_names != new_names
}

/// Resolve a requested tool name against the cached `tools/list` result:
/// exact match first, then the sanitized form used in `mcp__server__tool`
/// names. Falls back to the requested name so the server can report the
/// real error (e.g. unknown tool after a catalog change).
fn resolve_cached_tool(cached: &[Value], tool_name: &str) -> String {
    cached
        .iter()
        .find_map(|tool| {
            let name = tool.get("name").and_then(Value::as_str)?;
            (name == tool_name || sanitize(name) == tool_name).then(|| name.to_string())
        })
        .unwrap_or_else(|| tool_name.to_string())
}

#[allow(dead_code)]
fn truncate_output(value: &str, cap: usize) -> String {
    let count = value.chars().count();
    if count <= cap {
        return value.to_string();
    }
    let mut out: String = value.chars().take(cap).collect();
    out.push_str(&format!(
        "\n[MCP output truncated at {cap} chars; {count} chars total. Request a narrower query or page the result.]"
    ));
    out
}

fn tool_spec(server: &str, tool: &str, definition: &Value) -> Value {
    let description = definition
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("MCP tool");
    let schema = definition
        .get("inputSchema")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let properties = schema
        .get("properties")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let required = schema.get("required").cloned().unwrap_or_else(|| json!([]));
    json!({
        "type": "function",
        "function": {
            "name": fq_name(server, tool),
            "description": truncate(description, 400),
            "parameters": {"type": "object", "properties": properties, "required": required},
        }
    })
}

fn fq_name(server: &str, tool: &str) -> String {
    format!("mcp__{}__{}", sanitize(server), sanitize(tool))
}

fn parse_fq(name: &str) -> Result<(String, String), String> {
    let mut parts = name.split("__");
    if parts.next() != Some("mcp") {
        return Err(format!("invalid MCP tool name '{name}'"));
    }
    let server = parts
        .next()
        .ok_or_else(|| format!("invalid MCP tool name '{name}'"))?;
    let tool = parts.collect::<Vec<_>>().join("__");
    if server.is_empty() || tool.is_empty() {
        return Err(format!("invalid MCP tool name '{name}'"));
    }
    Ok((server.to_string(), tool))
}

fn sanitize(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .take(48)
        .collect();
    let cleaned = cleaned.trim_matches('_').to_string();
    if cleaned.is_empty() {
        "x".to_string()
    } else {
        cleaned
    }
}

fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn canonical_scope(scope: &str) -> String {
    let scope = scope.trim().to_ascii_lowercase();
    if scope == "dev" {
        "developer".to_string()
    } else {
        scope
    }
}

fn scope_matches(scopes: &[String], scope: &str) -> bool {
    if scopes.is_empty() {
        return true;
    }
    let scope = canonical_scope(scope);
    scopes
        .iter()
        .any(|candidate| canonical_scope(candidate) == scope)
}

fn truncate(value: &str, cap: usize) -> String {
    value.chars().take(cap).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_failures_are_distinguished_from_tool_errors() {
        assert!(transport_failure("MCP server closed stdout"));
        assert!(transport_failure("MCP tool x timed out"));
        assert!(transport_failure("failed to write stdin: broken pipe"));
        assert!(!transport_failure("tool returned an error: invalid input"));
    }

    #[test]
    fn list_changed_flag_round_trips() {
        let server = McpServer::new("fake".to_string(), ServerConfig::default());
        assert!(!server.take_changed());
        server.list_changed.store(true, Ordering::Release);
        assert!(server.take_changed());
        assert!(!server.take_changed());
    }

    #[test]
    fn unknown_tool_errors_trigger_refresh() {
        assert!(is_unknown_tool_error("MCP error: Unknown tool: read_thing"));
        assert!(is_unknown_tool_error("unknown tool 'x'"));
        assert!(!is_unknown_tool_error(
            "tool returned an error: invalid input"
        ));
        assert!(!is_unknown_tool_error("MCP tool x timed out"));
    }

    #[test]
    fn auth_errors_trigger_login_flow() {
        assert!(is_auth_error(
            "authorization required: Bearer realm=\"mcp\""
        ));
        assert!(is_auth_error("HTTP 401 authorization required"));
        assert!(is_auth_error("MCP error Insufficient scope"));
        assert!(is_auth_error("OAuth refresh token was rejected"));
        assert!(is_auth_error("token refresh failed: timeout"));
        assert!(!is_auth_error("tool returned an error: invalid input"));
        assert!(!is_auth_error("MCP tool x timed out"));
        assert!(!is_auth_error("unknown tool 'x'"));
    }

    #[test]
    fn oauth_callback_query_is_parsed() {
        let parsed = parse_oauth_callback(
            "GET /callback?code=abc%201&state=xyz&iss=https%3A%2F%2Fidp HTTP/1.1",
        )
        .unwrap();
        assert_eq!(parsed.code, "abc 1");
        assert_eq!(parsed.state, "xyz");
        assert_eq!(parsed.iss.as_deref(), Some("https://idp"));
        // State is required to prevent CSRF confusion.
        assert!(parse_oauth_callback("GET /callback?code=only HTTP/1.1").is_none());
        assert!(parse_oauth_callback("garbage").is_none());
        assert!(parse_oauth_callback("").is_none());
        assert_eq!(url_decode("a+b%2Fc"), "a b/c");
        assert_eq!(url_decode("plain"), "plain");
    }

    #[test]
    fn oauth_token_path_is_filename_safe() {
        let path = oauth_token_path(std::path::Path::new("/root"), "../../evil");
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("evil.json")
        );
        let path = oauth_token_path(std::path::Path::new("/root"), "ok-name_1");
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("ok-name_1.json")
        );
    }

    #[test]
    fn catalog_changes_are_detected_by_name_set() {
        let old = vec![json!({"name": "a"}), json!({"name": "b"})];
        assert!(!tool_names_changed(
            &old,
            &[json!({"name": "b"}), json!({"name": "a"})]
        ));
        assert!(tool_names_changed(&old, &[json!({"name": "a"})]));
        assert!(tool_names_changed(
            &old,
            &[json!({"name": "a"}), json!({"name": "c"})]
        ));
        assert!(tool_names_changed(&[], &[json!({"name": "a"})]));
        assert!(!tool_names_changed(&[], &[]));
    }

    #[test]
    fn cached_tool_names_resolve_exact_then_sanitized() {
        let cached = vec![json!({"name": "read/thing"}), json!({"name": "other"})];
        assert_eq!(resolve_cached_tool(&cached, "read/thing"), "read/thing");
        assert_eq!(resolve_cached_tool(&cached, "read_thing"), "read/thing");
        // Unknown names pass through so the server reports the real error.
        assert_eq!(resolve_cached_tool(&cached, "missing"), "missing");
    }

    #[test]
    fn tool_search_ranks_name_matches_first() {
        let specs = vec![
            json!({"function": {"name": "mcp__gh__create_issue", "description": "Open a GitHub issue"}}),
            json!({"function": {"name": "mcp__db__query", "description": "Run SQL"}}),
        ];
        let hits = rank_tools(&specs, "create issue", 8);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(
            hits[0]["function"]["name"].as_str(),
            Some("mcp__gh__create_issue")
        );
        // Empty query lists everything up to the limit.
        assert_eq!(rank_tools(&specs, "", 8).len(), 2);
        assert_eq!(rank_tools(&specs, "run sql", 1).len(), 1);
    }

    #[test]
    fn fq_names_round_trip_and_sanitize() {
        let name = fq_name("my server", "read/thing");
        assert_eq!(name, "mcp__my_server__read_thing");
        let (server, tool) = parse_fq(&name).unwrap();
        assert_eq!(server, "my_server");
        assert_eq!(tool, "read_thing");
        assert!(parse_fq("not_mcp").is_err());
    }

    #[test]
    fn scopes_are_canonical_and_empty_is_global() {
        assert!(scope_matches(&[], "anything"));
        assert!(scope_matches(&["developer".to_string()], "Dev"));
        assert!(scope_matches(&["Dev".to_string()], "developer"));
        assert!(!scope_matches(&["ui".to_string()], "developer"));
    }

    #[test]
    fn tool_spec_uses_remote_schema() {
        let spec = tool_spec(
            "srv",
            "search",
            &json!({
                "description": "Search",
                "inputSchema": {"properties": {"q": {"type": "string"}}, "required": ["q"]}
            }),
        );
        assert_eq!(spec["function"]["name"], "mcp__srv__search");
        assert_eq!(spec["function"]["parameters"]["required"][0], "q");
    }

    #[test]
    fn config_defaults_are_sane() {
        let config: ServerConfig = serde_json::from_value(json!({"command": "npx"})).unwrap();
        assert_eq!(config.approval, "auto");
        assert_eq!(config.startup_timeout_sec, 10.0);
        assert_eq!(config.tool_timeout_sec, 60.0);
        assert!(valid_name("my-server_1"));
        assert!(!valid_name("bad name"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_bare_commands_use_cmd_shim() {
        assert!(needs_cmd_shim("browse"));
        assert!(needs_cmd_shim("npx"));
        assert!(needs_cmd_shim("thing.cmd"));
        assert!(needs_cmd_shim("thing.bat"));
        assert!(!needs_cmd_shim(r"C:\tools\browse.exe"));
        assert!(!needs_cmd_shim(r"C:\tools\node.exe"));
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_commands_run_directly() {
        assert!(!needs_cmd_shim("browse"));
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;
    use crate::ToolContext;
    use std::sync::Arc;
    use std::time::Duration;

    fn fake_server_script() -> &'static str {
        r#"
import readline from 'node:readline';

const rl = readline.createInterface({ input: process.stdin });
rl.on('line', (line) => {
  const text = line.trim();
  if (!text) return;
  const req = JSON.parse(text);
  const method = req.method;
  let result = {};
  if (method === 'initialize') {
    result = {
      protocolVersion: '2025-06-18',
      capabilities: {},
      serverInfo: { name: 'fake', version: '1' },
    };
  } else if (method === 'tools/list') {
    result = {
      tools: [{
        name: 'echo',
        description: 'Echo text',
        inputSchema: {
          type: 'object',
          properties: { text: { type: 'string' } },
          required: ['text'],
        },
      }],
    };
  } else if (method === 'tools/call') {
    const value = (req.params || {}).arguments?.text ?? '';
    result = { content: [{ type: 'text', text: 'echo:' + value }] };
  } else if (method === 'notifications/initialized') {
    return;
  }
  if (Object.hasOwn(req, 'id')) {
    process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: req.id, result }) + '\n');
  }
});
"#
    }

    #[tokio::test]
    async fn stdio_server_lists_and_calls_a_tool() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake_mcp.mjs");
        std::fs::write(&script, fake_server_script()).unwrap();
        let config = json!({"servers": {"fake": {
            "command": "node",
            "args": [script.to_string_lossy()],
            "scopes": ["developer"],
            "approval": "auto",
            "startup_timeout_sec": 10,
            "tool_timeout_sec": 10
        }}});
        let config_dir = dir.path().join(".deep_agents");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("mcp_servers.json"), config.to_string()).unwrap();
        let manager = Arc::new(McpManager {
            root: dir.path().to_path_buf(),
            servers: RwLock::new(BTreeMap::new()),
        });
        manager.clone().start_all().await;
        // start_all spawns the handshake in the background. CI Windows runners
        // can take several seconds to start `node` (Defender scan), so poll
        // well past the configured 10s startup timeout instead of 2s.
        let mut tools = Vec::new();
        for _ in 0..300 {
            tools = manager.tool_specs_for_scope("developer");
            if !tools.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(!tools.is_empty(), "fake MCP tools did not appear");
        assert_eq!(tools[0]["function"]["name"], "mcp__fake__echo");
        let ctx = ToolContext::new(
            dir.path(),
            "mcp-test",
            Arc::new(crate::QuestionBroker::new()),
        )
        .with_scope("developer");
        let result = manager
            .call("mcp__fake__echo", json!({"text": "hi"}), &ctx)
            .await
            .unwrap();
        assert_eq!(result, "echo:hi");
        let _ = manager.remove_server("fake").await;
    }

    fn notifying_server_script() -> &'static str {
        r#"
import readline from 'node:readline';

const rl = readline.createInterface({ input: process.stdin });
rl.on('line', (line) => {
  const text = line.trim();
  if (!text) return;
  const req = JSON.parse(text);
  const method = req.method;
  let result = {};
  if (method === 'initialize') {
    result = {
      protocolVersion: '2025-06-18',
      capabilities: {},
      serverInfo: { name: 'fake', version: '1' },
    };
  } else if (method === 'tools/list') {
    result = {
      tools: [{
        name: 'echo',
        description: 'Echo text',
        inputSchema: {
          type: 'object',
          properties: { text: { type: 'string' } },
          required: ['text'],
        },
      }],
    };
  } else if (method === 'tools/call') {
    const value = (req.params || {}).arguments?.text ?? '';
    result = { content: [{ type: 'text', text: 'echo:' + value }] };
  } else if (method === 'notifications/initialized') {
    return;
  }
  if (Object.hasOwn(req, 'id')) {
    process.stdout.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/tools/list_changed' }) + '\n');
    process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: req.id, result }) + '\n');
  }
});
"#
    }

    /// A catalog invalidation arriving with a response is recorded and
    /// refreshed without breaking the in-flight call.
    #[tokio::test]
    async fn list_changed_notification_refreshes_without_breaking_calls() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake_notify_mcp.mjs");
        std::fs::write(&script, notifying_server_script()).unwrap();
        let config = json!({"servers": {"fake": {
            "command": "node",
            "args": [script.to_string_lossy()],
            "scopes": ["developer"],
            "approval": "auto",
            "startup_timeout_sec": 10,
            "tool_timeout_sec": 10
        }}});
        let config_dir = dir.path().join(".deep_agents");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("mcp_servers.json"), config.to_string()).unwrap();
        let manager = Arc::new(McpManager {
            root: dir.path().to_path_buf(),
            servers: RwLock::new(BTreeMap::new()),
        });
        manager.clone().start_all().await;
        for _ in 0..300 {
            if manager.tool_revision("fake") == Some(1) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(manager.tool_revision("fake"), Some(1));
        let ctx = ToolContext::new(
            dir.path(),
            "mcp-test",
            Arc::new(crate::QuestionBroker::new()),
        )
        .with_scope("developer");
        // Every response carries a list_changed notification; the call still
        // succeeds and the catalog (unchanged names) keeps its revision.
        let result = manager
            .call("mcp__fake__echo", json!({"text": "hi"}), &ctx)
            .await
            .unwrap();
        assert_eq!(result, "echo:hi");
        assert_eq!(manager.tool_revision("fake"), Some(1));
        let _ = manager.remove_server("fake").await;
    }

    fn dynamic_server_script() -> &'static str {
        r#"
import readline from 'node:readline';
import fs from 'node:fs';

const marker = process.argv[2];
const rl = readline.createInterface({ input: process.stdin });
rl.on('line', (line) => {
  const text = line.trim();
  if (!text) return;
  const req = JSON.parse(text);
  const method = req.method;
  let result = {};
  if (method === 'initialize') {
    result = {
      protocolVersion: '2025-06-18',
      capabilities: {},
      serverInfo: { name: 'fake', version: '1' },
    };
  } else if (method === 'tools/list') {
    const tools = [{
      name: 'echo',
      description: 'Echo text',
      inputSchema: { type: 'object', properties: { text: { type: 'string' } } },
    }];
    if (marker && fs.existsSync(marker)) {
      tools.push({
        name: 'extra',
        description: 'Added later',
        inputSchema: { type: 'object', properties: {} },
      });
    }
    result = { tools };
  } else if (method === 'tools/call') {
    const value = (req.params || {}).arguments?.text ?? '';
    result = { content: [{ type: 'text', text: 'echo:' + value }] };
  } else if (method === 'notifications/initialized') {
    return;
  }
  if (Object.hasOwn(req, 'id')) {
    process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: req.id, result }) + '\n');
  }
});
"#
    }

    /// A server that gains a tool mid-run is picked up by refresh: the
    /// revision bumps only when the name set actually changes.
    #[tokio::test]
    async fn refresh_picks_up_added_tools_and_bumps_revision() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake_dynamic_mcp.mjs");
        std::fs::write(&script, dynamic_server_script()).unwrap();
        let marker = dir.path().join("v2");
        let config = json!({"servers": {"fake": {
            "command": "node",
            "args": [script.to_string_lossy(), marker.to_string_lossy()],
            "scopes": ["developer"],
            "approval": "auto",
            "startup_timeout_sec": 10,
            "tool_timeout_sec": 10
        }}});
        let config_dir = dir.path().join(".deep_agents");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("mcp_servers.json"), config.to_string()).unwrap();
        let manager = Arc::new(McpManager {
            root: dir.path().to_path_buf(),
            servers: RwLock::new(BTreeMap::new()),
        });
        manager.clone().start_all().await;
        for _ in 0..300 {
            if manager.tool_revision("fake") == Some(1) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(manager.tool_revision("fake"), Some(1));
        assert_eq!(manager.tool_specs_for_scope("developer").len(), 1);
        std::fs::write(&marker, "v2").unwrap();
        assert!(manager.refresh_server("fake").await.unwrap());
        assert_eq!(manager.tool_revision("fake"), Some(2));
        assert_eq!(manager.tool_specs_for_scope("developer").len(), 2);
        // Refreshing an unchanged catalog reports no change and keeps the
        // revision stable so provider prompt caches survive.
        assert!(!manager.refresh_server("fake").await.unwrap());
        assert_eq!(manager.tool_revision("fake"), Some(2));
        let _ = manager.remove_server("fake").await;
    }

    fn eliciting_server_script() -> &'static str {
        r#"
import readline from 'node:readline';

const rl = readline.createInterface({ input: process.stdin });
let pendingCall = null;
rl.on('line', (line) => {
  const text = line.trim();
  if (!text) return;
  const req = JSON.parse(text);
  const method = req.method;
  let result = {};
  if (method === 'initialize') {
    result = {
      protocolVersion: '2025-06-18',
      capabilities: {},
      serverInfo: { name: 'fake', version: '1' },
    };
  } else if (method === 'tools/list') {
    result = {
      tools: [{
        name: 'ask_name',
        description: 'Ask for a name',
        inputSchema: { type: 'object', properties: {} },
      }],
    };
  } else if (method === 'tools/call') {
    pendingCall = req.id;
    process.stdout.write(JSON.stringify({
      jsonrpc: '2.0', id: 'elicit-1', method: 'elicitation/create',
      params: {
        mode: 'form',
        message: 'Your name?',
        requestedSchema: { type: 'object', properties: { name: { type: 'string' } }, required: ['name'] },
      },
    }) + '\n');
    return;
  } else if (method === 'notifications/initialized') {
    return;
  } else if (req.id === 'elicit-1') {
    const name = ((req.result || {}).content || {}).name ?? 'stranger';
    const callId = pendingCall;
    pendingCall = null;
    process.stdout.write(JSON.stringify({
      jsonrpc: '2.0', id: callId, result: { content: [{ type: 'text', text: 'hello ' + name }] },
    }) + '\n');
    return;
  }
  if (Object.hasOwn(req, 'id')) {
    process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: req.id, result }) + '\n');
  }
});
"#
    }

    /// A server elicitation round-trips through the question broker: the
    /// test answers the `mcp-elicit` batch and the provided value lands in
    /// the tool result.
    #[tokio::test]
    async fn elicitation_form_answers_flow_into_call() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake_elicit_mcp.mjs");
        std::fs::write(&script, eliciting_server_script()).unwrap();
        let config = json!({"servers": {"fake": {
            "command": "node",
            "args": [script.to_string_lossy()],
            "scopes": ["developer"],
            "approval": "auto",
            "startup_timeout_sec": 10,
            "tool_timeout_sec": 60
        }}});
        let config_dir = dir.path().join(".deep_agents");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("mcp_servers.json"), config.to_string()).unwrap();
        let manager = Arc::new(McpManager {
            root: dir.path().to_path_buf(),
            servers: RwLock::new(BTreeMap::new()),
        });
        manager.clone().start_all().await;
        for _ in 0..300 {
            if manager.tool_revision("fake") == Some(1) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(manager.tool_revision("fake"), Some(1));
        let ctx = ToolContext::new(
            dir.path(),
            "mcp-test",
            Arc::new(crate::QuestionBroker::new()),
        )
        .with_scope("developer");
        let broker = ctx.questions.clone();
        let call_manager = manager.clone();
        let handle = tokio::spawn(async move {
            call_manager
                .call("mcp__fake__ask_name", json!({}), &ctx)
                .await
        });
        let mut answered = false;
        for _ in 0..300 {
            let mut target = None;
            for batch in broker.pending_batches() {
                let is_elicit = batch
                    .get("questions")
                    .and_then(Value::as_array)
                    .is_some_and(|questions| {
                        questions.iter().any(|question| {
                            question.get("id").and_then(Value::as_str) == Some("mcp-elicit")
                        })
                    });
                if is_elicit {
                    target = batch
                        .get("batch_id")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    break;
                }
            }
            if let Some(batch_id) = target {
                assert!(broker.answer(
                    &batch_id,
                    vec![json!({"id": "mcp-elicit", "values": {"name": "Ada"}})]
                ));
                answered = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(answered, "elicitation question never appeared");
        let result = handle.await.unwrap().expect("elicited call succeeds");
        assert_eq!(result, "hello Ada");
        let _ = manager.remove_server("fake").await;
    }
}
