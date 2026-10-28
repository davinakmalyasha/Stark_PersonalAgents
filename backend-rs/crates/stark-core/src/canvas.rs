//! `canvas_mode`: watch agent chats/groups and wake the orchestrator.
//!
//! This is the Rust counterpart of `tools/AddOnTool/canvasTools.py` +
//! `server/services/canvas_watch.py`. The tool only reads work state and
//! queues wakes; the server's canvas loop performs them, so a tool call never
//! blocks on another agent's run.

use serde_json::{Value, json};
use stark_store::{CanvasStore, GroupStore};
use stark_tools::ToolOutcome;

use crate::RunContext;
use stark_store::ChatStore;

fn is_agent_chat(chat: &Value) -> bool {
    chat["parentChatId"]
        .as_str()
        .is_some_and(|parent| !parent.trim().is_empty())
        || chat["id"].as_str().is_some_and(|id| id.starts_with("gp-"))
}

fn chat_snapshot(chat: &Value) -> Value {
    json!({
        "kind": "chat",
        "id": chat["id"],
        "workspacePath": chat["workspacePath"],
        "title": chat["title"],
        "parentChatId": chat["parentChatId"],
        "running": false,
        "lastRole": chat["lastRole"],
        "lastMessage": chat["lastMessage"],
        "updatedAt": chat["updatedAt"],
    })
}

fn group_snapshot(group: &Value) -> Value {
    json!({
        "kind": "group",
        "id": group["id"],
        "title": group["title"],
        "status": group["status"],
        "itemsDone": group["items_done"],
        "itemsTotal": group["items_total"],
        "milestonesDone": group["milestones_done"],
        "milestonesTotal": group["milestones_total"],
        "awaitingApproval": group["awaiting_approval"],
        "spentUsd": group["spent_usd"],
        "budgetUsd": group["budget_usd"],
    })
}

async fn all_agent_chats(ctx: &RunContext) -> Vec<Value> {
    ctx.store
        .all_chats()
        .await
        .into_iter()
        .filter(is_agent_chat)
        .map(|chat| chat_snapshot(&chat))
        .collect()
}

async fn all_groups() -> Vec<Value> {
    GroupStore::default_root()
        .list()
        .await
        .iter()
        .map(group_snapshot)
        .collect()
}

async fn available(ctx: &RunContext) -> String {
    let chats = all_agent_chats(ctx).await;
    let groups = all_groups().await;
    let mut lines = vec!["[CANVAS TARGETS]".to_string()];
    for chat in &chats {
        let role = chat["lastRole"].as_str().unwrap_or("idle");
        lines.push(format!(
            "- chat:{} · {} · {}",
            chat["id"].as_str().unwrap_or("?"),
            chat["title"].as_str().unwrap_or("?"),
            role
        ));
    }
    for group in &groups {
        lines.push(format!(
            "- group:{} · {} · {}",
            group["id"].as_str().unwrap_or("?"),
            group["title"].as_str().unwrap_or("?"),
            group["status"].as_str().unwrap_or("unknown")
        ));
    }
    if lines.len() == 1 {
        lines.push("(no agent chats or groups)".to_string());
    }
    lines.join("\n")
}

async fn snapshot(ctx: &RunContext, store: &CanvasStore) -> Result<Value, String> {
    let watch = store
        .active_watch(&ctx.chat_id)
        .await
        .map_err(|error| error.to_string())?;
    let Some(watch) = watch else {
        return Ok(json!({"watch": null, "chats": [], "groups": [], "events": []}));
    };
    let mode = watch["mode"].as_str().unwrap_or("all");
    let targets: Vec<String> = watch["targets"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let all_chats = ctx.store.all_chats().await;
    let all_groups = all_groups().await;
    let mut chats = Vec::new();
    let mut groups = Vec::new();
    if mode == "all" {
        chats = all_chats
            .iter()
            .filter(|chat| is_agent_chat(chat))
            .map(chat_snapshot)
            .collect();
    } else {
        for target in &targets {
            if let Some(group_id) = target.strip_prefix("group:") {
                if let Some(group) = all_groups.iter().find(|group| group["id"] == group_id) {
                    groups.push(group.clone());
                }
            } else if let Some(chat) = all_chats.iter().find(|chat| chat["id"] == target.as_str()) {
                chats.push(chat_snapshot(chat));
            }
        }
    }
    let events = store
        .events(watch["id"].as_str().unwrap_or(""), 20)
        .await
        .map_err(|error| error.to_string())?;
    Ok(json!({"watch": watch, "chats": chats, "groups": groups, "events": events}))
}

fn format_snapshot(data: &Value) -> String {
    let Some(watch) = data.get("watch").filter(|watch| !watch.is_null()) else {
        return "[OK] Canvas mode is off.".to_string();
    };
    let targets = watch["targets"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|targets| !targets.is_empty())
        .unwrap_or_else(|| "all agent chats".to_string());
    let mut lines = vec![
        format!(
            "[CANVAS MODE] {} · {}/{} actions",
            watch["mode"].as_str().unwrap_or("all"),
            watch["actionsUsed"].as_i64().unwrap_or(0),
            watch["maxActions"].as_i64().unwrap_or(0)
        ),
        format!("Targets: {targets}"),
    ];
    for chat in data["chats"].as_array().into_iter().flatten() {
        lines.push(format!(
            "- chat {} · {} · {}",
            chat["id"].as_str().unwrap_or("?"),
            chat["title"].as_str().unwrap_or("?"),
            chat["lastRole"].as_str().unwrap_or("idle")
        ));
    }
    for group in data["groups"].as_array().into_iter().flatten() {
        lines.push(format!(
            "- group {} · {} · {} · items {}/{}",
            group["id"].as_str().unwrap_or("?"),
            group["title"].as_str().unwrap_or("?"),
            group["status"].as_str().unwrap_or("unknown"),
            group["itemsDone"].as_i64().unwrap_or(0),
            group["itemsTotal"].as_i64().unwrap_or(0)
        ));
    }
    for event in data["events"].as_array().into_iter().flatten() {
        lines.push(format!(
            "  event {} · {}",
            event["kind"].as_str().unwrap_or("?"),
            event["sourceChatId"].as_str().unwrap_or("?")
        ));
    }
    lines.join("\n")
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

async fn enter(ctx: &RunContext, store: &CanvasStore, args: &Value) -> Result<String, String> {
    let mode = args
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("all")
        .trim()
        .to_ascii_lowercase();
    let mode = if matches!(mode.as_str(), "single" | "multiple" | "all") {
        mode
    } else {
        "all".to_string()
    };
    let targets = strings(args.get("targets"));
    if mode != "all" && targets.is_empty() {
        return Err("select at least one chat or group when mode is single/multiple".to_string());
    }
    let chats = ctx.store.all_chats().await;
    let groups = all_groups().await;
    for target in &targets {
        if let Some(group_id) = target.strip_prefix("group:") {
            if !groups.iter().any(|group| group["id"] == group_id) {
                return Err(format!("unknown group {group_id}"));
            }
        } else if !chats.iter().any(|chat| chat["id"] == target.as_str()) {
            return Err(format!("unknown chat {target}"));
        }
    }
    let wake_on = {
        let values = strings(args.get("wake_on"));
        if values.is_empty() {
            vec!["completed".to_string(), "failed".to_string()]
        } else {
            values
        }
    };
    let max_actions = args
        .get("max_actions")
        .and_then(Value::as_i64)
        .unwrap_or(20)
        .clamp(1, 500);
    let until = args
        .get("until")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    store
        .enter_watch(
            &ctx.chat_id,
            ctx.workspace.to_string_lossy().as_ref(),
            &mode,
            &targets,
            &wake_on,
            max_actions,
            until,
        )
        .await
        .map_err(|error| error.to_string())?;
    let data = snapshot(ctx, store).await?;
    Ok(format!("[OK] Canvas mode on.\n{}", format_snapshot(&data)))
}

pub async fn handle(ctx: &RunContext, args: &Value) -> ToolOutcome {
    let store = match CanvasStore::open(ctx.store.harness_root()).await {
        Ok(store) => store,
        Err(error) => return ToolOutcome::err(format!("Error: canvas store unavailable: {error}")),
    };
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("status")
        .trim()
        .to_ascii_lowercase();
    let result = match action.as_str() {
        "available" => Ok(available(ctx).await),
        "enter" => enter(ctx, &store, args).await,
        "status" | "read" => snapshot(ctx, &store)
            .await
            .map(|data| format_snapshot(&data)),
        "exit" => store
            .close_watch_for_chat(&ctx.chat_id)
            .await
            .map(|count| format!("[OK] Canvas mode off ({count} watch closed)."))
            .map_err(|error| error.to_string()),
        "message" => message(ctx, &store, args).await,
        other => Err(format!(
            "unknown action '{other}' (available|enter|status|read|message|exit)"
        )),
    };
    match result {
        Ok(content) => ToolOutcome::ok(content),
        Err(error) => ToolOutcome::err(format!("Error: {error}")),
    }
}

async fn message(ctx: &RunContext, store: &CanvasStore, args: &Value) -> Result<String, String> {
    let watch = store
        .active_watch(&ctx.chat_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "canvas mode is not active".to_string())?;
    let used = watch["actionsUsed"].as_i64().unwrap_or(0);
    let max = watch["maxActions"].as_i64().unwrap_or(0).max(1);
    if used >= max {
        return Err("canvas action budget reached; exit or raise max_actions".to_string());
    }
    let target = args
        .get("target")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "target is required".to_string())?;
    let text = args
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "message is required".to_string())?;
    if target.starts_with("group:") {
        return Err("use group_update for group projects".to_string());
    }
    if !CanvasStore::watch_matches(&watch, target) {
        return Err("target is not selected in this canvas watch".to_string());
    }
    let chats = ctx.store.all_chats().await;
    let chat = chats
        .iter()
        .find(|chat| chat["id"] == target)
        .ok_or_else(|| format!("unknown chat {target}"))?;
    let workspace = chat["workspacePath"].as_str().unwrap_or("");
    store
        .enqueue_wake(
            workspace,
            target,
            text,
            json!({
                "fromCanvas": true,
                "watchId": watch["id"],
                "orchestratorChatId": ctx.chat_id,
            }),
            "auto",
        )
        .await
        .map_err(|error| error.to_string())?;
    let used = store
        .bump_watch(watch["id"].as_str().unwrap_or(""))
        .await
        .map_err(|error| error.to_string())?;
    if used >= max {
        let _ = store.close_watch(watch["id"].as_str().unwrap_or("")).await;
    }
    Ok(format!(
        "[OK] Message queued for {target} ({used}/{max} actions)."
    ))
}

/// Notify matching active watches that `source_chat_id` finished. Returns the
/// number of orchestrator wakes queued. Called by the server after each run
/// (and by the group runner), never from inside `canvas_mode` itself.
pub async fn notify_run_finished(
    store_ref: &ChatStore,
    source_chat_id: &str,
    report: &str,
    status: &str,
) -> usize {
    let store = match CanvasStore::open(store_ref.harness_root()).await {
        Ok(store) => store,
        Err(error) => {
            tracing::warn!("canvas notify unavailable: {error}");
            return 0;
        }
    };
    let watches = match store.active_watches().await {
        Ok(watches) => watches,
        Err(error) => {
            tracing::warn!("canvas watches unavailable: {error}");
            return 0;
        }
    };
    if watches.is_empty() {
        return 0;
    }
    let source = match store_ref.get_chat(source_chat_id).await {
        Ok(Some(chat)) => chat,
        Ok(None) => json!({"title": source_chat_id}),
        Err(_) => json!({"title": source_chat_id}),
    };
    let parent = source["parentChatId"].as_str().unwrap_or("").to_string();
    let status_wire = match status {
        "complete" | "completed" => "completed",
        "error" | "failed" => "failed",
        "aborted" => "failed",
        other => other,
    };
    let mut queued = 0;
    for watch in watches {
        if !CanvasStore::wake_on_includes(&watch, status_wire) {
            continue;
        }
        if watch["mode"].as_str() == Some("all")
            && parent.is_empty()
            && !source_chat_id.starts_with("gp-")
        {
            continue;
        }
        if !CanvasStore::watch_matches(&watch, source_chat_id) {
            continue;
        }
        let used = watch["actionsUsed"].as_i64().unwrap_or(0);
        let max = watch["maxActions"].as_i64().unwrap_or(0).max(1);
        if used >= max {
            let _ = store.close_watch(watch["id"].as_str().unwrap_or("")).await;
            continue;
        }
        let title = source["title"].as_str().unwrap_or(source_chat_id);
        let event_text = format!(
            "[CANVAS EVENT] {title} ({source_chat_id}) {status_wire}.\n{}\n\nReview this result and decide whether to message the chat, send follow-up work, or take no action.",
            report.chars().take(4000).collect::<String>()
        );
        let orchestrator = watch["orchestratorChatId"].as_str().unwrap_or("");
        if orchestrator.is_empty() {
            continue;
        }
        let workspace = store_ref
            .chat_workspace_path(orchestrator)
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        if store
            .enqueue_wake(
                &workspace,
                orchestrator,
                &event_text,
                json!({
                    "canvasEvent": true,
                    "watchId": watch["id"],
                    "sourceChatId": source_chat_id,
                    "status": status_wire,
                }),
                "orchestrator",
            )
            .await
            .is_err()
        {
            continue;
        }
        let _ = store
            .record_event(
                watch["id"].as_str().unwrap_or(""),
                source_chat_id,
                status_wire,
                json!({"title": title, "report": report.chars().take(4000).collect::<String>()}),
            )
            .await;
        let used = store
            .bump_watch(watch["id"].as_str().unwrap_or(""))
            .await
            .unwrap_or(used + 1);
        if used >= max {
            let _ = store.close_watch(watch["id"].as_str().unwrap_or("")).await;
        }
        queued += 1;
    }
    queued
}

#[cfg(test)]
mod tests {
    use super::*;
    use stark_providers::{Provider, ProviderConfig};
    use stark_store::ChatStore;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use tokio::sync::broadcast;
    use tokio_util::sync::CancellationToken;

    async fn harness() -> (tempfile::TempDir, RunContext) {
        let dir = tempfile::tempdir().unwrap();
        let store = ChatStore::open(dir.path()).await.unwrap();
        let provider = Provider::new(ProviderConfig {
            mode: "deepseek".to_string(),
            base_url: "http://127.0.0.1:1".to_string(),
            api_key: "test".to_string(),
            model: "mock".to_string(),
        })
        .unwrap();
        let (tx, _rx) = broadcast::channel(8);
        let ctx = RunContext {
            chat_id: "orch".to_string(),
            workspace_id: "_general".to_string(),
            workspace: dir.path().to_path_buf(),
            role: None,
            scope: "orchestrator".to_string(),
            depth: 0,
            model: "mock".to_string(),
            temperature: 0.0,
            images: Vec::new(),
            provider,
            store,
            questions: Arc::new(stark_tools::QuestionBroker::new()),
            tx,
            latest: Arc::new(Mutex::new(None)),
            steering: Arc::new(Mutex::new(VecDeque::new())),
            follow_up: Arc::new(Mutex::new(VecDeque::new())),
            checkpoint: None,
            cancel: CancellationToken::new(),
            on_job_done: None,
        };
        (dir, ctx)
    }

    #[tokio::test]
    async fn enter_status_exit_roundtrip() {
        let (_dir, ctx) = harness().await;
        let entered = handle(
            &ctx,
            &json!({"action": "enter", "mode": "all", "max_actions": 3}),
        )
        .await;
        assert!(!entered.is_error, "{}", entered.content);
        assert!(entered.content.contains("Canvas mode on"));
        let status = handle(&ctx, &json!({"action": "status"})).await;
        assert!(status.content.contains("actions"));
        let exited = handle(&ctx, &json!({"action": "exit"})).await;
        assert!(exited.content.contains("Canvas mode off"));
        let after = handle(&ctx, &json!({"action": "read"})).await;
        assert!(after.content.contains("Canvas mode is off"));
    }

    #[tokio::test]
    async fn enter_requires_targets_for_single_mode() {
        let (_dir, ctx) = harness().await;
        let result = handle(
            &ctx,
            &json!({"action": "enter", "mode": "single", "targets": []}),
        )
        .await;
        assert!(result.is_error);
        assert!(result.content.contains("select at least one"));
    }

    #[tokio::test]
    async fn available_lists_header() {
        let (_dir, ctx) = harness().await;
        let result = handle(&ctx, &json!({"action": "available"})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("[CANVAS TARGETS]"));
    }
}

#[cfg(test)]
mod notify_tests {
    use super::*;
    use crate::RunContext;
    use stark_providers::{Provider, ProviderConfig};
    use stark_store::{CanvasStore, ChatStore};
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use tokio::sync::broadcast;
    use tokio_util::sync::CancellationToken;

    async fn ctx_for(root: &std::path::Path) -> RunContext {
        let store = ChatStore::open(root).await.unwrap();
        let provider = Provider::new(ProviderConfig {
            mode: "deepseek".to_string(),
            base_url: "http://127.0.0.1:1".to_string(),
            api_key: "test".to_string(),
            model: "mock".to_string(),
        })
        .unwrap();
        let (tx, _rx) = broadcast::channel(8);
        RunContext {
            chat_id: "orch".to_string(),
            workspace_id: "_general".to_string(),
            workspace: root.to_path_buf(),
            role: None,
            scope: "orchestrator".to_string(),
            depth: 0,
            model: "mock".to_string(),
            temperature: 0.0,
            images: Vec::new(),
            provider,
            store,
            questions: Arc::new(stark_tools::QuestionBroker::new()),
            tx,
            latest: Arc::new(Mutex::new(None)),
            steering: Arc::new(Mutex::new(VecDeque::new())),
            follow_up: Arc::new(Mutex::new(VecDeque::new())),
            checkpoint: None,
            cancel: CancellationToken::new(),
            on_job_done: None,
        }
    }

    #[tokio::test]
    async fn completion_enqueues_wake_for_matching_watch() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_for(dir.path()).await;
        ctx.store
            .create_chat("_general", "", "Orchestrator", "mock")
            .await
            .unwrap();
        ctx.store
            .create_child_chat("_general", "", "Worker", "mock", "orch")
            .await
            .unwrap();
        let store = CanvasStore::open(dir.path()).await.unwrap();
        store
            .enter_watch(
                "orch",
                "",
                "single",
                &["chat-worker".to_string()],
                &["completed".to_string()],
                5,
                None,
            )
            .await
            .unwrap();
        // The generated child id is chat-<uuid>; look it up from the store.
        let chats = ctx.store.all_chats().await;
        let source = chats
            .iter()
            .find(|chat| chat["title"] == "Worker")
            .and_then(|chat| chat["id"].as_str())
            .unwrap()
            .to_string();
        store.close_watch_for_chat("orch").await.unwrap();
        store
            .enter_watch(
                "orch",
                "",
                "single",
                std::slice::from_ref(&source),
                &["completed".to_string()],
                5,
                None,
            )
            .await
            .unwrap();
        let queued = notify_run_finished(&ctx.store, &source, "finished", "complete").await;
        assert_eq!(queued, 1);
        let dues = store.due_wakes(10).await.unwrap();
        assert_eq!(dues.len(), 1);
        assert_eq!(dues[0]["chatId"], "orch");
        assert!(dues[0]["text"].as_str().unwrap().contains("[CANVAS EVENT]"));
    }

    #[tokio::test]
    async fn non_matching_status_does_not_wake() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_for(dir.path()).await;
        ctx.store
            .create_child_chat("_general", "", "Worker", "mock", "orch")
            .await
            .unwrap();
        let store = CanvasStore::open(dir.path()).await.unwrap();
        store
            .enter_watch(
                "orch",
                "",
                "single",
                &["chat-never".to_string()],
                &["failed".to_string()],
                5,
                None,
            )
            .await
            .unwrap();
        let source = ctx
            .store
            .all_chats()
            .await
            .iter()
            .find(|chat| chat["title"] == "Worker")
            .and_then(|chat| chat["id"].as_str())
            .unwrap()
            .to_string();
        let queued = notify_run_finished(&ctx.store, &source, "finished", "complete").await;
        assert_eq!(queued, 0);
        assert!(store.due_wakes(10).await.unwrap().is_empty());
    }
}
