mod ask;
mod command;
mod edit;
mod paths;
mod questions;
mod read;
mod search;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use serde_json::{Value, json};
use stark_contract::ToolSpecFile;

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
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                }
            })
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
