//! Durable agent registry and mailbox.
//!
//! The three levels share one loop but have different policies:
//! `orchestrator` coordinates, `workspace` agents own a workspace and may
//! spawn `subagent` children. Every spawn and message is persisted before a
//! run is started, so a restart can recover queued work and notify parents.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::StoreError;

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[derive(Debug, Clone)]
pub struct AgentRecord {
    pub id: String,
    pub kind: String,
    pub path: String,
    pub parent_id: Option<String>,
    pub chat_id: String,
    pub workspace_id: String,
    pub scope: String,
    pub role: String,
    pub nickname: String,
    pub status: String,
    pub model: String,
    pub depth: i64,
    pub created_at: String,
    pub updated_at: String,
    pub last_active: Option<String>,
    pub result: String,
    pub usage: Value,
    pub access: String,
    pub repo_root: String,
    pub workspace_subdir: String,
    pub worktree_path: String,
    pub worktree_branch: String,
    pub worktree_base: String,
    pub worktree_state: String,
    pub worktree_diff: Value,
    pub worktree_error: String,
}

fn row_agent(row: &sqlx::sqlite::SqliteRow) -> AgentRecord {
    let usage: String = row
        .try_get("usage_json")
        .unwrap_or_else(|_| "{}".to_string());
    let diff: String = row
        .try_get("worktree_diff_json")
        .unwrap_or_else(|_| "{}".to_string());
    AgentRecord {
        id: row.get("id"),
        kind: row.get("kind"),
        path: row.get("path"),
        parent_id: row.try_get("parent_id").ok().flatten(),
        chat_id: row.get("chat_id"),
        workspace_id: row.try_get("workspace_id").unwrap_or_default(),
        scope: row.try_get("scope").unwrap_or_default(),
        role: row.try_get("role").unwrap_or_default(),
        nickname: row.try_get("nickname").unwrap_or_default(),
        status: row.get("status"),
        model: row.try_get("model").unwrap_or_default(),
        depth: row.try_get("depth").unwrap_or(0),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        last_active: row.try_get("last_active").ok().flatten(),
        result: row.try_get("result").unwrap_or_default(),
        usage: serde_json::from_str(&usage).unwrap_or_else(|_| json!({})),
        access: row.try_get("access").unwrap_or_default(),
        repo_root: row.try_get("repo_root").unwrap_or_default(),
        workspace_subdir: row.try_get("workspace_subdir").unwrap_or_default(),
        worktree_path: row.try_get("worktree_path").unwrap_or_default(),
        worktree_branch: row.try_get("worktree_branch").unwrap_or_default(),
        worktree_base: row.try_get("worktree_base").unwrap_or_default(),
        worktree_state: row.try_get("worktree_state").unwrap_or_default(),
        worktree_diff: serde_json::from_str(&diff).unwrap_or_else(|_| json!({})),
        worktree_error: row.try_get("worktree_error").unwrap_or_default(),
    }
}

fn row_message(row: &sqlx::sqlite::SqliteRow) -> Value {
    let metadata: String = row
        .try_get("metadata_json")
        .unwrap_or_else(|_| "{}".to_string());
    json!({
        "id": row.get::<String, _>("id"),
        "senderId": row.try_get::<Option<String>, _>("sender_id").ok().flatten(),
        "recipientId": row.get::<String, _>("recipient_id"),
        "kind": row.get::<String, _>("kind"),
        "body": row.get::<String, _>("body"),
        "metadata": serde_json::from_str::<Value>(&metadata).unwrap_or_else(|_| json!({})),
        "delivery": row.get::<String, _>("delivery"),
        "createdAt": row.get::<String, _>("created_at"),
        "consumedAt": row.try_get::<Option<String>, _>("consumed_at").ok().flatten(),
    })
}

#[derive(Clone)]
pub struct AgentStore {
    pool: SqlitePool,
    root: PathBuf,
}

impl AgentRecord {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "kind": self.kind,
            "path": self.path,
            "parentId": self.parent_id,
            "chatId": self.chat_id,
            "workspaceId": self.workspace_id,
            "scope": self.scope,
            "role": self.role,
            "nickname": self.nickname,
            "status": self.status,
            "model": self.model,
            "depth": self.depth,
            "createdAt": self.created_at,
            "updatedAt": self.updated_at,
            "lastActive": self.last_active,
            "result": self.result,
            "usage": self.usage,
            "access": self.access,
            "repoRoot": self.repo_root,
            "workspaceSubdir": self.workspace_subdir,
            "worktree": {
                "path": self.worktree_path,
                "branch": self.worktree_branch,
                "base": self.worktree_base,
                "state": self.worktree_state,
                "diff": self.worktree_diff,
                "error": self.worktree_error,
            },
        })
    }
}

impl AgentStore {
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

    pub fn root(&self) -> &Path {
        &self.root
    }

    async fn init(&self) -> Result<(), StoreError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS agents (
                id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                path TEXT NOT NULL UNIQUE,
                parent_id TEXT,
                chat_id TEXT NOT NULL UNIQUE,
                workspace_id TEXT DEFAULT '',
                scope TEXT DEFAULT '',
                role TEXT DEFAULT '',
                nickname TEXT DEFAULT '',
                status TEXT NOT NULL DEFAULT 'idle',
                model TEXT DEFAULT '',
                depth INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                last_active TEXT,
                result TEXT DEFAULT '',
                usage_json TEXT DEFAULT '{}'
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        let _ = sqlx::query("ALTER TABLE agents ADD COLUMN result TEXT DEFAULT ''")
            .execute(&self.pool)
            .await;
        for column in [
            "ALTER TABLE agents ADD COLUMN access TEXT DEFAULT ''",
            "ALTER TABLE agents ADD COLUMN repo_root TEXT DEFAULT ''",
            "ALTER TABLE agents ADD COLUMN workspace_subdir TEXT DEFAULT ''",
            "ALTER TABLE agents ADD COLUMN worktree_path TEXT DEFAULT ''",
            "ALTER TABLE agents ADD COLUMN worktree_branch TEXT DEFAULT ''",
            "ALTER TABLE agents ADD COLUMN worktree_base TEXT DEFAULT ''",
            "ALTER TABLE agents ADD COLUMN worktree_state TEXT DEFAULT ''",
            "ALTER TABLE agents ADD COLUMN worktree_diff_json TEXT DEFAULT '{}'",
            "ALTER TABLE agents ADD COLUMN worktree_error TEXT DEFAULT ''",
        ] {
            let _ = sqlx::query(column).execute(&self.pool).await;
        }
        sqlx::query("CREATE INDEX IF NOT EXISTS ix_agents_parent ON agents(parent_id, status)")
            .execute(&self.pool)
            .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS ix_agents_workspace ON agents(workspace_id, scope)",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS agent_messages (
                id TEXT PRIMARY KEY,
                sender_id TEXT,
                recipient_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                body TEXT NOT NULL,
                metadata_json TEXT DEFAULT '{}',
                delivery TEXT NOT NULL DEFAULT 'trigger',
                created_at TEXT NOT NULL,
                consumed_at TEXT
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS ix_agent_messages_recipient ON agent_messages(recipient_id, consumed_at, created_at)",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_agent(
        &self,
        kind: &str,
        path: &str,
        parent_id: Option<&str>,
        chat_id: &str,
        workspace_id: &str,
        scope: &str,
        role: &str,
        nickname: &str,
        model: &str,
        depth: i64,
    ) -> Result<AgentRecord, StoreError> {
        let id = format!("agent-{}", &Uuid::new_v4().simple().to_string()[..12]);
        let moment = now();
        sqlx::query(
            r#"
            INSERT INTO agents
                (id, kind, path, parent_id, chat_id, workspace_id, scope, role,
                 nickname, status, model, depth, created_at, updated_at, usage_json)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'idle', ?, ?, ?, ?, '{}')
            "#,
        )
        .bind(&id)
        .bind(kind)
        .bind(path)
        .bind(parent_id)
        .bind(chat_id)
        .bind(workspace_id)
        .bind(scope)
        .bind(role)
        .bind(nickname)
        .bind(model)
        .bind(depth)
        .bind(&moment)
        .bind(&moment)
        .execute(&self.pool)
        .await?;
        self.get(&id).await?.ok_or_else(|| StoreError::Invalid(id))
    }

    pub async fn get(&self, id: &str) -> Result<Option<AgentRecord>, StoreError> {
        let row = sqlx::query("SELECT * FROM agents WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(row_agent))
    }

    pub async fn by_chat(&self, chat_id: &str) -> Result<Option<AgentRecord>, StoreError> {
        let row = sqlx::query("SELECT * FROM agents WHERE chat_id = ?")
            .bind(chat_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(row_agent))
    }

    pub async fn workspace_agent(
        &self,
        workspace_id: &str,
        scope: &str,
    ) -> Result<Option<AgentRecord>, StoreError> {
        let row = sqlx::query(
            "SELECT * FROM agents WHERE kind = 'workspace' AND workspace_id = ? AND scope = ? AND parent_id IS NOT NULL ORDER BY created_at ASC LIMIT 1",
        )
        .bind(workspace_id)
        .bind(scope)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.as_ref().map(row_agent))
    }

    pub async fn set_access(&self, id: &str, access: &str) -> Result<(), StoreError> {
        sqlx::query("UPDATE agents SET access = ?, updated_at = ? WHERE id = ?")
            .bind(access)
            .bind(now())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn set_worktree(
        &self,
        id: &str,
        repo_root: &str,
        subdir: &str,
        path: &str,
        branch: &str,
        base: &str,
        state: &str,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE agents SET repo_root = ?, workspace_subdir = ?, worktree_path = ?, worktree_branch = ?, worktree_base = ?, worktree_state = ?, updated_at = ? WHERE id = ?",
        )
        .bind(repo_root)
        .bind(subdir)
        .bind(path)
        .bind(branch)
        .bind(base)
        .bind(state)
        .bind(now())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn set_worktree_state(
        &self,
        id: &str,
        state: &str,
        diff: &Value,
        error: &str,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE agents SET worktree_state = ?, worktree_diff_json = ?, worktree_error = ?, updated_at = ? WHERE id = ?",
        )
        .bind(state)
        .bind(diff.to_string())
        .bind(error)
        .bind(now())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn pending_messages_count(&self, recipient_id: &str) -> Result<i64, StoreError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM agent_messages WHERE recipient_id = ? AND consumed_at IS NULL",
        )
        .bind(recipient_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(count)
    }

    pub async fn list(&self, parent_id: Option<&str>) -> Result<Vec<AgentRecord>, StoreError> {
        let rows = if let Some(parent_id) = parent_id {
            sqlx::query("SELECT * FROM agents WHERE parent_id = ? ORDER BY created_at ASC")
                .bind(parent_id)
                .fetch_all(&self.pool)
                .await?
        } else {
            sqlx::query("SELECT * FROM agents ORDER BY created_at ASC")
                .fetch_all(&self.pool)
                .await?
        };
        Ok(rows.iter().map(row_agent).collect())
    }

    pub async fn set_status(&self, id: &str, status: &str, active: bool) -> Result<(), StoreError> {
        let moment = now();
        sqlx::query("UPDATE agents SET status = ?, updated_at = ?, last_active = ? WHERE id = ?")
            .bind(status)
            .bind(&moment)
            .bind(if active { Some(moment.as_str()) } else { None })
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_usage(&self, id: &str, usage: &Value) -> Result<(), StoreError> {
        sqlx::query("UPDATE agents SET usage_json = ?, updated_at = ? WHERE id = ?")
            .bind(usage.to_string())
            .bind(now())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn enqueue(
        &self,
        sender_id: Option<&str>,
        recipient_id: &str,
        kind: &str,
        body: &str,
        delivery: &str,
        metadata: Value,
    ) -> Result<String, StoreError> {
        let id = format!("msg-{}", &Uuid::new_v4().simple().to_string()[..12]);
        sqlx::query(
            r#"
            INSERT INTO agent_messages
                (id, sender_id, recipient_id, kind, body, metadata_json, delivery, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&id)
        .bind(sender_id)
        .bind(recipient_id)
        .bind(kind)
        .bind(body)
        .bind(metadata.to_string())
        .bind(delivery)
        .bind(now())
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    pub async fn take_messages(
        &self,
        recipient_id: &str,
        limit: usize,
    ) -> Result<Vec<Value>, StoreError> {
        let rows = sqlx::query(
            "SELECT * FROM agent_messages WHERE recipient_id = ? AND consumed_at IS NULL ORDER BY created_at ASC LIMIT ?",
        )
        .bind(recipient_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        let mut messages = Vec::with_capacity(rows.len());
        for row in &rows {
            let id: String = row.get("id");
            sqlx::query("UPDATE agent_messages SET consumed_at = ? WHERE id = ?")
                .bind(now())
                .bind(&id)
                .execute(&self.pool)
                .await?;
            messages.push(row_message(row));
        }
        Ok(messages)
    }

    pub async fn pending_trigger(&self, limit: usize) -> Result<Vec<AgentRecord>, StoreError> {
        let rows = sqlx::query(
            r#"
            SELECT a.* FROM agents a
            WHERE a.status IN ('idle', 'queued')
              AND EXISTS (
                SELECT 1 FROM agent_messages m
                WHERE m.recipient_id = a.id AND m.consumed_at IS NULL AND m.delivery = 'trigger'
              )
            ORDER BY a.updated_at ASC
            LIMIT ?
            "#,
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(row_agent).collect())
    }

    pub async fn claim(&self, id: &str) -> Result<bool, StoreError> {
        let result = sqlx::query(
            "UPDATE agents SET status = 'running', updated_at = ?, last_active = ? WHERE id = ? AND status IN ('idle', 'queued')",
        )
        .bind(now())
        .bind(now())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn requeue(&self, id: &str) -> Result<(), StoreError> {
        sqlx::query("UPDATE agents SET status = 'queued', updated_at = ? WHERE id = ? AND status = 'running'")
            .bind(now())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn recover_running(&self) -> Result<u64, StoreError> {
        let result = sqlx::query(
            "UPDATE agents SET status = 'interrupted', updated_at = ? WHERE status = 'running'",
        )
        .bind(now())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn finish(
        &self,
        id: &str,
        status: &str,
        report: &str,
        usage: Value,
    ) -> Result<(), StoreError> {
        let moment = now();
        sqlx::query(
            "UPDATE agents SET status = ?, updated_at = ?, last_active = ?, result = ?, usage_json = ? WHERE id = ?",
        )
        .bind(status)
        .bind(&moment)
        .bind(&moment)
        .bind(report)
        .bind(usage.to_string())
        .bind(id)
        .execute(&self.pool)
        .await?;
        // A follow-up delivered while the agent was running must start a new
        // turn when the current one settles. Closed agents stay closed.
        sqlx::query(
            r#"
            UPDATE agents SET status = 'queued', updated_at = ?
            WHERE id = ? AND status IN ('completed', 'failed', 'interrupted')
              AND EXISTS (
                SELECT 1 FROM agent_messages
                WHERE recipient_id = ? AND consumed_at IS NULL AND delivery = 'trigger'
              )
            "#,
        )
        .bind(&moment)
        .bind(id)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn notify_parent(
        &self,
        child: &AgentRecord,
        report: &str,
        status: &str,
    ) -> Result<Option<String>, StoreError> {
        let Some(parent_id) = child.parent_id.as_deref() else {
            return Ok(None);
        };
        let Some(parent) = self.get(parent_id).await? else {
            return Ok(None);
        };
        let id = self
            .enqueue(
                Some(&child.id),
                &parent.id,
                "result",
                report,
                "trigger",
                json!({
                    "childId": child.id,
                    "childChatId": child.chat_id,
                    "childPath": child.path,
                    "status": status,
                }),
            )
            .await?;
        Ok(Some(id))
    }

    pub async fn messages_all(&self, recipient_id: &str) -> Result<Vec<Value>, StoreError> {
        let rows = sqlx::query(
            "SELECT * FROM agent_messages WHERE recipient_id = ? ORDER BY created_at ASC",
        )
        .bind(recipient_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(row_message).collect())
    }

    pub async fn consume_message(&self, id: &str) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE agent_messages SET consumed_at = ? WHERE id = ? AND consumed_at IS NULL",
        )
        .bind(now())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn wake(&self, id: &str) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE agents SET status = 'queued', updated_at = ? WHERE id = ? AND status IN ('idle','interrupted','completed','failed','cancelled')",
        )
        .bind(now())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn messages(&self, recipient_id: &str) -> Result<Vec<Value>, StoreError> {
        let rows = sqlx::query(
            "SELECT * FROM agent_messages WHERE recipient_id = ? AND consumed_at IS NULL ORDER BY created_at ASC",
        )
        .bind(recipient_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(row_message).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> (tempfile::TempDir, AgentStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = AgentStore::open(dir.path()).await.unwrap();
        (dir, store)
    }

    #[tokio::test]
    async fn agent_mailbox_claim_and_finish_flow() {
        let (_dir, store) = store().await;
        let orchestrator = store
            .create_agent(
                "orchestrator",
                "orchestrator",
                None,
                "chat-orch",
                "_general",
                "orchestrator",
                "Orchestrator",
                "Orch",
                "mock",
                0,
            )
            .await
            .unwrap();
        let child = store
            .create_agent(
                "workspace",
                "workspace:_general:developer",
                Some(&orchestrator.id),
                "chat-dev",
                "_general",
                "developer",
                "Developer",
                "Dev",
                "mock",
                1,
            )
            .await
            .unwrap();
        store
            .enqueue(
                Some(&orchestrator.id),
                &child.id,
                "spawn",
                "build it",
                "trigger",
                json!({"title": "Build"}),
            )
            .await
            .unwrap();

        let pending = store.pending_trigger(10).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].chat_id, "chat-dev");
        assert!(store.claim(&child.id).await.unwrap());
        assert_eq!(
            store.get(&child.id).await.unwrap().unwrap().status,
            "running"
        );
        let messages = store.take_messages(&child.id, 10).await.unwrap();
        assert_eq!(messages[0]["body"], "build it");
        assert!(store.take_messages(&child.id, 10).await.unwrap().is_empty());

        store
            .finish(&child.id, "completed", "done", json!({"total_tokens": 12}))
            .await
            .unwrap();
        store
            .notify_parent(&child, "done", "completed")
            .await
            .unwrap();
        let parent_messages = store.messages(&orchestrator.id).await.unwrap();
        assert_eq!(parent_messages[0]["kind"], "result");
        assert_eq!(parent_messages[0]["metadata"]["childId"], child.id);
    }

    #[tokio::test]
    async fn running_agents_recover_as_interrupted() {
        let (_dir, store) = store().await;
        let agent = store
            .create_agent(
                "workspace",
                "workspace:test:developer",
                None,
                "chat-test",
                "_general",
                "developer",
                "Developer",
                "Dev",
                "mock",
                1,
            )
            .await
            .unwrap();
        assert!(store.claim(&agent.id).await.unwrap());
        assert_eq!(store.recover_running().await.unwrap(), 1);
        assert_eq!(
            store.get(&agent.id).await.unwrap().unwrap().status,
            "interrupted"
        );
    }

    #[tokio::test]
    async fn workspace_agent_lookup_is_scoped() {
        let (_dir, store) = store().await;
        let parent = store
            .create_agent(
                "orchestrator",
                "orchestrator:test",
                None,
                "chat-orch",
                "proj",
                "orchestrator",
                "Orchestrator",
                "",
                "mock",
                0,
            )
            .await
            .unwrap();
        // Root workspace agents (no parent) are never reused by delegation.
        store
            .create_agent(
                "workspace",
                "workspace:proj:developer",
                None,
                "chat-proj-dev",
                "proj",
                "developer",
                "Developer",
                "Dev",
                "mock",
                1,
            )
            .await
            .unwrap();
        assert!(
            store
                .workspace_agent("proj", "developer")
                .await
                .unwrap()
                .is_none()
        );
        store
            .create_agent(
                "workspace",
                "workspace:proj:developer:delegated",
                Some(&parent.id),
                "chat-proj-dev-2",
                "proj",
                "developer",
                "Developer",
                "Dev",
                "mock",
                1,
            )
            .await
            .unwrap();
        assert!(
            store
                .workspace_agent("proj", "developer")
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .workspace_agent("proj", "general")
                .await
                .unwrap()
                .is_none()
        );
    }
}
