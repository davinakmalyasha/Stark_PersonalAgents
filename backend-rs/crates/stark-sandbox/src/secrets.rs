//! Secret scrubbing for child processes.
//!
//! The server process holds provider API keys in its environment (referenced
//! by `key_env` in providers.json). Every child Stark spawns — bash sessions,
//! PTY sessions, MCP servers, browser automation, git — inherits that
//! environment by default. An agent running `env` or `printenv` would then see
//! every key, not just the one its provider needs.
//!
//! This module is the single place that decides what counts as a secret and
//! removes it. All spawn sites must call it; a spawn that skips it escapes
//! the policy. Values are never logged.

/// Suffixes (uppercased) that mark an env var as a secret. `_KEY` covers
/// `_API_KEY`; `_KEYS` covers legacy `*_API_KEYS` lists.
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

/// Returns true when `name` looks like a secret. Case-insensitive.
pub fn is_secret_var(name: &str) -> bool {
    let upper = name.trim().to_ascii_uppercase();
    if upper.is_empty() {
        return false;
    }
    // Exact well-known names that do not end with a listed suffix.
    if matches!(
        upper.as_str(),
        "GH_TOKEN" | "GITHUB_TOKEN" | "HF_TOKEN" | "STARK_API_KEY"
    ) {
        return true;
    }
    SECRET_SUFFIXES.iter().any(|suffix| upper.ends_with(suffix))
}

/// Names of secret vars currently present in this process. Used for
/// `env_remove` loops; values are never collected.
pub fn secret_var_names() -> Vec<String> {
    std::env::vars_os()
        .filter_map(|(key, _)| {
            let name = key.to_string_lossy().into_owned();
            is_secret_var(&name).then_some(name)
        })
        .collect()
}

/// Remove secret vars from a std child command. Keeps PATH, SYSTEMROOT, etc.
pub fn scrub_command(command: &mut std::process::Command) {
    for name in secret_var_names() {
        command.env_remove(name);
    }
}

/// Remove secret vars from a tokio child command.
pub fn scrub_tokio_command(command: &mut tokio::process::Command) {
    for name in secret_var_names() {
        command.env_remove(name);
    }
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
        ] {
            assert!(!is_secret_var(name), "{name} should not be secret");
        }
    }

    #[test]
    fn scrub_removes_secrets_but_keeps_path() {
        // `std::env::set_var` is unsafe in edition 2024 (multi-threaded
        // races); single-threaded tests are the documented safe context.
        unsafe {
            std::env::set_var("STARK_TEST_SCRUB_ME_KEY", "shh");
        }
        let mut command = std::process::Command::new("echo");
        command.env("STARK_TEST_SCRUB_ME_KEY", "shh");
        command.env("STARK_TEST_KEEP_ME", "yes");
        scrub_command(&mut command);
        // `get_envs` still yields removed keys with a `None` value; what
        // matters is the child no longer receives a value for it.
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
                .any(|(key, value)| key == "STARK_TEST_SCRUB_ME_KEY" && value.is_some()),
            "secret must be removed from the child env"
        );
        assert!(
            values
                .iter()
                .any(|(key, value)| key == "STARK_TEST_KEEP_ME" && value.as_deref() == Some("yes")),
            "non-secret env must survive scrubbing"
        );
        unsafe {
            std::env::remove_var("STARK_TEST_SCRUB_ME_KEY");
        }
    }
}
