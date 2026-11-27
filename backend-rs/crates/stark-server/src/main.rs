use std::collections::{HashMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Multipart, Path, Query, State};
use axum::http::header::ORIGIN;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::middleware;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use chrono::Utc;
use dashmap::DashMap;
use futures_util::{Stream, StreamExt, stream};
use serde::Deserialize;
use serde_json::{Value, json};
use stark_config::{AgentFile, ConfigStore, EffectiveAgent};
use stark_contract::{AppState, ServerEvent};
use stark_core::{
    AgentRegistry, RunContext, agent_access_for, preview_system_prompt, resolve_definition,
    run_agent, validate_definition,
};
use stark_providers::{ImagePart, Provider, ProviderConfig};
use stark_store::{
    AgentRecord, AgentStore, CanvasStore, ChatStore, GroupStore, MemoryStore, NewEntry,
    ScheduleRecord, ScheduleStore, WorkspaceRegistry, parse_schedule_time, schedule_json,
};
use stark_tools::{
    Checkpoint, HouseStore, PlanStore, QuestionBroker, evaluate_acceptance, public_device,
    public_state,
};
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

mod cloud;
mod data_transfer;
mod harness_lock;
mod pairing;
mod screen_routes;
mod voice;
use pairing::PairingStore;

type SseStream = Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>;

/// OpenCode V2's live EventFeed subscriber capacity.
const SSE_SUBSCRIBER_CAPACITY: usize = 4096;
/// Internal run broadcast capacity. The live bridge turns overflow into a
/// closed stream instead of silently dropping frames.
const RUN_EVENT_CAPACITY: usize = 4096;
const SSE_HEARTBEAT: Duration = Duration::from_secs(15);

struct RunHandle {
    run_id: String,
    cancel: CancellationToken,
    tx: broadcast::Sender<ServerEvent>,
    steering: Arc<Mutex<VecDeque<String>>>,
    follow_up: Arc<Mutex<VecDeque<String>>>,
    started_at: String,
    workspace_path: String,
    /// Latest live `AppState` produced by the run. Unlike `last_state`, this is
    /// available while the run is still active, so reconnecting clients can
    /// recover the current UI state instead of waiting for the next state event.
    latest: Arc<Mutex<Option<AppState>>>,
}

struct RunAttachment {
    run_id: String,
    receiver: broadcast::Receiver<ServerEvent>,
}

struct RunClaim {
    run_id: String,
    latest: Arc<Mutex<Option<AppState>>>,
    cancel: CancellationToken,
    tx: broadcast::Sender<ServerEvent>,
    steering: Arc<Mutex<VecDeque<String>>>,
    follow_up: Arc<Mutex<VecDeque<String>>>,
    workspace_path: String,
}

struct RunManager {
    runs: DashMap<String, RunHandle>,
    last_state: DashMap<String, Value>,
    /// FIFO insertion order for `last_state` eviction. DashMap has no
    /// ordering, so the oldest chat is dropped once the cap is reached.
    state_order: Mutex<VecDeque<String>>,
    /// Process-wide admission cap. Codex rejects with `AgentLimitReached`
    /// instead of over-subscribing the machine; 0 disables the cap.
    max_runs: usize,
    /// Serializes the length check and insertion so two chats cannot both
    /// take the last slot.
    admission: Mutex<()>,
    /// Set before the first cancellation during shutdown. Every claim path
    /// checks it so a delayed follow-up retry cannot start new work while the
    /// store is being closed.
    shutting_down: AtomicBool,
}

impl Default for RunManager {
    fn default() -> Self {
        Self {
            runs: DashMap::new(),
            last_state: DashMap::new(),
            state_order: Mutex::new(VecDeque::new()),
            max_runs: std::env::var("STARK_MAX_CONCURRENT_RUNS")
                .ok()
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(8),
            admission: Mutex::new(()),
            shutting_down: AtomicBool::new(false),
        }
    }
}

/// One cached `AppState` JSON per finished chat. 128 entries cover a long
/// session without growing for the life of the server.
const MAX_CACHED_STATES: usize = 128;

/// Why `RunManager::claim` refused. `AlreadyRunning` is per chat; `Limit` is
/// the machine-wide cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaimError {
    AlreadyRunning,
    Limit { max: usize },
    ShuttingDown,
}

impl ClaimError {
    fn message(self) -> String {
        match self {
            Self::AlreadyRunning => "This chat already has a running agent.".to_string(),
            Self::Limit { max } => format!(
                "Server run limit reached ({max} concurrent runs, STARK_MAX_CONCURRENT_RUNS). Wait for a run to finish and retry."
            ),
            Self::ShuttingDown => "The server is shutting down.".to_string(),
        }
    }
}

impl RunManager {
    fn claim(&self, chat_id: &str, claim: RunClaim) -> Result<(), ClaimError> {
        let RunClaim {
            run_id,
            latest,
            cancel,
            tx,
            steering,
            follow_up,
            workspace_path,
        } = claim;
        let _admission = self
            .admission
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if self.shutting_down.load(Ordering::SeqCst) {
            return Err(ClaimError::ShuttingDown);
        }
        if self.runs.contains_key(chat_id) {
            return Err(ClaimError::AlreadyRunning);
        }
        if self.max_runs > 0 && self.runs.len() >= self.max_runs {
            return Err(ClaimError::Limit { max: self.max_runs });
        }
        self.runs.insert(
            chat_id.to_string(),
            RunHandle {
                run_id,
                cancel,
                tx,
                steering,
                follow_up,
                started_at: Utc::now().to_rfc3339(),
                workspace_path,
                latest,
            },
        );
        Ok(())
    }

    fn finish(&self, chat_id: &str, state: Option<AppState>) {
        self.runs.remove(chat_id);
        if let Some(state) = state
            && let Ok(value) = serde_json::to_value(&state)
        {
            self.last_state.insert(chat_id.to_string(), value);
            if let Ok(mut order) = self.state_order.lock() {
                order.retain(|id| id != chat_id);
                order.push_back(chat_id.to_string());
                while order.len() > MAX_CACHED_STATES {
                    if let Some(oldest) = order.pop_front() {
                        self.last_state.remove(&oldest);
                    }
                }
            }
        }
    }

    fn forget(&self, chat_id: &str) {
        self.last_state.remove(chat_id);
        if let Ok(mut order) = self.state_order.lock() {
            order.retain(|id| id != chat_id);
        }
    }

    fn is_running(&self, chat_id: &str) -> bool {
        self.runs.contains_key(chat_id)
    }

    fn running_count(&self) -> usize {
        self.runs.len()
    }

    fn list_active(&self) -> Vec<Value> {
        let mut out: Vec<Value> = self
            .runs
            .iter()
            .map(|entry| {
                json!({
                    "running": true,
                    "chat_id": entry.key(),
                    "run_id": entry.run_id,
                    "workspace_path": entry.workspace_path,
                    "started_at": entry.started_at,
                })
            })
            .collect();
        out.sort_by(|left, right| {
            left.get("started_at")
                .and_then(Value::as_str)
                .unwrap_or("")
                .cmp(
                    right
                        .get("started_at")
                        .and_then(Value::as_str)
                        .unwrap_or(""),
                )
        });
        out
    }

    fn steer(&self, chat_id: &str, message: &str) -> bool {
        let Some(handle) = self.runs.get(chat_id) else {
            return false;
        };
        handle
            .steering
            .lock()
            .unwrap()
            .push_back(message.to_string());
        true
    }

    fn follow_up(&self, chat_id: &str, message: &str) -> bool {
        let Some(handle) = self.runs.get(chat_id) else {
            return false;
        };
        handle
            .follow_up
            .lock()
            .unwrap()
            .push_back(message.to_string());
        true
    }

    fn inbox_counts(&self, chat_id: &str) -> Value {
        let Some(handle) = self.runs.get(chat_id) else {
            return json!({"steering": 0, "follow_up": 0});
        };
        json!({
            "steering": handle.steering.lock().unwrap().len(),
            "follow_up": handle.follow_up.lock().unwrap().len(),
        })
    }

    fn cancel(&self, chat_id: &str) -> bool {
        let Some(handle) = self.runs.get(chat_id) else {
            return false;
        };
        handle.cancel.cancel();
        true
    }

    /// Stops new admissions and cancels every claimed run. Returns the number
    /// signalled so shutdown can log whether it actually interrupted work.
    fn cancel_all(&self) -> usize {
        self.shutting_down.store(true, Ordering::SeqCst);
        let mut cancelled = 0;
        for handle in self.runs.iter() {
            handle.cancel.cancel();
            cancelled += 1;
        }
        cancelled
    }

    fn subscribe(&self, chat_id: &str) -> Option<broadcast::Receiver<ServerEvent>> {
        self.runs.get(chat_id).map(|handle| handle.tx.subscribe())
    }

    /// Subscribe and capture the run metadata atomically. The receiver is
    /// created before the caller reads the rollout watermark, so durable and
    /// live observation cannot miss an append that lands during setup.
    fn attach(&self, chat_id: &str) -> Option<RunAttachment> {
        self.runs.get(chat_id).map(|handle| RunAttachment {
            run_id: handle.run_id.clone(),
            receiver: handle.tx.subscribe(),
        })
    }

    fn run_id(&self, chat_id: &str) -> Option<String> {
        self.runs.get(chat_id).map(|handle| handle.run_id.clone())
    }

    fn state(&self, chat_id: Option<&str>) -> Option<Value> {
        if let Some(chat_id) = chat_id {
            if let Some(handle) = self.runs.get(chat_id)
                && let Some(snapshot) = handle.latest.lock().ok().and_then(|guard| guard.clone())
                && let Ok(value) = serde_json::to_value(snapshot)
            {
                return Some(value);
            }
            return self.last_state.get(chat_id).map(|value| value.clone());
        }
        self.last_state
            .iter()
            .next()
            .map(|entry| entry.value().clone())
    }

    fn active(&self, chat_id: Option<&str>) -> Value {
        if let Some(chat_id) = chat_id {
            if let Some(handle) = self.runs.get(chat_id) {
                return json!({
                    "running": true,
                    "workspace_path": handle.workspace_path,
                    "chat_id": chat_id,
                    "run_id": handle.run_id,
                    "started_at": handle.started_at,
                });
            }
        } else if let Some(entry) = self.runs.iter().next() {
            return json!({
                "running": true,
                "workspace_path": entry.workspace_path,
                "chat_id": entry.key(),
                "started_at": entry.started_at,
            });
        }

        json!({
            "running": false,
            "workspace_path": "",
            "chat_id": chat_id.unwrap_or(""),
            "started_at": "",
        })
    }
}

#[derive(Clone)]
struct ServerState {
    store: ChatStore,
    provider: Provider,
    /// Declarative provider + agent config (hot-reloaded from disk).
    config: Arc<ConfigStore>,
    /// One client per configured provider, so agents can run on different
    /// gateways in the same process.
    providers: Arc<HashMap<String, Provider>>,
    /// Last `/models` auth/connectivity error per provider (empty = healthy).
    provider_health: Arc<Mutex<HashMap<String, String>>>,
    questions: Arc<QuestionBroker>,
    runs: Arc<RunManager>,
    registry: WorkspaceRegistry,
    pairing: Arc<PairingStore>,
    ui_state_path: PathBuf,
    harness_root: PathBuf,
    voice: Option<Arc<voice::VoiceRuntime>>,
}

#[derive(Clone)]
struct RunTarget {
    chat_id: String,
    workspace_id: String,
    workspace: PathBuf,
    scope: String,
    role: Option<String>,
    model: String,
    temperature: f64,
    depth: u32,
    agent_id: Option<String>,
    /// Resolved provider/model/effort/tools for this run.
    effective: Arc<EffectiveAgent>,
}

impl ServerState {
    /// Pick a different configured provider with a usable key and model. The
    /// explicit `STARK_FALLBACK_PROVIDER` wins; otherwise the first healthy
    /// other provider in `providers.json` order is used.
    fn fallback_config(&self, effective: &EffectiveAgent) -> Option<ProviderConfig> {
        let preferred = std::env::var("STARK_FALLBACK_PROVIDER")
            .ok()
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty());
        let candidates = self.config.provider_summaries();
        let chosen = candidates
            .iter()
            .find(|provider| {
                preferred
                    .as_deref()
                    .is_some_and(|wanted| provider.id.eq_ignore_ascii_case(wanted))
                    && !provider.id.eq_ignore_ascii_case(&effective.provider)
                    && provider.has_key
                    && !provider.models.is_empty()
            })
            .or_else(|| {
                candidates.iter().find(|provider| {
                    !provider.id.eq_ignore_ascii_case(&effective.provider)
                        && provider.has_key
                        && !provider.models.is_empty()
                })
            })?;
        let requested = if chosen.default_model.trim().is_empty() {
            chosen.models.first()?.id.clone()
        } else {
            chosen.default_model.clone()
        };
        let model = self
            .config
            .resolve_model(&chosen.id, &requested)
            .map(|(_, model)| model)
            .unwrap_or(requested);
        Some(ProviderConfig {
            mode: chosen.id.clone(),
            base_url: chosen.base_url.clone(),
            api_key: self.config.resolve_key(&chosen.id),
            model,
        })
    }

    fn attach_fallback(&self, provider: Provider, effective: &EffectiveAgent) -> Provider {
        match self.fallback_config(effective) {
            Some(config) => provider.with_fallback(config),
            None => provider,
        }
    }

    /// Client for the provider an agent resolved to. Falls back to the active
    /// provider only if the registry entry is missing.
    fn provider_for(&self, effective: &EffectiveAgent) -> Provider {
        if let Some(provider) = self.providers.get(&effective.provider) {
            let resolved = self.config.resolve_key(&effective.provider);
            let current = provider.config();
            // An empty resolution means the env var is unset; keep the key the
            // provider was built with instead of blanking a working client.
            let key = if resolved.is_empty() {
                current.api_key.clone()
            } else {
                resolved
            };
            let stale = !current.base_url.eq_ignore_ascii_case(&effective.base_url)
                || current.api_key != key
                || !current.mode.eq_ignore_ascii_case(&effective.provider);
            if !stale && current.model.eq_ignore_ascii_case(&effective.model) {
                return self
                    .attach_fallback(provider.clone(), effective)
                    .with_freeform(effective.freeform.clone());
            }
            // Hot reload: rebuild the client with the edited base/key/model.
            let mut config = current.clone();
            config.mode = effective.provider.clone();
            config.base_url = effective.base_url.clone();
            config.api_key = key;
            config.model = effective.model.clone();
            let rebuilt = Provider::new(config).unwrap_or_else(|error| {
                tracing::warn!(
                    "provider '{}' could not apply the edited config: {error}",
                    effective.provider
                );
                provider.clone()
            });
            return self
                .attach_fallback(rebuilt, effective)
                .with_freeform(effective.freeform.clone());
        }
        // A provider added to providers.json after startup is not in the
        // prebuilt map yet. Build it on demand so hot-reload covers new
        // gateways, not just edits to existing ones.
        let built = Provider::new(ProviderConfig {
            mode: effective.provider.clone(),
            base_url: effective.base_url.clone(),
            api_key: self.config.resolve_key(&effective.provider),
            model: effective.model.clone(),
        })
        .unwrap_or_else(|error| {
            tracing::warn!(
                "provider '{}' could not be built on demand: {error}",
                effective.provider
            );
            self.provider.clone()
        });
        self.attach_fallback(built, effective)
            .with_freeform(effective.freeform.clone())
    }
}

/// Resolve an agent's configuration, falling back to the built-in defaults when
/// a user-edited agent file is invalid. A broken file must not brick a chat.
fn agent_registry(state: &ServerState) -> AgentRegistry {
    AgentRegistry::load(&state.config, &state.harness_root)
}

fn effective_for(state: &ServerState, scope: &str, tier: &str) -> Arc<EffectiveAgent> {
    let registry = agent_registry(state);
    if let Some(agent) = registry.get(scope) {
        return Arc::new(agent.clone());
    }
    tracing::warn!("agent '{scope}' is not in the registry; using built-in defaults");
    Arc::new(
        state
            .config
            .effective(scope, tier)
            .or_else(|_| state.config.effective("__default__", tier))
            .unwrap_or_else(|_| stark_config::builtin_effective(tier)),
    )
}

fn legacy_env_names() -> Vec<&'static str> {
    [
        "STARK_PROVIDER",
        "STARK_PROVIDER_BASE",
        "STARK_MODEL",
        "STARK_API_KEY",
    ]
    .into_iter()
    .filter(|name| std::env::var(name).is_ok())
    .collect()
}

fn build_provider_map(config: &ConfigStore) -> HashMap<String, Provider> {
    let mut providers = HashMap::new();
    for summary in config.provider_summaries() {
        let id = summary.id.clone();
        let requested = if summary.default_model.trim().is_empty() {
            summary
                .models
                .first()
                .map(|model| model.id.clone())
                .unwrap_or_default()
        } else {
            summary.default_model.clone()
        };
        let model = config
            .resolve_model(&id, &requested)
            .map(|(_, model)| model)
            .unwrap_or_else(|_| requested.clone());
        let provider_config = ProviderConfig {
            mode: id.clone(),
            base_url: summary.base_url.clone(),
            api_key: config.resolve_key(&id),
            model,
        };
        match Provider::new(provider_config) {
            Ok(provider) => {
                providers.insert(id, provider);
            }
            Err(error) => tracing::warn!("provider '{id}' could not be initialized: {error}"),
        }
    }
    providers
}

#[derive(Debug, Deserialize)]
struct RunRequest {
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    model: String,
    #[serde(default = "default_temperature")]
    temperature: f64,
    #[serde(default)]
    workspace_path: String,
    #[serde(default)]
    workspace_id: String,
    #[serde(default)]
    chat_id: String,
    #[serde(default)]
    attachments: Vec<Value>,
    #[serde(default)]
    workspace_kind: String,
    /// Explicit agent key (built-in or custom). When empty the registry routes
    /// by workspace kind/path, preserving old clients.
    #[serde(default, alias = "agent_key")]
    agent: String,
}

fn default_temperature() -> f64 {
    0.7
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = tracing_log::LogTracer::init();
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "stark_server=info,tower_http=info".into()),
        )
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);

    // Fail-closed: the launcher always sets STARK_HARNESS_ROOT (persisted
    // `D:\StarkData` by default). Falling back to the launch cwd would silently
    // mix workspaces with harness data and read the wrong `.env`.
    let harness_root = match std::env::var("STARK_HARNESS_ROOT") {
        Ok(raw) if !raw.trim().is_empty() => PathBuf::from(raw.trim()),
        _ => {
            return Err(anyhow::anyhow!(
                "STARK_HARNESS_ROOT is not set. Launch via the Stark desktop app (it sets the persisted harness root) or set it explicitly."
            ));
        }
    };
    std::fs::create_dir_all(&harness_root).map_err(|error| {
        anyhow::anyhow!(
            "harness root {} unavailable: {error}. Pick a writable folder.",
            harness_root.display()
        )
    })?;
    // One server owns a harness root. The advisory lock is released by the OS
    // on exit or crash, so it never becomes a permanent stale lock.
    let harness_guard = harness_lock::HarnessLock::acquire(&harness_root)
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    tracing::info!("harness lock: {}", harness_guard.path().display());
    // Keys come ONLY from <harness_root>/.env. Never load the process cwd
    // .env: that file belongs to a workspace the agent can edit, and must
    // never supply provider keys.
    let app_env = harness_root.join(".env");
    if app_env.is_file() {
        let _ = dotenvy::from_path(&app_env);
    }
    // Load the Starlark execution policy. A missing or invalid file keeps the
    // built-in destructive-command rules; it never disables the policy.
    let policy = stark_policy::configure(&harness_root);
    match (&policy.path, &policy.error) {
        (Some(path), Some(error)) => {
            tracing::warn!("exec policy {} failed to load: {error}", path.display());
        }
        (Some(path), None) if policy.loaded => tracing::info!(
            "exec policy: {} ({} command rules, {} network rules, {} host executables)",
            path.display(),
            policy.rules,
            policy.network_rules,
            policy.host_executables
        ),
        _ => tracing::info!(
            "exec policy: built-in defaults ({} command rules)",
            policy.rules
        ),
    }
    // Children always get proxy variables; when network is off the proxy
    // denies every destination. Set STARK_NETWORK_PROXY=off only for
    // debugging on a machine with external OS isolation.
    let proxy_disabled = std::env::var("STARK_NETWORK_PROXY")
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "off" | "0" | "false" | "none"
            )
        });
    if proxy_disabled {
        tracing::warn!(
            "network proxy disabled by STARK_NETWORK_PROXY; child egress policy is advisory only"
        );
    } else {
        match stark_sandbox::netproxy::start_background("127.0.0.1:0").await {
            Ok(url) => tracing::info!(
                "network proxy: {url} (network {}, local destinations {})",
                if stark_sandbox::network_enabled() {
                    "enabled"
                } else {
                    "disabled"
                },
                if std::env::var("STARK_NETWORK_ALLOW_LOCAL")
                    .ok()
                    .is_some_and(|value| {
                        matches!(
                            value.trim().to_ascii_lowercase().as_str(),
                            "on" | "1" | "true" | "yes"
                        )
                    })
                {
                    "allowed"
                } else {
                    "blocked"
                }
            ),
            Err(error) => tracing::warn!(
                "network proxy failed to start: {error}; child egress policy is advisory only"
            ),
        }
    }
    // Provider/base/model come from `.deep_agents/config/providers.json`.
    // Environment variables only supply keys (referenced by `key_env`).
    let config = Arc::new(ConfigStore::open(&harness_root));
    for issue in config.issues() {
        tracing::warn!("config: {issue}");
    }
    let providers = Arc::new(build_provider_map(&config));
    let active_id = config.active_provider();
    let provider = providers
        .get(&active_id)
        .cloned()
        .or_else(|| providers.values().next().cloned())
        .unwrap_or(Provider::new(ProviderConfig::from_env())?);
    if !provider.config().has_key() {
        tracing::warn!(
            "no API key found for provider '{active_id}' (checked {:?}); set it in the environment",
            provider.config().mode
        );
    }
    tracing::info!(
        "provider: {active_id} · {} · model={}",
        provider.config().base_url,
        provider.config().model
    );
    tracing::info!("{}", stark_tools::sandbox_status(&harness_root));
    if stark_sandbox::strict() && !stark_sandbox::enforced() {
        tracing::error!(
            "STARK_SANDBOX_STRICT is set but no filesystem sandbox backend is enforcing; shell commands will be refused"
        );
    }
    // One cancellation token stops every long-lived background loop. Tasks
    // that only sleep are tracked so shutdown can abort them after the grace
    // period instead of leaving a writer alive behind a closed pool.
    let shutdown = CancellationToken::new();
    let mut background: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    // Learn the real context window per model from every configured provider
    // instead of guessing. Best-effort.
    let provider_health = Arc::new(Mutex::new(HashMap::new()));
    {
        let config = config.clone();
        let health = provider_health.clone();
        let shutdown = shutdown.clone();
        background.push(tokio::spawn(async move {
            loop {
                let mut learned = 0;
                // Rebuild from the live config so a key selected in the
                // provider panel (or a key file edit) is the one probed.
                for (id, provider) in build_provider_map(&config) {
                    // Unconfigured providers have no key; probing them only
                    // produces a 401 at every boot and tells us nothing.
                    if !provider.config().has_key() {
                        continue;
                    }
                    match provider.refresh_model_contexts().await {
                        Ok(count) => {
                            learned += count;
                            if let Ok(mut health) = health.lock() {
                                health.remove(&id);
                            }
                        }
                        Err(error) => {
                            tracing::warn!("provider '{id}' model check failed: {error}");
                            if let Ok(mut health) = health.lock() {
                                health.insert(id.clone(), error.to_string());
                            }
                        }
                    }
                }
                if learned > 0 {
                    tracing::info!("learned context windows for {learned} model(s)");
                }
                if !sleep_or_shutdown(std::time::Duration::from_secs(6 * 3600), &shutdown).await {
                    break;
                }
            }
        }));
    }
    // A staged import replaces files before the store opens; afterwards the
    // chat rows still carry the old absolute workspace prefix, so rebase them.
    let pending_import = data_transfer::apply_pending_import(&harness_root);
    let store = ChatStore::open(&harness_root).await?;
    if let Some((from_root, to_root)) = pending_import {
        match store.rebase_workspace_paths(&from_root, &to_root).await {
            Ok(changed) => tracing::info!(
                "import applied: rebased {changed} workspace path(s) from {} to {}",
                from_root.display(),
                to_root.display()
            ),
            Err(error) => tracing::warn!("import applied but path rebase failed: {error}"),
        }
    }
    let voice = stark_voice::resolve_models_dir(&harness_root).map(|dir| {
        tracing::info!("voice models: {}", dir.display());
        Arc::new(voice::VoiceRuntime::new(dir))
    });
    if voice.is_none() {
        tracing::warn!("no voice models directory found; voice/TTS endpoints will return 503");
    }
    let mcp_manager = stark_tools::init_mcp(&harness_root);
    // Bounded, silent cleanup of harness-generated files (spilled tool output,
    // rewind snapshots in every workspace, orphan import staging). Runs off
    // the boot path and repeats hourly. Nothing model-visible.
    {
        let sweeper_root = harness_root.clone();
        let shutdown = shutdown.clone();
        background.push(tokio::spawn(async move {
            let policy = stark_tools::RetentionPolicy::from_env();
            let mut low_disk = false;
            loop {
                let root = sweeper_root.clone();
                let (report, free) = tokio::task::spawn_blocking(move || {
                    let registry = WorkspaceRegistry::new(&root);
                    let mut workspaces: Vec<PathBuf> = registry
                        .list()
                        .into_iter()
                        .map(|record| PathBuf::from(record.path))
                        .filter(|path| path.is_dir())
                        .collect();
                    // Worktree agents checkpoint into the worktree, which is
                    // not a registered workspace. Sweep live worktrees too;
                    // settled ones are removed by worktree GC.
                    let worktrees = root.join(".deep_agents").join("worktrees");
                    if let Ok(entries) = std::fs::read_dir(worktrees) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.is_dir() {
                                workspaces.push(path);
                            }
                        }
                    }
                    let report = stark_tools::sweep_harness(&root, &workspaces, &policy);
                    (report, stark_tools::free_bytes(&root))
                })
                .await
                .unwrap_or_else(|error| {
                    tracing::warn!("retention sweep failed: {error}");
                    (stark_tools::RetentionReport::default(), None)
                });
                if !report.is_empty() {
                    tracing::info!(
                        "retention: removed {} scratch files ({:.1} MB), {} snapshot runs ({:.1} MB), {} imports",
                        report.scratch_removed,
                        report.scratch_bytes as f64 / (1024.0 * 1024.0),
                        report.snapshot_runs_removed,
                        report.snapshot_bytes as f64 / (1024.0 * 1024.0),
                        report.imports_removed
                    );
                }
                match free {
                    Some(bytes) if bytes < 2 * 1024 * 1024 * 1024 => {
                        if !low_disk {
                            tracing::warn!(
                                "harness drive has only {:.1} GB free; builds and runs may fail",
                                bytes as f64 / (1024.0 * 1024.0 * 1024.0)
                            );
                            low_disk = true;
                        }
                    }
                    Some(_) if low_disk => {
                        tracing::info!("harness drive free space recovered");
                        low_disk = false;
                    }
                    _ => {}
                }
                if !sleep_or_shutdown(std::time::Duration::from_secs(3600), &shutdown).await {
                    break;
                }
            }
        }));
    }
    let state = ServerState {
        store,
        provider,
        config: config.clone(),
        providers: providers.clone(),
        provider_health: provider_health.clone(),
        questions: Arc::new(QuestionBroker::new()),
        runs: Arc::new(RunManager::default()),
        registry: WorkspaceRegistry::new(&harness_root),
        pairing: Arc::new(PairingStore::new(&harness_root)),
        ui_state_path: harness_root.join(".deep_agents").join("ui_state.json"),
        harness_root,
        voice,
    };
    let codemode_state = state.clone();
    background.push(tokio::spawn(async move {
        deliver_code_mode_restart_notices(codemode_state).await;
    }));
    let scheduler_state = state.clone();
    let group_runner_state = state.clone();
    let canvas_state = state.clone();
    let agent_state = state.clone();
    background.push(tokio::spawn(async move {
        mcp_manager.start_all().await;
    }));
    // Daily chats.db backup (plus one at startup). Best-effort.
    {
        let backup_store = state.store.clone();
        let shutdown = shutdown.clone();
        background.push(tokio::spawn(async move {
            if let Err(error) = backup_store.backup_database().await {
                tracing::warn!("startup chats.db backup failed: {error}");
            }
            loop {
                if !sleep_or_shutdown(std::time::Duration::from_secs(24 * 3600), &shutdown).await {
                    break;
                }
                if let Err(error) = backup_store.backup_database().await {
                    tracing::warn!("daily chats.db backup failed: {error}");
                }
            }
        }));
    }
    // Daily portable snapshot (encrypted when STARK_SNAPSHOT_PASSPHRASE is
    // set). This is the backup that can be restored on another machine.
    {
        let snapshot_root = state.harness_root.clone();
        let snapshot_store = state.store.clone();
        let shutdown = shutdown.clone();
        background.push(tokio::spawn(async move {
            loop {
                match data_transfer::snapshot_once(&snapshot_root, &snapshot_store).await {
                    Ok(path) => tracing::info!("snapshot written: {}", path.display()),
                    Err(error) => tracing::warn!("snapshot failed: {error}"),
                }
                let hours = std::env::var("STARK_SNAPSHOT_HOURS")
                    .ok()
                    .and_then(|value| value.trim().parse::<u64>().ok())
                    .filter(|value| *value > 0)
                    .unwrap_or(24);
                if !sleep_or_shutdown(std::time::Duration::from_secs(hours * 3600), &shutdown).await
                {
                    break;
                }
            }
        }));
    }

    let app = Router::new()
        .route("/api/chats/{chat_id}/run", post(run_command_handler))
        .route("/api/chats/{chat_id}/events", get(chat_events_handler))
        .route("/api/chats/{chat_id}/log", get(chat_log_handler))
        .route("/api/abort", post(abort_handler))
        .route("/api/health", get(health_handler))
        .route("/api/state", get(state_handler))
        .route("/api/history", get(history_handler))
        .route("/api/runs/active", get(active_handler))
        .route("/api/ask/pending", get(pending_handler))
        .route("/api/ask/answer", post(answer_handler))
        .route("/api/models", get(models_handler))
        .route(
            "/api/provider",
            get(provider_handler).post(set_provider_handler),
        )
        .route(
            "/api/provider/keys",
            get(provider_keys_handler).post(add_provider_key_handler),
        )
        .route(
            "/api/provider/keys/active",
            post(activate_provider_key_handler),
        )
        .route(
            "/api/provider/keys/{key_id}",
            delete(remove_provider_key_handler),
        )
        .route("/api/provider/usage", get(provider_usage_handler))
        .route("/api/agent-catalog", get(agent_catalog_handler))
        .route("/api/config/agents", get(agents_config_handler))
        .route(
            "/api/config/agents/{key}",
            get(agent_config_handler)
                .put(save_agent_handler)
                .delete(delete_agent_handler),
        )
        .route(
            "/api/config/agents/{key}/preview",
            post(preview_agent_handler),
        )
        .route(
            "/api/config/agents/{key}/duplicate",
            post(duplicate_agent_handler),
        )
        .route(
            "/api/workspaces",
            get(workspaces_handler).post(add_workspace_handler),
        )
        .route(
            "/api/workspaces/{workspace_id}/clone",
            post(clone_workspace_handler),
        )
        .route(
            "/api/workspaces/{workspace_id}",
            get(workspace_handler).delete(remove_workspace_handler),
        )
        .route(
            "/api/workspaces/{workspace_id}/files",
            get(workspace_files_handler),
        )
        .route(
            "/api/workspaces/{workspace_id}/file",
            get(workspace_file_handler),
        )
        .route(
            "/api/workspaces/{workspace_id}/chats",
            get(chats_handler).post(create_chat_handler),
        )
        .route(
            "/api/workspaces/{workspace_id}/chats/{chat_id}",
            get(chat_handler).delete(delete_chat_handler),
        )
        .route(
            "/api/workspaces/{workspace_id}/chats/{chat_id}/title",
            put(rename_chat_handler),
        )
        .route(
            "/api/workspaces/{workspace_id}/chats/{chat_id}/messages",
            post(add_message_handler),
        )
        .route(
            "/api/ui/state",
            get(ui_state_handler).put(save_ui_state_handler),
        )
        .route("/api/session/reset", post(session_reset_handler))
        .route("/api/harness", get(harness_handler))
        .route("/api/data/export", get(export_data_handler))
        .route(
            "/api/data/import",
            post(import_data_handler).layer(DefaultBodyLimit::max(512 * 1024 * 1024)),
        )
        .route("/api/migration/status", get(migration_status_handler))
        .route("/api/migration/import", post(migration_import_handler))
        .route(
            "/api/workspaces/{workspace_id}/chats/{chat_id}/subagents",
            get(subagents_handler),
        )
        .route(
            "/api/workspaces/{workspace_id}/subagents/{child_id}",
            get(subagent_handler),
        )
        .route(
            "/api/workspaces/{workspace_id}/chats/{chat_id}/attachments",
            post(upload_attachments_handler).layer(DefaultBodyLimit::max(64 * 1024 * 1024)),
        )
        .route(
            "/api/workspaces/{workspace_id}/chats/{chat_id}/attachments/{attachment_id}",
            get(attachment_handler),
        )
        .route(
            "/api/workspaces/{workspace_id}/chats/{chat_id}/rewind",
            post(rewind_chat_handler),
        )
        .route(
            "/api/workspaces/{workspace_id}/chats/{chat_id}/fork",
            post(fork_chat_handler),
        )
        .route("/api/chat/{chat_id}/mode", post(chat_mode_handler))
        .route("/api/chat/{chat_id}/state", get(chat_state_handler))
        .route("/api/steer", post(steer_handler))
        .route("/api/followup", post(follow_up_handler))
        .route("/api/inbox", get(inbox_handler))
        .route("/api/chats/{chat_id}/snapshots", get(snapshots_handler))
        .route(
            "/api/chats/{chat_id}/snapshots/{snap_id}/restore",
            post(restore_snapshot_handler),
        )
        .route(
            "/api/chats/{chat_id}/runs/{run_id}/restore",
            post(restore_run_handler),
        )
        .route("/api/runs/{run_id}/undo", post(undo_run_handler))
        .route("/api/pairing", get(pairing_handler))
        .route("/api/pairing/rotate", post(pairing_rotate_handler))
        .route(
            "/api/pairing/devices",
            get(pairing_devices_handler).post(pairing_issue_handler),
        )
        .route(
            "/api/pairing/devices/{device_id}",
            delete(pairing_revoke_handler),
        )
        .route(
            "/api/groups",
            get(groups_handler).post(create_group_handler),
        )
        .route("/api/groups/{project_id}", get(group_handler))
        .route("/api/agents", get(agents_handler))
        .route("/api/agents/{agent_id}", get(agent_handler))
        .route("/api/agents/{agent_id}/cancel", post(agent_cancel_handler))
        .route(
            "/api/agents/{agent_id}/message",
            post(agent_message_handler),
        )
        .route("/api/agents/{agent_id}/diff", get(agent_diff_handler))
        .route("/api/agents/{agent_id}/merge", post(agent_merge_handler))
        .route(
            "/api/agents/{agent_id}/discard",
            post(agent_discard_handler),
        )
        .route("/api/mcp/servers", get(mcp_servers_handler))
        .route(
            "/api/mcp/servers/{server_name}",
            post(mcp_upsert_handler).delete(mcp_delete_handler),
        )
        .route(
            "/api/mcp/servers/{server_name}/restart",
            post(mcp_restart_handler),
        )
        .route(
            "/api/groups/{project_id}/update",
            post(group_update_handler),
        )
        .route(
            "/api/groups/{project_id}/items/{item_id}/approval",
            post(group_approval_handler),
        )
        .route(
            "/api/schedules",
            get(schedules_handler).post(create_schedule_handler),
        )
        .route(
            "/api/schedules/{schedule_id}",
            delete(delete_schedule_handler),
        )
        .route("/api/turn_detection", get(voice::turn_detection_handler))
        .route(
            "/api/vision/frame",
            post(screen_routes::vision_frame_handler)
                .layer(DefaultBodyLimit::max(16 * 1024 * 1024)),
        )
        .route("/api/voice/ws", get(screen_routes::voice_ws_handler))
        .route("/api/tts", post(voice::tts_handler))
        .route("/api/tts/stream", post(voice::tts_stream_handler))
        .route("/api/tts/cancel", post(voice::tts_cancel_handler))
        .route("/api/voice/start", post(voice::voice_start_handler))
        .route("/api/voice/status", get(voice::voice_status_handler))
        .route("/api/voice/stop", post(voice::voice_stop_handler))
        .route("/api/voice/mute", post(voice::voice_mute_handler))
        .route("/api/voice/interrupt", post(voice::voice_interrupt_handler))
        .route("/api/voice/peek", get(voice::voice_peek_handler))
        .route("/api/voice/send", post(voice::voice_send_handler))
        .route("/api/voice/warm_tts", post(voice::tts_warm_handler))
        .route("/api/voice/release_tts", post(voice::tts_release_handler))
        .route("/api/voice/release_asr", post(voice::release_asr_handler))
        .route("/api/voice/tts_status", get(voice::tts_status_handler))
        .route(
            "/api/transcribe",
            post(voice::transcribe_handler).layer(DefaultBodyLimit::max(32 * 1024 * 1024)),
        )
        .route("/api/canvas/{chat_id}/status", get(canvas_status_handler))
        .route("/api/house/state", get(house_state_handler))
        .route(
            "/api/house/devices/{device_id}",
            get(house_device_handler).patch(house_set_device_handler),
        )
        .route("/api/house/rooms/{room_id}", post(house_set_room_handler))
        .route(
            "/api/house/scenes/{scene_id}/run",
            post(house_run_scene_handler),
        )
        .route("/api/house/history", get(house_history_handler))
        .route("/api/house/reset", post(house_reset_handler))
        .route(
            "/api/notes/{scope_type}/{scope_id}",
            get(list_notes_handler),
        )
        .route("/api/notes", post(create_note_handler))
        .route(
            "/api/notes/{note_id}",
            patch(update_note_handler).delete(delete_note_handler),
        )
        .route("/api/memory", get(memory_list_handler))
        .route(
            "/api/memory/{workspace_id}",
            get(memory_status_handler)
                .put(memory_save_handler)
                .delete(memory_clear_handler),
        )
        .route(
            "/api/memory/{workspace_id}/consolidate",
            post(memory_consolidate_handler),
        )
        .route(
            "/api/memory/{workspace_id}/notes",
            post(memory_note_handler),
        )
        .route(
            "/api/memory/{workspace_id}/topics/{topic}",
            get(memory_topic_handler),
        )
        .route(
            "/api/memory/{workspace_id}/entries/{entry_id}",
            delete(memory_forget_handler),
        )
        .route(
            "/api/memory/{workspace_id}/entries/{entry_id}/pin",
            post(memory_pin_handler),
        )
        .route("/api/cloud/status", get(cloud_status_handler))
        .route("/api/cloud/handoffs", get(cloud_handoffs_handler))
        .route(
            "/api/cloud/handoffs/{handoff_id}",
            get(cloud_handoff_handler),
        )
        .route("/api/cloud/send", post(cloud_send_handler))
        .route(
            "/api/cloud/receive",
            post(cloud_receive_handler).layer(DefaultBodyLimit::max(512 * 1024 * 1024)),
        )
        // `origin_guard` is added last so it runs before auth/CORS. The CORS
        // layer stays permissive for the allowed origins; disallowed browser
        // origins never reach it.
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .layer(middleware::from_fn(origin_guard));

    let app = app.with_state(state.clone());

    let host = std::env::var("STARK_HOST")
        .or_else(|_| std::env::var("STARK_BIND"))
        .unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("STARK_PORT").unwrap_or_else(|_| "8124".to_string());
    let address = format!("{host}:{port}");
    let listener = tokio::net::TcpListener::bind(&address).await?;
    background.push(tokio::spawn(schedule_loop(
        scheduler_state,
        shutdown.clone(),
    )));
    background.push(tokio::spawn(group_loop(
        group_runner_state,
        shutdown.clone(),
    )));
    background.push(tokio::spawn(canvas_loop(canvas_state, shutdown.clone())));
    background.push(tokio::spawn(agent_loop(agent_state, shutdown.clone())));
    tracing::info!("stark-server listening on http://{address}");

    let signal_state = state.clone();
    let signal_shutdown = shutdown.clone();
    let signal_task = tokio::spawn(async move {
        shutdown_signal().await;
        tracing::info!("shutdown signal received");
        signal_shutdown.cancel();
        let runs = signal_state.runs.cancel_all();
        let children = stark_core::cancel_all_child_runs();
        let agents = stark_core::cancel_all_agent_runs();
        tracing::info!(
            "shutdown: cancelled {runs} run(s), {children} child run(s), {agents} agent run(s)"
        );
    });

    let server_shutdown = shutdown.clone();
    let server = async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move { server_shutdown.cancelled().await })
        .await
    };
    tokio::pin!(server);
    let grace = shutdown.clone();
    let deadline = async move {
        grace.cancelled().await;
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
    };
    let server_error = tokio::select! {
        result = &mut server => result.err(),
        _ = deadline => {
            tracing::warn!("graceful shutdown exceeded 15s; closing remaining connections");
            None
        }
    };

    // Stop loops and runs even when the server ended on its own (for example a
    // listener error) instead of a signal. Give loops a short cooperative
    // window first: aborting one halfway through `spawn_agent_run` can strand
    // a claimed run that no task will finish. Anything still running after the
    // window is aborted, then the admission gate and cancellation sweep run.
    shutdown.cancel();
    signal_task.abort();
    let _ = signal_task.await;
    let drain_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    while tokio::time::Instant::now() < drain_deadline
        && background.iter().any(|handle| !handle.is_finished())
    {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    for handle in &background {
        if !handle.is_finished() {
            handle.abort();
        }
    }
    let runs = state.runs.cancel_all();
    let children = stark_core::cancel_all_child_runs();
    let agents = stark_core::cancel_all_agent_runs();
    if runs + children + agents > 0 {
        tracing::info!(
            "shutdown: cancelled {runs} run(s), {children} child run(s), {agents} agent run(s)"
        );
    }
    wait_for_runs(&state, std::time::Duration::from_secs(10)).await;
    for handle in background {
        let _ = handle.await;
    }
    if tokio::time::timeout(std::time::Duration::from_secs(5), state.store.close())
        .await
        .is_err()
    {
        tracing::warn!("store close timed out; process exit will release SQLite handles");
    }
    if let Some(error) = server_error {
        return Err(error.into());
    }
    tracing::info!("stark-server stopped");
    Ok(())
}

/// Resolves on Ctrl-C, or on SIGTERM on Unix. Windows process termination that
/// does not go through the console handler still relies on OS-level cleanup;
/// this covers the normal desktop close and developer Ctrl-C paths.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(signal) => signal,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(windows)]
    {
        // A desktop app or test harness may use Ctrl-Break rather than
        // Ctrl-C. Both take the same graceful path.
        let mut ctrl_break = match tokio::signal::windows::ctrl_break() {
            Ok(signal) => signal,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = ctrl_break.recv() => {}
        }
    }
    #[cfg(all(not(unix), not(windows)))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Sleeps for `duration` unless shutdown starts first. Returns false when the
/// loop should stop.
async fn sleep_or_shutdown(duration: std::time::Duration, shutdown: &CancellationToken) -> bool {
    tokio::select! {
        _ = shutdown.cancelled() => false,
        _ = tokio::time::sleep(duration) => true,
    }
}

/// Waits for top-level and child runs to release their rollout writers. The
/// timeout is a bound, not a guarantee: a provider request that ignores
/// cancellation cannot hold process exit forever.
async fn wait_for_runs(state: &ServerState, timeout: std::time::Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        // A delayed follow-up retry or an already-spawned group task can reach
        // the claim gate just after the first sweep. Re-signal on every tick;
        // admissions are closed, so this drains instead of racing.
        state.runs.cancel_all();
        stark_core::cancel_all_child_runs();
        stark_core::cancel_all_agent_runs();
        let active = state.runs.running_count();
        let children = stark_core::child_run_count();
        if active == 0 && children == 0 {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            tracing::warn!(
                "shutdown: {active} run(s) and {children} child run(s) still active after grace period"
            );
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

async fn canvas_loop(state: ServerState, shutdown: CancellationToken) {
    let store = match CanvasStore::open(&state.harness_root).await {
        Ok(store) => store,
        Err(error) => {
            tracing::warn!("canvas mode disabled: {error}");
            return;
        }
    };
    if let Ok(count) = store.recover_dispatching().await
        && count > 0
    {
        tracing::info!("recovered {count} interrupted canvas wake(s)");
    }
    loop {
        if !sleep_or_shutdown(std::time::Duration::from_secs(1), &shutdown).await {
            break;
        }
        let due = match store.due_wakes(20).await {
            Ok(due) => due,
            Err(error) => {
                tracing::warn!("canvas wake query failed: {error}");
                continue;
            }
        };
        for wake in due {
            let Some(id) = wake["id"].as_str() else {
                continue;
            };
            let Some(chat_id) = wake["chatId"].as_str() else {
                continue;
            };
            let text = wake["text"].as_str().unwrap_or("").to_string();
            if let Err(error) = store.mark_wake(id, "dispatching").await {
                tracing::warn!("canvas wake {id} mark failed: {error}");
                continue;
            }
            let chat = match state.store.get_chat(chat_id).await {
                Ok(Some(chat)) => chat,
                _ => {
                    let _ = store.mark_wake(id, "failed").await;
                    continue;
                }
            };
            if state.runs.is_running(chat_id) {
                // A wake for a busy chat becomes the next turn, not a second
                // concurrent run.
                let queued = state.runs.follow_up(chat_id, &text);
                let _ = store
                    .mark_wake(id, if queued { "done" } else { "failed" })
                    .await;
                continue;
            }
            let target = match canvas_target(&state, &chat) {
                Ok(target) => target,
                Err(error) => {
                    tracing::warn!("canvas wake {id} target invalid: {error}");
                    let _ = store.mark_wake(id, "failed").await;
                    continue;
                }
            };
            spawn_follow_up_run(state.clone(), target, text);
            let _ = store.mark_wake(id, "done").await;
        }
    }
}

fn canvas_target(state: &ServerState, chat: &Value) -> Result<RunTarget, String> {
    let workspace_path = chat["workspacePath"].as_str().unwrap_or("");
    let workspace = if workspace_path.trim().is_empty() {
        state.harness_root.clone()
    } else {
        PathBuf::from(workspace_path)
    };
    let workspace_id = chat["workspaceId"].as_str().unwrap_or("_general");
    let parent = chat["parentChatId"].as_str().unwrap_or("");
    let registry = agent_registry(state);
    let persisted = chat
        .get("agentKey")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty());
    let agent = if let Some(key) = persisted {
        registry
            .get(key)
            .cloned()
            .ok_or_else(|| format!("agent '{key}' no longer exists; choose another agent"))?
    } else {
        let scope = if workspace_id == "_house" {
            "house"
        } else if workspace_id == "_dfd" {
            "dfd"
        } else if workspace_id == "_personal" {
            "general"
        } else if !parent.trim().is_empty() {
            if workspace != state.harness_root {
                "developer"
            } else {
                "general"
            }
        } else if workspace != state.harness_root {
            "developer"
        } else {
            "orchestrator"
        };
        registry
            .get(scope)
            .cloned()
            .unwrap_or_else(|| registry.route(scope, &workspace, &state.harness_root))
    };
    let mut effective = agent.clone();
    let model = chat["model"]
        .as_str()
        .filter(|model| !model.trim().is_empty())
        .and_then(|requested| {
            state
                .config
                .resolve_model(&effective.provider, requested)
                .ok()
                .map(|(_, model)| model)
        })
        .unwrap_or_else(|| effective.model.clone());
    effective.model = model.clone();
    let effective = Arc::new(effective);
    Ok(RunTarget {
        chat_id: chat["id"].as_str().unwrap_or("").to_string(),
        workspace_id: workspace_id.to_string(),
        workspace,
        scope: agent.key.to_string(),
        role: Some(agent.display.to_string()),
        model,
        temperature: 0.2,
        depth: 0,
        agent_id: None,
        effective,
    })
}

async fn schedule_loop(state: ServerState, shutdown: CancellationToken) {
    let store = match ScheduleStore::open(&state.harness_root).await {
        Ok(store) => store,
        Err(error) => {
            tracing::warn!("scheduler disabled: {error}");
            return;
        }
    };
    loop {
        if !sleep_or_shutdown(std::time::Duration::from_secs(15), &shutdown).await {
            break;
        }
        let due = match store.due(20).await {
            Ok(due) => due,
            Err(error) => {
                tracing::warn!("scheduler due query failed: {error}");
                continue;
            }
        };
        for record in due {
            if let Err(error) = store.mark_queued(&record.id).await {
                tracing::warn!("scheduler mark queued failed: {error}");
                continue;
            }
            let workspace = if record.workspace_path.trim().is_empty() {
                state.harness_root.clone()
            } else {
                PathBuf::from(&record.workspace_path)
            };
            let registry = agent_registry(&state);
            let mut effective = match registry.get(&record.scope).cloned() {
                Some(agent) => agent,
                None => {
                    tracing::warn!(
                        "schedule {} references missing agent '{}'; skipped",
                        record.id,
                        record.scope
                    );
                    let _ = state
                        .store
                        .append_message(
                            &record.chat_id,
                            "error",
                            &format!(
                                "Scheduled run skipped: agent '{}' no longer exists.",
                                record.scope
                            ),
                            false,
                        )
                        .await;
                    if let Err(error) = store.mark_after_run(&record).await {
                        tracing::warn!("scheduler mark after run failed: {error}");
                    }
                    continue;
                }
            };
            let scope = effective.key.clone();
            let model = state
                .config
                .resolve_model(&effective.provider, &record.model)
                .map(|(_, model)| model)
                .unwrap_or_else(|_| effective.model.clone());
            effective.model = model.clone();
            let role = record
                .role
                .clone()
                .or_else(|| Some(effective.display.clone()));
            let effective = Arc::new(effective);
            spawn_follow_up_run(
                state.clone(),
                RunTarget {
                    chat_id: record.chat_id.clone(),
                    workspace_id: record.workspace_id.clone(),
                    workspace,
                    scope,
                    role,
                    model,
                    temperature: record.temperature,
                    depth: 0,
                    agent_id: None,
                    effective,
                },
                record.prompt.clone(),
            );
            if let Err(error) = store.mark_after_run(&record).await {
                tracing::warn!("scheduler mark after run failed: {error}");
            }
        }
    }
}

async fn group_loop(state: ServerState, shutdown: CancellationToken) {
    let groups = GroupStore::default_root();
    match groups.recover_doing_items().await {
        Ok(count) if count > 0 => tracing::info!("recovered {count} interrupted group item(s)"),
        Ok(_) => {}
        Err(error) => tracing::warn!("group runner recovery failed: {error}"),
    }
    loop {
        if !sleep_or_shutdown(std::time::Duration::from_secs(2), &shutdown).await {
            break;
        }
        for project in groups
            .list()
            .await
            .into_iter()
            .filter(|project| project["status"] == "running")
        {
            let Some(project_id) = project["id"].as_str() else {
                continue;
            };
            match groups.claim_next_ready(project_id).await {
                Ok(Some(item)) => {
                    let state = state.clone();
                    let groups = groups.clone();
                    let project_id = project_id.to_string();
                    tokio::spawn(async move {
                        run_group_item(state, groups, project_id, item).await;
                    });
                }
                Ok(None) => {}
                Err(error) => tracing::warn!("group runner could not claim an item: {error}"),
            }
        }
    }
}

fn group_member_runtime(
    registry: &AgentRegistry,
    owner: &str,
    detail: &Value,
) -> Option<EffectiveAgent> {
    let owner = owner.trim();
    if owner.is_empty() {
        return None;
    }
    // Agent names and aliases live in one registry, so custom agents resolve
    // here exactly like built-ins.
    if let Some(agent) = registry.get(owner) {
        return Some(agent.clone());
    }
    let members = detail.get("members").and_then(Value::as_array)?;
    for member in members {
        let matches = ["name", "display", "kind", "derived_from"]
            .iter()
            .filter_map(|key| member.get(*key).and_then(Value::as_str))
            .any(|value| value.eq_ignore_ascii_case(owner));
        if !matches {
            continue;
        }
        for key in ["kind", "derived_from", "name", "display"] {
            if let Some(candidate) = member.get(key).and_then(Value::as_str)
                && let Some(agent) = registry.get(candidate)
            {
                return Some(agent.clone());
            }
        }
    }
    None
}

fn group_item_runtime(
    registry: &AgentRegistry,
    item: &Value,
    detail: &Value,
) -> Result<EffectiveAgent, String> {
    let owner = item
        .get("owner_member")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if owner.is_empty() {
        return registry
            .get("developer")
            .cloned()
            .ok_or_else(|| "the developer agent is unavailable".to_string());
    }
    group_member_runtime(registry, owner, detail)
        .ok_or_else(|| format!("owner_member '{owner}' does not match an agent"))
}

const GROUP_RUNTIME_NOTICE: &str = "Group projects run in their project folder. owner_member selects the agent; items tagged needs-approval (or flagged requires_approval) park for approval.";

async fn run_group_item(state: ServerState, groups: GroupStore, project_id: String, item: Value) {
    let item_id = item["id"].as_str().unwrap_or("").to_string();
    // Pre-run approval gate: a tagged (or flagged) item parks for the owner
    // instead of executing. The existing approval route returns it to `todo`
    // on grant, so the claim loop picks it up again — nothing is lost.
    let wants_approval = item
        .get("requires_approval")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || item
            .get("tags")
            .and_then(Value::as_array)
            .is_some_and(|tags| {
                tags.iter().any(|tag| {
                    tag.as_str().is_some_and(|tag| {
                        tag.eq_ignore_ascii_case("needs-approval")
                            || tag.eq_ignore_ascii_case("needs_approval")
                    })
                })
            });
    if wants_approval {
        if let Err(error) = groups
            .park_for_approval(
                &project_id,
                &item_id,
                "owner approval required before work starts",
            )
            .await
        {
            tracing::warn!("group {project_id} item {item_id} park failed: {error}");
        }
        return;
    }
    let detail = match groups.detail(&project_id, 0).await {
        Ok(detail) => detail,
        Err(error) => {
            tracing::warn!("group {project_id} item lookup failed: {error}");
            return;
        }
    };
    let origin_workspace_id = detail["project"]["origin_workspace_id"]
        .as_str()
        .unwrap_or("");
    let workspace_id = if origin_workspace_id.is_empty() {
        "_general".to_string()
    } else {
        origin_workspace_id.to_string()
    };
    // Group work belongs to the project folder, not the workspace that asked
    // for the project. Running in the origin workspace (often the harness
    // root for General) would let an item write outside its project.
    let workspace = detail["path"]
        .as_str()
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| state.harness_root.clone());
    let registry = agent_registry(&state);
    let agent = match group_item_runtime(&registry, &item, &detail) {
        Ok(agent) => agent,
        Err(error) => {
            let _ = groups
                .finish_item(
                    &project_id,
                    item["id"].as_str().unwrap_or(""),
                    false,
                    &error,
                )
                .await;
            return;
        }
    };
    let effective = Arc::new(agent.clone());
    let model = effective.model.clone();
    let provider = state.provider_for(&effective);
    let title = format!(
        "Group: {} ({})",
        item["title"].as_str().unwrap_or("Task"),
        agent.display
    );
    let chat = match state
        .store
        .create_chat(
            &workspace_id,
            workspace.to_string_lossy().as_ref(),
            &title,
            &model,
        )
        .await
    {
        Ok(chat) => chat,
        Err(error) => {
            let message = format!("Could not create the worker chat: {error}");
            let _ = groups
                .finish_item(
                    &project_id,
                    item["id"].as_str().unwrap_or(""),
                    false,
                    &message,
                )
                .await;
            return;
        }
    };
    let chat_id = chat["id"].as_str().unwrap_or_default().to_string();
    let (tx, _) = broadcast::channel(RUN_EVENT_CAPACITY);
    let cancel = CancellationToken::new();
    let steering = Arc::new(Mutex::new(VecDeque::new()));
    let follow_up = Arc::new(Mutex::new(VecDeque::new()));
    let run_id = Checkpoint::next_run_id();
    let latest = Arc::new(Mutex::new(None));
    if let Err(error) = state.runs.claim(
        &chat_id,
        RunClaim {
            run_id: run_id.clone(),
            latest: latest.clone(),
            cancel: cancel.clone(),
            tx: tx.clone(),
            steering: steering.clone(),
            follow_up: follow_up.clone(),
            workspace_path: workspace.to_string_lossy().to_string(),
        },
    ) {
        if error == ClaimError::ShuttingDown {
            return;
        }
        let message = error.message();
        let _ = groups
            .finish_item(
                &project_id,
                item["id"].as_str().unwrap_or(""),
                false,
                &message,
            )
            .await;
        return;
    }

    let checkpoint = Checkpoint::begin_with_run_id(&chat_id, &workspace, run_id);
    if workspace != state.harness_root {
        let snapshot = checkpoint.clone();
        let _ = tokio::task::spawn_blocking(move || snapshot.snapshot_all()).await;
    }
    let ctx = RunContext {
        chat_id: chat_id.clone(),
        workspace_id,
        workspace: workspace.clone(),
        role: Some(agent.display.to_string()),
        scope: agent.key.to_string(),
        depth: 0,
        file_access: agent_access_for(&agent, 0, false),
        model,
        temperature: 0.2,
        images: Vec::new(),
        provider,
        store: state.store.clone(),
        questions: state.questions.clone(),
        tx,
        latest: latest.clone(),
        steering,
        follow_up,
        checkpoint: Some(checkpoint.clone()),
        cancel,
        config: Some(effective.clone()),
        on_job_done: None,
    };
    let acceptance =
        serde_json::to_string_pretty(&item["acceptance"]).unwrap_or_else(|_| "[]".to_string());
    let prompt = format!(
        "You are acting as the {display} member for one group-project item.\n\nProject goal:\n{}\n\nMilestone:\n{}\n\nItem:\n{}\n\nDetails:\n{}\n\nAcceptance criteria (all must be met):\n{}\n\nWork only on this item. Report what changed and how each criterion was met.",
        detail["project"]["goal"].as_str().unwrap_or(""),
        item["milestone_title"].as_str().unwrap_or(""),
        item["title"].as_str().unwrap_or(""),
        item["detail"].as_str().unwrap_or(""),
        acceptance,
        display = agent.display,
    );
    let result = run_agent(ctx, prompt).await;
    checkpoint.finalize();
    let state_snapshot = latest.lock().ok().and_then(|guard| guard.clone());
    let aborted = state_snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.active_node == "aborted");
    let transcript_report = state
        .store
        .history(&chat_id, 12)
        .await
        .ok()
        .and_then(|history| {
            history
                .iter()
                .rev()
                .find(|message| message.role == "assistant" && !message.content.trim().is_empty())
                .map(|message| message.content.chars().take(12_000).collect::<String>())
        });
    let report = transcript_report
        .or_else(|| {
            state_snapshot
                .as_ref()
                .map(|snapshot| {
                    let output = snapshot.outputs.to_string();
                    if output == "null" || output == "{}" {
                        snapshot.live_terminal_log.clone()
                    } else {
                        output
                    }
                })
                .filter(|text| !text.trim().is_empty())
        })
        .unwrap_or_else(|| {
            if result.is_ok() {
                "Agent completed the item.".to_string()
            } else {
                String::new()
            }
        });
    let success = result.is_ok() && !aborted;
    let output = match result {
        Ok(()) if success => format!("Worker chat: {chat_id}\n\n{report}"),
        Ok(()) => "Agent run was aborted.".to_string(),
        Err(error) => error.to_string(),
    };
    // Post-run acceptance verification: machine-checkable criteria must hold
    // before the item counts as done; `manual` criteria park the item for the
    // owner instead of failing it. Unsupported/free-form criteria are noted
    // in the report, never invented as failures.
    let (success, output) = if success {
        let verdict = evaluate_acceptance(&workspace, &item["acceptance"]);
        if !verdict.failed.is_empty() {
            (
                false,
                format!(
                    "Acceptance verification failed: {}\n\nAgent report:\n{output}",
                    verdict.summary()
                ),
            )
        } else if !verdict.needs_human.is_empty() {
            let reason = format!("manual acceptance check needed: {}", verdict.summary());
            state.runs.finish(&chat_id, state_snapshot);
            if let Err(error) = groups
                .park_for_approval(&project_id, &item_id, &reason)
                .await
            {
                tracing::warn!("group {project_id} item {item_id} park failed: {error}");
            }
            return;
        } else {
            (
                true,
                format!("{output}\n\nVerification: {}", verdict.summary()),
            )
        }
    } else {
        (success, output)
    };
    state.runs.finish(&chat_id, state_snapshot);
    let canvas_report = output.clone();
    let canvas_status = if success { "complete" } else { "failed" };
    spawn_canvas_notification(
        state.clone(),
        format!("group:{project_id}"),
        canvas_report,
        canvas_status,
    );
    if let Err(error) = groups
        .finish_item(
            &project_id,
            item["id"].as_str().unwrap_or(""),
            success,
            &output,
        )
        .await
    {
        tracing::warn!("group {project_id} item finish failed: {error}");
    }
    notify_group_origin(
        &state,
        &groups,
        &project_id,
        item["id"].as_str().unwrap_or(""),
        item["title"].as_str().unwrap_or("item"),
        success,
        &output,
    )
    .await;
}

async fn notify_group_origin(
    state: &ServerState,
    groups: &GroupStore,
    project_id: &str,
    item_id: &str,
    title: &str,
    success: bool,
    report: &str,
) {
    let Some(chat_id) = groups.origin_chat(project_id).await else {
        return;
    };
    let status = if success { "completed" } else { "failed" };
    let report: String = report.chars().take(4_000).collect();
    let message = format!(
        "[GROUP RESULT] project={project_id} item={item_id} ({title}) status={status}\n\n{report}"
    );
    if state.runs.follow_up(&chat_id, &message) {
        return;
    }
    let Ok(Some(chat)) = state.store.get_chat(&chat_id).await else {
        return;
    };
    let target = match canvas_target(state, &chat) {
        Ok(target) => target,
        Err(error) => {
            tracing::warn!("group result target invalid: {error}");
            return;
        }
    };
    spawn_follow_up_run(state.clone(), target, message);
}

struct StartedRun {
    chat_id: String,
    run_id: String,
    after_seq: u64,
}

enum StartRunError {
    Http(StatusCode, Value),
    AlreadyRunning {
        chat_id: String,
        run_id: Option<String>,
        current_seq: u64,
    },
}

impl StartRunError {
    fn into_http(self) -> (StatusCode, Json<Value>) {
        match self {
            Self::Http(status, body) => (status, Json(body)),
            Self::AlreadyRunning {
                chat_id,
                run_id,
                current_seq,
            } => (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "This chat already has a running agent.",
                    "code": "already_running",
                    "chat_id": chat_id,
                    "run_id": run_id,
                    "current_seq": current_seq,
                })),
            ),
        }
    }
}

fn latest_seq(harness_root: &std::path::Path, chat_id: &str) -> u64 {
    stark_store::rollout::find_rollout(harness_root, chat_id)
        .and_then(|path| stark_store::rollout::latest_ordinal(&path))
        .unwrap_or(0)
}

/// Shared admission + launch path for `POST /api/chats/{chat_id}/run`.
async fn start_run(state: &ServerState, request: RunRequest) -> Result<StartedRun, StartRunError> {
    let mut prompt = request.prompt.trim().to_string();
    if !request.attachments.is_empty() {
        let note = attachment_note(&state.harness_root, &request.attachments);
        if prompt.is_empty() {
            prompt = "Review the attached files.".to_string();
        }
        if !note.is_empty() {
            prompt.push_str(
                "

Attached files:
",
            );
            prompt.push_str(&note);
        }
    }
    if prompt.is_empty() {
        return Err(StartRunError::Http(
            StatusCode::BAD_REQUEST,
            json!({"error": "prompt is required"}),
        ));
    }
    // Drain gate: deploys write this marker, wait for active runs to settle,
    // swap the binary, then remove it. New runs are refused meanwhile so a
    // restart can never kill a delegation mid-flight.
    if state.harness_root.join(".drain").exists() {
        return Err(StartRunError::Http(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({
                "error": "Stark is deploying a new version. Your message was not started — retry in a moment.",
                "draining": true,
            }),
        ));
    }
    // Concurrency cap: bounds parallel runs so a burst degrades into a clear
    // 429 instead of an OOM kill. Raise STARK_MAX_CONCURRENT_RUNS per host.
    let cap = concurrent_cap();
    let running = state.runs.running_count();
    if running >= cap {
        return Err(StartRunError::Http(
            StatusCode::TOO_MANY_REQUESTS,
            json!({
                "error": format!(
                    "Stark is already running {running} session(s) (limit {cap}). Wait for one to finish or raise STARK_MAX_CONCURRENT_RUNS."
                ),
                "busy": true,
                "running": running,
                "limit": cap,
            }),
        ));
    }

    let chat_id = if request.chat_id.trim().is_empty() {
        Uuid::new_v4().to_string()
    } else {
        request.chat_id.trim().to_string()
    };
    let workspace = if !request.workspace_path.trim().is_empty() {
        PathBuf::from(request.workspace_path.trim())
    } else if let Ok(Some(path)) = state.store.chat_workspace_path(&chat_id).await
        && !path.trim().is_empty()
    {
        PathBuf::from(path)
    } else {
        state.harness_root.clone()
    };
    let registry = agent_registry(state);
    let requested_agent = request.agent.trim();
    let agent = if !requested_agent.is_empty() {
        registry.get(requested_agent).cloned().ok_or_else(|| {
            StartRunError::Http(
                StatusCode::BAD_REQUEST,
                json!({"error": format!("unknown agent '{requested_agent}'")}),
            )
        })?
    } else {
        // Persisted chat agent wins over workspace routing, so a custom agent
        // keeps its identity on every follow-up turn.
        let persisted = match state.store.get_chat(&chat_id).await {
            Ok(Some(chat)) => chat
                .get("agentKey")
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|key| !key.trim().is_empty()),
            _ => None,
        };
        match persisted {
            Some(key) => registry.get(&key).cloned().ok_or_else(|| {
                StartRunError::Http(
                    StatusCode::BAD_REQUEST,
                    json!({
                        "error": format!(
                            "agent '{key}' no longer exists; choose another agent"
                        )
                    }),
                )
            })?,
            None => registry.route(&request.workspace_kind, &workspace, &state.harness_root),
        }
    };
    let mut effective = agent.clone();
    let model = if request.model.trim().is_empty() {
        effective.model.clone()
    } else {
        match state
            .config
            .resolve_model(&effective.provider, request.model.trim())
        {
            Ok((_, model)) => model,
            Err(error) => {
                return Err(StartRunError::Http(
                    StatusCode::BAD_REQUEST,
                    json!({"error": error.to_string()}),
                ));
            }
        }
    };
    // The model picker is a per-run override; keep the provider client in sync
    // with it (the agent config remains the default for the next run).
    effective.model = model.clone();
    let effective = Arc::new(effective);
    let provider = state.provider_for(&effective);
    let workspace_id = if !request.workspace_id.trim().is_empty() {
        request.workspace_id.trim().to_string()
    } else if let Ok(Some(existing)) = state.store.chat_workspace_id(&chat_id).await {
        existing
    } else if request
        .workspace_kind
        .trim()
        .eq_ignore_ascii_case("personal")
    {
        // Direct General-agent chats live in the Personal virtual workspace so
        // the sidebar's "General" mode lists them, separate from Main.
        "_personal".to_string()
    } else if request.workspace_path.trim().is_empty() {
        "_general".to_string()
    } else {
        state
            .store
            .workspace_id_for_path(workspace.to_string_lossy().as_ref())
    };

    let (tx, _) = broadcast::channel(RUN_EVENT_CAPACITY);
    let cancel = CancellationToken::new();
    let steering = Arc::new(Mutex::new(VecDeque::new()));
    let follow_up = Arc::new(Mutex::new(VecDeque::new()));
    let run_id = Checkpoint::next_run_id();
    let latest = Arc::new(Mutex::new(None));
    if let Err(error) = state.runs.claim(
        &chat_id,
        RunClaim {
            run_id: run_id.clone(),
            latest: latest.clone(),
            cancel: cancel.clone(),
            tx: tx.clone(),
            steering: steering.clone(),
            follow_up: follow_up.clone(),
            workspace_path: workspace.to_string_lossy().to_string(),
        },
    ) {
        if error == ClaimError::AlreadyRunning {
            let run_id = state.runs.run_id(&chat_id);
            let current_seq = latest_seq(&state.harness_root, &chat_id);
            return Err(StartRunError::AlreadyRunning {
                chat_id,
                run_id,
                current_seq,
            });
        }
        let status = match error {
            ClaimError::AlreadyRunning => StatusCode::CONFLICT,
            ClaimError::Limit { .. } => StatusCode::TOO_MANY_REQUESTS,
            ClaimError::ShuttingDown => StatusCode::SERVICE_UNAVAILABLE,
        };
        return Err(StartRunError::Http(
            status,
            json!({"error": error.message()}),
        ));
    }
    let after_seq = latest_seq(&state.harness_root, &chat_id);
    let checkpoint = Checkpoint::begin_with_run_id(&chat_id, &workspace, run_id.clone());

    if workspace != state.harness_root {
        let checkpoint = checkpoint.clone();
        let _ = tokio::task::spawn_blocking(move || checkpoint.snapshot_all()).await;
    }

    let target = RunTarget {
        chat_id: chat_id.clone(),
        workspace_id,
        workspace,
        scope: agent.key.to_string(),
        role: Some(agent.display.to_string()),
        model,
        temperature: request.temperature,
        depth: 0,
        agent_id: None,
        effective: effective.clone(),
    };
    // Image attachments (uploads, voice screen frames) ride the opening user
    // message as real vision parts; the text note in the prompt stays for
    // context. Images are one-shot: never persisted into chat history.
    let upload_root = state.harness_root.join(".deep_agents").join("uploads");
    let screen_root = state.harness_root.join(".deep_agents").join("live_screen");
    let run_images: Vec<ImagePart> =
        stark_tools::collect_inline_images(&[upload_root, screen_root], &request.attachments)
            .into_iter()
            .map(|image| ImagePart {
                url: image.data_url(),
                note: image.name,
            })
            .collect();
    let root_access = agent_access_for(&agent, 0, false);
    // Ensure the root agent exists before the run so status transitions are
    // visible in the tree for the whole run.
    let target = ensure_root_agent(state, target, root_access.as_str()).await;
    let ctx = RunContext {
        chat_id: target.chat_id.clone(),
        workspace_id: target.workspace_id.clone(),
        workspace: target.workspace.clone(),
        role: target.role.clone(),
        scope: target.scope.clone(),
        depth: target.depth,
        file_access: root_access,
        model: target.model.clone(),
        temperature: target.temperature,
        images: run_images,
        provider: provider.clone(),
        store: state.store.clone(),
        questions: state.questions.clone(),
        tx: tx.clone(),
        latest: latest.clone(),
        steering,
        follow_up,
        checkpoint: Some(checkpoint.clone()),
        cancel,
        config: Some(effective.clone()),
        on_job_done: Some(job_hook(state.clone(), target)),
    };

    let manager = state.runs.clone();
    let done_tx = tx.clone();
    let canvas_state = state.clone();
    let canvas_chat = chat_id.clone();
    let status_state = state.clone();
    let status_chat = chat_id.clone();
    let started_chat_id = chat_id.clone();
    let prompt_for_run = prompt;
    tokio::spawn(async move {
        let result = run_agent(ctx, prompt_for_run).await;
        checkpoint.finalize();
        let error = result.as_ref().err().map(ToString::to_string);
        let final_state = latest.lock().ok().and_then(|guard| guard.clone());
        let status = if error.is_some() {
            "error"
        } else if final_state
            .as_ref()
            .is_some_and(|state| state.active_node == "aborted")
        {
            "aborted"
        } else {
            "complete"
        };
        let report = final_state
            .as_ref()
            .and_then(|state| state.outputs.get("agent_report"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let usage = final_state
            .as_ref()
            .map(|state| serde_json::to_value(&state.token_usage).unwrap_or_else(|_| json!({})))
            .unwrap_or_else(|| json!({}));
        // Persist the terminal agent status BEFORE the client sees `done`.
        // Otherwise the tree polls and still sees `running`.
        finish_agent_status(&status_state, &status_chat, status, &report, &usage).await;
        manager.finish(&chat_id, final_state);
        let _ = done_tx.send(ServerEvent::new(
            "done",
            json!({"status": status, "error": error}),
        ));
        spawn_canvas_notification(canvas_state, canvas_chat, report, status);
    });

    Ok(StartedRun {
        chat_id: started_chat_id,
        run_id,
        after_seq,
    })
}

async fn run_command_handler(
    State(state): State<ServerState>,
    Path(chat_id): Path<String>,
    Json(mut request): Json<RunRequest>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    if chat_id.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "chat_id is required"})),
        ));
    }
    request.chat_id = chat_id;
    let started = start_run(&state, request)
        .await
        .map_err(StartRunError::into_http)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "accepted": true,
            "chat_id": started.chat_id,
            "run_id": started.run_id,
            "after_seq": started.after_seq,
        })),
    ))
}

fn job_hook(state: ServerState, target: RunTarget) -> stark_core::JobDoneHook {
    Arc::new(move |message| {
        if state.runs.follow_up(&target.chat_id, &message) {
            return;
        }
        spawn_follow_up_run(state.clone(), target.clone(), message);
    })
}

/// OpenCode's `SessionRestart` Code Mode branch: every script still running
/// when the server stopped becomes `Interrupted by server restart.`, and a
/// finished script whose notice never reached the session is delivered once.
async fn deliver_code_mode_restart_notices(state: ServerState) {
    let notices = stark_tools::recover_code_mode_notices(&state.harness_root);
    for notice in notices {
        let chat = match state.store.get_chat(&notice.chat_id).await {
            Ok(Some(chat)) => chat,
            _ => continue,
        };
        let target = match canvas_target(&state, &chat) {
            Ok(target) => target,
            Err(error) => {
                tracing::warn!(
                    "Code Mode restart notice for {} dropped: {error}",
                    notice.job_id
                );
                stark_tools::acknowledge_code_mode_notice(&state.harness_root, &notice.job_id);
                continue;
            }
        };
        if state.runs.follow_up(&target.chat_id, &notice.message) {
            stark_tools::acknowledge_code_mode_notice(&state.harness_root, &notice.job_id);
            continue;
        }
        spawn_follow_up_run(state.clone(), target, notice.message);
        stark_tools::acknowledge_code_mode_notice(&state.harness_root, &notice.job_id);
        tracing::info!("delivered Code Mode restart notice for {}", notice.job_id);
    }
}

fn spawn_follow_up_run(state: ServerState, target: RunTarget, message: String) {
    spawn_follow_up_run_attempt(state, target, message, 0);
}

/// A follow-up was triggered by a finished run, so a global slot is usually
/// free immediately. If another chat took it, retry with capped backoff
/// instead of dropping the message. Per-chat conflicts attach to the existing
/// run's steering queue.
fn spawn_follow_up_run_attempt(
    state: ServerState,
    mut target: RunTarget,
    message: String,
    attempt: u32,
) {
    // Re-resolve on every follow-up so config edits apply without waiting for
    // a new chat. A deleted custom agent is an explicit error, never a silent
    // fallback to a more privileged built-in.
    let runtime = match agent_registry(&state).get(&target.scope).cloned() {
        Some(agent) => agent,
        None => {
            let store = state.store.clone();
            let chat_id = target.chat_id.clone();
            let scope = target.scope.clone();
            tokio::spawn(async move {
                let _ = store
                    .append_message(
                        &chat_id,
                        "error",
                        &format!(
                            "Agent '{scope}' no longer exists. Pick another agent to continue."
                        ),
                        false,
                    )
                    .await;
            });
            return;
        }
    };
    target.model = runtime.model.clone();
    target.effective = Arc::new(runtime);
    let (tx, _) = broadcast::channel(RUN_EVENT_CAPACITY);
    let cancel = CancellationToken::new();
    let steering = Arc::new(Mutex::new(VecDeque::new()));
    let follow_up = Arc::new(Mutex::new(VecDeque::new()));
    let run_id = Checkpoint::next_run_id();
    let latest = Arc::new(Mutex::new(None));
    if let Err(error) = state.runs.claim(
        &target.chat_id,
        RunClaim {
            run_id: run_id.clone(),
            latest: latest.clone(),
            cancel: cancel.clone(),
            tx: tx.clone(),
            steering: steering.clone(),
            follow_up: follow_up.clone(),
            workspace_path: target.workspace.to_string_lossy().to_string(),
        },
    ) {
        match error {
            ClaimError::AlreadyRunning => {
                let _ = state.runs.follow_up(&target.chat_id, &message);
            }
            ClaimError::ShuttingDown => return,
            ClaimError::Limit { max } => {
                const MAX_ADMISSION_RETRIES: u32 = 6;
                if attempt >= MAX_ADMISSION_RETRIES {
                    tracing::warn!(
                        chat_id = %target.chat_id,
                        max,
                        message_chars = message.chars().count(),
                        "dropping follow-up: run limit stayed saturated"
                    );
                    // Make the drop visible in history instead of losing the
                    // message silently. Same role the turn loop uses for
                    // failures, so the UI renders it as an error.
                    let store = state.store.clone();
                    let chat_id = target.chat_id.clone();
                    tokio::spawn(async move {
                        let _ = store
                            .append_message(
                                &chat_id,
                                "error",
                                &format!(
                                    "Follow-up not started: the server stayed at its {max}-run limit. Send the message again once a run finishes."
                                ),
                                false,
                            )
                            .await;
                    });
                    return;
                }
                let delay = std::time::Duration::from_secs(1 << attempt.min(4));
                tokio::spawn(async move {
                    tokio::time::sleep(delay).await;
                    spawn_follow_up_run_attempt(state, target, message, attempt + 1);
                });
            }
        }
        return;
    }

    let checkpoint = Checkpoint::begin_with_run_id(&target.chat_id, &target.workspace, run_id);
    let follow_access =
        agent_access_for(&target.effective, target.depth, target.agent_id.is_some());
    let ctx = RunContext {
        chat_id: target.chat_id.clone(),
        workspace_id: target.workspace_id.clone(),
        workspace: target.workspace.clone(),
        role: target.role.clone(),
        scope: target.scope.clone(),
        depth: target.depth,
        file_access: follow_access,
        model: target.model.clone(),
        temperature: target.temperature,
        images: Vec::new(),
        provider: state.provider_for(&target.effective),
        store: state.store.clone(),
        questions: state.questions.clone(),
        tx: tx.clone(),
        latest: latest.clone(),
        steering,
        follow_up,
        checkpoint: Some(checkpoint.clone()),
        cancel,
        config: Some(target.effective.clone()),
        on_job_done: Some(job_hook(state.clone(), target.clone())),
    };
    let manager = state.runs.clone();
    let done_tx = tx.clone();
    let chat_id = target.chat_id.clone();
    let canvas_state = state.clone();
    let canvas_chat = target.chat_id.clone();
    let status_state = state.clone();
    let status_chat = target.chat_id.clone();
    {
        let state = state.clone();
        let chat_id = target.chat_id.clone();
        tokio::spawn(async move {
            mark_agent_status(&state, &chat_id, "running").await;
        });
    }
    tokio::spawn(async move {
        let result = run_agent(ctx, message).await;
        checkpoint.finalize();
        let error = result.as_ref().err().map(ToString::to_string);
        let final_state = latest.lock().ok().and_then(|guard| guard.clone());
        let status = if error.is_some() {
            "error"
        } else if final_state
            .as_ref()
            .is_some_and(|state| state.active_node == "aborted")
        {
            "aborted"
        } else {
            "complete"
        };
        let report = final_state
            .as_ref()
            .and_then(|state| state.outputs.get("agent_report"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let usage = final_state
            .as_ref()
            .map(|state| serde_json::to_value(&state.token_usage).unwrap_or_else(|_| json!({})))
            .unwrap_or_else(|| json!({}));
        finish_agent_status(&status_state, &status_chat, status, &report, &usage).await;
        manager.finish(&chat_id, final_state);
        let _ = done_tx.send(ServerEvent::new(
            "done",
            json!({"status": status, "error": error}),
        ));
        spawn_canvas_notification(canvas_state, canvas_chat, report, status);
    });
}

async fn ensure_root_agent(state: &ServerState, target: RunTarget, access: &str) -> RunTarget {
    let Ok(store) = AgentStore::open(&state.harness_root).await else {
        return target;
    };
    let (kind, path) = if target.scope == "orchestrator" {
        ("orchestrator", format!("orchestrator:{}", target.chat_id))
    } else {
        ("workspace", format!("root:{}", target.chat_id))
    };
    let agent_id = match store.by_chat(&target.chat_id).await {
        Ok(Some(agent)) => {
            let _ = store.set_status(&agent.id, "running", true).await;
            let _ = store.set_access(&agent.id, access).await;
            Some(agent.id)
        }
        _ => match store
            .create_agent(
                kind,
                &path,
                None,
                &target.chat_id,
                &target.workspace_id,
                &target.scope,
                target.role.as_deref().unwrap_or("Agent"),
                "",
                &target.model,
                0,
            )
            .await
        {
            Ok(agent) => {
                let _ = store.set_status(&agent.id, "running", true).await;
                let _ = store.set_access(&agent.id, access).await;
                Some(agent.id)
            }
            Err(_) => None,
        },
    };
    RunTarget { agent_id, ..target }
}

async fn finish_agent_status(
    state: &ServerState,
    chat_id: &str,
    run_status: &str,
    report: &str,
    usage: &Value,
) {
    let agent_status = match run_status {
        "complete" => "completed",
        "aborted" => "cancelled",
        _ => "failed",
    };
    let Ok(store) = AgentStore::open(&state.harness_root).await else {
        return;
    };
    let Ok(Some(agent)) = store.by_chat(chat_id).await else {
        return;
    };
    let _ = store
        .finish(&agent.id, agent_status, report, usage.clone())
        .await;
}

async fn mark_agent_status(state: &ServerState, chat_id: &str, status: &str) {
    let Ok(store) = AgentStore::open(&state.harness_root).await else {
        return;
    };
    if let Ok(Some(agent)) = store.by_chat(chat_id).await {
        let _ = store
            .set_status(&agent.id, status, status == "running")
            .await;
    }
}

async fn agent_loop(state: ServerState, shutdown: CancellationToken) {
    let store = match AgentStore::open(&state.harness_root).await {
        Ok(store) => store,
        Err(error) => {
            tracing::warn!("durable agents disabled: {error}");
            return;
        }
    };
    if let Ok(count) = store.recover_running().await
        && count > 0
    {
        tracing::info!("recovered {count} interrupted agent(s)");
        // The rollout file is never mutated here. An unfinished TurnStarted
        // without a matching TurnComplete/TurnAborted is the interruption
        // signal; resume derives it on read.
    }
    if let Ok(agents) = store.list(None).await {
        for note in stark_core::worktree::gc(&state.harness_root, &agents) {
            tracing::info!("worktree gc: {note}");
        }
    }
    loop {
        if !sleep_or_shutdown(std::time::Duration::from_secs(1), &shutdown).await {
            break;
        }
        let due = match store.pending_trigger(8).await {
            Ok(due) => due,
            Err(error) => {
                tracing::warn!("agent mailbox query failed: {error}");
                continue;
            }
        };
        for agent in due {
            if !store.claim(&agent.id).await.unwrap_or(false) {
                continue;
            }
            if !spawn_agent_run(state.clone(), store.clone(), agent.clone()).await {
                let _ = store.requeue(&agent.id).await;
            }
        }
    }
}

async fn agent_target(state: &ServerState, agent: &AgentRecord) -> Result<RunTarget, String> {
    let mut workspace_path = state
        .store
        .chat_workspace_path(&agent.chat_id)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    // An isolated agent runs in its worktree, not the parent workspace.
    if !agent.worktree_path.trim().is_empty() {
        let subdir = agent.workspace_subdir.trim().trim_matches('/');
        if subdir.is_empty() {
            workspace_path = agent.worktree_path.clone();
        } else {
            workspace_path = format!("{}/{}", agent.worktree_path.trim_end_matches('/'), subdir);
        }
    }
    let workspace = if workspace_path.trim().is_empty() {
        state.harness_root.clone()
    } else {
        PathBuf::from(workspace_path)
    };
    // Stored agents may carry a legacy alias ("coder", "dev", "frontend"), or
    // a custom key. Resolve through the one registry.
    let registry = agent_registry(state);
    let effective = registry
        .get(&agent.scope)
        .cloned()
        .ok_or_else(|| format!("agent '{}' no longer exists", agent.scope))?;
    let scope = effective.key.clone();
    // Config is authoritative. The stored `agent.model` is a snapshot from
    // creation time and may reference a provider the app no longer uses.
    Ok(RunTarget {
        chat_id: agent.chat_id.clone(),
        workspace_id: agent.workspace_id.clone(),
        workspace,
        scope,
        role: (!agent.role.is_empty())
            .then(|| agent.role.clone())
            .or_else(|| Some(effective.display.clone())),
        model: effective.model.clone(),
        temperature: 0.2,
        depth: agent.depth.max(0) as u32,
        agent_id: Some(agent.id.clone()),
        effective: Arc::new(effective),
    })
}

async fn spawn_agent_run(state: ServerState, store: AgentStore, agent: AgentRecord) -> bool {
    let target = match agent_target(&state, &agent).await {
        Ok(target) => target,
        Err(error) => {
            tracing::warn!("agent {} target invalid: {error}", agent.id);
            let _ = store.set_status(&agent.id, "failed", false).await;
            return false;
        }
    };
    let pending = match store.messages(&agent.id).await {
        Ok(messages) => messages,
        Err(error) => {
            tracing::warn!("agent {} mailbox failed: {error}", agent.id);
            return false;
        }
    };
    if pending.is_empty() {
        let _ = store.set_status(&agent.id, "idle", false).await;
        return false;
    }
    let (tx, _) = broadcast::channel(RUN_EVENT_CAPACITY);
    let cancel = CancellationToken::new();
    let steering = Arc::new(Mutex::new(VecDeque::new()));
    let follow_up = Arc::new(Mutex::new(VecDeque::new()));
    let run_id = Checkpoint::next_run_id();
    let latest = Arc::new(Mutex::new(None));
    if state
        .runs
        .claim(
            &target.chat_id,
            RunClaim {
                run_id: run_id.clone(),
                latest: latest.clone(),
                cancel: cancel.clone(),
                tx: tx.clone(),
                steering: steering.clone(),
                follow_up: follow_up.clone(),
                workspace_path: target.workspace.to_string_lossy().to_string(),
            },
        )
        .is_err()
    {
        return false;
    }
    let messages = match store.take_messages(&agent.id, 20).await {
        Ok(messages) => messages,
        Err(error) => {
            tracing::warn!("agent {} mailbox take failed: {error}", agent.id);
            state.runs.finish(&target.chat_id, None);
            return false;
        }
    };
    if messages.is_empty() {
        state.runs.finish(&target.chat_id, None);
        let _ = store.set_status(&agent.id, "idle", false).await;
        return false;
    }
    let prompt = messages
        .iter()
        .map(|message| message["body"].as_str().unwrap_or("").to_string())
        .collect::<Vec<_>>()
        .join("\n\n");
    let checkpoint = Checkpoint::begin_with_run_id(&target.chat_id, &target.workspace, run_id);
    let access = stark_core::FileAccess::from_str(&agent.access);
    let ctx = RunContext {
        chat_id: target.chat_id.clone(),
        workspace_id: target.workspace_id.clone(),
        workspace: target.workspace.clone(),
        role: target.role.clone(),
        scope: target.scope.clone(),
        depth: agent.depth.max(0) as u32,
        file_access: access,
        model: target.model.clone(),
        temperature: target.temperature,
        images: Vec::new(),
        provider: state.provider_for(&target.effective),
        store: state.store.clone(),
        questions: state.questions.clone(),
        tx: tx.clone(),
        latest: latest.clone(),
        steering,
        follow_up,
        checkpoint: Some(checkpoint.clone()),
        cancel: cancel.clone(),
        config: Some(target.effective.clone()),
        on_job_done: Some(job_hook(state.clone(), target.clone())),
    };
    stark_core::register_agent_cancel(&agent.id, cancel.clone());
    let manager = state.runs.clone();
    let done_tx = tx.clone();
    let canvas_state = state.clone();
    let canvas_chat = target.chat_id.clone();
    let worktree_path = agent.worktree_path.clone();
    let worktree_branch = agent.worktree_branch.clone();
    let worktree_base = agent.worktree_base.clone();
    tokio::spawn(async move {
        let result = run_agent(ctx, prompt).await;
        stark_core::unregister_agent_cancel(&agent.id);
        checkpoint.finalize();
        let error = result.as_ref().err().map(ToString::to_string);
        let final_state = latest.lock().ok().and_then(|guard| guard.clone());
        let status = if error.is_some() {
            "failed"
        } else if final_state
            .as_ref()
            .is_some_and(|state| state.active_node == "aborted")
        {
            "cancelled"
        } else {
            "completed"
        };
        let report = final_state
            .as_ref()
            .and_then(|state| state.outputs.get("agent_report"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let usage = final_state
            .as_ref()
            .and_then(|state| serde_json::to_value(&state.token_usage).ok())
            .unwrap_or_else(|| json!({}));
        // Commit isolated work before the parent sees the result, then
        // persist the terminal status before `done` so the tree is accurate.
        let mut worktree_note = String::new();
        if !worktree_path.trim().is_empty() {
            let info = stark_core::worktree::WorktreeInfo {
                path: std::path::PathBuf::from(&worktree_path),
                branch: worktree_branch.clone(),
                base: worktree_base.clone(),
                repo_root: std::path::PathBuf::from(agent.repo_root.clone()),
                subdir: std::path::PathBuf::from(agent.workspace_subdir.clone()),
            };
            match stark_core::worktree::commit_changes(&info, &format!("stark {}", agent.id)) {
                Ok(outcome) => {
                    let diff = stark_core::worktree::diff(&info.path, &info.base)
                        .unwrap_or_else(|_| json!({}));
                    let _ = store
                        .set_worktree_state(&agent.id, "active", &diff, "")
                        .await;
                    if outcome["committed"].as_bool().unwrap_or(false) {
                        worktree_note = format!(
                            "\n[WORKTREE] branch={} base={}\n{}",
                            info.branch,
                            info.base,
                            outcome["stat"].as_str().unwrap_or("")
                        );
                    }
                }
                Err(commit_error) => {
                    let _ = store
                        .set_worktree_state(&agent.id, "failed", &json!({}), &commit_error)
                        .await;
                    worktree_note = format!("\n[WORKTREE ERROR] {commit_error}");
                }
            }
        }
        let full_report = match &error {
            Some(error) => format!("{report}{worktree_note}\n\n[ERROR] {error}"),
            None => format!("{report}{worktree_note}"),
        };
        let _ = store.finish(&agent.id, status, &full_report, usage).await;
        manager.finish(&canvas_chat, final_state);
        let _ = done_tx.send(ServerEvent::new(
            "done",
            json!({"status": status, "error": error}),
        ));
        if let Ok(Some(message_id)) = store.notify_parent(&agent, &full_report, status).await {
            wake_parent_agent(
                &store,
                &canvas_state,
                &agent,
                &full_report,
                status,
                message_id,
            )
            .await;
        }
        if store
            .messages(&agent.id)
            .await
            .map(|messages| !messages.is_empty())
            .unwrap_or(false)
        {
            let _ = store.set_status(&agent.id, "queued", false).await;
        }
        spawn_canvas_notification(canvas_state, canvas_chat, report, status);
    });
    true
}

async fn wake_parent_agent(
    store: &AgentStore,
    state: &ServerState,
    child: &AgentRecord,
    report: &str,
    status: &str,
    message_id: String,
) {
    let Some(parent_id) = child.parent_id.as_deref() else {
        return;
    };
    let Ok(Some(parent)) = store.get(parent_id).await else {
        return;
    };
    let target = match agent_target(state, &parent).await {
        Ok(target) => target,
        Err(error) => {
            tracing::warn!("parent agent target invalid: {error}");
            return;
        }
    };
    let text = format!(
        "[AGENT RESULT] {} · {} · {}\nchat: {}\n{}\nReview the child result and continue.",
        child.path,
        child.role,
        status,
        child.chat_id,
        report.chars().take(12_000).collect::<String>()
    );
    // The mailbox row is the durable copy. The immediate wake is best-effort;
    // the parent's next turn also sees the unconsumed message if delivery
    // has not happened yet.
    if !state.runs.follow_up(&parent.chat_id, &text) {
        spawn_follow_up_run(state.clone(), target, text);
    }
    let _ = store.consume_message(&message_id).await;
}

fn spawn_canvas_notification(state: ServerState, chat_id: String, report: String, status: &str) {
    let status = status.to_string();
    tokio::spawn(async move {
        let _ =
            stark_core::canvas::notify_run_finished(&state.store, &chat_id, &report, &status).await;
    });
}

fn rollout_event(line: &stark_store::rollout::RolloutLine) -> (&'static str, String) {
    use stark_store::rollout::{EventMsgPayload, ResponseItemPayload, RolloutItem};
    match &line.item {
        RolloutItem::ResponseItem { payload, .. } => match payload {
            ResponseItemPayload::Message { role, content, .. } => {
                let text = match content {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                };
                ("delta", json!({"kind": "text", "role": role, "text": text}).to_string())
            }
            ResponseItemPayload::Reasoning { summary, .. } => {
                let text = summary.first().map(|entry| entry.text.clone()).unwrap_or_default();
                ("delta", json!({"kind": "reasoning", "text": text}).to_string())
            }
            ResponseItemPayload::FunctionCall { call_id, name, arguments, .. } => (
                "delta",
                json!({"kind": "tool_started", "tool": name, "id": call_id, "args": arguments}).to_string(),
            ),
            ResponseItemPayload::FunctionCallOutput { call_id, name, output, is_error } => (
                "delta",
                json!({"kind": "tool_finished", "tool": name, "id": call_id, "ok": !is_error, "preview": output.chars().take(300).collect::<String>()}).to_string(),
            ),
        },
        RolloutItem::EventMsg { payload } => match payload {
            EventMsgPayload::Error { message, .. } => ("error", json!({"message": message}).to_string()),
            EventMsgPayload::Warning { message } => ("warning", json!({"message": message}).to_string()),
            EventMsgPayload::TurnStarted { turn_id, .. } => ("turn_started", json!({"turn_id": turn_id}).to_string()),
            EventMsgPayload::TurnComplete { turn_id, error, last_agent_message, .. } => (
                "turn_complete",
                json!({"turn_id": turn_id, "error": error, "last_message": last_agent_message}).to_string(),
            ),
            EventMsgPayload::TurnAborted { turn_id, reason, error, .. } => (
                "turn_aborted",
                json!({"turn_id": turn_id, "reason": reason, "error": error}).to_string(),
            ),
        },
        RolloutItem::TokenUsageRecord { payload } => (
            "delta",
            json!({"kind": "usage", "input": payload.input_tokens, "output": payload.output_tokens}).to_string(),
        ),
        RolloutItem::SessionMeta { .. }
        | RolloutItem::TurnContext { .. }
        | RolloutItem::Compacted { .. } => ("state", json!({"kind": "rollout", "ordinal": line.ordinal}).to_string()),
    }
}

fn sse_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-transform"),
    );
    headers.insert(
        HeaderName::from_static("x-accel-buffering"),
        HeaderValue::from_static("no"),
    );
    headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    headers
}

fn opencode_event_value(event: ServerEvent) -> Value {
    json!({
        "id": event.id,
        "type": event.event,
        "created": event.created,
        "data": event.data,
    })
}

fn opencode_frame(value: Value) -> Event {
    Event::default().data(value.to_string())
}

fn opencode_sse_response(stream: SseStream) -> Response {
    (
        sse_headers(),
        Sse::new(stream).keep_alive(KeepAlive::new().interval(SSE_HEARTBEAT).text("heartbeat")),
    )
        .into_response()
}

fn parse_types(raw: Option<&str>) -> Option<Arc<HashSet<String>>> {
    let raw = raw?;
    let types: HashSet<String> = raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect();
    (!types.is_empty()).then(|| Arc::new(types))
}

fn query_flag(raw: Option<&str>) -> bool {
    raw.is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

#[derive(Debug, Deserialize, Default)]
struct EventFeedQuery {
    #[serde(default)]
    types: Option<String>,
}

fn connected_event(chat_id: &str, running: bool, seq: u64, run_id: Option<&str>) -> Value {
    let mut data = json!({
        "chat_id": chat_id,
        "running": running,
        "seq": seq,
    });
    if let Some(run_id) = run_id {
        data["run_id"] = json!(run_id);
    }
    opencode_event_value(ServerEvent::new("server.connected", data))
}

async fn chat_events_handler(
    State(state): State<ServerState>,
    Path(chat_id): Path<String>,
    Query(query): Query<EventFeedQuery>,
) -> Response {
    // Subscribe before reading the watermark. Events emitted between the
    // subscription and the handshake stay buffered in the receiver; the
    // announced seq lets the client open the durable log without a gap.
    let attached = state.runs.attach(&chat_id);
    let child = if attached.is_none() {
        stark_core::subscribe_child_run(&chat_id)
    } else {
        None
    };
    let running = attached.is_some() || child.is_some();
    let run_id = attached
        .as_ref()
        .map(|attachment| attachment.run_id.clone());
    let seq = latest_seq(&state.harness_root, &chat_id);
    let connected = opencode_frame(connected_event(&chat_id, running, seq, run_id.as_deref()));
    let receiver = attached.map(|attachment| attachment.receiver).or(child);
    let stream: SseStream = if let Some(receiver) = receiver {
        let live = live_opencode_stream(receiver, parse_types(query.types.as_deref()));
        Box::pin(stream::iter(vec![Ok(connected)]).chain(live))
    } else {
        Box::pin(stream::iter(vec![Ok(connected)]))
    };
    opencode_sse_response(stream)
}

/// Filtering by `types` must never hide run lifecycle. A client that asked
/// for token deltas still needs `done`/errors to know when to stop.
fn is_lifecycle_event(event: &str) -> bool {
    matches!(
        event,
        "done" | "error" | "turn_started" | "turn_complete" | "turn_aborted" | "run.finished"
    )
}

fn live_opencode_stream(
    receiver: broadcast::Receiver<ServerEvent>,
    types: Option<Arc<HashSet<String>>>,
) -> SseStream {
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(SSE_SUBSCRIBER_CAPACITY);
    tokio::spawn(async move {
        let mut receiver = receiver;
        loop {
            let received = tokio::select! {
                _ = tx.closed() => break,
                received = receiver.recv() => received,
            };
            match received {
                Ok(event) => {
                    if let Some(types) = &types
                        && !types.contains(&event.event)
                        && !is_lifecycle_event(&event.event)
                    {
                        continue;
                    }
                    if tx
                        .try_send(Ok(opencode_frame(opencode_event_value(event))))
                        .is_err()
                    {
                        tracing::warn!(
                            capacity = SSE_SUBSCRIBER_CAPACITY,
                            "live SSE subscriber overflow; closing stream"
                        );
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    tracing::warn!(missed, "live SSE subscriber lagged; closing stream");
                    break;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    Box::pin(ReceiverStream::new(rx))
}

#[derive(Debug, Deserialize, Default)]
struct ChatLogQuery {
    #[serde(default)]
    after: Option<u64>,
    #[serde(default)]
    follow: Option<String>,
    #[serde(default)]
    types: Option<String>,
}

async fn chat_log_handler(
    State(state): State<ServerState>,
    Path(chat_id): Path<String>,
    Query(query): Query<ChatLogQuery>,
) -> Response {
    let values = spawn_log_values(
        state.harness_root.clone(),
        chat_id,
        query.after,
        query_flag(query.follow.as_deref()),
        parse_types(query.types.as_deref()),
    );
    let stream: SseStream =
        Box::pin(ReceiverStream::new(values).map(|value| Ok(opencode_frame(value))));
    opencode_sse_response(stream)
}

/// Returns the event values in order: replay, `log.synced`, then live appends
/// when `follow` is set. Keeping this as values makes the replay/live handoff
/// testable without going through HTTP.
fn spawn_log_values(
    harness_root: std::path::PathBuf,
    chat_id: String,
    after: Option<u64>,
    follow: bool,
    types: Option<Arc<HashSet<String>>>,
) -> mpsc::Receiver<Value> {
    let (tx, rx) = mpsc::channel::<Value>(64);
    let mut appends = stark_store::rollout::subscribe_appends();
    tokio::spawn(async move {
        let mut path = stark_store::rollout::find_rollout(&harness_root, &chat_id);
        let mut cursor = after;
        let lines = load_rollout_lines(path.as_ref()).await;
        let target = max_ordinal(&lines);
        if !send_log_values(&tx, &chat_id, &mut cursor, lines, &types).await {
            return;
        }
        if tx.send(log_synced_value(&chat_id, target)).await.is_err() {
            return;
        }
        if !follow {
            return;
        }
        loop {
            let received = tokio::select! {
                _ = tx.closed() => break,
                received = appends.recv() => received,
            };
            match received {
                Ok(append) if append.chat_id == chat_id => {
                    if path.is_none() {
                        path = stark_store::rollout::find_rollout(&harness_root, &chat_id);
                    }
                    let lines = load_rollout_lines(path.as_ref()).await;
                    if !send_log_values(&tx, &chat_id, &mut cursor, lines, &types).await {
                        return;
                    }
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    tracing::warn!(
                        missed,
                        chat_id = %chat_id,
                        "durable log follower lagged; resyncing"
                    );
                    if path.is_none() {
                        path = stark_store::rollout::find_rollout(&harness_root, &chat_id);
                    }
                    let lines = load_rollout_lines(path.as_ref()).await;
                    if !send_log_values(&tx, &chat_id, &mut cursor, lines, &types).await {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    rx
}

async fn load_rollout_lines(
    path: Option<&std::path::PathBuf>,
) -> Vec<stark_store::rollout::RolloutLine> {
    let Some(path) = path.cloned() else {
        return Vec::new();
    };
    tokio::task::spawn_blocking(move || stark_store::rollout::read_rollout(&path))
        .await
        .unwrap_or_default()
}

fn max_ordinal(lines: &[stark_store::rollout::RolloutLine]) -> Option<u64> {
    lines.iter().filter_map(|line| line.ordinal).max()
}

async fn send_log_values(
    tx: &mpsc::Sender<Value>,
    chat_id: &str,
    cursor: &mut Option<u64>,
    lines: Vec<stark_store::rollout::RolloutLine>,
    types: &Option<Arc<HashSet<String>>>,
) -> bool {
    for value in log_values(chat_id, cursor, lines, types) {
        if tx.send(value).await.is_err() {
            return false;
        }
    }
    true
}

fn log_values(
    chat_id: &str,
    cursor: &mut Option<u64>,
    lines: Vec<stark_store::rollout::RolloutLine>,
    types: &Option<Arc<HashSet<String>>>,
) -> Vec<Value> {
    let mut values = Vec::new();
    for line in lines {
        let Some(ordinal) = line.ordinal else {
            continue;
        };
        if cursor.is_some_and(|current| ordinal <= current) {
            continue;
        }
        let Some(value) = durable_event_value(chat_id, &line) else {
            continue;
        };
        let event_type = value.get("type").and_then(Value::as_str).unwrap_or("");
        *cursor = Some(cursor.map_or(ordinal, |current| current.max(ordinal)));
        if let Some(types) = types
            && !types.contains(event_type)
            && !is_lifecycle_event(event_type)
        {
            continue;
        }
        values.push(value);
    }
    values
}

fn durable_event_value(chat_id: &str, line: &stark_store::rollout::RolloutLine) -> Option<Value> {
    let ordinal = line.ordinal?;
    let (event_type, data) = rollout_event(line);
    let data = serde_json::from_str(&data).unwrap_or(Value::String(data));
    Some(json!({
        "id": format!("evt_{chat_id}_{ordinal}"),
        "type": event_type,
        "created": line_created(line),
        "durable": {
            "aggregateID": chat_id,
            "seq": ordinal,
            "version": 1,
        },
        "data": data,
    }))
}

fn line_created(line: &stark_store::rollout::RolloutLine) -> i64 {
    chrono::DateTime::parse_from_rfc3339(&line.timestamp)
        .map(|date| date.timestamp_millis())
        .unwrap_or_else(|_| Utc::now().timestamp_millis())
}

fn log_synced_value(chat_id: &str, seq: Option<u64>) -> Value {
    let mut value = json!({
        "type": "log.synced",
        "aggregateID": chat_id,
    });
    if let Some(seq) = seq {
        value["seq"] = json!(seq);
    }
    value
}

#[derive(Debug, Deserialize)]
struct ChatRequest {
    chat_id: String,
}

async fn abort_handler(
    State(state): State<ServerState>,
    Json(request): Json<ChatRequest>,
) -> Json<Value> {
    let top = state.runs.cancel(&request.chat_id);
    let child = stark_core::cancel_child_run(&request.chat_id);
    Json(json!({"ok": top || child}))
}

async fn health_handler() -> Json<Value> {
    Json(json!({
        "ok": true,
        "code_version": "stark-rs-0.1",
        // Set by stark-launcher for the bundled desktop server; empty on the
        // VPS, where `build` names the deployed commit instead.
        "app_version": std::env::var("STARK_APP_VERSION").unwrap_or_default(),
        "build": std::env::var("STARK_BUILD_SHA").unwrap_or_default(),
        "origin": server_origin(),
        "cap": concurrent_cap(),
    }))
}

/// Which machine this server is: "local" by default, "vps" when
/// `STARK_ORIGIN=vps` is set on the cloud box. Rune's cloud badge reads this;
/// no absolute paths ever leave the server.
fn server_origin() -> String {
    std::env::var("STARK_ORIGIN")
        .ok()
        .map(|value| value.trim().to_lowercase())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "local".to_string())
}

fn concurrent_cap() -> usize {
    std::env::var("STARK_MAX_CONCURRENT_RUNS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(10)
}

#[derive(Debug, Deserialize)]
struct StateQuery {
    chat_id: Option<String>,
}

async fn state_handler(
    State(state): State<ServerState>,
    Query(query): Query<StateQuery>,
) -> Json<Value> {
    let mut value = state
        .runs
        .state(query.chat_id.as_deref())
        .or_else(|| {
            query
                .chat_id
                .as_deref()
                .and_then(stark_core::child_run_state)
                .and_then(|state| serde_json::to_value(state).ok())
        })
        .unwrap_or_else(|| serde_json::to_value(AppState::default()).unwrap_or(Value::Null));
    hydrate_plan(&state.harness_root, &mut value, query.chat_id.as_deref()).await;
    Json(value)
}

async fn hydrate_plan(harness_root: &std::path::Path, value: &mut Value, chat_id: Option<&str>) {
    if let Some(chat_id) = chat_id.filter(|chat_id| !chat_id.is_empty())
        && let Ok(plan) = PlanStore::new(harness_root.to_path_buf())
            .plan_state(chat_id)
            .await
        && !plan.is_empty()
    {
        value["plan"] = serde_json::to_value(plan).unwrap_or(Value::Null);
    }
}

#[derive(Debug, Deserialize)]
struct HistoryQuery {
    limit: Option<usize>,
}

async fn history_handler(
    State(state): State<ServerState>,
    Query(query): Query<HistoryQuery>,
) -> Json<Value> {
    let prompts = state
        .store
        .recent_user_prompts(query.limit.unwrap_or(20))
        .await
        .unwrap_or_default();
    Json(json!({"prompts": prompts}))
}

#[derive(Debug, Deserialize)]
struct ActiveQuery {
    chat_id: Option<String>,
    workspace_path: Option<String>,
}

async fn active_handler(
    State(state): State<ServerState>,
    Query(query): Query<ActiveQuery>,
) -> Json<Value> {
    let _ = query.workspace_path;
    let mut active = state.runs.active(query.chat_id.as_deref());
    if !active["running"].as_bool().unwrap_or(false)
        && let Some(chat_id) = query.chat_id.as_deref()
        && let Some(started_at) = stark_core::child_run_started_at(chat_id)
    {
        active = json!({
            "running": true,
            "workspace_path": "",
            "chat_id": chat_id,
            "started_at": started_at,
        });
    }
    // Cloud badge data: every running chat (drives per-row dots + the
    // details drawer), plus the host origin and concurrency cap. The
    // legacy per-chat fields above stay byte-identical.
    if let Ok(mut map) = serde_json::from_value::<serde_json::Map<String, Value>>(active.clone()) {
        map.insert("runs".to_string(), json!(state.runs.list_active()));
        map.insert("cap".to_string(), json!(concurrent_cap()));
        map.insert("origin".to_string(), json!(server_origin()));
        return Json(Value::Object(map));
    }
    Json(active)
}

/// Cloud handoff status: which machine this is, whether a VPS target is
/// configured (host only, never the token), the concurrency cap, and the
/// currently running chats. Powers the Cloud runs view and the per-chat
/// cloud badge.
async fn cloud_status_handler(State(state): State<ServerState>) -> Json<Value> {
    Json(json!({
        "origin": server_origin(),
        "cap": concurrent_cap(),
        "configured": cloud::cloud_configured(),
        "cloud_host": cloud::cloud_host(),
        "runs": state.runs.list_active(),
    }))
}

async fn cloud_handoffs_handler(State(state): State<ServerState>) -> Json<Value> {
    Json(json!({ "handoffs": cloud::load_handoffs(&state.harness_root) }))
}

async fn cloud_handoff_handler(
    State(state): State<ServerState>,
    Path(handoff_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let handoff = cloud::load_handoffs(&state.harness_root)
        .into_iter()
        .find(|item| item.id == handoff_id)
        .ok_or_else(|| not_found(format!("unknown handoff '{handoff_id}'")))?;
    Ok(Json(json!({ "handoff": handoff })))
}

#[derive(Debug, Deserialize)]
struct CloudSendRequest {
    #[serde(default)]
    chat_id: Option<String>,
    #[serde(default)]
    workspace_id: String,
    #[serde(default)]
    task: Option<String>,
    #[serde(default)]
    branch: Option<String>,
}

async fn cloud_send_handler(
    State(state): State<ServerState>,
    Json(request): Json<CloudSendRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if request.workspace_id.trim().is_empty() {
        return Err(bad_request("workspace_id is required".to_string()));
    }
    match cloud::build_handoff(
        &state.harness_root,
        &state.store,
        cloud::SendRequest {
            chat_id: request
                .chat_id
                .map(|id| id.trim().to_string())
                .filter(|id| !id.is_empty()),
            workspace_id: request.workspace_id.trim().to_string(),
            task: request
                .task
                .map(|task| task.trim().to_string())
                .filter(|task| !task.is_empty()),
            branch: request
                .branch
                .map(|branch| branch.trim().to_string())
                .filter(|branch| !branch.is_empty()),
        },
    )
    .await
    {
        Ok(handoff) => Ok(Json(json!({ "handoff": handoff }))),
        Err(error) => Err(bad_request(error)),
    }
}

async fn cloud_receive_handler(
    State(state): State<ServerState>,
    body: axum::body::Bytes,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    match cloud::receive_package(&state.harness_root, &state.store, &body).await {
        Ok(handoff) => Ok(Json(json!({ "handoff": handoff }))),
        Err(error) => Err(bad_request(error)),
    }
}

async fn agents_handler(State(state): State<ServerState>) -> Json<Value> {
    let Ok(store) = AgentStore::open(&state.harness_root).await else {
        return Json(json!({"agents": []}));
    };
    let records = store.list(None).await.unwrap_or_default();
    let mut agents = Vec::with_capacity(records.len());
    for record in records {
        let mut value = record.to_json();
        value["pendingMessages"] =
            json!(store.pending_messages_count(&record.id).await.unwrap_or(0));
        agents.push(value);
    }
    Json(json!({"agents": agents}))
}

async fn agent_handler(
    State(state): State<ServerState>,
    Path(agent_id): Path<String>,
) -> Json<Value> {
    let Ok(store) = AgentStore::open(&state.harness_root).await else {
        return Json(json!({"error": "agent store unavailable"}));
    };
    match store.get(&agent_id).await {
        Ok(Some(agent)) => {
            let pending = store.messages(&agent_id).await.unwrap_or_default();
            let history = store.messages_all(&agent_id).await.unwrap_or_default();
            let children = store
                .list(Some(&agent.id))
                .await
                .unwrap_or_default()
                .iter()
                .map(AgentRecord::to_json)
                .collect::<Vec<_>>();
            let mut value = agent.to_json();
            value["pendingMessages"] =
                json!(store.pending_messages_count(&agent.id).await.unwrap_or(0));
            Json(json!({
                "agent": value,
                "messages": pending,
                "history": history,
                "children": children,
            }))
        }
        Ok(None) => Json(json!({"error": "agent not found"})),
        Err(error) => Json(json!({"error": error.to_string()})),
    }
}

async fn agent_cancel_handler(
    State(state): State<ServerState>,
    Path(agent_id): Path<String>,
) -> Json<Value> {
    let Ok(store) = AgentStore::open(&state.harness_root).await else {
        return Json(json!({"error": "agent store unavailable"}));
    };
    let Ok(Some(agent)) = store.get(&agent_id).await else {
        return Json(json!({"error": "agent not found"}));
    };
    let mut cancelled = 0usize;
    let mut stack = vec![agent];
    while let Some(item) = stack.pop() {
        stark_core::cancel_agent_run(&item.id);
        let _ = store.set_status(&item.id, "cancelled", false).await;
        cancelled += 1;
        if let Ok(children) = store.list(Some(&item.id)).await {
            stack.extend(children);
        }
    }
    Json(json!({"ok": true, "cancelled": cancelled}))
}

#[derive(Debug, Deserialize, Default)]
struct AgentMessageRequest {
    #[serde(default)]
    message: String,
    #[serde(default)]
    delivery: String,
}

async fn agent_message_handler(
    State(state): State<ServerState>,
    Path(agent_id): Path<String>,
    Json(request): Json<AgentMessageRequest>,
) -> Json<Value> {
    if request.message.trim().is_empty() {
        return Json(json!({"error": "message is required"}));
    }
    let Ok(store) = AgentStore::open(&state.harness_root).await else {
        return Json(json!({"error": "agent store unavailable"}));
    };
    let Ok(Some(agent)) = store.get(&agent_id).await else {
        return Json(json!({"error": "agent not found"}));
    };
    let body = request.message.trim().to_string();
    // Running agents get steered immediately; idle agents get a trigger.
    if state.runs.is_running(&agent.chat_id) {
        let id = store
            .enqueue(
                Some(&agent.id),
                &agent.id,
                "message",
                &body,
                "queue",
                json!({"from": "ui"}),
            )
            .await
            .unwrap_or_default();
        let steered = state.runs.steer(&agent.chat_id, &body);
        if steered && !id.is_empty() {
            let _ = store.consume_message(&id).await;
        }
        return Json(json!({"ok": true, "steered": steered, "status": agent.status}));
    }
    let id = match store
        .enqueue(
            Some(&agent.id),
            &agent.id,
            "message",
            &body,
            if request.delivery == "queue" {
                "queue"
            } else {
                "trigger"
            },
            json!({"from": "ui"}),
        )
        .await
    {
        Ok(id) => id,
        Err(error) => return Json(json!({"error": error.to_string()})),
    };
    let _ = store.wake(&agent.id).await;
    Json(json!({"ok": true, "steered": false, "messageId": id}))
}

async fn agent_diff_handler(
    State(state): State<ServerState>,
    Path(agent_id): Path<String>,
) -> Json<Value> {
    let Ok(store) = AgentStore::open(&state.harness_root).await else {
        return Json(json!({"error": "agent store unavailable"}));
    };
    let Ok(Some(agent)) = store.get(&agent_id).await else {
        return Json(json!({"error": "agent not found"}));
    };
    if agent.worktree_path.trim().is_empty() || agent.worktree_branch.trim().is_empty() {
        return Json(json!({
            "agent": agent.to_json(),
            "diff": Value::Null,
            "note": "agent has no worktree",
        }));
    }
    let path = std::path::PathBuf::from(&agent.worktree_path);
    let diff =
        stark_core::worktree::diff(&path, &agent.worktree_base).unwrap_or_else(|_| json!({}));
    let _ = store
        .set_worktree_state(&agent.id, &agent.worktree_state, &diff, "")
        .await;
    Json(json!({"agent": agent.to_json(), "diff": diff}))
}

async fn agent_merge_handler(
    State(state): State<ServerState>,
    Path(agent_id): Path<String>,
) -> Json<Value> {
    let Ok(store) = AgentStore::open(&state.harness_root).await else {
        return Json(json!({"error": "agent store unavailable"}));
    };
    let Ok(Some(agent)) = store.get(&agent_id).await else {
        return Json(json!({"error": "agent not found"}));
    };
    if matches!(agent.status.as_str(), "running" | "queued") {
        return Json(json!({"error": "agent is still running; cancel or wait first"}));
    }
    if agent.worktree_path.trim().is_empty() {
        return Json(json!({"error": "agent has no worktree to merge"}));
    }
    let info = stark_core::worktree::WorktreeInfo {
        path: std::path::PathBuf::from(&agent.worktree_path),
        branch: agent.worktree_branch.clone(),
        base: agent.worktree_base.clone(),
        repo_root: std::path::PathBuf::from(&agent.repo_root),
        subdir: std::path::PathBuf::from(&agent.workspace_subdir),
    };
    match stark_core::worktree::merge(&info) {
        Ok(outcome) => {
            let _ = store
                .set_worktree_state(&agent.id, "merged", &json!({}), "")
                .await;
            Json(json!({"ok": true, "result": outcome}))
        }
        Err(error) => {
            let _ = store
                .set_worktree_state(&agent.id, "active", &json!({}), &error)
                .await;
            Json(json!({"error": error}))
        }
    }
}

async fn agent_discard_handler(
    State(state): State<ServerState>,
    Path(agent_id): Path<String>,
) -> Json<Value> {
    let Ok(store) = AgentStore::open(&state.harness_root).await else {
        return Json(json!({"error": "agent store unavailable"}));
    };
    let Ok(Some(agent)) = store.get(&agent_id).await else {
        return Json(json!({"error": "agent not found"}));
    };
    stark_core::cancel_agent_run(&agent.id);
    let _ = store.set_status(&agent.id, "cancelled", false).await;
    if agent.worktree_path.trim().is_empty() {
        return Json(json!({"ok": true, "discarded": false}));
    }
    let repo = if agent.repo_root.trim().is_empty() {
        std::path::PathBuf::from(&agent.worktree_path)
    } else {
        std::path::PathBuf::from(&agent.repo_root)
    };
    match stark_core::worktree::discard(
        &repo,
        &std::path::PathBuf::from(&agent.worktree_path),
        &agent.worktree_branch,
    ) {
        Ok(outcome) => {
            let _ = store
                .set_worktree_state(&agent.id, "discarded", &json!({}), "")
                .await;
            Json(json!({"ok": true, "result": outcome}))
        }
        Err(error) => Json(json!({"error": error})),
    }
}

#[derive(Debug, Deserialize)]
struct MemoryQuery {
    records: Option<usize>,
}

#[derive(Debug, Deserialize, Default)]
struct MemoryConsolidateRequest {
    #[serde(default)]
    mode: String,
}

#[derive(Debug, Deserialize)]
struct MemorySummaryRequest {
    summary: String,
}

#[derive(Debug, Deserialize)]
struct MemoryNoteRequest {
    text: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    topic: String,
    #[serde(default = "default_pin")]
    pinned: bool,
}

fn default_pin() -> bool {
    true
}

#[derive(Debug, Deserialize, Default)]
struct MemoryPinRequest {
    #[serde(default = "default_pin")]
    pinned: bool,
}

async fn memory_list_handler(State(state): State<ServerState>) -> Json<Value> {
    let Ok(store) = MemoryStore::open(&state.harness_root).await else {
        return Json(json!({"error": "memory store unavailable"}));
    };
    let mut workspaces = Vec::new();
    for (id, name, _, kind) in VIRTUAL_WORKSPACES {
        let status = store
            .status(id)
            .await
            .unwrap_or_else(|error| json!({"error": error.to_string()}));
        workspaces.push(json!({
            "id": id,
            "name": name,
            "kind": kind,
            "status": status,
        }));
    }
    for record in state.registry.list() {
        let status = store
            .status(&record.id)
            .await
            .unwrap_or_else(|error| json!({"error": error.to_string()}));
        workspaces.push(json!({
            "id": record.id,
            "name": record.name,
            "kind": "project",
            "path": record.path,
            "status": status,
        }));
    }
    Json(json!({
        "enabled": stark_store::memory_enabled(),
        "maxChars": stark_store::memory_max_chars(),
        "workspaces": workspaces,
    }))
}

async fn memory_status_handler(
    State(state): State<ServerState>,
    Path(workspace_id): Path<String>,
    Query(query): Query<MemoryQuery>,
) -> Json<Value> {
    let Ok(store) = MemoryStore::open(&state.harness_root).await else {
        return Json(json!({"error": "memory store unavailable"}));
    };
    let status = store
        .status(&workspace_id)
        .await
        .unwrap_or_else(|error| json!({"error": error.to_string()}));
    let summary = store
        .summary(&workspace_id, stark_store::memory_max_chars())
        .await;
    let index = store
        .index(&workspace_id, stark_store::memory_max_chars())
        .await;
    let topics = store.topics(&workspace_id).await.unwrap_or_default();
    let entries = store
        .entries(&workspace_id, None)
        .await
        .unwrap_or_default()
        .iter()
        .map(stark_store::MemoryEntry::to_json)
        .collect::<Vec<_>>();
    let records = store
        .records(&workspace_id, query.records.unwrap_or(20).clamp(1, 200))
        .await
        .unwrap_or_default();
    Json(json!({
        "status": status,
        "index": index,
        "summary": summary,
        "topics": topics,
        "entries": entries,
        "records": records,
    }))
}

async fn memory_save_handler(
    State(state): State<ServerState>,
    Path(workspace_id): Path<String>,
    Json(request): Json<MemorySummaryRequest>,
) -> Json<Value> {
    let Ok(store) = MemoryStore::open(&state.harness_root).await else {
        return Json(json!({"error": "memory store unavailable"}));
    };
    match store.save_summary(&workspace_id, &request.summary).await {
        Ok(()) => Json(json!({"ok": true})),
        Err(error) => Json(json!({"error": error.to_string()})),
    }
}

async fn memory_consolidate_handler(
    State(state): State<ServerState>,
    Path(workspace_id): Path<String>,
    request: Option<Json<MemoryConsolidateRequest>>,
) -> Json<Value> {
    let Ok(store) = MemoryStore::open(&state.harness_root).await else {
        return Json(json!({"error": "memory store unavailable"}));
    };
    let mode = request
        .and_then(|Json(request)| {
            let mode = request.mode.trim().to_ascii_lowercase();
            (!mode.is_empty()).then_some(mode)
        })
        .unwrap_or_else(|| "model".to_string());
    let mode = if matches!(mode.as_str(), "deterministic" | "rebuild" | "local") {
        stark_core::memory_agent::ConsolidateMode::Deterministic
    } else {
        stark_core::memory_agent::ConsolidateMode::Model
    };
    let cancel = CancellationToken::new();
    let memory_effective = effective_for(&state, "__memory__", "workspace");
    let memory_provider = state.provider_for(&memory_effective);
    match stark_core::memory_agent::consolidate_workspace(
        &memory_provider,
        &cancel,
        &store,
        &workspace_id,
        mode,
    )
    .await
    {
        Ok(outcome) => Json(json!({
            "ok": true,
            "summary": outcome.summary,
            "entries": outcome.entries,
            "source": outcome.source,
            "model": outcome.model,
            "fallback": outcome.fallback,
        })),
        Err(error) => Json(json!({"error": error})),
    }
}

async fn memory_note_handler(
    State(state): State<ServerState>,
    Path(workspace_id): Path<String>,
    Json(request): Json<MemoryNoteRequest>,
) -> Json<Value> {
    let Ok(store) = MemoryStore::open(&state.harness_root).await else {
        return Json(json!({"error": "memory store unavailable"}));
    };
    let text = request.text.trim();
    if text.is_empty() {
        return Json(json!({"error": "text is required"}));
    }
    match store
        .add_entry(
            &workspace_id,
            NewEntry {
                chat_id: String::new(),
                agent_id: None,
                kind: if request.kind.trim().is_empty() {
                    "note".to_string()
                } else {
                    request.kind
                },
                topic: if request.topic.trim().is_empty() {
                    "notes".to_string()
                } else {
                    request.topic
                },
                text: text.to_string(),
                source: "manual".to_string(),
                model: String::new(),
                pinned: request.pinned,
            },
        )
        .await
    {
        Ok(id) => Json(json!({"ok": true, "id": id})),
        Err(error) => Json(json!({"error": error.to_string()})),
    }
}

async fn memory_forget_handler(
    State(state): State<ServerState>,
    Path((workspace_id, entry_id)): Path<(String, String)>,
) -> Json<Value> {
    let Ok(store) = MemoryStore::open(&state.harness_root).await else {
        return Json(json!({"error": "memory store unavailable"}));
    };
    match store.forget_entry(&workspace_id, &entry_id).await {
        Ok(true) => Json(json!({"ok": true})),
        Ok(false) => Json(json!({"error": "memory entry not found"})),
        Err(error) => Json(json!({"error": error.to_string()})),
    }
}

async fn memory_pin_handler(
    State(state): State<ServerState>,
    Path((workspace_id, entry_id)): Path<(String, String)>,
    request: Option<Json<MemoryPinRequest>>,
) -> Json<Value> {
    let Ok(store) = MemoryStore::open(&state.harness_root).await else {
        return Json(json!({"error": "memory store unavailable"}));
    };
    let pinned = request.map(|Json(request)| request.pinned).unwrap_or(true);
    match store.pin_entry(&workspace_id, &entry_id, pinned).await {
        Ok(true) => Json(json!({"ok": true})),
        Ok(false) => Json(json!({"error": "memory entry not found"})),
        Err(error) => Json(json!({"error": error.to_string()})),
    }
}

async fn memory_topic_handler(
    State(state): State<ServerState>,
    Path((workspace_id, topic)): Path<(String, String)>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let Ok(store) = MemoryStore::open(&state.harness_root).await else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "memory store unavailable"})),
        ));
    };
    let _ = store.mark_topic_used(&workspace_id, &topic).await;
    match store.read_topic(&workspace_id, &topic).await {
        Ok(Some(text)) => Ok(Json(json!({"topic": topic, "content": text}))),
        Ok(None) => Err(not_found("memory topic not found")),
        Err(error) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": error.to_string()})),
        )),
    }
}

async fn memory_clear_handler(
    State(state): State<ServerState>,
    Path(workspace_id): Path<String>,
) -> Json<Value> {
    let Ok(store) = MemoryStore::open(&state.harness_root).await else {
        return Json(json!({"error": "memory store unavailable"}));
    };
    match store.clear(&workspace_id).await {
        Ok(()) => Json(json!({"ok": true})),
        Err(error) => Json(json!({"error": error.to_string()})),
    }
}

async fn pending_handler(State(state): State<ServerState>) -> Json<Value> {
    Json(json!({"batches": state.questions.pending_batches()}))
}

async fn mcp_servers_handler() -> Json<Value> {
    let servers = stark_tools::McpManager::global()
        .map(|manager| manager.status_snapshot())
        .unwrap_or_default();
    Json(json!({"servers": servers}))
}

async fn mcp_upsert_handler(Path(server_name): Path<String>, Json(body): Json<Value>) -> Response {
    let config = match serde_json::from_value::<stark_tools::McpServerConfig>(body) {
        Ok(config) => config,
        Err(error) => return mcp_error(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    let Some(manager) = stark_tools::McpManager::global() else {
        return mcp_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "MCP manager is not initialized",
        );
    };
    match manager.upsert_server(&server_name, config).await {
        Ok(servers) => Json(json!({"servers": servers})).into_response(),
        Err(error) => mcp_error(StatusCode::BAD_REQUEST, &error),
    }
}

async fn mcp_delete_handler(Path(server_name): Path<String>) -> Response {
    let Some(manager) = stark_tools::McpManager::global() else {
        return mcp_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "MCP manager is not initialized",
        );
    };
    match manager.remove_server(&server_name).await {
        Ok(servers) => Json(json!({"servers": servers})).into_response(),
        Err(error) => mcp_error(StatusCode::BAD_REQUEST, &error),
    }
}

async fn mcp_restart_handler(Path(server_name): Path<String>) -> Response {
    let Some(manager) = stark_tools::McpManager::global() else {
        return mcp_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "MCP manager is not initialized",
        );
    };
    match manager.restart_server(&server_name).await {
        Ok(servers) => Json(json!({"servers": servers})).into_response(),
        Err(error) => mcp_error(StatusCode::NOT_FOUND, &error),
    }
}

fn mcp_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error": message}))).into_response()
}

#[derive(Debug, Deserialize)]
struct AnswerRequest {
    batch_id: String,
    #[serde(default)]
    answers: Vec<Value>,
}

async fn answer_handler(
    State(state): State<ServerState>,
    Json(request): Json<AnswerRequest>,
) -> impl IntoResponse {
    if state.questions.answer(&request.batch_id, request.answers) {
        (StatusCode::OK, Json(json!({"ok": true}))).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "unknown or expired batch_id"})),
        )
            .into_response()
    }
}

#[derive(Debug, Deserialize)]
struct ProviderQuery {
    #[serde(default)]
    agent: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProviderRequest {
    provider: String,
}

#[derive(Debug, Deserialize)]
struct AgentQuery {
    #[serde(default)]
    tier: Option<String>,
}

async fn models_handler(State(state): State<ServerState>) -> Json<Value> {
    let active = state.config.active_provider();
    let summaries = state.config.provider_summaries();
    let models: Vec<Value> = summaries
        .iter()
        .find(|provider| provider.id == active)
        .map(|provider| {
            provider
                .models
                .iter()
                .map(|model| {
                    json!({
                        "id": model.id,
                        "name": model.label,
                        "key": model.key,
                        "provider": active,
                        "reasoning": model.reasoning,
                        "context": model.context,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Json(json!({ "models": models, "provider": active }))
}

async fn provider_handler(
    State(state): State<ServerState>,
    Query(query): Query<ProviderQuery>,
) -> Json<Value> {
    let active = state.config.active_provider();
    let summaries = state.config.provider_summaries();
    let active_provider = summaries.iter().find(|provider| provider.id == active);
    let effective = query
        .agent
        .as_deref()
        .map(|agent| effective_for(&state, agent, "workspace"));
    let legacy_env: Vec<&str> = legacy_env_names();
    Json(json!({
        "provider": active,
        "provider_label": active_provider.map(|provider| provider.label.clone()).unwrap_or_default(),
        "base_url": active_provider.map(|provider| provider.base_url.clone()).unwrap_or_default(),
        "model_flash": active_provider
            .and_then(|provider| provider.models.first())
            .map(|model| model.id.clone())
            .unwrap_or_default(),
        "model_pro": active_provider
            .and_then(|provider| provider.models.first())
            .map(|model| model.id.clone())
            .unwrap_or_default(),
        "has_key": active_provider.is_some_and(|provider| provider.has_key),
        "models": active_provider.map(|provider| provider.models.clone()).unwrap_or_default(),
        "providers": summaries,
        "issues": state.config.issues(),
        "config_file": state.config.has_file(),
        "legacy_env": legacy_env,
        "health": state.provider_health.lock().map(|health| health.clone()).unwrap_or_default(),
        "effective": effective,
    }))
}

async fn set_provider_handler(
    State(state): State<ServerState>,
    Json(request): Json<ProviderRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    state
        .config
        .set_active(request.provider.trim())
        .map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": error.to_string()})),
            )
        })?;
    Ok(Json(json!({
        "ok": true,
        "provider": state.config.active_provider(),
        "message": "Provider saved. The next run uses it; no restart needed.",
    })))
}

#[derive(Debug, Deserialize)]
struct ProviderSelector {
    #[serde(default)]
    provider: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProviderKeyAddRequest {
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    label: String,
    secret: String,
}

#[derive(Debug, Deserialize)]
struct ProviderKeyActiveRequest {
    #[serde(default)]
    provider: Option<String>,
    id: String,
}

#[derive(Debug, Deserialize)]
struct ProviderUsageQuery {
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    key_id: Option<String>,
}

fn require_local(address: &SocketAddr) -> Result<(), (StatusCode, Json<Value>)> {
    if address.ip().is_loopback() {
        Ok(())
    } else {
        Err((StatusCode::FORBIDDEN, Json(json!({"error": "Local only"}))))
    }
}

fn canonical_provider(
    state: &ServerState,
    requested: Option<&str>,
) -> Result<String, (StatusCode, Json<Value>)> {
    let active = state.config.active_provider();
    let wanted = requested.map(str::trim).filter(|id| !id.is_empty());
    let target = wanted.unwrap_or(active.as_str());
    state
        .config
        .provider_summaries()
        .into_iter()
        .find(|provider| provider.id.eq_ignore_ascii_case(target))
        .map(|provider| provider.id)
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("unknown provider '{target}'")})),
            )
        })
}

fn provider_keys_payload(
    state: &ServerState,
    provider: &str,
) -> Result<Value, (StatusCode, Json<Value>)> {
    let report = state.config.provider_keys(provider).map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": error.to_string()})),
        )
    })?;
    let summaries = state.config.provider_summaries();
    let summary = summaries
        .iter()
        .find(|summary| summary.id == report.provider);
    let requested = summary
        .map(|summary| {
            if summary.default_model.trim().is_empty() {
                summary
                    .models
                    .first()
                    .map(|model| model.key.clone())
                    .unwrap_or_default()
            } else {
                summary.default_model.clone()
            }
        })
        .unwrap_or_default();
    let model = state
        .config
        .resolve_model(&report.provider, &requested)
        .map(|(_, model)| model)
        .unwrap_or(requested);
    Ok(json!({
        "provider": report.provider,
        "label": summary.map(|summary| summary.label.clone()).unwrap_or_default(),
        "base_url": summary.map(|summary| summary.base_url.clone()).unwrap_or_default(),
        "model": model,
        "has_key": !state.config.resolve_key(&report.provider).is_empty(),
        "active": report.active,
        "has_env": report.has_env,
        "keys": report.keys,
        "issues": state.config.issues(),
    }))
}

fn clear_provider_health(state: &ServerState, provider: &str) {
    if let Ok(mut health) = state.provider_health.lock() {
        health.remove(provider);
    }
}

async fn verify_provider_key(base_url: &str, api_key: &str) -> Result<(), (StatusCode, String)> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .user_agent("stark-rs/0.1")
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| {
            (
                StatusCode::BAD_GATEWAY,
                format!("Could not create the key check client: {error}"),
            )
        })?;
    let response = client
        .get(url)
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|error| {
            (
                StatusCode::BAD_GATEWAY,
                format!("Could not reach the provider to verify the key: {error}"),
            )
        })?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let code = status.as_u16();
    let message = match code {
        401 => "The provider rejected this key (401).".to_string(),
        403 => "This key is not authorized for the provider (403).".to_string(),
        429 => "The provider is rate-limiting key checks; try again shortly (429).".to_string(),
        _ => format!("The provider rejected the key check (HTTP {code})."),
    };
    let code = if code == 429 {
        StatusCode::TOO_MANY_REQUESTS
    } else {
        StatusCode::BAD_REQUEST
    };
    Err((code, message))
}

fn usage_window(value: Option<&Value>) -> Value {
    let Some(value) = value.filter(|value| value.is_object()) else {
        return Value::Null;
    };
    let Some(percent) = value.get("percent").and_then(Value::as_f64) else {
        return Value::Null;
    };
    json!({
        "status": value.get("status").and_then(Value::as_str).unwrap_or(""),
        "percent": percent.clamp(0.0, 100.0),
        "resets_at": value.get("resetsAt").and_then(Value::as_str).unwrap_or(""),
    })
}

fn parse_opencode_usage(value: &Value) -> Value {
    let Some(usage) = value.get("usage").filter(|usage| usage.is_object()) else {
        return Value::Null;
    };
    let rolling = usage_window(usage.get("rolling"));
    let weekly = usage_window(usage.get("weekly"));
    let monthly = usage_window(usage.get("monthly"));
    if rolling.is_null() && weekly.is_null() && monthly.is_null() {
        Value::Null
    } else {
        json!({
            "rolling": rolling,
            "weekly": weekly,
            "monthly": monthly,
        })
    }
}

async fn fetch_opencode_usage(base_url: &str, api_key: &str) -> Result<Value, String> {
    let url = format!("{}/usage", base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .user_agent("stark-rs/0.1")
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| format!("Could not create the usage client: {error}"))?;
    let response = client
        .get(url)
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|error| format!("Could not reach OpenCode usage: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(match status.as_u16() {
            401 => "OpenCode rejected this key (401).".to_string(),
            403 => "This key has no OpenCode Go subscription (403).".to_string(),
            429 => "OpenCode is rate-limiting usage requests; try again shortly (429).".to_string(),
            code => format!("OpenCode usage returned HTTP {code}."),
        });
    }
    let value: Value = response
        .json()
        .await
        .map_err(|error| format!("Could not parse OpenCode usage: {error}"))?;
    let usage = parse_opencode_usage(&value);
    if usage.is_null() {
        Err("OpenCode returned no usage meters for this key.".to_string())
    } else {
        Ok(usage)
    }
}

async fn provider_keys_handler(
    State(state): State<ServerState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Query(query): Query<ProviderSelector>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_local(&address)?;
    let provider = canonical_provider(&state, query.provider.as_deref())?;
    Ok(Json(provider_keys_payload(&state, &provider)?))
}

async fn add_provider_key_handler(
    State(state): State<ServerState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Json(request): Json<ProviderKeyAddRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_local(&address)?;
    let provider = canonical_provider(&state, request.provider.as_deref())?;
    let secret = request
        .secret
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim();
    if secret.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Enter an API key first."})),
        ));
    }
    if secret.len() > 4_096 || secret.chars().any(char::is_whitespace) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "That API key is not valid."})),
        ));
    }
    let summary = state
        .config
        .provider_summaries()
        .into_iter()
        .find(|summary| summary.id == provider)
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("unknown provider '{provider}'")})),
            )
        })?;
    verify_provider_key(&summary.base_url, secret)
        .await
        .map_err(|(status, error)| (status, Json(json!({"error": error}))))?;
    state
        .config
        .add_provider_key(&provider, &request.label, secret)
        .map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": error.to_string()})),
            )
        })?;
    clear_provider_health(&state, &provider);
    let mut payload = provider_keys_payload(&state, &provider)?;
    if let Some(object) = payload.as_object_mut() {
        object.insert(
            "message".to_string(),
            json!("Key saved and active. The next run uses it."),
        );
    }
    Ok(Json(payload))
}

async fn activate_provider_key_handler(
    State(state): State<ServerState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Json(request): Json<ProviderKeyActiveRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_local(&address)?;
    let provider = canonical_provider(&state, request.provider.as_deref())?;
    state
        .config
        .activate_provider_key(&provider, &request.id)
        .map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": error.to_string()})),
            )
        })?;
    clear_provider_health(&state, &provider);
    let mut payload = provider_keys_payload(&state, &provider)?;
    if let Some(object) = payload.as_object_mut() {
        object.insert(
            "message".to_string(),
            json!("Active key changed. The next run uses it."),
        );
    }
    Ok(Json(payload))
}

async fn remove_provider_key_handler(
    State(state): State<ServerState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    axum::extract::Path(key_id): axum::extract::Path<String>,
    Query(query): Query<ProviderSelector>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_local(&address)?;
    let provider = canonical_provider(&state, query.provider.as_deref())?;
    state
        .config
        .remove_provider_key(&provider, &key_id)
        .map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": error.to_string()})),
            )
        })?;
    clear_provider_health(&state, &provider);
    let mut payload = provider_keys_payload(&state, &provider)?;
    if let Some(object) = payload.as_object_mut() {
        object.insert("message".to_string(), json!("API key removed."));
    }
    Ok(Json(payload))
}

async fn provider_usage_handler(
    State(state): State<ServerState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Query(query): Query<ProviderUsageQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_local(&address)?;
    let provider = canonical_provider(&state, query.provider.as_deref())?;
    let report = state.config.provider_keys(&provider).map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": error.to_string()})),
        )
    })?;
    let key_id = query
        .key_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .unwrap_or(report.active.as_str())
        .to_string();
    let base = json!({
        "provider": provider,
        "key_id": key_id,
        "checked_ms": now_millis(),
    });
    let respond = |usage: Value, error: Option<String>| {
        let mut value = base.clone();
        if let Some(object) = value.as_object_mut() {
            object.insert("usage".to_string(), usage);
            object.insert(
                "error".to_string(),
                error.map(Value::String).unwrap_or(Value::Null),
            );
        }
        Json(value)
    };
    if key_id.is_empty() {
        return Ok(respond(
            Value::Null,
            Some("No API key is configured.".to_string()),
        ));
    }
    let api_key = state.config.resolve_key_entry(&provider, &key_id);
    if api_key.is_empty() {
        return Ok(respond(
            Value::Null,
            Some("That API key is no longer saved.".to_string()),
        ));
    }
    if !provider.eq_ignore_ascii_case("opencode") {
        return Ok(respond(
            Value::Null,
            Some("Usage reporting is only available for OpenCode Go.".to_string()),
        ));
    }
    let base_url = state
        .config
        .provider_summaries()
        .into_iter()
        .find(|summary| summary.id == provider)
        .map(|summary| summary.base_url)
        .unwrap_or_default();
    match fetch_opencode_usage(&base_url, &api_key).await {
        Ok(usage) => Ok(respond(usage, None)),
        Err(error) => Ok(respond(Value::Null, Some(error))),
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn agent_issues(registry: &AgentRegistry, key: &str) -> Vec<String> {
    let prefix = format!("{key}:");
    registry
        .issues()
        .iter()
        .filter(|issue| issue.starts_with(&prefix))
        .cloned()
        .collect()
}

fn builtin_spec(key: &str) -> Option<stark_core::AgentSpec> {
    stark_core::workspace_agents()
        .iter()
        .find(|spec| spec.key == key)
        .copied()
}

fn builtin_override(state: &ServerState, spec: stark_core::AgentSpec) -> Option<AgentFile> {
    state.config.agent(spec.key).or_else(|| {
        spec.aliases
            .iter()
            .find_map(|alias| state.config.agent(alias))
    })
}

async fn agents_config_handler(State(state): State<ServerState>) -> Json<Value> {
    let registry = agent_registry(&state);
    let mut agents: Vec<(i32, String, Value)> = Vec::new();
    for spec in stark_core::workspace_agents() {
        let effective = registry
            .get(spec.key)
            .cloned()
            .unwrap_or_else(|| (*effective_for(&state, spec.key, spec.tier)).clone());
        let issues = agent_issues(&registry, spec.key);
        let configured = builtin_override(&state, *spec);
        agents.push((
            effective.ui_order,
            spec.key.to_string(),
            json!({
                "key": spec.key,
                "display": effective.display,
                "description": effective.description,
                "tier": effective.tier,
                "builtin": true,
                "editable": true,
                "deletable": false,
                "resettable": configured.is_some(),
                "configured": configured.is_some(),
                "valid": issues.is_empty(),
                "errors": issues,
                "override": configured,
                "effective": effective,
            }),
        ));
    }
    for (key, file) in state.config.agent_files() {
        // Built-in keys are listed above. Legacy alias files (coder.json) are
        // migrated into their canonical built-in, so they must not appear as
        // a second agent here.
        if stark_core::agent_for_scope(&key).is_some() {
            continue;
        }
        let effective = registry.get(&key).cloned();
        let issues = agent_issues(&registry, &key);
        let order = effective
            .as_ref()
            .map(|agent| agent.ui_order)
            .or(file.ui.as_ref().and_then(|ui| ui.order))
            .unwrap_or(1000);
        agents.push((
            order,
            key.clone(),
            json!({
                "key": key,
                "display": effective.as_ref().map(|agent| agent.display.clone()).unwrap_or_else(|| file.display.clone()),
                "description": effective.as_ref().map(|agent| agent.description.clone()).unwrap_or_else(|| file.description.clone()),
                "tier": effective.as_ref().map(|agent| agent.tier.clone()).unwrap_or_else(|| file.tier.clone()),
                "builtin": false,
                "editable": true,
                "deletable": true,
                "resettable": false,
                "configured": true,
                "valid": effective.is_some() && issues.is_empty(),
                "errors": issues,
                "override": file,
                "effective": effective,
            }),
        ));
    }
    agents.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    Json(json!({
        "agents": agents.into_iter().map(|(_, _, value)| value).collect::<Vec<_>>(),
        "issues": registry.issues(),
        "config_issues": state.config.issues(),
        "active_provider": state.config.active_provider(),
        "providers": state.config.provider_summaries(),
        "config_file": state.config.has_file(),
        "legacy_env": legacy_env_names(),
        "health": state.provider_health.lock().map(|health| health.clone()).unwrap_or_default(),
        "capabilities": {
            "agent_builder": true,
            "custom_agents": true,
            "custom_code_mode": false,
            "schema_version": 2,
        },
    }))
}

async fn agent_config_handler(
    State(state): State<ServerState>,
    Path(key): Path<String>,
    Query(query): Query<AgentQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let registry = agent_registry(&state);
    let requested = key.trim().to_ascii_lowercase();
    let canonical = registry
        .canonical_key(&requested)
        .unwrap_or_else(|| requested.clone());
    if registry.get(&canonical).is_none() && state.config.agent(&canonical).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("unknown agent '{key}'")})),
        ));
    }
    let builtin = builtin_spec(&canonical);
    let effective = registry.get(&canonical).cloned().or_else(|| {
        builtin.map(|spec| {
            (*effective_for(&state, spec.key, query.tier.as_deref().unwrap_or(spec.tier))).clone()
        })
    });
    let issues = agent_issues(&registry, &canonical);
    let override_file = builtin
        .and_then(|spec| builtin_override(&state, spec))
        .or_else(|| state.config.agent(&canonical));
    Ok(Json(json!({
        "key": canonical,
        "display": effective.as_ref().map(|agent| agent.display.clone())
            .or_else(|| override_file.as_ref().map(|file| file.display.clone()))
            .unwrap_or_else(|| key.clone()),
        "tier": effective.as_ref().map(|agent| agent.tier.clone())
            .or_else(|| builtin.map(|spec| spec.tier.to_string()))
            .unwrap_or_else(|| "workspace".to_string()),
        "builtin": builtin.is_some(),
        "editable": true,
        "deletable": builtin.is_none(),
        "valid": effective.is_some() && issues.is_empty(),
        "errors": issues,
        "override": override_file,
        "effective": effective,
    })))
}

async fn save_agent_handler(
    State(state): State<ServerState>,
    Path(key): Path<String>,
    Json(agent): Json<AgentFile>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let registry = agent_registry(&state);
    let requested = key.trim().to_ascii_lowercase();
    let canonical = registry
        .canonical_key(&requested)
        .unwrap_or_else(|| requested.clone());
    let check = validate_definition(
        &state.config,
        &state.harness_root,
        &canonical,
        &agent,
        &registry,
    );
    if !check.errors.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "invalid agent definition",
                "errors": check.errors,
                "warnings": check.warnings,
            })),
        ));
    }
    state
        .config
        .save_agent(&canonical, agent)
        .map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": error.to_string()})),
            )
        })?;
    let registry = agent_registry(&state);
    let effective = registry.get(&canonical).cloned();
    Ok(Json(json!({
        "ok": true,
        "key": canonical,
        "builtin": builtin_spec(&canonical).is_some(),
        "warnings": check.warnings,
        "override": state.config.agent(&canonical),
        "effective": effective,
        "errors": effective.is_none().then(|| agent_issues(&registry, &canonical)),
    })))
}

async fn delete_agent_handler(
    State(state): State<ServerState>,
    Path(key): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let registry = agent_registry(&state);
    let requested = key.trim().to_ascii_lowercase();
    let canonical = registry
        .canonical_key(&requested)
        .unwrap_or_else(|| requested.clone());
    let builtin = builtin_spec(&canonical).is_some();
    if !builtin && state.config.agent(&canonical).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("unknown agent '{key}'")})),
        ));
    }
    state.config.delete_agent(&canonical).map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": error.to_string()})),
        )
    })?;
    // A reset must also remove legacy alias files; otherwise coder.json would
    // immediately re-apply as a developer override.
    if let Some(spec) = builtin_spec(&canonical) {
        for alias in spec.aliases {
            let _ = state.config.delete_agent(alias);
        }
    }
    let registry = agent_registry(&state);
    let effective = registry.get(&canonical).cloned();
    Ok(Json(json!({
        "ok": true,
        "key": canonical,
        "builtin": builtin,
        "reset": builtin,
        "deleted": !builtin,
        "effective": effective,
    })))
}

#[derive(Debug, Default, Deserialize)]
struct PreviewRequest {
    #[serde(default)]
    draft: Option<AgentFile>,
    #[serde(default)]
    workspace_path: String,
    #[serde(default)]
    mode: String,
}

fn tool_names(specs: &[Value]) -> Vec<String> {
    specs
        .iter()
        .filter_map(|spec| {
            spec.get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

async fn preview_agent_handler(
    State(state): State<ServerState>,
    Path(key): Path<String>,
    Json(request): Json<PreviewRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let registry = agent_registry(&state);
    let requested = key.trim().to_ascii_lowercase();
    let canonical = registry
        .canonical_key(&requested)
        .unwrap_or_else(|| requested.clone());
    let builtin = builtin_spec(&canonical);
    let file = request
        .draft
        .or_else(|| state.config.agent(&canonical))
        .or_else(|| builtin.map(|_| AgentFile::default()));
    let Some(file) = file else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("unknown agent '{key}'")})),
        ));
    };
    let check = validate_definition(
        &state.config,
        &state.harness_root,
        &canonical,
        &file,
        &registry,
    );
    if !check.errors.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "invalid agent definition",
                "errors": check.errors,
                "warnings": check.warnings,
            })),
        ));
    }
    let effective = resolve_definition(&state.config, &state.harness_root, &canonical, &file)
        .map_err(|error| (StatusCode::BAD_REQUEST, Json(json!({"error": error}))))?;
    let workspace = if request.workspace_path.trim().is_empty() {
        state.harness_root.clone()
    } else {
        PathBuf::from(request.workspace_path.trim())
    };
    let workspace_id = state
        .store
        .workspace_id_for_path(workspace.to_string_lossy().as_ref());
    let mode = if request.mode.trim().is_empty() {
        "build"
    } else {
        request.mode.trim()
    };
    let system_prompt = preview_system_prompt(
        &effective,
        &workspace,
        &workspace_id,
        &state.harness_root,
        mode,
    )
    .await;
    let code_mode = effective
        .code_mode
        .as_deref()
        .and_then(stark_tools::CodeMode::parse)
        .unwrap_or_default();
    let full = code_mode.enabled()
        && (matches!(code_mode, stark_tools::CodeMode::Only)
            || effective.full_codemode.unwrap_or(false));
    let direct = stark_tools::tool_specs_for_run_with_code_mode_full(
        mode,
        &canonical,
        &effective.access,
        &HashSet::new(),
        effective.tools.as_deref(),
        effective.mcp.as_deref(),
        code_mode,
        full,
    );
    let program = if code_mode.enabled() {
        stark_tools::program_specs_for_run_full(
            mode,
            &canonical,
            &effective.access,
            effective.tools.as_deref(),
            effective.mcp.as_deref(),
            full,
        )
    } else {
        Vec::new()
    };
    let mut warnings = check.warnings;
    warnings.extend(agent_issues(&registry, &canonical));
    Ok(Json(json!({
        "ok": true,
        "key": canonical,
        "builtin": builtin.is_some(),
        "effective": effective,
        "system_prompt": system_prompt,
        "prompt_tokens": stark_core::estimate_prompt_tokens(&system_prompt),
        "tools": {
            "direct": tool_names(&direct),
            "program": tool_names(&program),
            "direct_specs": direct,
            "program_specs": program,
        },
        "warnings": warnings,
    })))
}

#[derive(Debug, Default, Deserialize)]
struct DuplicateRequest {
    #[serde(default)]
    key: String,
    #[serde(default)]
    display: Option<String>,
}

async fn duplicate_agent_handler(
    State(state): State<ServerState>,
    Path(key): Path<String>,
    Json(request): Json<DuplicateRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let registry = agent_registry(&state);
    let requested = key.trim().to_ascii_lowercase();
    let canonical = registry
        .canonical_key(&requested)
        .unwrap_or_else(|| requested.clone());
    let Some(source) = registry.get(&canonical).cloned() else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("unknown agent '{key}'")})),
        ));
    };
    let new_key = request.key.trim().to_ascii_lowercase();
    if new_key.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "duplicate requires a new key"})),
        ));
    }
    let display = request
        .display
        .filter(|display| !display.trim().is_empty())
        .unwrap_or_else(|| format!("{} Copy", source.display));
    let mut file = state.config.agent(&canonical).unwrap_or_default();
    file.key = new_key.clone();
    file.display = display;
    file.description = source.description.clone();
    file.tier = source.tier.clone();
    file.access = Some(source.access.clone());
    // A custom agent has no compiled base instructions, so a duplicate of a
    // general-profile built-in must carry those instructions in its persona.
    let base_instructions = if source.prompt_profile == "general" {
        stark_core::agent_for_scope(&canonical).map(|spec| spec.instructions)
    } else {
        None
    };
    let persona = [
        source.persona.clone(),
        base_instructions.map(str::to_string),
    ]
    .into_iter()
    .flatten()
    .filter(|value| !value.trim().is_empty())
    .collect::<Vec<_>>()
    .join("\n\n");
    file.prompt = Some(stark_config::AgentPromptFile {
        profile: Some(source.prompt_profile.clone()),
        persona: (!persona.is_empty()).then_some(persona),
    });
    file.system_prompt = None;
    // Custom agents are direct-only, so the execute dispatcher cannot be
    // copied from Developer.
    let read_only = source.access == "read";
    file.tools = source.tools.as_ref().map(|tools| {
        tools
            .iter()
            .filter(|tool| tool.as_str() != "execute")
            // A read-only custom agent cannot carry mutating tools. Built-in
            // read-only agents enforce the same rule at the surface layer.
            .filter(|tool| !read_only || !stark_tools::is_read_blocked(tool))
            .cloned()
            .collect()
    });
    file.skills = Some(
        source
            .skills
            .clone()
            .unwrap_or_else(|| stark_tools::skill_names(&state.harness_root, &state.harness_root)),
    );
    file.mcp = Some(
        source
            .mcp
            .clone()
            .unwrap_or_else(|| stark_tools::configured_servers(&state.harness_root)),
    );
    file.aliases = None;
    file.delegation = Some(stark_config::AgentDelegationFile {
        delegatable: Some(source.delegatable),
        can_delegate: Some(source.can_delegate),
        can_spawn: Some(source.can_spawn),
    });
    file.ui = Some(stark_config::AgentUiFile {
        visible: Some(source.ui_visible),
        order: Some(source.ui_order),
    });
    file.code_mode = Some("off".to_string());
    file.full_codemode = None;
    let check = validate_definition(
        &state.config,
        &state.harness_root,
        &new_key,
        &file,
        &registry,
    );
    if !check.errors.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "invalid duplicate",
                "errors": check.errors,
                "warnings": check.warnings,
            })),
        ));
    }
    state.config.save_agent(&new_key, file).map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": error.to_string()})),
        )
    })?;
    let registry = agent_registry(&state);
    Ok(Json(json!({
        "ok": true,
        "key": new_key,
        "source": canonical,
        "effective": registry.get(&new_key),
    })))
}

async fn agent_catalog_handler(State(state): State<ServerState>) -> Json<Value> {
    let registry = agent_registry(&state);
    let tools: Vec<Value> = stark_tools::contract()
        .exposed_tools()
        .filter(|tool| stark_tools::is_implemented(&tool.name))
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.parameters,
                "program": stark_tools::is_program(&tool.name),
                "codemode": stark_tools::is_codemode(&tool.name),
                "read_blocked": stark_tools::is_read_blocked(&tool.name),
                "plan_write": stark_tools::is_plan_write(&tool.name),
            })
        })
        .collect();
    let agents: Vec<Value> = registry
        .visible()
        .into_iter()
        .map(|agent| {
            json!({
                "key": agent.key,
                "display": agent.display,
                "description": agent.description,
                "tier": agent.tier,
                "builtin": agent.builtin,
                "access": agent.access,
                "prompt_profile": agent.prompt_profile,
                "tools": agent.tools,
                "code_mode": agent.code_mode,
                "ui_order": agent.ui_order,
            })
        })
        .collect();
    let mcp = stark_tools::McpManager::global()
        .map(|manager| manager.status_snapshot())
        .unwrap_or_default();
    Json(json!({
        "schema_version": 2,
        "capabilities": {
            "agent_builder": true,
            "custom_agents": true,
            "custom_code_mode": false,
            "duplicate": true,
            "preview": true,
        },
        "agents": agents,
        "tools": tools,
        "skills": stark_tools::skill_names(&state.harness_root, &state.harness_root),
        "mcp": mcp,
        "mcp_configured": stark_tools::configured_servers(&state.harness_root),
        "providers": state.config.provider_summaries(),
        "active_provider": state.config.active_provider(),
        "issues": registry.issues(),
    }))
}

/// Build one portable backup zip. Excludes `.env`, models, logs and WAL.
async fn export_data_handler(
    State(state): State<ServerState>,
) -> Result<Response, (StatusCode, Json<Value>)> {
    let path = data_transfer::export(&state.harness_root, &state.store)
        .await
        .map_err(server_error)?;
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|error| server_error(format!("cannot read export {}: {error}", path.display())))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("stark-backup.zip")
        .to_string();
    let mut response = Response::new(axum::body::Body::from(bytes));
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/zip"),
    );
    if let Ok(value) =
        axum::http::HeaderValue::from_str(&format!("attachment; filename=\"{name}\""))
    {
        response
            .headers_mut()
            .insert(axum::http::header::CONTENT_DISPOSITION, value);
    }
    Ok(response)
}

/// Stage an uploaded backup. Applied on the next start (the DB is live here).
async fn import_data_handler(
    State(state): State<ServerState>,
    mut multipart: Multipart,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut bytes: Option<Vec<u8>> = None;
    let mut projects_root: Option<String> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| bad_request(format!("invalid upload: {error}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name == "projects_root" {
            projects_root = Some(
                field
                    .text()
                    .await
                    .map_err(|error| bad_request(format!("invalid projects_root: {error}")))?,
            );
        } else if name == "file" {
            bytes = Some(
                field
                    .bytes()
                    .await
                    .map_err(|error| bad_request(format!("cannot read upload: {error}")))?
                    .to_vec(),
            );
        }
    }
    let bytes = bytes.ok_or_else(|| bad_request("upload a Stark backup zip as 'file'"))?;
    let staged = data_transfer::stage_import(&state.harness_root, &bytes, projects_root.as_deref())
        .map_err(bad_request)?;
    Ok(Json(staged))
}

fn server_error(message: impl Into<String>) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": message.into() })),
    )
}

/// Harness locations for Settings. Never includes secrets.
async fn harness_handler(State(state): State<ServerState>) -> Json<Value> {
    let exe = std::env::current_exe()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    Json(json!({
        "harness_root": state.harness_root.to_string_lossy(),
        "install_path": exe,
        "workspaces": state
            .registry
            .list()
            .into_iter()
            .map(|record| {
                let exists = std::path::Path::new(&record.path).is_dir();
                json!({
                    "id": record.id,
                    "name": record.name,
                    "path": record.path,
                    "repo": record.repo,
                    "branch": record.branch,
                    "exists": exists,
                })
            })
            .collect::<Vec<_>>(),
    }))
}

#[derive(Debug, Deserialize)]
struct MigrationImportRequest {
    source: String,
}

fn migration_candidate(label: &str, root: PathBuf, active: &std::path::Path) -> Value {
    let chats = root.join(".deep_agents").join("chats.db");
    let sessions = root.join(".deep_agents").join("sessions");
    let config_dir = root.join(".deep_agents").join("config");
    let is_active = same_path(&root, active);
    json!({
        "id": label,
        "path": root.to_string_lossy(),
        "active": is_active,
        "has_chats_db": chats.is_file(),
        "has_sessions": sessions.is_dir(),
        "has_config": config_dir.is_dir(),
    })
}

fn same_path(left: &std::path::Path, right: &std::path::Path) -> bool {
    let normalize = |p: &std::path::Path| {
        std::fs::canonicalize(p)
            .map(|path| path.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_else(|_| p.to_string_lossy().to_ascii_lowercase())
    };
    normalize(left) == normalize(right)
}

/// One-time import sources. Never merged automatically; the user picks a
/// source in Settings and this copies it over the active harness root after
/// backing the active files up to `.bak`.
async fn migration_status_handler(State(state): State<ServerState>) -> Json<Value> {
    let active = state.harness_root.clone();
    let mut candidates = Vec::new();
    #[cfg(windows)]
    {
        let appdata = std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(r"C:\Users\Public"));
        candidates.push(migration_candidate(
            "appdata",
            appdata.join("com.stark.workspace"),
            &active,
        ));
    }
    // Repo root heuristic: only listed when it actually holds harness data.
    if let Ok(cwd) = std::env::current_dir() {
        for ancestor in [cwd.clone()]
            .into_iter()
            .chain(cwd.ancestors().map(PathBuf::from))
            .take(4)
        {
            if ancestor.join("backend-rs").join("Cargo.toml").exists()
                && !same_path(&ancestor, &active)
            {
                candidates.push(migration_candidate("repo", ancestor, &active));
                break;
            }
        }
    }
    Json(json!({ "active": active.to_string_lossy(), "candidates": candidates }))
}

async fn migration_import_handler(
    State(state): State<ServerState>,
    Json(request): Json<MigrationImportRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let source_raw = request.source.trim();
    if source_raw.is_empty() {
        return Err(bad_request("pick a migration source"));
    }
    // Either a known candidate id or an explicit folder path (e.g. a repo
    // root that currently holds the harness data). Paths are validated: they
    // must exist, contain `.deep_agents`, and differ from the active root.
    let status = migration_status_handler(State(state.clone())).await;
    let mut source = status
        .get("candidates")
        .and_then(serde_json::Value::as_array)
        .and_then(|candidates| {
            candidates.iter().find(|candidate| {
                candidate
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|id| id.eq_ignore_ascii_case(source_raw))
            })
        })
        .and_then(|candidate| {
            candidate
                .get("path")
                .and_then(serde_json::Value::as_str)
                .map(PathBuf::from)
        });
    if source.is_none() {
        let candidate = PathBuf::from(source_raw);
        if candidate.is_absolute() && candidate.join(".deep_agents").is_dir() {
            source = Some(candidate);
        }
    }
    let Some(source) = source else {
        return Err(bad_request(format!(
            "unknown migration source '{source_raw}' (use a listed id or a folder containing .deep_agents)"
        )));
    };
    if same_path(&source, &state.harness_root) {
        return Err(bad_request(
            "that source is already the active harness root",
        ));
    }
    let deep = |name: &str| {
        (
            source.join(".deep_agents").join(name),
            state.harness_root.join(".deep_agents").join(name),
        )
    };
    let mut copied: Vec<String> = Vec::new();
    for name in ["chats.db", "sessions", "config", "workspaces.json"] {
        let (from, to) = deep(name);
        if !from.exists() {
            continue;
        }
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| bad_request(format!("cannot create harness dir: {error}")))?;
        }
        if to.exists() {
            let backup = to.with_extension(format!(
                "{}.bak",
                to.extension().and_then(|ext| ext.to_str()).unwrap_or("db")
            ));
            let _ = std::fs::remove_file(&backup);
            let _ = std::fs::remove_dir_all(&backup);
            if std::fs::rename(&to, &backup).is_err() {
                let _ = std::fs::copy(&to, &backup);
            }
        }
        let outcome = if from.is_dir() {
            copy_dir_all(&from, &to).map_err(bad_request)
        } else {
            std::fs::copy(&from, &to)
                .map(|_| ())
                .map_err(|error| bad_request(format!("cannot copy {name}: {error}")))
        };
        outcome?;
        copied.push(name.to_string());
    }
    if copied.is_empty() {
        return Err(bad_request(
            "nothing to import: source has no chats, sessions, or config",
        ));
    }
    Ok(Json(json!({
        "ok": true,
        "source": source.to_string_lossy(),
        "copied": copied,
        "message": "Imported. Restart the app to use the migrated data.",
    })))
}

fn copy_dir_all(from: &std::path::Path, to: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|error| format!("cannot create dir: {error}"))?;
    for entry in std::fs::read_dir(from).map_err(|error| format!("cannot list dir: {error}"))? {
        let entry = entry.map_err(|error| format!("cannot read entry: {error}"))?;
        let dest = to.join(entry.file_name());
        let kind = entry
            .file_type()
            .map_err(|error| format!("cannot stat: {error}"))?;
        if kind.is_dir() {
            copy_dir_all(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), &dest).map_err(|error| format!("cannot copy: {error}"))?;
        }
    }
    Ok(())
}

const VIRTUAL_WORKSPACES: [(&str, &str, &str, &str); 4] = [
    ("_general", "General", "", "general"),
    ("_house", "House", "_house", "house"),
    ("_personal", "Personal", "_personal", "personal"),
    ("_dfd", "DFD", "_dfd", "dfd"),
];

fn virtual_workspace(id: &str) -> Option<(&'static str, &'static str, &'static str)> {
    VIRTUAL_WORKSPACES
        .iter()
        .find(|(candidate, _, _, _)| *candidate == id)
        .map(|(_, name, path, kind)| (*name, *path, *kind))
}

fn workspace_path(state: &ServerState, id: &str) -> Option<String> {
    if let Some((_, path, _)) = virtual_workspace(id) {
        return Some(path.to_string());
    }
    state.registry.get(id).map(|workspace| workspace.path)
}

fn bad_request(message: impl Into<String>) -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error": message.into()})),
    )
}

fn not_found(message: impl Into<String>) -> (StatusCode, Json<Value>) {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"error": message.into()})),
    )
}

async fn workspaces_handler(State(state): State<ServerState>) -> Json<Value> {
    let mut workspaces = Vec::new();
    for (id, name, path, kind) in VIRTUAL_WORKSPACES {
        let (chat_count, updated_at) = state.store.chat_stats(id).await;
        workspaces.push(json!({
            "id": id,
            "name": name,
            "path": path,
            "exists": true,
            "chatCount": chat_count,
            "addedAt": "",
            "updatedAt": updated_at,
            "isGeneral": kind == "general",
            "isHouse": kind == "house",
            "isPersonal": kind == "personal",
            "isDfd": kind == "dfd",
            "workspaceKind": kind,
        }));
    }

    let mut projects = Vec::new();
    for record in state.registry.list() {
        let (chat_count, updated_at) = state.store.chat_stats(&record.id).await;
        let exists = std::path::Path::new(&record.path).is_dir();
        let updated_at = if updated_at.is_empty() {
            record.added_at.clone()
        } else {
            updated_at
        };
        projects.push(json!({
            "id": record.id,
            "name": record.name,
            "path": record.path,
            "exists": exists,
            "chatCount": chat_count,
            "addedAt": record.added_at,
            "updatedAt": updated_at,
            "isGeneral": false,
            "workspaceKind": "project",
        }));
    }
    projects.sort_by(|left, right| {
        right["updatedAt"]
            .as_str()
            .unwrap_or("")
            .cmp(left["updatedAt"].as_str().unwrap_or(""))
    });
    workspaces.extend(projects);
    Json(json!({"workspaces": workspaces}))
}

#[derive(Debug, Deserialize)]
struct AddWorkspaceRequest {
    #[serde(default)]
    path: String,
    #[serde(default)]
    name: String,
}

async fn add_workspace_handler(
    State(state): State<ServerState>,
    Json(request): Json<AddWorkspaceRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let record = state
        .registry
        .add(&request.path, &request.name)
        .map_err(bad_request)?;
    Ok(Json(json!({
        "id": record.id,
        "name": record.name,
        "path": record.path,
        "exists": true,
        "chatCount": 0,
        "addedAt": record.added_at,
        "updatedAt": record.added_at,
        "isGeneral": false,
        "workspaceKind": "project",
    })))
}

/// Re-clone a registered workspace whose folder is missing on this machine.
async fn clone_workspace_handler(
    State(state): State<ServerState>,
    Path(workspace_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let registry = stark_store::WorkspaceRegistry::new(&state.harness_root);
    let record = registry
        .list()
        .into_iter()
        .find(|workspace| workspace.id == workspace_id)
        .ok_or_else(|| not_found(format!("unknown workspace '{workspace_id}'")))?;
    let Some(repo) = record.repo.as_deref().filter(|repo| !repo.is_empty()) else {
        return Err(bad_request(format!(
            "workspace '{}' has no stored repo URL",
            record.name
        )));
    };
    stark_store::validate_repo_url(repo)
        .map_err(|error| bad_request(format!("invalid repository URL: {error}")))?;
    let path = PathBuf::from(&record.path);
    if path.is_dir() {
        return Ok(Json(
            json!({ "ok": true, "already": true, "path": record.path }),
        ));
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| bad_request(format!("cannot create {}: {error}", parent.display())))?;
    }
    let mut command = tokio::process::Command::new("git");
    command.arg("clone").arg("--depth").arg("1");
    if let Some(branch) = record.branch.as_deref().filter(|branch| !branch.is_empty()) {
        stark_store::validate_branch_name(branch)
            .map_err(|error| bad_request(format!("invalid branch: {error}")))?;
        command.arg("--branch").arg(branch);
    }
    stark_sandbox::secrets::scrub_tokio_command(&mut command);
    command.env("GIT_TERMINAL_PROMPT", "0");
    let output = command
        .arg("--")
        .arg(repo)
        .arg(&path)
        .output()
        .await
        .map_err(|error| server_error(format!("cannot run git: {error}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr
            .chars()
            .rev()
            .take(800)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        let _ = tokio::fs::remove_dir_all(&path).await;
        return Err(server_error(format!("git clone failed: {}", tail.trim())));
    }
    Ok(Json(json!({
        "ok": true,
        "already": false,
        "path": record.path,
        "repo": repo,
    })))
}

async fn remove_workspace_handler(
    State(state): State<ServerState>,
    Path(workspace_id): Path<String>,
) -> Json<Value> {
    let status = if state.registry.remove(&workspace_id) {
        "ok"
    } else {
        "not found"
    };
    Json(json!({"status": status}))
}

fn chat_run_status(chat: &Value, running: bool) -> &'static str {
    if running {
        return "running";
    }
    if chat["lastKind"].as_str() == Some("aborted") {
        return "aborted";
    }
    match chat["lastRole"].as_str().unwrap_or("") {
        "error" => "failed",
        "agent" | "assistant" => "done",
        _ => "idle",
    }
}

async fn chats_handler(
    State(state): State<ServerState>,
    Path(workspace_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if workspace_path(&state, &workspace_id).is_none() {
        return Err(not_found("workspace not found"));
    }
    let mut chats = state.store.list_chats(&workspace_id).await;
    for chat in &mut chats {
        let chat_id = chat["id"].as_str().unwrap_or_default();
        let status = chat_run_status(
            chat,
            state.runs.is_running(chat_id) || stark_core::child_run_active(chat_id),
        );
        chat["status"] = Value::String(status.to_string());
    }
    Ok(Json(json!({"chats": chats})))
}

#[derive(Debug, Deserialize)]
struct CreateChatRequest {
    #[serde(default)]
    title: String,
    #[serde(default = "default_model")]
    model: String,
}

fn default_model() -> String {
    "Automatic Fallback".to_string()
}

async fn create_chat_handler(
    State(state): State<ServerState>,
    Path(workspace_id): Path<String>,
    Json(request): Json<CreateChatRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let Some(path) = workspace_path(&state, &workspace_id) else {
        return Err(not_found("workspace not found"));
    };
    let chat = state
        .store
        .create_chat(&workspace_id, &path, &request.title, &request.model)
        .await
        .map_err(|error| bad_request(error.to_string()))?;
    Ok(Json(chat))
}

async fn chat_handler(
    State(state): State<ServerState>,
    Path((_workspace_id, chat_id)): Path<(String, String)>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    match state.store.get_chat(&chat_id).await {
        Ok(Some(mut chat)) => {
            // Interruption is derived, never written: an unfinished TurnStarted
            // without a matching TurnComplete/TurnAborted means the turn did
            // not settle. New chats have no turns, running chats are skipped
            // by checking the live run registry first.
            if state.runs.subscribe(&chat_id).is_none()
                && !stark_core::child_run_active(&chat_id)
                && let Some(path) =
                    stark_store::rollout::find_rollout(&state.harness_root, &chat_id)
            {
                let lines = stark_store::rollout::read_rollout(&path);
                if let Some(turn_id) = stark_store::rollout::unfinished_turn(&lines) {
                    chat["interrupted_turn_id"] = Value::String(turn_id);
                }
            }
            Ok(Json(chat))
        }
        Ok(None) => Err(not_found("chat not found")),
        Err(error) => Err(bad_request(error.to_string())),
    }
}

async fn delete_chat_handler(
    State(state): State<ServerState>,
    Path((_workspace_id, chat_id)): Path<(String, String)>,
) -> Json<Value> {
    let ok = state.store.delete_chat(&chat_id).await.unwrap_or(false);
    state.runs.forget(&chat_id);
    Json(json!({"status": if ok { "ok" } else { "not found" }}))
}

#[derive(Debug, Deserialize)]
struct RenameChatRequest {
    #[serde(default)]
    title: String,
}

async fn rename_chat_handler(
    State(state): State<ServerState>,
    Path((_workspace_id, chat_id)): Path<(String, String)>,
    Json(request): Json<RenameChatRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let ok = state
        .store
        .update_chat_title(&chat_id, &request.title)
        .await
        .map_err(|error| bad_request(error.to_string()))?;
    if ok {
        Ok(Json(json!({"status": "ok"})))
    } else {
        Err(not_found("chat not found"))
    }
}

#[derive(Debug, Deserialize)]
struct AddMessageRequest {
    #[serde(default = "default_role")]
    role: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    metadata: Option<Value>,
}

fn default_role() -> String {
    "user".to_string()
}

async fn add_message_handler(
    State(state): State<ServerState>,
    Path((_workspace_id, chat_id)): Path<(String, String)>,
    Json(request): Json<AddMessageRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if state
        .store
        .get_chat(&chat_id)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return Err(not_found("chat not found"));
    }
    let message = state
        .store
        .add_message(
            &chat_id,
            &request.role,
            &request.content,
            request.metadata.as_ref(),
        )
        .await
        .map_err(|error| bad_request(error.to_string()))?;
    Ok(Json(message))
}

async fn ui_state_handler(State(state): State<ServerState>) -> Json<Value> {
    let value = tokio::fs::read_to_string(&state.ui_state_path)
        .await
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| json!({}));
    Json(value)
}

async fn save_ui_state_handler(
    State(state): State<ServerState>,
    Json(value): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if let Some(parent) = state.ui_state_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| bad_request(error.to_string()))?;
    }
    let text =
        serde_json::to_string_pretty(&value).map_err(|error| bad_request(error.to_string()))?;
    tokio::fs::write(&state.ui_state_path, text)
        .await
        .map_err(|error| bad_request(error.to_string()))?;
    Ok(Json(json!({"ok": true})))
}

async fn undo_run_handler(
    State(state): State<ServerState>,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut roots = vec![state.harness_root.clone()];
    roots.extend(
        state
            .registry
            .list()
            .into_iter()
            .map(|workspace| std::path::PathBuf::from(workspace.path)),
    );
    for root in roots {
        if let Some((ok, messages)) = Checkpoint::restore_run_any(&root, &run_id) {
            return Ok(Json(json!({
                "status": "undone",
                "ok": ok,
                "results": messages,
            })));
        }
    }
    Err(not_found("This run is no longer available for undo."))
}

async fn session_reset_handler() -> Json<Value> {
    Json(json!({"ok": true}))
}

#[derive(Debug, Deserialize)]
struct CreateGroupRequest {
    #[serde(default)]
    goal: String,
    #[serde(default)]
    workspace_id: String,
}

fn starter_group_manifest(goal: &str) -> Value {
    let mut slug = String::new();
    let mut dash = false;
    for character in goal.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            if dash && !slug.is_empty() {
                slug.push('-');
            }
            dash = false;
            slug.push(character);
        } else {
            dash = true;
        }
        if slug.len() >= 48 {
            break;
        }
    }
    let id = slug.trim_matches('-');
    let id = if id.is_empty() { "group-project" } else { id };
    let id = format!("{}-{}", id, &Uuid::new_v4().simple().to_string()[..6]);
    json!({
        "id": id,
        "goal": goal,
        "status": "active",
        "milestones": [
            {
                "id": "m1",
                "title": "Plan",
                "items": [{
                    "id": "m1-i1",
                    "title": "Break the goal into a clear deliverable",
                    "detail": format!("Review the goal and prepare a concrete plan: {goal}"),
                    "acceptance": ["The deliverable and its completion checks are recorded"]
                }]
            },
            {
                "id": "m2",
                "title": "Deliver",
                "depends_on": ["m1"],
                "items": [{
                    "id": "m2-i1",
                    "title": "Complete the requested work",
                    "detail": goal,
                    "acceptance": ["The requested deliverable is complete and summarized"]
                }]
            }
        ]
    })
}

async fn create_group_handler(
    State(_state): State<ServerState>,
    Json(request): Json<CreateGroupRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let goal = request.goal.trim();
    if goal.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "goal is required"})),
        ));
    }
    let detail = GroupStore::default_root()
        .create(&starter_group_manifest(goal), request.workspace_id.trim())
        .await
        .map_err(group_error)?;
    let id = detail["id"].as_str().unwrap_or_default();
    Ok(Json(json!({
        "id": id,
        "path": detail["path"],
        "title": goal,
        "runtime_notice": GROUP_RUNTIME_NOTICE,
        "project": detail["project"],
    })))
}

#[derive(Debug, Deserialize)]
struct ScheduleQuery {
    #[serde(default)]
    workspace_id: String,
}

#[derive(Debug, Deserialize)]
struct ScheduleCreateRequest {
    #[serde(default)]
    workspace_id: String,
    chat_id: String,
    #[serde(default)]
    name: String,
    prompt: String,
    at: String,
    #[serde(default = "default_schedule_recurrence")]
    recurrence: String,
}

fn default_schedule_recurrence() -> String {
    "none".to_string()
}

async fn schedules_handler(
    State(state): State<ServerState>,
    Query(query): Query<ScheduleQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let store = ScheduleStore::open(&state.harness_root)
        .await
        .map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": error.to_string()})),
            )
        })?;
    let schedules = store
        .list("")
        .await
        .map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": error.to_string()})),
            )
        })?
        .into_iter()
        .filter(|record| query.workspace_id.is_empty() || record.workspace_id == query.workspace_id)
        .map(|record| schedule_json(&record))
        .collect::<Vec<_>>();
    Ok(Json(json!({"schedules": schedules})))
}

async fn create_schedule_handler(
    State(state): State<ServerState>,
    Json(request): Json<ScheduleCreateRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let recurrence = request.recurrence.trim().to_lowercase();
    if !matches!(recurrence.as_str(), "none" | "daily" | "weekly" | "monthly") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "recurrence must be none, daily, weekly, or monthly"})),
        ));
    }
    let chat_id = request.chat_id.trim();
    if chat_id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "chat_id is required"})),
        ));
    }
    let workspace_id = state
        .store
        .chat_workspace_id(chat_id)
        .await
        .map_err(|error| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": error.to_string()})),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "Chat not found"})),
            )
        })?;
    if !request.workspace_id.trim().is_empty() && request.workspace_id != workspace_id {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Chat does not belong to that workspace"})),
        ));
    }
    if request.prompt.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "prompt is required"})),
        ));
    }
    let chat = state
        .store
        .get_chat(chat_id)
        .await
        .map_err(|error| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": error.to_string()})),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "Chat not found"})),
            )
        })?;
    let Some(at) = parse_schedule_time(&request.at) else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "at must be an ISO timestamp or HH:MM"})),
        ));
    };
    let workspace_path = state
        .store
        .chat_workspace_path(chat_id)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    let record = ScheduleRecord {
        id: String::new(),
        chat_id: chat_id.to_string(),
        workspace_id,
        workspace_path,
        scope: "orchestrator".to_string(),
        role: Some("Orchestrator".to_string()),
        model: chat["model"]
            .as_str()
            .unwrap_or("Automatic Fallback")
            .to_string(),
        temperature: 0.0,
        name: if request.name.trim().is_empty() {
            "Scheduled task".to_string()
        } else {
            request.name.trim().to_string()
        },
        prompt: request.prompt.trim().to_string(),
        next_run_at: at.to_rfc3339(),
        recurrence,
        status: "scheduled".to_string(),
    };
    let store = ScheduleStore::open(&state.harness_root)
        .await
        .map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": error.to_string()})),
            )
        })?;
    let record = store.create(record).await.map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": error.to_string()})),
        )
    })?;
    Ok(Json(schedule_json(&record)))
}

async fn delete_schedule_handler(
    State(state): State<ServerState>,
    Path(schedule_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let store = ScheduleStore::open(&state.harness_root)
        .await
        .map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": error.to_string()})),
            )
        })?;
    let cancelled = store.cancel(&schedule_id).await.map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": error.to_string()})),
        )
    })?;
    if !cancelled {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Schedule not found"})),
        ));
    }
    Ok(Json(
        json!({"ok": true, "id": schedule_id, "status": "cancelled"}),
    ))
}

#[derive(Debug, Deserialize)]
struct GroupQuery {
    #[serde(default)]
    workspace_id: String,
}

async fn groups_handler(
    State(state): State<ServerState>,
    Query(query): Query<GroupQuery>,
) -> Json<Value> {
    let scoped =
        !query.workspace_id.is_empty() && state.registry.get(&query.workspace_id).is_some();
    let projects = GroupStore::default_root().list().await;
    let mut projects = if scoped {
        projects
            .into_iter()
            .filter(|project| {
                project["origin_workspace_id"].as_str() == Some(query.workspace_id.as_str())
            })
            .collect()
    } else {
        projects
    };
    for project in &mut projects {
        if project["status"] == "active" {
            project["status"] = json!("running");
        }
    }
    Json(json!({"projects": projects, "scoped": scoped}))
}

async fn group_handler(
    State(_state): State<ServerState>,
    Path(project_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut detail = GroupStore::default_root()
        .detail(&project_id, 200)
        .await
        .map_err(group_error)?;
    if detail["project"]["status"] == "active" {
        detail["project"]["status"] = json!("running");
    }
    detail["runtime_notice"] = json!(GROUP_RUNTIME_NOTICE);
    Ok(Json(detail))
}

#[derive(Debug, Deserialize)]
struct GroupUpdateRequest {
    #[serde(default)]
    op: String,
    #[serde(default)]
    params: Value,
}

async fn group_update_handler(
    Path(project_id): Path<String>,
    Json(request): Json<GroupUpdateRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    GroupStore::default_root()
        .update(&project_id, &request.op, &request.params)
        .await
        .map(Json)
        .map_err(group_error)
}

#[derive(Debug, Deserialize)]
struct GroupApprovalRequest {
    #[serde(default)]
    grant: bool,
    #[serde(default)]
    note: String,
}

async fn group_approval_handler(
    Path((project_id, item_id)): Path<(String, String)>,
    Json(request): Json<GroupApprovalRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    GroupStore::default_root()
        .approval(&project_id, &item_id, request.grant, &request.note)
        .await
        .map(Json)
        .map_err(group_error)
}

fn group_error(error: String) -> (StatusCode, Json<Value>) {
    let status =
        if error.to_lowercase().contains("unknown") || error.to_lowercase().contains("not found") {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::BAD_REQUEST
        };
    (status, Json(json!({"error": error})))
}

async fn canvas_status_handler(
    State(state): State<ServerState>,
    Path(chat_id): Path<String>,
) -> Json<Value> {
    let Ok(store) = CanvasStore::open(&state.harness_root).await else {
        return Json(json!({"active": false, "watch": null}));
    };
    match store.active_watch(&chat_id).await {
        Ok(Some(watch)) => Json(json!({"active": true, "watch": watch})),
        _ => Json(json!({"active": false, "watch": null})),
    }
}

fn valid_note_scope(scope_type: &str, scope_id: &str) -> bool {
    matches!(scope_type, "chat" | "group") && !scope_id.trim().is_empty()
}

#[derive(Debug, Deserialize)]
struct HousePatch {
    #[serde(default)]
    state: Value,
}

#[derive(Debug, Deserialize)]
struct HouseHistoryQuery {
    #[serde(default)]
    device_id: String,
    hours: Option<i64>,
}

fn house_error(error: String) -> (StatusCode, Json<Value>) {
    let status =
        if error.to_lowercase().contains("unknown") || error.to_lowercase().contains("not found") {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::BAD_REQUEST
        };
    (status, Json(json!({"error": error})))
}

async fn house_state_handler(
    State(state): State<ServerState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    HouseStore::new(&state.harness_root)
        .state()
        .await
        .map(|state| Json(public_state(&state)))
        .map_err(house_error)
}

async fn house_device_handler(
    State(state): State<ServerState>,
    Path(device_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    match HouseStore::new(&state.harness_root)
        .device(&device_id)
        .await
    {
        Ok(Some(device)) => Ok(Json(public_device(&device_id, &device))),
        Ok(None) => Err(not_found("Device not found")),
        Err(error) => Err(house_error(error)),
    }
}

async fn house_set_device_handler(
    State(state): State<ServerState>,
    Path(device_id): Path<String>,
    Json(request): Json<HousePatch>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let Some(patch) = request.state.as_object() else {
        return Err(bad_request("state must be an object"));
    };
    HouseStore::new(&state.harness_root)
        .set_device(&device_id, patch)
        .await
        .map(|device| Json(public_device(&device_id, &device)))
        .map_err(house_error)
}

async fn house_set_room_handler(
    State(state): State<ServerState>,
    Path(room_id): Path<String>,
    Json(request): Json<HousePatch>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let Some(patch) = request.state.as_object() else {
        return Err(bad_request("state must be an object"));
    };
    HouseStore::new(&state.harness_root)
        .set_room(&room_id, patch)
        .await
        .map(Json)
        .map_err(house_error)
}

async fn house_run_scene_handler(
    State(state): State<ServerState>,
    Path(scene_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let result = HouseStore::new(&state.harness_root)
        .run_scene(&scene_id)
        .await
        .map_err(house_error)?;
    Ok(Json(json!({
        "sceneId": scene_id,
        "name": result["scene"]["name"].as_str().unwrap_or(""),
        "applied": result["applied"],
        "skipped": result["skipped"],
    })))
}

async fn house_history_handler(
    State(state): State<ServerState>,
    Query(query): Query<HouseHistoryQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if query.device_id.trim().is_empty() {
        return Err(bad_request("device_id is required"));
    }
    HouseStore::new(&state.harness_root)
        .history(query.device_id.trim(), query.hours.unwrap_or(24))
        .await
        .map(Json)
        .map_err(house_error)
}

async fn house_reset_handler(
    State(state): State<ServerState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let result = HouseStore::new(&state.harness_root)
        .reset()
        .await
        .map_err(house_error)?;
    Ok(Json(json!({
        "ok": true,
        "state": public_state(&result["state"]),
    })))
}

async fn list_notes_handler(
    State(state): State<ServerState>,
    Path((scope_type, scope_id)): Path<(String, String)>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !valid_note_scope(&scope_type, &scope_id) {
        return Err(bad_request("invalid scope"));
    }
    let notes = state.store.list_notes(&scope_type, &scope_id).await;
    Ok(Json(json!({"notes": notes})))
}

#[derive(Debug, Deserialize)]
struct NoteCreateRequest {
    #[serde(default = "default_note_scope")]
    scope_type: String,
    #[serde(default)]
    scope_id: String,
    #[serde(default = "default_note_title")]
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    todos: Value,
}

fn default_note_scope() -> String {
    "chat".to_string()
}

fn default_note_title() -> String {
    "Note".to_string()
}

async fn create_note_handler(
    State(state): State<ServerState>,
    Json(request): Json<NoteCreateRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !valid_note_scope(&request.scope_type, &request.scope_id) {
        return Err(bad_request("invalid scope"));
    }
    let note = state
        .store
        .create_note(
            &request.scope_type,
            &request.scope_id,
            &request.title,
            &request.body,
            &request.todos,
        )
        .await
        .map_err(|error| bad_request(error.to_string()))?;
    Ok(Json(note))
}

#[derive(Debug, Deserialize)]
struct NoteUpdateRequest {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    todos: Option<Value>,
}

async fn update_note_handler(
    State(state): State<ServerState>,
    Path(note_id): Path<String>,
    Json(request): Json<NoteUpdateRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let note = state
        .store
        .update_note(
            &note_id,
            request.title.as_deref(),
            request.body.as_deref(),
            request.todos.as_ref(),
        )
        .await
        .map_err(|error| bad_request(error.to_string()))?;
    match note {
        Some(note) => Ok(Json(note)),
        None => Err(not_found("note not found")),
    }
}

async fn delete_note_handler(
    State(state): State<ServerState>,
    Path(note_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let deleted = state
        .store
        .delete_note(&note_id)
        .await
        .map_err(|error| bad_request(error.to_string()))?;
    if deleted {
        Ok(Json(json!({"ok": true})))
    } else {
        Err(not_found("note not found"))
    }
}

async fn workspace_handler(
    State(state): State<ServerState>,
    Path(workspace_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if let Some((name, path, kind)) = virtual_workspace(&workspace_id) {
        let (chat_count, updated_at) = state.store.chat_stats(&workspace_id).await;
        return Ok(Json(json!({
            "id": workspace_id,
            "name": name,
            "path": path,
            "exists": true,
            "chatCount": chat_count,
            "addedAt": "",
            "updatedAt": updated_at,
            "isGeneral": kind == "general",
            "isHouse": kind == "house",
            "isPersonal": kind == "personal",
            "isDfd": kind == "dfd",
            "workspaceKind": kind,
        })));
    }
    let Some(record) = state.registry.get(&workspace_id) else {
        return Err(not_found("workspace not found"));
    };
    let (chat_count, updated_at) = state.store.chat_stats(&workspace_id).await;
    Ok(Json(json!({
        "id": record.id,
        "name": record.name,
        "path": record.path,
        "exists": std::path::Path::new(&record.path).is_dir(),
        "chatCount": chat_count,
        "addedAt": record.added_at,
        "updatedAt": if updated_at.is_empty() { record.added_at } else { updated_at },
        "isGeneral": false,
        "workspaceKind": "project",
    })))
}

const FILE_SKIP_DIRS: [&str; 15] = [
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    "venv",
    "venv312",
    ".venv",
    "__pycache__",
    ".pytest_cache",
    ".ruff_cache",
    ".agent-memory",
    ".deep_agents",
    "dist",
    "build",
    ".next",
];

const FILE_MAX_SIZE: u64 = 5 * 1024 * 1024;
const FILE_PREVIEW_MAX: u64 = 200 * 1024;

#[derive(Debug, Deserialize)]
struct FilesQuery {
    max_depth: Option<usize>,
}

async fn workspace_files_handler(
    State(state): State<ServerState>,
    Path(workspace_id): Path<String>,
    Query(query): Query<FilesQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let Some(path) = workspace_path(&state, &workspace_id) else {
        return Err(not_found("workspace not found"));
    };
    if path.is_empty() || !std::path::Path::new(&path).is_dir() {
        return Err(not_found("workspace folder not found"));
    }
    let max_depth = query.max_depth.unwrap_or(8).min(32);
    let files = tokio::task::spawn_blocking(move || list_files(&path, max_depth))
        .await
        .map_err(|error| bad_request(error.to_string()))?;
    Ok(Json(json!({"files": files})))
}

async fn workspace_file_handler(
    State(state): State<ServerState>,
    Path(workspace_id): Path<String>,
    Query(query): Query<FileQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let Some(root) = workspace_path(&state, &workspace_id) else {
        return Err(not_found("workspace not found"));
    };
    if root.is_empty() {
        return Err(not_found("workspace folder not found"));
    }
    let file_path = query.path.unwrap_or_default();
    match tokio::task::spawn_blocking(move || read_file(&root, &file_path)).await {
        Ok(Ok(content)) => Ok(Json(content)),
        Ok(Err((status, message))) => Err((status, Json(json!({"error": message})))),
        Err(error) => Err(bad_request(error.to_string())),
    }
}

#[derive(Debug, Deserialize)]
struct FileQuery {
    path: Option<String>,
}

fn list_files(root: &str, max_depth: usize) -> Vec<Value> {
    let root = std::path::Path::new(root);
    let mut files = Vec::new();
    walk_files(root, 0, max_depth, "", &mut files);

    let project_memory = root.join(".agent-memory").join("project.md");
    if project_memory.is_file()
        && let Ok(metadata) = project_memory.metadata()
    {
        files.push(json!({
            "name": "project.md",
            "path": ".agent-memory/project.md",
            "type": "file",
            "size": metadata.len(),
        }));
    }

    files.sort_by(|left, right| {
        let left_dir = left["type"] == "dir";
        let right_dir = right["type"] == "dir";
        right_dir.cmp(&left_dir).then_with(|| {
            left["path"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .cmp(&right["path"].as_str().unwrap_or("").to_lowercase())
        })
    });
    files
}

fn walk_files(
    dir: &std::path::Path,
    depth: usize,
    max_depth: usize,
    prefix: &str,
    out: &mut Vec<Value>,
) {
    if depth > max_depth {
        return;
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .collect();
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_lowercase());

    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        if FILE_SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let full = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            out.push(json!({"name": name, "path": rel, "type": "dir"}));
            walk_files(&full, depth + 1, max_depth, &rel, out);
        } else if metadata.is_file() {
            if metadata.len() > FILE_MAX_SIZE {
                continue;
            }
            out.push(json!({
                "name": name,
                "path": rel,
                "type": "file",
                "size": metadata.len(),
            }));
        }
    }
}

fn read_file(root: &str, requested: &str) -> Result<Value, (StatusCode, String)> {
    let root = std::fs::canonicalize(root).map_err(|_| {
        (
            StatusCode::NOT_FOUND,
            "Workspace folder not found".to_string(),
        )
    })?;
    let candidate = std::fs::canonicalize(root.join(requested))
        .map_err(|_| (StatusCode::NOT_FOUND, "File not found".to_string()))?;
    if !candidate.starts_with(&root) {
        return Err((
            StatusCode::BAD_REQUEST,
            "Path outside workspace".to_string(),
        ));
    }
    if !candidate.is_file() {
        return Err((StatusCode::NOT_FOUND, "File not found".to_string()));
    }
    let metadata = std::fs::metadata(&candidate)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    if metadata.len() > FILE_PREVIEW_MAX {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            "File too large to preview".to_string(),
        ));
    }
    let bytes = std::fs::read(&candidate)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok(json!({
        "path": requested,
        "content": String::from_utf8_lossy(&bytes),
        "truncated": false,
    }))
}

fn attachment_note(harness_root: &std::path::Path, attachments: &[Value]) -> String {
    let roots: Vec<std::path::PathBuf> = [
        harness_root.join(".deep_agents").join("uploads"),
        harness_root.join(".deep_agents").join("live_screen"),
    ]
    .into_iter()
    .filter_map(|root| std::fs::canonicalize(root).ok())
    .collect();
    if roots.is_empty() {
        return String::new();
    }
    attachments
        .iter()
        .filter_map(|attachment| {
            let name = attachment.get("name").and_then(Value::as_str)?;
            let mime = attachment
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("application/octet-stream");
            let size = attachment.get("size").and_then(Value::as_u64).unwrap_or(0);
            let path = attachment
                .get("path")
                .and_then(Value::as_str)
                .and_then(|path| std::fs::canonicalize(path).ok())
                .filter(|path| roots.iter().any(|root| path.starts_with(root)))?;
            Some(format!(
                "- {name} ({mime}, {size} bytes): {}",
                path.display()
            ))
        })
        .collect::<Vec<_>>()
        .join(
            "
",
        )
}

async fn subagents_handler(
    State(state): State<ServerState>,
    Path((workspace_id, chat_id)): Path<(String, String)>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if workspace_path(&state, &workspace_id).is_none() {
        return Err(not_found("workspace not found"));
    }
    let parent_id = match AgentStore::open(&state.harness_root).await {
        Ok(store) => store
            .by_chat(&chat_id)
            .await
            .ok()
            .flatten()
            .map(|agent| agent.id),
        Err(_) => None,
    };
    let subagents = if let Some(parent_id) = parent_id {
        match AgentStore::open(&state.harness_root).await {
            Ok(store) => store
                .list(Some(&parent_id))
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|agent| {
                    json!({
                        "child_id": agent.chat_id,
                        "agent_id": agent.id,
                        "name": agent.role,
                        "label": agent.nickname,
                        "status": agent.status,
                        "created": 0,
                        "parent_session": chat_id,
                    })
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    } else {
        Vec::new()
    };
    Ok(Json(json!({"subagents": subagents})))
}

async fn subagent_handler(
    State(state): State<ServerState>,
    Path((workspace_id, child_id)): Path<(String, String)>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if workspace_path(&state, &workspace_id).is_none() {
        return Err(not_found("workspace not found"));
    }
    if !child_id.starts_with("cont-") {
        return Err(not_found("subagent session not found"));
    }
    let Some(chat) = state
        .store
        .get_chat(&child_id)
        .await
        .map_err(|error| bad_request(error.to_string()))?
    else {
        return Err(not_found("subagent session not found"));
    };
    let title = chat["title"].as_str().unwrap_or("Subagent").to_string();
    let live = match AgentStore::open(&state.harness_root).await {
        Ok(store) => store.by_chat(&child_id).await.ok().flatten().map(|agent| {
            json!({
                "child_id": child_id,
                "agent_id": agent.id,
                "name": agent.role,
                "label": agent.nickname,
                "status": agent.status,
                "report": agent.result,
                "created": 0,
                "parent_session": agent.parent_id,
            })
        }),
        Err(_) => None,
    }
    .unwrap_or_else(|| {
        json!({
            "child_id": child_id,
            "name": title,
            "label": title,
            "status": "idle",
            "created": 0,
            "parent_session": null,
        })
    });
    let entries: Vec<Value> = chat["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|message| message["metadata"]["isTrace"] != true)
        .map(|message| {
            let role = message["role"].as_str().unwrap_or("assistant");
            json!({
                "kind": if role == "user" { "user" } else if role == "tool" { "tool" } else { "assistant" },
                "content": message["content"],
                "toolName": message["metadata"]["toolName"],
                "isError": role == "error",
                "ts": 0,
            })
        })
        .collect();
    Ok(Json(json!({
        "info": live,
        "entries": entries,
    })))
}

const MAX_ATTACHMENTS: usize = 8;
const MAX_ATTACHMENT_BYTES: usize = 20 * 1024 * 1024;

async fn upload_attachments_handler(
    State(state): State<ServerState>,
    Path((workspace_id, chat_id)): Path<(String, String)>,
    mut multipart: Multipart,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if workspace_path(&state, &workspace_id).is_none() {
        return Err(not_found("workspace not found"));
    }
    if state
        .store
        .get_chat(&chat_id)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return Err(not_found("chat not found"));
    }
    let Some(workspace_component) = safe_component(&workspace_id) else {
        return Err(bad_request("invalid workspace id"));
    };
    let Some(chat_component) = safe_component(&chat_id) else {
        return Err(bad_request("invalid chat id"));
    };

    let upload_dir = state
        .harness_root
        .join(".deep_agents")
        .join("uploads")
        .join(workspace_component)
        .join(chat_component);
    tokio::fs::create_dir_all(&upload_dir)
        .await
        .map_err(|error| bad_request(error.to_string()))?;

    let mut attachments = Vec::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| bad_request(format!("invalid multipart upload: {error}")))?
    {
        if attachments.len() >= MAX_ATTACHMENTS {
            return Err(bad_request("A maximum of 8 files can be uploaded"));
        }
        let Some(raw_name) = field.file_name().map(str::to_string) else {
            continue;
        };
        let name = safe_filename(&raw_name);
        let mime = field
            .content_type()
            .unwrap_or("application/octet-stream")
            .to_string();
        let bytes = field
            .bytes()
            .await
            .map_err(|error| bad_request(format!("failed to read upload: {error}")))?;
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            return Err(bad_request(format!("{name} is larger than 20 MB")));
        }

        let stored_name = format!("{}-{}", &Uuid::new_v4().simple().to_string()[..8], name);
        let target = upload_dir.join(&stored_name);
        tokio::fs::write(&target, &bytes)
            .await
            .map_err(|error| bad_request(error.to_string()))?;
        attachments.push(json!({
            "id": stored_name,
            "name": name,
            "type": mime,
            "size": bytes.len(),
            "path": target,
            "isImage": mime.starts_with("image/"),
        }));
    }

    if attachments.is_empty() {
        return Err(bad_request("no files uploaded"));
    }
    Ok(Json(json!({"attachments": attachments})))
}

fn attachment_mime(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "ogg" | "oga" => "audio/ogg",
        "webm" => "audio/webm",
        "flac" => "audio/flac",
        "pdf" => "application/pdf",
        "txt" | "md" | "log" => "text/plain; charset=utf-8",
        "json" => "application/json",
        "csv" => "text/csv; charset=utf-8",
        "mp4" => "video/mp4",
        _ => "application/octet-stream",
    }
}

fn safe_attachment_component(value: &str) -> Option<&str> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains(['/', '\\'])
        || value.chars().any(char::is_control)
    {
        return None;
    }
    Some(value)
}

async fn attachment_handler(
    State(state): State<ServerState>,
    Path((workspace_id, chat_id, attachment_id)): Path<(String, String, String)>,
) -> Result<Response, (StatusCode, Json<Value>)> {
    if workspace_path(&state, &workspace_id).is_none() {
        return Err(not_found("workspace not found"));
    }
    if state
        .store
        .get_chat(&chat_id)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return Err(not_found("chat not found"));
    }
    let Some(workspace_component) = safe_component(&workspace_id) else {
        return Err(bad_request("invalid workspace id"));
    };
    let Some(chat_component) = safe_component(&chat_id) else {
        return Err(bad_request("invalid chat id"));
    };
    let Some(name) = safe_attachment_component(&attachment_id) else {
        return Err(bad_request("invalid attachment id"));
    };
    let upload_dir = state
        .harness_root
        .join(".deep_agents")
        .join("uploads")
        .join(workspace_component)
        .join(chat_component);
    let Ok(canonical_dir) = tokio::fs::canonicalize(&upload_dir).await else {
        return Err(not_found("attachment not found"));
    };
    let path = upload_dir.join(name);
    let Ok(canonical_path) = tokio::fs::canonicalize(&path).await else {
        return Err(not_found("attachment not found"));
    };
    if !canonical_path.starts_with(&canonical_dir) {
        return Err(not_found("attachment not found"));
    }
    let bytes = tokio::fs::read(&canonical_path)
        .await
        .map_err(|error| bad_request(error.to_string()))?;
    let mime = attachment_mime(&canonical_path);
    let inline = mime.starts_with("image/")
        || mime.starts_with("audio/")
        || mime.starts_with("video/")
        || mime == "application/pdf";
    let fallback_name: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_graphic() && !matches!(character, '"' | '\\') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let disposition = format!(
        "{}; filename=\"{}\"",
        if inline { "inline" } else { "attachment" },
        fallback_name
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_DISPOSITION, disposition)
        .header(header::CACHE_CONTROL, "private, max-age=3600")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from(bytes))
        .map_err(|error| bad_request(error.to_string()))
}

fn safe_component(value: &str) -> Option<&str> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.chars().any(|character| {
            !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
        })
    {
        return None;
    }
    Some(value)
}

fn safe_filename(value: &str) -> String {
    let base = value
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("attachment")
        .trim();
    let cleaned: String = base
        .chars()
        .map(|character| {
            if character.is_control() {
                '_'
            } else {
                character
            }
        })
        .collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "attachment".to_string()
    } else {
        cleaned
    }
}

#[derive(Debug, Deserialize)]
struct RewindRequest {
    #[serde(default)]
    message_id: String,
    #[serde(default)]
    restore_files: Option<bool>,
    #[serde(default)]
    force: bool,
}

#[derive(Debug, Deserialize, Default)]
struct ForkRequest {
    #[serde(default)]
    title: String,
}

async fn fork_chat_handler(
    State(state): State<ServerState>,
    Path((workspace_id, chat_id)): Path<(String, String)>,
    Json(request): Json<ForkRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let Some(workspace) = workspace_path(&state, &workspace_id) else {
        return Err(not_found("workspace not found"));
    };
    if state.runs.is_running(&chat_id) {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"error": "This chat is still running. Stop it before forking."})),
        ));
    }
    let chat = state
        .store
        .fork_chat(&workspace_id, &workspace, &chat_id, &request.title)
        .await
        .map_err(|error| bad_request(error.to_string()))?;
    Ok(Json(chat))
}

async fn rewind_chat_handler(
    State(state): State<ServerState>,
    Path((workspace_id, chat_id)): Path<(String, String)>,
    Json(request): Json<RewindRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let Some(workspace) = workspace_path(&state, &workspace_id) else {
        return Err(not_found("workspace not found"));
    };
    if request.message_id.is_empty() {
        return Err(bad_request("message_id is required"));
    }
    // Rewinding is allowed while a run is active: stop it first, give the
    // run task a moment to persist its aborted tail, then truncate. The wait
    // prevents the aborted marker from landing after the rewind and making
    // the message reappear.
    if state.runs.is_running(&chat_id) || stark_core::child_run_active(&chat_id) {
        state.runs.cancel(&chat_id);
        stark_core::cancel_child_run(&chat_id);
        for _ in 0..40 {
            if !state.runs.is_running(&chat_id) && !stark_core::child_run_active(&chat_id) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        if state.runs.is_running(&chat_id) || stark_core::child_run_active(&chat_id) {
            return Err((
                StatusCode::CONFLICT,
                Json(json!({"error": "This chat is still stopping. Try again in a moment."})),
            ));
        }
    }
    let Some(removed_messages) = state
        .store
        .messages_from(&chat_id, &request.message_id)
        .await
        .map_err(|error| bad_request(error.to_string()))?
    else {
        return Err(not_found("Message not found"));
    };
    let user_message_ids: Vec<String> = removed_messages
        .iter()
        .filter(|message| message["role"] == "user")
        .filter_map(|message| message["id"].as_str().map(str::to_string))
        .collect();

    let mut report = stark_tools::RestoreReport::default();
    if request.restore_files != Some(false) && !user_message_ids.is_empty() {
        let root = state
            .store
            .chat_workspace_path(&chat_id)
            .await
            .ok()
            .flatten()
            .filter(|path| !path.is_empty())
            .unwrap_or(workspace);
        report = Checkpoint::restore_for_messages(
            std::path::Path::new(&root),
            &chat_id,
            &user_message_ids,
            request.force,
        );
        if !report.ok() && !request.force {
            return Err((
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "Some files changed after this checkpoint. Retry with force=true to overwrite them.",
                    "restore": report.to_json(),
                })),
            ));
        }
    }

    let Some((prompt, removed)) = state
        .store
        .truncate_from_message(&chat_id, &request.message_id)
        .await
        .map_err(|error| bad_request(error.to_string()))?
    else {
        return Err(not_found("Message not found"));
    };
    Ok(Json(json!({
        "ok": true,
        "prompt": prompt,
        "removed": removed,
        "restore": report.to_json(),
    })))
}

#[derive(Debug, Deserialize)]
struct ChatModeRequest {
    #[serde(default)]
    mode: String,
}

async fn chat_mode_handler(
    State(state): State<ServerState>,
    Path(chat_id): Path<String>,
    Json(request): Json<ChatModeRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mode = request.mode.trim();
    if !matches!(mode, "plan" | "build") {
        return Err(bad_request("mode must be 'plan' or 'build'"));
    }
    let current = state
        .store
        .chat_mode(&chat_id)
        .await
        .map_err(|error| bad_request(error.to_string()))?;
    if current.is_none() {
        return Err(not_found("chat not found"));
    }
    if !state
        .store
        .set_chat_mode(&chat_id, mode)
        .await
        .map_err(|error| bad_request(error.to_string()))?
    {
        return Err(not_found("chat not found"));
    }
    Ok(Json(json!({"ok": true, "mode": mode})))
}

async fn chat_state_handler(
    State(state): State<ServerState>,
    Path(chat_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mode = state
        .store
        .chat_mode(&chat_id)
        .await
        .map_err(|error| bad_request(error.to_string()))?;
    match mode {
        Some(mode) => Ok(Json(json!({"chat_id": chat_id, "mode": mode}))),
        None => Err(not_found("chat not found")),
    }
}

#[derive(Debug, Deserialize)]
struct SteerRequest {
    #[serde(default)]
    chat_id: String,
    #[serde(default)]
    message: String,
}

async fn steer_handler(
    State(state): State<ServerState>,
    Json(request): Json<SteerRequest>,
) -> Json<Value> {
    let ok = !request.message.trim().is_empty()
        && state.runs.steer(&request.chat_id, request.message.trim());
    Json(json!({"ok": ok}))
}

async fn follow_up_handler(
    State(state): State<ServerState>,
    Json(request): Json<SteerRequest>,
) -> Json<Value> {
    let ok = !request.message.trim().is_empty()
        && state
            .runs
            .follow_up(&request.chat_id, request.message.trim());
    Json(json!({"ok": ok}))
}

#[derive(Debug, Deserialize)]
struct InboxQuery {
    #[serde(default)]
    chat_id: String,
}

async fn inbox_handler(
    State(state): State<ServerState>,
    Query(query): Query<InboxQuery>,
) -> Json<Value> {
    Json(state.runs.inbox_counts(&query.chat_id))
}

#[derive(Debug, Deserialize)]
struct SnapshotQuery {
    #[serde(default)]
    workspace_path: String,
    limit: Option<usize>,
    #[serde(default)]
    force: bool,
}

async fn snapshot_workspace(state: &ServerState, chat_id: &str, requested: &str) -> Option<String> {
    if !requested.trim().is_empty() {
        return Some(requested.trim().to_string());
    }
    state
        .store
        .chat_workspace_path(chat_id)
        .await
        .ok()
        .flatten()
        .filter(|path| !path.is_empty())
}

async fn snapshots_handler(
    State(state): State<ServerState>,
    Path(chat_id): Path<String>,
    Query(query): Query<SnapshotQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let workspace = snapshot_workspace(&state, &chat_id, &query.workspace_path).await;
    let Some(workspace) = workspace else {
        return Ok(Json(json!({"groups": [], "workspace_path": ""})));
    };
    let limit = query.limit.unwrap_or(100);
    let groups = Checkpoint::list(std::path::Path::new(&workspace), &chat_id, limit);
    Ok(Json(json!({
        "groups": groups,
        "workspace_path": workspace,
        "chat_id": chat_id,
    })))
}

async fn restore_snapshot_handler(
    State(state): State<ServerState>,
    Path((chat_id, snap_id)): Path<(String, String)>,
    Query(query): Query<SnapshotQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let Some(workspace) = snapshot_workspace(&state, &chat_id, &query.workspace_path).await else {
        return Err(bad_request("no workspace_path"));
    };
    if !query.force
        && let Some((path, true)) =
            Checkpoint::inspect_snapshot(std::path::Path::new(&workspace), &chat_id, &snap_id)
    {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({
                "ok": false,
                "message": format!("{path} has changed since this snapshot; retry with force=true"),
            })),
        ));
    }
    let (ok, message) =
        Checkpoint::restore_snapshot(std::path::Path::new(&workspace), &chat_id, &snap_id);
    if !ok {
        return Err(not_found(message));
    }
    Ok(Json(json!({"ok": true, "message": message})))
}

async fn restore_run_handler(
    State(state): State<ServerState>,
    Path((chat_id, run_id)): Path<(String, String)>,
    Query(query): Query<SnapshotQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let Some(workspace) = snapshot_workspace(&state, &chat_id, &query.workspace_path).await else {
        return Err(bad_request("no workspace_path"));
    };
    let (ok, messages) =
        Checkpoint::restore_run(std::path::Path::new(&workspace), &chat_id, &run_id);
    Ok(Json(json!({"ok": ok, "messages": messages})))
}

/// Browser origins that may call the local API. Rune web and loopback dev
/// servers are explicit; any other website is rejected before CORS can
/// advertise the response. Loopback-only local processes are still trusted by
/// the auth middleware.
fn allowed_origins() -> &'static Vec<String> {
    static ORIGINS: OnceLock<Vec<String>> = OnceLock::new();
    ORIGINS.get_or_init(|| {
        std::env::var("STARK_ALLOWED_ORIGINS")
            .ok()
            .map(|value| value.split(',').filter_map(normalize_origin).collect())
            .unwrap_or_default()
    })
}

fn normalize_origin(value: &str) -> Option<String> {
    let value = value.trim().trim_end_matches('/');
    if value.is_empty() || value.eq_ignore_ascii_case("null") {
        return None;
    }
    Some(value.to_ascii_lowercase())
}

fn is_loopback_origin(origin: &str) -> bool {
    let Some(rest) = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or(rest);
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split_once(']').map(|(host, _)| host).unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or(authority)
    };
    host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
}

fn host_only(value: &str) -> String {
    let value = value.trim().to_ascii_lowercase();
    if value.starts_with('[') {
        return value
            .split_once(']')
            .map(|(host, _)| host.trim_start_matches('[').to_string())
            .unwrap_or(value);
    }
    match value.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|character| character.is_ascii_digit()) => {
            host.to_string()
        }
        _ => value,
    }
}

fn allowed_hosts() -> &'static Vec<String> {
    static HOSTS: OnceLock<Vec<String>> = OnceLock::new();
    HOSTS.get_or_init(|| {
        std::env::var("STARK_ALLOWED_HOSTS")
            .ok()
            .map(|value| {
                value
                    .split(',')
                    .map(host_only)
                    .filter(|host| !host.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    })
}

fn loopback_bind() -> bool {
    static LOOPBACK: OnceLock<bool> = OnceLock::new();
    *LOOPBACK.get_or_init(|| {
        let host = std::env::var("STARK_HOST")
            .or_else(|_| std::env::var("STARK_BIND"))
            .unwrap_or_else(|_| "127.0.0.1".to_string());
        let host = host.trim();
        host.eq_ignore_ascii_case("127.0.0.1")
            || host.eq_ignore_ascii_case("localhost")
            || host == "::1"
            || host == "[::1]"
    })
}

fn host_allowed(host_header: &str) -> bool {
    let host = host_only(host_header);
    if host.is_empty() {
        return false;
    }
    if matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1") {
        return true;
    }
    allowed_hosts().iter().any(|allowed| allowed == &host)
}

fn origin_allowed(origin: &str, headers: &HeaderMap) -> bool {
    let Some(origin) = normalize_origin(origin) else {
        return false;
    };
    if allowed_origins()
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(&origin))
        || is_loopback_origin(&origin)
    {
        return true;
    }
    // Same-origin requests (Rune web, custom port) are allowed when the
    // Origin authority matches the Host header.
    let Some(host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let host = host.trim().to_ascii_lowercase();
    origin == format!("http://{host}") || origin == format!("https://{host}")
}

/// Rejects browser requests from unknown origins and, when the server is
/// bound to loopback, requests whose Host is not a local name. This closes the
/// DNS-rebinding / malicious-webpage path to the API without breaking Rune
/// web or a loopback dev server.
async fn origin_guard(
    request: axum::extract::Request,
    next: middleware::Next,
) -> axum::response::Response {
    let headers = request.headers();
    if let Some(origin) = headers.get(ORIGIN).and_then(|value| value.to_str().ok())
        && !origin_allowed(origin, headers)
    {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "Origin is not allowed to call the local API"})),
        )
            .into_response();
    }
    if loopback_bind()
        && let Some(host) = headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
        && !host_allowed(host)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Host is not a loopback name"})),
        )
            .into_response();
    }
    next.run(request).await
}

async fn auth_middleware(
    State(state): State<ServerState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    request: axum::extract::Request,
    next: middleware::Next,
) -> axum::response::Response {
    // Only API calls require the pairing token. The server no longer serves
    // a browser shell; Rune connects directly to the API.
    let path = request.uri().path();
    if !path.starts_with("/api/") {
        return next.run(request).await;
    }
    let local = address.ip().is_loopback();
    if local || request.method() == axum::http::Method::OPTIONS {
        return next.run(request).await;
    }
    let token = request
        .headers()
        .get("x-api-token")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if state.pairing.authorize(token) {
        return next.run(request).await;
    }
    // Bad tokens are cheap to guess and this endpoint is reachable from the
    // LAN. A small fixed delay caps brute-force attempts without affecting
    // legitimate clients (which send the right token on the first try).
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": "Pairing required"})),
    )
        .into_response()
}

async fn pairing_handler(
    State(state): State<ServerState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !address.ip().is_loopback() {
        return Err((StatusCode::FORBIDDEN, Json(json!({"error": "Local only"}))));
    }
    Ok(Json(json!({"token": state.pairing.token()})))
}

#[derive(Debug, Deserialize)]
struct IssuePairingRequest {
    #[serde(default)]
    name: String,
}

async fn pairing_devices_handler(
    State(state): State<ServerState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    Ok(Json(json!({ "devices": state.pairing.devices() })))
}

async fn pairing_issue_handler(
    State(state): State<ServerState>,
    Json(request): Json<IssuePairingRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let issued = state.pairing.issue(&request.name);
    Ok(Json(json!({
        "id": issued.device.id,
        "name": issued.device.name,
        "token": issued.token,
        "created_at": issued.device.created_at,
    })))
}

async fn pairing_revoke_handler(
    State(state): State<ServerState>,
    Path(device_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let removed = state.pairing.revoke(&device_id);
    Ok(Json(json!({ "ok": removed, "id": device_id })))
}

async fn pairing_rotate_handler(
    State(state): State<ServerState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !address.ip().is_loopback() {
        return Err((StatusCode::FORBIDDEN, Json(json!({"error": "Local only"}))));
    }
    Ok(Json(json!({"token": state.pairing.rotate()})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_opencode_usage_meters() {
        let value = json!({
            "usage": {
                "rolling": {"status": "ok", "percent": 42, "resetsAt": "2099-01-01T00:00:00Z"},
                "weekly": {"status": "ok", "percent": 12.5, "resetsAt": "2099-01-02T00:00:00Z"},
                "monthly": {"status": "rate-limited", "percent": 100, "resetsAt": "2099-01-03T00:00:00Z"}
            }
        });
        let usage = parse_opencode_usage(&value);
        assert_eq!(usage["rolling"]["percent"], 42.0);
        assert_eq!(usage["weekly"]["percent"], 12.5);
        assert_eq!(usage["monthly"]["status"], "rate-limited");
        assert!(parse_opencode_usage(&json!({})).is_null());
        assert!(parse_opencode_usage(&json!({"usage": {}})).is_null());
    }

    async fn group_provider_response(
        axum::extract::State(fails): axum::extract::State<bool>,
        Json(_body): Json<Value>,
    ) -> axum::response::Response {
        use axum::body::Body;
        use axum::http::header;
        use axum::response::Response;

        if fails {
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("mock provider failure"))
                .unwrap();
        }
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Group item complete\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        );
        Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(body))
            .unwrap()
    }

    async fn group_provider(fails: bool) -> String {
        use axum::Router;
        use axum::routing::post;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/chat/completions", post(group_provider_response))
            .with_state(fails);
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{address}")
    }

    fn write_mock_provider_config(root: &std::path::Path, base_url: &str) {
        let dir = root.join(".deep_agents").join("config");
        std::fs::create_dir_all(&dir).unwrap();
        let config = json!({
            "version": 1,
            "active": "opencode",
            "providers": {
                "opencode": {
                    "label": "Mock",
                    "base_url": base_url,
                    "key_env": "STARK_TEST_MOCK_KEY",
                    "default_model": "mock",
                    "models": {
                        "mock": {"id": "mock", "label": "Mock", "reasoning": false}
                    }
                }
            },
            "matrix": {
                "opencode": {
                    "orchestrator": {"model": "mock", "thinking": "none"},
                    "workspace": {"model": "mock", "thinking": "none"},
                    "specialist": {"model": "mock", "thinking": "none"}
                }
            }
        });
        std::fs::write(
            dir.join("providers.json"),
            serde_json::to_string_pretty(&config).unwrap(),
        )
        .unwrap();
    }

    async fn group_test_state(root: &std::path::Path, base_url: String) -> ServerState {
        write_mock_provider_config(root, &base_url);
        let store = ChatStore::open(root).await.unwrap();
        let provider = Provider::new(ProviderConfig {
            mode: "deepseek".to_string(),
            base_url,
            api_key: "test".to_string(),
            model: "mock".to_string(),
        })
        .unwrap();
        let config = Arc::new(ConfigStore::open(root));
        let mut provider_map = HashMap::new();
        provider_map.insert("opencode".to_string(), provider.clone());
        ServerState {
            store,
            provider,
            config,
            providers: Arc::new(provider_map),
            provider_health: Arc::new(Mutex::new(HashMap::new())),
            questions: Arc::new(QuestionBroker::new()),
            runs: Arc::new(RunManager::default()),
            registry: WorkspaceRegistry::new(root),
            pairing: Arc::new(PairingStore::new(root)),
            ui_state_path: root.join(".deep_agents").join("ui_state.json"),
            harness_root: root.to_path_buf(),
            voice: None,
        }
    }

    async fn group_with_items(dir: &std::path::Path, items: Value) -> GroupStore {
        let groups = GroupStore::new(dir.join("projects"));
        groups
            .create(
                &json!({
                    "id": "mock-run",
                    "goal": "Do things",
                    "milestones": [{
                        "id": "m1",
                        "title": "Build",
                        "items": items,
                    }]
                }),
                "_general",
            )
            .await
            .unwrap();
        groups
    }

    async fn group_with_one_ready_item(root: &std::path::Path) -> GroupStore {
        let groups = GroupStore::new(root.join("projects"));
        groups.create(&json!({
            "id": "mock-run",
            "goal": "Create a small deliverable",
            "milestones": [{
                "id": "m1",
                "title": "Build",
                "items": [{"id": "i1", "title": "Write it", "detail": "Create the deliverable", "acceptance": ["It exists"]}]
            }]
        }), "_general").await.unwrap();
        groups
    }

    #[tokio::test]
    async fn ask_pending_route_exposes_chat_id_for_voice_routing() {
        let dir = tempfile::tempdir().unwrap();
        let state = group_test_state(dir.path(), group_provider(false).await).await;
        let broker = state.questions.clone();
        let ask = tokio::spawn(async move {
            broker
                .ask(
                    "voice-chat-1",
                    vec![serde_json::json!({"id": "q1", "question": "Go?"})],
                    30,
                )
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let Json(body) = pending_handler(State(state.clone())).await;
        let batches = body["batches"].as_array().cloned().unwrap_or_default();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0]["chat_id"], serde_json::json!("voice-chat-1"));
        let batch_id = batches[0]["batch_id"].as_str().unwrap().to_string();
        assert!(state.questions.answer(
            &batch_id,
            vec![serde_json::json!({"id": "q1", "custom": "yes"})]
        ));
        let answers = ask.await.unwrap().expect("ask resolves");
        assert_eq!(answers[0]["custom"], serde_json::json!("yes"));
    }

    #[tokio::test]
    async fn group_item_with_needs_approval_tag_parks_without_running() {
        let dir = tempfile::tempdir().unwrap();
        let groups = group_with_items(
            dir.path(),
            json!([{"id": "i1", "title": "Risky", "detail": "Do it", "acceptance": [], "tags": ["needs-approval"]}]),
        )
        .await;
        let item = groups.claim_next_ready("mock-run").await.unwrap().unwrap();
        let state = group_test_state(dir.path(), group_provider(false).await).await;
        run_group_item(state, groups.clone(), "mock-run".to_string(), item).await;
        let fetched = groups.item("mock-run", "i1").await.unwrap();
        assert_eq!(fetched["status"], json!("awaiting_approval"));
    }

    #[tokio::test]
    async fn group_item_with_failing_criterion_finishes_failed() {
        let dir = tempfile::tempdir().unwrap();
        let groups = group_with_items(
            dir.path(),
            json!([{"id": "i1", "title": "Write it", "detail": "Create the deliverable", "acceptance": [{"kind": "file_exists", "path": "proof.txt"}]}]),
        )
        .await;
        let item = groups.claim_next_ready("mock-run").await.unwrap().unwrap();
        let state = group_test_state(dir.path(), group_provider(false).await).await;
        run_group_item(state, groups.clone(), "mock-run".to_string(), item).await;
        let detail = groups.detail("mock-run", 0).await.unwrap();
        assert_eq!(detail["items"][0]["status"], "failed");
        assert!(
            detail["items"][0]["error"]
                .as_str()
                .unwrap_or_default()
                .contains("Acceptance verification failed")
        );
    }

    #[tokio::test]
    async fn group_item_with_passing_criterion_finishes_done() {
        let dir = tempfile::tempdir().unwrap();
        let groups = group_with_items(
            dir.path(),
            json!([{"id": "i1", "title": "Write it", "detail": "Create the deliverable", "acceptance": [{"kind": "file_exists", "path": "proof.txt"}]}]),
        )
        .await;
        // The runner evaluates acceptance against the project folder.
        std::fs::write(
            dir.path()
                .join("projects")
                .join("mock-run")
                .join("proof.txt"),
            "deliverable",
        )
        .unwrap();
        let item = groups.claim_next_ready("mock-run").await.unwrap().unwrap();
        let state = group_test_state(dir.path(), group_provider(false).await).await;
        run_group_item(state, groups.clone(), "mock-run".to_string(), item).await;
        let detail = groups.detail("mock-run", 0).await.unwrap();
        assert_eq!(detail["items"][0]["status"], "done");
        assert!(
            detail["items"][0]["result"]
                .as_str()
                .unwrap_or_default()
                .contains("Verification:")
        );
    }

    #[tokio::test]
    async fn group_item_with_manual_criterion_parks_for_owner() {
        let dir = tempfile::tempdir().unwrap();
        let groups = group_with_items(
            dir.path(),
            json!([{"id": "i1", "title": "Polish", "detail": "Make it pretty", "acceptance": [{"kind": "manual", "question": "Does it look right?"}]}]),
        )
        .await;
        let item = groups.claim_next_ready("mock-run").await.unwrap().unwrap();
        let state = group_test_state(dir.path(), group_provider(false).await).await;
        run_group_item(state, groups.clone(), "mock-run".to_string(), item).await;
        let fetched = groups.item("mock-run", "i1").await.unwrap();
        assert_eq!(fetched["status"], json!("awaiting_approval"));
    }

    #[tokio::test]
    async fn group_runner_executes_a_ready_item_in_a_child_chat_with_mock_provider() {
        let dir = tempfile::tempdir().unwrap();
        let groups = group_with_one_ready_item(dir.path()).await;
        let item = groups.claim_next_ready("mock-run").await.unwrap().unwrap();
        let state = group_test_state(dir.path(), group_provider(false).await).await;
        run_group_item(state.clone(), groups.clone(), "mock-run".to_string(), item).await;

        let detail = groups.detail("mock-run", 30).await.unwrap();
        assert_eq!(detail["items"][0]["status"], "done");
        assert!(
            detail["items"][0]["result"]
                .as_str()
                .unwrap()
                .contains("Group item complete")
        );
        assert!(
            detail["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|event| event["kind"] == "item.done")
        );
        let chats = state.store.list_chats("_general").await;
        assert_eq!(chats.len(), 1);
        assert!(
            chats[0]["title"]
                .as_str()
                .unwrap()
                .starts_with("Group: Write it")
        );
        let chat_id = chats[0]["id"].as_str().unwrap();
        let expected = dir
            .path()
            .join("projects")
            .join("mock-run")
            .to_string_lossy()
            .to_string();
        let actual = state.store.chat_workspace_path(chat_id).await.unwrap();
        assert_eq!(actual.as_deref(), Some(expected.as_str()));
    }

    #[tokio::test]
    async fn group_runner_records_a_mock_provider_failure() {
        let dir = tempfile::tempdir().unwrap();
        let groups = group_with_one_ready_item(dir.path()).await;
        let item = groups.claim_next_ready("mock-run").await.unwrap().unwrap();
        let state = group_test_state(dir.path(), group_provider(true).await).await;
        run_group_item(state, groups.clone(), "mock-run".to_string(), item).await;

        let detail = groups.detail("mock-run", 30).await.unwrap();
        assert_eq!(detail["items"][0]["status"], "failed");
        assert!(
            detail["items"][0]["error"]
                .as_str()
                .unwrap()
                .contains("provider")
        );
        assert!(
            detail["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|event| event["kind"] == "item.failed")
        );
    }

    #[test]
    fn lists_files_skips_metadata_and_keeps_dirs_first() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("src").join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(root.join("README.md"), "hello").unwrap();
        std::fs::create_dir(root.join("node_modules")).unwrap();
        std::fs::write(root.join("node_modules").join("x.js"), "skip").unwrap();
        std::fs::create_dir(root.join(".agent-memory")).unwrap();
        std::fs::write(root.join(".agent-memory").join("project.md"), "memory").unwrap();

        let files = list_files(root.to_string_lossy().as_ref(), 8);
        let paths: Vec<&str> = files
            .iter()
            .map(|file| file["path"].as_str().unwrap())
            .collect();
        assert_eq!(paths[0], "src");
        assert!(paths.contains(&"src/main.rs"));
        assert!(paths.contains(&"README.md"));
        assert!(paths.contains(&".agent-memory/project.md"));
        assert!(!paths.contains(&"node_modules"));
    }

    #[tokio::test]
    async fn durable_agent_run_completes_and_notifies_parent() {
        use axum::Router;
        use axum::body::Body;
        use axum::http::header;
        use axum::routing::post;

        async fn mock_provider(Json(_body): Json<Value>) -> Response {
            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"Child done\"}}]}

",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}

",
                "data: [DONE]

"
            );
            Response::builder()
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from(body))
                .unwrap()
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route("/chat/completions", post(mock_provider)),
            )
            .await
            .unwrap();
        });

        let dir = tempfile::tempdir().unwrap();
        let state = group_test_state(dir.path(), format!("http://{address}")).await;
        let agents = AgentStore::open(dir.path()).await.unwrap();
        let parent_chat = state
            .store
            .create_chat(
                "_general",
                dir.path().to_string_lossy().as_ref(),
                "Parent",
                "mock",
            )
            .await
            .unwrap();
        let child_chat = state
            .store
            .create_child_chat(
                "_general",
                dir.path().to_string_lossy().as_ref(),
                "Child",
                "mock",
                parent_chat["id"].as_str().unwrap(),
            )
            .await
            .unwrap();
        let parent = agents
            .create_agent(
                "orchestrator",
                "orchestrator:test",
                None,
                parent_chat["id"].as_str().unwrap(),
                "_general",
                "orchestrator",
                "Orchestrator",
                "Orch",
                "mock",
                0,
            )
            .await
            .unwrap();
        let child = agents
            .create_agent(
                "workspace",
                "workspace:_general:developer",
                Some(&parent.id),
                child_chat["id"].as_str().unwrap(),
                "_general",
                "developer",
                "Developer",
                "Dev",
                "mock",
                1,
            )
            .await
            .unwrap();
        agents
            .enqueue(
                Some(&parent.id),
                &child.id,
                "spawn",
                "do the work",
                "trigger",
                json!({}),
            )
            .await
            .unwrap();
        assert!(agents.claim(&child.id).await.unwrap());
        assert!(spawn_agent_run(state.clone(), agents.clone(), child.clone()).await);

        for _ in 0..100 {
            let status = agents.get(&child.id).await.unwrap().unwrap().status;
            if status == "completed" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let finished = agents.get(&child.id).await.unwrap().unwrap();
        assert_eq!(finished.status, "completed", "{:?}", finished.result);
        assert!(finished.result.contains("Child done"));
        let mut parent_result = false;
        for _ in 0..100 {
            let parent_messages = agents.messages_all(&parent.id).await.unwrap();
            if parent_messages
                .iter()
                .any(|message| message["kind"] == "result")
            {
                parent_result = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(parent_result);
    }

    #[tokio::test]
    async fn follow_up_run_injects_but_does_not_persist_internal_delegation_result() {
        use axum::body::Body;
        use axum::http::header;
        use axum::response::Response;
        use axum::routing::post;
        use axum::{Json, Router};

        async fn mock_provider(
            axum::extract::State(requests): axum::extract::State<Arc<Mutex<Vec<Value>>>>,
            Json(body): Json<Value>,
        ) -> Response {
            requests.lock().unwrap().push(body);
            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"Woke\"}}]}

",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}

",
                "data: [DONE]

"
            );
            Response::builder()
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from(body))
                .unwrap()
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/chat/completions", post(mock_provider))
            .with_state(requests.clone());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let dir = tempfile::tempdir().unwrap();
        let store = ChatStore::open(dir.path()).await.unwrap();
        let base_url = format!("http://{address}");
        write_mock_provider_config(dir.path(), &base_url);
        let provider = Provider::new(ProviderConfig {
            mode: "deepseek".to_string(),
            base_url,
            api_key: "test".to_string(),
            model: "mock".to_string(),
        })
        .unwrap();
        let config = Arc::new(ConfigStore::open(dir.path()));
        let mut provider_map = HashMap::new();
        provider_map.insert("opencode".to_string(), provider.clone());
        let providers = Arc::new(provider_map);
        let state = ServerState {
            store: store.clone(),
            provider,
            config,
            providers,
            provider_health: Arc::new(Mutex::new(HashMap::new())),
            questions: Arc::new(QuestionBroker::new()),
            runs: Arc::new(RunManager::default()),
            registry: WorkspaceRegistry::new(dir.path()),
            pairing: Arc::new(PairingStore::new(dir.path())),
            ui_state_path: dir.path().join("ui_state.json"),
            harness_root: dir.path().to_path_buf(),
            voice: None,
        };
        spawn_follow_up_run(
            state.clone(),
            RunTarget {
                chat_id: "wake-chat".to_string(),
                workspace_id: "_general".to_string(),
                workspace: dir.path().to_path_buf(),
                scope: "orchestrator".to_string(),
                role: Some("Orchestrator".to_string()),
                model: "mock".to_string(),
                temperature: 0.0,
                depth: 0,
                agent_id: None,
                effective: effective_for(&state, "orchestrator", "orchestrator"),
            },
            "[DELEGATION RESULT] job done".to_string(),
        );

        for _ in 0..100 {
            let history = store.history("wake-chat", 10).await.unwrap_or_default();
            if history.iter().any(|message| message.content == "Woke") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let history = store.history("wake-chat", 10).await.unwrap();
        assert!(
            !history
                .iter()
                .any(|message| message.content == "[DELEGATION RESULT] job done")
        );
        assert!(history.iter().any(|message| message.content == "Woke"));
        assert!(requests.lock().unwrap().iter().any(|request| {
            request["messages"].as_array().is_some_and(|messages| {
                messages.iter().any(|message| {
                    message["content"].as_str() == Some("[DELEGATION RESULT] job done")
                })
            })
        }));
    }

    #[tokio::test]
    async fn state_hydration_includes_a_persisted_plan() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join(".deep_agents").join("tasks");
        std::fs::create_dir_all(&tasks).unwrap();
        std::fs::write(
            tasks.join("task_chat-1.json"),
            serde_json::to_string(&json!({
                "plan": {"agent": [{"step": "inspect", "status": "in_progress"}]}
            }))
            .unwrap(),
        )
        .unwrap();

        let mut value = json!({});
        hydrate_plan(dir.path(), &mut value, Some("chat-1")).await;
        assert_eq!(value["plan"]["agent"][0]["step"], "inspect");
    }

    #[test]
    fn group_owner_member_selects_the_agent() {
        let dir = tempfile::tempdir().unwrap();
        let registry = AgentRegistry::open(dir.path());
        let detail = json!({
            "members": [
                {"name": "Nova", "display": "Nova", "kind": "ui"},
                {"name": "Ops", "display": "Ops", "kind": "devops"}
            ]
        });
        assert_eq!(
            group_item_runtime(&registry, &json!({"owner_member": "UI"}), &detail)
                .unwrap()
                .key,
            "ui"
        );
        assert_eq!(
            group_item_runtime(&registry, &json!({"owner_member": "research"}), &detail)
                .unwrap()
                .key,
            "general"
        );
        assert_eq!(
            group_item_runtime(&registry, &json!({"owner_member": "Nova"}), &detail)
                .unwrap()
                .key,
            "ui"
        );
        assert_eq!(
            group_item_runtime(&registry, &json!({"owner_member": "ops"}), &detail)
                .unwrap()
                .key,
            "devops"
        );
        assert!(
            group_item_runtime(&registry, &json!({"owner_member": "unknown"}), &detail).is_err()
        );
        assert_eq!(
            group_item_runtime(&registry, &json!({}), &detail)
                .unwrap()
                .key,
            "developer"
        );
    }

    #[tokio::test]
    async fn custom_agent_crud_preview_and_delete() {
        let dir = tempfile::tempdir().unwrap();
        let state = group_test_state(dir.path(), "http://127.0.0.1:9".to_string()).await;
        let file: AgentFile = serde_json::from_value(json!({
            "version": 2,
            "display": "Backend",
            "description": "Rust backend work",
            "access": "write",
            "prompt": {"profile": "coding", "persona": "You own backend-rs."},
            "tools": ["read", "edit", "glob", "grep"],
            "skills": [],
            "mcp": [],
            "delegation": {"delegatable": true, "can_delegate": false, "can_spawn": false},
            "ui": {"visible": true, "order": 25}
        }))
        .unwrap();
        let saved = save_agent_handler(
            State(state.clone()),
            Path("backend".to_string()),
            Json(file),
        )
        .await
        .unwrap();
        assert_eq!(saved.0["key"], "backend");
        assert_eq!(saved.0["effective"]["access"], "write");
        assert_eq!(saved.0["effective"]["prompt_profile"], "coding");

        let listed = agents_config_handler(State(state.clone())).await;
        let entry = listed.0["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["key"] == "backend")
            .expect("custom agent in list");
        assert_eq!(entry["builtin"], false);
        assert_eq!(entry["deletable"], true);
        assert_eq!(entry["valid"], true);

        let preview = preview_agent_handler(
            State(state.clone()),
            Path("backend".to_string()),
            Json(PreviewRequest {
                draft: None,
                workspace_path: dir.path().to_string_lossy().to_string(),
                mode: "build".to_string(),
            }),
        )
        .await
        .unwrap();
        let prompt = preview.0["system_prompt"].as_str().unwrap();
        assert!(prompt.contains("You own backend-rs."));
        assert!(preview.0["tools"]["direct"].as_array().is_some());
        assert!(preview.0["prompt_tokens"].as_u64().unwrap() > 0);

        let deleted = delete_agent_handler(State(state.clone()), Path("backend".to_string()))
            .await
            .unwrap();
        assert_eq!(deleted.0["deleted"], true);
        let after = agent_config_handler(
            State(state.clone()),
            Path("backend".to_string()),
            Query(AgentQuery { tier: None }),
        )
        .await;
        assert!(after.is_err());
    }

    #[tokio::test]
    async fn legacy_alias_file_is_listed_as_its_builtin_and_reset_removes_it() {
        let dir = tempfile::tempdir().unwrap();
        let state = group_test_state(dir.path(), "http://127.0.0.1:9".to_string()).await;
        let agents_dir = dir.path().join(".deep_agents/config/agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("coder.json"),
            r#"{"display":"Legacy Coder"}"#,
        )
        .unwrap();
        let listed = agents_config_handler(State(state.clone())).await;
        let keys: Vec<String> = listed.0["agents"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|entry| entry["key"].as_str().map(str::to_string))
            .collect();
        assert!(!keys.contains(&"coder".to_string()), "{keys:?}");
        let developer = listed.0["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["key"] == "developer")
            .unwrap();
        assert_eq!(developer["display"], "Legacy Coder");
        let _ = delete_agent_handler(State(state.clone()), Path("developer".to_string()))
            .await
            .unwrap();
        assert!(!agents_dir.join("coder.json").exists());
        let listed = agents_config_handler(State(state)).await;
        let developer = listed.0["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["key"] == "developer")
            .unwrap();
        assert_eq!(developer["display"], "Developer");
    }

    #[tokio::test]
    async fn duplicating_builtins_produces_safe_custom_agents() {
        let dir = tempfile::tempdir().unwrap();
        let state = group_test_state(dir.path(), "http://127.0.0.1:9".to_string()).await;
        let developer = duplicate_agent_handler(
            State(state.clone()),
            Path("developer".to_string()),
            Json(DuplicateRequest {
                key: "backend2".to_string(),
                display: None,
            }),
        )
        .await
        .unwrap();
        let tools = developer.0["effective"]["tools"].as_array().unwrap();
        assert!(!tools.iter().any(|tool| tool == "execute"));
        assert_eq!(developer.0["effective"]["code_mode"], "off");
        assert_eq!(developer.0["effective"]["prompt_profile"], "coding");

        let house = duplicate_agent_handler(
            State(state.clone()),
            Path("house".to_string()),
            Json(DuplicateRequest {
                key: "house2".to_string(),
                display: None,
            }),
        )
        .await
        .unwrap();
        assert_eq!(house.0["effective"]["access"], "read");
        let persona = house.0["effective"]["persona"].as_str().unwrap_or("");
        assert!(persona.contains("You are the House agent"), "{persona}");
    }

    #[tokio::test]
    async fn custom_agent_rejects_unsafe_definitions() {
        let dir = tempfile::tempdir().unwrap();
        let state = group_test_state(dir.path(), "http://127.0.0.1:9".to_string()).await;
        let unknown: AgentFile = serde_json::from_value(json!({
            "version": 2,
            "tools": ["read", "not_a_tool"]
        }))
        .unwrap();
        let result =
            save_agent_handler(State(state.clone()), Path("bad".to_string()), Json(unknown)).await;
        assert!(result.is_err());

        let read_only_edit: AgentFile = serde_json::from_value(json!({
            "version": 2,
            "access": "read",
            "tools": ["read", "edit"]
        }))
        .unwrap();
        let result = save_agent_handler(
            State(state.clone()),
            Path("readonly".to_string()),
            Json(read_only_edit),
        )
        .await;
        assert!(result.is_err());
        assert!(state.config.agent("bad").is_none());
        assert!(state.config.agent("readonly").is_none());
    }

    #[test]
    fn reads_files_and_blocks_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("a.txt"), "alpha").unwrap();
        std::fs::write(dir.path().join("outside.txt"), "nope").unwrap();

        let content = read_file(root.to_string_lossy().as_ref(), "a.txt").unwrap();
        assert_eq!(content["content"], "alpha");

        let error = read_file(root.to_string_lossy().as_ref(), "../outside.txt").unwrap_err();
        assert_eq!(error.0, StatusCode::BAD_REQUEST);

        let missing = read_file(root.to_string_lossy().as_ref(), "missing.txt").unwrap_err();
        assert_eq!(missing.0, StatusCode::NOT_FOUND);
    }

    #[test]
    fn last_state_is_capped_and_forgettable() {
        let runs = RunManager::default();
        for index in 0..(MAX_CACHED_STATES + 10) {
            let chat = format!("chat-{index}");
            let state = stark_contract::AppState {
                client_request: chat.clone(),
                ..Default::default()
            };
            runs.finish(&chat, Some(state));
        }
        assert_eq!(runs.last_state.len(), MAX_CACHED_STATES);
        assert!(runs.last_state.get("chat-0").is_none());
        assert!(
            runs.last_state
                .get(&format!("chat-{}", MAX_CACHED_STATES + 9))
                .is_some()
        );
        runs.forget(&format!("chat-{}", MAX_CACHED_STATES + 9));
        assert!(
            runs.last_state
                .get(&format!("chat-{}", MAX_CACHED_STATES + 9))
                .is_none()
        );
        assert_eq!(runs.last_state.len(), MAX_CACHED_STATES - 1);
    }

    #[test]
    fn run_admission_enforces_the_global_cap_and_releases_on_finish() {
        let runs = RunManager {
            max_runs: 1,
            ..RunManager::default()
        };
        let claim = |chat: &str| {
            let (tx, _rx) = broadcast::channel(4);
            runs.claim(
                chat,
                RunClaim {
                    run_id: format!("run-{chat}"),
                    latest: Arc::new(Mutex::new(None)),
                    cancel: CancellationToken::new(),
                    tx,
                    steering: Arc::new(Mutex::new(VecDeque::new())),
                    follow_up: Arc::new(Mutex::new(VecDeque::new())),
                    workspace_path: "C:/workspace".to_string(),
                },
            )
        };
        assert_eq!(claim("chat-a"), Ok(()));
        // Same chat is rejected first, even when the machine cap is free.
        assert_eq!(claim("chat-a"), Err(ClaimError::AlreadyRunning));
        // A second chat hits the machine-wide cap (Codex's AgentLimitReached).
        assert_eq!(claim("chat-b"), Err(ClaimError::Limit { max: 1 }));
        runs.finish("chat-a", None);
        assert_eq!(claim("chat-b"), Ok(()));
    }

    #[test]
    fn cancel_all_signals_every_claimed_run() {
        let runs = RunManager::default();
        let first = CancellationToken::new();
        let second = CancellationToken::new();
        for (chat, cancel) in [("chat-a", first.clone()), ("chat-b", second.clone())] {
            let (tx, _rx) = broadcast::channel(4);
            runs.claim(
                chat,
                RunClaim {
                    run_id: format!("run-{chat}"),
                    latest: Arc::new(Mutex::new(None)),
                    cancel,
                    tx,
                    steering: Arc::new(Mutex::new(VecDeque::new())),
                    follow_up: Arc::new(Mutex::new(VecDeque::new())),
                    workspace_path: "C:/workspace".to_string(),
                },
            )
            .unwrap();
        }
        assert_eq!(runs.cancel_all(), 2);
        assert!(first.is_cancelled());
        assert!(second.is_cancelled());
    }

    #[test]
    fn shutdown_closes_admission() {
        let runs = RunManager::default();
        assert_eq!(runs.cancel_all(), 0);
        let (tx, _rx) = broadcast::channel(4);
        let error = runs
            .claim(
                "late-chat",
                RunClaim {
                    run_id: "run-late".to_string(),
                    latest: Arc::new(Mutex::new(None)),
                    cancel: CancellationToken::new(),
                    tx,
                    steering: Arc::new(Mutex::new(VecDeque::new())),
                    follow_up: Arc::new(Mutex::new(VecDeque::new())),
                    workspace_path: "C:/workspace".to_string(),
                },
            )
            .unwrap_err();
        assert_eq!(error, ClaimError::ShuttingDown);
    }

    #[tokio::test]
    async fn shutdown_interrupts_background_sleep() {
        let shutdown = CancellationToken::new();
        shutdown.cancel();
        assert!(
            !sleep_or_shutdown(std::time::Duration::from_secs(3600), &shutdown).await,
            "an already-cancelled token must wake the sleeper"
        );
    }

    fn warning_line(ordinal: u64, message: &str) -> stark_store::rollout::RolloutLine {
        serde_json::from_value(json!({
            "timestamp": "2026-01-01T00:00:00.000Z",
            "ordinal": ordinal,
            "type": "event_msg",
            "payload": {"type": "warning", "message": message},
        }))
        .unwrap()
    }

    #[test]
    fn opencode_event_envelope_carries_id_type_and_created() {
        let value = opencode_event_value(ServerEvent::new("delta", json!({"kind": "text"})));
        assert!(value["id"].as_str().unwrap().starts_with("evt_"), "{value}");
        assert_eq!(value["type"], "delta");
        assert!(value["created"].as_i64().unwrap() > 0);
        assert_eq!(value["data"]["kind"], "text");
    }

    #[test]
    fn durable_log_event_uses_rollout_ordinal_as_seq() {
        let line = warning_line(7, "durable");
        let value = durable_event_value("chat-log", &line).unwrap();
        assert_eq!(value["id"], "evt_chat-log_7");
        assert_eq!(value["type"], "warning");
        assert_eq!(value["durable"]["aggregateID"], "chat-log");
        assert_eq!(value["durable"]["seq"], 7);
        assert_eq!(value["durable"]["version"], 1);
        assert_eq!(value["data"]["message"], "durable");
    }

    #[test]
    fn log_values_use_an_exclusive_cursor_and_respect_type_filters() {
        let lines = vec![
            warning_line(1, "one"),
            warning_line(2, "two"),
            warning_line(3, "three"),
        ];
        let types = Some(Arc::new(HashSet::from(["warning".to_string()])));
        let mut cursor = Some(1);
        let values = log_values("chat-log", &mut cursor, lines, &types);
        assert_eq!(values.len(), 2);
        assert_eq!(values[0]["durable"]["seq"], 2);
        assert_eq!(values[1]["durable"]["seq"], 3);
        assert_eq!(cursor, Some(3));
    }

    #[test]
    fn log_values_advance_the_cursor_past_filtered_events() {
        let lines = vec![warning_line(1, "one"), warning_line(2, "two")];
        let types = Some(Arc::new(HashSet::from(["error".to_string()])));
        let mut cursor = None;
        let values = log_values("chat-log", &mut cursor, lines, &types);
        assert!(values.is_empty());
        assert_eq!(cursor, Some(2));
    }

    #[test]
    fn log_synced_marker_matches_the_opencode_shape() {
        let value = log_synced_value("chat-log", Some(9));
        assert_eq!(value["type"], "log.synced");
        assert_eq!(value["aggregateID"], "chat-log");
        assert_eq!(value["seq"], 9);
        let empty = log_synced_value("chat-log", None);
        assert!(empty.get("seq").is_none());
    }

    #[tokio::test]
    async fn durable_log_replays_then_follows_live_appends() {
        use stark_store::rollout::{EventMsgPayload, RolloutItem, RolloutWriter};

        let dir = tempfile::tempdir().unwrap();
        let mut writer = RolloutWriter::open(dir.path(), "chat-follow", "ws", dir.path(), "model")
            .await
            .unwrap();
        // The writer's session_meta is ordinal 0; this is ordinal 1.
        writer
            .append(RolloutItem::EventMsg {
                payload: EventMsgPayload::Warning {
                    message: "replayed".to_string(),
                },
            })
            .await
            .unwrap();

        let mut values = spawn_log_values(
            dir.path().to_path_buf(),
            "chat-follow".to_string(),
            Some(0),
            true,
            None,
        );
        let replayed = tokio::time::timeout(std::time::Duration::from_secs(2), values.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(replayed["durable"]["seq"], 1);
        assert_eq!(replayed["data"]["message"], "replayed");

        let synced = tokio::time::timeout(std::time::Duration::from_secs(2), values.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(synced["type"], "log.synced");
        assert_eq!(synced["seq"], 1);

        let ordinal = writer
            .append(RolloutItem::EventMsg {
                payload: EventMsgPayload::Warning {
                    message: "live".to_string(),
                },
            })
            .await
            .unwrap();
        let live = tokio::time::timeout(std::time::Duration::from_secs(2), values.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(live["durable"]["seq"], ordinal);
        assert_eq!(live["data"]["message"], "live");
    }

    fn turn_complete_line(ordinal: u64) -> stark_store::rollout::RolloutLine {
        serde_json::from_value(json!({
            "timestamp": "2026-01-01T00:00:01.000Z",
            "ordinal": ordinal,
            "type": "event_msg",
            "payload": {
                "type": "turn_complete",
                "turn_id": "turn-1",
                "last_agent_message": "done"
            },
        }))
        .unwrap()
    }

    #[test]
    fn connected_event_carries_chat_running_seq_and_run_id() {
        let value = connected_event("chat-live", true, 42, Some("run-1"));
        assert_eq!(value["type"], "server.connected");
        assert!(value["id"].as_str().unwrap().starts_with("evt_"));
        assert_eq!(value["data"]["chat_id"], "chat-live");
        assert_eq!(value["data"]["running"], true);
        assert_eq!(value["data"]["seq"], 42);
        assert_eq!(value["data"]["run_id"], "run-1");

        let idle = connected_event("chat-idle", false, 0, None);
        assert_eq!(idle["data"]["running"], false);
        assert!(idle["data"].get("run_id").is_none());
    }

    #[test]
    fn lifecycle_events_bypass_type_filters() {
        assert!(is_lifecycle_event("done"));
        assert!(is_lifecycle_event("error"));
        assert!(is_lifecycle_event("turn_complete"));
        assert!(!is_lifecycle_event("delta"));

        let types = Some(Arc::new(HashSet::from(["delta".to_string()])));
        let lines = vec![warning_line(1, "hidden"), turn_complete_line(2)];
        let mut cursor = Some(0);
        let values = log_values("chat-log", &mut cursor, lines, &types);
        assert_eq!(values.len(), 1);
        assert_eq!(values[0]["type"], "turn_complete");
        assert_eq!(values[0]["durable"]["seq"], 2);
        assert_eq!(cursor, Some(2));
    }

    #[test]
    fn state_prefers_live_run_state_over_the_last_finished_state() {
        let runs = RunManager::default();
        let latest = Arc::new(Mutex::new(Some(AppState {
            active_node: "agent".to_string(),
            ..AppState::default()
        })));
        let (tx, _rx) = broadcast::channel(4);
        runs.claim(
            "chat-live",
            RunClaim {
                run_id: "run-live".to_string(),
                latest,
                cancel: CancellationToken::new(),
                tx,
                steering: Arc::new(Mutex::new(VecDeque::new())),
                follow_up: Arc::new(Mutex::new(VecDeque::new())),
                workspace_path: "C:/workspace".to_string(),
            },
        )
        .unwrap();
        let value = runs.state(Some("chat-live")).unwrap();
        assert_eq!(value["active_node"], "agent");
        let active = runs.active(Some("chat-live"));
        assert_eq!(active["run_id"], "run-live");

        runs.finish(
            "chat-live",
            Some(AppState {
                active_node: "done".to_string(),
                ..AppState::default()
            }),
        );
        let finished = runs.state(Some("chat-live")).unwrap();
        assert_eq!(finished["active_node"], "done");
    }

    #[test]
    fn already_running_has_a_stable_conflict_shape() {
        let (status, Json(body)) = StartRunError::AlreadyRunning {
            chat_id: "chat-1".to_string(),
            run_id: Some("run-1".to_string()),
            current_seq: 12,
        }
        .into_http();
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["code"], "already_running");
        assert_eq!(body["chat_id"], "chat-1");
        assert_eq!(body["run_id"], "run-1");
        assert_eq!(body["current_seq"], 12);
    }

    #[tokio::test]
    async fn latest_seq_tracks_the_durable_watermark_for_after_seq() {
        use stark_store::rollout::{EventMsgPayload, RolloutItem, RolloutWriter};

        let dir = tempfile::tempdir().unwrap();
        let mut writer = RolloutWriter::open(dir.path(), "chat-seq", "ws", dir.path(), "model")
            .await
            .unwrap();
        let ordinal = writer
            .append(RolloutItem::EventMsg {
                payload: EventMsgPayload::Warning {
                    message: "watermark".to_string(),
                },
            })
            .await
            .unwrap();
        assert_eq!(latest_seq(dir.path(), "chat-seq"), ordinal);
        assert_eq!(latest_seq(dir.path(), "chat-missing"), 0);
    }

    #[tokio::test]
    async fn custom_agent_runs_with_a_closed_tool_surface_and_persists_on_follow_up() {
        use axum::Router;
        use axum::body::Body;
        use axum::http::header;
        use axum::routing::post;

        async fn capture_provider(
            State(seen): State<Arc<Mutex<Vec<Value>>>>,
            Json(body): Json<Value>,
        ) -> Response {
            seen.lock().unwrap().push(body);
            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"Custom reply\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n"
            );
            Response::builder()
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from(body))
                .unwrap()
        }

        let seen: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let captured = seen.clone();
        tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route("/chat/completions", post(capture_provider))
                    .with_state(captured),
            )
            .await
            .unwrap();
        });

        let dir = tempfile::tempdir().unwrap();
        let state = group_test_state(dir.path(), format!("http://{address}")).await;
        let file: AgentFile = serde_json::from_value(json!({
            "version": 2,
            "display": "Backend",
            "description": "Rust backend work",
            "access": "read",
            "prompt": {"profile": "general", "persona": "CUSTOM PERSONA: own backend-rs."},
            "tools": ["read", "grep"],
            "skills": [],
            "mcp": []
        }))
        .unwrap();
        let _ = save_agent_handler(
            State(state.clone()),
            Path("backend".to_string()),
            Json(file),
        )
        .await
        .unwrap();

        let request = RunRequest {
            prompt: "inspect".to_string(),
            model: "mock".to_string(),
            temperature: 0.0,
            workspace_path: dir.path().to_string_lossy().to_string(),
            workspace_id: "_general".to_string(),
            chat_id: "chat-custom".to_string(),
            attachments: Vec::new(),
            workspace_kind: "general".to_string(),
            agent: "backend".to_string(),
        };
        let _ = run_command_handler(
            State(state.clone()),
            Path("chat-custom".to_string()),
            Json(request),
        )
        .await
        .unwrap();
        for _ in 0..400 {
            if !state.runs.is_running("chat-custom") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert!(!state.runs.is_running("chat-custom"));
        let chat = state.store.get_chat("chat-custom").await.unwrap().unwrap();
        assert_eq!(chat["agentKey"], "backend");
        {
            let requests = seen.lock().unwrap();
            assert_eq!(requests.len(), 1);
            let body = &requests[0];
            let system = body["messages"][0]["content"].as_str().unwrap_or("");
            assert!(system.contains("CUSTOM PERSONA"), "{system}");
            let tools: Vec<String> = body["tools"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|tool| {
                    tool.pointer("/function/name")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect();
            assert_eq!(tools, vec!["read", "grep"]);
        }

        // Follow-up with no explicit agent must keep the persisted custom
        // agent instead of re-routing to General.
        let follow_up = RunRequest {
            prompt: "continue".to_string(),
            model: "mock".to_string(),
            temperature: 0.0,
            workspace_path: dir.path().to_string_lossy().to_string(),
            workspace_id: "_general".to_string(),
            chat_id: "chat-custom".to_string(),
            attachments: Vec::new(),
            workspace_kind: "general".to_string(),
            agent: String::new(),
        };
        let _ = run_command_handler(
            State(state.clone()),
            Path("chat-custom".to_string()),
            Json(follow_up),
        )
        .await
        .unwrap();
        for _ in 0..400 {
            if !state.runs.is_running("chat-custom") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        {
            let requests = seen.lock().unwrap();
            assert_eq!(requests.len(), 2);
            let system = requests[1]["messages"][0]["content"].as_str().unwrap_or("");
            assert!(system.contains("CUSTOM PERSONA"), "{system}");
            let tools: Vec<String> = requests[1]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|tool| {
                    tool.pointer("/function/name")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect();
            assert_eq!(tools, vec!["read", "grep"]);
        }

        // Deleting the agent makes the persisted chat fail explicitly instead
        // of silently routing to a more privileged built-in.
        let _ = delete_agent_handler(State(state.clone()), Path("backend".to_string()))
            .await
            .unwrap();
        let missing = run_command_handler(
            State(state.clone()),
            Path("chat-custom".to_string()),
            Json(RunRequest {
                prompt: "after delete".to_string(),
                model: "mock".to_string(),
                temperature: 0.0,
                workspace_path: dir.path().to_string_lossy().to_string(),
                workspace_id: "_general".to_string(),
                chat_id: "chat-custom".to_string(),
                attachments: Vec::new(),
                workspace_kind: "general".to_string(),
                agent: String::new(),
            }),
        )
        .await;
        assert!(missing.is_err());
        let (_, Json(error)) = missing.unwrap_err();
        assert!(
            error["error"]
                .as_str()
                .unwrap_or("")
                .contains("no longer exists"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn command_endpoint_starts_a_run_and_after_seq_predates_the_log() {
        async fn command_mock_provider(Json(_body): Json<Value>) -> Response {
            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"Command reply\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n"
            );
            Response::builder()
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from(body))
                .unwrap()
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route("/chat/completions", post(command_mock_provider)),
            )
            .await
            .unwrap();
        });

        let dir = tempfile::tempdir().unwrap();
        let state = group_test_state(dir.path(), format!("http://{address}")).await;
        let request = RunRequest {
            prompt: "command test".to_string(),
            model: "mock".to_string(),
            temperature: 0.0,
            workspace_path: dir.path().to_string_lossy().to_string(),
            workspace_id: "_general".to_string(),
            chat_id: String::new(),
            attachments: Vec::new(),
            workspace_kind: "general".to_string(),
            agent: String::new(),
        };
        let started = run_command_handler(
            State(state.clone()),
            Path("chat-command".to_string()),
            Json(request),
        )
        .await
        .unwrap();
        let (status, Json(body)) = started;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(body["accepted"], true);
        assert_eq!(body["chat_id"], "chat-command");
        assert!(body["run_id"].as_str().unwrap().starts_with("run-"));
        let after_seq = body["after_seq"].as_u64().unwrap();

        for _ in 0..400 {
            if !state.runs.is_running("chat-command") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert!(!state.runs.is_running("chat-command"));

        let mut values = spawn_log_values(
            dir.path().to_path_buf(),
            "chat-command".to_string(),
            Some(after_seq),
            false,
            None,
        );
        let mut saw_assistant = false;
        loop {
            let value = tokio::time::timeout(std::time::Duration::from_secs(2), values.recv())
                .await
                .unwrap()
                .unwrap();
            if value["type"] == "log.synced" {
                break;
            }
            if value["type"] == "delta"
                && value["data"]["role"] == "assistant"
                && value["data"]["text"] == "Command reply"
            {
                saw_assistant = true;
            }
            assert!(
                value["durable"]["seq"].as_u64().unwrap() > after_seq,
                "log event must be after the command cursor"
            );
        }
        assert!(saw_assistant);
    }

    #[test]
    fn event_feed_type_filters_and_flags_parse() {
        let types = parse_types(Some("delta, state,,delta")).unwrap();
        assert!(types.contains("delta"));
        assert!(types.contains("state"));
        assert_eq!(types.len(), 2);
        assert!(parse_types(Some("")).is_none());
        assert!(query_flag(Some("true")));
        assert!(query_flag(Some("1")));
        assert!(!query_flag(Some("0")));
    }
}

#[cfg(test)]
mod origin_tests {
    use super::*;

    fn headers(host: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::HOST,
            axum::http::HeaderValue::from_str(host).unwrap(),
        );
        headers
    }

    #[test]
    fn local_origins_are_allowed() {
        let request_headers = headers("127.0.0.1:8124");
        for origin in ["http://localhost:5173", "http://127.0.0.1:5174"] {
            assert!(
                origin_allowed(origin, &request_headers),
                "{origin} should be allowed"
            );
        }
        assert!(!origin_allowed("https://evil.example", &request_headers));
        assert!(!origin_allowed("null", &request_headers));
    }

    #[test]
    fn origin_must_match_host_for_remote_same_origin() {
        let request_headers = headers("192.168.1.20:8124");
        assert!(origin_allowed("http://192.168.1.20:8124", &request_headers));
        assert!(!origin_allowed(
            "http://192.168.1.20:9999",
            &request_headers
        ));
    }

    #[test]
    fn loopback_host_check_accepts_local_names_only() {
        assert!(host_allowed("127.0.0.1:8124"));
        assert!(host_allowed("localhost:8124"));
        assert!(host_allowed("[::1]:8124"));
        assert!(!host_allowed("evil.example:8124"));
        assert!(!host_allowed("192.168.1.20:8124"));
    }

    #[test]
    fn host_only_strips_ports_and_ipv6_brackets() {
        assert_eq!(host_only("LOCALHOST:8124"), "localhost");
        assert_eq!(host_only("[::1]:8124"), "::1");
        assert_eq!(host_only("example.com"), "example.com");
    }
}
