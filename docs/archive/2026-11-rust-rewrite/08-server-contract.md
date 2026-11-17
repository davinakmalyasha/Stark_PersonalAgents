# 08 — Server Contract

The Rust `stark-server` is the only backend. It serves the HTTP/SSE/WS API and,
for remote mode, the frontend on the same origin.

---

## 1. Process and ports

| Setting | Default |
|---|---|
| `STARK_HOST` / `STARK_BIND` | `127.0.0.1` |
| `STARK_PORT` | `8124` dev, `8123` desktop |
| `STARK_HARNESS_ROOT` | app data dir (dev may point at the repo) |
| `STARK_FRONTEND_DIST` | `<harness>/frontend/dist` |

One Tokio multi-thread runtime. In-process loops: agent supervisor (1s),
group supervisor (15s), schedule dispatcher, canvas wake dispatcher.

---

## 2. Route inventory

### 2.1 Run / stream

```
POST /api/run
GET  /api/stream/{chat_id}
POST /api/abort
POST /api/steer
POST /api/followup
GET  /api/inbox
```

### 2.2 State / provider

```
GET  /api/health
GET  /api/state
GET  /api/history
GET  /api/runs/active
GET  /api/models
GET  /api/provider
GET  /api/ui/state
PUT  /api/ui/state
POST /api/session/reset
```

### 2.3 Workspaces / chats / files

```
GET|POST /api/workspaces
GET|DELETE /api/workspaces/{id}
GET  /api/workspaces/{id}/files
GET  /api/workspaces/{id}/file
GET|POST /api/workspaces/{id}/chats
GET|DELETE /api/workspaces/{id}/chats/{chat_id}
PUT  /api/workspaces/{id}/chats/{chat_id}/title
POST /api/workspaces/{id}/chats/{chat_id}/messages
POST /api/workspaces/{id}/chats/{chat_id}/attachments
POST /api/workspaces/{id}/chats/{chat_id}/rewind
GET  /api/workspaces/{id}/chats/{chat_id}/subagents
GET  /api/workspaces/{id}/subagents/{child_id}
GET  /api/chat/{chat_id}/mode
POST /api/chat/{chat_id}/mode
GET  /api/chat/{chat_id}/state
GET  /api/chats/{chat_id}/snapshots
POST /api/chats/{chat_id}/snapshots/{snap_id}/restore
POST /api/chats/{chat_id}/runs/{run_id}/restore
POST /api/runs/{run_id}/undo
```

### 2.4 Agents

```
GET  /api/agents
GET  /api/agents/{agent_id}
POST /api/agents/{agent_id}/cancel
POST /api/agents/{agent_id}/message
GET  /api/agents/{agent_id}/diff
POST /api/agents/{agent_id}/merge
POST /api/agents/{agent_id}/discard
```

### 2.5 Memory

```
GET    /api/memory
GET    /api/memory/{workspace_id}
PUT    /api/memory/{workspace_id}
DELETE /api/memory/{workspace_id}
POST   /api/memory/{workspace_id}/consolidate
POST   /api/memory/{workspace_id}/notes
GET    /api/memory/{workspace_id}/topics/{topic}
DELETE /api/memory/{workspace_id}/entries/{entry_id}
POST   /api/memory/{workspace_id}/entries/{entry_id}/pin
```

### 2.6 Groups / schedules / notes / canvas

```
GET|POST /api/groups
GET  /api/groups/{project_id}
POST /api/groups/{project_id}/update
POST /api/groups/{project_id}/items/{item_id}/approval
GET|POST /api/schedules
DELETE /api/schedules/{schedule_id}
GET  /api/notes/{scope_type}/{scope_id}
POST /api/notes
PATCH|DELETE /api/notes/{note_id}
GET  /api/canvas/{chat_id}/status
```

### 2.7 MCP

```
GET    /api/mcp/servers
POST   /api/mcp/servers/{name}
DELETE /api/mcp/servers/{name}
POST   /api/mcp/servers/{name}/restart
```

### 2.8 House

```
GET  /api/house/state
GET  /api/house/devices/{device_id}
POST /api/house/rooms/{room_id}
POST /api/house/scenes/{scene_id}/run
GET  /api/house/history
POST /api/house/reset
```

### 2.9 Voice / screen

```
GET  /api/turn_detection
GET  /api/voice/status
GET  /api/voice/peek
GET  /api/voice/tts_status
POST /api/voice/start
POST /api/voice/stop
POST /api/voice/mute
POST /api/voice/interrupt
POST /api/voice/send
POST /api/voice/release_asr
POST /api/voice/release_tts
POST /api/voice/warm_tts
POST /api/tts
POST /api/tts/stream
POST /api/tts/cancel
POST /api/transcribe
POST /api/vision/frame
WS   /api/voice/ws
```

### 2.10 Pairing / asks

```
GET  /api/pairing
POST /api/pairing/rotate
GET  /api/ask/pending
POST /api/ask/answer
```

Dropped and returning 404 by design: `/api/agentsadmin/*`,
`/api/settings/*`, `/api/workspaces/{id}/rules`, `/api/health/voice`.
No current frontend surface calls them.

---

## 3. SSE contract

`GET /api/stream/{chat_id}` emits named events:

| Event | Payload |
|---|---|
| `hello` | `{ chat_id }` |
| `state` | full `AppState` |
| `delta` | `Delta` (`text`, `reasoning`, `usage`, `tool_started`, `tool_finished`, `end`) |
| `done` | `{ status, error }` |

- `status`: `complete` | `error` | `aborted`.
- The agent's terminal status is persisted **before** `done`.
- A closed stream without `done` means transport loss; the client detaches.
- Replay: `/api/stream` sends the latest `AppState` before subscribing.

---

## 4. AppState

```ts
interface AppState {
  active_node: string;
  next_agent: string;
  completed_nodes: string[];
  thoughts: Record<string, string>;
  client_request: string;
  outputs: Record<string, unknown>;
  project_path: string;
  agents_plan: string;
  plan?: Record<string, PlanItem[]>;
  antigravity_log: unknown[];
  live_terminal_log: string;
  selected_model: string;
  selected_temp: number;
  token_usage: TokenUsage;
  artifacts?: RunArtifacts;
  project_ref?: ProjectRef | null;
  error?: string;
  traceback?: string;
  state_version: number;
}
```

`delta` fields are additive; never rename an existing kind without a frontend
change.

---

## 5. Auth and pairing

- Loopback clients pass freely.
- Non-loopback `/api/*` requires `x-api-token` matching the pairing token.
- `GET /api/pairing` and `POST /api/pairing/rotate` are local-only.
- The SPA shell (`/`, `/assets/*`, deep links) is **public** so a phone can
  load the pairing gate before it has a token.
- Tokens are never logged.

---

## 6. Static frontend

- `GET /assets/*` serves `frontend/dist/assets`.
- `GET /{*path}` serves a real file if present, otherwise `index.html`.
- Unknown `/api/*` returns JSON 404, never the SPA.
- Path traversal is rejected by resolving under `dist`.

---

## 7. Lifecycle

- Startup: open stores, recover interrupted agents/wakes, start loops, load MCP.
- Shutdown: cancel active runs, stop MCP children, close DB pools.
- Agent run: claim → mailbox → worktree → run → auto-commit → terminal
  status → notify parent.
- Run error: persist `error`/`traceback` in `AppState`, emit `done:error`.

---

## 8. Contract tests

- Route inventory: every frontend-used path exists; dropped paths 404.
- SSE replay and final state.
- `AppState` JSON matches `frontend/src/types`.
- Chat CRUD/attachment/rewind on fixture DBs.
- Auth: local free, remote without token 401, rotated token works.
- Static: shell public, assets correct type, API 404 JSON, traversal contained.
