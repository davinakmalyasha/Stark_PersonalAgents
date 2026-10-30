//! Session tools: todo_write and ask_user_question.
//!
//! The working directory is runtime context (Zone 1), not a tool: it is
//! injected into the system prompt every turn and relative paths resolve
//! server-side. Navigation uses `bash` with a per-call `workdir`.

use serde_json::{Value, json};

use crate::{ToolContext, ToolOutcome};

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
    for answer in &answers {
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
    // Plan-review approval marker (replaces exit_plan_mode): when a question
    // carries intent.kind == "plan-review" and the user selects the approve
    // option, the run loop flips the chat to build mode. Rejection keeps the
    // chat in plan mode.
    let mut approved = false;
    for question in questions.iter() {
        let is_review = question
            .get("intent")
            .and_then(|intent| intent.get("kind"))
            .and_then(Value::as_str)
            == Some("plan-review");
        if !is_review {
            continue;
        }
        let approve = question
            .get("intent")
            .and_then(|intent| intent.get("approve"))
            .and_then(Value::as_str)
            .unwrap_or("Approve plan");
        let qid = question.get("id").and_then(Value::as_str).unwrap_or("");
        if answers.iter().any(|answer| {
            answer.get("id").and_then(Value::as_str) == Some(qid)
                && answer
                    .get("selected")
                    .and_then(Value::as_array)
                    .is_some_and(|selected| {
                        selected.iter().any(|value| {
                            selected_text(value) == approve || value.as_str() == Some(approve)
                        })
                    })
        }) {
            approved = true;
        }
    }
    if approved {
        lines.push(
            "[PLAN APPROVED] Stop here. Implementation begins on the next turn in build mode."
                .to_string(),
        );
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

    #[tokio::test]
    async fn plan_review_approval_adds_a_marker() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let broker = ctx.questions.clone();
        let task = tokio::spawn(async move {
            ask_user_question(
                &ctx,
                &json!({"questions": [{
                    "id": "plan-review",
                    "question": "Approve this plan?",
                    "header": "Plan review",
                    "options": ["Approve plan", "Keep planning"],
                    "intent": {"kind": "plan-review", "approve": "Approve plan"}
                }]}),
            )
            .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let batches = broker.pending_batches();
        assert_eq!(batches.len(), 1);
        let batch_id = batches[0]["batch_id"].as_str().unwrap().to_string();
        assert!(broker.answer(
            &batch_id,
            vec![json!({"id": "plan-review", "selected": ["Approve plan"]})],
        ));
        let out = task.await.unwrap();
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("[PLAN APPROVED]"), "{}", out.content);
    }

    #[tokio::test]
    async fn plan_review_rejection_has_no_marker() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let broker = ctx.questions.clone();
        let task = tokio::spawn(async move {
            ask_user_question(
                &ctx,
                &json!({"questions": [{
                    "id": "plan-review",
                    "question": "Approve this plan?",
                    "options": ["Approve plan", "Keep planning"],
                    "intent": {"kind": "plan-review", "approve": "Approve plan"}
                }]}),
            )
            .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let batches = broker.pending_batches();
        assert_eq!(batches.len(), 1);
        let batch_id = batches[0]["batch_id"].as_str().unwrap().to_string();
        assert!(broker.answer(
            &batch_id,
            vec![json!({"id": "plan-review", "selected": ["Keep planning"]})],
        ));
        let out = task.await.unwrap();
        assert!(!out.is_error, "{}", out.content);
        assert!(!out.content.contains("[PLAN APPROVED]"), "{}", out.content);
    }
}
