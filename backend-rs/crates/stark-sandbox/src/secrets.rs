//! Child-process environment isolation.
//!
//! The server holds provider API keys and other credentials in its process
//! environment. Children (bash, PTY sessions, MCP servers, browser automation,
//! git) must not inherit them. The old implementation removed variables whose
//! names looked secret, which missed names such as `AWS_ACCESS_KEY_ID`,
//! `DATABASE_URL`, `MYSQL_PWD`, and any custom credential name.
//!
//! This module now builds a **fresh environment from an allowlist**:
//!
//! 1. `env_clear()` on the child command.
//! 2. Re-add only OS/shell essentials (PATH, SystemRoot, HOME, temp dirs,
//!    Rust/Cargo homes, locale).
//! 3. Re-add explicitly provided per-spawn variables (workspace/session IDs,
//!    `GIT_TERMINAL_PROMPT`, MCP server config, …).
//!
//! `STARK_ENV_ALLOW` may add comma-separated variable names for unusual
//! toolchains. It never re-enables a name that [`is_secret_var`] identifies.

use std::collections::BTreeSet;
use std::ffi::OsString;

/// Suffixes (uppercased) that mark an env var as a secret. Kept as defense in
/// depth: an explicitly re-added variable is still rejected if it looks secret.
const SECRET_SUFFIXES: &[&str] = &[
    "_API_KEY",
    "_API_KEYS",
    "_KEY",
    "_KEYS",
    "_TOKEN",
    "_TOKENS",
    "_SECRET",
    "_SECRETS",
    "_PASSWORD",
    "_PASSWD",
    "_PRIVATE_KEY",
];

/// Core variables every shell/toolchain may need. Names are case-insensitive
/// on Windows and matched as exact names on Unix.
#[cfg(windows)]
const CORE_ENV_VARS: &[&str] = &[
    // Path resolution and shell
    "PATH",
    "PATHEXT",
    "COMSPEC",
    "SYSTEMROOT",
    "WINDIR",
    "SYSTEMDRIVE",
    "SHELL",
    "TERM",
    "HOME",
    "PWD",
    "TZ",
    // User/profile locations
    "USERNAME",
    "USERDOMAIN",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    // Program locations
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "PROGRAMW6432",
    "COMMONPROGRAMFILES",
    "COMMONPROGRAMFILES(X86)",
    "COMMONPROGRAMW6432",
    // Temp locations
    "TEMP",
    "TMP",
    "TMPDIR",
    // Processor/runtime metadata used by build tools
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "PROCESSOR_IDENTIFIER",
    "OS",
    // Local Rust/Cargo toolchain. These are paths, not credentials.
    "CARGO_HOME",
    "RUSTUP_HOME",
    "CARGO_TARGET_DIR",
    "RUSTUP_TOOLCHAIN",
    "RUSTUP_DIST_SERVER",
    "RUSTUP_UPDATE_ROOT",
    // Git Bash / MSYS2 hints when present.
    "MSYSTEM",
    "MINGW_PREFIX",
    "MINGW_CHOST",
    "MINGW_PACKAGE_PREFIX",
];

#[cfg(not(windows))]
const CORE_ENV_VARS: &[&str] = &[
    "PATH",
    "SHELL",
    "TERM",
    "HOME",
    "PWD",
    "TZ",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "TEMP",
    "TMP",
    "LANG",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "CARGO_TARGET_DIR",
    "RUSTUP_TOOLCHAIN",
];

/// Returns true when `name` looks like a secret. Case-insensitive.
pub fn is_secret_var(name: &str) -> bool {
    let upper = name.trim().to_ascii_uppercase();
    if upper.is_empty() {
        return false;
    }
    if matches!(
        upper.as_str(),
        "GH_TOKEN"
            | "GITHUB_TOKEN"
            | "HF_TOKEN"
            | "STARK_API_KEY"
            | "AWS_ACCESS_KEY_ID"
            | "DATABASE_URL"
            | "MYSQL_PWD"
            | "PGPASSWORD"
    ) {
        return true;
    }
    SECRET_SUFFIXES.iter().any(|suffix| upper.ends_with(suffix))
}

fn extra_allow_names() -> BTreeSet<String> {
    std::env::var("STARK_ENV_ALLOW")
        .ok()
        .map(|value| {
            value
                .split(',')
                .map(|name| name.trim().to_ascii_uppercase())
                .filter(|name| !name.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn is_allowed_name(name: &str, extra: &BTreeSet<String>) -> bool {
    let upper = name.trim().to_ascii_uppercase();
    if upper.is_empty() || is_secret_var(&upper) {
        return false;
    }
    CORE_ENV_VARS
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(&upper))
        || upper.starts_with("LC_")
        || extra.contains(&upper)
}

/// Environment pairs for a child process. Reads the current process
/// environment but only copies allowed names. Values are never logged.
pub fn env_pairs() -> Vec<(String, String)> {
    let extra = extra_allow_names();
    std::env::vars_os()
        .filter_map(|(key, value)| {
            let name = key.to_string_lossy().into_owned();
            if !is_allowed_name(&name, &extra) {
                return None;
            }
            Some((name, value.to_string_lossy().into_owned()))
        })
        .collect()
}

/// Names of secret vars currently present in this process. Kept for callers
/// that need to remove a variable explicitly from a command builder.
pub fn secret_var_names() -> Vec<String> {
    std::env::vars_os()
        .filter_map(|(key, _)| {
            let name = key.to_string_lossy().into_owned();
            is_secret_var(&name).then_some(name)
        })
        .collect()
}

fn explicit_env(command: &std::process::Command) -> Vec<(OsString, OsString)> {
    command
        .get_envs()
        .filter_map(|(key, value)| value.map(|value| (key.to_os_string(), value.to_os_string())))
        .collect()
}

/// Replaces a std child's inherited environment with the allowlist, preserving
/// only explicitly-set variables that do not look secret.
pub fn isolate_command(command: &mut std::process::Command) {
    let explicit = explicit_env(command);
    command.env_clear();
    for (key, value) in env_pairs() {
        command.env(key, value);
    }
    for (key, value) in explicit {
        let name = key.to_string_lossy();
        if !is_secret_var(&name) {
            command.env(key, value);
        }
    }
}

/// Replaces a Tokio child's inherited environment with the allowlist.
///
/// `tokio::process::Command` has no `get_envs` in the supported version, so
/// callers must set per-spawn variables **after** this call.
pub fn isolate_tokio_command(command: &mut tokio::process::Command) {
    command.env_clear();
    for (key, value) in env_pairs() {
        command.env(key, value);
    }
}

/// Compatibility wrappers for existing spawn sites. Both now isolate rather
/// than merely scrubbing; explicit Tokio env must be set after the call.
pub fn scrub_command(command: &mut std::process::Command) {
    isolate_command(command);
}

pub fn scrub_tokio_command(command: &mut tokio::process::Command) {
    isolate_tokio_command(command);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_standard_secret_names() {
        for name in [
            "OPENCODE_API_KEY",
            "DEEPSEEK_API_KEY",
            "OPENROUTER_API_KEYS",
            "STARK_API_KEY",
            "COMMAND_CODE_API_KEY",
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "HF_TOKEN",
            "TELEGRAM_BOT_TOKEN",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_ACCESS_KEY_ID",
            "DATABASE_URL",
            "MYSQL_PWD",
            "MY_DB_PASSWORD",
            "SOME_PRIVATE_KEY",
        ] {
            assert!(is_secret_var(name), "{name} should be secret");
        }
    }

    #[test]
    fn keeps_non_secret_names() {
        for name in [
            "PATH",
            "SYSTEMROOT",
            "STARK_WORKSPACE",
            "STARK_SESSION_ID",
            "STARK_SANDBOX_MODE",
            "TERM",
            "HOME",
            "USER",
            "CARGO_HOME",
        ] {
            assert!(!is_secret_var(name), "{name} should not be secret");
        }
    }

    #[test]
    fn allowlist_includes_essentials_not_credentials() {
        let extra = BTreeSet::new();
        assert!(is_allowed_name("PATH", &extra));
        assert!(is_allowed_name("SystemRoot", &extra));
        assert!(is_allowed_name("LC_ALL", &extra));
        assert!(!is_allowed_name("AWS_ACCESS_KEY_ID", &extra));
        assert!(!is_allowed_name("DATABASE_URL", &extra));
        assert!(!is_allowed_name("SOME_RANDOM_TOKEN", &extra));
    }

    #[test]
    fn isolate_command_clears_inherited_secrets() {
        // `std::env::set_var` is unsafe in edition 2024 (multi-threaded races);
        // single-threaded tests are the documented safe context.
        unsafe {
            std::env::set_var("STARK_TEST_INHERITED_KEY", "shh");
            std::env::set_var("STARK_TEST_CORE_VALUE", "kept");
        }
        let mut command = std::process::Command::new("echo");
        command.env("STARK_TEST_EXPLICIT", "yes");
        command.env("STARK_TEST_EXPLICIT_KEY", "no");
        isolate_command(&mut command);
        let values: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert!(
            !values
                .iter()
                .any(|(key, value)| key == "STARK_TEST_INHERITED_KEY" && value.is_some()),
            "inherited secret must not be present"
        );
        assert!(
            !values
                .iter()
                .any(|(key, value)| key == "STARK_TEST_EXPLICIT_KEY" && value.is_some()),
            "explicit secret must not be present"
        );
        assert!(
            values
                .iter()
                .any(|(key, value)| key == "STARK_TEST_EXPLICIT" && value.as_deref() == Some("yes")),
            "explicit non-secret must survive"
        );
        unsafe {
            std::env::remove_var("STARK_TEST_INHERITED_KEY");
            std::env::remove_var("STARK_TEST_CORE_VALUE");
        }
    }
}
