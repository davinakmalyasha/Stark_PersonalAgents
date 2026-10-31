//! Codex multi-agent v2 tools (Pi-simplified): spawn_agent, message_agent,
//! wait_agent, list_agents, interrupt_agent, close_agent.
//!
//! `message_agent` merges Codex send_message (queue only) and followup_task
//! (trigger a turn): `wake=true` triggers an idle agent, `wake=false`
//! (default) only queues. Stark's durable agent store and mailbox provide the
//! execution substrate. Developer subagents share the parent workspace,
//! matching Codex rather than Stark's old worktree isolation.

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use stark_store::{AgentRecord, AgentStore};
use stark_tools::ToolOutcome;

use crate::{RunContext, agent_for_scope, agents_tools};

const MAX_CHILDREN: usize = 6;
const MAX_DEPTH: i64 = 1;
const DEFAULT_WAIT_MS: u64 = 120_000;
const MAX_WAIT_MS: u64 = 3_600_000;

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

async fn store(ctx: &RunContext) -> Result<AgentStore, String> {
    AgentStore::open(ctx.store.harness_root())
        .await
        .map_err(|error| error.to_string())
}

async fn current(ctx: &RunContext, store: &AgentStore) -> Result<AgentRecord, String> {
    agents_tools::current_agent(ctx, store).await
}

fn root_id(current: &AgentRecord) -> String {
    if let Some(rest) = current.path.strip_prefix("/root/") {
        return rest
            .split('/')
            .next()
            .filter(|value| !value.is_empty())
            .unwrap_or("root")
            .to_string();
    }
    let cleaned: String = current
        .chat_id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(10)
        .collect();
    if cleaned.is_empty() {
        "root".to_string()
    } else {
        cleaned
    }
}

fn root_path(current: &AgentRecord) -> String {
    format!("/root/{}", root_id(current))
}

fn canonical_path(current: &AgentRecord, task_name: &str) -> String {
    format!("{}/{}", root_path(current), task_name)
}

fn same_tree(current: &AgentRecord, agent: &AgentRecord) -> bool {
    agent.path.starts_with(&root_path(current))
}

fn task_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
}

async fn children(store: &AgentStore, parent: &AgentRecord) -> Result<Vec<AgentRecord>, String> {
    store
        .list(Some(&parent.id))
        .await
        .map_err(|error| error.to_string())
}

async fn resolve_target(
    store: &AgentStore,
    current: &AgentRecord,
    raw: &str,
) -> Result<AgentRecord, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("target is required".to_string());
    }
    if let Some(agent) = store.get(raw).await.map_err(|error| error.to_string())? {
        if same_tree(current, &agent) {
            return Ok(agent);
        }
        return Err("agent is not in this task tree".to_string());
    }
    let all = store.list(None).await.map_err(|error| error.to_string())?;
    let canonical = if raw.starts_with('/') {
        raw.to_string()
    } else {
        format!("{}/{}", root_path(current), raw)
    };
    let mut found: Option<AgentRecord> = None;
    for agent in all {
        if agent.id == current.id {
            continue;
        }
        if (agent.path == canonical
            || agent.path == raw
            || agent.nickname == raw
            || agent.path.ends_with(&format!("/{raw}")))
            && same_tree(current, &agent)
        {
            found = Some(agent);
            break;
        }
    }
    found.ok_or_else(|| format!("unknown agent '{raw}'"))
}

async fn is_descendant_of(
    store: &AgentStore,
    agent: &AgentRecord,
    ancestor: &AgentRecord,
) -> Result<bool, String> {
    let mut cursor = agent.parent_id.clone();
    while let Some(parent_id) = cursor {
        if parent_id == ancestor.id {
            return Ok(true);
        }
        let Some(parent) = store
            .get(&parent_id)
            .await
            .map_err(|error| error.to_string())?
        else {
            break;
        };
        cursor = parent.parent_id;
    }
    Ok(false)
}

async fn spawn_agent(ctx: &RunContext, args: &Value) -> Result<String, String> {
    let store = store(ctx).await?;
    let current = current(ctx, &store).await?;
    if current.depth >= MAX_DEPTH {
        return Err("Agent depth limit reached. Solve the task yourself.".to_string());
    }
    let task_name = args
        .get("task_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .ok_or_else(|| "spawn_agent requires task_name".to_string())?;
    if !task_name_ok(task_name) {
        return Err(
            "task_name must use lowercase letters, digits, and underscores (max 64)".to_string(),
        );
    }
    let message = args
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .ok_or_else(|| "spawn_agent requires message".to_string())?;
    let open = children(&store, &current)
        .await?
        .into_iter()
        .filter(|agent| agent.status != "closed")
        .count();
    if open >= MAX_CHILDREN {
        return Err(format!(
            "agent limit reached: {open} open children (max {MAX_CHILDREN})"
        ));
    }
    if args
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Err("reasoning_effort is not supported by this provider yet".to_string());
    }
    if let Some(agent_type) = args.get("agent_type").and_then(Value::as_str) {
        let normalized = agent_type.trim().to_ascii_lowercase();
        if !normalized.is_empty() && normalized != "developer" && normalized != "dev" {
            return Err(format!(
                "subagent type '{agent_type}' is not configured yet; use Developer"
            ));
        }
    }
    let path = canonical_path(&current, task_name);
    if store
        .list(None)
        .await
        .map_err(|error| error.to_string())?
        .iter()
        .any(|agent| agent.path == path)
    {
        return Err(format!(
            "task_name '{task_name}' already exists under this agent"
        ));
    }
    let model = args
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .unwrap_or(&ctx.model)
        .to_string();
    let fork_turns = args
        .get("fork_turns")
        .and_then(Value::as_str)
        .unwrap_or("all");
    if !matches!(fork_turns, "all" | "none") {
        return Err("fork_turns must be `all` or `none`".to_string());
    }
    let workspace_path = ctx.workspace.to_string_lossy().to_string();
    let chat = ctx
        .store
        .create_child_chat(
            &ctx.workspace_id,
            &workspace_path,
            task_name,
            &model,
            &ctx.chat_id,
        )
        .await
        .map_err(|error| error.to_string())?;
    let chat_id = chat["id"].as_str().unwrap_or("").to_string();
    if fork_turns == "all" {
        let history = ctx
            .store
            .history(&ctx.chat_id, 40)
            .await
            .map_err(|error| error.to_string())?;
        for message in history {
            if message.role != "user" && message.role != "assistant" {
                continue;
            }
            let _ = ctx
                .store
                .append_message(&chat_id, &message.role, &message.content, false)
                .await;
        }
    }
    let spec = agent_for_scope("developer").ok_or_else(|| "developer agent missing".to_string())?;
    let agent = store
        .create_agent(
            "subagent",
            &path,
            Some(&current.id),
            &chat_id,
            &ctx.workspace_id,
            "developer",
            spec.display,
            task_name,
            &model,
            current.depth + 1,
        )
        .await
        .map_err(|error| error.to_string())?;
    let _ = store.set_access(&agent.id, "write").await;
    store
        .enqueue(
            Some(&current.id),
            &agent.id,
            "spawn",
            &format!("[TASK] {path}\n\n{message}"),
            "trigger",
            json!({
                "task_name": task_name,
                "task_path": path,
                "fork_turns": fork_turns,
                "created_at": now(),
            }),
        )
        .await
        .map_err(|error| error.to_string())?;
    store
        .wake(&agent.id)
        .await
        .map_err(|error| error.to_string())?;
    Ok(json!({"task_name": task_name, "nickname": task_name}).to_string())
}

async fn message_agent(ctx: &RunContext, args: &Value) -> Result<String, String> {
    let store = store(ctx).await?;
    let current = current(ctx, &store).await?;
    let target = args
        .get("target")
        .and_then(Value::as_str)
        .ok_or_else(|| "message_agent requires target".to_string())?;
    let message = args
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .ok_or_else(|| "message_agent requires message".to_string())?;
    let wake = args.get("wake").and_then(Value::as_bool).unwrap_or(false);
    let target = resolve_target(&store, &current, target).await?;
    if matches!(target.status.as_str(), "closed") {
        return Err(format!("agent {} is closed", target.path));
    }
    let id = store
        .enqueue(
            Some(&current.id),
            &target.id,
            if wake { "followup" } else { "message" },
            message,
            if wake { "trigger" } else { "queue" },
            json!({"task_path": target.path}),
        )
        .await
        .map_err(|error| error.to_string())?;
    if wake {
        store
            .wake(&target.id)
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(json!({"submission_id": id}).to_string())
}

async fn wait_agent(ctx: &RunContext, args: &Value) -> Result<String, String> {
    let store = store(ctx).await?;
    let current = current(ctx, &store).await?;
    let timeout_ms = args
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_WAIT_MS)
        .clamp(10_000, MAX_WAIT_MS);
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let all = store.list(None).await.map_err(|error| error.to_string())?;
        let agents: Vec<AgentRecord> = all
            .into_iter()
            .filter(|agent| agent.id != current.id && same_tree(&current, agent))
            .collect();
        let completed: Vec<&AgentRecord> = agents
            .iter()
            .filter(|agent| {
                matches!(
                    agent.status.as_str(),
                    "completed" | "failed" | "cancelled" | "interrupted" | "closed"
                )
            })
            .collect();
        let mailbox = store
            .messages(&current.id)
            .await
            .map_err(|error| error.to_string())?;
        if !completed.is_empty() || !mailbox.is_empty() {
            let summary = if !mailbox.is_empty() {
                format!(
                    "{} mailbox update(s); {} child agent(s) in a final status",
                    mailbox.len(),
                    completed.len()
                )
            } else {
                completed
                    .iter()
                    .map(|agent| format!("{}: {}", agent.path, agent.status))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            return Ok(json!({"message": summary, "timed_out": false}).to_string());
        }
        if Instant::now() >= deadline {
            return Ok(json!({
                "message": format!("No mailbox update within {timeout_ms}ms"),
                "timed_out": true
            })
            .to_string());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn list_agents(ctx: &RunContext, args: &Value) -> Result<String, String> {
    let store = store(ctx).await?;
    let current = current(ctx, &store).await?;
    let prefix = args
        .get("path_prefix")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|prefix| !prefix.is_empty());
    let all = store.list(None).await.map_err(|error| error.to_string())?;
    let mut rows = Vec::new();
    for agent in all {
        if agent.id == current.id || agent.status == "closed" {
            continue;
        }
        if !same_tree(&current, &agent) {
            continue;
        }
        if let Some(prefix) = prefix
            && !agent.path.starts_with(prefix)
        {
            continue;
        }
        rows.push(json!({
            "agent_name": if agent.path.is_empty() { agent.id.clone() } else { agent.path.clone() },
            "agent_status": agent.status,
        }));
    }
    Ok(json!({"agents": rows}).to_string())
}

async fn interrupt_agent(ctx: &RunContext, args: &Value) -> Result<String, String> {
    let store = store(ctx).await?;
    let current = current(ctx, &store).await?;
    let target = args
        .get("target")
        .and_then(Value::as_str)
        .ok_or_else(|| "interrupt_agent requires target".to_string())?;
    let target = resolve_target(&store, &current, target).await?;
    if !is_descendant_of(&store, &target, &current).await? {
        return Err("can only interrupt a child agent".to_string());
    }
    let previous = target.status.clone();
    crate::cancel_agent_run(&target.id);
    let _ = store.set_status(&target.id, "interrupted", false).await;
    Ok(json!({"previous_status": previous}).to_string())
}

async fn close_tree(store: &AgentStore, agent: &AgentRecord) -> Result<(), String> {
    for child in store
        .list(Some(&agent.id))
        .await
        .map_err(|error| error.to_string())?
    {
        Box::pin(close_tree(store, &child)).await?;
    }
    crate::cancel_agent_run(&agent.id);
    store
        .set_status(&agent.id, "closed", false)
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

async fn close_agent(ctx: &RunContext, args: &Value) -> Result<String, String> {
    let store = store(ctx).await?;
    let current = current(ctx, &store).await?;
    let target = args
        .get("target")
        .and_then(Value::as_str)
        .ok_or_else(|| "close_agent requires target".to_string())?;
    let target = resolve_target(&store, &current, target).await?;
    if !is_descendant_of(&store, &target, &current).await? {
        return Err("can only close a child agent".to_string());
    }
    let previous = target.status.clone();
    close_tree(&store, &target).await?;
    Ok(json!({"previous_status": previous}).to_string())
}

pub async fn handle(ctx: &RunContext, name: &str, args: &Value) -> ToolOutcome {
    let result = match name {
        "spawn_agent" => spawn_agent(ctx, args).await,
        "message_agent" => message_agent(ctx, args).await,
        "wait_agent" => wait_agent(ctx, args).await,
        "list_agents" => list_agents(ctx, args).await,
        "interrupt_agent" => interrupt_agent(ctx, args).await,
        "close_agent" => close_agent(ctx, args).await,
        other => Err(format!("unknown multi-agent tool '{other}'")),
    };
    match result {
        Ok(content) => ToolOutcome::ok(content),
        Err(error) => ToolOutcome::err(format!("Error: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use super::*;
    use stark_providers::{Provider, ProviderConfig};
    use tokio::sync::broadcast;
    use tokio_util::sync::CancellationToken;

    async fn context(dir: &std::path::Path) -> RunContext {
        let store = stark_store::ChatStore::open(dir).await.unwrap();
        let provider = Provider::new(ProviderConfig {
            mode: "deepseek".to_string(),
            base_url: "http://127.0.0.1:1".to_string(),
            api_key: "test".to_string(),
            model: "mock".to_string(),
        })
        .unwrap();
        let (tx, _) = broadcast::channel(16);
        RunContext {
            chat_id: "parent-chat".to_string(),
            workspace_id: "_general".to_string(),
            workspace: dir.to_path_buf(),
            role: Some("Developer".to_string()),
            scope: "developer".to_string(),
            depth: 0,
            file_access: crate::FileAccess::Write,
            model: "mock".to_string(),
            temperature: 0.2,
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
    async fn spawns_lists_messages_and_closes_a_task_path_child() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path()).await;
        let spawned = handle(
            &ctx,
            "spawn_agent",
            &json!({"task_name": "task_1", "message": "do the thing", "fork_turns": "none"}),
        )
        .await;
        assert!(!spawned.is_error, "{}", spawned.content);

        let store = AgentStore::open(dir.path()).await.unwrap();
        let agents = store.list(None).await.unwrap();
        let child = agents
            .iter()
            .find(|agent| agent.path.ends_with("/task_1"))
            .expect("child exists");
        assert_eq!(child.access, "write");
        assert_eq!(child.status, "queued");
        assert_eq!(child.depth, 1);

        let listed = handle(&ctx, "list_agents", &json!({})).await;
        assert!(!listed.is_error, "{}", listed.content);
        assert!(listed.content.contains("task_1"), "{}", listed.content);

        let message = handle(
            &ctx,
            "message_agent",
            &json!({"target": "task_1", "message": "status?"}),
        )
        .await;
        assert!(!message.is_error, "{}", message.content);

        let followup = handle(
            &ctx,
            "message_agent",
            &json!({"target": "task_1", "message": "continue", "wake": true}),
        )
        .await;
        assert!(!followup.is_error, "{}", followup.content);

        let closed = handle(&ctx, "close_agent", &json!({"target": "task_1"})).await;
        assert!(!closed.is_error, "{}", closed.content);
        let child = store.get(&child.id).await.unwrap().unwrap();
        assert_eq!(child.status, "closed");
    }

    #[tokio::test]
    async fn depth_limit_blocks_nested_spawn() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = context(dir.path()).await;
        ctx.depth = 1;
        let out = handle(
            &ctx,
            "spawn_agent",
            &json!({"task_name": "nested", "message": "no"}),
        )
        .await;
        assert!(out.is_error);
        assert!(out.content.contains("depth limit"), "{}", out.content);
    }
}
