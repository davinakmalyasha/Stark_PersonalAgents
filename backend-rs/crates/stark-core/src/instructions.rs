//! Codex-style project instructions.
//!
//! Loads `AGENTS.override.md` or `AGENTS.md` from the global harness home and
//! from the project root down to the workspace. The files are concatenated in
//! order, bounded, and injected as a separate instruction block.

use std::path::{Path, PathBuf};

const DEFAULT_FILE: &str = "AGENTS.md";
const OVERRIDE_FILE: &str = "AGENTS.override.md";
const MAX_BYTES: usize = 64 * 1024;

fn read_doc(dir: &Path) -> Option<(PathBuf, String)> {
    for name in [OVERRIDE_FILE, DEFAULT_FILE] {
        let path = dir.join(name);
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let Ok(data) = std::fs::read(&path) else {
            continue;
        };
        let text = String::from_utf8_lossy(&data).trim().to_string();
        if !text.is_empty() {
            return Some((path, text));
        }
    }
    None
}

fn project_root(workspace: &Path) -> PathBuf {
    let mut current = workspace.to_path_buf();
    loop {
        if current.join(".git").exists() {
            return current;
        }
        let Some(parent) = current.parent() else {
            return workspace.to_path_buf();
        };
        if parent == current {
            return workspace.to_path_buf();
        }
        current = parent.to_path_buf();
    }
}

fn chain(root: &Path, workspace: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut current = workspace.to_path_buf();
    loop {
        dirs.push(current.clone());
        if current == root {
            break;
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if parent == current {
            break;
        }
        current = parent.to_path_buf();
    }
    dirs.reverse();
    dirs
}

/// Returns the combined instruction text for a workspace, bounded to 64KB.
pub fn load(harness_root: &Path, workspace: &Path) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    let global = harness_root.join(".deep_agents");
    if let Some((_, text)) = read_doc(&global) {
        parts.push(text);
    }
    let root = project_root(workspace);
    for dir in chain(&root, workspace) {
        if let Some((path, text)) = read_doc(&dir) {
            parts.push(format!("## {}\n{}", path.to_string_lossy(), text));
        }
    }
    if parts.is_empty() {
        return None;
    }
    let mut out = String::from("[PROJECT INSTRUCTIONS]\n");
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            out.push_str("\n\n--- project-doc ---\n\n");
        }
        out.push_str(part);
        if out.len() >= MAX_BYTES {
            break;
        }
    }
    Some(out.chars().take(MAX_BYTES).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_root_override_and_nested_files_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("src/nested")).unwrap();
        std::fs::write(root.join("AGENTS.md"), "root rules").unwrap();
        std::fs::write(root.join("src/AGENTS.override.md"), "src override").unwrap();
        std::fs::write(root.join("src/nested/AGENTS.md"), "nested rules").unwrap();
        let text = load(root, &root.join("src/nested")).unwrap();
        assert!(text.contains("root rules"));
        assert!(text.contains("src override"));
        assert!(text.contains("nested rules"));
        let root_pos = text.find("root rules").unwrap();
        let src_pos = text.find("src override").unwrap();
        let nested_pos = text.find("nested rules").unwrap();
        assert!(root_pos < src_pos && src_pos < nested_pos);
    }

    #[test]
    fn global_instructions_are_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let harness = dir.path().join("harness");
        let deep = harness.join(".deep_agents");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("AGENTS.md"), "global rules").unwrap();
        let workspace = dir.path().join("project");
        std::fs::create_dir_all(&workspace).unwrap();
        let text = load(&harness, &workspace).unwrap();
        assert!(text.contains("global rules"));
    }

    #[test]
    fn missing_files_return_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path(), &dir.path().join("missing")).is_none());
    }
}
