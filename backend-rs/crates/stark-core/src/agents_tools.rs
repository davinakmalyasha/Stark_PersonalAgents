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
    let (kind, path) = if ctx.scope == "orchestrator" {
        ("orchestrator", format!("orchestrator:{}", ctx.chat_id))
    } else if ctx.depth == 0 {
        ("workspace", format!("root:{}", ctx.chat_id))
    } else if ctx.depth == 1 {
        (
            "workspace",
            format!("workspace:{}:{}", ctx.workspace_id, ctx.scope),
        )
    } else {
        (
            "subagent",
            format!(
                "workspace:{}:{}:{}",
                ctx.workspace_id, ctx.scope, ctx.chat_id
            ),
        )
    };
    let access = crate::default_access(&ctx.scope, ctx.depth, false);
    let agent = store
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
        .map_err(|error| error.to_string())?;
    let _ = store.set_access(&agent.id, access.as_str()).await;
    Ok(store
        .get(&agent.id)
        .await
        .map_err(|error| error.to_string())?
        .unwrap_or(agent))
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
                // Reuse the durable workspace agent. If its previous worktree
                // is terminal or missing, isolate a fresh one.
                let access = requested_access(args, spec.key, current.depth + 1);
                if access == crate::FileAccess::Worktree
                    && (existing.worktree_state == "merged"
                        || existing.worktree_state == "discarded"
                        || existing.worktree_state == "missing"
                        || existing.worktree_state.is_empty()
                        || !std::path::Path::new(&existing.worktree_path).exists())
                {
                    let _ = attach_worktree(ctx, store, &existing, &workspace_path).await;
                }
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
                let access = requested_access(args, spec.key, current.depth + 1);
                if access == crate::FileAccess::Worktree {
                    let _ = attach_worktree(ctx, store, &agent, &workspace_path).await;
                } else {
                    let _ = store.set_access(&agent.id, access.as_str()).await;
                }
                let agent = store
                    .get(&agent.id)
                    .await
                    .map_err(|error| error.to_string())?
                    .unwrap_or(agent);
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
            // A subagent inherits the parent workspace (which may itself be a
            // worktree). Nested isolation branches from the parent worktree.
            let parent_workspace = if !current.worktree_path.trim().is_empty()
                && std::path::Path::new(&current.worktree_path).exists()
            {
                let subdir = current.workspace_subdir.trim().trim_matches('/');
                if subdir.is_empty() {
                    std::path::PathBuf::from(&current.worktree_path)
                } else {
                    std::path::PathBuf::from(&current.worktree_path).join(subdir)
                }
            } else {
                ctx.workspace.clone()
            };
            let chat = ctx
                .store
                .create_child_chat(
                    &ctx.workspace_id,
                    parent_workspace.to_string_lossy().as_ref(),
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
            let access = requested_access(args, spec.key, current.depth + 1);
            if access == crate::FileAccess::Worktree {
                let _ = attach_worktree(ctx, store, &agent, &parent_workspace).await;
            } else {
                let _ = store.set_access(&agent.id, access.as_str()).await;
            }
            let agent = store
                .get(&agent.id)
                .await
                .map_err(|error| error.to_string())?
                .unwrap_or(agent);
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
    let fresh = store
        .get(&agent.id)
        .await
        .map_err(|error| error.to_string())?
        .unwrap_or(agent);
    let mut text = format!(
        "[AGENT SPAWNED]\nid={}\npath={}\nkind={}\nstatus={}\nchat={}\naccess={}",
        fresh.id,
        fresh.path,
        fresh.kind,
        fresh.status,
        fresh.chat_id,
        if fresh.access.is_empty() {
            "write"
        } else {
            &fresh.access
        },
    );
    if !fresh.worktree_branch.is_empty() {
        text.push_str(&format!(
            "\nworktree branch={} base={}",
            fresh.worktree_branch, fresh.worktree_base
        ));
    }
    if created {
        text.push_str("\nThe server supervisor will start it.");
    } else {
        text.push_str("\nReused the workspace agent and queued the task.");
    }
    if worktree_policy() == "required"
        && fresh.access == "worktree"
        && fresh.worktree_path.is_empty()
    {
        return Err("worktree isolation required but unavailable for this workspace".to_string());
    }
    if wait_seconds > 0 {
        let detail = wait_for(store, &fresh.id, wait_seconds).await?;
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
        "id={}\npath={}\nkind={}\nstatus={}\nchat={}\nrole={}\naccess={}",
        agent.id,
        agent.path,
        agent.kind,
        agent.status,
        agent.chat_id,
        agent.role,
        if agent.access.is_empty() {
            "write"
        } else {
            &agent.access
        }
    );
    if !agent.worktree_branch.is_empty() {
        text.push_str(&format!(
            "\nworktree branch={} base={} state={} path={}",
            agent.worktree_branch,
            agent.worktree_base,
            if agent.worktree_state.is_empty() {
                "active"
            } else {
                &agent.worktree_state
            },
            agent.worktree_path
        ));
        if let Some(stat) = agent.worktree_diff.get("stat").and_then(Value::as_str)
            && !stat.trim().is_empty()
        {
            text.push_str("\ndiff:\n");
            text.push_str(&truncate(stat, 4_000));
        }
    }
    if !agent.result.is_empty() {
        text.push_str("\nresult:\n");
        text.push_str(&truncate(&agent.result, 12_000));
    }
    text
}

fn requested_access(args: &Value, scope: &str, depth: i64) -> crate::FileAccess {
    let mut access = crate::default_access(scope, depth.max(0) as u32, true);
    if args
        .get("write_access")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && access.is_read_only()
    {
        access = crate::FileAccess::Worktree;
    }
    if args
        .get("isolate")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && !access.is_read_only()
    {
        access = crate::FileAccess::Worktree;
    }
    access
}

fn worktree_policy() -> String {
    crate::agents::worktree_mode()
}

/// Create an isolated worktree for `agent_id` running in `workspace`.
/// Returns the worktree info and any fallback note. On non-git workspaces
/// the caller drops back to direct writes unless STARK_WORKTREES=required.
fn isolate_agent(
    harness_root: &std::path::Path,
    workspace: &std::path::Path,
    agent_id: &str,
) -> Result<(crate::worktree::WorktreeInfo, Option<String>), String> {
    match crate::worktree::create(harness_root, workspace, agent_id) {
        Ok(info) => Ok((info, None)),
        Err(error) => {
            if worktree_policy() == "required" {
                return Err(format!(
                    "worktree isolation required but unavailable: {error}"
                ));
            }
            Err(format!(
                "worktree unavailable, using direct writes: {error}"
            ))
        }
    }
}

async fn attach_worktree(
    ctx: &RunContext,
    store: &AgentStore,
    agent: &AgentRecord,
    workspace: &std::path::Path,
) -> (crate::FileAccess, String) {
    let mode = worktree_policy();
    if mode == "off" {
        let _ = store.set_access(&agent.id, "write").await;
        return (
            crate::FileAccess::Write,
            "worktrees disabled (STARK_WORKTREES=off); editing live tree".to_string(),
        );
    }
    match isolate_agent(ctx.store.harness_root(), workspace, &agent.id) {
        Ok((info, _)) => {
            let effective = crate::worktree::effective_workspace(&info);
            let _ = ctx
                .store
                .set_chat_workspace_path(&agent.chat_id, &effective.to_string_lossy())
                .await;
            let _ = store
                .set_worktree(
                    &agent.id,
                    &info.repo_root.to_string_lossy(),
                    &info.subdir.to_string_lossy(),
                    &info.path.to_string_lossy(),
                    &info.branch,
                    &info.base,
                    "active",
                )
                .await;
            let _ = store.set_access(&agent.id, "worktree").await;
            let _ = store
                .set_worktree_state(&agent.id, "active", &json!({}), "")
                .await;
            (
                crate::FileAccess::Worktree,
                format!(
                    "isolated in worktree branch={} path={}",
                    info.branch,
                    info.path.display()
                ),
            )
        }
        Err(note) => {
            if mode == "required" {
                return (
                    crate::FileAccess::Write,
                    format!("[WORKTREE REQUIRED] {note}"),
                );
            }
            let _ = store.set_access(&agent.id, "write").await;
            (crate::FileAccess::Write, note)
        }
    }
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

fn worktree_info(agent: &AgentRecord) -> Result<crate::worktree::WorktreeInfo, String> {
    if agent.worktree_path.trim().is_empty() || agent.worktree_branch.trim().is_empty() {
        return Err("agent has no worktree".to_string());
    }
    Ok(crate::worktree::WorktreeInfo {
        path: std::path::PathBuf::from(&agent.worktree_path),
        branch: agent.worktree_branch.clone(),
        base: agent.worktree_base.clone(),
        repo_root: std::path::PathBuf::from(&agent.repo_root),
        subdir: std::path::PathBuf::from(&agent.workspace_subdir),
    })
}

async fn diff_agent(ctx: &RunContext, store: &AgentStore, args: &Value) -> Result<String, String> {
    let current = current_agent(ctx, store).await?;
    let id = args
        .get("agent_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "diff requires agent_id".to_string())?;
    let target = store
        .get(id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("unknown agent {id}"))?;
    if target.parent_id.as_deref() != Some(current.id.as_str()) && target.id != current.id {
        return Err("agent is not a child of this agent".to_string());
    }
    let info = worktree_info(&target)?;
    let diff = crate::worktree::diff(&info.path, &info.base).map_err(|error| error.to_string())?;
    let _ = store
        .set_worktree_state(&target.id, &target.worktree_state, &diff, "")
        .await;
    let stat = diff.get("stat").and_then(Value::as_str).unwrap_or("");
    let mut text = format!(
        "[WORKTREE DIFF] {} branch={} base={} state={}\n{}",
        target.id, info.branch, info.base, target.worktree_state, stat
    );
    if let Some(files) = diff.get("files") {
        text.push_str(&format!("\nfiles: {files}"));
    }
    Ok(text)
}

async fn merge_agent(ctx: &RunContext, store: &AgentStore, args: &Value) -> Result<String, String> {
    let current = current_agent(ctx, store).await?;
    let id = args
        .get("agent_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "merge requires agent_id".to_string())?;
    let target = store
        .get(id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("unknown agent {id}"))?;
    if target.parent_id.as_deref() != Some(current.id.as_str()) {
        return Err("agent is not a child of this agent".to_string());
    }
    if matches!(target.status.as_str(), "running" | "queued" | "idle") {
        return Err("child is still running; wait for a terminal status before merge".to_string());
    }
    let info = worktree_info(&target)?;
    match crate::worktree::merge(&info) {
        Ok(_) => {
            let _ = store
                .set_worktree_state(&target.id, "merged", &json!({}), "")
                .await;
            Ok(format!(
                "[WORKTREE MERGED] {} branch={} merged into {}",
                target.id,
                info.branch,
                info.repo_root.display()
            ))
        }
        Err(error) => {
            let _ = store
                .set_worktree_state(&target.id, "active", &json!({}), &error)
                .await;
            Err(error)
        }
    }
}

async fn discard_agent(
    ctx: &RunContext,
    store: &AgentStore,
    args: &Value,
) -> Result<String, String> {
    let current = current_agent(ctx, store).await?;
    let id = args
        .get("agent_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "discard requires agent_id".to_string())?;
    let target = store
        .get(id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("unknown agent {id}"))?;
    if target.parent_id.as_deref() != Some(current.id.as_str()) {
        return Err("agent is not a child of this agent".to_string());
    }
    crate::cancel_agent_run(&target.id);
    let _ = store.set_status(&target.id, "cancelled", false).await;
    if target.worktree_path.trim().is_empty() {
        return Ok(format!("[OK] discarded {} (no worktree)", target.id));
    }
    let repo = if target.repo_root.trim().is_empty() {
        std::path::PathBuf::from(&target.worktree_path)
    } else {
        std::path::PathBuf::from(&target.repo_root)
    };
    match crate::worktree::discard(
        &repo,
        &std::path::PathBuf::from(&target.worktree_path),
        &target.worktree_branch,
    ) {
        Ok(_) => {
            let _ = store
                .set_worktree_state(&target.id, "discarded", &json!({}), "")
                .await;
            Ok(format!("[WORKTREE DISCARDED] {}", target.id))
        }
        Err(error) => Err(error),
    }
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
        "diff" => diff_agent(ctx, &store, args).await,
        "merge" => merge_agent(ctx, &store, args).await,
        "discard" => discard_agent(ctx, &store, args).await,
        other => Err(format!(
            "unknown action '{other}' (spawn|list|status|send|report|cancel|diff|merge|discard)"
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
            file_access: crate::FileAccess::Write,
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
            config: None,
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
