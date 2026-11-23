# Stark architecture

Current-state reference for the shipped system. The code is the source of
truth; this page explains the component boundaries and how they connect.

## System shape

Stark is two Rust programs and one shared HTTP contract:

- **Rune UI** (`desktop-rune`) — native desktop app and wasm browser app.
- **Stark server** (`backend-rs`) — agent runtime, tools, persistence, API.
- **Transport** — JSON + SSE over `http://127.0.0.1:8124` by default.

There is no legacy web client, desktop wrapper, non-Rust backend, or
server-rendered SPA. The backend is API-only. The Rune wasm build is served
independently by its own static host or dev proxy.

## Rune client

| Crate | Responsibility |
|---|---|
| `stark-rune-app` | Shell, canvas, transcript, composer, settings, agents, cloud, group projects, voice |
| `stark-rune-web` | wasm entry point; mounts the same app |
| `stark-kit` | Pure models: markdown, transcript math, edge paths, shared data types |
| `stark-theme` | Design tokens and shared theme values |
| `stark-launcher` | Stable supervisor: versioned installs, server lifecycle, updates, rollback |

The UI is a single page. One app-drawn titlebar holds the navigation controls,
the open-session tabs and the new-session action. Below it, an icon rail, side
panel, canvas map and transcript surface stay mounted; panels swap content
instead of routing. On Windows Rune replaces the native caption with this 44pt
`.window_drag()` band and draws the minimize/maximize/close buttons over its
trailing edge; `Cx::chrome_insets` reserves that area and `Cx::caption_color`
keeps the symbols readable on the dark fill. Canvas cards are small chat
surfaces with their own drafts and sends. Overlays mount at the shell or modal
root so they escape clipping.

State is plain Rust. Host traits (`TranscriptHost`, `CanvasHost`,
`ProjectsHost`, `SettingsHost`, and related traits) let shared views resolve the
active thread or panel without borrowing the whole app. Handlers capture stable
IDs or keys, never row indices.

### Run transport

1. `POST /api/chats/{chat_id}/run` starts a run and returns
   `{accepted, chat_id, run_id, after_seq}`.
2. `GET /api/chats/{chat_id}/events` is the live feed. It starts with
   `server.connected` and ends with a terminal `done`.
3. `GET /api/chats/{chat_id}/log?after=&follow=1` is the durable replay log.
   Durable events carry `durable.seq`; replay ends with `log.synced`.
4. `POST /api/steer` adds a message to an active run.
5. `POST /api/abort` cancels the run.
6. `GET /api/state?chat_id=` returns live `AppState`.
7. `GET /api/runs/active?chat_id=` is the source of truth after a feed closes.

A live `done` is terminal. EOF is not success: the client checks the active-run
endpoint, rebuilds completed items from `/log` when still active, then resumes
`/events`. Live `evt_...` ids are never used as cursors; only `durable.seq` is.

## Backend crates

| Crate | Responsibility |
|---|---|
| `stark-server` | Axum routes, SSE, run admission, pairing, static-free API |
| `stark-core` | Agent loop, delegation, compaction, rollout traces, workspaces |
| `stark-providers` | OpenAI-compatible streaming, retries, tool-call deltas |
| `stark-tools` | Tool dispatch, inventory, skills, MCP, search, browser, retention |
| `stark-store` | SQLite, JSONL sessions, rollout logs, snapshots, backups |
| `stark-policy` | Approval policy and command rules |
| `stark-sandbox` | Process containment, secrets, network policy |
| `stark-contract` | Tool-spec schema and contract validation |
| `stark-voice` | Native STT/TTS and voice capture |

The server owns one harness root. It fails closed when `STARK_HARNESS_ROOT` is
unset. Provider routing and per-agent configuration live under
`<harness>/.deep_agents/config/`; environment variables supply keys only.

## Agent components

### Tools

`stark-tools/src/inventory.rs` is the source of truth for built-in tool
metadata. `backend-rs/specs/tool-specs.json` is the frozen model-facing
contract. Tools are grouped into scopes such as `developer`, `coder`,
`general`, `house`, `dfd`, `orchestrator`, `devops`, `explore`, and `ui`.

Dispatch passes through the policy and plan guards, the question broker, the
process manager, and MCP routing. Tool output is bounded before it reaches the
model.

### Skills

Skills are directories at `.agents/skills/<name>/SKILL.md` (workspace or
harness level). The catalog is injected into the prompt; the model loads a
skill with the `skill` tool. Agent configuration can restrict the visible
catalog.

### MCP

MCP servers are configured in `.deep_agents/mcp_servers.json` and use the
official `rmcp` client. Both stdio and Streamable-HTTP transports are supported.
Tools are exposed as `mcp__<server>__<tool>`, can be scoped or filtered per
agent, and large catalogs are discovered through `tool_search`. Approvals,
timeouts, output truncation, and per-run activation stay in Stark above the
protocol layer.

### Plugins

There is no separate plugin runtime in the current tree. The extension points
are built-in tools, skills, and MCP servers. Adding a tool means a `ToolMeta`
entry, a contract entry, and a dispatch arm; adding a capability does not
require loading arbitrary code into the server process.

## Persistence and state

The harness root holds:

- `.deep_agents/chats.db` — chats, messages, workspaces, agents, groups,
  memory, schedules, canvas, and related state.
- `.deep_agents/sessions/` and rollout JSONL — turn history and durable replay.
- `.deep_agents/snapshots/` — rewind checkpoints.
- `.deep_agents/uploads/`, `imports/`, `live_screen/` — retained inputs.
- `.deep_agents/config/` — providers, agents, MCP, and app configuration.
- `.deep_agents/worktrees/` — isolated agent worktrees.

Rollout ordinals are the durable cursor for the run log. Database snapshots,
backups, and retention are handled by `stark-store` and `stark-tools`.

## Security and execution

- `stark-policy` decides which commands and tools require approval.
- `stark-sandbox` contains process trees, filters secrets, and enforces the
  configured network policy; filesystem enforcement depends on the OS backend.
- The server binds loopback by default. Remote access requires the pairing
  token; non-loopback API calls are guarded before dispatch.
- A harness lock prevents two servers from owning the same data root.

## Voice

`stark-voice` owns native STT/TTS and model resolution. Voice models live in a
shared `models/` directory in the installed Rune layout. The Rune web build
uses the browser's audio stack; the native build uses the Rust audio engine.
The server exposes voice endpoints when models are present and returns `503`
when they are not.

The VPS runtime installs language toolchains (Rust, Go, Node, PostgreSQL, and a
browser CLI) for agent workspaces. They are execution dependencies for agent
tasks, not part of the Stark application stack.

## Packaging and deployment

- `desktop-rune/packaging/` builds the portable, versioned Rune install and
  the updater payload.
- `stark-launcher` owns `current.json`, `pending.json`, `blocked.json`, server
  lifecycle, and rollback. It is a Windows GUI-subsystem binary; the launcher,
  UI and bundled server embed the Stark icon, and the UI plus shortcuts share
  the `com.stark.rune` AppUserModelID for taskbar grouping.
- `.github/workflows/rune-master.yml` runs on every push to master: it builds,
  signs and publishes the desktop payload to `Stark_Rune_Releases`, updates the
  rolling `rune-master` installer release, and the installed app checks the
  feed on launch and every five minutes. Versions use
  `<next-patch>-master.<run>.<attempt>` so they increase with each build.
- `.github/workflows/build-linux.yml` and `deploy-vps.yml` ship the API-only
  Linux server from the same commit on every master push; no web client build
  is part of the artifact. `/api/health` reports the deployed revision in
  `build`.
- Windows x86_64 is the primary installed target today.

## Source map

```text
backend-rs/        Rust server and all backend crates
desktop-rune/      Rune native/wasm UI, launcher, packaging
docs/              this architecture reference only
scripts/           operational deploy and release helpers
.github/           Rust/Rune validation and release workflows
```
