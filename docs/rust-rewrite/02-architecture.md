# 02 — Architecture

Target layout, crate boundaries, runtime topology, and the rules that keep the
rewrite maintainable.

---

## 1. Placement

```
D:\MyProject\LangChain\
├── server/                  # Python backend (unchanged, reference)
├── agents/ llm/ tools/ ...  # Python (unchanged)
├── frontend/                # React/Electron (unchanged)
├── backend-rs/              # NEW Rust workspace
│   ├── Cargo.toml           # workspace
│   ├── rust-toolchain.toml
│   ├── crates/
│   ├── specs/
│   ├── tests/parity/
│   └── xtask/               # optional dev tasks
└── docs/rust-rewrite/       # these docs
```

- The Rust backend lives on the `rust-rewrite` branch.
- Python remains runnable on `:8123`; Rust runs on `:8124` during migration.
- The Electron shell can be pointed at either port via an env var.
- Cutover flips the port and removes the Python startup path; the Python tree
  can then be deleted or archived.

---

## 2. Workspace crates

Start with 8 crates; split only when there is a reason.

```
backend-rs/crates/
├── stark-contract/     # shared types: AppState, Delta, SSE, ToolSpec, errors
├── stark-providers/    # OpenAI-compatible client, SSE, model registry, fallback
├── stark-core/         # RunCtx, AgentLoop, event bus, cancellation, compaction
├── stark-tools/        # Tool trait, registry, dispatcher, built-in tools
├── stark-policy/       # command policy, fs permissions, envelope, approvals, sandbox
├── stark-store/        # sqlx SQLite, JSONL sessions, snapshots, migrations
├── stark-agents/       # AgentRegistry, routing, orchestrator graph, delegation
├── stark-groups/       # event-sourced group projects + supervisor
├── stark-memory/       # repo map, summaries, embeddings (feature-gated)
├── stark-server/       # axum routes, SSE/WS, static files, pairing
└── stark-cli/          # optional headless CLI / TUI
```

### Dependency direction

```
stark-contract  ◄────────────── everything
stark-providers ◄────── stark-core
stark-policy    ◄────── stark-tools
stark-store     ◄────── stark-core, stark-groups
stark-tools     ◄────── stark-core, stark-agents
stark-core      ◄────── stark-agents, stark-groups, stark-memory
stark-agents    ◄────── stark-server
stark-groups    ◄────── stark-server
stark-memory    ◄────── stark-agents, stark-server
stark-server    ◄────── stark-cli
```

Rules:
- No cycles.
- `stark-contract` has no runtime dependencies beyond `serde`, `schemars`,
  `ts-rs`, `uuid`, `chrono`, `thiserror`.
- `stark-core` never imports `stark-server` or `stark-cli`.
- `stark-tools` never imports `stark-agents` or `stark-groups` (tools receive
  data, not orchestration).
- `stark-policy` is pure where possible: `classify()` takes data and returns a
  decision; only `sandbox` touches the OS.
- `stark-store` owns all SQL; no other crate writes SQL directly.

---

## 3. Runtime topology

One process, one Tokio multi-thread runtime.

```
stark-server (axum)
 ├─ POST /api/run ─────────────► RunManager
 │                                 ├─ one task per active run
 │                                 ├─ AgentLoop (stark-core)
 │                                 ├─ ToolDispatcher (stark-tools)
 │                                 └─ Provider (stark-providers)
 ├─ GET /api/stream/{chat} ─────► SSE (broadcast + replay)
 ├─ POST /api/steer|followup|abort
 ├─ /api/workspaces|chats|groups|notes|house|...
 ├─ WS /api/voice/ws ───────────► proxy to Python voice sidecar (:8125)
 ├─ GroupSupervisor task ───────► 15s tick (stark-groups)
 ├─ ScheduleScheduler task ─────► 1s tick  (stark-store + core)
 └─ WakeDispatcher task ────────► due wakes (stark-store + core)
```

### Process boundary during migration

```
Electron/React ──► Rust :8124 ──► Python voice :8125
                       │
                       └──► Python Telegram bot (sidecar) ──► Rust API
```

- Voice stays Python (Moonshine/Pocket TTS).
- Telegram stays Python; it only needs the HTTP API to remain stable.
- Once parity is proven, the sidecars can be ported (`sherpa-rs`, `teloxide`).

---

## 4. Concurrency model

| Concern | Mechanism |
|---|---|
| Active run per chat | `DashMap<ChatId, RunHandle>` + `RunGuard` drop cleanup |
| Cancellation | `tokio_util::sync::CancellationToken` per run |
| Steering | `mpsc::Receiver<SteerMessage>` checked between steps |
| SSE fan-out | `tokio::sync::broadcast::Sender<ServerEvent>` per chat |
| State snapshots | `RwLock<AppState>` per chat + version counter |
| Approvals | `DashMap<ApprovalId, oneshot::Sender<Decision>>` |
| Group supervisor | one dedicated task; `tokio::time::interval(15s)` |
| Schedules/wakes | one task each; `interval(1s)`; DB-backed due queries |
| SQLite | `sqlx::SqlitePool` (WAL, busy_timeout); single writer for group events |
| CPU work | `spawn_blocking` (tree-sitter, embeddings, hashing, file IO) |
| Subprocesses | `tokio::process` + per-run process registry (`_procman` semantics) |

Never hold a `std::sync::Mutex` across `.await`. Use `tokio::sync` for async
locks. Clippy denies `await_holding_lock`.

---

## 5. Data flow

### 5.1 One run

```
POST /api/run
  validate chat/workspace
  create RunCtx (deps: path, chat, role, allowlist, window, caps, envelope)
  persist user message (chat_store) + append session entry
  create run task
  return SSE stream

run task:
  route(prompt, ctx) -> AgentKind
  build agent (provider, tools, system prompt, history)
  loop:
    provider.stream_chat(messages, tool_specs_tools)
      emit Delta(text/reasoning/usage)
      if tool calls:
        emit Delta(tool_started)
        dispatcher.call(tool, args, ctx)   // policy + approval + execute
        emit Delta(tool_finished)
        append tool result
    compact if over budget
    check cancellation / steering
  persist assistant message + session entries
  finalize snapshot/checkpoint
  emit State(final) + Done
```

### 5.2 State

`AppState` is rebuilt from the run + chat + plan + MCP status. The server
keeps a per-chat snapshot so `/api/stream/{chat}` can replay the latest state
before subscribing.

### 5.3 Group projects

```
tool (group_update) → op row → supervisor tick → one event + projection
supervisor → dispatch item → child run → item result → verification
events → SSE/board reads
```

The LLM never writes projections directly. The only mutation channel is an op.

---

## 6. Error model

| Layer | Type |
|---|---|
| Libraries | `thiserror` enums, specific variants |
| Binaries/server | `anyhow::Result` at the edge |
| Tool failures | `ToolResult { is_error, content, exit_code }` — never panic |
| Provider failures | `ProviderError` (auth, rate limit, transport, protocol, timeout) |
| Policy refusal | `Decision::Blocked(reason)` / `NeedsApproval` |
| HTTP | JSON `{ "error": "...", "detail": "..." }` with status codes |

Rules:
- No `unwrap()`/`expect()` in production code (clippy denies them, as Codex
  does).
- A tool panic is caught at the dispatcher and converted to `[ERR]` text.
- A supervisor tick catches per-project errors and never dies.
- A run error persists `error`/`traceback` into the final `AppState` (parity).

---

## 7. Configuration

| Source | Use |
|---|---|
| Environment | Provider keys, ports, paths, feature flags |
| `.deep_agents/` | Existing runtime config (MCP, workspaces, sessions) |
| `backend-rs/config.toml` | Rust-only defaults (optional) |
| `PROJECT.md` | Group-project envelope/manifest |
| `agents/specialists/*.json` | Specialist profiles (unchanged) |

Precedence: explicit env > project/manifest > mode defaults > Rust defaults.
Do not introduce a second source of truth for provider/model pins while the
Python modes still exist.

### Feature flags (`stark-server` / `stark-core`)
- `voice-sidecar` — proxy voice endpoints to Python (default on during migration)
- `telegram-sidecar` — health/status only (bot runs outside)
- `embeddings` — fastembed + vector store (default off)
- `sandbox-windows` — Windows sandbox enforcement (default off until Phase 6)
- `otel` — OpenTelemetry export (default off)
- `tui` — ratatui CLI (default off)

---

## 8. Build profiles and lints

Mirror Codex (`codex-rs/Cargo.toml`):

```toml
[profile.dev]
debug = "line-tables-only"

[profile.release]
lto = "thin"
debug = "line-tables-only"
split-debuginfo = "off"
strip = false
codegen-units = 4
```

Workspace lints (subset):
- `clippy::unwrap_used = "deny"`
- `clippy::expect_used = "deny"`
- `clippy::await_holding_lock = "deny"`
- `clippy::redundant_clone = "deny"`
- `clippy::needless_collect = "deny"`
- `clippy::uninlined_format_args = "deny"`

CI gate: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace`, `cargo audit` (optional), tool-spec parity.

---

## 9. Port strategy

**Strangler, side-by-side, contract-first.**

1. Rust runs on `:8124`; Python stays on `:8123`.
2. Frontend Vite proxy reads `STARK_PORT` (default `8123`) to choose. Electron
   still launches the Python server, so Phase 1 browser/Vite uses `STARK_PORT=8124`.
3. Every ported endpoint gets a contract test against the Python response.
4. A run fixture is replayed through both and normalized-diffed.
5. Cutover is a config flip, not a big-bang rewrite.
6. After a stable period, remove Python startup and archive the tree.

This is slower than a hard cutover but it is the only way to preserve 42 tool
contracts, ~90 routes, and an event-sourced state machine without a long
regression tail.

---

## 10. Non-goals

- No new UI framework.
- No new database engine.
- No microservices; one binary + sidecars.
- No agent framework.
- No behavior redesign while porting. Improvements go through a "deviation"
  entry and a separate change.
- No cross-platform desktop work in Phase 1; Windows first.
