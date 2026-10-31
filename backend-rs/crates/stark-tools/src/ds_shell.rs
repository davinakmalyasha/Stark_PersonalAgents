//! DeepSeek Harness-compatible shell and background-job tools.

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::paths::resolve_workspace_path;
use crate::processes::ProcessStatus;
use crate::{ToolContext, ToolOutcome};

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
const MAX_OUTPUT_CHARS: usize = 100_000;

fn resolve_workdir(ctx: &ToolContext, raw: Option<&str>) -> Result<PathBuf, String> {
    match raw.map(str::trim).filter(|raw| !raw.is_empty()) {
        Some(raw) => {
            let candidate = PathBuf::from(raw);
            let joined = if candidate.is_absolute() {
                candidate
            } else {
                ctx.current_dir().join(candidate)
            };
            resolve_workspace_path(&ctx.workspace, &joined.to_string_lossy())
        }
        None => Ok(ctx.current_dir()),
    }
}

fn truncate(text: String) -> String {
    if text.chars().count() <= MAX_OUTPUT_CHARS {
        return text;
    }
    let mut out: String = text.chars().take(MAX_OUTPUT_CHARS).collect();
    out.push_str("\n[output truncated at 100000 chars]");
    out
}

async fn approve_command(ctx: &ToolContext, command: &str, reason: &str) -> bool {
    let question = json!({
        "id": "bash-approval",
        "question": format!("Allow this command?\n\n{command}"),
        "header": "Approval",
        "detail": reason,
        "options": ["Approve", "Deny"],
    });
    match ctx.questions.ask(&ctx.chat_id, vec![question], 300).await {
        Ok(answers) => answers.iter().any(|answer| {
            answer
                .get("selected")
                .and_then(Value::as_array)
                .is_some_and(|selected| {
                    selected
                        .iter()
                        .any(|value| value.as_str() == Some("Approve"))
                })
        }),
        Err(_) => false,
    }
}

pub async fn bash(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let Some(command) = args.get("command").and_then(Value::as_str) else {
        return ToolOutcome::err("Error: bash requires command");
    };
    let Some(description) = args.get("description").and_then(Value::as_str) else {
        return ToolOutcome::err("Error: bash requires description");
    };
    if command.trim().is_empty() {
        return ToolOutcome::err("Error: bash requires a non-empty command");
    }
    if description.trim().is_empty() {
        return ToolOutcome::err("Error: bash requires a non-empty description");
    }
    let policy = stark_policy::evaluate_command(command);
    let permission = crate::permissions::effect(ctx, "bash", &[command.to_string()]);
    if matches!(permission, stark_policy::Effect::Deny) {
        return ToolOutcome::err(format!(
            "[BLOCKED] Command denied by permissions.json: {command}"
        ));
    }
    if matches!(policy.decision, stark_policy::Decision::Forbidden) {
        return ToolOutcome::err(format!(
            "[BLOCKED] Command denied by policy: {}",
            policy.reason.unwrap_or_else(|| "forbidden".to_string())
        ));
    }
    if (matches!(policy.decision, stark_policy::Decision::Prompt)
        || matches!(permission, stark_policy::Effect::Ask))
        && !ctx.approvals.is_approved(command)
    {
        if ctx.depth > 0 {
            return ToolOutcome::err(format!(
                "[BLOCKED] Subagents run non-interactively; command requires approval: {}",
                policy.reason.unwrap_or_else(|| "prompt".to_string())
            ));
        }
        if !approve_command(
            ctx,
            command,
            policy
                .reason
                .as_deref()
                .unwrap_or("This command requires approval."),
        )
        .await
        {
            return ToolOutcome::err(format!(
                "[BLOCKED] Command requires approval: {}",
                policy.reason.unwrap_or_else(|| "prompt".to_string())
            ));
        }
    }
    let workdir = match resolve_workdir(ctx, args.get("workdir").and_then(Value::as_str)) {
        Ok(path) => path,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };
    if args
        .get("run_in_background")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let spawned = match ctx
            .processes
            .spawn_session(command, &workdir, &ctx.chat_id, false)
        {
            Ok(value) => value,
            Err(error) => return ToolOutcome::err(format!("Error: {error}")),
        };
        return ToolOutcome::ok(
            json!({
                "kind": "background",
                "jobId": spawned.id.to_string(),
                "cwd": workdir.to_string_lossy(),
                "pid": spawned.pid,
            })
            .to_string(),
        );
    }

    let timeout_ms = args
        .get("timeoutMs")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .clamp(1_000, MAX_TIMEOUT_MS);
    let spawned = match ctx
        .processes
        .spawn_session(command, &workdir, &ctx.chat_id, false)
    {
        Ok(value) => value,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };
    let job = spawned.id.to_string();
    let (status, output) = match ctx.processes.wait_status(&job, timeout_ms).await {
        Ok(value) => value,
        Err(error) => {
            ctx.processes.remove(&job);
            return ToolOutcome::err(format!("Error: {error}"));
        }
    };
    match status {
        ProcessStatus::Exited(code) => {
            ctx.processes.remove(&job);
            let code = code.unwrap_or(-1);
            let text = truncate(output);
            if code == 0 {
                ToolOutcome::ok(format!("{text}\n[exit code: 0]"))
            } else {
                ToolOutcome::err(format!("{text}\n[exit code: {code}]"))
            }
        }
        ProcessStatus::Killed => {
            ctx.processes.remove(&job);
            ToolOutcome::err(format!("[killed]\n{}", truncate(output)))
        }
        ProcessStatus::Running => ToolOutcome::ok(
            json!({
                "kind": "promoted",
                "jobId": job,
                "cwd": workdir.to_string_lossy(),
                "timeoutMs": timeout_ms,
                "output": truncate(output),
            })
            .to_string(),
        ),
    }
}

pub async fn job(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let Some(action) = args.get("action").and_then(Value::as_str) else {
        return ToolOutcome::err("Error: job requires action");
    };
    match action {
        "list" => ToolOutcome::ok(ctx.processes.list_for(&ctx.chat_id)),
        "output" => {
            let Some(job_id) = args.get("job_id").and_then(Value::as_str) else {
                return ToolOutcome::err("Error: job output requires job_id");
            };
            if args.get("wait").and_then(Value::as_bool).unwrap_or(false) {
                let timeout = args
                    .get("timeout_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(30_000)
                    .min(3_600_000);
                let _ = ctx.processes.wait_status(job_id, timeout).await;
            }
            match ctx.processes.read_output(job_id) {
                Ok(text) => ToolOutcome::ok(text),
                Err(error) => ToolOutcome::err(format!("Error: {error}")),
            }
        }
        "kill" => {
            let Some(job_id) = args.get("job_id").and_then(Value::as_str) else {
                return ToolOutcome::err("Error: job kill requires job_id");
            };
            let reason = args
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("requested by the agent");
            match ctx.processes.kill(job_id) {
                Ok(()) => {
                    ToolOutcome::ok(format!("[OK] Kill requested for job {job_id} ({reason})"))
                }
                Err(error) => ToolOutcome::err(format!("Error: {error}")),
            }
        }
        other => ToolOutcome::err(format!(
            "Error: unknown job action '{other}' (use list | output | kill)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::QuestionBroker;

    fn context(dir: &std::path::Path) -> ToolContext {
        ToolContext::new(dir, "v2-shell", Arc::new(QuestionBroker::new()))
    }

    #[tokio::test]
    async fn denied_bash_never_spawns() {
        let dir = tempfile::tempdir().unwrap();
        let rules = dir.path().join(".deep_agents");
        std::fs::create_dir_all(&rules).unwrap();
        std::fs::write(
            rules.join("permissions.json"),
            r#"{"permissions":[{"action":"bash","resource":"*","effect":"deny"}]}"#,
        )
        .unwrap();
        let ctx = context(dir.path()).with_harness_root(dir.path());
        let out = bash(
            &ctx,
            &json!({"description": "should not run", "command": "echo hi > marker.txt"}),
        )
        .await;
        assert!(out.is_error, "{}", out.content);
        assert!(out.content.contains("permissions.json"), "{}", out.content);
        // Proof the process layer was never reached.
        assert!(!dir.path().join("marker.txt").exists());
    }

    #[tokio::test]
    async fn bash_runs_and_reports_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let out = bash(
            &ctx,
            &json!({"description": "Print a value", "command": "printf 'hello\n'"}),
        )
        .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("hello"), "{}", out.content);
        assert!(out.content.contains("[exit code: 0]"), "{}", out.content);
    }

    #[tokio::test]
    async fn background_jobs_can_be_listed_read_and_killed() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let started = bash(
            &ctx,
            &json!({
                "description": "Sleep in the background",
                "command": "sleep 10",
                "run_in_background": true
            }),
        )
        .await;
        assert!(!started.is_error, "{}", started.content);
        let value: Value = serde_json::from_str(&started.content).unwrap();
        let job_id = value["jobId"].as_str().unwrap().to_string();
        let listed = job(&ctx, &json!({"action": "list"})).await;
        assert!(!listed.is_error, "{}", listed.content);
        assert!(listed.content.contains(&job_id), "{}", listed.content);
        let read = job(&ctx, &json!({"action": "output", "job_id": job_id})).await;
        assert!(!read.is_error, "{}", read.content);
        let killed = job(&ctx, &json!({"action": "kill", "job_id": job_id})).await;
        assert!(!killed.is_error, "{}", killed.content);
    }

    #[tokio::test]
    async fn job_rejects_unknown_actions_and_missing_ids() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let unknown = job(&ctx, &json!({"action": "stop"})).await;
        assert!(unknown.is_error);
        let missing = job(&ctx, &json!({"action": "kill"})).await;
        assert!(missing.is_error);
        assert!(missing.content.contains("job_id"));
    }
}
