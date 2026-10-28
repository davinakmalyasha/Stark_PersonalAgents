//! Durable agent tool: spawn/list/status/send/report for the three-level tree.
//!
//! Orchestrator -> workspace agents -> subagents. The tool only writes durable
//! records and mailbox messages; the server supervisor leases and runs them.

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use stark_store::{AgentRecord, AgentStore};
use stark_tools::ToolOutcome;

use crate::{RunContext, agent_for_scope};

const MAX_DEPTH: i64 = 2;

fn truncate(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

fn sanitize(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        .take(24)
        .collect();
    if cleaned.is_empty() {
        "agent".to_string()
    } else {
        cleaned
    }
}

pub(crate) async fn current_agent(
    ctx: &RunContext,
    store: &AgentStore,
) -> Result<AgentRecord, String> {
    if let Some(agent) = store
        .by_chat(&ctx.chat_id)
        .await
        .map_err(|error| error.to_string())?
    {
        return Ok(agent);
    }
    let kind = if ctx.depth == 0 {
        "orchestrator"
    } else {
        "workspace"
    };
    let path = if kind == "orchestrator" {
        format!("orchestrator:{}", ctx.chat_id)
    } else {
        format!("workspace:{}:{}", ctx.workspace_id, ctx.scope)
    };
    store
        .create_agent(
            kind,
            &path,
            None,
            &ctx.chat_id,
            &ctx.workspace_id,
            &ctx.scope,
            ctx.role.as_deref().unwrap_or("Agent"),
            "",
            &ctx.model,
            ctx.depth as i64,
        )
        .await
        .map_err(|error| error.to_string())
}

async fn spawn(ctx: &RunContext, store: &AgentStore, args: &Value) -> Result<String, String> {
    let current = current_agent(ctx, store).await?;
    let kind = args
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("subagent");
    let task = args
        .get("task")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|task| !task.is_empty())
        .ok_or_else(|| "spawn requires task".to_string())?;
    let requested = args
        .get("agent")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let spec = agent_for_scope(requested)
        .or_else(|| agent_for_scope("general"))
        .ok_or_else(|| "unknown agent".to_string())?;
    let title = args
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| task.chars().take(60).collect());
    let wait_seconds = args
        .get("wait_seconds")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(300);
    let workspace_id = args
        .get("workspace_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&ctx.workspace_id)
        .to_string();
    let workspace_path = args
        .get("workspace_path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| ctx.workspace.clone());

    let (agent, created) = match kind {
        "workspace" => {
            if current.kind != "orchestrator" {
                return Err("only the orchestrator can spawn workspace agents".to_string());
            }
            if let Some(existing) = store
                .workspace_agent(&workspace_id, spec.key)
                .await
                .map_err(|error| error.to_string())?
            {
                (existing, false)
            } else {
                let chat = ctx
                    .store
                    .create_child_chat(
                        &workspace_id,
                        workspace_path.to_string_lossy().as_ref(),
                        &title,
                        &ctx.model,
                        &current.chat_id,
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                let chat_id = chat["id"].as_str().unwrap_or("").to_string();
                let path = format!("workspace:{}:{}", workspace_id, spec.key);
                let agent = store
                    .create_agent(
                        "workspace",
                        &path,
                        Some(&current.id),
                        &chat_id,
                        &workspace_id,
                        spec.key,
                        spec.display,
                        &title,
                        &ctx.model,
                        current.depth + 1,
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                (agent, true)
            }
        }
        "subagent" => {
            if current.kind != "workspace" {
                return Err("only a workspace agent can spawn subagents".to_string());
            }
            if current.depth >= MAX_DEPTH {
                return Err("subagent depth limit reached".to_string());
            }
            let chat = ctx
                .store
                .create_child_chat(
                    &ctx.workspace_id,
                    ctx.workspace.to_string_lossy().as_ref(),
                    &title,
                    &ctx.model,
                    &current.chat_id,
                )
                .await
                .map_err(|error| error.to_string())?;
            let chat_id = chat["id"].as_str().unwrap_or("").to_string();
            let short = &uuid::Uuid::new_v4().simple().to_string()[..8];
            let path = format!("{}/sub:{}", current.path, sanitize(requested));
            let path = format!("{path}-{short}");
            let agent = store
                .create_agent(
                    "subagent",
                    &path,
                    Some(&current.id),
                    &chat_id,
                    &ctx.workspace_id,
                    spec.key,
                    spec.display,
                    &title,
                    &ctx.model,
                    current.depth + 1,
                )
                .await
                .map_err(|error| error.to_string())?;
            (agent, true)
        }
        other => return Err(format!("unknown agent kind '{other}'")),
    };

    store
        .enqueue(
            Some(&current.id),
            &agent.id,
            "spawn",
            task,
            "trigger",
            json!({"title": title, "created": created}),
        )
        .await
        .map_err(|error| error.to_string())?;
    store
        .wake(&agent.id)
        .await
        .map_err(|error| error.to_string())?;
    let mut text = format!(
        "[AGENT SPAWNED]\nid={}\npath={}\nkind={}\nstatus={}\nchat={}",
        agent.id, agent.path, agent.kind, agent.status, agent.chat_id
    );
    if created {
        text.push_str("\nThe server supervisor will start it.");
    } else {
        text.push_str("\nReused the workspace agent and queued the task.");
    }
    if wait_seconds > 0 {
        let detail = wait_for(store, &agent.id, wait_seconds).await?;
        text.push('\n');
        text.push_str(&detail);
    }
    Ok(text)
}

pub(crate) async fn wait_for(store: &AgentStore, id: &str, seconds: u64) -> Result<String, String> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        let Some(agent) = store.get(id).await.map_err(|error| error.to_string())? else {
            return Err(format!("unknown agent {id}"));
        };
        if matches!(
            agent.status.as_str(),
            "completed" | "failed" | "cancelled" | "interrupted"
        ) || Instant::now() >= deadline
        {
            return Ok(format_agent(&agent));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn format_agent(agent: &AgentRecord) -> String {
    let mut text = format!(
        "id={}\npath={}\nkind={}\nstatus={}\nchat={}\nrole={}",
        agent.id, agent.path, agent.kind, agent.status, agent.chat_id, agent.role
    );
    if !agent.result.is_empty() {
        text.push_str("\nresult:\n");
        text.push_str(&truncate(&agent.result, 12_000));
    }
    text
}

async fn list(ctx: &RunContext, store: &AgentStore) -> Result<String, String> {
    let current = current_agent(ctx, store).await?;
    let agents = store
        .list(Some(&current.id))
        .await
        .map_err(|error| error.to_string())?;
    if agents.is_empty() {
        return Ok("No child agents.".to_string());
    }
    let lines = agents
        .iter()
        .map(|agent| {
            format!(
                "- {} · {} · {} · {} · chat={}",
                agent.id, agent.path, agent.kind, agent.status, agent.chat_id
            )
        })
        .collect::<Vec<_>>();
    Ok(format!("[AGENTS]\n{}", lines.join("\n")))
}

async fn send(ctx: &RunContext, store: &AgentStore, args: &Value) -> Result<String, String> {
    let current = current_agent(ctx, store).await?;
    let id = args
        .get("agent_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "send requires agent_id".to_string())?;
    let message = args
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .ok_or_else(|| "send requires message".to_string())?;
    let target = store
        .get(id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("unknown agent {id}"))?;
    if target.parent_id.as_deref() != Some(current.id.as_str())
        && current.parent_id.as_deref() != Some(target.id.as_str())
    {
        return Err("agent is not a child or parent of this agent".to_string());
    }
    store
        .enqueue(
            Some(&current.id),
            &target.id,
            "message",
            message,
            "trigger",
            json!({"from": current.path}),
        )
        .await
        .map_err(|error| error.to_string())?;
    if target.status != "running" {
        store
            .wake(&target.id)
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(format!(
        "[OK] message queued for {} ({})",
        target.id, target.status
    ))
}

async fn cancel(ctx: &RunContext, store: &AgentStore, args: &Value) -> Result<String, String> {
    let current = current_agent(ctx, store).await?;
    let id = args
        .get("agent_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "cancel requires agent_id".to_string())?;
    let target = store
        .get(id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("unknown agent {id}"))?;
    if target.parent_id.as_deref() != Some(current.id.as_str()) && target.id != current.id {
        return Err("agent is not a child of this agent".to_string());
    }
    let mut stack = vec![target.clone()];
    let mut cancelled = 0usize;
    while let Some(agent) = stack.pop() {
        crate::cancel_agent_run(&agent.id);
        let _ = store.set_status(&agent.id, "cancelled", false).await;
        cancelled += 1;
        if let Ok(children) = store.list(Some(&agent.id)).await {
            stack.extend(children);
        }
    }
    Ok(format!(
        "[OK] cancellation requested for {cancelled} agent(s)"
    ))
}

async fn report(ctx: &RunContext, store: &AgentStore, args: &Value) -> Result<String, String> {
    let current = current_agent(ctx, store).await?;
    let Some(parent_id) = current.parent_id.as_deref() else {
        return Err("orchestrator has no parent".to_string());
    };
    let message = args
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .ok_or_else(|| "report requires message".to_string())?;
    let quiet = args.get("quiet").and_then(Value::as_bool).unwrap_or(false);
    store
        .enqueue(
            Some(&current.id),
            parent_id,
            "report",
            message,
            if quiet { "queue" } else { "trigger" },
            json!({"from": current.path}),
        )
        .await
        .map_err(|error| error.to_string())?;
    if !quiet {
        store
            .wake(parent_id)
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(format!("[OK] reported to {parent_id}"))
}

pub async fn handle(ctx: &RunContext, args: &Value) -> ToolOutcome {
    let store = match AgentStore::open(ctx.store.harness_root()).await {
        Ok(store) => store,
        Err(error) => return ToolOutcome::err(format!("Error: agent store unavailable: {error}")),
    };
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("list")
        .trim()
        .to_lowercase();
    let result = match action.as_str() {
        "spawn" => spawn(ctx, &store, args).await,
        "list" => list(ctx, &store).await,
        "status" => {
            let id = args
                .get("agent_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if id.is_empty() {
                Err("status requires agent_id".to_string())
            } else {
                let wait = args
                    .get("wait_seconds")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .min(300);
                if wait == 0 {
                    match store.get(id).await {
                        Ok(Some(agent)) => Ok(format_agent(&agent)),
                        Ok(None) => Err(format!("unknown agent {id}")),
                        Err(error) => Err(error.to_string()),
                    }
                } else {
                    wait_for(&store, id, wait).await
                }
            }
        }
        "send" => send(ctx, &store, args).await,
        "report" => report(ctx, &store, args).await,
        "cancel" => cancel(ctx, &store, args).await,
        other => Err(format!(
            "unknown action '{other}' (spawn|list|status|send|report|cancel)"
        )),
    };
    match result {
        Ok(content) => ToolOutcome::ok(content),
        Err(error) => ToolOutcome::err(format!("Error: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use stark_providers::{Provider, ProviderConfig};
    use stark_store::AgentStore;
    use tokio::sync::broadcast;
    use tokio_util::sync::CancellationToken;

    async fn context(dir: &std::path::Path, chat_id: &str, depth: u32, scope: &str) -> RunContext {
        let store = crate::ChatStore::open(dir).await.unwrap();
        let provider = Provider::new(ProviderConfig {
            mode: "deepseek".to_string(),
            base_url: "http://127.0.0.1:1".to_string(),
            api_key: "test".to_string(),
            model: "mock".to_string(),
        })
        .unwrap();
        let (tx, _rx) = broadcast::channel(16);
        RunContext {
            chat_id: chat_id.to_string(),
            workspace_id: "_general".to_string(),
            workspace: dir.to_path_buf(),
            role: Some("Developer".to_string()),
            scope: scope.to_string(),
            depth,
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
    async fn spawns_workspace_then_subagent_and_reports() {
        let dir = tempfile::tempdir().unwrap();
        let orch = context(dir.path(), "orch", 0, "orchestrator").await;
        let result = handle(
            &orch,
            &json!({"action":"spawn","kind":"workspace","agent":"Developer","task":"build"}),
        )
        .await;
        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("AGENT SPAWNED"));

        let store = AgentStore::open(dir.path()).await.unwrap();
        let root = store.by_chat("orch").await.unwrap().unwrap();
        let workspace = store.list(Some(&root.id)).await.unwrap();
        assert_eq!(workspace.len(), 1);
        assert_eq!(workspace[0].kind, "workspace");
        let chat_id = workspace[0].chat_id.clone();

        let dev = context(dir.path(), &chat_id, 1, "developer").await;
        let result = handle(
            &dev,
            &json!({"action":"spawn","kind":"subagent","agent":"Explore","task":"inspect"}),
        )
        .await;
        assert!(!result.is_error, "{}", result.content);
        let children = store.list(Some(&workspace[0].id)).await.unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].kind, "subagent");

        let token = CancellationToken::new();
        crate::register_agent_cancel(&children[0].id, token.clone());
        let cancelled = handle(&dev, &json!({"action":"cancel","agent_id":children[0].id})).await;
        assert!(!cancelled.is_error, "{}", cancelled.content);
        assert!(token.is_cancelled());
        assert_eq!(
            store.get(&children[0].id).await.unwrap().unwrap().status,
            "cancelled"
        );

        let sub_chat = children[0].chat_id.clone();
        let sub = context(dir.path(), &sub_chat, 2, "explore").await;
        let reported = handle(
            &sub,
            &json!({"action":"report","message":"subagent update"}),
        )
        .await;
        assert!(!reported.is_error, "{}", reported.content);
        let parent_messages = store.messages(&workspace[0].id).await.unwrap();
        assert!(
            parent_messages
                .iter()
                .any(|message| message["kind"] == "report")
        );
    }

    #[tokio::test]
    async fn workspace_agents_cannot_spawn_workspace_agents() {
        let dir = tempfile::tempdir().unwrap();
        let dev = context(dir.path(), "chat-dev", 1, "developer").await;
        let result = handle(
            &dev,
            &json!({"action":"spawn","kind":"workspace","agent":"General","task":"x"}),
        )
        .await;
        assert!(result.is_error);
        assert!(result.content.contains("only the orchestrator"));
    }
}
