use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use serde_json::{Value, json};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

/// Group projects are folders, not a separate database.
///
/// The Python group runtime owns the canonical on-disk layout:
/// `<project>/.gp/state.db` plus a `PROJECT.md` manifest. Rust uses the same
/// layout so switching backends does not make existing projects disappear.
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
        if project_db(&path).exists() {
            return Err(format!("project '{id}' already exists"));
        }
        tokio::fs::create_dir_all(&path)
            .await
            .map_err(|error| error.to_string())?;
        tokio::fs::create_dir_all(&path.join(".gp"))
            .await
            .map_err(|error| error.to_string())?;
        if !origin_workspace_id.trim().is_empty() {
            let origin = json!({"workspace_id": origin_workspace_id.trim()});
            tokio::fs::write(path.join(".gp").join("origin.json"), format!("{origin}\n"))
                .await
                .map_err(|error| error.to_string())?;
        }

        // A JSON object is valid YAML. Keeping it in the Python fence means
        // the Python manifest parser can read a Rust-created project too.
        let yaml = serde_json::to_string_pretty(manifest).map_err(|error| error.to_string())?;
        tokio::fs::write(
            path.join("PROJECT.md"),
            format!("# {goal}\n\n## Why\n{goal}\n\n---\n```yaml\n{yaml}\n```\n"),
        )
        .await
        .map_err(|error| error.to_string())?;

        let pool = open_pool(&path, true).await?;
        let now = now();
        let status = "running";
        let completion = manifest
            .get("completion")
            .cloned()
            .unwrap_or_else(|| json!({"kind": "all_milestones_done"}));
        sqlx::query(
            "INSERT INTO projects (id, owner, status, halt_reason, warned, goal, budget_usd, spent_usd, budget_tokens, spent_tokens, deadline, completion_json, updated_at) VALUES (?, ?, ?, NULL, 0, ?, ?, 0, ?, 0, ?, ?, ?)",
        )
        .bind(&id)
        .bind(manifest.get("owner").and_then(Value::as_str).unwrap_or("localhost"))
        .bind(status)
        .bind(goal)
        .bind(manifest.get("budget_usd").and_then(Value::as_f64))
        .bind(manifest.get("budget_tokens").and_then(Value::as_i64))
        .bind(manifest.get("deadline").and_then(Value::as_str))
        .bind(completion.to_string())
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
                "INSERT INTO milestones (id, project_id, title, detail, status, seq_no, depends_on_json, acceptance_json, updated_at) VALUES (?, ?, ?, ?, 'todo', ?, ?, ?, ?)",
            )
            .bind(&milestone_id)
            .bind(&id)
            .bind(milestone.get("title").and_then(Value::as_str).unwrap_or(&milestone_id))
            .bind(milestone.get("detail").and_then(Value::as_str).unwrap_or(""))
            .bind(position as i64)
            .bind(milestone.get("depends_on").cloned().unwrap_or_else(|| json!([])).to_string())
            .bind(milestone.get("acceptance").cloned().unwrap_or_else(|| json!([])).to_string())
            .bind(now)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
            if let Some(items) = milestone.get("items").and_then(Value::as_array) {
                for (index, item) in items.iter().enumerate() {
                    insert_item(&pool, &id, &milestone_id, item, index, now).await?;
                }
            }
        }

        append_event(
            &pool,
            &id,
            "project.created",
            "orchestrator",
            None,
            None,
            json!({
                "goal": goal,
                "owner": manifest.get("owner").and_then(Value::as_str).unwrap_or("localhost"),
                "budget_usd": manifest.get("budget_usd"),
                "budget_tokens": manifest.get("budget_tokens"),
                "deadline": manifest.get("deadline"),
                "completion": completion,
                "milestones": milestones,
            }),
        )
        .await?;
        append_event(
            &pool,
            &id,
            "project.resumed",
            "orchestrator",
            None,
            None,
            json!({}),
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
            if !entry.path().is_dir() || !project_db(&entry.path()).is_file() {
                continue;
            }
            let Some(id) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let origin_workspace_id = read_origin(&entry.path()).await;
            let Ok(pool) = open_pool(&entry.path(), false).await else {
                continue;
            };
            let Ok(Some(project)) =
                sqlx::query("SELECT id, goal, status, owner FROM projects WHERE id = ?")
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
            let active_item = sqlx::query(
                "SELECT id, title, status, lease_owner, owner_member FROM items WHERE project_id = ? AND status IN ('doing', 'ready') ORDER BY CASE status WHEN 'doing' THEN 0 ELSE 1 END, id ASC LIMIT 1",
            )
            .bind(&id)
            .fetch_optional(&pool)
            .await
            .ok()
            .flatten()
            .map(|row| {
                json!({
                    "id": row.get::<String, _>("id"),
                    "title": row.get::<String, _>("title"),
                    "status": map_item_status(&row.get::<String, _>("status")),
                    "member": row.try_get::<Option<String>, _>("lease_owner").ok().flatten()
                        .or_else(|| row.try_get::<Option<String>, _>("owner_member").ok().flatten()),
                })
            });
            let last_message = sqlx::query(
                "SELECT seq, ts, kind, actor, item_id, data FROM events WHERE project_id = ? AND kind = 'message.sent' ORDER BY seq DESC LIMIT 1",
            )
            .bind(&id)
            .fetch_optional(&pool)
            .await
            .ok()
            .flatten()
            .and_then(|row| {
                message_from_event(&json!({
                    "seq": row.get::<i64, _>("seq"),
                    "ts": row.get::<f64, _>("ts"),
                    "kind": row.get::<String, _>("kind"),
                    "actor": row.try_get::<String, _>("actor").unwrap_or_default(),
                    "item_id": row.try_get::<Option<String>, _>("item_id").ok().flatten(),
                    "data": parse_json(row.get::<String, _>("data")),
                }))
            });
            let last_activity_ts: f64 =
                sqlx::query_scalar("SELECT COALESCE(MAX(ts), 0) FROM events WHERE project_id = ?")
                    .bind(&id)
                    .fetch_one(&pool)
                    .await
                    .unwrap_or(0.0);
            rows.push(json!({
                "id": project.get::<String, _>("id"),
                "path": entry.path(),
                "title": project.get::<String, _>("goal"),
                "goal": project.get::<String, _>("goal"),
                "status": map_project_status(&project.get::<String, _>("status")),
                "origin_workspace_id": origin_workspace_id,
                "shell_enabled": false,
                "spent_usd": null,
                "milestones_done": milestones_done,
                "milestones_total": milestones_total,
                "items_done": items_done,
                "items_total": items_total,
                "awaiting_approval": awaiting,
                "active_item": active_item.as_ref().map(|item| json!({
                    "id": item["id"],
                    "title": item["title"],
                    "status": item["status"],
                })),
                "active_member": active_item.as_ref().and_then(|item| item["member"].as_str()),
                "last_message": last_message,
                "last_activity_ts": last_activity_ts,
                "participants": [],
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

    /// Atomically claim the first dependency-ready item in a running project.
    pub async fn claim_next_ready(&self, project_id: &str) -> Result<Option<Value>, String> {
        let path = self.root.join(project_id);
        let pool = open_pool(&path, false).await?;
        let mut tx = pool.begin().await.map_err(|error| error.to_string())?;
        let active = sqlx::query(
            "UPDATE projects SET updated_at = updated_at WHERE id = ? AND status = 'running'",
        )
        .bind(project_id)
        .execute(&mut *tx)
        .await
        .map_err(|error| error.to_string())?
        .rows_affected()
            > 0;
        if !active {
            tx.commit().await.map_err(|error| error.to_string())?;
            return Ok(None);
        }

        let candidates = sqlx::query(
            "SELECT i.id, i.milestone_id, i.title, i.detail, i.acceptance_json, i.owner_member, m.title AS milestone_title, m.depends_on_json, p.goal FROM items i JOIN milestones m ON m.project_id = i.project_id AND m.id = i.milestone_id JOIN projects p ON p.id = i.project_id WHERE i.project_id = ? AND i.status IN ('todo', 'ready') ORDER BY m.seq_no ASC, i.id ASC",
        )
        .bind(project_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|error| error.to_string())?;

        for row in candidates {
            let milestone_id: String = row.get("milestone_id");
            let dependencies = parse_json(row.get("depends_on_json"));
            let mut dependencies_ready = true;
            if let Some(dependencies) = dependencies.as_array() {
                for dependency in dependencies.iter().filter_map(Value::as_str) {
                    let status = sqlx::query_scalar::<_, Option<String>>(
                        "SELECT status FROM milestones WHERE project_id = ? AND id = ?",
                    )
                    .bind(project_id)
                    .bind(dependency)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(|error| error.to_string())?
                    .flatten();
                    if status.as_deref() != Some("done") {
                        dependencies_ready = false;
                        break;
                    }
                }
            }
            if !dependencies_ready {
                continue;
            }

            let item_id: String = row.get("id");
            let lease_owner = "group-runner";
            let lease_expires = now() + 30.0 * 60.0;
            let updated = sqlx::query(
                "UPDATE items SET status = 'doing', attempts = attempts + 1, lease_owner = ?, lease_expires = ?, result = NULL, error = NULL, updated_at = ? WHERE project_id = ? AND id = ? AND status IN ('todo', 'ready')",
            )
            .bind(lease_owner)
            .bind(lease_expires)
            .bind(now())
            .bind(project_id)
            .bind(&item_id)
            .execute(&mut *tx)
            .await
            .map_err(|error| error.to_string())?;
            if updated.rows_affected() == 0 {
                continue;
            }
            sqlx::query("UPDATE milestones SET status = 'ready', updated_at = ? WHERE project_id = ? AND id = ? AND status IN ('todo', 'ready')")
                .bind(now())
                .bind(project_id)
                .bind(&milestone_id)
                .execute(&mut *tx)
                .await
                .map_err(|error| error.to_string())?;
            sqlx::query(
                "INSERT INTO events (project_id, ts, kind, actor, milestone_id, item_id, data) VALUES (?, ?, 'item.claimed', 'group-runner', ?, ?, ?)",
            )
            .bind(project_id)
            .bind(now())
            .bind(&milestone_id)
            .bind(&item_id)
            .bind(json!({"lease_owner": lease_owner, "lease_expires": lease_expires}).to_string())
            .execute(&mut *tx)
            .await
            .map_err(|error| error.to_string())?;
            let member = row
                .try_get::<Option<String>, _>("owner_member")
                .ok()
                .flatten()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "developer".to_string());
            let assignment = format!("Assigned: {}", row.get::<String, _>("title"));
            sqlx::query(
                "INSERT INTO events (project_id, ts, kind, actor, milestone_id, item_id, data) VALUES (?, ?, 'message.sent', 'supervisor', ?, ?, ?)",
            )
            .bind(project_id)
            .bind(now())
            .bind(&milestone_id)
            .bind(&item_id)
            .bind(json!({
                "from": "supervisor",
                "to": member,
                "body": assignment,
                "kind": "assignment",
                "deliver": false,
            }).to_string())
            .execute(&mut *tx)
            .await
            .map_err(|error| error.to_string())?;
            let claimed = json!({
                "id": item_id,
                "milestone_id": milestone_id,
                "title": row.get::<String, _>("title"),
                "detail": row.try_get::<Option<String>, _>("detail").ok().flatten().unwrap_or_default(),
                "acceptance": parse_json(row.get("acceptance_json")),
                "milestone_title": row.get::<String, _>("milestone_title"),
                "goal": row.get::<String, _>("goal"),
            });
            tx.commit().await.map_err(|error| error.to_string())?;
            return Ok(Some(claimed));
        }

        tx.commit().await.map_err(|error| error.to_string())?;
        Ok(None)
    }

    /// On server startup no `doing` item has a live worker, so release those
    /// claims and record recovery before the runner starts dispatching again.
    pub async fn recover_doing_items(&self) -> Result<usize, String> {
        let Ok(mut entries) = tokio::fs::read_dir(&self.root).await else {
            return Ok(0);
        };
        let mut recovered = 0;
        while let Ok(Some(entry)) = entries.next_entry().await {
            if !entry.path().is_dir() || !project_db(&entry.path()).is_file() {
                continue;
            }
            let Some(project_id) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let pool = open_pool(&entry.path(), false).await?;
            let rows = sqlx::query(
                "SELECT id, milestone_id FROM items WHERE project_id = ? AND status = 'doing'",
            )
            .bind(&project_id)
            .fetch_all(&pool)
            .await
            .map_err(|error| error.to_string())?;
            for row in rows {
                let item_id: String = row.get("id");
                let milestone_id: Option<String> = row.try_get("milestone_id").ok();
                sqlx::query("UPDATE items SET status = 'todo', lease_owner = NULL, lease_expires = NULL, updated_at = ? WHERE project_id = ? AND id = ? AND status = 'doing'")
                    .bind(now())
                    .bind(&project_id)
                    .bind(&item_id)
                    .execute(&pool)
                    .await
                    .map_err(|error| error.to_string())?;
                append_event(
                    &pool,
                    &project_id,
                    "item.released",
                    "group-runner",
                    milestone_id,
                    Some(item_id),
                    json!({}),
                )
                .await?;
                recovered += 1;
            }
        }
        Ok(recovered)
    }

    pub async fn finish_item(
        &self,
        project_id: &str,
        item_id: &str,
        success: bool,
        output: &str,
    ) -> Result<(), String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let item = sqlx::query(
            "SELECT milestone_id, lease_owner FROM items WHERE project_id = ? AND id = ? AND status = 'doing'",
        )
        .bind(project_id)
        .bind(item_id)
        .fetch_optional(&pool)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("item '{item_id}' is not claimed"))?;
        let milestone_id: Option<String> = item.try_get("milestone_id").ok();
        let member = item
            .try_get::<Option<String>, _>("lease_owner")
            .ok()
            .flatten()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "developer".to_string());
        let status = if success { "done" } else { "failed" };
        sqlx::query("UPDATE items SET status = ?, result = ?, error = ?, lease_owner = NULL, lease_expires = NULL, updated_at = ? WHERE project_id = ? AND id = ? AND status = 'doing'")
            .bind(status)
            .bind(if success { Some(output) } else { None })
            .bind(if success { None } else { Some(output) })
            .bind(now())
            .bind(project_id)
            .bind(item_id)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
        append_event(
            &pool,
            project_id,
            if success { "item.done" } else { "item.failed" },
            "group-runner",
            milestone_id.clone(),
            Some(item_id.to_string()),
            if success {
                json!({"result": output})
            } else {
                json!({"error": output})
            },
        )
        .await?;
        let message_body: String = output.chars().take(4_000).collect();
        append_event(
            &pool,
            project_id,
            "message.sent",
            member.as_str(),
            milestone_id.clone(),
            Some(item_id.to_string()),
            json!({
                "from": member,
                "to": "supervisor",
                "body": if message_body.trim().is_empty() {
                    if success { "Work item completed." } else { "Work item failed." }
                } else {
                    message_body.as_str()
                },
                "kind": "report",
                "deliver": false,
            }),
        )
        .await?;

        if success && let Some(milestone_id) = milestone_id {
            let remaining: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM items WHERE project_id = ? AND milestone_id = ? AND status != 'done'",
            )
            .bind(project_id)
            .bind(&milestone_id)
            .fetch_one(&pool)
            .await
            .map_err(|error| error.to_string())?;
            if remaining == 0 {
                let updated = sqlx::query("UPDATE milestones SET status = 'done', updated_at = ? WHERE project_id = ? AND id = ? AND status != 'done'")
                    .bind(now())
                    .bind(project_id)
                    .bind(&milestone_id)
                    .execute(&pool)
                    .await
                    .map_err(|error| error.to_string())?;
                if updated.rows_affected() > 0 {
                    append_event(
                        &pool,
                        project_id,
                        "milestone.done",
                        "group-runner",
                        Some(milestone_id),
                        None,
                        json!({}),
                    )
                    .await?;
                }
            }
        }
        if success {
            let unfinished: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM milestones WHERE project_id = ? AND status != 'done'",
            )
            .bind(project_id)
            .fetch_one(&pool)
            .await
            .map_err(|error| error.to_string())?;
            if unfinished == 0 {
                let updated = sqlx::query("UPDATE projects SET status = 'done', updated_at = ? WHERE id = ? AND status = 'running'")
                    .bind(now())
                    .bind(project_id)
                    .execute(&pool)
                    .await
                    .map_err(|error| error.to_string())?;
                if updated.rows_affected() > 0 {
                    append_event(
                        &pool,
                        project_id,
                        "project.closed",
                        "group-runner",
                        None,
                        None,
                        json!({"status": "done"}),
                    )
                    .await?;
                }
            }
        }
        Ok(())
    }

    pub async fn detail(&self, project_id: &str, event_limit: usize) -> Result<Value, String> {
        validate_id(project_id)?;
        let path = self.root.join(project_id);
        if !path.is_dir() {
            return Err(format!("unknown project '{project_id}'"));
        }
        let origin_workspace_id = read_origin(&path).await;
        let pool = open_pool(&path, false).await?;
        let project = sqlx::query(
            "SELECT id, goal, status, budget_usd, deadline, halt_reason FROM projects WHERE id = ?",
        )
        .bind(project_id)
        .fetch_optional(&pool)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("unknown project '{project_id}'"))?;
        let milestones = sqlx::query(
            "SELECT id, project_id, title, status, depends_on_json FROM milestones WHERE project_id = ? ORDER BY seq_no ASC, id ASC",
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
                "status": map_milestone_status(&row.get::<String, _>("status")),
                "depends_on": parse_json(row.get::<String, _>("depends_on_json")),
            })
        })
        .collect::<Vec<_>>();
        let items = sqlx::query(
            "SELECT id, project_id, milestone_id, title, detail, status, acceptance_json, result, error, attempts, max_attempts, owner_member, lease_owner, blocked_reason, invalidated_reason, updated_at FROM items WHERE project_id = ? ORDER BY milestone_id ASC, id ASC",
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
                "status": map_item_status(&row.get::<String, _>("status")),
                "acceptance": parse_json(row.get::<String, _>("acceptance_json")),
                "attempts": row.try_get::<i64, _>("attempts").unwrap_or(0),
                "max_attempts": row.try_get::<i64, _>("max_attempts").unwrap_or(3),
                "owner_member": row.try_get::<Option<String>, _>("owner_member").ok().flatten(),
                "lease_owner": row.try_get::<Option<String>, _>("lease_owner").ok().flatten(),
                "blocked_reason": row.try_get::<Option<String>, _>("blocked_reason").ok().flatten(),
                "invalidated_reason": row.try_get::<Option<String>, _>("invalidated_reason").ok().flatten(),
                "result": row.try_get::<Option<String>, _>("result").ok().flatten(),
                "error": row.try_get::<Option<String>, _>("error").ok().flatten(),
                "updated_at": row.get::<f64, _>("updated_at"),
            })
        })
        .collect::<Vec<_>>();
        let events = events_for(&pool, project_id, 0, event_limit).await?;
        let messages = messages_from_events(&events);
        Ok(json!({
            "id": project.get::<String, _>("id"),
            "path": path,
            "shell_enabled": false,
            "project": {
                "goal": project.get::<String, _>("goal"),
                "status": map_project_status(&project.get::<String, _>("status")),
                "origin_workspace_id": origin_workspace_id,
                "budget_usd": project.try_get::<Option<f64>, _>("budget_usd").ok().flatten(),
                "deadline": project.try_get::<Option<String>, _>("deadline").ok().flatten(),
                "halt_reason": project.try_get::<Option<String>, _>("halt_reason").ok().flatten(),
            },
            "messages": messages,
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
            "status": map_item_status(&row.get::<String, _>("status")),
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
                let (status, kind) = match op {
                    "pause" => ("paused", "project.paused"),
                    "resume" => ("running", "project.resumed"),
                    _ => ("done", "project.closed"),
                };
                sqlx::query("UPDATE projects SET status = ?, updated_at = ? WHERE id = ?")
                    .bind(status)
                    .bind(now)
                    .bind(project_id)
                    .execute(&pool)
                    .await
                    .map_err(|error| error.to_string())?;
                append_event(
                    &pool,
                    project_id,
                    kind,
                    "owner",
                    None,
                    None,
                    if op == "close" {
                        json!({"status": "done"})
                    } else {
                        json!({})
                    },
                )
                .await?;
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
                insert_item(&pool, project_id, milestone_id, params, 0, now).await?;
                append_event(
                    &pool,
                    project_id,
                    "item.created",
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
                    "INSERT INTO milestones (id, project_id, title, detail, status, seq_no, depends_on_json, acceptance_json, updated_at) VALUES (?, ?, ?, '', 'todo', ?, '[]', '[]', ?)",
                )
                .bind(&id)
                .bind(project_id)
                .bind(title)
                .bind(position)
                .bind(now)
                .execute(&pool)
                .await
                .map_err(|error| error.to_string())?;
                append_event(
                    &pool,
                    project_id,
                    "milestone.created",
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
                    "invalidated"
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
                    if op == "cancel_item" {
                        "item.cancelled"
                    } else {
                        "item.invalidated"
                    },
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
        let status = if grant { "todo" } else { "failed" };
        sqlx::query("UPDATE items SET status = ?, lease_owner = NULL, lease_expires = NULL, updated_at = ? WHERE project_id = ? AND id = ?")
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
                "approval.granted"
            } else {
                "approval.rejected"
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

async fn insert_item(
    pool: &SqlitePool,
    project_id: &str,
    milestone_id: &str,
    item: &Value,
    index: usize,
    now: f64,
) -> Result<String, String> {
    let item_id = item
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("{milestone_id}-item-{index}"));
    validate_id(&item_id)?;
    sqlx::query(
        "INSERT INTO items (id, project_id, milestone_id, title, detail, status, attempts, max_attempts, owner_member, workdir, lease_owner, lease_expires, depends_on_json, acceptance_json, tags_json, cost_tokens, cost_usd, result, error, blocked_reason, invalidated_reason, updated_at) VALUES (?, ?, ?, ?, ?, 'todo', 0, ?, ?, ?, NULL, NULL, ?, ?, ?, 0, 0, '', '', NULL, NULL, ?)",
    )
    .bind(&item_id)
    .bind(project_id)
    .bind(milestone_id)
    .bind(item.get("title").and_then(Value::as_str).unwrap_or(""))
    .bind(item.get("detail").and_then(Value::as_str).unwrap_or(""))
    .bind(item.get("max_attempts").and_then(Value::as_i64).unwrap_or(3))
    .bind(item.get("owner_member").and_then(Value::as_str).unwrap_or(""))
    .bind(item.get("workdir").and_then(Value::as_str).unwrap_or(""))
    .bind(item.get("depends_on").cloned().unwrap_or_else(|| json!([])).to_string())
    .bind(item.get("acceptance").cloned().unwrap_or_else(|| json!([])).to_string())
    .bind(item.get("tags").cloned().unwrap_or_else(|| json!([])).to_string())
    .bind(now)
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok(item_id)
}

async fn open_pool(path: &Path, create: bool) -> Result<SqlitePool, String> {
    let db = project_db(path);
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
        "CREATE TABLE IF NOT EXISTS events (seq INTEGER PRIMARY KEY AUTOINCREMENT, project_id TEXT NOT NULL, ts REAL NOT NULL, kind TEXT NOT NULL, actor TEXT NOT NULL, milestone_id TEXT, item_id TEXT, data TEXT)",
        "CREATE INDEX IF NOT EXISTS ix_events_project ON events(project_id, seq)",
        "CREATE TABLE IF NOT EXISTS projects (id TEXT PRIMARY KEY, owner TEXT, status TEXT NOT NULL DEFAULT 'created', halt_reason TEXT, warned INTEGER DEFAULT 0, goal TEXT, budget_usd REAL, spent_usd REAL DEFAULT 0, budget_tokens INTEGER, spent_tokens INTEGER DEFAULT 0, deadline REAL, completion_json TEXT, updated_at REAL)",
        "CREATE TABLE IF NOT EXISTS milestones (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, title TEXT, detail TEXT, status TEXT NOT NULL DEFAULT 'todo', seq_no INTEGER DEFAULT 0, depends_on_json TEXT, acceptance_json TEXT, updated_at REAL)",
        "CREATE INDEX IF NOT EXISTS ix_milestones_project ON milestones(project_id, seq_no)",
        "CREATE TABLE IF NOT EXISTS items (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, milestone_id TEXT, title TEXT, detail TEXT, status TEXT NOT NULL DEFAULT 'todo', attempts INTEGER DEFAULT 0, max_attempts INTEGER DEFAULT 3, owner_member TEXT, workdir TEXT, lease_owner TEXT, lease_expires REAL, depends_on_json TEXT, acceptance_json TEXT, tags_json TEXT, cost_tokens INTEGER DEFAULT 0, cost_usd REAL DEFAULT 0, result TEXT, error TEXT, blocked_reason TEXT, invalidated_reason TEXT, updated_at REAL)",
        "CREATE INDEX IF NOT EXISTS ix_items_project ON items(project_id, status)",
        "CREATE INDEX IF NOT EXISTS ix_items_milestone ON items(milestone_id, status)",
        "CREATE TABLE IF NOT EXISTS ops (id INTEGER PRIMARY KEY AUTOINCREMENT, project_id TEXT NOT NULL, kind TEXT NOT NULL, payload_json TEXT, actor TEXT, state TEXT NOT NULL DEFAULT 'pending', error TEXT, created_at REAL, applied_at REAL)",
        "CREATE INDEX IF NOT EXISTS ix_ops_pending ON ops(project_id, state)",
        "CREATE TABLE IF NOT EXISTS projection_checkpoint (project_id TEXT PRIMARY KEY, last_seq INTEGER NOT NULL DEFAULT 0)",
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
        "INSERT INTO events (project_id, ts, kind, actor, milestone_id, item_id, data) VALUES (?, ?, ?, ?, ?, ?, ?)",
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
        "SELECT seq, ts, kind, actor, milestone_id, item_id, data FROM events WHERE project_id = ? AND seq > ? ORDER BY seq ASC LIMIT ?",
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
                "actor": row.try_get::<String, _>("actor").unwrap_or_default(),
                "milestone_id": row.try_get::<Option<String>, _>("milestone_id").ok().flatten(),
                "item_id": row.try_get::<Option<String>, _>("item_id").ok().flatten(),
                "data": parse_json(row.get::<String, _>("data")),
            })
        })
        .collect())
}

fn message_from_event(event: &Value) -> Option<Value> {
    if event["kind"] != "message.sent" {
        return None;
    }
    let data = &event["data"];
    let body = data["body"].as_str()?.trim();
    if body.is_empty() {
        return None;
    }
    let mut message = json!({
        "seq": event["seq"],
        "ts": event["ts"],
        "from": data["from"].as_str().or_else(|| event["actor"].as_str()).unwrap_or("unknown"),
        "to": data["to"].as_str().unwrap_or("all"),
        "body": body,
        "kind": data["kind"].as_str().unwrap_or("note"),
        "item_id": event["item_id"].as_str().or_else(|| data["item_id"].as_str()),
        "deliver": data["deliver"].as_bool().unwrap_or(true),
    });
    if let Some(reply_to) = data.get("reply_to") {
        message["reply_to"] = reply_to.clone();
    }
    Some(message)
}

fn messages_from_events(events: &[Value]) -> Vec<Value> {
    events.iter().filter_map(message_from_event).collect()
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
            "SELECT COUNT(*) FROM items WHERE project_id = ? AND blocked_reason = 'awaiting_approval'"
        }
        _ => return 0,
    };
    sqlx::query_scalar(query)
        .bind(project_id)
        .fetch_one(pool)
        .await
        .unwrap_or(0)
}

fn project_db(path: &Path) -> PathBuf {
    path.join(".gp").join("state.db")
}

async fn read_origin(path: &Path) -> Option<String> {
    let text = tokio::fs::read_to_string(path.join(".gp").join("origin.json"))
        .await
        .ok()?;
    serde_json::from_str::<Value>(&text)
        .ok()?
        .get("workspace_id")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn parse_json(raw: String) -> Value {
    serde_json::from_str(&raw).unwrap_or(Value::Null)
}

fn map_project_status(status: &str) -> String {
    match status {
        "active" => "running".to_string(),
        other => other.to_string(),
    }
}

fn map_milestone_status(status: &str) -> String {
    match status {
        "todo" => "pending".to_string(),
        "ready" => "active".to_string(),
        other => other.to_string(),
    }
}

fn map_item_status(status: &str) -> String {
    match status {
        "todo" => "pending".to_string(),
        other => other.to_string(),
    }
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
        assert_eq!(store.events("demo", 0, 10).await.unwrap().len(), 3);
        let item = store.item("demo", "i1").await.unwrap();
        assert_eq!(item["status"], "pending");
    }

    #[tokio::test]
    async fn claims_dependency_ready_items_and_marks_milestones_done() {
        let dir = tempfile::tempdir().unwrap();
        let store = GroupStore::new(dir.path());
        store.create(&json!({
            "id": "ready-run",
            "goal": "Ship a demo",
            "milestones": [
                {"id": "m1", "title": "First", "items": [{"id": "i1", "title": "One"}]},
                {"id": "m2", "title": "Second", "depends_on": ["m1"], "items": [{"id": "i2", "title": "Two"}]}
            ]
        }), "ws-1").await.unwrap();

        let first = store.claim_next_ready("ready-run").await.unwrap().unwrap();
        assert_eq!(first["id"], "i1");
        assert!(store.claim_next_ready("ready-run").await.unwrap().is_none());
        assert_eq!(
            store.item("ready-run", "i1").await.unwrap()["status"],
            "doing"
        );
        store
            .finish_item("ready-run", "i1", true, "First item complete")
            .await
            .unwrap();
        assert_eq!(
            store.detail("ready-run", 20).await.unwrap()["milestones"][0]["status"],
            "done"
        );

        let second = store.claim_next_ready("ready-run").await.unwrap().unwrap();
        assert_eq!(second["id"], "i2");
        store
            .finish_item("ready-run", "i2", true, "Second item complete")
            .await
            .unwrap();
        let detail = store.detail("ready-run", 20).await.unwrap();
        assert_eq!(detail["milestones"][1]["status"], "done");
        assert_eq!(detail["items"][1]["status"], "done");
    }

    #[tokio::test]
    async fn paused_projects_do_not_dispatch_and_failed_items_record_events() {
        let dir = tempfile::tempdir().unwrap();
        let store = GroupStore::new(dir.path());
        store.create(&json!({
            "id": "paused-run",
            "goal": "Ship a demo",
            "milestones": [{"id": "m1", "title": "First", "items": [{"id": "i1", "title": "One"}]}]
        }), "ws-1").await.unwrap();
        store
            .update("paused-run", "pause", &json!({}))
            .await
            .unwrap();
        assert!(
            store
                .claim_next_ready("paused-run")
                .await
                .unwrap()
                .is_none()
        );
        store
            .update("paused-run", "resume", &json!({}))
            .await
            .unwrap();
        let item = store.claim_next_ready("paused-run").await.unwrap().unwrap();
        assert_eq!(item["id"], "i1");
        store
            .finish_item("paused-run", "i1", false, "mock provider failed")
            .await
            .unwrap();
        let detail = store.detail("paused-run", 20).await.unwrap();
        assert_eq!(detail["items"][0]["status"], "failed");
        assert_eq!(detail["items"][0]["error"], "mock provider failed");
        assert!(
            detail["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|event| event["kind"] == "item.failed")
        );
    }

    #[tokio::test]
    async fn startup_recovery_releases_a_claimed_item_for_a_new_worker() {
        let dir = tempfile::tempdir().unwrap();
        let store = GroupStore::new(dir.path());
        store.create(&json!({
            "id": "restart-run",
            "goal": "Ship a demo",
            "milestones": [{"id": "m1", "title": "First", "items": [{"id": "i1", "title": "One"}]}]
        }), "ws-1").await.unwrap();
        store
            .claim_next_ready("restart-run")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(store.recover_doing_items().await.unwrap(), 1);
        assert_eq!(
            store.item("restart-run", "i1").await.unwrap()["status"],
            "pending"
        );
        assert!(
            store
                .claim_next_ready("restart-run")
                .await
                .unwrap()
                .is_some()
        );
    }
}
