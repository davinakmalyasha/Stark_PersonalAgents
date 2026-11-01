//! Declarative provider + agent configuration.
//!
//! This replaces the old single `STARK_PROVIDER` env switch. The files are the
//! source of truth:
//!
//! ```text
//! .deep_agents/config/providers.json
//! .deep_agents/config/agents/<key>.json
//! ```
//!
//! Environment variables are only used for secrets (`key_env` references).
//! Provider mode, base URL, model ids, tier defaults, and reasoning effort all
//! come from the files (or the built-in defaults when a file is absent).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use serde::{Deserialize, Serialize};

pub const CONFIG_VERSION: u32 = 1;
pub const THINKING_LEVELS: &[&str] = &["minimal", "low", "medium", "high"];

fn default_version() -> u32 {
    CONFIG_VERSION
}
fn default_kind() -> String {
    "openai-compatible".to_string()
}

/// One or many environment variable names. Many is used for fallbacks:
/// `["OPENCODE_API_KEY", "DEEPSEEK_API_KEY"]`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum EnvList {
    One(String),
    Many(Vec<String>),
}

impl EnvList {
    pub fn names(&self) -> Vec<String> {
        match self {
            Self::One(name) => vec![name.clone()],
            Self::Many(names) => names.clone(),
        }
    }
}

impl From<&str> for EnvList {
    fn from(value: &str) -> Self {
        Self::One(value.to_string())
    }
}

impl From<Vec<&str>> for EnvList {
    fn from(value: Vec<&str>) -> Self {
        Self::Many(value.into_iter().map(str::to_string).collect())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSpec {
    /// Canonical id sent to the provider.
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub context: Option<usize>,
    #[serde(default)]
    pub reasoning: bool,
    /// Accepted alternative spellings (old persisted slugs, UI labels).
    #[serde(default)]
    pub aliases: Vec<String>,
}

impl ModelSpec {
    fn matches(&self, requested: &str) -> bool {
        self.id.eq_ignore_ascii_case(requested)
            || self.label.eq_ignore_ascii_case(requested)
            || self
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(requested))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderSpec {
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub base_url: String,
    /// Environment variable(s) holding the API key. Values are never stored in
    /// the config file.
    #[serde(default)]
    pub key_env: Option<EnvList>,
    #[serde(default)]
    pub models: BTreeMap<String, ModelSpec>,
    #[serde(default)]
    pub default_model: String,
}

impl ProviderSpec {
    pub fn label(&self) -> &str {
        if self.label.is_empty() {
            "Provider"
        } else {
            &self.label
        }
    }

    pub fn key_names(&self) -> Vec<String> {
        self.key_env
            .as_ref()
            .map(EnvList::names)
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TierSpec {
    pub model: String,
    #[serde(default)]
    pub thinking: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProvidersFile {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub active: String,
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderSpec>,
    /// provider -> tier -> defaults.
    #[serde(default)]
    pub matrix: BTreeMap<String, BTreeMap<String, TierSpec>>,
}

impl ProvidersFile {
    /// Built-in defaults. Used when no `providers.json` exists and as the
    /// fallback when the file is invalid.
    pub fn builtin() -> Self {
        let mut providers = BTreeMap::new();
        providers.insert(
            "opencode".to_string(),
            ProviderSpec {
                kind: default_kind(),
                label: "OpenCode Go".to_string(),
                base_url: "https://opencode.ai/zen/go/v1".to_string(),
                key_env: Some(EnvList::from(vec!["OPENCODE_API_KEY", "DEEPSEEK_API_KEY"])),
                models: BTreeMap::from([
                    (
                        "flash".to_string(),
                        ModelSpec {
                            id: "deepseek-v4.1-flash".to_string(),
                            label: "DeepSeek V4.1 Flash".to_string(),
                            context: Some(131_072),
                            reasoning: true,
                            aliases: vec![
                                "deepseek/deepseek-v4.1-flash-1017".to_string(),
                                "deepseek/deepseek-v4.1-flash".to_string(),
                                "deepseek-v4.1-flash".to_string(),
                                "deepseek-flash".to_string(),
                                "deepseek:v4.1-flash".to_string(),
                            ],
                        },
                    ),
                    (
                        "mimo-flash".to_string(),
                        ModelSpec {
                            id: "mimo-v2.6-flash".to_string(),
                            label: "MiMo V2.6 Flash".to_string(),
                            context: None,
                            reasoning: true,
                            aliases: vec![
                                "xiaomi/mimo-v2.6-flash".to_string(),
                                "mimo-v2.6-flash".to_string(),
                            ],
                        },
                    ),
                    (
                        "mimo-pro".to_string(),
                        ModelSpec {
                            id: "mimo-v2.6-pro".to_string(),
                            label: "MiMo V2.6 Pro".to_string(),
                            context: None,
                            reasoning: true,
                            aliases: vec![
                                "xiaomi/mimo-v2.6-pro".to_string(),
                                "mimo-v2.6-pro".to_string(),
                            ],
                        },
                    ),
                    (
                        "muse".to_string(),
                        ModelSpec {
                            id: "muse-spark-1.3-contributor".to_string(),
                            label: "Muse Spark 1.3 Contributor".to_string(),
                            context: None,
                            reasoning: true,
                            aliases: vec!["meta/muse-spark-1.3-contributor".to_string()],
                        },
                    ),
                ]),
                default_model: "flash".to_string(),
            },
        );
        providers.insert(
            "openrouter".to_string(),
            ProviderSpec {
                kind: default_kind(),
                label: "OpenRouter".to_string(),
                base_url: "https://openrouter.ai/api/v1".to_string(),
                key_env: Some(EnvList::from(vec![
                    "OPENROUTER_API_KEY",
                    "OPENROUTER_API_KEYS",
                ])),
                models: BTreeMap::from([(
                    "flash".to_string(),
                    ModelSpec {
                        id: "deepseek/deepseek-v4.1-flash-1017".to_string(),
                        label: "DeepSeek V4.1 Flash (OpenRouter)".to_string(),
                        context: Some(131_072),
                        reasoning: false,
                        aliases: vec![
                            "deepseek/deepseek-v4.1-flash".to_string(),
                            "deepseek-v4.1-flash".to_string(),
                        ],
                    },
                )]),
                default_model: "flash".to_string(),
            },
        );
        providers.insert(
            "deepseek".to_string(),
            ProviderSpec {
                kind: default_kind(),
                label: "DeepSeek".to_string(),
                base_url: "https://api.deepseek.com".to_string(),
                key_env: Some(EnvList::from("DEEPSEEK_API_KEY")),
                models: BTreeMap::from([(
                    "chat".to_string(),
                    ModelSpec {
                        id: "deepseek-chat".to_string(),
                        label: "DeepSeek Chat".to_string(),
                        context: Some(65_536),
                        reasoning: false,
                        aliases: vec![],
                    },
                )]),
                default_model: "chat".to_string(),
            },
        );
        providers.insert(
            "command".to_string(),
            ProviderSpec {
                kind: default_kind(),
                label: "Command Code".to_string(),
                base_url: "https://api.commandcode.ai/provider/v1".to_string(),
                key_env: Some(EnvList::from("COMMAND_CODE_API_KEY")),
                models: BTreeMap::from([(
                    "flash".to_string(),
                    ModelSpec {
                        id: "deepseek/deepseek-v4.1-flash".to_string(),
                        label: "DeepSeek V4.1 Flash (Command)".to_string(),
                        context: None,
                        reasoning: true,
                        aliases: vec!["deepseek-v4.1-flash".to_string()],
                    },
                )]),
                default_model: "flash".to_string(),
            },
        );

        let mut matrix = BTreeMap::new();
        for provider in ["opencode", "openrouter", "deepseek", "command"] {
            matrix.insert(
                provider.to_string(),
                BTreeMap::from([
                    (
                        "orchestrator".to_string(),
                        TierSpec {
                            model: if provider == "deepseek" {
                                "chat"
                            } else {
                                "flash"
                            }
                            .to_string(),
                            thinking: "low".to_string(),
                        },
                    ),
                    (
                        "workspace".to_string(),
                        TierSpec {
                            model: if provider == "deepseek" {
                                "chat"
                            } else {
                                "flash"
                            }
                            .to_string(),
                            thinking: "high".to_string(),
                        },
                    ),
                    (
                        "specialist".to_string(),
                        TierSpec {
                            model: if provider == "deepseek" {
                                "chat"
                            } else {
                                "flash"
                            }
                            .to_string(),
                            thinking: "medium".to_string(),
                        },
                    ),
                ]),
            );
        }

        Self {
            version: CONFIG_VERSION,
            active: "opencode".to_string(),
            providers,
            matrix,
        }
    }

    fn provider(&self, id: &str) -> Option<&ProviderSpec> {
        self.providers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(id))
            .map(|(_, value)| value)
    }

    pub fn provider_id(&self, id: &str) -> Option<String> {
        self.providers
            .keys()
            .find(|key| key.eq_ignore_ascii_case(id))
            .cloned()
    }

    pub fn active_id(&self) -> String {
        if self.provider(&self.active).is_some() {
            self.provider_id(&self.active)
                .unwrap_or_else(|| self.active.clone())
        } else if let Some((key, _)) = self.providers.iter().next() {
            key.clone()
        } else {
            "opencode".to_string()
        }
    }

    fn tier(&self, provider: &str, tier: &str) -> Option<&TierSpec> {
        self.matrix.get(provider).and_then(|tiers| {
            tiers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(tier))
                .map(|(_, value)| value)
        })
    }

    fn resolve_model(
        &self,
        provider_id: &str,
        requested: &str,
    ) -> Result<(String, String), ConfigError> {
        let Some(provider) = self.provider(provider_id) else {
            return Err(ConfigError::UnknownProvider(provider_id.to_string()));
        };
        let requested = requested.trim();
        if requested.is_empty() {
            return self.default_model(provider_id, provider);
        }
        if let Some((key, model)) = provider
            .models
            .iter()
            .find(|(key, model)| key.eq_ignore_ascii_case(requested) || model.matches(requested))
        {
            return Ok((key.clone(), model.id.clone()));
        }
        Err(ConfigError::UnknownModel {
            provider: provider_id.to_string(),
            model: requested.to_string(),
            available: provider.models.keys().cloned().collect(),
        })
    }

    fn default_model(
        &self,
        provider_id: &str,
        provider: &ProviderSpec,
    ) -> Result<(String, String), ConfigError> {
        let requested = if provider.default_model.is_empty() {
            provider
                .models
                .keys()
                .next()
                .cloned()
                .ok_or_else(|| ConfigError::NoModels(provider_id.to_string()))?
        } else {
            provider.default_model.clone()
        };
        self.resolve_model(provider_id, &requested)
    }
}

/// Per-agent overrides. `null` or a missing field means inherit.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentFile {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub display: String,
    #[serde(default)]
    pub tier: String,
    /// Provider id override (must exist in providers.json).
    pub provider: Option<String>,
    /// Model key / id / alias for the selected provider.
    pub model: Option<String>,
    /// `inherit`, `none`, or minimal|low|medium|high.
    pub thinking: Option<String>,
    /// Extra allowlist on top of the scope's tool contract. `null` = contract
    /// default; an empty list explicitly disables all tools.
    pub tools: Option<Vec<String>>,
    /// Skill names to include. `null` = all discovered skills.
    pub skills: Option<Vec<String>>,
    /// MCP servers (`["github"]`) or null for all configured servers.
    pub mcp: Option<Vec<String>>,
    /// Replaces the built-in agent preamble. Harness/tool/safety text still
    /// applies.
    pub system_prompt: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct EffectiveAgent {
    pub key: String,
    pub display: String,
    pub tier: String,
    pub provider: String,
    pub provider_label: String,
    pub base_url: String,
    pub model_key: String,
    pub model: String,
    pub thinking: Option<String>,
    /// Whether the selected model accepts a reasoning-effort parameter.
    pub reasoning: bool,
    pub tools: Option<Vec<String>>,
    pub skills: Option<Vec<String>>,
    pub mcp: Option<Vec<String>>,
    pub system_prompt: Option<String>,
    /// Environment variable names that can hold this provider's key.
    pub key_env: Vec<String>,
    pub has_key: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProviderSummary {
    pub id: String,
    pub label: String,
    pub base_url: String,
    pub key_env: Vec<String>,
    pub has_key: bool,
    pub default_model: String,
    pub models: Vec<ModelSummary>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ModelSummary {
    pub key: String,
    pub id: String,
    pub label: String,
    pub context: Option<usize>,
    pub reasoning: bool,
    pub aliases: Vec<String>,
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("unknown provider '{0}'")]
    UnknownProvider(String),
    #[error("provider '{provider}' has no model matching '{model}' (available: {})", available.join(", "))]
    UnknownModel {
        provider: String,
        model: String,
        available: Vec<String>,
    },
    #[error("provider '{0}' has no models")]
    NoModels(String),
    #[error("invalid thinking level '{level}' (use inherit/none/minimal/low/medium/high)")]
    InvalidThinking { level: String },
    #[error("invalid agent key '{0}' (lowercase letters, digits, '_' or '-')")]
    InvalidAgentKey(String),
    #[error("{0}")]
    Io(String),
    #[error("{0}")]
    Parse(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Signature {
    providers: Option<(u64, u128)>,
    agents: BTreeMap<String, (u64, u128)>,
}

impl Signature {
    fn read(root: &Path) -> Self {
        let dir = root.join(".deep_agents").join("config");
        let providers = file_stamp(&dir.join("providers.json"));
        let mut agents = BTreeMap::new();
        if let Ok(entries) = std::fs::read_dir(dir.join("agents")) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|value| value.to_str()) != Some("json") {
                    continue;
                }
                if let (Some(name), Some(stamp)) = (
                    path.file_stem().and_then(|value| value.to_str()),
                    file_stamp(&path),
                ) {
                    agents.insert(name.to_ascii_lowercase(), stamp);
                }
            }
        }
        Self { providers, agents }
    }
}

fn file_stamp(path: &Path) -> Option<(u64, u128)> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((metadata.len(), modified))
}

pub fn builtin_effective(tier: &str) -> EffectiveAgent {
    let providers = ProvidersFile::builtin();
    let provider_id = providers.active_id();
    let provider = providers
        .provider(&provider_id)
        .expect("builtin provider exists");
    let tier_spec = providers.tier(&provider_id, tier);
    let requested = tier_spec
        .map(|spec| spec.model.clone())
        .filter(|model| !model.trim().is_empty())
        .unwrap_or_else(|| provider.default_model.clone());
    let (model_key, model) = providers
        .resolve_model(&provider_id, &requested)
        .unwrap_or_else(|_| (requested.clone(), requested));
    let reasoning = provider
        .models
        .get(&model_key)
        .is_some_and(|model| model.reasoning);
    EffectiveAgent {
        key: "default".to_string(),
        display: "Default".to_string(),
        tier: tier.to_string(),
        provider: provider_id,
        provider_label: provider.label().to_string(),
        base_url: provider.base_url.clone(),
        model_key,
        model,
        thinking: tier_spec
            .map(|spec| spec.thinking.trim())
            .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("inherit"))
            .map(|value| value.to_ascii_lowercase()),
        reasoning,
        tools: None,
        skills: None,
        mcp: None,
        system_prompt: None,
        key_env: provider.key_names(),
        has_key: false,
    }
}

struct Loaded {
    providers: ProvidersFile,
    agents: BTreeMap<String, AgentFile>,
    issues: Vec<String>,
    /// True when a providers.json exists (so set_active can preserve it).
    has_file: bool,
}

/// Thread-safe config store. Re-reads the files when their mtimes change, so
/// edits apply without restarting the app.
#[derive(Clone)]
pub struct ConfigStore {
    root: PathBuf,
    state: Arc<RwLock<Loaded>>,
    signature: Arc<Mutex<Signature>>,
}

impl ConfigStore {
    pub fn open(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let signature = Signature::read(&root);
        let loaded = Self::load(&root, &signature);
        Self {
            root,
            state: Arc::new(RwLock::new(loaded)),
            signature: Arc::new(Mutex::new(signature)),
        }
    }

    fn load(root: &Path, _signature: &Signature) -> Loaded {
        let dir = root.join(".deep_agents").join("config");
        let providers_path = dir.join("providers.json");
        let mut issues = Vec::new();
        let mut has_file = false;
        let providers = match std::fs::read_to_string(&providers_path) {
            Ok(text) => {
                has_file = true;
                match parse_providers(&text) {
                    Ok(mut providers) => {
                        merge_builtin_providers(&mut providers);
                        providers
                    }
                    Err(error) => {
                        issues.push(format!("{}: {error}", providers_path.display()));
                        ProvidersFile::builtin()
                    }
                }
            }
            Err(_) => ProvidersFile::builtin(),
        };

        let mut agents = BTreeMap::new();
        let agents_dir = dir.join("agents");
        if let Ok(entries) = std::fs::read_dir(&agents_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|value| value.to_str()) != Some("json") {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
                    continue;
                };
                let key = stem.to_ascii_lowercase();
                match std::fs::read_to_string(&path)
                    .map_err(|error| error.to_string())
                    .and_then(|text| {
                        serde_json::from_str::<AgentFile>(&text).map_err(|error| error.to_string())
                    }) {
                    Ok(mut agent) => {
                        if agent.key.is_empty() {
                            agent.key = key.clone();
                        } else if !agent.key.eq_ignore_ascii_case(&key) {
                            issues.push(format!(
                                "{}: key '{}' does not match filename '{key}'",
                                path.display(),
                                agent.key
                            ));
                        }
                        agent.key = key.clone();
                        if let Err(error) = validate_agent(&providers, &agent) {
                            issues.push(format!("{}: {error}", path.display()));
                        }
                        agents.insert(key, agent);
                    }
                    Err(error) => {
                        issues.push(format!("{}: {error}", path.display()));
                    }
                }
            }
        }
        issues.extend(validate_providers(&providers));
        Loaded {
            providers,
            agents,
            issues,
            has_file,
        }
    }

    fn ensure_fresh(&self) {
        let signature = Signature::read(&self.root);
        let changed = self
            .signature
            .lock()
            .map(|current| *current != signature)
            .unwrap_or(true);
        if !changed {
            return;
        }
        let loaded = Self::load(&self.root, &signature);
        if let Ok(mut state) = self.state.write() {
            *state = loaded;
        }
        if let Ok(mut current) = self.signature.lock() {
            *current = signature;
        }
    }

    pub fn providers(&self) -> ProvidersFile {
        self.ensure_fresh();
        self.state
            .read()
            .map(|state| state.providers.clone())
            .unwrap_or_else(|_| ProvidersFile::builtin())
    }

    pub fn agent(&self, key: &str) -> Option<AgentFile> {
        self.ensure_fresh();
        self.state.read().ok().and_then(|state| {
            state
                .agents
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(key))
                .map(|(_, agent)| agent.clone())
        })
    }

    pub fn issues(&self) -> Vec<String> {
        self.ensure_fresh();
        self.state
            .read()
            .map(|state| state.issues.clone())
            .unwrap_or_default()
    }

    /// Resolve the effective provider/model/effort for an agent.
    ///
    /// Precedence: agent override > tier matrix > provider default.
    pub fn effective(&self, key: &str, default_tier: &str) -> Result<EffectiveAgent, ConfigError> {
        self.ensure_fresh();
        let state = self
            .state
            .read()
            .map_err(|_| ConfigError::Io("config lock poisoned".to_string()))?;
        let providers = &state.providers;
        let agent = state
            .agents
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, agent)| agent.clone())
            .unwrap_or_default();

        let provider_id = agent
            .provider
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                providers
                    .provider_id(value)
                    .ok_or_else(|| ConfigError::UnknownProvider(value.to_string()))
            })
            .transpose()?
            .unwrap_or_else(|| providers.active_id());
        let provider = providers
            .provider(&provider_id)
            .ok_or_else(|| ConfigError::UnknownProvider(provider_id.clone()))?;

        let tier = if agent.tier.trim().is_empty() {
            if default_tier.trim().is_empty() {
                "workspace".to_string()
            } else {
                default_tier.trim().to_ascii_lowercase()
            }
        } else {
            agent.tier.trim().to_ascii_lowercase()
        };
        let tier_spec = providers.tier(&provider_id, &tier);

        let requested_model = agent
            .model
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| tier_spec.map(|spec| spec.model.clone()))
            .unwrap_or_else(|| provider.default_model.clone());
        let (model_key, model_id) = providers.resolve_model(&provider_id, &requested_model)?;
        let reasoning = provider
            .models
            .get(&model_key)
            .is_some_and(|model| model.reasoning);

        let thinking = match agent
            .thinking
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Some(value) if !value.eq_ignore_ascii_case("inherit") => validate_thinking(value)?,
            _ => match tier_spec.map(|spec| spec.thinking.trim()) {
                Some(value) if !value.is_empty() && !value.eq_ignore_ascii_case("inherit") => {
                    validate_thinking(value)?
                }
                _ => None,
            },
        };

        let key_env = provider.key_names();
        // Resolve keys directly from the provider spec: calling
        // `resolve_key_with` here would re-enter the state lock.
        let has_key = key_env.iter().any(|name| {
            std::env::var(name)
                .ok()
                .is_some_and(|value| value.split(',').any(|part| !part.trim().is_empty()))
        });

        Ok(EffectiveAgent {
            key: key.to_ascii_lowercase(),
            display: if agent.display.is_empty() {
                key.to_string()
            } else {
                agent.display.clone()
            },
            tier,
            provider: provider_id,
            provider_label: provider.label().to_string(),
            base_url: provider.base_url.clone(),
            model_key,
            model: model_id,
            thinking,
            reasoning,
            tools: agent.tools.clone(),
            skills: agent.skills.clone(),
            mcp: agent.mcp.clone(),
            system_prompt: agent.system_prompt.clone(),
            key_env,
            has_key,
        })
    }

    /// Resolve a provider's key from its configured env names. Comma-separated
    /// values are accepted (legacy `*_API_KEYS` lists).
    pub fn resolve_key(&self, provider_id: &str) -> String {
        self.resolve_key_with(provider_id, |name| std::env::var(name).ok())
    }

    pub fn resolve_key_with(
        &self,
        provider_id: &str,
        get: impl Fn(&str) -> Option<String>,
    ) -> String {
        let providers = self.providers();
        let Some(provider) = providers.provider(provider_id) else {
            return String::new();
        };
        for name in provider.key_names() {
            if let Some(value) = get(&name)
                && let Some(first) = value
                    .split(',')
                    .map(str::trim)
                    .find(|candidate| !candidate.is_empty())
            {
                return first.to_string();
            }
        }
        String::new()
    }

    /// Resolve a model key/id/alias for a given provider to `(key, id)`.
    pub fn resolve_model(
        &self,
        provider_id: &str,
        requested: &str,
    ) -> Result<(String, String), ConfigError> {
        self.providers().resolve_model(provider_id, requested)
    }

    pub fn active_provider(&self) -> String {
        self.providers().active_id()
    }

    pub fn provider_summaries(&self) -> Vec<ProviderSummary> {
        let providers = self.providers();
        let mut out = Vec::new();
        for (id, provider) in &providers.providers {
            let models = provider
                .models
                .iter()
                .map(|(key, model)| ModelSummary {
                    key: key.clone(),
                    id: model.id.clone(),
                    label: if model.label.is_empty() {
                        model.id.clone()
                    } else {
                        model.label.clone()
                    },
                    context: model.context,
                    reasoning: model.reasoning,
                    aliases: model.aliases.clone(),
                })
                .collect();
            out.push(ProviderSummary {
                id: id.clone(),
                label: provider.label().to_string(),
                base_url: provider.base_url.clone(),
                key_env: provider.key_names(),
                has_key: !self.resolve_key(id).is_empty(),
                default_model: provider.default_model.clone(),
                models,
            });
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// Persist a new active provider. Materializes the built-in config on the
    /// first switch so the choice survives restarts.
    pub fn set_active(&self, provider_id: &str) -> Result<(), ConfigError> {
        let providers = self.providers();
        let Some(canonical) = providers.provider_id(provider_id) else {
            return Err(ConfigError::UnknownProvider(provider_id.to_string()));
        };
        let mut file = providers;
        file.active = canonical;
        self.write_providers(&file)
    }

    pub fn save_agent(&self, key: &str, mut agent: AgentFile) -> Result<(), ConfigError> {
        let key = key.trim().to_ascii_lowercase();
        if !valid_agent_key(&key) {
            return Err(ConfigError::InvalidAgentKey(key));
        }
        let providers = self.providers();
        agent.key = key.clone();
        if agent.version == 0 {
            agent.version = CONFIG_VERSION;
        }
        validate_agent(&providers, &agent)?;
        let dir = self.root.join(".deep_agents").join("config").join("agents");
        std::fs::create_dir_all(&dir).map_err(|error| ConfigError::Io(error.to_string()))?;
        let text = serde_json::to_string_pretty(&agent)
            .map_err(|error| ConfigError::Parse(error.to_string()))?;
        std::fs::write(dir.join(format!("{key}.json")), text)
            .map_err(|error| ConfigError::Io(error.to_string()))?;
        self.reload();
        Ok(())
    }

    pub fn delete_agent(&self, key: &str) -> Result<(), ConfigError> {
        let key = key.trim().to_ascii_lowercase();
        let path = self
            .root
            .join(".deep_agents")
            .join("config")
            .join("agents")
            .join(format!("{key}.json"));
        match std::fs::remove_file(path) {
            Ok(()) => {
                self.reload();
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(ConfigError::Io(error.to_string())),
        }
    }

    pub fn write_providers(&self, file: &ProvidersFile) -> Result<(), ConfigError> {
        let dir = self.root.join(".deep_agents").join("config");
        std::fs::create_dir_all(&dir).map_err(|error| ConfigError::Io(error.to_string()))?;
        validate_providers(file)
            .into_iter()
            .next()
            .map_or(Ok(()), |error| Err(ConfigError::Parse(error)))?;
        let text = serde_json::to_string_pretty(file)
            .map_err(|error| ConfigError::Parse(error.to_string()))?;
        std::fs::write(dir.join("providers.json"), text)
            .map_err(|error| ConfigError::Io(error.to_string()))?;
        self.reload();
        Ok(())
    }

    pub fn reload(&self) {
        let signature = Signature::read(&self.root);
        let loaded = Self::load(&self.root, &signature);
        if let Ok(mut state) = self.state.write() {
            *state = loaded;
        }
        if let Ok(mut current) = self.signature.lock() {
            *current = signature;
        }
    }

    pub fn has_file(&self) -> bool {
        self.ensure_fresh();
        self.state
            .read()
            .map(|state| state.has_file)
            .unwrap_or(false)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

fn parse_providers(text: &str) -> Result<ProvidersFile, String> {
    serde_json::from_str::<ProvidersFile>(text).map_err(|error| error.to_string())
}

/// Keep the shipped providers (and their aliases) available even when the file
/// only overrides one entry. File values win.
fn merge_builtin_providers(file: &mut ProvidersFile) {
    let builtin = ProvidersFile::builtin();
    for (id, provider) in builtin.providers {
        match file.providers.get_mut(&id) {
            Some(existing) => {
                if existing.label.is_empty() {
                    existing.label = provider.label;
                }
                if existing.base_url.trim().is_empty() {
                    existing.base_url = provider.base_url;
                }
                if existing.key_env.is_none() {
                    existing.key_env = provider.key_env;
                }
                if existing.default_model.trim().is_empty() {
                    existing.default_model = provider.default_model;
                }
                if existing.kind.trim().is_empty() {
                    existing.kind = provider.kind;
                }
                for (model_key, model) in provider.models {
                    existing.models.entry(model_key).or_insert(model);
                }
            }
            None => {
                file.providers.insert(id, provider);
            }
        }
    }
    for (provider, tiers) in builtin.matrix {
        file.matrix.entry(provider).or_insert(tiers);
    }
}

fn validate_providers(file: &ProvidersFile) -> Vec<String> {
    let mut issues = Vec::new();
    if file.providers.is_empty() {
        issues.push("no providers configured".to_string());
    }
    if file.provider(&file.active).is_none() {
        issues.push(format!(
            "active provider '{}' is not defined; using the first provider",
            file.active
        ));
    }
    for (id, provider) in &file.providers {
        if provider.base_url.trim().is_empty() {
            issues.push(format!("provider '{id}' has an empty base_url"));
        }
        if provider.models.is_empty() {
            issues.push(format!("provider '{id}' has no models"));
        } else if !provider.default_model.is_empty()
            && !provider.models.keys().any(|key| {
                key.eq_ignore_ascii_case(&provider.default_model)
                    || provider
                        .models
                        .get(key)
                        .is_some_and(|model| model.matches(&provider.default_model))
            })
        {
            issues.push(format!(
                "provider '{id}' default_model '{}' is not in its models",
                provider.default_model
            ));
        }
        if provider.key_names().is_empty() {
            issues.push(format!("provider '{id}' has no key_env"));
        }
    }
    for (provider_id, tiers) in &file.matrix {
        if file.provider(provider_id).is_none() {
            issues.push(format!(
                "matrix references unknown provider '{provider_id}'"
            ));
        }
        for (tier, spec) in tiers {
            if !spec.thinking.trim().is_empty()
                && !spec.thinking.eq_ignore_ascii_case("inherit")
                && validate_thinking(&spec.thinking).is_err()
            {
                issues.push(format!(
                    "matrix {provider_id}/{tier} has invalid thinking '{}'",
                    spec.thinking
                ));
            }
            if !spec.model.trim().is_empty()
                && file.resolve_model(provider_id, &spec.model).is_err()
            {
                issues.push(format!(
                    "matrix {provider_id}/{tier} references unknown model '{}'",
                    spec.model
                ));
            }
        }
    }
    issues
}

fn validate_agent(providers: &ProvidersFile, agent: &AgentFile) -> Result<(), ConfigError> {
    if let Some(provider) = agent
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        && providers.provider(provider).is_none()
    {
        return Err(ConfigError::UnknownProvider(provider.to_string()));
    }
    if let Some(thinking) = agent
        .thinking
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        validate_thinking(thinking)?;
    }
    if let Some(model) = agent
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let provider_id = agent
            .provider
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| providers.active_id());
        providers.resolve_model(&provider_id, model)?;
    }
    Ok(())
}

fn validate_thinking(level: &str) -> Result<Option<String>, ConfigError> {
    let level = level.trim().to_ascii_lowercase();
    match level.as_str() {
        "" | "inherit" | "none" | "disabled" => Ok(None),
        "minimal" | "low" | "medium" | "high" => Ok(Some(level)),
        _ => Err(ConfigError::InvalidThinking { level }),
    }
}

fn valid_agent_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '_'
                || character == '-'
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> (tempfile::TempDir, ConfigStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::open(dir.path());
        (dir, store)
    }

    #[test]
    fn builtin_defaults_to_opencode_flash() {
        let (_dir, store) = temp_root();
        let effective = store.effective("developer", "workspace").unwrap();
        assert_eq!(effective.provider, "opencode");
        assert_eq!(effective.base_url, "https://opencode.ai/zen/go/v1");
        assert_eq!(effective.model, "deepseek-v4.1-flash");
        assert_eq!(effective.thinking.as_deref(), Some("high"));
        assert!(effective.key_env.contains(&"OPENCODE_API_KEY".to_string()));
    }

    #[test]
    fn model_aliases_resolve_to_canonical_ids() {
        let providers = ProvidersFile::builtin();
        let (key, id) = providers
            .resolve_model("opencode", "deepseek/deepseek-v4.1-flash-1017")
            .unwrap();
        assert_eq!(key, "flash");
        assert_eq!(id, "deepseek-v4.1-flash");
        let (key, id) = providers.resolve_model("opencode", "flash").unwrap();
        assert_eq!(key, "flash");
        assert_eq!(id, "deepseek-v4.1-flash");
    }

    #[test]
    fn unknown_model_is_an_error_not_a_silent_pass_through() {
        let providers = ProvidersFile::builtin();
        let error = providers
            .resolve_model("opencode", "someprovider/not-real")
            .unwrap_err();
        assert!(error.to_string().contains("no model matching"));
    }

    #[test]
    fn agent_can_override_provider_model_and_thinking() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let config = root.join(".deep_agents/config/agents");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("developer.json"),
            r#"{
                "provider": "openrouter",
                "model": "flash",
                "thinking": "low"
            }"#,
        )
        .unwrap();
        let store = ConfigStore::open(root);
        let effective = store.effective("developer", "workspace").unwrap();
        assert_eq!(effective.provider, "openrouter");
        assert_eq!(effective.base_url, "https://openrouter.ai/api/v1");
        assert_eq!(effective.model, "deepseek/deepseek-v4.1-flash-1017");
        assert_eq!(effective.thinking.as_deref(), Some("low"));
    }

    #[test]
    fn thinking_inherits_from_matrix_when_absent() {
        let (_dir, store) = temp_root();
        let effective = store.effective("orchestrator", "orchestrator").unwrap();
        assert_eq!(effective.thinking.as_deref(), Some("low"));
        let effective = store.effective("devops", "specialist").unwrap();
        assert_eq!(effective.thinking.as_deref(), Some("medium"));
    }

    #[test]
    fn env_key_resolution_is_provider_specific_and_comma_aware() {
        let (_dir, store) = temp_root();
        let key = store.resolve_key_with("opencode", |name| match name {
            "OPENCODE_API_KEY" => None,
            "DEEPSEEK_API_KEY" => Some(" first-key , second ".to_string()),
            _ => None,
        });
        assert_eq!(key, "first-key");
        let key = store.resolve_key_with("openrouter", |name| {
            (name == "OPENROUTER_API_KEYS").then(|| "a,b".to_string())
        });
        assert_eq!(key, "a");
    }

    #[test]
    fn saving_agent_persists_and_hot_reloads() {
        let (dir, store) = temp_root();
        store
            .save_agent(
                "developer",
                AgentFile {
                    model: Some("mimo-flash".to_string()),
                    thinking: Some("minimal".to_string()),
                    ..AgentFile::default()
                },
            )
            .unwrap();
        assert!(
            dir.path()
                .join(".deep_agents/config/agents/developer.json")
                .is_file()
        );
        let effective = store.effective("developer", "workspace").unwrap();
        assert_eq!(effective.model, "mimo-v2.6-flash");
        assert_eq!(effective.thinking.as_deref(), Some("minimal"));
    }

    #[test]
    fn invalid_provider_file_falls_back_and_records_issue() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join(".deep_agents/config");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("providers.json"), "{ not json").unwrap();
        let store = ConfigStore::open(dir.path());
        assert!(!store.issues().is_empty());
        assert_eq!(
            store.effective("developer", "workspace").unwrap().provider,
            "opencode"
        );
    }

    #[test]
    fn setting_active_materializes_the_file() {
        let (dir, store) = temp_root();
        store.set_active("openrouter").unwrap();
        assert!(
            dir.path()
                .join(".deep_agents/config/providers.json")
                .is_file()
        );
        assert_eq!(
            store.effective("developer", "workspace").unwrap().provider,
            "openrouter"
        );
    }
}
