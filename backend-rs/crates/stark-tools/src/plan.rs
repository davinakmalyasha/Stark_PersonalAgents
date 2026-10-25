use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use stark_contract::PlanItem;
use thiserror::Error;

use crate::{ToolContext, ToolOutcome};

const MAX_STEP_CHARS: usize = 200;
const PLAN_STATUSES: &[&str] = &["pending", "in_progress", "completed"];

#[derive(Debug, Error)]
pub enum PlanError {
    #[error("invalid chat id: {0}")]
    InvalidChatId(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Clone, Debug)]
pub struct PlanStore {
    root: PathBuf,
}

impl PlanStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn task_path(&self, chat_id: &str) -> Result<PathBuf, PlanError> {
        let filename = if chat_id.is_empty() {
            "task.json".to_string()
        } else {
            if !chat_id.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
            }) {
                return Err(PlanError::InvalidChatId(chat_id.to_string()));
            }
            format!("task_{chat_id}.json")
        };
        Ok(self.root.join(".deep_agents").join("tasks").join(filename))
    }

    pub async fn load(&self, chat_id: &str) -> Result<Option<Value>, PlanError> {
        let path = self.task_path(chat_id)?;
        match tokio::fs::read_to_string(&path).await {
            Ok(raw) => Ok(serde_json::from_str(&raw).ok()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub async fn save(&self, chat_id: &str, task: &Value) -> Result<(), PlanError> {
        let path = self.task_path(chat_id)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let raw = serde_json::to_string_pretty(task)?;
        let temporary = path.with_extension("json.tmp");
        tokio::fs::write(&temporary, raw).await?;
        if let Err(error) = tokio::fs::rename(&temporary, &path).await {
            if path.exists() {
                tokio::fs::remove_file(&path).await?;
                tokio::fs::rename(&temporary, &path).await?;
            } else {
                return Err(error.into());
            }
        }
        Ok(())
    }

    pub async fn plan_state(
        &self,
        chat_id: &str,
    ) -> Result<BTreeMap<String, Vec<PlanItem>>, PlanError> {
        let Some(task) = self.load(chat_id).await? else {
            return Ok(BTreeMap::new());
        };
        Ok(plan_state_from_task(&task))
    }

    pub async fn load_plan(&self, chat_id: &str, scope: &str) -> Result<Vec<PlanItem>, PlanError> {
        let plans = self.plan_state(chat_id).await?;
        let scope = scope.trim().to_lowercase();
        Ok(if scope.is_empty() {
            plans
                .values()
                .find(|items| !items.is_empty())
                .cloned()
                .unwrap_or_default()
        } else {
            plans.get(&scope).cloned().unwrap_or_default()
        })
    }

    pub async fn progress_block(&self, chat_id: &str, scope: &str) -> Result<String, PlanError> {
        let items = self.load_plan(chat_id, scope).await?;
        Ok(progress_block(&items))
    }

    async fn write_plan(
        &self,
        chat_id: &str,
        scope: &str,
        items: &[PlanItem],
    ) -> Result<BTreeMap<String, Vec<PlanItem>>, PlanError> {
        let now = now_iso();
        let mut task = self
            .load(chat_id)
            .await?
            .unwrap_or_else(|| new_task(chat_id, &now));
        if !task.is_object() {
            task = new_task(chat_id, &now);
        }
        let object = task.as_object_mut().expect("task is an object");
        object
            .entry("task_id".to_string())
            .or_insert_with(|| json!(format!("auto_{chat_id}")));
        object
            .entry("chat_id".to_string())
            .or_insert_with(|| json!(chat_id));
        object
            .entry("created_at".to_string())
            .or_insert_with(|| json!(now));
        object
            .entry("status".to_string())
            .or_insert_with(|| json!("in_progress"));
        let plans = object
            .entry("plan".to_string())
            .or_insert_with(|| json!({}));
        if !plans.is_object() {
            *plans = json!({});
        }
        plans
            .as_object_mut()
            .expect("plan is an object")
            .insert(normalize_scope(scope), serde_json::to_value(items)?);
        object.insert("updated_at".to_string(), json!(now));
        self.save(chat_id, &task).await?;
        Ok(plan_state_from_task(&task))
    }
}

pub async fn update_plan(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let plan_arg = args.get("plan").filter(|value| !value.is_null());
    let ops_arg = args.get("ops").filter(|value| !value.is_null());
    if plan_arg.is_some() && ops_arg.is_some() {
        return ToolOutcome::err(
            "Error: pass either plan (full replacement) or ops (incremental), not both.",
        );
    }

    let (items, notes) = if let Some(ops) = ops_arg {
        let current = match ctx.plan_store.load_plan(&ctx.chat_id, &ctx.scope).await {
            Ok(items) => items,
            Err(error) => return ToolOutcome::err(format!("Error: {error}")),
        };
        let (patched, mut patch_notes) = apply_ops(&current, ops);
        let (items, errors) = normalize_plan_items(&patched);
        patch_notes.extend(errors);
        if items.is_empty() {
            let suffix = note_suffix(&patch_notes);
            return ToolOutcome::err(format!("Error: ops left the plan empty{suffix}"));
        }
        (items, patch_notes)
    } else {
        let Some(plan) = plan_arg else {
            return ToolOutcome::err(
                "Error: update_plan requires a non-empty plan: plan must contain at least one step",
            );
        };
        let (items, errors) = normalize_plan_value(plan);
        if items.is_empty() {
            let suffix = note_suffix(&errors);
            return ToolOutcome::err(format!(
                "Error: update_plan requires a non-empty plan{suffix}"
            ));
        }
        (items, errors)
    };

    let completed = items
        .iter()
        .filter(|item| item.status == "completed")
        .count();
    let in_progress = items
        .iter()
        .filter(|item| item.status == "in_progress")
        .count();
    let plan = match ctx
        .plan_store
        .write_plan(&ctx.chat_id, &ctx.scope, &items)
        .await
    {
        Ok(plan) => plan,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };
    let suffix = note_suffix(&notes);
    ToolOutcome::ok(format!(
        "Plan updated ({completed}/{} completed, {in_progress} in progress){suffix}",
        items.len()
    ))
    .with_plan(plan)
}

fn normalize_plan_value(value: &Value) -> (Vec<PlanItem>, Vec<String>) {
    let Some(raw_items) = value.as_array() else {
        return (
            Vec::new(),
            vec!["plan items must be objects with 'step' and 'status'".to_string()],
        );
    };
    let mut parsed = Vec::new();
    let mut errors = Vec::new();
    for raw in raw_items {
        let Some(object) = raw.as_object() else {
            errors.push("plan items must be objects with 'step' and 'status'".to_string());
            continue;
        };
        parsed.push(PlanItem {
            step: object
                .get("step")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            status: object
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending")
                .to_string(),
        });
    }
    let (items, item_errors) = normalize_plan_items(&parsed);
    errors.extend(item_errors);
    (items, errors)
}

fn normalize_plan_items(raw_items: &[PlanItem]) -> (Vec<PlanItem>, Vec<String>) {
    let mut items = Vec::new();
    let mut errors = Vec::new();
    for raw in raw_items {
        let step = raw.step.trim();
        if step.is_empty() {
            continue;
        }
        let status = raw.status.trim().to_lowercase();
        if !PLAN_STATUSES.contains(&status.as_str()) {
            errors.push(format!(
                "invalid status '{status}' (must be pending|in_progress|completed)"
            ));
            continue;
        }
        items.push(PlanItem {
            step: step.chars().take(MAX_STEP_CHARS).collect(),
            status,
        });
    }
    if items.is_empty() {
        if errors.is_empty() {
            errors.push("plan must contain at least one step".to_string());
        }
        return (items, errors);
    }

    let mut seen_in_progress = false;
    let mut demoted = 0;
    for item in &mut items {
        if item.status == "in_progress" {
            if seen_in_progress {
                item.status = "pending".to_string();
                demoted += 1;
            } else {
                seen_in_progress = true;
            }
        }
    }
    if demoted > 0 {
        errors.push(format!(
            "demoted {demoted} extra in_progress step(s) to pending (at most one allowed)"
        ));
    }
    (items, errors)
}

fn apply_ops(current: &[PlanItem], raw_ops: &Value) -> (Vec<PlanItem>, Vec<String>) {
    let mut working = current.to_vec();
    let mut notes = Vec::new();
    let Some(ops) = raw_ops.as_array() else {
        notes.push("ops must be an array".to_string());
        return (working, notes);
    };
    for raw in ops {
        let Some(object) = raw.as_object() else {
            notes.push("skipped a non-object op".to_string());
            continue;
        };
        let kind = object
            .get("op")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_lowercase();
        if kind == "add" {
            let step = object
                .get("step")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if step.is_empty() {
                notes.push("add needs 'step'".to_string());
                continue;
            }
            let status = clean_status(object.get("status").and_then(Value::as_str), &mut notes);
            working.push(PlanItem {
                step: step.chars().take(MAX_STEP_CHARS).collect(),
                status,
            });
            continue;
        }
        if matches!(kind.as_str(), "set_status" | "rename" | "remove") {
            let needle = object
                .get("match")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_lowercase();
            if needle.is_empty() {
                notes.push(format!("{kind} needs 'match'"));
                continue;
            }
            let matches: Vec<usize> = working
                .iter()
                .enumerate()
                .filter(|(_, item)| item.step.to_lowercase().contains(&needle))
                .map(|(index, _)| index)
                .collect();
            if matches.is_empty() {
                notes.push(format!(
                    "no step matches '{}'",
                    object.get("match").and_then(Value::as_str).unwrap_or("")
                ));
                continue;
            }
            if kind == "remove" {
                working = working
                    .into_iter()
                    .enumerate()
                    .filter(|(index, _)| !matches.contains(index))
                    .map(|(_, item)| item)
                    .collect();
            } else if kind == "set_status" {
                let status = clean_status(object.get("status").and_then(Value::as_str), &mut notes);
                for index in matches {
                    working[index].status = status.clone();
                }
            } else {
                let step = object
                    .get("step")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                if step.is_empty() {
                    notes.push("rename needs 'step' (new text)".to_string());
                    continue;
                }
                for index in matches {
                    working[index].step = step.chars().take(MAX_STEP_CHARS).collect();
                }
            }
            continue;
        }
        notes.push(format!(
            "unknown op '{}' (want add|set_status|rename|remove)",
            object.get("op").and_then(Value::as_str).unwrap_or("")
        ));
    }
    (working, notes)
}

fn clean_status(value: Option<&str>, notes: &mut Vec<String>) -> String {
    let status = value.unwrap_or("pending").trim().to_lowercase();
    if PLAN_STATUSES.contains(&status.as_str()) {
        return status;
    }
    notes.push(format!(
        "invalid status '{}' (want pending|in_progress|completed) — used pending",
        value.unwrap_or("")
    ));
    "pending".to_string()
}

fn note_suffix(notes: &[String]) -> String {
    if notes.is_empty() {
        String::new()
    } else {
        format!(" ({})", notes.join("; "))
    }
}

fn plan_state_from_task(task: &Value) -> BTreeMap<String, Vec<PlanItem>> {
    let mut result = BTreeMap::new();
    let Some(plans) = task.get("plan").and_then(Value::as_object) else {
        return result;
    };
    for (scope, value) in plans {
        let Some(items) = value.as_array() else {
            continue;
        };
        let parsed: Vec<PlanItem> = items
            .iter()
            .filter_map(|item| {
                let step = item.get("step").and_then(Value::as_str)?.trim();
                if step.is_empty() {
                    return None;
                }
                let status = item
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("pending")
                    .trim()
                    .to_lowercase();
                Some(PlanItem {
                    step: step.chars().take(MAX_STEP_CHARS).collect(),
                    status: if PLAN_STATUSES.contains(&status.as_str()) {
                        status
                    } else {
                        "pending".to_string()
                    },
                })
            })
            .collect();
        if !parsed.is_empty() {
            result.insert(normalize_scope(scope), parsed);
        }
    }
    result
}

fn progress_block(items: &[PlanItem]) -> String {
    if items.is_empty() {
        return String::new();
    }
    let completed = items
        .iter()
        .filter(|item| item.status == "completed")
        .count();
    if completed == items.len() {
        return String::new();
    }
    let mut lines = vec![format!(
        "=== CURRENT PLAN ({completed}/{} completed) ===",
        items.len()
    )];
    for (index, item) in items.iter().enumerate() {
        let marker = match item.status.as_str() {
            "completed" => "[x]",
            "in_progress" => "[/]",
            _ => "[ ]",
        };
        lines.push(format!("  {}. {marker} {}", index + 1, item.step));
    }
    lines.push("=== END PLAN ===".to_string());
    lines.join("\n")
}

fn normalize_scope(scope: &str) -> String {
    let scope = scope.trim().to_lowercase();
    if scope.is_empty() {
        "agent".to_string()
    } else {
        scope
    }
}

fn new_task(chat_id: &str, now: &str) -> Value {
    json!({
        "task_id": format!("auto_{chat_id}"),
        "chat_id": chat_id,
        "user_request": "",
        "created_at": now,
        "updated_at": now,
        "status": "in_progress",
        "artifacts": {
            "files_created": [],
            "files_modified": [],
            "tests_passing": false
        },
        "plan": {}
    })
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tempfile::tempdir;

    use super::*;
    use crate::questions::QuestionBroker;

    fn context(workspace: &std::path::Path) -> ToolContext {
        ToolContext::new(workspace, "chat-plan", Arc::new(QuestionBroker::new()))
    }

    async fn stored_plan(ctx: &ToolContext) -> Vec<PlanItem> {
        ctx.plan_store
            .load_plan(&ctx.chat_id, &ctx.scope)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn full_replacement_persists_and_returns_plan() {
        let dir = tempdir().unwrap();
        let ctx = context(dir.path());
        let outcome = update_plan(
            &ctx,
            &json!({
                "plan": [
                    {"step": "build x", "status": "in_progress"},
                    {"step": "verify y", "status": "pending"}
                ]
            }),
        )
        .await;
        assert!(!outcome.is_error, "{}", outcome.content);
        assert_eq!(
            outcome.content,
            "Plan updated (0/2 completed, 1 in progress)"
        );
        assert_eq!(stored_plan(&ctx).await[0].step, "build x");
        assert!(outcome.plan.is_some());
        assert_eq!(outcome.plan.unwrap()["agent"][0].step, "build x");
    }

    #[tokio::test]
    async fn ops_add_set_rename_and_remove() {
        let dir = tempdir().unwrap();
        let ctx = context(dir.path());
        update_plan(
            &ctx,
            &json!({
                "plan": [
                    {"step": "build x", "status": "in_progress"},
                    {"step": "drop me", "status": "pending"}
                ]
            }),
        )
        .await;
        let outcome = update_plan(
            &ctx,
            &json!({
                "ops": [
                    {"op": "set_status", "match": "build", "status": "completed"},
                    {"op": "add", "step": "ship it"},
                    {"op": "rename", "match": "ship", "step": "ship it now"},
                    {"op": "remove", "match": "drop"}
                ]
            }),
        )
        .await;
        assert!(!outcome.is_error, "{}", outcome.content);
        assert_eq!(
            outcome.content,
            "Plan updated (1/2 completed, 0 in progress)"
        );
        let plan = stored_plan(&ctx).await;
        assert_eq!(plan[0].step, "build x");
        assert_eq!(plan[0].status, "completed");
        assert_eq!(plan[1].step, "ship it now");
    }

    #[tokio::test]
    async fn rejects_both_modes_and_empty_plans() {
        let dir = tempdir().unwrap();
        let ctx = context(dir.path());
        let both = update_plan(
            &ctx,
            &json!({
                "plan": [{"step": "a", "status": "pending"}],
                "ops": [{"op": "add", "step": "b"}]
            }),
        )
        .await;
        assert!(both.content.starts_with("Error:"));
        assert!(both.plan.is_none());

        let empty = update_plan(
            &ctx,
            &json!({"ops": [{"op": "set_status", "match": "nope", "status": "completed"}]}),
        )
        .await;
        assert!(empty.content.starts_with("Error:"));
        assert!(empty.plan.is_none());
    }

    #[tokio::test]
    async fn demotes_extra_in_progress_and_caps_steps() {
        let dir = tempdir().unwrap();
        let ctx = context(dir.path());
        let long_step = "x".repeat(MAX_STEP_CHARS + 50);
        let outcome = update_plan(
            &ctx,
            &json!({
                "plan": [
                    {"step": "first", "status": "in_progress"},
                    {"step": "second", "status": "in_progress"},
                    {"step": long_step, "status": "pending"}
                ]
            }),
        )
        .await;
        assert!(!outcome.is_error, "{}", outcome.content);
        assert!(outcome.content.contains("demoted 1 extra"));
        let plan = stored_plan(&ctx).await;
        assert_eq!(plan[1].status, "pending");
        assert_eq!(plan[2].step.chars().count(), MAX_STEP_CHARS);
    }

    #[tokio::test]
    async fn preserves_unknown_task_fields_and_progress_block() {
        let dir = tempdir().unwrap();
        let ctx = context(dir.path());
        let path = ctx.plan_store.task_path(&ctx.chat_id).unwrap();
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(
            &path,
            serde_json::to_string_pretty(&json!({
                "task_id": "existing",
                "custom": {"keep": true},
                "plan": {"agent": [{"step": "old", "status": "pending"}]}
            }))
            .unwrap(),
        )
        .await
        .unwrap();

        update_plan(
            &ctx,
            &json!({"plan": [{"step": "new", "status": "in_progress"}]}),
        )
        .await;
        let task: Value =
            serde_json::from_str(&tokio::fs::read_to_string(&path).await.unwrap()).unwrap();
        assert_eq!(task["custom"]["keep"], true);
        assert_eq!(task["task_id"], "existing");
        let block = ctx
            .plan_store
            .progress_block(&ctx.chat_id, &ctx.scope)
            .await
            .unwrap();
        assert!(block.contains("0/1 completed"));
        assert!(block.contains("[/] new"));

        update_plan(
            &ctx,
            &json!({"plan": [{"step": "new", "status": "completed"}]}),
        )
        .await;
        let completed = ctx
            .plan_store
            .progress_block(&ctx.chat_id, &ctx.scope)
            .await
            .unwrap();
        assert!(completed.is_empty());
    }
}
