use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanItem {
    pub step: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TokenUsage {
    #[serde(default)]
    pub total_input_tokens: u64,
    #[serde(default)]
    pub total_output_tokens: u64,
    #[serde(default)]
    pub total_cache_hit_tokens: u64,
    #[serde(default)]
    pub total_cache_miss_tokens: u64,
    #[serde(default)]
    pub total_cost: f64,
    #[serde(default)]
    pub calls: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppState {
    #[serde(default)]
    pub active_node: String,
    #[serde(default)]
    pub next_agent: String,
    #[serde(default)]
    pub completed_nodes: Vec<String>,
    #[serde(default)]
    pub thoughts: BTreeMap<String, String>,
    #[serde(default)]
    pub client_request: String,
    #[serde(default = "empty_object")]
    pub outputs: Value,
    #[serde(default)]
    pub project_path: String,
    #[serde(default)]
    pub agents_plan: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<BTreeMap<String, Vec<PlanItem>>>,
    #[serde(default)]
    pub antigravity_log: Vec<Value>,
    #[serde(default)]
    pub live_terminal_log: String,
    #[serde(default)]
    pub selected_model: String,
    #[serde(default)]
    pub selected_temp: f64,
    #[serde(default)]
    pub token_usage: TokenUsage,
    /// Provider-reported context used by the last call (input + output).
    #[serde(default)]
    pub context_tokens: u64,
    /// Model context window used for compaction (0 when unknown).
    #[serde(default)]
    pub context_window: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_ref: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traceback: Option<String>,
    #[serde(default)]
    pub state_version: u64,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            active_node: String::new(),
            next_agent: String::new(),
            completed_nodes: Vec::new(),
            thoughts: BTreeMap::new(),
            client_request: String::new(),
            outputs: empty_object(),
            project_path: String::new(),
            agents_plan: String::new(),
            plan: None,
            antigravity_log: Vec::new(),
            live_terminal_log: String::new(),
            selected_model: String::new(),
            selected_temp: 0.7,
            token_usage: TokenUsage::default(),
            context_tokens: 0,
            context_window: 0,
            artifacts: None,
            project_ref: None,
            error: None,
            traceback: None,
            state_version: 0,
        }
    }
}

fn empty_object() -> Value {
    json!({})
}
