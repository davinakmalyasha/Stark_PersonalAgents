use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_BASELINE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_BASELINE_FILES: usize = 20_000;
const MAX_LIST: usize = 100;
const SKIP_DIRS: [&str; 11] = [
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    "venv",
    "venv312",
    ".venv",
    "__pycache__",
    ".deep_agents",
    "target",
    "dist",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct FileEntry {
    #[serde(default)]
    snapshot_id: Option<String>,
    #[serde(default)]
    existed: bool,
    #[serde(default)]
    created: bool,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    after_sha: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Manifest {
    run_id: String,
    chat_id: String,
    workspace_path: String,
    #[serde(default)]
    user_message_id: String,
    created_at: i64,
    #[serde(default)]
    files: BTreeMap<String, FileEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SnapshotGroup {
    pub run_id: String,
    pub created_at: i64,
    pub files: Vec<serde_json::Value>,
    pub total_size: u64,
}

#[derive(Debug, Clone, Default)]
pub struct RestoreReport {
    pub restored: Vec<String>,
    pub removed: Vec<String>,
    pub conflicts: Vec<String>,
    pub runs: Vec<String>,
}

impl RestoreReport {
    pub fn ok(&self) -> bool {
        self.conflicts.is_empty()
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "ok": self.ok(),
            "restored": self.restored,
            "removed": self.removed,
            "conflicts": self.conflicts,
            "runs": self.runs,
        })
    }
}

pub struct Checkpoint {
    workspace: PathBuf,
    run_id: String,
    dir: PathBuf,
    manifest: Mutex<Manifest>,
}

impl Checkpoint {
    pub fn begin(chat_id: impl Into<String>, workspace: impl Into<PathBuf>) -> Arc<Self> {
        let chat_id = chat_id.into();
        let workspace = workspace.into();
        let run_id = format!(
            "run-{}-{}",
            chrono::Utc::now().timestamp_millis(),
            &Uuid::new_v4().simple().to_string()[..8]
        );
        let dir = workspace
            .join(".deep_agents")
            .join("snapshots")
            .join(&chat_id)
            .join(&run_id);
        let manifest = Manifest {
            run_id: run_id.clone(),
            chat_id: chat_id.clone(),
            workspace_path: workspace.to_string_lossy().to_string(),
            user_message_id: String::new(),
            created_at: chrono::Utc::now().timestamp_millis(),
            files: BTreeMap::new(),
        };
        let checkpoint = Arc::new(Self {
            workspace,
            run_id,
            dir,
            manifest: Mutex::new(manifest),
        });
        checkpoint.save();
        checkpoint
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn set_user_message(&self, message_id: &str) {
        if let Ok(mut manifest) = self.manifest.lock() {
            manifest.user_message_id = message_id.to_string();
        }
        self.save();
    }

    pub fn snapshot_file(&self, path: &Path) {
        let Ok(root) = std::path::absolute(&self.workspace) else {
            return;
        };
        let Ok(absolute) = std::path::absolute(path) else {
            return;
        };
        let Ok(relative) = absolute.strip_prefix(&root) else {
            return;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        if relative.is_empty() {
            return;
        }

        let mut manifest = match self.manifest.lock() {
            Ok(manifest) => manifest,
            Err(_) => return,
        };
        if manifest.files.contains_key(&relative) {
            return;
        }

        if !absolute.is_file() {
            manifest.files.insert(
                relative,
                FileEntry {
                    created: true,
                    existed: false,
                    ..FileEntry::default()
                },
            );
            drop(manifest);
            self.save();
            return;
        }

        let Ok(metadata) = absolute.metadata() else {
            return;
        };
        if metadata.len() > MAX_FILE_BYTES {
            return;
        }
        let Ok(bytes) = std::fs::read(&absolute) else {
            return;
        };
        let snapshot_id = format!("snap-{}", &Uuid::new_v4().simple().to_string()[..12]);
        let safe_name = relative.replace('/', "__").replace(':', "_");
        if std::fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        if std::fs::write(self.dir.join(format!("{safe_name}.snap")), &bytes).is_err() {
            return;
        }
        manifest.files.insert(
            relative,
            FileEntry {
                snapshot_id: Some(snapshot_id),
                existed: true,
                created: false,
                size: bytes.len() as u64,
                after_sha: None,
            },
        );
        drop(manifest);
        self.save();
    }

    pub fn snapshot_all(&self) {
        let Ok(root) = std::path::absolute(&self.workspace) else {
            return;
        };
        let mut files = Vec::new();
        collect_files(&root, &root, 0, &mut files);
        let mut count = 0usize;
        let mut total = 0u64;
        for (_, path) in files {
            if count >= MAX_BASELINE_FILES || total >= MAX_BASELINE_BYTES {
                break;
            }
            let Ok(metadata) = path.metadata() else {
                continue;
            };
            if metadata.len() > MAX_FILE_BYTES {
                continue;
            }
            total = total.saturating_add(metadata.len());
            count += 1;
            self.snapshot_file(&path);
        }
    }

    pub fn snapshot_patch(&self, patch: &str) {
        for line in patch.lines() {
            let path = line
                .strip_prefix("*** Update File: ")
                .or_else(|| line.strip_prefix("*** Add File: "))
                .or_else(|| line.strip_prefix("*** Delete File: "));
            let Some(path) = path else {
                continue;
            };
            let path = path.trim();
            if path.is_empty() {
                continue;
            }
            self.snapshot_file(&self.workspace.join(path));
        }
    }

    pub fn finalize(&self) {
        let Ok(mut manifest) = self.manifest.lock() else {
            return;
        };
        let mut unchanged = Vec::new();
        for (relative, entry) in &mut manifest.files {
            let target = self.workspace.join(relative);
            entry.after_sha = if target.is_file() {
                std::fs::read(&target).ok().map(|bytes| sha(&bytes))
            } else {
                None
            };
            if entry.created {
                continue;
            }
            if let Some(after) = entry.after_sha.as_deref()
                && let Some(snapshot_id) = entry.snapshot_id.as_deref()
            {
                let safe_name = relative.replace('/', "__").replace(':', "_");
                let snapshot = self.dir.join(format!("{safe_name}.snap"));
                if std::fs::read(&snapshot)
                    .ok()
                    .map(|bytes| sha(&bytes))
                    .as_deref()
                    == Some(after)
                {
                    unchanged.push((relative.clone(), safe_name, snapshot_id.to_string()));
                }
            }
        }
        for (relative, safe_name, _) in unchanged {
            manifest.files.remove(&relative);
            let _ = std::fs::remove_file(self.dir.join(format!("{safe_name}.snap")));
        }
        drop(manifest);
        self.save();
    }

    fn save(&self) {
        let Ok(manifest) = self.manifest.lock() else {
            return;
        };
        if std::fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        if let Ok(text) = serde_json::to_string_pretty(&*manifest) {
            let _ = std::fs::write(self.dir.join("manifest.json"), text);
        }
    }

    fn runs_dir(workspace: &Path, chat_id: &str) -> PathBuf {
        workspace
            .join(".deep_agents")
            .join("snapshots")
            .join(chat_id)
    }

    fn load(workspace: &Path, chat_id: &str, run_id: &str) -> Option<Manifest> {
        let path = Self::runs_dir(workspace, chat_id)
            .join(run_id)
            .join("manifest.json");
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn load_all(workspace: &Path, chat_id: &str) -> Vec<Manifest> {
        let dir = Self::runs_dir(workspace, chat_id);
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut manifests = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path().join("manifest.json");
            if let Ok(text) = std::fs::read_to_string(path)
                && let Ok(manifest) = serde_json::from_str::<Manifest>(&text)
            {
                manifests.push(manifest);
            }
        }
        manifests.sort_by_key(|manifest| std::cmp::Reverse(manifest.created_at));
        manifests
    }

    pub fn list(workspace: &Path, chat_id: &str, limit: usize) -> Vec<SnapshotGroup> {
        let mut groups = Vec::new();
        for manifest in Self::load_all(workspace, chat_id)
            .into_iter()
            .take(limit.min(MAX_LIST))
        {
            let mut files = Vec::new();
            let mut total_size = 0;
            for (path, entry) in manifest.files {
                if entry.snapshot_id.is_none() {
                    continue;
                }
                total_size += entry.size;
                files.push(serde_json::json!({"path": path, "size": entry.size}));
            }
            if !files.is_empty() {
                groups.push(SnapshotGroup {
                    run_id: manifest.run_id,
                    created_at: manifest.created_at,
                    files,
                    total_size,
                });
            }
        }
        groups
    }

    pub fn restore_snapshot(workspace: &Path, chat_id: &str, snapshot_id: &str) -> (bool, String) {
        for manifest in Self::load_all(workspace, chat_id) {
            for (relative, entry) in manifest.files {
                if entry.snapshot_id.as_deref() != Some(snapshot_id) {
                    continue;
                }
                return Self::restore_entry(
                    workspace,
                    chat_id,
                    &manifest.run_id,
                    &relative,
                    &entry,
                );
            }
        }
        (false, format!("snapshot {snapshot_id} not found"))
    }

    pub fn restore_run_any(workspace: &Path, run_id: &str) -> Option<(bool, Vec<String>)> {
        if run_id.is_empty()
            || run_id.chars().any(|character| {
                !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
            })
        {
            return None;
        }
        let dir = workspace.join(".deep_agents").join("snapshots");
        let entries = std::fs::read_dir(dir).ok()?;
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let chat_id = entry.file_name().to_string_lossy().to_string();
            if entry.path().join(run_id).join("manifest.json").is_file() {
                return Some(Self::restore_run(workspace, &chat_id, run_id));
            }
        }
        None
    }

    pub fn restore_run(workspace: &Path, chat_id: &str, run_id: &str) -> (bool, Vec<String>) {
        let Some(manifest) = Self::load(workspace, chat_id, run_id) else {
            return (false, vec!["run not found".to_string()]);
        };
        let mut messages = Vec::new();
        let mut ok = true;
        for (relative, entry) in manifest.files {
            if entry.snapshot_id.is_some() || entry.created {
                let (success, message) =
                    Self::restore_entry(workspace, chat_id, &manifest.run_id, &relative, &entry);
                messages.push(message);
                ok &= success;
            }
        }
        (ok, messages)
    }

    pub fn restore_for_messages(
        workspace: &Path,
        chat_id: &str,
        message_ids: &[String],
        force: bool,
    ) -> RestoreReport {
        let wanted: std::collections::HashSet<&str> =
            message_ids.iter().map(String::as_str).collect();
        let mut report = RestoreReport::default();
        for manifest in Self::load_all(workspace, chat_id) {
            if !wanted.contains(manifest.user_message_id.as_str()) {
                continue;
            }
            report.runs.push(manifest.run_id.clone());
            for (relative, entry) in manifest.files {
                let Some(target) = safe_target(workspace, &relative) else {
                    report
                        .conflicts
                        .push(format!("{relative}: outside workspace"));
                    continue;
                };
                let current_sha = target
                    .is_file()
                    .then(|| std::fs::read(&target).ok().map(|bytes| sha(&bytes)))
                    .flatten();
                if !force
                    && let Some(expected) = entry.after_sha.as_deref()
                    && current_sha.as_deref() != Some(expected)
                {
                    report
                        .conflicts
                        .push(format!("{relative}: changed since the run"));
                    continue;
                }
                if entry.created {
                    if target.is_file() && std::fs::remove_file(&target).is_ok() {
                        report.removed.push(relative);
                    }
                    continue;
                }
                if entry.snapshot_id.is_some() {
                    let (ok, _) = Self::restore_entry(
                        workspace,
                        chat_id,
                        &manifest.run_id,
                        &relative,
                        &entry,
                    );
                    if ok {
                        report.restored.push(relative);
                    } else {
                        report.conflicts.push(format!("{relative}: restore failed"));
                    }
                } else {
                    report
                        .conflicts
                        .push(format!("{relative}: no snapshot (shell/untracked change)"));
                }
            }
        }
        report
    }

    fn restore_entry(
        workspace: &Path,
        chat_id: &str,
        run_id: &str,
        relative: &str,
        entry: &FileEntry,
    ) -> (bool, String) {
        if entry.snapshot_id.is_none() {
            return (false, format!("{relative}: no snapshot"));
        }
        let Some(target) = safe_target(workspace, relative) else {
            return (false, format!("{relative}: outside workspace"));
        };
        let safe_name = relative.replace('/', "__").replace(':', "_");
        let snapshot = Self::runs_dir(workspace, chat_id)
            .join(run_id)
            .join(format!("{safe_name}.snap"));
        let Ok(bytes) = std::fs::read(&snapshot) else {
            return (false, format!("{relative}: snapshot file missing"));
        };
        if let Some(parent) = target.parent()
            && std::fs::create_dir_all(parent).is_err()
        {
            return (false, format!("{relative}: could not create directory"));
        }
        match std::fs::write(&target, bytes) {
            Ok(()) => (true, format!("restored {relative}")),
            Err(error) => (false, format!("{relative}: {error}")),
        }
    }
}

fn collect_files(root: &Path, dir: &Path, depth: usize, out: &mut Vec<(String, PathBuf)>) {
    if depth > 64 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            collect_files(root, &path, depth + 1, out);
        } else if metadata.is_file()
            && let Ok(relative) = path.strip_prefix(root)
        {
            out.push((relative.to_string_lossy().replace('\\', "/"), path));
        }
    }
}

fn sha(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn safe_target(workspace: &Path, relative: &str) -> Option<PathBuf> {
    let root = std::path::absolute(workspace).ok()?;
    let target = std::path::absolute(root.join(relative)).ok()?;
    target.starts_with(&root).then_some(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_and_restores_file_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "before").unwrap();
        let checkpoint = Checkpoint::begin("chat", dir.path());
        checkpoint.set_user_message("u1");
        checkpoint.snapshot_file(&file);
        std::fs::write(&file, "after").unwrap();
        checkpoint.finalize();

        let groups = Checkpoint::list(dir.path(), "chat", 100);
        assert_eq!(groups.len(), 1);
        let snapshot_id = {
            let manifest = Checkpoint::load(dir.path(), "chat", checkpoint.run_id()).unwrap();
            manifest.files["a.txt"].snapshot_id.clone().unwrap()
        };
        let (ok, _) = Checkpoint::restore_snapshot(dir.path(), "chat", &snapshot_id);
        assert!(ok);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "before");
    }

    #[test]
    fn snapshot_all_captures_shell_changes_and_prunes_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        let changed = dir.path().join("src").join("changed.txt");
        let deleted = dir.path().join("deleted.txt");
        std::fs::write(&changed, "before").unwrap();
        std::fs::write(&deleted, "gone soon").unwrap();
        std::fs::write(dir.path().join("same.txt"), "same").unwrap();

        let checkpoint = Checkpoint::begin("chat", dir.path());
        checkpoint.set_user_message("u4");
        checkpoint.snapshot_all();
        std::fs::write(&changed, "after").unwrap();
        std::fs::remove_file(&deleted).unwrap();
        checkpoint.finalize();

        let groups = Checkpoint::list(dir.path(), "chat", 10);
        assert_eq!(groups.len(), 1);
        let paths: Vec<&str> = groups[0]
            .files
            .iter()
            .map(|file| file["path"].as_str().unwrap())
            .collect();
        assert!(paths.contains(&"src/changed.txt"));
        assert!(paths.contains(&"deleted.txt"));
        assert!(!paths.contains(&"same.txt"));

        let report =
            Checkpoint::restore_for_messages(dir.path(), "chat", &["u4".to_string()], false);
        assert!(report.ok(), "{report:?}");
        assert_eq!(std::fs::read_to_string(&changed).unwrap(), "before");
        assert_eq!(std::fs::read_to_string(&deleted).unwrap(), "gone soon");
    }

    #[test]
    fn restores_a_run_without_chat_id() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "before").unwrap();
        let checkpoint = Checkpoint::begin("chat", dir.path());
        checkpoint.set_user_message("u3");
        checkpoint.snapshot_file(&file);
        std::fs::write(&file, "after").unwrap();
        checkpoint.finalize();

        let (ok, messages) = Checkpoint::restore_run_any(dir.path(), checkpoint.run_id()).unwrap();
        assert!(ok, "{messages:?}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "before");
    }

    #[test]
    fn removes_created_files_on_checkpoint_restore() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("new.txt");
        let checkpoint = Checkpoint::begin("chat", dir.path());
        checkpoint.set_user_message("u2");
        checkpoint.snapshot_file(&file);
        std::fs::write(&file, "created").unwrap();
        checkpoint.finalize();

        let report =
            Checkpoint::restore_for_messages(dir.path(), "chat", &["u2".to_string()], false);
        assert!(report.ok(), "{:?}", report);
        assert_eq!(report.removed, vec!["new.txt"]);
        assert!(!file.exists());
    }
}
