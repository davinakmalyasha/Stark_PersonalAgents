# 02 — Architecture

Target layout, crate boundaries, runtime topology, and the rules that keep the
codebase maintainable. This describes the current Rust system.

---

## 1. Placement

```
D:\MyProject\LangChain\
├── backend-rs/              # Rust workspace (the only backend)
│   ├── Cargo.toml           # workspace
│   ├── crates/
│   ├── specs/               # frozen tool contract
│   └── scripts/             # release tooling (Node)
├── frontend/                # React/Tauri (unchanged product surface)
└── docs/rust-rewrite/       # these docs
```

- The Tauri shell launches the `stark-server` sidecar binary.
- The same binary serves the SPA + API for remote/mobile mode.
- Port defaults: `8123` desktop, `8124` dev. `STARK_BIND` is an alias of `STARK_HOST`.

---

## 2. Workspace crates

```
backend-rs/crates/
├── stark-contract/     # shared types: AppState, Delta, SSE, ToolSpec, errors
├── stark-providers/    # provider client, SSE, model registry, affinity
├── stark-core/         # RunCtx, agent loop, worktrees, memory consolidation, delegation
├── stark-tools/        # Tool trait, registry, dispatcher, built-in tools, code mode
├── stark-policy/       # read-only command policy, approvals
├── stark-store/        # sqlx SQLite, JSONL sessions, memory, groups, schedules
├── stark-server/       # axum routes, SSE, static files, pairing, loops
└── stark-voice/        # native STT/TTS (Moonshine + Pocket TTS)
```

### Dependency direction

```
stark-contract  ◄────────────── everything
stark-providers ◄────── stark-core
stark-policy    ◄────── stark-tools
stark-store     ◄────── stark-core, stark-server
stark-tools     ◄────── stark-core
stark-core      ◄────── stark-server
stark-voice     ◄────── stark-server (feature: voice-stt)
```

Rules:
- No cycles.
- `stark-contract` has no runtime dependencies beyond `serde`,
  `ts-rs`, `uuid`, `chrono`, `thiserror`.
- `stark-core` never imports `stark-server`.
- `stark-tools` is dispatch + tools; orchestration lives in `stark-core`.
- `stark-policy` is pure where possible: `check_read_only()` takes a command
  and returns a decision; nothing else touches the OS for policy.
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
 ├─ /api/workspaces|chats|groups|notes|house|memory|agents|...
 ├─ GET /{*path} ───────────────► static frontend/dist (SPA fallback)
 ├─ AgentSupervisor task ───────► 1s mailbox tick (durable agents)
 ├─ GroupSupervisor task ───────► 15s tick
 ├─ ScheduleScheduler task ─────► due schedules
 └─ WakeDispatcher task ────────► due canvas wakes
```

No sidecars. Voice runs in-process. The only external processes are MCP
stdio servers and user-configured commands.

---

## 4. Concurrency model

| Concern | Mechanism |
|---|---|
| Active run per chat | `DashMap<ChatId, RunHandle>` |
| Cancellation | `tokio_util::sync::CancellationToken` per run |
| Steering | Lock-free queues checked between steps |
| SSE fan-out | `tokio::sync::broadcast::Sender<ServerEvent>` per chat |
| State snapshots | `Mutex<Option<AppState>>` per run + version counter |
| Approvals | In-app question broker (`ask_user`) |
| Group supervisor | one dedicated task; `tokio::time::interval(15s)` |
| Agent loop | one task; `interval(1s)`; DB-backed due queries |
| SQLite | `sqlx::SqlitePool` (WAL, busy_timeout) |
| CPU work | `spawn_blocking` (file IO, snapshots) |
| Subprocesses | per-run process registry with named sessions |

Never hold a `std::sync::Mutex` across `.await`. Clippy denies
`await_holding_lock`.

---

## 5. Data flow

### 5.1 One run

```
POST /api/run
  validate chat/workspace
  ensure agent record (running)
  create RunCtx (workspace, scope, depth, file access)
  persist user message + append session entry
  create run task
  return SSE stream

run task:
  build system prompt (AGENTS.md + memory index)
  loop:
    provider.stream_chat(messages, tool_specs)
      emit Delta(text/reasoning/usage)
      if tool calls:
        emit Delta(tool_started)
        dispatcher.call(tool, args, ctx)   // policy + approval + execute
        emit Delta(tool_finished)
        append tool result
    compact if over budget
    check cancellation / steering / budgets
  persist assistant message + session entries
  record stage-1 memory; maybe consolidate in background
  persist terminal agent status BEFORE done
  emit State(final) + Done
```

### 5.2 State

`AppState` is rebuilt from the run + chat + plan + MCP status. The server
keeps a per-chat snapshot so `/api/stream/{chat}` can replay the latest state
before subscribing.

### 5.3 Durable agents

```
agent tool (spawn) → agent row + mailbox trigger → supervisor tick → claim
→ child run in worktree → auto-commit → store.finish → notify parent
→ parent steered or queued → merge/discard via tool or API
```

### 5.4 Group projects

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
| Tool failures | `ToolOutcome { is_error, content }` — never panic |
| Provider failures | `ProviderError` (auth, rate limit, transport, protocol, timeout) |
| Policy refusal | `[BLOCKED] ...` tool text |
| HTTP | JSON `{ "error": "..." }` with status codes |

Rules:
- No `unwrap()`/`expect()` in production code (clippy denies them, as Codex
  does).
- A run error persists `error` into the final `AppState`.
- A supervisor tick catches per-agent/per-project errors and never dies.

---

## 7. Configuration

| Source | Use |
|---|---|
| Environment | Provider keys, ports, paths, feature flags |
| `.deep_agents/` | Runtime state (chats.db, memory, workspaces, sessions) |
| `AGENTS.md` hierarchy | Manual project instructions (global + nested) |

Precedence: explicit env > workspace state > built-in defaults.

### Environment

- `STARK_PORT` (default `8123` desktop, `8124` dev)
- `STARK_HOST` / `STARK_BIND` (default `127.0.0.1`; `0.0.0.0` for remote)
- `STARK_FRONTEND_DIST` (override the served SPA dir)
- `STARK_PROVIDER`, `STARK_PROVIDER_BASE`, `STARK_API_KEY`, `STARK_MODEL`
- `STARK_MEMORY`, `STARK_MEMORY_MAX_CHARS`, `STARK_MEMORY_CONSOLIDATE_AFTER`, `STARK_MEMORY_MODEL`
- `STARK_WORKTREES` (`auto` / `required` / `off`)
- `STARK_RUN_MAX_SECONDS`, `STARK_RUN_MAX_TOKENS`, `STARK_RUN_MAX_TOOL_CALLS`

---

## 8. Build profiles and lints

```toml
[profile.dev]
debug = "line-tables-only"

[profile.release]
lto = "thin"
debug = "line-tables-only"
```

Workspace lints (subset):
- `clippy::unwrap_used = "deny"`
- `clippy::expect_used = "deny"`
- `clippy::await_holding_lock = "deny"`

CI gate: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace`, plus the `stark-contract` frozen tool-spec test and a
frontend `tsc` + Vite build.

---

## 9. Non-goals

- No new UI framework.
- No new database engine.
- No microservices; one binary.
- No agent framework.
- No vector embeddings or repo-map injection.
- No cross-platform desktop work beyond Windows-first builds.
