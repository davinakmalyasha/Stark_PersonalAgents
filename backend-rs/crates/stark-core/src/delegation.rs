//! Orchestrator → workspace delegation, backed by durable agents.
//!
//! The legacy job registry is gone. `delegate` creates/reuses a durable
//! workspace agent and queues the task; the server supervisor runs it.

use serde_json::{Value, json};
use stark_store::{AgentRecord, AgentStore, WorkspaceRegistry};
use stark_tools::ToolOutcome;

use crate::{RunContext, agent_for_scope, agents_tools};

const MAX_JOBS: usize = 8;

struct JobItem {
    agent: crate::AgentSpec,
    task: String,
    outcome: String,
    acceptance: String,
    title: String,
    workspace: String,
    project: String,
    isolate: bool,
}

pub(crate) async fn delegate(ctx: &RunContext, args: &Value) -> ToolOutcome {
    let items = match parse_items(args) {
        Ok(items) => items,
        Err(error) => return ToolOutcome::err(error),
    };
    let store = match AgentStore::open(ctx.store.harness_root()).await {
        Ok(store) => store,
        Err(error) => return ToolOutcome::err(format!("Error: agent store unavailable: {error}")),
    };
    let _ = agents_tools::current_agent(ctx, &store).await;
    let mut rows = Vec::new();
    let mut children = Vec::new();
    for item in items {
        let (workspace_id, workspace_path) = match resolve_target(ctx, &item) {
            Ok(target) => target,
            Err(error) => return ToolOutcome::err(error),
        };
        let task = job_prompt(&item);
        let result = agents_tools::handle(
            ctx,
            &json!({
                "action": "spawn",
                "kind": "workspace",
                "agent": item.agent.display,
                "task": task,
                "title": item.title,
                "workspace_id": workspace_id,
                "workspace_path": workspace_path,
                "isolate": item.isolate,
                "wait_seconds": 0
            }),
        )
        .await;
        if result.is_error {
            return result;
        }
        let Some(agent) = store
            .workspace_agent(&workspace_id, item.agent.key)
            .await
            .ok()
            .flatten()
        else {
            return ToolOutcome::err("Error: delegated agent was not persisted");
        };
        rows.push(format!(
            "- {} · {} · {} — {} (chat: {})",
            agent.id, agent.role, item.title, agent.status, agent.chat_id
        ));
        children.push(json!({
            "agentId": agent.id,
            "chatId": agent.chat_id,
            "agentName": agent.role,
            "label": item.title,
            "workspaceId": workspace_id,
        }));
    }
    ToolOutcome::ok(format!(
        "[DELEGATION STARTED]\n{}\nUse delegation_status with wait_seconds to collect results; job_message/job_cancel manage the agent.",
        rows.join("\n")
    ))
    .with_children(children)
}

fn resolve_target(ctx: &RunContext, item: &JobItem) -> Result<(String, String), String> {
    let harness = ctx.store.harness_root();
    let registry = WorkspaceRegistry::new(harness);
    if !item.workspace.trim().is_empty() {
        let query = item.workspace.trim();
        if let Some(record) = registry
            .get(query)
            .or_else(|| registry.find_by_path(query))
            .or_else(|| {
                registry.list().into_iter().find(|workspace| {
                    workspace.name.eq_ignore_ascii_case(query)
                        || workspace.id.eq_ignore_ascii_case(query)
                })
            })
        {
            return Ok((record.id, record.path));
        }
        // Virtual workspaces (_general, _house, ...) have no registry entry.
        if query.starts_with('_') {
            let path = if query == ctx.workspace_id {
                ctx.workspace.to_string_lossy().to_string()
            } else {
                harness.to_string_lossy().to_string()
            };
            return Ok((query.to_string(), path));
        }
        // A raw filesystem path is also acceptable.
        if std::path::Path::new(query).is_dir() {
            let id = ctx.store.workspace_id_for_path(query);
            return Ok((id, query.to_string()));
        }
        return Err(format!("Error: unknown workspace '{}'", item.workspace));
    }
    if !item.project.trim().is_empty() {
        let query = item.project.trim().to_ascii_lowercase();
        let mut matches: Vec<_> = registry
            .list()
            .into_iter()
            .filter(|workspace| {
                workspace.name.to_ascii_lowercase().contains(&query)
                    || workspace.id.to_ascii_lowercase().contains(&query)
                    || workspace.path.to_ascii_lowercase().contains(&query)
            })
            .collect();
        if matches.is_empty() {
            return Err(format!("Error: unknown project '{}'", item.project));
        }
        matches.sort_by_key(|workspace| workspace.name.len());
        let record = matches.remove(0);
        return Ok((record.id, record.path));
    }
    match item.agent.key {
        "house" => Ok((
            "_house".to_string(),
            harness.join("_house").to_string_lossy().to_string(),
        )),
        "general" => Ok((
            "_general".to_string(),
            harness.to_string_lossy().to_string(),
        )),
        "dfd" => Ok((
            "_dfd".to_string(),
            harness.join("_dfd").to_string_lossy().to_string(),
        )),
        "developer" | "ui" | "devops" => {
            // Never silently write a project agent into the harness root.
            let in_project = ctx.workspace != *harness && !ctx.workspace_id.starts_with('_');
            if in_project {
                return Ok((
                    ctx.workspace_id.clone(),
                    ctx.workspace.to_string_lossy().to_string(),
                ));
            }
            Err(format!(
                "Error: {} delegation requires a workspace or project target; current chat is not in a project",
                item.agent.display
            ))
        }
        _ => Ok((
            ctx.workspace_id.clone(),
            ctx.workspace.to_string_lossy().to_string(),
        )),
    }
}

pub(crate) async fn job_message(ctx: &RunContext, args: &Value) -> ToolOutcome {
    let Some(id) = args
        .get("job_id")
        .and_then(Value::as_str)
        .or_else(|| args.get("agent_id").and_then(Value::as_str))
    else {
        return ToolOutcome::err("Error: job_message requires job_id");
    };
    let Some(message) = args.get("message").and_then(Value::as_str) else {
        return ToolOutcome::err("Error: job_message requires message");
    };
    agents_tools::handle(
        ctx,
        &json!({"action":"send","agent_id":id,"message":message}),
    )
    .await
}

pub(crate) async fn job_cancel(ctx: &RunContext, args: &Value) -> ToolOutcome {
    let Some(id) = args
        .get("job_id")
        .and_then(Value::as_str)
        .or_else(|| args.get("agent_id").and_then(Value::as_str))
    else {
        return ToolOutcome::err("Error: job_cancel requires job_id");
    };
    agents_tools::handle(ctx, &json!({"action":"cancel","agent_id":id})).await
}

pub(crate) async fn delegation_status(ctx: &RunContext, args: &Value) -> ToolOutcome {
    let store = match AgentStore::open(ctx.store.harness_root()).await {
        Ok(store) => store,
        Err(error) => return ToolOutcome::err(format!("Error: agent store unavailable: {error}")),
    };
    let id = args.get("id").and_then(Value::as_str).unwrap_or("").trim();
    if !id.is_empty() {
        let wait = args
            .get("wait_seconds")
            .and_then(Value::as_u64)
            .unwrap_or(60)
            .min(300);
        let detail = if wait == 0 {
            match store.get(id).await {
                Ok(Some(agent)) => Ok(format_agent(&agent)),
                Ok(None) => Err(format!("Error: unknown delegation '{id}'")),
                Err(error) => Err(error.to_string()),
            }
        } else {
            agents_tools::wait_for(&store, id, wait).await
        };
        return match detail {
            Ok(text) => ToolOutcome::ok(text),
            Err(error) => ToolOutcome::err(error),
        };
    }
    let current = match agents_tools::current_agent(ctx, &store).await {
        Ok(agent) => agent,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };
    let agents = store.list(Some(&current.id)).await.unwrap_or_default();
    if agents.is_empty() {
        return ToolOutcome::ok("[DELEGATIONS]\nNo active delegations.".to_string());
    }
    let rows = agents
        .iter()
        .map(|agent| {
            format!(
                "- {} · {} · {} — {} (chat: {})",
                agent.id, agent.role, agent.nickname, agent.status, agent.chat_id
            )
        })
        .collect::<Vec<_>>();
    ToolOutcome::ok(format!("[DELEGATIONS]\n{}", rows.join("\n")))
}

fn format_agent(agent: &AgentRecord) -> String {
    let mut text = format!(
        "[AGENT {}]\nrole={}\nstatus={}\nchat={}",
        agent.id, agent.role, agent.status, agent.chat_id
    );
    if !agent.result.is_empty() {
        text.push_str("\nresult:\n");
        text.push_str(&agent.result.chars().take(12_000).collect::<String>());
    }
    text
}

fn parse_items(args: &Value) -> Result<Vec<JobItem>, String> {
    let Some(items) = args.get("items").and_then(Value::as_array) else {
        return Err("Error: delegate requires a non-empty items array".to_string());
    };
    if items.is_empty() {
        return Err("Error: delegate requires a non-empty items array".to_string());
    }
    if items.len() > MAX_JOBS {
        return Err(format!(
            "Error: at most {MAX_JOBS} jobs can be delegated at once"
        ));
    }
    let mut parsed = Vec::with_capacity(items.len());
    for (index, raw) in items.iter().enumerate() {
        let object = raw
            .as_object()
            .ok_or_else(|| format!("Error: items[{index}] must be an object"))?;
        let agent_name = object
            .get("agent")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let agent = agent_for_scope(agent_name)
            .filter(|agent| matches!(agent.key, "developer" | "general" | "house" | "ui" | "dfd"))
            .ok_or_else(|| {
                format!("Error: items[{index}].agent must be Developer, General, House, UI, or DFD")
            })?;
        let task = object
            .get("task")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if task.is_empty() {
            return Err(format!("Error: items[{index}].task is required"));
        }
        let title = object
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let title = if title.is_empty() {
            task.chars().take(60).collect()
        } else {
            title
        };
        parsed.push(JobItem {
            agent,
            task,
            outcome: object
                .get("outcome")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
            acceptance: object
                .get("acceptance")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
            title,
            workspace: object
                .get("workspace")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
            project: object
                .get("project")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
            isolate: object
                .get("isolate")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        });
    }
    Ok(parsed)
}

fn job_prompt(item: &JobItem) -> String {
    let mut prompt = item.task.clone();
    if !item.outcome.is_empty() {
        prompt.push_str("\n\nExpected outcome:\n");
        prompt.push_str(&item.outcome);
    }
    if !item.acceptance.is_empty() {
        prompt.push_str("\n\nAcceptance criteria:\n");
        prompt.push_str(&item.acceptance);
    }
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(agent: &str, task: &str) -> Value {
        json!({"agent": agent, "task": task})
    }

    #[test]
    fn validates_items() {
        assert!(parse_items(&json!({"items": []})).is_err());
        assert!(parse_items(&json!({"items": [item("Orchestrator", "x")]})).is_err());
        assert!(parse_items(&json!({"items": [item("Developer", "")]})).is_err());
        let parsed = parse_items(&json!({
            "items": [item("developer", "Build it"), item("General", "Research it")]
        }))
        .unwrap();
        assert_eq!(parsed[0].agent.key, "developer");
        assert_eq!(parsed[1].agent.key, "general");
    }

    #[test]
    fn prompt_includes_outcome_and_acceptance() {
        let parsed = parse_items(&json!({
            "items": [{
                "agent": "Developer",
                "task": "Build it",
                "outcome": "A working build",
                "acceptance": "Tests pass"
            }]
        }))
        .unwrap();
        let prompt = job_prompt(&parsed[0]);
        assert!(prompt.contains("Expected outcome:\nA working build"));
        assert!(prompt.contains("Acceptance criteria:\nTests pass"));
    }
}
