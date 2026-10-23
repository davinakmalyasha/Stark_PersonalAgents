use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceRecord {
    pub id: String,
    pub name: String,
    pub path: String,
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
        std::fs::write(&path, text)
            .map_err(|error| format!("failed to write workspace registry: {error}"))
    }

    pub fn list(&self) -> Vec<WorkspaceRecord> {
        self.load().workspaces
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
            return Ok(existing.clone());
        }

        let name = if name.trim().is_empty() {
            absolute
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
                .unwrap_or_else(|| "Workspace".to_string())
        } else {
            name.trim().to_string()
        };
        let record = WorkspaceRecord {
            id: format!("ws-{}", &Uuid::new_v4().simple().to_string()[..8]),
            name,
            path,
            added_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        };
        registry.workspaces.push(record.clone());
        self.save(&registry)?;
        Ok(record)
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
}
