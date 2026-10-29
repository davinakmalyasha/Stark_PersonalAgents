//! The `memory` tool: read the compact workspace index, read topic files on
//! demand, search durable entries, and add/forget/pin notes.
//!
//! This mirrors the Codex/Claude split: the index is injected at the start of
//! a run; the long-form memory is only read when the agent asks for it.

use serde_json::Value;
use stark_store::{MemoryStore, NewEntry};

use crate::{ToolContext, ToolOutcome};

pub async fn handle(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let store = match MemoryStore::open(&ctx.harness_root).await {
        Ok(store) => store,
        Err(error) => return ToolOutcome::err(format!("Error: memory store unavailable: {error}")),
    };
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("index")
        .trim()
        .to_lowercase();
    if matches!(action.as_str(), "remember" | "forget" | "pin") && ctx.mode == "plan" {
        return ToolOutcome::err(
            "[BLOCKED] Plan mode is read-only. Memory writes are disabled until /build.",
        );
    }
    match action.as_str() {
        "index" => match store
            .index(&ctx.workspace_id, stark_store::memory_max_chars())
            .await
        {
            Some(index) => ToolOutcome::ok(index),
            None => ToolOutcome::ok(
                "[MEMORY] No durable workspace memory yet. Completed turns appear here after consolidation."
                    .to_string(),
            ),
        },
        "summary" => match store
            .summary(&ctx.workspace_id, stark_store::memory_max_chars())
            .await
        {
            Some(summary) => ToolOutcome::ok(summary),
            None => ToolOutcome::ok("[MEMORY] No long-form summary yet.".to_string()),
        },
        "list" => match store.topics(&ctx.workspace_id).await {
            Ok(topics) => {
                if topics.is_empty() {
                    return ToolOutcome::ok("[MEMORY] No topics yet.".to_string());
                }
                let mut text = String::from("[MEMORY TOPICS]\n");
                for topic in topics {
                    text.push_str(&format!(
                        "- {}: {} entries, {} chars\n",
                        topic["name"].as_str().unwrap_or("?"),
                        topic["entries"].as_u64().unwrap_or(0),
                        topic["chars"].as_u64().unwrap_or(0),
                    ));
                }
                text.push_str("Read with: memory action=\"read\" topic=\"<name>\"");
                ToolOutcome::ok(text)
            }
            Err(error) => ToolOutcome::err(format!("Error: {error}")),
        },
        "read" => {
            let Some(topic) = args.get("topic").and_then(Value::as_str) else {
                return ToolOutcome::err("Error: memory action='read' requires topic");
            };
            match store.read_topic(&ctx.workspace_id, topic).await {
                Ok(Some(text)) => {
                    let _ = store.mark_topic_used(&ctx.workspace_id, topic).await;
                    ToolOutcome::ok(text.chars().take(32_000).collect::<String>())
                }
                Ok(None) => ToolOutcome::err(format!(
                    "Error: unknown memory topic '{topic}'. Use action='list'."
                )),
                Err(error) => ToolOutcome::err(format!("Error: {error}")),
            }
        }
        "search" => {
            let Some(query) = args.get("query").and_then(Value::as_str) else {
                return ToolOutcome::err("Error: memory action='search' requires query");
            };
            match store.search(&ctx.workspace_id, query, 20).await {
                Ok(entries) if entries.is_empty() => {
                    ToolOutcome::ok(format!("[MEMORY] No entries matching '{query}'."))
                }
                Ok(entries) => {
                    let mut text = format!("[MEMORY SEARCH] {}\n", query);
                    for entry in entries {
                        text.push_str(&format!(
                            "- [{}] {} ({})\n",
                            entry.kind, entry.text, entry.topic
                        ));
                    }
                    ToolOutcome::ok(text)
                }
                Err(error) => ToolOutcome::err(format!("Error: {error}")),
            }
        }
        "remember" => {
            let Some(text) = args
                .get("text")
                .or_else(|| args.get("memory"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
            else {
                return ToolOutcome::err("Error: memory action='remember' requires text");
            };
            let kind = args
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("note");
            let topic = args
                .get("topic")
                .and_then(Value::as_str)
                .unwrap_or("notes");
            let pinned = args.get("pinned").and_then(Value::as_bool).unwrap_or(true);
            match store
                .add_entry(
                    &ctx.workspace_id,
                    NewEntry {
                        chat_id: ctx.chat_id.clone(),
                        agent_id: None,
                        kind: kind.to_string(),
                        topic: topic.to_string(),
                        text: text.to_string(),
                        source: "tool".to_string(),
                        model: String::new(),
                        pinned,
                    },
                )
                .await
            {
                Ok(id) => ToolOutcome::ok(format!(
                    "[MEMORY] Saved {kind} to {topic} (id: {id})."
                )),
                Err(error) => ToolOutcome::err(format!("Error: {error}")),
            }
        }
        "forget" => {
            let Some(entry_id) = args
                .get("entry_id")
                .or_else(|| args.get("id"))
                .and_then(Value::as_str)
            else {
                return ToolOutcome::err("Error: memory action='forget' requires entry_id");
            };
            match store.forget_entry(&ctx.workspace_id, entry_id).await {
                Ok(true) => ToolOutcome::ok(format!("[MEMORY] Forgot entry {entry_id}.")),
                Ok(false) => ToolOutcome::err(format!("Error: unknown memory entry {entry_id}")),
                Err(error) => ToolOutcome::err(format!("Error: {error}")),
            }
        }
        "pin" => {
            let Some(entry_id) = args
                .get("entry_id")
                .or_else(|| args.get("id"))
                .and_then(Value::as_str)
            else {
                return ToolOutcome::err("Error: memory action='pin' requires entry_id");
            };
            let pinned = args.get("pinned").and_then(Value::as_bool).unwrap_or(true);
            match store
                .pin_entry(&ctx.workspace_id, entry_id, pinned)
                .await
            {
                Ok(true) => ToolOutcome::ok(format!(
                    "[MEMORY] {} entry {entry_id}.",
                    if pinned { "Pinned" } else { "Unpinned" }
                )),
                Ok(false) => ToolOutcome::err(format!("Error: unknown memory entry {entry_id}")),
                Err(error) => ToolOutcome::err(format!("Error: {error}")),
            }
        }
        "status" => match store.status(&ctx.workspace_id).await {
            Ok(status) => ToolOutcome::ok(serde_json::to_string_pretty(&status).unwrap_or_default()),
            Err(error) => ToolOutcome::err(format!("Error: {error}")),
        },
        other => ToolOutcome::err(format!(
            "Error: unknown memory action '{other}' (index|summary|list|read|search|remember|forget|pin|status)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use crate::QuestionBroker;

    use super::*;

    #[tokio::test]
    async fn remember_index_read_and_forget() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path(), "chat", Arc::new(QuestionBroker::new()))
            .with_workspace_id("_general");
        let saved = handle(
            &ctx,
            &json!({"action":"remember","text":"Use pnpm, not npm","topic":"preferences"}),
        )
        .await;
        assert!(!saved.is_error, "{}", saved.content);
        let index = handle(&ctx, &json!({"action":"index"})).await;
        assert!(index.content.contains("pnpm"), "{}", index.content);
        let read = handle(&ctx, &json!({"action":"read","topic":"preferences"})).await;
        assert!(read.content.contains("pnpm"), "{}", read.content);
        let list = handle(&ctx, &json!({"action":"list"})).await;
        assert!(list.content.contains("preferences"), "{}", list.content);
        let search = handle(&ctx, &json!({"action":"search","query":"pnpm"})).await;
        assert!(search.content.contains("pnpm"), "{}", search.content);
    }

    #[tokio::test]
    async fn plan_mode_blocks_memory_writes() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path(), "chat", Arc::new(QuestionBroker::new()))
            .with_workspace_id("_general")
            .with_mode("plan");
        let saved = handle(&ctx, &json!({"action":"remember","text":"x"})).await;
        assert!(saved.is_error);
        let index = handle(&ctx, &json!({"action":"index"})).await;
        assert!(!index.is_error);
    }
}
