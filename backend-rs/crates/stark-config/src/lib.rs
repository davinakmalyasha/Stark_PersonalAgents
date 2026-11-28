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
//!
//! API keys managed from the app live in
//! `.deep_agents/secrets/provider_keys.json`. The file is deliberately outside
//! the config directory so portable backups never pick it up; one active key
//! per provider wins over the `.env` fallback.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const CONFIG_VERSION: u32 = 1;
/// Agent definitions are versioned independently from the provider file. v1
/// files remain readable; every save writes v2.
pub const AGENT_VERSION: u32 = 2;
pub const THINKING_LEVELS: &[&str] = &["minimal", "low", "medium", "high"];

const PROVIDER_KEYS_FILE: &str = "provider_keys.json";
const PROVIDER_KEYS_DIR: &str = "secrets";

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

/// One saved API key. The secret never leaves the config store except through
/// [`ConfigStore::resolve_key`]; API responses use [`ProviderKeyInfo`].
#[derive(Clone, Deserialize, Serialize)]
struct ProviderKeyEntry {
    id: String,
    #[serde(default)]
    label: String,
    secret: String,
    #[serde(default)]
    created_ms: u64,
}

impl std::fmt::Debug for ProviderKeyEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderKeyEntry")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("secret", &"<redacted>")
            .field("created_ms", &self.created_ms)
            .finish()
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct ProviderKeySet {
    /// Selected managed key id. Empty means the `.env` fallback.
    #[serde(default)]
    active: String,
    #[serde(default)]
    keys: Vec<ProviderKeyEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ProviderKeysFile {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    providers: BTreeMap<String, ProviderKeySet>,
}

impl Default for ProviderKeysFile {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            providers: BTreeMap::new(),
        }
    }
}

/// Public, secret-free key metadata. `masked` is safe to render and log.
#[derive(Clone, Debug, Serialize)]
pub struct ProviderKeyInfo {
    pub id: String,
    pub label: String,
    pub masked: String,
    /// `managed` or `env`.
    pub source: String,
    pub active: bool,
    pub created_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProviderKeysReport {
    pub provider: String,
    pub keys: Vec<ProviderKeyInfo>,
    /// Managed key id, `env`, or empty when no key is configured.
    pub active: String,
    pub has_env: bool,
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
    /// OpenCode freeform tool formats this model accepts: `text`, `lark`,
    /// and/or `regex`. Empty keeps JSON function tools.
    #[serde(default)]
    pub freeform: Vec<String>,
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
                            freeform: Vec::new(),
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
                            freeform: Vec::new(),
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
                            freeform: Vec::new(),
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
                            freeform: Vec::new(),
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
                        freeform: Vec::new(),
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
                        freeform: Vec::new(),
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
                        freeform: Vec::new(),
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
        if requested.is_empty()
            || requested.eq_ignore_ascii_case("Automatic Fallback")
            || requested.eq_ignore_ascii_case("auto")
        {
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

/// Prompt selection + additive persona for an agent definition.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentPromptFile {
    /// `coding` uses the Developer composer; `general` uses the generic
    /// composer. `null` keeps the built-in profile (or `general` for custom).
    pub profile: Option<String>,
    /// Appended after the harness/safety text. It can specialize the agent but
    /// can never remove code-enforced guards.
    pub persona: Option<String>,
}

/// Delegation capabilities. All default to false for custom agents.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentDelegationFile {
    /// May be selected as a `delegate` target.
    pub delegatable: Option<bool>,
    /// May call `delegate` / inspect delegation state.
    pub can_delegate: Option<bool>,
    /// May spawn durable child agents.
    pub can_spawn: Option<bool>,
}

/// UI metadata for the agent picker.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentUiFile {
    pub visible: Option<bool>,
    pub order: Option<i32>,
}

/// Opt-in routing for custom agents. Empty fields match any value; both
/// non-empty means both must match.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentRoutingFile {
    #[serde(default)]
    pub workspace_kinds: Vec<String>,
    #[serde(default)]
    pub paths: Vec<String>,
}

/// Per-agent overrides for built-ins, or a full custom definition.
///
/// `null` or a missing field means inherit. For custom agents the runtime
/// closes the surface: a missing `tools` list means no tools, and missing
/// `skills`/`mcp` means none.
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
    pub description: String,
    #[serde(default)]
    pub tier: String,
    /// Provider id override (must exist in providers.json).
    pub provider: Option<String>,
    /// Model key / id / alias for the selected provider.
    pub model: Option<String>,
    /// `inherit`, `none`, or minimal|low|medium|high.
    pub thinking: Option<String>,
    /// Built-ins: extra allowlist on top of the scope's tool contract.
    /// Custom agents: the complete, closed tool list.
    pub tools: Option<Vec<String>>,
    /// Skill names to include. Built-ins: `null` = all discovered skills.
    /// Custom: `null` = none.
    pub skills: Option<Vec<String>>,
    /// MCP servers. Built-ins: `null` = all configured servers. Custom:
    /// `null` = none.
    pub mcp: Option<Vec<String>>,
    /// Legacy v1 persona. v2 writes `prompt.persona`; when both are present
    /// the request is rejected so there is one source of truth.
    pub system_prompt: Option<String>,
    /// Prompt profile + persona (v2).
    pub prompt: Option<AgentPromptFile>,
    /// Built-ins: `read`/`write`/`worktree` must match the compiled cap.
    /// Custom: `read` (default) or `write`.
    pub access: Option<String>,
    /// Extra compatibility names. Aliases are never listed as agents.
    pub aliases: Option<Vec<String>>,
    pub delegation: Option<AgentDelegationFile>,
    pub ui: Option<AgentUiFile>,
    /// Custom-agent routing. Built-ins keep compiled routing.
    pub routing: Option<AgentRoutingFile>,
    /// Code Mode partition: `off`, `hybrid`, or `only`. `null` inherits the
    /// scope default.
    pub code_mode: Option<String>,
    /// OpenCode `full_codemode`: Bun/path globals, `timeout`/`background`
    /// script jobs, and the pinned file/shell catalog.
    pub full_codemode: Option<bool>,
}

#[derive(Clone, Debug, Serialize)]
pub struct EffectiveAgent {
    pub key: String,
    pub display: String,
    pub description: String,
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
    /// `coding` or `general`.
    pub prompt_profile: String,
    /// Additive persona text (v2 `prompt.persona` or legacy `system_prompt`).
    pub persona: Option<String>,
    /// Stable key aliases (custom agents only; built-ins come from AgentSpec).
    pub aliases: Vec<String>,
    /// Resolved root access: `read`, `write`, or `worktree`.
    pub access: String,
    /// Resolved spawned/delegated access.
    pub access_spawned: String,
    /// True for the eight compiled agents, false for custom definitions.
    pub builtin: bool,
    pub delegatable: bool,
    pub can_delegate: bool,
    pub can_spawn: bool,
    pub ui_visible: bool,
    pub ui_order: i32,
    /// Custom-agent routing filters. Empty for built-ins unless compiled.
    pub routing_workspace_kinds: Vec<String>,
    pub routing_paths: Vec<String>,
    /// Resolved Code Mode partition (`off`, `hybrid`, or `only`).
    pub code_mode: Option<String>,
    /// Resolved OpenCode `full_codemode` override. `None` inherits the scope
    /// default (coding agents default to on).
    pub full_codemode: Option<bool>,
    /// OpenCode freeform formats advertised by the selected model.
    pub freeform: Vec<String>,
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
    pub freeform: Vec<String>,
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
    #[error("invalid code mode '{value}' (use off/hybrid/only)")]
    InvalidCodeMode { value: String },
    #[error("invalid agent key '{0}' (lowercase letters, digits, '_' or '-')")]
    InvalidAgentKey(String),
    #[error("invalid agent alias '{0}' (lowercase letters, digits, '_' or '-')")]
    InvalidAgentAlias(String),
    #[error("invalid access '{0}' (use read or write; worktree is for spawned built-ins)")]
    InvalidAccess(String),
    #[error("invalid prompt profile '{0}' (use coding or general)")]
    InvalidPromptProfile(String),
    #[error("invalid API key: {0}")]
    InvalidProviderKey(String),
    #[error("unknown API key '{0}'")]
    UnknownProviderKey(String),
    #[error("{0}")]
    Io(String),
    #[error("{0}")]
    Parse(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Signature {
    providers: Option<(u64, u128)>,
    agents: BTreeMap<String, (u64, u128)>,
    keys: Option<(u64, u128)>,
}

impl Signature {
    fn read(root: &Path) -> Self {
        let dir = root.join(".deep_agents").join("config");
        let providers = file_stamp(&dir.join("providers.json"));
        let keys = file_stamp(&provider_keys_path(root));
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
        Self {
            providers,
            agents,
            keys,
        }
    }
}

fn provider_keys_path(root: &Path) -> PathBuf {
    root.join(".deep_agents")
        .join(PROVIDER_KEYS_DIR)
        .join(PROVIDER_KEYS_FILE)
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn mask_key(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() <= 4 {
        return "…".to_string();
    }
    let suffix: String = chars[chars.len() - 4..].iter().collect();
    let prefix = if secret.starts_with("oc_") {
        "oc_"
    } else if secret.starts_with("sk-") {
        "sk-"
    } else {
        ""
    };
    format!("{prefix}…{suffix}")
}

fn env_key_for(provider: &ProviderSpec, get: impl Fn(&str) -> Option<String>) -> String {
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
    let freeform = provider
        .models
        .get(&model_key)
        .map(|model| model.freeform.clone())
        .unwrap_or_default();
    EffectiveAgent {
        key: "default".to_string(),
        display: "Default".to_string(),
        description: String::new(),
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
        freeform,
        tools: None,
        skills: None,
        mcp: None,
        system_prompt: None,
        prompt_profile: "general".to_string(),
        persona: None,
        aliases: Vec::new(),
        access: "read".to_string(),
        access_spawned: "read".to_string(),
        builtin: false,
        delegatable: false,
        can_delegate: false,
        can_spawn: false,
        ui_visible: true,
        ui_order: 0,
        routing_workspace_kinds: Vec::new(),
        routing_paths: Vec::new(),
        code_mode: None,
        full_codemode: None,
        key_env: provider.key_names(),
        has_key: false,
    }
}

struct Loaded {
    providers: ProvidersFile,
    agents: BTreeMap<String, AgentFile>,
    keys: ProviderKeysFile,
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
        let keys_path = provider_keys_path(root);
        let keys = match std::fs::read_to_string(&keys_path) {
            Ok(text) => match serde_json::from_str::<ProviderKeysFile>(&text) {
                Ok(keys) => keys,
                Err(error) => {
                    issues.push(format!("{}: {error}", keys_path.display()));
                    ProviderKeysFile::default()
                }
            },
            Err(_) => ProviderKeysFile::default(),
        };
        issues.extend(validate_providers(&providers));
        Loaded {
            providers,
            agents,
            keys,
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

    /// Every parsed agent file, sorted by filename key. Invalid files remain
    /// visible here so the registry/API can report exactly what is wrong; they
    /// are never applied to a run.
    pub fn agent_files(&self) -> Vec<(String, AgentFile)> {
        self.ensure_fresh();
        self.state
            .read()
            .map(|state| {
                state
                    .agents
                    .iter()
                    .map(|(key, agent)| (key.clone(), agent.clone()))
                    .collect()
            })
            .unwrap_or_default()
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
        let agent = state
            .agents
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, agent)| agent.clone())
            .unwrap_or_default();
        Self::effective_from_state(&state, key, default_tier, &agent)
    }

    /// Resolve an unsaved draft against the live provider table. Used by
    /// preview/save validation so the UI sees the exact effective record.
    pub fn effective_for_file(
        &self,
        key: &str,
        default_tier: &str,
        agent: &AgentFile,
    ) -> Result<EffectiveAgent, ConfigError> {
        self.ensure_fresh();
        let state = self
            .state
            .read()
            .map_err(|_| ConfigError::Io("config lock poisoned".to_string()))?;
        Self::effective_from_state(&state, key, default_tier, agent)
    }

    fn effective_from_state(
        state: &Loaded,
        key: &str,
        default_tier: &str,
        agent: &AgentFile,
    ) -> Result<EffectiveAgent, ConfigError> {
        let providers = &state.providers;
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

        let freeform = provider
            .models
            .get(&model_key)
            .map(|model| model.freeform.clone())
            .unwrap_or_default();
        let key_env = provider.key_names();
        // Resolve keys from the state we already hold: calling `resolve_key`
        // here would re-enter the lock.
        let has_key = !Self::resolve_key_from(state, &provider_id).is_empty();

        let persona = agent
            .prompt
            .as_ref()
            .and_then(|prompt| prompt.persona.clone())
            .or_else(|| agent.system_prompt.clone())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let prompt_profile = agent
            .prompt
            .as_ref()
            .and_then(|prompt| prompt.profile.as_deref())
            .map(|value| match value.trim().to_ascii_lowercase().as_str() {
                "plain" => "general".to_string(),
                other => other.to_string(),
            })
            .unwrap_or_else(|| "general".to_string());
        let access = agent
            .access
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| match value.to_ascii_lowercase().as_str() {
                "readonly" | "read-only" => "read".to_string(),
                other => other.to_string(),
            })
            .unwrap_or_else(|| "read".to_string());
        let access_spawned = if access == "read" {
            "read".to_string()
        } else {
            "worktree".to_string()
        };
        let delegation = agent.delegation.clone().unwrap_or_default();
        let ui = agent.ui.clone().unwrap_or_default();
        let routing = agent.routing.clone().unwrap_or_default();
        Ok(EffectiveAgent {
            key: key.to_ascii_lowercase(),
            display: if agent.display.is_empty() {
                key.to_string()
            } else {
                agent.display.clone()
            },
            description: agent.description.clone(),
            tier,
            provider: provider_id,
            provider_label: provider.label().to_string(),
            base_url: provider.base_url.clone(),
            model_key,
            model: model_id,
            thinking,
            reasoning,
            freeform,
            tools: agent.tools.clone(),
            skills: agent.skills.clone(),
            mcp: agent.mcp.clone(),
            system_prompt: agent.system_prompt.clone(),
            prompt_profile,
            persona,
            aliases: agent.aliases.clone().unwrap_or_default(),
            access,
            access_spawned,
            builtin: false,
            delegatable: delegation.delegatable.unwrap_or(false),
            can_delegate: delegation.can_delegate.unwrap_or(false),
            can_spawn: delegation.can_spawn.unwrap_or(false),
            ui_visible: ui.visible.unwrap_or(true),
            ui_order: ui.order.unwrap_or(0),
            routing_workspace_kinds: routing
                .workspace_kinds
                .iter()
                .map(|kind| kind.trim().to_ascii_lowercase())
                .filter(|kind| !kind.is_empty())
                .collect(),
            routing_paths: routing
                .paths
                .iter()
                .map(|path| path.trim().replace('\\', "/"))
                .filter(|path| !path.is_empty())
                .collect(),
            code_mode: agent.code_mode.clone(),
            full_codemode: agent.full_codemode,
            key_env,
            has_key,
        })
    }

    /// Resolve the active key for a provider: the selected managed key first,
    /// then the configured `.env` fallback.
    pub fn resolve_key(&self, provider_id: &str) -> String {
        self.ensure_fresh();
        self.state
            .read()
            .map(|state| Self::resolve_key_from(&state, provider_id))
            .unwrap_or_default()
    }

    /// Resolve a specific saved key, or the `.env` fallback when `key_id` is
    /// empty or `env`. Used by the usage endpoint so a click shows exactly the
    /// reports of that key.
    pub fn resolve_key_entry(&self, provider_id: &str, key_id: &str) -> String {
        self.ensure_fresh();
        let Ok(state) = self.state.read() else {
            return String::new();
        };
        let Some(provider) = state.providers.provider(provider_id) else {
            return String::new();
        };
        let id = key_id.trim();
        if id.is_empty() || id.eq_ignore_ascii_case("env") {
            return env_key_for(provider, |name| std::env::var(name).ok());
        }
        state
            .keys
            .providers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(provider_id))
            .and_then(|(_, set)| set.keys.iter().find(|key| key.id == id))
            .map(|key| key.secret.clone())
            .unwrap_or_default()
    }

    /// Environment-only resolution kept for callers that supply a key getter
    /// (tests and legacy fallbacks). Managed keys are intentionally ignored.
    pub fn resolve_key_with(
        &self,
        provider_id: &str,
        get: impl Fn(&str) -> Option<String>,
    ) -> String {
        let providers = self.providers();
        let Some(provider) = providers.provider(provider_id) else {
            return String::new();
        };
        env_key_for(provider, get)
    }

    fn resolve_key_from(state: &Loaded, provider_id: &str) -> String {
        let Some(provider) = state.providers.provider(provider_id) else {
            return String::new();
        };
        if let Some((_, set)) = state
            .keys
            .providers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(provider_id))
            && !set.active.trim().is_empty()
            && let Some(key) = set
                .keys
                .iter()
                .find(|key| key.id.eq_ignore_ascii_case(set.active.trim()))
        {
            return key.secret.clone();
        }
        env_key_for(provider, |name| std::env::var(name).ok())
    }

    /// Secret-free key list for the provider panel.
    pub fn provider_keys(&self, provider_id: &str) -> Result<ProviderKeysReport, ConfigError> {
        self.ensure_fresh();
        let state = self
            .state
            .read()
            .map_err(|_| ConfigError::Io("config lock poisoned".to_string()))?;
        Self::provider_keys_from(&state, provider_id)
    }

    fn provider_keys_from(
        state: &Loaded,
        provider_id: &str,
    ) -> Result<ProviderKeysReport, ConfigError> {
        let provider = state
            .providers
            .provider_id(provider_id)
            .ok_or_else(|| ConfigError::UnknownProvider(provider_id.to_string()))?;
        let spec = state
            .providers
            .provider(&provider)
            .ok_or_else(|| ConfigError::UnknownProvider(provider.clone()))?;
        let set = state.keys.providers.get(&provider);
        let env = env_key_for(spec, |name| std::env::var(name).ok());
        let managed_active = set
            .filter(|set| !set.active.trim().is_empty())
            .and_then(|set| {
                set.keys
                    .iter()
                    .find(|key| key.id.eq_ignore_ascii_case(set.active.trim()))
            })
            .map(|key| key.id.clone());
        let active = if let Some(id) = &managed_active {
            id.clone()
        } else if env.is_empty() {
            String::new()
        } else {
            "env".to_string()
        };
        let mut keys = Vec::new();
        if !env.is_empty() {
            keys.push(ProviderKeyInfo {
                id: "env".to_string(),
                label: "Environment (.env)".to_string(),
                masked: mask_key(&env),
                source: "env".to_string(),
                active: active == "env",
                created_ms: None,
            });
        }
        if let Some(set) = set {
            for key in &set.keys {
                keys.push(ProviderKeyInfo {
                    id: key.id.clone(),
                    label: if key.label.trim().is_empty() {
                        "API key".to_string()
                    } else {
                        key.label.clone()
                    },
                    masked: mask_key(&key.secret),
                    source: "managed".to_string(),
                    active: managed_active.as_deref() == Some(key.id.as_str()),
                    created_ms: (key.created_ms > 0).then_some(key.created_ms),
                });
            }
        }
        Ok(ProviderKeysReport {
            provider,
            keys,
            active,
            has_env: !env.is_empty(),
        })
    }

    /// Save a key and make it active. The caller is responsible for checking
    /// it against the provider before persisting.
    pub fn add_provider_key(
        &self,
        provider_id: &str,
        label: &str,
        secret: &str,
    ) -> Result<ProviderKeysReport, ConfigError> {
        let secret = clean_secret(secret)?;
        self.mutate_keys(provider_id, |set| {
            if set.keys.len() >= 20 {
                return Err(ConfigError::InvalidProviderKey(
                    "at most 20 keys can be saved per provider".to_string(),
                ));
            }
            if set.keys.iter().any(|key| key.secret == secret) {
                return Err(ConfigError::InvalidProviderKey(
                    "this key is already saved".to_string(),
                ));
            }
            let mut id = format!("key-{}", now_millis());
            while set.keys.iter().any(|key| key.id == id) {
                id.push('x');
            }
            let label = label.trim();
            let label = if label.is_empty() {
                format!("Key {}", set.keys.len() + 1)
            } else {
                label.chars().take(64).collect()
            };
            set.keys.push(ProviderKeyEntry {
                id: id.clone(),
                label,
                secret,
                created_ms: now_millis(),
            });
            set.active = id;
            Ok(())
        })
    }

    /// Select a saved key, or `env`/empty for the `.env` fallback.
    pub fn activate_provider_key(
        &self,
        provider_id: &str,
        key_id: &str,
    ) -> Result<ProviderKeysReport, ConfigError> {
        let id = key_id.trim();
        self.mutate_keys(provider_id, |set| {
            if id.is_empty() || id.eq_ignore_ascii_case("env") {
                set.active.clear();
                return Ok(());
            }
            let key = set
                .keys
                .iter()
                .find(|key| key.id.eq_ignore_ascii_case(id))
                .ok_or_else(|| ConfigError::UnknownProviderKey(id.to_string()))?;
            set.active = key.id.clone();
            Ok(())
        })
    }

    /// Remove a managed key. Removing the active key promotes the next saved
    /// key, then the `.env` fallback.
    pub fn remove_provider_key(
        &self,
        provider_id: &str,
        key_id: &str,
    ) -> Result<ProviderKeysReport, ConfigError> {
        let id = key_id.trim();
        if id.is_empty() || id.eq_ignore_ascii_case("env") {
            return Err(ConfigError::InvalidProviderKey(
                "the .env fallback cannot be removed here".to_string(),
            ));
        }
        self.mutate_keys(provider_id, |set| {
            let index = set
                .keys
                .iter()
                .position(|key| key.id.eq_ignore_ascii_case(id))
                .ok_or_else(|| ConfigError::UnknownProviderKey(id.to_string()))?;
            let removed = set.keys.remove(index);
            if set.active == removed.id {
                set.active = set
                    .keys
                    .first()
                    .map(|key| key.id.clone())
                    .unwrap_or_default();
            }
            Ok(())
        })
    }

    fn mutate_keys(
        &self,
        provider_id: &str,
        change: impl FnOnce(&mut ProviderKeySet) -> Result<(), ConfigError>,
    ) -> Result<ProviderKeysReport, ConfigError> {
        self.ensure_fresh();
        let mut state = self
            .state
            .write()
            .map_err(|_| ConfigError::Io("config lock poisoned".to_string()))?;
        let provider = state
            .providers
            .provider_id(provider_id)
            .ok_or_else(|| ConfigError::UnknownProvider(provider_id.to_string()))?;
        let mut file = state.keys.clone();
        if file.version == 0 {
            file.version = CONFIG_VERSION;
        }
        let set = file.providers.entry(provider.clone()).or_default();
        change(set)?;
        let text = serde_json::to_string_pretty(&file)
            .map_err(|error| ConfigError::Parse(error.to_string()))?;
        let path = provider_keys_path(&self.root);
        atomic_write_json(&path, &text)?;
        restrict_private(&path);
        restrict_private(&path.with_extension("json.bak"));
        state.keys = file;
        let report = Self::provider_keys_from(&state, &provider)?;
        let signature = Signature::read(&self.root);
        if let Ok(mut current) = self.signature.lock() {
            *current = signature;
        }
        Ok(report)
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
                    freeform: model.freeform.clone(),
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

    /// Validate a definition without writing it. Used by the API preview and
    /// save paths before the registry/runtime checks run.
    pub fn validate_agent(&self, agent: &AgentFile) -> Result<(), ConfigError> {
        validate_agent(&self.providers(), agent)
    }

    pub fn save_agent(&self, key: &str, mut agent: AgentFile) -> Result<(), ConfigError> {
        let key = key.trim().to_ascii_lowercase();
        if !valid_agent_key(&key) {
            return Err(ConfigError::InvalidAgentKey(key));
        }
        let providers = self.providers();
        agent.key = key.clone();
        agent.version = AGENT_VERSION;
        validate_agent(&providers, &agent)?;
        let dir = self.root.join(".deep_agents").join("config").join("agents");
        std::fs::create_dir_all(&dir).map_err(|error| ConfigError::Io(error.to_string()))?;
        let text = serde_json::to_string_pretty(&agent)
            .map_err(|error| ConfigError::Parse(error.to_string()))?;
        atomic_write_json(&dir.join(format!("{key}.json")), &text)?;
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
        if path.is_file() {
            let backup = path.with_extension("json.bak");
            let _ = std::fs::copy(&path, &backup);
        }
        match std::fs::remove_file(&path) {
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
        atomic_write_json(&dir.join("providers.json"), &text)?;
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

fn clean_secret(secret: &str) -> Result<String, ConfigError> {
    let secret = secret
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim()
        .to_string();
    if secret.is_empty() {
        return Err(ConfigError::InvalidProviderKey(
            "the key is empty".to_string(),
        ));
    }
    if secret.len() > 4_096 {
        return Err(ConfigError::InvalidProviderKey(
            "the key is too long".to_string(),
        ));
    }
    if secret.chars().any(|character| character.is_whitespace()) {
        return Err(ConfigError::InvalidProviderKey(
            "the key contains whitespace".to_string(),
        ));
    }
    Ok(secret)
}

/// Key files are private to the logged-in OS user. On Windows the profile ACL
/// is the boundary; on Unix tighten the file itself.
fn restrict_private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// Atomic JSON write: temp file in the same directory + rename, keeping a
/// `.bak` of the previous version. A crash mid-write can never leave a
/// half-written config.
fn atomic_write_json(path: &Path, text: &str) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| ConfigError::Io(error.to_string()))?;
    }
    if path.is_file() {
        let backup = path.with_extension("json.bak");
        // Best-effort: a missing backup must not block the save.
        let _ = std::fs::copy(path, &backup);
    }
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text).map_err(|error| ConfigError::Io(error.to_string()))?;
    std::fs::rename(&temp, path).map_err(|error| ConfigError::Io(error.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod atomic_tests {
    use super::*;

    #[test]
    fn atomic_write_keeps_backup_of_previous() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.json");
        atomic_write_json(&path, "{\"a\":1}").unwrap();
        atomic_write_json(&path, "{\"a\":2}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"a\":2}");
        assert_eq!(
            std::fs::read_to_string(path.with_extension("json.bak")).unwrap(),
            "{\"a\":1}"
        );
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
    if let Some(code_mode) = agent
        .code_mode
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        validate_code_mode(code_mode)?;
    }
    if let Some(access) = agent
        .access
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        match access.to_ascii_lowercase().as_str() {
            "read" | "readonly" | "read-only" | "write" => {}
            _ => {
                return Err(ConfigError::InvalidAccess(access.to_string()));
            }
        }
    }
    if let Some(prompt) = &agent.prompt {
        if let Some(profile) = prompt
            .profile
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            match profile.to_ascii_lowercase().as_str() {
                "coding" | "general" | "plain" => {}
                _ => {
                    return Err(ConfigError::InvalidPromptProfile(profile.to_string()));
                }
            }
        }
        let persona = prompt
            .persona
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let legacy = agent
            .system_prompt
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if persona.is_some() && legacy.is_some() {
            return Err(ConfigError::Parse(
                "use either prompt.persona or the legacy system_prompt, not both".to_string(),
            ));
        }
    }
    if let Some(aliases) = &agent.aliases {
        let mut seen = std::collections::BTreeSet::new();
        for alias in aliases {
            let alias = alias.trim().to_ascii_lowercase();
            if !valid_agent_key(&alias) {
                return Err(ConfigError::InvalidAgentAlias(alias));
            }
            if !seen.insert(alias.clone()) {
                return Err(ConfigError::InvalidAgentAlias(format!(
                    "duplicate alias '{alias}'"
                )));
            }
        }
    }
    if let Some(model) = agent
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter(|value| !value.eq_ignore_ascii_case("Automatic Fallback"))
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

fn validate_code_mode(value: &str) -> Result<(), ConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "off" | "none" | "disabled" | "hybrid" | "both" | "preferred" | "only" | "strict" => Ok(()),
        _ => Err(ConfigError::InvalidCodeMode {
            value: value.to_string(),
        }),
    }
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
    fn model_freeform_capability_resolves() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join(".deep_agents/config");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("providers.json"),
            r#"{
                "version": 1,
                "active": "custom",
                "providers": {
                    "custom": {
                        "label": "Custom",
                        "base_url": "https://example.test/v1",
                        "key_env": "CUSTOM_KEY",
                        "models": {
                            "m": {
                                "id": "model-m",
                                "freeform": ["text", "lark"]
                            }
                        },
                        "default_model": "m"
                    }
                },
                "matrix": {"custom": {"workspace": {"model": "m"}}}
            }"#,
        )
        .unwrap();
        let store = ConfigStore::open(dir.path());
        let effective = store.effective("developer", "workspace").unwrap();
        assert_eq!(effective.provider, "custom");
        assert_eq!(effective.model, "model-m");
        assert_eq!(effective.freeform, vec!["text", "lark"]);
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
    fn automatic_fallback_inherits_the_tier_default() {
        let providers = ProvidersFile::builtin();
        let (key, id) = providers
            .resolve_model("opencode", "Automatic Fallback")
            .unwrap();
        assert_eq!(key, "flash");
        assert_eq!(id, "deepseek-v4.1-flash");
        assert_eq!(
            providers
                .resolve_model("opencode", "automatic fallback")
                .unwrap(),
            ("flash".to_string(), "deepseek-v4.1-flash".to_string())
        );
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
    fn provider_keys_are_saved_selected_and_removed() {
        let (dir, store) = temp_root();
        let first = store
            .add_provider_key("opencode", "Personal", "oc_test_a1b2")
            .unwrap();
        let first_id = first.active.clone();
        assert!(!first_id.is_empty());
        assert!(
            first
                .keys
                .iter()
                .any(|key| { key.id == first_id && key.active && key.masked == "oc_…a1b2" })
        );
        assert_eq!(store.resolve_key("opencode"), "oc_test_a1b2");
        assert!(store.effective("developer", "workspace").unwrap().has_key);
        let path = dir.path().join(".deep_agents/secrets/provider_keys.json");
        assert!(path.is_file());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("oc_test_a1b2"));
        assert!(
            !serde_json::to_string(&first)
                .unwrap()
                .contains("oc_test_a1b2")
        );

        let second = store
            .add_provider_key("opencode", "Backup", "oc_test_c3d4")
            .unwrap();
        let second_id = second.active.clone();
        assert_eq!(
            second
                .keys
                .iter()
                .filter(|key| key.source == "managed")
                .count(),
            2
        );
        assert_eq!(store.resolve_key("opencode"), "oc_test_c3d4");

        let selected = store.activate_provider_key("opencode", &first_id).unwrap();
        assert_eq!(selected.active, first_id);
        assert_eq!(store.resolve_key("opencode"), "oc_test_a1b2");

        let removed = store.remove_provider_key("opencode", &first_id).unwrap();
        assert_eq!(removed.active, second_id);
        assert_eq!(store.resolve_key("opencode"), "oc_test_c3d4");
        assert!(store.remove_provider_key("opencode", "env").is_err());
        assert!(store.add_provider_key("opencode", "", "bad key").is_err());

        drop(store);
        let reopened = ConfigStore::open(dir.path());
        assert_eq!(reopened.resolve_key("opencode"), "oc_test_c3d4");
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
    fn code_mode_round_trips_and_rejects_unknown_values() {
        let (_dir, store) = temp_root();
        store
            .save_agent(
                "developer",
                AgentFile {
                    code_mode: Some("only".to_string()),
                    ..AgentFile::default()
                },
            )
            .unwrap();
        assert_eq!(
            store
                .effective("developer", "workspace")
                .unwrap()
                .code_mode
                .as_deref(),
            Some("only")
        );
        let invalid = store.save_agent(
            "developer",
            AgentFile {
                code_mode: Some("turbo".to_string()),
                ..AgentFile::default()
            },
        );
        assert!(matches!(invalid, Err(ConfigError::InvalidCodeMode { .. })));
    }

    #[test]
    fn full_codemode_round_trips() {
        let (_dir, store) = temp_root();
        store
            .save_agent(
                "developer",
                AgentFile {
                    full_codemode: Some(false),
                    ..AgentFile::default()
                },
            )
            .unwrap();
        assert_eq!(
            store
                .effective("developer", "workspace")
                .unwrap()
                .full_codemode,
            Some(false)
        );
        assert_eq!(
            store.effective("ui", "workspace").unwrap().full_codemode,
            None
        );
    }

    #[test]
    fn agent_v2_prompt_access_and_delegation_round_trip() {
        let (dir, store) = temp_root();
        store
            .save_agent(
                "developer",
                AgentFile {
                    display: "Dev".to_string(),
                    access: Some("read".to_string()),
                    prompt: Some(AgentPromptFile {
                        profile: Some("coding".to_string()),
                        persona: Some("Own the code.".to_string()),
                    }),
                    delegation: Some(AgentDelegationFile {
                        delegatable: Some(false),
                        can_delegate: Some(false),
                        can_spawn: Some(false),
                    }),
                    ui: Some(AgentUiFile {
                        visible: Some(false),
                        order: Some(3),
                    }),
                    routing: Some(AgentRoutingFile {
                        workspace_kinds: vec!["Coding".to_string()],
                        paths: vec!["D:\\work\\**".to_string()],
                    }),
                    ..AgentFile::default()
                },
            )
            .unwrap();
        let path = dir.path().join(".deep_agents/config/agents/developer.json");
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(raw["version"], 2);
        let effective = store.effective("developer", "workspace").unwrap();
        assert_eq!(effective.access, "read");
        assert_eq!(effective.access_spawned, "read");
        assert_eq!(effective.prompt_profile, "coding");
        assert_eq!(effective.persona.as_deref(), Some("Own the code."));
        assert!(!effective.ui_visible);
        assert_eq!(effective.ui_order, 3);
        assert_eq!(effective.routing_workspace_kinds, vec!["coding"]);
        assert_eq!(effective.routing_paths, vec!["D:/work/**"]);
    }

    #[test]
    fn agent_v1_system_prompt_maps_to_persona() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join(".deep_agents/config/agents");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("developer.json"),
            r#"{"system_prompt":"Legacy persona."}"#,
        )
        .unwrap();
        let store = ConfigStore::open(dir.path());
        let effective = store.effective("developer", "workspace").unwrap();
        assert_eq!(effective.persona.as_deref(), Some("Legacy persona."));
    }

    #[test]
    fn agent_rejects_two_persona_sources_and_invalid_access() {
        let (_dir, store) = temp_root();
        let both = store.save_agent(
            "developer",
            AgentFile {
                system_prompt: Some("legacy".to_string()),
                prompt: Some(AgentPromptFile {
                    persona: Some("new".to_string()),
                    ..AgentPromptFile::default()
                }),
                ..AgentFile::default()
            },
        );
        assert!(both.is_err());
        let access = store.save_agent(
            "developer",
            AgentFile {
                access: Some("worktree".to_string()),
                ..AgentFile::default()
            },
        );
        assert!(matches!(access, Err(ConfigError::InvalidAccess(_))));
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
