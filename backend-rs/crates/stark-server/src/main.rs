use std::convert::Infallible;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::sse::{Event, Sse};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use chrono::Utc;
use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use futures_util::{Stream, StreamExt, stream};
use serde::Deserialize;
use serde_json::{Value, json};
use stark_contract::{AppState, ServerEvent};
use stark_core::{RunContext, run_agent};
use stark_providers::{Provider, ProviderConfig};
use stark_store::{ChatStore, WorkspaceRegistry};
use stark_tools::QuestionBroker;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

type SseStream = Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>;

struct RunHandle {
    cancel: CancellationToken,
    tx: broadcast::Sender<ServerEvent>,
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
        workspace_path: &str,
    ) -> bool {
        match self.runs.entry(chat_id.to_string()) {
            Entry::Occupied(_) => false,
            Entry::Vacant(entry) => {
                entry.insert(RunHandle {
                    cancel,
                    tx,
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
    ui_state_path: PathBuf,
    harness_root: PathBuf,
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
            delete(remove_workspace_handler),
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
        .route("/api/groups", get(groups_handler))
        .route("/api/canvas/{chat_id}/status", get(canvas_status_handler))
        .route("/api/notes/chat/{chat_id}", get(notes_handler))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let host = std::env::var("STARK_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("STARK_PORT").unwrap_or_else(|_| "8124".to_string());
    let address = format!("{host}:{port}");
    let listener = tokio::net::TcpListener::bind(&address).await?;
    tracing::info!("stark-server listening on http://{address}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn run_handler(
    State(state): State<ServerState>,
    Json(request): Json<RunRequest>,
) -> Result<Sse<SseStream>, (StatusCode, Json<Value>)> {
    let prompt = request.prompt.trim().to_string();
    let _ = (&request.attachments, &request.workspace_kind);
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
    if !state.runs.claim(
        &chat_id,
        cancel.clone(),
        tx.clone(),
        workspace.to_string_lossy().as_ref(),
    ) {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"error": "This chat already has a running agent."})),
        ));
    }

    let latest = Arc::new(Mutex::new(None));
    let ctx = RunContext {
        chat_id: chat_id.clone(),
        workspace_id,
        workspace,
        model,
        temperature: request.temperature,
        provider: state.provider.clone(),
        store: state.store.clone(),
        questions: state.questions.clone(),
        tx: tx.clone(),
        latest: latest.clone(),
        cancel,
    };

    let manager = state.runs.clone();
    let done_tx = tx.clone();
    let prompt_for_run = prompt;
    tokio::spawn(async move {
        let result = run_agent(ctx, prompt_for_run).await;
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
    let value = state
        .runs
        .state(query.chat_id.as_deref())
        .unwrap_or_else(|| serde_json::to_value(AppState::default()).unwrap_or(Value::Null));
    Json(value)
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

async fn session_reset_handler() -> Json<Value> {
    Json(json!({"ok": true}))
}

async fn groups_handler() -> Json<Value> {
    Json(json!({"projects": [], "scoped": false}))
}

async fn canvas_status_handler() -> Json<Value> {
    Json(json!({"active": false, "watch": null}))
}

async fn notes_handler() -> Json<Value> {
    Json(json!({"notes": []}))
}
