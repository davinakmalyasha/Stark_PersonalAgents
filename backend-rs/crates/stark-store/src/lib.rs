use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use thiserror::Error;
use uuid::Uuid;

mod agents;
mod canvas;
mod groups;
mod jobs;
mod memory;
mod schedules;
mod workspaces;
pub use agents::{AgentRecord, AgentStore};
pub use canvas::CanvasStore;
pub use groups::GroupStore;
pub use jobs::{JobRecord, JobStore};
pub use memory::{
    MemoryEntry, MemoryStore, NewEntry, Stage1Record,
    consolidate_after as memory_consolidate_after, enabled as memory_enabled,
    max_chars as memory_max_chars,
};
pub use schedules::{
    ScheduleRecord, ScheduleStore, parse_time as parse_schedule_time, record_json as schedule_json,
};
pub use workspaces::{WorkspaceRecord, WorkspaceRegistry};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid chat id: {0}")]
    InvalidChatId(String),
    #[error("invalid state: {0}")]
    Invalid(String),
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
    jobs: JobStore,
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

        let jobs = JobStore::open(&harness_root).await?;
        jobs.fail_stale().await?;
        let store = Self {
            pool,
            harness_root,
            jobs,
        };
        store.init().await?;
        Ok(store)
    }

    pub fn harness_root(&self) -> &Path {
        &self.harness_root
    }

    pub fn jobs(&self) -> &JobStore {
        &self.jobs
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
        for statement in [
            "ALTER TABLE chats ADD COLUMN parent_chat_id TEXT",
            "ALTER TABLE chats ADD COLUMN parent_workspace_id TEXT",
            "ALTER TABLE chats ADD COLUMN token_usage_json TEXT",
        ] {
            let _ = sqlx::query(statement).execute(&self.pool).await;
        }
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
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS agent_notes (
                id TEXT PRIMARY KEY,
                scope_type TEXT NOT NULL,
                scope_id TEXT NOT NULL,
                workspace_path TEXT DEFAULT '',
                title TEXT DEFAULT '',
                body TEXT DEFAULT '',
                todos_json TEXT DEFAULT '[]',
                created_by TEXT DEFAULT 'orchestrator',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_agent_notes_scope ON agent_notes(scope_type, scope_id)",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub fn workspace_id_for_path(&self, workspace_path: &str) -> String {
        let path = workspace_path.trim();
        if path.is_empty() || path == "_general" {
            return "_general".to_string();
        }
        if matches!(path, "_house" | "_personal" | "_dfd") {
            return path.to_string();
        }
        WorkspaceRegistry::new(&self.harness_root)
            .find_by_path(path)
            .map(|workspace| workspace.id)
            .unwrap_or_else(|| path.to_string())
    }

    /// Cache/routing affinity for a chat: a child shares its parent's id,
    /// because provider prompt caches are keyed by conversation family.
    pub async fn chat_affinity(&self, chat_id: &str) -> Result<String, StoreError> {
        let affinity = sqlx::query_scalar::<_, String>(
            "SELECT COALESCE(NULLIF(parent_chat_id, ''), id) FROM chats WHERE id = ?",
        )
        .bind(chat_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(affinity.unwrap_or_else(|| chat_id.to_string()))
    }

    pub async fn chat_workspace_id(&self, chat_id: &str) -> Result<Option<String>, StoreError> {
        validate_chat_id(chat_id)?;
        let row = sqlx::query("SELECT workspace_id FROM chats WHERE id = ?")
            .bind(chat_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.and_then(|row| {
            row.try_get::<Option<String>, _>("workspace_id")
                .ok()
                .flatten()
        }))
    }

    pub async fn ensure_chat(
        &self,
        chat_id: &str,
        workspace_id: &str,
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
        .bind(workspace_id)
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

    pub async fn list_notes(&self, scope_type: &str, scope_id: &str) -> Vec<Value> {
        let rows = sqlx::query(
            "SELECT id, scope_type, scope_id, workspace_path, title, body, todos_json, created_by, created_at, updated_at FROM agent_notes WHERE scope_type = ? AND scope_id = ? ORDER BY updated_at DESC",
        )
        .bind(scope_type)
        .bind(scope_id)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        rows.into_iter().map(note_row).collect()
    }

    pub async fn create_note(
        &self,
        scope_type: &str,
        scope_id: &str,
        title: &str,
        body: &str,
        todos: &Value,
    ) -> Result<Value, StoreError> {
        let id = format!("note-{}", &Uuid::new_v4().simple().to_string()[..10]);
        let now = now_iso();
        let title = if title.trim().is_empty() {
            "Note".to_string()
        } else {
            title.trim().to_string()
        };
        let todos = normalize_todos(todos);
        sqlx::query(
            "INSERT INTO agent_notes (id, scope_type, scope_id, workspace_path, title, body, todos_json, created_by, created_at, updated_at) VALUES (?, ?, ?, '', ?, ?, ?, 'user', ?, ?)",
        )
        .bind(&id)
        .bind(scope_type)
        .bind(scope_id)
        .bind(&title)
        .bind(body)
        .bind(todos.to_string())
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(json!({
            "id": id,
            "scopeType": scope_type,
            "scopeId": scope_id,
            "workspacePath": "",
            "title": title,
            "body": body,
            "todos": todos,
            "createdBy": "user",
            "createdAt": now,
            "updatedAt": now,
        }))
    }

    pub async fn update_note(
        &self,
        note_id: &str,
        title: Option<&str>,
        body: Option<&str>,
        todos: Option<&Value>,
    ) -> Result<Option<Value>, StoreError> {
        let existing = sqlx::query(
            "SELECT id, scope_type, scope_id, workspace_path, title, body, todos_json, created_by, created_at, updated_at FROM agent_notes WHERE id = ?",
        )
        .bind(note_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = existing else {
            return Ok(None);
        };
        let title = title
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| row.get::<String, _>("title"));
        let body = body
            .map(str::to_string)
            .unwrap_or_else(|| row.get::<String, _>("body"));
        let todos = todos.map(normalize_todos).unwrap_or_else(|| {
            serde_json::from_str(&row.get::<String, _>("todos_json")).unwrap_or_else(|_| json!([]))
        });
        let now = now_iso();
        sqlx::query("UPDATE agent_notes SET title = ?, body = ?, todos_json = ?, updated_at = ? WHERE id = ?")
            .bind(&title)
            .bind(&body)
            .bind(todos.to_string())
            .bind(&now)
            .bind(note_id)
            .execute(&self.pool)
            .await?;
        Ok(Some(json!({
            "id": note_id,
            "scopeType": row.get::<String, _>("scope_type"),
            "scopeId": row.get::<String, _>("scope_id"),
            "workspacePath": row.get::<String, _>("workspace_path"),
            "title": title,
            "body": body,
            "todos": todos,
            "createdBy": row.get::<String, _>("created_by"),
            "createdAt": row.get::<String, _>("created_at"),
            "updatedAt": now,
        })))
    }

    pub async fn delete_note(&self, note_id: &str) -> Result<bool, StoreError> {
        let result = sqlx::query("DELETE FROM agent_notes WHERE id = ?")
            .bind(note_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn chat_stats(&self, workspace_id: &str) -> (i64, String) {
        let row = sqlx::query(
            "SELECT COUNT(*), COALESCE(MAX(updated_at), '') FROM chats WHERE workspace_id = ?",
        )
        .bind(workspace_id)
        .fetch_one(&self.pool)
        .await;
        match row {
            Ok(row) => (row.get::<i64, _>(0), row.get::<String, _>(1)),
            Err(_) => (0, String::new()),
        }
    }

    pub async fn list_chats(&self, workspace_id: &str) -> Vec<Value> {
        let rows = sqlx::query(
            r#"
            SELECT c.id, c.title, c.model, c.created_at, c.updated_at, c.message_count,
                   c.parent_chat_id, c.parent_workspace_id,
                   (SELECT m.content FROM messages m WHERE m.chat_id = c.id
                    ORDER BY m.timestamp DESC, m.rowid DESC LIMIT 1) AS last_message,
                   (SELECT m.role FROM messages m WHERE m.chat_id = c.id
                    ORDER BY m.timestamp DESC, m.rowid DESC LIMIT 1) AS last_role
            FROM chats c
            WHERE c.workspace_id = ?
            ORDER BY c.updated_at DESC
            "#,
        )
        .bind(workspace_id)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        rows.into_iter()
            .map(|row| {
                let mut item = json!({
                    "id": row.get::<String, _>("id"),
                    "title": row.get::<String, _>("title"),
                    "messageCount": row.get::<i64, _>("message_count"),
                    "model": row.get::<String, _>("model"),
                    "createdAt": row.get::<String, _>("created_at"),
                    "updatedAt": row.get::<String, _>("updated_at"),
                });
                if let Some(parent) = row
                    .try_get::<Option<String>, _>("parent_chat_id")
                    .ok()
                    .flatten()
                {
                    item["parentChatId"] = Value::String(parent);
                }
                if let Some(parent) = row
                    .try_get::<Option<String>, _>("parent_workspace_id")
                    .ok()
                    .flatten()
                {
                    item["parentWorkspaceId"] = Value::String(parent);
                }
                if let Some(last) = row
                    .try_get::<Option<String>, _>("last_message")
                    .ok()
                    .flatten()
                {
                    let snippet: String = last.split_whitespace().collect::<Vec<_>>().join(" ");
                    item["lastMessage"] = Value::String(snippet.chars().take(180).collect());
                }
                if let Some(role) = row.try_get::<Option<String>, _>("last_role").ok().flatten() {
                    item["lastRole"] = Value::String(display_role(&role));
                }
                item
            })
            .collect()
    }

    /// Every chat across workspaces with its workspace/parent metadata.
    /// Canvas mode needs the global view; `list_chats` stays workspace-scoped.
    pub async fn all_chats(&self) -> Vec<Value> {
        let rows = sqlx::query(
            r#"
            SELECT c.id, c.workspace_id, c.workspace_path, c.title, c.model,
                   c.created_at, c.updated_at, c.message_count, c.parent_chat_id,
                   (SELECT m.content FROM messages m WHERE m.chat_id = c.id
                    ORDER BY m.timestamp DESC, m.rowid DESC LIMIT 1) AS last_message,
                   (SELECT m.role FROM messages m WHERE m.chat_id = c.id
                    ORDER BY m.timestamp DESC, m.rowid DESC LIMIT 1) AS last_role
            FROM chats c
            ORDER BY c.updated_at DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        rows.into_iter()
            .map(|row| {
                let last = row
                    .try_get::<Option<String>, _>("last_message")
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let snippet: String = last.split_whitespace().collect::<Vec<_>>().join(" ");
                json!({
                    "id": row.get::<String, _>("id"),
                    "workspaceId": row.get::<String, _>("workspace_id"),
                    "workspacePath": row.get::<String, _>("workspace_path"),
                    "title": row.get::<String, _>("title"),
                    "model": row.get::<String, _>("model"),
                    "createdAt": row.get::<String, _>("created_at"),
                    "updatedAt": row.get::<String, _>("updated_at"),
                    "messageCount": row.get::<i64, _>("message_count"),
                    "parentChatId": row.try_get::<Option<String>, _>("parent_chat_id").ok().flatten(),
                    "lastRole": row
                        .try_get::<Option<String>, _>("last_role")
                        .ok()
                        .flatten()
                        .map(|role| display_role(&role))
                        .unwrap_or_default(),
                    "lastMessage": snippet.chars().take(1200).collect::<String>(),
                })
            })
            .collect()
    }

    pub async fn create_chat(
        &self,
        workspace_id: &str,
        workspace_path: &str,
        title: &str,
        model: &str,
    ) -> Result<Value, StoreError> {
        let id = format!("chat-{}", &Uuid::new_v4().simple().to_string()[..12]);
        validate_chat_id(&id)?;
        let title = if title.trim().is_empty() {
            "New Chat".to_string()
        } else {
            title.trim().to_string()
        };
        let now = now_iso();
        sqlx::query(
            r#"
            INSERT INTO chats
                (id, workspace_id, workspace_path, title, model, created_at, updated_at, message_count, mode)
            VALUES (?, ?, ?, ?, ?, ?, ?, 0, 'build')
            "#,
        )
        .bind(&id)
        .bind(workspace_id)
        .bind(workspace_path)
        .bind(&title)
        .bind(model)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(json!({
            "id": id,
            "title": title,
            "model": model,
            "createdAt": now,
            "updatedAt": now,
            "messages": [],
        }))
    }

    pub async fn create_child_chat(
        &self,
        workspace_id: &str,
        workspace_path: &str,
        title: &str,
        model: &str,
        parent_chat_id: &str,
    ) -> Result<Value, StoreError> {
        self.create_linked_chat(
            workspace_id,
            workspace_path,
            title,
            model,
            parent_chat_id,
            workspace_id,
        )
        .await
    }

    pub async fn create_linked_chat(
        &self,
        workspace_id: &str,
        workspace_path: &str,
        title: &str,
        model: &str,
        parent_chat_id: &str,
        parent_workspace_id: &str,
    ) -> Result<Value, StoreError> {
        let id = format!("del-{}", &Uuid::new_v4().simple().to_string()[..10]);
        validate_chat_id(&id)?;
        let title = if title.trim().is_empty() {
            "New Chat".to_string()
        } else {
            title.trim().chars().take(80).collect()
        };
        let now = now_iso();
        sqlx::query(
            r#"
            INSERT INTO chats
                (id, workspace_id, workspace_path, title, model, created_at, updated_at, message_count, mode, parent_chat_id, parent_workspace_id)
            VALUES (?, ?, ?, ?, ?, ?, ?, 0, 'build', ?, ?)
            "#,
        )
        .bind(&id)
        .bind(workspace_id)
        .bind(workspace_path)
        .bind(&title)
        .bind(model)
        .bind(&now)
        .bind(&now)
        .bind(parent_chat_id)
        .bind(parent_workspace_id)
        .execute(&self.pool)
        .await?;
        Ok(json!({
            "id": id,
            "title": title,
            "model": model,
            "createdAt": now,
            "updatedAt": now,
            "messages": [],
            "parentChatId": parent_chat_id,
        }))
    }

    pub async fn get_chat(&self, chat_id: &str) -> Result<Option<Value>, StoreError> {
        validate_chat_id(chat_id)?;
        let row = sqlx::query(
            "SELECT id, workspace_id, workspace_path, title, model, created_at, updated_at, pinned, token_usage_json, parent_chat_id, parent_workspace_id FROM chats WHERE id = ?",
        )
        .bind(chat_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };

        let messages = sqlx::query(
            "SELECT id, role, content, timestamp, is_trace, metadata_json FROM messages WHERE chat_id = ? ORDER BY timestamp ASC, rowid ASC",
        )
        .bind(chat_id)
        .fetch_all(&self.pool)
        .await?;
        let messages: Vec<Value> = messages
            .into_iter()
            .map(|message| {
                let is_trace = message.get::<i64, _>("is_trace") != 0;
                let metadata: Option<String> = message.get("metadata_json");
                let mut value = json!({
                    "id": message.get::<String, _>("id"),
                    "role": display_role(&message.get::<String, _>("role")),
                    "content": message.get::<String, _>("content"),
                    "timestamp": message.get::<String, _>("timestamp"),
                });
                if let Some(raw) = metadata {
                    if let Ok(mut metadata) = serde_json::from_str::<Value>(&raw) {
                        if is_trace {
                            metadata["isTrace"] = Value::Bool(true);
                        }
                        value["metadata"] = metadata;
                    }
                } else if is_trace {
                    value["metadata"] = json!({"isTrace": true});
                }
                value
            })
            .collect();

        let mut deduped: Vec<Value> = Vec::with_capacity(messages.len());
        for message in messages {
            let duplicate = deduped.last().is_some_and(|previous| {
                previous["role"] == "agent"
                    && message["role"] == "agent"
                    && previous["content"] == message["content"]
            });
            if duplicate {
                deduped.pop();
            }
            deduped.push(message);
        }
        let messages = deduped;

        let mut chat = json!({
            "id": row.get::<String, _>("id"),
            "title": row.get::<String, _>("title"),
            "model": row.get::<String, _>("model"),
            "createdAt": row.get::<String, _>("created_at"),
            "updatedAt": row.get::<String, _>("updated_at"),
            "messages": messages,
            "isPinned": row.get::<i64, _>("pinned") != 0,
            "workspaceId": row.get::<String, _>("workspace_id"),
            "workspacePath": row.get::<String, _>("workspace_path"),
        });
        if let Some(parent) = row
            .try_get::<Option<String>, _>("parent_chat_id")
            .ok()
            .flatten()
        {
            chat["parentChatId"] = Value::String(parent);
        }
        if let Some(parent) = row
            .try_get::<Option<String>, _>("parent_workspace_id")
            .ok()
            .flatten()
        {
            chat["parentWorkspaceId"] = Value::String(parent);
        }
        if let Some(raw) = row
            .try_get::<Option<String>, _>("token_usage_json")
            .ok()
            .flatten()
            && let Ok(usage) = serde_json::from_str::<Value>(&raw)
        {
            chat["token_usage"] = usage;
        }
        Ok(Some(chat))
    }

    pub async fn delete_chat(&self, chat_id: &str) -> Result<bool, StoreError> {
        validate_chat_id(chat_id)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM messages WHERE chat_id = ?")
            .bind(chat_id)
            .execute(&mut *tx)
            .await?;
        let result = sqlx::query("DELETE FROM chats WHERE id = ?")
            .bind(chat_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn update_chat_title(&self, chat_id: &str, title: &str) -> Result<bool, StoreError> {
        validate_chat_id(chat_id)?;
        let result = sqlx::query("UPDATE chats SET title = ?, updated_at = ? WHERE id = ?")
            .bind(title)
            .bind(now_iso())
            .bind(chat_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn chat_mode(&self, chat_id: &str) -> Result<Option<String>, StoreError> {
        validate_chat_id(chat_id)?;
        let row = sqlx::query("SELECT mode FROM chats WHERE id = ?")
            .bind(chat_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|row| row.get::<String, _>("mode")))
    }

    pub async fn set_chat_mode(&self, chat_id: &str, mode: &str) -> Result<bool, StoreError> {
        validate_chat_id(chat_id)?;
        if !matches!(mode, "plan" | "build") {
            return Ok(false);
        }
        let result = sqlx::query("UPDATE chats SET mode = ?, updated_at = ? WHERE id = ?")
            .bind(mode)
            .bind(now_iso())
            .bind(chat_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn chat_workspace_path(&self, chat_id: &str) -> Result<Option<String>, StoreError> {
        validate_chat_id(chat_id)?;
        let row = sqlx::query("SELECT workspace_path FROM chats WHERE id = ?")
            .bind(chat_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.and_then(|row| {
            row.try_get::<Option<String>, _>("workspace_path")
                .ok()
                .flatten()
        }))
    }

    pub async fn messages_from(
        &self,
        chat_id: &str,
        message_id: &str,
    ) -> Result<Option<Vec<Value>>, StoreError> {
        validate_chat_id(chat_id)?;
        let rows = sqlx::query(
            "SELECT id, role, content FROM messages WHERE chat_id = ? ORDER BY timestamp ASC, rowid ASC",
        )
        .bind(chat_id)
        .fetch_all(&self.pool)
        .await?;
        let Some(index) = rows
            .iter()
            .position(|row| row.get::<String, _>("id") == message_id)
        else {
            return Ok(None);
        };
        Ok(Some(
            rows[index..]
                .iter()
                .map(|row| {
                    json!({
                        "id": row.get::<String, _>("id"),
                        "role": display_role(&row.get::<String, _>("role")),
                        "content": row.get::<String, _>("content"),
                    })
                })
                .collect(),
        ))
    }

    pub async fn truncate_from_message(
        &self,
        chat_id: &str,
        message_id: &str,
    ) -> Result<Option<(String, Vec<Value>)>, StoreError> {
        validate_chat_id(chat_id)?;
        if message_id.is_empty() {
            return Ok(None);
        }
        let rows = sqlx::query(
            "SELECT id, role, content FROM messages WHERE chat_id = ? ORDER BY timestamp ASC, rowid ASC",
        )
        .bind(chat_id)
        .fetch_all(&self.pool)
        .await?;
        let Some(index) = rows
            .iter()
            .position(|row| row.get::<String, _>("id") == message_id)
        else {
            return Ok(None);
        };
        let prompt = rows[index].get::<String, _>("content");
        let removed: Vec<Value> = rows[index..]
            .iter()
            .map(|row| {
                json!({
                    "id": row.get::<String, _>("id"),
                    "role": display_role(&row.get::<String, _>("role")),
                })
            })
            .collect();
        let keep = &rows[..index];

        let mut tx = self.pool.begin().await?;
        for row in &rows[index..] {
            sqlx::query("DELETE FROM messages WHERE id = ? AND chat_id = ?")
                .bind(row.get::<String, _>("id"))
                .bind(chat_id)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("UPDATE chats SET updated_at = ?, message_count = ? WHERE id = ?")
            .bind(now_iso())
            .bind(keep.len() as i64)
            .bind(chat_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some((prompt, removed)))
    }

    pub async fn add_message(
        &self,
        chat_id: &str,
        role: &str,
        content: &str,
        metadata: Option<&Value>,
    ) -> Result<Value, StoreError> {
        let id = self.append_message(chat_id, role, content, false).await?;
        let mut message = json!({
            "id": id,
            "role": role,
            "content": content,
            "timestamp": now_iso(),
        });
        if let Some(metadata) = metadata {
            message["metadata"] = metadata.clone();
        }
        Ok(message)
    }

    pub async fn read_session_entries(
        &self,
        workspace_id: &str,
        workspace: &Path,
        chat_id: &str,
    ) -> Result<Vec<Value>, StoreError> {
        validate_chat_id(chat_id)?;
        let path = session_path_for(workspace, workspace_id, chat_id);
        let Ok(text) = tokio::fs::read_to_string(path).await else {
            return Ok(Vec::new());
        };
        let entries: Vec<Value> = text
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .collect();
        let Some(last_compaction) = entries
            .iter()
            .rposition(|entry| entry.get("type").and_then(Value::as_str) == Some("compaction"))
        else {
            return Ok(entries
                .into_iter()
                .filter(|entry| entry.get("type").and_then(Value::as_str) == Some("message"))
                .collect());
        };
        let compaction = &entries[last_compaction];
        let mut result = Vec::new();
        let checkpoint_format = compaction
            .get("checkpoint_format")
            .and_then(Value::as_str)
            .unwrap_or("");
        let summary = compaction
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("");
        let tail = compaction
            .get("retained_tail")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if checkpoint_format == "opencode-v1" {
            let recent = recent_text(&tail);
            result.push(json!({
                "type": "message",
                "role": "user",
                "content": format!(
                    "<conversation-checkpoint>
<summary>
{summary}
</summary>
<recent-context>
{recent}
</recent-context>
</conversation-checkpoint>"
                ),
            }));
        } else {
            if !summary.is_empty() {
                result.push(json!({
                    "type": "message",
                    "role": "system",
                    "content": format!("[CONTEXT SUMMARY]
                {summary}"),
                }));
            }
            result.extend(tail);
        }
        result.extend(
            entries[last_compaction + 1..]
                .iter()
                .filter(|entry| entry.get("type").and_then(Value::as_str) == Some("message"))
                .cloned(),
        );
        Ok(result)
    }

    pub async fn append_session_entry(
        &self,
        workspace: &Path,
        chat_id: &str,
        entry: Value,
    ) -> Result<(), StoreError> {
        let workspace_id = self.workspace_id_for_path(workspace.to_string_lossy().as_ref());
        self.append_session_entry_for(&workspace_id, workspace, chat_id, entry)
            .await
    }

    pub async fn append_session_entry_for(
        &self,
        workspace_id: &str,
        workspace: &Path,
        chat_id: &str,
        mut entry: Value,
    ) -> Result<(), StoreError> {
        validate_chat_id(chat_id)?;
        let path = session_path_for(workspace, workspace_id, chat_id);
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

fn note_row(row: sqlx::sqlite::SqliteRow) -> Value {
    let todos = serde_json::from_str::<Value>(&row.get::<String, _>("todos_json"))
        .unwrap_or_else(|_| json!([]));
    json!({
        "id": row.get::<String, _>("id"),
        "scopeType": row.get::<String, _>("scope_type"),
        "scopeId": row.get::<String, _>("scope_id"),
        "workspacePath": row.get::<String, _>("workspace_path"),
        "title": row.get::<String, _>("title"),
        "body": row.get::<String, _>("body"),
        "todos": todos,
        "createdBy": row.get::<String, _>("created_by"),
        "createdAt": row.get::<String, _>("created_at"),
        "updatedAt": row.get::<String, _>("updated_at"),
    })
}

fn normalize_todos(value: &Value) -> Value {
    let items = value.as_array().cloned().unwrap_or_default();
    let normalized: Vec<Value> = items
        .into_iter()
        .filter_map(|item| match item {
            Value::String(text) => {
                let text = text.trim().to_string();
                (!text.is_empty()).then(|| json!({"text": text, "done": false}))
            }
            Value::Object(object) => {
                let text = object
                    .get("text")
                    .or_else(|| object.get("title"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                (!text.is_empty()).then(|| {
                    json!({
                        "text": text,
                        "done": object.get("done").and_then(Value::as_bool).unwrap_or(false)
                    })
                })
            }
            _ => None,
        })
        .collect();
    Value::Array(normalized)
}

fn display_role(role: &str) -> String {
    if role == "assistant" {
        "agent".to_string()
    } else {
        role.to_string()
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

fn session_path_for(workspace: &Path, workspace_id: &str, chat_id: &str) -> PathBuf {
    let (kind, id) = match workspace_id {
        "_general" | "" => ("general", "_general"),
        "_house" => ("house", "_house"),
        "_personal" => ("personal", "_personal"),
        "_dfd" => ("dfd", "_dfd"),
        id if id.starts_with("ws-") => ("project", id),
        _ => ("project", "_unregistered"),
    };
    workspace
        .join(".deep_agents")
        .join("sessions")
        .join(kind)
        .join(id)
        .join(format!("{chat_id}.jsonl"))
}

fn recent_text(entries: &[Value]) -> String {
    let mut out = String::new();
    for entry in entries {
        let role = entry.get("role").and_then(Value::as_str).unwrap_or("");
        let content = &entry["content"];
        let mut rendered = String::new();
        if let Some(text) = content.as_str() {
            rendered.push_str(text);
        } else if let Some(blocks) = content.as_array() {
            for block in blocks {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    rendered.push_str(text);
                } else if block.get("type").and_then(Value::as_str) == Some("toolCall") {
                    if !rendered.is_empty() {
                        rendered.push('\n');
                    }
                    rendered.push_str(&format!(
                        "[Assistant tool call]: {}({})",
                        block.get("name").and_then(Value::as_str).unwrap_or(""),
                        block.get("arguments").cloned().unwrap_or_else(|| json!({}))
                    ));
                }
            }
        }
        if rendered.trim().is_empty() {
            continue;
        }
        let label = match role {
            "user" => "User",
            "assistant" => "Assistant",
            "toolResult" => "Tool result",
            other => other,
        };
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("[{label}]: {rendered}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn notes_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChatStore::open(dir.path()).await.unwrap();
        let note = store
            .create_note(
                "chat",
                "chat-1",
                "Title",
                "Body",
                &json!(["one", {"text": "two", "done": true}]),
            )
            .await
            .unwrap();
        let id = note["id"].as_str().unwrap();
        assert_eq!(note["todos"][0]["text"], "one");
        assert_eq!(note["todos"][1]["done"], true);

        let notes = store.list_notes("chat", "chat-1").await;
        assert_eq!(notes.len(), 1);

        let updated = store
            .update_note(id, Some("Renamed"), None, Some(&json!(["done"])))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated["title"], "Renamed");
        assert_eq!(updated["todos"][0]["text"], "done");

        assert!(store.delete_note(id).await.unwrap());
        assert!(store.list_notes("chat", "chat-1").await.is_empty());
    }

    #[tokio::test]
    async fn truncate_and_mode_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChatStore::open(dir.path()).await.unwrap();
        let chat = store
            .create_chat("_general", "", "Mode", "mock")
            .await
            .unwrap();
        let chat_id = chat["id"].as_str().unwrap();
        let first = store
            .add_message(chat_id, "user", "one", None)
            .await
            .unwrap();
        store
            .add_message(chat_id, "assistant", "two", None)
            .await
            .unwrap();
        let last = store
            .add_message(chat_id, "user", "three", None)
            .await
            .unwrap();

        assert_eq!(
            store.chat_mode(chat_id).await.unwrap().as_deref(),
            Some("build")
        );
        assert!(store.set_chat_mode(chat_id, "plan").await.unwrap());
        assert_eq!(
            store.chat_mode(chat_id).await.unwrap().as_deref(),
            Some("plan")
        );

        let (prompt, removed) = store
            .truncate_from_message(chat_id, last["id"].as_str().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(prompt, "three");
        assert_eq!(removed.len(), 1);
        assert_eq!(
            store.get_chat(chat_id).await.unwrap().unwrap()["messages"]
                .as_array()
                .unwrap()
                .len(),
            2
        );

        let (prompt, removed) = store
            .truncate_from_message(chat_id, first["id"].as_str().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(prompt, "one");
        assert_eq!(removed.len(), 2);
        assert!(
            store.get_chat(chat_id).await.unwrap().unwrap()["messages"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn chat_crud_uses_workspace_bucket() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let registry = WorkspaceRegistry::new(dir.path());
        let record = registry
            .add(project.to_string_lossy().as_ref(), "Project")
            .unwrap();

        let store = ChatStore::open(dir.path()).await.unwrap();
        let chat = store
            .create_chat(&record.id, &record.path, "New Chat", "mock")
            .await
            .unwrap();
        let chat_id = chat["id"].as_str().unwrap();

        store
            .add_message(chat_id, "user", "hello", None)
            .await
            .unwrap();
        store.update_chat_title(chat_id, "Renamed").await.unwrap();

        let chats = store.list_chats(&record.id).await;
        assert_eq!(chats.len(), 1);
        assert_eq!(chats[0]["title"], "Renamed");
        assert_eq!(chats[0]["messageCount"], 1);
        assert_eq!(chats[0]["lastMessage"], "hello");
        assert_eq!(
            store.chat_workspace_id(chat_id).await.unwrap().as_deref(),
            Some(record.id.as_str())
        );
        assert_eq!(store.chat_affinity(chat_id).await.unwrap(), chat_id);

        let loaded = store.get_chat(chat_id).await.unwrap().unwrap();
        assert_eq!(loaded["messages"][0]["content"], "hello");
        assert!(store.delete_chat(chat_id).await.unwrap());
        assert!(store.list_chats(&record.id).await.is_empty());
    }

    #[tokio::test]
    async fn child_chat_keeps_parent_link_and_title() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChatStore::open(dir.path()).await.unwrap();
        let chat = store
            .create_child_chat("_general", "", "Delegated task", "mock", "parent-chat")
            .await
            .unwrap();
        let chat_id = chat["id"].as_str().unwrap();
        assert!(chat_id.starts_with("del-"));
        assert_eq!(chat["title"], "Delegated task");
        let loaded = store.get_chat(chat_id).await.unwrap().unwrap();
        assert_eq!(loaded["parentChatId"], "parent-chat");
        assert_eq!(loaded["title"], "Delegated task");
        assert_eq!(store.chat_affinity(chat_id).await.unwrap(), "parent-chat");
    }

    #[tokio::test]
    async fn compaction_entry_restores_summary_tail_and_later_messages() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChatStore::open(dir.path()).await.unwrap();
        for entry in [
            json!({"type": "message", "role": "user", "content": "old"}),
            json!({
                "type": "compaction",
                "summary": "old work summarized",
                "retained_tail": [
                    {"type": "message", "role": "user", "content": "recent"},
                    {"type": "message", "role": "assistant", "content": "answer"}
                ]
            }),
            json!({"type": "message", "role": "user", "content": "after"}),
        ] {
            store
                .append_session_entry_for("_general", dir.path(), "chat-compact", entry)
                .await
                .unwrap();
        }
        let entries = store
            .read_session_entries("_general", dir.path(), "chat-compact")
            .await
            .unwrap();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0]["role"], "system");
        assert!(
            entries[0]["content"]
                .as_str()
                .unwrap()
                .contains("old work summarized")
        );
        assert_eq!(entries[1]["content"], "recent");
        assert_eq!(entries[3]["content"], "after");
        assert!(!entries.iter().any(|entry| entry["content"] == "old"));
    }

    #[tokio::test]
    async fn opencode_checkpoint_restores_one_user_message() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChatStore::open(dir.path()).await.unwrap();
        for entry in [
            json!({"type": "message", "role": "user", "content": "old"}),
            json!({
                "type": "compaction",
                "checkpoint_format": "opencode-v1",
                "summary": "checkpoint summary",
                "retained_tail": [
                    {"type": "message", "role": "user", "content": "recent"},
                    {"type": "message", "role": "assistant", "content": "answer"}
                ]
            }),
            json!({"type": "message", "role": "user", "content": "after"}),
        ] {
            store
                .append_session_entry_for("_general", dir.path(), "chat-checkpoint", entry)
                .await
                .unwrap();
        }
        let entries = store
            .read_session_entries("_general", dir.path(), "chat-checkpoint")
            .await
            .unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["role"], "user");
        let content = entries[0]["content"].as_str().unwrap();
        assert!(content.starts_with("<conversation-checkpoint>"));
        assert!(content.contains("checkpoint summary"));
        assert!(content.contains("[User]: recent"));
        assert_eq!(entries[1]["content"], "after");
        assert!(!entries.iter().any(|entry| entry["content"] == "old"));
    }

    #[tokio::test]
    async fn chat_round_trip_and_session_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChatStore::open(dir.path()).await.unwrap();
        store
            .ensure_chat(
                "chat-1",
                "_general",
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
            .append_session_entry_for(
                "_general",
                dir.path(),
                "chat-1",
                json!({"type": "message", "role": "user", "content": "Hello"}),
            )
            .await
            .unwrap();
        let path = session_path_for(dir.path(), "_general", "chat-1");
        let raw = tokio::fs::read_to_string(path).await.unwrap();
        assert!(raw.contains("\"type\":\"session\""));
        assert!(raw.contains("\"role\":\"user\""));
    }
}
