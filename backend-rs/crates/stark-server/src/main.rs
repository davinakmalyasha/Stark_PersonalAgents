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
use axum::response::Response;
use axum::response::sse::{Event, Sse};
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use chrono::Utc;
use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use futures_util::{Stream, StreamExt, stream};
use serde::Deserialize;
use serde_json::{Value, json};
use stark_contract::{AppState, ServerEvent};
use stark_core::{RunContext, agent_for_scope, resolve_agent, run_agent};
use stark_providers::{CURATED_MODELS, ImagePart, Provider, ProviderConfig};
use stark_store::{
    AgentRecord, AgentStore, CanvasStore, ChatStore, GroupStore, MemoryStore, ScheduleRecord,
    ScheduleStore, WorkspaceRegistry, parse_schedule_time, schedule_json,
};
use stark_tools::{
    Checkpoint, HouseStore, PlanStore, QuestionBroker, evaluate_acceptance, public_device,
    public_state,
};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

mod pairing;
mod screen_routes;
mod voice;
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
    let _ = tracing_log::LogTracer::init();
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "stark_server=info,tower_http=info".into()),
        )
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);

    let harness_root = std::env::var("STARK_HARNESS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let app_env = harness_root.join(".env");
    if app_env.is_file() {
        let _ = dotenvy::from_path(&app_env);
    }
    let provider_config = ProviderConfig::from_env();
    if !provider_config.has_key() {
        tracing::warn!(
            "no provider API key found; set STARK_API_KEY, OPENAI_API_KEY, or DEEPSEEK_API_KEY"
        );
    }
    let provider = Provider::new(provider_config)?;
    let store = ChatStore::open(&harness_root).await?;
    let voice = stark_voice::resolve_models_dir(&harness_root).map(|dir| {
        tracing::info!("voice models: {}", dir.display());
        Arc::new(voice::VoiceRuntime::new(dir))
    });
    if voice.is_none() {
        tracing::warn!("no voice models directory found; voice/TTS endpoints will return 503");
    }
    let mcp_manager = stark_tools::init_mcp(&harness_root);
    let state = ServerState {
        store,
        provider,
        questions: Arc::new(QuestionBroker::new()),
        runs: Arc::new(RunManager::default()),
        registry: WorkspaceRegistry::new(&harness_root),
        pairing: Arc::new(PairingStore::new(&harness_root)),
        ui_state_path: harness_root.join(".deep_agents").join("ui_state.json"),
        harness_root,
        voice,
    };
    let scheduler_state = state.clone();
    let group_runner_state = state.clone();
    let canvas_state = state.clone();
    let agent_state = state.clone();
    tokio::spawn(async move {
        mcp_manager.start_all().await;
    });

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
        .route(
            "/api/groups",
            get(groups_handler).post(create_group_handler),
        )
        .route("/api/groups/{project_id}", get(group_handler))
        .route("/api/agents", get(agents_handler))
        .route("/api/agents/{agent_id}", get(agent_handler))
        .route("/api/agents/{agent_id}/cancel", post(agent_cancel_handler))
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
        .route(
            "/api/memory/{workspace_id}",
            get(memory_status_handler).delete(memory_clear_handler),
        )
        .route(
            "/api/memory/{workspace_id}/consolidate",
            post(memory_consolidate_handler),
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
    tokio::spawn(schedule_loop(scheduler_state));
    tokio::spawn(group_loop(group_runner_state));
    tokio::spawn(canvas_loop(canvas_state));
    tokio::spawn(agent_loop(agent_state));
    tracing::info!("stark-server listening on http://{address}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

async fn canvas_loop(state: ServerState) {
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
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
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
            let target = canvas_target(&state, &chat);
            spawn_follow_up_run(state.clone(), target, text);
            let _ = store.mark_wake(id, "done").await;
        }
    }
}

fn canvas_target(state: &ServerState, chat: &Value) -> RunTarget {
    let workspace_path = chat["workspacePath"].as_str().unwrap_or("");
    let workspace = if workspace_path.trim().is_empty() {
        state.harness_root.clone()
    } else {
        PathBuf::from(workspace_path)
    };
    let workspace_id = chat["workspaceId"].as_str().unwrap_or("_general");
    let parent = chat["parentChatId"].as_str().unwrap_or("");
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
    let agent = stark_core::agent_for_scope(scope)
        .unwrap_or_else(|| stark_core::agent_for_scope("general").expect("general agent"));
    let model = chat["model"]
        .as_str()
        .filter(|model| !model.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| state.provider.config().resolve_model(""));
    RunTarget {
        chat_id: chat["id"].as_str().unwrap_or("").to_string(),
        workspace_id: workspace_id.to_string(),
        workspace,
        scope: agent.key.to_string(),
        role: Some(agent.display.to_string()),
        model,
        temperature: 0.2,
    }
}

async fn schedule_loop(state: ServerState) {
    let store = match ScheduleStore::open(&state.harness_root).await {
        Ok(store) => store,
        Err(error) => {
            tracing::warn!("scheduler disabled: {error}");
            return;
        }
    };
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
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
            spawn_follow_up_run(
                state.clone(),
                RunTarget {
                    chat_id: record.chat_id.clone(),
                    workspace_id: record.workspace_id.clone(),
                    workspace,
                    scope: record.scope.clone(),
                    role: record.role.clone(),
                    model: record.model.clone(),
                    temperature: record.temperature,
                },
                record.prompt.clone(),
            );
            if let Err(error) = store.mark_after_run(&record).await {
                tracing::warn!("scheduler mark after run failed: {error}");
            }
        }
    }
}

async fn group_loop(state: ServerState) {
    let groups = GroupStore::default_root();
    match groups.recover_doing_items().await {
        Ok(count) if count > 0 => tracing::info!("recovered {count} interrupted group item(s)"),
        Ok(_) => {}
        Err(error) => tracing::warn!("group runner recovery failed: {error}"),
    }
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
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
    let agent = agent_for_scope("developer").expect("the developer agent is registered");
    let model = state.provider.config().resolve_model("");
    let title = format!("Group: {}", item["title"].as_str().unwrap_or("Task"));
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
    let (tx, _) = broadcast::channel(256);
    let cancel = CancellationToken::new();
    let steering = Arc::new(Mutex::new(VecDeque::new()));
    let follow_up = Arc::new(Mutex::new(VecDeque::new()));
    if !state.runs.claim(
        &chat_id,
        cancel.clone(),
        tx.clone(),
        steering.clone(),
        follow_up.clone(),
        workspace.to_string_lossy().as_ref(),
    ) {
        let _ = groups
            .finish_item(
                &project_id,
                item["id"].as_str().unwrap_or(""),
                false,
                "Worker chat already has an active run.",
            )
            .await;
        return;
    }

    let checkpoint = Checkpoint::begin(&chat_id, &workspace);
    if workspace != state.harness_root {
        let snapshot = checkpoint.clone();
        let _ = tokio::task::spawn_blocking(move || snapshot.snapshot_all()).await;
    }
    let latest = Arc::new(Mutex::new(None));
    let ctx = RunContext {
        chat_id: chat_id.clone(),
        workspace_id,
        workspace: workspace.clone(),
        role: Some(agent.display.to_string()),
        scope: agent.key.to_string(),
        depth: 0,
        model,
        temperature: 0.2,
        images: Vec::new(),
        provider: state.provider.clone(),
        store: state.store.clone(),
        questions: state.questions.clone(),
        tx,
        latest: latest.clone(),
        steering,
        follow_up,
        checkpoint: Some(checkpoint.clone()),
        cancel,
        on_job_done: None,
    };
    let acceptance =
        serde_json::to_string_pretty(&item["acceptance"]).unwrap_or_else(|_| "[]".to_string());
    let prompt = format!(
        "Execute one item for this group project.\n\nProject goal:\n{}\n\nMilestone:\n{}\n\nItem:\n{}\n\nDetails:\n{}\n\nAcceptance criteria (all must be met):\n{}\n\nWork only on this item. Report what changed and how each criterion was met.",
        detail["project"]["goal"].as_str().unwrap_or(""),
        item["milestone_title"].as_str().unwrap_or(""),
        item["title"].as_str().unwrap_or(""),
        item["detail"].as_str().unwrap_or(""),
        acceptance,
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
        images: run_images,
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
    let canvas_state = state.clone();
    let canvas_chat = chat_id.clone();
    let status_state = state.clone();
    let status_chat = chat_id.clone();
    mark_agent_status(&state, &chat_id, "running").await;
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
        manager.finish(&chat_id, final_state);
        let _ = done_tx.send(ServerEvent::new(
            "done",
            json!({"status": status, "error": error}),
        ));
        mark_agent_status(&status_state, &status_chat, "idle").await;
        spawn_canvas_notification(canvas_state, canvas_chat, report, status);
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
        images: Vec::new(),
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
        manager.finish(&chat_id, final_state);
        let _ = done_tx.send(ServerEvent::new(
            "done",
            json!({"status": status, "error": error}),
        ));
        mark_agent_status(&status_state, &status_chat, "idle").await;
        spawn_canvas_notification(canvas_state, canvas_chat, report, status);
    });
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

async fn agent_loop(state: ServerState) {
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
    }
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
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

async fn agent_target(state: &ServerState, agent: &AgentRecord) -> RunTarget {
    let workspace_path = state
        .store
        .chat_workspace_path(&agent.chat_id)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    let workspace = if workspace_path.trim().is_empty() {
        state.harness_root.clone()
    } else {
        PathBuf::from(workspace_path)
    };
    let model = if agent.model.trim().is_empty() {
        state.provider.config().resolve_model("")
    } else {
        agent.model.clone()
    };
    RunTarget {
        chat_id: agent.chat_id.clone(),
        workspace_id: agent.workspace_id.clone(),
        workspace,
        scope: agent.scope.clone(),
        role: (!agent.role.is_empty()).then(|| agent.role.clone()),
        model,
        temperature: 0.2,
    }
}

async fn spawn_agent_run(state: ServerState, store: AgentStore, agent: AgentRecord) -> bool {
    let target = agent_target(&state, &agent).await;
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
        .map(|message| {
            format!(
                "[{} from {}]\n{}",
                message["kind"].as_str().unwrap_or("message"),
                message["senderId"].as_str().unwrap_or("user"),
                message["body"].as_str().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let checkpoint = Checkpoint::begin(&target.chat_id, &target.workspace);
    let latest = Arc::new(Mutex::new(None));
    let ctx = RunContext {
        chat_id: target.chat_id.clone(),
        workspace_id: target.workspace_id.clone(),
        workspace: target.workspace.clone(),
        role: target.role.clone(),
        scope: target.scope.clone(),
        depth: agent.depth.max(0) as u32,
        model: target.model.clone(),
        temperature: target.temperature,
        images: Vec::new(),
        provider: state.provider.clone(),
        store: state.store.clone(),
        questions: state.questions.clone(),
        tx: tx.clone(),
        latest: latest.clone(),
        steering,
        follow_up,
        checkpoint: Some(checkpoint.clone()),
        cancel: cancel.clone(),
        on_job_done: Some(job_hook(state.clone(), target.clone())),
    };
    stark_core::register_agent_cancel(&agent.id, cancel.clone());
    let manager = state.runs.clone();
    let done_tx = tx.clone();
    let canvas_state = state.clone();
    let canvas_chat = target.chat_id.clone();
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
        manager.finish(&canvas_chat, final_state);
        let _ = done_tx.send(ServerEvent::new(
            "done",
            json!({"status": status, "error": error}),
        ));
        let _ = store.finish(&agent.id, status, &report, usage).await;
        if let Ok(Some(message_id)) = store.notify_parent(&agent, &report, status).await {
            wake_parent_agent(&store, &canvas_state, &agent, &report, message_id).await;
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
    message_id: String,
) {
    let Some(parent_id) = child.parent_id.as_deref() else {
        return;
    };
    let Ok(Some(parent)) = store.get(parent_id).await else {
        return;
    };
    let target = agent_target(state, &parent).await;
    let text = format!(
        "[AGENT RESULT] {} · {} · {}\nchat: {}\n{}\nReview the child result and continue.",
        child.path,
        child.role,
        child.status,
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

async fn agents_handler(State(state): State<ServerState>) -> Json<Value> {
    let Ok(store) = AgentStore::open(&state.harness_root).await else {
        return Json(json!({"agents": []}));
    };
    let agents = store
        .list(None)
        .await
        .unwrap_or_default()
        .iter()
        .map(AgentRecord::to_json)
        .collect::<Vec<_>>();
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
            let messages = store.messages(&agent_id).await.unwrap_or_default();
            Json(json!({"agent": agent.to_json(), "messages": messages}))
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

async fn memory_status_handler(
    State(state): State<ServerState>,
    Path(workspace_id): Path<String>,
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
    Json(json!({"status": status, "summary": summary}))
}

async fn memory_consolidate_handler(
    State(state): State<ServerState>,
    Path(workspace_id): Path<String>,
) -> Json<Value> {
    let Ok(store) = MemoryStore::open(&state.harness_root).await else {
        return Json(json!({"error": "memory store unavailable"}));
    };
    match store.consolidate(&workspace_id, 30).await {
        Ok(summary) => Json(json!({"ok": true, "summary": summary})),
        Err(error) => Json(json!({"error": error.to_string()})),
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

async fn models_handler(State(state): State<ServerState>) -> Json<Value> {
    let mode = state.provider.config().mode.clone();
    let models: Vec<Value> = CURATED_MODELS
        .iter()
        .map(|(id, name)| json!({"id": id, "name": name, "provider": mode}))
        .collect();
    Json(json!({ "models": models }))
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
        "runtime_notice": "Rust group projects share the Python project store. The Rust runner uses one workspace agent per item; member-specific routing and approval gates are not yet implemented.",
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
    detail["runtime_notice"] = json!(
        "Rust group projects share the Python project store. The Rust runner uses one workspace agent per item; member-specific routing and approval gates are not yet implemented."
    );
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

    async fn group_test_state(root: &std::path::Path, base_url: String) -> ServerState {
        let store = ChatStore::open(root).await.unwrap();
        let provider = Provider::new(ProviderConfig {
            mode: "deepseek".to_string(),
            base_url,
            api_key: "test".to_string(),
            model: "mock".to_string(),
        })
        .unwrap();
        ServerState {
            store,
            provider,
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
