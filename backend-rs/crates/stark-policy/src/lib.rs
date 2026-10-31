mod decision;
mod error;
mod executable_name;
mod parser;
mod permissions;
mod policy;
mod read_only;
pub mod rule;

use std::sync::OnceLock;

pub use decision::Decision;
pub use error::{Error, ErrorLocation, Result, TextPosition, TextRange};
pub use parser::PolicyParser;
pub use permissions::{Effect, PermissionSet, Rule as PermissionRule};
pub use policy::{Evaluation, MatchOptions, Policy, RequirementsExecPolicy};
pub use read_only::{check_read_only, is_read_only_command};
pub use rule::{PatternToken, PrefixPattern, PrefixRule, Rule, RuleMatch, RuleRef};

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
"#;

static DEFAULT_POLICY: OnceLock<Policy> = OnceLock::new();

#[derive(Debug, Clone)]
pub struct PolicyDecision {
    pub decision: Decision,
    pub reason: Option<String>,
}

pub fn default_policy() -> &'static Policy {
    DEFAULT_POLICY.get_or_init(|| {
        let mut parser = PolicyParser::new();
        parser
            .parse("stark-default.rules", DEFAULT_RULES)
            .expect("default policy rules must parse");
        parser.build()
    })
}

pub fn evaluate_command(command: &str) -> PolicyDecision {
    let Some(tokens) = shlex::split(command) else {
        return PolicyDecision {
            decision: Decision::Forbidden,
            reason: Some("command could not be tokenized".to_string()),
        };
    };
    let evaluation = default_policy().check(&tokens, &heuristics_decision);
    let reason = evaluation.matched_rules.iter().find_map(|rule| match rule {
        RuleMatch::PrefixRuleMatch { justification, .. } => justification.clone(),
        RuleMatch::HeuristicsRuleMatch { .. } => Some("destructive command heuristic".to_string()),
    });
    PolicyDecision {
        decision: evaluation.decision,
        reason,
    }
}

fn heuristics_decision(tokens: &[String]) -> Decision {
    let normalized = tokens.join(" ").to_lowercase();
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
}
