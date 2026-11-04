//! Orchestrator → workspace delegation, backed by durable agents.
//!
//! The legacy job registry is gone. `delegate` creates/reuses a durable
//! workspace agent and queues the task; the server supervisor runs it.

use serde_json::{Value, json};
use stark_store::{AgentRecord, AgentStore, WorkspaceRegistry};
use stark_tools::ToolOutcome;

use std::time::{Duration, Instant};

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

const VIRTUAL_WORKSPACE_IDS: [&str; 4] = ["_general", "_house", "_personal", "_dfd"];

/// Exact identity only. Substring matching silently routes work to the wrong
/// project when names overlap, so an ambiguous name returns the candidate ids
/// instead of a guess.
fn match_record<'a>(
    records: &'a [stark_store::WorkspaceRecord],
    query: &str,
) -> Result<Option<&'a stark_store::WorkspaceRecord>, String> {
    let query = query.trim();
    if let Some(record) = records
        .iter()
        .find(|record| record.id.eq_ignore_ascii_case(query))
    {
        return Ok(Some(record));
    }
    let mut matches = records
        .iter()
        .filter(|record| record.name.eq_ignore_ascii_case(query));
    let Some(first) = matches.next() else {
        return Ok(None);
    };
    let rest: Vec<_> = matches.collect();
    if rest.is_empty() {
        return Ok(Some(first));
    }
    let mut ids: Vec<&str> = std::iter::once(first.id.as_str())
        .chain(rest.iter().map(|record| record.id.as_str()))
        .collect();
    ids.sort_unstable();
    Err(format!(
        "Error: workspace name '{query}' is ambiguous. Retry with workspace_id (one of: {}).",
        ids.join(", ")
    ))
}

fn resolve_target(ctx: &RunContext, item: &JobItem) -> Result<(String, String), String> {
    let harness = ctx.store.harness_root();
    let registry = WorkspaceRegistry::new(harness);
    let query = if !item.workspace.trim().is_empty() {
        item.workspace.trim()
    } else if !item.project.trim().is_empty() {
        item.project.trim()
    } else {
        ""
    };
    if !query.is_empty() {
        // Virtual workspaces (_general, _house, ...) have no registry entry.
        if let Some(virtual_id) = VIRTUAL_WORKSPACE_IDS
            .iter()
            .find(|id| id.eq_ignore_ascii_case(query))
        {
            let path = if virtual_id.eq_ignore_ascii_case(&ctx.workspace_id) {
                ctx.workspace.to_string_lossy().to_string()
            } else {
                harness.to_string_lossy().to_string()
            };
            return Ok((virtual_id.to_string(), path));
        }
        let records = registry.list();
        return match match_record(&records, query) {
            Ok(Some(record)) => Ok((record.id.clone(), record.path.clone())),
            Ok(None) => Err(format!(
                "Error: unknown workspace '{query}'. Use a workspace id or an exact name."
            )),
            Err(error) => Err(error),
        };
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
            .unwrap_or(300)
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
    // Long-poll instead of tight polling: wait until the children settle, the
    // user sends another message, or the deadline (default 300s) passes. This
    // is one tool call, so repeated identical calls (and the repeat guard)
    // never fire.
    let wait = args
        .get("wait_seconds")
        .and_then(Value::as_u64)
        .unwrap_or(300)
        .min(300);
    let deadline = Instant::now() + Duration::from_secs(wait);
    let agents = loop {
        let agents = store.list(Some(&current.id)).await.unwrap_or_default();
        let busy = agents
            .iter()
            .any(|agent| matches!(agent.status.as_str(), "running" | "queued"));
        let steered = ctx
            .steering
            .lock()
            .map(|queue| !queue.is_empty())
            .unwrap_or(false);
        if !busy || steered || ctx.cancel.is_cancelled() || Instant::now() >= deadline {
            break agents;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
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

    fn record(id: &str, name: &str) -> stark_store::WorkspaceRecord {
        stark_store::WorkspaceRecord {
            id: id.to_string(),
            name: name.to_string(),
            path: format!("C:/projects/{id}"),
            root: None,
            rel: None,
            repo: None,
            branch: None,
            added_at: String::new(),
        }
    }

    #[test]
    fn resolves_exact_id_before_name() {
        let records = vec![record("ws-1", "api"), record("ws-2", "api-v2")];
        assert_eq!(
            match_record(&records, "WS-1")
                .unwrap()
                .map(|record| record.id.as_str()),
            Some("ws-1")
        );
        assert_eq!(
            match_record(&records, "api")
                .unwrap()
                .map(|record| record.id.as_str()),
            Some("ws-1")
        );
    }

    #[test]
    fn ambiguous_names_list_candidate_ids() {
        let records = vec![record("ws-1", "api"), record("ws-2", "API")];
        let error = match_record(&records, "api").unwrap_err();
        assert!(error.contains("ambiguous"), "{error}");
        assert!(error.contains("ws-1") && error.contains("ws-2"), "{error}");
    }

    #[test]
    fn unknown_queries_do_not_substring_match() {
        let records = vec![record("ws-1", "api-gateway")];
        assert!(match_record(&records, "gateway").unwrap().is_none());
    }
}
