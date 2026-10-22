use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid chat id: {0}")]
    InvalidChatId(String),
}

#[derive(Debug, Clone)]
pub struct StoredMessage {
    pub role: String,
    pub content: String,
}

#[derive(Clone)]
pub struct ChatStore {
    pool: SqlitePool,
    harness_root: PathBuf,
}

impl ChatStore {
    pub async fn open(harness_root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let harness_root = harness_root.into();
        let db_path = harness_root.join(".deep_agents").join("chats.db");
        if let Some(parent) = db_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let options = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;

        let store = Self { pool, harness_root };
        store.init().await?;
        Ok(store)
    }

    pub fn harness_root(&self) -> &Path {
        &self.harness_root
    }

    async fn init(&self) -> Result<(), StoreError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS chats (
                id TEXT PRIMARY KEY,
                workspace_id TEXT,
                workspace_path TEXT,
                title TEXT,
                model TEXT,
                created_at TEXT,
                updated_at TEXT,
                message_count INTEGER DEFAULT 0,
                mode TEXT DEFAULT 'build',
                pinned INTEGER DEFAULT 0,
                tasks_json TEXT
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY,
                chat_id TEXT,
                role TEXT,
                content TEXT,
                timestamp TEXT,
                is_trace INTEGER DEFAULT 0,
                metadata_json TEXT,
                FOREIGN KEY(chat_id) REFERENCES chats(id) ON DELETE CASCADE
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_messages_chat_ts ON messages(chat_id, timestamp)",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn ensure_chat(
        &self,
        chat_id: &str,
        workspace_path: &str,
        model: &str,
        prompt: &str,
    ) -> Result<(), StoreError> {
        validate_chat_id(chat_id)?;
        let now = now_iso();
        let title = title_from(prompt);
        sqlx::query(
            r#"
            INSERT INTO chats
                (id, workspace_id, workspace_path, title, model, created_at, updated_at, message_count, mode)
            VALUES (?, ?, ?, ?, ?, ?, ?, 0, 'build')
            ON CONFLICT(id) DO UPDATE SET
                workspace_path = excluded.workspace_path,
                model = excluded.model,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(chat_id)
        .bind(workspace_path)
        .bind(workspace_path)
        .bind(title)
        .bind(model)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn append_message(
        &self,
        chat_id: &str,
        role: &str,
        content: &str,
        is_trace: bool,
    ) -> Result<String, StoreError> {
        validate_chat_id(chat_id)?;
        let id = Uuid::new_v4().to_string();
        let now = now_iso();
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO messages (id, chat_id, role, content, timestamp, is_trace, metadata_json) VALUES (?, ?, ?, ?, ?, ?, NULL)",
        )
        .bind(&id)
        .bind(chat_id)
        .bind(role)
        .bind(content)
        .bind(&now)
        .bind(if is_trace { 1 } else { 0 })
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE chats SET message_count = (SELECT COUNT(*) FROM messages WHERE chat_id = ?), updated_at = ? WHERE id = ?",
        )
        .bind(chat_id)
        .bind(&now)
        .bind(chat_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(id)
    }

    pub async fn history(
        &self,
        chat_id: &str,
        limit: usize,
    ) -> Result<Vec<StoredMessage>, StoreError> {
        let rows = sqlx::query(
            r#"
            SELECT role, content FROM (
                SELECT role, content, timestamp, rowid
                FROM messages
                WHERE chat_id = ? AND is_trace = 0
                ORDER BY timestamp DESC, rowid DESC
                LIMIT ?
            )
            ORDER BY timestamp ASC, rowid ASC
            "#,
        )
        .bind(chat_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| StoredMessage {
                role: row.get::<String, _>("role"),
                content: row.get::<String, _>("content"),
            })
            .collect())
    }

    pub async fn recent_user_prompts(&self, limit: usize) -> Result<Vec<String>, StoreError> {
        let rows = sqlx::query(
            "SELECT content FROM messages WHERE role = 'user' AND is_trace = 0 ORDER BY rowid DESC LIMIT ?",
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        let mut prompts: Vec<String> = rows
            .into_iter()
            .map(|row| row.get::<String, _>("content"))
            .collect();
        prompts.reverse();
        Ok(prompts)
    }

    pub async fn append_session_entry(
        &self,
        workspace: &Path,
        chat_id: &str,
        mut entry: Value,
    ) -> Result<(), StoreError> {
        validate_chat_id(chat_id)?;
        let path = session_path(workspace, chat_id);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let is_new = !path.exists();
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;

        use tokio::io::AsyncWriteExt;
        if is_new {
            let header = json!({
                "type": "session",
                "version": 1,
                "id": Uuid::new_v4().to_string(),
                "timestamp": Utc::now().timestamp_millis(),
                "cwd": workspace,
                "parent_session": null,
                "delegation_depth": 0,
                "name": null,
                "chat_id": chat_id,
            });
            file.write_all(format!("{header}\n").as_bytes()).await?;
        }

        if let Some(object) = entry.as_object_mut() {
            object
                .entry("id")
                .or_insert_with(|| Value::String(Uuid::new_v4().to_string()));
            object.entry("parent_id").or_insert(Value::Null);
            object
                .entry("timestamp")
                .or_insert_with(|| Value::from(Utc::now().timestamp_millis()));
        }
        file.write_all(format!("{entry}\n").as_bytes()).await?;
        file.flush().await?;
        Ok(())
    }
}

fn validate_chat_id(chat_id: &str) -> Result<(), StoreError> {
    if chat_id.is_empty()
        || !chat_id.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return Err(StoreError::InvalidChatId(chat_id.to_string()));
    }
    Ok(())
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn title_from(prompt: &str) -> String {
    let title: String = prompt
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(60)
        .collect();
    if title.is_empty() {
        "New Chat".to_string()
    } else {
        title
    }
}

fn session_path(workspace: &Path, chat_id: &str) -> PathBuf {
    workspace
        .join(".deep_agents")
        .join("sessions")
        .join(format!("{chat_id}.jsonl"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn chat_round_trip_and_session_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChatStore::open(dir.path()).await.unwrap();
        store
            .ensure_chat(
                "chat-1",
                dir.path().to_string_lossy().as_ref(),
                "mock",
                "Hello",
            )
            .await
            .unwrap();
        store
            .append_message("chat-1", "user", "Hello", false)
            .await
            .unwrap();
        store
            .append_message("chat-1", "assistant", "Hi there", false)
            .await
            .unwrap();

        let history = store.history("chat-1", 10).await.unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].content, "Hello");
        assert_eq!(history[1].role, "assistant");

        let prompts = store.recent_user_prompts(10).await.unwrap();
        assert_eq!(prompts, vec!["Hello"]);

        store
            .append_session_entry(
                dir.path(),
                "chat-1",
                json!({"type": "message", "role": "user", "content": "Hello"}),
            )
            .await
            .unwrap();
        let path = session_path(dir.path(), "chat-1");
        let raw = tokio::fs::read_to_string(path).await.unwrap();
        assert!(raw.contains("\"type\":\"session\""));
        assert!(raw.contains("\"role\":\"user\""));
    }
}
