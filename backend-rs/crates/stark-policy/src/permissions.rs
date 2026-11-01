//! OpenCode-style permission rules.
//!
//! Rules are `(action, resource, effect)` triples evaluated last-match-wins.
//! An operation may touch several resources; **any deny wins, then any ask**,
//! otherwise it is allowed. Defaults match OpenCode: most actions are allowed,
//! `external_directory` and `doom_loop` ask, and `.env` reads ask.

use std::path::Path;

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    Allow,
    Ask,
    Deny,
}

impl Effect {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "allow" => Some(Self::Allow),
            "ask" => Some(Self::Ask),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Ask => "ask",
            Self::Deny => "deny",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Rule {
    pub action: String,
    pub resource: String,
    pub effect: Effect,
}

#[derive(Clone, Debug, Default)]
pub struct PermissionSet {
    rules: Vec<Rule>,
}

impl PermissionSet {
    pub fn from_json(value: &Value) -> Self {
        let mut rules = Vec::new();
        if let Some(items) = value.get("permissions").and_then(Value::as_array) {
            for item in items {
                let Some(action) = item.get("action").and_then(Value::as_str) else {
                    continue;
                };
                let resource = item.get("resource").and_then(Value::as_str).unwrap_or("*");
                let Some(effect) = item
                    .get("effect")
                    .and_then(Value::as_str)
                    .and_then(Effect::parse)
                else {
                    continue;
                };
                rules.push(Rule {
                    action: action.to_ascii_lowercase(),
                    resource: resource.to_string(),
                    effect,
                });
            }
        }
        Self { rules }
    }

    /// Load `.deep_agents/permissions.json` under the harness root. Missing or
    /// invalid files fall back to the default (empty) set.
    pub fn load(harness_root: &Path) -> Self {
        Self::read(harness_root)
            .map(|value| Self::from_json(&value))
            .unwrap_or_default()
    }

    /// Load global rules and append the matching agent's rules, so agent rules
    /// win where they overlap (OpenCode merge order).
    pub fn load_for(harness_root: &Path, scope: &str) -> Self {
        let Some(value) = Self::read(harness_root) else {
            return Self::default();
        };
        let mut set = Self::from_json(&value);
        let scope = scope.to_ascii_lowercase();
        if let Some(agents) = value.get("agents").and_then(Value::as_object) {
            let mut append = |agent: &Value| {
                let extra = Self::from_json(agent);
                set.rules.extend(extra.rules);
            };
            if let Some(agent) = agents.get(&scope) {
                append(agent);
            }
            if let Some(agent) = agents.get("*") {
                append(agent);
            }
        }
        set
    }

    fn read(harness_root: &Path) -> Option<Value> {
        let path = harness_root.join(".deep_agents").join("permissions.json");
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str::<Value>(&text).ok()
    }

    /// Evaluate one operation over all its resources.
    pub fn evaluate(&self, action: &str, resources: &[String]) -> Effect {
        let action = action.to_ascii_lowercase();
        let mut verdict = Effect::Allow;
        for resource in resources {
            match self.rule_effect(&action, resource) {
                Effect::Deny => return Effect::Deny,
                Effect::Ask => verdict = Effect::Ask,
                Effect::Allow => {}
            }
        }
        verdict
    }

    fn rule_effect(&self, action: &str, resource: &str) -> Effect {
        // Last matching rule wins.
        let mut matched: Option<Effect> = None;
        for rule in &self.rules {
            if action_matches(&rule.action, action)
                && resource_matches(&rule.resource, resource, action)
            {
                matched = Some(rule.effect);
            }
        }
        if let Some(effect) = matched {
            return effect;
        }
        default_effect(action, resource)
    }
}

/// Auto-approve asks (never denies), unless the user opted out.
pub fn auto_approve(harness_root: &Path) -> bool {
    if let Ok(value) = std::env::var("STARK_AUTO_APPROVE") {
        return matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        );
    }
    PermissionSet::read(harness_root)
        .and_then(|value| value.get("auto_approve").and_then(Value::as_bool))
        .unwrap_or(false)
}

fn default_effect(action: &str, resource: &str) -> Effect {
    match action {
        "external_directory" | "doom_loop" => Effect::Ask,
        "read" if is_env_file(resource) => Effect::Ask,
        _ => Effect::Allow,
    }
}

fn is_env_file(resource: &str) -> bool {
    let name = resource
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(resource)
        .to_ascii_lowercase();
    (name == ".env" || name.starts_with(".env.")) && !name.ends_with(".example")
}

fn action_matches(pattern: &str, action: &str) -> bool {
    wildcard_match(
        &pattern.to_ascii_lowercase(),
        &action.to_ascii_lowercase(),
        true,
    )
}

fn resource_matches(pattern: &str, resource: &str, action: &str) -> bool {
    // A bare `*` is the universal catch-all, even for paths with separators.
    if pattern == "*" {
        return true;
    }
    let pattern = expand_home(pattern);
    let resource = expand_home(resource);
    // Commands are whole strings; paths use glob semantics (`*` stays within a
    // segment, `**` crosses directories), matching OpenCode's examples.
    let command_like = matches!(action, "bash" | "doom_loop");
    wildcard_match(
        &pattern.replace('\\', "/").to_ascii_lowercase(),
        &resource.replace('\\', "/").to_ascii_lowercase(),
        command_like,
    )
}

fn expand_home(value: &str) -> String {
    if let Some(rest) = value.strip_prefix("~/")
        && let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))
    {
        return format!("{}/{}", home.to_string_lossy().replace('\\', "/"), rest);
    }
    value.to_string()
}

/// `*` matches any run (or one segment when `cross_separator` is false), `?`
/// one character, `**` any run including separators; everything else is
/// literal. Small backtracking matcher — patterns and inputs are short.
fn wildcard_match(pattern: &str, value: &str, cross_separator: bool) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let value: Vec<char> = value.chars().collect();
    matches_at(&pattern, &value, cross_separator)
}

fn matches_at(pattern: &[char], value: &[char], cross: bool) -> bool {
    let Some(&head) = pattern.first() else {
        return value.is_empty();
    };
    match head {
        '*' => {
            let double = pattern.get(1) == Some(&'*');
            let rest = if double { &pattern[2..] } else { &pattern[1..] };
            let can_cross = cross || double;
            if matches_at(rest, value, cross) {
                return true;
            }
            for index in 0..value.len() {
                if !can_cross && value[index] == '/' {
                    break;
                }
                if matches_at(rest, &value[index + 1..], cross) {
                    return true;
                }
            }
            false
        }
        '?' => !value.is_empty() && matches_at(&pattern[1..], &value[1..], cross),
        literal => {
            !value.is_empty()
                && value[0] == literal
                && matches_at(&pattern[1..], &value[1..], cross)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn set(rules: Value) -> PermissionSet {
        PermissionSet::from_json(&json!({ "permissions": rules }))
    }

    #[test]
    fn last_matching_rule_wins() {
        let set = set(json!([
            {"action": "bash", "resource": "*", "effect": "ask"},
            {"action": "bash", "resource": "git *", "effect": "allow"},
            {"action": "bash", "resource": "rm *", "effect": "deny"}
        ]));
        assert_eq!(
            set.evaluate("bash", &["git status".to_string()]),
            Effect::Allow
        );
        assert_eq!(
            set.evaluate("bash", &["rm -rf build".to_string()]),
            Effect::Deny
        );
        assert_eq!(set.evaluate("bash", &["ls".to_string()]), Effect::Ask);
    }

    #[test]
    fn deny_wins_across_resources() {
        let set = set(json!([
            {"action": "edit", "resource": "src/**", "effect": "allow"},
            {"action": "edit", "resource": "secrets/*", "effect": "deny"}
        ]));
        assert_eq!(
            set.evaluate(
                "edit",
                &["src/main.rs".to_string(), "secrets/key".to_string()]
            ),
            Effect::Deny
        );
    }

    #[test]
    fn defaults_match_opencode() {
        let set = set(json!([]));
        assert_eq!(set.evaluate("bash", &["ls".to_string()]), Effect::Allow);
        assert_eq!(
            set.evaluate("external_directory", &["C:/other".to_string()]),
            Effect::Ask
        );
        assert_eq!(set.evaluate("doom_loop", &["x".to_string()]), Effect::Ask);
        assert_eq!(set.evaluate("read", &[".env".to_string()]), Effect::Ask);
        assert_eq!(
            set.evaluate("read", &[".env.example".to_string()]),
            Effect::Allow
        );
    }

    #[test]
    fn agent_rules_override_global() {
        let value = json!({
            "permissions": [
                {"action": "edit", "resource": "*", "effect": "allow"}
            ],
            "agents": {
                "explore": {
                    "permissions": [
                        {"action": "edit", "resource": "*", "effect": "deny"}
                    ]
                }
            }
        });
        let global = PermissionSet::from_json(&value);
        assert_eq!(
            global.evaluate("edit", &["src/main.rs".to_string()]),
            Effect::Allow
        );
        let mut merged = PermissionSet::from_json(&value);
        let agent = value["agents"]["explore"].clone();
        merged.rules.extend(PermissionSet::from_json(&agent).rules);
        assert_eq!(
            merged.evaluate("edit", &["src/main.rs".to_string()]),
            Effect::Deny
        );
    }

    #[test]
    fn load_for_merges_agent_rules_from_disk() {
        let dir = std::env::temp_dir().join(format!("stark-perm-{}", std::process::id()));
        let agents = dir.join(".deep_agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("permissions.json"),
            r#"{
                "permissions": [{"action":"edit","resource":"*","effect":"allow"}],
                "agents": {"explore": {"permissions": [{"action":"edit","resource":"*","effect":"deny"}]}}
            }"#,
        )
        .unwrap();
        let global = PermissionSet::load(&dir);
        assert_eq!(
            global.evaluate("edit", &["src/main.rs".to_string()]),
            Effect::Allow
        );
        let explore = PermissionSet::load_for(&dir, "explore");
        assert_eq!(
            explore.evaluate("edit", &["src/main.rs".to_string()]),
            Effect::Deny
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wildcards_and_paths() {
        let set = set(json!([
            {"action": "read", "resource": "src/*.rs", "effect": "deny"}
        ]));
        assert_eq!(
            set.evaluate("read", &["src/main.rs".to_string()]),
            Effect::Deny
        );
        assert_ne!(
            set.evaluate("read", &["src/nested/main.rs".to_string()]),
            Effect::Deny
        );
    }
}
