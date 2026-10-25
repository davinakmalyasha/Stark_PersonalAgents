use serde_json::{Value, json};
use stark_tools::ToolOutcome;

use crate::RunContext;

pub(crate) async fn give_note(ctx: &RunContext, args: &Value) -> ToolOutcome {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("create")
        .trim()
        .to_lowercase();
    let (scope_type, scope_id) = target(ctx, args);
    match action.as_str() {
        "create" => {
            if scope_type.is_empty() || scope_id.is_empty() {
                return ToolOutcome::err("Error: no target chat/group.");
            }
            let title = args.get("title").and_then(Value::as_str).unwrap_or("Note");
            let body = args.get("body").and_then(Value::as_str).unwrap_or("");
            let todos = args.get("todos").cloned().unwrap_or_else(|| json!([]));
            match ctx
                .store
                .create_note(&scope_type, &scope_id, title, body, &todos)
                .await
            {
                Ok(note) => {
                    let note_id = note["id"].as_str().unwrap_or("").to_string();
                    ToolOutcome::ok(format!(
                        "[OK] Note created: {note_id}\n{}",
                        format_notes(&[note])
                    ))
                }
                Err(error) => ToolOutcome::err(format!("Error: {error}")),
            }
        }
        "update" => {
            let Some(note_id) = args.get("note_id").and_then(Value::as_str) else {
                return ToolOutcome::err("Error: note_id is required to update.");
            };
            let title = args
                .get("title")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty());
            let body = args
                .get("body")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty());
            let todos = args.get("todos");
            match ctx.store.update_note(note_id, title, body, todos).await {
                Ok(Some(note)) => {
                    ToolOutcome::ok(format!("[OK] Note updated.\n{}", format_notes(&[note])))
                }
                Ok(None) => ToolOutcome::err(format!("Error: unknown note '{note_id}'.")),
                Err(error) => ToolOutcome::err(format!("Error: {error}")),
            }
        }
        "remove" => {
            let Some(note_id) = args.get("note_id").and_then(Value::as_str) else {
                return ToolOutcome::err("Error: note_id is required to remove.");
            };
            match ctx.store.delete_note(note_id).await {
                Ok(true) => ToolOutcome::ok("[OK] Note removed."),
                Ok(false) => ToolOutcome::err(format!("Error: unknown note '{note_id}'.")),
                Err(error) => ToolOutcome::err(format!("Error: {error}")),
            }
        }
        "list" => {
            if scope_type.is_empty() || scope_id.is_empty() {
                return ToolOutcome::err("Error: no target chat/group.");
            }
            let notes = ctx.store.list_notes(&scope_type, &scope_id).await;
            ToolOutcome::ok(format_notes(&notes))
        }
        other => ToolOutcome::err(format!(
            "Error: unknown action '{other}' (create|update|remove|list)."
        )),
    }
}

fn target(ctx: &RunContext, args: &Value) -> (String, String) {
    let raw = args
        .get("target")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if let Some(value) = raw.strip_prefix("chat:") {
        return ("chat".to_string(), value.trim().to_string());
    }
    if let Some(value) = raw.strip_prefix("group:") {
        return ("group".to_string(), value.trim().to_string());
    }
    ("chat".to_string(), ctx.chat_id.clone())
}

fn format_notes(notes: &[Value]) -> String {
    if notes.is_empty() {
        return "[OK] No notes.".to_string();
    }
    let mut lines = vec!["[NOTES]".to_string()];
    for note in notes {
        lines.push(format!(
            "- {} · {}",
            note["id"].as_str().unwrap_or(""),
            note["title"].as_str().unwrap_or("")
        ));
        if let Some(body) = note["body"].as_str().filter(|body| !body.is_empty()) {
            lines.push(format!("  {body}"));
        }
        if let Some(todos) = note["todos"].as_array() {
            for todo in todos {
                let mark = if todo["done"].as_bool().unwrap_or(false) {
                    "x"
                } else {
                    " "
                };
                lines.push(format!(
                    "  [{mark}] {}",
                    todo["text"].as_str().unwrap_or("")
                ));
            }
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_note_checklist() {
        let text = format_notes(&[json!({
            "id": "note-1",
            "title": "Plan",
            "body": "Keep it short",
            "todos": [
                {"text": "one", "done": true},
                {"text": "two", "done": false}
            ]
        })]);
        assert!(text.contains("- note-1 · Plan"));
        assert!(text.contains("[x] one"));
        assert!(text.contains("[ ] two"));
    }
}
