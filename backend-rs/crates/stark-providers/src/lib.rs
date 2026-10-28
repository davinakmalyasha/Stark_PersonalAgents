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
    pub output_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
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
    #[error("provider HTTP {status}: {body}")]
    HttpStatus { status: u16, body: String },
    #[error("provider request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("provider stream failed: {0}")]
    Stream(String),
    #[error("provider returned invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
}

pub type EventStream = Pin<Box<dyn Stream<Item = Result<ProviderEvent, ProviderError>> + Send>>;

#[derive(Clone)]
pub struct Provider {
    client: reqwest::Client,
    config: ProviderConfig,
}

impl Provider {
    pub fn new(config: ProviderConfig) -> Result<Self, ProviderError> {
        let client = reqwest::Client::builder()
            .user_agent("stark-rs/0.1")
            .build()?;
        Ok(Self { client, config })
    }

    pub fn config(&self) -> &ProviderConfig {
        &self.config
    }

    pub async fn stream_chat(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        temperature: f64,
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
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools.to_vec());
            body["tool_choice"] = Value::String("auto".to_string());
        }

        let mut attempt = 0usize;
        let response = loop {
            let response = self
                .client
                .post(self.config.endpoint())
                .bearer_auth(&self.config.api_key)
                .json(&body)
                .send()
                .await?;
            if response.status().is_success() {
                break response;
            }
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            if matches!(status, 429 | 500 | 502 | 503 | 504) && attempt < 2 {
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_millis(700 * attempt as u64)).await;
                continue;
            }
            return Err(ProviderError::HttpStatus { status, body });
        };

        let events = response.bytes_stream().eventsource();
        let stream = async_stream::try_stream! {
            let mut events = Box::pin(events);
            while let Some(event) = events.next().await {
                let event = event.map_err(|err| ProviderError::Stream(err.to_string()))?;
                for parsed in parse_sse_data(&event.data)? {
                    yield parsed;
                }
            }
        };

        Ok(Box::pin(stream))
    }
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
    Usage {
        input_tokens: value
            .get("prompt_tokens")
            .or_else(|| value.get("input_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_tokens: value
            .get("completion_tokens")
            .or_else(|| value.get("output_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        total_tokens: value
            .get("total_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
