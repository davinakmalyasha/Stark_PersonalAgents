mod decision;
mod error;
mod executable_name;
mod parser;
mod permissions;
mod policy;
mod read_only;
pub mod rule;

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

pub use decision::Decision;
pub use error::{Error, ErrorLocation, Result, TextPosition, TextRange};
pub use parser::PolicyParser;
pub use permissions::{Effect, PermissionSet, Rule as PermissionRule, auto_approve};
pub use policy::{Evaluation, MatchOptions, Policy, RequirementsExecPolicy};
pub use read_only::{check_read_only, is_known_safe_command, is_read_only_command};
pub use rule::{
    NetworkRule, NetworkRuleProtocol, PatternToken, PrefixPattern, PrefixRule, Rule, RuleMatch,
    RuleRef,
};

const DEFAULT_RULES: &str = r#"
prefix_rule(pattern = ["shutdown"], decision = "forbidden", justification = "system shutdown")
prefix_rule(pattern = ["reboot"], decision = "forbidden", justification = "system reboot")
prefix_rule(pattern = ["poweroff"], decision = "forbidden", justification = "system poweroff")
prefix_rule(pattern = ["diskpart"], decision = "forbidden", justification = "disk partitioning")
prefix_rule(pattern = ["format"], decision = "forbidden", justification = "disk formatting")
prefix_rule(pattern = ["bcdedit"], decision = "forbidden", justification = "boot configuration changes")
prefix_rule(pattern = ["vssadmin", "delete", "shadows"], decision = "forbidden", justification = "shadow-copy deletion")
prefix_rule(pattern = ["wbadmin", "delete"], decision = "forbidden", justification = "backup deletion")
prefix_rule(pattern = ["cipher", "/w"], decision = "forbidden", justification = "free-space wiping")
prefix_rule(pattern = ["reg", "delete"], decision = "forbidden", justification = "registry deletion")
prefix_rule(pattern = ["takeown", "/f", "C:\\"], decision = "forbidden", justification = "system ownership changes")
prefix_rule(pattern = ["icacls", "C:\\"], decision = "forbidden", justification = "system ACL changes")
prefix_rule(pattern = ["git", "reset", "--hard"], decision = "forbidden", justification = "destructive git reset")
prefix_rule(pattern = ["git", "clean", "-fdx"], decision = "forbidden", justification = "destructive git clean")
prefix_rule(pattern = ["npm", "publish"], decision = "prompt", justification = "publishes a package")
prefix_rule(pattern = ["docker", "system", "prune"], decision = "forbidden", justification = "container data deletion")
prefix_rule(pattern = ["docker", "volume", "prune"], decision = "forbidden", justification = "volume data deletion")
prefix_rule(pattern = ["rm", "-rf", "/"], decision = "forbidden", justification = "filesystem-root deletion")
prefix_rule(pattern = ["rm", "-fr", "/"], decision = "forbidden", justification = "filesystem-root deletion")
prefix_rule(pattern = ["rm", "-rf", "~"], decision = "forbidden", justification = "home-directory deletion")
prefix_rule(pattern = ["rm", "-rf", "C:\\"], decision = "forbidden", justification = "system-drive deletion")
prefix_rule(pattern = ["env"], decision = "forbidden", justification = "secret exfiltration: process environment may hold provider keys")
prefix_rule(pattern = ["printenv"], decision = "forbidden", justification = "secret exfiltration: process environment may hold provider keys")
"#;

const POLICY_FILE_NAME: &str = "execpolicy.star";

#[derive(Debug, Clone)]
pub struct PolicyDecision {
    pub decision: Decision,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct PolicyStatus {
    pub path: Option<PathBuf>,
    pub loaded: bool,
    pub rules: usize,
    pub network_rules: usize,
    pub host_executables: usize,
    pub error: Option<String>,
}

struct RuntimePolicy {
    policy: Arc<Policy>,
    root: Option<PathBuf>,
    path: Option<PathBuf>,
    status: PolicyStatus,
}

static RUNTIME: OnceLock<RwLock<RuntimePolicy>> = OnceLock::new();
/// Serializes policy-file appends inside this process. The file itself is
/// locked as well, so a second server cannot interleave a rule line.
static APPEND_LOCK: Mutex<()> = Mutex::new(());

fn runtime() -> &'static RwLock<RuntimePolicy> {
    RUNTIME.get_or_init(|| {
        RwLock::new(RuntimePolicy {
            policy: Arc::new(build_default_policy()),
            root: None,
            path: None,
            status: PolicyStatus::default(),
        })
    })
}

fn build_default_policy() -> Policy {
    let mut parser = PolicyParser::new();
    parser
        .parse("stark-default.rules", DEFAULT_RULES)
        .expect("default policy rules must parse");
    parser.build()
}

fn build_policy_with_file(path: &Path) -> std::result::Result<Policy, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut parser = PolicyParser::new();
    parser
        .parse("stark-default.rules", DEFAULT_RULES)
        .map_err(|error| error.to_string())?;
    parser
        .parse(&path.to_string_lossy(), &text)
        .map_err(|error| error.to_string())?;
    Ok(parser.build())
}

fn policy_counts(policy: &Policy) -> (usize, usize, usize) {
    let rules = policy
        .rules()
        .iter_all()
        .map(|(_, rules)| rules.len())
        .sum::<usize>();
    (
        rules,
        policy.network_rules().len(),
        policy.host_executables().len(),
    )
}

fn set_runtime(
    policy: Arc<Policy>,
    root: Option<PathBuf>,
    path: Option<PathBuf>,
    status: PolicyStatus,
) {
    let mut guard = runtime()
        .write()
        .unwrap_or_else(|poison| poison.into_inner());
    guard.policy = policy;
    guard.root = root;
    guard.path = path;
    guard.status = status;
}

pub fn default_policy() -> Arc<Policy> {
    runtime()
        .read()
        .unwrap_or_else(|poison| poison.into_inner())
        .policy
        .clone()
}

/// Loads the harness execution policy once at startup. The default destructive
/// rules stay active even when the file is missing or invalid.
pub fn configure(harness_root: &Path) -> PolicyStatus {
    let path = std::env::var("STARK_EXECPOLICY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| harness_root.join(".deep_agents").join(POLICY_FILE_NAME));

    let (policy, error) = if path.is_file() {
        match build_policy_with_file(&path) {
            Ok(policy) => (Arc::new(policy), None),
            Err(error) => (Arc::new(build_default_policy()), Some(error)),
        }
    } else {
        (Arc::new(build_default_policy()), None)
    };
    let loaded = path.is_file() && error.is_none();
    let (rules, network_rules, host_executables) = policy_counts(&policy);
    let status = PolicyStatus {
        path: Some(path.clone()),
        loaded,
        rules,
        network_rules,
        host_executables,
        error,
    };
    set_runtime(
        policy,
        Some(harness_root.to_path_buf()),
        Some(path),
        status.clone(),
    );
    status
}

/// Re-reads the policy file last passed to [`configure`].
pub fn reload() -> PolicyStatus {
    let root = runtime()
        .read()
        .unwrap_or_else(|poison| poison.into_inner())
        .root
        .clone();
    let Some(root) = root else {
        return runtime()
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .status
            .clone();
    };
    configure(&root)
}

pub fn evaluate_command(command: &str) -> PolicyDecision {
    let Some(tokens) = shlex::split(command) else {
        return PolicyDecision {
            decision: Decision::Forbidden,
            reason: Some("command could not be tokenized".to_string()),
        };
    };
    let policy = default_policy();
    let options = MatchOptions {
        resolve_host_executables: !policy.host_executables().is_empty(),
    };
    let evaluation = policy.check_with_options(&tokens, &heuristics_decision, &options);
    let decision = decision_with_allow_override(&evaluation);
    let reason = evaluation.matched_rules.iter().find_map(|rule| match rule {
        RuleMatch::PrefixRuleMatch { justification, .. } => justification.clone(),
        RuleMatch::HeuristicsRuleMatch { .. } => Some("destructive command heuristic".to_string()),
    });
    PolicyDecision { decision, reason }
}

fn decision_with_allow_override(evaluation: &Evaluation) -> Decision {
    let has_forbidden = evaluation
        .matched_rules
        .iter()
        .any(|rule| rule.decision() == Decision::Forbidden);
    let has_allow = evaluation
        .matched_rules
        .iter()
        .any(|rule| rule.decision() == Decision::Allow);
    // A persisted "Always allow" prefix may override a built-in `prompt` rule
    // (for example npm publish), but never a forbidden rule.
    if !has_forbidden && has_allow {
        Decision::Allow
    } else {
        evaluation.decision
    }
}

/// Evaluates a host/protocol pair against the loaded network rules.
/// `default_decision` is used when no rule matches — callers pass `Forbidden`
/// when host network access is disabled, `Allow` when it is enabled.
pub fn evaluate_network(
    host: &str,
    protocol: NetworkRuleProtocol,
    default_decision: Decision,
) -> PolicyDecision {
    default_policy().check_network(host, protocol, default_decision)
}

fn append_policy_line(line: &str) -> std::result::Result<(), String> {
    let path = runtime()
        .read()
        .unwrap_or_else(|poison| poison.into_inner())
        .path
        .clone()
        .ok_or_else(|| {
            "no execution policy is configured; set STARK_EXECPOLICY or start the server"
                .to_string()
        })?;
    append_policy_line_at(&path, line)?;
    let status = reload();
    if let Some(error) = status.error {
        return Err(format!("policy saved but reload failed: {error}"));
    }
    Ok(())
}

fn append_policy_line_at(path: &Path, line: &str) -> std::result::Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let _process_guard = APPEND_LOCK
        .lock()
        .map_err(|_| "policy append lock is poisoned".to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    file.lock()
        .map_err(|error| format!("cannot lock {}: {error}", path.display()))?;

    let mut contents = String::new();
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.read_to_string(&mut contents))
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if !contents.lines().any(|existing| existing == line) {
        if !contents.is_empty() && !contents.ends_with('\n') {
            file.write_all(b"\n")
                .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        }
        file.write_all(line.as_bytes())
            .and_then(|_| file.write_all(b"\n"))
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        file.flush()
            .map_err(|error| format!("cannot flush {}: {error}", path.display()))?;
    }
    let _ = file.unlock();
    drop(file);
    Ok(())
}

/// Adds an `allow` prefix rule to the harness policy file and reloads it.
/// Callers should only persist a prefix the user explicitly approved.
pub fn append_allow_prefix(prefix: &[String]) -> std::result::Result<(), String> {
    if prefix.is_empty() {
        return Err("prefix rule requires at least one token".to_string());
    }
    let tokens = prefix
        .iter()
        .map(|token| serde_json::to_string(token).map_err(|error| error.to_string()))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let line = format!(
        "prefix_rule(pattern = [{}], decision = \"allow\")",
        tokens.join(", ")
    );
    append_policy_line(&line)
}

/// Adds a network rule to the harness policy file and reloads it.
pub fn append_network_rule(
    host: &str,
    protocol: NetworkRuleProtocol,
    decision: Decision,
    justification: Option<&str>,
) -> std::result::Result<(), String> {
    let host = rule::normalize_network_rule_host(host).map_err(|error| error.to_string())?;
    let decision = match decision {
        Decision::Allow => "allow",
        Decision::Prompt => "prompt",
        Decision::Forbidden => "deny",
    };
    let host = serde_json::to_string(&host).map_err(|error| error.to_string())?;
    let protocol =
        serde_json::to_string(protocol.as_policy_string()).map_err(|error| error.to_string())?;
    let mut line = format!("network_rule(host={host}, protocol={protocol}, decision={decision})");
    if let Some(justification) = justification {
        let justification =
            serde_json::to_string(justification).map_err(|error| error.to_string())?;
        line = format!(
            "network_rule(host={host}, protocol={protocol}, decision={decision}, justification={justification})"
        );
    }
    append_policy_line(&line)
}

fn heuristics_decision(tokens: &[String]) -> Decision {
    let normalized = tokens.join(" ").to_lowercase();
    // Secret exfiltration: environment dumps and direct .env reads are denied
    // at the policy layer, not just in the read tool. `.env.example` stays
    // allowed (it is a template, not a secret).
    if tokens.len() == 1 && tokens[0].eq_ignore_ascii_case("set") {
        return Decision::Forbidden;
    }
    if tokens.first().is_some_and(|program| {
        program.eq_ignore_ascii_case("env") || program.eq_ignore_ascii_case("printenv")
    }) {
        return Decision::Forbidden;
    }
    if normalized == "export -p" || normalized.starts_with("export -p ") {
        return Decision::Forbidden;
    }
    if normalized.contains(".env") && !normalized.contains(".env.example") {
        return Decision::Forbidden;
    }
    if normalized.contains("rm -rf /")
        || normalized.contains("rm -fr /")
        || normalized.contains("rm -rf ~")
        || normalized.contains("set-executionpolicy unrestricted")
        || normalized.contains("set-executionpolicy bypass")
        || normalized.contains("invoke-expression")
        || normalized.contains("iex (")
        || normalized.contains("> c:\\")
        || normalized.contains(">> c:\\")
    {
        return Decision::Forbidden;
    }
    if (normalized.contains("curl ") || normalized.contains("wget "))
        && (normalized.contains("| bash")
            || normalized.contains("| sh")
            || normalized.contains("| zsh"))
    {
        return Decision::Forbidden;
    }
    Decision::Allow
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_allows_normal_commands() {
        assert_eq!(evaluate_command("ls -la").decision, Decision::Allow);
        assert_eq!(evaluate_command("cargo test").decision, Decision::Allow);
    }

    #[test]
    fn policy_forbids_destructive_commands() {
        assert_eq!(
            evaluate_command("shutdown /s").decision,
            Decision::Forbidden
        );
        assert_eq!(
            evaluate_command("git reset --hard HEAD").decision,
            Decision::Forbidden
        );
        assert_eq!(evaluate_command("rm -rf /").decision, Decision::Forbidden);
    }

    #[test]
    fn policy_prompts_for_publish() {
        assert_eq!(evaluate_command("npm publish").decision, Decision::Prompt);
    }

    #[test]
    fn policy_file_parses_prefix_and_network_rules() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("execpolicy.star");
        std::fs::write(
            &path,
            r#"
prefix_rule(pattern = ["mytool", "deploy"], decision = "forbidden", justification = "no deploys")
prefix_rule(pattern = ["publish"], decision = "prompt", justification = "asks first")
prefix_rule(pattern = ["publish"], decision = "allow")
network_rule(host = "example.com", protocol = "https", decision = "forbidden", justification = "blocked")
network_rule(host = "api.example.com", protocol = "https", decision = "allow")
"#,
        )
        .unwrap();
        let policy = build_policy_with_file(&path).unwrap();
        assert_eq!(
            policy
                .check(&["mytool".to_string(), "deploy".to_string()], &|_| {
                    Decision::Allow
                })
                .decision,
            Decision::Forbidden
        );
        let prompt = policy.check(&["publish".to_string()], &|_| Decision::Allow);
        assert_eq!(prompt.decision, Decision::Prompt);
        assert_eq!(decision_with_allow_override(&prompt), Decision::Allow);
        assert_eq!(
            policy
                .check_network("example.com", NetworkRuleProtocol::Https, Decision::Allow)
                .decision,
            Decision::Forbidden
        );
        assert_eq!(
            policy
                .check_network(
                    "api.example.com",
                    NetworkRuleProtocol::Https,
                    Decision::Allow
                )
                .decision,
            Decision::Allow
        );
        assert_eq!(
            policy
                .check_network("other.example", NetworkRuleProtocol::Https, Decision::Allow)
                .decision,
            Decision::Allow
        );
    }

    #[test]
    fn policy_append_is_idempotent_and_preserves_existing_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("execpolicy.star");
        std::fs::write(
            &path,
            "prefix_rule(pattern = [\"existing\"], decision = \"allow\")",
        )
        .unwrap();
        let line = "prefix_rule(pattern = [\"new\"], decision = \"allow\")";
        append_policy_line_at(&path, line).unwrap();
        append_policy_line_at(&path, line).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("existing"));
        assert_eq!(text.matches(line).count(), 1);
    }

    #[test]
    fn policy_denies_secret_exfiltration() {
        assert_eq!(evaluate_command("env").decision, Decision::Forbidden);
        assert_eq!(
            evaluate_command("env | grep KEY").decision,
            Decision::Forbidden
        );
        assert_eq!(evaluate_command("printenv").decision, Decision::Forbidden);
        assert_eq!(
            evaluate_command("printenv OPENCODE_API_KEY").decision,
            Decision::Forbidden
        );
        assert_eq!(evaluate_command("set").decision, Decision::Forbidden);
        assert_eq!(evaluate_command("export -p").decision, Decision::Forbidden);
        assert_eq!(evaluate_command("cat .env").decision, Decision::Forbidden);
        assert_eq!(
            evaluate_command("cat D:\\StarkData\\.env").decision,
            Decision::Forbidden
        );
        // Templates stay allowed.
        assert_ne!(
            evaluate_command("cat .env.example").decision,
            Decision::Forbidden
        );
        assert_eq!(evaluate_command("ls -la").decision, Decision::Allow);
    }
}
