//! Cross-process ownership of one harness root.
//!
//! Two servers sharing a harness root would both run schedules, groups,
//! canvas wakes, retention, and rollout writers. The TCP port only prevents
//! two servers on the *same* port; different ports or a not-yet-bound listener
//! leave the data directory unprotected. This lock closes that gap.
//!
//! The lock is an OS advisory whole-file lock, so it is released automatically
//! when the process exits or crashes. The lock file itself stays empty; a
//! sibling `.owner` file carries diagnostics (pid, start time, executable,
//! version) without being made unreadable by the locked byte range on Windows.

use std::collections::HashSet;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

static HELD: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

fn held() -> &'static Mutex<HashSet<PathBuf>> {
    HELD.get_or_init(|| Mutex::new(HashSet::new()))
}

#[derive(Debug)]
pub enum LockError {
    Busy {
        path: PathBuf,
        owner: String,
    },
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl std::fmt::Display for LockError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy { path, owner } if owner.is_empty() => write!(
                formatter,
                "another Stark server already owns this harness ({}); close it before starting a second server",
                path.display()
            ),
            Self::Busy { path, owner } => write!(
                formatter,
                "another Stark server already owns this harness ({}; {owner}); close it before starting a second server",
                path.display()
            ),
            Self::Io { path, source } => {
                write!(
                    formatter,
                    "cannot lock harness {}: {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for LockError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Busy { .. } => None,
        }
    }
}

#[derive(Debug)]
pub struct HarnessLock {
    file: Option<File>,
    path: PathBuf,
}

impl Drop for HarnessLock {
    fn drop(&mut self) {
        // Close first: on platforms where `flock` converts instead of
        // conflicting within one process, the in-process guard must not be
        // removed while the OS handle can still be reused.
        self.file.take();
        if let Ok(mut held) = held().lock() {
            held.remove(&self.path);
        }
    }
}

fn owner_path(lock_path: &Path) -> PathBuf {
    lock_path.with_extension("owner")
}

fn owner_text(lock_path: &Path) -> String {
    std::fs::read_to_string(owner_path(lock_path))
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(4)
        .collect::<Vec<_>>()
        .join(", ")
}

fn release_held(path: &Path) {
    if let Ok(mut held) = held().lock() {
        held.remove(path);
    }
}

impl HarnessLock {
    /// Acquires exclusive ownership of `root`. Fails immediately when another
    /// process holds the lock; a stale file without a live lock is reusable.
    pub fn acquire(root: &Path) -> Result<Self, LockError> {
        let path = root.join(".deep_agents").join("harness.lock");
        let Some(parent) = path.parent() else {
            return Err(LockError::Io {
                path,
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "harness lock has no parent directory",
                ),
            });
        };
        if let Err(source) = std::fs::create_dir_all(parent) {
            return Err(LockError::Io { path, source });
        }

        // The OS lock is the cross-process boundary. This in-process guard
        // also makes duplicate acquisition deterministic on platforms where
        // `flock` converts an existing lock instead of reporting WouldBlock.
        let mut local = held().lock().unwrap_or_else(|poison| poison.into_inner());
        if local.contains(&path) {
            drop(local);
            let owner = owner_text(&path);
            return Err(LockError::Busy { path, owner });
        }
        local.insert(path.clone());
        drop(local);

        let file = match OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(source) => {
                release_held(&path);
                return Err(LockError::Io { path, source });
            }
        };

        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                release_held(&path);
                let owner = owner_text(&path);
                return Err(LockError::Busy { path, owner });
            }
            Err(TryLockError::Error(source)) => {
                release_held(&path);
                return Err(LockError::Io { path, source });
            }
        }

        // Diagnostics live in a sibling file. Windows makes a locked byte
        // range unreadable through another handle, so the owner must not be
        // stored inside the locked file. Best-effort: diagnostics failing must
        // not prevent the server from starting.
        let _ = std::fs::write(
            owner_path(&path),
            format!(
                "pid={}\nstarted={}\nexe={}\nversion={}\n",
                std::process::id(),
                chrono::Utc::now().to_rfc3339(),
                std::env::current_exe()
                    .map(|exe| exe.to_string_lossy().to_string())
                    .unwrap_or_default(),
                env!("CARGO_PKG_VERSION")
            ),
        );
        Ok(Self {
            file: Some(file),
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_is_rejected_until_the_first_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let first = HarnessLock::acquire(dir.path()).unwrap();
        assert!(first.path().ends_with("harness.lock"));
        let error = HarnessLock::acquire(dir.path()).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("already owns"), "{text}");
        assert!(text.contains("pid="), "{text}");
        drop(first);
        HarnessLock::acquire(dir.path()).unwrap();
    }

    #[test]
    fn stale_lock_file_without_a_live_lock_is_reused() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join(".deep_agents").join("harness.lock");
        std::fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
        std::fs::write(&lock_path, "pid=999999\n").unwrap();
        let lock = HarnessLock::acquire(dir.path()).unwrap();
        let text = std::fs::read_to_string(owner_path(lock.path())).unwrap();
        assert!(text.contains(&format!("pid={}", std::process::id())));
    }
}
