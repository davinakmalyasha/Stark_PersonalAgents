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
    if args
        .get("background")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return ToolOutcome::err(
            "Error: background commands are not implemented in the Rust Phase 1 backend",
        );
    }
    if args
        .get("process_action")
        .and_then(Value::as_str)
        .is_some_and(|action| !action.trim().is_empty())
    {
        return ToolOutcome::err(
            "Error: process_action is not implemented in the Rust Phase 1 backend",
        );
    }

    let Some(command) = args.get("command").and_then(Value::as_str) else {
        return ToolOutcome::err("Error: run_command requires command");
    };
    if let stark_policy::CommandDecision::Deny(reason) = stark_policy::classify_command(command) {
        return ToolOutcome::err(format!("[BLOCKED] Command denied by policy: {reason}"));
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
