//! Append-only rollout log: the canonical trace of a chat.
//!
//! One JSONL file per chat, written as work happens. The file is the source
//! of truth; `chats.db` messages are a projection for lists and search.
//!
//! Line envelope matches the reference format:
//! `{"timestamp":"...","ordinal":N,"type":"...","payload":{...}}`
//! Readers tolerate a truncated final line (crash mid-write) and skip
//! unknown item types so newer writers stay forward-compatible.

use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{StoreError, projects_root};

/// Portable cwd stored in `session_meta`. Absolute paths never enter the log.
pub fn portable_cwd(workspace: &Path) -> String {
    let root = projects_root();
    if let Ok(rel) = workspace.strip_prefix(&root) {
        let rel = rel.to_string_lossy().replace('\\', "/");
        if rel.is_empty() {
            return "%PROJECTS_ROOT%".to_string();
        }
        return format!("%PROJECTS_ROOT%/{rel}");
    }
    // Virtual workspaces and harness roots have no registry folder.
    workspace
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionMetaPayload {
    pub session_id: String,
    pub id: String,
    pub timestamp: String,
    pub cwd: String,
    pub workspace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub originator: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TurnContextPayload {
    pub turn_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReasoningSummary {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseItemPayload {
    Message {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        role: String,
        content: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_reason: Option<String>,
    },
    Reasoning {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        summary: Vec<ReasoningSummary>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<String>,
    },
    FunctionCall {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        call_id: String,
        name: String,
        arguments: String,
    },
    FunctionCallOutput {
        call_id: String,
        name: String,
        output: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventMsgPayload {
    Error {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },
    Warning {
        message: String,
    },
    TurnStarted {
        turn_id: String,
    },
    TurnComplete {
        turn_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompactedPayload {
    pub summary: String,
    #[serde(default)]
    pub retained_tail: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TokenUsagePayload {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
}

/// Canonical rollout item. Wire names match the reference implementation for
/// the shared subset (`session_meta`, `turn_context`, `response_item`,
/// `event_msg`, `compacted`, `token_usage_record`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RolloutItem {
    SessionMeta {
        payload: SessionMetaPayload,
    },
    TurnContext {
        payload: TurnContextPayload,
    },
    ResponseItem {
        payload: ResponseItemPayload,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<Value>,
    },
    EventMsg {
        payload: EventMsgPayload,
    },
    Compacted {
        payload: CompactedPayload,
    },
    TokenUsageRecord {
        payload: TokenUsagePayload,
    },
}

/// One persisted JSONL record.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RolloutLine {
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ordinal: Option<u64>,
    #[serde(flatten)]
    pub item: RolloutItem,
}

impl<'de> Deserialize<'de> for RolloutLine {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            timestamp: String,
            #[serde(default)]
            ordinal: Option<u64>,
            #[serde(rename = "type")]
            kind: String,
            #[serde(default)]
            payload: Value,
            #[serde(default)]
            metadata: Option<Value>,
        }
        let raw = Raw::deserialize(deserializer)?;
        let item = match raw.kind.as_str() {
            "session_meta" => RolloutItem::SessionMeta {
                payload: serde_json::from_value(raw.payload).map_err(serde::de::Error::custom)?,
            },
            "turn_context" => RolloutItem::TurnContext {
                payload: serde_json::from_value(raw.payload).map_err(serde::de::Error::custom)?,
            },
            "response_item" => {
                let payload: ResponseItemPayload =
                    serde_json::from_value(raw.payload).map_err(serde::de::Error::custom)?;
                RolloutItem::ResponseItem {
                    payload,
                    metadata: raw.metadata,
                }
            }
            "event_msg" => RolloutItem::EventMsg {
                payload: serde_json::from_value(raw.payload).map_err(serde::de::Error::custom)?,
            },
            "compacted" => RolloutItem::Compacted {
                payload: serde_json::from_value(raw.payload).map_err(serde::de::Error::custom)?,
            },
            "token_usage_record" | "token_usage" => RolloutItem::TokenUsageRecord {
                payload: serde_json::from_value(raw.payload).map_err(serde::de::Error::custom)?,
            },
            other => {
                return Err(serde::de::Error::custom(format!(
                    "unknown rollout item type: {other}"
                )));
            }
        };
        Ok(Self {
            timestamp: raw.timestamp,
            ordinal: raw.ordinal,
            item,
        })
    }
}

pub fn now_timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn sanitize_chat_id(chat_id: &str) -> String {
    chat_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn rollout_dir(harness_root: &Path) -> PathBuf {
    harness_root.join(".deep_agents").join("rollouts")
}

pub fn new_rollout_path(harness_root: &Path, chat_id: &str) -> PathBuf {
    let now = Utc::now();
    let date = now.format("%Y/%m/%d").to_string();
    let stamp = now.format("%Y-%m-%dT%H-%M-%S").to_string();
    rollout_dir(harness_root).join(date).join(format!(
        "rollout-{stamp}-{}.jsonl",
        sanitize_chat_id(chat_id)
    ))
}

/// Finds the newest rollout file for a chat by scanning the rollouts tree.
/// Prefers the index table when available; falls back to a scan.
pub fn find_rollout(harness_root: &Path, chat_id: &str) -> Option<PathBuf> {
    let safe = sanitize_chat_id(chat_id);
    let root = rollout_dir(harness_root);
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = path.file_name()?.to_string_lossy().to_string();
            if name.ends_with(&format!("-{safe}.jsonl")) && name.starts_with("rollout-") {
                let modified = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::UNIX_EPOCH);
                if best.as_ref().is_none_or(|(time, _)| &modified >= time) {
                    best = Some((modified, path));
                }
            }
        }
    }
    best.map(|(_, path)| path)
}

/// Reads a rollout file. A truncated final line (crash mid-write) is ignored;
/// unknown item types are skipped so newer writers stay readable.
pub fn read_rollout(path: &Path) -> Vec<RolloutLine> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    parse_rollout_text(&text)
}

pub fn parse_rollout_text(text: &str) -> Vec<RolloutLine> {
    let mut lines = Vec::new();
    let mut raw_lines: Vec<&str> = text.lines().collect();
    // Drop a truncated final line: valid JSONL lines end with `}` (possibly
    // followed by whitespace). A crash mid-write leaves a partial object.
    if let Some(last) = raw_lines.last()
        && !last.trim().is_empty()
        && !last.trim_end().ends_with('}')
        && serde_json::from_str::<Value>(last).is_err()
    {
        raw_lines.pop();
    }
    for line in raw_lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<RolloutLine>(line) {
            Ok(parsed) => lines.push(parsed),
            Err(_) => {
                // Unknown future item types must not break resume.
                continue;
            }
        }
    }
    lines
}

/// Append-only writer. Each `append` assigns the next ordinal and flushes.
pub struct RolloutWriter {
    path: PathBuf,
    next_ordinal: u64,
}

impl RolloutWriter {
    pub async fn open(
        harness_root: &Path,
        chat_id: &str,
        workspace_id: &str,
        workspace: &Path,
        model: &str,
    ) -> Result<Self, StoreError> {
        if let Some(existing) = find_rollout(harness_root, chat_id) {
            let lines = read_rollout(&existing);
            let next_ordinal = lines
                .iter()
                .filter_map(|line| line.ordinal)
                .max()
                .map(|max| max + 1)
                .unwrap_or(lines.len() as u64);
            return Ok(Self {
                path: existing,
                next_ordinal,
            });
        }
        let path = new_rollout_path(harness_root, chat_id);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let rel = workspace
            .strip_prefix(crate::projects_root())
            .ok()
            .map(|rel| rel.to_string_lossy().replace('\\', "/"))
            .filter(|rel| !rel.is_empty());
        let mut writer = Self {
            path,
            next_ordinal: 0,
        };
        writer
            .append(RolloutItem::SessionMeta {
                payload: SessionMetaPayload {
                    session_id: chat_id.to_string(),
                    id: chat_id.to_string(),
                    timestamp: now_timestamp(),
                    cwd: portable_cwd(workspace),
                    workspace_id: workspace_id.to_string(),
                    rel,
                    model: Some(model.to_string()),
                    provider: None,
                    originator: Some("stark".to_string()),
                },
            })
            .await?;
        Ok(writer)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn relative_path(&self, harness_root: &Path) -> String {
        self.path
            .strip_prefix(harness_root)
            .map(|rel| rel.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| self.path.to_string_lossy().to_string())
    }

    pub async fn append(&mut self, item: RolloutItem) -> Result<u64, StoreError> {
        let ordinal = self.next_ordinal;
        self.next_ordinal += 1;
        let line = RolloutLine {
            timestamp: now_timestamp(),
            ordinal: Some(ordinal),
            item,
        };
        let mut text =
            serde_json::to_string(&line).map_err(|e| StoreError::Invalid(e.to_string()))?;
        text.push('\n');
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await?;
        file.write_all(text.as_bytes()).await?;
        file.flush().await?;
        Ok(ordinal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollout_round_trip() {
        let line = RolloutLine {
            timestamp: "2025-05-07T17:24:21.000Z".to_string(),
            ordinal: Some(3),
            item: RolloutItem::ResponseItem {
                payload: ResponseItemPayload::Message {
                    id: None,
                    role: "user".to_string(),
                    content: serde_json::json!("hello"),
                    provider: None,
                    model: None,
                    stop_reason: None,
                },
                metadata: None,
            },
        };
        let text = serde_json::to_string(&line).unwrap();
        assert!(text.contains("\"type\":\"response_item\""));
        let parsed: RolloutLine = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, line);
    }

    #[test]
    fn truncated_final_line_is_ignored() {
        let good = serde_json::to_string(&RolloutLine {
            timestamp: "2025-05-07T17:24:21.000Z".to_string(),
            ordinal: Some(0),
            item: RolloutItem::EventMsg {
                payload: EventMsgPayload::Warning {
                    message: "reconnecting".to_string(),
                },
            },
        })
        .unwrap();
        let text = format!(
            "{good}\n{{\"timestamp\":\"2025-05-07T17:24:22.000Z\",\"ordinal\":1,\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"assistant\",\"content\":\"par"
        );
        let lines = parse_rollout_text(&text);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn unknown_item_types_are_skipped() {
        let good = serde_json::to_string(&RolloutLine {
            timestamp: "2025-05-07T17:24:21.000Z".to_string(),
            ordinal: Some(0),
            item: RolloutItem::EventMsg {
                payload: EventMsgPayload::Warning {
                    message: "ok".to_string(),
                },
            },
        })
        .unwrap();
        let future = r#"{"timestamp":"2025-05-07T17:24:22.000Z","ordinal":1,"type":"future_thing","payload":{}}"#;
        let lines = parse_rollout_text(&format!("{good}\n{future}\n"));
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn portable_cwd_uses_token() {
        let dir = tempfile::tempdir().unwrap();
        let previous = std::env::var("STARK_PROJECTS_ROOT").ok();
        unsafe { std::env::set_var("STARK_PROJECTS_ROOT", dir.path()) };
        let workspace = dir.path().join("api");
        let cwd = portable_cwd(&workspace);
        unsafe {
            match previous {
                Some(value) => std::env::set_var("STARK_PROJECTS_ROOT", value),
                None => std::env::remove_var("STARK_PROJECTS_ROOT"),
            }
        }
        assert_eq!(cwd, "%PROJECTS_ROOT%/api");
    }
}
