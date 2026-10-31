//! Process sandbox configuration and spawn hooks.
//!
//! Two axes, mirroring Codex:
//! - [`SandboxMode`]: what a child process may touch (read-only, workspace
//!   write, full access).
//! - [`ApprovalPolicy`]: when to ask the user before escalating.
//!
//! The hook ([`SandboxProfile::wrap_command`]) is applied to **every** process
//! Stark spawns (plain pipes and PTY). Today the only backend is the explicit
//! `none` backend: it reports that enforcement is unavailable instead of
//! pretending otherwise. AppContainer (Windows) and Landlock (Linux) plug in
//! behind the same hook without touching the call sites.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

impl SandboxMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "read-only" | "readonly" | "read_only" => Some(Self::ReadOnly),
            "workspace-write" | "workspace_write" | "workspace" => Some(Self::WorkspaceWrite),
            "danger-full-access" | "danger" | "full" | "none" => Some(Self::DangerFullAccess),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::DangerFullAccess => "danger-full-access",
        }
    }

    pub fn from_env() -> Self {
        std::env::var("STARK_SANDBOX")
            .ok()
            .and_then(|value| Self::parse(&value))
            .unwrap_or(Self::WorkspaceWrite)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalPolicy {
    Untrusted,
    OnFailure,
    OnRequest,
    Never,
}

impl ApprovalPolicy {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "untrusted" => Some(Self::Untrusted),
            "on-failure" | "on_failure" => Some(Self::OnFailure),
            "on-request" | "on_request" => Some(Self::OnRequest),
            "never" => Some(Self::Never),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Untrusted => "untrusted",
            Self::OnFailure => "on-failure",
            Self::OnRequest => "on-request",
            Self::Never => "never",
        }
    }

    pub fn from_env() -> Self {
        std::env::var("STARK_APPROVAL")
            .ok()
            .and_then(|value| Self::parse(&value))
            .unwrap_or(Self::OnRequest)
    }
}

/// Which isolation backend this build has. `none` means the hook reports the
/// mode to the model but cannot enforce it.
pub fn backend_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "none (AppContainer pending)"
    } else if cfg!(target_os = "linux") {
        "none (Landlock pending)"
    } else if cfg!(target_os = "macos") {
        "none (Seatbelt pending)"
    } else {
        "none"
    }
}

/// True only when a real enforcement backend is compiled in.
pub fn enforced() -> bool {
    false
}

#[derive(Clone, Debug)]
pub struct SandboxProfile {
    pub mode: SandboxMode,
    pub approval: ApprovalPolicy,
    pub workspace: PathBuf,
    pub network: bool,
}

impl SandboxProfile {
    pub fn from_env(workspace: impl Into<PathBuf>) -> Self {
        let network = std::env::var("STARK_SANDBOX_NETWORK")
            .ok()
            .map(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "on" | "1" | "true"
                )
            })
            .unwrap_or(false);
        Self {
            mode: SandboxMode::from_env(),
            approval: ApprovalPolicy::from_env(),
            workspace: workspace.into(),
            network,
        }
    }

    pub fn new(workspace: impl Into<PathBuf>, mode: SandboxMode) -> Self {
        Self {
            mode,
            approval: ApprovalPolicy::OnRequest,
            workspace: workspace.into(),
            network: false,
        }
    }

    /// Apply the sandbox hook to a std process. Every spawn path must call
    /// this; a spawn that skips it escapes the policy.
    pub fn wrap_command(&self, command: &mut Command) {
        for (key, value) in self.env() {
            command.env(key, value);
        }
        backend::wrap(command, self);
    }

    /// Tokio variant used by the process manager.
    pub fn wrap_tokio_command(&self, command: &mut tokio::process::Command) {
        for (key, value) in self.env() {
            command.env(key, value);
        }
        backend::wrap_tokio(command, self);
    }

    /// Environment pairs for spawn paths that build their own command
    /// (portable-pty's CommandBuilder).
    pub fn env(&self) -> Vec<(&'static str, String)> {
        vec![
            ("STARK_SANDBOX_MODE", self.mode.as_str().to_string()),
            ("STARK_APPROVAL", self.approval.as_str().to_string()),
            (
                "STARK_SANDBOX_NETWORK",
                if self.network { "on" } else { "off" }.to_string(),
            ),
        ]
    }

    /// One prompt line that states the truth: requested mode, real backend,
    /// and whether enforcement is active.
    pub fn prompt_line(&self) -> String {
        let enforcement = if enforced() {
            "enforced"
        } else {
            "not enforced (no OS backend in this build)"
        };
        format!(
            "Sandbox: {mode} requested, network {network}, backend {backend} — {enforcement}.",
            mode = self.mode.as_str(),
            network = if self.network { "on" } else { "off" },
            backend = backend_name(),
            enforcement = enforcement,
        )
    }
}

impl Default for SandboxProfile {
    fn default() -> Self {
        Self {
            mode: SandboxMode::WorkspaceWrite,
            approval: ApprovalPolicy::OnRequest,
            workspace: PathBuf::from("."),
            network: false,
        }
    }
}

mod backend {
    use super::SandboxProfile;
    use std::process::Command;

    /// Placeholder backend: no kernel enforcement yet. Keeping the hook here
    /// means no spawn site changes when AppContainer/Landlock land.
    pub fn wrap(_command: &mut Command, _profile: &SandboxProfile) {}

    pub fn wrap_tokio(_command: &mut tokio::process::Command, _profile: &SandboxProfile) {}
}

/// Convenience for callers that only have a workspace path.
pub fn for_workspace(workspace: &Path) -> SandboxProfile {
    SandboxProfile::from_env(workspace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_parse_and_round_trip() {
        assert_eq!(SandboxMode::parse("read-only"), Some(SandboxMode::ReadOnly));
        assert_eq!(
            SandboxMode::parse("workspace-write"),
            Some(SandboxMode::WorkspaceWrite)
        );
        assert_eq!(
            SandboxMode::parse("danger-full-access"),
            Some(SandboxMode::DangerFullAccess)
        );
        assert_eq!(SandboxMode::parse("bogus"), None);
        for mode in [
            SandboxMode::ReadOnly,
            SandboxMode::WorkspaceWrite,
            SandboxMode::DangerFullAccess,
        ] {
            assert_eq!(SandboxMode::parse(mode.as_str()), Some(mode));
        }
    }

    #[test]
    fn approval_policies_parse() {
        assert_eq!(
            ApprovalPolicy::parse("on-failure"),
            Some(ApprovalPolicy::OnFailure)
        );
        assert_eq!(ApprovalPolicy::parse("never"), Some(ApprovalPolicy::Never));
        assert_eq!(ApprovalPolicy::parse("sometimes"), None);
    }

    #[test]
    fn prompt_line_is_honest_about_enforcement() {
        let profile = SandboxProfile::new("/tmp/project", SandboxMode::WorkspaceWrite);
        let line = profile.prompt_line();
        assert!(line.contains("workspace-write"));
        assert!(line.contains("network off"));
        assert!(line.contains("not enforced"), "{line}");
        assert!(!enforced());
    }

    #[test]
    fn wrapped_child_receives_sandbox_env() {
        let profile = SandboxProfile::new(".", SandboxMode::ReadOnly);
        #[cfg(windows)]
        let mut command = {
            let mut command = std::process::Command::new("cmd");
            command.args(["/C", "echo %STARK_SANDBOX_MODE%"]);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = std::process::Command::new("sh");
            command.args(["-c", "printf %s \"$STARK_SANDBOX_MODE\""]);
            command
        };
        profile.wrap_command(&mut command);
        let output = command.output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "read-only",
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn wrap_sets_sandbox_env_for_children() {
        let profile = SandboxProfile::new("/tmp/project", SandboxMode::ReadOnly);
        let mut command = Command::new("bash");
        profile.wrap_command(&mut command);
        let envs: Vec<String> = command
            .get_envs()
            .map(|(key, value)| {
                format!(
                    "{}={}",
                    key.to_string_lossy(),
                    value
                        .map(|value| value.to_string_lossy().to_string())
                        .unwrap_or_default()
                )
            })
            .collect();
        assert!(
            envs.iter()
                .any(|entry| entry == "STARK_SANDBOX_MODE=read-only")
        );
        assert!(
            envs.iter()
                .any(|entry| entry == "STARK_SANDBOX_NETWORK=off")
        );
    }
}
