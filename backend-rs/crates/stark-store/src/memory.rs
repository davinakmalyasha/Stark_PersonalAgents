//! Codex-style workspace memory.
//!
//! Completed turns produce compact stage-1 records. Consolidation folds the
//! newest records into one bounded workspace summary. The summary is injected
//! as context, never as a replacement for the session history.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::StoreError;

const DEFAULT_SUMMARY_CHARS: usize = 8_000;
const MAX_RECORD_CHARS: usize = 12_000;

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
                consolidated_at TEXT
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS ix_memory_stage1_workspace ON memory_stage1(workspace_id, created_at DESC)",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS memory_summary (
                workspace_id TEXT PRIMARY KEY,
                summary TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                consolidated_count INTEGER NOT NULL DEFAULT 0
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn record(
        &self,
        workspace_id: &str,
        chat_id: &str,
        agent_id: Option<&str>,
        request: &str,
        report: &str,
    ) -> Result<(), StoreError> {
        if report.trim().is_empty() {
            return Ok(());
        }
        let raw: String = report.chars().take(MAX_RECORD_CHARS).collect();
        let summary = format!(
            "Request: {}\nOutcome: {}",
            request.chars().take(500).collect::<String>(),
            raw.chars().take(2_000).collect::<String>()
        );
        sqlx::query(
            r#"
            INSERT INTO memory_stage1
                (id, workspace_id, chat_id, agent_id, request, summary, raw_memory,
                 created_at, selected_for_phase2)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1)
            "#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(workspace_id)
        .bind(chat_id)
        .bind(agent_id)
        .bind(request.chars().take(2_000).collect::<String>())
        .bind(&summary)
        .bind(&raw)
        .bind(now())
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
            Some(summary.chars().take(max_chars).collect())
        }
    }

    pub async fn consolidate(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<String, StoreError> {
        let rows = sqlx::query(
            "SELECT summary FROM memory_stage1 WHERE workspace_id = ? AND selected_for_phase2 = 1 ORDER BY created_at DESC LIMIT ?",
        )
        .bind(workspace_id)
        .bind(limit.max(1) as i64)
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            return Ok(String::new());
        }
        let mut lines = vec!["# Workspace memory\n".to_string()];
        for row in &rows {
            let summary: String = row.get("summary");
            lines.push(format!("- {}", summary.replace('\n', " ")));
        }
        let summary = lines.join("\n");
        let now = now();
        sqlx::query(
            r#"
            INSERT INTO memory_summary (workspace_id, summary, updated_at, consolidated_count)
            VALUES (?, ?, ?, ?)
            ON CONFLICT(workspace_id) DO UPDATE SET
                summary = excluded.summary,
                updated_at = excluded.updated_at,
                consolidated_count = excluded.consolidated_count
            "#,
        )
        .bind(workspace_id)
        .bind(&summary)
        .bind(&now)
        .bind(limit as i64)
        .execute(&self.pool)
        .await?;
        self.write_summary_file(workspace_id, &summary).await?;
        Ok(summary)
    }

    async fn write_summary_file(
        &self,
        workspace_id: &str,
        summary: &str,
    ) -> Result<(), StoreError> {
        let dir = self.root.join(".deep_agents").join("memory");
        tokio::fs::create_dir_all(&dir).await?;
        tokio::fs::write(dir.join(format!("{}.md", safe_id(workspace_id))), summary).await?;
        Ok(())
    }

    pub async fn clear(&self, workspace_id: &str) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM memory_stage1 WHERE workspace_id = ?")
            .bind(workspace_id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM memory_summary WHERE workspace_id = ?")
            .bind(workspace_id)
            .execute(&self.pool)
            .await?;
        let path = self
            .root
            .join(".deep_agents")
            .join("memory")
            .join(format!("{}.md", safe_id(workspace_id)));
        let _ = tokio::fs::remove_file(path).await;
        Ok(())
    }

    pub async fn status(&self, workspace_id: &str) -> Result<Value, StoreError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM memory_stage1 WHERE workspace_id = ? AND selected_for_phase2 = 1",
        )
        .bind(workspace_id)
        .fetch_one(&self.pool)
        .await?;
        let summary: Option<String> =
            sqlx::query_scalar("SELECT summary FROM memory_summary WHERE workspace_id = ?")
                .bind(workspace_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(json!({
            "workspaceId": workspace_id,
            "stage1Count": count,
            "hasSummary": summary.as_deref().is_some_and(|value| !value.trim().is_empty()),
            "summaryChars": summary.map(|value| value.chars().count()).unwrap_or(0),
        }))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stage1_consolidates_into_bounded_summary_and_resets() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(dir.path()).await.unwrap();
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
        let loaded = store.summary("_general", 8).await.unwrap();
        assert!(loaded.chars().count() <= 8);
        let status = store.status("_general").await.unwrap();
        assert_eq!(status["stage1Count"], 2);
        assert_eq!(status["hasSummary"], true);
        store.clear("_general").await.unwrap();
        assert!(store.summary("_general", 100).await.is_none());
    }
}
