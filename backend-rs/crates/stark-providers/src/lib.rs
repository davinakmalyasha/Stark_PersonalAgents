use std::pin::Pin;

use eventsource_stream::Eventsource;
use futures_util::{Stream, StreamExt};
use serde::ser::{SerializeStruct, Serializer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub mode: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

/// Optional per-response output cap. Unset by default (Codex parity: the
/// gateway applies the model's own limit). Set `STARK_MAX_OUTPUT_TOKENS` only
/// to bound spend; the 402 handler otherwise adapts automatically.
fn max_output_tokens() -> Option<u64> {
    std::env::var("STARK_MAX_OUTPUT_TOKENS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|value| value.clamp(256, 65_536))
}

/// OpenRouter-style 402s state what the balance can cover: "…can only afford
/// 26688". Returns that number so the request can retry with an affordable cap.
fn affordable_output_tokens(body: &str) -> Option<u64> {
    let marker = "can only afford ";
    let start = body.find(marker)? + marker.len();
    let digits: String = body[start..]
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// One-line credit error instead of the gateway's nested JSON dump.
fn credit_error(body: &str, affordable: Option<u64>) -> String {
    match affordable {
        Some(tokens) => format!(
            "OpenRouter: balance covers {tokens} output tokens. Reduce STARK_MAX_OUTPUT_TOKENS or top up credits."
        ),
        None => format!("OpenRouter rejected the request (402): {body}"),
    }
}

/// Extract the useful bit of a gateway error body.
fn gateway_message(body: &str) -> String {
    let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let message = value
        .pointer("/error/message")
        .or_else(|| value.pointer("/error/0/message"))
        .or_else(|| value.get("message"))
        .and_then(Value::as_str)
        .unwrap_or(body)
        .trim();
    message.chars().take(400).collect()
}

fn friendly_status(status: u16, body: &str, attempts: usize) -> String {
    let message = gateway_message(body);
    match status {
        401 | 403 => format!(
            "Authentication failed (HTTP {status}). Check STARK_API_KEY / OPENCODE_API_KEY. {message}"
        ),
        404 => format!("Model or endpoint not found (HTTP 404). Check the model id. {message}"),
        402 => format!("Insufficient credits (HTTP 402). {message}"),
        408 => format!("Provider timed out (HTTP 408) after {attempts} attempt(s). {message}"),
        429 => format!("Rate limited (HTTP 429) after {attempts} attempt(s). {message}"),
        500..=599 => {
            format!("Provider unavailable (HTTP {status}) after {attempts} attempt(s). {message}")
        }
        400 => format!("Request rejected (HTTP 400). {message}"),
        _ => format!("Provider error (HTTP {status}). {message}"),
    }
}

/// The only models the Stark UI offers. Single-provider setup: everything
/// runs through opencode go; the dropdown is a curated shortlist, not a
/// provider matrix.
pub const CURATED_MODELS: &[(&str, &str)] = &[
    ("deepseek-v4.1-flash", "DeepSeek V4.1 Flash"),
    ("mimo-v2.6-flash", "MiMo V2.6 Flash"),
    ("mimo-v2.6-pro", "MiMo V2.6 Pro"),
    ("muse-spark-1.3-contributor", "Muse Spark 1.3 Contributor"),
];

/// Canonical opencode go model id for a requested id or alias. Unknown ids
/// pass through untouched so custom models keep working.
pub fn canonical_opencode_model(requested: &str) -> String {
    match requested {
        "deepseek/deepseek-v4.1-flash-1017"
        | "deepseek/deepseek-v4.1-flash"
        | "deepseek-v4.1-flash"
        | "deepseek-flash"
        | "deepseek:v4.1-flash" => "deepseek-v4.1-flash",
        "xiaomi/mimo-v2.6-flash" | "mimo-v2.6-flash" | "mimo-flash" => "mimo-v2.6-flash",
        "xiaomi/mimo-v2.6-pro" | "mimo-v2.6-pro" | "mimo-pro" => "mimo-v2.6-pro",
        "meta/muse-spark-1.3-contributor" | "muse-spark-1.3-contributor" => {
            "muse-spark-1.3-contributor"
        }
        _ => requested,
    }
    .to_string()
}

impl ProviderConfig {
    pub fn from_env() -> Self {
        let requested_mode = std::env::var("STARK_PROVIDER")
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        let explicit_base = first_env(&["STARK_PROVIDER_BASE", "OPENAI_BASE_URL"]);
        let explicit_key = first_env(&["STARK_API_KEY", "OPENAI_API_KEY"]);
        let explicit_model = first_env(&["STARK_MODEL", "OPENAI_MODEL"]);

        if requested_mode == "openrouter" {
            return Self {
                mode: "openrouter".to_string(),
                base_url: explicit_base
                    .or_else(|| first_env(&["OPENROUTER_BASE_URL"]))
                    .unwrap_or_else(|| "https://openrouter.ai/api/v1".to_string()),
                api_key: explicit_key
                    .or_else(|| first_env(&["OPENROUTER_API_KEY"]))
                    .or_else(|| first_key(&["OPENROUTER_API_KEYS"]))
                    .unwrap_or_default(),
                model: explicit_model
                    .or_else(|| first_env(&["OPENROUTER_MODEL", "STARK_MODEL"]))
                    .unwrap_or_else(|| "deepseek/deepseek-v4.1-flash-1017".to_string()),
            };
        }

        if requested_mode == "command" || requested_mode == "commandcode" {
            return Self {
                mode: "command".to_string(),
                base_url: explicit_base
                    .or_else(|| first_env(&["COMMAND_CODE_BASE_URL"]))
                    .unwrap_or_else(|| "https://api.commandcode.ai/provider/v1".to_string()),
                api_key: explicit_key
                    .or_else(|| first_env(&["COMMAND_CODE_API_KEY"]))
                    .unwrap_or_default(),
                model: explicit_model
                    .or_else(|| first_env(&["COMMAND_CODE_MODEL_FLASH", "COMMAND_CODE_MODEL_ORCH"]))
                    .unwrap_or_else(|| "deepseek/deepseek-v4.1-flash".to_string()),
            };
        }

        // No explicit provider: opencode go is the default. Explicit
        // STARK_PROVIDER values (including "deepseek") still work as before.
        if requested_mode.is_empty() || requested_mode == "opencode" {
            return Self {
                mode: "opencode".to_string(),
                base_url: explicit_base
                    .or_else(|| first_env(&["DEEPSEEK_BASE_URL"]))
                    .unwrap_or_else(|| "https://opencode.ai/zen/go/v1".to_string()),
                api_key: explicit_key
                    .or_else(|| first_env(&["OPENCODE_API_KEY", "DEEPSEEK_API_KEY"]))
                    .unwrap_or_default(),
                model: explicit_model
                    .or_else(|| first_env(&["DEEPSEEK_MODEL_FLASH", "DEEPSEEK_MODEL"]))
                    .unwrap_or_else(|| "deepseek-v4.1-flash".to_string()),
            };
        }

        Self {
            mode: requested_mode,
            base_url: explicit_base
                .or_else(|| first_env(&["DEEPSEEK_BASE_URL"]))
                .unwrap_or_else(|| "https://api.deepseek.com".to_string()),
            api_key: explicit_key
                .or_else(|| first_env(&["DEEPSEEK_API_KEY", "OPENCODE_API_KEY"]))
                .unwrap_or_default(),
            model: explicit_model
                .or_else(|| first_env(&["DEEPSEEK_MODEL", "DEEPSEEK_MODEL_FLASH"]))
                .unwrap_or_else(|| "deepseek-chat".to_string()),
        }
    }

    pub fn resolve_model(&self, requested: &str) -> String {
        let requested = requested.trim();
        if requested.is_empty() || requested.eq_ignore_ascii_case("Automatic Fallback") {
            return self.model.clone();
        }
        if self.mode.as_str() == "opencode" {
            return canonical_opencode_model(requested);
        }
        if matches!(self.mode.as_str(), "command" | "openrouter") {
            // Curated bare ids map to gateway slugs; unknown ids pass through.
            match requested {
                "deepseek-v4.1-flash" | "deepseek-flash" | "deepseek:v4.1-flash" => {
                    return self.model.clone();
                }
                "mimo-v2.6-flash" | "mimo-flash" => {
                    return "xiaomi/mimo-v2.6-flash".to_string();
                }
                "mimo-v2.6-pro" | "mimo-pro" => {
                    return "xiaomi/mimo-v2.6-pro".to_string();
                }
                "muse-spark-1.3-contributor" => {
                    return "meta/muse-spark-1.3-contributor".to_string();
                }
                _ => {}
            }
            return requested.to_string();
        }
        requested.to_string()
    }

    pub fn endpoint(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        if base.ends_with("/chat/completions") {
            base.to_string()
        } else {
            format!("{base}/chat/completions")
        }
    }

    pub fn has_key(&self) -> bool {
        !self.api_key.trim().is_empty()
    }
}

fn first_env(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
    })
}

fn first_key(names: &[&str]) -> Option<String> {
    first_env(names).map(|value| {
        value
            .split(',')
            .map(str::trim)
            .find(|part| !part.is_empty())
            .unwrap_or("")
            .to_string()
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionCall,
}

impl ToolCall {
    pub fn function(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            kind: "function".to_string(),
            function: FunctionCall {
                name: name.into(),
                arguments: arguments.into(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImagePart {
    pub url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImagePart>,
}

impl Serialize for ChatMessage {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("ChatMessage", 5)?;
        state.serialize_field("role", &self.role)?;
        if self.images.is_empty() {
            if let Some(content) = &self.content {
                state.serialize_field("content", content)?;
            }
        } else {
            let mut parts = Vec::new();
            let mut text = self.content.clone().unwrap_or_default();
            for image in &self.images {
                if !image.note.is_empty() {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str("- ");
                    text.push_str(&image.note);
                }
            }
            if !text.is_empty() {
                parts.push(json!({"type": "text", "text": text}));
            }
            for image in &self.images {
                parts.push(json!({
                    "type": "image_url",
                    "image_url": {"url": image.url},
                }));
            }
            state.serialize_field("content", &parts)?;
        }
        if let Some(tool_calls) = &self.tool_calls {
            state.serialize_field("tool_calls", tool_calls)?;
        }
        if let Some(tool_call_id) = &self.tool_call_id {
            state.serialize_field("tool_call_id", tool_call_id)?;
        }
        if let Some(name) = &self.name {
            state.serialize_field("name", name)?;
        }
        state.end()
    }
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self::text("system", content)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::text("user", content)
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::text("assistant", content)
    }

    pub fn assistant_tool_calls(content: Option<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".to_string(),
            content,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            name: None,
            images: Vec::new(),
        }
    }

    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_string(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            name: None,
            images: Vec::new(),
        }
    }

    fn text(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.to_string(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            images: Vec::new(),
        }
    }

    pub fn user_with_images(content: impl Into<String>, images: Vec<ImagePart>) -> Self {
        Self {
            role: "user".to_string(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            images,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub cached_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub cost: f64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ModelPricing {
    /// USD per million input tokens.
    pub input: f64,
    /// USD per million cached input tokens.
    pub cached_input: f64,
    /// USD per million output tokens.
    pub output: f64,
}

impl ModelPricing {
    pub fn cost(&self, usage: &Usage) -> f64 {
        let cached = usage.cached_tokens.min(usage.input_tokens);
        let miss = usage.input_tokens.saturating_sub(cached);
        (miss as f64 * self.input
            + cached as f64 * self.cached_input
            + usage.output_tokens as f64 * self.output)
            / 1_000_000.0
    }
}

/// Pricing is provider-reported when available. When it is not, this reads
/// optional per-model or global USD-per-million overrides.
pub fn pricing_for(model: &str) -> ModelPricing {
    let key: String = model
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    let price = |suffix: &str| {
        std::env::var(format!("STARK_PRICING_{key}_{suffix}"))
            .ok()
            .or_else(|| std::env::var(format!("STARK_PRICING_{suffix}")).ok())
            .and_then(|value| value.trim().parse::<f64>().ok())
            .filter(|value| *value >= 0.0)
            .unwrap_or(0.0)
    };
    ModelPricing {
        input: price("INPUT"),
        cached_input: price("CACHED_INPUT"),
        output: price("OUTPUT"),
    }
}

#[derive(Debug, Clone)]
pub enum ProviderEvent {
    Text(String),
    Reasoning(String),
    ToolCall {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments: Option<String>,
    },
    Usage(Usage),
    Done {
        finish_reason: Option<String>,
    },
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("missing API key; set STARK_API_KEY, OPENAI_API_KEY, or DEEPSEEK_API_KEY")]
    MissingApiKey,
    #[error("{body}")]
    HttpStatus { status: u16, body: String },
    #[error("provider request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("{0}")]
    Transport(String),
    #[error("provider stream failed: {0}")]
    Stream(String),
    #[error("provider returned invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
}

pub type EventStream = Pin<Box<dyn Stream<Item = Result<ProviderEvent, ProviderError>> + Send>>;

#[derive(Debug, Clone, Default)]
pub struct Completion {
    pub text: String,
    pub usage: Usage,
}

#[derive(Clone)]
pub struct Provider {
    client: reqwest::Client,
    config: ProviderConfig,
}

/// Context windows reported by the provider's `/models` endpoint. Populated at
/// startup so compaction uses the real window instead of a hardcoded guess.
static MODEL_CONTEXTS: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, usize>>,
> = std::sync::OnceLock::new();

fn model_contexts() -> &'static std::sync::RwLock<std::collections::HashMap<String, usize>> {
    MODEL_CONTEXTS.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

/// Real context window for a model, when the provider reported one.
pub fn context_window(model: &str) -> Option<usize> {
    model_contexts()
        .read()
        .ok()
        .and_then(|map| map.get(model).copied())
}

pub fn remember_context_window(model: &str, window: usize) {
    if window == 0 || model.trim().is_empty() {
        return;
    }
    if let Ok(mut map) = model_contexts().write() {
        map.insert(model.to_string(), window);
    }
}

/// Parse OpenAI/OpenRouter model lists into `(id, context window)` pairs.
fn parse_model_windows(value: &Value) -> Vec<(String, usize)> {
    let models = value
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut windows = Vec::new();
    for model in models {
        let Some(id) = model.get("id").and_then(Value::as_str) else {
            continue;
        };
        let window = model
            .get("context_length")
            .or_else(|| model.get("max_context_length"))
            .or_else(|| {
                model
                    .get("top_provider")
                    .and_then(|provider| provider.get("context_length"))
            })
            .and_then(Value::as_u64)
            .filter(|value| *value > 0);
        if let Some(window) = window {
            windows.push((id.to_string(), window as usize));
        }
    }
    windows
}

/// Routing/cache identity for a provider request. opencode's gateway uses
/// `affinity` for cache routing and `session_id` for the concrete session.
#[derive(Clone, Copy, Debug)]
pub struct ProviderSession<'a> {
    pub session_id: &'a str,
    pub affinity: &'a str,
}

impl Provider {
    /// Fetch model metadata (context window) from the OpenAI-compatible
    /// `/models` endpoint and cache it for compaction. Best-effort: failures
    /// leave the conservative default in place.
    pub async fn refresh_model_contexts(&self) -> Result<usize, ProviderError> {
        let url = format!("{}/models", self.config.endpoint().trim_end_matches('/'));
        let response = self
            .client
            .get(url)
            .bearer_auth(&self.config.api_key)
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(ProviderError::HttpStatus {
                status: response.status().as_u16(),
                body: "model metadata unavailable".to_string(),
            });
        }
        let value: Value = response.json().await?;
        let windows = parse_model_windows(&value);
        for (id, window) in &windows {
            remember_context_window(id, *window);
        }
        Ok(windows.len())
    }

    pub fn new(config: ProviderConfig) -> Result<Self, ProviderError> {
        let client = reqwest::Client::builder()
            .user_agent("stark-rs/0.1")
            // Fail fast instead of hanging on a dead network, and cap a single
            // request so a stalled stream surfaces as an error, not silence.
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(1_800))
            .build()?;
        Ok(Self { client, config })
    }

    pub fn config(&self) -> &ProviderConfig {
        &self.config
    }

    /// Clone this provider with a different model. Used by background memory
    /// work so extraction/consolidation can run on a cheaper model without
    /// changing the foreground agent's model.
    pub fn with_model(&self, model: &str) -> Result<Self, ProviderError> {
        let model = model.trim();
        if model.is_empty() {
            return Ok(self.clone());
        }
        let mut config = self.config.clone();
        config.model = self.config.resolve_model(model);
        Self::new(config)
    }

    /// Collect a non-tool stream into one text result. Memory extraction and
    /// consolidation never call tools, so a plain helper keeps those paths
    /// small and testable.
    pub async fn complete_text_for_session(
        &self,
        messages: &[ChatMessage],
        temperature: f64,
        session: ProviderSession<'_>,
    ) -> Result<Completion, ProviderError> {
        let mut stream = self
            .stream_chat_for_session(messages, &[], temperature, session)
            .await?;
        let mut text = String::new();
        let mut usage = Usage::default();
        while let Some(event) = stream.next().await {
            match event? {
                ProviderEvent::Text(chunk) => text.push_str(&chunk),
                ProviderEvent::Usage(value) => usage = value,
                ProviderEvent::Done { .. } => break,
                ProviderEvent::Reasoning(_) | ProviderEvent::ToolCall { .. } => {}
            }
        }
        Ok(Completion {
            text: text.trim().to_string(),
            usage,
        })
    }

    pub async fn stream_chat(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        temperature: f64,
    ) -> Result<EventStream, ProviderError> {
        self.stream_chat_inner(messages, tools, temperature, None)
            .await
    }

    /// Stream a request with the session/affinity headers opencode's gateway
    /// needs. Other providers ignore the extra headers.
    pub async fn stream_chat_for_session(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        temperature: f64,
        session: ProviderSession<'_>,
    ) -> Result<EventStream, ProviderError> {
        self.stream_chat_inner(messages, tools, temperature, Some(session))
            .await
    }

    async fn stream_chat_inner(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        temperature: f64,
        session: Option<ProviderSession<'_>>,
    ) -> Result<EventStream, ProviderError> {
        if !self.config.has_key() {
            return Err(ProviderError::MissingApiKey);
        }

        let mut body = json!({
            "model": self.config.model,
            "messages": messages,
            "stream": true,
            "temperature": temperature,
            "stream_options": {"include_usage": true},
        });
        // No max_tokens by default: the provider applies the model's own
        // limit, exactly like Codex. Only an explicit env cap or a 402 retry
        // adds one.
        let mut max_output = max_output_tokens();
        if let Some(cap) = max_output {
            body["max_tokens"] = json!(cap);
        }
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools.to_vec());
            body["tool_choice"] = Value::String("auto".to_string());
        }

        let headers = session_headers(&self.config, session);
        let mut attempt = 0usize;
        let response = loop {
            let mut request = self
                .client
                .post(self.config.endpoint())
                .bearer_auth(&self.config.api_key);
            for (name, value) in &headers {
                request = request.header(*name, value);
            }
            let response = request.json(&body).send().await.map_err(|error| {
                if error.is_timeout() {
                    ProviderError::Transport(format!(
                        "Timed out connecting to {} after {}s. Check your network or the provider status.",
                        self.config.endpoint(),
                        15
                    ))
                } else if error.is_connect() {
                    ProviderError::Transport(format!(
                        "Cannot reach {} (connection failed). Check your network, proxy, or VPN.",
                        self.config.endpoint()
                    ))
                } else {
                    ProviderError::Transport(format!(
                        "Connection to {} failed: {error}",
                        self.config.endpoint()
                    ))
                }
            })?;
            if response.status().is_success() {
                break response;
            }
            let status = response.status().as_u16();
            let body_text = response.text().await.unwrap_or_default();
            if matches!(status, 429 | 500 | 502 | 503 | 504) && attempt < 2 {
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_millis(700 * attempt as u64)).await;
                continue;
            }
            // Low balance: ask for only what the account can cover and retry
            // instead of failing the whole turn.
            if status == 402 {
                let affordable = affordable_output_tokens(&body_text);
                if let Some(tokens) = affordable {
                    let retry_cap = tokens.clamp(256, 65_536);
                    let current = max_output.unwrap_or(u64::MAX);
                    if attempt < 2 && retry_cap < current {
                        attempt += 1;
                        max_output = Some(retry_cap);
                        body["max_tokens"] = json!(retry_cap);
                        continue;
                    }
                }
                return Err(ProviderError::HttpStatus {
                    status,
                    body: credit_error(&body_text, affordable),
                });
            }
            return Err(ProviderError::HttpStatus {
                status,
                body: friendly_status(status, &body_text, attempt + 1),
            });
        };

        let events = response.bytes_stream().eventsource();
        let stream = async_stream::try_stream! {
            let mut events = Box::pin(events);
            loop {
                // A stalled connection must surface as an error, not a hang.
                let next = match tokio::time::timeout(
                    std::time::Duration::from_secs(120),
                    events.next(),
                )
                .await
                {
                    Err(_) => Err(ProviderError::Stream(
                        "no data from the provider for 120s (stream stalled or connection lagging)".to_string(),
                    ))?,
                    Ok(None) => break,
                    Ok(Some(Err(error))) => Err(ProviderError::Stream(format!(
                        "connection lost mid-stream: {error}"
                    )))?,
                    Ok(Some(Ok(event))) => event,
                };
                for parsed in parse_sse_data(&next.data)? {
                    yield parsed;
                }
            }
        };

        Ok(Box::pin(stream))
    }
}

fn session_headers(
    config: &ProviderConfig,
    session: Option<ProviderSession<'_>>,
) -> Vec<(&'static str, String)> {
    if config.mode != "opencode" {
        return Vec::new();
    }
    let Some(session) = session else {
        return Vec::new();
    };
    let affinity = session.affinity.trim();
    if affinity.is_empty() {
        return Vec::new();
    }
    vec![
        ("x-opencode-session", affinity.to_string()),
        ("x-session-affinity", affinity.to_string()),
        ("X-Session-Id", affinity.to_string()),
        ("x-opencode-session-id", session.session_id.to_string()),
        ("x-opencode-client", "stark-rs".to_string()),
    ]
}

pub fn parse_sse_data(data: &str) -> Result<Vec<ProviderEvent>, ProviderError> {
    if data.trim() == "[DONE]" {
        return Ok(vec![ProviderEvent::Done {
            finish_reason: None,
        }]);
    }

    let value: Value = serde_json::from_str(data)?;
    if let Some(error) = value.get("error") {
        return Err(ProviderError::Stream(error.to_string()));
    }

    let mut out = Vec::new();
    if let Some(usage) = value.get("usage").filter(|usage| !usage.is_null()) {
        out.push(ProviderEvent::Usage(parse_usage(usage)));
    }

    let Some(choice) = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
    else {
        return Ok(out);
    };

    if let Some(delta) = choice.get("delta") {
        if let Some(reasoning) = delta
            .get("reasoning_content")
            .or_else(|| delta.get("reasoning"))
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
        {
            out.push(ProviderEvent::Reasoning(reasoning.to_string()));
        }
        if let Some(text) = delta
            .get("content")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
        {
            out.push(ProviderEvent::Text(text.to_string()));
        }
        if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for (position, tool_call) in tool_calls.iter().enumerate() {
                let index = tool_call
                    .get("index")
                    .and_then(Value::as_u64)
                    .map(|value| value as usize)
                    .unwrap_or(position);
                let id = tool_call
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let function = tool_call.get("function");
                let name = function
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let arguments = function
                    .and_then(|function| function.get("arguments"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                out.push(ProviderEvent::ToolCall {
                    index,
                    id,
                    name,
                    arguments,
                });
            }
        }
    }

    if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
        out.push(ProviderEvent::Done {
            finish_reason: Some(reason.to_string()),
        });
    }

    Ok(out)
}

fn parse_usage(value: &Value) -> Usage {
    let input_tokens = value
        .get("prompt_tokens")
        .or_else(|| value.get("input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = value
        .get("completion_tokens")
        .or_else(|| value.get("output_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cached_tokens = value
        .pointer("/prompt_tokens_details/cached_tokens")
        .or_else(|| value.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let reasoning_tokens = value
        .pointer("/completion_tokens_details/reasoning_tokens")
        .or_else(|| value.get("reasoning_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cost = value
        .get("cost")
        .or_else(|| value.get("total_cost"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    Usage {
        input_tokens,
        cached_tokens,
        output_tokens,
        reasoning_tokens,
        total_tokens: value
            .get("total_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(input_tokens + output_tokens),
        cost,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credit_affordability_parses_and_clamps() {
        let body = r#"{"error":{"message":"This request requires more credits, or fewer max_tokens. You requested up to 131072 tokens, but can only afford 26688. To increase, visit https://openrouter.ai/settings/credits and upgrade to a paid account"}}"#;
        assert_eq!(affordable_output_tokens(body), Some(26_688));
        assert_eq!(affordable_output_tokens("no numbers here"), None);
        // No default cap (Codex parity); an explicit env value is clamped.
        assert!(max_output_tokens().is_none());
    }

    #[test]
    fn parses_context_windows_from_model_list() {
        let value = json!({"data": [
            {"id": "test-context-model", "context_length": 262144},
            {"id": "mimo-v2.6-pro", "top_provider": {"context_length": 131072}},
            {"id": "no-window"}
        ]});
        let windows = parse_model_windows(&value);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].1, 262_144);
        assert_eq!(windows[1].1, 131_072);
        remember_context_window("test-context-model", 262_144);
        assert_eq!(context_window("test-context-model"), Some(262_144));
    }

    #[test]
    fn credit_error_is_one_line() {
        let text = credit_error("ignored", Some(26_688));
        assert!(
            text.contains("balance covers 26688 output tokens"),
            "{text}"
        );
        assert!(!text.contains('\n'));
    }

    #[test]
    fn parses_text_and_reasoning() {
        let events = parse_sse_data(
            r#"{"choices":[{"delta":{"reasoning_content":"think","content":"hello"},"finish_reason":null}]}"#,
        )
        .unwrap();
        assert!(matches!(&events[0], ProviderEvent::Reasoning(text) if text == "think"));
        assert!(matches!(&events[1], ProviderEvent::Text(text) if text == "hello"));
    }

    #[test]
    fn parses_tool_call_delta() {
        let events = parse_sse_data(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"read_file","arguments":"{\"file_path\":"}}]}}]}"#,
        )
        .unwrap();
        assert!(matches!(
            &events[0],
            ProviderEvent::ToolCall { index: 0, id: Some(id), name: Some(name), .. }
            if id == "c1" && name == "read_file"
        ));
    }

    #[test]
    fn parses_cached_reasoning_and_cost() {
        let usage = parse_usage(&json!({
            "prompt_tokens": 100,
            "completion_tokens": 20,
            "total_tokens": 120,
            "prompt_tokens_details": {"cached_tokens": 80},
            "completion_tokens_details": {"reasoning_tokens": 5},
            "cost": 0.0123
        }));
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.cached_tokens, 80);
        assert_eq!(usage.output_tokens, 20);
        assert_eq!(usage.reasoning_tokens, 5);
        assert!((usage.cost - 0.0123).abs() < f64::EPSILON);
        let pricing = ModelPricing {
            input: 1.0,
            cached_input: 0.1,
            output: 2.0,
        };
        let cost = pricing.cost(&usage);
        assert!((cost - 0.000068).abs() < 1e-9, "{cost}");
    }

    #[test]
    fn parses_usage_and_done() {
        let usage = parse_sse_data(
            r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":4,"total_tokens":14}}"#,
        )
        .unwrap();
        assert!(matches!(&usage[0], ProviderEvent::Usage(u) if u.total_tokens == 14));

        let done = parse_sse_data("[DONE]").unwrap();
        assert!(matches!(
            &done[0],
            ProviderEvent::Done {
                finish_reason: None
            }
        ));
    }

    #[test]
    fn image_messages_use_openai_content_parts() {
        let message = ChatMessage::user_with_images(
            "look here",
            vec![ImagePart {
                url: "data:image/png;base64,AAAA".to_string(),
                note: "screen".to_string(),
            }],
        );
        let value = serde_json::to_value(&message).unwrap();
        assert_eq!(value["role"], "user");
        let parts = value["content"].as_array().unwrap();
        assert_eq!(parts[0]["type"], "text");
        assert!(parts[0]["text"].as_str().unwrap().contains("screen"));
        assert_eq!(parts[1]["type"], "image_url");
        assert_eq!(parts[1]["image_url"]["url"], "data:image/png;base64,AAAA");
    }

    #[test]
    fn command_mode_maps_canonical_model_to_gateway_slug() {
        let config = ProviderConfig {
            mode: "command".to_string(),
            base_url: "https://api.commandcode.ai/provider/v1".to_string(),
            api_key: "test".to_string(),
            model: "deepseek/deepseek-v4.1-flash".to_string(),
        };
        assert_eq!(
            config.resolve_model("deepseek-v4.1-flash"),
            "deepseek/deepseek-v4.1-flash"
        );
        assert_eq!(
            config.resolve_model("poolside/laguna-s-2.1-free"),
            "poolside/laguna-s-2.1-free"
        );
    }

    #[test]
    fn gateway_modes_map_curated_ids_to_vendor_slugs() {
        for mode in ["command", "openrouter"] {
            let config = ProviderConfig {
                mode: mode.to_string(),
                base_url: "https://example.invalid".to_string(),
                api_key: "test".to_string(),
                model: "deepseek/deepseek-v4.1-flash-1017".to_string(),
            };
            assert_eq!(
                config.resolve_model("deepseek-v4.1-flash"),
                "deepseek/deepseek-v4.1-flash-1017"
            );
            assert_eq!(
                config.resolve_model("mimo-v2.6-flash"),
                "xiaomi/mimo-v2.6-flash"
            );
            assert_eq!(
                config.resolve_model("mimo-v2.6-pro"),
                "xiaomi/mimo-v2.6-pro"
            );
            assert_eq!(
                config.resolve_model("muse-spark-1.3-contributor"),
                "meta/muse-spark-1.3-contributor"
            );
        }
    }

    #[test]
    fn opencode_aliases_canonicalize_to_curated_ids() {
        let config = ProviderConfig {
            mode: "opencode".to_string(),
            base_url: "https://opencode.ai/zen/go/v1".to_string(),
            api_key: "test".to_string(),
            model: "deepseek-v4.1-flash".to_string(),
        };
        assert_eq!(
            config.resolve_model("Automatic Fallback"),
            "deepseek-v4.1-flash"
        );
        assert_eq!(
            config.resolve_model("deepseek/deepseek-v4.1-flash-1017"),
            "deepseek-v4.1-flash"
        );
        assert_eq!(
            config.resolve_model("xiaomi/mimo-v2.6-flash"),
            "mimo-v2.6-flash"
        );
        assert_eq!(
            config.resolve_model("xiaomi/mimo-v2.6-pro"),
            "mimo-v2.6-pro"
        );
        assert_eq!(
            config.resolve_model("meta/muse-spark-1.3-contributor"),
            "muse-spark-1.3-contributor"
        );
        assert_eq!(
            config.resolve_model("some-custom-model"),
            "some-custom-model"
        );
    }

    #[test]
    fn opencode_session_headers_use_affinity_not_just_session_id() {
        let config = ProviderConfig {
            mode: "opencode".to_string(),
            base_url: "https://opencode.ai/zen/go/v1".to_string(),
            api_key: "test".to_string(),
            model: "deepseek-v4.1-flash".to_string(),
        };
        let headers = session_headers(
            &config,
            Some(ProviderSession {
                session_id: "chat-child",
                affinity: "chat-parent",
            }),
        );
        assert!(headers.contains(&("x-opencode-session", "chat-parent".to_string())));
        assert!(headers.contains(&("x-opencode-session-id", "chat-child".to_string())));
        assert!(headers.contains(&("x-session-affinity", "chat-parent".to_string())));
        assert!(headers.contains(&("X-Session-Id", "chat-parent".to_string())));

        let openrouter = ProviderConfig {
            mode: "openrouter".to_string(),
            ..config.clone()
        };
        assert!(
            session_headers(
                &openrouter,
                Some(ProviderSession {
                    session_id: "chat",
                    affinity: "chat",
                })
            )
            .is_empty()
        );
        assert!(session_headers(&config, None).is_empty());
    }

    #[test]
    fn curated_models_cover_the_four_supported_ids() {
        let ids: Vec<&str> = CURATED_MODELS.iter().map(|(id, _)| *id).collect();
        assert_eq!(
            ids,
            vec![
                "deepseek-v4.1-flash",
                "mimo-v2.6-flash",
                "mimo-v2.6-pro",
                "muse-spark-1.3-contributor",
            ]
        );
    }
}
