use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delta {
    pub kind: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ts: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ms: Option<u64>,
}

impl Delta {
    pub fn text(text: impl Into<String>) -> Self {
        Self::new("text", text)
    }

    pub fn reasoning(text: impl Into<String>) -> Self {
        Self::new("reasoning", text)
    }

    pub fn usage(text: impl Into<String>) -> Self {
        Self::new("usage", text)
    }

    pub fn end(text: impl Into<String>) -> Self {
        Self::new("end", text)
    }

    pub fn tool_started(tool: impl Into<String>, args: impl Into<String>) -> Self {
        Self {
            kind: "tool_started".to_string(),
            text: String::new(),
            agent: None,
            model: None,
            ts: None,
            tool: Some(tool.into()),
            args: Some(args.into()),
            ok: None,
            ms: None,
        }
    }

    pub fn tool_finished(
        tool: impl Into<String>,
        args: impl Into<String>,
        ok: bool,
        ms: u64,
        preview: impl Into<String>,
    ) -> Self {
        Self {
            kind: "tool_finished".to_string(),
            text: preview.into(),
            agent: None,
            model: None,
            ts: None,
            tool: Some(tool.into()),
            args: Some(args.into()),
            ok: Some(ok),
            ms: Some(ms),
        }
    }

    pub fn with_agent(mut self, agent: impl Into<String>) -> Self {
        self.agent = Some(agent.into());
        self
    }

    fn new(kind: &str, text: impl Into<String>) -> Self {
        Self {
            kind: kind.to_string(),
            text: text.into(),
            agent: None,
            model: None,
            ts: None,
            tool: None,
            args: None,
            ok: None,
            ms: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ServerEvent {
    pub event: String,
    pub data: Value,
}

impl ServerEvent {
    pub fn new(event: impl Into<String>, data: impl Serialize) -> Self {
        Self {
            event: event.into(),
            data: serde_json::to_value(data).unwrap_or(Value::Null),
        }
    }
}
