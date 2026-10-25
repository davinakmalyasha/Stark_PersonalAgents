use std::path::Path;
use std::time::Duration;

use chrono::Utc;
use serde_json::{Value, json};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

use crate::StoreError;

#[derive(Clone, Debug)]
pub struct JobRecord {
    pub id: String,
    pub parent_chat_id: String,
    pub agent: String,
    pub chat_id: String,
    pub title: String,
    pub status: String,
    pub task: String,
    pub outcome: String,
    pub acceptance: String,
    pub report: String,
    pub error: Option<String>,
    pub created_at: i64,
    pub finished_at: Option<i64>,
}

#[derive(Clone)]
pub struct JobStore {
    pool: SqlitePool,
}

impl JobStore {
    pub async fn open(harness_root: &Path) -> Result<Self, StoreError> {
        let db_path = harness_root.join(".deep_agents").join("jobs.db");
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
            CREATE TABLE IF NOT EXISTS jobs (
                id TEXT PRIMARY KEY,
                parent_chat_id TEXT,
                project_id TEXT,
                item_id TEXT,
                agent TEXT NOT NULL DEFAULT '',
                chat_id TEXT,
                title TEXT,
                status TEXT NOT NULL,
                attempts INTEGER DEFAULT 0,
                lease_owner TEXT,
                lease_expires REAL,
                workdir TEXT,
                usage_json TEXT,
                result TEXT,
                error TEXT,
                artifact TEXT,
                created_at REAL NOT NULL,
                updated_at REAL NOT NULL,
                finished_at REAL,
                entry_json TEXT
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS ix_jobs_parent ON jobs(parent_chat_id, status)")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn upsert(&self, job: &JobRecord) -> Result<(), StoreError> {
        let now = Utc::now().timestamp_millis();
        let entry = json!({
            "task": job.task,
            "outcome": job.outcome,
            "acceptance": job.acceptance,
        });
        sqlx::query(
            r#"
            INSERT INTO jobs
                (id, parent_chat_id, agent, chat_id, title, status, result, error, created_at, updated_at, finished_at, entry_json)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                agent = excluded.agent,
                chat_id = excluded.chat_id,
                title = excluded.title,
                status = excluded.status,
                result = excluded.result,
                error = excluded.error,
                updated_at = excluded.updated_at,
                finished_at = excluded.finished_at,
                entry_json = excluded.entry_json
            "#,
        )
        .bind(&job.id)
        .bind(&job.parent_chat_id)
        .bind(&job.agent)
        .bind(&job.chat_id)
        .bind(&job.title)
        .bind(&job.status)
        .bind(&job.report)
        .bind(&job.error)
        .bind(job.created_at as f64)
        .bind(now as f64)
        .bind(job.finished_at.map(|value| value as f64))
        .bind(entry.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get(&self, id: &str) -> Result<Option<JobRecord>, StoreError> {
        let row = sqlx::query(
            "SELECT id, parent_chat_id, agent, chat_id, title, status, result, error, created_at, finished_at, entry_json FROM jobs WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(row_to_record))
    }

    pub async fn list(
        &self,
        parent_chat_id: &str,
        status: Option<&str>,
    ) -> Result<Vec<JobRecord>, StoreError> {
        let rows = if let Some(status) = status.filter(|status| !status.is_empty()) {
            sqlx::query(
                "SELECT id, parent_chat_id, agent, chat_id, title, status, result, error, created_at, finished_at, entry_json FROM jobs WHERE parent_chat_id = ? AND status = ? ORDER BY created_at ASC",
            )
            .bind(parent_chat_id)
            .bind(status)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT id, parent_chat_id, agent, chat_id, title, status, result, error, created_at, finished_at, entry_json FROM jobs WHERE parent_chat_id = ? ORDER BY created_at ASC",
            )
            .bind(parent_chat_id)
            .fetch_all(&self.pool)
            .await?
        };
        Ok(rows.into_iter().map(row_to_record).collect())
    }

    pub async fn fail_stale(&self) -> Result<(), StoreError> {
        let now = Utc::now().timestamp_millis() as f64;
        sqlx::query(
            "UPDATE jobs SET status = 'failed', error = ?, updated_at = ?, finished_at = ? WHERE status IN ('queued', 'running')",
        )
        .bind("server restarted before the job completed")
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

fn row_to_record(row: sqlx::sqlite::SqliteRow) -> JobRecord {
    let entry: Value = row
        .try_get::<Option<String>, _>("entry_json")
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(|| json!({}));
    JobRecord {
        id: row.get("id"),
        parent_chat_id: row
            .try_get::<Option<String>, _>("parent_chat_id")
            .ok()
            .flatten()
            .unwrap_or_default(),
        agent: row.get("agent"),
        chat_id: row
            .try_get::<Option<String>, _>("chat_id")
            .ok()
            .flatten()
            .unwrap_or_default(),
        title: row
            .try_get::<Option<String>, _>("title")
            .ok()
            .flatten()
            .unwrap_or_default(),
        status: row.get("status"),
        task: entry
            .get("task")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        outcome: entry
            .get("outcome")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        acceptance: entry
            .get("acceptance")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        report: row
            .try_get::<Option<String>, _>("result")
            .ok()
            .flatten()
            .unwrap_or_default(),
        error: row.try_get::<Option<String>, _>("error").ok().flatten(),
        created_at: row.try_get::<f64, _>("created_at").unwrap_or(0.0) as i64,
        finished_at: row
            .try_get::<Option<f64>, _>("finished_at")
            .ok()
            .flatten()
            .map(|value| value as i64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn job_records_round_trip_and_stale_jobs_fail() {
        let dir = tempfile::tempdir().unwrap();
        let store = JobStore::open(dir.path()).await.unwrap();
        let job = JobRecord {
            id: "del-1".to_string(),
            parent_chat_id: "parent".to_string(),
            agent: "Developer".to_string(),
            chat_id: "chat-1".to_string(),
            title: "Build".to_string(),
            status: "running".to_string(),
            task: "build it".to_string(),
            outcome: "works".to_string(),
            acceptance: "tests pass".to_string(),
            report: String::new(),
            error: None,
            created_at: 1,
            finished_at: None,
        };
        store.upsert(&job).await.unwrap();
        let loaded = store.get("del-1").await.unwrap().unwrap();
        assert_eq!(loaded.task, "build it");
        assert_eq!(loaded.outcome, "works");

        let listed = store.list("parent", None).await.unwrap();
        assert_eq!(listed.len(), 1);
        store.fail_stale().await.unwrap();
        let failed = store.get("del-1").await.unwrap().unwrap();
        assert_eq!(failed.status, "failed");
        assert!(failed.error.unwrap().contains("restarted"));
    }
}
