use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use tokio::process::Command;

use crate::{ToolContext, ToolOutcome};

const MAX_SNAPSHOT_CHARS: usize = 24_000;
const MAX_MCP_CHARS: usize = 24_000;
const DEFAULT_WAIT_MS: u64 = 5_000;

pub async fn browser_act(ctx: &ToolContext, args: &Value) -> ToolOutcome {
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
        "read" => read(args).await,
        "a11y" => a11y(args).await,
        "record" => record(ctx, args).await,
        "network" => network(ctx, args).await,
        "screenshot" => screenshot(ctx, args).await,
        "close" => {
            let _ = raw_command(&["close", "--all"], Duration::from_secs(10)).await;
            ToolOutcome::ok("Browser closed.")
        }
        other => ToolOutcome::err(format!(
            "Error: unknown browser_act action '{other}'. Valid: navigate, click, fill, type, press, extract, read, a11y, record, network, screenshot, close"
        )),
    }
}

async fn navigate(args: &Value) -> ToolOutcome {
    let url = required(args, "url");
    if url.trim().is_empty() {
        return ToolOutcome::err("Error: browser_act navigate requires url");
    }
    let url = if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else {
        format!("https://{url}")
    };
    if let Err(error) = crate::ds_web::validate_public_url(&url).await {
        return ToolOutcome::err(format!("Error: {error}"));
    }
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

async fn read(args: &Value) -> ToolOutcome {
    if let Some(url) = arg_str(args, "url")
        && let Err(error) = crate::ds_web::validate_public_url(url).await
    {
        return ToolOutcome::err(format!("Error: {error}"));
    }
    let mut payload = serde_json::Map::new();
    copy_str(args, &mut payload, "url");
    copy_str(args, &mut payload, "filter");
    copy_str(args, &mut payload, "llms");
    copy_bool(args, &mut payload, "outline");
    copy_bool(args, &mut payload, "raw");
    mcp_call(
        "agent_browser_read",
        Value::Object(payload),
        Duration::from_secs(90),
    )
    .await
}

async fn a11y(args: &Value) -> ToolOutcome {
    if let Some(url) = arg_str(args, "url")
        && let Err(error) = crate::ds_web::validate_public_url(url).await
    {
        return ToolOutcome::err(format!("Error: {error}"));
    }
    let mut payload = serde_json::Map::new();
    copy_str(args, &mut payload, "url");
    copy_str(args, &mut payload, "selector");
    copy_str(args, &mut payload, "tags");
    mcp_call(
        "agent_browser_a11y",
        Value::Object(payload),
        Duration::from_secs(90),
    )
    .await
}

async fn record(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let action = arg_str(args, "record_action")
        .unwrap_or("start")
        .to_ascii_lowercase();
    if action == "stop" {
        return mcp_call(
            "agent_browser_record_stop",
            Value::Object(serde_json::Map::new()),
            Duration::from_secs(30),
        )
        .await;
    }
    if !matches!(action.as_str(), "start" | "restart") {
        return ToolOutcome::err(format!(
            "Error: invalid record_action '{action}'. Use start, stop, or restart."
        ));
    }
    let Some(path) = arg_str(args, "path") else {
        return ToolOutcome::err("Error: record needs path (for example recordings/demo.webm)");
    };
    if path.trim().is_empty() {
        return ToolOutcome::err("Error: record needs a non-empty path");
    }
    let path = match crate::paths::resolve_workspace_path(&ctx.workspace, path) {
        Ok(path) => path,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };
    let path_text = path.to_string_lossy().to_string();
    if let Some(url) = arg_str(args, "url")
        && let Err(error) = crate::ds_web::validate_public_url(url).await
    {
        return ToolOutcome::err(format!("Error: {error}"));
    }
    let mut payload = serde_json::Map::new();
    payload.insert("path".to_string(), Value::String(path_text));
    copy_str(args, &mut payload, "url");
    copy_bool(args, &mut payload, "cursor");
    copy_as(args, &mut payload, "contact_sheet", "contactSheet");
    copy_u64(args, &mut payload, "fps");
    let tool = if action == "restart" {
        "agent_browser_record_restart"
    } else {
        "agent_browser_record_start"
    };
    mcp_call(tool, Value::Object(payload), Duration::from_secs(30)).await
}

async fn network(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let action = arg_str(args, "network_action")
        .unwrap_or("requests")
        .to_ascii_lowercase();
    let mut payload = serde_json::Map::new();
    let tool = match action.as_str() {
        "requests" => {
            copy_str(args, &mut payload, "filter");
            copy_str(args, &mut payload, "method");
            copy_str(args, &mut payload, "status");
            copy_as(args, &mut payload, "resource_type", "type");
            "agent_browser_network_requests"
        }
        "request" => {
            let Some(request_id) = arg_str(args, "request_id") else {
                return ToolOutcome::err(
                    "Error: network_action=request needs request_id (from network_action=requests)",
                );
            };
            payload.insert(
                "requestId".to_string(),
                Value::String(request_id.to_string()),
            );
            "agent_browser_network_request"
        }
        "har_start" => "agent_browser_network_har_start",
        "har_stop" => {
            if let Some(path) = arg_str(args, "path") {
                match crate::paths::resolve_workspace_path(&ctx.workspace, path) {
                    Ok(path) => {
                        payload.insert(
                            "path".to_string(),
                            Value::String(path.to_string_lossy().to_string()),
                        );
                    }
                    Err(error) => return ToolOutcome::err(format!("Error: {error}")),
                }
            }
            "agent_browser_network_har_stop"
        }
        "route" => {
            let Some(url) = arg_str(args, "url") else {
                return ToolOutcome::err("Error: network_action=route needs url");
            };
            payload.insert("url".to_string(), Value::String(url.to_string()));
            copy_str(args, &mut payload, "body");
            copy_as(args, &mut payload, "resource_type", "resourceType");
            copy_as(args, &mut payload, "allow_origin", "allowOrigin");
            copy_bool(args, &mut payload, "abort");
            "agent_browser_network_route"
        }
        "unroute" => {
            copy_str(args, &mut payload, "url");
            "agent_browser_network_unroute"
        }
        other => {
            return ToolOutcome::err(format!(
                "Error: invalid network_action '{other}'. Use requests, request, har_start, har_stop, route, or unroute."
            ));
        }
    };
    mcp_call(tool, Value::Object(payload), Duration::from_secs(60)).await
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn copy_str(args: &Value, payload: &mut serde_json::Map<String, Value>, key: &str) {
    copy_as(args, payload, key, key);
}

fn copy_as(args: &Value, payload: &mut serde_json::Map<String, Value>, key: &str, dest: &str) {
    if let Some(value) = arg_str(args, key) {
        payload.insert(dest.to_string(), Value::String(value.to_string()));
    }
}

fn copy_bool(args: &Value, payload: &mut serde_json::Map<String, Value>, key: &str) {
    if let Some(value) = args.get(key).and_then(Value::as_bool) {
        payload.insert(key.to_string(), Value::Bool(value));
    }
}

fn copy_u64(args: &Value, payload: &mut serde_json::Map<String, Value>, key: &str) {
    if let Some(value) = args.get(key).and_then(Value::as_u64) {
        payload.insert(key.to_string(), Value::from(value));
    }
}

/// Run one MCP tool without a persistent stdio session and render its text
/// content. This is what makes 0.41's `mcp call` useful here: `browser_act`
/// already spawns one short process per action, so no daemon lifecycle changes.
async fn mcp_call(tool: &str, args: Value, timeout: Duration) -> ToolOutcome {
    let json = args.to_string();
    let raw = raw_command(&["mcp", "call", tool, json.as_str()], timeout).await;
    render_mcp_result(&raw)
}

fn render_mcp_result(raw: &str) -> ToolOutcome {
    let trimmed = raw.trim();
    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
        return truncated_ok(trimmed);
    };
    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or(trimmed);
        return ToolOutcome::err(format!("Error: {message}"));
    }
    if let Some(items) = value.get("content").and_then(Value::as_array) {
        let mut parts = Vec::new();
        for item in items {
            if let Some(text) = item.get("text").and_then(Value::as_str) {
                parts.push(text.to_string());
            }
        }
        if !parts.is_empty() {
            let joined = parts.join("\n");
            return truncated_ok(joined.trim());
        }
    }
    truncated_ok(trimmed)
}

fn truncated_ok(text: &str) -> ToolOutcome {
    if text.is_empty() {
        return ToolOutcome::ok("(empty)");
    }
    if text.chars().count() > MAX_MCP_CHARS {
        let head: String = text.chars().take(MAX_MCP_CHARS).collect();
        return ToolOutcome::ok(format!("{head}\n[PARTIAL: output truncated]"));
    }
    ToolOutcome::ok(text.to_string())
}

async fn screenshot(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let requested = args
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty());
    let path = if let Some(raw) = requested {
        if ctx.access == "read" {
            // Screenshots are harness artifacts, never workspace writes, for
            // a read-only agent. Ignore a model-supplied path and use scratch.
            scratch_screenshot(ctx)
        } else {
            match crate::paths::resolve_workspace_path(&ctx.workspace, raw) {
                Ok(path) => path,
                Err(error) => return ToolOutcome::err(format!("Error: {error}")),
            }
        }
    } else {
        scratch_screenshot(ctx)
    };
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

fn scratch_screenshot(ctx: &ToolContext) -> PathBuf {
    let directory = ctx.harness_root.join(".deep_agents").join("scratch");
    let _ = std::fs::create_dir_all(&directory);
    directory.join(format!(
        "screenshot_{}.png",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    ))
}

async fn command(args: &[&str], timeout: Duration) -> ToolOutcome {
    ToolOutcome::ok(raw_command(args, timeout).await)
}

async fn raw_command(args: &[&str], timeout: Duration) -> String {
    let mut command = browser_command();
    command.args(args);
    command.kill_on_drop(true);
    // Browser automation never needs provider keys.
    stark_sandbox::secrets::scrub_tokio_command(&mut command);
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

    #[tokio::test]
    async fn record_requires_a_path_and_valid_action() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path(), "chat", Arc::new(QuestionBroker::new()));
        let missing = browser_act(&ctx, &serde_json::json!({"action": "record"})).await;
        assert!(missing.is_error);
        assert!(missing.content.contains("needs path"));
        let invalid = browser_act(
            &ctx,
            &serde_json::json!({"action": "record", "record_action": "pause", "path": "a.webm"}),
        )
        .await;
        assert!(invalid.is_error);
        assert!(invalid.content.contains("invalid record_action"));
    }

    #[tokio::test]
    async fn network_validates_actions_before_spawning() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path(), "chat", Arc::new(QuestionBroker::new()));
        let unknown = browser_act(
            &ctx,
            &serde_json::json!({"action": "network", "network_action": "proxy"}),
        )
        .await;
        assert!(unknown.is_error);
        assert!(unknown.content.contains("invalid network_action"));
        let missing_id = browser_act(
            &ctx,
            &serde_json::json!({"action": "network", "network_action": "request"}),
        )
        .await;
        assert!(missing_id.is_error);
        assert!(missing_id.content.contains("request_id"));
        let missing_url = browser_act(
            &ctx,
            &serde_json::json!({"action": "network", "network_action": "route"}),
        )
        .await;
        assert!(missing_url.is_error);
        assert!(missing_url.content.contains("needs url"));
    }

    #[test]
    fn renders_mcp_content_and_errors() {
        let ok = render_mcp_result(
            r#"{"_meta":{"agentBrowserVersion":"0.41.2"},"content":[{"type":"text","text":"hello"},{"type":"text","text":"world"}]}"#,
        );
        assert!(!ok.is_error);
        assert_eq!(ok.content, "hello\nworld");
        let err = render_mcp_result(
            r#"{"_meta":{"agentBrowserVersion":"0.41.2"},"error":{"code":-32602,"message":"Unknown tool: read"}}"#,
        );
        assert!(err.is_error);
        assert!(err.content.contains("Unknown tool"));
        let partial = render_mcp_result(&"x".repeat(MAX_MCP_CHARS + 10));
        assert!(partial.content.contains("[PARTIAL"));
    }

    #[tokio::test]
    async fn navigate_rejects_private_destinations_before_spawning() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path(), "chat", Arc::new(QuestionBroker::new()));
        let outcome = browser_act(
            &ctx,
            &serde_json::json!({"action": "navigate", "url": "http://127.0.0.1:8080"}),
        )
        .await;
        assert!(outcome.is_error, "{}", outcome.content);
        assert!(
            outcome.content.contains("blocked private destination")
                || outcome.content.contains("localhost"),
            "{}",
            outcome.content
        );
    }

    #[test]
    fn locates_the_windows_npm_shim_when_present() {
        if let Some(path) = windows_npm_binary() {
            assert!(path.ends_with("agent-browser.cmd"));
        }
    }
}
