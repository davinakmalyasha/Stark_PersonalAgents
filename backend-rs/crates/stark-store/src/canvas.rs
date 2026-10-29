//! Durable canvas watch state and wake queue.
//!
//! Canvas wake tables in `.deep_agents/chats.db`: `pending_wakes`,
//! `canvas_watches`, `canvas_events`.

use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::StoreError;

#[derive(Clone)]
pub struct CanvasStore {
    pool: SqlitePool,
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn row_wake(row: &sqlx::sqlite::SqliteRow) -> Value {
    let metadata: String = row
        .try_get("metadata_json")
        .unwrap_or_else(|_| "{}".to_string());
    json!({
        "id": row.get::<String, _>("id"),
        "workspacePath": row.get::<String, _>("workspace_path"),
        "chatId": row.get::<String, _>("chat_id"),
        "text": row.get::<String, _>("text"),
        "metadata": serde_json::from_str::<Value>(&metadata).unwrap_or_else(|_| json!({})),
        "mode": row.get::<String, _>("mode"),
        "status": row.get::<String, _>("status"),
        "attempts": row.get::<i64, _>("attempts"),
        "notBefore": row.get::<String, _>("not_before"),
        "createdAt": row.get::<String, _>("created_at"),
        "updatedAt": row.get::<String, _>("updated_at"),
    })
}

fn row_watch(row: &sqlx::sqlite::SqliteRow) -> Value {
    let targets: String = row
        .try_get("targets_json")
        .unwrap_or_else(|_| "[]".to_string());
    let wake_on: String = row
        .try_get("wake_on_json")
        .unwrap_or_else(|_| "[]".to_string());
    json!({
        "id": row.get::<String, _>("id"),
        "orchestratorChatId": row.get::<String, _>("orchestrator_chat_id"),
        "workspacePath": row.get::<String, _>("workspace_path"),
        "mode": row.get::<String, _>("mode"),
        "targets": serde_json::from_str::<Value>(&targets).unwrap_or_else(|_| json!([])),
        "wakeOn": serde_json::from_str::<Value>(&wake_on).unwrap_or_else(|_| json!([])),
        "maxActions": row.get::<i64, _>("max_actions"),
        "actionsUsed": row.get::<i64, _>("actions_used"),
        "until": row.try_get::<Option<String>, _>("until").ok().flatten(),
        "status": row.get::<String, _>("status"),
        "createdAt": row.get::<String, _>("created_at"),
        "updatedAt": row.get::<String, _>("updated_at"),
    })
}

impl CanvasStore {
    pub async fn open(harness_root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = harness_root.into();
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
        let store = Self { pool };
        store.init().await?;
        Ok(store)
    }

    async fn init(&self) -> Result<(), StoreError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS pending_wakes (
                id TEXT PRIMARY KEY,
                workspace_path TEXT DEFAULT '',
                chat_id TEXT NOT NULL,
                text TEXT NOT NULL,
                metadata_json TEXT DEFAULT '{}',
                mode TEXT DEFAULT 'orchestrator',
                status TEXT DEFAULT 'queued',
                attempts INTEGER DEFAULT 0,
                not_before TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_pending_wakes_due ON pending_wakes(status, not_before)",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS canvas_watches (
                id TEXT PRIMARY KEY,
                orchestrator_chat_id TEXT NOT NULL,
                workspace_path TEXT DEFAULT '',
                mode TEXT DEFAULT 'all',
                targets_json TEXT DEFAULT '[]',
                wake_on_json TEXT DEFAULT '["completed","failed"]',
                max_actions INTEGER DEFAULT 20,
                actions_used INTEGER DEFAULT 0,
                until TEXT,
                status TEXT DEFAULT 'active',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_canvas_watches_chat ON canvas_watches(orchestrator_chat_id, status)",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS canvas_events (
                id TEXT PRIMARY KEY,
                watch_id TEXT NOT NULL,
                source_chat_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                payload_json TEXT DEFAULT '{}',
                created_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_canvas_events_watch ON canvas_events(watch_id, created_at)",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn enqueue_wake(
        &self,
        workspace_path: &str,
        chat_id: &str,
        text: &str,
        metadata: Value,
        mode: &str,
    ) -> Result<Value, StoreError> {
        let moment = now();
        let id = format!("wake-{}", &Uuid::new_v4().simple().to_string()[..10]);
        sqlx::query(
            "INSERT INTO pending_wakes (id, workspace_path, chat_id, text, metadata_json, mode, status, attempts, not_before, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, 'queued', 0, ?, ?, ?)",
        )
        .bind(&id)
        .bind(workspace_path)
        .bind(chat_id)
        .bind(text)
        .bind(metadata.to_string())
        .bind(mode)
        .bind(&moment)
        .bind(&moment)
        .bind(&moment)
        .execute(&self.pool)
        .await?;
        Ok(json!({
            "id": id,
            "chatId": chat_id,
            "workspacePath": workspace_path,
            "text": text,
            "mode": mode,
            "status": "queued",
        }))
    }

    pub async fn due_wakes(&self, limit: usize) -> Result<Vec<Value>, StoreError> {
        let moment = now();
        let rows = sqlx::query(
            "SELECT id, workspace_path, chat_id, text, metadata_json, mode, status, attempts, not_before, created_at, updated_at FROM pending_wakes WHERE status = 'queued' AND not_before <= ? ORDER BY created_at ASC LIMIT ?",
        )
        .bind(&moment)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(row_wake).collect())
    }

    pub async fn mark_wake(&self, id: &str, status: &str) -> Result<(), StoreError> {
        sqlx::query("UPDATE pending_wakes SET status = ?, updated_at = ? WHERE id = ?")
            .bind(status)
            .bind(now())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn recover_dispatching(&self) -> Result<u64, StoreError> {
        let result = sqlx::query(
            "UPDATE pending_wakes SET status = 'queued', updated_at = ? WHERE status = 'dispatching'",
        )
        .bind(now())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn enter_watch(
        &self,
        orchestrator_chat_id: &str,
        workspace_path: &str,
        mode: &str,
        targets: &[String],
        wake_on: &[String],
        max_actions: i64,
        until: Option<String>,
    ) -> Result<Value, StoreError> {
        let moment = now();
        sqlx::query(
            "UPDATE canvas_watches SET status = 'closed', updated_at = ? WHERE orchestrator_chat_id = ? AND status = 'active'",
        )
        .bind(&moment)
        .bind(orchestrator_chat_id)
        .execute(&self.pool)
        .await?;
        let id = format!("watch-{}", &Uuid::new_v4().simple().to_string()[..10]);
        sqlx::query(
            "INSERT INTO canvas_watches (id, orchestrator_chat_id, workspace_path, mode, targets_json, wake_on_json, max_actions, actions_used, until, status, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?, 'active', ?, ?)",
        )
        .bind(&id)
        .bind(orchestrator_chat_id)
        .bind(workspace_path)
        .bind(mode)
        .bind(json!(targets).to_string())
        .bind(json!(wake_on).to_string())
        .bind(max_actions.max(1))
        .bind(until)
        .bind(&moment)
        .bind(&moment)
        .execute(&self.pool)
        .await?;
        self.get_watch(&id)
            .await?
            .ok_or_else(|| StoreError::Invalid(id))
    }

    pub async fn get_watch(&self, id: &str) -> Result<Option<Value>, StoreError> {
        let row = sqlx::query(
            "SELECT id, orchestrator_chat_id, workspace_path, mode, targets_json, wake_on_json, max_actions, actions_used, until, status, created_at, updated_at FROM canvas_watches WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.as_ref().map(row_watch))
    }

    pub async fn active_watch(
        &self,
        orchestrator_chat_id: &str,
    ) -> Result<Option<Value>, StoreError> {
        let row = sqlx::query(
            "SELECT id, orchestrator_chat_id, workspace_path, mode, targets_json, wake_on_json, max_actions, actions_used, until, status, created_at, updated_at FROM canvas_watches WHERE orchestrator_chat_id = ? AND status = 'active' ORDER BY created_at DESC LIMIT 1",
        )
        .bind(orchestrator_chat_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let watch = row_watch(&row);
        if let Some(until) = watch["until"].as_str()
            && parse_time(until).is_some_and(|deadline| deadline <= Utc::now())
        {
            if let Some(id) = watch["id"].as_str() {
                self.close_watch(id).await?;
            }
            return Ok(None);
        }
        Ok(Some(watch))
    }

    pub async fn active_watches(&self) -> Result<Vec<Value>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, orchestrator_chat_id, workspace_path, mode, targets_json, wake_on_json, max_actions, actions_used, until, status, created_at, updated_at FROM canvas_watches WHERE status = 'active' ORDER BY created_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(row_watch).collect())
    }

    pub async fn close_watch(&self, id: &str) -> Result<(), StoreError> {
        sqlx::query("UPDATE canvas_watches SET status = 'closed', updated_at = ? WHERE id = ?")
            .bind(now())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn close_watch_for_chat(&self, chat_id: &str) -> Result<u64, StoreError> {
        let result = sqlx::query(
            "UPDATE canvas_watches SET status = 'closed', updated_at = ? WHERE orchestrator_chat_id = ? AND status = 'active'",
        )
        .bind(now())
        .bind(chat_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn bump_watch(&self, id: &str) -> Result<i64, StoreError> {
        sqlx::query("UPDATE canvas_watches SET actions_used = actions_used + 1, updated_at = ? WHERE id = ?")
            .bind(now())
            .bind(id)
            .execute(&self.pool)
            .await?;
        let used: i64 = sqlx::query_scalar("SELECT actions_used FROM canvas_watches WHERE id = ?")
            .bind(id)
            .fetch_one(&self.pool)
            .await?;
        Ok(used)
    }

    pub async fn record_event(
        &self,
        watch_id: &str,
        source_chat_id: &str,
        kind: &str,
        payload: Value,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO canvas_events (id, watch_id, source_chat_id, kind, payload_json, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(watch_id)
        .bind(source_chat_id)
        .bind(kind)
        .bind(payload.to_string())
        .bind(now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn events(&self, watch_id: &str, limit: usize) -> Result<Vec<Value>, StoreError> {
        let rows = sqlx::query(
            "SELECT source_chat_id, kind, payload_json, created_at FROM canvas_events WHERE watch_id = ? ORDER BY created_at DESC LIMIT ?",
        )
        .bind(watch_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|row| {
                let payload: String = row
                    .try_get("payload_json")
                    .unwrap_or_else(|_| "{}".to_string());
                json!({
                    "sourceChatId": row.get::<String, _>("source_chat_id"),
                    "kind": row.get::<String, _>("kind"),
                    "payload": serde_json::from_str::<Value>(&payload).unwrap_or_else(|_| json!({})),
                    "createdAt": row.get::<String, _>("created_at"),
                })
            })
            .collect())
    }

    /// Does this watch include `source_chat_id`? `mode=all` means every agent
    /// chat; single/multiple use the explicit target list (chat or group ids).
    pub fn watch_matches(watch: &Value, source_chat_id: &str) -> bool {
        if source_chat_id.is_empty() || watch["orchestratorChatId"].as_str() == Some(source_chat_id)
        {
            return false;
        }
        if watch["mode"].as_str() == Some("all") {
            return true;
        }
        watch["targets"].as_array().is_some_and(|targets| {
            targets.iter().any(|target| {
                let target = target.as_str().unwrap_or("");
                target == source_chat_id
                    || target
                        .strip_prefix("group:")
                        .is_some_and(|group| source_chat_id.starts_with(&format!("gp-{group}-")))
            })
        })
    }

    pub fn wake_on_includes(watch: &Value, status: &str) -> bool {
        watch["wakeOn"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(status)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> (tempfile::TempDir, CanvasStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = CanvasStore::open(dir.path()).await.unwrap();
        (dir, store)
    }

    #[tokio::test]
    async fn watch_lifecycle_and_snapshot_fields() {
        let (_dir, store) = store().await;
        let watch = store
            .enter_watch(
                "orch",
                "",
                "multiple",
                &["chat-a".to_string(), "group:g1".to_string()],
                &["completed".to_string()],
                5,
                None,
            )
            .await
            .unwrap();
        assert_eq!(watch["mode"], "multiple");
        assert_eq!(watch["targets"][0], "chat-a");
        assert_eq!(watch["actionsUsed"], 0);
        let active = store.active_watch("orch").await.unwrap().unwrap();
        assert_eq!(active["id"], watch["id"]);
        assert!(CanvasStore::watch_matches(&active, "chat-a"));
        assert!(!CanvasStore::watch_matches(&active, "chat-b"));
        assert!(CanvasStore::wake_on_includes(&active, "completed"));
        assert!(!CanvasStore::wake_on_includes(&active, "failed"));
        assert_eq!(store.close_watch_for_chat("orch").await.unwrap(), 1);
        assert!(store.active_watch("orch").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn wake_queue_is_due_and_recoverable() {
        let (_dir, store) = store().await;
        let wake = store
            .enqueue_wake(
                "",
                "orch",
                "[CANVAS EVENT] done",
                json!({"x": 1}),
                "orchestrator",
            )
            .await
            .unwrap();
        let id = wake["id"].as_str().unwrap();
        let due = store.due_wakes(10).await.unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0]["chatId"], "orch");
        assert_eq!(due[0]["metadata"]["x"], 1);
        store.mark_wake(id, "dispatching").await.unwrap();
        assert!(store.due_wakes(10).await.unwrap().is_empty());
        assert_eq!(store.recover_dispatching().await.unwrap(), 1);
        assert_eq!(store.due_wakes(10).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn events_are_recorded_and_budget_bumps() {
        let (_dir, store) = store().await;
        let watch = store
            .enter_watch("orch", "", "all", &[], &["completed".to_string()], 2, None)
            .await
            .unwrap();
        let id = watch["id"].as_str().unwrap();
        store
            .record_event(id, "chat-a", "completed", json!({"report": "ok"}))
            .await
            .unwrap();
        let events = store.events(id, 10).await.unwrap();
        assert_eq!(events[0]["sourceChatId"], "chat-a");
        assert_eq!(events[0]["kind"], "completed");
        assert_eq!(store.bump_watch(id).await.unwrap(), 1);
        assert_eq!(store.bump_watch(id).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn expired_watch_is_closed() {
        let (_dir, store) = store().await;
        store
            .enter_watch(
                "orch",
                "",
                "all",
                &[],
                &["completed".to_string()],
                5,
                Some("2000-01-01T00:00:00Z".to_string()),
            )
            .await
            .unwrap();
        assert!(store.active_watch("orch").await.unwrap().is_none());
    }
}
