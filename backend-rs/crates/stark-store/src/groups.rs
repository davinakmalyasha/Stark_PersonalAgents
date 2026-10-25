use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use serde_json::{Value, json};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

#[derive(Clone, Debug)]
pub struct GroupStore {
    root: PathBuf,
}

impl GroupStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn default_root() -> Self {
        Self::new(
            std::env::var("STARK_PROJECTS_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from(r"D:\StarkProjects")),
        )
    }

    pub async fn create(
        &self,
        manifest: &Value,
        origin_workspace_id: &str,
    ) -> Result<Value, String> {
        let goal = manifest
            .get("goal")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if goal.is_empty() {
            return Err("goal is required".to_string());
        }
        let milestones = manifest
            .get("milestones")
            .and_then(Value::as_array)
            .filter(|milestones| !milestones.is_empty())
            .ok_or_else(|| "at least one milestone is required".to_string())?;
        let id = manifest
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| slug(goal));
        validate_id(&id)?;
        let path = self.root.join(&id);
        if path.join("state.db").exists() {
            return Err(format!("project '{id}' already exists"));
        }
        tokio::fs::create_dir_all(&path)
            .await
            .map_err(|error| error.to_string())?;
        tokio::fs::write(
            path.join("PROJECT.md"),
            format!("# {goal}\n\nid: {id}\n\nstatus: active\n"),
        )
        .await
        .map_err(|error| error.to_string())?;

        let pool = open_pool(&path, true).await?;
        let now = now();
        let status = manifest
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("active");
        sqlx::query(
            "INSERT INTO project (id, goal, status, origin_workspace_id, budget_usd, deadline, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(goal)
        .bind(status)
        .bind(origin_workspace_id)
        .bind(manifest.get("budget_usd").and_then(Value::as_f64))
        .bind(manifest.get("deadline").and_then(Value::as_str))
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .map_err(|error| error.to_string())?;

        for (position, milestone) in milestones.iter().enumerate() {
            let milestone_id = milestone
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("ms-{position}"));
            validate_id(&milestone_id)?;
            sqlx::query(
                "INSERT INTO milestones (id, project_id, title, status, position, depends_on_json) VALUES (?, ?, ?, 'pending', ?, ?)",
            )
            .bind(&milestone_id)
            .bind(&id)
            .bind(milestone.get("title").and_then(Value::as_str).unwrap_or(&milestone_id))
            .bind(position as i64)
            .bind(milestone.get("depends_on").cloned().unwrap_or_else(|| json!([])).to_string())
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
            if let Some(items) = milestone.get("items").and_then(Value::as_array) {
                for (index, item) in items.iter().enumerate() {
                    let item_id = item
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("{milestone_id}-item-{index}"));
                    validate_id(&item_id)?;
                    sqlx::query(
                        "INSERT INTO items (id, project_id, milestone_id, title, detail, status, acceptance_json, updated_at) VALUES (?, ?, ?, ?, ?, 'pending', ?, ?)",
                    )
                    .bind(&item_id)
                    .bind(&id)
                    .bind(&milestone_id)
                    .bind(item.get("title").and_then(Value::as_str).unwrap_or(""))
                    .bind(item.get("detail").and_then(Value::as_str).unwrap_or(""))
                    .bind(item.get("acceptance").cloned().unwrap_or_else(|| json!([])).to_string())
                    .bind(now)
                    .execute(&pool)
                    .await
                    .map_err(|error| error.to_string())?;
                }
            }
        }
        append_event(
            &pool,
            &id,
            "project_created",
            "orchestrator",
            None,
            None,
            json!({"goal": goal}),
        )
        .await?;
        self.detail(&id, 60).await
    }

    pub async fn list(&self) -> Vec<Value> {
        let Ok(mut entries) = tokio::fs::read_dir(&self.root).await else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            if !entry.path().is_dir() || !entry.path().join("state.db").is_file() {
                continue;
            }
            let Some(id) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let Ok(pool) = open_pool(&entry.path(), false).await else {
                continue;
            };
            let Ok(Some(project)) = sqlx::query(
                "SELECT id, goal, status, origin_workspace_id, budget_usd FROM project WHERE id = ?",
            )
            .bind(&id)
            .fetch_optional(&pool)
            .await
            else {
                continue;
            };
            let milestones_total: i64 = count(&pool, "milestones", &id).await;
            let milestones_done: i64 =
                count_where(&pool, "milestones", &id, "status = 'done'").await;
            let items_total: i64 = count(&pool, "items", &id).await;
            let items_done: i64 = count_where(&pool, "items", &id, "status = 'done'").await;
            let awaiting: i64 =
                count_where(&pool, "items", &id, "status = 'awaiting_approval'").await;
            rows.push(json!({
                "id": project.get::<String, _>("id"),
                "path": entry.path(),
                "title": project.get::<String, _>("goal"),
                "status": project.get::<String, _>("status"),
                "origin_workspace_id": project.try_get::<Option<String>, _>("origin_workspace_id").ok().flatten(),
                "shell_enabled": false,
                "spent_usd": null,
                "milestones_done": milestones_done,
                "milestones_total": milestones_total,
                "items_done": items_done,
                "items_total": items_total,
                "awaiting_approval": awaiting,
            }));
        }
        rows.sort_by(|left, right| {
            left["title"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .cmp(&right["title"].as_str().unwrap_or("").to_lowercase())
        });
        rows
    }

    pub async fn detail(&self, project_id: &str, event_limit: usize) -> Result<Value, String> {
        validate_id(project_id)?;
        let path = self.root.join(project_id);
        if !path.is_dir() {
            return Err(format!("unknown project '{project_id}'"));
        }
        let pool = open_pool(&path, false).await?;
        let project = sqlx::query(
            "SELECT id, goal, status, origin_workspace_id, budget_usd, deadline, halt_reason FROM project WHERE id = ?",
        )
        .bind(project_id)
        .fetch_optional(&pool)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("unknown project '{project_id}'"))?;
        let milestones = sqlx::query(
            "SELECT id, project_id, title, status, depends_on_json FROM milestones WHERE project_id = ? ORDER BY position ASC, id ASC",
        )
        .bind(project_id)
        .fetch_all(&pool)
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|row| {
            json!({
                "id": row.get::<String, _>("id"),
                "project_id": row.get::<String, _>("project_id"),
                "title": row.get::<String, _>("title"),
                "status": row.get::<String, _>("status"),
                "depends_on": parse_json(row.get::<String, _>("depends_on_json")),
            })
        })
        .collect::<Vec<_>>();
        let items = sqlx::query(
            "SELECT id, project_id, milestone_id, title, detail, status, acceptance_json, result, error, updated_at FROM items WHERE project_id = ? ORDER BY milestone_id ASC, id ASC",
        )
        .bind(project_id)
        .fetch_all(&pool)
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|row| {
            json!({
                "id": row.get::<String, _>("id"),
                "project_id": row.get::<String, _>("project_id"),
                "milestone_id": row.get::<String, _>("milestone_id"),
                "title": row.get::<String, _>("title"),
                "detail": row.get::<String, _>("detail"),
                "status": row.get::<String, _>("status"),
                "acceptance": parse_json(row.get::<String, _>("acceptance_json")),
                "result": row.try_get::<Option<String>, _>("result").ok().flatten(),
                "error": row.try_get::<Option<String>, _>("error").ok().flatten(),
                "updated_at": row.get::<f64, _>("updated_at"),
            })
        })
        .collect::<Vec<_>>();
        let events = events_for(&pool, project_id, 0, event_limit).await?;
        Ok(json!({
            "id": project.get::<String, _>("id"),
            "path": path,
            "shell_enabled": false,
            "project": {
                "goal": project.get::<String, _>("goal"),
                "status": project.get::<String, _>("status"),
                "origin_workspace_id": project.try_get::<Option<String>, _>("origin_workspace_id").ok().flatten(),
                "budget_usd": project.try_get::<Option<f64>, _>("budget_usd").ok().flatten(),
                "deadline": project.try_get::<Option<String>, _>("deadline").ok().flatten(),
                "halt_reason": project.try_get::<Option<String>, _>("halt_reason").ok().flatten(),
            },
            "milestones": milestones,
            "items": items,
            "events": events,
        }))
    }

    pub async fn events(
        &self,
        project_id: &str,
        since: i64,
        limit: usize,
    ) -> Result<Vec<Value>, String> {
        let path = self.root.join(project_id);
        let pool = open_pool(&path, false).await?;
        events_for(&pool, project_id, since, limit).await
    }

    pub async fn item(&self, project_id: &str, item_id: &str) -> Result<Value, String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let row = sqlx::query(
            "SELECT id, project_id, milestone_id, title, detail, status, acceptance_json, result, error, updated_at FROM items WHERE project_id = ? AND id = ?",
        )
        .bind(project_id)
        .bind(item_id)
        .fetch_optional(&pool)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("unknown item '{item_id}'"))?;
        Ok(json!({
            "id": row.get::<String, _>("id"),
            "project_id": row.get::<String, _>("project_id"),
            "milestone_id": row.get::<String, _>("milestone_id"),
            "title": row.get::<String, _>("title"),
            "detail": row.get::<String, _>("detail"),
            "status": row.get::<String, _>("status"),
            "acceptance": parse_json(row.get::<String, _>("acceptance_json")),
            "result": row.try_get::<Option<String>, _>("result").ok().flatten(),
            "error": row.try_get::<Option<String>, _>("error").ok().flatten(),
            "updated_at": row.get::<f64, _>("updated_at"),
        }))
    }

    pub async fn update(
        &self,
        project_id: &str,
        op: &str,
        params: &Value,
    ) -> Result<Value, String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let now = now();
        let message = match op {
            "pause" | "resume" | "close" => {
                let status = match op {
                    "pause" => "paused",
                    "resume" => "active",
                    _ => "closed",
                };
                sqlx::query("UPDATE project SET status = ?, updated_at = ? WHERE id = ?")
                    .bind(status)
                    .bind(now)
                    .bind(project_id)
                    .execute(&pool)
                    .await
                    .map_err(|error| error.to_string())?;
                append_event(&pool, project_id, op, "owner", None, None, json!({})).await?;
                format!("Project {status}.")
            }
            "add_item" => {
                let milestone_id = params
                    .get("milestone_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "add_item needs milestone_id".to_string())?;
                let title = params
                    .get("title")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "add_item needs title".to_string())?;
                let id = params
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        format!("item-{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
                    });
                sqlx::query(
                    "INSERT INTO items (id, project_id, milestone_id, title, detail, status, acceptance_json, updated_at) VALUES (?, ?, ?, ?, ?, 'pending', ?, ?)",
                )
                .bind(&id)
                .bind(project_id)
                .bind(milestone_id)
                .bind(title)
                .bind(params.get("detail").and_then(Value::as_str).unwrap_or(""))
                .bind(params.get("acceptance").cloned().unwrap_or_else(|| json!([])).to_string())
                .bind(now)
                .execute(&pool)
                .await
                .map_err(|error| error.to_string())?;
                append_event(
                    &pool,
                    project_id,
                    "item_added",
                    "owner",
                    Some(milestone_id.to_string()),
                    Some(id.clone()),
                    json!({"title": title}),
                )
                .await?;
                format!("Added item {id}.")
            }
            "add_milestone" => {
                let title = params
                    .get("title")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "add_milestone needs title".to_string())?;
                let id = params
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        format!("ms-{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
                    });
                let position: i64 =
                    sqlx::query_scalar("SELECT COUNT(*) FROM milestones WHERE project_id = ?")
                        .bind(project_id)
                        .fetch_one(&pool)
                        .await
                        .unwrap_or(0);
                sqlx::query(
                    "INSERT INTO milestones (id, project_id, title, status, position, depends_on_json) VALUES (?, ?, ?, 'pending', ?, '[]')",
                )
                .bind(&id)
                .bind(project_id)
                .bind(title)
                .bind(position)
                .execute(&pool)
                .await
                .map_err(|error| error.to_string())?;
                append_event(
                    &pool,
                    project_id,
                    "milestone_added",
                    "owner",
                    Some(id.clone()),
                    None,
                    json!({"title": title}),
                )
                .await?;
                format!("Added milestone {id}.")
            }
            "cancel_item" | "invalidate_item" => {
                let item_id = params
                    .get("item_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| format!("{op} needs item_id"))?;
                let status = if op == "cancel_item" {
                    "cancelled"
                } else {
                    "pending"
                };
                sqlx::query(
                    "UPDATE items SET status = ?, updated_at = ? WHERE project_id = ? AND id = ?",
                )
                .bind(status)
                .bind(now)
                .bind(project_id)
                .bind(item_id)
                .execute(&pool)
                .await
                .map_err(|error| error.to_string())?;
                append_event(
                    &pool,
                    project_id,
                    op,
                    "owner",
                    None,
                    Some(item_id.to_string()),
                    params.clone(),
                )
                .await?;
                format!("Item {item_id} {status}.")
            }
            other => return Err(format!("unsupported op '{other}'")),
        };
        Ok(json!({"ok": true, "queued": false, "message": message}))
    }

    pub async fn approval(
        &self,
        project_id: &str,
        item_id: &str,
        grant: bool,
        note: &str,
    ) -> Result<Value, String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let status = if grant { "pending" } else { "failed" };
        sqlx::query("UPDATE items SET status = ?, updated_at = ? WHERE project_id = ? AND id = ?")
            .bind(status)
            .bind(now())
            .bind(project_id)
            .bind(item_id)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
        append_event(
            &pool,
            project_id,
            if grant {
                "approval_granted"
            } else {
                "approval_rejected"
            },
            "owner",
            None,
            Some(item_id.to_string()),
            json!({"note": note}),
        )
        .await?;
        Ok(
            json!({"ok": true, "message": if grant { "Approval granted." } else { "Approval rejected." }}),
        )
    }
}

async fn open_pool(path: &Path, create: bool) -> Result<SqlitePool, String> {
    let db = path.join("state.db");
    if !create && !db.is_file() {
        return Err("project state not found".to_string());
    }
    if let Some(parent) = db.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| error.to_string())?;
    }
    let options = SqliteConnectOptions::new()
        .filename(&db)
        .create_if_missing(create)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(3)
        .connect_with(options)
        .await
        .map_err(|error| error.to_string())?;
    if create {
        init(&pool).await?;
    }
    Ok(pool)
}

async fn init(pool: &SqlitePool) -> Result<(), String> {
    for statement in [
        "CREATE TABLE IF NOT EXISTS project (id TEXT PRIMARY KEY, goal TEXT NOT NULL, status TEXT NOT NULL, origin_workspace_id TEXT, budget_usd REAL, deadline TEXT, halt_reason TEXT, created_at REAL NOT NULL, updated_at REAL NOT NULL)",
        "CREATE TABLE IF NOT EXISTS milestones (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, title TEXT NOT NULL, status TEXT NOT NULL, position INTEGER NOT NULL, depends_on_json TEXT)",
        "CREATE TABLE IF NOT EXISTS items (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, milestone_id TEXT, title TEXT NOT NULL, detail TEXT, status TEXT NOT NULL, acceptance_json TEXT, result TEXT, error TEXT, updated_at REAL NOT NULL)",
        "CREATE TABLE IF NOT EXISTS events (seq INTEGER PRIMARY KEY AUTOINCREMENT, project_id TEXT NOT NULL, ts REAL NOT NULL, kind TEXT NOT NULL, actor TEXT, milestone_id TEXT, item_id TEXT, data_json TEXT)",
        "CREATE INDEX IF NOT EXISTS ix_group_events ON events(project_id, seq)",
    ] {
        sqlx::query(statement)
            .execute(pool)
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

async fn append_event(
    pool: &SqlitePool,
    project_id: &str,
    kind: &str,
    actor: &str,
    milestone_id: Option<String>,
    item_id: Option<String>,
    data: Value,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO events (project_id, ts, kind, actor, milestone_id, item_id, data_json) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(project_id)
    .bind(now())
    .bind(kind)
    .bind(actor)
    .bind(milestone_id)
    .bind(item_id)
    .bind(data.to_string())
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

async fn events_for(
    pool: &SqlitePool,
    project_id: &str,
    since: i64,
    limit: usize,
) -> Result<Vec<Value>, String> {
    let rows = sqlx::query(
        "SELECT seq, ts, kind, actor, milestone_id, item_id, data_json FROM events WHERE project_id = ? AND seq > ? ORDER BY seq ASC LIMIT ?",
    )
    .bind(project_id)
    .bind(since)
    .bind(limit as i64)
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok(rows
        .into_iter()
        .map(|row| {
            json!({
                "seq": row.get::<i64, _>("seq"),
                "ts": row.get::<f64, _>("ts"),
                "kind": row.get::<String, _>("kind"),
                "actor": row.try_get::<Option<String>, _>("actor").ok().flatten().unwrap_or_default(),
                "milestone_id": row.try_get::<Option<String>, _>("milestone_id").ok().flatten(),
                "item_id": row.try_get::<Option<String>, _>("item_id").ok().flatten(),
                "data": parse_json(row.get::<String, _>("data_json")),
            })
        })
        .collect())
}

async fn count(pool: &SqlitePool, table: &str, project_id: &str) -> i64 {
    let query = match table {
        "milestones" => "SELECT COUNT(*) FROM milestones WHERE project_id = ?",
        "items" => "SELECT COUNT(*) FROM items WHERE project_id = ?",
        _ => return 0,
    };
    sqlx::query_scalar(query)
        .bind(project_id)
        .fetch_one(pool)
        .await
        .unwrap_or(0)
}

async fn count_where(pool: &SqlitePool, table: &str, project_id: &str, filter: &str) -> i64 {
    let query = match (table, filter) {
        ("milestones", "status = 'done'") => {
            "SELECT COUNT(*) FROM milestones WHERE project_id = ? AND status = 'done'"
        }
        ("items", "status = 'done'") => {
            "SELECT COUNT(*) FROM items WHERE project_id = ? AND status = 'done'"
        }
        ("items", "status = 'awaiting_approval'") => {
            "SELECT COUNT(*) FROM items WHERE project_id = ? AND status = 'awaiting_approval'"
        }
        _ => return 0,
    };
    sqlx::query_scalar(query)
        .bind(project_id)
        .fetch_one(pool)
        .await
        .unwrap_or(0)
}

fn parse_json(raw: String) -> Value {
    serde_json::from_str(&raw).unwrap_or(Value::Null)
}

fn now() -> f64 {
    Utc::now().timestamp_millis() as f64 / 1000.0
}

fn validate_id(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return Err(format!("invalid project id: {value}"));
    }
    Ok(())
}

fn slug(goal: &str) -> String {
    let mut slug = String::new();
    let mut dash = false;
    for character in goal.chars() {
        if character.is_ascii_alphanumeric() {
            if dash && !slug.is_empty() {
                slug.push('-');
            }
            dash = false;
            slug.push(character.to_ascii_lowercase());
        } else {
            dash = true;
        }
        if slug.len() >= 48 {
            break;
        }
    }
    slug.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn project_lifecycle_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = GroupStore::new(dir.path());
        let detail = store
            .create(
                &json!({
                    "id": "demo",
                    "goal": "Ship a demo",
                    "milestones": [{
                        "id": "m1",
                        "title": "Build",
                        "items": [{"id": "i1", "title": "Write it", "acceptance": ["works"]}]
                    }]
                }),
                "ws-1",
            )
            .await
            .unwrap();
        assert_eq!(detail["project"]["goal"], "Ship a demo");
        assert_eq!(store.list().await.len(), 1);
        let updated = store.update("demo", "pause", &json!({})).await.unwrap();
        assert!(updated["ok"].as_bool().unwrap());
        let detail = store.detail("demo", 60).await.unwrap();
        assert_eq!(detail["project"]["status"], "paused");
        assert_eq!(store.events("demo", 0, 10).await.unwrap().len(), 2);
        let item = store.item("demo", "i1").await.unwrap();
        assert_eq!(item["status"], "pending");
    }
}
