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
mod personal;
mod plan;
mod processes;
mod questions;
mod read;
pub mod retention;
mod screen;
pub mod search;
mod skills;
mod truncate;
mod verify;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{Value, json};
use stark_contract::{PlanItem, ToolSpecFile};
use tokio_util::sync::CancellationToken;

pub use approvals::ApprovalCache;
pub use checkpoint::{Checkpoint, RestoreReport, SnapshotGroup};
pub use codemode::worker::{WorkerManager, runtime_status as code_mode_runtime_status};
pub use codemode::{
    execute_description as code_mode_execute_description,
    instructions::Rendered as CodeModeInstructions,
    jobs::{
        JobNotice, acknowledge_notice as acknowledge_code_mode_notice,
        recover_notices as recover_code_mode_notices,
    },
};
pub use house::{HouseStore, public_device, public_state};
pub use images::{InlineImage, collect_inline_images, is_image_mime, mime_for_ext};
pub use inventory::{
    INVENTORY, is_codemode, is_implemented, is_plan_write, is_program, is_read_blocked,
};
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
pub use mcp::configured_servers;
pub use mcp::{McpManager, ServerConfig as McpServerConfig, init as init_mcp, search_tools};
pub use plan::{PlanError, PlanStore, update_plan};
pub use processes::{ProcessManager, ProcessStatus};
pub use questions::QuestionBroker;
pub use retention::{RetentionPolicy, RetentionReport, free_bytes, sweep_harness};
pub use screen::{
    PendingImage, ScreenBridge, drain_images, latest_continuous_image, queue_image, screen_bridge,
};
pub use skills::{
    SkillEntry, catalog_block_for, catalog_block_for_agent, catalog_entries, skill_names,
};
pub use verify::{AcceptanceVerdict, CriterionStatus, CriterionVerdict, evaluate_acceptance};

/// Tool membership and guard flags live in [`inventory`]: one `ToolMeta`
/// entry per tool derives the implemented set, the plan-write guard, and the
/// read-only guard. The frozen wire schema stays in
/// `backend-rs/specs/tool-specs.json`.
pub const MCP_EAGER_LIMIT: usize = 8;

static CONTRACT: OnceLock<ToolSpecFile> = OnceLock::new();

/// How an agent surfaces tools to the model.
///
/// Mirrors OpenCode v2's Code Mode partition at the agent level:
/// - `Off`: every tool is a direct call and `execute` is not exposed.
/// - `Hybrid`: OpenCode base Code Mode. Core tools stay direct; the long-tail
///   catalog is callable only from inside `execute`.
/// - `Only`: OpenCode's execute-only full surface. Core tools move into the
///   catalog and `glob`/`grep` are hidden; direct calls are repaired into a
///   one-call program.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CodeMode {
    #[default]
    Off,
    Hybrid,
    Only,
}

impl CodeMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "disabled" => Some(Self::Off),
            "hybrid" | "both" | "preferred" => Some(Self::Hybrid),
            "only" | "strict" => Some(Self::Only),
            _ => None,
        }
    }

    pub fn enabled(self) -> bool {
        !matches!(self, Self::Off)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Hybrid => "hybrid",
            Self::Only => "only",
        }
    }
}

/// Canonical scope for Code Mode policy. Mirrors the compatibility aliases in
/// `stark_core::agents` (`stark-tools` cannot depend on `stark-core`), so old
/// sessions and configs carrying a removed scope keep that scope's behavior
/// instead of silently falling back to `Off`.
pub fn canonical_code_mode_scope(scope: &str) -> &str {
    match scope.trim().to_ascii_lowercase().as_str() {
        "coder" | "dev" | "coding" | "engineer" => "developer",
        _ => scope.trim(),
    }
}

/// Coding workspace agents use OpenCode's base Code Mode: core tools stay
/// direct and the long-tail catalog runs through `execute`. Every other scope
/// keeps direct tool calls until its contract opts in.
pub fn default_code_mode(scope: &str) -> CodeMode {
    match canonical_code_mode_scope(scope)
        .to_ascii_lowercase()
        .as_str()
    {
        "developer" => CodeMode::Hybrid,
        _ => CodeMode::Off,
    }
}

/// Delivers a finished background Code Mode script as a follow-up message
/// (`<background-result kind="codemode" ...>`). Stark-core wires the run's
/// `on_job_done` hook here; the worker never blocks on it.
pub type BackgroundHook = Arc<dyn Fn(String) + Send + Sync>;
pub type AuditHook = Arc<dyn Fn(&str, &str, serde_json::Value) + Send + Sync>;

/// Whether the frozen contract for this scope exposes `execute`. Guards
/// against an agent config enabling Code Mode on a scope that has no program
/// tool to call.
pub fn scope_supports_code_mode(scope: &str) -> bool {
    contract()
        .scope_map
        .get(&canonical_code_mode_scope(scope).to_ascii_lowercase())
        .is_some_and(|tools| tools.iter().any(|tool| tool == "execute"))
}

/// Custom agents have no frozen contract scope. Their explicit `execute` tool
/// is the capability marker; built-ins keep the contract-based check.
pub fn supports_execute(scope: &str, has_execute: bool) -> bool {
    scope_supports_code_mode(scope) || has_execute
}

/// Whether an explicit tool-name list contains the Code Mode capability.
pub fn has_execute_tool(tools: &[String]) -> bool {
    tools
        .iter()
        .any(|tool| tool.trim().eq_ignore_ascii_case("execute"))
}

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
    /// Agent-level Code Mode partition. `Only` hides program-capable tools
    /// from the provider and repairs direct calls into `execute`.
    pub code_mode: CodeMode,
    /// OpenCode `full_codemode`: `fetch` plus `path`/`Bun` globals, script
    /// `timeout`/`background`, and the pinned file/shell catalog.
    pub full_codemode: bool,
    /// True while dispatching a tool call made from inside a `execute`
    /// program. The direct-call gate ignores program tools in this mode.
    pub in_program: bool,
    /// MCP tools activated by `tool_search` for this run. Large MCP catalogs
    /// are deferred: only these names are sent to the provider.
    pub active_mcp: Arc<Mutex<HashSet<String>>>,
    /// Optional per-agent allowlist. `None` keeps the scope contract; `Some`
    /// is enforced both when advertising tools and at dispatch time.
    pub allowed_tools: Option<Arc<HashSet<String>>>,
    /// Optional per-agent skill allowlist. `None` means every discovered
    /// skill; `Some(empty)` disables skill loading even if the tool is listed.
    pub allowed_skills: Option<Arc<HashSet<String>>>,
    /// Optional pinned SHA-256 per skill name.
    pub allowed_skill_hashes: Option<Arc<HashMap<String, String>>>,
    /// Optional per-agent MCP server allowlist (`["github"]`). Tool names are
    /// shaped `mcp__<server>__<tool>`.
    pub mcp_servers: Option<Arc<HashSet<String>>>,
    pub code_delta: Option<Arc<dyn Fn(stark_contract::Delta) + Send + Sync>>,
    /// Durable, model-invisible security audit sink. Best-effort: a sink
    /// failure must never block a tool.
    pub audit_hook: Option<AuditHook>,
    /// Shared Code Mode worker manager for this harness. Production runs set
    /// this in `stark-core`; tests and one-shot contexts leave it `None` and
    /// get a disposable worker (killed on drop).
    pub worker_manager: Option<Arc<crate::WorkerManager>>,
    /// Receives a finished background `execute` script as a follow-up message.
    /// Set from the run's `on_job_done` hook; `None` in tests drops it.
    pub background_hook: Option<BackgroundHook>,
    /// Cancels cancel-safe tools when the user stops the run. Tools that
    /// mutate files are deliberately not wrapped: Stop must never leave a
    /// half-written file. Codex kills the process group of a cancelled exec
    /// (`core/src/exec.rs`); this is the same contract for Stark tools.
    pub cancel: CancellationToken,
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
            code_mode: CodeMode::Off,
            full_codemode: false,
            in_program: false,
            active_mcp: Arc::new(Mutex::new(HashSet::new())),
            allowed_tools: None,
            allowed_skills: None,
            allowed_skill_hashes: None,
            mcp_servers: None,
            code_delta: None,
            audit_hook: None,
            worker_manager: None,
            background_hook: None,
            cancel: CancellationToken::new(),
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

    pub fn with_code_mode(mut self, code_mode: CodeMode) -> Self {
        self.code_mode = code_mode;
        self
    }

    pub fn with_full_codemode(mut self, full: bool) -> Self {
        self.full_codemode = full;
        self
    }

    /// `Only` is OpenCode's execute-only full surface, so it implies full
    /// even when the flag was not repeated on the context.
    fn codemode_full(&self) -> bool {
        self.full_codemode || matches!(self.code_mode, CodeMode::Only)
    }

    pub fn with_in_program(mut self, in_program: bool) -> Self {
        self.in_program = in_program;
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

    pub fn with_allowed_skills(mut self, allowed: Option<Vec<String>>) -> Self {
        self.allowed_skills = allowed.map(|names| {
            Arc::new(
                names
                    .into_iter()
                    .map(|name| name.to_ascii_lowercase())
                    .collect(),
            )
        });
        self
    }

    pub fn with_allowed_skill_hashes(mut self, hashes: Option<BTreeMap<String, String>>) -> Self {
        self.allowed_skill_hashes = hashes.map(|hashes| {
            Arc::new(
                hashes
                    .into_iter()
                    .map(|(name, hash)| {
                        (
                            name.trim().to_ascii_lowercase(),
                            hash.trim()
                                .trim_start_matches("sha256:")
                                .to_ascii_lowercase(),
                        )
                    })
                    .filter(|(name, hash)| !name.is_empty() && !hash.is_empty())
                    .collect(),
            )
        });
        self
    }

    pub fn with_audit_hook(mut self, hook: Option<AuditHook>) -> Self {
        self.audit_hook = hook;
        self
    }

    pub fn audit_event(&self, kind: &str, subject: &str, detail: serde_json::Value) {
        if let Some(hook) = &self.audit_hook {
            hook(kind, subject, detail);
        }
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

    /// Share the run's cancellation token. Direct dispatch and tests keep a
    /// fresh, never-cancelled token.
    pub fn with_cancel(mut self, cancel: CancellationToken) -> Self {
        self.cancel = cancel;
        self
    }

    fn tool_allowed(&self, name: &str) -> bool {
        // The dispatcher is infrastructure, not an optional tool: when Code
        // Mode is on, `tools: ["read"]` must not silently hide the only way
        // to call `read`. Turn Code Mode off instead.
        if name == "execute"
            && self.code_mode.enabled()
            && supports_execute(
                &self.scope,
                self.allowed_tools
                    .as_ref()
                    .is_some_and(|tools| tools.iter().any(|tool| tool == "execute")),
            )
        {
            return true;
        }
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

    /// Drop the inner-call timeline callback. Used by the implicit repair
    /// path so an auto-wrapped direct call shows one timeline row, not two.
    pub fn without_code_delta(mut self) -> Self {
        self.code_delta = None;
        self
    }

    pub fn with_background_hook(mut self, hook: Option<BackgroundHook>) -> Self {
        self.background_hook = hook;
        self
    }

    pub fn with_worker_manager(mut self, manager: Arc<crate::WorkerManager>) -> Self {
        self.worker_manager = Some(manager);
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

/// Why a call failed, so the harness can append the right repair hint instead
/// of one generic nudge. Mirrors Codex's split between argument rejection,
/// policy denial, exec failure, and transport failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolErrorClass {
    /// Wrong or missing arguments: retry with corrected arguments.
    Args,
    /// Unknown, removed, or unimplemented tool.
    Unknown,
    /// Policy/scope/approval denial: retrying unchanged fails identically.
    Blocked,
    /// Timed out: narrow the work or resume the session/job.
    Timeout,
    /// Transport or process failure below the tool (MCP stdio, CLI, network).
    Transport,
    /// Command ran and exited non-zero: read the output, fix the command.
    Exec,
    /// Tool ran and reported a failure: read the message before retrying.
    Tool,
    /// The user stopped the run. The turn ends; do not retry the call.
    Cancelled,
}

impl ToolErrorClass {
    /// One-line, model-actionable repair guidance. Codex keeps this in the
    /// tool result so the next sample sees it even after compaction.
    pub fn hint(self) -> &'static str {
        match self {
            Self::Args => {
                "Fix the argument names/values against the schema and retry; the same call will fail the same way."
            }
            Self::Unknown => {
                "This tool does not exist in this scope. Use tool_search for MCP tools or pick a listed built-in; do not call it again."
            }
            Self::Blocked => {
                "A policy blocked this call. Changing the arguments will not help unless the request itself changes; ask the user once, or work another way."
            }
            Self::Timeout => {
                "Timed out. Reduce the work size, raise timeoutMs, or resume the existing session/job instead of repeating the call."
            }
            Self::Transport => {
                "The layer below the tool failed, not the request. Retry once; if it fails again, check the server/process status and report it."
            }
            Self::Exec => {
                "The command exited non-zero. Read the output above, fix the command or environment, and change what you run next."
            }
            Self::Tool => {
                "The tool reported a failure. Read the message before retrying; repeating the identical call is not a retry strategy."
            }
            Self::Cancelled => {
                "The user stopped this run. Do not retry the call; wait for their next instruction."
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolOutcome {
    pub content: String,
    pub is_error: bool,
    pub exit_code: Option<i32>,
    pub plan: Option<BTreeMap<String, Vec<PlanItem>>>,
    pub children: Vec<Value>,
    /// Structured result data for clients (file diffs, tool-call rows, …).
    /// Never sent to the model; it rides the tool-finished delta.
    pub metadata: Option<Value>,
    /// Failure class for harness-side repair guidance (`None` on success).
    pub class: Option<ToolErrorClass>,
}

impl ToolOutcome {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            exit_code: None,
            plan: None,
            children: Vec::new(),
            metadata: None,
            class: None,
        }
    }

    pub fn err(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            exit_code: None,
            plan: None,
            children: Vec::new(),
            metadata: None,
            class: None,
        }
    }

    /// Typed failure: the harness can append the class's repair hint.
    pub fn err_class(class: ToolErrorClass, content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            exit_code: None,
            plan: None,
            children: Vec::new(),
            metadata: None,
            class: Some(class),
        }
    }

    pub fn with_code(content: impl Into<String>, is_error: bool, exit_code: Option<i32>) -> Self {
        Self {
            content: content.into(),
            is_error,
            exit_code,
            plan: None,
            children: Vec::new(),
            metadata: None,
            class: None,
        }
    }

    pub fn with_class(mut self, class: ToolErrorClass) -> Self {
        self.class = Some(class);
        self
    }

    pub fn with_plan(mut self, plan: BTreeMap<String, Vec<PlanItem>>) -> Self {
        self.plan = Some(plan);
        self
    }

    pub fn with_children(mut self, children: Vec<Value>) -> Self {
        self.children = children;
        self
    }

    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

/// A tool stopped by the user. The run loop notices `ctx.cancel` and ends the
/// turn through its abort path, so this result is only visible to callers that
/// dispatch a tool directly.
pub fn cancelled_outcome(tool: &str) -> ToolOutcome {
    ToolOutcome::err_class(
        ToolErrorClass::Cancelled,
        format!("[CANCELLED] {tool} stopped because the run was cancelled."),
    )
}

/// Race a cancel-safe tool against the run's cancellation token.
///
/// Only use this for tools whose future can be dropped without corrupting
/// state: shell jobs (killed explicitly), the browser CLI and MCP calls
/// (`kill_on_drop` / server survives), and status waits. File writes and patch
/// application are never wrapped.
pub(crate) async fn cancelable<F>(ctx: &ToolContext, tool: &str, future: F) -> ToolOutcome
where
    F: std::future::Future<Output = ToolOutcome>,
{
    if ctx.cancel.is_cancelled() {
        return cancelled_outcome(tool);
    }
    tokio::select! {
        biased;
        _ = ctx.cancel.cancelled() => cancelled_outcome(tool),
        outcome = future => outcome,
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
    tool_specs_for_run_with_code_mode(
        mode,
        scope,
        access,
        &HashSet::new(),
        None,
        None,
        CodeMode::Hybrid,
    )
}

/// Like [`tool_specs_for_scope_with_access`] but honors MCP deferral: when a
/// scope has more than [`MCP_EAGER_LIMIT`] MCP tools, only the ones activated
/// by `tool_search` are sent.
/// The unfiltered surface for a scope, before Code Mode hiding.
fn visible_specs_for_run(
    mode: &str,
    scope: &str,
    access: &str,
    active_mcp: &HashSet<String>,
    allowed_tools: Option<&[String]>,
    mcp_servers: Option<&[String]>,
) -> Vec<Value> {
    // Compatibility aliases ("coder", "dev", ...) resolve to the canonical
    // built-in before the contract lookup.
    let scope = canonical_code_mode_scope(scope.trim()).to_ascii_lowercase();
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
    // The v2 developer surface keeps the tool catalog stable across
    // plan/build for prefix-cache reuse; writes are blocked at dispatch.
    // Older scopes keep the legacy filter.
    let plan_filter = mode == "plan" && scope.as_str() != "developer";
    let mut specs: Vec<Value> = contract()
        .tools
        .iter()
        .filter(|tool| {
            // A custom scope only has tools through its explicit ACL. Never
            // fall through to the whole inventory when that ACL is missing.
            (allowed.is_some() || agent_allowed.is_some() || scope == "agent")
                && tool.exposed
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
                                | "apply_patch"
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

/// Program eligibility for built-ins and MCP tools. MCP servers opt out with
/// `"codemode": false`.
pub(crate) fn is_program_tool(name: &str) -> bool {
    if name.starts_with("mcp__") {
        mcp::is_codemode_tool(name)
    } else {
        is_program(name)
    }
}

/// OpenCode tools that keep `codemode: false` in the base surface: core
/// shell/file/search work stays direct until `full_codemode` pins it.
fn is_codemode_catalog_tool(name: &str) -> bool {
    is_program_tool(name) && inventory::is_codemode(name)
}

/// Whether a tool is reachable only through `execute` for the active surface.
fn codemode_tool_is_catalog(name: &str, full: bool) -> bool {
    if full {
        is_program_tool(name)
    } else {
        is_codemode_catalog_tool(name)
    }
}

fn mcp_server_allowed(name: &str, mcp_servers: Option<&[String]>) -> bool {
    let Some(servers) = mcp_servers else {
        return true;
    };
    let server = name
        .trim_start_matches("mcp__")
        .split("__")
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    servers
        .iter()
        .any(|allowed| allowed.trim().eq_ignore_ascii_case(&server))
}

fn spec_name(spec: &Value) -> &str {
    spec.get("function")
        .and_then(|function| function.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// Direct provider surface. `Off` removes `execute`; `Only` hides every
/// program-capable tool so the model must use a program; `Hybrid` keeps core
/// tools direct and catalogs only the long tail.
pub fn tool_specs_for_run(
    mode: &str,
    scope: &str,
    access: &str,
    active_mcp: &HashSet<String>,
    allowed_tools: Option<&[String]>,
    mcp_servers: Option<&[String]>,
) -> Vec<Value> {
    // Compatibility surface: the contract's scope map without applying an
    // agent-level Code Mode policy. Production callers use
    // `tool_specs_for_run_with_code_mode`.
    tool_specs_for_run_with_code_mode(
        mode,
        scope,
        access,
        active_mcp,
        allowed_tools,
        mcp_servers,
        CodeMode::Hybrid,
    )
}

pub fn tool_specs_for_run_with_code_mode(
    mode: &str,
    scope: &str,
    access: &str,
    active_mcp: &HashSet<String>,
    allowed_tools: Option<&[String]>,
    mcp_servers: Option<&[String]>,
    code_mode: CodeMode,
) -> Vec<Value> {
    tool_specs_for_run_with_code_mode_full(
        mode,
        scope,
        access,
        active_mcp,
        allowed_tools,
        mcp_servers,
        code_mode,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn tool_specs_for_run_with_code_mode_full(
    mode: &str,
    scope: &str,
    access: &str,
    active_mcp: &HashSet<String>,
    allowed_tools: Option<&[String]>,
    mcp_servers: Option<&[String]>,
    code_mode: CodeMode,
    full: bool,
) -> Vec<Value> {
    // OpenCode enables Code Mode only where `execute` is reachable. Without
    // it, every tool (including catalog tools) stays direct.
    let code_mode_enabled = code_mode.enabled();
    let explicit_execute = allowed_tools.is_some_and(has_execute_tool);
    let partition = code_mode_enabled && supports_execute(scope, explicit_execute);
    // `Only` is the execute-only full surface; `Hybrid` is the base surface
    // unless `full_codemode` promotes it.
    let full = partition && (full || matches!(code_mode, CodeMode::Only));
    let mut specs =
        visible_specs_for_run(mode, scope, access, active_mcp, allowed_tools, mcp_servers);
    if full {
        // OpenCode `fullCodeModeHidden`: programs search and fetch through
        // shell and the `fetch` global, so these only add noise.
        specs.retain(|spec| !matches!(spec_name(spec), "glob" | "grep" | "webfetch"));
    }
    if partition {
        match code_mode {
            CodeMode::Off => {}
            CodeMode::Only => specs.retain(|spec| !is_program_tool(spec_name(spec))),
            CodeMode::Hybrid => {
                if full {
                    specs.retain(|spec| !is_program_tool(spec_name(spec)));
                } else {
                    // Base Code Mode: core tools stay direct; only the
                    // long-tail catalog moves behind `execute`.
                    specs.retain(|spec| !is_codemode_catalog_tool(spec_name(spec)));
                }
            }
        }
        specs.retain(|spec| spec_name(spec) != "execute");
        specs.push(codemode::execute_spec(full));
    } else {
        // No `execute` contract for this scope: every tool stays direct.
        specs.retain(|spec| spec_name(spec) != "execute");
    }
    // OpenCode appends the Code Mode tool last so the direct tool prefix stays
    // stable and the dispatcher is visually separate.
    if let Some(index) = specs.iter().position(|spec| spec_name(spec) == "execute") {
        let execute = specs.remove(index);
        specs.push(execute);
    }
    specs
}

/// Tools callable from inside a `execute` program. Uses the same scope,
/// access, plan, and agent-allowlist filters as the direct surface so the
/// advertised catalog and the dispatch gate can never disagree.
pub fn program_specs_for_run(
    mode: &str,
    scope: &str,
    access: &str,
    allowed_tools: Option<&[String]>,
    mcp_servers: Option<&[String]>,
) -> Vec<Value> {
    program_specs_for_run_full(mode, scope, access, allowed_tools, mcp_servers, false)
}

pub fn program_specs_for_run_full(
    mode: &str,
    scope: &str,
    access: &str,
    allowed_tools: Option<&[String]>,
    mcp_servers: Option<&[String]>,
    full: bool,
) -> Vec<Value> {
    let mut specs: Vec<Value> = visible_specs_for_run(
        mode,
        scope,
        access,
        &HashSet::new(),
        allowed_tools,
        mcp_servers,
    )
    .into_iter()
    // OpenCode base Code Mode catalogs only the tools that opt in; core
    // tools stay direct unless `full_codemode` pins them.
    .filter(|spec| codemode_tool_is_catalog(spec_name(spec), full))
    .filter(|spec| !full || !matches!(spec_name(spec), "glob" | "grep" | "webfetch"))
    .map(|spec| if full { code_mode_spec(spec) } else { spec })
    .collect();
    // Deferred MCP tools are hidden from the direct provider list, but Code
    // Mode keeps the whole Code Mode server catalog searchable. Read-only
    // agents cannot call MCP at all, so the catalog avoids advertising it.
    let mut mcp_specs = if access == "read" {
        Vec::new()
    } else {
        mcp::codemode_tool_specs_for_scope(scope)
    };
    mcp_specs.retain(|spec| mcp_server_allowed(spec_name(spec), mcp_servers));
    for spec in mcp_specs {
        if !specs
            .iter()
            .any(|existing| spec_name(existing) == spec_name(&spec))
        {
            specs.push(spec);
        }
    }
    specs
}

/// OpenCode `full_codemode` Code Mode overrides. The shell loses its
/// background parameter and gets the 10-minute Code Mode foreground default.
fn code_mode_spec(mut spec: Value) -> Value {
    if spec_name(&spec) == "bash"
        && let Some(parameters) = spec
            .get_mut("function")
            .and_then(|function| function.get_mut("parameters"))
        && let Some(properties) = parameters
            .get_mut("properties")
            .and_then(Value::as_object_mut)
    {
        properties.remove("run_in_background");
        if let Some(timeout) = properties.get_mut("timeoutMs") {
            timeout["description"] = json!(
                "Optional timeout in milliseconds (default 600000). When it expires, a foreground command moves to the background and a background command is stopped."
            );
        }
        if let Some(parameters) = parameters.as_object_mut()
            && let Some(required) = parameters.get_mut("required").and_then(Value::as_array_mut)
        {
            required.retain(|value| value.as_str() != Some("run_in_background"));
        }
    }
    spec
}

/// Program catalog for a live tool context, honoring the agent allowlist and
/// MCP server allowlist.
pub fn program_specs_for_context(ctx: &ToolContext) -> Vec<Value> {
    let allowed: Option<Vec<String>> = ctx
        .allowed_tools
        .as_ref()
        .map(|names| names.iter().cloned().collect());
    let servers: Option<Vec<String>> = ctx
        .mcp_servers
        .as_ref()
        .map(|names| names.iter().cloned().collect());
    program_specs_for_run_full(
        &ctx.mode,
        &ctx.scope,
        &ctx.access,
        allowed.as_deref(),
        servers.as_deref(),
        ctx.codemode_full(),
    )
}

/// Per-chat Code Mode instruction observation: the frozen epoch baseline for
/// the system prompt plus, when the catalog changed, the chronological update
/// OpenCode stores as a system message. Persisted under
/// `.deep_agents/codemode/instructions`.
#[allow(clippy::too_many_arguments)]
pub fn render_code_mode_instructions_for_chat(
    harness_root: &std::path::Path,
    chat_id: &str,
    mode: &str,
    scope: &str,
    access: &str,
    allowed_tools: Option<&[String]>,
    mcp_servers: Option<&[String]>,
    full: bool,
) -> CodeModeInstructions {
    let specs = program_specs_for_run_full(mode, scope, access, allowed_tools, mcp_servers, full);
    let inventory = codemode::catalog_for_specs(&specs, full);
    codemode::instructions::render_for_chat(harness_root, chat_id, Some(&inventory))
}

/// The current Code Mode epoch baseline for rebuilding a system prompt.
pub fn code_mode_instructions_baseline_for_chat(
    harness_root: &std::path::Path,
    chat_id: &str,
) -> Option<String> {
    codemode::instructions::baseline_for_chat(harness_root, chat_id)
}

/// OpenCode `InstructionState.advanceEpoch` after a completed compaction.
pub fn rebase_code_mode_instructions_for_chat(harness_root: &std::path::Path, chat_id: &str) {
    codemode::instructions::rebase_for_chat(harness_root, chat_id);
}

/// OpenCode's `removed` observation when Code Mode is unavailable for this
/// chat's scope or agent.
pub fn remove_code_mode_instructions_for_chat(
    harness_root: &std::path::Path,
    chat_id: &str,
) -> CodeModeInstructions {
    codemode::instructions::render_for_chat(harness_root, chat_id, None)
}

/// Catalog text injected into the system prompt. Mirrors OpenCode v2's
/// `# Code Mode` section: direct tools are named only by exclusion, and the
/// complete list carries exact signatures. Signatures are rendered by the
/// OpenCode `tool-schema` port (`codemode::schema`), so prompt bytes match
/// the worker runtime's catalog.
pub fn render_code_mode_catalog(
    mode: &str,
    scope: &str,
    access: &str,
    allowed_tools: Option<&[String]>,
    mcp_servers: Option<&[String]>,
) -> String {
    render_code_mode_catalog_full(mode, scope, access, allowed_tools, mcp_servers, false)
}

pub fn render_code_mode_catalog_full(
    mode: &str,
    scope: &str,
    access: &str,
    allowed_tools: Option<&[String]>,
    mcp_servers: Option<&[String]>,
    full: bool,
) -> String {
    let specs = program_specs_for_run_full(mode, scope, access, allowed_tools, mcp_servers, full);
    codemode::render_catalog_for_specs(&specs, full)
}

/// `tools.name(...)` / `tools.ns.name(...)` / `tools["odd-name"](...)`.
/// Dotted program paths render per OpenCode `toolExpression` (with the
/// `tools` prefix included).
pub fn code_mode_tool_expression(name: &str) -> String {
    codemode::schema::tool_expression(&codemode::to_program_name(name))
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
                "navigate" | "extract" | "screenshot" | "read" | "a11y" | "close" | "" => None,
                "network" => {
                    let network_action = args
                        .get("network_action")
                        .and_then(Value::as_str)
                        .unwrap_or("requests");
                    match network_action {
                        "requests" | "request" => None,
                        _ => Some(format!(
                            "[BLOCKED] Read-only agent cannot use browser_act network_action '{network_action}'. Allowed: requests, request."
                        )),
                    }
                }
                _ => Some(format!(
                    "[BLOCKED] Read-only agent cannot use browser_act '{action}'. Allowed: navigate, extract, screenshot, read, a11y, network (requests/request), close."
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
        "personal_os" => {
            let action = args
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            let read_action = matches!(
                action.as_str(),
                "overview"
                    | "money_list"
                    | "money_summary"
                    | "todo_list"
                    | "food_summary"
                    | "training_summary"
            );
            if !read_action {
                return Some(format!(
                    "[BLOCKED] Read-only agent cannot write personal data ('{action}'). Allowed reads: overview, money_list, money_summary, todo_list, food_summary, training_summary."
                ));
            }
            if ctx.scope == "house" && action.starts_with("money") {
                return Some(
                    "[BLOCKED] House may read household context (overview, todo_list, food_summary, training_summary) but not money.".to_string(),
                );
            }
            None
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

/// OpenCode v2 behavior: a direct call to a Code Mode tool still runs, but
/// inside an implicit one-call program. The result carries a repair note so
/// the next turn uses `execute`. Inner timeline deltas are suppressed so the
/// transcript shows one tool row, not two.
async fn repair_direct_program_call(name: &str, args: &Value, ctx: &ToolContext) -> ToolOutcome {
    let args_json = serde_json::to_string(args).unwrap_or_else(|_| "{}".to_string());
    let expression = code_mode_tool_expression(name);
    let code = format!("return await {expression}({args_json});");
    let inner = ctx.clone().with_in_program(true).without_code_delta();
    let outcome = codemode::handle(&inner, &json!({"code": code})).await;
    let mut content = format!(
        "<tool-repair>{name} is a Code Mode tool, so this ran inside execute as {expression}(input). Call it there next time.</tool-repair>\n\n"
    );
    content.push_str(&outcome.content);
    ToolOutcome { content, ..outcome }
}

pub async fn dispatch(name: &str, args: &Value, ctx: &ToolContext) -> ToolOutcome {
    if !ctx.tool_allowed(name) {
        ctx.audit_event(
            "acl_denied",
            name,
            json!({"reason": "agent_tool_list", "scope": ctx.scope}),
        );
        return ToolOutcome::err_class(
            ToolErrorClass::Blocked,
            format!(
                "[BLOCKED] Tool '{name}' is not enabled for this agent. Change the agent's tools config or continue without it."
            ),
        );
    }
    // Removed tools answer with their replacement first: a legacy name is not
    // a scope problem, and telling the model to "use a listed tool" hides the
    // exact rename it needs.
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
        return ToolOutcome::err_class(
            ToolErrorClass::Unknown,
            format!("[BLOCKED] Tool '{legacy}' was removed. Use '{replacement}' instead."),
        );
    }
    // Enforce the frozen scope contract at dispatch, not only when
    // advertising. Without this a hallucinated legacy name (read_file,
    // run_command, ...) skipped the v2 permission gates.
    if !name.starts_with("mcp__") {
        let contract_scope = canonical_code_mode_scope(&ctx.scope).to_ascii_lowercase();
        if let Some(allowed) = contract().scope_map.get(&contract_scope) {
            if !allowed.iter().any(|tool| tool == name) {
                // A name that is not in the frozen contract at all is a
                // hallucination, not a policy denial: say so, and list what
                // exists.
                if contract().tool(name).is_none() {
                    ctx.audit_event(
                        "acl_denied",
                        name,
                        json!({"reason": "unknown_tool", "scope": ctx.scope}),
                    );
                    return ToolOutcome::err_class(
                        ToolErrorClass::Unknown,
                        format!(
                            "Error: tool '{name}' does not exist in this harness. Use tool_search to find MCP tools, or one of: {}",
                            allowed.join(", ")
                        ),
                    );
                }
                ctx.audit_event(
                    "acl_denied",
                    name,
                    json!({"reason": "scope_contract", "scope": ctx.scope}),
                );
                return ToolOutcome::err_class(
                    ToolErrorClass::Blocked,
                    format!(
                        "[BLOCKED] Tool '{name}' is not in the '{}' scope contract. Use one of: {}",
                        ctx.scope,
                        allowed.join(", ")
                    ),
                );
            }
        } else if ctx.scope != "agent" && ctx.allowed_tools.is_none() {
            // Custom agents have no frozen scope entry. Without an explicit
            // closed ACL they must fail closed, not fall through to every
            // built-in tool.
            return ToolOutcome::err_class(
                ToolErrorClass::Blocked,
                format!(
                    "[BLOCKED] Agent scope '{}' has no tool contract. Refusing to run '{name}'.",
                    ctx.scope
                ),
            );
        }
    }
    if ctx.depth > 0 && matches!(name, "ask_user_question" | "ask_user") {
        return ToolOutcome::err_class(
            ToolErrorClass::Blocked,
            "[BLOCKED] Subagents run non-interactively: the user cannot answer here. Do not ask questions. Decide with the information you have, or report the decision/blocker to your parent in your final message.",
        );
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
        return ToolOutcome::err_class(
            ToolErrorClass::Args,
            format!(
                "Error: {name} requires '{required}' ({kind}). Got: {received}. {hint} Retry with corrected arguments."
            ),
        );
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
                return ToolOutcome::err_class(
                    ToolErrorClass::Args,
                    format!(
                        "Error: {name}.{required} must be {kind}, got {}. Retry with corrected arguments.",
                        value
                    ),
                );
            }
        }
    }
    if let Some(reason) = plan_mode_denial(&ctx.mode, name, args) {
        ctx.audit_event(
            "acl_denied",
            name,
            json!({"reason": "plan_mode", "scope": ctx.scope}),
        );
        return ToolOutcome::err_class(ToolErrorClass::Blocked, reason);
    }
    if ctx.access == "read"
        && let Some(reason) = read_only_denial(name, args, ctx)
    {
        ctx.audit_event(
            "acl_denied",
            name,
            json!({"reason": "read_only", "scope": ctx.scope}),
        );
        return ToolOutcome::err_class(ToolErrorClass::Blocked, reason);
    }
    if name == "execute" {
        if ctx.in_program {
            return ToolOutcome::err_class(
                ToolErrorClass::Blocked,
                "execute cannot be called from inside execute. Return a value instead.",
            );
        }
        if !ctx.code_mode.enabled() {
            ctx.audit_event(
                "acl_denied",
                name,
                json!({"reason": "code_mode_disabled", "scope": ctx.scope}),
            );
            return ToolOutcome::err_class(
                ToolErrorClass::Blocked,
                "[BLOCKED] Code Mode is disabled for this agent. Call tools directly.",
            );
        }
        return codemode::handle(ctx, args).await;
    }
    // Code Mode partition. Nested calls may only reach program-capable
    // built-ins; direct calls to those tools in `Only` mode are repaired into
    // a one-call program so the model gets the result and a nudge instead of
    // a dead end.
    if ctx.in_program && !codemode_tool_is_catalog(name, ctx.codemode_full()) {
        return ToolOutcome::err_class(
            ToolErrorClass::Blocked,
            format!("{name} cannot be called from inside execute. Call it directly instead."),
        );
    }
    if !ctx.in_program
        && ctx.code_mode.enabled()
        && scope_supports_code_mode(&ctx.scope)
        && codemode_tool_is_catalog(name, ctx.codemode_full())
    {
        return repair_direct_program_call(name, args, ctx).await;
    }
    if name.starts_with("mcp__") {
        if ctx.access == "read" {
            ctx.audit_event(
                "acl_denied",
                name,
                json!({"reason": "read_only_mcp", "scope": ctx.scope}),
            );
            return ToolOutcome::err_class(
                ToolErrorClass::Blocked,
                "[BLOCKED] Read-only agent cannot call MCP tools. Report findings instead.",
            );
        }
        return cancelable(ctx, name, mcp::call(name, args, ctx)).await;
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
            _ => ToolOutcome::err_class(
                ToolErrorClass::Args,
                "Error: apply_patch requires a non-empty 'patch' string",
            ),
        },
        "edit" => ds_fs::edit(ctx, args).await,
        "glob" => ds_fs::glob(ctx, args).await,
        "grep" => ds_fs::grep(ctx, args).await,
        "ask_user_question" => ds_session::ask_user_question(ctx, args).await,
        "skill" => skills::skill(ctx, args).await,
        "tool_search" => mcp::search_tools(ctx, args).await,
        "update_plan" => update_plan(ctx, args).await,
        "browser_act" => cancelable(ctx, "browser_act", browser::browser_act(ctx, args)).await,
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
        "personal_os" => personal::personal_os(ctx, args).await,
        other => ToolOutcome::err_class(
            ToolErrorClass::Unknown,
            format!("Error: tool '{other}' is not implemented by this backend"),
        ),
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

    #[tokio::test]
    async fn dispatch_failures_carry_a_typed_class() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path()).with_scope("developer");

        // Unknown tool name.
        let unknown = dispatch("no_such_tool", &json!({}), &ctx).await;
        assert_eq!(
            unknown.class,
            Some(ToolErrorClass::Unknown),
            "{}",
            unknown.content
        );

        // Removed legacy name.
        let legacy = dispatch("run_command", &json!({"command": "ls"}), &ctx).await;
        assert_eq!(
            legacy.class,
            Some(ToolErrorClass::Unknown),
            "{}",
            legacy.content
        );

        // Missing required argument.
        let missing = dispatch("read", &json!({}), &ctx).await;
        assert_eq!(
            missing.class,
            Some(ToolErrorClass::Args),
            "{}",
            missing.content
        );

        // Scope denial.
        let house = context(dir.path()).with_scope("house");
        let blocked = dispatch("apply_patch", &json!({"input": "*** Begin Patch"}), &house).await;
        assert!(
            blocked.class.is_some(),
            "denials must be typed: {}",
            blocked.content
        );
    }

    #[tokio::test]
    async fn cancelled_token_short_circuits_cancel_safe_tools() {
        use tokio_util::sync::CancellationToken;

        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let ctx = context(dir.path()).with_cancel(cancel);
        // Cancel-safe tools return without spawning when already cancelled.
        let cancelled = dispatch("browser_act", &json!({"action": "extract"}), &ctx).await;
        assert!(cancelled.is_error, "{}", cancelled.content);
        assert_eq!(cancelled.class, Some(ToolErrorClass::Cancelled));
        // File reads are deliberately not wrapped: Stop must never leave a
        // half-written file, and reads complete normally.
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        let read = dispatch("read", &json!({"file_path": "a.txt"}), &ctx).await;
        assert!(!read.is_error, "{}", read.content);
        assert!(read.content.contains("hello"), "{}", read.content);
    }

    #[test]
    fn every_error_class_has_actionable_guidance() {
        for class in [
            ToolErrorClass::Args,
            ToolErrorClass::Unknown,
            ToolErrorClass::Blocked,
            ToolErrorClass::Timeout,
            ToolErrorClass::Transport,
            ToolErrorClass::Exec,
            ToolErrorClass::Tool,
            ToolErrorClass::Cancelled,
        ] {
            let hint = class.hint();
            assert!(hint.len() > 40, "{class:?} hint too thin: {hint}");
            assert!(hint.ends_with('.'), "{class:?} hint not a sentence: {hint}");
        }
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
        assert_eq!(names, vec!["read", "execute"]);
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
        let build = tool_specs_for_scope("build", "developer");
        let plan = tool_specs_for_scope("plan", "developer");
        assert!(tool_names(&build).contains(&"edit"));
        assert!(tool_names(&build).contains(&"spawn_agent"));
        // V2 coding catalogs stay stable across plan/build; dispatch carries
        // the plan guard instead of hiding the tools.
        assert!(tool_names(&plan).contains(&"edit"));
        assert!(tool_names(&plan).contains(&"spawn_agent"));
        assert!(tool_names(&plan).contains(&"execute"));
        assert!(!tool_names(&plan).contains(&"agent"));
        assert!(tool_names(&plan).contains(&"update_plan"));
        assert!(tool_names(&plan).contains(&"bash"));

        // Legacy scopes keep the old plan filter.
        let legacy_plan = tool_specs_for_scope("plan", "agent");
        assert!(!tool_names(&legacy_plan).contains(&"edit"));
        assert!(!tool_names(&legacy_plan).contains(&"spawn_agent"));

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
        // `execute` is a dispatcher; its inner calls carry the plan guard.
        assert!(plan_mode_denial("plan", "execute", &json!({"code": "return 1"})).is_none());
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
        assert!(!names.contains(&"web_fetch"));
        assert!(!names.contains(&"web_search"));
        assert!(!names.contains(&"read_file"));
        assert!(!names.contains(&"read_pdf"));

        let general = tool_specs_for_scope("build", "general");
        let names = tool_names(&general);
        assert_eq!(names.len(), 18);
        assert!(names.contains(&"bash"));
        assert!(names.contains(&"read"));
        assert!(names.contains(&"glob"));
        assert!(names.contains(&"grep"));
        assert!(names.contains(&"update_plan"));
        assert!(names.contains(&"read_pdf"));
        assert!(names.contains(&"create_docx"));
        assert!(names.contains(&"look_at_screen"));
        assert!(names.contains(&"set_screen_share"));
        assert!(names.contains(&"personal_os"));
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
        assert_eq!(names.len(), 12);
        assert!(names.contains(&"house_set_device"));
        assert!(names.contains(&"house_run_scene"));
        assert!(names.contains(&"personal_os"));
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
            46
        );
    }

    #[test]
    fn developer_scope_keeps_code_mode_in_plan_mode() {
        let developer_specs = tool_specs_for_scope("build", "developer");
        let developer: std::collections::BTreeSet<&str> =
            tool_names(&developer_specs).into_iter().collect();
        assert_eq!(developer.len(), 16);
        assert!(developer.contains("execute"));
        // Plan mode keeps the catalog stable for the coding surface.
        let plan_specs = tool_specs_for_scope("plan", "developer");
        let plan = tool_names(&plan_specs);
        assert!(plan.contains(&"execute"));
    }

    #[tokio::test]
    async fn personal_os_dispatch_follows_scope_and_access() {
        let dir = tempfile::tempdir().unwrap();

        let general = context(dir.path()).with_scope("general");
        let added = dispatch(
            "personal_os",
            &json!({"action": "todo_add", "text": "Water plants"}),
            &general,
        )
        .await;
        assert!(!added.is_error, "{}", added.content);
        let overview = dispatch("personal_os", &json!({"action": "overview"}), &general).await;
        assert!(!overview.is_error, "{}", overview.content);
        assert!(overview.content.contains("1 open"), "{}", overview.content);

        // House is read-only and never sees money.
        let house = context(dir.path()).with_scope("house").with_access("read");
        let read = dispatch("personal_os", &json!({"action": "todo_list"}), &house).await;
        assert!(!read.is_error, "{}", read.content);
        assert!(read.content.contains("Water plants"), "{}", read.content);
        let write = dispatch(
            "personal_os",
            &json!({"action": "todo_add", "text": "x"}),
            &house,
        )
        .await;
        assert!(write.is_error, "{}", write.content);
        assert!(write.content.contains("[BLOCKED]"), "{}", write.content);
        let money = dispatch("personal_os", &json!({"action": "money_summary"}), &house).await;
        assert!(money.is_error, "{}", money.content);
        assert!(money.content.contains("not money"), "{}", money.content);

        // A scope without personal_os is rejected by the contract.
        let explore = context(dir.path()).with_scope("explore");
        let denied = dispatch("personal_os", &json!({"action": "overview"}), &explore).await;
        assert!(denied.is_error, "{}", denied.content);
        assert!(
            denied.content.contains("scope contract"),
            "{}",
            denied.content
        );
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
        assert!(!names.contains(&"execute"));
        assert!(names.contains(&"read"));
        assert!(names.contains(&"grep"));
        assert!(names.contains(&"bash"));

        // A read-only coding agent keeps the dispatcher: inner reads work,
        // inner writes hit the read-only guard.
        let read_dev = tool_specs_for_scope_with_access("build", "developer", "read");
        let dev_names = tool_names(&read_dev);
        assert!(!dev_names.contains(&"edit"));
        assert!(!dev_names.contains(&"apply_patch"));
        assert!(dev_names.contains(&"execute"));
        assert!(dev_names.contains(&"read"));

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
        assert!(read_only_denial("browser_act", &json!({"action": "read"}), &ctx).is_none());
        assert!(
            read_only_denial(
                "browser_act",
                &json!({"action": "network", "network_action": "requests"}),
                &ctx
            )
            .is_none()
        );
        assert!(
            read_only_denial(
                "browser_act",
                &json!({"action": "network", "network_action": "route", "url": "*"}),
                &ctx
            )
            .is_some()
        );
        assert!(
            read_only_denial(
                "browser_act",
                &json!({"action": "record", "record_action": "start", "path": "a.webm"}),
                &ctx
            )
            .is_some()
        );
        assert!(
            read_only_denial("memory", &json!({"action": "remember", "text": "x"}), &ctx).is_some()
        );
        assert!(
            read_only_denial("memory", &json!({"action": "read", "topic": "facts"}), &ctx)
                .is_none()
        );
        assert!(read_only_denial("personal_os", &json!({"action": "overview"}), &ctx).is_none());
        assert!(read_only_denial("personal_os", &json!({"action": "todo_list"}), &ctx).is_none());
        assert!(
            read_only_denial("personal_os", &json!({"action": "money_summary"}), &ctx).is_none()
        );
        assert!(
            read_only_denial(
                "personal_os",
                &json!({"action": "money_add", "amount": 5}),
                &ctx
            )
            .is_some()
        );
        assert!(
            read_only_denial(
                "personal_os",
                &json!({"action": "todo_add", "text": "x"}),
                &ctx
            )
            .is_some()
        );

        // House reads household context but never money.
        let house = ToolContext::new(
            std::path::Path::new("."),
            "house-chat",
            Arc::new(QuestionBroker::new()),
        )
        .with_access("read")
        .with_scope("house");
        for action in ["overview", "todo_list", "food_summary", "training_summary"] {
            assert!(
                read_only_denial("personal_os", &json!({"action": action}), &house).is_none(),
                "house should read {action}"
            );
        }
        for action in ["money_summary", "money_list"] {
            assert!(
                read_only_denial("personal_os", &json!({"action": action}), &house).is_some(),
                "house must not read {action}"
            );
        }
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
        // A removed name answers with its replacement in every scope, before
        // the scope check can turn it into a generic "pick a listed tool".
        assert!(
            out_of_scope.content.contains("was removed"),
            "{}",
            out_of_scope.content
        );
        assert_eq!(out_of_scope.class, Some(ToolErrorClass::Unknown));
        // A real contract tool outside the scope is a policy denial.
        let denied = dispatch("personal_os", &json!({"action": "overview"}), &scoped).await;
        assert!(denied.is_error, "{}", denied.content);
        assert!(
            denied.content.contains("scope contract"),
            "{}",
            denied.content
        );
        assert_eq!(denied.class, Some(ToolErrorClass::Blocked));
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
            &json!({"action": "output", "job_id": job_id, "wait": true, "timeout_ms": 15000}),
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
            &json!({"action": "output", "job_id": job_id, "wait": true, "timeout_ms": 15000}),
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
            "spawn_agent",
            "message_agent",
            "wait_agent",
            "interrupt_agent",
            "close_agent",
            "update_plan",
            "execute",
        ];
        // execute is appended last by the surface builder.
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

#[cfg(test)]
mod code_mode_policy_tests {
    use super::*;
    use std::collections::HashSet;

    fn names(specs: &[Value]) -> Vec<&str> {
        specs
            .iter()
            .filter_map(|spec| spec["function"]["name"].as_str())
            .collect()
    }

    fn coding(workspace: &std::path::Path) -> ToolContext {
        ToolContext::new(workspace, "policy-test", Arc::new(QuestionBroker::new()))
            .with_scope("developer")
            .with_mode("build")
            .with_code_mode(CodeMode::Only)
            .with_full_codemode(true)
    }

    #[test]
    fn only_mode_hides_program_tools_and_keeps_direct_tools() {
        let specs = tool_specs_for_run_with_code_mode(
            "build",
            "developer",
            "write",
            &HashSet::new(),
            None,
            None,
            CodeMode::Only,
        );
        let names = names(&specs);
        for direct in [
            "execute",
            "job",
            "update_plan",
            "skill",
            "ask_user_question",
            "spawn_agent",
        ] {
            assert!(names.contains(&direct), "missing direct tool {direct}");
        }
        for nested in ["read", "edit", "apply_patch", "bash", "glob", "grep"] {
            assert!(!names.contains(&nested), "leaked nested tool {nested}");
        }
    }

    #[test]
    fn hybrid_and_off_modes_keep_the_expected_surface() {
        let hybrid = tool_specs_for_run_with_code_mode(
            "build",
            "developer",
            "write",
            &HashSet::new(),
            None,
            None,
            CodeMode::Hybrid,
        );
        let hybrid = names(&hybrid);
        assert!(hybrid.contains(&"read"));
        assert!(hybrid.contains(&"execute"));

        let off = tool_specs_for_run_with_code_mode(
            "build",
            "developer",
            "write",
            &HashSet::new(),
            None,
            None,
            CodeMode::Off,
        );
        let off = names(&off);
        assert!(off.contains(&"read"));
        assert!(!off.contains(&"execute"));
    }

    #[test]
    fn removed_coder_scope_keeps_developer_codemode() {
        // Compatibility alias (mirrors `stark_core::agents`): old sessions
        // and configs on `"coder"` must not silently fall back to `Off`.
        assert_eq!(default_code_mode("coder"), CodeMode::Hybrid);
        assert_eq!(default_code_mode("Coder"), CodeMode::Hybrid);
        assert_eq!(default_code_mode("dev"), CodeMode::Hybrid);
        assert!(scope_supports_code_mode("coder"));
        let direct = tool_specs_for_run_with_code_mode(
            "build",
            "coder",
            "write",
            &HashSet::new(),
            None,
            None,
            default_code_mode("coder"),
        );
        let direct_names = names(&direct);
        assert!(direct_names.contains(&"execute"), "{direct_names:?}");
        assert!(direct_names.contains(&"read"), "{direct_names:?}");
    }

    #[test]
    fn per_tool_codemode_flag_matches_opencode_opt_outs() {
        // OpenCode registers core tools with `codemode: false`; everything
        // else program-capable opts in.
        for core in ["bash", "read", "edit", "apply_patch", "glob", "grep"] {
            assert!(is_program(core), "{core} must stay program-capable");
            assert!(!is_codemode(core), "{core} must opt out of the catalog");
        }
        assert!(is_codemode("files_changed"));
        assert!(!is_codemode("execute"));
        assert!(!is_codemode("spawn_agent"));
    }

    #[test]
    fn base_codemode_keeps_core_tools_direct_and_catalogs_only_long_tail() {
        assert_eq!(default_code_mode("developer"), CodeMode::Hybrid);
        let direct = tool_specs_for_run_with_code_mode(
            "build",
            "developer",
            "write",
            &HashSet::new(),
            None,
            None,
            CodeMode::Hybrid,
        );
        let direct_names = names(&direct);
        for core in ["bash", "read", "edit", "apply_patch", "glob", "grep"] {
            assert!(direct_names.contains(&core), "base lost direct {core}");
        }
        assert!(direct_names.contains(&"execute"));
        let execute = direct
            .iter()
            .find(|spec| spec["function"]["name"] == "execute")
            .expect("execute");
        let properties = execute
            .pointer("/function/parameters/properties")
            .expect("execute properties");
        assert!(properties.get("timeout").is_none());
        assert!(properties.get("background").is_none());

        let catalog = program_specs_for_run("build", "developer", "write", None, None);
        let catalog_names = names(&catalog);
        for core in ["bash", "read", "edit", "apply_patch", "glob", "grep"] {
            assert!(
                !catalog_names.contains(&core),
                "base catalog leaked core {core}"
            );
        }
    }

    #[test]
    fn program_catalog_matches_the_dispatch_allowlist() {
        let catalog = program_specs_for_run_full("build", "developer", "write", None, None, true);
        let names = names(&catalog);
        // Full mode hides glob/grep/webfetch entirely; the rest are pinned.
        assert_eq!(names, vec!["bash", "read", "apply_patch", "edit"]);
        for spec in &catalog {
            assert!(spec["function"]["description"].is_string());
        }
        let text = render_code_mode_catalog_full("build", "developer", "write", None, None, true);
        assert!(text.starts_with("# Code Mode"));
        assert!(text.contains("The catalog is complete. Do not guess tool names."));
        assert!(text.contains("tools.read({"));
        assert!(text.contains("file_path: string,"));
        assert!(text.contains(": Promise<string | null>"));
        assert!(!text.contains("tools.spawn_agent"));
    }

    #[test]
    fn program_catalog_honors_the_agent_allowlist() {
        let allowed = vec!["read".to_string()];
        let catalog =
            program_specs_for_run_full("build", "developer", "write", Some(&allowed), None, true);
        assert_eq!(names(&catalog), vec!["read"]);

        // The dispatcher is implicit: an allowlist that names only nested
        // tools still leaves `execute` as the direct entry point.
        let direct = tool_specs_for_run_with_code_mode(
            "build",
            "developer",
            "write",
            &HashSet::new(),
            Some(&allowed),
            None,
            CodeMode::Only,
        );
        assert_eq!(names(&direct), vec!["execute"]);
    }

    #[tokio::test]
    async fn direct_program_calls_are_repaired_inside_execute() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "repair me").unwrap();
        let ctx = coding(dir.path());
        let outcome = dispatch("read", &json!({"file_path": "a.txt"}), &ctx).await;
        assert!(!outcome.is_error, "{}", outcome.content);
        assert!(
            outcome
                .content
                .starts_with("<tool-repair>read is a Code Mode tool"),
            "{}",
            outcome.content
        );
        assert!(outcome.content.contains("repair me"), "{}", outcome.content);
    }

    #[tokio::test]
    async fn nested_calls_reject_direct_only_tools() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = coding(dir.path()).with_in_program(true);
        let outcome = dispatch(
            "spawn_agent",
            &json!({"task_name": "nested", "message": "x"}),
            &ctx,
        )
        .await;
        assert!(outcome.is_error, "{}", outcome.content);
        assert!(
            outcome
                .content
                .contains("cannot be called from inside execute"),
            "{}",
            outcome.content
        );

        let ctx = coding(dir.path());
        let off = ctx.with_code_mode(CodeMode::Off);
        let outcome = dispatch("execute", &json!({"code": "return 1;"}), &off).await;
        assert!(outcome.is_error, "{}", outcome.content);
        assert!(
            outcome.content.contains("Code Mode is disabled"),
            "{}",
            outcome.content
        );
    }

    #[tokio::test]
    async fn plan_mode_programs_run_reads_but_block_writes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "plan read").unwrap();
        let ctx = coding(dir.path()).with_mode("plan");
        let read = dispatch(
            "execute",
            &json!({"code": "return tools.read({ file_path: 'a.txt' });"}),
            &ctx,
        )
        .await;
        assert!(!read.is_error, "{}", read.content);
        assert!(read.content.contains("plan read"), "{}", read.content);

        let write = dispatch(
            "execute",
            &json!({"code": "return tools.edit({ file_path: 'a.txt', old_string: 'plan', new_string: 'build' });"}),
            &ctx,
        )
        .await;
        assert_eq!(
            write.metadata.as_ref().and_then(|m| m["error"].as_bool()),
            Some(true)
        );
        assert!(write.content.contains("[BLOCKED]"), "{}", write.content);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "plan read"
        );
    }

    #[tokio::test]
    async fn read_only_programs_read_but_cannot_edit() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "read only").unwrap();
        let ctx = coding(dir.path()).with_access("read");
        let read = dispatch(
            "execute",
            &json!({"code": "return tools.read({ file_path: 'a.txt' });"}),
            &ctx,
        )
        .await;
        assert!(!read.is_error, "{}", read.content);
        assert!(read.content.contains("read only"), "{}", read.content);
        assert!(
            !read.content.contains("<tool-repair>"),
            "nested program calls must not be repaired: {}",
            read.content
        );

        let write = dispatch(
            "execute",
            &json!({"code": "return tools.edit({ file_path: 'a.txt', old_string: 'read', new_string: 'write' });"}),
            &ctx,
        )
        .await;
        assert_eq!(
            write.metadata.as_ref().and_then(|m| m["error"].as_bool()),
            Some(true)
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "read only"
        );
    }

    #[tokio::test]
    async fn full_codemode_bun_and_fetch_cross_the_rust_host() {
        let dir = tempfile::tempdir().unwrap();
        let manager = crate::WorkerManager::local(dir.path());
        let ctx = coding(dir.path())
            .with_full_codemode(true)
            .with_worker_manager(manager);
        let outcome = dispatch(
            "execute",
            &json!({
                "code": "await Bun.write('sub/dir/out.txt', 'hello'); const text = await Bun.file('sub/dir/out.txt').text(); const files = new Bun.Glob('**/*.txt').scanSync(); const fetched = await (await fetch('data:text/plain,hi')).text(); return { text, files, fetched };"
            }),
            &ctx,
        )
        .await;
        assert!(!outcome.is_error, "{}", outcome.content);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("sub/dir/out.txt")).unwrap(),
            "hello"
        );
        assert!(
            outcome.content.contains("\"text\":\"hello\""),
            "{}",
            outcome.content
        );
        assert!(outcome.content.contains("out.txt"), "{}", outcome.content);
        assert!(
            outcome.content.contains("\"fetched\":\"hi\""),
            "{}",
            outcome.content
        );
    }

    #[tokio::test]
    async fn execute_on_keeps_its_own_inventory_after_another_initialize() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "inventory a").unwrap();
        let ctx_a = coding(dir.path()).with_in_program(true);
        let defs_a = codemode::tool_defs_for_context(&ctx_a);
        // A different session with no program tools initializes last.
        let ctx_b = coding(dir.path())
            .with_in_program(true)
            .with_allowed_tools(Some(Vec::new()));
        let defs_b = codemode::tool_defs_for_context(&ctx_b);
        assert!(!defs_a.is_empty());
        assert!(defs_b.is_empty());

        let manager = crate::WorkerManager::local(dir.path());
        manager.initialize(&defs_a).await.unwrap();
        manager.initialize(&defs_b).await.unwrap();
        let inner = manager.prepare().await.unwrap();
        let result = manager
            .execute_on(
                inner,
                "run_inventory_a",
                &ctx_a,
                "return await tools.read({ file_path: 'a.txt' });",
                false,
                &defs_a,
            )
            .await
            .unwrap();
        assert!(!result.error, "{}", result.text);
        assert!(result.text.contains("inventory a"), "{}", result.text);
        assert!(
            !result.text.contains("<tool-repair>"),
            "the other inventory's initialize must not change this call: {}",
            result.text
        );
    }

    #[tokio::test]
    async fn code_mode_workers_are_isolated_per_chat() {
        let dir = tempfile::tempdir().unwrap();
        let manager_a = crate::WorkerManager::for_chat(dir.path(), "chat-a");
        let manager_b = crate::WorkerManager::for_chat(dir.path(), "chat-b");
        let cleanup_a = manager_a.clone();
        let cleanup_b = manager_b.clone();
        assert!(!Arc::ptr_eq(&manager_a, &manager_b));
        assert!(Arc::ptr_eq(
            &manager_a,
            &crate::WorkerManager::for_chat(dir.path(), "chat-a")
        ));

        let ctx_a = coding(dir.path()).with_worker_manager(manager_a);
        let cancel = ctx_a.cancel.clone();
        let blocking = tokio::spawn(async move {
            dispatch(
                "execute",
                &json!({"code": "for (;;) {}", "timeout": 600_000}),
                &ctx_a,
            )
            .await
        });
        // Let A start its synchronous loop before testing B.
        tokio::time::sleep(std::time::Duration::from_millis(1_500)).await;

        // Chat B must keep working while A is stuck.
        let ctx_b = coding(dir.path()).with_worker_manager(manager_b);
        let out = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            dispatch("execute", &json!({"code": "return 2"}), &ctx_b),
        )
        .await
        .expect("chat B must not wait on chat A");
        assert!(!out.is_error, "{}", out.content);

        cancel.cancel();
        let out = tokio::time::timeout(std::time::Duration::from_secs(15), blocking)
            .await
            .expect("chat A cancel must not hang")
            .unwrap();
        assert_eq!(out.class, Some(ToolErrorClass::Cancelled));
        cleanup_a.shutdown().await;
        cleanup_b.shutdown().await;
    }

    #[tokio::test]
    async fn execute_short_circuits_an_already_cancelled_run() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let ctx = coding(dir.path()).with_cancel(cancel);
        let out = dispatch("execute", &json!({"code": "return 1"}), &ctx).await;
        assert!(out.is_error, "{}", out.content);
        assert_eq!(
            out.class,
            Some(ToolErrorClass::Cancelled),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn cancel_kills_an_unresponsive_worker_and_respawns() {
        let dir = tempfile::tempdir().unwrap();
        let manager = crate::WorkerManager::local(dir.path());
        let ctx = coding(dir.path()).with_worker_manager(manager.clone());
        let cancel = ctx.cancel.clone();
        let started = dir.path().join("started.txt");
        let task = tokio::spawn(async move {
            dispatch(
                "execute",
                &json!({
                    "code": "await Bun.write('started.txt', '1'); for (;;) {}",
                    "timeout": 600_000
                }),
                &ctx,
            )
            .await
        });
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
        while !started.is_file() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the program never reached its loop"
            );
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let cancelled_at = tokio::time::Instant::now();
        cancel.cancel();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(15), task)
            .await
            .expect("cancel must not hang on a synchronous program")
            .unwrap();
        assert!(outcome.is_error, "{}", outcome.content);
        assert_eq!(
            outcome.class,
            Some(ToolErrorClass::Cancelled),
            "{}",
            outcome.content
        );
        assert!(
            cancelled_at.elapsed() < std::time::Duration::from_secs(12),
            "cancel took {:?}",
            cancelled_at.elapsed()
        );

        // The killed child must be replaced lazily, not leave a dead manager.
        let ctx = coding(dir.path()).with_worker_manager(manager);
        let out = dispatch("execute", &json!({"code": "return 1"}), &ctx).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains('1'), "{}", out.content);
    }

    #[tokio::test]
    async fn execute_results_use_opencode_truncation_and_spill() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = coding(dir.path()).with_worker_manager(crate::WorkerManager::local(dir.path()));
        let outcome = dispatch(
            "execute",
            &json!({
                "code": "return Array.from({ length: 3000 }, (_, i) => 'line ' + i).join('\\n');"
            }),
            &ctx,
        )
        .await;
        assert!(!outcome.is_error, "{}", outcome.content);
        assert!(
            outcome
                .content
                .contains("[showing lines 1-2000 of 3000; full output saved to"),
            "{}",
            outcome.content
        );
        let metadata = outcome.metadata.expect("metadata");
        assert_eq!(metadata["truncated"], true);
        let path = metadata["outputPath"].as_str().expect("output path");
        let full = std::fs::read_to_string(path).unwrap();
        assert!(full.starts_with("line 0\n"));
        assert!(full.ends_with("line 2999"));
    }

    #[tokio::test]
    async fn background_job_results_are_truncated_and_delivered_once() {
        use std::sync::Mutex;

        let dir = tempfile::tempdir().unwrap();
        let notices: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let captured = notices.clone();
        let ctx = coding(dir.path())
            .with_full_codemode(true)
            .with_worker_manager(crate::WorkerManager::local(dir.path()))
            .with_background_hook(Some(Arc::new(move |message| {
                captured.lock().unwrap().push(message);
            })));
        let outcome = dispatch(
            "execute",
            &json!({
                "background": true,
                "code": "return Array.from({ length: 3000 }, (_, i) => 'line ' + i).join('\\n');"
            }),
            &ctx,
        )
        .await;
        assert!(!outcome.is_error, "{}", outcome.content);
        assert!(outcome.content.contains("still running"));

        let mut message = String::new();
        for _ in 0..100 {
            if let Some(first) = notices.lock().unwrap().first().cloned() {
                message = first;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            message.contains("[showing lines 1-2000 of 3000; full output saved to"),
            "{message}"
        );
        assert!(
            message.contains("<background-result kind=\"codemode\""),
            "{message}"
        );

        // The delivered row is gone; only the spill file remains.
        let rows = std::fs::read_dir(dir.path().join(".deep_agents/codemode"))
            .map(|entries| entries.flatten().count())
            .unwrap_or(0);
        assert_eq!(rows, 0, "delivered job rows left: {rows}");
        let spill = std::fs::read_dir(dir.path().join(".deep_agents/tool-output")).unwrap();
        assert!(
            spill
                .flatten()
                .any(|entry| { entry.file_name().to_string_lossy().starts_with("tool_") })
        );
    }

    #[tokio::test]
    async fn foreground_timeout_moves_the_script_to_the_background() {
        use std::sync::Mutex;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0u8; 1024];
            let _ = socket.read(&mut buffer).await;
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            let _ = socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await;
        });

        let dir = tempfile::tempdir().unwrap();
        let notices: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let captured = notices.clone();
        let deltas: Arc<Mutex<Vec<stark_contract::Delta>>> = Arc::new(Mutex::new(Vec::new()));
        let progress = deltas.clone();
        let ctx = coding(dir.path())
            .with_full_codemode(true)
            .with_worker_manager(crate::WorkerManager::local(dir.path()))
            .with_code_delta(Arc::new(move |delta| {
                progress.lock().unwrap().push(delta);
            }))
            .with_background_hook(Some(Arc::new(move |message| {
                captured.lock().unwrap().push(message);
            })));
        let outcome = dispatch(
            "execute",
            &json!({
                "timeout": 50,
                "code": format!(
                    "const text = await (await fetch('http://{address}/')).text(); await Bun.write('late.txt', text); return text;"
                )
            }),
            &ctx,
        )
        .await;
        assert!(!outcome.is_error, "{}", outcome.content);
        let job_id = outcome
            .metadata
            .as_ref()
            .and_then(|metadata| metadata["jobID"].as_str())
            .expect("job id");
        assert!(job_id.starts_with("cm_"));
        assert!(outcome.content.contains("still running"));

        let mut delivered = false;
        for _ in 0..80 {
            if !notices.lock().unwrap().is_empty() {
                delivered = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(delivered, "no background notice");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("late.txt")).unwrap(),
            "ok"
        );
        let message = notices.lock().unwrap().first().cloned().unwrap_or_default();
        assert!(
            message.contains("<background-result kind=\"codemode\""),
            "{message}"
        );
        assert!(message.contains("ok"), "{message}");

        let progress: Vec<stark_contract::Delta> = deltas.lock().unwrap().clone();
        assert!(
            progress.iter().any(|delta| {
                delta.kind == "tool_progress"
                    && delta
                        .metadata
                        .as_ref()
                        .and_then(|metadata| metadata["toolCalls"].as_array())
                        .is_some_and(|calls| calls.iter().any(|call| call["tool"] == "fetch"))
            }),
            "{progress:?}"
        );
    }

    #[tokio::test]
    async fn full_codemode_bun_writes_respect_plan_and_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = coding(dir.path())
            .with_mode("plan")
            .with_full_codemode(true)
            .with_worker_manager(crate::WorkerManager::local(dir.path()));
        let outcome = dispatch(
            "execute",
            &json!({"code": "await Bun.write('blocked.txt', 'no'); return 'unreachable';"}),
            &ctx,
        )
        .await;
        assert_eq!(
            outcome
                .metadata
                .as_ref()
                .and_then(|value| value["error"].as_bool()),
            Some(true)
        );
        assert!(outcome.content.contains("[BLOCKED]"), "{}", outcome.content);
        assert!(!dir.path().join("blocked.txt").exists());

        let read_only = coding(dir.path())
            .with_access("read")
            .with_full_codemode(true)
            .with_worker_manager(crate::WorkerManager::local(dir.path()));
        let outcome = dispatch(
            "execute",
            &json!({"code": "await Bun.write('blocked.txt', 'no'); return 'unreachable';"}),
            &read_only,
        )
        .await;
        assert_eq!(
            outcome
                .metadata
                .as_ref()
                .and_then(|value| value["error"].as_bool()),
            Some(true)
        );
        assert!(outcome.content.contains("Read-only"), "{}", outcome.content);
        assert!(!dir.path().join("blocked.txt").exists());
    }

    #[test]
    fn full_codemode_matches_the_opencode_surface() {
        let direct = tool_specs_for_run_with_code_mode_full(
            "build",
            "developer",
            "write",
            &HashSet::new(),
            None,
            None,
            CodeMode::Only,
            true,
        );
        let direct_names = names(&direct);
        assert_eq!(direct_names.last(), Some(&"execute"), "{direct_names:?}");
        assert!(!direct_names.contains(&"read"), "{direct_names:?}");
        assert!(!direct_names.contains(&"edit"), "{direct_names:?}");
        assert!(!direct_names.contains(&"bash"), "{direct_names:?}");
        assert!(!direct_names.contains(&"glob"), "{direct_names:?}");
        assert!(!direct_names.contains(&"grep"), "{direct_names:?}");

        let execute = direct
            .iter()
            .find(|spec| spec["function"]["name"] == "execute")
            .expect("execute");
        let properties = execute
            .pointer("/function/parameters/properties")
            .expect("execute properties");
        assert!(properties.get("timeout").is_some());
        assert!(properties.get("background").is_some());
        assert!(properties.get("max_tool_calls").is_none());
        assert!(
            execute["function"]["description"]
                .as_str()
                .unwrap()
                .contains("Bun.file")
        );

        let program = program_specs_for_run_full("build", "developer", "write", None, None, true);
        let program_names = names(&program);
        assert!(program_names.contains(&"read"), "{program_names:?}");
        assert!(program_names.contains(&"edit"), "{program_names:?}");
        assert!(program_names.contains(&"bash"), "{program_names:?}");
        assert!(!program_names.contains(&"glob"), "{program_names:?}");
        assert!(!program_names.contains(&"grep"), "{program_names:?}");
        let bash = program
            .iter()
            .find(|spec| spec["function"]["name"] == "bash")
            .expect("bash");
        assert!(
            bash.pointer("/function/parameters/properties")
                .unwrap()
                .get("run_in_background")
                .is_none()
        );

        let catalog =
            render_code_mode_catalog_full("build", "developer", "write", None, None, true);
        assert!(catalog.contains("tools.read({"), "{catalog}");
        assert!(
            catalog.contains(
                "tools.apply_patch(/** Patch text. Start with *** Begin Patch and end with *** End Patch. */ input: string): Promise<string | null>"
            ),
            "{catalog}"
        );
        assert!(!catalog.contains("tools.glob("), "{catalog}");
        assert!(!catalog.contains("tools.grep("), "{catalog}");
    }

    #[test]
    fn custom_scope_can_opt_into_hybrid_code_mode_with_execute() {
        let tools = vec![
            "read".to_string(),
            "files_changed".to_string(),
            "execute".to_string(),
        ];
        let direct = tool_specs_for_run_with_code_mode(
            "build",
            "backend",
            "read",
            &HashSet::new(),
            Some(&tools),
            Some(&[]),
            CodeMode::Hybrid,
        );
        let direct_names = names(&direct);
        assert!(direct_names.contains(&"read"), "{direct_names:?}");
        assert!(direct_names.contains(&"execute"), "{direct_names:?}");
        assert!(!direct_names.contains(&"files_changed"), "{direct_names:?}");
        let program =
            program_specs_for_run_full("build", "backend", "read", Some(&tools), Some(&[]), false);
        let program_names = names(&program);
        assert!(
            program_names.contains(&"files_changed"),
            "{program_names:?}"
        );
        assert!(!program_names.contains(&"execute"), "{program_names:?}");
    }

    #[test]
    fn custom_scope_is_closed_by_its_acl() {
        let tools = vec!["read".to_string(), "grep".to_string()];
        let specs = tool_specs_for_run_with_code_mode(
            "build",
            "backend",
            "read",
            &HashSet::new(),
            Some(&tools),
            Some(&[]),
            CodeMode::Off,
        );
        let names = names(&specs);
        assert_eq!(names, vec!["read", "grep"]);
        assert!(!names.contains(&"edit"));
        // A custom scope with no explicit ACL must expose nothing, not the
        // whole built-in inventory.
        let none = tool_specs_for_run_with_code_mode(
            "build",
            "backend",
            "write",
            &HashSet::new(),
            None,
            None,
            CodeMode::Off,
        );
        assert!(names_of(&none).is_empty());
    }

    fn names_of(specs: &[serde_json::Value]) -> Vec<String> {
        specs
            .iter()
            .filter_map(|spec| {
                spec.get("function")
                    .and_then(|function| function.get("name"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
            .collect()
    }
}
