use serde_json::Value;

use crate::{ToolContext, ToolOutcome};

pub async fn ask_user(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let Some(questions) = args.get("questions").and_then(Value::as_array) else {
        return ToolOutcome::err("Error: ask_user requires questions");
    };
    if questions.is_empty() {
        return ToolOutcome::err("Error: ask_user requires at least one question");
    }

    let timeout = args
        .get("timeout")
        .and_then(Value::as_u64)
        .unwrap_or(600)
        .max(1);

    let answers = match ctx
        .questions
        .ask(&ctx.chat_id, questions.clone(), timeout)
        .await
    {
        Ok(answers) => answers,
        Err(error) => return ToolOutcome::err(format!("Error: ask_user {error}")),
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
                    .filter_map(Value::as_str)
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
