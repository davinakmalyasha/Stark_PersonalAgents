//! Codex/Claude-style workspace memory.
//!
//! Completed turns produce deterministic stage-1 records. A bounded,
//! model-assisted consolidation pass turns those records into durable entries,
//! a compact `MEMORY.md` index, topic files, and a long-form `summary.md`.
//!
//! The agent sees only the compact index by default and reads topic files on
//! demand through the `memory` tool. This is the same split Codex and Claude
//! Code use: a small always-on summary plus an on-demand long-form store.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use regex::Regex;
use serde_json::{Value, json};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::StoreError;

const DEFAULT_SUMMARY_CHARS: usize = 8_000;
const MAX_RECORD_CHARS: usize = 12_000;
const MAX_ENTRY_CHARS: usize = 800;
const MAX_TOPIC_FILE_CHARS: usize = 64_000;
const MAX_INDEX_LINES: usize = 120;

pub const TOPICS: &[&str] = &[
    "decisions",
    "facts",
    "commands",
    "pitfalls",
    "preferences",
    "tasks",
    "recent",
    "notes",
];

pub const KINDS: &[&str] = &[
    "decision",
    "fact",
    "command",
    "pitfall",
    "preference",
    "task",
    "note",
];

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn safe_id(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "default".to_string()
    } else {
        cleaned
    }
}

fn truncate_chars(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        value.to_string()
    } else {
        value.chars().take(max).collect()
    }
}

fn normalize_topic(value: &str) -> String {
    let value = value.trim().to_ascii_lowercase();
    if TOPICS.contains(&value.as_str()) {
        return value;
    }
    match value.as_str() {
        "architecture" | "design" | "designs" => "decisions".to_string(),
        "command" | "commands" | "build" | "test" | "tests" => "commands".to_string(),
        "bug" | "bugs" | "pitfall" | "pitfalls" | "issue" | "issues" => "pitfalls".to_string(),
        "preference" | "preferences" | "style" | "convention" | "conventions" => {
            "preferences".to_string()
        }
        "task" | "tasks" | "work" | "recent" => "tasks".to_string(),
        _ => "facts".to_string(),
    }
}

fn normalize_kind(value: &str) -> String {
    let value = value.trim().to_ascii_lowercase();
    if KINDS.contains(&value.as_str()) {
        return value;
    }
    match value.as_str() {
        "architecture" | "design" => "decision".to_string(),
        "command" | "build" | "test" => "command".to_string(),
        "bug" | "issue" => "pitfall".to_string(),
        "preference" | "style" | "convention" => "preference".to_string(),
        "task" | "work" => "task".to_string(),
        _ => "fact".to_string(),
    }
}

fn redact_patterns() -> &'static Vec<Regex> {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            r"(?i)(api[_-]?key|token|password|secret)\s*[:=]\s*[^\s,;]+",
            r"sk-[A-Za-z0-9_-]{16,}",
            r"ghp_[A-Za-z0-9]{16,}",
            r"(?i)bearer\s+[A-Za-z0-9._-]{12,}",
        ]
        .into_iter()
        .filter_map(|pattern| Regex::new(pattern).ok())
        .collect()
    })
}

pub fn redact_secrets(value: &str) -> String {
    let mut text = value.to_string();
    for pattern in redact_patterns() {
        text = pattern
            .replace_all(&text, |caps: &regex::Captures| {
                if let Some(name) = caps.get(1) {
                    format!("{} = [REDACTED]", name.as_str())
                } else {
                    "[REDACTED]".to_string()
                }
            })
            .to_string();
    }
    text
}

#[derive(Clone, Debug)]
pub struct Stage1Record {
    pub id: String,
    pub chat_id: String,
    pub agent_id: Option<String>,
    pub request: String,
    pub summary: String,
    pub raw_memory: String,
    pub created_at: String,
}

#[derive(Clone, Debug)]
pub struct MemoryEntry {
    pub id: String,
    pub workspace_id: String,
    pub chat_id: String,
    pub agent_id: Option<String>,
    pub kind: String,
    pub topic: String,
    pub text: String,
    pub source: String,
    pub model: String,
    pub pinned: bool,
    pub usage_count: i64,
    pub last_used: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl MemoryEntry {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "workspaceId": self.workspace_id,
            "chatId": self.chat_id,
            "agentId": self.agent_id,
            "kind": self.kind,
            "topic": self.topic,
            "text": self.text,
            "source": self.source,
            "model": self.model,
            "pinned": self.pinned,
            "usageCount": self.usage_count,
            "lastUsed": self.last_used,
            "createdAt": self.created_at,
            "updatedAt": self.updated_at,
        })
    }
}

#[derive(Clone, Debug)]
pub struct NewEntry {
    pub chat_id: String,
    pub agent_id: Option<String>,
    pub kind: String,
    pub topic: String,
    pub text: String,
    pub source: String,
    pub model: String,
    pub pinned: bool,
}

fn row_stage1(row: &sqlx::sqlite::SqliteRow) -> Stage1Record {
    Stage1Record {
        id: row.get("id"),
        chat_id: row.get("chat_id"),
        agent_id: row.try_get("agent_id").ok().flatten(),
        request: row.get("request"),
        summary: row.get("summary"),
        raw_memory: row.get("raw_memory"),
        created_at: row.get("created_at"),
    }
}

fn row_entry(row: &sqlx::sqlite::SqliteRow) -> MemoryEntry {
    MemoryEntry {
        id: row.get("id"),
        workspace_id: row.get("workspace_id"),
        chat_id: row.try_get("chat_id").unwrap_or_default(),
        agent_id: row.try_get("agent_id").ok().flatten(),
        kind: row.get("kind"),
        topic: row.get("topic"),
        text: row.get("text"),
        source: row.try_get("source").unwrap_or_default(),
        model: row.try_get("model").unwrap_or_default(),
        pinned: row.try_get::<i64, _>("pinned").unwrap_or(0) != 0,
        usage_count: row.try_get("usage_count").unwrap_or(0),
        last_used: row.try_get("last_used").ok().flatten(),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

#[derive(Clone)]
pub struct MemoryStore {
    pool: SqlitePool,
    root: PathBuf,
}

impl MemoryStore {
    pub async fn open(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        let db_path = root.join(".deep_agents").join("chats.db");
        if let Some(parent) = db_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let options = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(3)
            .connect_with(options)
            .await?;
        let store = Self { pool, root };
        store.init().await?;
        Ok(store)
    }

    async fn init(&self) -> Result<(), StoreError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS memory_stage1 (
                id TEXT PRIMARY KEY,
                workspace_id TEXT NOT NULL,
                chat_id TEXT NOT NULL,
                agent_id TEXT,
                request TEXT NOT NULL,
                summary TEXT NOT NULL,
                raw_memory TEXT NOT NULL,
                created_at TEXT NOT NULL,
                usage_count INTEGER NOT NULL DEFAULT 0,
                last_usage TEXT,
                selected_for_phase2 INTEGER NOT NULL DEFAULT 1,
                consolidated_at TEXT,
                source TEXT NOT NULL DEFAULT 'turn'
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        let _ = sqlx::query("ALTER TABLE memory_stage1 ADD COLUMN source TEXT DEFAULT 'turn'")
            .execute(&self.pool)
            .await;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS ix_memory_stage1_workspace ON memory_stage1(workspace_id, created_at DESC)",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS memory_entries (
                id TEXT PRIMARY KEY,
                workspace_id TEXT NOT NULL,
                chat_id TEXT NOT NULL DEFAULT '',
                agent_id TEXT,
                kind TEXT NOT NULL,
                topic TEXT NOT NULL,
                text TEXT NOT NULL,
                source_stage1_id TEXT,
                source TEXT NOT NULL DEFAULT 'model',
                model TEXT NOT NULL DEFAULT '',
                pinned INTEGER NOT NULL DEFAULT 0,
                usage_count INTEGER NOT NULL DEFAULT 0,
                last_used TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS ix_memory_entries_workspace ON memory_entries(workspace_id, topic, created_at DESC)",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "CREATE UNIQUE INDEX IF NOT EXISTS ix_memory_entries_unique ON memory_entries(workspace_id, topic, text)",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS memory_summary (
                workspace_id TEXT PRIMARY KEY,
                summary TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                consolidated_count INTEGER NOT NULL DEFAULT 0,
                source TEXT NOT NULL DEFAULT 'deterministic',
                model TEXT NOT NULL DEFAULT '',
                usage_json TEXT NOT NULL DEFAULT '{}'
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        let _ = sqlx::query(
            "ALTER TABLE memory_summary ADD COLUMN source TEXT DEFAULT 'deterministic'",
        )
        .execute(&self.pool)
        .await;
        let _ = sqlx::query("ALTER TABLE memory_summary ADD COLUMN model TEXT DEFAULT ''")
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("ALTER TABLE memory_summary ADD COLUMN usage_json TEXT DEFAULT '{}'")
            .execute(&self.pool)
            .await;
        Ok(())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn memory_dir(&self, workspace_id: &str) -> PathBuf {
        self.root
            .join(".deep_agents")
            .join("memory")
            .join(safe_id(workspace_id))
    }

    fn topics_dir(&self, workspace_id: &str) -> PathBuf {
        self.memory_dir(workspace_id).join("topics")
    }

    /// Record one completed turn. This is deterministic and must stay cheap:
    /// it runs synchronously at the end of every eligible turn.
    pub async fn record(
        &self,
        workspace_id: &str,
        chat_id: &str,
        agent_id: Option<&str>,
        request: &str,
        report: &str,
    ) -> Result<String, StoreError> {
        if report.trim().is_empty() {
            return Ok(String::new());
        }
        let request = redact_secrets(request);
        let raw: String = truncate_chars(&redact_secrets(report), MAX_RECORD_CHARS);
        let summary = format!(
            "Request: {}\nOutcome: {}",
            truncate_chars(&request, 500),
            truncate_chars(&raw, 2_000)
        );
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            r#"
            INSERT INTO memory_stage1
                (id, workspace_id, chat_id, agent_id, request, summary, raw_memory,
                 created_at, selected_for_phase2, source)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, 'turn')
            "#,
        )
        .bind(&id)
        .bind(workspace_id)
        .bind(chat_id)
        .bind(agent_id)
        .bind(&request)
        .bind(&summary)
        .bind(&raw)
        .bind(now())
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    pub async fn pending_stage1(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<Stage1Record>, StoreError> {
        let rows = sqlx::query(
            r#"
            SELECT id, chat_id, agent_id, request, summary, raw_memory, created_at
            FROM memory_stage1
            WHERE workspace_id = ? AND consolidated_at IS NULL
            ORDER BY created_at ASC
            LIMIT ?
            "#,
        )
        .bind(workspace_id)
        .bind(limit.max(1) as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(row_stage1).collect())
    }

    pub async fn pending_count(&self, workspace_id: &str) -> Result<i64, StoreError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM memory_stage1 WHERE workspace_id = ? AND consolidated_at IS NULL",
        )
        .bind(workspace_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(count)
    }

    /// Store the result of a model or deterministic consolidation.
    #[allow(clippy::too_many_arguments)]
    pub async fn apply_consolidation(
        &self,
        workspace_id: &str,
        summary: &str,
        entries: &[NewEntry],
        consumed_ids: &[String],
        source: &str,
        model: &str,
        usage: Value,
    ) -> Result<(), StoreError> {
        let moment = now();
        let summary = truncate_chars(&redact_secrets(summary), max_chars());
        let mut tx = self.pool.begin().await?;
        for entry in entries {
            let topic = normalize_topic(&entry.topic);
            let kind = normalize_kind(&entry.kind);
            let text = truncate_chars(&redact_secrets(&entry.text), MAX_ENTRY_CHARS);
            if text.trim().is_empty() {
                continue;
            }
            sqlx::query(
                r#"
                INSERT INTO memory_entries
                    (id, workspace_id, chat_id, agent_id, kind, topic, text,
                     source, model, pinned, created_at, updated_at)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT(workspace_id, topic, text) DO UPDATE SET
                    pinned = MAX(memory_entries.pinned, excluded.pinned),
                    updated_at = excluded.updated_at
                "#,
            )
            .bind(Uuid::new_v4().to_string())
            .bind(workspace_id)
            .bind(&entry.chat_id)
            .bind(&entry.agent_id)
            .bind(&kind)
            .bind(&topic)
            .bind(&text)
            .bind(&entry.source)
            .bind(&entry.model)
            .bind(i64::from(entry.pinned))
            .bind(&moment)
            .bind(&moment)
            .execute(&mut *tx)
            .await?;
        }
        for id in consumed_ids {
            sqlx::query(
                "UPDATE memory_stage1 SET consolidated_at = ?, selected_for_phase2 = 1 WHERE id = ?",
            )
            .bind(&moment)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            r#"
            INSERT INTO memory_summary
                (workspace_id, summary, updated_at, consolidated_count, source, model, usage_json)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(workspace_id) DO UPDATE SET
                summary = excluded.summary,
                updated_at = excluded.updated_at,
                consolidated_count = excluded.consolidated_count,
                source = excluded.source,
                model = excluded.model,
                usage_json = excluded.usage_json
            "#,
        )
        .bind(workspace_id)
        .bind(&summary)
        .bind(&moment)
        .bind(consumed_ids.len() as i64)
        .bind(source)
        .bind(model)
        .bind(usage.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.rebuild_files(workspace_id).await?;
        Ok(())
    }

    /// Deterministic fallback: turn pending stage-1 records into recent-work
    /// entries and a compact summary. No model call.
    pub async fn consolidate(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<String, StoreError> {
        let records = self.pending_stage1(workspace_id, limit.max(1)).await?;
        if records.is_empty() {
            return Ok(self
                .summary(workspace_id, max_chars())
                .await
                .unwrap_or_default());
        }
        let mut entries = Vec::with_capacity(records.len());
        let mut lines = vec!["## Recent work".to_string()];
        let mut consumed = Vec::with_capacity(records.len());
        for record in records {
            let outcome = truncate_chars(
                record.summary.lines().last().unwrap_or(&record.summary),
                500,
            );
            let text = format!(
                "{} → {}",
                truncate_chars(&record.request.replace('\n', " "), 220),
                truncate_chars(&outcome, 400)
            );
            lines.push(format!("- {text}"));
            entries.push(NewEntry {
                chat_id: record.chat_id.clone(),
                agent_id: record.agent_id.clone(),
                kind: "task".to_string(),
                topic: "recent".to_string(),
                text,
                source: "deterministic".to_string(),
                model: String::new(),
                pinned: false,
            });
            consumed.push(record.id);
        }
        let summary = truncate_chars(&lines.join("\n"), max_chars());
        self.apply_consolidation(
            workspace_id,
            &summary,
            &entries,
            &consumed,
            "deterministic",
            "",
            json!({}),
        )
        .await?;
        Ok(summary)
    }

    pub async fn add_entry(
        &self,
        workspace_id: &str,
        entry: NewEntry,
    ) -> Result<String, StoreError> {
        let moment = now();
        let topic = normalize_topic(&entry.topic);
        let kind = normalize_kind(&entry.kind);
        let text = truncate_chars(&redact_secrets(&entry.text), MAX_ENTRY_CHARS);
        if text.trim().is_empty() {
            return Err(StoreError::Invalid("memory text is empty".to_string()));
        }
        sqlx::query(
            r#"
            INSERT INTO memory_entries
                (id, workspace_id, chat_id, agent_id, kind, topic, text,
                 source, model, pinned, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(workspace_id, topic, text) DO UPDATE SET
                pinned = MAX(memory_entries.pinned, excluded.pinned),
                updated_at = excluded.updated_at
            "#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(workspace_id)
        .bind(&entry.chat_id)
        .bind(&entry.agent_id)
        .bind(&kind)
        .bind(&topic)
        .bind(&text)
        .bind(&entry.source)
        .bind(&entry.model)
        .bind(i64::from(entry.pinned))
        .bind(&moment)
        .bind(&moment)
        .execute(&self.pool)
        .await?;
        let id: String = sqlx::query_scalar(
            "SELECT id FROM memory_entries WHERE workspace_id = ? AND topic = ? AND text = ?",
        )
        .bind(workspace_id)
        .bind(&topic)
        .bind(&text)
        .fetch_one(&self.pool)
        .await?;
        self.rebuild_files(workspace_id).await?;
        Ok(id)
    }

    pub async fn entries(
        &self,
        workspace_id: &str,
        topic: Option<&str>,
    ) -> Result<Vec<MemoryEntry>, StoreError> {
        let rows = if let Some(topic) = topic {
            sqlx::query(
                "SELECT * FROM memory_entries WHERE workspace_id = ? AND topic = ? ORDER BY pinned DESC, created_at DESC",
            )
            .bind(workspace_id)
            .bind(normalize_topic(topic))
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT * FROM memory_entries WHERE workspace_id = ? ORDER BY pinned DESC, created_at DESC",
            )
            .bind(workspace_id)
            .fetch_all(&self.pool)
            .await?
        };
        Ok(rows.iter().map(row_entry).collect())
    }

    pub async fn search(
        &self,
        workspace_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, StoreError> {
        let like = format!("%{}%", query.trim());
        let rows = sqlx::query(
            r#"
            SELECT * FROM memory_entries
            WHERE workspace_id = ? AND (text LIKE ? OR topic LIKE ? OR kind LIKE ?)
            ORDER BY pinned DESC, created_at DESC
            LIMIT ?
            "#,
        )
        .bind(workspace_id)
        .bind(&like)
        .bind(&like)
        .bind(&like)
        .bind(limit.clamp(1, 50) as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(row_entry).collect())
    }

    pub async fn forget_entry(
        &self,
        workspace_id: &str,
        entry_id: &str,
    ) -> Result<bool, StoreError> {
        let result = sqlx::query("DELETE FROM memory_entries WHERE workspace_id = ? AND id = ?")
            .bind(workspace_id)
            .bind(entry_id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() > 0 {
            self.rebuild_files(workspace_id).await?;
        }
        Ok(result.rows_affected() > 0)
    }

    pub async fn pin_entry(
        &self,
        workspace_id: &str,
        entry_id: &str,
        pinned: bool,
    ) -> Result<bool, StoreError> {
        let result = sqlx::query(
            "UPDATE memory_entries SET pinned = ?, updated_at = ? WHERE workspace_id = ? AND id = ?",
        )
        .bind(i64::from(pinned))
        .bind(now())
        .bind(workspace_id)
        .bind(entry_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() > 0 {
            self.rebuild_files(workspace_id).await?;
        }
        Ok(result.rows_affected() > 0)
    }

    pub async fn mark_topic_used(&self, workspace_id: &str, topic: &str) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE memory_entries SET usage_count = usage_count + 1, last_used = ? WHERE workspace_id = ? AND topic = ?",
        )
        .bind(now())
        .bind(workspace_id)
        .bind(normalize_topic(topic))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn summary(&self, workspace_id: &str, max_chars: usize) -> Option<String> {
        let row = sqlx::query(
            "SELECT summary FROM memory_summary WHERE workspace_id = ? AND summary != ''",
        )
        .bind(workspace_id)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()?;
        let summary: String = row.get("summary");
        let summary = summary.trim();
        if summary.is_empty() {
            None
        } else {
            Some(truncate_chars(summary, max_chars.max(1)))
        }
    }

    pub async fn save_summary(&self, workspace_id: &str, summary: &str) -> Result<(), StoreError> {
        let summary = truncate_chars(&redact_secrets(summary), max_chars());
        let moment = now();
        sqlx::query(
            r#"
            INSERT INTO memory_summary
                (workspace_id, summary, updated_at, consolidated_count, source, model, usage_json)
            VALUES (?, ?, ?, 0, 'manual', '', '{}')
            ON CONFLICT(workspace_id) DO UPDATE SET
                summary = excluded.summary,
                updated_at = excluded.updated_at,
                source = 'manual',
                model = ''
            "#,
        )
        .bind(workspace_id)
        .bind(&summary)
        .bind(&moment)
        .execute(&self.pool)
        .await?;
        let dir = self.memory_dir(workspace_id);
        tokio::fs::create_dir_all(&dir).await?;
        tokio::fs::write(dir.join("summary.md"), format!("{summary}\n")).await?;
        self.rebuild_files(workspace_id).await?;
        Ok(())
    }

    pub async fn index(&self, workspace_id: &str, max_chars: usize) -> Option<String> {
        let path = self.memory_dir(workspace_id).join("MEMORY.md");
        if let Ok(text) = tokio::fs::read_to_string(&path).await {
            let text = text.trim();
            if !text.is_empty() {
                return Some(truncate_chars(text, max_chars.max(1)));
            }
        }
        // Compatibility with the old single-summary store. The next
        // consolidation writes the new index/topic layout.
        let summary = self.summary(workspace_id, max_chars).await?;
        Some(format!(
            "# Workspace Memory\n\n{summary}\n\nUse the memory tool to read details."
        ))
    }

    pub async fn topics(&self, workspace_id: &str) -> Result<Vec<Value>, StoreError> {
        let entries = self.entries(workspace_id, None).await?;
        let mut counts: BTreeMap<String, (usize, i64)> = BTreeMap::new();
        for entry in entries {
            let slot = counts.entry(entry.topic.clone()).or_insert((0, 0));
            slot.0 += 1;
            slot.1 += entry.usage_count;
        }
        let mut topics = Vec::new();
        for (topic, (count, usage)) in counts {
            let path = self.topics_dir(workspace_id).join(format!("{topic}.md"));
            let chars = tokio::fs::read_to_string(&path)
                .await
                .map(|text| text.chars().count())
                .unwrap_or(0);
            topics.push(json!({
                "name": topic,
                "entries": count,
                "usageCount": usage,
                "chars": chars,
            }));
        }
        Ok(topics)
    }

    pub async fn read_topic(
        &self,
        workspace_id: &str,
        topic: &str,
    ) -> Result<Option<String>, StoreError> {
        let topic = normalize_topic(topic);
        let path = self.topics_dir(workspace_id).join(format!("{topic}.md"));
        match tokio::fs::read_to_string(path).await {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub async fn status(&self, workspace_id: &str) -> Result<Value, StoreError> {
        let stage1: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM memory_stage1 WHERE workspace_id = ? AND selected_for_phase2 = 1",
        )
        .bind(workspace_id)
        .fetch_one(&self.pool)
        .await?;
        let pending: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM memory_stage1 WHERE workspace_id = ? AND consolidated_at IS NULL",
        )
        .bind(workspace_id)
        .fetch_one(&self.pool)
        .await?;
        let entries: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM memory_entries WHERE workspace_id = ?")
                .bind(workspace_id)
                .fetch_one(&self.pool)
                .await?;
        let summary: Option<String> =
            sqlx::query_scalar("SELECT summary FROM memory_summary WHERE workspace_id = ?")
                .bind(workspace_id)
                .fetch_optional(&self.pool)
                .await?;
        let meta = sqlx::query(
            "SELECT updated_at, source, model FROM memory_summary WHERE workspace_id = ?",
        )
        .bind(workspace_id)
        .fetch_optional(&self.pool)
        .await?;
        let index_chars =
            tokio::fs::read_to_string(self.memory_dir(workspace_id).join("MEMORY.md"))
                .await
                .map(|text| text.chars().count())
                .unwrap_or(0);
        let topic_count = self.topics(workspace_id).await?.len();
        Ok(json!({
            "workspaceId": workspace_id,
            "stage1Count": stage1,
            "pendingCount": pending,
            "entryCount": entries,
            "topicCount": topic_count,
            "hasSummary": summary.as_deref().is_some_and(|value| !value.trim().is_empty()),
            "summaryChars": summary.map(|value| value.chars().count()).unwrap_or(0),
            "indexChars": index_chars,
            "updatedAt": meta.as_ref().map(|row| row.get::<String, _>("updated_at")).unwrap_or_default(),
            "source": meta.as_ref().map(|row| row.get::<String, _>("source")).unwrap_or_default(),
            "model": meta.as_ref().map(|row| row.get::<String, _>("model")).unwrap_or_default(),
            "maxChars": max_chars(),
        }))
    }

    pub async fn records(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<Value>, StoreError> {
        let rows = sqlx::query(
            r#"
            SELECT id, chat_id, agent_id, request, summary, raw_memory, created_at, consolidated_at
            FROM memory_stage1 WHERE workspace_id = ?
            ORDER BY created_at DESC LIMIT ?
            "#,
        )
        .bind(workspace_id)
        .bind(limit.clamp(1, 200) as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|row| {
                json!({
                    "id": row.get::<String, _>("id"),
                    "chatId": row.get::<String, _>("chat_id"),
                    "agentId": row.try_get::<Option<String>, _>("agent_id").ok().flatten(),
                    "request": row.get::<String, _>("request"),
                    "summary": row.get::<String, _>("summary"),
                    "rawMemory": truncate_chars(&row.get::<String, _>("raw_memory"), 2_000),
                    "createdAt": row.get::<String, _>("created_at"),
                    "consolidatedAt": row.try_get::<Option<String>, _>("consolidated_at").ok().flatten(),
                })
            })
            .collect())
    }

    /// Rebuild `MEMORY.md`, `summary.md`, and topic files from entries.
    pub async fn rebuild_files(&self, workspace_id: &str) -> Result<(), StoreError> {
        let entries = self.entries(workspace_id, None).await?;
        let summary = self
            .summary(workspace_id, 100_000)
            .await
            .unwrap_or_default();
        let dir = self.memory_dir(workspace_id);
        let topics_dir = self.topics_dir(workspace_id);
        if entries.is_empty() && summary.trim().is_empty() {
            let _ = tokio::fs::remove_file(dir.join("MEMORY.md")).await;
            let _ = tokio::fs::remove_file(dir.join("summary.md")).await;
            let _ = tokio::fs::remove_dir_all(&topics_dir).await;
            return Ok(());
        }
        tokio::fs::create_dir_all(&topics_dir).await?;

        let mut groups: BTreeMap<String, Vec<&MemoryEntry>> = BTreeMap::new();
        for entry in &entries {
            groups.entry(entry.topic.clone()).or_default().push(entry);
        }
        let keep: HashSet<String> = groups.keys().map(|topic| format!("{topic}.md")).collect();
        for (topic, mut items) in groups {
            items.sort_by(|left, right| {
                right
                    .pinned
                    .cmp(&left.pinned)
                    .then_with(|| right.created_at.cmp(&left.created_at))
            });
            let mut text = format!(
                "# {}\n\n_Workspace memory topic · {} entries_\n\n",
                topic_label(&topic),
                items.len()
            );
            for entry in items {
                text.push_str(&format!(
                    "- [{}] {} _(source: {}, {})_\n",
                    entry.kind,
                    entry.text,
                    if entry.chat_id.is_empty() {
                        entry.source.as_str()
                    } else {
                        entry.chat_id.as_str()
                    },
                    entry.created_at.get(..10).unwrap_or(&entry.created_at)
                ));
                if text.chars().count() >= MAX_TOPIC_FILE_CHARS {
                    break;
                }
            }
            tokio::fs::write(topics_dir.join(format!("{topic}.md")), text).await?;
        }
        if let Ok(mut dir) = tokio::fs::read_dir(&topics_dir).await {
            while let Ok(Some(item)) = dir.next_entry().await {
                let name = item.file_name().to_string_lossy().to_string();
                if name.ends_with(".md") && !keep.contains(&name) {
                    let _ = tokio::fs::remove_file(item.path()).await;
                }
            }
        }

        let long_form = if summary.trim().is_empty() {
            render_summary(&entries)
        } else {
            summary
        };
        tokio::fs::write(dir.join("summary.md"), format!("{long_form}\n")).await?;
        let index = if entries.is_empty() {
            format!(
                "# Workspace Memory\n\n{}\n\nUse the memory tool for details.",
                truncate_chars(&long_form, max_chars())
            )
        } else {
            render_index(workspace_id, &entries, max_chars())
        };
        tokio::fs::write(dir.join("MEMORY.md"), index).await?;
        Ok(())
    }

    pub async fn clear(&self, workspace_id: &str) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM memory_stage1 WHERE workspace_id = ?")
            .bind(workspace_id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM memory_entries WHERE workspace_id = ?")
            .bind(workspace_id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM memory_summary WHERE workspace_id = ?")
            .bind(workspace_id)
            .execute(&self.pool)
            .await?;
        let _ = tokio::fs::remove_dir_all(self.memory_dir(workspace_id)).await;
        let legacy = self
            .root
            .join(".deep_agents")
            .join("memory")
            .join(format!("{}.md", safe_id(workspace_id)));
        let _ = tokio::fs::remove_file(legacy).await;
        Ok(())
    }
}

fn topic_label(topic: &str) -> String {
    let mut chars = topic.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Memory".to_string(),
    }
}

fn render_index(workspace_id: &str, entries: &[MemoryEntry], max_chars: usize) -> String {
    let mut topics: BTreeMap<String, usize> = BTreeMap::new();
    for entry in entries {
        *topics.entry(entry.topic.clone()).or_default() += 1;
    }
    let topic_line = topics
        .iter()
        .map(|(topic, count)| format!("{topic} ({count})"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut text = format!(
        "# Workspace Memory\n\nWorkspace: {workspace_id}\nUpdated: {}\n",
        now()
    );
    if !topic_line.is_empty() {
        text.push_str(&format!("Topics: {topic_line}\n"));
    }
    text.push_str(
        "\nRead details with the memory tool: action=\"read\", topic=\"<name>\".\n\n## Index\n",
    );
    let mut sorted: Vec<&MemoryEntry> = entries.iter().collect();
    sorted.sort_by(|left, right| {
        right
            .pinned
            .cmp(&left.pinned)
            .then_with(|| right.last_used.cmp(&left.last_used))
            .then_with(|| right.created_at.cmp(&left.created_at))
    });
    for (lines, entry) in sorted.into_iter().enumerate() {
        if lines >= MAX_INDEX_LINES {
            text.push_str("- … more entries in topic files\n");
            break;
        }
        let line = format!("- [{}] {} ({})\n", entry.kind, entry.text, entry.topic);
        if text.chars().count() + line.chars().count() > max_chars {
            text.push_str("- … index capped; read topic files for more\n");
            break;
        }
        text.push_str(&line);
    }
    if entries.is_empty() {
        text.push_str("- No durable entries yet.\n");
    }
    text
}

fn render_summary(entries: &[MemoryEntry]) -> String {
    let mut groups: BTreeMap<String, Vec<&MemoryEntry>> = BTreeMap::new();
    for entry in entries {
        groups.entry(entry.topic.clone()).or_default().push(entry);
    }
    let mut text = String::from("# Workspace memory\n");
    for (topic, items) in groups {
        text.push_str(&format!("\n## {}\n", topic_label(&topic)));
        for entry in items {
            text.push_str(&format!("- {}\n", entry.text));
        }
    }
    truncate_chars(&text, max_chars())
}

pub fn enabled() -> bool {
    !matches!(
        std::env::var("STARK_MEMORY")
            .unwrap_or_else(|_| "on".to_string())
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "off" | "0" | "false" | "none"
    )
}

pub fn max_chars() -> usize {
    std::env::var("STARK_MEMORY_MAX_CHARS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_SUMMARY_CHARS)
}

pub fn consolidate_after() -> usize {
    std::env::var("STARK_MEMORY_CONSOLIDATE_AFTER")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(8)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> (tempfile::TempDir, MemoryStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(dir.path()).await.unwrap();
        (dir, store)
    }

    #[tokio::test]
    async fn stage1_consolidates_into_index_and_topics_and_resets() {
        let (_dir, store) = store().await;
        store
            .record(
                "_general",
                "chat-1",
                None,
                "build a parser",
                "parser shipped",
            )
            .await
            .unwrap();
        store
            .record("_general", "chat-2", None, "add tests", "tests added")
            .await
            .unwrap();
        let summary = store.consolidate("_general", 20).await.unwrap();
        assert!(summary.contains("parser shipped"));
        assert!(summary.contains("tests added"));
        let index = store.index("_general", 8_000).await.unwrap();
        assert!(index.contains("parser"));
        assert!(index.contains("recent"));
        let topics = store.topics("_general").await.unwrap();
        assert!(topics.iter().any(|topic| topic["name"] == "recent"));
        let loaded = store.summary("_general", 8).await.unwrap();
        assert!(loaded.chars().count() <= 8);
        let status = store.status("_general").await.unwrap();
        assert_eq!(status["stage1Count"], 2);
        assert_eq!(status["hasSummary"], true);
        assert_eq!(status["pendingCount"], 0);
        store.clear("_general").await.unwrap();
        assert!(store.summary("_general", 100).await.is_none());
        assert!(store.index("_general", 100).await.is_none());
    }

    #[tokio::test]
    async fn manual_entries_are_pinned_and_forgettable() {
        let (_dir, store) = store().await;
        let id = store
            .add_entry(
                "_general",
                NewEntry {
                    chat_id: "chat".to_string(),
                    agent_id: None,
                    kind: "preference".to_string(),
                    topic: "preferences".to_string(),
                    text: "Use pnpm, not npm".to_string(),
                    source: "manual".to_string(),
                    model: String::new(),
                    pinned: true,
                },
            )
            .await
            .unwrap();
        let index = store.index("_general", 8_000).await.unwrap();
        assert!(index.contains("pnpm"));
        assert!(store.forget_entry("_general", &id).await.unwrap());
        assert!(store.index("_general", 8_000).await.is_none());
    }

    #[tokio::test]
    async fn secrets_are_redacted_before_storage() {
        let (_dir, store) = store().await;
        store
            .record(
                "_general",
                "chat",
                None,
                "use key sk-1234567890abcdefghij",
                "token = supersecretvalue",
            )
            .await
            .unwrap();
        let records = store.records("_general", 10).await.unwrap();
        let text = format!("{records:?}");
        assert!(!text.contains("supersecretvalue"));
        assert!(!text.contains("1234567890abcdefghij"));
    }
}
