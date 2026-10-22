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

    let workspace_normalized = normalize(workspace);
    if !path_starts_with(&normalized, &workspace_normalized) {
        return Err(format!("path escapes the workspace: {raw}"));
    }
    Ok(normalized)
}

pub fn relative_display(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
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
