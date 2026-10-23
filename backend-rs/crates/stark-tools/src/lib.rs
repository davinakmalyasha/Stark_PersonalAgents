mod ask;
mod command;
mod edit;
mod head_tail;
mod patch;
mod paths;
mod processes;
mod questions;
mod read;
mod search;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use serde_json::{Value, json};
use stark_contract::ToolSpecFile;

pub use processes::{ProcessManager, ProcessStatus};
pub use questions::QuestionBroker;

const IMPLEMENTED: &[&str] = &[
    "read_file",
    "edit_file",
    "run_command",
    "search_code",
    "ask_user",
];

static CONTRACT: OnceLock<ToolSpecFile> = OnceLock::new();

#[derive(Clone)]
pub struct ToolContext {
    pub workspace: PathBuf,
    pub chat_id: String,
    pub questions: Arc<QuestionBroker>,
    pub processes: Arc<ProcessManager>,
}

impl ToolContext {
    pub fn new(
        workspace: impl Into<PathBuf>,
        chat_id: impl Into<String>,
        questions: Arc<QuestionBroker>,
    ) -> Self {
        Self {
            workspace: workspace.into(),
            chat_id: chat_id.into(),
            questions,
            processes: Arc::new(ProcessManager::new()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolOutcome {
    pub content: String,
    pub is_error: bool,
    pub exit_code: Option<i32>,
}

impl ToolOutcome {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            exit_code: None,
        }
    }

    pub fn err(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            exit_code: None,
        }
    }

    pub fn with_code(content: impl Into<String>, is_error: bool, exit_code: Option<i32>) -> Self {
        Self {
            content: content.into(),
            is_error,
            exit_code,
        }
    }
}

pub fn contract() -> &'static ToolSpecFile {
    CONTRACT.get_or_init(|| {
        ToolSpecFile::from_json_str(include_str!("../../../specs/tool-specs.json"))
            .expect("frozen tool contract must parse")
    })
}

pub fn tool_specs() -> Vec<Value> {
    contract()
        .tools
        .iter()
        .filter(|tool| tool.exposed && IMPLEMENTED.contains(&tool.name.as_str()))
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

pub async fn dispatch(name: &str, args: &Value, ctx: &ToolContext) -> ToolOutcome {
    match name {
        "read_file" => read::read_file(ctx, args).await,
        "edit_file" => edit::edit_file(ctx, args).await,
        "search_code" => search::search_code(ctx, args).await,
        "run_command" => command::run_command(ctx, args).await,
        "ask_user" => ask::ask_user(ctx, args).await,
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
            &json!({"command": "printf 'pty-hello\\n'", "tty": true, "timeout": 3000}),
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
                "yield_time_ms": 5000
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
