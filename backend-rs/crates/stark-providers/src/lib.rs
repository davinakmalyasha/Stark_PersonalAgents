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

/// Optional reasoning effort, passed through to providers that support it.
/// Resolved per run from the agent config, never from a global env var.
fn normalize_reasoning_effort(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "minimal" => Some("minimal"),
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        _ => None,
    }
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

/// Codes providers use for a hard account limit, not a transient 429. Codex
/// maps the same family to terminal `UsageLimitReached` / `QuotaExceeded`
/// (`codex-api/src/api_bridge.rs`).
const USAGE_LIMIT_CODES: [&str; 8] = [
    "usage_limit_reached",
    "usage_not_included",
    "insufficient_quota",
    "credit_balance_exhausted",
    "organization_spend_limit_exceeded",
    "project_spend_limit_exceeded",
    "organization_usage_limit_exceeded",
    "billing_hard_limit_reached",
];

fn is_usage_limit_body(body: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        let lower = body.to_ascii_lowercase();
        return USAGE_LIMIT_CODES.iter().any(|code| lower.contains(*code));
    };
    let mut candidates = Vec::new();
    if let Some(error) = value.get("error") {
        candidates.push(error);
        if let Some(first) = error.as_array().and_then(|items| items.first()) {
            candidates.push(first);
        }
    }
    candidates.push(&value);
    candidates.iter().any(|candidate| {
        ["type", "code"].iter().any(|key| {
            candidate
                .get(*key)
                .and_then(Value::as_str)
                .is_some_and(|code| {
                    USAGE_LIMIT_CODES
                        .iter()
                        .any(|known| code.eq_ignore_ascii_case(known))
                })
        })
    })
}

fn usage_limit_message(status: u16, body: &str, retry_after: Option<RetryAfter>) -> String {
    let detail = gateway_message(body);
    let detail = if detail.is_empty() {
        "the provider reports no remaining quota".to_string()
    } else {
        detail
    };
    // Codex prints the reset time in local time (`protocol/src/error.rs`);
    // gateways advertise it as `resets_at` (epoch/RFC 3339) or `resets_in`.
    let reset = usage_reset_at(body).or_else(|| {
        retry_after.map(|advice| {
            chrono::Utc::now() + chrono::Duration::from_std(advice.remaining()).unwrap_or_default()
        })
    });
    let advice = match reset {
        Some(reset) => format!(" Try again at {}.", format_retry_time(reset)),
        None => " Try again later.".to_string(),
    };
    format!(
        "Usage limit reached (HTTP {status}). {detail}. Retrying will not help; check your plan/billing or switch providers.{advice}"
    )
}

/// Reset time advertised by a usage-limit response. Providers differ: some
/// send epoch seconds or an RFC 3339 string in `resets_at`, others a
/// `resets_in` second count.
fn usage_reset_at(body: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let value: Value = serde_json::from_str(body).ok()?;
    let mut candidates: Vec<&Value> = vec![&value];
    if let Some(error) = value.get("error") {
        candidates.push(error);
        if let Some(first) = error.as_array().and_then(|items| items.first()) {
            candidates.push(first);
        }
    }
    for candidate in candidates {
        if let Some(reset) = candidate.get("resets_at").and_then(parse_reset_value) {
            return Some(reset);
        }
        if let Some(secs) = candidate.get("resets_in").and_then(Value::as_i64) {
            return Some(chrono::Utc::now() + chrono::Duration::seconds(secs.max(0)));
        }
        if let Some(reset) = candidate.get("reset_at").and_then(parse_reset_value) {
            return Some(reset);
        }
    }
    None
}

fn parse_reset_value(value: &Value) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Some(secs) = value.as_i64() {
        return chrono::DateTime::from_timestamp(secs, 0);
    }
    let text = value.as_str()?.trim();
    if let Ok(secs) = text.parse::<i64>() {
        return chrono::DateTime::from_timestamp(secs, 0);
    }
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|value| value.with_timezone(&chrono::Utc))
}

/// Codex's `format_retry_timestamp`: same-day resets show only the time,
/// later resets include the date.
fn format_retry_time(reset: chrono::DateTime<chrono::Utc>) -> String {
    let local = reset.with_timezone(&chrono::Local);
    let now = chrono::Local::now();
    if local.date_naive() == now.date_naive() {
        local.format("%-I:%M %p").to_string()
    } else {
        local.format("%b %-d, %Y %-I:%M %p").to_string()
    }
}

fn friendly_status(
    status: u16,
    body: &str,
    attempts: usize,
    retry_after: Option<RetryAfter>,
) -> String {
    let message = gateway_message(body);
    // Codex surfaces the server's advice instead of hiding it behind the
    // retry budget. Applies to the terminal 429 once retries are exhausted.
    let advice = retry_after
        .map(|advice| {
            format!(
                " The server asked to retry in {}s.",
                advice.remaining().as_secs().max(1)
            )
        })
        .unwrap_or_default();
    match status {
        401 | 403 => format!(
            "Authentication failed (HTTP {status}). Check the provider's key_env variable (OPENCODE_API_KEY / DEEPSEEK_API_KEY / OPENROUTER_API_KEY). {message}"
        ),
        404 => format!("Model or endpoint not found (HTTP 404). Check the model id. {message}"),
        402 => format!("Insufficient credits (HTTP 402). {message}"),
        408 => format!("Provider timed out (HTTP 408) after {attempts} attempt(s). {message}"),
        429 => format!("Rate limited (HTTP 429) after {attempts} attempt(s). {message}{advice}"),
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
    /// Legacy constructor: environment variables supply the **key only**.
    /// Provider/base/model come from `.deep_agents/config/providers.json`
    /// (see `stark-config`); this fallback is OpenCode Go so a stale
    /// `STARK_PROVIDER=openrouter` in a workspace `.env` can never hijack it.
    pub fn from_env() -> Self {
        Self {
            mode: "opencode".to_string(),
            base_url: "https://opencode.ai/zen/go/v1".to_string(),
            api_key: first_env(&["OPENCODE_API_KEY", "DEEPSEEK_API_KEY"]).unwrap_or_default(),
            model: "deepseek-v4.1-flash".to_string(),
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
    /// Exact OpenAI custom-tool options line (`// @options: ...`) replayed for
    /// a freeform call. Never sent on the function-tool wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_header: Option<String>,
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
            custom_header: None,
        }
    }

    pub fn with_custom_header(mut self, header: Option<&str>) -> Self {
        self.custom_header = header.map(str::to_string);
        self
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
            } else if self.role == "assistant" && self.tool_calls.is_some() {
                // OpenCode lowers a textless tool-call turn to `content: null`.
                state.serialize_field("content", &Value::Null)?;
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
    /// A raw OpenAI custom-tool chunk, flattened into JSON arguments by the
    /// provider's freeform stream adapter.
    CustomTool {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        input: Option<String>,
    },
    /// The final parsed arguments and exact custom-tool header for one call.
    ToolCallFinal {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments: String,
        header: Option<String>,
    },
    Usage(Usage),
    Done {
        finish_reason: Option<String>,
    },
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error(
        "missing API key; add OPENCODE_API_KEY to D:\\StarkData\\.env (or configure providers in Settings → Agents)"
    )]
    MissingApiKey,
    #[error("{body}")]
    HttpStatus {
        status: u16,
        body: String,
        /// Server retry advice captured when the response arrived.
        retry_after: Option<RetryAfter>,
    },
    #[error("{0}")]
    UsageLimit(String),
    #[error("provider request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("{0}")]
    Transport(String),
    #[error("provider stream failed: {message}")]
    Stream {
        message: String,
        /// Server retry advice captured from the streamed error frame.
        retry_after: Option<RetryAfter>,
    },
    /// A response stopped by the provider's content filter. Codex models this
    /// as its own typed error so the turn can record guidance before retrying.
    #[error("stream disconnected before completion: {0}")]
    ContentFilter(String),
    #[error("provider returned invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
}

impl ProviderError {
    /// A stream failure with no server advice. Call sites that have an SSE
    /// error frame attach `retry_after` directly.
    fn stream(message: impl Into<String>) -> Self {
        Self::Stream {
            message: message.into(),
            retry_after: None,
        }
    }

    /// Terminal errors never retry: auth, not-found, bad request, quota,
    /// invalid payload. Everything else (transport, stream, 408/425/429/5xx)
    /// is transient.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::MissingApiKey | Self::UsageLimit(_) => false,
            // A malformed stream chunk is a transport failure, not a terminal
            // request error. Codex classifies `Json` as retryable too.
            Self::Json(_)
            | Self::Request(_)
            | Self::Transport(_)
            | Self::Stream { .. }
            | Self::ContentFilter(_) => true,
            Self::HttpStatus { status, .. } => {
                matches!(status, 408 | 425 | 429 | 500 | 502 | 503 | 504)
            }
        }
    }

    pub fn is_terminal(&self) -> bool {
        !self.is_retryable()
    }

    /// True when the provider's content filter stopped the response. The turn
    /// loop records Codex's `<content_filter_guidance>` fragment before it
    /// retries.
    pub fn is_content_filter(&self) -> bool {
        matches!(self, Self::ContentFilter(_))
    }

    /// Server-advised delay wins over local backoff: the captured deadline is
    /// honored as-is (Codex parity), and elapsed time is not restarted when the
    /// error is passed between callers. Otherwise exponential backoff with
    /// jitter applies. Returns `None` for terminal errors.
    pub fn retry_delay(&self, attempt: u64) -> Option<std::time::Duration> {
        if !self.is_retryable() {
            return None;
        }
        match self {
            Self::HttpStatus {
                body, retry_after, ..
            } => {
                if let Some(retry_after) = retry_after {
                    return Some(retry_after.remaining());
                }
                if let Some(retry_after) = RetryAfter::from_body(body) {
                    return Some(retry_after.remaining());
                }
            }
            Self::Stream {
                message,
                retry_after,
            } => {
                if let Some(retry_after) = retry_after {
                    return Some(retry_after.remaining());
                }
                // SSE error frames carry the advice in the frame body.
                if let Some(retry_after) = RetryAfter::from_body(message) {
                    return Some(retry_after.remaining());
                }
            }
            _ => {}
        }
        Some(backoff_delay(attempt))
    }
}

/// Retry budget for the HTTP handshake. Defaults match the reference
/// implementation (4 request retries, 5 stream retries).
pub fn request_max_retries() -> u64 {
    std::env::var("STARK_REQUEST_MAX_RETRIES")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(4)
}

pub fn stream_max_retries() -> u64 {
    std::env::var("STARK_STREAM_MAX_RETRIES")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(5)
}

/// Idle timeout for SSE streams. Default 300s.
pub fn stream_idle_timeout() -> std::time::Duration {
    let secs = std::env::var("STARK_STREAM_IDLE_TIMEOUT_SEC")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(300);
    std::time::Duration::from_secs(secs.max(1))
}

/// Exponential backoff starting at 200ms, doubling per attempt, ±10% jitter.
/// Attempts 0 and 1 both use the initial delay.
pub fn backoff_delay(attempt: u64) -> std::time::Duration {
    let exp = 2f64.powi(attempt.saturating_sub(1) as i32);
    let base = 200f64 * exp;
    // Jitter without pulling in rand: hash the attempt + nanos.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as f64)
        .unwrap_or(0.0);
    let jitter = 0.9 + (nanos % 1000.0) / 1000.0 * 0.2;
    std::time::Duration::from_millis((base * jitter) as u64)
}

/// Server retry advice captured once as an absolute deadline. Codex keeps the
/// same shape (`RetryAfter(Instant)`), so passing the error between callers
/// cannot restart the clock and no clamp shortens the server's ask.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetryAfter(std::time::Instant);

impl RetryAfter {
    /// Captures an interval measured from receipt of the server's advice.
    pub fn from_delay(delay: std::time::Duration) -> Option<Self> {
        std::time::Instant::now().checked_add(delay).map(Self)
    }

    /// Reads either nonnegative delay seconds or an HTTP date, exactly like
    /// Codex's `RetryAfter::from_header`. No local clamp.
    pub fn from_header(value: &str) -> Option<Self> {
        // Sample the clock first so a scheduling pause cannot shorten the
        // server's deadline.
        let received_at = std::time::Instant::now();
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        let delay = if value.bytes().all(|byte| byte.is_ascii_digit()) {
            std::time::Duration::from_secs(value.parse().ok()?)
        } else {
            let date = httpdate::parse_http_date(value).ok()?;
            date.duration_since(std::time::SystemTime::now())
                .unwrap_or_default()
        };
        received_at.checked_add(delay).map(Self)
    }

    pub fn from_headers(headers: &reqwest::header::HeaderMap) -> Option<Self> {
        let value = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
        Self::from_header(value)
    }

    /// Reads gateways that embed the advice in the response body. Explicit
    /// units only — `retry_after_ms` (milliseconds), `retry_after_seconds`
    /// (seconds), and `retry_after` (seconds, the OpenAI convention). No
    /// value heuristic and no cap.
    pub fn from_body(body: &str) -> Option<Self> {
        let value: Value = serde_json::from_str(body).ok()?;
        Self::from_value(&value)
    }

    /// Same as `from_body` for an already-parsed chunk (SSE error frames).
    pub fn from_value(value: &Value) -> Option<Self> {
        let mut candidates: Vec<&Value> = vec![value];
        if let Some(error) = value.get("error") {
            candidates.push(error);
            if let Some(first) = error.as_array().and_then(|items| items.first()) {
                candidates.push(first);
            }
        }
        candidates
            .into_iter()
            .find_map(delay_in_value)
            .and_then(Self::from_delay)
    }

    pub fn deadline(self) -> std::time::Instant {
        self.0
    }

    pub fn remaining(self) -> std::time::Duration {
        self.0.saturating_duration_since(std::time::Instant::now())
    }
}

fn delay_in_value(value: &Value) -> Option<std::time::Duration> {
    if let Some(ms) = value.get("retry_after_ms").and_then(Value::as_u64) {
        return Some(std::time::Duration::from_millis(ms));
    }
    if let Some(seconds) = value.get("retry_after_seconds").and_then(Value::as_u64) {
        return Some(std::time::Duration::from_secs(seconds));
    }
    value
        .get("retry_after")
        .and_then(Value::as_u64)
        .map(std::time::Duration::from_secs)
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
    /// Optional second provider used when the primary handshake fails or its
    /// circuit is open. The fallback keeps its own base URL, key, and model.
    fallback: Option<ProviderConfig>,
    /// OpenCode freeform formats this model advertises (`text`, `lark`,
    /// `regex`). Empty keeps JSON function tools.
    freeform: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
struct CircuitState {
    failures: u32,
    opened_at: Option<std::time::Instant>,
}

static CIRCUITS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, CircuitState>>,
> = std::sync::OnceLock::new();
const CIRCUIT_THRESHOLD: u32 = 3;
const CIRCUIT_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(60);

fn circuits() -> &'static std::sync::Mutex<std::collections::HashMap<String, CircuitState>> {
    CIRCUITS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn circuit_key(config: &ProviderConfig) -> String {
    format!("{}|{}|{}", config.mode, config.base_url, config.model)
}

fn circuit_open(key: &str) -> bool {
    let Ok(mut circuits) = circuits().lock() else {
        return false;
    };
    let Some(state) = circuits.get_mut(key) else {
        return false;
    };
    if let Some(opened) = state.opened_at {
        if opened.elapsed() < CIRCUIT_COOLDOWN {
            return true;
        }
        // Half-open: allow one probe after the cooldown.
        state.opened_at = None;
        state.failures = 0;
    }
    false
}

fn record_failure(key: &str) {
    if let Ok(mut circuits) = circuits().lock() {
        let state = circuits.entry(key.to_string()).or_insert(CircuitState {
            failures: 0,
            opened_at: None,
        });
        state.failures = state.failures.saturating_add(1);
        if state.failures >= CIRCUIT_THRESHOLD {
            state.opened_at = Some(std::time::Instant::now());
        }
    }
}

fn record_success(key: &str) {
    if let Ok(mut circuits) = circuits().lock() {
        circuits.remove(key);
    }
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
    /// Agent-configured reasoning effort. `None` inherits the provider/model.
    pub reasoning_effort: Option<&'a str>,
}

fn models_url(base_url: &str) -> String {
    format!("{}/models", base_url.trim_end_matches('/'))
}

const FREEFORM_PREFIX: &str = "// @options:";

#[derive(Clone)]
struct FreeformField {
    name: String,
    schema: Value,
    required: bool,
}

/// OpenCode `OpenAIFreeform.resolve` for the one host tool that declares
/// freeform: `execute` with its required `code` string. Returns the custom
/// tool when the model advertises `text` (and `lark` when there are other
/// parameters).
fn freeform_execute(tool: &Value, formats: &[String]) -> Option<Value> {
    if !formats.iter().any(|format| format == "text") {
        return None;
    }
    let function = tool.get("function")?;
    if function.get("name")?.as_str()? != "execute" {
        return None;
    }
    let input = function.get("parameters")?;
    let properties = input.get("properties")?.as_object()?;
    let required = input
        .get("required")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !required.iter().any(|value| value.as_str() == Some("code"))
        || properties
            .get("code")
            .and_then(|schema| schema.get("type"))
            .and_then(Value::as_str)
            != Some("string")
    {
        return None;
    }
    let mut ordered: Vec<String> = ["timeout", "background"]
        .into_iter()
        .filter(|name| properties.contains_key(*name))
        .map(str::to_string)
        .collect();
    let extras: Vec<String> = properties
        .keys()
        .filter(|name| name.as_str() != "code" && !ordered.contains(name))
        .cloned()
        .collect();
    ordered.extend(extras);
    let fields: Vec<FreeformField> = ordered
        .into_iter()
        .filter_map(|name| {
            let schema = properties.get(&name)?.clone();
            Some(FreeformField {
                required: required
                    .iter()
                    .any(|value| value.as_str() == Some(name.as_str())),
                name,
                schema,
            })
        })
        .collect();
    let lark = formats.iter().any(|format| format == "lark");
    let description = function
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let description = if fields.is_empty() {
        description.to_string()
    } else {
        format!("{description}\n\n{}", freeform_options_description(&fields))
    };
    let format = if fields.is_empty() || !lark {
        json!({"type": "text"})
    } else {
        json!({
            "type": "grammar",
            "grammar": {"syntax": "lark", "definition": freeform_line_grammar(&fields)},
        })
    };
    Some(json!({
        "type": "custom",
        "custom": {
            "name": "execute",
            "description": description,
            "format": format,
        },
    }))
}

/// OpenCode `OpenAIFreeform.resolve` for Stark's two freeform tools:
/// `execute` (raw text, optional options line) and `apply_patch` (fixed Lark
/// grammar). Other tools keep their JSON function schema.
fn freeform_tools(
    tools: &[Value],
    formats: &[String],
) -> (
    Vec<Value>,
    std::collections::HashMap<String, FreeformCustom>,
) {
    if formats.is_empty() {
        return (tools.to_vec(), std::collections::HashMap::new());
    }
    let mut customs = std::collections::HashMap::new();
    let out = tools
        .iter()
        .map(|tool| {
            if let Some(custom) = freeform_execute(tool, formats) {
                customs.insert(
                    "execute".to_string(),
                    FreeformCustom {
                        field: "code".to_string(),
                        options: freeform_tool_has_options(tool),
                    },
                );
                return custom;
            }
            if let Some((custom, definition)) = freeform_patch(tool, formats) {
                customs.insert("apply_patch".to_string(), definition);
                return custom;
            }
            tool.clone()
        })
        .collect();
    (out, customs)
}

/// OpenCode `OpenAIFreeform.resolve` for the patch tool: a Lark-only freeform
/// with OpenCode's fixed grammar. Text-only models keep the JSON tool.
fn freeform_patch(tool: &Value, formats: &[String]) -> Option<(Value, FreeformCustom)> {
    if !formats.iter().any(|format| format == "lark") {
        return None;
    }
    let function = tool.get("function")?;
    if function.get("name")?.as_str()? != "apply_patch" {
        return None;
    }
    let input = function.get("parameters")?;
    let properties = input.get("properties")?.as_object()?;
    if properties.len() != 1 {
        return None;
    }
    let (field, schema) = properties.iter().next()?;
    if schema.get("type").and_then(Value::as_str) != Some("string") {
        return None;
    }
    let required = input.get("required")?.as_array()?;
    if !required.iter().any(|value| value.as_str() == Some(field)) {
        return None;
    }
    let description = function
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let custom = json!({
        "type": "custom",
        "custom": {
            "name": "apply_patch",
            "description": description,
            "format": {
                "type": "grammar",
                "grammar": {"syntax": "lark", "definition": PATCH_GRAMMAR},
            },
        },
    });
    Some((
        custom,
        FreeformCustom {
            field: field.clone(),
            options: false,
        },
    ))
}

/// OpenCode's fixed `packages/core/src/tool/patch-grammar.txt` (MIT).
const PATCH_GRAMMAR: &str = include_str!("patch_grammar.lark");

/// One declared custom tool: the required string field and whether the first
/// line carries the tool's other parameters (`// @options: {...}`).
#[derive(Clone, Debug)]
struct FreeformCustom {
    field: String,
    options: bool,
}

fn freeform_tool_has_options(tool: &Value) -> bool {
    tool.get("function")
        .and_then(|function| function.get("parameters"))
        .and_then(|parameters| parameters.get("properties"))
        .and_then(Value::as_object)
        .is_some_and(|properties| properties.keys().any(|name| name != "code"))
}

/// ECMAScript `Number::toString` for the values JSON carries. `JSON.stringify`
/// is what OpenCode uses to re-encode parsed tool input, so integral floats
/// lose their `.0` and exponent forms gain an explicit sign.
fn js_number(number: &serde_json::Number) -> String {
    if let Some(value) = number.as_i64() {
        return value.to_string();
    }
    if let Some(value) = number.as_u64() {
        return value.to_string();
    }
    let value = number.as_f64().unwrap_or(0.0);
    if value == 0.0 {
        return "0".to_string();
    }
    let abs = value.abs();
    if abs >= 1e21 || abs < 1e-6 {
        let formatted = format!("{value:e}");
        if let Some((mantissa, exponent)) = formatted.split_once('e') {
            let sign = if exponent.starts_with('-') { "" } else { "+" };
            return format!("{mantissa}e{sign}{exponent}");
        }
        return formatted;
    }
    if value.fract() == 0.0 {
        return format!("{value:.0}");
    }
    format!("{value}")
}

/// Compact JSON with JavaScript semantics (key order and number rendering),
/// matching OpenCode's `ProviderShared.encodeJson`.
fn js_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(number) => js_number(number),
        Value::String(text) => serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string()),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(js_json).collect::<Vec<_>>().join(",")
        ),
        Value::Object(object) => format!(
            "{{{}}}",
            object
                .iter()
                .map(|(key, value)| format!(
                    "{}:{}",
                    serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string()),
                    js_json(value)
                ))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

fn freeform_json(value: &Value) -> String {
    js_json(value)
}

fn freeform_lark_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

fn freeform_minimum(schema: &Value) -> i64 {
    let minimum = schema.get("minimum").and_then(Value::as_i64);
    let exclusive = schema
        .get("exclusiveMinimum")
        .and_then(Value::as_i64)
        .map(|value| value + 1);
    minimum
        .into_iter()
        .chain(exclusive)
        .max()
        .unwrap_or(i64::MIN)
}

fn freeform_value_rule(schema: &Value) -> Option<String> {
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        if values.is_empty() {
            return None;
        }
        return Some(format!(
            "({})",
            values
                .iter()
                .map(freeform_json)
                .collect::<Vec<_>>()
                .join(" | ")
        ));
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("boolean") => Some("(\"true\" | \"false\")".to_string()),
        Some("number") => {
            Some("/-?(?:0|[1-9][0-9]*)(?:\\.[0-9]+)?(?:[eE][+-]?[0-9]+)?/".to_string())
        }
        Some("string") => Some(
            "/\"(?:[^\"\\\\\\x00-\\x1f]|\\\\[\"\\\\\\/bfnrt]|\\\\u[0-9a-fA-F]{4})*\"/".to_string(),
        ),
        Some("integer") => {
            let lowest = freeform_minimum(schema);
            if lowest >= 1 {
                Some("/[1-9][0-9]*/".to_string())
            } else if lowest >= 0 {
                Some("/0|[1-9][0-9]*/".to_string())
            } else {
                Some("/-?(?:0|[1-9][0-9]*)/".to_string())
            }
        }
        _ => None,
    }
}

fn freeform_sample(schema: &Value) -> Value {
    if let Some(value) = schema
        .get("enum")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
    {
        return value.clone();
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("boolean") => json!(true),
        Some("string") => json!("..."),
        _ => json!(freeform_minimum(schema).max(1)),
    }
}

fn freeform_options_description(fields: &[FreeformField]) -> String {
    let required: Vec<&FreeformField> = fields.iter().filter(|field| field.required).collect();
    let sample = if required.is_empty() {
        fields
            .iter()
            .find(|field| field.schema.get("type").and_then(Value::as_str) == Some("boolean"))
            .or_else(|| fields.first())
    } else {
        required.first().copied()
    };
    let example = sample
        .map(|field| {
            format!(
                "{}: {}",
                freeform_json(&Value::String(field.name.clone())),
                freeform_json(&freeform_sample(&field.schema))
            )
        })
        .unwrap_or_default();
    let mut out = vec![if required.is_empty() {
        "To set the parameters below, start with a `// @options:` line holding a JSON object of them, then write `code` from the next line. Leave the line out to use the defaults.".to_string()
    } else {
        "Start with a `// @options:` line holding a JSON object of the parameters below, then write `code` from the next line.".to_string()
    }];
    for field in fields {
        let kind = field
            .schema
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let description = field
            .schema
            .get("description")
            .and_then(Value::as_str)
            .map(|description| format!(": {description}"))
            .unwrap_or_default();
        out.push(format!(
            "- `{}` ({}{}){description}",
            field.name,
            kind,
            if field.required { ", required" } else { "" }
        ));
    }
    out.push("For example:".to_string());
    out.push(format!("{FREEFORM_PREFIX} {{{example}}}"));
    out.push("<code>".to_string());
    out.join("\n")
}

fn freeform_line_grammar(fields: &[FreeformField]) -> String {
    let rules: Vec<String> = fields
        .iter()
        .filter_map(|field| freeform_value_rule(&field.schema))
        .collect();
    if rules.len() != fields.len() {
        return String::new();
    }
    let has_required = fields.iter().any(|field| field.required);
    let pairs = fields
        .iter()
        .zip(rules.iter())
        .map(|(field, rule)| {
            format!(
                "{} \" \"* \":\" \" \"* {}",
                freeform_lark_string(&freeform_json(&Value::String(field.name.clone()))),
                rule
            )
        })
        .collect::<Vec<_>>()
        .join(" | ");
    [
        format!(
            "start: ff_line ff_body{}",
            if has_required { "" } else { " | ff_plain" }
        ),
        format!(
            "ff_line: {} \" \"* \"{{\" \" \"* ff_pair (\" \"* \",\" \" \"* ff_pair)* \" \"* \"}}\" \"\\n\"",
            freeform_lark_string(FREEFORM_PREFIX)
        ),
        format!("ff_pair: {pairs}"),
        "ff_body: /[\\s\\S]+/".to_string(),
        format!("ff_plain: /{}/", freeform_not_starting_with(FREEFORM_PREFIX)),
    ]
    .join("\n")
}

fn freeform_not_starting_with(prefix: &str) -> String {
    let chars: Vec<char> = prefix.chars().collect();
    let head = |end: usize| {
        chars[..end]
            .iter()
            .map(|character| freeform_escape_regex(&character.to_string()))
            .collect::<String>()
    };
    let differs = chars
        .iter()
        .enumerate()
        .map(|(index, character)| {
            format!(
                "{}[^{}]",
                head(index),
                freeform_escape_class(&character.to_string())
            )
        })
        .collect::<Vec<_>>();
    let shorter = (0..chars.len().saturating_sub(1))
        .map(|index| head(index + 1))
        .collect::<Vec<_>>();
    format!(
        "(?:{})[\\s\\S]*{}",
        differs.join("|"),
        if shorter.is_empty() {
            String::new()
        } else {
            format!("|{}", shorter.join("|"))
        }
    )
}

fn freeform_escape_regex(text: &str) -> String {
    text.chars()
        .map(|character| {
            if matches!(
                character,
                '\\' | '^'
                    | '$'
                    | '.'
                    | '*'
                    | '+'
                    | '?'
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '|'
                    | '/'
            ) {
                format!("\\{character}")
            } else {
                character.to_string()
            }
        })
        .collect()
}

fn freeform_escape_class(text: &str) -> String {
    text.chars()
        .map(|character| {
            if matches!(character, '\\' | ']' | '^' | '-' | '/') {
                format!("\\{character}")
            } else {
                character.to_string()
            }
        })
        .collect()
}

/// OpenCode `OpenAIFreeform.parse`: the raw call text becomes JSON arguments
/// for the tool's field, and a valid first-line options object is returned as
/// the replay header. Malformed options text stays raw, exactly like OpenCode,
/// so downstream JSON parsing rejects the call instead of running the line.
fn freeform_parse(raw: &str, options: bool, field: &str) -> (String, Option<String>) {
    let plain = || (js_json(&json!({ field: raw })), None);
    if !options {
        return plain();
    }
    let Some(end) = raw.find('\n') else {
        return plain();
    };
    let header = &raw[..end];
    let Some(options_text) = header.strip_prefix(FREEFORM_PREFIX) else {
        return plain();
    };
    let Ok(Value::Object(mut object)) = serde_json::from_str::<Value>(options_text) else {
        return (raw.to_string(), None);
    };
    object.insert(field.to_string(), Value::String(raw[end + 1..].to_string()));
    (js_json(&Value::Object(object)), Some(header.to_string()))
}

fn freeform_options(object: &serde_json::Map<String, Value>) -> Option<String> {
    let mut options = Vec::new();
    for name in ["timeout", "background"] {
        if let Some(value) = object.get(name) {
            options.push(format!(
                "{}:{}",
                freeform_json(&Value::String(name.to_string())),
                freeform_json(value)
            ));
        }
    }
    (!options.is_empty()).then(|| options.join(","))
}

fn freeform_header_matches(header: &str, input: &Value) -> bool {
    let Some(saved) = header
        .strip_prefix(FREEFORM_PREFIX)
        .and_then(|value| serde_json::from_str::<Value>(value.trim()).ok())
        .and_then(|value| value.as_object().cloned())
    else {
        return false;
    };
    let Some(object) = input.as_object() else {
        return false;
    };
    ["timeout", "background"]
        .iter()
        .all(|name| saved.get(*name) == object.get(*name))
}

fn freeform_raw(input: &Value, header: Option<&str>, custom: &FreeformCustom) -> String {
    let Some(object) = input.as_object() else {
        return freeform_json(input);
    };
    let Some(value) = object.get(&custom.field).and_then(Value::as_str) else {
        return freeform_json(input);
    };
    if !custom.options {
        return value.to_string();
    }
    let Some(options) = freeform_options(object) else {
        return value.to_string();
    };
    if let Some(header) = header
        && freeform_header_matches(header, input)
    {
        return format!("{header}\n{value}");
    }
    format!("{FREEFORM_PREFIX} {{{options}}}\n{value}")
}

fn freeform_messages(
    messages: &[ChatMessage],
    customs: &std::collections::HashMap<String, FreeformCustom>,
) -> Value {
    let mut value = serde_json::to_value(messages).unwrap_or_else(|_| Value::Array(Vec::new()));
    let Some(items) = value.as_array_mut() else {
        return value;
    };
    for message in items {
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(calls) = message.get_mut("tool_calls").and_then(Value::as_array_mut) else {
            continue;
        };
        for call in calls.iter_mut() {
            let name = call
                .get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let Some(custom) = name.as_deref().and_then(|name| customs.get(name)) else {
                if let Some(object) = call.as_object_mut() {
                    object.remove("custom_header");
                }
                continue;
            };
            let Some(arguments) = call
                .get("function")
                .and_then(|function| function.get("arguments"))
                .and_then(Value::as_str)
            else {
                if let Some(object) = call.as_object_mut() {
                    object.remove("custom_header");
                }
                continue;
            };
            let Ok(input) = serde_json::from_str::<Value>(arguments) else {
                if let Some(object) = call.as_object_mut() {
                    object.remove("custom_header");
                }
                continue;
            };
            let id = call.get("id").cloned().unwrap_or(Value::Null);
            let header = call
                .get("custom_header")
                .and_then(Value::as_str)
                .map(str::to_string);
            *call = json!({
                "id": id,
                "type": "custom",
                "custom": {
                    "name": name,
                    "input": freeform_raw(&input, header.as_deref(), custom),
                },
            });
        }
    }
    value
}

/// OpenCode's OpenAI Chat lowering for chronological `Message.system(...)`
/// updates: XML-escape the text, wrap it in `<system-update>`, and merge it
/// into the preceding user message when one is last, exactly like
/// `ProviderShared.wrappedSystemUpdate` + `lowerMessages`.
fn wrap_system_update(text: &str) -> String {
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!("<system-update>\n{escaped}\n</system-update>")
}

/// OpenCode re-encodes a replayed function call from its parsed input
/// (`encodeJson(part.input)`), which drops insignificant whitespace, normalizes
/// escapes/numbers, and keeps key order. Malformed arguments never became a
/// tool call upstream, so they are left untouched.
fn normalize_function_arguments(value: &mut Value) {
    let Some(messages) = value.as_array_mut() else {
        return;
    };
    for message in messages {
        let Some(calls) = message.get_mut("tool_calls").and_then(Value::as_array_mut) else {
            continue;
        };
        for call in calls {
            if call.get("type").and_then(Value::as_str) != Some("function") {
                continue;
            }
            let Some(constructed) = call.get("function") else {
                continue;
            };
            let Some(arguments) = constructed.get("arguments").and_then(Value::as_str) else {
                continue;
            };
            let Ok(parsed) = serde_json::from_str::<Value>(arguments) else {
                continue;
            };
            if let Some(function) = call.get_mut("function") {
                function["arguments"] = Value::String(js_json(&parsed));
            }
        }
    }
}

fn lower_system_updates(value: &mut Value) {
    let Some(messages) = value.as_array_mut() else {
        return;
    };
    let mut out: Vec<Value> = Vec::with_capacity(messages.len());
    for (index, message) in std::mem::take(messages).into_iter().enumerate() {
        if index > 0 && message.get("role").and_then(Value::as_str) == Some("system") {
            let wrapped = wrap_system_update(
                message
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            );
            if let Some(previous) = out.last_mut()
                && previous.get("role").and_then(Value::as_str) == Some("user")
            {
                match previous.get_mut("content") {
                    Some(Value::String(content)) => {
                        content.push('\n');
                        content.push_str(&wrapped);
                        continue;
                    }
                    Some(Value::Array(parts)) => {
                        parts.push(json!({"type": "text", "text": wrapped}));
                        continue;
                    }
                    _ => {}
                }
            }
            out.push(json!({"role": "user", "content": wrapped}));
            continue;
        }
        out.push(message);
    }
    *messages = out;
}

fn freeform_strip_headers(value: &mut Value) {
    let Some(items) = value.as_array_mut() else {
        return;
    };
    for message in items {
        let Some(calls) = message.get_mut("tool_calls").and_then(Value::as_array_mut) else {
            continue;
        };
        for call in calls {
            if let Some(object) = call.as_object_mut() {
                object.remove("custom_header");
            }
        }
    }
}

fn freeform_stream_delta(raw: &str) -> String {
    let encoded = serde_json::to_string(raw).unwrap_or_else(|_| "\"\"".to_string());
    encoded[1..encoded.len().saturating_sub(1)].to_string()
}

fn freeform_stream_start(field: &str, raw: &str) -> String {
    format!(
        "{{{}:\"{}",
        js_json(&Value::String(field.to_string())),
        freeform_stream_delta(raw)
    )
}

fn freeform_stream_close() -> &'static str {
    "\"}"
}

fn freeform_stream(
    stream: EventStream,
    customs: std::collections::HashMap<String, FreeformCustom>,
) -> EventStream {
    let out = async_stream::try_stream! {
        let mut stream = stream;
        let mut order: Vec<usize> = Vec::new();
        let mut pending: std::collections::BTreeMap<
            usize,
            (Option<String>, Option<String>, String, bool),
        > = std::collections::BTreeMap::new();
        while let Some(event) = stream.next().await {
            match event? {
                ProviderEvent::CustomTool {
                    index,
                    id,
                    name,
                    input,
                } => {
                    let entry = pending.entry(index).or_insert_with(|| {
                        order.push(index);
                        (None, None, String::new(), false)
                    });
                    if id.is_some() {
                        entry.0 = id.clone();
                    }
                    if name.is_some() {
                        entry.1 = name.clone();
                    }
                    let chunk = input.unwrap_or_default();
                    entry.2.push_str(&chunk);
                    let first = !entry.3;
                    entry.3 = true;
                    let field = entry
                        .1
                        .as_deref()
                        .and_then(|name| customs.get(name))
                        .map(|custom| custom.field.as_str())
                        .unwrap_or("input");
                    let arguments = if first {
                        freeform_stream_start(field, &chunk)
                    } else {
                        freeform_stream_delta(&chunk)
                    };
                    yield ProviderEvent::ToolCall {
                        index,
                        id,
                        name,
                        arguments: Some(arguments),
                    };
                }
                ProviderEvent::Done { finish_reason } => {
                    for index in order.drain(..) {
                        if let Some((id, name, raw, _opened)) = pending.remove(&index) {
                            yield ProviderEvent::ToolCall {
                                index,
                                id: None,
                                name: None,
                                arguments: Some(freeform_stream_close().to_string()),
                            };
                            let custom = name.as_deref().and_then(|name| customs.get(name));
                            let (arguments, header) = match custom {
                                Some(custom) => {
                                    freeform_parse(&raw, custom.options, &custom.field)
                                }
                                None => freeform_parse(&raw, false, "input"),
                            };
                            yield ProviderEvent::ToolCallFinal {
                                index,
                                id,
                                name,
                                arguments,
                                header,
                            };
                        }
                    }
                    yield ProviderEvent::Done { finish_reason };
                }
                other => yield other,
            }
        }
    };
    Box::pin(out)
}

impl Provider {
    /// Fetch model metadata (context window) from the OpenAI-compatible
    /// `/models` endpoint and cache it for compaction. Best-effort: failures
    /// leave the conservative default in place.
    pub async fn refresh_model_contexts(&self) -> Result<usize, ProviderError> {
        let url = models_url(&self.config.base_url);
        let response = self
            .client
            .get(url)
            .bearer_auth(&self.config.api_key)
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::HttpStatus {
                status,
                body: format!("HTTP {status}: {}", gateway_message(&body)),
                retry_after: None,
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
        Ok(Self {
            client,
            config,
            fallback: None,
            freeform: Vec::new(),
        })
    }

    /// Attach a second provider used when the primary handshake fails or its
    /// circuit breaker is open.
    pub fn with_fallback(mut self, fallback: ProviderConfig) -> Self {
        self.fallback = Some(fallback);
        self
    }

    /// OpenCode model capability: freeform tool formats (`text`, `lark`,
    /// `regex`). An empty list keeps the JSON function schema.
    pub fn with_freeform(mut self, formats: Vec<String>) -> Self {
        self.freeform = formats
            .into_iter()
            .map(|format| format.trim().to_ascii_lowercase())
            .filter(|format| matches!(format.as_str(), "text" | "lark" | "regex"))
            .collect();
        self
    }

    pub fn fallback(&self) -> Option<&ProviderConfig> {
        self.fallback.as_ref()
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
        let mut provider = Self::new(config)?;
        provider.fallback = self.fallback.clone();
        if provider
            .config
            .model
            .eq_ignore_ascii_case(&self.config.model)
        {
            provider.freeform = self.freeform.clone();
        }
        Ok(provider)
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
                ProviderEvent::Reasoning(_)
                | ProviderEvent::ToolCall { .. }
                | ProviderEvent::CustomTool { .. }
                | ProviderEvent::ToolCallFinal { .. } => {}
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
        let primary_key = circuit_key(&self.config);
        if circuit_open(&primary_key) {
            if let Some(fallback) = &self.fallback {
                return self
                    .stream_with_config(fallback, messages, tools, temperature, session)
                    .await;
            }
            return Err(ProviderError::Transport(
                "provider circuit is open after repeated failures; retry after 60s or configure a fallback provider"
                    .to_string(),
            ));
        }
        match self
            .stream_with_config(&self.config, messages, tools, temperature, session)
            .await
        {
            Ok(stream) => {
                record_success(&primary_key);
                Ok(stream)
            }
            Err(error) => {
                record_failure(&primary_key);
                let Some(fallback) = &self.fallback else {
                    return Err(error);
                };
                if matches!(
                    &error,
                    ProviderError::MissingApiKey | ProviderError::Json(_)
                ) {
                    return Err(error);
                }
                let fallback_key = circuit_key(fallback);
                if circuit_open(&fallback_key) {
                    return Err(ProviderError::Transport(format!(
                        "primary provider failed: {error}; fallback circuit is open"
                    )));
                }
                match self
                    .stream_with_config(fallback, messages, tools, temperature, session)
                    .await
                {
                    Ok(stream) => {
                        record_success(&fallback_key);
                        Ok(stream)
                    }
                    Err(fallback_error) => {
                        record_failure(&fallback_key);
                        Err(ProviderError::Transport(format!(
                            "primary provider failed: {error}; fallback provider failed: {fallback_error}"
                        )))
                    }
                }
            }
        }
    }

    async fn stream_with_config(
        &self,
        config: &ProviderConfig,
        messages: &[ChatMessage],
        tools: &[Value],
        temperature: f64,
        session: Option<ProviderSession<'_>>,
    ) -> Result<EventStream, ProviderError> {
        if !config.has_key() {
            return Err(ProviderError::MissingApiKey);
        }

        let formats: &[String] = if config.model.eq_ignore_ascii_case(&self.config.model) {
            &self.freeform
        } else {
            &[]
        };
        let (outgoing_tools, customs) = freeform_tools(tools, formats);
        let freeform = !customs.is_empty();
        let mut outgoing_messages = if freeform {
            freeform_messages(messages, &customs)
        } else {
            let mut value =
                serde_json::to_value(messages).unwrap_or_else(|_| Value::Array(Vec::new()));
            freeform_strip_headers(&mut value);
            value
        };
        normalize_function_arguments(&mut outgoing_messages);
        lower_system_updates(&mut outgoing_messages);
        let mut body = json!({
            "model": config.model,
            "messages": outgoing_messages,
            "stream": true,
            "temperature": temperature,
            "stream_options": {"include_usage": true},
        });
        // OpenCode computes `max_tokens` from its models.dev output limit and
        // context estimate. Stark has no output-limit catalog, so it leaves
        // the cap to the provider unless `STARK_MAX_OUTPUT_TOKENS` is set, or
        // a 402 retry fits it to the affordable balance. This is a model-policy
        // difference outside the Code Mode tools/messages surface.
        let mut max_output = max_output_tokens();
        if let Some(cap) = max_output {
            body["max_tokens"] = json!(cap);
        }
        if let Some(effort) = session
            .and_then(|session| session.reasoning_effort)
            .and_then(normalize_reasoning_effort)
        {
            if config.mode == "openrouter" {
                body["reasoning"] = json!({ "effort": effort });
            } else {
                body["reasoning_effort"] = json!(effort);
            }
        }
        if !outgoing_tools.is_empty() {
            // OpenCode omits `tool_choice` unless the caller selected one
            // (its step-limit "none" is the only such case). The provider
            // default is already `auto`.
            body["tools"] = Value::Array(outgoing_tools);
        }

        let headers = session_headers(config, session);
        let max_attempts = request_max_retries();
        let mut attempt = 0u64;
        let response = loop {
            let mut request = self
                .client
                .post(config.endpoint())
                .bearer_auth(&config.api_key);
            for (name, value) in &headers {
                request = request.header(*name, value);
            }
            let response = match request.json(&body).send().await {
                Ok(response) => response,
                Err(error) => {
                    let failure = if error.is_timeout() {
                        ProviderError::Transport(format!(
                            "Timed out connecting to {} after {}s. Check your network or the provider status.",
                            config.endpoint(),
                            15
                        ))
                    } else if error.is_connect() {
                        ProviderError::Transport(format!(
                            "Cannot reach {} (connection failed). Check your network, proxy, or VPN.",
                            config.endpoint()
                        ))
                    } else {
                        ProviderError::Transport(format!(
                            "Connection to {} failed: {error}",
                            config.endpoint()
                        ))
                    };
                    if failure.is_retryable() && attempt < max_attempts {
                        attempt += 1;
                        tokio::time::sleep(
                            failure
                                .retry_delay(attempt)
                                .unwrap_or_else(|| backoff_delay(attempt)),
                        )
                        .await;
                        continue;
                    }
                    return Err(failure);
                }
            };
            if response.status().is_success() {
                break response;
            }
            let status = response.status().as_u16();
            let retry_after = RetryAfter::from_headers(response.headers());
            let body_text = response.text().await.unwrap_or_default();
            if status == 429 && is_usage_limit_body(&body_text) {
                return Err(ProviderError::UsageLimit(usage_limit_message(
                    status,
                    &body_text,
                    retry_after,
                )));
            }
            if matches!(status, 429 | 500 | 502 | 503 | 504 | 408 | 425) && attempt < max_attempts {
                attempt += 1;
                // Honor the server's deadline exactly; the retry budget, not a
                // local clamp, bounds how long the run waits.
                let delay = retry_after
                    .map(RetryAfter::remaining)
                    .unwrap_or_else(|| backoff_delay(attempt));
                tokio::time::sleep(delay).await;
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
                    retry_after: None,
                });
            }
            return Err(ProviderError::HttpStatus {
                status,
                body: friendly_status(status, &body_text, (attempt + 1) as usize, retry_after),
                retry_after,
            });
        };

        let idle = stream_idle_timeout();
        let events = response.bytes_stream().eventsource();
        let stream = async_stream::try_stream! {
            let mut events = Box::pin(events);
            let mut saw_done = false;
            loop {
                // A stalled connection must surface as an error, not a hang.
                let next = match tokio::time::timeout(idle, events.next()).await
                {
                    Err(_) if saw_done => break,
                    Err(_) => Err(ProviderError::stream(
                        format!("no data from the provider for {}s (stream stalled or connection lagging)", idle.as_secs()),
                    ))?,
                    Ok(None) => break,
                    Ok(Some(Err(_))) if saw_done => break,
                    Ok(Some(Err(error))) => Err(ProviderError::stream(format!(
                        "connection lost mid-stream: {error}"
                    )))?,
                    Ok(Some(Ok(event))) => event,
                };
                let parsed = match parse_sse_data(&next.data) {
                    Ok(parsed) => parsed,
                    Err(_) if saw_done => break,
                    Err(error) => Err(error)?,
                };
                for event in parsed {
                    if matches!(&event, ProviderEvent::Done { .. }) {
                        saw_done = true;
                    }
                    yield event;
                }
            }
            if !saw_done {
                // Codex's `CodexErrorDetails::Stream`: a stream that ends
                // before the response is complete is transient, not a
                // successful empty answer. The turn loop retries it.
                Err(ProviderError::stream(
                    "stream disconnected before completion: provider closed the stream without a finish_reason or [DONE]",
                ))?;
            }
        };

        Ok(if freeform {
            freeform_stream(Box::pin(stream), customs)
        } else {
            Box::pin(stream)
        })
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
        // SSE error frames often carry the server's retry advice; keep it so
        // the turn waits for the advised deadline instead of local backoff.
        return Err(ProviderError::Stream {
            message: error.to_string(),
            retry_after: RetryAfter::from_value(&value),
        });
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
                if let Some(custom) = tool_call.get("custom") {
                    out.push(ProviderEvent::CustomTool {
                        index,
                        id,
                        name: custom
                            .get("name")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        input: custom
                            .get("input")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    });
                    continue;
                }
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
        if reason == "content_filter" {
            // Codex treats a content-filter stop as a retryable stream
            // failure, not as a successful empty answer.
            return Err(ProviderError::ContentFilter(
                "content filter blocked the response".to_string(),
            ));
        }
        if reason == "error" {
            // Gateways report an internal failure through finish_reason
            // instead of an HTTP status. Codex never records that as a
            // completed answer; surface it so the turn retries.
            return Err(ProviderError::stream(
                "stream disconnected before completion: provider reported finish_reason=error",
            ));
        }
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
    fn models_url_uses_the_base_not_the_chat_endpoint() {
        assert_eq!(
            models_url("https://opencode.ai/zen/go/v1"),
            "https://opencode.ai/zen/go/v1/models"
        );
        assert_eq!(
            models_url("https://openrouter.ai/api/v1/"),
            "https://openrouter.ai/api/v1/models"
        );
        assert!(!models_url("https://example.invalid/v1").contains("chat/completions"));
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
                reasoning_effort: None,
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
                    reasoning_effort: None,
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

    #[test]
    fn retryable_errors_retry_and_terminal_errors_do_not() {
        let retryable = vec![
            ProviderError::Transport("down".to_string()),
            ProviderError::Stream {
                message: "lost".to_string(),
                retry_after: None,
            },
            ProviderError::Json(serde_json::from_str::<Value>("{").unwrap_err()),
            ProviderError::HttpStatus {
                status: 429,
                body: "rate limited".to_string(),
                retry_after: None,
            },
            ProviderError::HttpStatus {
                status: 503,
                body: "unavailable".to_string(),
                retry_after: None,
            },
        ];
        for error in retryable {
            assert!(error.is_retryable(), "{error:?}");
            assert!(error.retry_delay(1).is_some());
        }
        let terminal = vec![
            ProviderError::MissingApiKey,
            ProviderError::UsageLimit("out of credits".to_string()),
            ProviderError::HttpStatus {
                status: 401,
                body: "auth".to_string(),
                retry_after: None,
            },
            ProviderError::HttpStatus {
                status: 400,
                body: "bad".to_string(),
                retry_after: None,
            },
            ProviderError::HttpStatus {
                status: 402,
                body: "credits".to_string(),
                retry_after: None,
            },
        ];
        for error in terminal {
            assert!(!error.is_retryable(), "{error:?}");
            assert!(error.retry_delay(1).is_none());
        }
    }

    #[test]
    fn malformed_sse_chunks_are_retryable() {
        let error = parse_sse_data("{not json").unwrap_err();
        assert!(error.is_retryable(), "{error:?}");
    }

    #[test]
    fn content_filter_is_a_retryable_stream_failure() {
        let error =
            parse_sse_data(r#"{"choices":[{"delta":{},"finish_reason":"content_filter"}]}"#)
                .unwrap_err();
        assert!(error.is_content_filter(), "{error:?}");
        assert!(error.is_retryable());
        // The message keeps the Codex classification prefix.
        assert!(
            error
                .to_string()
                .contains("stream disconnected before completion"),
            "{error}"
        );
    }

    #[test]
    fn usage_limit_bodies_are_terminal() {
        assert!(is_usage_limit_body(
            r#"{"error":{"type":"usage_limit_reached","message":"You have no credits"}}"#
        ));
        assert!(is_usage_limit_body(
            r#"{"error":{"code":"insufficient_quota","message":"quota"}}"#
        ));
        assert!(is_usage_limit_body(
            r#"{"error":[{"code":"insufficient_quota"}]}"#
        ));
        assert!(is_usage_limit_body("credit_balance_exhausted"));
        assert!(!is_usage_limit_body(
            r#"{"error":{"type":"rate_limit_exceeded","message":"slow down"}}"#
        ));
        let message = usage_limit_message(429, r#"{"error":{"message":"out of credits"}}"#, None);
        assert!(message.contains("out of credits"), "{message}");
        assert!(message.contains("Retrying will not help"), "{message}");
        assert!(message.contains("Try again later"), "{message}");
    }

    #[test]
    fn usage_limit_message_prints_the_advertised_reset() {
        // `resets_in` is relative; the printed time is still local and the
        // Codex-style suffix is present.
        let message = usage_limit_message(
            429,
            r#"{"error":{"message":"quota","resets_in":3600}}"#,
            None,
        );
        assert!(message.contains("Try again at "), "{message}");
        // Epoch seconds and RFC 3339 both parse.
        assert!(usage_reset_at(r#"{"resets_at":4102444800}"#).is_some());
        assert!(usage_reset_at(r#"{"error":{"resets_at":"2099-01-01T00:00:00Z"}}"#).is_some());
        assert!(usage_reset_at(r#"{"error":{"message":"none"}}"#).is_none());
    }

    #[test]
    fn retry_after_header_parses_seconds_without_clamping() {
        let three = RetryAfter::from_header("3").expect("3s advice");
        assert!((2..=3).contains(&three.remaining().as_secs()));
        // Codex honors long server deadlines; the old 60s clamp is gone.
        let long = RetryAfter::from_header("120").expect("120s advice");
        assert!((115..=120).contains(&long.remaining().as_secs()));
        let five_minutes = RetryAfter::from_header("300").expect("advice");
        let remaining = five_minutes.remaining().as_secs();
        assert!((290..=300).contains(&remaining), "{remaining}");
    }

    #[test]
    fn retry_after_http_date_is_honored() {
        let when = std::time::SystemTime::now() + std::time::Duration::from_secs(45);
        let header = httpdate::fmt_http_date(when);
        let advice = RetryAfter::from_header(&header).expect("http date");
        assert!(
            (40..=45).contains(&advice.remaining().as_secs()),
            "expected ~45s, got {:?}",
            advice.remaining()
        );
    }

    #[test]
    fn retry_after_body_uses_explicit_units_without_caps() {
        // Milliseconds are explicit; a long ask is preserved.
        let body = r#"{"error":{"message":"slow down","retry_after_ms":900000}}"#;
        let advice = RetryAfter::from_body(body).expect("ms advice");
        assert!(advice.remaining().as_secs() >= 890);
        // Bare `retry_after` follows the OpenAI seconds convention.
        let advice = RetryAfter::from_body(r#"{"retry_after":300}"#).expect("seconds advice");
        assert!((295..=300).contains(&advice.remaining().as_secs()));
        // Ambiguous strings are not guessed.
        assert!(RetryAfter::from_body(r#"{"retry_after":"5"}"#).is_none());
    }

    #[test]
    fn stream_error_frame_carries_retry_advice() {
        let error = parse_sse_data(r#"{"error":{"message":"overloaded","retry_after_ms":120000}}"#)
            .unwrap_err();
        let delay = error.retry_delay(1).expect("advised delay");
        assert!(delay.as_secs() >= 110, "{delay:?}");
    }

    #[test]
    fn retry_after_deadline_survives_error_construction() {
        let advice = RetryAfter::from_delay(std::time::Duration::from_secs(90));
        let error = ProviderError::HttpStatus {
            status: 429,
            body: "rate limited".to_string(),
            retry_after: advice,
        };
        let delay = error.retry_delay(1).expect("retry delay");
        assert!(delay.as_secs() >= 85, "{delay:?}");
    }

    #[test]
    fn finish_reason_error_is_a_retryable_stream_failure() {
        let error =
            parse_sse_data(r#"{"choices":[{"delta":{},"finish_reason":"error"}]}"#).unwrap_err();
        assert!(matches!(error, ProviderError::Stream { .. }), "{error:?}");
        assert!(error.is_retryable());
    }

    #[test]
    fn backoff_grows_with_attempt() {
        let first = backoff_delay(1);
        let third = backoff_delay(3);
        assert!(third > first);
        assert!(first.as_millis() >= 180 && first.as_millis() <= 220);
    }

    #[test]
    fn circuit_opens_after_repeated_failures_and_resets_on_success() {
        let key = "test-circuit|http://primary|model";
        record_success(key);
        assert!(!circuit_open(key));
        for _ in 0..CIRCUIT_THRESHOLD {
            record_failure(key);
        }
        assert!(circuit_open(key));
        record_success(key);
        assert!(!circuit_open(key));
    }

    fn execute_tool() -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "execute",
                "description": "desc",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "code": {"type": "string"},
                        "timeout": {
                            "type": "integer",
                            "maximum": 9_007_199_254_740_991u64,
                            "exclusiveMinimum": 0,
                            "description": "Optional time in milliseconds to wait for the script before moving it to the background (default 120000). The script keeps running in the background after that; it is never stopped. With `background: true` the call returns immediately instead."
                        },
                        "background": {
                            "type": "boolean",
                            "description": "Run the script in the background and return immediately with its job ID (useful for long-running work). You will be notified when it completes. DO NOT poll for completion."
                        }
                    },
                    "required": ["code"],
                    "additionalProperties": true
                }
            }
        })
    }

    fn execute_custom() -> FreeformCustom {
        FreeformCustom {
            field: "code".to_string(),
            options: true,
        }
    }

    fn execute_customs() -> std::collections::HashMap<String, FreeformCustom> {
        std::collections::HashMap::from([("execute".to_string(), execute_custom())])
    }

    #[test]
    fn freeform_tool_wire_matches_opencode_source() {
        let fixture: Value =
            serde_json::from_str(include_str!("../testdata/opencode_freeform_sequence.json"))
                .unwrap();
        for case in fixture["tools"].as_array().unwrap() {
            let formats = case["formats"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_string())
                .collect::<Vec<_>>();
            let tools = case["tools"].as_array().unwrap().clone();
            let (converted, customs) = freeform_tools(&tools, &formats);
            assert_eq!(
                Value::Array(converted),
                case["expected"],
                "tools {}",
                case["name"]
            );
            let custom_names = case["expected"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|tool| tool["type"] == "custom")
                .map(|tool| tool["custom"]["name"].as_str().unwrap().to_string())
                .collect::<Vec<_>>();
            assert_eq!(
                customs
                    .keys()
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>(),
                custom_names.into_iter().collect(),
                "custom map {}",
                case["name"]
            );
        }
    }

    #[test]
    fn freeform_execute_matches_opencode_wire() {
        let full = serde_json::from_str::<Value>(include_str!(
            "../testdata/opencode_freeform_execute.json"
        ))
        .unwrap();
        assert_eq!(
            freeform_execute(&execute_tool(), &["text".into(), "lark".into()]),
            Some(full.clone())
        );
        // A lark-only model does not accept `execute`'s `text` freeform.
        assert_eq!(freeform_execute(&execute_tool(), &["lark".into()]), None);
        // Text-only models get the options line as unconstrained text, not a
        // Lark grammar.
        let mut text_full = full.clone();
        text_full["custom"]["format"] = json!({"type": "text"});
        assert_eq!(
            freeform_execute(&execute_tool(), &["text".into()]),
            Some(text_full)
        );

        let text = serde_json::from_str::<Value>(include_str!(
            "../testdata/opencode_freeform_execute_text.json"
        ))
        .unwrap();
        let mut simple = execute_tool();
        simple["function"]["parameters"] = json!({"type": "object", "properties": {"code": {"type": "string"}}, "required": ["code"]});
        assert_eq!(freeform_execute(&simple, &["text".into()]), Some(text));
    }

    fn apply_patch_tool() -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "apply_patch",
                "description": "Apply a unified patch.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "input": {
                            "type": "string",
                            "description": "Patch text. Start with *** Begin Patch and end with *** End Patch."
                        }
                    },
                    "required": ["input"]
                }
            }
        })
    }

    fn patch_custom() -> FreeformCustom {
        FreeformCustom {
            field: "input".to_string(),
            options: false,
        }
    }

    #[test]
    fn apply_patch_uses_opencode_patch_grammar() {
        let tools = vec![execute_tool(), apply_patch_tool()];
        let (converted, customs) = freeform_tools(&tools, &["text".into(), "lark".into()]);
        assert!(customs.contains_key("execute"));
        assert!(customs.contains_key("apply_patch"));
        let patch = converted
            .iter()
            .find(|tool| tool["custom"]["name"] == "apply_patch")
            .expect("patch custom tool");
        assert_eq!(patch["type"], json!("custom"));
        assert_eq!(
            patch["custom"]["description"],
            json!("Apply a unified patch.")
        );
        assert_eq!(patch["custom"]["format"]["type"], json!("grammar"));
        assert_eq!(
            patch["custom"]["format"]["grammar"]["definition"],
            json!(PATCH_GRAMMAR)
        );
        assert_eq!(
            patch["custom"]["format"]["grammar"]["syntax"],
            json!("lark")
        );
        assert!(!PATCH_GRAMMAR.contains("// @options:"));
        assert!(PATCH_GRAMMAR.starts_with("start: begin_patch hunk+ end_patch\n"));
        assert!(PATCH_GRAMMAR.ends_with("%import common.LF\n"));

        // Lark-only models accept the patch tool but not text freeform execute.
        let (converted, customs) = freeform_tools(&tools, &["lark".into()]);
        assert!(!customs.contains_key("execute"));
        assert!(customs.contains_key("apply_patch"));
        assert_eq!(converted[0]["type"], json!("function"));
        assert_eq!(converted[1]["type"], json!("custom"));

        // Text-only models keep the patch JSON tool.
        let (converted, customs) = freeform_tools(&tools, &["text".into()]);
        assert!(customs.contains_key("execute"));
        assert!(!customs.contains_key("apply_patch"));
        assert_eq!(converted[1]["type"], json!("function"));
    }

    #[test]
    fn apply_patch_replay_and_stream_use_the_input_field() {
        assert_eq!(
            freeform_raw(
                &json!({"input": "*** Begin Patch\n*** End Patch"}),
                None,
                &patch_custom()
            ),
            "*** Begin Patch\n*** End Patch"
        );
        let messages = vec![ChatMessage::assistant_tool_calls(
            None,
            vec![ToolCall::function(
                "call_1",
                "apply_patch",
                r#"{"input":"*** Begin Patch\n*** End Patch"}"#,
            )],
        )];
        let customs =
            std::collections::HashMap::from([("apply_patch".to_string(), patch_custom())]);
        let value = freeform_messages(&messages, &customs);
        assert_eq!(
            value[0]["tool_calls"][0],
            json!({
                "id": "call_1",
                "type": "custom",
                "custom": {
                    "name": "apply_patch",
                    "input": "*** Begin Patch\n*** End Patch"
                }
            })
        );
    }

    #[tokio::test]
    async fn apply_patch_stream_fragments_use_the_input_field() {
        let events = vec![
            Ok(ProviderEvent::CustomTool {
                index: 0,
                id: Some("c1".to_string()),
                name: Some("apply_patch".to_string()),
                input: Some(
                    "*** Begin Patch
"
                    .to_string(),
                ),
            }),
            Ok(ProviderEvent::CustomTool {
                index: 0,
                id: None,
                name: None,
                input: Some("*** End Patch".to_string()),
            }),
            Ok(ProviderEvent::Done {
                finish_reason: Some("tool_calls".to_string()),
            }),
        ];
        let customs =
            std::collections::HashMap::from([("apply_patch".to_string(), patch_custom())]);
        let mut stream = freeform_stream(Box::pin(futures_util::stream::iter(events)), customs);
        let mut fragments = String::new();
        let mut final_call = None;
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                ProviderEvent::ToolCall {
                    arguments: Some(fragment),
                    ..
                } => fragments.push_str(&fragment),
                ProviderEvent::ToolCallFinal {
                    arguments, header, ..
                } => final_call = Some((arguments, header)),
                ProviderEvent::Done { .. } => break,
                _ => {}
            }
        }
        assert_eq!(fragments, "{\"input\":\"*** Begin Patch\\n*** End Patch\"}");
        assert_eq!(
            final_call,
            Some((
                "{\"input\":\"*** Begin Patch\\n*** End Patch\"}".to_string(),
                None
            ))
        );
    }

    #[test]
    fn freeform_arguments_and_replay_match_opencode() {
        assert_eq!(
            freeform_parse(
                "// @options: {\"timeout\": 1}
code",
                true,
                "code"
            )
            .0,
            json!({"timeout": 1, "code": "code"}).to_string()
        );
        assert_eq!(
            freeform_parse("plain code", false, "code").0,
            json!({"code": "plain code"}).to_string()
        );
        // OpenCode keeps malformed options text raw so JSON parsing rejects it.
        assert_eq!(
            freeform_parse(
                "// @options: {timeout: 1}
x",
                true,
                "code"
            ),
            (
                "// @options: {timeout: 1}
x"
                .to_string(),
                None
            )
        );
        assert_eq!(
            freeform_parse("// @options: {\"timeout\": 1}", true, "code").0,
            json!({"code": "// @options: {\"timeout\": 1}"}).to_string()
        );
        // An undeclared custom tool falls back to the `input` field.
        assert_eq!(
            freeform_parse("raw", false, "input").0,
            json!({"input": "raw"}).to_string()
        );
        assert_eq!(
            freeform_raw(
                &json!({"code": "x", "timeout": 1, "background": true}),
                None,
                &execute_custom()
            ),
            "// @options: {\"timeout\":1,\"background\":true}
x"
        );
        assert_eq!(
            freeform_raw(&json!({"code": "x"}), None, &execute_custom()),
            "x"
        );
        let exact = "// @options: { \"background\" : true, \"timeout\": 2}";
        let input = json!({"code": "x", "timeout": 2, "background": true});
        assert_eq!(
            freeform_raw(&input, Some(exact), &execute_custom()),
            format!(
                "{exact}
x"
            )
        );
        assert_eq!(
            freeform_raw(
                &json!({"code": "x", "timeout": 3, "background": true}),
                Some(exact),
                &execute_custom()
            ),
            "// @options: {\"timeout\":3,\"background\":true}
x"
        );

        let messages = vec![ChatMessage::assistant_tool_calls(
            None,
            vec![ToolCall::function(
                "call_1",
                "execute",
                r#"{"code":"x","timeout":1,"background":true}"#,
            )],
        )];
        let value = freeform_messages(&messages, &execute_customs());
        let expected_input = "// @options: {\"timeout\":1,\"background\":true}
"
        .to_string()
            + "x";
        assert_eq!(
            value[0]["tool_calls"][0],
            json!({
                "id": "call_1",
                "type": "custom",
                "custom": {
                    "name": "execute",
                    "input": expected_input
                }
            })
        );
    }

    #[test]
    fn custom_tool_deltas_parse() {
        let events = parse_sse_data(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","type":"custom","custom":{"name":"execute","input":"let x"}}]}}]}"#,
        )
        .unwrap();
        assert!(matches!(
            &events[0],
            ProviderEvent::CustomTool { index: 0, id: Some(id), name: Some(name), input: Some(input) }
                if id == "c1" && name == "execute" && input == "let x"
        ));
    }

    #[tokio::test]
    async fn freeform_stream_rebuilds_json_arguments() {
        let events = vec![
            Ok(ProviderEvent::CustomTool {
                index: 0,
                id: Some("c1".to_string()),
                name: Some("execute".to_string()),
                input: Some("let ".to_string()),
            }),
            Ok(ProviderEvent::CustomTool {
                index: 0,
                id: None,
                name: None,
                input: Some("x = 1".to_string()),
            }),
            Ok(ProviderEvent::Done {
                finish_reason: Some("tool_calls".to_string()),
            }),
        ];
        let mut stream = freeform_stream(
            Box::pin(futures_util::stream::iter(events)),
            execute_customs(),
        );
        let first = stream.next().await.unwrap().unwrap();
        assert!(matches!(
            first,
            ProviderEvent::ToolCall { index: 0, arguments: Some(arguments), .. }
                if arguments == "{\"code\":\"let "
        ));
        let second = stream.next().await.unwrap().unwrap();
        assert!(matches!(
            second,
            ProviderEvent::ToolCall { index: 0, arguments: Some(arguments), .. }
                if arguments == "x = 1"
        ));
        let close = stream.next().await.unwrap().unwrap();
        assert!(matches!(
            close,
            ProviderEvent::ToolCall { index: 0, arguments: Some(arguments), .. }
                if arguments == "\"}"
        ));
        let final_call = stream.next().await.unwrap().unwrap();
        assert!(matches!(
            final_call,
            ProviderEvent::ToolCallFinal { index: 0, arguments, header: None, .. }
                if arguments == r#"{"code":"let x = 1"}"#
        ));
        assert!(matches!(
            stream.next().await.unwrap().unwrap(),
            ProviderEvent::Done { .. }
        ));
    }

    #[tokio::test]
    async fn freeform_stream_keeps_the_exact_options_header() {
        let header = "// @options: { \"timeout\" : 1}";
        let raw = format!(
            "{header}
x"
        );
        let events = vec![
            Ok(ProviderEvent::CustomTool {
                index: 0,
                id: Some("c1".to_string()),
                name: Some("execute".to_string()),
                input: Some(raw),
            }),
            Ok(ProviderEvent::Done {
                finish_reason: Some("tool_calls".to_string()),
            }),
        ];
        let mut stream = freeform_stream(
            Box::pin(futures_util::stream::iter(events)),
            execute_customs(),
        );
        let _opening = stream.next().await.unwrap().unwrap();
        let _close = stream.next().await.unwrap().unwrap();
        let final_call = stream.next().await.unwrap().unwrap();
        match final_call {
            ProviderEvent::ToolCallFinal {
                index: 0,
                arguments,
                header: Some(saved),
                ..
            } => {
                assert_eq!(arguments, r#"{"timeout":1,"code":"x"}"#);
                assert_eq!(saved, header);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn textless_tool_turn_serializes_content_null() {
        let message = ChatMessage::assistant_tool_calls(
            None,
            vec![ToolCall::function("c1", "read", r#"{"file_path":"a"}"#)],
        );
        let value = serde_json::to_value(&message).unwrap();
        assert_eq!(value["content"], Value::Null);
        assert_eq!(value["role"], json!("assistant"));
        let text = ChatMessage::assistant("thinking");
        assert_eq!(
            serde_json::to_value(&text).unwrap()["content"],
            json!("thinking")
        );
    }

    #[test]
    fn replayed_function_arguments_match_opencode_encode_json() {
        let fixture: Value =
            serde_json::from_str(include_str!("../testdata/opencode_freeform_sequence.json"))
                .unwrap();
        for case in fixture["json"].as_array().unwrap() {
            let parsed = serde_json::from_str::<Value>(case["raw"].as_str().unwrap()).unwrap();
            assert_eq!(
                js_json(&parsed),
                case["normalized"].as_str().unwrap(),
                "json {}",
                case["raw"]
            );
        }

        let mut messages = json!([
            {"role": "system", "content": "base"},
            {"role": "assistant", "tool_calls": [
                {"id": "c1", "type": "function", "function": {
                    "name": "read", "arguments": "{\"file_path\":\"a.txt\", \"limit\": 2}"
                }},
                {"id": "c2", "type": "custom", "custom": {"name": "execute", "input": "x"}}
            ]},
            {"role": "assistant", "tool_calls": [
                {"id": "c3", "type": "function", "function": {
                    "name": "read", "arguments": "{not json"
                }}
            ]}
        ]);
        normalize_function_arguments(&mut messages);
        assert_eq!(
            messages[1]["tool_calls"][0]["function"]["arguments"],
            json!("{\"file_path\":\"a.txt\",\"limit\":2}")
        );
        assert_eq!(messages[1]["tool_calls"][1]["custom"]["input"], json!("x"));
        assert_eq!(
            messages[2]["tool_calls"][0]["function"]["arguments"],
            json!("{not json")
        );
    }

    #[test]
    fn freeform_parse_and_replay_match_opencode_source() {
        let fixture: Value =
            serde_json::from_str(include_str!("../testdata/opencode_freeform_sequence.json"))
                .unwrap();
        for case in fixture["parse"].as_array().unwrap() {
            let raw = case["raw"].as_str().unwrap();
            let (options, field) = match case["custom"].as_str().unwrap() {
                "full" => (true, "code"),
                "plain" => (false, "code"),
                _ => (false, "input"),
            };
            let (arguments, header) = freeform_parse(raw, options, field);
            assert_eq!(
                arguments,
                case["input"].as_str().unwrap(),
                "parse {}",
                case["name"]
            );
            assert_eq!(
                header.as_deref(),
                case["header"].as_str(),
                "parse header {}",
                case["name"]
            );
        }
        for case in fixture["replay"].as_array().unwrap() {
            let raw = freeform_raw(&case["input"], case["header"].as_str(), &execute_custom());
            assert_eq!(
                raw,
                case["raw"].as_str().unwrap(),
                "replay {}",
                case["name"]
            );
        }
    }

    #[tokio::test]
    async fn freeform_stream_fragments_match_opencode_source() {
        let fixture: Value =
            serde_json::from_str(include_str!("../testdata/opencode_freeform_sequence.json"))
                .unwrap();
        for case in fixture["stream"].as_array().unwrap() {
            let chunks = case["chunks"].as_array().unwrap();
            let mut events = Vec::new();
            for (index, chunk) in chunks.iter().enumerate() {
                events.push(Ok(ProviderEvent::CustomTool {
                    index: 0,
                    id: (index == 0).then(|| "c1".to_string()),
                    name: (index == 0).then(|| "execute".to_string()),
                    input: Some(chunk.as_str().unwrap().to_string()),
                }));
            }
            events.push(Ok(ProviderEvent::Done {
                finish_reason: Some("tool_calls".to_string()),
            }));
            let mut stream = freeform_stream(
                Box::pin(futures_util::stream::iter(events)),
                execute_customs(),
            );
            let mut fragments = Vec::new();
            while let Some(event) = stream.next().await {
                match event.unwrap() {
                    ProviderEvent::ToolCall {
                        arguments: Some(fragment),
                        ..
                    } => fragments.push(fragment),
                    ProviderEvent::Done { .. } => break,
                    _ => {}
                }
            }
            let expected = case["fragments"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_string())
                .collect::<Vec<_>>();
            assert_eq!(fragments, expected, "stream {}", case["name"]);
        }
    }

    #[test]
    fn system_update_wrapping_matches_opencode_source() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../stark-tools/src/codemode/testdata/opencode_instruction_sequence.json"
        ))
        .unwrap();
        let mut checked = 0;
        for scenario in fixture["scenarios"].as_array().unwrap() {
            for step in scenario["steps"].as_array().unwrap() {
                let Some(update) = step["update"].as_str() else {
                    continue;
                };
                assert_eq!(
                    wrap_system_update(update),
                    step["wrapped"].as_str().unwrap(),
                    "{}",
                    scenario["name"]
                );
                checked += 1;
            }
        }
        assert!(checked >= 6, "checked {checked} updates");
    }

    #[test]
    fn chronological_system_updates_match_opencode_lowering() {
        let messages = vec![
            ChatMessage::system("base"),
            ChatMessage::user("first"),
            ChatMessage::system("use <x> & y"),
            ChatMessage::assistant("ok"),
            ChatMessage::system("second"),
        ];
        let mut value = serde_json::to_value(&messages).unwrap();
        lower_system_updates(&mut value);
        assert_eq!(
            value,
            json!([
                {"role": "system", "content": "base"},
                {"role": "user", "content": "first\n<system-update>\nuse &lt;x&gt; &amp; y\n</system-update>"},
                {"role": "assistant", "content": "ok"},
                {"role": "user", "content": "<system-update>\nsecond\n</system-update>"}
            ])
        );
    }

    #[test]
    fn system_updates_append_to_user_content_parts() {
        let messages = vec![
            ChatMessage::system("base"),
            ChatMessage::user_with_images(
                "look",
                vec![ImagePart {
                    url: "data:image/png;base64,AA==".to_string(),
                    note: String::new(),
                }],
            ),
            ChatMessage::system("catalog changed"),
            ChatMessage::user("next"),
        ];
        let mut value = serde_json::to_value(&messages).unwrap();
        lower_system_updates(&mut value);
        let content = value[1]["content"].as_array().expect("parts");
        assert_eq!(content[0], json!({"type": "text", "text": "look"}));
        assert_eq!(content[1]["type"], json!("image_url"));
        assert_eq!(
            content[2],
            json!({"type": "text", "text": "<system-update>\ncatalog changed\n</system-update>"})
        );
        assert_eq!(value[2], json!({"role": "user", "content": "next"}));
    }

    #[tokio::test]
    async fn freeform_request_and_stream_round_trip() {
        use std::sync::{Arc, Mutex};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let captured = Arc::new(Mutex::new(serde_json::Value::Null));
        let seen = captured.clone();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let read = socket.read(&mut chunk).await.unwrap();
                if read == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..read]);
                let separator = [13u8, 10u8, 13, 10];
                let Some(position) = buffer
                    .windows(separator.len())
                    .position(|window| window == separator)
                else {
                    continue;
                };
                let text = String::from_utf8_lossy(&buffer[..position]);
                let length = text
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                if buffer.len() >= position + separator.len() + length {
                    let body = &buffer[position + separator.len()..];
                    if let Ok(value) = serde_json::from_slice(body) {
                        *seen.lock().unwrap() = value;
                    }
                    break;
                }
            }
            let blank = String::from_utf8(vec![10, 10]).unwrap();
            let events = [
                r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","type":"custom","custom":{"name":"execute","input":"let "}}]}}]}"#,
                r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"custom":{"input":"x = 1"}}]}}]}"#,
                r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
                "data: [DONE]",
                "",
            ]
            .join(&blank);
            let mut response = Vec::new();
            response.extend_from_slice(b"HTTP/1.1 200 OK");
            response.extend_from_slice(&[13, 10]);
            response.extend_from_slice(b"Content-Type: text/event-stream");
            response.extend_from_slice(&[13, 10]);
            response.extend_from_slice(format!("Content-Length: {}", events.len()).as_bytes());
            response.extend_from_slice(&[13, 10]);
            response.extend_from_slice(b"Connection: close");
            response.extend_from_slice(&[13, 10, 13, 10]);
            response.extend_from_slice(events.as_bytes());
            let _ = socket.write_all(&response).await;
        });

        let provider = Provider::new(ProviderConfig {
            mode: "opencode".to_string(),
            base_url: format!("http://{address}"),
            api_key: "test".to_string(),
            model: "mock".to_string(),
        })
        .unwrap()
        .with_freeform(vec!["text".to_string(), "lark".to_string()]);
        let messages = vec![
            ChatMessage::assistant_tool_calls(
                None,
                vec![
                    ToolCall::function("c0", "execute", r#"{"code":"old","timeout":1}"#)
                        .with_custom_header(Some("// @options: { \"timeout\" : 1}")),
                ],
            ),
            ChatMessage::tool("c0", "done"),
            ChatMessage::user("run it"),
        ];
        let mut stream = provider
            .stream_chat(&messages, &[execute_tool()], 0.0)
            .await
            .unwrap();
        let mut fragments = String::new();
        let mut final_call = None;
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                ProviderEvent::ToolCall {
                    arguments: Some(arguments),
                    ..
                } => fragments.push_str(&arguments),
                ProviderEvent::ToolCallFinal {
                    arguments, header, ..
                } => final_call = Some((arguments, header)),
                ProviderEvent::Done { .. } => break,
                _ => {}
            }
        }
        assert_eq!(fragments, r#"{"code":"let x = 1"}"#);
        assert_eq!(
            final_call,
            Some((r#"{"code":"let x = 1"}"#.to_string(), None))
        );

        let body = captured.lock().unwrap().clone();
        assert!(body.get("tool_choice").is_none());
        assert!(body.get("max_tokens").is_none());
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["stream_options"], json!({"include_usage": true}));
        assert_eq!(body["temperature"], json!(0.0));
        assert_eq!(body["messages"][0]["content"], Value::Null);
        assert_eq!(body["tools"][0]["type"], json!("custom"));
        assert_eq!(body["tools"][0]["custom"]["name"], json!("execute"));
        assert_eq!(
            body["tools"][0]["custom"]["format"]["type"],
            json!("grammar")
        );
        assert_eq!(
            body["messages"][0]["tool_calls"][0]["type"],
            json!("custom")
        );
        assert_eq!(
            body["messages"][0]["tool_calls"][0]["custom"]["input"],
            json!("// @options: { \"timeout\" : 1}\nold")
        );
        assert_eq!(body["messages"][2]["content"], json!("run it"));
    }

    #[test]
    fn provider_keeps_a_fallback_config() {
        let primary = Provider::new(ProviderConfig {
            mode: "opencode".to_string(),
            base_url: "https://primary.example/v1".to_string(),
            api_key: "primary".to_string(),
            model: "primary-model".to_string(),
        })
        .unwrap()
        .with_fallback(ProviderConfig {
            mode: "openrouter".to_string(),
            base_url: "https://fallback.example/v1".to_string(),
            api_key: "fallback".to_string(),
            model: "fallback-model".to_string(),
        });
        assert_eq!(primary.config().model, "primary-model");
        assert_eq!(
            primary.fallback().map(|config| config.model.as_str()),
            Some("fallback-model")
        );
    }
}
