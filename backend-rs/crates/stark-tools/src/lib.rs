mod approvals;
mod ask;
mod browser;
mod changes;
mod checkpoint;
mod codemode;
mod command;
mod dfd;
mod docs;
mod ds_fs;
mod ds_session;
mod ds_shell;
mod ds_web;
mod edit;
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
mod search;
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
pub use stark_sandbox::{ApprovalPolicy, SandboxMode, SandboxProfile};

/// Prompt line stating the requested sandbox mode and the real backend.
pub fn sandbox_prompt_line(workspace: &Path) -> String {
    SandboxProfile::from_env(workspace).prompt_line()
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
pub use skills::catalog_block_for;
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
    /// Agent depth. Codex subagents run with non-interactive approvals.
    pub depth: u32,
    /// "read" blocks writes at dispatch; "write"/"worktree" allow them.
    pub access: String,
    /// MCP tools activated by `tool_search` for this run. Large MCP catalogs
    /// are deferred: only these names are sent to the provider.
    pub active_mcp: Arc<Mutex<HashSet<String>>>,
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
    tool_specs_for_run(mode, scope, access, &HashSet::new())
}

/// Like [`tool_specs_for_scope_with_access`] but honors MCP deferral: when a
/// scope has more than [`MCP_EAGER_LIMIT`] MCP tools, only the ones activated
/// by `tool_search` are sent.
pub fn tool_specs_for_run(
    mode: &str,
    scope: &str,
    access: &str,
    active_mcp: &HashSet<String>,
) -> Vec<Value> {
    let scope = scope.trim().to_lowercase();
    let allowed = contract().scope_map.get(&scope);
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
                && !(plan_filter && is_plan_write(tool.name.as_str()))
                && !(read_only
                    && matches!(
                        tool.name.as_str(),
                        "edit_file"
                            | "run_code"
                            | "call_subagent"
                            | "write"
                            | "edit"
                            | "todo_write"
                            | "spawn_agent"
                            | "message_agent"
                            | "interrupt_agent"
                            | "close_agent"
                            | "create_docx"
                            | "create_xlsx"
                            | "create_coding_project"
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
                    ))
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

pub async fn dispatch(name: &str, args: &Value, ctx: &ToolContext) -> ToolOutcome {
    if ctx.depth > 0 && matches!(name, "ask_user_question" | "ask_user") {
        return ToolOutcome::err(
            "[BLOCKED] Subagents run non-interactively: the user cannot answer here. Do not ask questions. Decide with the information you have, or report the decision/blocker to your parent in your final message.",
        );
    }
    if let Some(tool) = contract().tool(name)
        && let Some(required) = tool
            .required
            .iter()
            .find(|required| args.get(required.as_str()).is_none())
    {
        return ToolOutcome::err(format!("Error: {name} requires {required}"));
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
        "read_file" => read::read_file(ctx, args).await,
        "edit_file" => edit::edit_file(ctx, args).await,
        "search_code" => search::search_code(ctx, args).await,
        "run_command" => command::run_command(ctx, args).await,
        "bash" => ds_shell::bash(ctx, args).await,
        "job" => ds_shell::job(ctx, args).await,
        "read" => ds_fs::read(ctx, args).await,
        "write" => ds_fs::write(ctx, args).await,
        "edit" => ds_fs::edit(ctx, args).await,
        "glob" => ds_fs::glob(ctx, args).await,
        "grep" => ds_fs::grep(ctx, args).await,
        "todo_write" => ds_session::todo_write(ctx, args).await,
        "ask_user_question" => ds_session::ask_user_question(ctx, args).await,
        "skill" => skills::skill(ctx, args).await,
        "tool_search" => mcp::search_tools(ctx, args).await,
        "web_search" => ds_web::web_search(ctx, args).await,
        "web_fetch" => ds_web::web_fetch(ctx, args).await,
        "ask_user" => ask::ask_user(ctx, args).await,
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
    fn tool_specs_exposes_unified_exec_fields() {
        let specs = tool_specs();
        let run = specs
            .iter()
            .find(|spec| spec["function"]["name"] == "run_command")
            .expect("run_command spec");
        let properties = run
            .pointer("/function/parameters/properties")
            .expect("run_command properties");
        assert!(properties.get("tty").is_some());
        assert!(properties.get("yield_time_ms").is_some());
        assert!(tool_names(&specs).contains(&"update_plan"));
    }

    #[test]
    fn plan_mode_hides_write_tools_and_guards_shell() {
        let build = tool_specs_for("build");
        let plan = tool_specs_for("plan");
        assert!(tool_names(&build).contains(&"edit_file"));
        assert!(tool_names(&build).contains(&"call_subagent"));
        assert!(!tool_names(&plan).contains(&"edit_file"));
        assert!(!tool_names(&plan).contains(&"call_subagent"));
        assert!(!tool_names(&plan).contains(&"run_code"));
        assert!(!tool_names(&plan).contains(&"agent"));
        assert!(tool_names(&plan).contains(&"update_plan"));
        assert!(tool_names(&plan).contains(&"run_command"));

        assert!(
            plan_mode_denial(
                "plan",
                "edit_file",
                &json!({"file_path": "a.txt", "new_string": "x"})
            )
            .is_some()
        );
        assert!(plan_mode_denial("plan", "run_command", &json!({"command": "ls"})).is_none());
        assert!(
            plan_mode_denial(
                "plan",
                "run_command",
                &json!({"command": "echo x > out.txt"})
            )
            .is_some()
        );
        assert!(plan_mode_denial("build", "edit_file", &json!({})).is_none());
        assert!(plan_mode_denial("plan", "run_code", &json!({"code": "return 1"})).is_some());
        assert!(plan_mode_denial("plan", "agent", &json!({"action": "spawn"})).is_some());
    }

    #[test]
    fn scope_limits_tools_to_agent_acl() {
        let developer = tool_specs_for_scope("build", "developer");
        let names = tool_names(&developer);
        assert!(names.contains(&"read"));
        assert!(names.contains(&"write"));
        assert!(names.contains(&"edit"));
        assert!(names.contains(&"spawn_agent"));
        assert!(names.contains(&"todo_write"));
        assert!(names.contains(&"web_fetch"));
        assert!(!names.contains(&"read_file"));
        assert!(!names.contains(&"read_pdf"));

        let general = tool_specs_for_scope("build", "general");
        let names = tool_names(&general);
        assert_eq!(names.len(), 12);
        assert!(names.contains(&"run_command"));
        assert!(names.contains(&"update_plan"));
        assert!(names.contains(&"read_pdf"));
        assert!(names.contains(&"create_docx"));
        assert!(names.contains(&"look_at_screen"));
        assert!(names.contains(&"set_screen_share"));
        assert!(!names.contains(&"edit_file"));
        assert!(!names.contains(&"read_file"));

        let dfd = tool_specs_for_scope("build", "dfd");
        let names = tool_names(&dfd);
        assert_eq!(names.len(), 9);
        assert!(names.contains(&"create_coding_project"));
        assert!(names.contains(&"dfd_bridge"));
        assert!(names.contains(&"create_client"));
        assert!(names.contains(&"files_changed"));

        let house = tool_specs_for_scope("build", "house");
        let names = tool_names(&house);
        assert_eq!(names.len(), 11);
        assert!(names.contains(&"house_set_device"));
        assert!(names.contains(&"house_run_scene"));

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
        assert!(names.contains(&"create_coding_project"));
        assert!(names.contains(&"ask_user"));
        assert!(names.contains(&"delegate"));
        assert!(names.contains(&"delegation_status"));
        assert!(names.contains(&"job_message"));
        assert!(names.contains(&"job_cancel"));
        assert!(names.contains(&"give_note"));
        assert_eq!(
            tool_names(&tool_specs_for_scope("build", "agent")).len(),
            57
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
        assert_eq!(developer.len(), 20);
        assert_eq!(coder.len(), 21);
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
        assert!(!names.contains(&"edit_file"));
        assert!(!names.contains(&"run_code"));
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"search_code"));

        let ctx = ToolContext::new(
            std::path::Path::new("."),
            "chat",
            Arc::new(QuestionBroker::new()),
        )
        .with_access("read");
        assert!(read_only_denial("edit_file", &json!({"file_path": "a"}), &ctx).is_some());
        assert!(
            read_only_denial("run_command", &json!({"command": "echo x > out.txt"}), &ctx)
                .is_some()
        );
        assert!(read_only_denial("run_command", &json!({"command": "ls"}), &ctx).is_none());
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
            "edit_file",
            &json!({"file_path": "a.txt", "old_string": "hello", "new_string": "bye"}),
            &ctx,
        )
        .await;
        assert!(edit.is_error);
        assert!(edit.content.contains("[BLOCKED]"));

        let read = dispatch("read_file", &json!({"file_path": "a.txt"}), &ctx).await;
        assert!(!read.is_error, "{}", read.content);
    }

    #[tokio::test]
    async fn read_image_queues_inline_vision() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        std::fs::write(dir.path().join("shot.png"), b"\x89PNG\r\n\x1a\nfake").unwrap();
        let read = dispatch("read_file", &json!({"file_path": "shot.png"}), &ctx).await;
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
            "edit_file",
            &json!({"file_path": "src/demo.txt", "new_string": "alpha\nbeta\n"}),
            &ctx,
        )
        .await;
        assert!(!write.is_error, "{}", write.content);

        let read = dispatch("read_file", &json!({"file_path": "src/demo.txt"}), &ctx).await;
        assert!(!read.is_error, "{}", read.content);
        assert!(read.content.contains("[FILE] src/demo.txt"));
        assert!(read.content.contains("alpha"));

        let edit = dispatch(
            "edit_file",
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

        let search = dispatch(
            "search_code",
            &json!({"pattern": "gamma", "path": "src"}),
            &ctx,
        )
        .await;
        assert!(!search.is_error, "{}", search.content);
        assert!(search.content.contains("src/demo.txt:2: gamma"));
    }

    #[tokio::test]
    async fn read_rejects_path_escape() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let read = dispatch(
            "read_file",
            &json!({"file_path": "../../outside.txt"}),
            &ctx,
        )
        .await;
        assert!(read.is_error);
        assert!(read.content.contains("escapes the workspace"));
    }

    #[tokio::test]
    async fn run_command_returns_exit_and_output() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let result = dispatch("run_command", &json!({"command": "printf 'hello'"}), &ctx).await;
        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("[OK] Exit 0"));
        assert!(result.content.contains("hello"));
    }

    #[tokio::test]
    async fn run_command_supports_pty_and_resumable_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());

        let pty = dispatch(
            "run_command",
            &json!({"command": "printf 'pty-hello\\n'", "tty": true, "timeout": 15000}),
            &ctx,
        )
        .await;
        assert!(!pty.is_error, "{}", pty.content);
        assert!(pty.content.contains("pty-hello"), "{}", pty.content);

        let started = dispatch(
            "run_command",
            &json!({
                "command": "sleep 0.8; printf 'session-done\\n'",
                "yield_time_ms": 100
            }),
            &ctx,
        )
        .await;
        assert!(!started.is_error, "{}", started.content);
        assert!(started.content.contains("[RUNNING]"), "{}", started.content);
        let id = started
            .content
            .split("session #")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .expect("session id")
            .to_string();
        let polled = dispatch(
            "run_command",
            &json!({
                "command": id,
                "process_action": "send",
                "yield_time_ms": 15000
            }),
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
    async fn background_process_ready_logs_and_kill() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let command = "printf 'bg-ready\\n'; sleep 2";
        let started = dispatch(
            "run_command",
            &json!({
                "command": command,
                "background": true,
                "expect": "bg-ready",
                "ready_timeout": 20000
            }),
            &ctx,
        )
        .await;
        assert!(!started.is_error, "{}", started.content);
        assert!(started.content.contains("[READY]"));

        let logs = dispatch(
            "run_command",
            &json!({"command": command, "process_action": "logs"}),
            &ctx,
        )
        .await;
        assert!(logs.content.contains("bg-ready"));

        let killed = dispatch(
            "run_command",
            &json!({"command": command, "process_action": "kill"}),
            &ctx,
        )
        .await;
        assert!(!killed.is_error, "{}", killed.content);
    }

    #[tokio::test]
    async fn run_command_blocks_destructive_policy_hits() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let result = dispatch("run_command", &json!({"command": "rm -rf /"}), &ctx).await;
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
                    "ask_user",
                    &json!({
                        "questions": [{"id": "q1", "question": "Continue?"}],
                        "timeout": 10
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
            "write",
            "edit",
            "glob",
            "grep",
            "todo_write",
            "ask_user_question",
            "skill",
            "web_search",
            "web_fetch",
            "spawn_agent",
            "message_agent",
            "wait_agent",
            "list_agents",
            "interrupt_agent",
            "close_agent",
            "resume_agent",
            "tool_search",
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
            "update_plan",
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
