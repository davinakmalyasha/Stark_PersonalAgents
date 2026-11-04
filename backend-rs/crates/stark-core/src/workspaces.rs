//! Generic workspace registry tool (replaces `create_coding_project`).
//!
//! One primitive, no use-case in the name — the registry counterpart to
//! `run_command`:
//!
//! - `create`: new folder under `root` (or register an existing `path`)
//! - `list`: registered folders + virtual General/House/Personal/DFD
//! - `info`: one workspace by id, name, or path
//! - `remove`: unregister only — files are never deleted
//!
//! Starting work stays in `delegate` / `agent spawn` / `create_group` — this
//! tool never starts an agent and never creates a chat.

use std::path::{Path, PathBuf};

use serde_json::Value;
use stark_store::WorkspaceRegistry;
use stark_tools::ToolOutcome;

use crate::RunContext;

const DEFAULT_PROJECTS_ROOT: &str = r"D:\MyProject";
const DFD_CLIENTS_ROOT: &str = r"D:\MyProject\DFDClientProjects";

/// Virtual workspaces hardcoded in the server (`main.rs` VIRTUAL_WORKSPACES).
/// Never stored in `workspaces.json`; listed here so `list`/`info` see them.
const VIRTUAL_WORKSPACES: [(&str, &str, &str, &str); 4] = [
    ("_general", "General", "", "general"),
    ("_house", "House", "_house", "house"),
    ("_personal", "Personal", "_personal", "personal"),
    ("_dfd", "DFD", "_dfd", "dfd"),
];

pub(crate) async fn workspace(ctx: &RunContext, args: &Value) -> ToolOutcome {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match action.as_str() {
        "create" => create(ctx, args).await,
        "list" => list(ctx).await,
        "info" => info(ctx, args).await,
        "remove" => remove(ctx, args).await,
        _ => ToolOutcome::err(
            "Error: workspace requires action (one of: create, list, info, remove).",
        ),
    }
}

async fn create(ctx: &RunContext, args: &Value) -> ToolOutcome {
    // Registry writes are orchestration, not filesystem edits — but only the
    // orchestrator may do them with read-only file access. Spawned children
    // with read access (explore, deep subagents) may only list/inspect.
    if ctx.file_access.is_read_only() && ctx.scope != "orchestrator" {
        return ToolOutcome::err(
            "[BLOCKED] Read-only agent cannot create workspaces. Report findings instead.",
        );
    }
    let registry = WorkspaceRegistry::new(ctx.store.harness_root());

    // Register-an-existing-folder path.
    if let Some(raw) = args.get("path").and_then(Value::as_str).map(str::trim)
        && !raw.is_empty()
    {
        let path = PathBuf::from(raw);
        if !path.is_dir() {
            return ToolOutcome::err(format!(
                "Error: workspace path not found (must be an existing directory): {}",
                path.display()
            ));
        }
        if ctx.scope == "dfd" && !path_is_within(&path, Path::new(DFD_CLIENTS_ROOT)) {
            return ToolOutcome::err(format!(
                "Error: DFD workspaces must live inside {DFD_CLIENTS_ROOT}; got '{}'.",
                path.display()
            ));
        }
        if same_path(&path, ctx.store.harness_root()) {
            return ToolOutcome::err(
                "Error: the harness root is not a project workspace.".to_string(),
            );
        }
        let name = args
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        match registry.add(path.to_string_lossy().as_ref(), &name) {
            Ok(record) => ToolOutcome::ok(format!(
                "[OK] workspace '{}' registered.\nWorkspace: {}\nPath: {}",
                record.name, record.id, record.path
            )),
            Err(error) => ToolOutcome::err(format!("Error: {error}")),
        }
    } else {
        // Create-by-name path.
        let Some(name) = args
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .and_then(sanitize_name)
        else {
            return ToolOutcome::err(
                "Error: action=create needs name (1-64 chars, filesystem-safe) or path (existing directory).",
            );
        };
        if matches!(name.to_lowercase().as_str(), "general" | "_general") {
            return ToolOutcome::err(
                "Error: 'General' is reserved for the general chat workspace.",
            );
        }
        let root = if let Some(raw) = args.get("root").and_then(Value::as_str).map(str::trim)
            && !raw.is_empty()
        {
            PathBuf::from(raw)
        } else {
            projects_root()
        };
        let projects = projects_root();
        if !path_is_within(&root, &projects) {
            return ToolOutcome::err(format!(
                "Error: workspace root must stay inside {}; got '{}'.",
                projects.display(),
                root.display()
            ));
        }
        if ctx.scope == "dfd" && !path_is_within(&root, Path::new(DFD_CLIENTS_ROOT)) {
            return ToolOutcome::err(format!(
                "Error: DFD workspaces must live inside {DFD_CLIENTS_ROOT}; got '{}'.",
                root.display()
            ));
        }
        let path = root.join(&name);
        if path.exists() && !path.is_dir() {
            return ToolOutcome::err(format!(
                "Error: Cannot create workspace: '{}' exists and is not a directory.",
                path.display()
            ));
        }
        if let Err(error) = tokio::fs::create_dir_all(&path).await {
            return ToolOutcome::err(format!(
                "Error: Failed to create workspace folder '{}': {error}",
                path.display()
            ));
        }
        match registry.add(path.to_string_lossy().as_ref(), &name) {
            Ok(record) => ToolOutcome::ok(format!(
                "[OK] workspace '{}' created.\nWorkspace: {}\nPath: {}",
                record.name, record.id, record.path
            )),
            Err(error) => ToolOutcome::err(format!("Error: {error}")),
        }
    }
}

async fn list(ctx: &RunContext) -> ToolOutcome {
    let registry = WorkspaceRegistry::new(ctx.store.harness_root());
    let mut lines = Vec::new();
    for (id, name, _path, kind) in VIRTUAL_WORKSPACES {
        let (chats, _) = ctx.store.chat_stats(id).await;
        lines.push(format!(
            "- {id} | {name} | (virtual, kind={kind}) | chats:{chats}"
        ));
    }
    for record in registry.list() {
        let (chats, _) = ctx.store.chat_stats(&record.id).await;
        let exists = if Path::new(&record.path).is_dir() {
            "exists"
        } else {
            "missing"
        };
        lines.push(format!(
            "- {} | {} | {} | chats:{chats} | {exists}",
            record.id, record.name, record.path
        ));
    }
    ToolOutcome::ok(format!(
        "[OK] workspaces ({}):\n{}",
        lines.len(),
        lines.join("\n")
    ))
}

async fn info(ctx: &RunContext, args: &Value) -> ToolOutcome {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .map(str::to_string)
        .or_else(|| {
            args.get("workspace_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|query| !query.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_default();
    if query.is_empty() {
        return ToolOutcome::err(
            "Error: action=info needs query (id, name, or path) or workspace_id.",
        );
    }
    // Virtual workspaces first.
    for (id, name, path, kind) in VIRTUAL_WORKSPACES {
        if query.eq_ignore_ascii_case(id)
            || query.eq_ignore_ascii_case(name)
            || (!path.is_empty() && query.eq_ignore_ascii_case(path))
        {
            let (chats, updated) = ctx.store.chat_stats(id).await;
            return ToolOutcome::ok(format!(
                "[OK] workspace '{name}'.\nWorkspace: {id}\nKind: {kind} (virtual)\nChats: {chats}\nUpdated: {updated}"
            ));
        }
    }
    let registry = WorkspaceRegistry::new(ctx.store.harness_root());
    let key = normalize_path(&query);
    let found = registry.list().into_iter().find(|record| {
        record.id.eq_ignore_ascii_case(&query)
            || record.name.eq_ignore_ascii_case(&query)
            || normalize_path(&record.path) == key
    });
    match found {
        Some(record) => {
            let (chats, updated) = ctx.store.chat_stats(&record.id).await;
            let exists = if Path::new(&record.path).is_dir() {
                "exists"
            } else {
                "missing"
            };
            ToolOutcome::ok(format!(
                "[OK] workspace '{}'.\nWorkspace: {}\nPath: {}\nChats: {chats} | {exists}\nUpdated: {updated}",
                record.name, record.id, record.path
            ))
        }
        None => ToolOutcome::err(format!("Error: no workspace matches '{query}'.")),
    }
}

async fn remove(ctx: &RunContext, args: &Value) -> ToolOutcome {
    if ctx.file_access.is_read_only() && ctx.scope != "orchestrator" {
        return ToolOutcome::err(
            "[BLOCKED] Read-only agent cannot remove workspaces. Report findings instead.",
        );
    }
    let id = args
        .get("workspace_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .or_else(|| {
            args.get("query")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|query| !query.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_default();
    if id.is_empty() {
        return ToolOutcome::err("Error: action=remove needs workspace_id.");
    }
    if VIRTUAL_WORKSPACES
        .iter()
        .any(|(candidate, _, _, _)| candidate.eq_ignore_ascii_case(&id))
    {
        return ToolOutcome::err(format!(
            "Error: '{id}' is a virtual workspace and cannot be removed."
        ));
    }
    let registry = WorkspaceRegistry::new(ctx.store.harness_root());
    if registry.remove(&id) {
        ToolOutcome::ok(format!(
            "[OK] workspace '{id}' unregistered (files untouched)."
        ))
    } else {
        // Fall back to resolving by name/path so callers can pass a query.
        let key = normalize_path(&id);
        let found = registry.list().into_iter().find(|record| {
            record.name.eq_ignore_ascii_case(&id) || normalize_path(&record.path) == key
        });
        match found {
            Some(record) if registry.remove(&record.id) => ToolOutcome::ok(format!(
                "[OK] workspace '{}' unregistered (files untouched).",
                record.id
            )),
            _ => ToolOutcome::err(format!("Error: no workspace matches '{id}'.")),
        }
    }
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

fn same_path(left: &Path, right: &Path) -> bool {
    let normalize = |path: &Path| {
        std::fs::canonicalize(path)
            .map(|path| path.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_else(|_| path.to_string_lossy().to_ascii_lowercase())
    };
    normalize(left) == normalize(right)
}

fn normalize_path(path: &str) -> String {
    let path = path.trim();
    let absolute = std::path::absolute(Path::new(path)).unwrap_or_else(|_| PathBuf::from(path));
    absolute
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .replace('/', "\\")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use serde_json::json;
    use stark_providers::{Provider, ProviderConfig};
    use stark_store::ChatStore;
    use stark_tools::QuestionBroker;
    use tokio::sync::broadcast;
    use tokio_util::sync::CancellationToken;

    use super::*;

    /// RAII guard that sets an env var and restores the previous value on
    /// drop, so the confinement check can point at the temp dir.
    struct EnvGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &std::path::Path) -> Self {
            let previous = std::env::var(key).ok();
            // `set_var` is unsafe in edition 2024; single-threaded test use.
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.previous {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    async fn test_context(
        dir: &std::path::Path,
        scope: &str,
        access: crate::FileAccess,
    ) -> RunContext {
        let store = ChatStore::open(dir).await.unwrap();
        let provider = Provider::new(ProviderConfig {
            mode: "deepseek".to_string(),
            base_url: "http://127.0.0.1:1".to_string(),
            api_key: "test".to_string(),
            model: "mock".to_string(),
        })
        .unwrap();
        let (tx, _rx) = broadcast::channel(64);
        RunContext {
            chat_id: "test-chat".to_string(),
            workspace_id: "_general".to_string(),
            workspace: dir.to_path_buf(),
            role: Some("Orchestrator".to_string()),
            scope: scope.to_string(),
            depth: 0,
            file_access: access,
            model: "mock".to_string(),
            temperature: 0.0,
            images: Vec::new(),
            provider,
            store,
            questions: Arc::new(QuestionBroker::new()),
            tx,
            latest: Arc::new(Mutex::new(None)),
            steering: Arc::new(Mutex::new(VecDeque::new())),
            follow_up: Arc::new(Mutex::new(VecDeque::new())),
            checkpoint: None,
            cancel: CancellationToken::new(),
            config: None,
            on_job_done: None,
        }
    }

    #[tokio::test]
    async fn create_list_info_remove_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("projects");
        // Point the projects root at the temp dir so the confinement check
        // passes without touching the real D:\MyProject.
        let _guard = EnvGuard::set("STARK_PROJECTS_ROOT", dir.path());
        let ctx = test_context(dir.path(), "orchestrator", crate::FileAccess::Read).await;

        // create by name under an explicit root (orchestrator may write the
        // registry with read-only file access).
        let created = workspace(
            &ctx,
            &json!({"action": "create", "name": "Coffee Shop", "root": root.to_string_lossy()}),
        )
        .await;
        assert!(!created.is_error, "{}", created.content);
        assert!(created.content.contains("coffee-shop"));
        assert!(root.join("coffee-shop").is_dir());

        let listed = workspace(&ctx, &json!({"action": "list"})).await;
        assert!(!listed.is_error, "{}", listed.content);
        assert!(listed.content.contains("coffee-shop"));
        assert!(listed.content.contains("_general"));

        let info = workspace(&ctx, &json!({"action": "info", "query": "coffee-shop"})).await;
        assert!(!info.is_error, "{}", info.content);
        assert!(info.content.contains("coffee-shop"));

        let id = WorkspaceRegistry::new(dir.path())
            .list()
            .into_iter()
            .find(|record| record.name == "coffee-shop")
            .expect("registered")
            .id;
        let removed = workspace(&ctx, &json!({"action": "remove", "workspace_id": id})).await;
        assert!(!removed.is_error, "{}", removed.content);
        // Files are untouched by unregister.
        assert!(root.join("coffee-shop").is_dir());
        assert!(WorkspaceRegistry::new(dir.path()).list().is_empty());
    }

    #[tokio::test]
    async fn read_only_non_orchestrator_cannot_create_or_remove() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_context(dir.path(), "explore", crate::FileAccess::Read).await;
        let denied = workspace(
            &ctx,
            &json!({"action": "create", "name": "nope", "root": dir.path().join("p").to_string_lossy()}),
        )
        .await;
        assert!(denied.is_error);
        assert!(denied.content.contains("[BLOCKED]"));

        let denied_remove = workspace(
            &ctx,
            &json!({"action": "remove", "workspace_id": "ws-12345678"}),
        )
        .await;
        assert!(denied_remove.is_error);

        // ...but listing and info stay available.
        let listed = workspace(&ctx, &json!({"action": "list"})).await;
        assert!(!listed.is_error, "{}", listed.content);
    }

    #[tokio::test]
    async fn virtual_workspaces_cannot_be_removed() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_context(dir.path(), "orchestrator", crate::FileAccess::Read).await;
        let denied = workspace(
            &ctx,
            &json!({"action": "remove", "workspace_id": "_general"}),
        )
        .await;
        assert!(denied.is_error);
        assert!(denied.content.contains("virtual"));
    }

    #[tokio::test]
    async fn dfd_create_is_confined_to_client_root() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_context(dir.path(), "dfd", crate::FileAccess::Write).await;
        // Registering an existing folder outside the DFD client root is refused.
        let outside = dir.path().join("elsewhere");
        std::fs::create_dir_all(&outside).unwrap();
        let denied = workspace(
            &ctx,
            &json!({"action": "create", "path": outside.to_string_lossy()}),
        )
        .await;
        assert!(denied.is_error, "{}", denied.content);
        assert!(denied.content.contains("DFD"));
    }
}
