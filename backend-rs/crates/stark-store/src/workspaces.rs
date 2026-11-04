use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Token stored in `workspaces.json` instead of a machine-specific absolute
/// prefix. The active value comes from `STARK_PROJECTS_ROOT` (or the platform
/// default), so the same registry resolves on a laptop and on a VPS with a
/// completely different folder layout.
pub const PROJECTS_ROOT_TOKEN: &str = "%PROJECTS_ROOT%";

/// Root that project folders live under on this machine.
///
/// `STARK_PROJECTS_ROOT` wins. Platform defaults keep the old Windows layout
/// and give Linux/macOS a sane, writable location instead of a literal
/// `D:\MyProject` directory name.
pub fn projects_root() -> PathBuf {
    if let Ok(raw) = std::env::var("STARK_PROJECTS_ROOT") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    #[cfg(windows)]
    {
        PathBuf::from(r"D:\MyProject")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/srv/stark-projects")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceRecord {
    pub id: String,
    pub name: String,
    /// Absolute path on the machine that wrote the record. Always present so
    /// older readers keep working; recomputed from `root`/`rel` on read.
    #[serde(default)]
    pub path: String,
    /// Portable prefix: currently only `PROJECTS_ROOT`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// Path relative to `root`, using `/` separators for portability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rel: Option<String>,
    #[serde(rename = "addedAt", default)]
    pub added_at: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RegistryFile {
    #[serde(default)]
    workspaces: Vec<WorkspaceRecord>,
}

#[derive(Debug, Clone)]
pub struct WorkspaceRegistry {
    harness_root: PathBuf,
}

impl WorkspaceRegistry {
    pub fn new(harness_root: impl Into<PathBuf>) -> Self {
        Self {
            harness_root: harness_root.into(),
        }
    }

    fn registry_path(&self) -> PathBuf {
        self.harness_root
            .join(".deep_agents")
            .join("workspaces.json")
    }

    fn load(&self) -> RegistryFile {
        let path = self.registry_path();
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn save(&self, registry: &RegistryFile) -> Result<(), String> {
        let path = self.registry_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create registry directory: {error}"))?;
        }
        let text = serde_json::to_string_pretty(registry)
            .map_err(|error| format!("failed to serialize workspace registry: {error}"))?;
        // Atomic: temp + rename with a .bak of the previous registry.
        if path.is_file() {
            let _ = std::fs::copy(&path, path.with_extension("json.bak"));
        }
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, text)
            .map_err(|error| format!("failed to write workspace registry: {error}"))?;
        std::fs::rename(&temp, &path)
            .map_err(|error| format!("failed to write workspace registry: {error}"))
    }

    /// Records with `path` resolved for this machine.
    pub fn list(&self) -> Vec<WorkspaceRecord> {
        let root = projects_root();
        self.load()
            .workspaces
            .into_iter()
            .map(|record| resolve_record(record, &root))
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<WorkspaceRecord> {
        self.list().into_iter().find(|workspace| workspace.id == id)
    }

    pub fn find_by_path(&self, path: &str) -> Option<WorkspaceRecord> {
        let key = normalize_path(path);
        self.list()
            .into_iter()
            .find(|workspace| normalize_path(&workspace.path) == key)
    }

    /// Adds a folder. When it lives under the projects root, the record is
    /// stored portably (`%PROJECTS_ROOT%` + relative path) so importing the
    /// registry on another machine resolves to that machine's root.
    pub fn add(&self, folder_path: &str, name: &str) -> Result<WorkspaceRecord, String> {
        let folder = folder_path.trim();
        if folder.is_empty() {
            return Err("path is required".to_string());
        }
        let absolute = std::path::absolute(Path::new(folder))
            .map_err(|error| format!("invalid path: {error}"))?;
        if !absolute.is_dir() {
            return Err(format!("folder not found: {}", absolute.display()));
        }
        let path = absolute.to_string_lossy().to_string();
        let key = normalize_path(&path);

        let mut registry = self.load();
        if let Some(existing) = registry
            .workspaces
            .iter()
            .find(|workspace| normalize_path(&workspace.path) == key)
        {
            return Ok(resolve_record(existing.clone(), &projects_root()));
        }

        let name = if name.trim().is_empty() {
            absolute
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
                .unwrap_or_else(|| "Workspace".to_string())
        } else {
            name.trim().to_string()
        };
        let (root, rel) = portable_parts(&absolute);
        let record = WorkspaceRecord {
            id: format!("ws-{}", &Uuid::new_v4().simple().to_string()[..8]),
            name,
            path,
            root,
            rel,
            added_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        };
        registry.workspaces.push(record.clone());
        self.save(&registry)?;
        Ok(resolve_record(record, &projects_root()))
    }

    pub fn remove(&self, id: &str) -> bool {
        let mut registry = self.load();
        let before = registry.workspaces.len();
        registry.workspaces.retain(|workspace| workspace.id != id);
        if registry.workspaces.len() == before {
            return false;
        }
        self.save(&registry).is_ok()
    }

    /// The raw, un-resolved records exactly as stored (used by export).
    pub fn stored_records(&self) -> Vec<WorkspaceRecord> {
        self.load().workspaces
    }
}

/// Splits an absolute path into a portable `(root token, relative)` pair when
/// it sits under the current projects root; otherwise returns `(None, None)`
/// and the record remains machine-specific.
fn portable_parts(path: &Path) -> (Option<String>, Option<String>) {
    let root = projects_root();
    let Ok(relative) = path.strip_prefix(&root) else {
        return (None, None);
    };
    let rel = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/");
    if rel.is_empty() {
        return (None, None);
    }
    (Some(PROJECTS_ROOT_TOKEN.to_string()), Some(rel))
}

/// Recomputes `path` from `root`/`rel` for the given projects root. Records
/// without portable fields keep their stored absolute path.
pub fn resolve_record(mut record: WorkspaceRecord, root: &Path) -> WorkspaceRecord {
    if record.root.as_deref() == Some(PROJECTS_ROOT_TOKEN)
        && let Some(rel) = record.rel.as_deref().filter(|rel| !rel.is_empty())
    {
        let joined = rel
            .split('/')
            .fold(root.to_path_buf(), |path, part| path.join(part));
        record.path = joined.to_string_lossy().to_string();
    }
    record
}

/// Rewrites a stored record's absolute `path` so it points at `new_root`
/// when the old path lived under `old_root`. Used by import on boot.
pub fn rebase_record(
    mut record: WorkspaceRecord,
    old_root: &Path,
    new_root: &Path,
) -> WorkspaceRecord {
    let key = normalize_path(&record.path);
    let old = normalize_path(&old_root.to_string_lossy());
    if let Some(rest) = key.strip_prefix(&old) {
        let rest = rest.trim_start_matches('\\').trim_start_matches('/');
        let mut path = new_root.to_path_buf();
        for part in rest.split(['\\', '/']).filter(|part| !part.is_empty()) {
            path.push(part);
        }
        record.path = path.to_string_lossy().to_string();
        if record.root.is_none()
            && let (Some(token), Some(rel)) = portable_parts(&path)
        {
            record.root = Some(token);
            record.rel = Some(rel);
        }
    }
    record
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
    use super::*;

    #[test]
    fn add_find_and_remove_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let registry = WorkspaceRegistry::new(dir.path());

        let record = registry
            .add(project.to_string_lossy().as_ref(), "Project")
            .unwrap();
        assert_eq!(record.name, "Project");
        assert_eq!(registry.list().len(), 1);
        assert_eq!(
            registry
                .find_by_path(project.to_string_lossy().as_ref())
                .unwrap()
                .id,
            record.id
        );

        assert!(registry.remove(&record.id));
        assert!(registry.list().is_empty());
    }

    #[test]
    fn portable_record_resolves_against_new_root() {
        let record = WorkspaceRecord {
            id: "ws-1".to_string(),
            name: "coffee-shop".to_string(),
            path: r"D:\MyProject\coffee-shop".to_string(),
            root: Some(PROJECTS_ROOT_TOKEN.to_string()),
            rel: Some("coffee-shop".to_string()),
            added_at: String::new(),
        };
        let resolved = resolve_record(record, Path::new("/srv/stark-projects"));
        let expected = Path::new("/srv/stark-projects")
            .join("coffee-shop")
            .to_string_lossy()
            .to_string();
        assert_eq!(resolved.path, expected);
    }

    #[test]
    fn rebase_rewrites_prefix_and_adds_portability() {
        let record = WorkspaceRecord {
            id: "ws-1".to_string(),
            name: "app".to_string(),
            path: r"D:\MyProject\app".to_string(),
            root: None,
            rel: None,
            added_at: String::new(),
        };
        let rebased = rebase_record(
            record,
            Path::new(r"D:\MyProject"),
            Path::new("/srv/stark-projects"),
        );
        let expected = Path::new("/srv/stark-projects")
            .join("app")
            .to_string_lossy()
            .to_string();
        assert_eq!(rebased.path, expected);
    }

    #[test]
    fn absolute_paths_outside_the_root_stay_absolute() {
        let record = WorkspaceRecord {
            id: "ws-1".to_string(),
            name: "elsewhere".to_string(),
            path: r"D:\Other\app".to_string(),
            root: None,
            rel: None,
            added_at: String::new(),
        };
        let rebased = rebase_record(
            record,
            Path::new(r"D:\MyProject"),
            Path::new("/srv/stark-projects"),
        );
        assert_eq!(rebased.path, r"D:\Other\app");
    }
}
