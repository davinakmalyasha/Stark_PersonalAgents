use std::collections::VecDeque;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use axum::extract::{ConnectInfo, DefaultBodyLimit, Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::middleware;
use axum::response::IntoResponse;
use axum::response::sse::{Event, Sse};
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use chrono::Utc;
use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use futures_util::{Stream, StreamExt, stream};
use serde::Deserialize;
use serde_json::{Value, json};
use stark_contract::{AppState, ServerEvent};
use stark_core::{RunContext, resolve_agent, run_agent, subagent_status, subagent_status_for};
use stark_providers::{Provider, ProviderConfig};
use stark_store::{ChatStore, GroupStore, WorkspaceRegistry};
use stark_tools::{Checkpoint, HouseStore, PlanStore, QuestionBroker, public_device, public_state};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

mod pairing;
use pairing::PairingStore;

type SseStream = Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>;

struct RunHandle {
    cancel: CancellationToken,
    tx: broadcast::Sender<ServerEvent>,
    steering: Arc<Mutex<VecDeque<String>>>,
    follow_up: Arc<Mutex<VecDeque<String>>>,
    started_at: String,
    workspace_path: String,
}

#[derive(Default)]
struct RunManager {
    runs: DashMap<String, RunHandle>,
    last_state: DashMap<String, Value>,
}

impl RunManager {
    fn claim(
        &self,
        chat_id: &str,
        cancel: CancellationToken,
        tx: broadcast::Sender<ServerEvent>,
        steering: Arc<Mutex<VecDeque<String>>>,
        follow_up: Arc<Mutex<VecDeque<String>>>,
        workspace_path: &str,
    ) -> bool {
        match self.runs.entry(chat_id.to_string()) {
            Entry::Occupied(_) => false,
            Entry::Vacant(entry) => {
                entry.insert(RunHandle {
                    cancel,
                    tx,
                    steering,
                    follow_up,
                    started_at: Utc::now().to_rfc3339(),
                    workspace_path: workspace_path.to_string(),
                });
                true
            }
        }
    }

    fn finish(&self, chat_id: &str, state: Option<AppState>) {
        self.runs.remove(chat_id);
        if let Some(state) = state
            && let Ok(value) = serde_json::to_value(&state)
        {
            self.last_state.insert(chat_id.to_string(), value);
        }
    }

    fn is_running(&self, chat_id: &str) -> bool {
        self.runs.contains_key(chat_id)
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

    fn subscribe(&self, chat_id: &str) -> Option<broadcast::Receiver<ServerEvent>> {
        self.runs.get(chat_id).map(|handle| handle.tx.subscribe())
    }

    fn state(&self, chat_id: Option<&str>) -> Option<Value> {
        if let Some(chat_id) = chat_id {
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
    questions: Arc<QuestionBroker>,
    runs: Arc<RunManager>,
    registry: WorkspaceRegistry,
    pairing: Arc<PairingStore>,
    ui_state_path: PathBuf,
    harness_root: PathBuf,
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
}

fn default_temperature() -> f64 {
    0.7
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "stark_server=info,tower_http=info".into()),
        )
        .init();

    let harness_root = std::env::var("STARK_HARNESS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let provider_config = ProviderConfig::from_env();
    if !provider_config.has_key() {
        tracing::warn!(
            "no provider API key found; set STARK_API_KEY, OPENAI_API_KEY, or DEEPSEEK_API_KEY"
        );
    }
    let provider = Provider::new(provider_config)?;
    let store = ChatStore::open(&harness_root).await?;
    let state = ServerState {
        store,
        provider,
        questions: Arc::new(QuestionBroker::new()),
        runs: Arc::new(RunManager::default()),
        registry: WorkspaceRegistry::new(&harness_root),
        pairing: Arc::new(PairingStore::new(&harness_root)),
        ui_state_path: harness_root.join(".deep_agents").join("ui_state.json"),
        harness_root,
    };

    let app = Router::new()
        .route("/api/run", post(run_handler))
        .route("/api/stream/{chat_id}", get(stream_handler))
        .route("/api/abort", post(abort_handler))
        .route("/api/health", get(health_handler))
        .route("/api/state", get(state_handler))
        .route("/api/history", get(history_handler))
        .route("/api/runs/active", get(active_handler))
        .route("/api/ask/pending", get(pending_handler))
        .route("/api/ask/answer", post(answer_handler))
        .route("/api/models", get(models_handler))
        .route("/api/provider", get(provider_handler))
        .route(
            "/api/workspaces",
            get(workspaces_handler).post(add_workspace_handler),
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
            "/api/workspaces/{workspace_id}/chats/{chat_id}/rewind",
            post(rewind_chat_handler),
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
        .route("/api/groups", get(groups_handler))
        .route("/api/groups/{project_id}", get(group_handler))
        .route(
            "/api/groups/{project_id}/update",
            post(group_update_handler),
        )
        .route(
            "/api/groups/{project_id}/items/{item_id}/approval",
            post(group_approval_handler),
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
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .with_state(state);

    let host = std::env::var("STARK_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("STARK_PORT").unwrap_or_else(|_| "8124".to_string());
    let address = format!("{host}:{port}");
    let listener = tokio::net::TcpListener::bind(&address).await?;
    tracing::info!("stark-server listening on http://{address}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

async fn run_handler(
    State(state): State<ServerState>,
    Json(request): Json<RunRequest>,
) -> Result<Sse<SseStream>, (StatusCode, Json<Value>)> {
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
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "prompt is required"})),
        ));
    }

    let chat_id = if request.chat_id.trim().is_empty() {
        Uuid::new_v4().to_string()
    } else {
        request.chat_id.trim().to_string()
    };
    let workspace = if request.workspace_path.trim().is_empty() {
        state.harness_root.clone()
    } else {
        PathBuf::from(request.workspace_path.trim())
    };
    let agent = resolve_agent(&request.workspace_kind, &workspace, &state.harness_root);

    let model = state.provider.config().resolve_model(&request.model);
    let workspace_id = if !request.workspace_id.trim().is_empty() {
        request.workspace_id.trim().to_string()
    } else if let Ok(Some(existing)) = state.store.chat_workspace_id(&chat_id).await {
        existing
    } else if request.workspace_path.trim().is_empty() {
        "_general".to_string()
    } else {
        state
            .store
            .workspace_id_for_path(workspace.to_string_lossy().as_ref())
    };

    let (tx, _) = broadcast::channel(256);
    let receiver = tx.subscribe();
    let cancel = CancellationToken::new();
    let steering = Arc::new(Mutex::new(VecDeque::new()));
    let follow_up = Arc::new(Mutex::new(VecDeque::new()));
    let checkpoint = Checkpoint::begin(&chat_id, &workspace);
    if !state.runs.claim(
        &chat_id,
        cancel.clone(),
        tx.clone(),
        steering.clone(),
        follow_up.clone(),
        workspace.to_string_lossy().as_ref(),
    ) {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"error": "This chat already has a running agent."})),
        ));
    }

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
    };
    let latest = Arc::new(Mutex::new(None));
    let ctx = RunContext {
        chat_id: target.chat_id.clone(),
        workspace_id: target.workspace_id.clone(),
        workspace: target.workspace.clone(),
        role: target.role.clone(),
        scope: target.scope.clone(),
        depth: 0,
        model: target.model.clone(),
        temperature: target.temperature,
        provider: state.provider.clone(),
        store: state.store.clone(),
        questions: state.questions.clone(),
        tx: tx.clone(),
        latest: latest.clone(),
        steering,
        follow_up,
        checkpoint: Some(checkpoint.clone()),
        cancel,
        on_job_done: Some(job_hook(state.clone(), target)),
    };

    let manager = state.runs.clone();
    let done_tx = tx.clone();
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
        manager.finish(&chat_id, final_state);
        let _ = done_tx.send(ServerEvent::new(
            "done",
            json!({"status": status, "error": error}),
        ));
    });

    let stream: SseStream = Box::pin(sse_stream(receiver));
    Ok(Sse::new(stream))
}

fn job_hook(state: ServerState, target: RunTarget) -> stark_core::JobDoneHook {
    Arc::new(move |message| {
        if state.runs.follow_up(&target.chat_id, &message) {
            return;
        }
        spawn_follow_up_run(state.clone(), target.clone(), message);
    })
}

fn spawn_follow_up_run(state: ServerState, target: RunTarget, message: String) {
    let (tx, _) = broadcast::channel(256);
    let cancel = CancellationToken::new();
    let steering = Arc::new(Mutex::new(VecDeque::new()));
    let follow_up = Arc::new(Mutex::new(VecDeque::new()));
    if !state.runs.claim(
        &target.chat_id,
        cancel.clone(),
        tx.clone(),
        steering.clone(),
        follow_up.clone(),
        target.workspace.to_string_lossy().as_ref(),
    ) {
        let _ = state.runs.follow_up(&target.chat_id, &message);
        return;
    }

    let checkpoint = Checkpoint::begin(&target.chat_id, &target.workspace);
    let latest = Arc::new(Mutex::new(None));
    let ctx = RunContext {
        chat_id: target.chat_id.clone(),
        workspace_id: target.workspace_id.clone(),
        workspace: target.workspace.clone(),
        role: target.role.clone(),
        scope: target.scope.clone(),
        depth: 0,
        model: target.model.clone(),
        temperature: target.temperature,
        provider: state.provider.clone(),
        store: state.store.clone(),
        questions: state.questions.clone(),
        tx: tx.clone(),
        latest: latest.clone(),
        steering,
        follow_up,
        checkpoint: Some(checkpoint.clone()),
        cancel,
        on_job_done: Some(job_hook(state.clone(), target.clone())),
    };
    let manager = state.runs.clone();
    let done_tx = tx.clone();
    let chat_id = target.chat_id.clone();
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
        manager.finish(&chat_id, final_state);
        let _ = done_tx.send(ServerEvent::new(
            "done",
            json!({"status": status, "error": error}),
        ));
    });
}

async fn stream_handler(
    State(state): State<ServerState>,
    Path(chat_id): Path<String>,
) -> Sse<SseStream> {
    let hello = Event::default()
        .event("hello")
        .data(json!({"chat_id": chat_id}).to_string());
    if let Some(receiver) = state.runs.subscribe(&chat_id) {
        let head = stream::once(async move { Ok(hello) });
        let body = sse_stream(receiver);
        let combined = head.chain(body);
        let stream: SseStream = Box::pin(combined);
        Sse::new(stream)
    } else {
        let done = Event::default()
            .event("done")
            .data(json!({"status": "complete"}).to_string());
        let events: Vec<Result<Event, Infallible>> = vec![Ok(hello), Ok(done)];
        let stream: SseStream = Box::pin(stream::iter(events));
        Sse::new(stream)
    }
}

fn sse_stream(
    receiver: broadcast::Receiver<ServerEvent>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    BroadcastStream::new(receiver).filter_map(|item| async move {
        match item {
            Ok(event) => Some(Ok(Event::default()
                .event(event.event)
                .data(event.data.to_string()))),
            Err(_) => None,
        }
    })
}

#[derive(Debug, Deserialize)]
struct ChatRequest {
    chat_id: String,
}

async fn abort_handler(
    State(state): State<ServerState>,
    Json(request): Json<ChatRequest>,
) -> Json<Value> {
    Json(json!({"ok": state.runs.cancel(&request.chat_id)}))
}

async fn health_handler() -> Json<Value> {
    Json(json!({"ok": true, "code_version": "stark-rs-0.1"}))
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
    Json(state.runs.active(query.chat_id.as_deref()))
}

async fn pending_handler(State(state): State<ServerState>) -> Json<Value> {
    Json(json!({"batches": state.questions.pending_batches()}))
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

async fn models_handler(State(state): State<ServerState>) -> Json<Value> {
    let model = state.provider.config().model.clone();
    Json(json!({
        "models": [{"id": model, "name": model, "provider": state.provider.config().mode}]
    }))
}

async fn provider_handler(State(state): State<ServerState>) -> Json<Value> {
    let config = state.provider.config();
    Json(json!({
        "provider": config.mode,
        "base_url": config.base_url,
        "model_flash": config.model,
        "model_pro": config.model,
        "has_key": config.has_key(),
    }))
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

async fn chats_handler(
    State(state): State<ServerState>,
    Path(workspace_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if workspace_path(&state, &workspace_id).is_none() {
        return Err(not_found("workspace not found"));
    }
    let chats = state.store.list_chats(&workspace_id).await;
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
        Ok(Some(chat)) => Ok(Json(chat)),
        Ok(None) => Err(not_found("chat not found")),
        Err(error) => Err(bad_request(error.to_string())),
    }
}

async fn delete_chat_handler(
    State(state): State<ServerState>,
    Path((_workspace_id, chat_id)): Path<(String, String)>,
) -> Json<Value> {
    let ok = state.store.delete_chat(&chat_id).await.unwrap_or(false);
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
    let projects = if scoped {
        projects
            .into_iter()
            .filter(|project| {
                project["origin_workspace_id"].as_str() == Some(query.workspace_id.as_str())
            })
            .collect()
    } else {
        projects
    };
    Json(json!({"projects": projects, "scoped": scoped}))
}

async fn group_handler(
    State(_state): State<ServerState>,
    Path(project_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    GroupStore::default_root()
        .detail(&project_id, 60)
        .await
        .map(Json)
        .map_err(group_error)
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

async fn canvas_status_handler() -> Json<Value> {
    Json(json!({"active": false, "watch": null}))
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
    let Ok(upload_root) = std::fs::canonicalize(harness_root.join(".deep_agents").join("uploads"))
    else {
        return String::new();
    };
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
                .filter(|path| path.starts_with(&upload_root))?;
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
    let prefix = format!("cont-{chat_id}-");
    let live = subagent_status(&chat_id);
    let subagents: Vec<Value> = state
        .store
        .list_chats(&workspace_id)
        .await
        .into_iter()
        .filter(|chat| {
            chat["id"]
                .as_str()
                .is_some_and(|id| id.starts_with(&prefix))
        })
        .map(|chat| {
            let id = chat["id"].as_str().unwrap_or("");
            if let Some(info) = live
                .iter()
                .find(|info| info["child_id"].as_str() == Some(id))
            {
                return info.clone();
            }
            let title = chat["title"].as_str().unwrap_or("Subagent");
            json!({
                "child_id": id,
                "name": title,
                "label": title,
                "status": "idle",
                "created": 0,
                "parent_session": chat_id,
            })
        })
        .collect();
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
    let live = subagent_status_for(&child_id).unwrap_or_else(|| {
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
    if state.runs.is_running(&chat_id) {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"error": "This chat is still running. Stop it before rewinding."})),
        ));
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

async fn auth_middleware(
    State(state): State<ServerState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    request: axum::extract::Request,
    next: middleware::Next,
) -> axum::response::Response {
    let local = address.ip().is_loopback();
    if local || request.method() == axum::http::Method::OPTIONS {
        return next.run(request).await;
    }
    let token = request
        .headers()
        .get("x-api-token")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if token == state.pairing.token() {
        return next.run(request).await;
    }
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
    async fn follow_up_run_persists_a_woken_reply() {
        use axum::body::Body;
        use axum::http::header;
        use axum::response::Response;
        use axum::routing::post;
        use axum::{Json, Router};

        async fn mock_provider(Json(_body): Json<Value>) -> Response {
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
        let app = Router::new().route("/chat/completions", post(mock_provider));
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let dir = tempfile::tempdir().unwrap();
        let store = ChatStore::open(dir.path()).await.unwrap();
        let provider = Provider::new(ProviderConfig {
            mode: "deepseek".to_string(),
            base_url: format!("http://{address}"),
            api_key: "test".to_string(),
            model: "mock".to_string(),
        })
        .unwrap();
        let state = ServerState {
            store: store.clone(),
            provider,
            questions: Arc::new(QuestionBroker::new()),
            runs: Arc::new(RunManager::default()),
            registry: WorkspaceRegistry::new(dir.path()),
            pairing: Arc::new(PairingStore::new(dir.path())),
            ui_state_path: dir.path().join("ui_state.json"),
            harness_root: dir.path().to_path_buf(),
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
            history
                .iter()
                .any(|message| message.content == "[DELEGATION RESULT] job done")
        );
        assert!(history.iter().any(|message| message.content == "Woke"));
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
}
