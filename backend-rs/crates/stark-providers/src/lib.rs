use std::pin::Pin;

use eventsource_stream::Eventsource;
use futures_util::{Stream, StreamExt};
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

impl ProviderConfig {
    pub fn from_env() -> Self {
        let requested_mode = std::env::var("STARK_PROVIDER")
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        let explicit_base = first_env(&["STARK_PROVIDER_BASE", "OPENAI_BASE_URL"]);
        let explicit_key = first_env(&["STARK_API_KEY", "OPENAI_API_KEY"]);
        let explicit_model = first_env(&["STARK_MODEL", "OPENAI_MODEL"]);

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

        if requested_mode == "opencode" {
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
            mode: if requested_mode.is_empty() {
                "deepseek".to_string()
            } else {
                requested_mode
            },
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
        if self.mode == "command"
            && matches!(
                requested,
                "deepseek-v4.1-flash" | "deepseek-flash" | "deepseek:v4.1-flash"
            )
        {
            return self.model.clone();
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
pub struct ChatMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
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
        }
    }

    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_string(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            name: None,
        }
    }

    fn text(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.to_string(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
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

        let response = self
            .client
            .post(self.config.endpoint())
            .bearer_auth(&self.config.api_key)
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::HttpStatus { status, body });
        }

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
}
