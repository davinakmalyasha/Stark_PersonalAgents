use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use serde_json::Value;

use crate::{ToolContext, ToolOutcome};

const MAX_EVENTS: usize = 50;
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".deep_agents",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".turbo",
    ".venv",
    "venv",
    "__pycache__",
];

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    modified: Option<SystemTime>,
    len: u64,
}

type Snapshot = BTreeMap<PathBuf, FileStamp>;

static SNAPSHOTS: OnceLock<Mutex<HashMap<PathBuf, Snapshot>>> = OnceLock::new();

fn snapshots() -> &'static Mutex<HashMap<PathBuf, Snapshot>> {
    SNAPSHOTS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub async fn files_changed(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(5)
        .clamp(1, MAX_EVENTS as u64) as usize;
    let root = ctx.workspace.clone();
    let current = match tokio::task::spawn_blocking(move || snapshot(&root)).await {
        Ok(snapshot) => snapshot,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };
    let mut guard = snapshots().lock().unwrap();
    let previous = guard.insert(ctx.workspace.clone(), current.clone());
    let Some(previous) = previous else {
        return ToolOutcome::ok("[file-watcher] no external changes recorded yet.");
    };

    let mut events = Vec::new();
    for (path, stamp) in &current {
        match previous.get(path) {
            None => events.push((path, "created")),
            Some(old) if old != stamp => events.push((path, "modified")),
            _ => {}
        }
    }
    for path in previous.keys() {
        if !current.contains_key(path) {
            events.push((path, "deleted"));
        }
    }
    if events.is_empty() {
        return ToolOutcome::ok("[file-watcher] no external changes recorded yet.");
    }
    events.sort_by_key(|(path, _)| *path);
    let selected: Vec<_> = events.iter().rev().take(limit).collect();
    let mut lines = vec![format!(
        "[file-watcher] last {} external change(s):",
        selected.len()
    )];
    for (path, change) in selected {
        let display = path.strip_prefix(&ctx.workspace).unwrap_or(path).display();
        lines.push(format!("  {change:>8}  {display}"));
    }
    ToolOutcome::ok(lines.join("\n"))
}

fn snapshot(root: &Path) -> Snapshot {
    let mut snapshot = BTreeMap::new();
    walk(root, root, &mut snapshot);
    snapshot
}

fn walk(root: &Path, directory: &Path, snapshot: &mut Snapshot) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !SKIP_DIRS.contains(&name.as_str()) {
                walk(root, &path, snapshot);
            }
            continue;
        }
        if !metadata.is_file() {
            continue;
        }
        let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        snapshot.insert(
            relative,
            FileStamp {
                modified: metadata.modified().ok(),
                len: metadata.len(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use crate::QuestionBroker;

    use super::*;

    #[tokio::test]
    async fn reports_created_modified_and_deleted_files() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("a.txt"), "one")
            .await
            .unwrap();
        let ctx = ToolContext::new(dir.path(), "changes", Arc::new(QuestionBroker::new()));
        let first = files_changed(&ctx, &json!({"limit": 10})).await;
        assert!(first.content.contains("no external changes"));

        tokio::fs::write(dir.path().join("a.txt"), "two two")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("b.txt"), "new")
            .await
            .unwrap();
        let second = files_changed(&ctx, &json!({"limit": 10})).await;
        assert!(second.content.contains("modified"));
        assert!(second.content.contains("created"));
        assert!(second.content.contains("a.txt"));
        assert!(second.content.contains("b.txt"));

        tokio::fs::remove_file(dir.path().join("b.txt"))
            .await
            .unwrap();
        let third = files_changed(&ctx, &json!({"limit": 10})).await;
        assert!(third.content.contains("deleted"));
        assert!(third.content.contains("b.txt"));
    }
}
