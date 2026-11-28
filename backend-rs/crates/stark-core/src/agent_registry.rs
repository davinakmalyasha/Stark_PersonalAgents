//! Runtime agent registry.
//!
//! Built-ins are compiled [`AgentSpec`] seeds. Custom agents are validated
//! records under `.deep_agents/config/agents/*.json`. The registry resolves
//! both into one [`EffectiveAgent`] shape, applies compiled caps, and is the
//! single source of truth for the picker, run selection, tools, access,
//! prompts, delegation, and schedules.
//!
//! Safety rules:
//! - Built-in access/prompt/tool caps cannot be widened by config.
//! - Custom agents have a closed tool list. Missing tools is invalid, never
//!   "all tools"; an explicit empty list is a talk-only agent.
//! - Custom MCP/skills default to none, not all.
//! - Invalid definitions are quarantined, never partially applied.
//! - Custom Code Mode is opt-in hybrid and requires the explicit `execute`
//!   tool; custom `only`/`full_codemode` stay built-in Developer features.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use stark_config::{AgentFile, ConfigStore, EffectiveAgent};
use stark_tools::contract;

use crate::agents::{AgentSpec, FileAccess, access_from_str, for_scope, workspace_agents};

const RESERVED_KEYS: &[&str] = &["agent", "default", "execute", "model", "all", "none"];

#[derive(Clone, Debug, Default)]
pub struct AgentRegistry {
    agents: Vec<EffectiveAgent>,
    by_key: HashMap<String, usize>,
    aliases: HashMap<String, String>,
    issues: Vec<String>,
    warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct RouteDecision {
    pub agent: EffectiveAgent,
    /// Where the decision came from, e.g. `workspace_kind:developer`.
    pub matched: Option<String>,
    /// Set only when multiple custom routing rules matched; the built-in
    /// fallback is used.
    pub warning: Option<String>,
}

impl Default for RouteDecision {
    fn default() -> Self {
        Self {
            agent: stark_config::builtin_effective("workspace"),
            matched: None,
            warning: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct DefinitionCheck {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl DefinitionCheck {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

impl AgentRegistry {
    pub fn load(config: &ConfigStore, harness_root: &Path) -> Self {
        let mut registry = Self::default();
        let saved_files = config.agent_files();
        // v1 compatibility: an old file named after a built-in alias
        // (`coder.json`) is an override for the canonical built-in
        // (`developer`). An explicit canonical file wins.
        let mut alias_overrides: HashMap<String, AgentFile> = HashMap::new();
        for (key, file) in &saved_files {
            if let Some(spec) = workspace_agents()
                .iter()
                .find(|spec| spec.aliases.contains(&key.as_str()))
            {
                alias_overrides
                    .entry(spec.key.to_string())
                    .or_insert_with(|| file.clone());
            }
        }
        // Built-ins first: their keys and aliases win every collision.
        for spec in workspace_agents() {
            let raw_present = config.agent(spec.key).is_some();
            if raw_present && alias_overrides.contains_key(spec.key) {
                registry.issues.push(format!(
                    "{}: canonical override wins; legacy alias override ignored",
                    spec.key
                ));
            }
            let file = config
                .agent(spec.key)
                .or_else(|| alias_overrides.get(spec.key).cloned())
                .and_then(|file| {
                    if let Err(error) = config.validate_agent(&file) {
                        registry
                            .issues
                            .push(format!("{}: invalid override ignored: {error}", spec.key));
                        return None;
                    }
                    let errors = builtin_override_errors(spec, &file);
                    if !errors.is_empty() {
                        registry.issues.push(format!(
                            "{}: override ignored: {}",
                            spec.key,
                            errors.join("; ")
                        ));
                        return None;
                    }
                    Some(file)
                });
            let fallback = AgentFile::default();
            let selected = file.as_ref().unwrap_or(&fallback);
            let mut effective = config
                .effective_for_file(spec.key, spec.tier, selected)
                .unwrap_or_else(|error| {
                    registry.issues.push(format!("{}: {error}", spec.key));
                    stark_config::builtin_effective(spec.tier)
                });
            registry.apply_builtin(*spec, file.as_ref(), &mut effective);
            registry.insert(effective);
        }
        // Custom definitions. Built-in overrides and legacy alias files were
        // consumed above.
        for (key, file) in &saved_files {
            if workspace_agents()
                .iter()
                .any(|spec| spec.key == key.as_str())
                || workspace_agents()
                    .iter()
                    .any(|spec| spec.aliases.contains(&key.as_str()))
            {
                continue;
            }
            if let Err(error) = registry.apply_custom_key(key) {
                registry.issues.push(format!("{key}: {error}"));
                continue;
            }
            if let Err(error) = config.validate_agent(file) {
                registry
                    .issues
                    .push(format!("{key}: invalid definition ignored: {error}"));
                continue;
            }
            let mut effective = match config.effective(key, "workspace") {
                Ok(effective) => effective,
                Err(error) => {
                    registry.issues.push(format!("{key}: {error}"));
                    continue;
                }
            };
            match registry.apply_custom(harness_root, key, file, &mut effective) {
                Ok(()) => registry.insert(effective),
                Err(error) => registry.issues.push(format!("{key}: {error}")),
            }
        }
        registry.rebuild_index();
        registry
    }

    /// Convenience for core paths that only have the harness root.
    pub fn open(harness_root: &Path) -> Self {
        let config = ConfigStore::open(harness_root);
        Self::load(&config, harness_root)
    }

    pub fn list(&self) -> &[EffectiveAgent] {
        &self.agents
    }

    /// Agents the picker should show, in UI order.
    pub fn visible(&self) -> Vec<&EffectiveAgent> {
        let mut agents: Vec<&EffectiveAgent> = self
            .agents
            .iter()
            .filter(|agent| agent.ui_visible)
            .collect();
        agents.sort_by(|left, right| {
            left.ui_order
                .cmp(&right.ui_order)
                .then_with(|| left.key.cmp(&right.key))
        });
        agents
    }

    pub fn get(&self, key: &str) -> Option<&EffectiveAgent> {
        let key = key.trim().to_ascii_lowercase();
        if let Some(index) = self.by_key.get(&key) {
            return self.agents.get(*index);
        }
        let canonical = self.aliases.get(&key)?;
        self.by_key
            .get(canonical)
            .and_then(|index| self.agents.get(*index))
    }

    pub fn canonical_key(&self, key: &str) -> Option<String> {
        self.get(key).map(|agent| agent.key.clone())
    }

    pub fn issues(&self) -> &[String] {
        &self.issues
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Route a request when the client did not name an agent. The decision
    /// keeps the warning (ambiguous routing) so the caller can log/expose it.
    pub fn route_decision(
        &self,
        workspace_kind: &str,
        workspace_path: &Path,
        harness_root: &Path,
    ) -> RouteDecision {
        let kind = workspace_kind.trim().to_ascii_lowercase();
        // An explicit custom key/alias wins.
        if !kind.is_empty()
            && let Some(agent) = self.get(&kind)
            && !agent.builtin
        {
            return RouteDecision {
                agent: agent.clone(),
                matched: Some(format!("workspace_kind:{kind}")),
                warning: None,
            };
        }
        // Explicit built-in keys and virtual workspace kinds win over custom
        // routing. Generic aliases like "coding" do not.
        if is_explicit_builtin_kind(&kind) {
            return RouteDecision {
                agent: self.builtin_route(&kind, workspace_path, harness_root),
                matched: Some(format!("workspace_kind:{kind}")),
                warning: None,
            };
        }
        let matches: Vec<&EffectiveAgent> = self
            .agents
            .iter()
            .filter(|agent| !agent.builtin && agent.ui_visible)
            .filter(|agent| routing_matches(agent, &kind, workspace_path))
            .collect();
        if matches.len() == 1 {
            let agent = matches[0].clone();
            return RouteDecision {
                agent,
                matched: Some(format!("routing:{kind}")),
                warning: None,
            };
        }
        let warning = (matches.len() > 1).then(|| {
            format!(
                "ambiguous agent routing for workspace kind '{kind}' and path '{}': {}",
                workspace_path.display(),
                matches
                    .iter()
                    .map(|agent| agent.key.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        });
        let agent = self.builtin_route(&kind, workspace_path, harness_root);
        RouteDecision {
            agent,
            matched: None,
            warning,
        }
    }

    /// Compatibility wrapper for callers that do not need the warning.
    pub fn route(
        &self,
        workspace_kind: &str,
        workspace_path: &Path,
        harness_root: &Path,
    ) -> EffectiveAgent {
        self.route_decision(workspace_kind, workspace_path, harness_root)
            .agent
    }

    fn builtin_route(
        &self,
        kind: &str,
        workspace_path: &Path,
        harness_root: &Path,
    ) -> EffectiveAgent {
        if !kind.is_empty()
            && let Some(agent) = self.get(kind)
            && agent.builtin
        {
            return agent.clone();
        }
        let key = match kind {
            "house" | "house_workspace" => "house",
            "personal" | "personal_workspace" | "general_workspace" => "general",
            "dfd" | "dfd_workspace" => "dfd",
            "design" | "design_workspace" => "ui",
            _ if workspace_path != harness_root => "developer",
            _ => "orchestrator",
        };
        self.get(key)
            .cloned()
            .unwrap_or_else(|| builtin_runtime(key, self))
    }

    fn insert(&mut self, effective: EffectiveAgent) {
        if self
            .by_key
            .get(&effective.key)
            .is_some_and(|index| self.agents.get(*index).is_some())
        {
            self.issues
                .push(format!("{}: duplicate agent key", effective.key));
            return;
        }
        let index = self.agents.len();
        self.by_key.insert(effective.key.clone(), index);
        self.agents.push(effective);
    }

    fn apply_builtin(
        &mut self,
        spec: AgentSpec,
        file: Option<&AgentFile>,
        effective: &mut EffectiveAgent,
    ) {
        let key = spec.key;
        effective.key = key.to_string();
        effective.builtin = true;
        effective.aliases = spec.aliases.iter().map(|alias| alias.to_string()).collect();
        effective.access = spec.access_root.as_str().to_string();
        effective.access_spawned = spec.access_spawned.as_str().to_string();
        effective.prompt_profile = spec.prompt_profile.to_string();
        effective.delegatable = spec.delegatable;
        effective.can_delegate = spec.can_delegate;
        effective.can_spawn = spec.can_spawn;
        effective.ui_order = spec.ui_order;
        effective.routing_workspace_kinds.clear();
        effective.routing_paths.clear();
        // Start from the compiled seed. The optional override applies on top,
        // so an ignored/invalid override can never leak a field through.
        effective.display = spec.display.to_string();
        effective.description = spec.description.to_string();
        if let Some(file) = file {
            if !file.display.trim().is_empty() {
                effective.display = file.display.trim().to_string();
            }
            if !file.description.trim().is_empty() {
                effective.description = file.description.trim().to_string();
            }
            if let Some(access) = file
                .access
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                let requested = normalize_access(access);
                let cap = spec.access_root.as_str();
                if requested.is_some_and(|requested| access_rank(requested) <= access_rank(cap)) {
                    // Narrowing is allowed; widening is not.
                    effective.access = requested.unwrap().to_string();
                    effective.access_spawned = if effective.access == "read" {
                        "read".to_string()
                    } else {
                        spec.access_spawned.as_str().to_string()
                    };
                } else {
                    self.issues.push(format!(
                        "{key}: built-in access is capped at '{cap}'; ignoring '{access}'"
                    ));
                }
            }
            if let Some(profile) = file
                .prompt
                .as_ref()
                .and_then(|prompt| prompt.profile.as_deref())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                && normalize_profile(profile) != Some(effective.prompt_profile.as_str())
            {
                self.issues.push(format!(
                    "{key}: built-in prompt profile is fixed to '{}'; ignoring '{}'",
                    effective.prompt_profile, profile
                ));
            }
            if let Some(aliases) = &file.aliases
                && !aliases.is_empty()
            {
                self.issues.push(format!(
                    "{key}: built-in aliases are fixed; ignoring custom aliases"
                ));
            }
            if let Some(delegation) = &file.delegation {
                if delegation.delegatable == Some(true) && !spec.delegatable {
                    self.issues.push(format!(
                        "{key}: built-in 'delegatable' cap is false; ignoring escalation"
                    ));
                }
                if delegation.can_delegate == Some(true) && !spec.can_delegate {
                    self.issues.push(format!(
                        "{key}: built-in 'can_delegate' cap is false; ignoring escalation"
                    ));
                }
                if delegation.can_spawn == Some(true) && !spec.can_spawn {
                    self.issues.push(format!(
                        "{key}: built-in 'can_spawn' cap is false; ignoring escalation"
                    ));
                }
                effective.delegatable = spec.delegatable && delegation.delegatable != Some(false);
                effective.can_delegate =
                    spec.can_delegate && delegation.can_delegate != Some(false);
                effective.can_spawn = spec.can_spawn && delegation.can_spawn != Some(false);
            }
            if let Some(ui) = &file.ui {
                if let Some(visible) = ui.visible {
                    effective.ui_visible = visible;
                }
                if let Some(order) = ui.order {
                    effective.ui_order = order;
                }
            }
        }
        let contract_tools: Vec<String> =
            contract().scope_map.get(key).cloned().unwrap_or_default();
        let selected: Option<HashSet<String>> = file.and_then(|file| {
            file.tools.as_ref().map(|tools| {
                tools
                    .iter()
                    .map(|tool| tool.trim().to_ascii_lowercase())
                    .filter(|tool| !tool.is_empty())
                    .collect()
            })
        });
        let mut tools: Vec<String> = contract_tools
            .into_iter()
            .filter(|tool| selected.as_ref().is_none_or(|set| set.contains(tool)))
            .collect();
        if let Some(selected) = &selected {
            for tool in selected {
                if !tools.iter().any(|allowed| allowed == tool) {
                    self.issues.push(format!(
                        "{key}: tool '{tool}' is not in the built-in scope contract; ignored"
                    ));
                }
            }
        }
        tools.sort();
        tools.dedup();
        effective.tools = Some(tools);
        effective.skills = file.and_then(|file| file.skills.clone());
        effective.mcp = file.and_then(|file| file.mcp.clone());
        let code_mode = file
            .and_then(|file| file.code_mode.as_deref())
            .and_then(stark_tools::CodeMode::parse)
            .unwrap_or_else(|| stark_tools::default_code_mode(key));
        let code_mode = if code_mode.enabled() && !stark_tools::scope_supports_code_mode(key) {
            self.issues.push(format!(
                "{key}: Code Mode is not available for this built-in; using off"
            ));
            stark_tools::CodeMode::Off
        } else {
            code_mode
        };
        effective.code_mode = Some(code_mode.as_str().to_string());
        if !code_mode.enabled()
            && let Some(tools) = effective.tools.as_mut()
        {
            tools.retain(|tool| tool != "execute");
        }
        effective.full_codemode = file
            .and_then(|file| file.full_codemode)
            .filter(|_| code_mode.enabled());
    }

    fn apply_custom_key(&self, key: &str) -> Result<(), String> {
        if RESERVED_KEYS.contains(&key) {
            return Err(format!("'{key}' is a reserved key"));
        }
        if workspace_agents().iter().any(|spec| spec.key == key) {
            return Err(format!("'{key}' is a built-in agent"));
        }
        if workspace_agents()
            .iter()
            .any(|spec| spec.aliases.contains(&key))
        {
            return Err(format!("'{key}' is already a built-in alias"));
        }
        if self.get(key).is_some()
            || self.agents.iter().any(|agent| {
                agent
                    .aliases
                    .iter()
                    .any(|alias| alias.eq_ignore_ascii_case(key))
            })
        {
            return Err(format!("'{key}' is already an agent key or alias"));
        }
        Ok(())
    }

    fn apply_custom(
        &mut self,
        harness_root: &Path,
        key: &str,
        file: &AgentFile,
        effective: &mut EffectiveAgent,
    ) -> Result<(), String> {
        self.apply_custom_key(key)?;
        let access = normalize_access(effective.access.as_str()).ok_or_else(|| {
            format!(
                "custom access must be read or write (got '{}')",
                effective.access
            )
        })?;
        effective.access = access.to_string();
        effective.access_spawned = if access == "read" {
            "read".to_string()
        } else {
            "worktree".to_string()
        };
        effective.builtin = false;
        effective.key = key.to_string();
        effective.prompt_profile = normalize_profile(&effective.prompt_profile)
            .unwrap_or("general")
            .to_string();
        let (routing_kinds, routing_paths) = custom_routing(file)?;
        effective.routing_workspace_kinds = routing_kinds;
        effective.routing_paths = routing_paths;
        let tools = file.tools.as_ref().ok_or_else(|| {
            "custom agents must declare an explicit tools list (use [] for none)".to_string()
        })?;
        let tools = validate_custom_tools(tools, access, true)?;
        let code_mode = custom_code_mode(file, &tools)?;
        if matches!(code_mode, stark_tools::CodeMode::Hybrid)
            && !tools.iter().any(|tool| stark_tools::is_codemode(tool))
        {
            self.warnings.push(format!(
                "{key}: hybrid Code Mode has no catalog tools; add a program-capable tool or use off"
            ));
        }
        let delegation = file.delegation.clone().unwrap_or_default();
        let can_delegate = delegation.can_delegate.unwrap_or(false);
        let can_spawn = delegation.can_spawn.unwrap_or(false);
        validate_capability_tools(&tools, can_delegate, can_spawn)?;
        effective.tools = Some(tools);
        // Skills may be workspace-local; keep the declared allowlist and let
        // the runtime catalog filter it. MCP servers are global config, so a
        // missing server is dropped with a warning.
        let (skills, _) =
            inspect_custom_skills(None, harness_root, file.skills.as_deref().unwrap_or(&[]));
        effective.skills = Some(skills);
        let (mcp, unknown_mcp) =
            inspect_custom_mcp(harness_root, file.mcp.as_deref().unwrap_or(&[]));
        for server in unknown_mcp {
            self.warnings.push(format!(
                "{key}: MCP server '{server}' is not configured; dropped"
            ));
        }
        effective.mcp = Some(mcp);
        effective.code_mode = Some(code_mode.as_str().to_string());
        effective.full_codemode = None;
        effective.delegatable = delegation.delegatable.unwrap_or(false);
        effective.can_delegate = can_delegate;
        effective.can_spawn = can_spawn;
        if let Some(ui) = &file.ui {
            effective.ui_visible = ui.visible.unwrap_or(true);
            effective.ui_order = ui.order.unwrap_or(0);
        }
        Ok(())
    }

    fn rebuild_index(&mut self) {
        self.by_key.clear();
        self.aliases.clear();
        let mut alias_owner: HashMap<String, String> = HashMap::new();
        for (index, agent) in self.agents.iter().enumerate() {
            self.by_key.insert(agent.key.clone(), index);
        }
        // Built-in aliases are static and already unique. Custom aliases are
        // accepted only when they cannot shadow a key or another alias.
        let builtin_aliases: HashSet<String> = workspace_agents()
            .iter()
            .flat_map(|spec| spec.aliases.iter().map(|alias| alias.to_string()))
            .collect();
        for alias in &builtin_aliases {
            if let Some(agent) = workspace_agents()
                .iter()
                .find(|spec| spec.aliases.iter().any(|item| item == alias))
            {
                alias_owner.insert(alias.clone(), agent.key.to_string());
            }
        }
        for agent in &self.agents {
            if agent.builtin {
                continue;
            }
            for alias in &agent.aliases {
                let alias = alias.trim().to_ascii_lowercase();
                if alias.is_empty() || alias == agent.key {
                    continue;
                }
                if self.by_key.contains_key(&alias) || alias_owner.contains_key(&alias) {
                    self.issues.push(format!(
                        "{}: alias '{alias}' collides with an existing agent key or alias; ignored",
                        agent.key
                    ));
                    continue;
                }
                alias_owner.insert(alias.clone(), agent.key.clone());
            }
        }
        self.aliases = alias_owner;
    }
}

fn is_explicit_builtin_kind(kind: &str) -> bool {
    if kind.is_empty() {
        return false;
    }
    const VIRTUAL: &[&str] = &[
        "house",
        "house_workspace",
        "personal",
        "personal_workspace",
        "general_workspace",
        "dfd",
        "dfd_workspace",
        "design",
        "design_workspace",
    ];
    workspace_agents().iter().any(|spec| spec.key == kind) || VIRTUAL.contains(&kind)
}

fn routing_matches(agent: &EffectiveAgent, kind: &str, workspace_path: &Path) -> bool {
    if agent.routing_workspace_kinds.is_empty() && agent.routing_paths.is_empty() {
        return false;
    }
    if !agent.routing_workspace_kinds.is_empty()
        && !agent
            .routing_workspace_kinds
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(kind))
    {
        return false;
    }
    if agent.routing_paths.is_empty() {
        return true;
    }
    let normalized = workspace_path.to_string_lossy().replace('\\', "/");
    let canonical = workspace_path
        .canonicalize()
        .ok()
        .map(|path| path.to_string_lossy().replace('\\', "/"));
    agent.routing_paths.iter().any(|pattern| {
        globset::Glob::new(pattern)
            .ok()
            .map(|glob| glob.compile_matcher())
            .is_some_and(|matcher| {
                matcher.is_match(&normalized)
                    || canonical
                        .as_deref()
                        .is_some_and(|path| matcher.is_match(path))
            })
    })
}

fn builtin_runtime(key: &str, registry: &AgentRegistry) -> EffectiveAgent {
    let spec = for_scope(key).unwrap_or_else(|| for_scope("general").expect("general"));
    let mut effective = stark_config::builtin_effective(spec.tier);
    effective.key = spec.key.to_string();
    effective.display = spec.display.to_string();
    effective.description = spec.description.to_string();
    effective.tier = spec.tier.to_string();
    effective.builtin = true;
    effective.aliases = spec.aliases.iter().map(|alias| alias.to_string()).collect();
    effective.access = spec.access_root.as_str().to_string();
    effective.access_spawned = spec.access_spawned.as_str().to_string();
    effective.prompt_profile = spec.prompt_profile.to_string();
    effective.delegatable = spec.delegatable;
    effective.can_delegate = spec.can_delegate;
    effective.can_spawn = spec.can_spawn;
    effective.ui_order = spec.ui_order;
    effective.tools = contract().scope_map.get(spec.key).cloned();
    let _ = registry;
    effective
}

pub fn root_access(agent: &EffectiveAgent) -> FileAccess {
    access_from_str(&agent.access)
}

pub fn spawned_access(agent: &EffectiveAgent) -> FileAccess {
    access_from_str(&agent.access_spawned)
}

pub fn access_for(agent: &EffectiveAgent, depth: u32, spawned: bool) -> FileAccess {
    if depth >= 2 {
        return FileAccess::Read;
    }
    if spawned {
        spawned_access(agent)
    } else {
        root_access(agent)
    }
}

fn access_rank(value: &str) -> u8 {
    match value {
        "read" => 0,
        _ => 1,
    }
}

fn normalize_access(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "read" | "readonly" | "read-only" => Some("read"),
        "write" => Some("write"),
        _ => None,
    }
}

fn normalize_profile(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "coding" => Some("coding"),
        "general" | "plain" => Some("general"),
        _ => None,
    }
}

fn validate_custom_tools(
    tools: &[String],
    access: &str,
    strict: bool,
) -> Result<Vec<String>, String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for raw in tools {
        let name = raw.trim().to_ascii_lowercase();
        if name.is_empty() {
            continue;
        }
        if !seen.insert(name.clone()) {
            continue;
        }
        let Some(spec) = contract().tool(&name) else {
            if strict {
                return Err(format!("unknown tool '{name}'"));
            }
            continue;
        };
        if !spec.exposed || !stark_tools::is_implemented(&name) {
            if strict {
                return Err(format!("tool '{name}' is not available to agents"));
            }
            continue;
        }
        if access == "read" && stark_tools::is_read_blocked(&name) {
            if strict {
                return Err(format!(
                    "read-only agents cannot receive the mutating tool '{name}'"
                ));
            }
            continue;
        }
        out.push(name);
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn custom_routing(file: &AgentFile) -> Result<(Vec<String>, Vec<String>), String> {
    let Some(routing) = &file.routing else {
        return Ok((Vec::new(), Vec::new()));
    };
    let mut kinds: Vec<String> = routing
        .workspace_kinds
        .iter()
        .map(|kind| kind.trim().to_ascii_lowercase())
        .filter(|kind| !kind.is_empty())
        .collect();
    kinds.sort();
    kinds.dedup();
    let mut paths = Vec::new();
    for raw in &routing.paths {
        let pattern = raw.trim().replace('\\', "/");
        if pattern.is_empty() {
            continue;
        }
        globset::Glob::new(&pattern)
            .map_err(|error| format!("invalid routing path glob '{pattern}': {error}"))?;
        paths.push(pattern);
    }
    paths.sort();
    paths.dedup();
    if kinds.is_empty() && paths.is_empty() {
        return Err("routing needs at least one workspace kind or path pattern".to_string());
    }
    Ok((kinds, paths))
}

fn custom_code_mode(file: &AgentFile, tools: &[String]) -> Result<stark_tools::CodeMode, String> {
    let mode = file
        .code_mode
        .as_deref()
        .and_then(stark_tools::CodeMode::parse)
        .unwrap_or_default();
    if matches!(mode, stark_tools::CodeMode::Only) {
        return Err(
            "custom agents support code_mode off or hybrid; 'only' is built-in Developer only"
                .to_string(),
        );
    }
    if file.full_codemode == Some(true) {
        return Err("full_codemode is built-in Developer only".to_string());
    }
    let has_execute = stark_tools::has_execute_tool(tools);
    match mode {
        stark_tools::CodeMode::Hybrid if !has_execute => {
            Err("code_mode 'hybrid' requires the execute tool in the custom tools list".to_string())
        }
        stark_tools::CodeMode::Off if has_execute => {
            Err("the execute tool requires code_mode 'hybrid' for custom agents".to_string())
        }
        _ => Ok(mode),
    }
}

fn validate_capability_tools(
    tools: &[String],
    can_delegate: bool,
    can_spawn: bool,
) -> Result<(), String> {
    let has = |name: &str| tools.iter().any(|tool| tool == name);
    if !can_delegate
        && ["delegate", "delegation_status", "job_message", "job_cancel"]
            .iter()
            .any(|name| has(name))
    {
        return Err("delegation tools require delegation.can_delegate = true".to_string());
    }
    if !can_spawn
        && [
            "spawn_agent",
            "message_agent",
            "wait_agent",
            "interrupt_agent",
            "close_agent",
            "agent",
        ]
        .iter()
        .any(|name| has(name))
    {
        return Err("child-agent tools require delegation.can_spawn = true".to_string());
    }
    Ok(())
}

fn normalize_names(names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = names
        .iter()
        .map(|name| name.trim().to_ascii_lowercase())
        .filter(|name| !name.is_empty())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Returns `(normalized, unknown)`. An unknown skill may still be valid in a
/// workspace that was not supplied at save/load time, so callers decide
/// whether it is an error, a warning, or a runtime filter.
fn inspect_custom_skills(
    workspace: Option<&Path>,
    harness_root: &Path,
    skills: &[String],
) -> (Vec<String>, Vec<String>) {
    let root = workspace.unwrap_or(harness_root);
    let known: HashSet<String> = stark_tools::skill_names(root, harness_root)
        .into_iter()
        .map(|name| name.to_ascii_lowercase())
        .collect();
    let normalized = normalize_names(skills);
    let unknown = normalized
        .iter()
        .filter(|name| !known.contains(*name))
        .cloned()
        .collect();
    (normalized, unknown)
}

fn inspect_custom_mcp(harness_root: &Path, servers: &[String]) -> (Vec<String>, Vec<String>) {
    let known: HashSet<String> = stark_tools::configured_servers(harness_root)
        .into_iter()
        .map(|name| name.to_ascii_lowercase())
        .collect();
    let normalized = normalize_names(servers);
    let unknown = normalized
        .iter()
        .filter(|name| !known.contains(*name))
        .cloned()
        .collect();
    (normalized, unknown)
}

fn builtin_override_errors(spec: &AgentSpec, file: &AgentFile) -> Vec<String> {
    let mut errors = Vec::new();
    if let Some(access) = file
        .access
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        match normalize_access(access) {
            Some(requested) if access_rank(requested) <= access_rank(spec.access_root.as_str()) => {
            }
            _ => errors.push(format!(
                "built-in access is capped at '{}'",
                spec.access_root.as_str()
            )),
        }
    }
    if let Some(profile) = file
        .prompt
        .as_ref()
        .and_then(|prompt| prompt.profile.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        && normalize_profile(profile) != Some(spec.prompt_profile)
    {
        errors.push(format!(
            "built-in prompt profile is fixed to '{}'",
            spec.prompt_profile
        ));
    }
    if let Some(delegation) = &file.delegation {
        if delegation.delegatable == Some(true) && !spec.delegatable {
            errors.push("built-in is not delegatable".to_string());
        }
        if delegation.can_delegate == Some(true) && !spec.can_delegate {
            errors.push("built-in cannot delegate".to_string());
        }
        if delegation.can_spawn == Some(true) && !spec.can_spawn {
            errors.push("built-in cannot spawn agents".to_string());
        }
    }
    if file
        .routing
        .as_ref()
        .is_some_and(|routing| !routing.workspace_kinds.is_empty() || !routing.paths.is_empty())
    {
        errors.push("routing is available to custom agents only".to_string());
    }
    let scope_tools: HashSet<String> = contract()
        .scope_map
        .get(spec.key)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect();
    if let Some(tools) = &file.tools {
        for tool in tools {
            let tool = tool.trim().to_ascii_lowercase();
            if !scope_tools.contains(&tool) {
                errors.push(format!(
                    "tool '{tool}' is not in the built-in scope contract"
                ));
            }
        }
    }
    let mode = file
        .code_mode
        .as_deref()
        .and_then(stark_tools::CodeMode::parse)
        .unwrap_or_else(|| stark_tools::default_code_mode(spec.key));
    if mode.enabled() && !stark_tools::scope_supports_code_mode(spec.key) {
        errors.push(format!(
            "Code Mode is not available for the '{}' built-in",
            spec.key
        ));
    }
    errors
}

/// Resolve an unsaved draft into the exact runtime record. The API preview
/// uses this so its prompt/tool view matches a later run byte-for-byte.
pub fn resolve_definition(
    config: &ConfigStore,
    harness_root: &Path,
    key: &str,
    file: &AgentFile,
) -> Result<EffectiveAgent, String> {
    let key = key.trim().to_ascii_lowercase();
    let mut registry = AgentRegistry::default();
    if let Some(spec) = workspace_agents().iter().find(|spec| spec.key == key) {
        let mut effective = config
            .effective_for_file(&key, spec.tier, file)
            .map_err(|error| error.to_string())?;
        registry.apply_builtin(*spec, Some(file), &mut effective);
        return Ok(effective);
    }
    let mut effective = config
        .effective_for_file(&key, "workspace", file)
        .map_err(|error| error.to_string())?;
    registry.apply_custom(harness_root, &key, file, &mut effective)?;
    Ok(effective)
}

/// Validate a draft before it is written. `existing` lets the API check alias
/// collisions against the current registry.
pub fn validate_definition(
    config: &ConfigStore,
    harness_root: &Path,
    key: &str,
    file: &AgentFile,
    existing: &AgentRegistry,
) -> DefinitionCheck {
    validate_definition_in(config, harness_root, key, file, existing, None)
}

/// Workspace-aware validation used by the save/preview API. A workspace lets
/// project-local skills validate strictly; without one they are warnings
/// because they may exist only in the project being edited.
pub fn validate_definition_in(
    config: &ConfigStore,
    harness_root: &Path,
    key: &str,
    file: &AgentFile,
    existing: &AgentRegistry,
    workspace: Option<&Path>,
) -> DefinitionCheck {
    let mut check = DefinitionCheck::default();
    let key = key.trim().to_ascii_lowercase();
    if let Err(error) = config.validate_agent(file) {
        check.errors.push(error.to_string());
    }
    if let Err(error) = existing.apply_custom_key(&key) {
        // Built-in overrides are valid, and PUT to an existing custom key is
        // an edit. A new custom key cannot shadow a built-in key/alias.
        let editing_existing_custom = existing
            .get(&key)
            .is_some_and(|agent| agent.key == key && !agent.builtin);
        if workspace_agents().iter().any(|spec| spec.key == key) || editing_existing_custom {
            // Built-in caps or the existing custom record are checked below.
        } else {
            check.errors.push(error);
        }
    }
    if let Some(access) = file
        .access
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        && normalize_access(access).is_none()
    {
        check
            .errors
            .push(format!("invalid access '{access}' (use read or write)"));
    }
    if let Some(profile) = file
        .prompt
        .as_ref()
        .and_then(|prompt| prompt.profile.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        && normalize_profile(profile).is_none()
    {
        check.errors.push(format!(
            "invalid prompt profile '{profile}' (use coding or general)"
        ));
    }
    if let Some(aliases) = &file.aliases {
        let mut seen = HashSet::new();
        for alias in aliases {
            let alias = alias.trim().to_ascii_lowercase();
            if alias.is_empty() {
                continue;
            }
            if !seen.insert(alias.clone()) {
                check.errors.push(format!("duplicate alias '{alias}'"));
            }
            if alias == key {
                check
                    .errors
                    .push("an agent cannot alias its own key".to_string());
            }
            let owned_by_same_agent = existing.get(&alias).is_some_and(|agent| agent.key == key);
            if RESERVED_KEYS.contains(&alias.as_str())
                || ((existing.by_key.contains_key(&alias) || existing.aliases.contains_key(&alias))
                    && !owned_by_same_agent)
            {
                check.errors.push(format!(
                    "alias '{alias}' collides with an existing key or alias"
                ));
            }
        }
    }
    let builtin = workspace_agents().iter().find(|spec| spec.key == key);
    if let Some(spec) = builtin {
        check.errors.extend(builtin_override_errors(spec, file));
    } else {
        // Custom definition.
        if let Err(error) = custom_routing(file) {
            check.errors.push(error);
        }
        let access = file
            .access
            .as_deref()
            .and_then(normalize_access)
            .unwrap_or("read");
        match file.tools.as_ref() {
            Some(tools) => match validate_custom_tools(tools, access, true) {
                Ok(tools) => {
                    let delegation = file.delegation.clone().unwrap_or_default();
                    if let Err(error) = validate_capability_tools(
                        &tools,
                        delegation.can_delegate.unwrap_or(false),
                        delegation.can_spawn.unwrap_or(false),
                    ) {
                        check.errors.push(error);
                    }
                    match custom_code_mode(file, &tools) {
                        Ok(stark_tools::CodeMode::Hybrid) => {
                            if !tools.iter().any(|tool| stark_tools::is_codemode(tool)) {
                                check.warnings.push(
                                    "hybrid Code Mode has no catalog tools; add a program-capable tool or use off"
                                        .to_string(),
                                );
                            }
                        }
                        Ok(_) => {}
                        Err(error) => check.errors.push(error),
                    }
                    if tools.is_empty() {
                        check
                            .warnings
                            .push("this agent has no tools and can only talk".to_string());
                    }
                    if access == "read" && tools.iter().any(|tool| tool == "bash") {
                        check.warnings.push(
                            "read-only bash is allowed; mutating commands are blocked at dispatch"
                                .to_string(),
                        );
                    }
                }
                Err(error) => check.errors.push(error),
            },
            None => check.errors.push(
                "custom agents must declare an explicit tools list (use [] for none)".to_string(),
            ),
        }
        let (_, unknown_skills) = inspect_custom_skills(
            workspace,
            harness_root,
            file.skills.as_deref().unwrap_or(&[]),
        );
        for skill in unknown_skills {
            let message = format!("unknown skill '{skill}'");
            if workspace.is_some() {
                check.errors.push(message);
            } else {
                check
                    .warnings
                    .push(format!("{message}; it may be workspace-local"));
            }
        }
        let (_, unknown_mcp) = inspect_custom_mcp(harness_root, file.mcp.as_deref().unwrap_or(&[]));
        for server in unknown_mcp {
            check.errors.push(format!("unknown MCP server '{server}'"));
        }
        if !file.skills.as_deref().unwrap_or(&[]).is_empty()
            && !file
                .tools
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .any(|tool| tool.trim().eq_ignore_ascii_case("skill"))
        {
            check.errors.push(
                "skills are configured but the 'skill' tool is not in the tool list".to_string(),
            );
        }
    }
    check
}

#[cfg(test)]
mod tests {
    use super::*;
    use stark_config::AgentPromptFile;

    fn write_agent(root: &Path, key: &str, value: serde_json::Value) {
        let dir = root.join(".deep_agents").join("config").join("agents");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{key}.json")),
            serde_json::to_string_pretty(&value).unwrap(),
        )
        .unwrap();
    }

    fn custom(key: &str, tools: &[&str], access: &str) -> AgentFile {
        AgentFile {
            version: 2,
            key: key.to_string(),
            display: key.to_string(),
            access: Some(access.to_string()),
            tools: Some(tools.iter().map(|tool| tool.to_string()).collect()),
            prompt: Some(AgentPromptFile {
                profile: Some("general".to_string()),
                persona: Some("Own this domain.".to_string()),
            }),
            ..AgentFile::default()
        }
    }

    #[test]
    fn builtins_resolve_and_keep_their_caps() {
        let dir = tempfile::tempdir().unwrap();
        let config = ConfigStore::open(dir.path());
        let registry = AgentRegistry::load(&config, dir.path());
        assert_eq!(registry.list().len(), 8);
        assert!(
            registry
                .get("coder")
                .is_some_and(|agent| agent.key == "developer")
        );
        assert!(registry.get("coder").is_some_and(|agent| agent.builtin));
        let developer = registry.get("developer").unwrap();
        assert_eq!(developer.access, "write");
        assert_eq!(developer.access_spawned, "worktree");
        assert!(
            developer
                .tools
                .as_ref()
                .unwrap()
                .contains(&"execute".to_string())
        );
        assert_eq!(developer.code_mode.as_deref(), Some("hybrid"));
        let house = registry.get("house").unwrap();
        assert_eq!(house.access, "read");
        assert!(
            house
                .tools
                .as_ref()
                .unwrap()
                .contains(&"house_get_state".to_string())
        );
        assert!(!house.tools.as_ref().unwrap().contains(&"edit".to_string()));
    }

    #[test]
    fn custom_agents_are_closed_and_validated() {
        let dir = tempfile::tempdir().unwrap();
        write_agent(
            dir.path(),
            "backend",
            serde_json::to_value(custom(
                "backend",
                &["bash", "read", "edit", "glob", "grep"],
                "write",
            ))
            .unwrap(),
        );
        let config = ConfigStore::open(dir.path());
        let registry = AgentRegistry::load(&config, dir.path());
        let backend = registry.get("backend").expect("custom agent");
        assert!(!backend.builtin);
        assert_eq!(backend.access, "write");
        assert_eq!(backend.access_spawned, "worktree");
        assert_eq!(
            backend.tools.as_deref().unwrap(),
            &["bash", "edit", "glob", "grep", "read"]
        );
        assert_eq!(backend.skills.as_deref(), Some(&[][..]));
        assert_eq!(backend.mcp.as_deref(), Some(&[][..]));
        assert_eq!(backend.code_mode.as_deref(), Some("off"));
        assert_eq!(backend.persona.as_deref(), Some("Own this domain."));
        assert!(
            registry
                .visible()
                .iter()
                .any(|agent| agent.key == "backend")
        );
    }

    #[test]
    fn custom_definitions_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        write_agent(
            dir.path(),
            "no-tools",
            serde_json::json!({"version": 2, "display": "No Tools"}),
        );
        write_agent(
            dir.path(),
            "unknown-tool",
            serde_json::json!({
                "version": 2,
                "tools": ["read", "not_a_tool"]
            }),
        );
        write_agent(
            dir.path(),
            "read-write-tool",
            serde_json::json!({
                "version": 2,
                "access": "read",
                "tools": ["read", "edit"]
            }),
        );
        let config = ConfigStore::open(dir.path());
        let registry = AgentRegistry::load(&config, dir.path());
        assert!(registry.get("no-tools").is_none());
        assert!(registry.get("unknown-tool").is_none());
        assert!(registry.get("read-write-tool").is_none());
        assert!(
            registry
                .issues()
                .iter()
                .any(|issue| issue.contains("no-tools"))
        );
        assert!(
            registry
                .issues()
                .iter()
                .any(|issue| issue.contains("not_a_tool"))
        );
        assert!(
            registry
                .issues()
                .iter()
                .any(|issue| issue.contains("mutating tool"))
        );
    }

    #[test]
    fn legacy_coder_file_is_a_developer_override() {
        let dir = tempfile::tempdir().unwrap();
        write_agent(
            dir.path(),
            "coder",
            serde_json::json!({"display": "Legacy Coder", "model": "mimo-flash"}),
        );
        let config = ConfigStore::open(dir.path());
        let registry = AgentRegistry::load(&config, dir.path());
        let developer = registry.get("developer").unwrap();
        assert_eq!(developer.display, "Legacy Coder");
        assert_eq!(developer.model, "mimo-v2.6-flash");
        assert!(!registry.list().iter().any(|agent| agent.key == "coder"));
    }

    #[test]
    fn custom_key_cannot_shadow_a_builtin_alias() {
        let dir = tempfile::tempdir().unwrap();
        let config = ConfigStore::open(dir.path());
        let registry = AgentRegistry::load(&config, dir.path());
        let file: AgentFile =
            serde_json::from_value(serde_json::json!({"version": 2, "tools": ["read"]})).unwrap();
        let check = validate_definition(&config, dir.path(), "coder", &file, &registry);
        assert!(
            check
                .errors
                .iter()
                .any(|error| error.contains("built-in alias")),
            "{:?}",
            check.errors
        );
    }

    #[test]
    fn custom_key_cannot_shadow_another_custom_alias() {
        let dir = tempfile::tempdir().unwrap();
        let mut first = custom("first", &["read"], "read");
        first.aliases = Some(vec!["second".to_string()]);
        write_agent(dir.path(), "first", serde_json::to_value(first).unwrap());
        write_agent(
            dir.path(),
            "second",
            serde_json::to_value(custom("second", &["read"], "read")).unwrap(),
        );
        let config = ConfigStore::open(dir.path());
        let registry = AgentRegistry::load(&config, dir.path());
        assert_eq!(registry.get("first").unwrap().key, "first");
        // The rejected key is not listed; the alias still resolves to the
        // agent that owns it.
        assert_eq!(registry.get("second").unwrap().key, "first");
        assert!(registry.list().iter().all(|agent| agent.key != "second"));
        assert!(
            registry
                .issues()
                .iter()
                .any(|issue| issue.contains("second"))
        );
    }

    #[test]
    fn custom_alias_resolves_and_is_never_listed() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = custom("backend", &["read"], "read");
        file.aliases = Some(vec!["be".to_string()]);
        write_agent(dir.path(), "backend", serde_json::to_value(file).unwrap());
        let config = ConfigStore::open(dir.path());
        let registry = AgentRegistry::load(&config, dir.path());
        assert_eq!(registry.get("be").unwrap().key, "backend");
        assert!(registry.visible().iter().all(|agent| agent.key != "be"));
    }

    #[test]
    fn invalid_builtin_override_is_ignored_as_a_whole() {
        let dir = tempfile::tempdir().unwrap();
        write_agent(
            dir.path(),
            "developer",
            serde_json::json!({
                "display": "Bad Override",
                "tools": ["read", "not_a_tool"]
            }),
        );
        let config = ConfigStore::open(dir.path());
        let registry = AgentRegistry::load(&config, dir.path());
        let developer = registry.get("developer").unwrap();
        assert_eq!(developer.display, "Developer");
        assert!(
            developer
                .tools
                .as_ref()
                .unwrap()
                .contains(&"edit".to_string())
        );
        assert!(
            registry
                .issues()
                .iter()
                .any(|issue| issue.contains("not_a_tool"))
        );
    }

    #[test]
    fn builtin_overrides_cannot_widen_access_or_unknown_tools() {
        let dir = tempfile::tempdir().unwrap();
        let config = ConfigStore::open(dir.path());
        let registry = AgentRegistry::load(&config, dir.path());
        let widen = AgentFile {
            access: Some("write".to_string()),
            ..AgentFile::default()
        };
        let check = validate_definition(&config, dir.path(), "house", &widen, &registry);
        assert!(check.errors.iter().any(|error| error.contains("capped")));
        let narrow = AgentFile {
            access: Some("read".to_string()),
            ..AgentFile::default()
        };
        let check = validate_definition(&config, dir.path(), "developer", &narrow, &registry);
        assert!(check.errors.is_empty(), "{:?}", check.errors);
        let effective = resolve_definition(&config, dir.path(), "developer", &narrow).unwrap();
        assert_eq!(effective.access, "read");
        assert_eq!(effective.access_spawned, "read");
        let unknown = AgentFile {
            tools: Some(vec!["read".to_string(), "not_a_tool".to_string()]),
            ..AgentFile::default()
        };
        let check = validate_definition(&config, dir.path(), "developer", &unknown, &registry);
        assert!(
            check
                .errors
                .iter()
                .any(|error| error.contains("not_a_tool"))
        );
    }

    #[test]
    fn orchestration_tools_require_their_capability() {
        let dir = tempfile::tempdir().unwrap();
        let config = ConfigStore::open(dir.path());
        let registry = AgentRegistry::load(&config, dir.path());
        let file = custom("backend", &["read", "spawn_agent"], "write");
        let check = validate_definition(&config, dir.path(), "backend", &file, &registry);
        assert!(
            check.errors.iter().any(|error| error.contains("can_spawn")),
            "{:?}",
            check.errors
        );
        let mut delegated = custom("backend", &["read", "delegate"], "read");
        delegated.delegation = Some(stark_config::AgentDelegationFile {
            can_delegate: Some(true),
            ..stark_config::AgentDelegationFile::default()
        });
        let check = validate_definition(&config, dir.path(), "backend", &delegated, &registry);
        assert!(check.errors.is_empty(), "{:?}", check.errors);
    }

    #[test]
    fn custom_code_mode_requires_execute_and_hybrid() {
        let dir = tempfile::tempdir().unwrap();
        let config = ConfigStore::open(dir.path());
        let registry = AgentRegistry::load(&config, dir.path());
        let no_execute = AgentFile {
            code_mode: Some("hybrid".to_string()),
            tools: Some(vec!["read".to_string()]),
            ..AgentFile::default()
        };
        let check = validate_definition(&config, dir.path(), "backend", &no_execute, &registry);
        assert!(
            check
                .errors
                .iter()
                .any(|error| error.contains("requires the execute"))
        );
        let only = AgentFile {
            code_mode: Some("only".to_string()),
            tools: Some(vec!["read".to_string(), "execute".to_string()]),
            ..AgentFile::default()
        };
        let check = validate_definition(&config, dir.path(), "backend", &only, &registry);
        assert!(
            check
                .errors
                .iter()
                .any(|error| error.contains("built-in Developer only"))
        );
        let full = AgentFile {
            code_mode: Some("hybrid".to_string()),
            full_codemode: Some(true),
            tools: Some(vec!["read".to_string(), "execute".to_string()]),
            ..AgentFile::default()
        };
        let check = validate_definition(&config, dir.path(), "backend", &full, &registry);
        assert!(
            check
                .errors
                .iter()
                .any(|error| error.contains("full_codemode"))
        );
        let hybrid = AgentFile {
            code_mode: Some("hybrid".to_string()),
            tools: Some(vec![
                "read".to_string(),
                "files_changed".to_string(),
                "execute".to_string(),
            ]),
            ..AgentFile::default()
        };
        let check = validate_definition(&config, dir.path(), "backend", &hybrid, &registry);
        assert!(check.errors.is_empty(), "{:?}", check.errors);
        let effective = resolve_definition(&config, dir.path(), "backend", &hybrid).unwrap();
        assert_eq!(effective.code_mode.as_deref(), Some("hybrid"));
        assert!(
            effective
                .tools
                .as_ref()
                .unwrap()
                .contains(&"execute".to_string())
        );
    }

    #[test]
    fn custom_routing_is_unique_match_only() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("work");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(dir.path().join(".deep_agents/config/agents")).unwrap();
        let path_pattern = workspace.to_string_lossy().replace('\\', "/");
        let mut backend = custom("backend", &["read"], "read");
        backend.routing = Some(stark_config::AgentRoutingFile {
            workspace_kinds: vec!["coding".to_string()],
            paths: vec![path_pattern.clone()],
        });
        write_agent(
            dir.path(),
            "backend",
            serde_json::to_value(backend).unwrap(),
        );
        let config = ConfigStore::open(dir.path());
        let registry = AgentRegistry::load(&config, dir.path());
        let decision = registry.route_decision("coding", &workspace, dir.path());
        assert_eq!(decision.agent.key, "backend");
        assert!(decision.warning.is_none());
        // A virtual workspace kind is explicit and wins over routing.
        let decision = registry.route_decision("design", &workspace, dir.path());
        assert_eq!(decision.agent.key, "ui");
        let decision = registry.route_decision("general_workspace", &workspace, dir.path());
        assert_eq!(decision.agent.key, "general");
    }

    #[test]
    fn ambiguous_routing_falls_back_with_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("work");
        std::fs::create_dir_all(&workspace).unwrap();
        let path_pattern = workspace.to_string_lossy().replace('\\', "/");
        for key in ["alpha", "beta"] {
            let mut file = custom(key, &["read"], "read");
            file.routing = Some(stark_config::AgentRoutingFile {
                workspace_kinds: vec!["coding".to_string()],
                paths: vec![path_pattern.clone()],
            });
            write_agent(dir.path(), key, serde_json::to_value(file).unwrap());
        }
        let config = ConfigStore::open(dir.path());
        let registry = AgentRegistry::load(&config, dir.path());
        let decision = registry.route_decision("coding", &workspace, dir.path());
        assert_eq!(decision.agent.key, "developer");
        assert!(decision.warning.is_some());
    }

    #[test]
    fn routing_requires_valid_nonempty_rules() {
        let dir = tempfile::tempdir().unwrap();
        let config = ConfigStore::open(dir.path());
        let registry = AgentRegistry::load(&config, dir.path());
        let mut file = custom("backend", &["read"], "read");
        file.routing = Some(stark_config::AgentRoutingFile {
            workspace_kinds: Vec::new(),
            paths: vec!["[".to_string()],
        });
        let check = validate_definition(&config, dir.path(), "backend", &file, &registry);
        assert!(check.errors.iter().any(|error| error.contains("glob")));
    }

    #[test]
    fn route_prefers_explicit_custom_agents() {
        let dir = tempfile::tempdir().unwrap();
        write_agent(
            dir.path(),
            "backend",
            serde_json::to_value(custom("backend", &["read"], "read")).unwrap(),
        );
        let config = ConfigStore::open(dir.path());
        let registry = AgentRegistry::load(&config, dir.path());
        assert_eq!(
            registry.route("backend", dir.path(), dir.path()).key,
            "backend"
        );
        assert_eq!(
            registry.route("coding", dir.path(), dir.path()).key,
            "developer"
        );
    }
}
