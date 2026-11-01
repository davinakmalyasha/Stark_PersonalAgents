use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use stark_store::WorkspaceRegistry;
use stark_tools::ToolOutcome;
use tokio::sync::broadcast;

use crate::{RunContext, read_report, run_agent};

const DEFAULT_PROJECTS_ROOT: &str = r"D:\MyProject";
const DFD_CLIENTS_ROOT: &str = r"D:\MyProject\DFDClientProjects";

pub(crate) async fn create_coding_project(ctx: &RunContext, args: &Value) -> ToolOutcome {
    create_project_in(ctx, args, None).await
}

async fn create_project_in(
    ctx: &RunContext,
    args: &Value,
    root_override: Option<PathBuf>,
) -> ToolOutcome {
    let raw_name = args
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let Some(name) = sanitize_name(raw_name) else {
        return ToolOutcome::err(
            "Error: Project name is empty after sanitizing. Provide a valid name.",
        );
    };
    if matches!(name.to_lowercase().as_str(), "general" | "_general") {
        return ToolOutcome::err("Error: 'General' is reserved for the general chat workspace.");
    }
    if name.len() > 64 {
        return ToolOutcome::err("Error: Project name must be at most 64 characters.");
    }
    let concept = args
        .get("concept")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let parent_root = args
        .get("parent_root")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let base = root_override.unwrap_or_else(projects_root);
    let parent = if parent_root.is_empty() {
        base
    } else {
        let parent = PathBuf::from(parent_root);
        let dfd_root = std::env::var("DFD_CLIENTS_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DFD_CLIENTS_ROOT));
        if !path_is_within(&parent, &dfd_root) {
            return ToolOutcome::err(format!(
                "Error: create_project parent_root must be inside {}; got '{}'.",
                dfd_root.display(),
                parent.display()
            ));
        }
        parent
    };
    let path = parent.join(&name);
    if path.exists() && !path.is_dir() {
        return ToolOutcome::err(format!(
            "Error: Cannot create project: '{}' exists and is not a directory.",
            path.display()
        ));
    }
    if let Err(error) = tokio::fs::create_dir_all(&path).await {
        return ToolOutcome::err(format!(
            "Error: Failed to create project folder '{}': {error}",
            path.display()
        ));
    }

    let registry = WorkspaceRegistry::new(ctx.store.harness_root());
    let already_existed = registry
        .find_by_path(path.to_string_lossy().as_ref())
        .is_some();
    let workspace = match registry.add(path.to_string_lossy().as_ref(), &name) {
        Ok(workspace) => workspace,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };
    let chat = match ctx
        .store
        .create_linked_chat(
            &workspace.id,
            &workspace.path,
            &format!("{name} — Dev Session"),
            &ctx.model,
            &ctx.chat_id,
            &ctx.workspace_id,
        )
        .await
    {
        Ok(chat) => chat,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };
    let chat_id = chat["id"].as_str().unwrap_or("").to_string();
    let task = if concept.is_empty() {
        "Get familiar with the empty project and report what you will build.".to_string()
    } else {
        concept
    };
    let wait = args.get("wait").and_then(Value::as_bool).unwrap_or(true);

    if wait {
        let (child, latest) = project_context(ctx, &chat_id, &workspace.id, &path);
        if let Err(error) = run_agent(child, task).await {
            return ToolOutcome::err(format!("Error: Failed to set up project '{name}': {error}"));
        }
        ToolOutcome::ok(format!(
            "[OK] Project '{name}' created.\nFolder: {}\nWorkspace: {}\nChat: {chat_id}\n\nDev's first report:\n{}",
            path.display(),
            workspace.id,
            read_report(&latest)
        ))
    } else {
        let (child, latest) = project_context(ctx, &chat_id, &workspace.id, &path);
        let hook = ctx.on_job_done.clone();
        let name_for_task = name.clone();
        let chat_for_task = chat_id.clone();
        tokio::spawn(async move {
            let result = run_agent(child, task).await;
            if let Some(hook) = hook {
                let body = match result {
                    Ok(()) => read_report(&latest),
                    Err(error) => format!("failed: {error}"),
                };
                hook(format!(
                    "[PROJECT RESULT] {name_for_task}\nchat: {chat_for_task}\n{body}"
                ));
            }
        });
        ToolOutcome::ok(format!(
            "[OK] Project '{name}' created.\nFolder: {}\nWorkspace: {}\nChat: {chat_id}\nDev agent: starting in the background — its first report lands in the project chat.",
            path.display(),
            workspace.id
        ))
    }
    .with_project_note(already_existed)
}

fn project_context(
    ctx: &RunContext,
    chat_id: &str,
    workspace_id: &str,
    workspace: &Path,
) -> (RunContext, Arc<Mutex<Option<stark_contract::AppState>>>) {
    let (tx, _) = broadcast::channel(64);
    let latest = Arc::new(Mutex::new(None));
    let child = RunContext {
        chat_id: chat_id.to_string(),
        workspace_id: workspace_id.to_string(),
        workspace: workspace.to_path_buf(),
        role: Some("Developer".to_string()),
        scope: "developer".to_string(),
        depth: ctx.depth + 1,
        file_access: crate::FileAccess::Write,
        model: ctx.model.clone(),
        temperature: ctx.temperature,
        images: Vec::new(),
        provider: ctx.provider.clone(),
        store: ctx.store.clone(),
        questions: ctx.questions.clone(),
        tx,
        latest: latest.clone(),
        steering: Arc::new(Mutex::new(VecDeque::new())),
        follow_up: Arc::new(Mutex::new(VecDeque::new())),
        checkpoint: None,
        cancel: ctx.cancel.child_token(),
        config: None,
        on_job_done: None,
    };
    (child, latest)
}

fn projects_root() -> PathBuf {
    std::env::var("STARK_PROJECTS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_PROJECTS_ROOT))
}

fn sanitize_name(value: &str) -> Option<String> {
    let mut name = String::new();
    let mut pending_dash = false;
    for character in value.trim().chars() {
        if character.is_ascii_alphanumeric() {
            if pending_dash && !name.is_empty() {
                name.push('-');
            }
            pending_dash = false;
            name.push(character.to_ascii_lowercase());
        } else if matches!(character, '-' | '_' | '.') {
            if !name.is_empty() {
                name.push(character);
            }
        } else {
            pending_dash = true;
        }
    }
    let name = name.trim_matches(['-', '_', '.']).to_string();
    if name.is_empty() || name == "." || name == ".." || name.starts_with('.') {
        None
    } else {
        Some(name)
    }
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    let path = path.to_string_lossy().to_lowercase().replace('/', "\\");
    let root = root.to_string_lossy().to_lowercase().replace('/', "\\");
    path == root
        || path
            .strip_prefix(&root)
            .is_some_and(|rest| rest.starts_with('\\'))
}

trait ProjectNote {
    fn with_project_note(self, already_existed: bool) -> Self;
}

impl ProjectNote for ToolOutcome {
    fn with_project_note(self, already_existed: bool) -> Self {
        if already_existed {
            ToolOutcome::ok(format!(
                "{}\n\n[OK] Project workspace already existed; created a new Dev session.",
                self.content
            ))
        } else {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::header;
    use axum::response::Response;
    use axum::routing::post;
    use axum::{Json, Router};
    use stark_providers::{Provider, ProviderConfig};
    use stark_store::ChatStore;
    use stark_tools::QuestionBroker;

    use super::*;
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    async fn mock_provider(Json(_body): Json<Value>) -> Response {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Project ready\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        );
        Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(body))
            .unwrap()
    }

    #[tokio::test]
    async fn creates_project_workspace_chat_and_developer_report() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route("/chat/completions", post(mock_provider)),
            )
            .await
            .unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let projects = dir.path().join("projects");
        let store = ChatStore::open(dir.path()).await.unwrap();
        let provider = Provider::new(ProviderConfig {
            mode: "deepseek".to_string(),
            base_url: format!("http://{address}"),
            api_key: "test".to_string(),
            model: "mock".to_string(),
        })
        .unwrap();
        let (tx, _rx) = broadcast::channel(64);
        let ctx = RunContext {
            chat_id: "orchestrator".to_string(),
            workspace_id: "_general".to_string(),
            workspace: dir.path().to_path_buf(),
            role: Some("Orchestrator".to_string()),
            scope: "orchestrator".to_string(),
            depth: 0,
            file_access: crate::FileAccess::Write,
            model: "mock".to_string(),
            temperature: 0.0,
            images: Vec::new(),
            provider,
            store: store.clone(),
            questions: Arc::new(QuestionBroker::new()),
            tx,
            latest: Arc::new(Mutex::new(None)),
            steering: Arc::new(Mutex::new(VecDeque::new())),
            follow_up: Arc::new(Mutex::new(VecDeque::new())),
            checkpoint: None,
            cancel: CancellationToken::new(),
            config: None,
            on_job_done: None,
        };
        let result = create_project_in(
            &ctx,
            &json!({"name": "Snake Game", "concept": "Build it", "wait": true}),
            Some(projects.clone()),
        )
        .await;
        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("snake-game"));
        assert!(result.content.contains("Project ready"));
        let path = projects.join("snake-game");
        assert!(path.is_dir());
        let registry = WorkspaceRegistry::new(dir.path());
        let workspace = registry
            .find_by_path(path.to_string_lossy().as_ref())
            .expect("workspace registered");
        let chats = store.list_chats(&workspace.id).await;
        assert_eq!(chats.len(), 1);
        assert!(chats[0]["title"].as_str().unwrap().contains("Dev Session"));
    }
}
