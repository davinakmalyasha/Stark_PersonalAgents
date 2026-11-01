//! Tool-side permission gate backed by `.deep_agents/permissions.json`.
//!
//! Mirrors OpenCode: `deny` blocks before execution, `ask` prompts the user
//! (subagents are non-interactive and are blocked), `allow` passes through.
//! `STARK_AUTO_APPROVE` (or `"auto_approve": true`) turns asks into allows,
//! but never overrides a deny.

use serde_json::json;

use stark_policy::{Effect, PermissionSet};

use crate::{ToolContext, ToolOutcome};

pub async fn effect(ctx: &ToolContext, action: &str, resources: &[String]) -> Effect {
    PermissionSet::load_for(&ctx.harness_root, &ctx.scope).evaluate(action, resources)
}

/// Doom-loop gate: `Some(outcome)` blocks the repeat, `None` lets it run.
pub async fn doom_loop_gate(ctx: &ToolContext, tool: &str) -> Option<ToolOutcome> {
    gate(ctx, "doom_loop", &[tool.to_string()], tool).await
}

/// Absolute path arguments that point outside the workspace (used by the
/// `external_directory` rule for shell commands).
pub fn external_paths(command: &str, workspace: &std::path::Path) -> Vec<String> {
    let root = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let Some(tokens) = shlex::split(command) else {
        return Vec::new();
    };
    let mut outside = Vec::new();
    for token in tokens {
        let candidate = token.trim_matches(|character| character == '"' || character == '\'');
        let looks_absolute = candidate.starts_with('/')
            || candidate.starts_with('\\')
            || candidate.chars().nth(1) == Some(':');
        if !looks_absolute || candidate.len() < 3 {
            continue;
        }
        let path = std::path::Path::new(candidate);
        let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if !resolved.starts_with(&root) {
            outside.push(resolved.to_string_lossy().to_string());
        }
    }
    outside
}

fn session_key(action: &str, resources: &[String]) -> String {
    format!("{action}\u{0}{}", resources.join("\u{1}"))
}

/// Returns `Some(blocked_outcome)` when the call must not run.
pub async fn gate(
    ctx: &ToolContext,
    action: &str,
    resources: &[String],
    label: &str,
) -> Option<ToolOutcome> {
    let key = session_key(action, resources);
    if ctx.approvals.is_session_allowed(&key) {
        return None;
    }
    match effect(ctx, action, resources).await {
        Effect::Allow => None,
        Effect::Deny => Some(ToolOutcome::err(format!(
            "[BLOCKED] {action} denied by permissions.json ({label}). Change the request or update the rule."
        ))),
        Effect::Ask => {
            if stark_policy::auto_approve(&ctx.harness_root) {
                return None;
            }
            if ctx.depth > 0 {
                return Some(ToolOutcome::err(format!(
                    "[BLOCKED] Subagents run non-interactively; {action} on {label} requires user approval."
                )));
            }
            match ask_user(ctx, action, label).await {
                AskOutcome::Once => None,
                AskOutcome::Session => {
                    ctx.approvals.allow_session(&key);
                    None
                }
                AskOutcome::Deny => Some(ToolOutcome::err(format!(
                    "[BLOCKED] {action} on {label} was not approved."
                ))),
            }
        }
    }
}

enum AskOutcome {
    Once,
    Session,
    Deny,
}

async fn ask_user(ctx: &ToolContext, action: &str, label: &str) -> AskOutcome {
    let question = json!({
        "id": format!("permission-{action}"),
        "header": "Permission",
        "question": format!("Allow this {action}?\n\n{label}"),
        "options": ["Allow once", "Allow for session", "Deny"],
    });
    let Ok(answers) = ctx.questions.ask(&ctx.chat_id, vec![question], 300).await else {
        return AskOutcome::Deny;
    };
    let selected = answers.iter().find_map(|answer| {
        answer
            .get("selected")
            .and_then(serde_json::Value::as_array)
            .and_then(|selected| selected.first())
            .and_then(serde_json::Value::as_str)
    });
    match selected {
        Some("Allow once") => AskOutcome::Once,
        Some("Allow for session") => AskOutcome::Session,
        _ => AskOutcome::Deny,
    }
}
