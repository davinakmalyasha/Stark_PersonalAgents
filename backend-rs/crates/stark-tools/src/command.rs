use std::time::Instant;

use serde_json::Value;

use crate::paths::resolve_workspace_path;
use crate::processes::ProcessStatus;
use crate::{ToolContext, ToolOutcome};

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
const DEFAULT_YIELD_MS: u64 = 10_000;
const MAX_YIELD_MS: u64 = 30_000;
const MAX_OUTPUT_CHARS: usize = 100_000;

pub async fn run_command(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let command = args.get("command").and_then(Value::as_str).unwrap_or("");
    let process_action = args
        .get("process_action")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();

    if !process_action.is_empty() {
        return handle_process_action(ctx, process_action, command, args).await;
    }

    if command.trim().is_empty() {
        return ToolOutcome::err("Error: run_command requires command");
    }
    let policy = stark_policy::evaluate_command(command);
    if matches!(policy.decision, stark_policy::Decision::Forbidden) {
        return ToolOutcome::err(format!(
            "[BLOCKED] Command denied by policy: {}",
            policy.reason.unwrap_or_else(|| "forbidden".to_string())
        ));
    }
    let prefix_rule: Vec<String> = args
        .get("prefix_rule")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let escalated = args
        .get("sandbox_permissions")
        .and_then(Value::as_str)
        .is_some_and(|value| value == "require_escalated");
    let needs_approval = (matches!(policy.decision, stark_policy::Decision::Prompt) || escalated)
        && !ctx.approvals.is_approved(command);
    if needs_approval {
        let justification = args
            .get("justification")
            .and_then(Value::as_str)
            .or(policy.reason.as_deref());
        if !request_command_approval(ctx, command, justification).await {
            return ToolOutcome::err(format!(
                "[BLOCKED] Command requires approval: {}",
                policy.reason.unwrap_or_else(|| "prompt".to_string())
            ));
        }
        if !prefix_rule.is_empty() {
            ctx.approvals.approve(&prefix_rule);
        }
    }
    let workdir = match args.get("workdir").and_then(Value::as_str) {
        Some(raw) if !raw.trim().is_empty() => match resolve_workspace_path(&ctx.workspace, raw) {
            Ok(path) => path,
            Err(error) => return ToolOutcome::err(format!("Error: {error}")),
        },
        _ => ctx.workspace.clone(),
    };
    let tty = args.get("tty").and_then(Value::as_bool).unwrap_or(false);

    if args
        .get("background")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let spawned = match ctx
            .processes
            .spawn_background(command, &workdir, &ctx.chat_id, tty)
        {
            Ok(value) => value,
            Err(error) => return ToolOutcome::err(format!("Error: {error}")),
        };
        let mut note = format!(
            "[OK] Started background process '{}' (session #{}, PID {}).",
            spawned.name, spawned.id, spawned.pid
        );
        if let Some(expect) = args
            .get("expect")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            let ready_timeout = args
                .get("ready_timeout")
                .and_then(Value::as_u64)
                .unwrap_or(15_000);
            match ctx
                .processes
                .wait_ready(&spawned.id.to_string(), expect, ready_timeout)
                .await
            {
                Ok(ready) => note.push_str(&format!("\n{ready}")),
                Err(error) => note.push_str(&format!("\n[READY_TIMEOUT] {error}")),
            }
        }
        return ToolOutcome::ok(note);
    }

    let mut timeout_ms = args
        .get("timeout")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_TIMEOUT_MS);
    if timeout_ms < 1000 {
        timeout_ms *= 1000;
    }
    timeout_ms = timeout_ms.min(MAX_TIMEOUT_MS);
    let explicit_yield = args.get("yield_time_ms").and_then(Value::as_u64);
    let yield_ms = explicit_yield
        .map(|value| value.clamp(250, MAX_YIELD_MS))
        .unwrap_or(DEFAULT_YIELD_MS);
    let started = Instant::now();

    let spawned = match ctx
        .processes
        .spawn_session(command, &workdir, &ctx.chat_id, tty)
    {
        Ok(value) => value,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };
    let session = spawned.id.to_string();
    let wait_ms = if explicit_yield.is_some() {
        yield_ms
    } else {
        timeout_ms
    };
    let (status, output) = match ctx.processes.wait_status(&session, wait_ms).await {
        Ok(value) => value,
        Err(error) => {
            ctx.processes.remove(&session);
            return ToolOutcome::err(format!("Error: {error}"));
        }
    };
    let elapsed = started.elapsed().as_millis() as u64;

    match status {
        ProcessStatus::Exited(code) => {
            ctx.processes.remove(&session);
            let text = truncate(output);
            let code = code.unwrap_or(-1);
            if code == 0 {
                ToolOutcome::with_code(format!("[OK] Exit 0 ({elapsed}ms)\n{text}"), false, Some(0))
            } else {
                ToolOutcome::with_code(
                    format!("[ERR] Exit {code} ({elapsed}ms)\n{text}"),
                    true,
                    Some(code),
                )
            }
        }
        ProcessStatus::Killed => {
            ctx.processes.remove(&session);
            ToolOutcome::err(format!("[KILLED] Process {session}\n{}", truncate(output)))
        }
        ProcessStatus::Running => {
            if explicit_yield.is_some() {
                ctx.processes.mark_read(&session);
                ToolOutcome::ok(format!(
                    "[RUNNING] session #{session} still running after {elapsed}ms\n{}",
                    truncate(output)
                ))
            } else {
                let _ = ctx.processes.kill(&session);
                ctx.processes.remove(&session);
                ToolOutcome::err(format!(
                    "[TIMEOUT] Command exceeded {timeout_ms}ms\n{command}\n{}",
                    truncate(output)
                ))
            }
        }
    }
}

async fn request_command_approval(ctx: &ToolContext, command: &str, reason: Option<&str>) -> bool {
    let question = serde_json::json!({
        "id": "command-approval",
        "question": format!("Allow this command?\n\n{command}"),
        "header": "Approval",
        "detail": reason.unwrap_or("This command requires explicit approval."),
        "options": ["Approve", "Deny"],
        "intent": {"kind": "plan-review", "approve": "Approve"}
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

async fn handle_process_action(
    ctx: &ToolContext,
    action: &str,
    command: &str,
    args: &Value,
) -> ToolOutcome {
    match action {
        "list" => ToolOutcome::ok(ctx.processes.list()),
        "kill_all" => {
            let count = ctx.processes.kill_all();
            ToolOutcome::ok(if count == 0 {
                "No running processes to kill.".to_string()
            } else {
                format!("[OK] Killed {count} process(es).")
            })
        }
        "logs" => {
            if command.trim().is_empty() {
                return ToolOutcome::err("Error: process_action='logs' requires command");
            }
            let tail = args
                .get("tail_lines")
                .and_then(Value::as_u64)
                .unwrap_or(200) as usize;
            match ctx.processes.logs(command, tail) {
                Ok(logs) => ToolOutcome::ok(logs),
                Err(error) => ToolOutcome::err(format!("Error: {error}")),
            }
        }
        "kill" => {
            if command.trim().is_empty() {
                return ToolOutcome::err("Error: process_action='kill' requires command");
            }
            match ctx.processes.kill(command) {
                Ok(()) => ToolOutcome::ok(format!("[OK] Killed {command}")),
                Err(error) => ToolOutcome::err(format!("Error: {error}")),
            }
        }
        "wait" => {
            if command.trim().is_empty() {
                return ToolOutcome::err("Error: process_action='wait' requires command");
            }
            let timeout = args
                .get("timeout")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_TIMEOUT_MS);
            match ctx.processes.wait(command, timeout).await {
                Ok(message) => ToolOutcome::ok(format!("[OK] {message}")),
                Err(error) => ToolOutcome::err(format!("[TIMEOUT] {error}")),
            }
        }
        "ready" => {
            if command.trim().is_empty() {
                return ToolOutcome::err("Error: process_action='ready' requires command");
            }
            let Some(expect) = args
                .get("expect")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
            else {
                return ToolOutcome::err("Error: process_action='ready' requires expect (regex)");
            };
            let ready_timeout = args
                .get("ready_timeout")
                .and_then(Value::as_u64)
                .unwrap_or(15_000);
            match ctx
                .processes
                .wait_ready(command, expect, ready_timeout)
                .await
            {
                Ok(message) => ToolOutcome::ok(message),
                Err(error) => ToolOutcome::err(format!("[NOT READY] {error}")),
            }
        }
        "send" => {
            if command.trim().is_empty() {
                return ToolOutcome::err("Error: process_action='send' requires command");
            }
            let input = args.get("input").and_then(Value::as_str).unwrap_or("");
            let default_yield = if input.is_empty() { 5_000 } else { 250 };
            let yield_ms = args
                .get("yield_time_ms")
                .and_then(Value::as_u64)
                .unwrap_or(default_yield)
                .clamp(250, MAX_YIELD_MS);
            match ctx.processes.send(command, input, yield_ms).await {
                Ok((status, output)) => {
                    let mut text = format!("[OK] Session {command} — {}", status_label(&status));
                    if !output.trim().is_empty() {
                        text.push('\n');
                        text.push_str(&truncate(output));
                    }
                    if matches!(status, ProcessStatus::Exited(_)) {
                        ctx.processes.remove(command);
                    }
                    ToolOutcome::ok(text)
                }
                Err(error) => ToolOutcome::err(format!("Error: {error}")),
            }
        }
        other => ToolOutcome::err(format!(
            "Error: unknown process_action '{other}' (list|logs|kill|wait|ready|send|kill_all)"
        )),
    }
}

fn status_label(status: &ProcessStatus) -> String {
    match status {
        ProcessStatus::Running => "running".to_string(),
        ProcessStatus::Exited(Some(code)) => format!("exited({code})"),
        ProcessStatus::Exited(None) => "exited(unknown)".to_string(),
        ProcessStatus::Killed => "killed".to_string(),
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
