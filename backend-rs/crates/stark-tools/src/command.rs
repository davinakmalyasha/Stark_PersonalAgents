use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::process::Command;

use crate::paths::resolve_workspace_path;
use crate::{ToolContext, ToolOutcome};

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
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
    match policy.decision {
        stark_policy::Decision::Forbidden => {
            return ToolOutcome::err(format!(
                "[BLOCKED] Command denied by policy: {}",
                policy.reason.unwrap_or_else(|| "forbidden".to_string())
            ));
        }
        stark_policy::Decision::Prompt => {
            if !request_command_approval(ctx, command, policy.reason.as_deref()).await {
                return ToolOutcome::err(format!(
                    "[BLOCKED] Command requires approval: {}",
                    policy.reason.unwrap_or_else(|| "prompt".to_string())
                ));
            }
        }
        stark_policy::Decision::Allow => {}
    }
    let workdir = match args.get("workdir").and_then(Value::as_str) {
        Some(raw) if !raw.trim().is_empty() => match resolve_workspace_path(&ctx.workspace, raw) {
            Ok(path) => path,
            Err(error) => return ToolOutcome::err(format!("Error: {error}")),
        },
        _ => ctx.workspace.clone(),
    };

    let mut timeout_ms = args
        .get("timeout")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_TIMEOUT_MS);
    if timeout_ms < 1000 {
        timeout_ms *= 1000;
    }
    timeout_ms = timeout_ms.min(MAX_TIMEOUT_MS);

    if args
        .get("background")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let (name, pid) = match ctx.processes.spawn(command, &workdir, &ctx.chat_id) {
            Ok(value) => value,
            Err(error) => return ToolOutcome::err(format!("Error: {error}")),
        };
        let mut note = format!("[OK] Started background process '{name}' (PID {pid}).");
        if let Some(expect) = args
            .get("expect")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            let ready_timeout = args
                .get("ready_timeout")
                .and_then(Value::as_u64)
                .unwrap_or(15_000);
            match ctx.processes.wait_ready(&name, expect, ready_timeout).await {
                Ok(ready) => note.push_str(&format!("\n{ready}")),
                Err(error) => note.push_str(&format!("\n[READY_TIMEOUT] {error}")),
            }
        }
        return ToolOutcome::ok(note);
    }

    let bash = find_bash();
    let started = Instant::now();
    let mut process = Command::new(&bash);
    process
        .arg("-lc")
        .arg(command)
        .current_dir(&workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let child = match process.spawn() {
        Ok(child) => child,
        Err(error) => return ToolOutcome::err(format!("Error: failed to start shell: {error}")),
    };

    let result =
        tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait_with_output()).await;
    let elapsed = started.elapsed().as_millis() as u64;

    match result {
        Ok(Ok(output)) => {
            let code = output.status.code().unwrap_or(-1);
            let mut text = String::from_utf8_lossy(&output.stdout).to_string();
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            let text = truncate(text);
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
        Ok(Err(error)) => ToolOutcome::err(format!("Error: shell failed: {error}")),
        Err(_) => ToolOutcome::err(format!(
            "[TIMEOUT] Command exceeded {timeout_ms}ms\n{command}"
        )),
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
    match ctx.questions.ask(vec![question], 300).await {
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
                "No background processes to kill.".to_string()
            } else {
                format!("[OK] Killed {count} background process(es).")
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
            match ctx.processes.send(command, input).await {
                Ok(()) => ToolOutcome::ok(format!("[OK] Sent input to {command}")),
                Err(error) => ToolOutcome::err(format!("Error: {error}")),
            }
        }
        other => ToolOutcome::err(format!(
            "Error: unknown process_action '{other}' (list|logs|kill|wait|ready|send|kill_all)"
        )),
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

fn find_bash() -> PathBuf {
    if let Ok(path) = std::env::var("STARK_BASH")
        && Path::new(&path).is_file()
    {
        return PathBuf::from(path);
    }
    for path in [
        r"C:\Program Files\Git\bin\bash.exe",
        r"C:\Program Files\Git\usr\bin\bash.exe",
        r"C:\Program Files (x86)\Git\bin\bash.exe",
    ] {
        if Path::new(path).is_file() {
            return PathBuf::from(path);
        }
    }
    PathBuf::from("bash")
}
