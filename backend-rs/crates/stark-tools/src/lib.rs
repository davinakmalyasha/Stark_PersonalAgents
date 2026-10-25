mod approvals;
mod ask;
mod browser;
mod changes;
mod checkpoint;
mod command;
mod dfd;
mod docs;
mod edit;
mod head_tail;
mod house;
mod patch;
mod paths;
mod plan;
mod processes;
mod questions;
mod read;
mod search;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use serde_json::{Value, json};
use stark_contract::{PlanItem, ToolSpecFile};

pub use approvals::ApprovalCache;
pub use checkpoint::{Checkpoint, RestoreReport, SnapshotGroup};
pub use house::{HouseStore, public_device, public_state};
pub use plan::{PlanError, PlanStore, update_plan};
pub use processes::{ProcessManager, ProcessStatus};
pub use questions::QuestionBroker;

const IMPLEMENTED: &[&str] = &[
    "read_file",
    "edit_file",
    "run_command",
    "search_code",
    "ask_user",
    "call_subagent",
    "update_plan",
    "browser_act",
    "delegate",
    "delegation_status",
    "job_message",
    "job_cancel",
    "give_note",
    "house_get_state",
    "house_get_device",
    "house_set_device",
    "house_run_scene",
    "house_get_history",
    "dfd_bridge",
    "create_client",
    "files_changed",
    "read_pdf",
    "create_docx",
    "create_xlsx",
    "create_coding_project",
    "create_group",
    "group_board",
    "group_item_detail",
    "group_events",
    "group_update",
    "schedule",
];

pub const PLAN_WRITE_TOOLS: &[&str] = &[
    "edit_file",
    "create_project",
    "create_coding_project",
    "create_group",
    "create_client",
    "github_apply_gate",
    "house_set_device",
    "house_run_scene",
    "group_update",
    "create_docx",
    "create_xlsx",
    "delegate",
    "call_subagent",
];

static CONTRACT: OnceLock<ToolSpecFile> = OnceLock::new();

#[derive(Clone)]
pub struct ToolContext {
    pub workspace: PathBuf,
    pub chat_id: String,
    pub questions: Arc<QuestionBroker>,
    pub processes: Arc<ProcessManager>,
    pub checkpoint: Option<Arc<Checkpoint>>,
    pub approvals: Arc<ApprovalCache>,
    pub plan_store: Arc<PlanStore>,
    pub harness_root: PathBuf,
    pub scope: String,
    pub mode: String,
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
            workspace,
            chat_id: chat_id.into(),
            questions,
            processes: Arc::new(ProcessManager::new()),
            checkpoint: None,
            approvals: Arc::new(ApprovalCache::new()),
            scope: "agent".to_string(),
            mode: "build".to_string(),
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

    pub fn with_mode(mut self, mode: impl Into<String>) -> Self {
        self.mode = mode.into();
        self
    }
}

#[derive(Debug, Clone)]
pub struct ToolOutcome {
    pub content: String,
    pub is_error: bool,
    pub exit_code: Option<i32>,
    pub plan: Option<BTreeMap<String, Vec<PlanItem>>>,
}

impl ToolOutcome {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            exit_code: None,
            plan: None,
        }
    }

    pub fn err(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            exit_code: None,
            plan: None,
        }
    }

    pub fn with_code(content: impl Into<String>, is_error: bool, exit_code: Option<i32>) -> Self {
        Self {
            content: content.into(),
            is_error,
            exit_code,
            plan: None,
        }
    }

    pub fn with_plan(mut self, plan: BTreeMap<String, Vec<PlanItem>>) -> Self {
        self.plan = Some(plan);
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
    let scope = scope.trim().to_lowercase();
    let allowed = contract().scope_map.get(&scope);
    contract()
        .tools
        .iter()
        .filter(|tool| {
            tool.exposed
                && IMPLEMENTED.contains(&tool.name.as_str())
                && allowed.is_none_or(|names| names.contains(&tool.name))
                && !(mode == "plan" && PLAN_WRITE_TOOLS.contains(&tool.name.as_str()))
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
        .collect()
}

pub fn plan_mode_denial(mode: &str, name: &str, args: &Value) -> Option<String> {
    if mode != "plan" {
        return None;
    }
    if PLAN_WRITE_TOOLS.contains(&name) {
        return Some(format!(
            "[BLOCKED] Plan mode is read-only. Type /build to leave plan mode. ({name} is a write-class tool.)"
        ));
    }
    if name == "run_command" {
        let process_action = args
            .get("process_action")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let background = args
            .get("background")
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
                "[BLOCKED] Plan mode is read-only. Type /build to leave plan mode. (run_command with write/side-effect verbs is blocked: {reason})"
            ));
        }
    }
    None
}

pub async fn dispatch(name: &str, args: &Value, ctx: &ToolContext) -> ToolOutcome {
    if let Some(reason) = plan_mode_denial(&ctx.mode, name, args) {
        return ToolOutcome::err(reason);
    }
    match name {
        "read_file" => read::read_file(ctx, args).await,
        "edit_file" => edit::edit_file(ctx, args).await,
        "search_code" => search::search_code(ctx, args).await,
        "run_command" => command::run_command(ctx, args).await,
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
        "read_pdf" => docs::read_pdf(ctx, args).await,
        "create_docx" => docs::create_docx(ctx, args).await,
        "create_xlsx" => docs::create_xlsx(ctx, args).await,
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
    }

    #[test]
    fn scope_limits_tools_to_agent_acl() {
        let developer = tool_specs_for_scope("build", "developer");
        let names = tool_names(&developer);
        assert!(names.contains(&"edit_file"));
        assert!(names.contains(&"call_subagent"));
        assert!(names.contains(&"update_plan"));
        assert!(!names.contains(&"read_pdf"));

        let general = tool_specs_for_scope("build", "general");
        let names = tool_names(&general);
        assert_eq!(names.len(), 8);
        assert!(names.contains(&"run_command"));
        assert!(names.contains(&"update_plan"));
        assert!(names.contains(&"read_pdf"));
        assert!(names.contains(&"create_docx"));
        assert!(!names.contains(&"edit_file"));
        assert!(!names.contains(&"read_file"));

        let dfd = tool_specs_for_scope("build", "dfd");
        let names = tool_names(&dfd);
        assert_eq!(names.len(), 7);
        assert!(names.contains(&"create_coding_project"));
        assert!(names.contains(&"dfd_bridge"));
        assert!(names.contains(&"create_client"));
        assert!(names.contains(&"files_changed"));

        let house = tool_specs_for_scope("build", "house");
        let names = tool_names(&house);
        assert_eq!(names.len(), 9);
        assert!(names.contains(&"house_set_device"));
        assert!(names.contains(&"house_run_scene"));

        let orchestrator = tool_specs_for_scope("build", "orchestrator");
        let names = tool_names(&orchestrator);
        assert_eq!(names.len(), 13);
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
            31
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
