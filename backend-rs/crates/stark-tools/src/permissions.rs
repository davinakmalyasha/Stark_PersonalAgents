//! Tool-side permission gate backed by `.deep_agents/permissions.json`.
//!
//! Mirrors OpenCode: `deny` blocks before execution, `ask` prompts the user
//! (subagents are non-interactive and are blocked), `allow` passes through.

use serde_json::json;

use stark_policy::{Effect, PermissionSet};

use crate::{ToolContext, ToolOutcome};

pub fn effect(ctx: &ToolContext, action: &str, resources: &[String]) -> Effect {
    PermissionSet::load(&ctx.harness_root).evaluate(action, resources)
}

/// Returns `Some(blocked_outcome)` when the call must not run.
pub async fn gate(
    ctx: &ToolContext,
    action: &str,
    resources: &[String],
    label: &str,
) -> Option<ToolOutcome> {
    match effect(ctx, action, resources) {
        Effect::Allow => None,
        Effect::Deny => Some(ToolOutcome::err(format!(
            "[BLOCKED] {action} denied by permissions.json ({label}). Change the request or update the rule."
        ))),
        Effect::Ask => {
            if ctx.depth > 0 {
                return Some(ToolOutcome::err(format!(
                    "[BLOCKED] Subagents run non-interactively; {action} on {label} requires user approval."
                )));
            }
            let approved = ask_user(ctx, action, label).await;
            if approved {
                None
            } else {
                Some(ToolOutcome::err(format!(
                    "[BLOCKED] {action} on {label} was not approved."
                )))
            }
        }
    }
}

async fn ask_user(ctx: &ToolContext, action: &str, label: &str) -> bool {
    let question = json!({
        "id": format!("permission-{action}"),
        "header": "Permission",
        "question": format!("Allow this {action}?\n\n{label}"),
        "options": ["Allow once", "Deny"],
    });
    match ctx.questions.ask(&ctx.chat_id, vec![question], 300).await {
        Ok(answers) => answers.iter().any(|answer| {
            answer
                .get("selected")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|selected| {
                    selected
                        .iter()
                        .any(|value| value.as_str() == Some("Allow once"))
                })
        }),
        Err(_) => false,
    }
}
