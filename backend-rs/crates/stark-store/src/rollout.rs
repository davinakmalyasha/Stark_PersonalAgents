//! Append-only rollout log: the canonical trace of a chat.
//!
//! One JSONL file per chat, written as work happens. The file is the source
//! of truth; `chats.db` messages are a projection for lists and search.
//!
//! Line envelope matches the reference format:
//! `{"timestamp":"...","ordinal":N,"type":"...","payload":{...}}`
//! Readers tolerate a truncated final line (crash mid-write) and skip
//! unknown item types so newer writers stay forward-compatible.

use std::collections::HashSet;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;

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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        custom_header: Option<String>,
    },
    FunctionCallOutput {
        call_id: String,
        name: String,
        output: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TurnErrorDetails {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        root_turn_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        started_at: Option<i64>,
    },
    TurnComplete {
        turn_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_agent_message: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<TurnErrorDetails>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        started_at: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        completed_at: Option<i64>,
    },
    TurnAborted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<TurnErrorDetails>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        started_at: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        completed_at: Option<i64>,
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

/// A durable rollout line was appended. Durable log followers subscribe before
/// their replay read so an append committed during replay is not lost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolloutAppend {
    pub chat_id: String,
    pub ordinal: u64,
}

static APPENDS: OnceLock<broadcast::Sender<RolloutAppend>> = OnceLock::new();

fn append_sender() -> &'static broadcast::Sender<RolloutAppend> {
    APPENDS.get_or_init(|| broadcast::channel(4096).0)
}

pub fn subscribe_appends() -> broadcast::Receiver<RolloutAppend> {
    append_sender().subscribe()
}

/// Highest durable ordinal currently in a rollout file.
pub fn latest_ordinal(path: &Path) -> Option<u64> {
    read_rollout(path)
        .into_iter()
        .filter_map(|line| line.ordinal)
        .max()
}

static WRITER_LOCKS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

fn writer_locks() -> &'static Mutex<HashSet<PathBuf>> {
    WRITER_LOCKS.get_or_init(|| Mutex::new(HashSet::new()))
}

#[derive(Debug)]
struct WriterLock {
    file: Option<File>,
    path: PathBuf,
}

impl Drop for WriterLock {
    fn drop(&mut self) {
        self.file.take();
        release_writer_lock(&self.path);
    }
}

fn writer_lock_path(harness_root: &Path, chat_id: &str) -> PathBuf {
    harness_root
        .join(".deep_agents")
        .join("rollout-locks")
        .join(format!("{}.lock", sanitize_chat_id(chat_id)))
}

fn acquire_writer_lock(harness_root: &Path, chat_id: &str) -> Result<WriterLock, StoreError> {
    let path = writer_lock_path(harness_root, chat_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut local = writer_locks()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    if local.contains(&path) {
        drop(local);
        return Err(locked_error(chat_id, &path));
    }
    local.insert(path.clone());
    drop(local);

    let file = match OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) => {
            release_writer_lock(&path);
            return Err(error.into());
        }
    };
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            release_writer_lock(&path);
            return Err(locked_error(chat_id, &path));
        }
        Err(TryLockError::Error(error)) => {
            release_writer_lock(&path);
            return Err(error.into());
        }
    }
    // Keep diagnostics outside the locked file: Windows makes a locked byte
    // range unreadable through another handle.
    let _ = std::fs::write(
        owner_path(&path),
        format!(
            "chat={chat_id}\npid={}\nstarted={}\nexe={}\n",
            std::process::id(),
            Utc::now().to_rfc3339(),
            std::env::current_exe()
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_default()
        ),
    );
    Ok(WriterLock {
        file: Some(file),
        path,
    })
}

fn locked_error(chat_id: &str, lock_path: &Path) -> StoreError {
    let owner = std::fs::read_to_string(owner_path(lock_path))
        .unwrap_or_default()
        .lines()
        .take(4)
        .collect::<Vec<_>>()
        .join(", ");
    StoreError::Locked(format!(
        "rollout for chat '{chat_id}' is already being written by another Stark process ({owner})"
    ))
}

fn release_writer_lock(path: &Path) {
    if let Ok(mut locks) = writer_locks().lock() {
        locks.remove(path);
    }
}

fn owner_path(lock_path: &Path) -> PathBuf {
    lock_path.with_extension("owner")
}

/// Highest ordinal in the raw JSONL, including item types this binary does not
/// understand. `read_rollout` skips unknown types, so using its length would
/// reuse an ordinal and break `Last-Event-ID` replay.
fn next_ordinal(path: &Path) -> u64 {
    let Ok(text) = std::fs::read_to_string(path) else {
        return 0;
    };
    let max = text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|line| line.get("ordinal").and_then(Value::as_u64))
        .max();
    max.map(|value| value + 1)
        .unwrap_or_else(|| text.lines().filter(|line| !line.trim().is_empty()).count() as u64)
}

/// Repairs a crash-truncated final line before appending.
///
/// A complete line ends with `}` after optional whitespace (the same rule the
/// reader uses). If the trailing bytes do not, truncate back to the previous
/// newline so the next append starts a fresh JSON object. If they do, add the
/// missing newline. Unknown future item types are preserved: completeness is a
/// structural check, not a deserialize check.
fn repair_rollout_tail(path: &Path) -> Result<(), StoreError> {
    let bytes = std::fs::read(path)?;
    if bytes.is_empty() {
        return Ok(());
    }
    let tail_start = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    if tail_start == bytes.len() {
        return Ok(());
    }
    let tail = &bytes[tail_start..];
    let complete = tail
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .is_some_and(|index| tail[index] == b'}');
    if complete {
        let mut file = OpenOptions::new().append(true).open(path)?;
        file.write_all(b"\n")?;
        file.flush()?;
    } else {
        let file = OpenOptions::new().write(true).open(path)?;
        file.set_len(tail_start as u64)?;
        file.sync_all()?;
    }
    Ok(())
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

/// Returns the newest started turn that has no matching complete/abort.
/// `None` means the log is settled (or has no turns at all). The recorded
/// file is never mutated; this is derived on read.
pub fn unfinished_turn(lines: &[RolloutLine]) -> Option<String> {
    let mut started: Vec<String> = Vec::new();
    let mut finished: std::collections::HashSet<String> = std::collections::HashSet::new();
    for line in lines {
        if let RolloutItem::EventMsg { payload } = &line.item {
            match payload {
                EventMsgPayload::TurnStarted { turn_id, .. } => {
                    if !finished.contains(turn_id) {
                        started.push(turn_id.clone());
                    }
                }
                EventMsgPayload::TurnComplete { turn_id, .. } => {
                    finished.insert(turn_id.clone());
                }
                EventMsgPayload::TurnAborted {
                    turn_id, reason, ..
                } => {
                    if let Some(turn_id) = turn_id {
                        finished.insert(turn_id.clone());
                    } else {
                        // Abort without a turn id settles the newest open turn.
                        let _ = reason;
                        if let Some(open) = started.iter().rev().find(|id| !finished.contains(*id))
                        {
                            finished.insert(open.clone());
                        }
                    }
                }
                _ => {}
            }
        }
    }
    started.into_iter().rev().find(|id| !finished.contains(id))
}

/// Convenience for open/resume paths: settled when there is no unfinished turn.
pub fn is_settled(lines: &[RolloutLine]) -> bool {
    unfinished_turn(lines).is_none()
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
///
/// The writer holds an advisory lock for the chat's lifetime. The harness lock
/// already prevents two servers from owning the same root; this second lock
/// also protects direct CLI/test writers and makes ordinal allocation safe if
/// the ownership model ever changes. Locks are released by the OS if a process
/// dies, so a crash cannot leave a permanent stale lock.
#[derive(Debug)]
pub struct RolloutWriter {
    path: PathBuf,
    chat_id: String,
    next_ordinal: u64,
    _writer_lock: WriterLock,
}

impl RolloutWriter {
    pub async fn open(
        harness_root: &Path,
        chat_id: &str,
        workspace_id: &str,
        workspace: &Path,
        model: &str,
    ) -> Result<Self, StoreError> {
        crate::validate_chat_id(chat_id)?;
        let writer_lock = acquire_writer_lock(harness_root, chat_id)?;
        if let Some(existing) = find_rollout(harness_root, chat_id) {
            repair_rollout_tail(&existing)?;
            return Ok(Self {
                next_ordinal: next_ordinal(&existing),
                path: existing,
                chat_id: chat_id.to_string(),
                _writer_lock: writer_lock,
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
            chat_id: chat_id.to_string(),
            next_ordinal: 0,
            _writer_lock: writer_lock,
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
        let _ = append_sender().send(RolloutAppend {
            chat_id: self.chat_id.clone(),
            ordinal,
        });
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

    #[tokio::test]
    async fn appends_are_broadcast_and_latest_ordinal_tracks_the_file() {
        use tokio::sync::broadcast::error::TryRecvError;

        let dir = tempfile::tempdir().unwrap();
        let mut receiver = subscribe_appends();
        let mut writer = RolloutWriter::open(dir.path(), "chat-notify", "ws", dir.path(), "model")
            .await
            .unwrap();
        let ordinal = writer
            .append(RolloutItem::EventMsg {
                payload: EventMsgPayload::Warning {
                    message: "notify".to_string(),
                },
            })
            .await
            .unwrap();
        let mut notified = false;
        for _ in 0..50 {
            match receiver.try_recv() {
                Ok(append) if append.chat_id == "chat-notify" && append.ordinal == ordinal => {
                    notified = true;
                    break;
                }
                Ok(_) => {}
                Err(TryRecvError::Empty) => {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                Err(TryRecvError::Closed) => break,
                Err(TryRecvError::Lagged(_)) => {}
            }
        }
        assert!(notified);
        assert_eq!(latest_ordinal(writer.path()), Some(ordinal));
    }

    #[tokio::test]
    async fn writer_lock_rejects_a_second_writer() {
        let dir = tempfile::tempdir().unwrap();
        let first = RolloutWriter::open(dir.path(), "chat-lock", "ws", dir.path(), "model")
            .await
            .unwrap();
        let error = RolloutWriter::open(dir.path(), "chat-lock", "ws", dir.path(), "model")
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("already being written"),
            "{error}"
        );
        assert!(error.to_string().contains("pid="), "{error}");
        drop(first);
        RolloutWriter::open(dir.path(), "chat-lock", "ws", dir.path(), "model")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn writer_repairs_a_truncated_tail() {
        use std::io::Write as _;

        let dir = tempfile::tempdir().unwrap();
        let mut first = RolloutWriter::open(dir.path(), "chat-tail", "ws", dir.path(), "model")
            .await
            .unwrap();
        first
            .append(RolloutItem::EventMsg {
                payload: EventMsgPayload::Warning {
                    message: "first".to_string(),
                },
            })
            .await
            .unwrap();
        let path = first.path().to_path_buf();
        drop(first);

        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(
            br#"{"timestamp":"t","ordinal":1,"type":"event_msg","payload":{"type":"warning","message":"par"#,
        )
        .unwrap();
        drop(file);

        let mut second = RolloutWriter::open(dir.path(), "chat-tail", "ws", dir.path(), "model")
            .await
            .unwrap();
        let next = second
            .append(RolloutItem::EventMsg {
                payload: EventMsgPayload::Warning {
                    message: "recovered".to_string(),
                },
            })
            .await
            .unwrap();
        assert_eq!(next, 2);
        let lines = read_rollout(&path);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].ordinal, Some(0));
        assert_eq!(lines[1].ordinal, Some(1));
        assert_eq!(lines[2].ordinal, Some(2));
    }

    #[tokio::test]
    async fn writer_continues_after_an_unknown_item_type() {
        use std::io::Write as _;

        let dir = tempfile::tempdir().unwrap();
        let first = RolloutWriter::open(dir.path(), "chat-future", "ws", dir.path(), "model")
            .await
            .unwrap();
        let path = first.path().to_path_buf();
        drop(first);
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(br#"{"timestamp":"t","ordinal":7,"type":"future_thing","payload":{}}"#)
            .unwrap();
        file.write_all(b"\n").unwrap();
        drop(file);

        let mut second = RolloutWriter::open(dir.path(), "chat-future", "ws", dir.path(), "model")
            .await
            .unwrap();
        let next = second
            .append(RolloutItem::EventMsg {
                payload: EventMsgPayload::Warning {
                    message: "after future".to_string(),
                },
            })
            .await
            .unwrap();
        assert_eq!(next, 8);
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

    fn started(turn: &str) -> RolloutLine {
        RolloutLine {
            timestamp: "t".to_string(),
            ordinal: Some(0),
            item: RolloutItem::EventMsg {
                payload: EventMsgPayload::TurnStarted {
                    turn_id: turn.to_string(),
                    root_turn_id: None,
                    started_at: None,
                },
            },
        }
    }

    fn completed(turn: &str) -> RolloutLine {
        RolloutLine {
            timestamp: "t".to_string(),
            ordinal: Some(1),
            item: RolloutItem::EventMsg {
                payload: EventMsgPayload::TurnComplete {
                    turn_id: turn.to_string(),
                    last_agent_message: None,
                    error: None,
                    started_at: None,
                    completed_at: None,
                },
            },
        }
    }

    #[test]
    fn unfinished_turn_tracks_started_without_complete() {
        assert_eq!(unfinished_turn(&[]), None);
        assert_eq!(unfinished_turn(&[started("a")]), Some("a".to_string()));
        assert_eq!(unfinished_turn(&[started("a"), completed("a")]), None);
        assert_eq!(
            unfinished_turn(&[started("a"), started("b")]),
            Some("b".to_string())
        );
    }

    #[test]
    fn aborted_turn_counts_as_settled() {
        let lines = vec![
            started("a"),
            RolloutLine {
                timestamp: "t".to_string(),
                ordinal: Some(1),
                item: RolloutItem::EventMsg {
                    payload: EventMsgPayload::TurnAborted {
                        turn_id: Some("a".to_string()),
                        reason: "cancelled".to_string(),
                        error: None,
                        started_at: None,
                        completed_at: None,
                    },
                },
            },
        ];
        assert_eq!(unfinished_turn(&lines), None);
        assert!(is_settled(&lines));
    }

    #[test]
    fn turn_abort_wire_round_trip() {
        let line = RolloutLine {
            timestamp: "2025-05-07T17:24:21.000Z".to_string(),
            ordinal: Some(5),
            item: RolloutItem::EventMsg {
                payload: EventMsgPayload::TurnAborted {
                    turn_id: Some("turn-abc".to_string()),
                    reason: "cancelled".to_string(),
                    error: None,
                    started_at: Some(1),
                    completed_at: Some(2),
                },
            },
        };
        let text = serde_json::to_string(&line).unwrap();
        assert!(text.contains("\"type\":\"turn_aborted\""));
        let parsed: RolloutLine = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, line);
    }

    #[test]
    fn turn_complete_carries_error() {
        let line = RolloutLine {
            timestamp: "2025-05-07T17:24:21.000Z".to_string(),
            ordinal: Some(6),
            item: RolloutItem::EventMsg {
                payload: EventMsgPayload::TurnComplete {
                    turn_id: "turn-abc".to_string(),
                    last_agent_message: Some("partial".to_string()),
                    error: Some(TurnErrorDetails {
                        message: "boom".to_string(),
                        code: Some("timeout".to_string()),
                    }),
                    started_at: None,
                    completed_at: None,
                },
            },
        };
        let text = serde_json::to_string(&line).unwrap();
        let parsed: RolloutLine = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, line);
    }
}
