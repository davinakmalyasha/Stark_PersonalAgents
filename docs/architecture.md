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

### Desktop presence

The native app is also a background desktop surface:

- **Stark Island** (`island.rs`) is a Rune overlay at the top-center of the
  primary display. It is transparent, topmost and skipped from the taskbar;
  only the notch itself is an input region, so the rest of its 620×560 window
  is click-through. The island exists only in live mode, and arming alone
  raises it: the prompt-dock mic warms the speech models and shows the idle
  island (104×24 mark + status dot, white hairline, no glow), while only the
  island's mic — or `Ctrl+Space` while armed — starts capture.
  Hover opens the **Overview / Work** shell on the active thread. Overview is
  the default and replaces the old status line with a two-seat mode rail:
  - **Overview** is a glanceable dashboard: running agents and their current
    activity, active run time, items waiting on the user, and the last three
    finished/stopped/failed runs. The top-right chevron opens Work and the
    adjacent close button collapses the island. There is no live status strip
    and no count in the rail.
  - **Work** is the activity shelf: running chats, active group projects and
    cloud runs, three rows at a time with the rest scrolling. The top-right
    add control fans into Chat / Group / Cloud; each offers a new item or picks
    an existing one. Choosing a chat opens one pinned in-overlay chat at a time
    (no tab strip), backed by the same markdown renderer as the main
    transcript. The chat surface captures the whole overlay so a click can
    never leak through.
  The chat prompt dock is one inline row: text field, Live split control and
  send. The Live control's left side starts/stops capture; its chevron opens
  the anchored settings menu. Per-chat settings are **Dictate** (STT turns
  speech into an editable draft), **Live** (STT auto-sends and the final reply
  is spoken through the backend TTS), **Off**, and an independent **Share
  screen** toggle. The screen worker captures the selected display, uploads
  frames to `/api/vision/frame`, and attaches the latest frame to the next
  turn. Only one chat can be live at a time; leaving the chat ends capture,
  speech and screen sharing. Live chats opt into keyboard activation
  (`OverlayHandle::set_activatable`) so the dock can take keys; the window
  style alone never steals focus. A fresh hover starts on Overview again.
  Turning the dock off disarms every bus, releases the models and hides the
  island. A hidden island releases its GPU surface after 20s.
- **Background lifetime** (`presence.rs`, `shell.rs`) uses `keep_running`:
  closing the main window hides it and stops capture but keeps live mode
  armed — agents, the tray and the idle island stay alive. `--background`
  starts windowless (tray only) and
  the launcher forwards `STARK_BACKGROUND` across update restarts.
- **Speech models are live-mode owned.** The backend loads Moonshine STT
  lazily on first live use (`session.start`, file transcription, backend
  capture) and `POST /api/voice/release_asr` drops it; TTS warms when the
  first bus arms or a chat selects Live (`POST /api/voice/warm_tts`) and
  releases only once no bus is armed, no chat is live and no reply is speaking
  (`POST /api/voice/release_tts`). The UI's on-device engine
  loads on first local dictation and releases when the models release.
  Stopping capture drops back to armed-idle — models stay warm. An idle,
  disarmed app holds no STT memory in either process; the ~500 MB UI baseline
  is the Rune/GPU/fonts working set, not speech.
- **Tray** (`tray.rs`) owns a native status icon and menu: Open Stark, Listen,
  Show/Hide island, Auto-send voice, Start with Windows and Quit. A second
  launch signals the running UI through `Local\StarkRuneOpen` instead of
  showing an "already running" dialog.
- **Shortcuts** are OS-global: `Ctrl+Space` arms live mode, then toggles
  listening while armed, and `Ctrl+Alt+S`
  opens Stark. Voice finals either stage a reviewable composer draft or send
  immediately, depending on the local **Auto-send voice** preference.
- **Preferences** (`prefs.rs`) live in `.deep_agents/desktop.json`: island
  visibility, auto-send, capture exclusion and Start with Windows. The file is
  local and is not part of chat exports.
- The browser build keeps the in-page voice dialog; native-only overlays, tray
  and global shortcuts are compiled out on wasm.

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
`<harness>/.deep_agents/config/`. Keys come from the active entry in
`<harness>/.deep_agents/secrets/provider_keys.json`, falling back to the
provider's `key_env` variables in `<harness>/.env`.

The desktop provider panel manages those saved keys through local-only
`/api/provider/keys*` routes and reads OpenCode Go allowance from the official
`{base_url}/usage` endpoint. The secrets directory is deliberately outside the
portable backup set. The active key applies on the next run; no server restart
is needed.

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
- `.deep_agents/secrets/` — saved provider API keys; never exported.
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
