//! DeepSeek Harness-compatible shell and background-job tools.

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::paths::resolve_workspace_path;
use crate::processes::ProcessStatus;
use crate::{ToolContext, ToolErrorClass, ToolOutcome};

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
/// Default model-facing output budget (~25k tokens at ~4 chars/token,
/// Claude-class). Overridable per call with Codex's `max_output_tokens`.
const MAX_OUTPUT_CHARS: usize = 100_000;
const MIN_OUTPUT_CHARS: usize = 1_000;
const MAX_OUTPUT_CHARS_CAP: usize = 200_000;

fn resolve_workdir(
    ctx: &ToolContext,
    raw: Option<&str>,
    allow_outside: bool,
) -> Result<PathBuf, String> {
    match raw.map(str::trim).filter(|raw| !raw.is_empty()) {
        Some(raw) => {
            let candidate = PathBuf::from(raw);
            let joined = if candidate.is_absolute() {
                candidate
            } else {
                ctx.current_dir().join(candidate)
            };
            if allow_outside {
                return Ok(joined.canonicalize().unwrap_or(joined));
            }
            resolve_workspace_path(&ctx.workspace, &joined.to_string_lossy())
        }
        None => Ok(ctx.current_dir()),
    }
}

fn truncate_with_limit(text: &str, limit: usize) -> String {
    let count = text.chars().count();
    if count <= limit {
        return text.to_string();
    }
    // Keep both ends: failures usually live at the tail, while the command
    // header lives at the head. Head-only truncation hid test failures.
    let head_budget = limit / 2;
    let tail_budget = limit - head_budget;
    let head: String = text.chars().take(head_budget).collect();
    let tail: String = text.chars().skip(count - tail_budget).collect();
    format!(
        "{head}\n[... {} chars omitted (showing {} head + {} tail of {} chars total). Request a narrower command or read the job log with job output.]\n{tail}",
        count.saturating_sub(limit),
        head_budget,
        tail_budget,
        count
    )
}

/// Codex shell contract: per-call `max_output_tokens` overrides the default
/// budget. Tokens convert at ~4 chars/token (same approximation Codex and the
/// OpenAI Agents SDK use). The default stays generous so routine logs are
/// never hidden from the model.
fn output_limit(args: &Value) -> usize {
    args.get("max_output_tokens")
        .and_then(Value::as_u64)
        .map(|tokens| {
            (tokens.saturating_mul(4) as usize).clamp(MIN_OUTPUT_CHARS, MAX_OUTPUT_CHARS_CAP)
        })
        .unwrap_or(MAX_OUTPUT_CHARS)
}

async fn approve_command(ctx: &ToolContext, command: &str, reason: &str) -> bool {
    let question = json!({
        "id": "bash-approval",
        "question": format!("Allow this command?\n\n{command}"),
        "header": "Approval",
        "detail": reason,
        "options": ["Approve", "Allow for session", "Always allow", "Deny"],
    });
    let Ok(answers) = ctx.questions.ask(&ctx.chat_id, vec![question], 300).await else {
        return false;
    };
    let selected = answers.iter().find_map(|answer| {
        answer
            .get("selected")
            .and_then(Value::as_array)
            .and_then(|selected| selected.first())
            .and_then(Value::as_str)
    });
    match selected {
        Some("Approve") => true,
        Some("Allow for session") => {
            if let Some(tokens) = shlex::split(command) {
                ctx.approvals.approve(&tokens);
            }
            true
        }
        Some("Always allow") => {
            if let Some(tokens) = shlex::split(command) {
                ctx.approvals.approve(&tokens);
                if let Err(error) = stark_policy::append_allow_prefix(&tokens) {
                    tracing::warn!("could not persist approved command prefix: {error}");
                }
            }
            true
        }
        _ => false,
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
    let permission = crate::permissions::effect(ctx, "bash", &[command.to_string()]).await;
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
    let auto = stark_policy::auto_approve(&ctx.harness_root);
    // Codex policy: reads may cross workspace boundaries; writes may not.
    // A verifiably read-only command (ls, cat, rg, git status, ...) is allowed
    // outside the workspace. Mutating commands still hit external_directory.
    let read_only_command = stark_policy::check_read_only(command).is_ok();
    let outside = crate::permissions::external_paths(command, &ctx.workspace);
    if !outside.is_empty() && !auto && !read_only_command {
        match crate::permissions::effect(ctx, "external_directory", &outside).await {
            stark_policy::Effect::Deny => {
                return ToolOutcome::err(format!(
                    "[BLOCKED] external_directory denied: {}",
                    outside.join(", ")
                ));
            }
            stark_policy::Effect::Ask => {
                if stark_sandbox::SandboxProfile::from_env(&ctx.workspace).approval
                    == stark_sandbox::ApprovalPolicy::Never
                {
                    return ToolOutcome::err(format!(
                        "[BLOCKED] external_directory requires approval, but approval policy is 'never': {}",
                        outside.join(", ")
                    ));
                }
                if ctx.depth > 0
                    || !approve_command(
                        ctx,
                        command,
                        "This command touches paths outside the workspace.",
                    )
                    .await
                {
                    return ToolOutcome::err(format!(
                        "[BLOCKED] external_directory requires approval: {}",
                        outside.join(", ")
                    ));
                }
            }
            stark_policy::Effect::Allow => {}
        }
    }
    if (matches!(policy.decision, stark_policy::Decision::Prompt)
        || matches!(permission, stark_policy::Effect::Ask))
        && !ctx.approvals.is_approved(command)
        && !auto
    {
        if stark_sandbox::SandboxProfile::from_env(&ctx.workspace).approval
            == stark_sandbox::ApprovalPolicy::Never
        {
            return ToolOutcome::err(format!(
                "[BLOCKED] Command requires approval, but approval policy is 'never': {}",
                policy.reason.unwrap_or_else(|| "prompt".to_string())
            ));
        }
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
    let workdir = match resolve_workdir(
        ctx,
        args.get("workdir").and_then(Value::as_str),
        read_only_command,
    ) {
        Ok(path) => path,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };
    if !workdir.is_dir() {
        return ToolOutcome::err(format!(
            "Error: workdir does not exist or is not a directory: {}. Create it first or omit workdir.",
            workdir.display()
        ));
    }
    if ctx.cancel.is_cancelled() {
        return crate::cancelled_outcome("bash");
    }
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
    let output_limit = output_limit(args);
    let spawned = match ctx
        .processes
        .spawn_session(command, &workdir, &ctx.chat_id, false)
    {
        Ok(value) => value,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };
    let job = spawned.id.to_string();
    // Dropping `wait_status` does not stop the child. On cancel, kill the
    // foreground job explicitly (Codex kills the whole process group on
    // cancellation, `core/src/exec.rs`) and report a cancelled result.
    let waited = tokio::select! {
        biased;
        _ = ctx.cancel.cancelled() => None,
        result = ctx.processes.wait_status(&job, timeout_ms) => Some(result),
    };
    let Some(waited) = waited else {
        let _ = ctx.processes.kill(&job);
        ctx.processes.remove(&job);
        return crate::cancelled_outcome("bash");
    };
    let (status, output) = match waited {
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
            let text = with_spill(ctx, &output, output_limit);
            if code == 0 {
                ToolOutcome::ok(format!("{text}\n[exit code: 0]"))
            } else {
                ToolOutcome::with_code(format!("{text}\n[exit code: {code}]"), true, Some(code))
                    .with_class(ToolErrorClass::Exec)
            }
        }
        ProcessStatus::Killed => {
            ctx.processes.remove(&job);
            ToolOutcome::err_class(
                ToolErrorClass::Exec,
                format!("[killed]\n{}", with_spill(ctx, &output, output_limit)),
            )
        }
        ProcessStatus::Running => ToolOutcome::ok(
            json!({
                "kind": "promoted",
                "jobId": job,
                "cwd": workdir.to_string_lossy(),
                "timeoutMs": timeout_ms,
                "output": with_spill(ctx, &output, output_limit),
            })
            .to_string(),
        ),
    }
}

/// Truncates to the call budget and, when truncation fires, spills the full
/// output to scratch so the marker can point the agent at the complete log
/// (what the bash contract already promises) instead of hiding data.
fn with_spill(ctx: &ToolContext, output: &str, limit: usize) -> String {
    let mut text = truncate_with_limit(output, limit);
    if output.chars().count() > limit
        && let Some(path) = crate::truncate::spill_full_output(&ctx.harness_root, "bash", output)
    {
        text.push_str(&format!(
            "\n[full output saved to {} — read it with an offset/limit]",
            path.to_string_lossy()
        ));
    }
    text
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
                let cancelled = tokio::select! {
                    biased;
                    _ = ctx.cancel.cancelled() => true,
                    _ = ctx.processes.wait_status(job_id, timeout) => false,
                };
                if cancelled {
                    return crate::cancelled_outcome("job output");
                }
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
    async fn missing_workdir_is_actionable() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let out = bash(
            &ctx,
            &json!({"description": "list", "command": "ls", "workdir": "missing-folder"}),
        )
        .await;
        assert!(out.is_error, "{}", out.content);
        assert!(
            out.content.contains("workdir does not exist"),
            "{}",
            out.content
        );
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

    #[tokio::test]
    async fn bash_cancel_kills_the_foreground_job() {
        use tokio_util::sync::CancellationToken;

        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let ctx = context(dir.path()).with_cancel(cancel.clone());
        let started = std::time::Instant::now();
        let task = tokio::spawn(async move {
            bash(
                &ctx,
                &json!({"description": "Sleep until cancelled", "command": "sleep 30"}),
            )
            .await
        });
        // Let the child spawn, then stop the run. Codex returns a
        // non-timeout exec result promptly on cancellation.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        cancel.cancel();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), task)
            .await
            .expect("cancelled bash must return promptly")
            .expect("bash task");
        assert!(outcome.is_error, "{}", outcome.content);
        assert_eq!(outcome.class, Some(ToolErrorClass::Cancelled));
        assert!(outcome.content.contains("cancelled"), "{}", outcome.content);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(15),
            "took {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn job_output_wait_is_cancel_safe() {
        use tokio_util::sync::CancellationToken;

        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let ctx = context(dir.path()).with_cancel(cancel.clone());
        let started = bash(
            &ctx,
            &json!({
                "description": "Sleep in the background",
                "command": "sleep 30",
                "run_in_background": true
            }),
        )
        .await;
        assert!(!started.is_error, "{}", started.content);
        let value: Value = serde_json::from_str(&started.content).unwrap();
        let job_id = value["jobId"].as_str().unwrap().to_string();
        let waiting = {
            let ctx = ctx.clone();
            tokio::spawn(async move {
                job(
                    &ctx,
                    &json!({"action": "output", "job_id": job_id, "wait": true, "timeout_ms": 30_000}),
                )
                .await
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        cancel.cancel();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), waiting)
            .await
            .expect("cancelled wait must return promptly")
            .expect("wait task");
        assert!(outcome.is_error, "{}", outcome.content);
        assert_eq!(outcome.class, Some(ToolErrorClass::Cancelled));
        // The wait was cancelled, not the job: clean up the sleeper so the
        // test does not leave a 30s child behind.
        let _ = ctx.processes.kill_all();
    }
}
