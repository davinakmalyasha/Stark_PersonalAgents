use std::path::{Component, Path, PathBuf};

pub fn resolve_workspace_path(workspace: &Path, raw: &str) -> Result<PathBuf, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("path is empty".to_string());
    }

    let candidate = Path::new(raw);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        workspace.join(candidate)
    };

    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(format!("path escapes the workspace: {raw}"));
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }

    let workspace_normalized = workspace
        .canonicalize()
        .unwrap_or_else(|_| normalize(workspace));
    // Resolve symlinks when the target exists; for new files resolve the
    // nearest existing parent so a symlinked directory cannot escape.
    let resolved = canonicalize_best_effort(&normalized);
    if !path_starts_with(&resolved, &workspace_normalized) {
        return Err(format!("path escapes the workspace: {raw}"));
    }
    // Keep the lexical path for callers (checkpoints and relative displays
    // expect the original drive form); the canonical path above is the
    // symlink-escape check.
    Ok(normalized)
}

fn canonicalize_best_effort(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }
    let mut suffix = Vec::new();
    let mut current = path.to_path_buf();
    loop {
        if let Some(name) = current.file_name() {
            suffix.push(name.to_os_string());
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if let Ok(mut canonical) = parent.canonicalize() {
            for part in suffix.iter().rev() {
                canonical.push(part);
            }
            return canonical;
        }
        current = parent.to_path_buf();
    }
    normalize(path)
}

pub fn relative_display(workspace: &Path, path: &Path) -> String {
    if let Ok(relative) = path.strip_prefix(workspace) {
        return relative.to_string_lossy().replace('\\', "/");
    }
    if let Ok(canonical_workspace) = workspace.canonicalize()
        && let Ok(relative) = path.strip_prefix(&canonical_workspace)
    {
        return relative.to_string_lossy().replace('\\', "/");
    }
    path.to_string_lossy().replace('\\', "/")
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn path_starts_with(path: &Path, base: &Path) -> bool {
    if cfg!(windows) {
        let path = path.to_string_lossy().to_lowercase();
        let base = base.to_string_lossy().to_lowercase();
        path == base
            || path.starts_with(&format!("{base}\\"))
            || path.starts_with(&format!("{base}/"))
    } else {
        path.starts_with(base)
    }
}
