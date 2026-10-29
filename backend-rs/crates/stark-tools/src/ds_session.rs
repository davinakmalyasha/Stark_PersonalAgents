//! DeepSeek Harness-compatible session tools: working_directory,
//! todo_write, and ask_user_question.

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::paths::resolve_workspace_path;
use crate::{ToolContext, ToolOutcome};

pub async fn working_directory(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let Some(raw) = args.get("cd").and_then(Value::as_str).map(str::trim) else {
        return ToolOutcome::ok(ctx.current_dir().to_string_lossy().to_string());
    };
    if raw.is_empty() {
        return ToolOutcome::ok(ctx.current_dir().to_string_lossy().to_string());
    }
    let candidate = PathBuf::from(raw);
    let joined = if candidate.is_absolute() {
        candidate
    } else {
        ctx.current_dir().join(candidate)
    };
    let path = match resolve_workspace_path(&ctx.workspace, &joined.to_string_lossy()) {
        Ok(path) => path,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };
    if !path.is_dir() {
        return ToolOutcome::err(format!("Error: not a directory: {raw}"));
    }
    ctx.set_current_dir(path.clone());
    ToolOutcome::ok(path.to_string_lossy().to_string())
}

pub async fn todo_write(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let Some(todos) = args.get("todos").and_then(Value::as_array) else {
        return ToolOutcome::err("Error: todo_write requires todos");
    };
    let mut plan = Vec::new();
    for todo in todos {
        let Some(content) = todo.get("content").and_then(Value::as_str) else {
            return ToolOutcome::err("Error: every todo requires content");
        };
        let Some(status) = todo.get("status").and_then(Value::as_str) else {
            return ToolOutcome::err("Error: every todo requires status");
        };
        if !matches!(status, "pending" | "in_progress" | "completed") {
            return ToolOutcome::err(format!(
                "Error: invalid todo status '{status}' (must be pending|in_progress|completed)"
            ));
        }
        plan.push(json!({"step": content, "status": status}));
    }
    let outcome = crate::plan::update_plan(ctx, &json!({"plan": plan})).await;
    if outcome.is_error {
        return outcome;
    }
    ToolOutcome {
        content: outcome.content,
        is_error: false,
        exit_code: None,
        plan: outcome.plan,
        children: Vec::new(),
    }
}

fn selected_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Object(object) => object
            .get("label")
            .or_else(|| object.get("value"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        _ => String::new(),
    }
}

pub async fn ask_user_question(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    if ctx.depth > 0 {
        return ToolOutcome::err(
            "[BLOCKED] Subagents run non-interactively; ask the parent to resolve this.",
        );
    }
    let Some(questions) = args.get("questions").and_then(Value::as_array) else {
        return ToolOutcome::err("Error: ask_user_question requires questions");
    };
    if questions.is_empty() {
        return ToolOutcome::err("Error: ask_user_question requires at least one question");
    }
    let answers = match ctx
        .questions
        .ask(&ctx.chat_id, questions.clone(), 600)
        .await
    {
        Ok(answers) => answers,
        Err(error) => return ToolOutcome::err(format!("Error: ask_user_question {error}")),
    };
    let mut lines = vec!["[USER ANSWERS]".to_string()];
    for answer in answers {
        let id = answer.get("id").and_then(Value::as_str).unwrap_or("?");
        let selected = answer
            .get("selected")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .map(selected_text)
                    .filter(|text| !text.is_empty())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let custom = answer
            .get("custom")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let mut parts = Vec::new();
        if !selected.is_empty() {
            parts.push(selected);
        }
        if !custom.is_empty() {
            parts.push(format!("custom: {custom}"));
        }
        let text = if parts.is_empty() {
            "(no answer)".to_string()
        } else {
            parts.join("; ")
        };
        lines.push(format!("- {id}: {text}"));
    }
    ToolOutcome::ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::QuestionBroker;

    fn context(dir: &std::path::Path) -> ToolContext {
        ToolContext::new(dir, "v2-session", Arc::new(QuestionBroker::new()))
    }

    #[tokio::test]
    async fn working_directory_reads_and_changes_cwd() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let ctx = context(dir.path());
        let changed = working_directory(&ctx, &json!({"cd": "sub"})).await;
        assert!(!changed.is_error, "{}", changed.content);
        assert!(ctx.current_dir().ends_with("sub"));
        let read = working_directory(&ctx, &json!({})).await;
        assert!(read.content.ends_with("sub"), "{}", read.content);
    }

    #[tokio::test]
    async fn todo_write_updates_the_plan_store() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let out = todo_write(
            &ctx,
            &json!({"todos": [
                {"content": "inspect", "status": "completed"},
                {"content": "implement", "status": "in_progress"}
            ]}),
        )
        .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.plan.is_some());
        let saved = ctx
            .plan_store
            .load_plan("v2-session", "agent")
            .await
            .unwrap();
        assert_eq!(saved.len(), 2);
        assert_eq!(saved[1].status, "in_progress");
    }
}
