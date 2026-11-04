//! Workspace state block injected into the system prompt.
//!
//! A run is bound to exactly one workspace. That binding is described by
//! identity and a folder relative to the projects root, never by a
//! machine-specific absolute path. Keeping the block compact and stable lets
//! request-prefix caching survive plan switches, and keeps transcripts
//! portable without a path-rewriting pass on import.

use std::path::Path;

use stark_store::WorkspaceRegistry;

/// Virtual workspaces exist without a registry record.
const VIRTUAL: [(&str, &str); 4] = [
    ("_general", "General"),
    ("_house", "House"),
    ("_personal", "Personal"),
    ("_dfd", "DFD"),
];

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn virtual_name(workspace_id: &str) -> Option<&'static str> {
    VIRTUAL
        .iter()
        .find(|(id, _)| id.eq_ignore_ascii_case(workspace_id))
        .map(|(_, name)| *name)
}

/// True when two paths point at the same folder. Worktree runs deliberately
/// differ from the registered project folder.
fn same_path(left: &Path, right: &str) -> bool {
    let left = std::path::absolute(left).unwrap_or_else(|_| left.to_path_buf());
    let right =
        std::path::absolute(Path::new(right)).unwrap_or_else(|_| std::path::PathBuf::from(right));
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

fn folder_for(record: Option<&stark_store::WorkspaceRecord>, workspace: &Path) -> String {
    if let Some(rel) = record
        .and_then(|record| record.rel.as_deref())
        .map(str::trim)
        .filter(|rel| !rel.is_empty())
    {
        return rel.trim_matches('/').to_string();
    }
    workspace
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string())
}

/// Renders the `<workspaces>` block for the run.
///
/// Only the primary workspace is included. The agent can call
/// `workspace(action="list")` when it needs the registry; duplicating every
/// project in the prompt would grow the cached prefix and leak machine layout.
pub fn render(harness_root: &Path, workspace_id: &str, workspace: &Path) -> String {
    let registry = WorkspaceRegistry::new(harness_root);
    let record = registry.get(workspace_id);
    let name = record
        .as_ref()
        .map(|record| record.name.clone())
        .or_else(|| virtual_name(workspace_id).map(str::to_string))
        .or_else(|| {
            workspace
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "workspace".to_string());
    let worktree = record
        .as_ref()
        .is_some_and(|record| !record.path.is_empty() && !same_path(workspace, &record.path));
    let status = if worktree {
        "worktree"
    } else if record.is_none() && virtual_name(workspace_id).is_some() {
        "virtual"
    } else if workspace.is_dir() {
        "ready"
    } else {
        "missing"
    };
    let folder = if worktree || (record.is_none() && virtual_name(workspace_id).is_some()) {
        ".".to_string()
    } else {
        folder_for(record.as_ref(), workspace)
    };
    let repo = record
        .as_ref()
        .and_then(|record| record.repo.as_deref())
        .map(str::trim)
        .filter(|repo| !repo.is_empty());

    let mut rendered = String::from("<workspaces>\n");
    rendered.push_str(&format!(
        "  <workspace id=\"{}\" name=\"{}\" primary=\"true\" status=\"{}\">\n",
        escape(workspace_id),
        escape(&name),
        status
    ));
    rendered.push_str(&format!("    <folder>{}</folder>\n", escape(&folder)));
    if let Some(repo) = repo {
        rendered.push_str(&format!("    <repo>{}</repo>\n", escape(repo)));
    }
    rendered.push_str("  </workspace>\n</workspaces>");
    rendered
}

/// One-sentence contract that accompanies the block.
pub const USAGE_HINT: &str = "The primary workspace folder is the working directory already. Use paths relative to it in commands and replies; use an absolute path only when the user asks for one.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_registry_identity_without_an_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let projects = dir.path().join("projects");
        let folder = projects.join("coffee-shop");
        std::fs::create_dir_all(&folder).unwrap();
        let registry = WorkspaceRegistry::new(dir.path());
        let record = registry
            .add_with_repo(
                folder.to_string_lossy().as_ref(),
                "coffee-shop",
                "owner/coffee-shop",
                Some("main"),
            )
            .unwrap();
        let rendered = render(dir.path(), &record.id, &folder);

        assert!(rendered.contains(&format!("id=\"{}\"", record.id)));
        assert!(rendered.contains("name=\"coffee-shop\""));
        assert!(rendered.contains("<folder>coffee-shop</folder>"));
        assert!(rendered.contains("<repo>owner/coffee-shop</repo>"));
        assert!(rendered.contains("status=\"ready\""));
        assert!(!rendered.contains(&dir.path().to_string_lossy().to_string()));
    }

    #[test]
    fn virtual_workspace_uses_its_display_name() {
        let dir = tempfile::tempdir().unwrap();
        let rendered = render(dir.path(), "_general", dir.path());
        assert!(rendered.contains("name=\"General\""));
        assert!(rendered.contains("status=\"virtual\""));
        assert!(rendered.contains("<folder>.</folder>"));
    }

    #[test]
    fn worktree_run_is_distinct_from_the_registry_folder() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("project");
        let worktree = dir.path().join("worktree");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        let registry = WorkspaceRegistry::new(dir.path());
        let record = registry
            .add(base.to_string_lossy().as_ref(), "project")
            .unwrap();
        let rendered = render(dir.path(), &record.id, &worktree);
        assert!(rendered.contains("status=\"worktree\""));
        assert!(rendered.contains("<folder>.</folder>"));
        assert!(!rendered.contains(&worktree.to_string_lossy().to_string()));
    }
}
