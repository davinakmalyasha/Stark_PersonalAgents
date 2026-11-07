mod approvals;
mod browser;
mod changes;
mod checkpoint;
mod codemode;
mod dfd;
mod docs;
mod ds_fs;
mod ds_session;
mod ds_shell;
mod ds_web;
mod head_tail;
mod house;
mod images;
mod inventory;
mod mcp;
mod memory;
mod patch;
mod paths;
mod permissions;
mod plan;
mod processes;
mod questions;
mod read;
mod screen;
mod skills;
mod verify;

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{Value, json};
use stark_contract::{PlanItem, ToolSpecFile};

pub use approvals::ApprovalCache;
pub use checkpoint::{Checkpoint, RestoreReport, SnapshotGroup};
pub use house::{HouseStore, public_device, public_state};
pub use images::{InlineImage, collect_inline_images, is_image_mime, mime_for_ext};
pub use inventory::{INVENTORY, is_implemented, is_plan_write, is_program, is_read_blocked};
pub use permissions::doom_loop_gate;
pub use stark_sandbox::{ApprovalPolicy, SandboxMode, SandboxProfile};

/// Prompt line stating the requested sandbox mode and the real backend.
pub fn sandbox_prompt_line(workspace: &Path) -> String {
    SandboxProfile::from_env(workspace).prompt_line()
}

/// Like [`sandbox_prompt_line`], but the agent's file-access mode wins over
/// the process default. A read-only Orchestrator must not be told it has
/// workspace-write access.
pub fn sandbox_prompt_line_for(workspace: &Path, access: &str) -> String {
    let mut profile = SandboxProfile::from_env(workspace);
    if access == "read" {
        profile.mode = SandboxMode::ReadOnly;
    }
    profile.prompt_line()
}

/// Startup log line for the server.
pub fn sandbox_status(workspace: &Path) -> String {
    SandboxProfile::from_env(workspace).prompt_line()
}
pub use mcp::{McpManager, ServerConfig as McpServerConfig, init as init_mcp, search_tools};
pub use plan::{PlanError, PlanStore, update_plan};
pub use processes::{ProcessManager, ProcessStatus};
pub use questions::QuestionBroker;
pub use screen::{
    PendingImage, ScreenBridge, drain_images, latest_continuous_image, queue_image, screen_bridge,
};
pub use skills::{catalog_block_for, catalog_block_for_agent};
pub use verify::{AcceptanceVerdict, CriterionStatus, CriterionVerdict, evaluate_acceptance};

/// Tool membership and guard flags now live in [`inventory`]: one `ToolMeta`
/// entry per tool derives the implemented set, the plan-write guard, the
/// read-only guard, and the frontend labels. The frozen wire schema stays in
/// `backend-rs/specs/tool-specs.json`.
pub const MCP_EAGER_LIMIT: usize = 8;

static CONTRACT: OnceLock<ToolSpecFile> = OnceLock::new();

#[derive(Clone)]
pub struct ToolContext {
    pub workspace: PathBuf,
    /// Session working directory (Zone 1 runtime context). Relative paths in
    /// v2 tools resolve from here; the workspace root stays the containment
    /// boundary for old tools and checkpoints.
    pub cwd: Arc<Mutex<PathBuf>>,
    /// Files observed with `read` in this run. DeepSeek's fs policy requires
    /// a read before an existing file can be written or edited.
    pub read_paths: Arc<Mutex<HashSet<PathBuf>>>,
    pub workspace_id: String,
    pub chat_id: String,
    pub questions: Arc<QuestionBroker>,
    pub processes: Arc<ProcessManager>,
    pub checkpoint: Option<Arc<Checkpoint>>,
    pub approvals: Arc<ApprovalCache>,
    pub plan_store: Arc<PlanStore>,
    pub harness_root: PathBuf,
    pub scope: String,
    pub mode: String,
    /// Agent depth. Subagents run with non-interactive approvals.
    pub depth: u32,
    /// "read" blocks writes at dispatch; "write"/"worktree" allow them.
    pub access: String,
    /// MCP tools activated by `tool_search` for this run. Large MCP catalogs
    /// are deferred: only these names are sent to the provider.
    pub active_mcp: Arc<Mutex<HashSet<String>>>,
    /// Optional per-agent allowlist. `None` keeps the scope contract; `Some`
    /// is enforced both when advertising tools and at dispatch time.
    pub allowed_tools: Option<Arc<HashSet<String>>>,
    /// Optional per-agent MCP server allowlist (`["github"]`). Tool names are
    /// shaped `mcp__<server>__<tool>`.
    pub mcp_servers: Option<Arc<HashSet<String>>>,
    pub code_delta: Option<Arc<dyn Fn(stark_contract::Delta) + Send + Sync>>,
}

impl ToolContext {
    pub fn new(
        workspace: impl Into<PathBuf>,
        chat_id: impl Into<String>,
        questions: Arc<QuestionBroker>,
    ) -> Self {
        let workspace = workspace.into();
        Self {
            plan_store: Arc::new(PlanStore::new(&workspace)),
            harness_root: workspace.clone(),
            cwd: Arc::new(Mutex::new(workspace.clone())),
            read_paths: Arc::new(Mutex::new(HashSet::new())),
            workspace,
            workspace_id: "default".to_string(),
            chat_id: chat_id.into(),
            questions,
            processes: Arc::new(ProcessManager::new()),
            checkpoint: None,
            approvals: Arc::new(ApprovalCache::new()),
            scope: "agent".to_string(),
            mode: "build".to_string(),
            depth: 0,
            access: "write".to_string(),
            active_mcp: Arc::new(Mutex::new(HashSet::new())),
            allowed_tools: None,
            mcp_servers: None,
            code_delta: None,
        }
    }

    pub fn with_checkpoint(mut self, checkpoint: Arc<Checkpoint>) -> Self {
        self.checkpoint = Some(checkpoint);
        self
    }

    pub fn with_plan_store(mut self, plan_store: Arc<PlanStore>) -> Self {
        self.plan_store = plan_store;
        self
    }

    pub fn with_harness_root(mut self, harness_root: impl Into<PathBuf>) -> Self {
        self.harness_root = harness_root.into();
        self
    }

    pub fn with_scope(mut self, scope: impl Into<String>) -> Self {
        self.scope = scope.into();
        self
    }

    pub fn with_workspace_id(mut self, workspace_id: impl Into<String>) -> Self {
        self.workspace_id = workspace_id.into();
        self
    }

    pub fn with_mode(mut self, mode: impl Into<String>) -> Self {
        self.mode = mode.into();
        self
    }

    pub fn with_depth(mut self, depth: u32) -> Self {
        self.depth = depth;
        self
    }

    pub fn with_access(mut self, access: impl Into<String>) -> Self {
        self.access = access.into();
        self
    }

    pub fn with_allowed_tools(mut self, allowed: Option<Vec<String>>) -> Self {
        self.allowed_tools = allowed.map(|names| {
            Arc::new(
                names
                    .into_iter()
                    .map(|name| name.to_ascii_lowercase())
                    .collect(),
            )
        });
        self
    }

    pub fn with_mcp_servers(mut self, servers: Option<Vec<String>>) -> Self {
        self.mcp_servers = servers.map(|names| {
            Arc::new(
                names
                    .into_iter()
                    .map(|name| name.to_ascii_lowercase())
                    .collect(),
            )
        });
        self
    }

    fn tool_allowed(&self, name: &str) -> bool {
        // MCP tools are governed by the server allowlist, not the tool list:
        // `tools: ["read"]` + `mcp: ["github"]` must still expose GitHub tools.
        if name.starts_with("mcp__") {
            if let Some(servers) = &self.mcp_servers {
                let server = name
                    .trim_start_matches("mcp__")
                    .split("__")
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                return servers.contains(&server);
            }
            return true;
        }
        if let Some(allowed) = &self.allowed_tools
            && !allowed.contains(&name.to_ascii_lowercase())
        {
            return false;
        }
        true
    }

    pub fn with_code_delta(
        mut self,
        delta: Arc<dyn Fn(stark_contract::Delta) + Send + Sync>,
    ) -> Self {
        self.code_delta = Some(delta);
        self
    }

    pub fn current_dir(&self) -> PathBuf {
        self.cwd
            .lock()
            .map(|cwd| cwd.clone())
            .unwrap_or_else(|_| self.workspace.clone())
    }

    pub fn set_current_dir(&self, path: impl Into<PathBuf>) {
        if let Ok(mut cwd) = self.cwd.lock() {
            *cwd = path.into();
        }
    }

    pub fn mark_read(&self, path: impl Into<PathBuf>) {
        if let Ok(mut paths) = self.read_paths.lock() {
            paths.insert(path.into());
        }
    }

    pub fn was_read(&self, path: &Path) -> bool {
        self.read_paths
            .lock()
            .map(|paths| paths.contains(path))
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone)]
pub struct ToolOutcome {
    pub content: String,
    pub is_error: bool,
    pub exit_code: Option<i32>,
    pub plan: Option<BTreeMap<String, Vec<PlanItem>>>,
    pub children: Vec<Value>,
}

impl ToolOutcome {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            exit_code: None,
            plan: None,
            children: Vec::new(),
        }
    }

    pub fn err(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            exit_code: None,
            plan: None,
            children: Vec::new(),
        }
    }

    pub fn with_code(content: impl Into<String>, is_error: bool, exit_code: Option<i32>) -> Self {
        Self {
            content: content.into(),
            is_error,
            exit_code,
            plan: None,
            children: Vec::new(),
        }
    }

    pub fn with_plan(mut self, plan: BTreeMap<String, Vec<PlanItem>>) -> Self {
        self.plan = Some(plan);
        self
    }

    pub fn with_children(mut self, children: Vec<Value>) -> Self {
        self.children = children;
        self
    }
}

pub fn contract() -> &'static ToolSpecFile {
    CONTRACT.get_or_init(|| {
        ToolSpecFile::from_json_str(include_str!("../../../specs/tool-specs.json"))
            .expect("frozen tool contract must parse")
    })
}

pub fn tool_specs() -> Vec<Value> {
    tool_specs_for("build")
}

pub fn tool_specs_for(mode: &str) -> Vec<Value> {
    tool_specs_for_scope(mode, "agent")
}

pub fn tool_specs_for_scope(mode: &str, scope: &str) -> Vec<Value> {
    tool_specs_for_scope_with_access(mode, scope, "write")
}

pub fn tool_specs_for_scope_with_access(mode: &str, scope: &str, access: &str) -> Vec<Value> {
    tool_specs_for_run(mode, scope, access, &HashSet::new(), None, None)
}

/// Like [`tool_specs_for_scope_with_access`] but honors MCP deferral: when a
/// scope has more than [`MCP_EAGER_LIMIT`] MCP tools, only the ones activated
/// by `tool_search` are sent.
pub fn tool_specs_for_run(
    mode: &str,
    scope: &str,
    access: &str,
    active_mcp: &HashSet<String>,
    allowed_tools: Option<&[String]>,
    mcp_servers: Option<&[String]>,
) -> Vec<Value> {
    let scope = scope.trim().to_lowercase();
    let allowed = contract().scope_map.get(&scope);
    let agent_allowed: Option<HashSet<String>> = allowed_tools.map(|names| {
        names
            .iter()
            .map(|name| name.trim().to_ascii_lowercase())
            .filter(|name| !name.is_empty())
            .collect()
    });
    let mcp_allowed: Option<HashSet<String>> = mcp_servers.map(|names| {
        names
            .iter()
            .map(|name| name.trim().to_ascii_lowercase())
            .filter(|name| !name.is_empty())
            .collect()
    });
    let read_only = access == "read";
    // New v2 surfaces (developer, coder) keep the tool catalog stable across
    // plan/build for prefix-cache reuse; writes are blocked at dispatch.
    // Older scopes keep the legacy filter.
    let plan_filter = mode == "plan" && !matches!(scope.as_str(), "developer" | "coder");
    let mut specs: Vec<Value> = contract()
        .tools
        .iter()
        .filter(|tool| {
            tool.exposed
                && is_implemented(tool.name.as_str())
                && allowed.is_none_or(|names| names.contains(&tool.name))
                && agent_allowed
                    .as_ref()
                    .is_none_or(|names| names.contains(&tool.name.to_ascii_lowercase()))
                && !(plan_filter && is_plan_write(tool.name.as_str()))
                && !(read_only && {
                    // Orchestration tools act through server-owned state
                    // (registry, groups, schedules, notes, canvas), never
                    // direct filesystem writes — so the orchestrator keeps
                    // them even with read-only file access. Every other
                    // read-only scope still loses them.
                    let orchestration_exempt = scope == "orchestrator"
                        && matches!(
                            tool.name.as_str(),
                            "workspace"
                                | "create_group"
                                | "group_update"
                                | "schedule"
                                | "give_note"
                                | "canvas_mode"
                        );
                    !orchestration_exempt
                        && matches!(
                            tool.name.as_str(),
                            "edit_file"
                                | "run_code"
                                | "call_subagent"
                                | "edit"
                                | "spawn_agent"
                                | "message_agent"
                                | "interrupt_agent"
                                | "close_agent"
                                | "create_docx"
                                | "create_xlsx"
                                | "workspace"
                                | "create_group"
                                | "create_client"
                                | "house_set_device"
                                | "house_run_scene"
                                | "look_at_screen"
                                | "set_screen_share"
                                | "group_update"
                                | "schedule"
                                | "canvas_mode"
                                | "give_note"
                        )
                })
        })
        .map(|tool| {
            let mut spec = json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                }
            });
            if tool.name == "run_command"
                && let Some(properties) = spec
                    .pointer_mut("/function/parameters/properties")
                    .and_then(Value::as_object_mut)
            {
                properties.insert(
                    "tty".to_string(),
                    json!({
                        "type": "boolean",
                        "description": "Allocate a ConPTY for interactive commands (Windows)."
                    }),
                );
                properties.insert(
                    "yield_time_ms".to_string(),
                    json!({
                        "type": "integer",
                        "description": "Return a session ID if the command is still running after this many ms (250-30000). Omit to block up to timeout."
                    }),
                );
                properties.insert(
                    "sandbox_permissions".to_string(),
                    json!({
                        "type": "string",
                        "enum": ["use_default", "require_escalated"],
                        "description": "Request user approval before running this command."
                    }),
                );
                properties.insert(
                    "justification".to_string(),
                    json!({
                        "type": "string",
                        "description": "User-facing reason for requiring approval."
                    }),
                );
                properties.insert(
                    "prefix_rule".to_string(),
                    json!({
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Command prefix to approve for the rest of this run, for example git pull."
                    }),
                );
                properties.insert(
                    "process_action".to_string(),
                    json!({
                        "type": "string",
                        "enum": ["list", "logs", "kill", "wait", "ready", "send", "kill_all"],
                        "description": "'list' | 'logs' | 'kill' | 'wait' | 'ready' | 'send' | 'kill_all' (command carries a session ID or background process name)."
                    }),
                );
            }
            spec
        })
        .collect();
    let mut mcp_specs = mcp::tool_specs_for_scope(&scope);
    if let Some(servers) = &mcp_allowed {
        mcp_specs.retain(|spec| {
            let name = spec
                .get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let server = name
                .trim_start_matches("mcp__")
                .split("__")
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            servers.contains(&server)
        });
    }
    if mcp_specs.len() > MCP_EAGER_LIMIT {
        mcp_specs.retain(|spec| {
            spec.get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
                .is_some_and(|name| active_mcp.contains(name))
        });
    }
    specs.extend(mcp_specs);
    specs
}

pub fn plan_mode_denial(mode: &str, name: &str, args: &Value) -> Option<String> {
    if mode != "plan" {
        return None;
    }
    if is_plan_write(name) {
        return Some(format!(
            "[BLOCKED] Plan mode is read-only. Type /build to leave plan mode. ({name} is a write-class tool.)"
        ));
    }
    if name == "run_command" || name == "bash" {
        let process_action = args
            .get("process_action")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let background = args
            .get("background")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || args
                .get("run_in_background")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        if !process_action.is_empty() || background {
            return Some(
                "[BLOCKED] Plan mode is read-only. Type /build to leave plan mode. (background processes and process actions are blocked.)"
                    .to_string(),
            );
        }
        let command = args.get("command").and_then(Value::as_str).unwrap_or("");
        if let Err(reason) = stark_policy::check_read_only(command) {
            return Some(format!(
                "[BLOCKED] Plan mode is read-only. Type /build to leave plan mode. ({name} with write/side-effect verbs is blocked: {reason})"
            ));
        }
    }
    if name == "job" {
        let action = args.get("action").and_then(Value::as_str).unwrap_or("");
        if action == "kill" {
            return Some(
                "[BLOCKED] Plan mode is read-only. Type /build to leave plan mode. (job kill is blocked.)"
                    .to_string(),
            );
        }
    }
    None
}

pub fn read_only_denial(name: &str, args: &Value, ctx: &ToolContext) -> Option<String> {
    if ctx.access != "read" {
        return None;
    }
    if is_read_blocked(name) {
        return Some(format!(
            "[BLOCKED] Read-only agent cannot call {name}. Report findings or a proposed patch instead."
        ));
    }
    match name {
        "run_command" | "bash" => {
            let background = args
                .get("background")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || args
                    .get("run_in_background")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
            let process_action = args
                .get("process_action")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if background || !process_action.is_empty() {
                return Some(
                    "[BLOCKED] Read-only agents may not start background jobs or manage processes. Use a foreground read-only command."
                        .to_string(),
                );
            }
            let command = args.get("command").and_then(Value::as_str).unwrap_or("");
            if let Err(reason) = stark_policy::check_read_only(command) {
                Some(format!(
                    "[BLOCKED] Read-only agent cannot run mutating commands ({reason}). Use read/grep."
                ))
            } else {
                None
            }
        }
        "job" => {
            let action = args.get("action").and_then(Value::as_str).unwrap_or("");
            match action {
                "kill" => Some(
                    "[BLOCKED] Read-only agent cannot kill jobs. Report findings instead."
                        .to_string(),
                ),
                _ => None,
            }
        }
        "browser_act" => {
            let action = args.get("action").and_then(Value::as_str).unwrap_or("");
            match action {
                "navigate" | "extract" | "screenshot" | "close" | "" => None,
                _ => Some(format!(
                    "[BLOCKED] Read-only agent cannot use browser_act '{action}'. Allowed: navigate, extract, screenshot, close."
                )),
            }
        }
        "memory" => {
            let action = args
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("index");
            match action {
                "remember" | "forget" | "pin" => Some(
                    "[BLOCKED] Read-only agent cannot write memory. Use read/search instead."
                        .to_string(),
                ),
                _ => None,
            }
        }
        "agent" => {
            let action = args.get("action").and_then(Value::as_str).unwrap_or("list");
            match action {
                "merge" | "discard" => Some(format!(
                    "[BLOCKED] Read-only agent cannot use agent action '{action}'."
                )),
                _ => None,
            }
        }
        _ => None,
    }
}

fn argument_matches(value: &Value, kind: &str) -> bool {
    match kind {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.is_i64() || value.is_u64(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => true,
    }
}

pub async fn dispatch(name: &str, args: &Value, ctx: &ToolContext) -> ToolOutcome {
    if !ctx.tool_allowed(name) {
        return ToolOutcome::err(format!(
            "[BLOCKED] Tool '{name}' is not enabled for this agent. Change the agent's tools config or continue without it."
        ));
    }
    // Enforce the frozen scope contract at dispatch, not only when
    // advertising. Without this a hallucinated legacy name (read_file,
    // run_command, ...) skipped the v2 permission gates.
    if !name.starts_with("mcp__")
        && let Some(allowed) = contract().scope_map.get(&ctx.scope)
        && !allowed.iter().any(|tool| tool == name)
    {
        return ToolOutcome::err(format!(
            "[BLOCKED] Tool '{name}' is not in the '{}' scope contract. Use one of: {}",
            ctx.scope,
            allowed.join(", ")
        ));
    }
    if ctx.depth > 0 && matches!(name, "ask_user_question" | "ask_user") {
        return ToolOutcome::err(
            "[BLOCKED] Subagents run non-interactively: the user cannot answer here. Do not ask questions. Decide with the information you have, or report the decision/blocker to your parent in your final message.",
        );
    }
    const LEGACY_REPLACEMENTS: &[(&str, &str)] = &[
        ("read_file", "read"),
        ("edit_file", "edit"),
        ("search_code", "grep"),
        ("run_command", "bash"),
        ("ask_user", "ask_user_question"),
        ("call_subagent", "spawn_agent"),
        ("todo_write", "update_plan"),
    ];
    if let Some((legacy, replacement)) = LEGACY_REPLACEMENTS
        .iter()
        .find(|(legacy, _)| *legacy == name)
    {
        return ToolOutcome::err(format!(
            "[BLOCKED] Tool '{legacy}' was removed. Use '{replacement}' instead."
        ));
    }
    if let Some(tool) = contract().tool(name)
        && let Some(required) = tool.required.iter().find(|required| {
            args.get(required.as_str())
                .is_none_or(serde_json::Value::is_null)
        })
    {
        let schema = tool.parameters.pointer(&format!("/properties/{required}"));
        let kind = schema
            .and_then(|value| value.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("value");
        let hint = schema
            .and_then(|value| value.get("description"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let received = args
            .get(required.as_str())
            .map(Value::to_string)
            .unwrap_or_else(|| "nothing".to_string());
        return ToolOutcome::err(format!(
            "Error: {name} requires '{required}' ({kind}). Got: {received}. {hint} Retry with corrected arguments."
        ));
    }
    if let Some(tool) = contract().tool(name) {
        for required in &tool.required {
            let Some(value) = args.get(required.as_str()) else {
                continue;
            };
            let Some(kind) = tool
                .parameters
                .pointer(&format!("/properties/{required}/type"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            if !argument_matches(value, kind) {
                return ToolOutcome::err(format!(
                    "Error: {name}.{required} must be {kind}, got {}. Retry with corrected arguments.",
                    value
                ));
            }
        }
    }
    if let Some(reason) = plan_mode_denial(&ctx.mode, name, args) {
        return ToolOutcome::err(reason);
    }
    if ctx.access == "read"
        && let Some(reason) = read_only_denial(name, args, ctx)
    {
        return ToolOutcome::err(reason);
    }
    if name.starts_with("mcp__") {
        if ctx.access == "read" {
            return ToolOutcome::err(
                "[BLOCKED] Read-only agent cannot call MCP tools. Report findings instead.",
            );
        }
        return mcp::call(name, args, ctx).await;
    }
    if name == "run_code" {
        return codemode::handle(ctx, args).await;
    }
    match name {
        "bash" => ds_shell::bash(ctx, args).await,
        "job" => ds_shell::job(ctx, args).await,
        "read" => ds_fs::read(ctx, args).await,
        "apply_patch" => match args
            .get("input")
            .or_else(|| args.get("patch"))
            .and_then(Value::as_str)
        {
            Some(patch) if !patch.trim().is_empty() => crate::patch::apply_patch(ctx, patch).await,
            _ => ToolOutcome::err("Error: apply_patch requires a non-empty 'patch' string"),
        },
        "edit" => ds_fs::edit(ctx, args).await,
        "glob" => ds_fs::glob(ctx, args).await,
        "grep" => ds_fs::grep(ctx, args).await,
        "ask_user_question" => ds_session::ask_user_question(ctx, args).await,
        "skill" => skills::skill(ctx, args).await,
        "tool_search" => mcp::search_tools(ctx, args).await,
        "web_search" => ds_web::web_search(ctx, args).await,
        "web_fetch" => ds_web::web_fetch(ctx, args).await,
        "update_plan" => update_plan(ctx, args).await,
        "browser_act" => browser::browser_act(ctx, args).await,
        "house_get_state" => house::house_get_state(ctx, args).await,
        "house_get_device" => house::house_get_device(ctx, args).await,
        "house_set_device" => house::house_set_device(ctx, args).await,
        "house_run_scene" => house::house_run_scene(ctx, args).await,
        "house_get_history" => house::house_get_history(ctx, args).await,
        "dfd_bridge" => dfd::dfd_bridge(ctx, args).await,
        "create_client" => dfd::create_client(ctx, args).await,
        "files_changed" => changes::files_changed(ctx, args).await,
        "look_at_screen" => screen::look_at_screen(ctx, args).await,
        "set_screen_share" => screen::set_screen_share(ctx, args).await,
        "read_pdf" => docs::read_pdf(ctx, args).await,
        "create_docx" => docs::create_docx(ctx, args).await,
        "create_xlsx" => docs::create_xlsx(ctx, args).await,
        "memory" => memory::handle(ctx, args).await,
        other => ToolOutcome::err(format!(
            "Error: tool '{other}' is not implemented in the Rust Phase 1 backend"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(workspace: &std::path::Path) -> ToolContext {
        ToolContext::new(workspace, "test-chat", Arc::new(QuestionBroker::new()))
    }

    fn tool_names(specs: &[Value]) -> Vec<&str> {
        specs
            .iter()
            .filter_map(|spec| spec["function"]["name"].as_str())
            .collect()
    }

    #[test]
    fn agent_allowlists_gate_advertised_tools_and_dispatch() {
        let ctx =
            context(std::path::Path::new(".")).with_allowed_tools(Some(vec!["read".to_string()]));
        assert!(ctx.tool_allowed("read"));
        assert!(!ctx.tool_allowed("bash"));
        assert!(ctx.tool_allowed("mcp__github__create_issue"));

        let ctx = context(std::path::Path::new("."))
            .with_allowed_tools(Some(vec!["read".to_string()]))
            .with_mcp_servers(Some(vec!["github".to_string()]));
        assert!(ctx.tool_allowed("read"));
        assert!(!ctx.tool_allowed("bash"));
        assert!(ctx.tool_allowed("mcp__github__create_issue"));
        assert!(!ctx.tool_allowed("mcp__sentry__list_issues"));

        let specs = tool_specs_for_run(
            "build",
            "developer",
            "write",
            &HashSet::new(),
            Some(&["read".to_string()]),
            Some(&[]),
        );
        let names = tool_names(&specs);
        assert_eq!(names, vec!["read"]);
    }

    #[test]
    fn tool_specs_exposes_shell_fields() {
        let specs = tool_specs();
        let shell = specs
            .iter()
            .find(|spec| spec["function"]["name"] == "bash")
            .expect("bash spec");
        let properties = shell
            .pointer("/function/parameters/properties")
            .expect("bash properties");
        assert!(properties.get("workdir").is_some());
        assert!(properties.get("timeoutMs").is_some());
        assert!(tool_names(&specs).contains(&"update_plan"));
    }

    #[test]
    fn plan_mode_hides_write_tools_and_guards_shell() {
        let build = tool_specs_for("build");
        let plan = tool_specs_for("plan");
        assert!(tool_names(&build).contains(&"edit"));
        assert!(tool_names(&build).contains(&"spawn_agent"));
        assert!(!tool_names(&plan).contains(&"edit"));
        assert!(!tool_names(&plan).contains(&"spawn_agent"));
        assert!(!tool_names(&plan).contains(&"run_code"));
        assert!(!tool_names(&plan).contains(&"agent"));
        assert!(tool_names(&plan).contains(&"update_plan"));
        assert!(tool_names(&plan).contains(&"bash"));

        assert!(
            plan_mode_denial(
                "plan",
                "edit",
                &json!({"file_path": "a.txt", "new_string": "x"})
            )
            .is_some()
        );
        assert!(plan_mode_denial("plan", "bash", &json!({"command": "ls"})).is_none());
        assert!(
            plan_mode_denial("plan", "bash", &json!({"command": "echo x > out.txt"})).is_some()
        );
        assert!(plan_mode_denial("build", "edit", &json!({})).is_none());
        assert!(plan_mode_denial("plan", "run_code", &json!({"code": "return 1"})).is_some());
        assert!(plan_mode_denial("plan", "agent", &json!({"action": "spawn"})).is_some());
    }

    #[test]
    fn scope_limits_tools_to_agent_acl() {
        let developer = tool_specs_for_scope("build", "developer");
        let names = tool_names(&developer);
        assert!(names.contains(&"read"));
        assert!(names.contains(&"apply_patch"));
        assert!(names.contains(&"edit"));
        assert!(names.contains(&"spawn_agent"));
        assert!(names.contains(&"update_plan"));
        assert!(names.contains(&"web_fetch"));
        assert!(!names.contains(&"read_file"));
        assert!(!names.contains(&"read_pdf"));

        let general = tool_specs_for_scope("build", "general");
        let names = tool_names(&general);
        assert_eq!(names.len(), 19);
        assert!(names.contains(&"bash"));
        assert!(names.contains(&"read"));
        assert!(names.contains(&"glob"));
        assert!(names.contains(&"grep"));
        assert!(names.contains(&"update_plan"));
        assert!(names.contains(&"read_pdf"));
        assert!(names.contains(&"create_docx"));
        assert!(names.contains(&"look_at_screen"));
        assert!(names.contains(&"set_screen_share"));
        assert!(!names.contains(&"edit_file"));
        assert!(!names.contains(&"read_file"));
        assert!(!names.contains(&"run_command"));

        let dfd = tool_specs_for_scope("build", "dfd");
        let names = tool_names(&dfd);
        assert_eq!(names.len(), 9);
        assert!(names.contains(&"workspace"));
        assert!(names.contains(&"dfd_bridge"));
        assert!(names.contains(&"create_client"));
        assert!(names.contains(&"files_changed"));
        assert!(names.contains(&"bash"));
        assert!(!names.contains(&"run_command"));

        let house = tool_specs_for_scope("build", "house");
        let names = tool_names(&house);
        assert_eq!(names.len(), 11);
        assert!(names.contains(&"house_set_device"));
        assert!(names.contains(&"house_run_scene"));
        assert!(names.contains(&"bash"));
        assert!(!names.contains(&"run_command"));

        let orchestrator = tool_specs_for_scope("build", "orchestrator");
        let names = tool_names(&orchestrator);
        assert_eq!(names.len(), 16);
        assert!(names.contains(&"canvas_mode"));
        assert!(names.contains(&"schedule"));
        assert!(names.contains(&"create_group"));
        assert!(names.contains(&"group_board"));
        assert!(names.contains(&"group_item_detail"));
        assert!(names.contains(&"group_events"));
        assert!(names.contains(&"group_update"));
        assert!(names.contains(&"workspace"));
        assert!(names.contains(&"delegate"));
        assert!(names.contains(&"delegation_status"));
        assert!(names.contains(&"job_message"));
        assert!(names.contains(&"job_cancel"));
        assert!(names.contains(&"give_note"));
        // The manager inspects only through read-only bash and coordinates.
        for tool in ["bash", "memory", "ask_user_question"] {
            assert!(names.contains(&tool), "orchestrator missing {tool}");
        }
        assert!(!names.contains(&"ask_user"));
        assert!(!names.contains(&"agent"));
        assert_eq!(
            tool_names(&tool_specs_for_scope("build", "agent")).len(),
            48
        );
    }

    #[test]
    fn coder_scope_is_developer_plus_run_code() {
        let developer_specs = tool_specs_for_scope("build", "developer");
        let developer: std::collections::BTreeSet<&str> =
            tool_names(&developer_specs).into_iter().collect();
        let coder_specs = tool_specs_for_scope("build", "coder");
        let coder: std::collections::BTreeSet<&str> =
            tool_names(&coder_specs).into_iter().collect();
        assert_eq!(developer.len(), 17);
        assert_eq!(coder.len(), 18);
        assert!(coder.contains("run_code"));
        assert!(!developer.contains("run_code"));
        let extra: Vec<&&str> = coder.difference(&developer).collect();
        assert_eq!(extra, vec![&"run_code"]);
        // Plan mode keeps the catalog stable for the coder surface.
        let plan_specs = tool_specs_for_scope("plan", "coder");
        let plan = tool_names(&plan_specs);
        assert!(plan.contains(&"run_code"));
    }

    #[tokio::test]
    async fn subagents_cannot_ask_the_user() {
        let dir = tempfile::tempdir().unwrap();
        let child = context(dir.path()).with_depth(1);
        let out = dispatch(
            "ask_user_question",
            &json!({"questions": [{"question": "which image source?", "options": ["svg", "photos"]}]}),
            &child,
        )
        .await;
        assert!(out.is_error, "{}", out.content);
        assert!(
            out.content.contains("Subagents run non-interactively"),
            "{}",
            out.content
        );
        // The root agent keeps the channel.
        let root = context(dir.path()).with_depth(0);
        assert!(read_only_denial("ask_user_question", &json!({}), &root).is_none());
    }

    #[test]
    fn read_only_access_hides_write_tools_and_blocks_dispatch() {
        let read = tool_specs_for_scope_with_access("build", "explore", "read");
        let names = tool_names(&read);
        assert!(!names.contains(&"edit"));
        assert!(!names.contains(&"run_code"));
        assert!(names.contains(&"read"));
        assert!(names.contains(&"grep"));
        assert!(names.contains(&"bash"));

        // Orchestrator keeps its orchestration tools with read-only file
        // access: they act through server-owned state, not direct writes.
        let orch_read = tool_specs_for_scope_with_access("build", "orchestrator", "read");
        let orch_names = tool_names(&orch_read);
        for tool in [
            "workspace",
            "create_group",
            "group_update",
            "schedule",
            "give_note",
            "canvas_mode",
            "bash",
            "memory",
            "ask_user_question",
        ] {
            assert!(
                orch_names.contains(&tool),
                "orchestrator lost {tool} with read access"
            );
        }
        // ...but still loses direct filesystem writes.
        for tool in ["edit", "create_docx"] {
            assert!(
                !orch_names.contains(&tool),
                "orchestrator kept filesystem write {tool} with read access"
            );
        }
        // Other read-only scopes still lose the orchestration tools.
        assert!(!names.contains(&"schedule"));
        assert!(!names.contains(&"canvas_mode"));

        let ctx = ToolContext::new(
            std::path::Path::new("."),
            "chat",
            Arc::new(QuestionBroker::new()),
        )
        .with_access("read");
        assert!(read_only_denial("edit", &json!({"file_path": "a"}), &ctx).is_some());
        assert!(read_only_denial("bash", &json!({"command": "echo x > out.txt"}), &ctx).is_some());
        assert!(read_only_denial("bash", &json!({"command": "ls"}), &ctx).is_none());
        assert!(
            read_only_denial(
                "bash",
                &json!({"command": "ls", "run_in_background": true}),
                &ctx
            )
            .is_some()
        );
        assert!(
            read_only_denial(
                "bash",
                &json!({"command": "ls", "process_action": "kill"}),
                &ctx
            )
            .is_some()
        );
        assert!(
            read_only_denial(
                "browser_act",
                &json!({"action": "click", "selector": "a"}),
                &ctx
            )
            .is_some()
        );
        assert!(
            read_only_denial(
                "browser_act",
                &json!({"action": "extract", "selector": "a"}),
                &ctx
            )
            .is_none()
        );
        assert!(
            read_only_denial("memory", &json!({"action": "remember", "text": "x"}), &ctx).is_some()
        );
        assert!(
            read_only_denial("memory", &json!({"action": "read", "topic": "facts"}), &ctx)
                .is_none()
        );
    }

    #[tokio::test]
    async fn plan_mode_dispatch_blocks_edits_but_allows_reads() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        let ctx = context(dir.path()).with_mode("plan");

        let edit = dispatch(
            "edit",
            &json!({"file_path": "a.txt", "old_string": "hello", "new_string": "bye"}),
            &ctx,
        )
        .await;
        assert!(edit.is_error);
        assert!(edit.content.contains("[BLOCKED]"));

        let read = dispatch("read", &json!({"file_path": "a.txt"}), &ctx).await;
        assert!(!read.is_error, "{}", read.content);
    }

    #[tokio::test]
    async fn read_image_queues_inline_vision() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        std::fs::write(dir.path().join("shot.png"), b"\x89PNG\r\n\x1a\nfake").unwrap();
        let read = dispatch("read", &json!({"file_path": "shot.png"}), &ctx).await;
        assert!(!read.is_error, "{}", read.content);
        assert!(read.content.contains("[IMAGE]"));
        let queued = crate::screen::drain_images("test-chat");
        assert_eq!(queued.len(), 1);
        assert!(queued[0].url.starts_with("data:image/png;base64,"));
    }

    #[tokio::test]
    async fn read_edit_and_search_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());

        let write = dispatch(
            "apply_patch",
            &json!({"input": "*** Begin Patch\n*** Add File: src/demo.txt\n+alpha\n+beta\n*** End Patch"}),
            &ctx,
        )
        .await;
        assert!(!write.is_error, "{}", write.content);

        let read = dispatch("read", &json!({"file_path": "src/demo.txt"}), &ctx).await;
        assert!(!read.is_error, "{}", read.content);
        assert!(read.content.contains("[FILE] src/demo.txt"));
        assert!(read.content.contains("alpha"));

        let edit = dispatch(
            "edit",
            &json!({
                "file_path": "src/demo.txt",
                "old_string": "beta",
                "new_string": "gamma"
            }),
            &ctx,
        )
        .await;
        assert!(!edit.is_error, "{}", edit.content);
        assert!(edit.content.contains("[OK] Edited"));

        let search = dispatch("grep", &json!({"pattern": "gamma", "path": "src"}), &ctx).await;
        assert!(!search.is_error, "{}", search.content);
        assert!(search.content.contains("src/demo.txt:2: gamma"));
    }

    #[tokio::test]
    async fn read_rejects_path_escape() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let read = dispatch("read", &json!({"file_path": "../../outside.txt"}), &ctx).await;
        assert!(read.is_error);
        assert!(read.content.contains("escapes the workspace"));
    }

    #[tokio::test]
    async fn dispatch_enforces_the_scope_contract() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        let scoped = context(dir.path()).with_scope("developer");
        let out_of_scope = dispatch("read_file", &json!({"file_path": "a.txt"}), &scoped).await;
        assert!(out_of_scope.is_error, "{}", out_of_scope.content);
        assert!(
            out_of_scope.content.contains("scope contract"),
            "{}",
            out_of_scope.content
        );
        let legacy = dispatch(
            "read_file",
            &json!({"file_path": "a.txt"}),
            &context(dir.path()),
        )
        .await;
        assert!(legacy.is_error, "{}", legacy.content);
        assert!(legacy.content.contains("was removed"), "{}", legacy.content);
        let current = dispatch("read", &json!({"file_path": "a.txt"}), &scoped).await;
        assert!(!current.is_error, "{}", current.content);
    }

    #[tokio::test]
    async fn dispatch_rejects_wrong_argument_types() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let out = dispatch("read", &json!({"file_path": 7}), &ctx).await;
        assert!(out.is_error, "{}", out.content);
        assert!(out.content.contains("must be string"), "{}", out.content);
    }

    #[tokio::test]
    async fn bash_returns_exit_and_output() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let result = dispatch(
            "bash",
            &json!({"description": "Print a value", "command": "printf 'hello'"}),
            &ctx,
        )
        .await;
        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("[exit code: 0]"));
        assert!(result.content.contains("hello"));
    }

    #[tokio::test]
    async fn bash_supports_resumable_background_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let started = dispatch(
            "bash",
            &json!({
                "description": "Print a value after a short delay",
                "command": "sleep 0.5; printf 'session-done\\n'",
                "run_in_background": true
            }),
            &ctx,
        )
        .await;
        assert!(!started.is_error, "{}", started.content);
        let job_id = started
            .content
            .split("\"jobId\":")
            .nth(1)
            .and_then(|rest| rest.split(',').next())
            .map(|value| value.trim().trim_matches('"').to_string())
            .expect("job id");
        let polled = dispatch(
            "job",
            &json!({"action": "output", "job_id": job_id, "wait": true, "timeout_ms": 5000}),
            &ctx,
        )
        .await;
        assert!(!polled.is_error, "{}", polled.content);
        assert!(
            polled.content.contains("session-done"),
            "{}",
            polled.content
        );
    }

    #[tokio::test]
    async fn background_job_logs_and_kill() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let command = "printf 'bg-ready\\n'; sleep 2";
        let started = dispatch(
            "bash",
            &json!({
                "description": "Print a ready marker and wait",
                "command": command,
                "run_in_background": true
            }),
            &ctx,
        )
        .await;
        assert!(!started.is_error, "{}", started.content);
        let job_id = started
            .content
            .split("\"jobId\":")
            .nth(1)
            .and_then(|rest| rest.split(',').next())
            .map(|value| value.trim().trim_matches('"').to_string())
            .expect("job id");

        let logs = dispatch(
            "job",
            &json!({"action": "output", "job_id": job_id, "wait": true, "timeout_ms": 5000}),
            &ctx,
        )
        .await;
        assert!(logs.content.contains("bg-ready"), "{}", logs.content);

        let killed = dispatch("job", &json!({"action": "kill", "job_id": job_id}), &ctx).await;
        assert!(!killed.is_error, "{}", killed.content);
    }

    #[tokio::test]
    async fn bash_blocks_destructive_policy_hits() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let result = dispatch(
            "bash",
            &json!({"description": "Dangerous command", "command": "rm -rf /"}),
            &ctx,
        )
        .await;
        assert!(result.is_error);
        assert!(result.content.contains("[BLOCKED]"));
    }

    #[tokio::test]
    async fn ask_user_resolves_through_broker() {
        let dir = tempfile::tempdir().unwrap();
        let broker = Arc::new(QuestionBroker::new());
        let ctx = ToolContext::new(dir.path(), "chat", broker.clone());

        let task = tokio::spawn({
            let ctx = ctx.clone();
            async move {
                dispatch(
                    "ask_user_question",
                    &json!({
                        "questions": [{"id": "q1", "question": "Continue?"}]
                    }),
                    &ctx,
                )
                .await
            }
        });

        for _ in 0..50 {
            let pending = broker.pending_batches();
            if let Some(batch) = pending.first() {
                let batch_id = batch["batch_id"].as_str().unwrap();
                assert!(broker.answer(
                    batch_id,
                    vec![json!({"id": "q1", "selected": ["Yes"], "custom": ""})]
                ));
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        let result = task.await.unwrap();
        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("[USER ANSWERS]"));
        assert!(result.content.contains("q1: Yes"));
    }
}

#[cfg(test)]
mod v2_surface_tests {
    use super::*;

    fn names_for(scope: &str) -> Vec<String> {
        tool_specs_for_scope("build", scope)
            .iter()
            .filter_map(|spec| spec["function"]["name"].as_str().map(str::to_string))
            .collect()
    }

    #[test]
    fn developer_v2_surface_matches_the_frozen_manifest() {
        let names: Vec<String> = tool_specs_for_scope("build", "developer")
            .iter()
            .filter_map(|spec| spec["function"]["name"].as_str().map(str::to_string))
            .collect();
        let expected = [
            "bash",
            "job",
            "read",
            "apply_patch",
            "edit",
            "glob",
            "grep",
            "ask_user_question",
            "skill",
            "web_search",
            "web_fetch",
            "spawn_agent",
            "message_agent",
            "wait_agent",
            "interrupt_agent",
            "close_agent",
            "update_plan",
        ];
        assert_eq!(
            names,
            expected
                .iter()
                .map(|name| name.to_string())
                .collect::<Vec<_>>()
        );
        for legacy in [
            "read_file",
            "edit_file",
            "run_command",
            "search_code",
            "ask_user",
            "todo_write",
            "call_subagent",
            "run_code",
            "agent",
            "memory",
            "working_directory",
            "exit_plan_mode",
            "read_image",
            "job_kill",
            "job_list",
            "job_output",
            "send_message",
            "followup_task",
        ] {
            assert!(
                !names.iter().any(|name| name == legacy),
                "legacy Developer tool {legacy}"
            );
        }
    }

    #[test]
    fn no_scope_advertises_legacy_tool_names() {
        let legacy = [
            "read_file",
            "edit_file",
            "search_code",
            "run_command",
            "ask_user",
            "call_subagent",
            "todo_write",
        ];
        for scope in [
            "developer",
            "coder",
            "general",
            "house",
            "dfd",
            "orchestrator",
            "devops",
            "explore",
            "ui",
            "agent",
        ] {
            let names = names_for(scope);
            for name in legacy {
                assert!(
                    !names.iter().any(|candidate| candidate == name),
                    "{scope} still advertises legacy tool {name}"
                );
            }
        }
    }

    #[test]
    fn developer_plan_mode_keeps_the_same_catalog() {
        let build: Vec<String> = tool_specs_for_scope("build", "developer")
            .iter()
            .filter_map(|spec| spec["function"]["name"].as_str().map(str::to_string))
            .collect();
        let plan: Vec<String> = tool_specs_for_scope("plan", "developer")
            .iter()
            .filter_map(|spec| spec["function"]["name"].as_str().map(str::to_string))
            .collect();
        assert_eq!(build, plan);
    }
}
