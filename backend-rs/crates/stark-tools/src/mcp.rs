//! Minimal MCP client: stdio and Streamable-HTTP JSON-RPC.
//!
//! Config is the existing `.deep_agents/mcp_servers.json` format. Tools are
//! exposed dynamically as `mcp__<server>__<tool>` and appended to the normal
//! per-scope tool list. Calls are serialized per server and bounded.

use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

use crate::{ToolContext, ToolOutcome};

const PROTOCOL_VERSION: &str = "2025-06-18";
const CLIENT_NAME: &str = "stark-rs";
const CLIENT_VERSION: &str = "2.0.0";
const MAX_TOOLS_PER_SERVER: usize = 500;

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
    transport: Mutex<Option<Transport>>,
}

impl McpServer {
    fn new(name: String, config: ServerConfig) -> Self {
        Self {
            name,
            config,
            status: RwLock::new("starting".to_string()),
            error: RwLock::new(String::new()),
            tools: RwLock::new(Vec::new()),
            transport: Mutex::new(None),
        }
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

enum Transport {
    Stdio(Box<StdioTransport>),
    Http(HttpTransport),
}

struct StdioTransport {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    child: Child,
    next_id: u64,
}

struct HttpTransport {
    client: reqwest::Client,
    url: String,
    headers: BTreeMap<String, String>,
    token: String,
    next_id: u64,
}

impl Transport {
    async fn start(config: &ServerConfig) -> Result<Self, String> {
        if !config.url.trim().is_empty() {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs_f64(config.tool_timeout_sec.max(1.0)))
                .build()
                .map_err(|error| error.to_string())?;
            let token = if config.bearer_token_env_var.trim().is_empty() {
                String::new()
            } else {
                std::env::var(config.bearer_token_env_var.trim()).unwrap_or_default()
            };
            return Ok(Self::Http(HttpTransport {
                client,
                url: config.url.clone(),
                headers: config.http_headers.clone(),
                token,
                next_id: 1,
            }));
        }
        if config.command.trim().is_empty() {
            return Err("stdio MCP server requires 'command'".to_string());
        }
        let mut command = if cfg!(windows)
            && (config.command.eq_ignore_ascii_case("npx")
                || config.command.eq_ignore_ascii_case("uvx")
                || config.command.to_ascii_lowercase().ends_with(".cmd"))
        {
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
        let mut child = command.spawn().map_err(|error| error.to_string())?;
        let stdin = child.stdin.take().ok_or("MCP child has no stdin")?;
        let stdout = child.stdout.take().ok_or("MCP child has no stdout")?;
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!("[mcp] {line}");
                }
            });
        }
        Ok(Self::Stdio(Box::new(StdioTransport {
            stdin,
            stdout: BufReader::new(stdout),
            child,
            next_id: 1,
        })))
    }

    async fn initialize(&mut self) -> Result<(), String> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": CLIENT_NAME, "version": CLIENT_VERSION},
        });
        let _ = self.request("initialize", params).await?;
        self.notify("notifications/initialized", json!({})).await
    }

    async fn list_tools(&mut self) -> Result<Vec<Value>, String> {
        let result = self.request("tools/list", json!({})).await?;
        Ok(result
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    async fn call_tool(
        &mut self,
        tool: &str,
        args: Value,
        timeout: Duration,
    ) -> Result<String, String> {
        let result = tokio::time::timeout(
            timeout,
            self.request("tools/call", json!({"name": tool, "arguments": args})),
        )
        .await
        .map_err(|_| format!("MCP tool {tool} timed out"))??;
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            return Err(content_text(&result));
        }
        let text = content_text(&result);
        if text.is_empty() {
            Ok(result.to_string())
        } else {
            Ok(text)
        }
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        match self {
            Self::Stdio(transport) => transport.request(method, params).await,
            Self::Http(transport) => transport.request(method, params).await,
        }
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        match self {
            Self::Stdio(transport) => transport.notify(method, params).await,
            Self::Http(transport) => transport.notify(method, params).await,
        }
    }

    async fn close(&mut self) {
        match self {
            Self::Stdio(transport) => {
                let _ = transport.child.kill().await;
                let _ = transport.child.wait().await;
            }
            Self::Http(_) => {}
        }
    }
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

impl StdioTransport {
    async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.stdin
            .write_all(format!("{line}\n").as_bytes())
            .await
            .map_err(|error| error.to_string())?;
        self.stdin
            .flush()
            .await
            .map_err(|error| error.to_string())?;
        loop {
            let mut response = String::new();
            let read = tokio::time::timeout(
                Duration::from_secs(60),
                self.stdout.read_line(&mut response),
            )
            .await
            .map_err(|_| format!("MCP request {method} timed out"))?
            .map_err(|error| error.to_string())?;
            if read == 0 {
                return Err("MCP server closed stdout".to_string());
            }
            let value: Value = serde_json::from_str(response.trim())
                .map_err(|error| format!("invalid MCP response for {method}: {error}"))?;
            if value.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = value.get("error") {
                return Err(error.to_string());
            }
            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        let line = json!({"jsonrpc": "2.0", "method": method, "params": params});
        self.stdin
            .write_all(format!("{line}\n").as_bytes())
            .await
            .map_err(|error| error.to_string())?;
        self.stdin.flush().await.map_err(|error| error.to_string())
    }
}

impl HttpTransport {
    async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let mut request = self
            .client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        if !self.token.is_empty() {
            request = request.bearer_auth(&self.token);
        }
        for (key, value) in &self.headers {
            request = request.header(key, value);
        }
        let response = request.send().await.map_err(|error| error.to_string())?;
        if !response.status().is_success() {
            return Err(format!("HTTP {}", response.status()));
        }
        let text = response.text().await.map_err(|error| error.to_string())?;
        let payload = if text.contains("data:") {
            text.lines()
                .rev()
                .find_map(|line| {
                    let line = line.strip_prefix("data:")?.trim();
                    (!line.is_empty()).then_some(line)
                })
                .unwrap_or("")
        } else {
            text.trim()
        };
        let value: Value = serde_json::from_str(payload)
            .map_err(|error| format!("invalid MCP HTTP response: {error}"))?;
        if let Some(error) = value.get("error") {
            return Err(error.to_string());
        }
        Ok(value.get("result").cloned().unwrap_or(Value::Null))
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        let mut request = self
            .client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .json(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
        if !self.token.is_empty() {
            request = request.bearer_auth(&self.token);
        }
        for (key, value) in &self.headers {
            request = request.header(key, value);
        }
        let response = request.send().await.map_err(|error| error.to_string())?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!("HTTP {}", response.status()))
        }
    }
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
    }

    async fn connect_server(&self, server: Arc<McpServer>) {
        let result = async {
            let mut transport = Transport::start(&server.config)
                .await
                .map_err(|error| error.to_string())?;
            transport.initialize().await?;
            let tools = transport.list_tools().await?;
            let mut guard = server.transport.lock().await;
            *guard = Some(transport);
            Ok::<_, String>(tools)
        }
        .await;
        match result {
            Ok(tools) => {
                if let Ok(mut current) = server.tools.write() {
                    *current = tools.into_iter().take(MAX_TOOLS_PER_SERVER).collect();
                }
                server.set_status("connected", "");
            }
            Err(error) => server.set_status("failed", &error),
        }
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
        specs
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
            .and_then(|tools| {
                tools.iter().find_map(|tool| {
                    let name = tool.get("name").and_then(Value::as_str)?;
                    (name == tool_name || sanitize(name) == tool_name).then(|| name.to_string())
                })
            })
            .unwrap_or_else(|| tool_name.clone());
        if server.config.approval == "prompt" {
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
        let mut transport = server.transport.lock().await;
        let transport = transport
            .as_mut()
            .ok_or_else(|| format!("MCP server '{server_name}' has no transport"))?;
        transport.call_tool(&actual_tool, args, timeout).await
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
            && let Some(mut transport) = server.transport.lock().await.take()
        {
            transport.close().await;
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
            && let Some(mut transport) = old.transport.lock().await.take()
        {
            transport.close().await;
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

pub async fn call(fq_name: &str, args: &Value, ctx: &ToolContext) -> ToolOutcome {
    let Some(manager) = McpManager::global() else {
        return ToolOutcome::err("Error: MCP is not initialized");
    };
    match manager.call(fq_name, args.clone(), ctx).await {
        Ok(text) => ToolOutcome::ok(truncate(&text, 20_000)),
        Err(error) => ToolOutcome::err(format!("Error: MCP call failed: {error}")),
    }
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
}

#[cfg(test)]
mod live_tests {
    use super::*;
    use crate::ToolContext;
    use std::sync::Arc;
    use std::time::Duration;

    fn fake_server_script() -> &'static str {
        r#"
import json, sys
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    method = req.get("method")
    if method == "initialize":
        result = {"protocolVersion": "2025-06-18", "capabilities": {}, "serverInfo": {"name": "fake", "version": "1"}}
    elif method == "tools/list":
        result = {"tools": [{"name": "echo", "description": "Echo text", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}}]}
    elif method == "tools/call":
        text = (req.get("params") or {}).get("arguments", {}).get("text", "")
        result = {"content": [{"type": "text", "text": "echo:" + text}]}
    elif method == "notifications/initialized":
        continue
    else:
        result = {}
    if "id" in req:
        print(json.dumps({"jsonrpc": "2.0", "id": req["id"], "result": result}), flush=True)
"#
    }

    #[tokio::test]
    async fn stdio_server_lists_and_calls_a_tool() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake_mcp.py");
        std::fs::write(&script, fake_server_script()).unwrap();
        let config = json!({"servers": {"fake": {
            "command": "python",
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
        let mut tools = Vec::new();
        for _ in 0..20 {
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
}
