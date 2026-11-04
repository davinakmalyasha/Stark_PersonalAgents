//! Opt-in Landlock filesystem confinement for spawned children (Linux only).
//!
//! Enabled with `STARK_LANDLOCK=1`. The policy is deliberately conservative:
//!
//! - reads and exec are allowed everywhere (tools must inspect the system);
//! - writes are allowed only inside the workspace, `/tmp`, the user's tool
//!   caches (`~/.cargo`, `~/.rustup`, `~/.npm`, `~/.cache`, `~/.config`,
//!   `~/.local`) and `/dev/null`;
//! - `read-only` mode gets no write rules at all;
//! - `danger-full-access` skips Landlock entirely.
//!
//! Everything is prepared in the parent process (paths opened/allocated), so
//! the `pre_exec` closure only performs syscalls. Failure to apply is returned
//! to the caller, which fails the spawn: an opted-in sandbox must not silently
//! degrade to unsandboxed execution.
//!
//! Known gap: portable-pty has no `pre_exec` hook, so PTY sessions are not
//! covered by this policy.

use std::ffi::CString;
use std::io;
use std::path::PathBuf;

use super::{SandboxMode, SandboxProfile};

const SYS_CREATE_RULESET: i64 = 444;
const SYS_ADD_RULE: i64 = 445;
const SYS_RESTRICT_SELF: i64 = 446;
const RULE_PATH_BENEATH: i32 = 1;
const CREATE_RULESET_VERSION: i64 = 1;
const PR_SET_NO_NEW_PRIVS: i32 = 38;

const ACCESS_EXECUTE: u64 = 1 << 0;
const ACCESS_WRITE_FILE: u64 = 1 << 1;
const ACCESS_READ_FILE: u64 = 1 << 2;
const ACCESS_READ_DIR: u64 = 1 << 3;
const ACCESS_REMOVE_DIR: u64 = 1 << 4;
const ACCESS_REMOVE_FILE: u64 = 1 << 5;
const ACCESS_MAKE_CHAR: u64 = 1 << 6;
const ACCESS_MAKE_DIR: u64 = 1 << 7;
const ACCESS_MAKE_REG: u64 = 1 << 8;
const ACCESS_MAKE_SOCK: u64 = 1 << 9;
const ACCESS_MAKE_FIFO: u64 = 1 << 10;
const ACCESS_MAKE_BLOCK: u64 = 1 << 11;
const ACCESS_MAKE_SYM: u64 = 1 << 12;
const ACCESS_REFER: u64 = 1 << 13;
const ACCESS_TRUNCATE: u64 = 1 << 14;

const READ_ONLY: u64 = ACCESS_EXECUTE | ACCESS_READ_FILE | ACCESS_READ_DIR;
const WRITE: u64 = ACCESS_WRITE_FILE
    | ACCESS_REMOVE_DIR
    | ACCESS_REMOVE_FILE
    | ACCESS_MAKE_CHAR
    | ACCESS_MAKE_DIR
    | ACCESS_MAKE_REG
    | ACCESS_MAKE_SOCK
    | ACCESS_MAKE_FIFO
    | ACCESS_MAKE_BLOCK
    | ACCESS_MAKE_SYM;

const O_PATH: i32 = 0o10000000;
const O_CLOEXEC: i32 = 0o2000000;

#[repr(C)]
struct RulesetAttr {
    handled_access_fs: u64,
}

#[repr(C, packed)]
struct PathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

unsafe extern "C" {
    fn syscall(number: i64, ...) -> i64;
    fn open(path: *const i8, flags: i32, ...) -> i32;
    fn close(fd: i32) -> i32;
    fn prctl(option: i32, ...) -> i32;
}

pub fn enabled() -> bool {
    cfg!(target_os = "linux")
        && std::env::var("STARK_LANDLOCK")
            .map(|value| matches!(value.trim(), "1" | "true" | "on" | "yes"))
            .unwrap_or(false)
}

/// Everything needed by the child, allocated before fork.
pub struct Prepared {
    entries: Vec<(CString, u64)>,
    handled: u64,
}

impl Prepared {
    /// Build the policy in the parent. Returns `None` when Landlock is off or
    /// the profile asks for full access.
    pub fn for_profile(profile: &SandboxProfile) -> Option<Self> {
        if !enabled() || profile.mode == SandboxMode::DangerFullAccess {
            return None;
        }
        // ABI 1+ is required. TRUNCATE needs ABI 3, REFER needs ABI 2.
        let abi = unsafe {
            syscall(
                SYS_CREATE_RULESET,
                std::ptr::null::<RulesetAttr>(),
                0usize,
                CREATE_RULESET_VERSION,
            )
        };
        if abi < 1 {
            return None;
        }
        let abi = abi as u32;
        let mut handled = READ_ONLY | WRITE;
        if abi >= 2 {
            handled |= ACCESS_REFER;
        }
        if abi >= 3 {
            handled |= ACCESS_TRUNCATE;
        }

        let mut entries: Vec<(CString, u64)> = Vec::new();
        if let Ok(path) = CString::new("/") {
            entries.push((path, READ_ONLY));
        }
        // /dev/null is a file rule: allow the write redirections shells use.
        if let Ok(path) = CString::new("/dev/null") {
            entries.push((path, ACCESS_WRITE_FILE));
        }
        if profile.mode == SandboxMode::WorkspaceWrite {
            let mut writable: Vec<PathBuf> = vec![profile.workspace.clone(), PathBuf::from("/tmp")];
            if let Ok(home) = std::env::var("HOME") {
                let home = PathBuf::from(home);
                for sub in [".cargo", ".rustup", ".npm", ".cache", ".config", ".local"] {
                    writable.push(home.join(sub));
                }
            }
            for path in writable {
                if !path.exists() {
                    continue;
                }
                if let Ok(c) = CString::new(path.to_string_lossy().as_bytes()) {
                    entries.push((c, READ_ONLY | WRITE));
                }
            }
        }
        Some(Self { entries, handled })
    }

    /// Apply the policy to the current thread/process. Syscalls only.
    pub fn apply(&self) -> io::Result<()> {
        let attr = RulesetAttr {
            handled_access_fs: self.handled,
        };
        let ruleset = unsafe {
            syscall(
                SYS_CREATE_RULESET,
                &attr as *const RulesetAttr,
                std::mem::size_of::<RulesetAttr>(),
                0i64,
            )
        };
        if ruleset < 0 {
            return Err(io::Error::last_os_error());
        }
        let ruleset = ruleset as i32;
        for (path, allowed) in &self.entries {
            let fd = unsafe { open(path.as_ptr(), O_PATH | O_CLOEXEC) };
            if fd < 0 {
                continue;
            }
            let rule = PathBeneathAttr {
                allowed_access: *allowed,
                parent_fd: fd,
            };
            unsafe {
                syscall(
                    SYS_ADD_RULE,
                    ruleset,
                    RULE_PATH_BENEATH,
                    &rule as *const PathBeneathAttr,
                    0i64,
                );
                close(fd);
            }
        }
        let prctl_result = unsafe { prctl(PR_SET_NO_NEW_PRIVS, 1i64, 0i64, 0i64, 0i64) };
        if prctl_result != 0 {
            unsafe { close(ruleset) };
            return Err(io::Error::last_os_error());
        }
        let restricted = unsafe { syscall(SYS_RESTRICT_SELF, ruleset, 0i64) };
        unsafe { close(ruleset) };
        if restricted != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

/// True when a profile would be confined by Landlock.
pub fn would_enforce(profile: &SandboxProfile) -> bool {
    enabled() && profile.mode != SandboxMode::DangerFullAccess
}
