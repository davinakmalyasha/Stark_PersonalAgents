//! Code mode: a restricted JavaScript program that calls Stark tools.
//!
//! The model gets one `run_code` tool instead of many round-trips. The
//! program can read/search several files, compute over the results, edit, and
//! return one compact value. Intermediate tool output never enters the
//! conversation.
//!
//! QuickJS is embedded with no host APIs. Programs can only reach the
//! allowlisted tools through the `tools` bridge, which goes through the normal
//! [`crate::dispatch`] path (approvals, plan guard, MCP, process manager).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rquickjs::{Context, Function, Runtime};
use serde_json::{Value, json};

use crate::{ToolContext, ToolOutcome};

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
const DEFAULT_MAX_TOOL_CALLS: usize = 200;
const MAX_MAX_TOOL_CALLS: usize = 500;
const MAX_CODE_BYTES: usize = 64 * 1024;
const MAX_RESULT_CHARS: usize = 32 * 1024;
const MAX_LOG_LINES: usize = 50;
const MAX_LOG_CHARS: usize = 8 * 1024;
const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
const STACK_LIMIT: usize = 512 * 1024;

/// Tools that are deliberately not reachable from inside a program. They are
/// interactive, recursive, or manage long-running work that belongs to the
/// outer agent loop. Derived from the inventory `program` flag (Codex
/// `allowed_callers` direct-only); the canonical list lives in `inventory`.
fn program_blocked(name: &str) -> bool {
    !crate::is_program(name)
}

const BOOTSTRAP: &str = r#"
(function () {
  const catalog = JSON.parse(__catalog_json);
  globalThis.tools = {};
  for (const item of catalog) {
    globalThis.tools[item.name] = function (args) {
      const raw = __call_tool(item.name, JSON.stringify(args === undefined ? {} : args));
      const result = JSON.parse(raw);
      if (!result.ok) throw new Error(result.error || "tool failed");
      return result.content;
    };
  }
  globalThis.search = function (query) {
    const q = String(query || "").toLowerCase();
    const hits = [];
    for (const item of catalog) {
      if (!q || item.name.toLowerCase().includes(q) || (item.description || "").toLowerCase().includes(q)) {
        hits.push(item);
      }
      if (hits.length >= 50) break;
    }
    return JSON.stringify(hits);
  };
  globalThis.console = {
    log: function () {
      const parts = [];
      for (const value of arguments) {
        try { parts.push(typeof value === "string" ? value : JSON.stringify(value)); }
        catch (e) { parts.push(String(value)); }
      }
      __log(parts.join(" "));
    },
    error: function () {
      const parts = [];
      for (const value of arguments) {
        try { parts.push(typeof value === "string" ? value : JSON.stringify(value)); }
        catch (e) { parts.push(String(value)); }
      }
      __log(parts.join(" "));
    }
  };
})();
"#;

struct BridgeState {
    calls: usize,
    logs: Vec<String>,
}

struct Bridge {
    ctx: ToolContext,
    handle: tokio::runtime::Handle,
    allowed: HashSet<String>,
    deadline: Instant,
    max_calls: usize,
    state: Mutex<BridgeState>,
}

impl Bridge {
    fn call(&self, name: &str, args_json: &str) -> String {
        if !self.allowed.contains(name) {
            return error_envelope(format!("tool '{name}' is not available in code mode"));
        }
        if Instant::now() >= self.deadline {
            return error_envelope("code mode timed out".to_string());
        }
        {
            let mut state = self.state.lock().unwrap();
            if state.calls >= self.max_calls {
                return error_envelope(format!("tool call limit reached ({})", self.max_calls));
            }
            state.calls += 1;
        }
        let args: Value = serde_json::from_str(args_json).unwrap_or_else(|_| json!({}));
        if name == "run_command" {
            if args.get("background").and_then(Value::as_bool) == Some(true) {
                return error_envelope(
                    "background commands are not allowed in code mode".to_string(),
                );
            }
            if args
                .get("process_action")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty())
            {
                return error_envelope("process actions are not allowed in code mode".to_string());
            }
        }
        self.emit(stark_contract::Delta::tool_started(name, args_json));
        let started = Instant::now();
        let outcome = self
            .handle
            .block_on(crate::dispatch(name, &args, &self.ctx));
        let preview: String = outcome.content.chars().take(400).collect();
        self.emit(stark_contract::Delta::tool_finished(
            name,
            args_json,
            !outcome.is_error,
            started.elapsed().as_millis() as u64,
            preview,
        ));
        if outcome.is_error {
            error_envelope(outcome.content)
        } else {
            json!({"ok": true, "content": outcome.content}).to_string()
        }
    }

    fn log(&self, line: String) {
        let mut state = self.state.lock().unwrap();
        if state.logs.len() >= MAX_LOG_LINES {
            return;
        }
        let line: String = line.chars().take(2_000).collect();
        state.logs.push(line);
    }

    fn logs(&self) -> Vec<String> {
        let state = self.state.lock().unwrap();
        let mut out = String::new();
        let mut lines = Vec::new();
        for line in &state.logs {
            if out.len() + line.len() > MAX_LOG_CHARS {
                break;
            }
            out.push_str(line);
            lines.push(line.clone());
        }
        lines
    }

    fn emit(&self, delta: stark_contract::Delta) {
        if let Some(callback) = &self.ctx.code_delta {
            callback(delta);
        }
    }
}

fn error_envelope(message: String) -> String {
    json!({"ok": false, "error": message}).to_string()
}

pub async fn handle(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let code = match args.get("code").and_then(Value::as_str) {
        Some(code) if !code.trim().is_empty() => code.to_string(),
        _ => return ToolOutcome::err("Error: run_code requires a non-empty 'code' string"),
    };
    if code.len() > MAX_CODE_BYTES {
        return ToolOutcome::err(format!(
            "Error: code is too large ({} bytes, max {MAX_CODE_BYTES})",
            code.len()
        ));
    }
    let timeout_ms = args
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .clamp(1_000, MAX_TIMEOUT_MS);
    let max_calls = args
        .get("max_tool_calls")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(DEFAULT_MAX_TOOL_CALLS)
        .clamp(1, MAX_MAX_TOOL_CALLS);

    let specs = crate::tool_specs_for_scope_with_access(&ctx.mode, &ctx.scope, &ctx.access);
    let mut catalog = Vec::new();
    let mut allowed = HashSet::new();
    for spec in specs {
        let Some(function) = spec.get("function") else {
            continue;
        };
        let Some(name) = function.get("name").and_then(Value::as_str) else {
            continue;
        };
        if program_blocked(name) || !allowed.insert(name.to_string()) {
            continue;
        }
        // Required arg names ride along so programs construct calls
        // correctly without paying full parameter schemas every turn.
        let required: Vec<&str> = function
            .pointer("/parameters/required")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        catalog.push(json!({
            "name": name,
            "description": function
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("Stark tool"),
            "required": required,
        }));
    }

    let bridge = Arc::new(Bridge {
        ctx: ctx.clone(),
        handle: tokio::runtime::Handle::current(),
        allowed,
        deadline: Instant::now() + Duration::from_millis(timeout_ms),
        max_calls,
        state: Mutex::new(BridgeState {
            calls: 0,
            logs: Vec::new(),
        }),
    });
    let catalog_json = serde_json::to_string(&catalog).unwrap_or_else(|_| "[]".to_string());
    let log_bridge = bridge.clone();
    let result = tokio::task::spawn_blocking(move || execute(bridge, code, catalog_json)).await;
    let logs = log_bridge.logs();
    let raw = match result {
        Ok(Ok(raw)) => raw,
        Ok(Err(error)) => return ToolOutcome::err(format!("[CODE MODE ERROR]\n{error}")),
        Err(error) => {
            return ToolOutcome::err(format!(
                "[CODE MODE ERROR]\ncode mode worker failed: {error}"
            ));
        }
    };

    let parsed: Value = serde_json::from_str(&raw).unwrap_or_else(|_| Value::String(raw.clone()));
    if let Some(error) = parsed.get("__error").and_then(Value::as_str) {
        let stack = parsed.get("__stack").and_then(Value::as_str).unwrap_or("");
        let message = if stack.is_empty() {
            error.to_string()
        } else {
            format!("{error}\n{stack}")
        };
        return ToolOutcome::err(format!("[CODE MODE ERROR]\n{message}"));
    }

    let mut content = String::from("[CODE MODE RESULT]\n");
    content.push_str(&raw.chars().take(MAX_RESULT_CHARS).collect::<String>());
    if !logs.is_empty() {
        content.push_str("\n\n[CODE MODE LOGS]\n");
        content.push_str(&logs.join("\n"));
    }
    ToolOutcome::ok(content)
}

fn execute(bridge: Arc<Bridge>, code: String, catalog_json: String) -> Result<String, String> {
    let runtime = Runtime::new().map_err(|error| error.to_string())?;
    runtime.set_memory_limit(MEMORY_LIMIT);
    runtime.set_max_stack_size(STACK_LIMIT);
    let deadline = bridge.deadline;
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
    let context = Context::full(&runtime).map_err(|error| error.to_string())?;
    context.with(|ctx| {
        let globals = ctx.globals();
        let call = {
            let bridge = bridge.clone();
            Function::new(ctx.clone(), move |name: String, args: String| -> String {
                bridge.call(&name, &args)
            })
            .map_err(|error| error.to_string())?
        };
        globals
            .set("__call_tool", call)
            .map_err(|error| error.to_string())?;
        let log = {
            let bridge = bridge.clone();
            Function::new(ctx.clone(), move |line: String| -> String {
                bridge.log(line);
                String::new()
            })
            .map_err(|error| error.to_string())?
        };
        globals
            .set("__log", log)
            .map_err(|error| error.to_string())?;
        globals
            .set("__catalog_json", catalog_json)
            .map_err(|error| error.to_string())?;
        let _: rquickjs::Value = ctx
            .eval(BOOTSTRAP)
            .map_err(|error| format!("code mode bootstrap failed: {error}"))?;

        let script = format!(
            r#"(function () {{
  try {{
    const __value = (function () {{
{code}
    }})();
    return JSON.stringify(__value === undefined ? null : __value);
  }} catch (e) {{
    return JSON.stringify({{
      __error: String(e && e.message ? e.message : e),
      __stack: String(e && e.stack ? e.stack : "")
    }});
  }}
}})()"#
        );
        ctx.eval::<String, _>(script)
            .map_err(|error| format!("code mode failed: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn context(dir: &std::path::Path) -> ToolContext {
        ToolContext::new(
            dir,
            "code-mode-test",
            Arc::new(crate::QuestionBroker::new()),
        )
        .with_scope("developer")
        .with_mode("build")
    }

    fn result_json(outcome: &ToolOutcome) -> Value {
        let body = outcome
            .content
            .strip_prefix("[CODE MODE RESULT]\n")
            .unwrap_or(&outcome.content);
        serde_json::from_str(body).expect("result is JSON")
    }

    #[tokio::test]
    async fn reads_files_through_the_tool_bridge() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello code mode").unwrap();
        let ctx = context(dir.path());
        let outcome = handle(
            &ctx,
            &json!({
                "code": r#"
const text = tools.read({ file_path: "a.txt" });
return { found: text.includes("hello code mode") };
"#
            }),
        )
        .await;
        assert!(!outcome.is_error, "{}", outcome.content);
        assert_eq!(result_json(&outcome)["found"], true);
    }

    #[tokio::test]
    async fn edits_files_through_the_tool_bridge() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        let ctx = context(dir.path());
        let outcome = handle(
            &ctx,
            &json!({
                "code": r#"
tools.read({ file_path: "a.txt" });
tools.edit({ file_path: "a.txt", old_string: "hello", new_string: "bye" });
return tools.read({ file_path: "a.txt" });
"#
            }),
        )
        .await;
        assert!(!outcome.is_error, "{}", outcome.content);
        let saved = std::fs::read_to_string(dir.path().join("a.txt")).unwrap();
        assert_eq!(saved, "bye");
    }

    #[tokio::test]
    async fn search_lists_tools_and_disallowed_tools_are_absent() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let outcome = handle(
            &ctx,
            &json!({
                "code": r#"
const hits = JSON.parse(search("read"));
return {
  hasRead: hits.some((item) => item.name === "read"),
  hasAsk: typeof tools.ask_user,
  hasProcess: typeof process,
  hasRequire: typeof require
};
"#
            }),
        )
        .await;
        assert!(!outcome.is_error, "{}", outcome.content);
        let value = result_json(&outcome);
        assert_eq!(value["hasRead"], true);
        assert_eq!(value["hasAsk"], "undefined");
        assert_eq!(value["hasProcess"], "undefined");
        assert_eq!(value["hasRequire"], "undefined");
        // Program-visible catalog carries required arg names.
        let catalog = handle(&ctx, &json!({"code": "return JSON.parse(search(\"\"));"})).await;
        assert!(!catalog.is_error, "{}", catalog.content);
        let read = result_json(&catalog)
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["name"] == "read")
            .cloned()
            .expect("read in catalog");
        assert!(
            read["required"]
                .as_array()
                .unwrap()
                .contains(&json!("file_path")),
            "{read}"
        );
    }

    #[tokio::test]
    async fn tool_call_limit_is_enforced() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        let ctx = context(dir.path());
        let outcome = handle(
            &ctx,
            &json!({
                "code": r#"
let n = 0;
for (let i = 0; i < 5; i++) {
  tools.read({ file_path: "a.txt" });
  n++;
}
return n;
"#,
                "max_tool_calls": 2
            }),
        )
        .await;
        assert!(outcome.is_error);
        assert!(
            outcome.content.contains("tool call limit"),
            "{}",
            outcome.content
        );
    }

    #[tokio::test]
    async fn delegation_tools_are_absent_inside_programs() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        // Excluded from the program catalog, so the properties are undefined
        // and any call attempt throws before reaching the bridge.
        let outcome = handle(
            &ctx,
            &json!({"code": "return { spawn: typeof tools.spawn_agent, msg: typeof tools.message_agent, ask: typeof tools.ask_user_question };"}),
        )
        .await;
        assert!(!outcome.is_error, "{}", outcome.content);
        let value = result_json(&outcome);
        assert_eq!(value["spawn"], "undefined");
        assert_eq!(value["msg"], "undefined");
        assert_eq!(value["ask"], "undefined");
    }

    #[tokio::test]
    async fn inner_calls_emit_timeline_deltas() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = seen.clone();
        let ctx = context(dir.path()).with_code_delta(Arc::new(move |delta| {
            seen_clone
                .lock()
                .unwrap()
                .push((delta.kind.clone(), delta.tool.clone().unwrap_or_default()));
        }));
        let outcome = handle(
            &ctx,
            &json!({"code": "return tools.read({ file_path: \"a.txt\" });"}),
        )
        .await;
        assert!(!outcome.is_error, "{}", outcome.content);
        let events = seen.lock().unwrap().clone();
        assert!(
            events.contains(&("tool_started".to_string(), "read".to_string())),
            "{events:?}"
        );
        assert!(
            events.contains(&("tool_finished".to_string(), "read".to_string())),
            "{events:?}"
        );
    }

    #[tokio::test]
    async fn timeout_interrupts_an_infinite_loop() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let outcome = handle(
            &ctx,
            &json!({"code": "while (true) {}", "timeout_ms": 1000}),
        )
        .await;
        assert!(outcome.is_error, "{}", outcome.content);
    }
}
