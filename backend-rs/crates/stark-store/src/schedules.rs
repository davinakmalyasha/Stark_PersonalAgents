use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Datelike, Local, TimeZone, Timelike, Utc};
use serde_json::{Value, json};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::StoreError;

#[derive(Clone, Debug)]
pub struct ScheduleRecord {
    pub id: String,
    pub chat_id: String,
    pub workspace_id: String,
    pub workspace_path: String,
    pub scope: String,
    pub role: Option<String>,
    pub model: String,
    pub temperature: f64,
    pub name: String,
    pub prompt: String,
    pub next_run_at: String,
    pub recurrence: String,
    pub status: String,
}

#[derive(Clone)]
pub struct ScheduleStore {
    pool: SqlitePool,
}

impl ScheduleStore {
    pub async fn open(harness_root: &Path) -> Result<Self, StoreError> {
        let path = harness_root.join(".deep_agents").join("schedules.db");
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let options = SqliteConnectOptions::new()
            .filename(&path)
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
            CREATE TABLE IF NOT EXISTS orchestrator_schedules (
                id TEXT PRIMARY KEY,
                chat_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL,
                workspace_path TEXT NOT NULL,
                scope TEXT NOT NULL,
                role TEXT,
                model TEXT NOT NULL,
                temperature REAL NOT NULL,
                name TEXT NOT NULL,
                prompt TEXT NOT NULL,
                run_at TEXT NOT NULL,
                next_run_at TEXT NOT NULL,
                recurrence TEXT NOT NULL,
                status TEXT NOT NULL,
                last_run_at TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS ix_schedules_due ON orchestrator_schedules(status, next_run_at)",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn create(&self, mut record: ScheduleRecord) -> Result<ScheduleRecord, StoreError> {
        let now = Utc::now().to_rfc3339();
        record.id = format!("sched-{}", &Uuid::new_v4().simple().to_string()[..10]);
        record.status = "scheduled".to_string();
        sqlx::query(
            r#"
            INSERT INTO orchestrator_schedules
                (id, chat_id, workspace_id, workspace_path, scope, role, model, temperature,
                 name, prompt, run_at, next_run_at, recurrence, status, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&record.id)
        .bind(&record.chat_id)
        .bind(&record.workspace_id)
        .bind(&record.workspace_path)
        .bind(&record.scope)
        .bind(&record.role)
        .bind(&record.model)
        .bind(record.temperature)
        .bind(&record.name)
        .bind(&record.prompt)
        .bind(&record.next_run_at)
        .bind(&record.next_run_at)
        .bind(&record.recurrence)
        .bind(&record.status)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(record)
    }

    pub async fn list(&self, chat_id: &str) -> Result<Vec<ScheduleRecord>, StoreError> {
        let rows = if chat_id.is_empty() {
            sqlx::query(
                "SELECT * FROM orchestrator_schedules WHERE status IN ('scheduled','queued','running') ORDER BY next_run_at ASC",
            )
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT * FROM orchestrator_schedules WHERE chat_id = ? AND status IN ('scheduled','queued','running') ORDER BY next_run_at ASC",
            )
            .bind(chat_id)
            .fetch_all(&self.pool)
            .await?
        };
        Ok(rows.into_iter().map(row_to_record).collect())
    }

    pub async fn due(&self, limit: usize) -> Result<Vec<ScheduleRecord>, StoreError> {
        let now = Utc::now().to_rfc3339();
        let rows = sqlx::query(
            "SELECT * FROM orchestrator_schedules WHERE status = 'scheduled' AND next_run_at <= ? ORDER BY next_run_at ASC LIMIT ?",
        )
        .bind(now)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(row_to_record).collect())
    }

    pub async fn cancel(&self, schedule_id: &str) -> Result<bool, StoreError> {
        let result = sqlx::query(
            "UPDATE orchestrator_schedules SET status = 'cancelled', updated_at = ? WHERE id = ?",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(schedule_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn mark_queued(&self, schedule_id: &str) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE orchestrator_schedules SET status = 'queued', updated_at = ? WHERE id = ?",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(schedule_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_after_run(&self, record: &ScheduleRecord) -> Result<(), StoreError> {
        let now = Utc::now();
        let next = next_after(now, &record.recurrence);
        let (status, next_run) = match next {
            Some(next) => ("scheduled", next.to_rfc3339()),
            None => ("completed", record.next_run_at.clone()),
        };
        sqlx::query(
            "UPDATE orchestrator_schedules SET status = ?, last_run_at = ?, next_run_at = ?, updated_at = ? WHERE id = ?",
        )
        .bind(status)
        .bind(now.to_rfc3339())
        .bind(next_run)
        .bind(now.to_rfc3339())
        .bind(&record.id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

pub fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let clock = value
        .trim_end_matches(|character: char| character.is_ascii_alphabetic())
        .trim();
    if let Some((hour, minute)) = clock.split_once(':')
        && let (Ok(hour), Ok(minute)) = (hour.trim().parse::<u32>(), minute.trim().parse::<u32>())
        && hour < 24
        && minute < 60
    {
        let now = Local::now();
        let mut local = now
            .date_naive()
            .and_hms_opt(hour, minute, 0)
            .and_then(|naive| Local.from_local_datetime(&naive).single())?;
        if local <= now {
            local += chrono::Duration::days(1);
        }
        return Some(local.with_timezone(&Utc));
    }
    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Some(parsed.with_timezone(&Utc));
    }
    if let Ok(parsed) = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S") {
        return Some(
            Local
                .from_local_datetime(&parsed)
                .single()?
                .with_timezone(&Utc),
        );
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        let naive = date.and_hms_opt(9, 0, 0)?;
        return Some(
            Local
                .from_local_datetime(&naive)
                .single()?
                .with_timezone(&Utc),
        );
    }
    None
}

fn next_after(now: DateTime<Utc>, recurrence: &str) -> Option<DateTime<Utc>> {
    let recurrence = recurrence.trim().to_lowercase();
    match recurrence.as_str() {
        "" | "none" | "once" => None,
        "daily" => Some(now + chrono::Duration::days(1)),
        "weekly" => Some(now + chrono::Duration::days(7)),
        "monthly" => {
            let month = now.month() % 12 + 1;
            let year = now.year() + i32::from(now.month() == 12);
            Some(
                Utc.with_ymd_and_hms(year, month, now.day().min(28), now.hour(), now.minute(), 0)
                    .single()
                    .unwrap_or(now + chrono::Duration::days(30)),
            )
        }
        _ if recurrence.starts_with("interval-") => recurrence
            .trim_start_matches("interval-")
            .parse::<i64>()
            .ok()
            .filter(|minutes| *minutes > 0)
            .map(|minutes| now + chrono::Duration::minutes(minutes)),
        _ => None,
    }
}

fn row_to_record(row: sqlx::sqlite::SqliteRow) -> ScheduleRecord {
    ScheduleRecord {
        id: row.get("id"),
        chat_id: row.get("chat_id"),
        workspace_id: row.get("workspace_id"),
        workspace_path: row.get("workspace_path"),
        scope: row.get("scope"),
        role: row.try_get::<Option<String>, _>("role").ok().flatten(),
        model: row.get("model"),
        temperature: row.try_get::<f64, _>("temperature").unwrap_or(0.0),
        name: row.get("name"),
        prompt: row.get("prompt"),
        next_run_at: row.get("next_run_at"),
        recurrence: row.get("recurrence"),
        status: row.get("status"),
    }
}

pub fn record_json(record: &ScheduleRecord) -> Value {
    json!({
        "id": record.id,
        "name": record.name,
        "prompt": record.prompt,
        "chatId": record.chat_id,
        "workspaceId": record.workspace_id,
        "workspacePath": record.workspace_path,
        "nextRunAt": record.next_run_at,
        "recurrence": record.recurrence,
        "status": record.status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn schedules_round_trip_and_recur() {
        let dir = tempfile::tempdir().unwrap();
        let store = ScheduleStore::open(dir.path()).await.unwrap();
        let at = (Utc::now() - chrono::Duration::seconds(1)).to_rfc3339();
        let record = store
            .create(ScheduleRecord {
                id: String::new(),
                chat_id: "chat".to_string(),
                workspace_id: "_general".to_string(),
                workspace_path: String::new(),
                scope: "orchestrator".to_string(),
                role: Some("Orchestrator".to_string()),
                model: "mock".to_string(),
                temperature: 0.0,
                name: "check".to_string(),
                prompt: "check now".to_string(),
                next_run_at: at,
                recurrence: "daily".to_string(),
                status: String::new(),
            })
            .await
            .unwrap();
        assert!(record.id.starts_with("sched-"));
        assert_eq!(store.due(10).await.unwrap().len(), 1);
        store.mark_queued(&record.id).await.unwrap();
        assert!(store.due(10).await.unwrap().is_empty());
        store.mark_after_run(&record).await.unwrap();
        assert_eq!(store.list("chat").await.unwrap()[0].status, "scheduled");
        store.cancel(&record.id).await.unwrap();
        assert!(store.list("chat").await.unwrap().is_empty());
    }

    #[test]
    fn parses_clock_times() {
        assert!(parse_time("09:30").is_some());
        assert!(parse_time("2026-01-02T03:04:05Z").is_some());
    }
}
