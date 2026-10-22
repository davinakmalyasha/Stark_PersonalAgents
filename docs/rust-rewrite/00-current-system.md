# 00 — Current System (the contract)

This document freezes what the Rust rewrite must preserve. It is an inventory,
not a design. The authoritative sources are the code paths named here.

---

## 1. Product

**Stark** is a local-first, Windows desktop multi-agent workspace.

- Frontend: React 19 + Vite 8 + Electron 44 (`frontend/`)
- Backend: Python 3.13 + FastAPI + uvicorn on port **8123** (`server/`)
- Transport: HTTP + SSE for runs; WebSocket for live voice
- Persistence: SQLite + append-only JSONL + per-project event stores
- Front doors: web/Electron UI, Telegram bot, on-device voice (Moonshine STT +
  Pocket TTS), smart-home panel, DFD agency panel
- Packaging: Electron is a thin shell; it builds the frontend and spawns the
  Python backend (`frontend/electron/main.cjs`)

---

## 2. Runtime topology

```
Electron / browser
   │  fetch + SSE        │ WebSocket (voice)
   ▼                     ▼
FastAPI :8123  ──────────────────────────────┐
   │                                          │
   ├─ routing.decide_route (pure rules)       │
   ├─ orchestrator graph (pydantic-graph)     │
   ├─ delegation fan-out (threads)            │
   ├─ workspace agents (pydantic-ai loops)    │
   ├─ tool warehouse + policy/approvals       │
   ├─ state bus + SSE bus + run tracker       │
   ├─ session JSONL + chat SQLite             │
   ├─ group supervisor (15s tick, no LLM)     │
   ├─ schedulers (wakes, canvas, schedules)   │
   └─ Voice / Telegram / House / DFD          │
```

The backend is one process. Threads are used for parallel delegations and
CPU-bound voice calls; asyncio drives HTTP/SSE. Shared mutable state lives in
`sync/state_sync.py` (`shared_state` + `contextvars` for child runs).

---

## 3. Agent tiers (fixed concept)

Source of truth: `docs/memory/fixed-concept.md`.

| Tier | Role | Behavior |
|---|---|---|
| **Orchestrator** | Single coordinator | Receives the user task, splits it into jobs, delegates, parks, verifies, reports. **Does not code.** |
| **Workspace agent** | Scoped single-agent loop | Developer (code), General (docs/web), House (devices/scenes), DFD (agency). Owns its history/compaction/session. |
| **Specialist** | JSON-configured profile | UI, DevOps, Explore, Compactor. Prompt + tools + model + thinking + caps. |
| **Subagent** | Ephemeral | Small prompt + tool subset, one focused job, returns a report. |
| **Supervisor** | Deterministic group-project runtime | No LLM. Applies ops, computes readiness, dispatches, leases, verifies, closes. |

**Rules**
- The orchestrator is not a coder; its tools are `delegate`, status, steering,
  cancel, `create_project`, `ask_user`, `propose_plan`.
- Workspace agents are single-agent loops; they own their tool calls.
- Subagents see nothing from the parent conversation except the job description.
- Adding an agent = one registry entry/spec, not edits across consumers.

---

## 4. Request flow

```
prompt + workspace_path + chat_id + workspace_kind
  → routing.decide_route()                       pure rules, no LLM
      ├─ explicit kind: house / general / dfd / design
      ├─ creation phrasing → orchestrator
      ├─ existing project hit → direct Developer (zero relay tokens)
      └─ otherwise → orchestrator
  → stream_for_kind()
      ├─ direct agent (general/house/dfd/design/dev)
      └─ orchestrator graph:
            analyze → call_llm → decide → park → verify → end
            delegate tool → parallel child chats/threads
            inbox result → boss wakes → summary
  → SSE: hello → delta* → state* → done
```

Project routing details: `routing/router.py`, `agents/project_router.py`.
Graph details: `agents/graph/orchestrator_graph.py`, `agents/delegation.py`.

---

## 5. Feature inventory

### 5.1 Chat & workspaces
- Workspaces: `_general`, `_house`, `_personal`, `_dfd`, and project folders.
- Chat list/create/get/delete; messages; attachments; rewind; title; pin.
- Subagent child chats and status.
- File explorer and file reads; workspace rules and profile.
- Plan/build mode per chat.
- FTS5 search over messages.

### 5.2 Runs & streaming
- `POST /api/run` returns an SSE stream.
- Events: `hello`, `delta`, `state`, `done`.
- Deltas: `text`, `reasoning`, `usage`, `end`, `tool_started`, `tool_finished`.
- `state` is a full `AppState` snapshot.
- Steering (`/api/steer`), follow-ups (`/api/followup`), abort (`/api/abort`).
- One active run per chat; `/api/runs/active`; reattach via `/api/stream/{chat_id}`.
- Run-scoped undo (`/api/runs/{run_id}/undo`) and chat snapshots/restore.
- Token usage and cost accounting; artifacts and final report.

### 5.3 Agents & orchestration
- Deterministic front-door router.
- Durable orchestrator graph with disk checkpoints (crash = resume).
- Delegation jobs: fresh and resume; parallel child chats.
- Job status/message/cancel; result reduction into one block.
- Specialist profiles and ephemeral subagents.
- Plan mode and the plan dock (`update_plan`, `propose_plan`).
- Context compaction and token diet.
- Provider modes: `free`, `deepseek`, `opencode`, `command`; model pins per
  agent and tier (`llm/provider_modes.py`).
- Free-provider pool with rate limits and fallback (`llm/free_providers.py`).

### 5.4 Tool warehouse & guardrails
- One registry; tools assigned to scopes, never hand-wired per agent.
- Scope ACL (`docs/tools/_SCOPE_MAP.md`, `tools/_schemas.py`).
- Filesystem permission (`tools/_runtime.py:enforce_fs_permission`).
- Command policy (`tools/MainTool/_cmdpolicy.py`): tokenized, quote-aware,
  destructive/credential rules, plan-mode read-only allowlist.
- Approvals: `ask_user`, `ApprovalRequired`, deferred tool calls, MCP `prompt`.
- Default-deny project envelope: autonomous / needs_approval / blocked / never.
- External-write idempotency ledger (events: `write.reserved/done/skipped`).
- Snapshots before edits; chat-level rewind (`tools/checkpoints.py`).
- Output caps + `/scratch` offload; secret redaction.
- Parallel-safe tool calls (`tools/_parallel.py`).

### 5.5 Group projects
- Self-contained project folder (`PROJECT.md`, `agents/`, `skills/`, `tools/`,
  `mcp/`, `deps/`, `context/`, `.gp/`).
- Event-sourced `state.db`; board is a projection.
- Supervisor tick: recover → apply ops → gate → reap → milestones → readiness →
  dispatch → collect → verify → invalidate → complete → emit.
- Leases, attempts, budgets, deadlines, stall detection.
- Acceptance criteria: `check`, `manual`, `llm`, `budget`.
- Approvals + idempotency; audit log.
- Board/events/detail tools; schedules, notes, canvas watches.

### 5.6 Memory
- Append-only session JSONL with a tree (`server/session/log.py`).
- Derived LLM context; compaction entries with retained tail.
- Summaries, project identity card, `features.md`, `project.md`.
- Repo map via tree-sitter (`memory/repo_map_generator.py`).
- Optional embeddings (fastembed/BGE).
- Notes (`server/store/agent_notes.py`).

### 5.7 Front doors & domains
- Voice: on-device Moonshine streaming STT, Pocket TTS, barge-in, mute,
  rolling transcript, screen-share frames (`server/routers/voice.py`, `Voice/`).
- Telegram: text/voice, workspace routing, DFD approvals (`Telegram/`).
- House: rooms, devices, scenes, energy/security summary, history.
- DFD: leads, orders, milestones, notes, activities, clients.
- GitHub: repo creation + tiered CI/security gate.
- Documents: PDF read, DOCX/XLSX creation.
- Browser: `agent-browser` CLI (Rust) via subprocess.
- Screen: live capture + vision channel.

### 5.8 Admin
- Health (backend + voice), models/provider switch, MCP server status/reload,
  pairing token/rotate, clients/bots/usage/dead reminders, UI state, session reset.

---

## 6. Tool inventory (42 registered definitions)

The authoritative definitions are `tools/_schemas.py` (`TOOL_DEFINITIONS`, 42
entries) and per-tool docs in `docs/tools/*.md`. Descriptions are prompt API:
copy verbatim.

`propose_plan` is implemented in `tools/MainTool/proposePlan.py` and allowed by
the plan-mode guard, but it is **not** in `TOOL_DEFINITIONS` or the runtime
`_SCOPE_MAP`; the native tool schema never exposes it. It is not part of the
frozen 42-tool contract. Revisit it deliberately in Phase 4 (plan mode).

| Group | Tool | One-line purpose |
|---|---|---|
| Core | `read_file` | Read one or many files, paged, with caching stubs |
| Core | `edit_file` | Targeted replace, whole-file write, or multi-file patch; optional verify |
| Core | `run_command` | Shell (Git Bash) foreground/background + process manager |
| Core | `search_code` | Regex content/path search with symbol annotation and output styles |
| Core | `ast_search` | Structural Python search (warehouse tool) |
| Docs | `read_pdf` | Extract PDF text, optionally by page |
| Docs | `create_docx` | Create a Word document |
| Docs | `create_xlsx` | Create an Excel sheet from headers/rows |
| Interaction | `ask_user` | Structured questions/options; waits for the answer |
| Interaction | `update_plan` | Replace the plan dock contents |
| Interaction | `files_changed` | List recent external file changes |
| Browser | `browser_act` | Navigate/click/fill/type/press/extract/screenshot/close |
| Browser | `browser_navigate` | Open URL and return an accessibility snapshot |
| Browser | `browser_extract` | Extract text/HTML/attributes/title/URL |
| Browser | `browser_screenshot` | Screenshot the current page |
| Browser | `browser_close` | Close the browser session |
| Screen | `look_at_screen` | Capture the user's screen as a vision attachment |
| Screen | `set_screen_share` | Toggle screen-share mode (on/off/continuous) |
| Delegation | `delegate` | Spawn workspace agents as parallel jobs |
| Delegation | `delegation_status` | List delegations with live statuses |
| Delegation | `job_message` | Steer a running delegation |
| Delegation | `job_cancel` | Cancel one running delegation |
| Delegation | `call_subagent` | Run/spawn/status a specialist subagent |
| Projects | `create_coding_project` | Create/register a project folder with a Dev agent |
| Projects | `create_group` | Start a durable group project |
| Projects | `group_board` | Read milestones/items/status/spend/blockers |
| Projects | `group_item_detail` | Read one item's full detail |
| Projects | `group_events` | Read the project event log |
| Projects | `group_update` | Submit one project mutation op |
| Projects | `schedule` | Create a timed orchestrator wake |
| Projects | `give_note` | Create/maintain a sticky note for a chat/project |
| Projects | `canvas_mode` | Watch agent chats and wake on completion/failure |
| House | `house_get_state` | Rooms, devices, scenes, energy, security |
| House | `house_get_device` | One device's state and capabilities |
| House | `house_set_device` | Set light/thermostat/lock/plug/cover state |
| House | `house_run_scene` | Activate a scene |
| House | `house_get_history` | Mock hourly energy history |
| Business | `dfd_bridge` | DFD agency API (non-money operations) |
| Business | `create_client` | Scaffold a client folder (requires approval) |
| GitHub | `github_apply_gate` | Create repo and install tiered CI/security gate |
| Memory | `update_feature` | Update long-term feature memory |
| Memory | `update_project_meta` | Update project metadata memory |

### 6.1 Scope ACL

Source: runtime `_SCOPE_MAP` in `tools/_schemas.py`, exported to
`backend-rs/specs/tool-specs.json`.

| Scope | Tools |
|---|---|
| `developer` | `read_file`, `edit_file`, `run_command`, `search_code`, `ask_user`, `update_plan`, `call_subagent` |
| `general` | `run_command`, `read_pdf`, `create_docx`, `create_xlsx`, `ask_user`, `update_plan`, `browser_act`, `files_changed`, `look_at_screen`, `set_screen_share` |
| `house` | `house_get_state`, `house_get_device`, `house_set_device`, `house_run_scene`, `house_get_history`, `run_command`, `browser_act`, `ask_user`, `update_plan` |
| `dfd` | `dfd_bridge`, `create_client`, `create_coding_project`, `run_command`, `ask_user`, `update_plan`, `files_changed` |
| `orchestrator` | `create_coding_project`, `ask_user`, `delegate`, `delegation_status`, `job_message`, `job_cancel`, `create_group`, `group_board`, `group_item_detail`, `group_events`, `group_update`, `schedule`, `give_note`, `canvas_mode` |
| `devops` | `github_apply_gate`, `run_command`, `read_file`, `edit_file`, `search_code`, `update_plan` |
| `explore` | `read_file`, `search_code`, `run_command` |
| `ui` | `browser_act`, `read_file`, `search_code`, `run_command`, `edit_file`, `ask_user`, `update_plan` |

Tools outside a listed scope are warehouse-only until explicitly assigned.

### 6.2 Tool result contract

Tool implementations return text; the runtime derives metadata
(`tools/_registry.py:ToolExecutionResult`):

- Success tag: `[OK] Exit 0`
- Failure tag: `[ERR] Exit N`
- `[TIMEOUT]` / "timed out" → `timed_out`
- "offloaded" / `/scratch/` → `offloaded`
- Explicit error prefixes: `Error:`, `[ERR]`, `[ERROR]`, `[BLOCKED]`,
  `[REJECTED]`, `[NOT FOUND]`

This text is what the model sees. Preserve it.

---

## 7. API surface (~90 routes + 1 WebSocket)

### Run / stream
`POST /api/run`, `GET /api/stream/{chat_id}`, `POST /api/steer`,
`POST /api/followup`, `POST /api/abort`, `GET /api/runs/active`,
`POST /api/runs/{run_id}/undo`, `GET /api/state`, `GET /api/history`,
`GET /api/models`, `GET|POST /api/provider`, `POST /api/session/reset`.

### Workspaces & chats
`GET|POST /api/workspaces`, `GET /api/workspaces/{id}`,
`GET|POST /api/workspaces/{id}/chats`,
`GET|DELETE /api/workspaces/{id}/chats/{chat_id}`,
`POST /api/workspaces/{id}/chats/{chat_id}/messages`,
`POST .../attachments`, `POST .../rewind`, `PUT .../title`,
`GET .../subagents`, `GET /api/workspaces/{id}/subagents/{child_id}`,
`GET .../files`, `GET .../file`, `GET|POST .../rules`,
`GET|POST /api/settings/profile`, `GET|POST /api/settings/general-profile`,
`GET /api/chat/{chat_id}/mode`, `GET /api/chat/{chat_id}/state`.

### Groups / notes / canvas
`GET /api/groups`, `GET /api/groups/{project_id}`,
`POST /api/groups/{project_id}/items/{item_id}/approval`,
`POST /api/groups/{project_id}/update`,
`GET|POST /api/notes`, `GET|PUT|DELETE /api/notes/{note_id}`,
`GET /api/notes/{scope_type}/{scope_id}`,
`GET /api/canvas/{chat_id}/status`.

### House
`GET /api/house/state` (mounted under `/api/house`),
`GET /api/house/devices/{device_id}`, `POST /api/house/rooms/{room_id}`,
`POST /api/house/scenes/{scene_id}/run`, `GET /api/house/history`,
`POST /api/house/reset`.

### Voice
`GET /api/turn_detection`, `POST /api/voice/start|stop|mute|interrupt|send`,
`GET /api/voice/status|peek|tts_status`, `POST /api/transcribe`,
`POST /api/tts`, `POST /api/tts/cancel`, `GET /api/tts/stream`,
`POST /api/voice/release_asr|release_tts|warm_tts`,
`POST /api/vision/frame`, `WS /api/voice/ws`.

### Infra / admin
`GET /api/health`, `GET /api/health/voice`, `GET /api/pairing`,
`POST /api/pairing/rotate`, `GET|PUT /api/ui/state`,
`GET /api/mcp/servers`, `GET|POST|DELETE /api/mcp/servers/{name}`,
`POST /api/mcp/servers/{name}/restart`,
`GET /api/chats/{chat_id}/snapshots`,
`POST /api/chats/{chat_id}/snapshots/{snap_id}/restore`,
`POST /api/chats/{chat_id}/runs/{run_id}/restore`,
`GET /api/ask/pending`, `POST /api/ask/answer`,
`GET /api/inbox`, `GET /api/clients`, `GET /api/bots`,
`POST /api/bots/{bot_id}/disable`, `GET /api/usage`,
`GET /api/dead-reminders`.

---

## 8. Persistence

| Store | Path | Purpose |
|---|---|---|
| Chat DB | `.deep_agents/chats.db` | `chats`, `messages`, FTS5 index |
| Sessions | `.deep_agents/sessions/{kind}/{ws}/{chat}.jsonl` | Canonical append-only history |
| Group project | `<project>/.gp/state.db` | Events (truth) + projections |
| Jobs | `.deep_agents/jobs.db` | Delegation job records |
| Notes | `.deep_agents/agent_notes.db` (or shared DB) | Sticky notes |
| Wakes/canvas | `.deep_agents/agent_wakes.db` | Pending wakes, canvas watches/events |
| Schedules | `.deep_agents/schedules.db` | Orchestrator schedules |
| Snapshots | `<workspace>/.deep_agents/snapshots/{chat}/{run}/` | Pre-edit file bytes + index |
| House | `server/house/HOUSE_STATE_PATH` | JSON state |
| MCP | `.deep_agents/mcp_servers.json` | Server configs |
| Workspaces | `.deep_agents/workspaces.json` | Workspace registry |

Key schemas are reproduced in [09-persistence.md](./09-persistence.md).

---

## 9. Contracts and invariants

### 9.1 SSE
```
event: hello   data: {"chat_id": "..."}          # reattach only
event: delta   data: {kind, text, agent?, model?, ts?, tool?, args?, ok?, ms?}
event: state   data: {AppState}
event: done    data: {"status": "complete"|"error"}
```

### 9.2 AppState (frontend `types/index.ts`)
`active_node`, `next_agent`, `completed_nodes`, `thoughts`, `client_request`,
`outputs`, `project_path`, `agents_plan`, `plan`, `mcp_servers`,
`antigravity_log`, `live_terminal_log`, `selected_model`, `selected_temp`,
`token_usage`, `artifacts`, `project_ref`, `workspace_created`, `error`,
`traceback`.

### 9.3 Run lifecycle
- One active run per chat; a second run is rejected.
- Client disconnect currently cancels the run; reattach is supported for
  active runs.
- Abort sets the cancellation token; steering is queued and checked between
  model/tool steps.
- Final state is persisted and the checkpoint finalized before `done`.

### 9.4 Group-project invariants
- Events are the only truth; projections are rebuildable.
- One supervisor writer; ops applied in submission order; one event per op.
- Default-deny envelope; `never` = tool absent, `blocked` = attempt refused.
- External writes dedupe by `item_id:shape:payload_sha`; a reserved-but-not-done
  key asks a human instead of retrying blindly.
- A parked approval keeps the item `doing` and the lease renewed.
- Boot never trusts tables: `rebuild()` from events.

### 9.5 Compaction
- Trigger below the model context window; never split tool-call/tool-result
  pairs; reinject the system prompt; retain a self-contained tail checkpoint.

### 9.6 Tool execution
- Scope ACL → path permission → command policy → approval → execute →
  redact → cap/offload → `[OK]/[ERR]` text contract.
- Snapshot before edit; rewind restores bytes.
- Parallel-safe tools may run concurrently; others are serialized.

---

## 10. File map

| Path | Responsibility |
|---|---|
| `server/api_server.py` | App factory, middleware, run streaming, static |
| `server/routers/*` | HTTP domains (chats, groups, runs, voice, house, …) |
| `server/store/*` | SQLite stores (chats, groups, jobs, notes, wakes, schedules) |
| `server/realtime/*` | SSE bus, inbox, steer queue |
| `server/session/log.py` | JSONL session tree |
| `agents/*` | Orchestrator, workspace agents, delegation, subagents |
| `agents/graph/*` | pydantic-graph orchestrator |
| `agents/group/*` | Group projects (events, planner, policy, runner, store) |
| `llm/*` | Providers, transport, native tools, compaction, model history |
| `tools/*` | Tool registry, schemas, runtime guards, implementations |
| `tools/MainTool/*` | Files, shell, policy, process manager, patches |
| `tools/AddOnTool/*` | Browser, GitHub, delegation, subagents, group, notes, schedules |
| `tools/mcp/*` | MCP bridge |
| `memory/*` | Compaction, summaries, repo map, embeddings |
| `routing/*` | Deterministic front-door router |
| `sync/*` | State bus + sync manager |
| `Voice/*` | STT/TTS engine |
| `Telegram/*` | Telegram bot |
| `Desktop/*` | Workspace manager / desktop bridge |
| `frontend/*` | React UI + Electron shell |
| `tests/*` | pytest suite (24 files, ~7.5k LOC) |
| `docs/tools/*` | Per-tool docs (the tool contract) |
| `docs/group-projects/*` | Group-project design of record |

---

## 11. Tests and CI

- Canonical runner: `python -m pytest tests/ -q`.
- `tests/run_all.py` is a zero-dependency fallback (skips fixtures).
- Golden fixture: `tests/fixtures/agent_registry_golden.json`.
- CI: `.github/workflows/template-validate.yml` installs
  `requirements-test.txt` and runs pytest.
- The test suite imports the real runtime; parity tests must not require the
  voice stack.

---

## 12. What must not change

1. Tool names, descriptions, parameter schemas, and result text.
2. SSE event names and `Delta`/`AppState` shapes.
3. Route paths and request/response JSON.
4. SQLite table/column names and JSONL entry format (so old chats open).
5. Agent roles, scopes, delegation semantics, and approval tiers.
6. Group-project event vocabulary and reducer semantics.
7. Default-deny safety posture.
