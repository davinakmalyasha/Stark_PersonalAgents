//! Code Mode on OpenCode v2 semantics: a confined psychoscript program (Bun
//! sidecar) that scripts Stark tool calls and composes their results.
//!
//! The model gets one `execute` tool instead of many round-trips. The program
//! can read/search several files, compute over the results, edit, and return
//! one compact value. Intermediate tool output never enters the conversation.
//!
//! Every nested call runs through [`crate::dispatch`] with the executing run's
//! [`ToolContext`] (approvals, plan guard, MCP, process manager). OpenCode's
//! `fetch`/`path`/`Bun` globals run in the confined sidecar, but every host
//! operation crosses back to Rust for the same permission and plan guards.
//!
//! Long scripts follow OpenCode's job contract: with `full_codemode`,
//! foreground blocks up to `timeout` (default 2 minutes); slower scripts move
//! to the background and report through `<background-result kind="codemode"
//! ...>` follow-ups.

pub mod catalog;
pub mod extensions;
pub mod instructions;
pub mod jobs;
pub mod schema;
pub mod tool_output;
pub mod worker;

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::{ToolContext, ToolErrorClass, ToolOutcome};

pub use jobs::{BLOCK_TIMEOUT_MS, JobStatus};
use worker::{WorkerManager, WorkerToolDef};

/// The Code Mode dispatcher tool (OpenCode `execute`).
pub const EXECUTE_TOOL: &str = "execute";

/// OpenCode `FullInput` timeout description (`2 * 60 * 1_000`).
const TIMEOUT_DESCRIPTION: &str = "Optional time in milliseconds to wait for the script before moving it to the background (default 120000). The script keeps running in the background after that; it is never stopped. With `background: true` the call returns immediately instead.";
const BACKGROUND_DESCRIPTION: &str = "Run the script in the background and return immediately with its job ID (useful for long-running work). You will be notified when it completes. DO NOT poll for completion.";

/// Flat dispatch name -> dotted program path (`mcp__srv__tool` -> `srv.tool`).
/// Built-ins keep their flat name (they never contain `.`).
pub fn to_program_name(flat: &str) -> String {
    match flat.strip_prefix("mcp__") {
        Some(rest) => rest.split("__").collect::<Vec<_>>().join("."),
        None => flat.to_string(),
    }
}

/// Dotted program path -> flat dispatch name (`srv.tool` -> `mcp__srv__tool`).
/// Names without a dot are built-ins and pass through unchanged.
pub fn to_dispatch_name(dotted: &str) -> String {
    if dotted.contains('.') {
        format!("mcp__{}", dotted.replace('.', "__"))
    } else {
        dotted.to_string()
    }
}

/// Tools that are single required strings in OpenCode's Code Mode catalog
/// (`tools.patch(patchText: string)`). Stark's `apply_patch` is the analogue.
const FREEFORM_TOOLS: &[&str] = &["apply_patch"];

pub fn is_freeform(dispatch_name: &str) -> bool {
    FREEFORM_TOOLS.contains(&dispatch_name)
}

/// Tool definitions for the worker, from the run's program catalog. Deferred
/// MCP tools stay out: the program discovers them through `search`.
pub fn tool_defs_for_context(ctx: &ToolContext) -> Vec<WorkerToolDef> {
    let mut defs = Vec::new();
    let mut seen = HashSet::new();
    for spec in crate::program_specs_for_context(ctx) {
        let Some(name) = spec
            .get("function")
            .and_then(|function| function.get("name"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        if !seen.insert(name.to_string()) {
            continue;
        }
        let description = spec
            .get("function")
            .and_then(|function| function.get("description"))
            .and_then(Value::as_str)
            .unwrap_or("Stark tool")
            .to_string();
        let input_schema = spec
            .get("function")
            .and_then(|function| function.get("parameters"))
            .cloned()
            .unwrap_or(json!({"type": "object"}));
        defs.push(WorkerToolDef {
            name: to_program_name(name),
            description,
            input_schema,
            freeform: is_freeform(name),
        });
    }
    // Deterministic runtime identity: the worker caches runtimes by this list.
    defs.sort_by(|left, right| left.name.cmp(&right.name));
    defs
}

/// OpenCode `CodeModeTool.catalog`: the same inventory in the same order.
pub fn catalog_for_specs(specs: &[Value], full: bool) -> Vec<catalog::CatalogItem> {
    let pinned: HashSet<String> = if full {
        pinned_program_tools()
            .iter()
            .map(|name| to_program_name(name))
            .collect()
    } else {
        HashSet::new()
    };
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    for spec in specs {
        let Some(name) = spec
            .get("function")
            .and_then(|function| function.get("name"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        if !seen.insert(name.to_string()) {
            continue;
        }
        let description = spec
            .get("function")
            .and_then(|function| function.get("description"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let input_schema = spec
            .get("function")
            .and_then(|function| function.get("parameters"))
            .cloned()
            .unwrap_or(json!({"type": "object"}));
        let program_name = to_program_name(name);
        // Stark tools return model-facing text (or null for empty results).
        let output_schema = json!({"type": ["string", "null"]});
        items.push(catalog::CatalogItem {
            path: program_name.clone(),
            description,
            signature: schema::tool_signature(
                &program_name,
                &input_schema,
                &output_schema,
                is_freeform(name),
            ),
            pinned: pinned.contains(&to_program_name(name)),
        });
    }
    items.sort_by(|left, right| left.path.cmp(&right.path));
    items
}

/// Prompt section for an agent turn. Rendered in Rust from the worker-shaped
/// catalog so no sidecar round-trip sits on the prompt path; signatures are
/// byte-identical to the worker's runtime (see `schema` tests).
pub fn render_catalog_for_specs(specs: &[Value], full: bool) -> String {
    let items = catalog_for_specs(specs, full);
    catalog::render(&catalog::summarize(&items, catalog::INLINE_BUDGET_TOKENS))
}

/// The pinned program tools with `full_codemode` (OpenCode read/edit/patch/
/// shell). Stark has no `write`; `apply_patch` is its patch tool.
pub fn pinned_program_tools() -> Vec<&'static str> {
    vec!["bash", "read", "edit", "apply_patch"]
}

/// OpenCode `description(full)`, byte-for-byte.
pub fn execute_description(full: bool) -> String {
    let mut lines = vec![
        if full {
            "Run JavaScript in a confined Code Mode runtime to work with files and data, script tool calls and HTTP requests, and compose their results."
        } else {
            "Run JavaScript in a confined Code Mode runtime to script tool calls and HTTP requests and compose their results."
        }
        .to_string(),
        "Write plain JavaScript and send it exactly as written: no code fence, and no JSON or other wrapper around it."
            .to_string(),
        "Each call is a fresh program; nothing carries over from earlier calls.".to_string(),
        if full {
            "`fetch` is available for HTTP requests, and Bun's file APIs (`Bun.file`, `Bun.write`, `Bun.Glob`) for files, in place of Node's `fs`. Imports and timers are unavailable: there is no `setTimeout` or sleep. All other external access goes through `tools`."
        } else {
            "`fetch` is available for HTTP requests. Imports, direct filesystem access, and timers are unavailable: there is no `setTimeout` or sleep. All other external access goes through `tools`."
        }
        .to_string(),
        "Inside `execute`, the only callable tools are those explicitly listed in the Code Mode catalog instructions or returned by the `search` function. Ignore tools shown outside the Code Mode catalog. They are not available in the Code Mode runtime."
            .to_string(),
        "Call tools through `tools` using only exact paths and signatures from the catalog. Do not infer or normalize tool names; preserve bracket notation such as `tools.<namespace>[\"tool-name\"](input)`."
            .to_string(),
        "Prefer an explicit `return`; if omitted, the final top-level expression becomes the result."
            .to_string(),
        "Await every call whose completion matters; pending calls are interrupted when execution ends. Run independent calls concurrently with `Promise.allSettled`."
            .to_string(),
    ];
    if full {
        lines.push(
            "Scripts still running after their timeout move to the background and keep running. Background scripts return a job ID immediately, and you will be notified when they complete; do not poll for them."
                .to_string(),
        );
    }
    lines.join("\n")
}

/// OpenCode `CodeMode.Input` / `CodeModeTool.FullInput` as the provider-facing
/// JSON Schema. Effect's `toJsonSchemaDocument(..., { onExcessProperty:
/// "error" })` emits `additionalProperties: false`; execute dispatch rejects
/// excess arguments the same way.
pub fn execute_parameters(full: bool) -> Value {
    let mut properties = Map::new();
    properties.insert("code".to_string(), json!({"type": "string"}));
    if full {
        properties.insert(
            "timeout".to_string(),
            json!({
                "type": "integer",
                "maximum": 9_007_199_254_740_991u64,
                "exclusiveMinimum": 0,
                "description": TIMEOUT_DESCRIPTION,
            }),
        );
        properties.insert(
            "background".to_string(),
            json!({"type": "boolean", "description": BACKGROUND_DESCRIPTION}),
        );
    }
    json!({
        "type": "object",
        "properties": Value::Object(properties),
        "required": ["code"],
        "additionalProperties": false,
    })
}

/// The `execute` function definition for one agent turn.
pub fn execute_spec(full: bool) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": EXECUTE_TOOL,
            "description": execute_description(full),
            "parameters": execute_parameters(full),
        }
    })
}

pub async fn handle(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let Some(code) = args.get("code").and_then(Value::as_str) else {
        return ToolOutcome::err_class(
            ToolErrorClass::Args,
            "Error: execute requires a 'code' string",
        );
    };
    let full = ctx.codemode_full();
    let allowed: &[&str] = if full {
        &["code", "timeout", "background"]
    } else {
        &["code"]
    };
    if let Some(object) = args.as_object()
        && let Some(unknown) = object.keys().find(|key| !allowed.contains(&key.as_str()))
    {
        return ToolOutcome::err_class(
            ToolErrorClass::Args,
            format!("Error: unexpected execute argument '{unknown}'"),
        );
    }
    let background = if full {
        match args.get("background") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(value)) => *value,
            Some(_) => {
                return ToolOutcome::err_class(
                    ToolErrorClass::Args,
                    "Error: execute background must be a boolean",
                );
            }
        }
    } else {
        false
    };
    let timeout_ms = if full {
        match args.get("timeout") {
            None | Some(Value::Null) => BLOCK_TIMEOUT_MS,
            Some(value) => match value.as_u64() {
                Some(timeout) if timeout > 0 && timeout <= 9_007_199_254_740_991 => timeout,
                _ => {
                    return ToolOutcome::err_class(
                        ToolErrorClass::Args,
                        "Error: execute timeout must be a positive integer",
                    );
                }
            },
        }
    } else {
        BLOCK_TIMEOUT_MS
    };

    let defs = Arc::new(tool_defs_for_context(ctx));
    let manager = match ctx.worker_manager.clone() {
        Some(manager) => manager,
        // Tests and ad-hoc contexts get a disposable worker; production
        // agents attach the shared manager in stark-core.
        None => WorkerManager::local(&ctx.harness_root),
    };
    let initialized = tokio::select! {
        biased;
        _ = ctx.cancel.cancelled() => return crate::cancelled_outcome(EXECUTE_TOOL),
        result = manager.initialize(&defs) => result,
    };
    if let Err(error) = initialized {
        return ToolOutcome::err_class(
            ToolErrorClass::Transport,
            format!("code mode worker error: {error}"),
        );
    }
    if ctx.cancel.is_cancelled() {
        return crate::cancelled_outcome(EXECUTE_TOOL);
    }
    let prepared = tokio::select! {
        biased;
        _ = ctx.cancel.cancelled() => return crate::cancelled_outcome(EXECUTE_TOOL),
        result = manager.prepare() => result,
    };
    let inner = match prepared {
        Ok(inner) => inner,
        Err(error) => {
            return ToolOutcome::err_class(
                ToolErrorClass::Transport,
                format!("code mode worker error: {error}"),
            );
        }
    };

    let run_id = jobs::new_job_id();
    // A script's own token. Stop cancels it only while the call is still
    // foreground; once a script moves to the background OpenCode leaves it
    // running.
    let script_cancel = CancellationToken::new();
    // The worker's nested calls are already inside a program. Mark the host
    // context so `dispatch` runs the real tool instead of repairing the call
    // into another implicit `execute` (which would prepend `<tool-repair>` to
    // the program's result).
    let mut script_ctx = ctx.clone().with_in_program(true);
    script_ctx.cancel = script_cancel.clone();

    if background {
        let job_manager = jobs::JobManager::for_root(&ctx.harness_root);
        job_manager.start(&run_id, &ctx.chat_id);
        let join = spawn_execute(
            manager.clone(),
            inner.clone(),
            run_id.clone(),
            script_ctx,
            code.to_string(),
            full,
            defs.clone(),
        );
        spawn_background_join(
            join,
            job_manager,
            run_id.clone(),
            ctx.background_hook.clone(),
        );
        return background_outcome(&run_id);
    }

    let mut join = spawn_execute(
        manager.clone(),
        inner.clone(),
        run_id.clone(),
        script_ctx,
        code.to_string(),
        full,
        defs.clone(),
    );
    let completed = if full {
        tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => None,
            result = &mut join => Some(result),
            _ = tokio::time::sleep(std::time::Duration::from_millis(timeout_ms)) => None,
        }
    } else {
        tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => None,
            result = &mut join => Some(result),
        }
    };
    match completed {
        Some(Ok(Ok(result))) => result_outcome(result, &ctx.harness_root),
        Some(Ok(Err(error))) => {
            if error == "CANCELLED" {
                manager.forget(&run_id).await;
                return crate::cancelled_outcome(EXECUTE_TOOL);
            }
            ToolOutcome::err_class(
                ToolErrorClass::Transport,
                format!("code mode worker error: {error}"),
            )
        }
        Some(Err(join_error)) => ToolOutcome::err_class(
            ToolErrorClass::Transport,
            format!("code mode worker task failed: {join_error}"),
        ),
        None => {
            if ctx.cancel.is_cancelled() {
                script_cancel.cancel();
                manager.cancel(&run_id).await;
                return crate::cancelled_outcome(EXECUTE_TOOL);
            }
            // Foreground timed out: adopt the still-running execution instead
            // of starting a second one (OpenCode: it is never stopped). The
            // script keeps its own token, so a later Stop cannot kill it.
            let job_manager = jobs::JobManager::for_root(&ctx.harness_root);
            job_manager.start(&run_id, &ctx.chat_id);
            spawn_background_join(
                join,
                job_manager,
                run_id.clone(),
                ctx.background_hook.clone(),
            );
            background_outcome(&run_id)
        }
    }
}

/// Spawn one worker execution on an already-prepared worker. `execute_on`
/// does not lazily start the sidecar, so the future is `Send`.
fn spawn_execute(
    manager: Arc<WorkerManager>,
    inner: Arc<worker::WorkerInner>,
    run_id: String,
    ctx: ToolContext,
    code: String,
    full: bool,
    tools: Arc<Vec<WorkerToolDef>>,
) -> tokio::task::JoinHandle<Result<worker::WorkerExecuteResult, String>> {
    tokio::spawn(async move {
        // Keep the manager alive for the whole script: background scripts and
        // in-flight tool calls rely on its child/registry.
        manager
            .execute_on(inner, &run_id, &ctx, &code, full, &tools)
            .await
    })
}

fn spawn_background_join(
    join: tokio::task::JoinHandle<Result<worker::WorkerExecuteResult, String>>,
    job_manager: Arc<jobs::JobManager>,
    run_id: String,
    hook: Option<crate::BackgroundHook>,
) {
    tokio::spawn(async move {
        let result = match join.await {
            Ok(result) => result,
            Err(error) => Err(format!("code mode worker task failed: {error}")),
        };
        finish_background(&job_manager, &run_id, result, hook.as_ref());
    });
}

fn finish_background(
    job_manager: &Arc<jobs::JobManager>,
    run_id: &str,
    result: Result<worker::WorkerExecuteResult, String>,
    hook: Option<&crate::BackgroundHook>,
) {
    match result {
        Ok(result) => {
            // OpenCode `CodeModeJob` truncates the job text before it stores
            // the result or renders the completion notice.
            let truncated = tool_output::truncate(
                &result.text,
                &tool_output::directory(job_manager.harness_root()),
            );
            let status = if result.error {
                JobStatus::Error
            } else {
                JobStatus::Completed
            };
            let (output, error) = if result.error {
                (None, Some(truncated.text))
            } else {
                (Some(truncated.text), None)
            };
            job_manager.finish(run_id, status, output, error);
        }
        Err(error) => {
            job_manager.finish(run_id, JobStatus::Error, None, Some(error));
        }
    }
    if let (Some(info), Some(hook)) = (job_manager.get(run_id), hook) {
        hook(jobs::completion_message(&info));
        job_manager.mark_delivered(run_id);
    }
}

fn background_outcome(run_id: &str) -> ToolOutcome {
    let text = jobs::still_running_text(run_id);
    ToolOutcome::ok(text).with_metadata(json!({"jobID": run_id}))
}

/// OpenCode returns program failures as normal tool results with
/// `metadata.error = true`; only transport failures are tool errors.
fn result_outcome(
    result: worker::WorkerExecuteResult,
    harness_root: &std::path::Path,
) -> ToolOutcome {
    let truncated = tool_output::truncate(&result.text, &tool_output::directory(harness_root));
    let mut metadata = json!({
        "toolCalls": result.tool_calls,
        "truncated": truncated.truncated,
    });
    if let Some(path) = &truncated.output_path {
        metadata["outputPath"] = Value::String(path.to_string_lossy().into_owned());
    }
    if result.error {
        metadata["error"] = Value::Bool(true);
    }
    if !result.files.is_empty() {
        metadata["files"] = Value::Array(result.files);
    }
    ToolOutcome::ok(truncated.text).with_metadata(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotted_names_round_trip() {
        assert_eq!(to_program_name("read"), "read");
        assert_eq!(
            to_program_name("mcp__github__get_issue"),
            "github.get_issue"
        );
        assert_eq!(to_program_name("mcp__srv__a__b"), "srv.a.b");
        assert_eq!(to_dispatch_name("read"), "read");
        assert_eq!(
            to_dispatch_name("github.get_issue"),
            "mcp__github__get_issue"
        );
        assert_eq!(to_dispatch_name("srv.a.b"), "mcp__srv__a__b");
    }

    #[test]
    fn execute_specs_match_opencode() {
        let base = execute_spec(false);
        assert_eq!(base["function"]["description"], execute_description(false));
        assert!(
            base["function"]["parameters"]["properties"]
                .get("timeout")
                .is_none()
        );
        assert!(
            base["function"]["parameters"]["properties"]
                .get("max_tool_calls")
                .is_none()
        );
        assert_eq!(
            base["function"]["parameters"]["additionalProperties"],
            json!(false)
        );

        let full = execute_spec(true);
        assert_eq!(full["function"]["description"], execute_description(true));
        assert_eq!(
            full["function"]["parameters"]["properties"]["timeout"]["exclusiveMinimum"],
            json!(0)
        );
        assert_eq!(
            full["function"]["parameters"]["properties"]["timeout"]["maximum"],
            json!(9_007_199_254_740_991u64)
        );
        assert_eq!(
            full["function"]["parameters"]["properties"]["background"]["type"],
            json!("boolean")
        );
        assert!(
            full["function"]["description"]
                .as_str()
                .unwrap()
                .contains("do not poll")
        );
    }

    #[test]
    fn execute_parameters_match_the_effect_json_schema() {
        assert_eq!(
            execute_parameters(false),
            json!({
                "type": "object",
                "properties": {"code": {"type": "string"}},
                "required": ["code"],
                "additionalProperties": false
            })
        );
        assert_eq!(
            execute_parameters(true),
            json!({
                "type": "object",
                "properties": {
                    "code": {"type": "string"},
                    "timeout": {
                        "type": "integer",
                        "maximum": 9_007_199_254_740_991u64,
                        "exclusiveMinimum": 0,
                        "description": "Optional time in milliseconds to wait for the script before moving it to the background (default 120000). The script keeps running in the background after that; it is never stopped. With `background: true` the call returns immediately instead."
                    },
                    "background": {
                        "type": "boolean",
                        "description": "Run the script in the background and return immediately with its job ID (useful for long-running work). You will be notified when it completes. DO NOT poll for completion."
                    }
                },
                "required": ["code"],
                "additionalProperties": false
            })
        );
    }

    #[test]
    fn execute_contract_text_matches_opencode_source() {
        let fixture: Value =
            serde_json::from_str(include_str!("testdata/opencode_execute_contract.json")).unwrap();
        assert_eq!(
            execute_description(false),
            fixture["description"]["base"].as_str().unwrap()
        );
        assert_eq!(
            execute_description(true),
            fixture["description"]["full"].as_str().unwrap()
        );
        assert_eq!(
            TIMEOUT_DESCRIPTION,
            fixture["timeout_description"].as_str().unwrap()
        );
        assert_eq!(
            BACKGROUND_DESCRIPTION,
            fixture["background_description"].as_str().unwrap()
        );
    }

    #[test]
    fn execute_parameters_match_the_opencode_effect_schema() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../stark-providers/testdata/opencode_freeform_sequence.json"
        ))
        .unwrap();
        assert_eq!(execute_parameters(false), fixture["execute_schema"]["base"]);
        assert_eq!(execute_parameters(true), fixture["execute_schema"]["full"]);
    }

    #[test]
    fn still_running_text_matches_opencode() {
        assert_eq!(
            jobs::still_running_text("cm_abc"),
            "Script cm_abc is still running; its result will arrive in this session."
        );
    }
}
