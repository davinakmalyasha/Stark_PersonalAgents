# 08 — Server Contract

The Rust server must be a drop-in replacement for the FastAPI surface the
React/Electron UI, Telegram bot, and voice overlay already use.

Source: `server/api_server.py`, `server/routers/*`, `frontend/src/services/api/*`.

---

## 1. Axum layout

```
stark-server/
  src/
    app.rs            # router assembly, middleware, state
    state.rs          # AppState, EventBus, RunManager
    routes/
      run.rs          # /api/run, /api/stream, steer, followup, abort
      chats.rs        # workspaces, chats, messages, files, rules
      groups.rs       # groups, approvals, update
      notes.rs
      canvas.rs
      house.rs
      voice.rs        # proxy to Python sidecar
      mcp.rs
      snapshots.rs
      admin.rs        # health, models, provider, pairing, ui, usage
    sse.rs            # SSE formatting, replay, heartbeat
    ws.rs             # voice WebSocket proxy
    static_files.rs   # frontend dist
    auth.rs           # pairing token
```

`AppState` (server state, not the UI snapshot):

```rust
pub struct ServerState {
    pub store: Arc<Store>,
    pub runs: Arc<RunManager>,
    pub bus: Arc<EventBus>,
    pub agents: Arc<AgentRuntime>,
    pub groups: Arc<GroupService>,
    pub voice: Arc<VoiceProxy>,
    pub config: Arc<Config>,
    pub started_at: Instant,
}
```

---

## 2. Route groups

Paths are frozen. Keep the exact strings, including `/api/ask/...` and
`/api/runs/...`.

### 2.1 Run / stream
| Method | Path | Purpose |
|---|---|---|
| POST | `/api/run` | Start a run; returns SSE |
| GET | `/api/stream/{chat_id}` | Reattach to an active run |
| POST | `/api/steer` | Queue a steering message |
| POST | `/api/followup` | Queue a follow-up turn |
| POST | `/api/abort` | Cancel a run |
| GET | `/api/runs/active` | Current run info |
| POST | `/api/runs/{run_id}/undo` | Run-scoped undo |
| GET | `/api/state` | Current state snapshot |
| GET | `/api/history` | Recent prompts |
| GET | `/api/models` | Model options |
| GET/POST | `/api/provider` | Provider mode |
| POST | `/api/session/reset` | Reset session state |

### 2.2 Workspaces / chats
`GET|POST /api/workspaces`, `GET /api/workspaces/{id}`,
`GET|POST /api/workspaces/{id}/chats`,
`GET|DELETE /api/workspaces/{id}/chats/{chat_id}`,
`POST .../chats/{chat_id}/messages`, `POST .../attachments`,
`POST .../rewind`, `PUT .../title`, `GET .../subagents`,
`GET /api/workspaces/{id}/subagents/{child_id}`,
`GET .../files`, `GET .../file`, `GET|POST .../rules`,
`GET|POST /api/settings/profile`, `GET|POST /api/settings/general-profile`,
`GET /api/chat/{chat_id}/mode`, `GET /api/chat/{chat_id}/state`.

### 2.3 Groups / notes / canvas
`GET /api/groups`, `GET /api/groups/{project_id}`,
`POST /api/groups/{project_id}/items/{item_id}/approval`,
`POST /api/groups/{project_id}/update`,
`GET|POST /api/notes`, `GET|PUT|DELETE /api/notes/{note_id}`,
`GET /api/notes/{scope_type}/{scope_id}`,
`GET /api/canvas/{chat_id}/status`.

### 2.4 House
`GET /api/house/state`, `GET /api/house/devices/{device_id}`,
`POST /api/house/rooms/{room_id}`,
`POST /api/house/scenes/{scene_id}/run`, `GET /api/house/history`,
`POST /api/house/reset`.

Mount the house router under `/api/house`; the Python router is mounted with
that prefix (`server/house/router.py`).

### 2.5 Voice
`GET /api/turn_detection`,
`POST /api/voice/start|stop|mute|interrupt|send`,
`GET /api/voice/status|peek|tts_status`,
`POST /api/transcribe`, `POST /api/tts`, `POST /api/tts/cancel`,
`GET /api/tts/stream`,
`POST /api/voice/release_asr|release_tts|warm_tts`,
`POST /api/vision/frame`, `WS /api/voice/ws`.

### 2.6 Infra / admin
`GET /api/health`, `GET /api/health/voice`, `GET /api/pairing`,
`POST /api/pairing/rotate`, `GET|PUT /api/ui/state`,
`GET /api/mcp/servers`, `GET|POST|DELETE /api/mcp/servers/{name}`,
`POST /api/mcp/servers/{name}/restart`,
`GET /api/chats/{chat_id}/snapshots`,
`POST /api/chats/{chat_id}/snapshots/{snap_id}/restore`,
`POST /api/chats/{chat_id}/runs/{run_id}/restore`,
`GET /api/ask/pending`, `POST /api/ask/answer`, `GET /api/inbox`,
`GET /api/clients`, `GET /api/bots`, `POST /api/bots/{bot_id}/disable`,
`GET /api/usage`, `GET /api/dead-reminders`.

### 2.7 Static
`GET /{*path}` serves `frontend/dist`; unknown API paths return JSON 404, not
the SPA. Preserve the current fallback behavior for client-side routes.

---

## 3. SSE contract

Format (exact):

```text
event: delta
data: {"kind":"text","text":"Hel","agent":"developer","model":"...","ts":123}

```

Events:
| Event | Payload |
|---|---|
| `hello` | `{"chat_id": "..."}` (reattach only) |
| `delta` | `Delta` |
| `state` | `AppState` snapshot |
| `done` | `{"status":"complete"|"error"}` |

`Delta`:
```json
{
  "kind": "text|reasoning|usage|end|tool_started|tool_finished",
  "text": "",
  "agent": "developer",
  "model": "...",
  "ts": 123,
  "tool": "read_file",
  "args": "{\"file_path\":\"...\"}",
  "ok": true,
  "ms": 12
}
```

Ordering:
1. Optional `hello` on reattach.
2. `delta*` interleaved with `state*`.
3. Final `state` (complete, with `artifacts`).
4. `done`.

Heartbeat: send a comment (`: ping\n\n`) every 15s so proxies keep the
connection open. Do not send a `delta` just to keep alive.

Reattach:
- `GET /api/stream/{chat_id}` subscribes to the chat bus.
- Replay the latest `AppState` first.
- If the run finished while detached, replay the final state + `done`.
- If no run is active, emit the last known state and close.

Disconnect:
- Preserve current behavior: a client disconnect cancels the run
  (`request.is_disconnected()` → cancellation). If this is changed, it is a
  product decision and must be documented.

---

## 4. `AppState` and `Delta`

Port the frontend types exactly (`frontend/src/types/index.ts`).

```rust
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "snake_case")]
pub struct AppState {
    pub active_node: String,
    pub next_agent: String,
    pub completed_nodes: Vec<String>,
    pub thoughts: HashMap<String, String>,
    pub client_request: String,
    pub outputs: AgentOutputs,
    pub project_path: String,
    pub agents_plan: String,
    pub plan: Option<HashMap<String, Vec<PlanItem>>>,
    pub mcp_servers: Option<Vec<McpServerStatus>>,
    pub antigravity_log: Vec<LogEntry>,
    pub live_terminal_log: String,
    pub selected_model: String,
    pub selected_temp: f64,
    pub token_usage: TokenUsage,
    pub artifacts: Option<RunArtifacts>,
    pub project_ref: Option<ProjectRef>,
    pub workspace_created: Option<bool>,
    pub error: Option<String>,
    pub traceback: Option<String>,
}
```

Notes:
- `outputs` includes `requirements`, `tech_spec`, `code`, `agent_report`,
  `test_report`, plus optional `gherkin`, `mermaid`.
- `state_version` is used for dedupe; include it even if not in the TS
  interface.
- `_intermediate: true` may be present on intermediate snapshots; preserve it.
- Do not rename fields; the UI reads them directly.

---

## 5. Run lifecycle

```rust
pub struct RunManager {
    active: DashMap<ChatId, RunHandle>,
    tracker: Arc<RunTracker>,
}

pub struct RunHandle {
    pub run_id: RunId,
    pub cancel: CancellationToken,
    pub steer: mpsc::Sender<SteerMessage>,
    pub bus: broadcast::Sender<ServerEvent>,
}
```

Rules:
- One active run per chat; a second `POST /api/run` returns 409 with the
  existing message ("This chat already has a running agent...").
- Create the user message and session entry before starting the task.
- Abort: cancel token + settle state; do not leave the chat locked.
- Steering: enqueue; the loop drains between turns.
- Follow-up: enqueue a user message for the next turn.
- On finish: persist assistant message, usage, artifacts; finalize snapshots;
  publish final state; emit `done`; clear the active run.
- On error: persist `error`/`traceback` in the final state; emit `done` with
  `status: "error"`.
- Run cleanup stops background processes owned by the run (`_procman`
  `cleanup_owner`).

---

## 6. Voice WebSocket proxy

During migration, keep the Python voice engine:

- `stark-server` proxies `WS /api/voice/ws` to
  `ws://127.0.0.1:8125/api/voice/ws`.
- Proxy `/api/voice/*`, `/api/tts*`, `/api/transcribe`, `/api/vision/*` over
  HTTP.
- Do not buffer audio; forward frames as they arrive.
- Preserve close codes and error payloads.
- Add a health probe (`/api/health/voice`) that checks the sidecar.

Long-term: replace with Rust STT/TTS; that is a separate project.

---

## 7. Auth / pairing

Port the existing behavior from `server/api_server.py`:

- Local requests are free.
- Phone/remote requests need the pairing token.
- `GET /api/pairing` returns the current pairing info; `POST
  /api/pairing/rotate` rotates it.
- Preserve token transport (query/header) exactly; the UI/QR flow depends on
  it.
- Do not log tokens.

If the Python middleware is opaque, extract the exact rules into tests before
porting.

---

## 8. Errors

JSON shape:
```json
{ "error": "human-readable message", "detail": "optional" }
```

Status codes:
- `400` invalid input
- `404` unknown chat/workspace/project
- `409` run already active / conflicting op
- `422` validation / empty transcript
- `500` unhandled error (also persisted into the run state)

Never return HTML for API errors. Never leak stack traces to remote clients;
keep them in the run state/logs.

---

## 9. Single instance, CORS, shutdown

- Bind `8123` (or `STARK_PORT`); if occupied by another Stark, exit quietly.
- CORS: allow the Electron/Vite origins used today; do not widen to `*` in
  production.
- Graceful shutdown:
  - stop accepting requests,
  - cancel active runs,
  - stop supervisor/schedulers,
  - flush session logs and SQLite,
  - stop sidecar processes the server started.
- Log a startup banner with version, port, DB paths, and provider mode.

---

## 10. Contract tests

- Route inventory diff: every Python route has a Rust route (method + path).
- SSE replay: recorded run → same event sequence and final state.
- `AppState` JSON schema diff against `frontend/src/types/index.ts`.
- Chat CRUD/attachment/rewind parity on a fixture DB.
- Reattach: disconnect mid-run, reconnect, receive state + done.
- Abort: no messages after cancellation; final state marked.
- Voice proxy: binary frames unchanged; close codes preserved.
- Auth: local allowed, remote without token rejected, rotated token works.
