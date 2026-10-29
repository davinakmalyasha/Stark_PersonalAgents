//! Developer v2 prompt: the DeepSeek Harness `standard` preset shape.
//!
//! Most behavior lives in the tool descriptions. This module owns the short
//! identity/persona/runtime/plan sections plus the project instruction block.

use chrono::Local;

use crate::instructions;

const IDENTITY: &str = "You are an AI agent powered by Stark.";

const PLAN_SECTION: &str = r#"You are in plan mode. Stay in plan mode until the user approves your plan through exit_plan_mode or switches the session mode. Imperative language to implement changes means plan the implementation, not execute it. A user's conversational agreement — including an answer confirming something you asked — approves nothing and does not end plan mode; fold the confirmed decision into the plan and submit it through exit_plan_mode.

Explore first. Use non-mutating reads, searches, static analysis, and checks to ground the plan in the actual repository. Do not edit or write files, change configuration, run formatters or code generation that rewrites tracked files, commit, or otherwise carry out the plan. Prefer existing functions and patterns over new machinery.

The tool catalog stays the same across modes for request-cache stability. These plan-mode rules override any later tool description or guidance that suggests using mutation tools; those tools remain listed only to keep the request shape stable. Do not use todo_write to track this planning phase: it tracks implementation after an approved plan, while the plan itself belongs in exit_plan_mode.

Resolve discoverable facts by inspection. Use ask_user_question only for user-owned choices or material ambiguity that inspection cannot answer. Do not ask the user where code lives or how current behavior works when you can find out.

Make the plan decision-complete: state the goal and success criteria; group implementation changes by subsystem; identify public API, schema, and data-flow changes; cover edge cases, failure modes, tests, acceptance criteria, and explicit assumptions. Keep it concise enough to review but detailed enough that another engineer can implement it without making design decisions.

When ready, call exit_plan_mode with the complete plan markdown, starting with a # title. Make exit_plan_mode the only and final tool call in that assistant response: it presents the plan for approval, and implementation begins only in a later step after approval. Do not paste the final plan as a plain reply or ask "should I proceed?" through prose or ask_user_question. If review rejects it, incorporate the feedback and present again. If the review channel is unavailable or aborted, stay in plan mode and ask the user to switch modes manually; do not proceed with implementation."#;

const TOOL_GUIDANCE: &str = r#"Tool guidance:
- Check the [exit code: N] marker on every bash result; investigate failures before moving on.
- Read a file before editing or overwriting it. Use exact literal matches for edits."#;

fn platform_label() -> &'static str {
    if cfg!(windows) {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else {
        "Linux"
    }
}

pub async fn system_prompt(
    workspace: &std::path::Path,
    harness_root: &std::path::Path,
    model: &str,
    mode: &str,
) -> String {
    let now = Local::now().format("%Y-%m-%d %H:%M:%S %Z");
    let mut prompt = format!(
        "{IDENTITY}\n\nYou are a coding agent powered by the {model} model.\n\n\
         Current runtime context. This snapshot supersedes earlier runtime-context snapshots.\n\n\
         Working directory: {}\nPlatform: {}\nShell: Git Bash (`bash -c`)\nCurrent time: {now}",
        workspace.to_string_lossy(),
        platform_label()
    );
    if mode == "plan" {
        prompt.push_str("\n\n");
        prompt.push_str(PLAN_SECTION);
    }
    prompt.push_str("\n\n");
    prompt.push_str(TOOL_GUIDANCE);
    if let Some(instructions) = instructions::load(harness_root, workspace) {
        prompt.push_str("\n\n");
        prompt.push_str(&instructions);
    }
    if let Some(skills) = stark_tools::catalog_block_for(workspace, harness_root).await {
        prompt.push_str("\n\n");
        prompt.push_str(&skills);
    }
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn developer_prompt_has_deepseek_shape_and_skills() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "Project rule.").unwrap();
        let skill_dir = dir.path().join(".agents/skills/review");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: review\ndescription: Review code.\n---\n# Review\n",
        )
        .unwrap();
        let prompt = system_prompt(dir.path(), dir.path(), "deepseek-v4.1-flash", "build").await;
        assert!(prompt.starts_with("You are an AI agent powered by Stark."));
        assert!(prompt.contains("powered by the deepseek-v4.1-flash model"));
        assert!(prompt.contains("Git Bash"));
        assert!(prompt.contains("Project rule."));
        assert!(prompt.contains("review: Review code."));
        assert!(!prompt.contains("[WORKSPACE MEMORY]"));
        assert!(!prompt.contains("prefer run_code"));
    }

    #[tokio::test]
    async fn plan_prompt_contains_exit_plan_mode_contract() {
        let dir = tempfile::tempdir().unwrap();
        let prompt = system_prompt(dir.path(), dir.path(), "model", "plan").await;
        assert!(prompt.contains("exit_plan_mode"));
        assert!(prompt.contains("Do not use todo_write"));
    }
}
