use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration as ChronoDuration, Local, Months, TimeZone, Utc};
use serde_json::{Value, json};
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{Row, SqlitePool};

/// Group projects are folders, not a separate database.
///
/// Canonical layout: `<project>/.gp/state.db` plus a `PROJECT.md` manifest.
#[derive(Clone, Debug)]
pub struct GroupStore {
    root: PathBuf,
}

impl GroupStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn default_root() -> Self {
        // One portable data root: group projects live inside the harness root
        // (`<harness>/.deep_agents/groups`) so a backup/import carries them.
        // Legacy installs that still have D:\StarkProjects keep using it until
        // the user migrates; STARK_GROUPS_ROOT always wins.
        if let Ok(raw) = std::env::var("STARK_GROUPS_ROOT") {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return Self::new(PathBuf::from(trimmed));
            }
        }
        if let Ok(harness) = std::env::var("STARK_HARNESS_ROOT") {
            let trimmed = harness.trim();
            if !trimmed.is_empty() {
                return Self::new(PathBuf::from(trimmed).join(".deep_agents").join("groups"));
            }
        }
        #[cfg(windows)]
        {
            let legacy = PathBuf::from(r"D:\StarkProjects");
            if legacy.is_dir() {
                return Self::new(legacy);
            }
        }
        Self::new(crate::projects_root().join(".groups"))
    }

    pub async fn create(
        &self,
        manifest: &Value,
        origin_workspace_id: &str,
    ) -> Result<Value, String> {
        self.create_with_origin(manifest, origin_workspace_id, "")
            .await
    }

    pub async fn create_with_origin(
        &self,
        manifest: &Value,
        origin_workspace_id: &str,
        origin_chat_id: &str,
    ) -> Result<Value, String> {
        let goal = manifest
            .get("goal")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if goal.is_empty() {
            return Err("goal is required".to_string());
        }
        let title = manifest
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(goal);
        let workspace_path = manifest
            .get("workspace_path")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if !workspace_path.is_empty() && !Path::new(workspace_path).is_dir() {
            return Err(format!("workspace folder not found: {workspace_path}"));
        }
        let id = manifest
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| slug(title));
        validate_id(&id)?;
        let path = self.root.join(&id);
        if project_db(&path).exists() {
            return Err(format!("project '{id}' already exists"));
        }

        let schedule = manifest.get("schedule").unwrap_or(&Value::Null);
        let start = manifest.get("start").unwrap_or(&Value::Null);
        let start_mode = manifest
            .get("start_mode")
            .or_else(|| start.get("mode"))
            .and_then(Value::as_str)
            .unwrap_or("now")
            .trim()
            .to_ascii_lowercase();
        let start_mode = match start_mode.as_str() {
            "scheduled" | "schedule" | "later" => "scheduled",
            "manual" | "draft" => "manual",
            _ => "now",
        };
        let start_at = parse_time(
            manifest
                .get("start_at")
                .or_else(|| start.get("at"))
                .or_else(|| schedule.get("start_at")),
        );
        let recurrence = manifest
            .get("recurrence")
            .or_else(|| schedule.get("recurrence"))
            .and_then(Value::as_str)
            .unwrap_or("none")
            .trim()
            .to_ascii_lowercase();
        let recurrence = match recurrence.as_str() {
            "daily" | "weekly" | "monthly" => recurrence,
            _ => "none".to_string(),
        };
        let repeat_active = manifest
            .get("repeat_active")
            .and_then(Value::as_bool)
            .unwrap_or(recurrence != "none");
        let plan_review = manifest
            .get("plan_review")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let auto_plan = manifest
            .get("auto_plan")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let planner_agent = manifest
            .get("planner_agent")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("orchestrator")
            .to_ascii_lowercase();
        let max_parallel = manifest
            .get("max_parallel")
            .and_then(Value::as_i64)
            .unwrap_or(1)
            .clamp(1, 8);
        let milestones = manifest
            .get("milestones")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let scheduled_future = start_mode == "scheduled" && start_at.is_some_and(|at| at > now());
        let status = if milestones.is_empty() {
            if auto_plan { "planning" } else { "draft" }
        } else if scheduled_future {
            "scheduled"
        } else {
            "running"
        };
        let plan_status = if milestones.is_empty() {
            if auto_plan { "queued" } else { "none" }
        } else {
            "ready"
        };

        tokio::fs::create_dir_all(&path)
            .await
            .map_err(|error| error.to_string())?;
        tokio::fs::create_dir_all(&path.join(".gp"))
            .await
            .map_err(|error| error.to_string())?;
        if !origin_workspace_id.trim().is_empty() || !origin_chat_id.trim().is_empty() {
            let origin = json!({
                "workspace_id": origin_workspace_id.trim(),
                "chat_id": origin_chat_id.trim(),
                "workspace_path": workspace_path,
            });
            tokio::fs::write(path.join(".gp").join("origin.json"), format!("{origin}\n"))
                .await
                .map_err(|error| error.to_string())?;
        }
        let manifest_text =
            serde_json::to_string_pretty(manifest).map_err(|error| error.to_string())?;
        tokio::fs::write(
            path.join("PROJECT.md"),
            format!(
                "# {title}\n\n## Goal\n{goal}\n\n## Work folder\n{workspace_path}\n\n---\n```yaml\n{manifest_text}\n```\n"
            ),
        )
        .await
        .map_err(|error| error.to_string())?;

        let pool = open_pool(&path, true).await?;
        let timestamp = now();
        let completion = manifest
            .get("completion")
            .cloned()
            .unwrap_or_else(|| json!({"kind": "all_milestones_done"}));
        sqlx::query(
            "INSERT INTO projects (id, owner, status, halt_reason, warned, goal, title, workspace_path, origin_chat_id, cycle, plan_status, plan_review, start_mode, start_at, next_run_at, recurrence, repeat_active, planner_agent, max_parallel, budget_usd, spent_usd, budget_tokens, spent_tokens, deadline, completion_json, updated_at) VALUES (?, ?, ?, NULL, 0, ?, ?, ?, ?, 1, ?, ?, ?, ?, NULL, ?, ?, ?, ?, ?, 0, ?, 0, ?, ?, ?)",
        )
        .bind(&id)
        .bind(
            manifest
                .get("owner")
                .and_then(Value::as_str)
                .unwrap_or("localhost"),
        )
        .bind(status)
        .bind(goal)
        .bind(title)
        .bind(workspace_path)
        .bind(origin_chat_id.trim())
        .bind(plan_status)
        .bind(if plan_review { 1 } else { 0 })
        .bind(start_mode)
        .bind(start_at)
        .bind(&recurrence)
        .bind(if repeat_active { 1 } else { 0 })
        .bind(&planner_agent)
        .bind(max_parallel)
        .bind(manifest.get("budget_usd").and_then(Value::as_f64))
        .bind(manifest.get("budget_tokens").and_then(Value::as_i64))
        .bind(manifest.get("deadline").and_then(Value::as_str))
        .bind(completion.to_string())
        .bind(timestamp)
        .execute(&pool)
        .await
        .map_err(|error| error.to_string())?;

        let members = parse_member_specs(manifest.get("members"));
        insert_members(&pool, &id, &members).await?;

        for (position, milestone) in milestones.iter().enumerate() {
            let milestone_id = milestone
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("ms-{position}"));
            validate_id(&milestone_id)?;
            sqlx::query(
                "INSERT INTO milestones (id, project_id, title, detail, status, seq_no, cycle, depends_on_json, acceptance_json, owner_member_id, due_at, updated_at) VALUES (?, ?, ?, ?, 'todo', ?, 1, ?, ?, ?, ?, ?)",
            )
            .bind(&milestone_id)
            .bind(&id)
            .bind(milestone.get("title").and_then(Value::as_str).unwrap_or(&milestone_id))
            .bind(milestone.get("detail").and_then(Value::as_str).unwrap_or(""))
            .bind(position as i64)
            .bind(
                milestone
                    .get("depends_on")
                    .cloned()
                    .unwrap_or_else(|| json!([]))
                    .to_string(),
            )
            .bind(
                milestone
                    .get("acceptance")
                    .cloned()
                    .unwrap_or_else(|| json!([]))
                    .to_string(),
            )
            .bind(milestone.get("owner_member_id").and_then(Value::as_str))
            .bind(parse_time(milestone.get("due_at")))
            .bind(timestamp)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
            if let Some(items) = milestone.get("items").and_then(Value::as_array) {
                for (index, item) in items.iter().enumerate() {
                    insert_item(&pool, &id, &milestone_id, item, index, timestamp).await?;
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
                "title": title,
                "workspace_path": workspace_path,
                "owner": manifest.get("owner").and_then(Value::as_str).unwrap_or("localhost"),
                "budget_usd": manifest.get("budget_usd"),
                "budget_tokens": manifest.get("budget_tokens"),
                "deadline": manifest.get("deadline"),
                "completion": completion,
                "members": members.iter().map(|member| json!({"agent_key": member.agent_key, "display": member.display, "slot": member.slot})).collect::<Vec<_>>(),
                "schedule": {"start_mode": start_mode, "start_at": start_at, "recurrence": recurrence, "repeat_active": repeat_active},
                "milestones": milestones,
            }),
        )
        .await?;
        append_event(
            &pool,
            &id,
            if status == "running" {
                "project.resumed"
            } else {
                "project.created.pending"
            },
            "orchestrator",
            None,
            None,
            json!({"status": status}),
        )
        .await?;
        pool.close().await;
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
            if id == ".trash" {
                continue;
            }
            let origin_workspace_id = read_origin(&entry.path()).await;
            let Ok(pool) = open_pool(&entry.path(), false).await else {
                continue;
            };
            let Ok(Some(project)) = sqlx::query("SELECT * FROM projects WHERE id = ?")
                .bind(&id)
                .fetch_optional(&pool)
                .await
            else {
                continue;
            };
            let cycle: i64 = project.try_get("cycle").unwrap_or(1);
            let milestones_total: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM milestones WHERE project_id = ? AND cycle = ?",
            )
            .bind(&id)
            .bind(cycle)
            .fetch_one(&pool)
            .await
            .unwrap_or(0);
            let milestones_done: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM milestones WHERE project_id = ? AND cycle = ? AND status = 'done'",
            )
            .bind(&id)
            .bind(cycle)
            .fetch_one(&pool)
            .await
            .unwrap_or(0);
            let items_total: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM items WHERE project_id = ? AND cycle = ?")
                    .bind(&id)
                    .bind(cycle)
                    .fetch_one(&pool)
                    .await
                    .unwrap_or(0);
            let items_done: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM items WHERE project_id = ? AND cycle = ? AND status = 'done'",
            )
            .bind(&id)
            .bind(cycle)
            .fetch_one(&pool)
            .await
            .unwrap_or(0);
            let awaiting: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM items WHERE project_id = ? AND cycle = ? AND status = 'awaiting_approval'",
            )
            .bind(&id)
            .bind(cycle)
            .fetch_one(&pool)
            .await
            .unwrap_or(0);
            let active_item = sqlx::query(
                "SELECT i.id, i.title, i.status, i.owner_member_id, COALESCE(m.display, i.owner_member) AS member_display FROM items i LEFT JOIN members m ON m.id = i.owner_member_id WHERE i.project_id = ? AND i.cycle = ? AND i.status IN ('doing', 'ready') ORDER BY CASE i.status WHEN 'doing' THEN 0 ELSE 1 END, i.id ASC LIMIT 1",
            )
            .bind(&id)
            .bind(cycle)
            .fetch_optional(&pool)
            .await
            .ok()
            .flatten()
            .map(|row| {
                json!({
                    "id": row.get::<String, _>("id"),
                    "title": row.get::<String, _>("title"),
                    "status": map_item_status(&row.get::<String, _>("status")),
                    "member": row.try_get::<Option<String>, _>("member_display").ok().flatten(),
                })
            });
            let last_message = latest_message(&pool, &id).await;
            let last_activity_ts: f64 =
                sqlx::query_scalar("SELECT COALESCE(MAX(ts), 0) FROM events WHERE project_id = ?")
                    .bind(&id)
                    .fetch_one(&pool)
                    .await
                    .unwrap_or(0.0);
            let participants = sqlx::query_scalar::<_, String>(
                "SELECT display FROM members WHERE project_id = ? AND status = 'active' ORDER BY created_at, id",
            )
            .bind(&id)
            .fetch_all(&pool)
            .await
            .unwrap_or_default();
            let spent_usd: f64 = sqlx::query_scalar(
                "SELECT COALESCE(SUM(cost_usd), 0) FROM group_runs WHERE project_id = ?",
            )
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap_or(0.0);
            rows.push(json!({
                "id": project.get::<String, _>("id"),
                "path": entry.path(),
                "title": project.try_get::<String, _>("title").ok().filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| project.get::<String, _>("goal")),
                "goal": project.get::<String, _>("goal"),
                "workspace_path": project.try_get::<String, _>("workspace_path").unwrap_or_default(),
                "status": map_project_status(&project.get::<String, _>("status")),
                "cycle": cycle,
                "plan_status": project.try_get::<String, _>("plan_status").unwrap_or_else(|_| "none".to_string()),
                "plan_review": project.try_get::<i64, _>("plan_review").unwrap_or(1) != 0,
                "recurrence": project.try_get::<String, _>("recurrence").unwrap_or_else(|_| "none".to_string()),
                "repeat_active": project.try_get::<i64, _>("repeat_active").unwrap_or(0) != 0,
                "start_mode": project.try_get::<String, _>("start_mode").unwrap_or_else(|_| "now".to_string()),
                "start_at": project.try_get::<Option<f64>, _>("start_at").ok().flatten(),
                "next_run_at": project.try_get::<Option<f64>, _>("next_run_at").ok().flatten(),
                "origin_workspace_id": origin_workspace_id,
                "shell_enabled": false,
                "spent_usd": spent_usd,
                "milestones_done": milestones_done,
                "milestones_total": milestones_total,
                "items_done": items_done,
                "items_total": items_total,
                "awaiting_approval": awaiting,
                "active_item": active_item,
                "active_member": active_item.as_ref().and_then(|item| item["member"].as_str()),
                "last_message": last_message,
                "last_activity_ts": last_activity_ts,
                "participants": participants,
                "members_count": participants.len(),
            }));
            pool.close().await;
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
    /// Atomically claim the first dependency-ready item with a free member
    /// slot. One member row is one parallel slot, so a member can have at most
    /// one item in flight. The project max_parallel caps the whole team.
    pub async fn claim_next_ready(&self, project_id: &str) -> Result<Option<Value>, String> {
        let path = self.root.join(project_id);
        let pool = open_pool(&path, false).await?;
        let project = sqlx::query(
            "SELECT id, goal, title, status, cycle, max_parallel FROM projects WHERE id = ?",
        )
        .bind(project_id)
        .fetch_optional(&pool)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("unknown project '{project_id}'"))?;
        if project.get::<String, _>("status") != "running" {
            return Ok(None);
        }
        let cycle: i64 = project.try_get("cycle").unwrap_or(1);
        let max_parallel = project
            .try_get::<i64, _>("max_parallel")
            .unwrap_or(1)
            .clamp(1, 8);
        let active_runs: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM items WHERE project_id = ? AND cycle = ? AND status = 'doing'",
        )
        .bind(project_id)
        .bind(cycle)
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
        if active_runs >= max_parallel {
            return Ok(None);
        }

        let member_rows = sqlx::query(
            "SELECT m.id, m.agent_key, m.display, (SELECT COUNT(*) FROM items i WHERE i.owner_member_id = m.id AND i.project_id = m.project_id AND i.cycle = ? AND i.status = 'doing') AS active, (SELECT COUNT(*) FROM items i WHERE i.owner_member_id = m.id AND i.project_id = m.project_id AND i.cycle = ?) AS assigned FROM members m WHERE m.project_id = ? AND m.status = 'active' ORDER BY assigned ASC, m.slot_index ASC, m.id ASC",
        )
        .bind(cycle)
        .bind(cycle)
        .bind(project_id)
        .fetch_all(&pool)
        .await
        .map_err(|error| error.to_string())?;
        let mut available: Vec<(String, String, String, i64)> = member_rows
            .iter()
            .filter(|row| row.try_get::<i64, _>("active").unwrap_or(0) == 0)
            .map(|row| {
                (
                    row.get::<String, _>("id"),
                    row.get::<String, _>("agent_key"),
                    row.get::<String, _>("display"),
                    row.try_get::<i64, _>("assigned").unwrap_or(0),
                )
            })
            .collect();
        if available.is_empty() {
            return Ok(None);
        }

        let candidates = sqlx::query(
            "SELECT i.id, i.milestone_id, i.title, i.detail, i.acceptance_json, i.tags_json, i.priority, i.requires_approval, i.owner_member_id, m.title AS milestone_title, m.depends_on_json FROM items i JOIN milestones m ON m.project_id = i.project_id AND m.id = i.milestone_id WHERE i.project_id = ? AND i.cycle = ? AND i.status IN ('todo', 'ready') AND (i.next_attempt_at IS NULL OR i.next_attempt_at <= ?) ORDER BY m.seq_no ASC, i.priority DESC, i.id ASC",
        )
        .bind(project_id)
        .bind(cycle)
        .bind(now())
        .fetch_all(&pool)
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
                    .fetch_optional(&pool)
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
            let requested_member = row
                .try_get::<Option<String>, _>("owner_member_id")
                .ok()
                .flatten();
            let chosen = requested_member
                .as_ref()
                .and_then(|member_id| {
                    available
                        .iter()
                        .enumerate()
                        .find(|(_, member)| &member.0 == member_id)
                        .map(|(index, _)| index)
                })
                .or_else(|| {
                    available
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, member)| (member.3, member.0.clone()))
                        .map(|(index, _)| index)
                });
            let Some(index) = chosen else {
                continue;
            };
            let member = available[index].clone();
            available[index].3 += 1;
            let lease_owner = format!(
                "group-runner-{}",
                &uuid::Uuid::new_v4().simple().to_string()[..8]
            );
            let lease_expires = now() + 2.0 * 60.0 * 60.0;
            let timestamp = now();
            let mut tx = crate::begin_immediate(&pool)
                .await
                .map_err(|error| error.to_string())?;
            let updated = sqlx::query(
                "UPDATE items SET status = 'doing', attempts = attempts + 1, owner_member_id = ?, lease_owner = ?, lease_expires = ?, result = NULL, error = NULL, started_at = ?, finished_at = NULL, next_attempt_at = NULL, updated_at = ? WHERE project_id = ? AND id = ? AND status IN ('todo', 'ready') AND (next_attempt_at IS NULL OR next_attempt_at <= ?)",
            )
            .bind(&member.0)
            .bind(&lease_owner)
            .bind(lease_expires)
            .bind(timestamp)
            .bind(timestamp)
            .bind(project_id)
            .bind(&item_id)
            .bind(timestamp)
            .execute(&mut *tx)
            .await
            .map_err(|error| error.to_string())?;
            if updated.rows_affected() == 0 {
                tx.rollback().await.ok();
                continue;
            }
            sqlx::query("UPDATE milestones SET status = 'active', updated_at = ? WHERE project_id = ? AND id = ? AND status IN ('todo', 'active')")
                .bind(timestamp)
                .bind(project_id)
                .bind(&milestone_id)
                .execute(&mut *tx)
                .await
                .map_err(|error| error.to_string())?;
            sqlx::query(
                "INSERT INTO events (project_id, ts, kind, actor, milestone_id, item_id, data) VALUES (?, ?, 'item.claimed', 'supervisor', ?, ?, ?)",
            )
            .bind(project_id)
            .bind(timestamp)
            .bind(&milestone_id)
            .bind(&item_id)
            .bind(
                json!({
                    "lease_owner": lease_owner,
                    "lease_expires": lease_expires,
                    "member_id": member.0,
                    "member": member.2,
                    "agent_key": member.1,
                })
                .to_string(),
            )
            .execute(&mut *tx)
            .await
            .map_err(|error| error.to_string())?;
            let assignment = format!("Assigned: {}", row.get::<String, _>("title"));
            sqlx::query(
                "INSERT INTO group_messages (project_id, ts, from_kind, from_label, to_kind, to_member_id, body, kind, item_id, deliver, delivered_at) VALUES (?, ?, 'supervisor', 'Orchestrator', 'member', ?, ?, 'assignment', ?, 1, ?)",
            )
            .bind(project_id)
            .bind(timestamp)
            .bind(&member.0)
            .bind(&assignment)
            .bind(&item_id)
            .bind(timestamp)
            .execute(&mut *tx)
            .await
            .map_err(|error| error.to_string())?;
            sqlx::query(
                "INSERT INTO events (project_id, ts, kind, actor, milestone_id, item_id, data) VALUES (?, ?, 'message.sent', 'supervisor', ?, ?, ?)",
            )
            .bind(project_id)
            .bind(timestamp)
            .bind(&milestone_id)
            .bind(&item_id)
            .bind(
                json!({
                    "from": "supervisor",
                    "to": member.0,
                    "body": assignment,
                    "kind": "assignment",
                    "deliver": false,
                })
                .to_string(),
            )
            .execute(&mut *tx)
            .await
            .map_err(|error| error.to_string())?;
            tx.commit().await.map_err(|error| error.to_string())?;
            return Ok(Some(json!({
                "id": item_id,
                "milestone_id": milestone_id,
                "title": row.get::<String, _>("title"),
                "detail": row.try_get::<Option<String>, _>("detail").ok().flatten().unwrap_or_default(),
                "acceptance": parse_json(row.get("acceptance_json")),
                "tags": parse_json(row.get("tags_json")),
                "priority": row.try_get::<i64, _>("priority").unwrap_or(0),
                "requires_approval": row.try_get::<i64, _>("requires_approval").unwrap_or(0) != 0,
                "owner_member_id": member.0,
                "member": {"id": member.0, "agent_key": member.1, "display": member.2},
                "milestone_title": row.get::<String, _>("milestone_title"),
                "goal": project.get::<String, _>("goal"),
                "title_project": project.try_get::<String, _>("title").unwrap_or_else(|_| project.get::<String, _>("goal")),
                "cycle": cycle,
            })));
        }
        Ok(None)
    }

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
            sqlx::query("UPDATE group_runs SET status = 'interrupted', finished_at = ?, error = COALESCE(error, 'server restarted') WHERE project_id = ? AND status = 'running'")
                .bind(now())
                .bind(&project_id)
                .execute(&pool)
                .await
                .map_err(|error| error.to_string())?;
            sqlx::query("UPDATE projects SET plan_status = 'queued' WHERE id = ? AND plan_status = 'running'")
                .bind(&project_id)
                .execute(&pool)
                .await
                .map_err(|error| error.to_string())?;
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
        self.finish_item_with_usage(project_id, item_id, success, output, 0, 0, 0.0, None)
            .await
    }

    /// Complete or retry one claimed item. Usage is the provider-reported
    /// tokens/cost for the run; callers that do not have it pass zero.
    #[allow(clippy::too_many_arguments)]
    pub async fn finish_item_with_usage(
        &self,
        project_id: &str,
        item_id: &str,
        success: bool,
        output: &str,
        input_tokens: i64,
        output_tokens: i64,
        cost_usd: f64,
        run_id: Option<&str>,
    ) -> Result<(), String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let item = sqlx::query(
            "SELECT i.milestone_id, i.owner_member_id, i.attempts, i.max_attempts, i.cycle, COALESCE(m.display, i.owner_member, 'Developer') AS owner_display FROM items i LEFT JOIN members m ON m.id = i.owner_member_id WHERE i.project_id = ? AND i.id = ? AND i.status = 'doing'",
        )
        .bind(project_id)
        .bind(item_id)
        .fetch_optional(&pool)
        .await
        .map_err(|error| error.to_string())?;
        let Some(item) = item else {
            // A cancelled/stopped item may already have been released while the
            // provider was finishing. Treat that as a no-op, not a failure.
            return Ok(());
        };
        let milestone_id: Option<String> = item.try_get("milestone_id").ok();
        let member_id: Option<String> = item.try_get("owner_member_id").ok().flatten();
        let member = item
            .try_get::<String, _>("owner_display")
            .unwrap_or_else(|_| "Developer".to_string());
        let attempts: i64 = item.try_get("attempts").unwrap_or(0);
        let max_attempts = item.try_get::<i64, _>("max_attempts").unwrap_or(3).max(1);
        let cycle: i64 = item.try_get("cycle").unwrap_or(1);
        let timestamp = now();
        let body: String = output.chars().take(8_000).collect();

        if success {
            sqlx::query(
                "UPDATE items SET status = 'done', result = ?, error = NULL, lease_owner = NULL, lease_expires = NULL, finished_at = ?, run_id = COALESCE(?, run_id), cost_tokens = cost_tokens + ?, cost_usd = cost_usd + ?, next_attempt_at = NULL, updated_at = ? WHERE project_id = ? AND id = ? AND status = 'doing'",
            )
            .bind(&body)
            .bind(timestamp)
            .bind(run_id)
            .bind(input_tokens + output_tokens)
            .bind(cost_usd)
            .bind(timestamp)
            .bind(project_id)
            .bind(item_id)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
            sqlx::query(
                "UPDATE projects SET spent_usd = COALESCE(spent_usd, 0) + ?, spent_tokens = COALESCE(spent_tokens, 0) + ?, updated_at = ? WHERE id = ?",
            )
            .bind(cost_usd)
            .bind(input_tokens + output_tokens)
            .bind(timestamp)
            .bind(project_id)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
            append_event(
                &pool,
                project_id,
                "item.done",
                &member,
                milestone_id.clone(),
                Some(item_id.to_string()),
                json!({"result": body, "run_id": run_id, "member_id": member_id, "cost_usd": cost_usd}),
            )
            .await?;
            self.append_message_record(
                &pool,
                project_id,
                "member",
                member_id.as_deref(),
                &member,
                "supervisor",
                None,
                if body.trim().is_empty() {
                    "Work item completed."
                } else {
                    &body
                },
                "report",
                Some(item_id),
                run_id,
            )
            .await?;
            if let Some(milestone_id) = milestone_id {
                self.refresh_milestone(&pool, project_id, &milestone_id, cycle)
                    .await?;
            }
            self.refresh_cycle(&pool, project_id, cycle).await?;
            return Ok(());
        }

        if attempts < max_attempts {
            let backoff = (30.0 * attempts as f64).clamp(30.0, 300.0);
            sqlx::query(
                "UPDATE items SET status = 'todo', error = ?, lease_owner = NULL, lease_expires = NULL, next_attempt_at = ?, updated_at = ? WHERE project_id = ? AND id = ? AND status = 'doing'",
            )
            .bind(&body)
            .bind(timestamp + backoff)
            .bind(timestamp)
            .bind(project_id)
            .bind(item_id)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
            append_event(
                &pool,
                project_id,
                "item.retry",
                &member,
                milestone_id.clone(),
                Some(item_id.to_string()),
                json!({"error": body, "attempt": attempts, "max_attempts": max_attempts, "retry_at": timestamp + backoff, "run_id": run_id, "member_id": member_id}),
            )
            .await?;
            return Ok(());
        }

        sqlx::query(
            "UPDATE items SET status = 'failed', result = NULL, error = ?, lease_owner = NULL, lease_expires = NULL, finished_at = ?, run_id = COALESCE(?, run_id), updated_at = ? WHERE project_id = ? AND id = ? AND status = 'doing'",
        )
        .bind(&body)
        .bind(timestamp)
        .bind(run_id)
        .bind(timestamp)
        .bind(project_id)
        .bind(item_id)
        .execute(&pool)
        .await
        .map_err(|error| error.to_string())?;
        append_event(
            &pool,
            project_id,
            "item.failed",
            &member,
            milestone_id.clone(),
            Some(item_id.to_string()),
            json!({"error": body, "attempts": attempts, "run_id": run_id, "member_id": member_id}),
        )
        .await?;
        self.append_message_record(
            &pool,
            project_id,
            "member",
            member_id.as_deref(),
            &member,
            "supervisor",
            None,
            if body.trim().is_empty() {
                "Work item failed."
            } else {
                &body
            },
            "report",
            Some(item_id),
            run_id,
        )
        .await?;
        if let Some(milestone_id) = milestone_id {
            sqlx::query("UPDATE milestones SET status = 'blocked', updated_at = ? WHERE project_id = ? AND id = ? AND status != 'done'")
                .bind(timestamp)
                .bind(project_id)
                .bind(&milestone_id)
                .execute(&pool)
                .await
                .map_err(|error| error.to_string())?;
            append_event(
                &pool,
                project_id,
                "milestone.blocked",
                "supervisor",
                Some(milestone_id),
                None,
                json!({"reason": "an item exhausted its retries"}),
            )
            .await?;
        }
        sqlx::query(
            "UPDATE projects SET status = 'blocked', halt_reason = ?, last_error = ?, updated_at = ? WHERE id = ? AND status = 'running'",
        )
        .bind("an item exhausted its retries")
        .bind(&body)
        .bind(timestamp)
        .bind(project_id)
        .execute(&pool)
        .await
        .map_err(|error| error.to_string())?;
        append_event(
            &pool,
            project_id,
            "project.blocked",
            "supervisor",
            None,
            Some(item_id.to_string()),
            json!({"reason": "item retries exhausted"}),
        )
        .await?;
        Ok(())
    }

    async fn refresh_milestone(
        &self,
        pool: &SqlitePool,
        project_id: &str,
        milestone_id: &str,
        cycle: i64,
    ) -> Result<(), String> {
        let failed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM items WHERE project_id = ? AND milestone_id = ? AND cycle = ? AND status IN ('failed', 'invalidated')",
        )
        .bind(project_id)
        .bind(milestone_id)
        .bind(cycle)
        .fetch_one(pool)
        .await
        .unwrap_or(0);
        if failed > 0 {
            sqlx::query("UPDATE milestones SET status = 'blocked', updated_at = ? WHERE project_id = ? AND id = ? AND status != 'done'")
                .bind(now())
                .bind(project_id)
                .bind(milestone_id)
                .execute(pool)
                .await
                .map_err(|error| error.to_string())?;
            return Ok(());
        }
        let remaining: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM items WHERE project_id = ? AND milestone_id = ? AND cycle = ? AND status NOT IN ('done', 'cancelled')",
        )
        .bind(project_id)
        .bind(milestone_id)
        .bind(cycle)
        .fetch_one(pool)
        .await
        .unwrap_or(0);
        if remaining == 0 {
            let updated = sqlx::query("UPDATE milestones SET status = 'done', updated_at = ? WHERE project_id = ? AND id = ? AND status != 'done'")
                .bind(now())
                .bind(project_id)
                .bind(milestone_id)
                .execute(pool)
                .await
                .map_err(|error| error.to_string())?;
            if updated.rows_affected() > 0 {
                append_event(
                    pool,
                    project_id,
                    "milestone.done",
                    "supervisor",
                    Some(milestone_id.to_string()),
                    None,
                    json!({}),
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn refresh_cycle(
        &self,
        pool: &SqlitePool,
        project_id: &str,
        cycle: i64,
    ) -> Result<(), String> {
        let unfinished: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM milestones WHERE project_id = ? AND cycle = ? AND status NOT IN ('done', 'cancelled')",
        )
        .bind(project_id)
        .bind(cycle)
        .fetch_one(pool)
        .await
        .unwrap_or(0);
        if unfinished > 0 {
            return Ok(());
        }
        let project =
            sqlx::query("SELECT recurrence, repeat_active, start_at FROM projects WHERE id = ?")
                .bind(project_id)
                .fetch_optional(pool)
                .await
                .map_err(|error| error.to_string())?;
        let Some(project) = project else {
            return Ok(());
        };
        let recurrence = project
            .try_get::<String, _>("recurrence")
            .unwrap_or_else(|_| "none".to_string());
        let repeat_active = project.try_get::<i64, _>("repeat_active").unwrap_or(0) != 0;
        let next_run_at = if repeat_active && recurrence != "none" {
            Some(next_occurrence(
                now(),
                &recurrence,
                project.try_get::<Option<f64>, _>("start_at").ok().flatten(),
            ))
        } else {
            None
        };
        let timestamp = now();
        sqlx::query(
            "UPDATE projects SET status = 'done', next_run_at = ?, halt_reason = NULL, last_error = NULL, updated_at = ? WHERE id = ?",
        )
        .bind(next_run_at)
        .bind(timestamp)
        .bind(project_id)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
        append_event(
            pool,
            project_id,
            "cycle.completed",
            "supervisor",
            None,
            None,
            json!({"cycle": cycle, "next_run_at": next_run_at, "recurrence": recurrence}),
        )
        .await?;
        if next_run_at.is_none() {
            append_event(
                pool,
                project_id,
                "project.closed",
                "supervisor",
                None,
                None,
                json!({"status": "done", "cycle": cycle}),
            )
            .await?;
        }
        Ok(())
    }

    /// Release one claimed item without counting it as a failed attempt. Used
    /// by Stop, pause-time cancellation, and shutdown recovery.
    pub async fn release_item(
        &self,
        project_id: &str,
        item_id: &str,
        reason: &str,
    ) -> Result<(), String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let updated = sqlx::query(
            "UPDATE items SET status = 'todo', lease_owner = NULL, lease_expires = NULL, started_at = NULL, attempts = MAX(attempts - 1, 0), updated_at = ? WHERE project_id = ? AND id = ? AND status = 'doing'",
        )
        .bind(now())
        .bind(project_id)
        .bind(item_id)
        .execute(&pool)
        .await
        .map_err(|error| error.to_string())?;
        if updated.rows_affected() > 0 {
            append_event(
                &pool,
                project_id,
                "item.released",
                "owner",
                None,
                Some(item_id.to_string()),
                json!({"reason": reason}),
            )
            .await?;
        }
        Ok(())
    }

    pub async fn release_doing_items(
        &self,
        project_id: &str,
        reason: &str,
    ) -> Result<usize, String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let rows = sqlx::query("SELECT id FROM items WHERE project_id = ? AND status = 'doing'")
            .bind(project_id)
            .fetch_all(&pool)
            .await
            .map_err(|error| error.to_string())?;
        let mut released = 0;
        for row in rows {
            let item_id: String = row.get("id");
            self.release_item(project_id, &item_id, reason).await?;
            released += 1;
        }
        Ok(released)
    }

    pub async fn active_runs(&self, project_id: &str) -> Result<Vec<Value>, String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let rows = sqlx::query(
            "SELECT id, item_id, member_id, chat_id, run_id, kind, started_at FROM group_runs WHERE project_id = ? AND status = 'running' ORDER BY started_at ASC",
        )
        .bind(project_id)
        .fetch_all(&pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(rows
            .into_iter()
            .map(|row| {
                json!({
                    "id": row.try_get::<String, _>("id").unwrap_or_default(),
                    "item_id": row.try_get::<Option<String>, _>("item_id").ok().flatten(),
                    "member_id": row.try_get::<Option<String>, _>("member_id").ok().flatten(),
                    "chat_id": row.try_get::<Option<String>, _>("chat_id").ok().flatten(),
                    "run_id": row.try_get::<Option<String>, _>("run_id").ok().flatten(),
                    "kind": row.try_get::<String, _>("kind").unwrap_or_else(|_| "item".to_string()),
                    "started_at": row.try_get::<Option<f64>, _>("started_at").ok().flatten(),
                })
            })
            .collect())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn start_run(
        &self,
        project_id: &str,
        cycle: i64,
        item_id: Option<&str>,
        member_id: Option<&str>,
        agent_key: &str,
        chat_id: Option<&str>,
        run_id: Option<&str>,
        kind: &str,
    ) -> Result<String, String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let id = format!(
            "group-run-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..12]
        );
        sqlx::query(
            "INSERT INTO group_runs (id, project_id, cycle, item_id, member_id, agent_key, chat_id, run_id, kind, status, started_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'running', ?)",
        )
        .bind(&id)
        .bind(project_id)
        .bind(cycle)
        .bind(item_id)
        .bind(member_id)
        .bind(agent_key)
        .bind(chat_id)
        .bind(run_id)
        .bind(kind)
        .bind(now())
        .execute(&pool)
        .await
        .map_err(|error| error.to_string())?;
        append_event(
            &pool,
            project_id,
            "run.started",
            "supervisor",
            None,
            item_id.map(str::to_string),
            json!({"run_id": id, "chat_id": chat_id, "member_id": member_id, "agent_key": agent_key, "kind": kind}),
        )
        .await?;
        Ok(id)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn finish_run(
        &self,
        project_id: &str,
        run_id: &str,
        status: &str,
        input_tokens: i64,
        output_tokens: i64,
        cost_usd: f64,
        report: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let kind: Option<String> = sqlx::query_scalar("SELECT kind FROM group_runs WHERE id = ?")
            .bind(run_id)
            .fetch_optional(&pool)
            .await
            .map_err(|error| error.to_string())?
            .flatten();
        sqlx::query(
            "UPDATE group_runs SET status = ?, finished_at = ?, input_tokens = ?, output_tokens = ?, cost_usd = ?, report = ?, error = ? WHERE project_id = ? AND id = ?",
        )
        .bind(status)
        .bind(now())
        .bind(input_tokens)
        .bind(output_tokens)
        .bind(cost_usd)
        .bind(report.map(|value| value.chars().take(12_000).collect::<String>()))
        .bind(error.map(|value| value.chars().take(8_000).collect::<String>()))
        .bind(project_id)
        .bind(run_id)
        .execute(&pool)
        .await
        .map_err(|error| error.to_string())?;
        if kind.as_deref() == Some("planning") {
            sqlx::query(
                "UPDATE projects SET spent_usd = COALESCE(spent_usd, 0) + ?, spent_tokens = COALESCE(spent_tokens, 0) + ?, updated_at = ? WHERE id = ?",
            )
            .bind(cost_usd)
            .bind(input_tokens + output_tokens)
            .bind(now())
            .bind(project_id)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
        }
        append_event(
            &pool,
            project_id,
            "run.finished",
            "supervisor",
            None,
            None,
            json!({"run_id": run_id, "status": status, "input_tokens": input_tokens, "output_tokens": output_tokens, "cost_usd": cost_usd, "kind": kind}),
        )
        .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn append_message_record(
        &self,
        pool: &SqlitePool,
        project_id: &str,
        from_kind: &str,
        from_member_id: Option<&str>,
        from_label: &str,
        to_kind: &str,
        to_member_id: Option<&str>,
        body: &str,
        kind: &str,
        item_id: Option<&str>,
        run_id: Option<&str>,
    ) -> Result<i64, String> {
        let timestamp = now();
        let result = sqlx::query(
            "INSERT INTO group_messages (project_id, ts, from_kind, from_member_id, from_label, to_kind, to_member_id, body, kind, item_id, run_id, deliver, delivered_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?)",
        )
        .bind(project_id)
        .bind(timestamp)
        .bind(from_kind)
        .bind(from_member_id)
        .bind(from_label)
        .bind(to_kind)
        .bind(to_member_id)
        .bind(body.chars().take(8_000).collect::<String>())
        .bind(kind)
        .bind(item_id)
        .bind(run_id)
        .bind(timestamp)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
        let seq = result.last_insert_rowid();
        append_event(
            pool,
            project_id,
            "message.sent",
            from_label,
            None,
            item_id.map(str::to_string),
            json!({"from": from_label, "to": to_member_id.unwrap_or(to_kind), "body": body.chars().take(8_000).collect::<String>(), "kind": kind, "deliver": false, "run_id": run_id, "seq": seq}),
        )
        .await?;
        Ok(seq)
    }

    pub async fn origin_chat(&self, project_id: &str) -> Option<String> {
        read_origin_chat(&self.root.join(project_id)).await
    }

    pub async fn detail(&self, project_id: &str, event_limit: usize) -> Result<Value, String> {
        validate_id(project_id)?;
        let path = self.root.join(project_id);
        if !path.is_dir() {
            return Err(format!("unknown project '{project_id}'"));
        }
        let origin_workspace_id = read_origin(&path).await;
        let origin_chat_id = read_origin_chat(&path).await;
        let pool = open_pool(&path, false).await?;
        let project = sqlx::query("SELECT * FROM projects WHERE id = ?")
            .bind(project_id)
            .fetch_optional(&pool)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("unknown project '{project_id}'"))?;
        let cycle: i64 = project.try_get("cycle").unwrap_or(1);
        let milestones = sqlx::query(
            "SELECT id, project_id, title, detail, status, seq_no, cycle, depends_on_json, acceptance_json, owner_member_id, due_at, updated_at FROM milestones WHERE project_id = ? ORDER BY cycle ASC, seq_no ASC, id ASC",
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
                "title": row.try_get::<String, _>("title").unwrap_or_default(),
                "detail": row.try_get::<String, _>("detail").unwrap_or_default(),
                "status": map_milestone_status(&row.get::<String, _>("status")),
                "seq_no": row.try_get::<i64, _>("seq_no").unwrap_or(0),
                "cycle": row.try_get::<i64, _>("cycle").unwrap_or(1),
                "depends_on": parse_json(row.get::<String, _>("depends_on_json")),
                "acceptance": parse_json(row.get::<String, _>("acceptance_json")),
                "owner_member_id": row.try_get::<Option<String>, _>("owner_member_id").ok().flatten(),
                "due_at": row.try_get::<Option<f64>, _>("due_at").ok().flatten(),
                "updated_at": row.get::<f64, _>("updated_at"),
            })
        })
        .collect::<Vec<_>>();
        let items = sqlx::query(
            "SELECT i.id, i.project_id, i.milestone_id, i.title, i.detail, i.status, i.cycle, i.acceptance_json, i.tags_json, i.result, i.error, i.attempts, i.max_attempts, i.owner_member, i.owner_member_id, i.lease_owner, i.blocked_reason, i.invalidated_reason, i.cost_tokens, i.cost_usd, i.priority, i.due_at, i.requires_approval, i.run_id, i.started_at, i.finished_at, i.next_attempt_at, i.updated_at, COALESCE(m.display, i.owner_member) AS owner_display FROM items i LEFT JOIN members m ON m.id = i.owner_member_id WHERE i.project_id = ? ORDER BY i.cycle ASC, i.milestone_id ASC, i.priority DESC, i.id ASC",
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
                "milestone_id": row.try_get::<Option<String>, _>("milestone_id").ok().flatten(),
                "title": row.try_get::<String, _>("title").unwrap_or_default(),
                "detail": row.try_get::<String, _>("detail").unwrap_or_default(),
                "status": map_item_status(&row.get::<String, _>("status")),
                "cycle": row.try_get::<i64, _>("cycle").unwrap_or(1),
                "acceptance": parse_json(row.get::<String, _>("acceptance_json")),
                "tags": parse_json(row.get::<String, _>("tags_json")),
                "attempts": row.try_get::<i64, _>("attempts").unwrap_or(0),
                "max_attempts": row.try_get::<i64, _>("max_attempts").unwrap_or(3),
                "owner_member": row.try_get::<Option<String>, _>("owner_member").ok().flatten(),
                "owner_member_id": row.try_get::<Option<String>, _>("owner_member_id").ok().flatten(),
                "owner_display": row.try_get::<Option<String>, _>("owner_display").ok().flatten(),
                "lease_owner": row.try_get::<Option<String>, _>("lease_owner").ok().flatten(),
                "blocked_reason": row.try_get::<Option<String>, _>("blocked_reason").ok().flatten(),
                "invalidated_reason": row.try_get::<Option<String>, _>("invalidated_reason").ok().flatten(),
                "result": row.try_get::<Option<String>, _>("result").ok().flatten(),
                "error": row.try_get::<Option<String>, _>("error").ok().flatten(),
                "cost_tokens": row.try_get::<i64, _>("cost_tokens").unwrap_or(0),
                "cost_usd": row.try_get::<Option<f64>, _>("cost_usd").ok().flatten(),
                "priority": row.try_get::<i64, _>("priority").unwrap_or(0),
                "due_at": row.try_get::<Option<f64>, _>("due_at").ok().flatten(),
                "requires_approval": row.try_get::<i64, _>("requires_approval").unwrap_or(0) != 0,
                "run_id": row.try_get::<Option<String>, _>("run_id").ok().flatten(),
                "started_at": row.try_get::<Option<f64>, _>("started_at").ok().flatten(),
                "finished_at": row.try_get::<Option<f64>, _>("finished_at").ok().flatten(),
                "next_attempt_at": row.try_get::<Option<f64>, _>("next_attempt_at").ok().flatten(),
                "updated_at": row.get::<f64, _>("updated_at"),
            })
        })
        .collect::<Vec<_>>();
        let events = recent_events(&pool, project_id, event_limit).await?;
        let mut messages = messages_for(&pool, project_id, 200).await?;
        if messages.is_empty() {
            messages = messages_from_events(&events);
        }
        let members = members_json(&pool, project_id, cycle).await?;
        let runs = runs_json(&pool, project_id, 40).await?;
        let usage = sqlx::query(
            "SELECT COUNT(*) AS runs, COALESCE(SUM(input_tokens), 0) AS input_tokens, COALESCE(SUM(output_tokens), 0) AS output_tokens, COALESCE(SUM(cost_usd), 0) AS cost_usd, COALESCE(SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), 0) AS failed FROM group_runs WHERE project_id = ?",
        )
        .bind(project_id)
        .fetch_one(&pool)
        .await
        .map_err(|error| error.to_string())?;
        let usage = json!({
            "runs": usage.try_get::<i64, _>("runs").unwrap_or(0),
            "input_tokens": usage.try_get::<i64, _>("input_tokens").unwrap_or(0),
            "output_tokens": usage.try_get::<i64, _>("output_tokens").unwrap_or(0),
            "cost_usd": usage.try_get::<f64, _>("cost_usd").unwrap_or(0.0),
            "failed_runs": usage.try_get::<i64, _>("failed").unwrap_or(0),
        });
        let cycles = sqlx::query(
            "SELECT cycle, COUNT(*) AS total, COALESCE(SUM(CASE WHEN status = 'done' THEN 1 ELSE 0 END), 0) AS done FROM milestones WHERE project_id = ? GROUP BY cycle ORDER BY cycle DESC",
        )
        .bind(project_id)
        .fetch_all(&pool)
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|row| {
            json!({
                "cycle": row.try_get::<i64, _>("cycle").unwrap_or(1),
                "milestones_total": row.try_get::<i64, _>("total").unwrap_or(0),
                "milestones_done": row.try_get::<i64, _>("done").unwrap_or(0),
            })
        })
        .collect::<Vec<_>>();
        let status = project.get::<String, _>("status");
        let title = project
            .try_get::<String, _>("title")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| project.get::<String, _>("goal"));
        pool.close().await;
        Ok(json!({
            "id": project.get::<String, _>("id"),
            "path": path,
            "workspace_path": project.try_get::<String, _>("workspace_path").unwrap_or_default(),
            "shell_enabled": false,
            "current_cycle": cycle,
            "plan_status": project.try_get::<String, _>("plan_status").unwrap_or_else(|_| "none".to_string()),
            "project": {
                "id": project.get::<String, _>("id"),
                "title": title,
                "goal": project.get::<String, _>("goal"),
                "status": map_project_status(&status),
                "cycle": cycle,
                "origin_workspace_id": origin_workspace_id,
                "origin_chat_id": origin_chat_id,
                "workspace_path": project.try_get::<String, _>("workspace_path").unwrap_or_default(),
                "budget_usd": project.try_get::<Option<f64>, _>("budget_usd").ok().flatten(),
                "spent_usd": project.try_get::<f64, _>("spent_usd").unwrap_or(0.0),
                "deadline": project.try_get::<Option<String>, _>("deadline").ok().flatten(),
                "halt_reason": project.try_get::<Option<String>, _>("halt_reason").ok().flatten(),
                "last_error": project.try_get::<Option<String>, _>("last_error").ok().flatten(),
                "plan_status": project.try_get::<String, _>("plan_status").unwrap_or_else(|_| "none".to_string()),
                "plan_review": project.try_get::<i64, _>("plan_review").unwrap_or(1) != 0,
                "planner_agent": project.try_get::<String, _>("planner_agent").unwrap_or_else(|_| "orchestrator".to_string()),
                "max_parallel": project.try_get::<i64, _>("max_parallel").unwrap_or(1),
                "schedule": {
                    "start_mode": project.try_get::<String, _>("start_mode").unwrap_or_else(|_| "now".to_string()),
                    "start_at": project.try_get::<Option<f64>, _>("start_at").ok().flatten(),
                    "next_run_at": project.try_get::<Option<f64>, _>("next_run_at").ok().flatten(),
                    "recurrence": project.try_get::<String, _>("recurrence").unwrap_or_else(|_| "none".to_string()),
                    "repeat_active": project.try_get::<i64, _>("repeat_active").unwrap_or(0) != 0,
                },
                "usage": usage.clone(),
                "cycles": cycles,
            },
            "members": members,
            "messages": messages,
            "milestones": milestones,
            "items": items,
            "runs": runs,
            "usage": usage,
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
        let message = match op {
            "start" | "resume" => self.start_project(project_id).await?,
            "pause" => self.pause_project(project_id).await?,
            "stop" => self.stop_project(project_id).await?,
            "close" => self.close_project(project_id).await?,
            "restart_cycle" | "run_again" => {
                let detail = self.start_next_cycle(project_id).await?;
                format!(
                    "Cycle {} started.",
                    detail["current_cycle"].as_i64().unwrap_or(1)
                )
            }
            "set_settings" | "settings" => self.set_project_settings(project_id, params).await?,
            "set_schedule" | "schedule" => {
                let mut settings = params.clone();
                if let Some(object) = settings.as_object_mut() {
                    object.insert("schedule_only".to_string(), json!(true));
                }
                self.set_project_settings(project_id, &settings).await?
            }
            "add_milestone" => self.add_milestone(project_id, params).await?,
            "update_milestone" => self.update_milestone(project_id, params).await?,
            "delete_milestone" => self.delete_milestone(project_id, params).await?,
            "add_item" => self.add_item(project_id, params).await?,
            "update_item" => self.update_item(project_id, params).await?,
            "delete_item" => self.delete_item(project_id, params).await?,
            "retry_item" => self.retry_item(project_id, params).await?,
            "cancel_item" | "invalidate_item" | "reopen_item" | "complete_item" => {
                self.set_item_status(project_id, op, params).await?
            }
            "add_member" => self.add_member(project_id, params).await?,
            "remove_member" => self.remove_member(project_id, params).await?,
            "message" => {
                let body = params
                    .get("body")
                    .or_else(|| params.get("content"))
                    .or_else(|| params.get("message"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| "message needs a non-empty body".to_string())?;
                let to = params
                    .get("to")
                    .or_else(|| params.get("target"))
                    .and_then(Value::as_str)
                    .unwrap_or("all");
                let kind = params.get("kind").and_then(Value::as_str).unwrap_or("note");
                self.send_message(
                    project_id,
                    body,
                    to,
                    kind,
                    "owner",
                    None,
                    "owner",
                    params.get("item_id").and_then(Value::as_str),
                    None,
                    true,
                )
                .await?;
                format!("Message sent to {to}.")
            }
            "approve_plan" | "plan_approve" => {
                self.approve_plan(project_id).await?;
                "Plan approved.".to_string()
            }
            "set_plan" => {
                let plan = params
                    .get("plan")
                    .cloned()
                    .unwrap_or_else(|| params.clone());
                self.apply_plan(project_id, &plan).await?;
                "Plan applied.".to_string()
            }
            "set_planning" | "retry_plan" => {
                self.set_planning(project_id).await?;
                "Planning queued.".to_string()
            }
            "set_workspace" => {
                let path = params
                    .get("workspace_path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "set_workspace needs workspace_path".to_string())?;
                self.set_workspace_path(project_id, path).await?;
                "Workspace updated.".to_string()
            }
            other => return Err(format!("unsupported op '{other}'")),
        };
        Ok(json!({"ok": true, "queued": false, "message": message}))
    }

    async fn start_project(&self, project_id: &str) -> Result<String, String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let project = sqlx::query(
            "SELECT status, plan_status, cycle, start_mode, start_at FROM projects WHERE id = ?",
        )
        .bind(project_id)
        .fetch_optional(&pool)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("unknown project '{project_id}'"))?;
        let status: String = project.get("status");
        if status == "done" {
            return Err("this cycle is done; use run_again to start a new cycle".to_string());
        }
        let cycle: i64 = project.try_get("cycle").unwrap_or(1);
        let active_runs: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM group_runs WHERE project_id = ? AND status = 'running'",
        )
        .bind(project_id)
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
        if active_runs > 0 && status == "running" {
            return Ok("Project is already running.".to_string());
        }
        let milestones: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM milestones WHERE project_id = ? AND cycle = ?",
        )
        .bind(project_id)
        .bind(cycle)
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
        let plan_status: String = project
            .try_get("plan_status")
            .unwrap_or_else(|_| "none".to_string());
        let next_status = if plan_status == "queued" || (milestones == 0 && plan_status != "none") {
            "planning"
        } else if milestones > 0 {
            "running"
        } else {
            return Err("add a milestone or run the planner before starting".to_string());
        };
        sqlx::query("UPDATE projects SET status = ?, halt_reason = NULL, last_error = NULL, updated_at = ? WHERE id = ?")
            .bind(next_status)
            .bind(now())
            .bind(project_id)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
        append_event(
            &pool,
            project_id,
            "project.resumed",
            "owner",
            None,
            None,
            json!({"status": next_status}),
        )
        .await?;
        Ok(format!("Project {next_status}."))
    }

    async fn pause_project(&self, project_id: &str) -> Result<String, String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        sqlx::query("UPDATE projects SET status = 'paused', updated_at = ? WHERE id = ?")
            .bind(now())
            .bind(project_id)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
        append_event(
            &pool,
            project_id,
            "project.paused",
            "owner",
            None,
            None,
            json!({}),
        )
        .await?;
        Ok("Project paused. The current item may finish; no new work starts.".to_string())
    }

    async fn stop_project(&self, project_id: &str) -> Result<String, String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        sqlx::query("UPDATE projects SET status = 'paused', updated_at = ? WHERE id = ?")
            .bind(now())
            .bind(project_id)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
        sqlx::query("UPDATE group_runs SET status = 'stopped', finished_at = ?, error = COALESCE(error, 'stopped by owner') WHERE project_id = ? AND status = 'running'")
            .bind(now())
            .bind(project_id)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
        let released = self
            .release_doing_items(project_id, "stopped by owner")
            .await?;
        append_event(
            &pool,
            project_id,
            "project.stopped",
            "owner",
            None,
            None,
            json!({"released_items": released}),
        )
        .await?;
        Ok(format!("Project stopped; {released} item(s) released."))
    }

    async fn close_project(&self, project_id: &str) -> Result<String, String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        sqlx::query("UPDATE projects SET status = 'done', repeat_active = 0, next_run_at = NULL, updated_at = ? WHERE id = ?")
            .bind(now())
            .bind(project_id)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
        sqlx::query("UPDATE group_runs SET status = 'stopped', finished_at = ?, error = COALESCE(error, 'project closed') WHERE project_id = ? AND status = 'running'")
            .bind(now())
            .bind(project_id)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
        self.release_doing_items(project_id, "project closed")
            .await?;
        append_event(
            &pool,
            project_id,
            "project.closed",
            "owner",
            None,
            None,
            json!({"status": "done"}),
        )
        .await?;
        Ok("Project closed.".to_string())
    }

    async fn set_project_settings(
        &self,
        project_id: &str,
        params: &Value,
    ) -> Result<String, String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let schedule_only = params
            .get("schedule_only")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let current = sqlx::query("SELECT * FROM projects WHERE id = ?")
            .bind(project_id)
            .fetch_optional(&pool)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("unknown project '{project_id}'"))?;
        let title = params
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| current.get::<String, _>("title"));
        let goal = params
            .get("goal")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| current.get::<String, _>("goal"));
        if title.chars().count() > 160 {
            return Err("title is too long (max 160)".to_string());
        }
        if goal.chars().count() > 6_000 {
            return Err("goal is too long (max 6000)".to_string());
        }
        let workspace_path = params
            .get("workspace_path")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                current
                    .try_get::<String, _>("workspace_path")
                    .unwrap_or_default()
            });
        if !workspace_path.is_empty() && !Path::new(&workspace_path).is_dir() {
            return Err(format!("workspace folder not found: {workspace_path}"));
        }
        let recurrence = params
            .get("recurrence")
            .and_then(Value::as_str)
            .map(|value| value.trim().to_ascii_lowercase())
            .unwrap_or_else(|| {
                current
                    .try_get::<String, _>("recurrence")
                    .unwrap_or_else(|_| "none".to_string())
            });
        let recurrence = match recurrence.as_str() {
            "daily" | "weekly" | "monthly" => recurrence,
            _ => "none".to_string(),
        };
        let repeat_active = params
            .get("repeat_active")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| current.try_get::<i64, _>("repeat_active").unwrap_or(0) != 0)
            && recurrence != "none";
        let plan_review = params
            .get("plan_review")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| current.try_get::<i64, _>("plan_review").unwrap_or(1) != 0);
        let planner_agent = params
            .get("planner_agent")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase())
            .unwrap_or_else(|| {
                current
                    .try_get::<String, _>("planner_agent")
                    .unwrap_or_else(|_| "orchestrator".to_string())
            });
        let max_parallel = params
            .get("max_parallel")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| current.try_get::<i64, _>("max_parallel").unwrap_or(1))
            .clamp(1, 8);
        let start_mode = params
            .get("start_mode")
            .and_then(Value::as_str)
            .map(|value| value.trim().to_ascii_lowercase())
            .map(|value| match value.as_str() {
                "scheduled" | "schedule" | "later" => "scheduled".to_string(),
                "manual" | "draft" => "manual".to_string(),
                _ => "now".to_string(),
            })
            .unwrap_or_else(|| {
                current
                    .try_get::<String, _>("start_mode")
                    .unwrap_or_else(|_| "now".to_string())
            });
        let start_at = if params.get("start_at").is_some() {
            parse_time(params.get("start_at"))
        } else {
            current.try_get::<Option<f64>, _>("start_at").ok().flatten()
        };
        let next_run_at = if params.get("next_run_at").is_some() {
            parse_time(params.get("next_run_at"))
        } else if repeat_active {
            current
                .try_get::<Option<f64>, _>("next_run_at")
                .ok()
                .flatten()
        } else {
            None
        };
        let status = current.get::<String, _>("status");
        let next_status = if start_mode == "scheduled"
            && start_at.is_some_and(|at| at > now())
            && !matches!(status.as_str(), "running" | "paused" | "blocked")
        {
            "scheduled".to_string()
        } else {
            status
        };
        let mut assignments = vec![
            "title = ?",
            "goal = ?",
            "workspace_path = ?",
            "recurrence = ?",
            "repeat_active = ?",
            "plan_review = ?",
            "planner_agent = ?",
            "max_parallel = ?",
            "start_mode = ?",
            "start_at = ?",
            "next_run_at = ?",
            "status = ?",
            "updated_at = ?",
        ];
        if schedule_only {
            assignments = vec![
                "recurrence = ?",
                "repeat_active = ?",
                "start_mode = ?",
                "start_at = ?",
                "next_run_at = ?",
                "status = ?",
                "updated_at = ?",
            ];
        }
        let sql = format!(
            "UPDATE projects SET {} WHERE id = ?",
            assignments.join(", ")
        );
        let mut query = sqlx::query(sqlx::AssertSqlSafe(sql.clone()));
        if !schedule_only {
            query = query
                .bind(&title)
                .bind(&goal)
                .bind(&workspace_path)
                .bind(&recurrence)
                .bind(if repeat_active { 1 } else { 0 })
                .bind(if plan_review { 1 } else { 0 })
                .bind(&planner_agent)
                .bind(max_parallel);
        } else {
            query = query
                .bind(&recurrence)
                .bind(if repeat_active { 1 } else { 0 });
        }
        query
            .bind(&start_mode)
            .bind(start_at)
            .bind(next_run_at)
            .bind(next_status)
            .bind(now())
            .bind(project_id)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
        if workspace_path
            != current
                .try_get::<String, _>("workspace_path")
                .unwrap_or_default()
        {
            self.set_workspace_path(project_id, &workspace_path).await?;
        }
        append_event(
            &pool,
            project_id,
            "project.updated",
            "owner",
            None,
            None,
            json!({
                "title": title,
                "goal": goal,
                "workspace_path": workspace_path,
                "recurrence": recurrence,
                "repeat_active": repeat_active,
                "start_mode": start_mode,
                "start_at": start_at,
                "max_parallel": max_parallel,
                "plan_review": plan_review,
                "planner_agent": planner_agent,
            }),
        )
        .await?;
        Ok("Project settings saved.".to_string())
    }

    async fn add_milestone(&self, project_id: &str, params: &Value) -> Result<String, String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let title = params
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "add_milestone needs title".to_string())?;
        let cycle = self.current_cycle(&pool, project_id).await?;
        let id = params
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("ms-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
        validate_id(&id)?;
        let position: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(seq_no), -1) + 1 FROM milestones WHERE project_id = ? AND cycle = ?",
        )
        .bind(project_id)
        .bind(cycle)
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
        sqlx::query(
            "INSERT INTO milestones (id, project_id, title, detail, status, seq_no, cycle, depends_on_json, acceptance_json, owner_member_id, due_at, updated_at) VALUES (?, ?, ?, ?, 'todo', ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(project_id)
        .bind(title)
        .bind(params.get("detail").and_then(Value::as_str).unwrap_or(""))
        .bind(position)
        .bind(cycle)
        .bind(params.get("depends_on").cloned().unwrap_or_else(|| json!([])).to_string())
        .bind(params.get("acceptance").cloned().unwrap_or_else(|| json!([])).to_string())
        .bind(params.get("owner_member_id").and_then(Value::as_str))
        .bind(parse_time(params.get("due_at")))
        .bind(now())
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
            json!({"title": title, "cycle": cycle}),
        )
        .await?;
        Ok(format!("Added milestone {id}."))
    }

    async fn update_milestone(&self, project_id: &str, params: &Value) -> Result<String, String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let milestone_id = params
            .get("milestone_id")
            .or_else(|| params.get("id"))
            .and_then(Value::as_str)
            .ok_or_else(|| "update_milestone needs milestone_id".to_string())?;
        let current = sqlx::query("SELECT * FROM milestones WHERE project_id = ? AND id = ?")
            .bind(project_id)
            .bind(milestone_id)
            .fetch_optional(&pool)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("unknown milestone '{milestone_id}'"))?;
        let title = params
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| current.get::<String, _>("title"));
        let detail = params
            .get("detail")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| current.try_get::<String, _>("detail").unwrap_or_default());
        let status = params
            .get("status")
            .and_then(Value::as_str)
            .map(normalize_milestone_status)
            .unwrap_or_else(|| current.get::<String, _>("status"));
        let cycle = current.try_get::<i64, _>("cycle").unwrap_or(1);
        let due_at = if params.get("due_at").is_some() {
            parse_time(params.get("due_at"))
        } else {
            current.try_get::<Option<f64>, _>("due_at").ok().flatten()
        };
        let owner_member_id = params
            .get("owner_member_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                current
                    .try_get::<Option<String>, _>("owner_member_id")
                    .ok()
                    .flatten()
            });
        let depends_on = params
            .get("depends_on")
            .cloned()
            .unwrap_or_else(|| parse_json(current.get::<String, _>("depends_on_json")));
        let acceptance = params
            .get("acceptance")
            .cloned()
            .unwrap_or_else(|| parse_json(current.get::<String, _>("acceptance_json")));
        sqlx::query(
            "UPDATE milestones SET title = ?, detail = ?, status = ?, depends_on_json = ?, acceptance_json = ?, owner_member_id = ?, due_at = ?, updated_at = ? WHERE project_id = ? AND id = ?",
        )
        .bind(&title)
        .bind(&detail)
        .bind(&status)
        .bind(depends_on.to_string())
        .bind(acceptance.to_string())
        .bind(&owner_member_id)
        .bind(due_at)
        .bind(now())
        .bind(project_id)
        .bind(milestone_id)
        .execute(&pool)
        .await
        .map_err(|error| error.to_string())?;
        if status == "done" {
            sqlx::query("UPDATE items SET status = 'cancelled', updated_at = ? WHERE project_id = ? AND milestone_id = ? AND cycle = ? AND status NOT IN ('done', 'cancelled')")
                .bind(now())
                .bind(project_id)
                .bind(milestone_id)
                .bind(cycle)
                .execute(&pool)
                .await
                .map_err(|error| error.to_string())?;
        }
        if status == "done" || status == "cancelled" {
            self.refresh_cycle(&pool, project_id, cycle).await?;
        } else if status == "active" {
            sqlx::query("UPDATE projects SET status = 'running', halt_reason = NULL, updated_at = ? WHERE id = ? AND status IN ('done', 'blocked')")
                .bind(now())
                .bind(project_id)
                .execute(&pool)
                .await
                .map_err(|error| error.to_string())?;
        }
        append_event(
            &pool,
            project_id,
            "milestone.updated",
            "owner",
            Some(milestone_id.to_string()),
            None,
            json!({"title": title, "status": status}),
        )
        .await?;
        Ok(format!("Milestone {milestone_id} updated."))
    }

    async fn delete_milestone(&self, project_id: &str, params: &Value) -> Result<String, String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let milestone_id = params
            .get("milestone_id")
            .or_else(|| params.get("id"))
            .and_then(Value::as_str)
            .ok_or_else(|| "delete_milestone needs milestone_id".to_string())?;
        let doing: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM items WHERE project_id = ? AND milestone_id = ? AND status = 'doing'",
        )
        .bind(project_id)
        .bind(milestone_id)
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
        if doing > 0 {
            return Err("stop the running item before deleting this milestone".to_string());
        }
        sqlx::query("DELETE FROM items WHERE project_id = ? AND milestone_id = ?")
            .bind(project_id)
            .bind(milestone_id)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
        let deleted = sqlx::query("DELETE FROM milestones WHERE project_id = ? AND id = ?")
            .bind(project_id)
            .bind(milestone_id)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
        if deleted.rows_affected() == 0 {
            return Err(format!("unknown milestone '{milestone_id}'"));
        }
        append_event(
            &pool,
            project_id,
            "milestone.deleted",
            "owner",
            Some(milestone_id.to_string()),
            None,
            json!({}),
        )
        .await?;
        Ok(format!("Deleted milestone {milestone_id}."))
    }

    async fn add_item(&self, project_id: &str, params: &Value) -> Result<String, String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let milestone_id = params
            .get("milestone_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "add_item needs milestone_id".to_string())?;
        let title = params
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "add_item needs title".to_string())?;
        let cycle = self.current_cycle(&pool, project_id).await?;
        let exists: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM milestones WHERE project_id = ? AND id = ?")
                .bind(project_id)
                .bind(milestone_id)
                .fetch_one(&pool)
                .await
                .unwrap_or(0);
        if exists == 0 {
            return Err(format!("unknown milestone '{milestone_id}'"));
        }
        let mut item = params.clone();
        if let Some(object) = item.as_object_mut() {
            object.insert("cycle".to_string(), json!(cycle));
        }
        let id = insert_item(&pool, project_id, milestone_id, &item, 0, now()).await?;
        append_event(
            &pool,
            project_id,
            "item.created",
            "owner",
            Some(milestone_id.to_string()),
            Some(id.clone()),
            json!({"title": title, "cycle": cycle}),
        )
        .await?;
        Ok(format!("Added item {id}."))
    }

    async fn update_item(&self, project_id: &str, params: &Value) -> Result<String, String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let item_id = params
            .get("item_id")
            .or_else(|| params.get("id"))
            .and_then(Value::as_str)
            .ok_or_else(|| "update_item needs item_id".to_string())?;
        let current = sqlx::query("SELECT * FROM items WHERE project_id = ? AND id = ?")
            .bind(project_id)
            .bind(item_id)
            .fetch_optional(&pool)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("unknown item '{item_id}'"))?;
        let title = params
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| current.get::<String, _>("title"));
        let detail = params
            .get("detail")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| current.try_get::<String, _>("detail").unwrap_or_default());
        let status = params
            .get("status")
            .and_then(Value::as_str)
            .map(normalize_item_status)
            .unwrap_or_else(|| current.get::<String, _>("status"));
        if current.get::<String, _>("status") == "doing" && status != "doing" {
            return Err("stop the running item before changing its status".to_string());
        }
        let owner_member_id = params
            .get("owner_member_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                current
                    .try_get::<Option<String>, _>("owner_member_id")
                    .ok()
                    .flatten()
            });
        let owner_display: Option<String> = if let Some(member_id) = owner_member_id.as_deref() {
            sqlx::query_scalar("SELECT display FROM members WHERE project_id = ? AND id = ?")
                .bind(project_id)
                .bind(member_id)
                .fetch_optional(&pool)
                .await
                .ok()
                .flatten()
        } else {
            None
        };
        let priority = params
            .get("priority")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| current.try_get::<i64, _>("priority").unwrap_or(0));
        let due_at = if params.get("due_at").is_some() {
            parse_time(params.get("due_at"))
        } else {
            current.try_get::<Option<f64>, _>("due_at").ok().flatten()
        };
        let requires_approval = params
            .get("requires_approval")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| current.try_get::<i64, _>("requires_approval").unwrap_or(0) != 0);
        let acceptance = params
            .get("acceptance")
            .cloned()
            .unwrap_or_else(|| parse_json(current.get::<String, _>("acceptance_json")));
        let cycle = current.try_get::<i64, _>("cycle").unwrap_or(1);
        let result_note = if status == "done" {
            Some("Marked done by the owner.")
        } else {
            None
        };
        sqlx::query(
            "UPDATE items SET title = ?, detail = ?, status = ?, owner_member_id = ?, owner_member = ?, priority = ?, due_at = ?, requires_approval = ?, acceptance_json = ?, result = COALESCE(?, result), finished_at = CASE WHEN ? = 'done' THEN ? ELSE finished_at END, updated_at = ? WHERE project_id = ? AND id = ?",
        )
        .bind(&title)
        .bind(&detail)
        .bind(&status)
        .bind(&owner_member_id)
        .bind(owner_display.unwrap_or_default())
        .bind(priority)
        .bind(due_at)
        .bind(if requires_approval { 1 } else { 0 })
        .bind(acceptance.to_string())
        .bind(result_note)
        .bind(&status)
        .bind(now())
        .bind(now())
        .bind(project_id)
        .bind(item_id)
        .execute(&pool)
        .await
        .map_err(|error| error.to_string())?;
        if status == "done" || status == "cancelled" {
            if let Some(milestone_id) = current
                .try_get::<Option<String>, _>("milestone_id")
                .ok()
                .flatten()
            {
                self.refresh_milestone(&pool, project_id, &milestone_id, cycle)
                    .await?;
            }
            self.refresh_cycle(&pool, project_id, cycle).await?;
        }
        append_event(
            &pool,
            project_id,
            "item.updated",
            "owner",
            current
                .try_get::<Option<String>, _>("milestone_id")
                .ok()
                .flatten(),
            Some(item_id.to_string()),
            json!({"title": title, "status": status}),
        )
        .await?;
        Ok(format!("Item {item_id} updated."))
    }

    async fn delete_item(&self, project_id: &str, params: &Value) -> Result<String, String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let item_id = params
            .get("item_id")
            .or_else(|| params.get("id"))
            .and_then(Value::as_str)
            .ok_or_else(|| "delete_item needs item_id".to_string())?;
        let status: Option<String> =
            sqlx::query_scalar("SELECT status FROM items WHERE project_id = ? AND id = ?")
                .bind(project_id)
                .bind(item_id)
                .fetch_optional(&pool)
                .await
                .map_err(|error| error.to_string())?
                .flatten();
        let Some(status) = status else {
            return Err(format!("unknown item '{item_id}'"));
        };
        if status == "doing" {
            return Err("stop the running item before deleting it".to_string());
        }
        sqlx::query("DELETE FROM items WHERE project_id = ? AND id = ?")
            .bind(project_id)
            .bind(item_id)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
        append_event(
            &pool,
            project_id,
            "item.deleted",
            "owner",
            None,
            Some(item_id.to_string()),
            json!({}),
        )
        .await?;
        Ok(format!("Deleted item {item_id}."))
    }

    async fn retry_item(&self, project_id: &str, params: &Value) -> Result<String, String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let item_id = params
            .get("item_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "retry_item needs item_id".to_string())?;
        let updated = sqlx::query(
            "UPDATE items SET status = 'todo', attempts = 0, error = NULL, blocked_reason = NULL, invalidated_reason = NULL, next_attempt_at = NULL, lease_owner = NULL, lease_expires = NULL, updated_at = ? WHERE project_id = ? AND id = ? AND status IN ('failed', 'cancelled', 'invalidated', 'blocked')",
        )
        .bind(now())
        .bind(project_id)
        .bind(item_id)
        .execute(&pool)
        .await
        .map_err(|error| error.to_string())?;
        if updated.rows_affected() == 0 {
            return Err(format!("item '{item_id}' is not retryable"));
        }
        sqlx::query("UPDATE projects SET status = 'running', halt_reason = NULL, last_error = NULL, updated_at = ? WHERE id = ? AND status = 'blocked'")
            .bind(now())
            .bind(project_id)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
        if let Some(milestone_id) = sqlx::query_scalar::<_, Option<String>>(
            "SELECT milestone_id FROM items WHERE project_id = ? AND id = ?",
        )
        .bind(project_id)
        .bind(item_id)
        .fetch_optional(&pool)
        .await
        .ok()
        .flatten()
        .flatten()
        {
            sqlx::query("UPDATE milestones SET status = 'active', updated_at = ? WHERE project_id = ? AND id = ? AND status = 'blocked'")
                .bind(now())
                .bind(project_id)
                .bind(milestone_id)
                .execute(&pool)
                .await
                .map_err(|error| error.to_string())?;
        }
        append_event(
            &pool,
            project_id,
            "item.retried",
            "owner",
            None,
            Some(item_id.to_string()),
            json!({}),
        )
        .await?;
        Ok(format!("Item {item_id} queued for retry."))
    }

    async fn set_item_status(
        &self,
        project_id: &str,
        op: &str,
        params: &Value,
    ) -> Result<String, String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let item_id = params
            .get("item_id")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{op} needs item_id"))?;
        let status = match op {
            "cancel_item" => "cancelled",
            "invalidate_item" => "invalidated",
            "complete_item" => "done",
            "reopen_item" => "todo",
            _ => return Err(format!("unsupported item status op '{op}'")),
        };
        let current = sqlx::query(
            "SELECT milestone_id, cycle, status FROM items WHERE project_id = ? AND id = ?",
        )
        .bind(project_id)
        .bind(item_id)
        .fetch_optional(&pool)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("unknown item '{item_id}'"))?;
        if current.get::<String, _>("status") == "doing" {
            return Err("stop the running item before changing its status".to_string());
        }
        sqlx::query("UPDATE items SET status = ?, updated_at = ? WHERE project_id = ? AND id = ?")
            .bind(status)
            .bind(now())
            .bind(project_id)
            .bind(item_id)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
        let cycle = current.try_get::<i64, _>("cycle").unwrap_or(1);
        if let Some(milestone_id) = current
            .try_get::<Option<String>, _>("milestone_id")
            .ok()
            .flatten()
        {
            self.refresh_milestone(&pool, project_id, &milestone_id, cycle)
                .await?;
        }
        self.refresh_cycle(&pool, project_id, cycle).await?;
        append_event(
            &pool,
            project_id,
            "item.status",
            "owner",
            current
                .try_get::<Option<String>, _>("milestone_id")
                .ok()
                .flatten(),
            Some(item_id.to_string()),
            json!({"status": status, "reason": params.get("reason")}),
        )
        .await?;
        Ok(format!("Item {item_id} set to {status}."))
    }

    async fn add_member(&self, project_id: &str, params: &Value) -> Result<String, String> {
        let agent_key = params
            .get("agent_key")
            .or_else(|| params.get("derived_from"))
            .or_else(|| params.get("key"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "add_member needs agent_key".to_string())?
            .to_ascii_lowercase();
        let amount = params
            .get("amount")
            .and_then(Value::as_i64)
            .unwrap_or(1)
            .clamp(1, 8);
        let display = params
            .get("display")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| title_case(&agent_key));
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let max_slot: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(slot_index), 0) FROM members WHERE project_id = ? AND agent_key = ?",
        )
        .bind(project_id)
        .bind(&agent_key)
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
        let total: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM members WHERE project_id = ? AND status = 'active'",
        )
        .bind(project_id)
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
        if total + amount > 12 {
            return Err("a project can have at most 12 active agent slots".to_string());
        }
        for index in 1..=amount {
            let slot = max_slot + index;
            let label = if amount > 1 || max_slot > 0 {
                format!("{display} {slot}")
            } else {
                display.clone()
            };
            sqlx::query(
                "INSERT OR IGNORE INTO members (id, project_id, agent_key, display, slot_index, status, created_at, updated_at) VALUES (?, ?, ?, ?, ?, 'active', ?, ?)",
            )
            .bind(member_id(&agent_key, slot, &label))
            .bind(project_id)
            .bind(&agent_key)
            .bind(&label)
            .bind(slot)
            .bind(now())
            .bind(now())
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
        }
        append_event(
            &pool,
            project_id,
            "member.added",
            "owner",
            None,
            None,
            json!({"agent_key": agent_key, "amount": amount}),
        )
        .await?;
        Ok(format!("Added {amount} {agent_key} slot(s)."))
    }

    async fn remove_member(&self, project_id: &str, params: &Value) -> Result<String, String> {
        let member_id = params
            .get("member_id")
            .or_else(|| params.get("id"))
            .and_then(Value::as_str)
            .ok_or_else(|| "remove_member needs member_id".to_string())?;
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let member = sqlx::query("SELECT agent_key, display FROM members WHERE project_id = ? AND id = ? AND status = 'active'")
            .bind(project_id)
            .bind(member_id)
            .fetch_optional(&pool)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("unknown member '{member_id}'"))?;
        let running: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM group_runs WHERE project_id = ? AND member_id = ? AND status = 'running'",
        )
        .bind(project_id)
        .bind(member_id)
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
        if running > 0 {
            return Err("this agent is still running an item; stop it first".to_string());
        }
        let active_members: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM members WHERE project_id = ? AND status = 'active'",
        )
        .bind(project_id)
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
        if active_members <= 1 {
            return Err("a project needs at least one agent slot".to_string());
        }
        let replacement: Option<String> = sqlx::query_scalar(
            "SELECT id FROM members WHERE project_id = ? AND status = 'active' AND id != ? ORDER BY CASE WHEN agent_key = ? THEN 0 ELSE 1 END, slot_index ASC LIMIT 1",
        )
        .bind(project_id)
        .bind(member_id)
        .bind(member.get::<String, _>("agent_key"))
        .fetch_optional(&pool)
        .await
        .ok()
        .flatten();
        sqlx::query("UPDATE items SET owner_member_id = ?, updated_at = ? WHERE project_id = ? AND owner_member_id = ? AND status NOT IN ('done', 'cancelled')")
            .bind(&replacement)
            .bind(now())
            .bind(project_id)
            .bind(member_id)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
        sqlx::query(
            "UPDATE members SET status = 'removed', updated_at = ? WHERE project_id = ? AND id = ?",
        )
        .bind(now())
        .bind(project_id)
        .bind(member_id)
        .execute(&pool)
        .await
        .map_err(|error| error.to_string())?;
        append_event(
            &pool,
            project_id,
            "member.removed",
            "owner",
            None,
            None,
            json!({"member_id": member_id, "display": member.get::<String, _>("display")}),
        )
        .await?;
        Ok(format!("Removed {}.", member.get::<String, _>("display")))
    }

    pub async fn approve_plan(&self, project_id: &str) -> Result<(), String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let project = sqlx::query("SELECT start_mode, start_at FROM projects WHERE id = ?")
            .bind(project_id)
            .fetch_optional(&pool)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("unknown project '{project_id}'"))?;
        let scheduled = project
            .try_get::<String, _>("start_mode")
            .unwrap_or_else(|_| "now".to_string())
            == "scheduled"
            && project
                .try_get::<Option<f64>, _>("start_at")
                .ok()
                .flatten()
                .is_some_and(|at| at > now());
        let status = if scheduled { "scheduled" } else { "running" };
        let updated = sqlx::query(
            "UPDATE projects SET status = ?, plan_status = 'ready', halt_reason = NULL, last_error = NULL, updated_at = ? WHERE id = ? AND plan_status = 'ready'",
        )
        .bind(status)
        .bind(now())
        .bind(project_id)
        .execute(&pool)
        .await
        .map_err(|error| error.to_string())?;
        if updated.rows_affected() == 0 {
            return Err("the plan is not ready for approval".to_string());
        }
        append_event(
            &pool,
            project_id,
            "plan.approved",
            "owner",
            None,
            None,
            json!({}),
        )
        .await?;
        Ok(())
    }

    pub async fn set_planning(&self, project_id: &str) -> Result<(), String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        sqlx::query(
            "UPDATE projects SET status = 'planning', plan_status = 'queued', halt_reason = NULL, last_error = NULL, updated_at = ? WHERE id = ?",
        )
        .bind(now())
        .bind(project_id)
        .execute(&pool)
        .await
        .map_err(|error| error.to_string())?;
        append_event(
            &pool,
            project_id,
            "plan.queued",
            "owner",
            None,
            None,
            json!({}),
        )
        .await?;
        Ok(())
    }

    /// Atomically take the planning lease. The group loop calls this before
    /// spawning a planner, so two ticks cannot create two plans.
    pub async fn claim_planning(&self, project_id: &str) -> Result<bool, String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let updated = sqlx::query(
            "UPDATE projects SET plan_status = 'running', updated_at = ? WHERE id = ? AND status = 'planning' AND plan_status IN ('queued', 'failed')",
        )
        .bind(now())
        .bind(project_id)
        .execute(&pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(updated.rows_affected() > 0)
    }

    pub async fn plan_failed(&self, project_id: &str, error: &str) -> Result<(), String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let error: String = error.chars().take(4_000).collect();
        sqlx::query(
            "UPDATE projects SET status = 'blocked', plan_status = 'failed', halt_reason = ?, last_error = ?, updated_at = ? WHERE id = ?",
        )
        .bind("planning failed")
        .bind(&error)
        .bind(now())
        .bind(project_id)
        .execute(&pool)
        .await
        .map_err(|error| error.to_string())?;
        append_event(
            &pool,
            project_id,
            "plan.failed",
            "supervisor",
            None,
            None,
            json!({"error": error}),
        )
        .await?;
        Ok(())
    }

    pub async fn apply_plan(&self, project_id: &str, plan: &Value) -> Result<(), String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let project = sqlx::query(
            "SELECT cycle, status, plan_review, start_mode, start_at FROM projects WHERE id = ?",
        )
        .bind(project_id)
        .fetch_optional(&pool)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("unknown project '{project_id}'"))?;
        let cycle = project.try_get::<i64, _>("cycle").unwrap_or(1);
        let done: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM items WHERE project_id = ? AND cycle = ? AND status = 'done'",
        )
        .bind(project_id)
        .bind(cycle)
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
        if done > 0 {
            return Err(
                "cannot replace a plan after work is done; start a new cycle instead".to_string(),
            );
        }
        let milestones = plan
            .get("milestones")
            .and_then(Value::as_array)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "plan needs at least one milestone".to_string())?;
        if milestones.len() > 20 {
            return Err("a plan can have at most 20 milestones".to_string());
        }
        let plan_review = project.try_get::<i64, _>("plan_review").unwrap_or(1) != 0;
        let members = sqlx::query(
            "SELECT id, agent_key, display FROM members WHERE project_id = ? AND status = 'active'",
        )
        .bind(project_id)
        .fetch_all(&pool)
        .await
        .map_err(|error| error.to_string())?;

        sqlx::query("DELETE FROM items WHERE project_id = ? AND cycle = ?")
            .bind(project_id)
            .bind(cycle)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
        sqlx::query("DELETE FROM milestones WHERE project_id = ? AND cycle = ?")
            .bind(project_id)
            .bind(cycle)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;

        for (position, milestone) in milestones.iter().enumerate() {
            let title = milestone
                .get("title")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!("milestone {position} needs a title"))?;
            let milestone_id = milestone
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("ms-{}-{}", cycle, position + 1));
            validate_id(&milestone_id)?;
            let owner = resolve_member(
                &members,
                milestone
                    .get("owner")
                    .or_else(|| milestone.get("owner_member"))
                    .and_then(Value::as_str),
            );
            sqlx::query(
                "INSERT INTO milestones (id, project_id, title, detail, status, seq_no, cycle, depends_on_json, acceptance_json, owner_member_id, due_at, updated_at) VALUES (?, ?, ?, ?, 'todo', ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&milestone_id)
            .bind(project_id)
            .bind(title)
            .bind(milestone.get("detail").and_then(Value::as_str).unwrap_or(""))
            .bind(position as i64)
            .bind(cycle)
            .bind(milestone.get("depends_on").cloned().unwrap_or_else(|| json!([])).to_string())
            .bind(milestone.get("acceptance").cloned().unwrap_or_else(|| json!([])).to_string())
            .bind(owner)
            .bind(parse_time(milestone.get("due_at")))
            .bind(now())
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
            let items = milestone
                .get("items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if items.len() > 40 {
                return Err(format!("milestone '{title}' has too many items"));
            }
            for (index, item) in items.iter().enumerate() {
                let mut item = item.clone();
                if let Some(object) = item.as_object_mut() {
                    object.insert("cycle".to_string(), json!(cycle));
                    if object.get("id").is_none() {
                        object.insert(
                            "id".to_string(),
                            json!(format!("item-{}-{}-{}", cycle, position + 1, index + 1)),
                        );
                    }
                    if object.get("owner_member_id").is_none()
                        && let Some(owner) = resolve_member(
                            &members,
                            object
                                .get("owner")
                                .or_else(|| object.get("owner_member"))
                                .and_then(Value::as_str),
                        )
                    {
                        object.insert("owner_member_id".to_string(), json!(owner));
                    }
                    if object
                        .get("acceptance")
                        .and_then(Value::as_array)
                        .is_none_or(Vec::is_empty)
                    {
                        object.insert(
                            "acceptance".to_string(),
                            json!(["The deliverable is complete and summarized"]),
                        );
                    }
                }
                insert_item(&pool, project_id, &milestone_id, &item, index, now()).await?;
            }
        }
        let scheduled = project
            .try_get::<String, _>("start_mode")
            .unwrap_or_else(|_| "now".to_string())
            == "scheduled"
            && project
                .try_get::<Option<f64>, _>("start_at")
                .ok()
                .flatten()
                .is_some_and(|at| at > now());
        let next_status = if scheduled {
            "scheduled"
        } else if plan_review {
            "plan_ready"
        } else {
            "running"
        };
        sqlx::query(
            "UPDATE projects SET plan_status = 'ready', status = ?, halt_reason = NULL, last_error = NULL, updated_at = ? WHERE id = ?",
        )
        .bind(next_status)
        .bind(now())
        .bind(project_id)
        .execute(&pool)
        .await
        .map_err(|error| error.to_string())?;
        append_event(
            &pool,
            project_id,
            "plan.ready",
            "supervisor",
            None,
            None,
            json!({"milestones": milestones.len(), "cycle": cycle, "review": plan_review}),
        )
        .await?;
        Ok(())
    }

    pub async fn start_next_cycle(&self, project_id: &str) -> Result<Value, String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let project = sqlx::query("SELECT cycle, status FROM projects WHERE id = ?")
            .bind(project_id)
            .fetch_optional(&pool)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("unknown project '{project_id}'"))?;
        let active: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM group_runs WHERE project_id = ? AND status = 'running'",
        )
        .bind(project_id)
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
        let doing: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM items WHERE project_id = ? AND cycle = ? AND status = 'doing'",
        )
        .bind(project_id)
        .bind(project.try_get::<i64, _>("cycle").unwrap_or(1))
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
        if active > 0 || doing > 0 {
            return Err(
                "wait for the current run to finish before starting a new cycle".to_string(),
            );
        }
        let old_cycle: i64 = project.try_get("cycle").unwrap_or(1);
        let new_cycle = old_cycle + 1;
        let milestones = sqlx::query(
            "SELECT * FROM milestones WHERE project_id = ? AND cycle = ? ORDER BY seq_no ASC, id ASC",
        )
        .bind(project_id)
        .bind(old_cycle)
        .fetch_all(&pool)
        .await
        .map_err(|error| error.to_string())?;
        if milestones.is_empty() {
            return Err("this project has no plan to repeat".to_string());
        }
        let items =
            sqlx::query("SELECT * FROM items WHERE project_id = ? AND cycle = ? ORDER BY id ASC")
                .bind(project_id)
                .bind(old_cycle)
                .fetch_all(&pool)
                .await
                .map_err(|error| error.to_string())?;
        let active_members: Vec<String> =
            sqlx::query_scalar("SELECT id FROM members WHERE project_id = ? AND status = 'active'")
                .bind(project_id)
                .fetch_all(&pool)
                .await
                .unwrap_or_default();
        let mut id_map = std::collections::HashMap::new();
        for milestone in &milestones {
            let old_id: String = milestone.get("id");
            let new_id = format!("{}-c{}", strip_cycle_suffix(&old_id), new_cycle);
            id_map.insert(old_id, new_id);
        }
        let mut tx = crate::begin_immediate(&pool)
            .await
            .map_err(|error| error.to_string())?;
        for milestone in &milestones {
            let old_id: String = milestone.get("id");
            let new_id = id_map
                .get(&old_id)
                .cloned()
                .unwrap_or_else(|| old_id.clone());
            let dependencies = parse_json(milestone.get::<String, _>("depends_on_json"))
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .map(|old| id_map.get(&old).cloned().unwrap_or(old))
                .collect::<Vec<_>>();
            let owner: Option<String> = milestone
                .try_get::<Option<String>, _>("owner_member_id")
                .ok()
                .flatten()
                .filter(|id| active_members.contains(id));
            sqlx::query(
                "INSERT INTO milestones (id, project_id, title, detail, status, seq_no, cycle, depends_on_json, acceptance_json, owner_member_id, due_at, updated_at) VALUES (?, ?, ?, ?, 'todo', ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&new_id)
            .bind(project_id)
            .bind(milestone.get::<String, _>("title"))
            .bind(milestone.try_get::<String, _>("detail").unwrap_or_default())
            .bind(milestone.try_get::<i64, _>("seq_no").unwrap_or(0))
            .bind(new_cycle)
            .bind(json!(dependencies).to_string())
            .bind(milestone.try_get::<String, _>("acceptance_json").unwrap_or_else(|_| "[]".to_string()))
            .bind(owner)
            .bind(milestone.try_get::<Option<f64>, _>("due_at").ok().flatten())
            .bind(now())
            .execute(&mut *tx)
            .await
            .map_err(|error| error.to_string())?;
        }
        for item in &items {
            let old_id: String = item.get("id");
            let milestone_old: Option<String> = item.try_get("milestone_id").ok().flatten();
            let new_id = format!("{}-c{}", strip_cycle_suffix(&old_id), new_cycle);
            let milestone_id = milestone_old
                .and_then(|old| id_map.get(&old).cloned())
                .unwrap_or_default();
            let owner: Option<String> = item
                .try_get::<Option<String>, _>("owner_member_id")
                .ok()
                .flatten()
                .filter(|id| active_members.contains(id));
            let mut cloned = item_to_plan_json(item);
            if let Some(object) = cloned.as_object_mut() {
                object.insert("id".to_string(), json!(new_id));
                object.insert("milestone_id".to_string(), json!(milestone_id));
                object.insert("cycle".to_string(), json!(new_cycle));
                object.insert("owner_member_id".to_string(), json!(owner));
            }
            sqlx::query(
                "INSERT INTO items (id, project_id, milestone_id, title, detail, status, attempts, max_attempts, owner_member, owner_member_id, workdir, lease_owner, lease_expires, depends_on_json, acceptance_json, tags_json, priority, due_at, requires_approval, cycle, cost_tokens, cost_usd, result, error, blocked_reason, invalidated_reason, updated_at) VALUES (?, ?, ?, ?, ?, 'todo', 0, ?, ?, ?, '', NULL, NULL, ?, ?, ?, ?, ?, ?, ?, 0, 0, '', '', NULL, NULL, ?)",
            )
            .bind(cloned.get("id").and_then(Value::as_str).unwrap_or(""))
            .bind(project_id)
            .bind(&milestone_id)
            .bind(cloned.get("title").and_then(Value::as_str).unwrap_or(""))
            .bind(cloned.get("detail").and_then(Value::as_str).unwrap_or(""))
            .bind(cloned.get("max_attempts").and_then(Value::as_i64).unwrap_or(3))
            .bind(cloned.get("owner_member").and_then(Value::as_str).unwrap_or(""))
            .bind(cloned.get("owner_member_id").and_then(Value::as_str))
            .bind(cloned.get("depends_on").cloned().unwrap_or_else(|| json!([])).to_string())
            .bind(cloned.get("acceptance").cloned().unwrap_or_else(|| json!([])).to_string())
            .bind(cloned.get("tags").cloned().unwrap_or_else(|| json!([])).to_string())
            .bind(cloned.get("priority").and_then(Value::as_i64).unwrap_or(0))
            .bind(parse_time(cloned.get("due_at")))
            .bind(if cloned.get("requires_approval").and_then(Value::as_bool).unwrap_or(false) { 1 } else { 0 })
            .bind(new_cycle)
            .bind(now())
            .execute(&mut *tx)
            .await
            .map_err(|error| error.to_string())?;
        }
        sqlx::query(
            "UPDATE projects SET cycle = ?, status = 'running', next_run_at = NULL, plan_status = 'ready', halt_reason = NULL, last_error = NULL, updated_at = ? WHERE id = ?",
        )
        .bind(new_cycle)
        .bind(now())
        .bind(project_id)
        .execute(&mut *tx)
        .await
        .map_err(|error| error.to_string())?;
        sqlx::query(
            "INSERT INTO events (project_id, ts, kind, actor, data) VALUES (?, ?, 'cycle.started', 'supervisor', ?)",
        )
        .bind(project_id)
        .bind(now())
        .bind(json!({"cycle": new_cycle}).to_string())
        .execute(&mut *tx)
        .await
        .map_err(|error| error.to_string())?;
        tx.commit().await.map_err(|error| error.to_string())?;
        self.detail(project_id, 200).await
    }

    async fn current_cycle(&self, pool: &SqlitePool, project_id: &str) -> Result<i64, String> {
        sqlx::query_scalar("SELECT cycle FROM projects WHERE id = ?")
            .bind(project_id)
            .fetch_optional(pool)
            .await
            .map_err(|error| error.to_string())?
            .flatten()
            .ok_or_else(|| format!("unknown project '{project_id}'"))
    }

    pub async fn set_workspace_path(&self, project_id: &str, path: &str) -> Result<(), String> {
        let path = path.trim();
        if path.is_empty() {
            return Err("workspace_path is empty".to_string());
        }
        let absolute = std::path::absolute(Path::new(path))
            .map_err(|error| format!("invalid workspace path: {error}"))?;
        if !absolute.is_dir() {
            return Err(format!(
                "workspace folder not found: {}",
                absolute.display()
            ));
        }
        let path_text = absolute.to_string_lossy().to_string();
        let pool = open_pool(&self.root.join(project_id), false).await?;
        sqlx::query("UPDATE projects SET workspace_path = ?, updated_at = ? WHERE id = ?")
            .bind(&path_text)
            .bind(now())
            .bind(project_id)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
        let origin = self.root.join(project_id).join(".gp").join("origin.json");
        let mut value = tokio::fs::read_to_string(&origin)
            .await
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .unwrap_or_else(|| json!({}));
        if let Some(object) = value.as_object_mut() {
            object.insert("workspace_path".to_string(), json!(path_text));
        }
        tokio::fs::write(&origin, format!("{value}\n"))
            .await
            .map_err(|error| error.to_string())?;
        append_event(
            &pool,
            project_id,
            "workspace.updated",
            "owner",
            None,
            None,
            json!({"workspace_path": path_text}),
        )
        .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_message(
        &self,
        project_id: &str,
        body: &str,
        to: &str,
        kind: &str,
        from_kind: &str,
        from_member_id: Option<&str>,
        from_label: &str,
        item_id: Option<&str>,
        run_id: Option<&str>,
        deliver: bool,
    ) -> Result<i64, String> {
        let body = body.trim();
        if body.is_empty() {
            return Err("message body is empty".to_string());
        }
        if body.chars().count() > 8_000 {
            return Err("message is too long (max 8000)".to_string());
        }
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let (to_kind, to_member_id) = resolve_target(&pool, project_id, to).await?;
        let timestamp = now();
        let result = sqlx::query(
            "INSERT INTO group_messages (project_id, ts, from_kind, from_member_id, from_label, to_kind, to_member_id, body, kind, item_id, run_id, deliver, delivered_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)",
        )
        .bind(project_id)
        .bind(timestamp)
        .bind(from_kind)
        .bind(from_member_id)
        .bind(from_label)
        .bind(&to_kind)
        .bind(&to_member_id)
        .bind(body)
        .bind(kind)
        .bind(item_id)
        .bind(run_id)
        .bind(if deliver { 1 } else { 0 })
        .execute(&pool)
        .await
        .map_err(|error| error.to_string())?;
        let seq = result.last_insert_rowid();
        append_event(
            &pool,
            project_id,
            "message.sent",
            from_label,
            None,
            item_id.map(str::to_string),
            json!({"from": from_label, "to": to_member_id.clone().unwrap_or(to_kind.clone()), "body": body, "kind": kind, "deliver": deliver, "run_id": run_id, "seq": seq}),
        )
        .await?;
        Ok(seq)
    }

    pub async fn pending_messages(
        &self,
        project_id: &str,
        member_id: &str,
    ) -> Result<Vec<Value>, String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let rows = sqlx::query(
            "SELECT seq, ts, from_kind, from_label, body, kind, item_id, run_id FROM group_messages WHERE project_id = ? AND deliver = 1 AND delivered_at IS NULL AND (to_kind = 'all' OR (to_kind = 'member' AND to_member_id = ?)) ORDER BY seq ASC LIMIT 20",
        )
        .bind(project_id)
        .bind(member_id)
        .fetch_all(&pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(rows
            .into_iter()
            .map(|row| {
                json!({
                    "seq": row.get::<i64, _>("seq"),
                    "ts": row.get::<f64, _>("ts"),
                    "from": row.try_get::<String, _>("from_label").unwrap_or_else(|_| "owner".to_string()),
                    "from_kind": row.try_get::<String, _>("from_kind").unwrap_or_default(),
                    "body": row.get::<String, _>("body"),
                    "kind": row.try_get::<String, _>("kind").unwrap_or_default(),
                    "item_id": row.try_get::<Option<String>, _>("item_id").ok().flatten(),
                    "run_id": row.try_get::<Option<String>, _>("run_id").ok().flatten(),
                })
            })
            .collect())
    }

    pub async fn mark_delivered(&self, project_id: &str, seqs: &[i64]) -> Result<(), String> {
        if seqs.is_empty() {
            return Ok(());
        }
        let pool = open_pool(&self.root.join(project_id), false).await?;
        for seq in seqs {
            sqlx::query("UPDATE group_messages SET delivered_at = ? WHERE project_id = ? AND seq = ? AND delivered_at IS NULL")
                .bind(now())
                .bind(project_id)
                .bind(seq)
                .execute(&pool)
                .await
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub async fn active_chats(&self, project_id: &str) -> Result<Vec<String>, String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        sqlx::query_scalar(
            "SELECT chat_id FROM group_runs WHERE project_id = ? AND status = 'running' AND chat_id IS NOT NULL",
        )
        .bind(project_id)
        .fetch_all(&pool)
        .await
        .map_err(|error| error.to_string())
    }

    pub async fn due_projects(&self) -> Vec<Value> {
        let now = now();
        self.list()
            .await
            .into_iter()
            .filter(|project| {
                let status = project["status"].as_str().unwrap_or("");
                let start_at = project["start_at"].as_f64().unwrap_or(0.0);
                let next_run_at = project["next_run_at"].as_f64().unwrap_or(0.0);
                (status == "scheduled" && start_at > 0.0 && start_at <= now)
                    || (status == "done"
                        && project["repeat_active"].as_bool().unwrap_or(false)
                        && next_run_at > 0.0
                        && next_run_at <= now)
            })
            .collect()
    }

    pub async fn start_due(&self, project_id: &str) -> Result<(), String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let status: Option<String> = sqlx::query_scalar("SELECT status FROM projects WHERE id = ?")
            .bind(project_id)
            .fetch_optional(&pool)
            .await
            .map_err(|error| error.to_string())?
            .flatten();
        match status.as_deref() {
            Some("scheduled") => {
                self.start_project(project_id).await?;
            }
            Some("done") => {
                let _ = self.start_next_cycle(project_id).await?;
            }
            _ => {}
        }
        Ok(())
    }

    pub async fn trash(&self, project_id: &str) -> Result<Value, String> {
        validate_id(project_id)?;
        let source = self.root.join(project_id);
        if !source.is_dir() {
            return Err(format!("unknown project '{project_id}'"));
        }
        let pool = open_pool(&source, false).await?;
        let active: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM group_runs WHERE project_id = ? AND status = 'running'",
        )
        .bind(project_id)
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
        if active > 0 {
            return Err("stop the project before moving it to Trash".to_string());
        }
        let project = sqlx::query("SELECT title, goal, status FROM projects WHERE id = ?")
            .bind(project_id)
            .fetch_optional(&pool)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("unknown project '{project_id}'"))?;
        let workspace_path = pool_project_workspace(&pool, project_id)
            .await
            .unwrap_or_default();
        let title = project
            .try_get::<String, _>("title")
            .unwrap_or_else(|_| project.get::<String, _>("goal"));
        pool.close().await;
        let trash_root = self.root.join(".trash");
        tokio::fs::create_dir_all(&trash_root)
            .await
            .map_err(|error| error.to_string())?;
        let folder = format!(
            "{}-{}",
            project_id,
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        );
        let target = trash_root.join(&folder);
        let record = json!({
            "original_id": project_id,
            "title": title,
            "deleted_at": now(),
            "workspace_path": workspace_path,
        });
        tokio::fs::write(source.join(".gp").join("trash.json"), format!("{record}\n"))
            .await
            .map_err(|error| error.to_string())?;
        rename_with_retry(&source, &target).await?;
        Ok(json!({"ok": true, "folder": folder, "record": record}))
    }

    pub async fn trash_list(&self) -> Vec<Value> {
        let trash_root = self.root.join(".trash");
        let Ok(mut entries) = tokio::fs::read_dir(&trash_root).await else {
            return Vec::new();
        };
        let mut out = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            if !entry.path().is_dir() {
                continue;
            }
            let Some(folder) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let text = tokio::fs::read_to_string(entry.path().join(".gp").join("trash.json"))
                .await
                .unwrap_or_default();
            let record = serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({}));
            out.push(json!({
                "folder": folder,
                "id": record.get("original_id").and_then(Value::as_str).unwrap_or(&folder),
                "title": record.get("title").and_then(Value::as_str).unwrap_or("Untitled project"),
                "deleted_at": record.get("deleted_at").and_then(Value::as_f64),
                "workspace_path": record.get("workspace_path").and_then(Value::as_str).unwrap_or(""),
            }));
        }
        out.sort_by(|left, right| {
            right["deleted_at"]
                .as_f64()
                .unwrap_or(0.0)
                .partial_cmp(&left["deleted_at"].as_f64().unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        out
    }

    pub async fn restore(&self, folder: &str) -> Result<Value, String> {
        validate_trash_name(folder)?;
        let source = self.root.join(".trash").join(folder);
        if !source.is_dir() {
            return Err(format!("unknown trash entry '{folder}'"));
        }
        let text = tokio::fs::read_to_string(source.join(".gp").join("trash.json"))
            .await
            .map_err(|error| error.to_string())?;
        let record = serde_json::from_str::<Value>(&text).map_err(|error| error.to_string())?;
        let original = record
            .get("original_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or(folder);
        validate_id(original)?;
        let target = self.root.join(original);
        if target.exists() {
            return Err(format!("a project named '{original}' already exists"));
        }
        rename_with_retry(&source, &target).await?;
        let _ = tokio::fs::remove_file(target.join(".gp").join("trash.json")).await;
        self.detail(original, 100).await
    }

    pub async fn permanent_delete(&self, folder: &str) -> Result<(), String> {
        validate_trash_name(folder)?;
        let target = self.root.join(".trash").join(folder);
        if !target.is_dir() {
            return Err(format!("unknown trash entry '{folder}'"));
        }
        tokio::fs::remove_dir_all(&target)
            .await
            .map_err(|error| error.to_string())
    }

    /// Park a claimed (`doing`) item for owner approval instead of running or
    /// finishing it. The existing approval route returns the item to `todo`
    /// on grant (so the runner picks it up again) or `failed` on reject.
    /// Idempotent: an already-parked item is left alone.
    pub async fn park_for_approval(
        &self,
        project_id: &str,
        item_id: &str,
        reason: &str,
    ) -> Result<bool, String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let current: Option<String> =
            sqlx::query_scalar("SELECT blocked_reason FROM items WHERE project_id = ? AND id = ?")
                .bind(project_id)
                .bind(item_id)
                .fetch_optional(&pool)
                .await
                .map_err(|error| error.to_string())?
                .flatten();
        if current.as_deref() == Some("awaiting_approval") {
            return Ok(false);
        }
        let updated = sqlx::query("UPDATE items SET status = 'awaiting_approval', blocked_reason = 'awaiting_approval', lease_owner = NULL, lease_expires = NULL, updated_at = ? WHERE project_id = ? AND id = ? AND status IN ('todo', 'ready', 'doing')")
            .bind(now())
            .bind(project_id)
            .bind(item_id)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
        if updated.rows_affected() == 0 {
            return Ok(false);
        }
        append_event(
            &pool,
            project_id,
            "approval.requested",
            "supervisor",
            None,
            Some(item_id.to_string()),
            json!({"reason": reason}),
        )
        .await?;
        let _ = self
            .append_message_record(
                &pool,
                project_id,
                "supervisor",
                None,
                "Orchestrator",
                "owner",
                None,
                reason,
                "approval",
                Some(item_id),
                None,
            )
            .await;
        Ok(true)
    }

    pub async fn approval(
        &self,
        project_id: &str,
        item_id: &str,
        grant: bool,
        note: &str,
    ) -> Result<Value, String> {
        let pool = open_pool(&self.root.join(project_id), false).await?;
        let item = sqlx::query(
            "SELECT milestone_id, cycle FROM items WHERE project_id = ? AND id = ? AND status = 'awaiting_approval'",
        )
        .bind(project_id)
        .bind(item_id)
        .fetch_optional(&pool)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("item '{item_id}' is not awaiting approval"))?;
        let status = if grant { "todo" } else { "cancelled" };
        sqlx::query(
            "UPDATE items SET status = ?, blocked_reason = NULL, lease_owner = NULL, lease_expires = NULL, next_attempt_at = NULL, updated_at = ? WHERE project_id = ? AND id = ?",
        )
        .bind(status)
        .bind(now())
        .bind(project_id)
        .bind(item_id)
        .execute(&pool)
        .await
        .map_err(|error| error.to_string())?;
        let milestone_id: Option<String> = item.try_get("milestone_id").ok().flatten();
        let cycle: i64 = item.try_get("cycle").unwrap_or(1);
        append_event(
            &pool,
            project_id,
            if grant {
                "approval.granted"
            } else {
                "approval.rejected"
            },
            "owner",
            milestone_id.clone(),
            Some(item_id.to_string()),
            json!({"note": note, "status": status}),
        )
        .await?;
        let _ = self
            .append_message_record(
                &pool,
                project_id,
                "owner",
                None,
                "owner",
                "supervisor",
                None,
                if note.trim().is_empty() {
                    if grant {
                        "Approved. Continue the item."
                    } else {
                        "Rejected. Stop this item."
                    }
                } else {
                    note
                },
                "note",
                Some(item_id),
                None,
            )
            .await;
        if !grant {
            if let Some(milestone_id) = milestone_id {
                self.refresh_milestone(&pool, project_id, &milestone_id, cycle)
                    .await?;
            }
            self.refresh_cycle(&pool, project_id, cycle).await?;
        }
        Ok(
            json!({"ok": true, "message": if grant { "Approval granted." } else { "Approval rejected." }}),
        )
    }
}

#[derive(Clone, Debug)]
struct MemberSpec {
    agent_key: String,
    display: String,
    slot: i64,
}

fn parse_member_specs(value: Option<&Value>) -> Vec<MemberSpec> {
    let defaults = || {
        vec![MemberSpec {
            agent_key: "developer".to_string(),
            display: "Developer".to_string(),
            slot: 1,
        }]
    };
    let Some(entries) = value.and_then(Value::as_array) else {
        return defaults();
    };
    if entries.is_empty() {
        return defaults();
    }
    let mut out = Vec::new();
    let mut max_slots: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    for entry in entries {
        let (agent_key, display, amount) = if let Some(key) = entry.as_str() {
            (key.trim().to_string(), String::new(), 1i64)
        } else {
            let key = entry
                .get("agent_key")
                .or_else(|| entry.get("key"))
                .or_else(|| entry.get("derived_from"))
                .or_else(|| entry.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            let display = entry
                .get("display")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            let amount = entry
                .get("amount")
                .or_else(|| entry.get("count"))
                .or_else(|| entry.get("slots"))
                .and_then(Value::as_i64)
                .unwrap_or(1)
                .clamp(1, 8);
            (key, display, amount)
        };
        if agent_key.is_empty() {
            continue;
        }
        let agent_key = agent_key.to_ascii_lowercase();
        let base_display = if display.is_empty() {
            title_case(&agent_key)
        } else {
            display
        };
        let first_slot = max_slots.get(&agent_key).copied().unwrap_or(0) + 1;
        for index in 1..=amount {
            let slot = first_slot + index - 1;
            let label = if amount > 1 || first_slot > 1 {
                format!("{base_display} {slot}")
            } else {
                base_display.clone()
            };
            out.push(MemberSpec {
                agent_key: agent_key.clone(),
                display: label,
                slot,
            });
        }
        max_slots.insert(agent_key, first_slot + amount - 1);
    }
    if out.is_empty() { defaults() } else { out }
}

async fn insert_members(
    pool: &SqlitePool,
    project_id: &str,
    members: &[MemberSpec],
) -> Result<(), String> {
    let timestamp = now();
    for member in members {
        sqlx::query(
            "INSERT OR IGNORE INTO members (id, project_id, agent_key, display, slot_index, status, created_at, updated_at) VALUES (?, ?, ?, ?, ?, 'active', ?, ?)",
        )
        .bind(member_id(&member.agent_key, member.slot, &member.display))
        .bind(project_id)
        .bind(&member.agent_key)
        .bind(&member.display)
        .bind(member.slot)
        .bind(timestamp)
        .bind(timestamp)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn title_case(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "ui" => return "UI".to_string(),
        "dfd" => return "DFD".to_string(),
        "devops" => return "DevOps".to_string(),
        "orchestrator" => return "Orchestrator".to_string(),
        "developer" => return "Developer".to_string(),
        "general" => return "General".to_string(),
        "explore" => return "Explore".to_string(),
        "house" => return "House".to_string(),
        _ => {}
    }
    let value = value.replace(['-', '_'], " ");
    let mut out = String::new();
    for word in value.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    if out.is_empty() {
        "Agent".to_string()
    } else {
        out
    }
}

fn parse_time(value: Option<&Value>) -> Option<f64> {
    let value = value?;
    if let Some(number) = value.as_f64() {
        return Some(number);
    }
    if let Some(number) = value.as_i64() {
        return Some(number as f64);
    }
    let text = value.as_str()?.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(parsed) = DateTime::parse_from_rfc3339(text) {
        return Some(parsed.timestamp() as f64 + parsed.timestamp_subsec_millis() as f64 / 1000.0);
    }
    for format in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(parsed) = chrono::NaiveDateTime::parse_from_str(text, format) {
            return Some(parsed.and_utc().timestamp() as f64);
        }
    }
    if let Ok(parsed) = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return Some(parsed.and_hms_opt(0, 0, 0)?.and_utc().timestamp() as f64);
    }
    None
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
    let cycle = item.get("cycle").and_then(Value::as_i64).unwrap_or(1);
    let requires_approval = item
        .get("requires_approval")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || item
            .get("tags")
            .and_then(Value::as_array)
            .is_some_and(|tags| {
                tags.iter().any(|tag| {
                    tag.as_str().is_some_and(|tag| {
                        tag.eq_ignore_ascii_case("needs-approval")
                            || tag.eq_ignore_ascii_case("needs_approval")
                    })
                })
            });
    sqlx::query(
        "INSERT INTO items (id, project_id, milestone_id, title, detail, status, attempts, max_attempts, owner_member, owner_member_id, workdir, lease_owner, lease_expires, depends_on_json, acceptance_json, tags_json, priority, due_at, requires_approval, cycle, cost_tokens, cost_usd, result, error, blocked_reason, invalidated_reason, updated_at) VALUES (?, ?, ?, ?, ?, 'todo', 0, ?, ?, ?, ?, NULL, NULL, ?, ?, ?, ?, ?, ?, ?, 0, 0, '', '', NULL, NULL, ?)",
    )
    .bind(&item_id)
    .bind(project_id)
    .bind(milestone_id)
    .bind(item.get("title").and_then(Value::as_str).unwrap_or(""))
    .bind(item.get("detail").and_then(Value::as_str).unwrap_or(""))
    .bind(item.get("max_attempts").and_then(Value::as_i64).unwrap_or(3))
    .bind(item.get("owner_member").and_then(Value::as_str).unwrap_or(""))
    .bind(item.get("owner_member_id").and_then(Value::as_str))
    .bind(item.get("workdir").and_then(Value::as_str).unwrap_or(""))
    .bind(item.get("depends_on").cloned().unwrap_or_else(|| json!([])).to_string())
    .bind(item.get("acceptance").cloned().unwrap_or_else(|| json!([])).to_string())
    .bind(item.get("tags").cloned().unwrap_or_else(|| json!([])).to_string())
    .bind(item.get("priority").and_then(Value::as_i64).unwrap_or(0))
    .bind(parse_time(item.get("due_at")))
    .bind(if requires_approval { 1 } else { 0 })
    .bind(cycle)
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
    let options = crate::sqlite_options(&db, create);
    let pool = SqlitePoolOptions::new()
        .max_connections(3)
        .connect_with(options)
        .await
        .map_err(|error| error.to_string())?;
    if create {
        init(&pool).await?;
    }
    ensure_schema(&pool).await?;
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
    crate::migrate::baseline(pool, "groups")
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Idempotent schema upgrade for every project database, including databases
/// created by older releases. The base tables stay compatible; all new state is
/// additive so a downgraded binary can still read the project.
async fn ensure_schema(pool: &SqlitePool) -> Result<(), String> {
    for statement in [
        "CREATE TABLE IF NOT EXISTS members (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, agent_key TEXT NOT NULL, display TEXT NOT NULL, slot_index INTEGER NOT NULL DEFAULT 1, status TEXT NOT NULL DEFAULT 'active', created_at REAL, updated_at REAL)",
        "CREATE INDEX IF NOT EXISTS ix_members_project ON members(project_id, status)",
        "CREATE TABLE IF NOT EXISTS group_runs (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, cycle INTEGER NOT NULL DEFAULT 1, item_id TEXT, member_id TEXT, agent_key TEXT, chat_id TEXT, run_id TEXT, kind TEXT NOT NULL DEFAULT 'item', status TEXT NOT NULL DEFAULT 'running', started_at REAL, finished_at REAL, input_tokens INTEGER NOT NULL DEFAULT 0, output_tokens INTEGER NOT NULL DEFAULT 0, cost_usd REAL NOT NULL DEFAULT 0, report TEXT, error TEXT)",
        "CREATE INDEX IF NOT EXISTS ix_runs_project ON group_runs(project_id, status)",
        "CREATE INDEX IF NOT EXISTS ix_runs_member ON group_runs(member_id, status)",
        "CREATE TABLE IF NOT EXISTS group_messages (seq INTEGER PRIMARY KEY AUTOINCREMENT, project_id TEXT NOT NULL, ts REAL NOT NULL, from_kind TEXT NOT NULL, from_member_id TEXT, from_label TEXT NOT NULL, to_kind TEXT NOT NULL DEFAULT 'all', to_member_id TEXT, body TEXT NOT NULL, kind TEXT NOT NULL DEFAULT 'note', item_id TEXT, run_id TEXT, deliver INTEGER NOT NULL DEFAULT 1, delivered_at REAL, reply_to INTEGER)",
        "CREATE INDEX IF NOT EXISTS ix_messages_project ON group_messages(project_id, seq)",
    ] {
        sqlx::query(statement)
            .execute(pool)
            .await
            .map_err(|error| error.to_string())?;
    }
    for (column, declaration) in [
        ("title", "TEXT DEFAULT ''"),
        ("workspace_path", "TEXT DEFAULT ''"),
        ("origin_chat_id", "TEXT DEFAULT ''"),
        ("cycle", "INTEGER NOT NULL DEFAULT 1"),
        ("plan_status", "TEXT NOT NULL DEFAULT 'none'"),
        ("plan_review", "INTEGER NOT NULL DEFAULT 1"),
        ("start_mode", "TEXT NOT NULL DEFAULT 'now'"),
        ("start_at", "REAL"),
        ("next_run_at", "REAL"),
        ("recurrence", "TEXT NOT NULL DEFAULT 'none'"),
        ("repeat_active", "INTEGER NOT NULL DEFAULT 0"),
        ("planner_agent", "TEXT NOT NULL DEFAULT 'orchestrator'"),
        ("max_parallel", "INTEGER NOT NULL DEFAULT 1"),
        ("last_error", "TEXT"),
    ] {
        crate::migrate::ensure_column(pool, "projects", column, declaration)
            .await
            .map_err(|error| error.to_string())?;
    }
    for (column, declaration) in [
        ("cycle", "INTEGER NOT NULL DEFAULT 1"),
        ("detail", "TEXT NOT NULL DEFAULT ''"),
        ("due_at", "REAL"),
        ("owner_member_id", "TEXT"),
    ] {
        crate::migrate::ensure_column(pool, "milestones", column, declaration)
            .await
            .map_err(|error| error.to_string())?;
    }
    for (column, declaration) in [
        ("cycle", "INTEGER NOT NULL DEFAULT 1"),
        ("owner_member_id", "TEXT"),
        ("priority", "INTEGER NOT NULL DEFAULT 0"),
        ("due_at", "REAL"),
        ("requires_approval", "INTEGER NOT NULL DEFAULT 0"),
        ("next_attempt_at", "REAL"),
        ("run_id", "TEXT"),
        ("started_at", "REAL"),
        ("finished_at", "REAL"),
    ] {
        crate::migrate::ensure_column(pool, "items", column, declaration)
            .await
            .map_err(|error| error.to_string())?;
    }

    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    if version < 2 {
        sqlx::query("UPDATE projects SET title = COALESCE(NULLIF(title, ''), goal, ''), start_mode = COALESCE(NULLIF(start_mode, ''), 'now'), recurrence = COALESCE(NULLIF(recurrence, ''), 'none'), cycle = COALESCE(cycle, 1), max_parallel = CASE WHEN COALESCE(max_parallel, 0) < 1 THEN 1 ELSE max_parallel END, plan_status = CASE WHEN COALESCE(plan_status, 'none') = 'none' AND EXISTS (SELECT 1 FROM milestones WHERE milestones.project_id = projects.id) THEN 'ready' ELSE COALESCE(plan_status, 'none') END")
            .execute(pool)
            .await
            .map_err(|error| error.to_string())?;
        backfill_members(pool).await?;
        sqlx::query("UPDATE members SET display = CASE agent_key WHEN 'ui' THEN 'UI' WHEN 'dfd' THEN 'DFD' WHEN 'devops' THEN 'DevOps' ELSE display END WHERE display IN ('Ui', 'Dfd', 'Devops')")
            .execute(pool)
            .await
            .map_err(|error| error.to_string())?;
        sqlx::query("PRAGMA user_version = 2")
            .execute(pool)
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Older projects stored member names as free text on items. Seed one member
/// per distinct name so the new capacity/usage model has real rows. New
/// projects always create members up front.
async fn backfill_members(pool: &SqlitePool) -> Result<(), String> {
    let rows = sqlx::query(
        "SELECT DISTINCT pi.id AS project_id, TRIM(COALESCE(NULLIF(i.owner_member, ''), NULLIF(i.lease_owner, ''))) AS name FROM items i JOIN projects pi ON pi.id = i.project_id WHERE TRIM(COALESCE(NULLIF(i.owner_member, ''), NULLIF(i.lease_owner, ''))) != '' ORDER BY pi.id, name",
    )
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())?;
    for row in rows {
        let project_id: String = row.try_get("project_id").unwrap_or_default();
        let name: String = row.try_get("name").unwrap_or_default();
        let lower = name.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "group-runner" | "supervisor" | "owner" | "human" | "you" | "user" | "all"
        ) || lower.starts_with("group-runner")
        {
            continue;
        }
        let agent_key = guess_agent_key(&name);
        let id = member_id(&agent_key, 1, &name);
        sqlx::query(
            "INSERT OR IGNORE INTO members (id, project_id, agent_key, display, slot_index, status, created_at, updated_at) VALUES (?, ?, ?, ?, 1, 'active', ?, ?)",
        )
        .bind(&id)
        .bind(&project_id)
        .bind(&agent_key)
        .bind(&name)
        .bind(now())
        .bind(now())
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
    }
    let projects = sqlx::query("SELECT id FROM projects")
        .fetch_all(pool)
        .await
        .map_err(|error| error.to_string())?;
    for project in projects {
        let project_id: String = project.get("id");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM members WHERE project_id = ?")
            .bind(&project_id)
            .fetch_one(pool)
            .await
            .unwrap_or(0);
        if count == 0 {
            sqlx::query("INSERT OR IGNORE INTO members (id, project_id, agent_key, display, slot_index, status, created_at, updated_at) VALUES (?, ?, 'developer', 'Developer', 1, 'active', ?, ?)")
                .bind(format!("{project_id}-developer"))
                .bind(&project_id)
                .bind(now())
                .bind(now())
                .execute(pool)
                .await
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn guess_agent_key(name: &str) -> String {
    let lower = name.trim().to_ascii_lowercase();
    match lower.as_str() {
        "developer" | "dev" | "developer 1" => "developer".to_string(),
        "ui" | "designer" | "frontend" => "ui".to_string(),
        "general" | "research" | "researcher" => "general".to_string(),
        "house" => "house".to_string(),
        "dfd" => "dfd".to_string(),
        "devops" | "ops" => "devops".to_string(),
        "explore" | "scout" => "explore".to_string(),
        "orchestrator" | "stark" | "supervisor" => "orchestrator".to_string(),
        _ => {
            let mut key = String::new();
            let mut dash = false;
            for character in lower.chars() {
                if character.is_ascii_alphanumeric() {
                    if dash && !key.is_empty() {
                        key.push('-');
                    }
                    dash = false;
                    key.push(character);
                } else {
                    dash = true;
                }
            }
            if key.is_empty() {
                "developer".to_string()
            } else {
                key
            }
        }
    }
}

fn member_id(agent_key: &str, slot: i64, display: &str) -> String {
    let stem = agent_key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
        .collect::<String>();
    let fallback = display
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    format!(
        "member-{}-{}",
        if stem.is_empty() { fallback } else { stem },
        slot
    )
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

async fn members_json(
    pool: &SqlitePool,
    project_id: &str,
    cycle: i64,
) -> Result<Vec<Value>, String> {
    let rows = sqlx::query(
        "SELECT id, project_id, agent_key, display, slot_index, status, created_at FROM members WHERE project_id = ? AND status = 'active' ORDER BY slot_index ASC, created_at ASC, id ASC",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        let id: String = row.get("id");
        let running: Option<Value> = sqlx::query(
            "SELECT r.id, r.chat_id, r.item_id, i.title AS item_title FROM group_runs r LEFT JOIN items i ON i.project_id = r.project_id AND i.id = r.item_id WHERE r.member_id = ? AND r.status = 'running' ORDER BY r.started_at DESC LIMIT 1",
        )
        .bind(&id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .map(|run| {
            json!({
                "id": run.try_get::<String, _>("id").unwrap_or_default(),
                "chat_id": run.try_get::<Option<String>, _>("chat_id").ok().flatten(),
                "item_id": run.try_get::<Option<String>, _>("item_id").ok().flatten(),
                "item_title": run.try_get::<Option<String>, _>("item_title").ok().flatten(),
            })
        });
        let assigned: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM items WHERE project_id = ? AND cycle = ? AND owner_member_id = ?",
        )
        .bind(project_id)
        .bind(cycle)
        .bind(&id)
        .fetch_one(pool)
        .await
        .unwrap_or(0);
        let done: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM items WHERE project_id = ? AND cycle = ? AND owner_member_id = ? AND status = 'done'",
        )
        .bind(project_id)
        .bind(cycle)
        .bind(&id)
        .fetch_one(pool)
        .await
        .unwrap_or(0);
        let failed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM items WHERE project_id = ? AND cycle = ? AND owner_member_id = ? AND status = 'failed'",
        )
        .bind(project_id)
        .bind(cycle)
        .bind(&id)
        .fetch_one(pool)
        .await
        .unwrap_or(0);
        let usage = sqlx::query(
            "SELECT COUNT(*) AS runs, COALESCE(SUM(input_tokens + output_tokens), 0) AS tokens, COALESCE(SUM(cost_usd), 0) AS cost_usd, MAX(finished_at) AS last_active FROM group_runs WHERE member_id = ?",
        )
        .bind(&id)
        .fetch_one(pool)
        .await
        .ok();
        out.push(json!({
            "id": id,
            "project_id": row.get::<String, _>("project_id"),
            "agent_key": row.get::<String, _>("agent_key"),
            "display": row.get::<String, _>("display"),
            "name": row.get::<String, _>("display"),
            "slot_index": row.try_get::<i64, _>("slot_index").unwrap_or(1),
            "status": row.try_get::<String, _>("status").unwrap_or_else(|_| "active".to_string()),
            "created_at": row.try_get::<f64, _>("created_at").ok(),
            "current_item": running.as_ref().and_then(|run| run["item_title"].as_str()),
            "active_run": running,
            "assigned": assigned,
            "done": done,
            "failed": failed,
            "running": if running.is_some() { 1 } else { 0 },
            "usage": {
                "runs": usage.as_ref().and_then(|row| row.try_get::<i64, _>("runs").ok()).unwrap_or(0),
                "tokens": usage.as_ref().and_then(|row| row.try_get::<i64, _>("tokens").ok()).unwrap_or(0),
                "cost_usd": usage.as_ref().and_then(|row| row.try_get::<f64, _>("cost_usd").ok()).unwrap_or(0.0),
                "last_active": usage.as_ref().and_then(|row| row.try_get::<Option<f64>, _>("last_active").ok().flatten()),
            },
        }));
    }
    Ok(out)
}

async fn messages_for(
    pool: &SqlitePool,
    project_id: &str,
    limit: usize,
) -> Result<Vec<Value>, String> {
    let rows = sqlx::query(
        "SELECT m.seq, m.ts, m.from_kind, m.from_member_id, m.from_label, m.to_kind, m.to_member_id, COALESCE(tm.display, m.to_member_id, m.to_kind) AS to_label, m.body, m.kind, m.item_id, m.run_id, m.deliver, m.delivered_at, m.reply_to FROM group_messages m LEFT JOIN members tm ON tm.id = m.to_member_id WHERE m.project_id = ? ORDER BY m.seq DESC LIMIT ?",
    )
    .bind(project_id)
    .bind(limit as i64)
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())?;
    let mut rows = rows;
    rows.reverse();
    Ok(rows
        .into_iter()
        .map(|row| {
            json!({
                "seq": row.get::<i64, _>("seq"),
                "ts": row.get::<f64, _>("ts"),
                "from": row.try_get::<String, _>("from_label").unwrap_or_else(|_| "unknown".to_string()),
                "to": row.try_get::<String, _>("to_label").unwrap_or_else(|_| "all".to_string()),
                "from_kind": row.try_get::<String, _>("from_kind").unwrap_or_else(|_| "system".to_string()),
                "from_member_id": row.try_get::<Option<String>, _>("from_member_id").ok().flatten(),
                "to_kind": row.try_get::<String, _>("to_kind").unwrap_or_else(|_| "all".to_string()),
                "to_member_id": row.try_get::<Option<String>, _>("to_member_id").ok().flatten(),
                "body": row.try_get::<String, _>("body").unwrap_or_default(),
                "kind": row.try_get::<String, _>("kind").unwrap_or_else(|_| "note".to_string()),
                "item_id": row.try_get::<Option<String>, _>("item_id").ok().flatten(),
                "run_id": row.try_get::<Option<String>, _>("run_id").ok().flatten(),
                "deliver": row.try_get::<i64, _>("deliver").unwrap_or(1) != 0,
                "delivered_at": row.try_get::<Option<f64>, _>("delivered_at").ok().flatten(),
                "reply_to": row.try_get::<Option<i64>, _>("reply_to").ok().flatten(),
            })
        })
        .collect())
}

async fn runs_json(
    pool: &SqlitePool,
    project_id: &str,
    limit: usize,
) -> Result<Vec<Value>, String> {
    let rows = sqlx::query(
        "SELECT r.id, r.project_id, r.cycle, r.item_id, r.member_id, r.agent_key, r.chat_id, r.run_id, r.kind, r.status, r.started_at, r.finished_at, r.input_tokens, r.output_tokens, r.cost_usd, r.report, r.error, i.title AS item_title, m.display AS member_display FROM group_runs r LEFT JOIN items i ON i.project_id = r.project_id AND i.id = r.item_id LEFT JOIN members m ON m.id = r.member_id WHERE r.project_id = ? ORDER BY r.started_at DESC, r.id DESC LIMIT ?",
    )
    .bind(project_id)
    .bind(limit as i64)
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok(rows
        .into_iter()
        .map(|row| {
            json!({
                "id": row.try_get::<String, _>("id").unwrap_or_default(),
                "project_id": row.try_get::<String, _>("project_id").unwrap_or_default(),
                "cycle": row.try_get::<i64, _>("cycle").unwrap_or(1),
                "item_id": row.try_get::<Option<String>, _>("item_id").ok().flatten(),
                "item_title": row.try_get::<Option<String>, _>("item_title").ok().flatten(),
                "member_id": row.try_get::<Option<String>, _>("member_id").ok().flatten(),
                "member": row.try_get::<Option<String>, _>("member_display").ok().flatten(),
                "agent_key": row.try_get::<Option<String>, _>("agent_key").ok().flatten(),
                "chat_id": row.try_get::<Option<String>, _>("chat_id").ok().flatten(),
                "run_id": row.try_get::<Option<String>, _>("run_id").ok().flatten(),
                "kind": row.try_get::<String, _>("kind").unwrap_or_else(|_| "item".to_string()),
                "status": row.try_get::<String, _>("status").unwrap_or_else(|_| "running".to_string()),
                "started_at": row.try_get::<Option<f64>, _>("started_at").ok().flatten(),
                "finished_at": row.try_get::<Option<f64>, _>("finished_at").ok().flatten(),
                "input_tokens": row.try_get::<i64, _>("input_tokens").unwrap_or(0),
                "output_tokens": row.try_get::<i64, _>("output_tokens").unwrap_or(0),
                "cost_usd": row.try_get::<f64, _>("cost_usd").unwrap_or(0.0),
                "report": row.try_get::<Option<String>, _>("report").ok().flatten(),
                "error": row.try_get::<Option<String>, _>("error").ok().flatten(),
            })
        })
        .collect())
}

async fn recent_events(
    pool: &SqlitePool,
    project_id: &str,
    limit: usize,
) -> Result<Vec<Value>, String> {
    let mut rows = sqlx::query(
        "SELECT seq, ts, kind, actor, milestone_id, item_id, data FROM events WHERE project_id = ? ORDER BY seq DESC LIMIT ?",
    )
    .bind(project_id)
    .bind(limit as i64)
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())?;
    rows.reverse();
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

async fn latest_message(pool: &SqlitePool, project_id: &str) -> Option<Value> {
    if let Ok(Some(row)) = sqlx::query(
        "SELECT seq, ts, from_kind, from_member_id, from_label, to_kind, to_member_id, body, kind, item_id, run_id FROM group_messages WHERE project_id = ? ORDER BY seq DESC LIMIT 1",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await
    {
        return Some(json!({
            "seq": row.get::<i64, _>("seq"),
            "ts": row.get::<f64, _>("ts"),
            "from": row.get::<String, _>("from_label"),
            "to": row.try_get::<String, _>("to_kind").unwrap_or_else(|_| "all".to_string()),
            "body": row.get::<String, _>("body"),
            "kind": row.get::<String, _>("kind"),
            "item_id": row.try_get::<Option<String>, _>("item_id").ok().flatten(),
            "run_id": row.try_get::<Option<String>, _>("run_id").ok().flatten(),
            "from_member_id": row.try_get::<Option<String>, _>("from_member_id").ok().flatten(),
            "to_member_id": row.try_get::<Option<String>, _>("to_member_id").ok().flatten(),
        }));
    }
    let row = sqlx::query(
        "SELECT seq, ts, kind, actor, item_id, data FROM events WHERE project_id = ? AND kind = 'message.sent' ORDER BY seq DESC LIMIT 1",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()?;
    message_from_event(&json!({
        "seq": row.get::<i64, _>("seq"),
        "ts": row.get::<f64, _>("ts"),
        "kind": row.get::<String, _>("kind"),
        "actor": row.try_get::<String, _>("actor").unwrap_or_default(),
        "item_id": row.try_get::<Option<String>, _>("item_id").ok().flatten(),
        "data": parse_json(row.get::<String, _>("data")),
    }))
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

async fn read_origin_chat(path: &Path) -> Option<String> {
    let text = tokio::fs::read_to_string(path.join(".gp").join("origin.json"))
        .await
        .ok()?;
    serde_json::from_str::<Value>(&text)
        .ok()?
        .get("chat_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
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

fn next_occurrence(now: f64, recurrence: &str, start_at: Option<f64>) -> f64 {
    let base = start_at.filter(|value| *value > 0.0).unwrap_or(now);
    let mut local = Local
        .timestamp_opt(base as i64, 0)
        .single()
        .unwrap_or_else(Local::now);
    // Walk forward from the configured anchor so a daily 09:00 project keeps
    // 09:00, instead of drifting to the time the previous cycle finished.
    for _ in 0..10_000 {
        local = match recurrence {
            "daily" => local + ChronoDuration::days(1),
            "weekly" => local + ChronoDuration::days(7),
            "monthly" => local
                .checked_add_months(Months::new(1))
                .unwrap_or_else(|| local + ChronoDuration::days(30)),
            _ => return now,
        };
        if local.timestamp() as f64 > now {
            return local.timestamp() as f64;
        }
    }
    now
}

fn normalize_milestone_status(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "new" | "pending" | "todo" => "todo".to_string(),
        "ongoing" | "active" | "ready" | "doing" => "active".to_string(),
        "done" | "complete" | "completed" => "done".to_string(),
        "blocked" | "failed" | "error" => "blocked".to_string(),
        "cancelled" | "canceled" => "cancelled".to_string(),
        other => other.to_string(),
    }
}

fn normalize_item_status(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "new" | "pending" | "todo" | "ready" => "todo".to_string(),
        "ongoing" | "active" | "running" | "doing" => "doing".to_string(),
        "needs_you" | "needs-approval" | "awaiting_approval" | "awaiting-approval" => {
            "awaiting_approval".to_string()
        }
        "done" | "complete" | "completed" => "done".to_string(),
        "failed" | "error" => "failed".to_string(),
        "blocked" => "blocked".to_string(),
        "cancelled" | "canceled" => "cancelled".to_string(),
        "invalidated" => "invalidated".to_string(),
        other => other.to_string(),
    }
}

fn resolve_member(rows: &[sqlx::sqlite::SqliteRow], owner: Option<&str>) -> Option<String> {
    let owner = owner?.trim();
    if owner.is_empty() {
        return None;
    }
    rows.iter()
        .find(|row| {
            row.try_get::<String, _>("id").ok().as_deref() == Some(owner)
                || row
                    .try_get::<String, _>("display")
                    .ok()
                    .is_some_and(|value| value.eq_ignore_ascii_case(owner))
                || row
                    .try_get::<String, _>("agent_key")
                    .ok()
                    .is_some_and(|value| value.eq_ignore_ascii_case(owner))
        })
        .and_then(|row| row.try_get::<String, _>("id").ok())
}

async fn resolve_target(
    pool: &SqlitePool,
    project_id: &str,
    to: &str,
) -> Result<(String, Option<String>), String> {
    let target = to.trim();
    let lower = target.to_ascii_lowercase();
    if lower.is_empty() || matches!(lower.as_str(), "all" | "everyone" | "team") {
        return Ok(("all".to_string(), None));
    }
    if matches!(lower.as_str(), "owner" | "you" | "human" | "user") {
        return Ok(("owner".to_string(), None));
    }
    if matches!(
        lower.as_str(),
        "supervisor" | "stark" | "orchestrator" | "planner"
    ) {
        return Ok(("supervisor".to_string(), None));
    }
    let member: Option<(String, String)> = sqlx::query_as(
        "SELECT id, display FROM members WHERE project_id = ? AND status = 'active' AND (id = ? OR display = ? OR agent_key = ?) LIMIT 1",
    )
    .bind(project_id)
    .bind(target)
    .bind(target)
    .bind(target)
    .fetch_optional(pool)
    .await
    .map_err(|error| error.to_string())?;
    if let Some((id, _display)) = member {
        return Ok(("member".to_string(), Some(id)));
    }
    // Display labels are case-insensitive in the UI.
    let rows =
        sqlx::query("SELECT id, display FROM members WHERE project_id = ? AND status = 'active'")
            .bind(project_id)
            .fetch_all(pool)
            .await
            .map_err(|error| error.to_string())?;
    for row in rows {
        let id: String = row.get("id");
        let display: String = row.get("display");
        if display.eq_ignore_ascii_case(target) {
            return Ok(("member".to_string(), Some(id)));
        }
    }
    Err(format!("unknown message target '{target}'"))
}

async fn pool_project_workspace(pool: &SqlitePool, project_id: &str) -> Option<String> {
    sqlx::query_scalar("SELECT workspace_path FROM projects WHERE id = ?")
        .bind(project_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
}

fn strip_cycle_suffix(id: &str) -> String {
    if let Some((base, suffix)) = id.rsplit_once("-c")
        && !base.is_empty()
        && suffix.chars().all(|character| character.is_ascii_digit())
    {
        return base.to_string();
    }
    id.to_string()
}

fn item_to_plan_json(row: &sqlx::sqlite::SqliteRow) -> Value {
    json!({
        "id": row.try_get::<String, _>("id").unwrap_or_default(),
        "milestone_id": row.try_get::<Option<String>, _>("milestone_id").ok().flatten(),
        "title": row.try_get::<String, _>("title").unwrap_or_default(),
        "detail": row.try_get::<String, _>("detail").unwrap_or_default(),
        "acceptance": parse_json(row.try_get::<String, _>("acceptance_json").unwrap_or_else(|_| "[]".to_string())),
        "tags": parse_json(row.try_get::<String, _>("tags_json").unwrap_or_else(|_| "[]".to_string())),
        "depends_on": parse_json(row.try_get::<String, _>("depends_on_json").unwrap_or_else(|_| "[]".to_string())),
        "priority": row.try_get::<i64, _>("priority").unwrap_or(0),
        "due_at": row.try_get::<Option<f64>, _>("due_at").ok().flatten(),
        "requires_approval": row.try_get::<i64, _>("requires_approval").unwrap_or(0) != 0,
        "max_attempts": row.try_get::<i64, _>("max_attempts").unwrap_or(3),
        "owner_member": row.try_get::<Option<String>, _>("owner_member").ok().flatten(),
        "cycle": row.try_get::<i64, _>("cycle").unwrap_or(1),
    })
}

async fn rename_with_retry(from: &Path, to: &Path) -> Result<(), String> {
    let mut last = String::new();
    for _ in 0..30 {
        match tokio::fs::rename(from, to).await {
            Ok(()) => return Ok(()),
            Err(error) => {
                last = error.to_string();
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
    }
    Err(format!("could not move project folder: {last}"))
}

fn validate_trash_name(folder: &str) -> Result<(), String> {
    let trimmed = folder.trim();
    if trimmed.is_empty()
        || trimmed == "."
        || trimmed == ".."
        || trimmed.len() > 180
        || trimmed.contains(['/', '\\'])
        || trimmed.chars().any(char::is_control)
    {
        return Err("invalid trash entry".to_string());
    }
    Ok(())
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
    async fn owner_message_is_projected_into_the_group_chat() {
        let dir = tempfile::tempdir().unwrap();
        let store = GroupStore::new(dir.path());
        store
            .create(
                &json!({
                    "id": "chat-demo",
                    "goal": "Talk to the team",
                    "milestones": [{
                        "id": "m1",
                        "title": "Build",
                        "items": [{"id": "i1", "title": "Do it"}]
                    }]
                }),
                "ws-1",
            )
            .await
            .unwrap();
        store
            .update(
                "chat-demo",
                "message",
                &json!({"body": "Please compare pricing first", "to": "all"}),
            )
            .await
            .unwrap();
        let detail = store.detail("chat-demo", 60).await.unwrap();
        let messages = detail["messages"].as_array().cloned().unwrap_or_default();
        assert!(messages.iter().any(|message| {
            message["body"] == "Please compare pricing first"
                && message["from"] == "owner"
                && message["to"] == "all"
        }));
    }

    #[tokio::test]
    async fn stores_origin_chat_for_group_notifications() {
        let dir = tempfile::tempdir().unwrap();
        let store = GroupStore::new(dir.path());
        store
            .create_with_origin(
                &json!({
                    "id": "origin-test",
                    "goal": "Notify the manager",
                    "milestones": [{
                        "id": "m1",
                        "title": "Build",
                        "items": [{"id": "i1", "title": "Do it"}]
                    }]
                }),
                "ws-1",
                "chat-origin",
            )
            .await
            .unwrap();
        assert_eq!(
            store.origin_chat("origin-test").await.as_deref(),
            Some("chat-origin")
        );
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
            "milestones": [{"id": "m1", "title": "First", "items": [{"id": "i1", "title": "One", "max_attempts": 1}]}]
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
    async fn workspace_and_member_slots_are_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("real-work");
        std::fs::create_dir_all(&workspace).unwrap();
        let store = GroupStore::new(dir.path().join("projects"));
        let detail = store
            .create(
                &json!({
                    "id": "slots",
                    "title": "Slots",
                    "goal": "Prove the team model",
                    "workspace_path": workspace.to_string_lossy(),
                    "members": [
                        {"agent_key": "developer", "amount": 2},
                        {"agent_key": "ui", "amount": 1}
                    ],
                    "max_parallel": 2,
                    "milestones": [{"id": "m1", "title": "Build", "items": [{"id": "i1", "title": "One"}]}]
                }),
                "ws-1",
            )
            .await
            .unwrap();
        assert_eq!(
            detail["workspace_path"],
            workspace.to_string_lossy().to_string()
        );
        assert_eq!(detail["members"].as_array().unwrap().len(), 3);
        assert_eq!(detail["project"]["max_parallel"], 2);
        let listed = store.list().await;
        assert_eq!(
            listed[0]["workspace_path"],
            workspace.to_string_lossy().to_string()
        );
        assert_eq!(listed[0]["members_count"], 3);
    }

    #[tokio::test]
    async fn member_slots_limit_parallel_claims() {
        let dir = tempfile::tempdir().unwrap();
        let store = GroupStore::new(dir.path().join("projects"));
        store
            .create(
                &json!({
                    "id": "capacity",
                    "goal": "Capacity",
                    "members": [{"agent_key": "developer", "amount": 2}],
                    "max_parallel": 2,
                    "milestones": [{"id": "m1", "title": "Build", "items": [
                        {"id": "i1", "title": "One"},
                        {"id": "i2", "title": "Two"},
                        {"id": "i3", "title": "Three"}
                    ]}]
                }),
                "ws-1",
            )
            .await
            .unwrap();
        let first = store.claim_next_ready("capacity").await.unwrap().unwrap();
        let second = store.claim_next_ready("capacity").await.unwrap().unwrap();
        assert_ne!(first["owner_member_id"], second["owner_member_id"]);
        for item in [&first, &second] {
            let member = item["owner_member_id"].as_str().unwrap();
            store
                .start_run(
                    "capacity",
                    1,
                    item["id"].as_str(),
                    Some(member),
                    item["member"]["agent_key"].as_str().unwrap(),
                    None,
                    None,
                    "item",
                )
                .await
                .unwrap();
        }
        assert!(store.claim_next_ready("capacity").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn repeat_cycle_preserves_previous_done_milestones() {
        let dir = tempfile::tempdir().unwrap();
        let store = GroupStore::new(dir.path().join("projects"));
        store
            .create(
                &json!({
                    "id": "repeat",
                    "goal": "Repeat",
                    "milestones": [{"id": "m1", "title": "One", "items": [{"id": "i1", "title": "Do it"}]}]
                }),
                "ws-1",
            )
            .await
            .unwrap();
        let item = store.claim_next_ready("repeat").await.unwrap().unwrap();
        store
            .finish_item("repeat", item["id"].as_str().unwrap(), true, "done")
            .await
            .unwrap();
        let done = store.detail("repeat", 20).await.unwrap();
        assert_eq!(done["project"]["status"], "done");
        assert_eq!(done["milestones"][0]["status"], "done");
        let next = store.start_next_cycle("repeat").await.unwrap();
        assert_eq!(next["current_cycle"], 2);
        assert_eq!(next["milestones"].as_array().unwrap().len(), 2);
        assert_eq!(next["milestones"][0]["status"], "done");
        assert_eq!(next["milestones"][1]["status"], "pending");
    }

    #[tokio::test]
    async fn owner_messages_queue_until_a_member_is_ready() {
        let dir = tempfile::tempdir().unwrap();
        let store = GroupStore::new(dir.path().join("projects"));
        store
            .create(
                &json!({
                    "id": "messages",
                    "goal": "Messages",
                    "members": [{"agent_key": "developer", "amount": 1}],
                    "milestones": [{"id": "m1", "title": "One", "items": [{"id": "i1", "title": "Do it"}]}]
                }),
                "ws-1",
            )
            .await
            .unwrap();
        let member = store.detail("messages", 20).await.unwrap()["members"][0]["id"]
            .as_str()
            .unwrap()
            .to_string();
        store
            .send_message(
                "messages",
                "Please use the blue palette",
                "all",
                "note",
                "owner",
                None,
                "owner",
                None,
                None,
                true,
            )
            .await
            .unwrap();
        let pending = store.pending_messages("messages", &member).await.unwrap();
        assert_eq!(pending.len(), 1);
        let seq = pending[0]["seq"].as_i64().unwrap();
        store.mark_delivered("messages", &[seq]).await.unwrap();
        assert!(
            store
                .pending_messages("messages", &member)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn trash_restore_keeps_the_workspace_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("work");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("keep.txt"), "keep").unwrap();
        let store = GroupStore::new(dir.path().join("projects"));
        store
            .create(
                &json!({
                    "id": "trash-me",
                    "goal": "Trash",
                    "workspace_path": workspace.to_string_lossy(),
                    "milestones": [{"id": "m1", "title": "One", "items": [{"id": "i1", "title": "Do it"}]}]
                }),
                "ws-1",
            )
            .await
            .unwrap();
        let trashed = store.trash("trash-me").await.unwrap();
        let folder = trashed["folder"].as_str().unwrap().to_string();
        assert!(store.list().await.is_empty());
        assert_eq!(store.trash_list().await.len(), 1);
        assert!(workspace.join("keep.txt").is_file());
        let restored = store.restore(&folder).await.unwrap();
        assert_eq!(restored["id"], "trash-me");
        assert!(
            store
                .list()
                .await
                .iter()
                .any(|project| project["id"] == "trash-me")
        );
    }

    #[tokio::test]
    async fn reviewed_plan_waits_for_approval() {
        let dir = tempfile::tempdir().unwrap();
        let store = GroupStore::new(dir.path().join("projects"));
        store
            .create(
                &json!({
                    "id": "plan-review",
                    "goal": "Plan review",
                    "auto_plan": false,
                    "plan_review": true,
                    "milestones": []
                }),
                "ws-1",
            )
            .await
            .unwrap();
        store
            .apply_plan(
                "plan-review",
                &json!({
                    "milestones": [{
                        "title": "Build",
                        "items": [{"title": "Write it", "acceptance": ["It exists"]}]
                    }]
                }),
            )
            .await
            .unwrap();
        let detail = store.detail("plan-review", 20).await.unwrap();
        assert_eq!(detail["project"]["status"], "plan_ready");
        assert!(
            store
                .claim_next_ready("plan-review")
                .await
                .unwrap()
                .is_none()
        );
        store.approve_plan("plan-review").await.unwrap();
        let detail = store.detail("plan-review", 20).await.unwrap();
        assert_eq!(detail["project"]["status"], "running");
        assert!(
            store
                .claim_next_ready("plan-review")
                .await
                .unwrap()
                .is_some()
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
