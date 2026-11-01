//! Developer prompt: OpenCode v2 system shape, adapted for Stark.
//!
//! Block 0 is adapted from OpenCode's `system.txt` (identity, Harness,
//! Communication, Working-in-codebases) with a software-engineering slant.
//! Most behavior lives in the tool descriptions. Stable content comes first
//! and volatile content last so prefix-cache reuse survives plan switches:
//! identity → persona → AGENTS.md → skills → plan → subagent note → runtime.

use chrono::Local;

use crate::instructions;

const IDENTITY: &str = "You are an AI agent powered by Stark.";

const HARNESS: &str = r#"# Harness
- Responses are rendered as Markdown.
- `<system-reminder>` blocks are harness instructions, not user-authored content. Read and follow them.
- Prefer parallelizing independent tool calls.
- Check the [exit code: N] marker on every bash result; investigate failures before moving on."#;

const COMMUNICATION: &str = r#"# Communication
- Use clear file paths when referring to files.
- Keep responses clear and concise, and avoid unnecessary technical jargon."#;

const WORKING_IN_CODEBASES: &str = r#"# Working in codebases
- Keep changes consistent with the structure, naming, style, and patterns of the surrounding code.
- Treat unfamiliar files or changes as potential user work and investigate before deleting or overwriting them.
- Read a file before editing or overwriting it. Use exact literal matches for edits.
- Verify changes with the project's tests or build when available.
- Persist until the task is fully resolved: implement, verify, then report. Do not stop at analysis.
- Never run destructive commands (git reset --hard, git checkout --, rm -rf) unless explicitly requested."#;

const SUBAGENT_NOTE: &str = "You are a subagent. Your final message is your report to the parent: state what changed, the files you touched, verification evidence, and unresolved risks. Do not ask the user questions; report blockers to the parent.";

const PLAN_SECTION: &str = r#"You are in plan mode. Stay in plan mode until the user approves your plan or switches the session mode. Imperative language to implement changes means plan the implementation, not execute it. A user's conversational agreement — including an answer confirming something you asked — approves nothing and does not end plan mode; fold the confirmed decision into the plan and present it for approval.

Explore first. Use non-mutating reads, searches, static analysis, and checks to ground the plan in the actual repository. Do not edit or write files, change configuration, run formatters or code generation that rewrites tracked files, commit, or otherwise carry out the plan. Prefer existing functions and patterns over new machinery.

The tool catalog stays the same across modes for request-cache stability. These plan-mode rules override any later tool description or guidance that suggests using mutation tools; those tools remain listed only to keep the request shape stable. Writes are blocked at dispatch with [BLOCKED]. Do not use todo_write to track this planning phase: it tracks implementation after an approved plan, while the plan itself belongs in the approval request.

Resolve discoverable facts by inspection. Use ask_user_question only for user-owned choices or material ambiguity that inspection cannot answer. Do not ask the user where code lives or how current behavior works when you can find out.

Make the plan decision-complete: state the goal and success criteria; group implementation changes by subsystem; identify public API, schema, and data-flow changes; cover edge cases, failure modes, tests, acceptance criteria, and explicit assumptions. Keep it concise enough to review but detailed enough that another engineer can implement it without making design decisions.

When ready, call ask_user_question with the complete plan markdown (starting with a # title) in the question detail and options ["Approve plan", "Keep planning"] with intent {"kind": "plan-review", "approve": "Approve plan"}. Make it the only and final tool call in that assistant response: approval flips the chat to build mode and implementation begins on a later turn; rejection keeps plan mode and returns feedback to incorporate. If the review channel is unavailable or aborted, stay in plan mode and ask the user to switch modes manually; do not proceed with implementation."#;

pub const CODEMODE_SECTION: &str = r#"# Program tool calls
You have `run_code`: one call runs a JavaScript program that calls Stark tools for you and returns one compact value. Prefer it when the work is a predictable multi-step sequence (read several files, compute over the results, edit, verify) and the program can return a smaller result than the raw tool outputs. Keep using direct tool calls for single lookups, for exploration where each result steers the next step, and for anything needing approval.

Inside a program: tools are synchronous on the global `tools` object (`const text = tools.read({ file_path: "src/lib.rs" });`), and `search("query")` lists available tools by name and description. Return one compact JSON-serializable value; intermediate tool output never enters the conversation, so do the filtering inside the program. Programs have no filesystem, network, process, or import access except through tools. Subagents, user questions, jobs, schedules, and canvas stay outside programs: call those tools directly, never from inside `run_code`."#;

fn platform_label() -> &'static str {
    if cfg!(windows) {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else {
        "Linux"
    }
}

#[cfg(test)]
pub async fn system_prompt(
    workspace: &std::path::Path,
    harness_root: &std::path::Path,
    model: &str,
    mode: &str,
    depth: u32,
    code_mode: bool,
) -> String {
    system_prompt_for_agent(workspace, harness_root, model, mode, depth, code_mode, None).await
}

/// Same prompt, with the agent's configured skill allowlist.
pub async fn system_prompt_for_agent(
    workspace: &std::path::Path,
    harness_root: &std::path::Path,
    model: &str,
    mode: &str,
    depth: u32,
    code_mode: bool,
    skills: Option<&[String]>,
) -> String {
    // Stable first: identity, persona, project instructions, skills.
    let mut prompt = format!(
        "{IDENTITY}\n\nYou are a coding agent powered by the {model} model, helping the user accomplish their software engineering goals.\n\n{HARNESS}\n\n{COMMUNICATION}\n\n{WORKING_IN_CODEBASES}"
    );
    if let Some(instructions) = instructions::load(harness_root, workspace) {
        prompt.push_str("\n\n");
        prompt.push_str(&instructions);
    }
    if let Some(skills) =
        stark_tools::catalog_block_for_agent(workspace, harness_root, skills).await
    {
        prompt.push_str("\n\n");
        prompt.push_str(&skills);
    }
    // Mode- and depth-varying sections stay after the stable prefix.
    if mode == "plan" {
        prompt.push_str("\n\n");
        prompt.push_str(PLAN_SECTION);
    }
    if depth > 0 {
        prompt.push_str("\n\n");
        prompt.push_str(SUBAGENT_NOTE);
    }
    if code_mode {
        prompt.push_str("\n\n");
        prompt.push_str(CODEMODE_SECTION);
    }
    // Volatile last: the date changes daily, so the tail recomputes at most
    // once a day instead of breaking the cached prefix every turn.
    let date = Local::now().format("%Y-%m-%d");
    prompt.push_str(&format!(
        "\n\nCurrent runtime context. This snapshot supersedes earlier runtime-context snapshots.\n\n\
         Working directory: {}\nPlatform: {}\nShell: bash (`bash -c`)\nCurrent date: {date}\n\
         File access: writes are not OS-sandboxed; stay inside the project unless the task requires otherwise.\n\
         {sandbox}\n\
         Approvals: destructive shell commands may pause for user approval.\n\
         Relative paths resolve from the working directory; use bash workdir for per-command directories.",
        workspace.to_string_lossy(),
        platform_label(),
        sandbox = stark_tools::sandbox_prompt_line(workspace)
    ));
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
        let prompt = system_prompt(
            dir.path(),
            dir.path(),
            "deepseek-v4.1-flash",
            "build",
            0,
            false,
        )
        .await;
        assert!(prompt.starts_with("You are an AI agent powered by Stark."));
        assert!(prompt.contains("powered by the deepseek-v4.1-flash model"));
        assert!(prompt.contains("Shell: bash (`bash -c`)"));
        assert!(prompt.contains("Project rule."));
        assert!(prompt.contains("review: Review code."));
        assert!(!prompt.contains("[WORKSPACE MEMORY]"));
        assert!(!prompt.contains("prefer run_code"));
        // OpenCode-adapted Block 0 with the software-engineering slant.
        assert!(prompt.contains("# Harness"));
        assert!(prompt.contains("# Communication"));
        assert!(prompt.contains("# Working in codebases"));
        assert!(prompt.contains("software engineering goals"));
        assert!(prompt.contains("Persist until the task is fully resolved"));
        assert!(prompt.contains("Never run destructive commands"));
        assert!(prompt.contains("Verify changes with the project's tests or build"));
        // Stable content precedes the volatile runtime tail.
        let harness = prompt.find("# Harness").unwrap();
        let instructions = prompt.find("Project rule.").unwrap();
        let skills = prompt.find("review: Review code.").unwrap();
        let runtime = prompt.find("Current runtime context.").unwrap();
        assert!(harness < instructions && instructions < skills && skills < runtime);
        // Day-precision date keeps the cached prefix stable intraday.
        assert!(prompt.contains("Current date:"));
        assert!(!prompt.contains("Current time:"));
    }

    #[tokio::test]
    async fn plan_prompt_uses_ask_user_question_for_approval() {
        let dir = tempfile::tempdir().unwrap();
        let prompt = system_prompt(dir.path(), dir.path(), "model", "plan", 0, false).await;
        assert!(prompt.contains("ask_user_question"));
        assert!(prompt.contains("plan-review"));
        assert!(!prompt.contains("exit_plan_mode"));
        assert!(prompt.contains("Do not use todo_write"));
        // Plan section sits after stable content, before the runtime tail.
        let plan = prompt.find("You are in plan mode.").unwrap();
        let runtime = prompt.find("Current runtime context.").unwrap();
        assert!(plan < runtime);
    }

    #[tokio::test]
    async fn subagent_note_comes_before_the_runtime_tail() {
        let dir = tempfile::tempdir().unwrap();
        let prompt = system_prompt(dir.path(), dir.path(), "model", "build", 1, false).await;
        assert!(prompt.contains("Your final message is your report to the parent"));
        let note = prompt.find("Your final message is your report").unwrap();
        let runtime = prompt.find("Current runtime context.").unwrap();
        assert!(note < runtime);
    }

    #[tokio::test]
    async fn codemode_section_only_with_flag_and_before_tail() {
        let dir = tempfile::tempdir().unwrap();
        let plain = system_prompt(dir.path(), dir.path(), "model", "build", 0, false).await;
        assert!(!plain.contains("# Program tool calls"));
        let coded = system_prompt(dir.path(), dir.path(), "model", "build", 0, true).await;
        assert!(coded.contains("# Program tool calls"));
        assert!(coded.contains("`run_code`"));
        assert!(coded.contains("call those tools directly, never from inside `run_code`"));
        let section = coded.find("# Program tool calls").unwrap();
        let runtime = coded.find("Current runtime context.").unwrap();
        assert!(section < runtime);
    }
}
