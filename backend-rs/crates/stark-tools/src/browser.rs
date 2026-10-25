use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use tokio::process::Command;

use crate::{ToolContext, ToolOutcome};

const MAX_SNAPSHOT_CHARS: usize = 24_000;
const DEFAULT_WAIT_MS: u64 = 5_000;

pub async fn browser_act(_ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("extract")
        .trim()
        .to_lowercase();
    match action.as_str() {
        "navigate" => navigate(args).await,
        "click" => {
            command(
                &["click", required(args, "selector")],
                Duration::from_secs(15),
            )
            .await
        }
        "fill" => {
            command(
                &["fill", required(args, "selector"), required(args, "text")],
                Duration::from_secs(15),
            )
            .await
        }
        "type" => {
            command(
                &["type", required(args, "selector"), required(args, "text")],
                Duration::from_secs(15),
            )
            .await
        }
        "press" => command(&["press", required(args, "key")], Duration::from_secs(15)).await,
        "extract" => extract(args).await,
        "screenshot" => screenshot(args).await,
        "close" => {
            let _ = raw_command(&["close", "--all"], Duration::from_secs(10)).await;
            ToolOutcome::ok("Browser closed.")
        }
        other => ToolOutcome::err(format!(
            "Error: unknown browser_act action '{other}'. Valid: navigate, click, fill, type, press, extract, screenshot, close"
        )),
    }
}

async fn navigate(args: &Value) -> ToolOutcome {
    let url = required(args, "url");
    let url = if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else {
        format!("https://{url}")
    };
    let open = raw_command(&["open", &url], Duration::from_secs(30)).await;
    if open.starts_with('(') {
        return ToolOutcome::ok(open);
    }
    let wait_ms = args
        .get("wait_ms")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_WAIT_MS)
        .min(3_000);
    tokio::time::sleep(Duration::from_millis(wait_ms)).await;
    let snapshot = raw_command(
        &["snapshot", "-i", "--max-output", "24000"],
        Duration::from_secs(20),
    )
    .await;
    let title = raw_command(&["get", "title"], Duration::from_secs(5)).await;
    let title = if title.to_lowercase().contains("empty") || title.to_lowercase().contains("error")
    {
        String::new()
    } else {
        title
    };
    let snapshot: String = snapshot.chars().take(MAX_SNAPSHOT_CHARS).collect();
    ToolOutcome::ok(format!(
        "[BROWSER] {url}\nTitle: {title}\n\nAccessibility Tree:\n{snapshot}"
    ))
}

async fn extract(args: &Value) -> ToolOutcome {
    let what = args
        .get("what")
        .and_then(Value::as_str)
        .unwrap_or("text")
        .trim()
        .to_lowercase();
    if !matches!(
        what.as_str(),
        "text" | "html" | "value" | "title" | "url" | "attr"
    ) {
        return ToolOutcome::err(format!(
            "(invalid: '{what}'. Valid: attr, html, text, title, url, value)"
        ));
    }
    let selector = args.get("selector").and_then(Value::as_str).unwrap_or("");
    if matches!(what.as_str(), "title" | "url") {
        return command(&["get", &what], Duration::from_secs(5)).await;
    }
    let selector = if selector.trim().is_empty() {
        "body"
    } else {
        selector
    };
    command(&["get", &what, selector], Duration::from_secs(15)).await
}

async fn screenshot(args: &Value) -> ToolOutcome {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let directory = std::env::temp_dir().join("agent-screenshots");
            let _ = std::fs::create_dir_all(&directory);
            directory.join(format!(
                "screenshot_{}.png",
                &uuid::Uuid::new_v4().simple().to_string()[..8]
            ))
        });
    let path_text = path.to_string_lossy().to_string();
    let result = raw_command(
        &["screenshot", "--full", &path_text],
        Duration::from_secs(20),
    )
    .await;
    if path.is_file() {
        let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        ToolOutcome::ok(format!("[SCREENSHOT] Saved to {path_text} ({size} bytes)"))
    } else {
        ToolOutcome::ok(format!("(screenshot result: {result})"))
    }
}

async fn command(args: &[&str], timeout: Duration) -> ToolOutcome {
    ToolOutcome::ok(raw_command(args, timeout).await)
}

async fn raw_command(args: &[&str], timeout: Duration) -> String {
    let mut command = browser_command();
    command.args(args);
    command.kill_on_drop(true);
    match tokio::time::timeout(timeout, command.output()).await {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !stdout.is_empty() {
                return stdout;
            }
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if stderr.is_empty() {
                "(empty)".to_string()
            } else {
                format!("(error: {stderr})")
            }
        }
        Ok(Err(error)) => format!("(error: {error})"),
        Err(_) => format!("(timed out after {}s)", timeout.as_secs()),
    }
}

fn browser_command() -> Command {
    if let Some(path) = windows_npm_binary() {
        return Command::new(path);
    }
    Command::new("agent-browser")
}

fn windows_npm_binary() -> Option<PathBuf> {
    let app_data = std::env::var_os("APPDATA")?;
    let path = Path::new(&app_data).join("npm").join("agent-browser.cmd");
    path.is_file().then_some(path)
}

fn required<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{QuestionBroker, ToolContext};
    use std::sync::Arc;

    #[tokio::test]
    async fn rejects_unknown_actions() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path(), "chat", Arc::new(QuestionBroker::new()));
        let outcome = browser_act(&ctx, &serde_json::json!({"action": "dance"})).await;
        assert!(outcome.is_error);
        assert!(outcome.content.contains("unknown browser_act action"));
    }

    #[test]
    fn locates_the_windows_npm_shim_when_present() {
        if let Some(path) = windows_npm_binary() {
            assert!(path.ends_with("agent-browser.cmd"));
        }
    }
}
