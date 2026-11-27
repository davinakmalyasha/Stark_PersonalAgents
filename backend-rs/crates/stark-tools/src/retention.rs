//! Harness disk retention: bounded cleanup of files the harness generates.
//!
//! Stark writes disposable artifacts in four places:
//!
//! - `scratch/*.log`, `scratch/*.txt`, `scratch/*.png` — full tool output
//!   spilled next to a truncated result, grep/glob overflow, and browser
//!   screenshots.
//! - `.deep_agents/tool-output/tool_*` — Code Mode's OpenCode-shaped
//!   `ToolOutput` spill; cleaned hourly by age, exactly like OpenCode.
//! - `<workspace>/.deep_agents/snapshots/<chat>/<run>/` — pre-edit file bytes
//!   for rewind (`checkpoint::Checkpoint`). Snapshots live in the project
//!   workspace, not in the harness root, so every registered workspace has to
//!   be swept.
//! - `.deep_agents/imports/<id>/` — staging for a portable import. A crash
//!   between extraction and the pending marker leaves an orphan.
//!
//! None of these are user documents. Codex has no equivalent sweep because it
//! never spills tool output to disk, so the policy below is Stark's own:
//! newest-first retention by count, total bytes, and age, with every limit
//! overridable and `0` disabling a dimension.
//!
//! Nothing here is model-visible: no context is injected, no tool output is
//! resized. Only files on disk are touched.
//!
//! Safety rules: only regular `*.log` / `*.txt` / `*.png` files directly inside
//! `scratch/`, only run directories inside `snapshots/<chat>/`, and only
//! staging directories inside `imports/` are ever touched. Anything
//! unrecognized is left alone. Uploads, rollouts, sessions, memory, and the
//! database backups are user data and are never swept.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Retention limits. Defaults are generous enough that a long working session
/// never loses something it might still point at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicy {
    /// Newest scratch files to keep (`0` = no count limit).
    pub scratch_files: usize,
    /// Total scratch bytes to keep, oldest evicted first (`0` = no limit).
    pub scratch_bytes: u64,
    /// Scratch files older than this many days are removed (`0` = no limit).
    pub scratch_age_days: u64,
    /// Newest snapshot run directories to keep per chat (`0` = keep all).
    pub snapshot_runs: usize,
    /// Total snapshot bytes to keep across all workspaces (`0` = no limit).
    pub snapshot_bytes: u64,
    /// Snapshot runs older than this many days are removed (`0` = no limit).
    pub snapshot_age_days: u64,
    /// Import staging directories older than this many days are removed unless
    /// the pending-import marker still references them (`0` = disable).
    pub import_age_days: u64,
}

const MIB: u64 = 1024 * 1024;

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            scratch_files: 200,
            scratch_bytes: 50 * MIB,
            scratch_age_days: 7,
            snapshot_runs: 20,
            snapshot_bytes: 1024 * MIB,
            snapshot_age_days: 14,
            import_age_days: 7,
        }
    }
}

fn env_usize(key: &str, fallback: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(fallback)
}

fn env_u64(key: &str, fallback: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(fallback)
}

impl RetentionPolicy {
    /// Reads `STARK_RETENTION_SCRATCH_FILES`, `_SCRATCH_MB`, `_SCRATCH_DAYS`,
    /// `_SNAPSHOT_RUNS`, `_SNAPSHOT_MB`, `_SNAPSHOT_DAYS`, and `_IMPORT_DAYS`.
    /// Unset values keep the defaults; `0` disables that dimension.
    pub fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            scratch_files: env_usize("STARK_RETENTION_SCRATCH_FILES", defaults.scratch_files),
            scratch_bytes: env_u64("STARK_RETENTION_SCRATCH_MB", defaults.scratch_bytes / MIB)
                * MIB,
            scratch_age_days: env_u64("STARK_RETENTION_SCRATCH_DAYS", defaults.scratch_age_days),
            snapshot_runs: env_usize("STARK_RETENTION_SNAPSHOT_RUNS", defaults.snapshot_runs),
            snapshot_bytes: env_u64("STARK_RETENTION_SNAPSHOT_MB", defaults.snapshot_bytes / MIB)
                * MIB,
            snapshot_age_days: env_u64("STARK_RETENTION_SNAPSHOT_DAYS", defaults.snapshot_age_days),
            import_age_days: env_u64("STARK_RETENTION_IMPORT_DAYS", defaults.import_age_days),
        }
    }
}

/// What one sweep removed, for the startup log line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RetentionReport {
    pub scratch_removed: usize,
    pub scratch_bytes: u64,
    pub snapshot_runs_removed: usize,
    pub snapshot_bytes: u64,
    pub imports_removed: usize,
}

impl RetentionReport {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

fn modified(path: &Path) -> SystemTime {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

fn age_days(modified: SystemTime, now: SystemTime) -> u64 {
    now.duration_since(modified)
        .unwrap_or(Duration::ZERO)
        .as_secs()
        / 86_400
}

fn scratch_dir(root: &Path) -> PathBuf {
    root.join(".deep_agents").join("scratch")
}

/// Scratch files are the harness's own output. The extension allowlist keeps
/// unrelated files a user might drop into `scratch/` untouched.
fn is_scratch_file(path: &Path) -> bool {
    path.is_file()
        && matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("log" | "txt" | "png")
        )
}

/// OpenCode `ToolOutput.DIRECTORY`: `tool-output` beside the harness data.
fn tool_output_dir(root: &Path) -> PathBuf {
    root.join(".deep_agents").join("tool-output")
}

/// OpenCode's cleanup regex is `^tool_[0-9a-f]{12}`, not the full identifier:
/// the 14-character random tail may contain any base62 character.
fn is_tool_output_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(rest) = name.strip_prefix("tool_") else {
        return false;
    };
    rest.len() >= 12
        && rest.as_bytes()[..12]
            .iter()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// OpenCode deletes tool-output files older than seven days, once an hour.
fn sweep_tool_output(root: &Path, now: SystemTime) -> (usize, u64) {
    let directory = tool_output_dir(root);
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return (0, 0);
    };
    let mut removed = 0usize;
    let mut freed = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_tool_output_file(&path) {
            continue;
        }
        let age = now.duration_since(modified(&path)).unwrap_or_default();
        if age <= Duration::from_secs(7 * 24 * 60 * 60) {
            continue;
        }
        let size = entry
            .metadata()
            .ok()
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        if std::fs::remove_file(&path).is_ok() {
            removed += 1;
            freed += size;
        }
    }
    (removed, freed)
}

/// Newest-first ordering that stays deterministic when timestamps tie: path
/// names break ties, so a sweep never depends on `read_dir` order.
fn newest_first<T: Ord>(a: &(SystemTime, T), b: &(SystemTime, T)) -> std::cmp::Ordering {
    b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1))
}

fn tree_bytes(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            total = total.saturating_add(tree_bytes(&entry.path()));
        } else if file_type.is_file() {
            total = total.saturating_add(entry.metadata().map(|meta| meta.len()).unwrap_or(0));
        }
    }
    total
}

/// Spilled tool output: newest-first by mtime, keep while under the count,
/// byte, and age limits. Only `*.log`, `*.txt`, and `*.png` files directly in
/// the directory.
pub fn sweep_scratch(root: &Path, policy: &RetentionPolicy, now: SystemTime) -> (usize, u64) {
    let directory = scratch_dir(root);
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return (0, 0);
    };
    let mut files: Vec<(SystemTime, PathBuf, u64)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !is_scratch_file(&path) {
                return None;
            }
            let size = entry.metadata().ok().map(|meta| meta.len()).unwrap_or(0);
            Some((modified(&path), path, size))
        })
        .collect();
    files.sort_by(|a, b| newest_first(&(a.0, a.1.clone()), &(b.0, b.1.clone())));

    let mut removed = 0usize;
    let mut freed = 0u64;
    let mut kept_bytes = 0u64;
    for (index, (mtime, path, size)) in files.into_iter().enumerate() {
        let over_count = policy.scratch_files > 0 && index >= policy.scratch_files;
        let over_bytes =
            policy.scratch_bytes > 0 && kept_bytes.saturating_add(size) > policy.scratch_bytes;
        let over_age =
            policy.scratch_age_days > 0 && age_days(mtime, now) >= policy.scratch_age_days;
        if over_count || over_bytes || over_age {
            if std::fs::remove_file(&path).is_ok() {
                removed += 1;
                freed += size;
            }
            continue;
        }
        kept_bytes = kept_bytes.saturating_add(size);
    }
    (removed, freed)
}

#[derive(Debug)]
struct SnapshotRun {
    path: PathBuf,
    chat_key: String,
    mtime: SystemTime,
    bytes: u64,
}

/// Newest-first ordering for snapshot runs, deterministic on equal mtimes.
fn snapshot_order(a: &SnapshotRun, b: &SnapshotRun) -> std::cmp::Ordering {
    b.mtime.cmp(&a.mtime).then_with(|| b.path.cmp(&a.path))
}

fn snapshot_runs_in(roots: &[PathBuf]) -> Vec<SnapshotRun> {
    let mut seen_roots = HashSet::new();
    let mut runs = Vec::new();
    for root in roots {
        let root = std::path::absolute(root).unwrap_or_else(|_| root.clone());
        if !seen_roots.insert(root.clone()) {
            continue;
        }
        let snapshots = root.join(".deep_agents").join("snapshots");
        let Ok(chats) = std::fs::read_dir(&snapshots) else {
            continue;
        };
        for chat in chats.flatten() {
            let chat_path = chat.path();
            if !chat_path.is_dir() {
                continue;
            }
            let chat_name = chat.file_name().to_string_lossy().to_string();
            let chat_key = format!("{}|{}", root.display(), chat_name);
            let Ok(run_dirs) = std::fs::read_dir(&chat_path) else {
                continue;
            };
            for run in run_dirs.flatten() {
                let path = run.path();
                if !path.is_dir() {
                    continue;
                }
                runs.push(SnapshotRun {
                    mtime: modified(&path),
                    bytes: tree_bytes(&path),
                    path,
                    chat_key: chat_key.clone(),
                });
            }
        }
    }
    runs
}

/// Snapshot runs across every workspace. Per-chat count/age caps apply first;
/// a global byte cap then evicts the oldest remaining runs, always protecting
/// the newest run for each chat so rewind still has a target.
fn sweep_snapshot_roots(
    roots: &[PathBuf],
    policy: &RetentionPolicy,
    now: SystemTime,
) -> (usize, u64) {
    if policy.snapshot_runs == 0 && policy.snapshot_age_days == 0 && policy.snapshot_bytes == 0 {
        return (0, 0);
    }
    let runs = snapshot_runs_in(roots);
    if runs.is_empty() {
        return (0, 0);
    }

    let mut by_chat: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, run) in runs.iter().enumerate() {
        by_chat.entry(run.chat_key.clone()).or_default().push(index);
    }

    let mut remove: HashSet<usize> = HashSet::new();
    let mut newest: HashMap<String, usize> = HashMap::new();
    for (chat_key, indices) in &by_chat {
        let mut sorted = indices.clone();
        sorted.sort_by(|&a, &b| snapshot_order(&runs[a], &runs[b]));
        if let Some(&first) = sorted.first() {
            newest.insert(chat_key.clone(), first);
        }
        for (position, &index) in sorted.iter().enumerate() {
            let over_count = policy.snapshot_runs > 0 && position >= policy.snapshot_runs;
            let over_age = policy.snapshot_age_days > 0
                && age_days(runs[index].mtime, now) >= policy.snapshot_age_days;
            if over_count || over_age {
                remove.insert(index);
            }
        }
    }

    let mut kept: Vec<usize> = (0..runs.len())
        .filter(|index| !remove.contains(index))
        .collect();
    let mut kept_bytes: u64 = kept.iter().map(|&index| runs[index].bytes).sum();
    if policy.snapshot_bytes > 0 && kept_bytes > policy.snapshot_bytes {
        // Newest first, then reverse for oldest-first eviction.
        kept.sort_by(|&a, &b| snapshot_order(&runs[a], &runs[b]).reverse());
        for &index in &kept {
            if kept_bytes <= policy.snapshot_bytes {
                break;
            }
            if newest.get(&runs[index].chat_key) == Some(&index) {
                continue;
            }
            if remove.insert(index) {
                kept_bytes = kept_bytes.saturating_sub(runs[index].bytes);
            }
        }
    }

    let mut removed = 0usize;
    let mut freed = 0u64;
    for &index in &remove {
        if std::fs::remove_dir_all(&runs[index].path).is_ok() {
            removed += 1;
            freed = freed.saturating_add(runs[index].bytes);
        }
    }
    (removed, freed)
}

/// Snapshot runs for one workspace: `.deep_agents/snapshots/<chat>/<run>/`.
/// The chat directory itself is never removed.
pub fn sweep_snapshots(workspace: &Path, policy: &RetentionPolicy, now: SystemTime) -> usize {
    sweep_snapshot_roots(&[workspace.to_path_buf()], policy, now).0
}

fn pending_import_staging(root: &Path) -> Option<PathBuf> {
    let marker = root.join(".deep_agents").join("pending_import.json");
    let text = std::fs::read_to_string(marker).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(PathBuf::from(value.get("staging")?.as_str()?))
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (std::path::absolute(a), std::path::absolute(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Import staging: `.deep_agents/imports/<id>/`. A directory referenced by the
/// pending marker is never touched. Orphans left by a crash are removed once
/// they are older than the policy threshold.
pub fn sweep_imports(root: &Path, policy: &RetentionPolicy, now: SystemTime) -> usize {
    if policy.import_age_days == 0 {
        return 0;
    }
    let directory = root.join(".deep_agents").join("imports");
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return 0;
    };
    let pending = pending_import_staging(root);
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if pending
            .as_ref()
            .is_some_and(|active| same_path(active, &path))
        {
            continue;
        }
        if age_days(modified(&path), now) >= policy.import_age_days
            && std::fs::remove_dir_all(&path).is_ok()
        {
            removed += 1;
        }
    }
    removed
}

/// One full pass over harness-owned artifacts. `workspaces` are the project
/// roots registered with the harness; `root` is always swept too because it is
/// the default workspace for general chats.
pub fn sweep_harness(
    root: &Path,
    workspaces: &[PathBuf],
    policy: &RetentionPolicy,
) -> RetentionReport {
    let now = SystemTime::now();
    let (scratch_removed, scratch_bytes) = sweep_scratch(root, policy, now);
    // OpenCode's ToolOutput layer does this independently of its scratch-like
    // artifacts: ten-to-the-hour sweep, seven-day age, no count/byte cap.
    let _ = sweep_tool_output(root, now);
    let mut roots = vec![root.to_path_buf()];
    roots.extend(workspaces.iter().cloned());
    let (snapshot_runs_removed, snapshot_bytes) = sweep_snapshot_roots(&roots, policy, now);
    RetentionReport {
        scratch_removed,
        scratch_bytes,
        snapshot_runs_removed,
        snapshot_bytes,
        imports_removed: sweep_imports(root, policy, now),
    }
}

/// Free bytes on the volume holding `path`, used for the disk warning.
/// Only Windows has an implementation today (the supported host); elsewhere
/// this returns `None` and the caller stays quiet.
pub fn free_bytes(path: &Path) -> Option<u64> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetDiskFreeSpaceExW(
                directory: *const u16,
                free_to_caller: *mut u64,
                total: *mut u64,
                free: *mut u64,
            ) -> i32;
        }
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut free_to_caller = 0u64;
        let mut total = 0u64;
        let mut free = 0u64;
        // SAFETY: the wide path is NUL-terminated and every out-pointer is
        // valid for the duration of the call.
        let ok = unsafe {
            GetDiskFreeSpaceExW(wide.as_ptr(), &mut free_to_caller, &mut total, &mut free)
        };
        (ok != 0).then_some(free_to_caller)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_scratch(root: &Path, name: &str, bytes: usize) {
        let directory = scratch_dir(root);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(name), vec![b'x'; bytes]).unwrap();
    }

    fn names(directory: &Path) -> Vec<String> {
        std::fs::read_dir(directory)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect()
    }

    #[test]
    fn scratch_count_cap_keeps_newest_files() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..5 {
            write_scratch(dir.path(), &format!("call_{index}.log"), 16);
        }
        let policy = RetentionPolicy {
            scratch_files: 2,
            scratch_bytes: 0,
            scratch_age_days: 0,
            ..RetentionPolicy::default()
        };
        let (removed, _) = sweep_scratch(dir.path(), &policy, SystemTime::now());
        assert_eq!(removed, 3);
        let remaining = names(&scratch_dir(dir.path()));
        assert!(
            remaining.contains(&"call_3.log".to_string()),
            "{remaining:?}"
        );
        assert!(
            remaining.contains(&"call_4.log".to_string()),
            "{remaining:?}"
        );
        assert_eq!(remaining.len(), 2);
    }

    #[test]
    fn scratch_byte_cap_evicts_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        write_scratch(dir.path(), "a_old.log", 100);
        write_scratch(dir.path(), "z_new.log", 100);
        let policy = RetentionPolicy {
            scratch_files: 0,
            scratch_bytes: 150,
            scratch_age_days: 0,
            ..RetentionPolicy::default()
        };
        let (removed, freed) = sweep_scratch(dir.path(), &policy, SystemTime::now());
        assert_eq!(removed, 1);
        assert_eq!(freed, 100);
        assert!(!scratch_dir(dir.path()).join("a_old.log").exists());
        assert!(scratch_dir(dir.path()).join("z_new.log").exists());
    }

    #[test]
    fn scratch_age_cap_drops_stale_files() {
        let dir = tempfile::tempdir().unwrap();
        write_scratch(dir.path(), "stale.log", 10);
        let policy = RetentionPolicy {
            scratch_files: 0,
            scratch_bytes: 0,
            scratch_age_days: 1,
            ..RetentionPolicy::default()
        };
        // Ask for a sweep "three days from now": the file is already old.
        let now = SystemTime::now() + Duration::from_secs(86_400 * 3);
        let (removed, _) = sweep_scratch(dir.path(), &policy, now);
        assert_eq!(removed, 1);
    }

    #[test]
    fn scratch_sweep_covers_every_harness_extension() {
        let dir = tempfile::tempdir().unwrap();
        write_scratch(dir.path(), "spill.log", 10);
        write_scratch(dir.path(), "grep.txt", 10);
        write_scratch(dir.path(), "screenshot.png", 10);
        let policy = RetentionPolicy {
            scratch_files: 0,
            scratch_bytes: 0,
            scratch_age_days: 1,
            ..RetentionPolicy::default()
        };
        let now = SystemTime::now() + Duration::from_secs(86_400 * 3);
        let (removed, _) = sweep_scratch(dir.path(), &policy, now);
        assert_eq!(removed, 3);
        assert!(names(&scratch_dir(dir.path())).is_empty());
    }

    #[test]
    fn tool_output_sweep_matches_opencode_prefix_and_age() {
        let dir = tempfile::tempdir().unwrap();
        let output = tool_output_dir(dir.path());
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(output.join("tool_0123456789ab"), "old").unwrap();
        std::fs::write(output.join("tool_0123456789g0"), "not hex").unwrap();
        std::fs::write(output.join("notes.txt"), "foreign").unwrap();

        let (removed, _) = sweep_tool_output(dir.path(), SystemTime::now());
        assert_eq!(removed, 0, "seven days have not passed");

        let now = SystemTime::now() + Duration::from_secs(86_400 * 8);
        let (removed, _) = sweep_tool_output(dir.path(), now);
        assert_eq!(removed, 1);
        assert!(!output.join("tool_0123456789ab").exists());
        assert!(output.join("tool_0123456789g0").exists());
        assert!(output.join("notes.txt").exists());
    }

    #[test]
    fn scratch_sweep_ignores_foreign_files() {
        let dir = tempfile::tempdir().unwrap();
        let directory = scratch_dir(dir.path());
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("notes.md"), "keep me").unwrap();
        std::fs::create_dir_all(directory.join("subdir")).unwrap();
        std::fs::write(directory.join("subdir").join("keep.log"), "keep").unwrap();
        let policy = RetentionPolicy {
            scratch_files: 1,
            scratch_bytes: 1,
            scratch_age_days: 1,
            ..RetentionPolicy::default()
        };
        let now = SystemTime::now() + Duration::from_secs(86_400 * 30);
        let (removed, _) = sweep_scratch(dir.path(), &policy, now);
        assert_eq!(removed, 0);
        assert!(directory.join("notes.md").exists());
        assert!(directory.join("subdir").join("keep.log").exists());
    }

    fn snapshot_chat(root: &Path, chat: &str, runs: usize, bytes: usize) {
        let directory = root.join(".deep_agents").join("snapshots").join(chat);
        for index in 0..runs {
            let run = directory.join(format!("run-{index}"));
            std::fs::create_dir_all(&run).unwrap();
            std::fs::write(run.join("manifest.json"), "{}").unwrap();
            std::fs::write(run.join("data.bin"), vec![b'x'; bytes]).unwrap();
        }
    }

    #[test]
    fn snapshot_cap_keeps_newest_runs_per_chat() {
        let dir = tempfile::tempdir().unwrap();
        snapshot_chat(dir.path(), "chat-1", 4, 8);
        let chat = dir
            .path()
            .join(".deep_agents")
            .join("snapshots")
            .join("chat-1");
        let policy = RetentionPolicy {
            snapshot_runs: 2,
            snapshot_bytes: 0,
            snapshot_age_days: 0,
            ..RetentionPolicy::default()
        };
        let removed = sweep_snapshots(dir.path(), &policy, SystemTime::now());
        assert_eq!(removed, 2);
        assert!(chat.join("run-3").exists());
        assert!(chat.join("run-2").exists());
        assert!(!chat.join("run-0").exists());
        // The chat directory itself always survives.
        assert!(chat.is_dir());
    }

    #[test]
    fn snapshot_cap_is_per_chat() {
        let dir = tempfile::tempdir().unwrap();
        snapshot_chat(dir.path(), "chat-a", 3, 8);
        snapshot_chat(dir.path(), "chat-b", 3, 8);
        let chats = dir.path().join(".deep_agents").join("snapshots");
        let policy = RetentionPolicy {
            snapshot_runs: 1,
            snapshot_bytes: 0,
            snapshot_age_days: 0,
            ..RetentionPolicy::default()
        };
        assert_eq!(sweep_snapshots(dir.path(), &policy, SystemTime::now()), 4);
        assert!(chats.join("chat-a").join("run-2").is_dir());
        assert!(chats.join("chat-b").join("run-2").is_dir());
    }

    #[test]
    fn snapshot_byte_cap_evicts_oldest_and_keeps_newest() {
        let dir = tempfile::tempdir().unwrap();
        snapshot_chat(dir.path(), "chat-1", 4, 10_000);
        let chat = dir
            .path()
            .join(".deep_agents")
            .join("snapshots")
            .join("chat-1");
        let policy = RetentionPolicy {
            snapshot_runs: 0,
            snapshot_bytes: 25_000,
            snapshot_age_days: 0,
            ..RetentionPolicy::default()
        };
        let removed = sweep_snapshots(dir.path(), &policy, SystemTime::now());
        assert_eq!(removed, 2);
        assert!(chat.join("run-3").is_dir());
        assert!(chat.join("run-2").is_dir());
        assert!(!chat.join("run-1").exists());
        assert!(!chat.join("run-0").exists());
    }

    #[test]
    fn snapshot_sweep_uses_the_real_checkpoint_layout() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..3 {
            let checkpoint = crate::Checkpoint::begin("chat", dir.path());
            std::fs::write(dir.path().join(format!("file-{index}.txt")), "content").unwrap();
            checkpoint.snapshot_all();
            checkpoint.finalize();
        }
        let policy = RetentionPolicy {
            snapshot_runs: 1,
            snapshot_bytes: 0,
            snapshot_age_days: 0,
            ..RetentionPolicy::default()
        };
        assert_eq!(sweep_snapshots(dir.path(), &policy, SystemTime::now()), 2);
        let chat = dir
            .path()
            .join(".deep_agents")
            .join("snapshots")
            .join("chat");
        assert_eq!(std::fs::read_dir(chat).unwrap().count(), 1);
    }

    #[test]
    fn sweep_harness_includes_registered_workspaces() {
        let harness = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        snapshot_chat(project.path(), "chat", 2, 8);
        let policy = RetentionPolicy {
            snapshot_runs: 1,
            snapshot_bytes: 0,
            snapshot_age_days: 0,
            ..RetentionPolicy::default()
        };
        let report = sweep_harness(harness.path(), &[project.path().to_path_buf()], &policy);
        assert_eq!(report.snapshot_runs_removed, 1);
        assert!(
            project
                .path()
                .join(".deep_agents/snapshots/chat/run-1")
                .is_dir()
        );
        assert!(
            !project
                .path()
                .join(".deep_agents/snapshots/chat/run-0")
                .exists()
        );
    }

    #[test]
    fn imports_sweep_removes_orphans_and_keeps_pending() {
        let dir = tempfile::tempdir().unwrap();
        let imports = dir.path().join(".deep_agents").join("imports");
        let orphan = imports.join("orphan");
        let active = imports.join("active");
        std::fs::create_dir_all(&orphan).unwrap();
        std::fs::create_dir_all(&active).unwrap();
        std::fs::write(active.join("data"), "keep").unwrap();
        let marker = dir.path().join(".deep_agents").join("pending_import.json");
        std::fs::write(
            marker,
            serde_json::json!({"staging": active.to_string_lossy()}).to_string(),
        )
        .unwrap();
        let policy = RetentionPolicy {
            import_age_days: 1,
            ..RetentionPolicy::default()
        };
        let now = SystemTime::now() + Duration::from_secs(86_400 * 30);
        assert_eq!(sweep_imports(dir.path(), &policy, now), 1);
        assert!(!orphan.exists());
        assert!(active.exists());
    }

    #[test]
    fn sweep_harness_reports_every_dimension() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..3 {
            write_scratch(dir.path(), &format!("call_{index}.log"), 8);
        }
        snapshot_chat(dir.path(), "c", 3, 8);
        let policy = RetentionPolicy {
            scratch_files: 1,
            snapshot_runs: 1,
            snapshot_bytes: 0,
            import_age_days: 0,
            ..RetentionPolicy::default()
        };
        let report = sweep_harness(dir.path(), &[], &policy);
        assert_eq!(report.scratch_removed, 2);
        assert_eq!(report.scratch_bytes, 16);
        assert_eq!(report.snapshot_runs_removed, 2);
        assert_eq!(report.imports_removed, 0);
        assert!(!report.is_empty());
    }

    #[test]
    fn missing_directories_are_no_ops() {
        let dir = tempfile::tempdir().unwrap();
        let report = sweep_harness(dir.path(), &[], &RetentionPolicy::default());
        assert!(report.is_empty());
        assert_eq!(
            sweep_snapshots(dir.path(), &RetentionPolicy::default(), SystemTime::now()),
            0
        );
        assert_eq!(
            sweep_imports(dir.path(), &RetentionPolicy::default(), SystemTime::now()),
            0
        );
    }

    #[test]
    fn zero_limits_disable_a_dimension() {
        let dir = tempfile::tempdir().unwrap();
        write_scratch(dir.path(), "keep.log", 4096);
        snapshot_chat(dir.path(), "c", 2, 8);
        let imports = dir.path().join(".deep_agents").join("imports").join("old");
        std::fs::create_dir_all(&imports).unwrap();
        let disabled = RetentionPolicy {
            scratch_files: 0,
            scratch_bytes: 0,
            scratch_age_days: 0,
            snapshot_runs: 0,
            snapshot_bytes: 0,
            snapshot_age_days: 0,
            import_age_days: 0,
        };
        let now = SystemTime::now() + Duration::from_secs(86_400 * 30);
        let (removed, _) = sweep_scratch(dir.path(), &disabled, now);
        assert_eq!(removed, 0);
        assert_eq!(sweep_snapshots(dir.path(), &disabled, now), 0);
        assert_eq!(sweep_imports(dir.path(), &disabled, now), 0);
        assert!(scratch_dir(dir.path()).join("keep.log").exists());
        assert!(imports.exists());
    }

    #[test]
    fn default_policy_matches_documented_numbers() {
        let policy = RetentionPolicy::default();
        assert_eq!(policy.scratch_files, 200);
        assert_eq!(policy.scratch_bytes, 50 * MIB);
        assert_eq!(policy.scratch_age_days, 7);
        assert_eq!(policy.snapshot_runs, 20);
        assert_eq!(policy.snapshot_bytes, 1024 * MIB);
        assert_eq!(policy.snapshot_age_days, 14);
        assert_eq!(policy.import_age_days, 7);
    }
}
