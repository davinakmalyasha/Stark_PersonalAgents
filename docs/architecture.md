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
- **Speech models are live-mode owned.** The backend loads Parakeet Unified
  English (0.6B Q8_0) and Silero VAD v6.2 lazily on first live use
  (`session.start`, file transcription) and `POST /api/voice/release_asr`
  drops both; TTS warms when the first bus arms or a chat selects Live
  (`POST /api/voice/warm_tts`) and releases only once no bus is armed, no chat
  is live and no reply is speaking (`POST /api/voice/release_tts`). The app
  captures PCM only; there is no on-device STT engine. Each completed turn
  passes Hex's 30 ms level gate and then Silero before Parakeet sees it, so a
  click, a fan or a noise-only turn never reaches the model; the client clears
  that turn's interim bubble and keeps capturing. The app sends its saved
  vocabulary (terms plus "sounds like" spellings and the Off/Light/Normal/
  Strong boost) with `session.start`; Parakeet's `ParakeetVocabularyOptions`
  rescores the decode and its exact-spelling pass fixes listed mishearings.
  Stopping capture drops back to armed-idle. An idle, disarmed app holds no
  STT memory; the ~500 MB UI baseline is the Rune/GPU/fonts working set, not
  speech.
- **Tray** (`tray.rs`) owns a native status icon and menu: Open Stark, Listen,
  Show/Hide island, Auto-send voice, Start with Windows and Quit. A second
  launch signals the running UI through `Local\StarkRuneOpen` instead of
  showing an "already running" dialog.
- **Shortcuts** are OS-global: `Ctrl+Space` arms live mode, then toggles
  listening while armed, and `Ctrl+Alt+S`
  opens Stark. Voice finals either stage a reviewable composer draft or send
  immediately, depending on the local **Auto-send voice** preference.
- **Preferences** (`prefs.rs`) live in `.deep_agents/desktop.json`: island
  visibility, auto-send, capture exclusion, Start with Windows, and the shared
  Parakeet vocabulary (one term per line, optional `|` sounds-like spellings)
  with its boost. The file is local and is not part of chat exports.
- The browser build keeps the in-page voice dialog; native-only overlays, tray
  and global shortcuts are compiled out on wasm.

State is plain Rust. Host traits (`TranscriptHost`, `CanvasHost`,
`ProjectsHost`, `SettingsHost`, and related traits) let shared views resolve the
active thread or panel without borrowing the whole app. Handlers capture stable
IDs or keys, never row indices.

### Group projects

Group projects are independent SQLite databases under
`.deep_agents/groups/<id>/.gp/state.db`. The database is metadata only; agents
execute in the project's explicit **work folder**. A project owns member slots
(one row per unit), a current cycle, milestones/items, a schedule/repeat anchor,
planner state, per-run usage, and a durable message/event log. The runner is
capacity-aware (per-slot and `max_parallel`), retries failed items, blocks the
project when retries are exhausted, and creates a fresh plan copy for each
repeat cycle while keeping prior cycles visible. See
[`group-projects.md`](group-projects.md) for the lifecycle and API.

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
contract. Tools are grouped into scopes such as `developer`,
`general`, `house`, `dfd`, `orchestrator`, `devops`, `explore`, and `ui`.

Dispatch passes through the policy and plan guards, the question broker, the
process manager, and MCP routing. Tool output is bounded before it reaches the
model.

### Agent registry

`stark-core/src/agent_registry.rs` is the runtime source of truth. It merges
the eight compiled `AgentSpec` seeds with custom records from
`.deep_agents/config/agents/*.json` into one `EffectiveAgent` shape used by the
picker, run selection, prompts, tools, access, delegation, schedules, and
groups.

- Built-ins are immutable seeds with compiled access, prompt-profile,
  delegation, and tool caps. A config file can narrow but never widen them.
- Custom agents have a closed tool list. Missing `tools` is an error; an
  explicit `[]` is a talk-only agent. Omitted `skills`/`mcp` means none for
  custom agents, not all. The per-agent skill allowlist is enforced both in
  the prompt catalog and at the `skill` dispatch gate.
- Invalid definitions are quarantined and reported; they are never partially
  applied.
- Aliases resolve at read time and are never listed. `coder` remains an alias
  for `developer`.
- Every run snapshots its runtime record. A chat stores its resolved
  `agent_key`; deleting that custom agent makes the next run fail explicitly
  instead of silently falling back to a more privileged built-in.
- Custom agents can opt into base Code Mode with `code_mode: "hybrid"` plus
  `execute` in their closed tool list. `execute` is the capability marker;
  custom `only`/`full_codemode` are rejected and stay built-in Developer
  features. Built-in Code Mode defaults are unchanged.
- Custom routing is opt-in and only used when no explicit agent/chat identity
  exists. A unique workspace-kind/path/repo match wins; ambiguous matches fall
  back to the built-in heuristic with a warning. Paths are normalized absolute
  globs: separators and Windows verbatim prefixes are canonicalized, matching
  is case-insensitive on Windows/macOS, and short/long Windows path forms are
  both tried. Invalid or relative rules are dropped with a warning at load
  instead of quarantining the agent. `routing.repos` matches a normalized
  `host/owner/repo` git remote; path and repo rules are ANDed when both exist.
- A workspace can be pinned to an agent explicitly through
  `PUT /api/workspaces/{id}/agent-binding`. Binding is never created
  automatically. Precedence is explicit run agent → chat `agentKey` →
  workspace binding → routing → built-in fallback.
- The registry is cached by `ConfigStore::revision()` (file mtimes/sizes), so
  the picker and a run see the same generation without parsing JSON on every
  request. Every effective agent carries a `definition_hash`; chats store the
  hash of their last run so a changed agent surfaces as `agent_changed`.
- Dispatch checks the effective ACL for every non-MCP tool. An unknown scope
  without an explicit ACL exposes nothing, not the full inventory.

The builder contract is documented in `docs/agent-builder-api.md` and
`backend-rs/specs/agent-def.schema.json`.

### Code Mode

The coding scope (`developer`) defaults to OpenCode's base Code Mode.
Core tools (`bash`, `read`, `apply_patch`, `edit`, `glob`, `grep`) stay on the
direct provider surface; the long-tail tools go into the `# Code Mode`
catalog and are called through `execute` alongside the direct-only tools
(`job`, `update_plan`, `skill`, `ask_user_question`, and delegation). Base
Code Mode exposes the `fetch` global only. `full_codemode` is opt-in and
defaults off, matching OpenCode release builds; enabling it pins the file and
shell tools into the catalog, hides `glob`/`grep`, adds the script
`timeout`/`background` inputs, and exposes `path` plus
Bun's `Bun.file`/`Bun.write`/`Bun.Glob`, exactly like OpenCode's full Code
Mode. `code_mode: "only"` is the execute-only full surface; `hybrid` is the
base surface; `off` removes `execute`.

`execute` runs the program in a Bun sidecar (`backend-rs/codemode-worker`,
Bun + `@opencode/codemode` on psychoscript) and is a dispatcher, not a
writer: nested calls cross back into Rust `dispatch` and pass through the same
plan, read-only, approval, MCP, cancellation, and output-bound gates as direct
calls. `fetch` and the Bun file APIs also cross back into Rust for
`webfetch`/`read`/`edit`/`external_directory` approval and plan/read-only
guards. MCP tools appear in programs under dotted paths (`github.get_issue`
for `mcp__github__get_issue`). A direct call to a catalog-only tool is
transparently repaired into an implicit one-call program and returns a
`<tool-repair>` note. With `full_codemode`, foreground `execute` blocks up to
`timeout` (default 2 minutes); slower scripts move to the background, return
`Script cm_... is still running; ...` with a job ID, and report through
`<background-result kind="codemode" ...>` follow-ups. OpenCode does not resume
Code Mode programs across a server restart: a running script becomes
`Interrupted by server restart.` and the server delivers that notice once at
boot. Its durable row is deleted only after the notice is queued, so a crash
before delivery produces it again instead of losing it. Workers are scoped per
chat (idle managers are reaped, and a process cap falls back to the
least-recently-used worker), so a synchronous program in one conversation
cannot block or kill another. Each `execute`
carries its own inventory key; the sidecar caches runtimes per inventory
(OpenCode's `WeakMap<Inventory, Runtime>` equivalent) instead of letting the
last `initialize` decide what a concurrent session sees. Because that sidecar
outlives sessions, it retains the four most recent inventories rather than an
unbounded set of language runtimes; this is transport-internal and never
model-visible. Production prefers a standalone `codemode-worker` binary built
with `bun build --compile`; Bun + source is the development fallback. The
server reports availability/runtime in `/api/health`. A delivered
background job is removed from memory and disk; no additional age or row cap
is applied. Like OpenCode, a script that stops yielding is not killed by the
two-minute foreground block; it only moves to the background. Stop is still
reliable: the host waits two seconds for the worker to acknowledge `cancel`,
then kills the Bun child (failing concurrent runs in that worker) and respawns
it lazily on the next call. Built-in
Developer config can set `code_mode` to `off`, `hybrid`, or `only`;
`off` restores the full direct surface and removes `execute`, while
`hybrid` keeps the base surface (core tools direct, long-tail through
`execute`, fetch only, no script timeout/background). Custom agents may opt
into `hybrid` by listing `execute`; their program catalog is built from the
same closed tool ACL.

The Code Mode catalog is an instruction source with OpenCode's split
delivery. The first observation is the chat's frozen epoch baseline: it is
rendered from the initial catalog, placed in the system prompt, and never
rewritten. A later `CodeModeInstructions.update`/`removed` delta is rendered
once and stored as a chronological system message, which the provider lowers
to OpenCode's XML-escaped `<system-update>` user text (merging into an
adjacent user message when one is last). The update precedes the user turn,
matching `InstructionState.prepare` before input promotion. A complete-to-partial
transition adds the `search` note; a removed tool/namespace gets OpenCode's
removal line; re-enabling after removal restates the full catalog as an update,
never as the baseline. A completed compaction advances the epoch like OpenCode's
`InstructionState.advanceEpoch`: the current catalog becomes the new baseline
and the in-memory system prompt is rebuilt for the next step, so updates that
fell out of history are not lost.

While a script runs, the worker publishes throttled `tool_progress` deltas
(at most one per 100 ms, with a trailing update) carrying the running
`toolCalls` rows and file diffs. The payload is OpenCode's `context.progress`
payload; only the transport differs (Stark has a dedicated delta, OpenCode
attaches the same data to the running tool call's state). The final result
carries the complete rows and diffs.

Every `execute` result and every background job notice passes through
OpenCode's `ToolOutput.truncate`: at most 2,000 lines and 50 KiB of UTF-8,
cut once at the line/byte boundary without splitting a grapheme, followed by
the exact `[showing lines …; full output saved to …]` marker. The full text
is spilled under `.deep_agents/tool-output/tool_<identifier>` and swept with
OpenCode's seven-day, hourly cleanup for that directory. This is the post-tool
stage OpenCode applies to all normalized tool results; without it a program
returning a large array would put megabytes into history and the next provider
request. The absolute directory is the harness data root and the identifier
carries OpenCode's random 12-hex/14-base62 tail, so fixtures substitute exactly
`tool-output/tool_FILE` for that one data field and byte-compare the rest of
the marker. The save-failure variant formats the runtime's OS error text and is
therefore not fixture-claimed across platforms.

Byte parity with OpenCode v2.4.1 is fixture-proven for the Code Mode surface:

- The catalog baseline, every update/removed block, the search-note
  transition, and the `<system-update>` wrapper are generated by the real
  OpenCode source under Bun and replayed against the Rust port
  (`backend-rs/codemode-worker/test/opencode-parity/`).
- `execute` and `apply_patch` custom-tool conversion is compared across every
  freeform format combination. `apply_patch` uses OpenCode's fixed
  `patch-grammar.txt` bytes when the model advertises Lark; text-only models
  keep the JSON tool.
- Freeform parse, exact `// @options:` header replay, malformed-header
  handling, streamed JSON fragments, and function-call argument normalization
  are compared with the real `OpenAIFreeform` helpers.
- `ToolOutput.truncate` is compared with the real source over line, byte,
  CRLF, and emoji/grapheme boundary cases, including the spill marker.

Not all provider-envelope fields are identical, because they depend on model
policy outside Code Mode: OpenCode computes `max_tokens` from its
models.dev output limits and context estimate, while Stark leaves the output
cap to the provider unless `STARK_MAX_OUTPUT_TOKENS` is set; Stark always sends
its configured temperature, while OpenCode only sends one the selected
model/agent sets; cache/store options follow each harness's provider layer.
Stark omits `tool_choice`, matching OpenCode's normal requests. These fields do
not change the Code Mode tools, messages, or results.

OpenCode declares `execute` freeform (`{ type: "text", field: "code" }`) and
uses OpenAI custom tools only when the selected model advertises a freeform
format. Stark models can declare `"freeform": ["text", "lark"]`; the provider
then sends `execute` as an OpenAI custom tool: plain text for `{ code }`, and
for `{ code, timeout, background }` either unconstrained text with the
`// @options:` first line or the exact Lark grammar when the model advertises
`lark`. It parses custom tool calls back into the normal JSON arguments and
replays past calls as custom calls. Models without the capability keep the JSON function
form, exactly as OpenCode does. The Zen `deepseek-v4.1-flash` model used here
advertises no freeform format, so it still receives the JSON function tool.

### Skills

Skills are directories at `.agents/skills/<name>/SKILL.md` (workspace or
harness level). The catalog is injected into the prompt; the model loads a
skill with the `skill` tool. Agent configuration can restrict the visible
catalog. Workspace skills are trusted instructions from the repository;
symlinked skill directories and `SKILL.md` files are ignored in every root so
a cloned repo cannot point at an arbitrary host file. An agent may pin a skill
to the SHA-256 of its `SKILL.md`; a changed pinned skill is refused at
dispatch. The preview API returns each resolved skill's source and hash.

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

Security/operations events also land in the `audit_events` table: route
decisions, ACL and skill denials, skill loads, worker kills, and agent
create/delete/reset. Audit rows carry keys, hashes, and reasons only — never
prompt text or file contents. `GET /api/audit` exposes a bounded, filterable
view, and rows older than 90 days are pruned at store open.

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
- The Code Mode worker is built with `bun run build` into a standalone
  `codemode-worker` binary (`backend-rs/codemode-worker/dist/`). The server
  discovers it next to itself, under `resources/`, or via
  `STARK_CODEMODE_WORKER_BIN`; dev checkouts fall back to Bun + source.
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
