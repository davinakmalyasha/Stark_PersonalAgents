# backend-rs

Codex-style Rust backend for Stark. Phase 0 froze the tool contract. Phase 1
adds the vertical slice: OpenAI-compatible streaming provider, hand-rolled
agent loop, five core tools, SQLite/JSONL storage, and the run/SSE server.

## Toolchain

Local development on this machine uses the GNU target on D: because C: is the
small OS volume:

```text
CARGO_HOME=D:\cargo
RUSTUP_HOME=D:\rustup
CARGO_TARGET_DIR=D:\cargo-target\stark
```

The compiler is `rustc 1.99.1` (`x86_64-pc-windows-gnu`) with WinLibs GCC in
`D:\toolchains\winlibs\mingw64\bin`. CI uses the MSVC target on `windows-latest`
to keep the code portable across both Windows toolchains.

## Commands

Run from `backend-rs/`:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Regenerate and verify the frozen contract from the repo root:

```bash
venv/Scripts/python.exe -B scripts/export_tool_specs.py
venv/Scripts/python.exe -B scripts/export_tool_specs.py --check
```

`specs/tool-specs.json` is generated from `tools/_schemas.py`. Do not edit it by
hand. Phase 0 freezes the 42 registered tools and the eight agent scopes.

## Run the Phase 1 server

From `backend-rs/`:

```bash
# Provider (OpenAI-compatible). `.env` is loaded if present.
export STARK_PROVIDER_BASE="https://api.deepseek.com"
export STARK_API_KEY="..."
export STARK_MODEL="deepseek-chat"
export STARK_HARNESS_ROOT="D:/MyProject/LangChain"
export STARK_PORT=8124

cargo run -p stark-server
```

Command provider mode reads the existing `.env` keys directly:

```bash
export STARK_PROVIDER=command   # uses COMMAND_CODE_BASE_URL / API_KEY / MODEL_*
export STARK_HARNESS_ROOT="D:/MyProject/LangChain"
export STARK_PORT=8124
```

The Command mode maps canonical `deepseek-v4.1-flash` to the gateway slug
`deepseek/deepseek-v4.1-flash`.

The Rust server defaults to `127.0.0.1:8124`; Python remains on `:8123`.
Point the Vite dev UI at it with:

```bash
cd ../frontend
STARK_PORT=8124 npm run dev
```

The Vite proxy reads `STARK_PORT`; Electron still starts the Python server
itself, so browser/Vite is the Phase 1 UI path.

Implemented routes: `POST /api/run`, `GET /api/stream/{chat_id}`,
`POST /api/abort`, `GET /api/health`, `GET /api/state`, `GET /api/history`,
`GET /api/runs/active`, `GET /api/ask/pending`, `POST /api/ask/answer`,
`GET /api/models`, `GET /api/provider`.

Session/UI routes share the Python `.deep_agents/workspaces.json` registry and
`chats.db`: `GET/POST /api/workspaces`, `DELETE /api/workspaces/{id}`,
`GET/POST /api/workspaces/{id}/chats`, `GET/DELETE .../chats/{chat_id}`,
`PUT .../title`, `POST .../messages`, `GET/PUT /api/ui/state`,
`POST /api/session/reset`, `GET /api/workspaces/{id}`,
`GET /api/workspaces/{id}/files`, `GET /api/workspaces/{id}/file?path=...`,
`GET .../chats/{chat_id}/subagents`, `GET .../subagents/{child_id}`,
`POST .../chats/{chat_id}/attachments`, `POST .../chats/{chat_id}/rewind`,
`POST /api/chat/{chat_id}/mode`, `GET /api/chat/{chat_id}/state`,
`POST /api/steer`, `POST /api/followup`, `GET /api/inbox`,
`GET /api/chats/{chat_id}/snapshots`,
`POST /api/chats/{chat_id}/snapshots/{snap_id}/restore`,
`POST /api/chats/{chat_id}/runs/{run_id}/restore`,
`POST /api/runs/{run_id}/undo`, `GET/POST /api/notes`,
`GET /api/notes/{scope_type}/{scope_id}`, `PATCH/DELETE /api/notes/{note_id}`. Uploads are capped at
8 files / 20 MB each and stored under `.deep_agents/uploads/`; run prompts
include their safe paths. `edit_file` snapshots the pre-write bytes into
`.deep_agents/snapshots/{chat}/{run}`; rewind restores those files and truncates
the transcript. Shell-created changes are not checkpointed yet. Steering
messages are delivered at the next agent step boundary. Notes are stored in `chats.db`. Pairing uses `.deep_agents/pairing.json`:
loopback clients bypass auth, remote clients must send `X-API-Token`, and
`GET /api/pairing` / `POST /api/pairing/rotate` are loopback-only. Bind
`STARK_HOST=0.0.0.0` to expose the server to a phone. Empty stubs remain for
`/api/groups` and canvas status.

`call_subagent` supports `run`, `spawn`, `status`, `message`, `stop`, and
`list`; child sessions are persisted as `cont-*` chats and exposed through the
subagent routes. `message` queues steering, `stop` cancels the child's token,
and the subagent routes report live child status.

Phase 2 progress: `edit_file` Mode C follows Codex `apply_patch` behavior and
passes Codex's 25 scenario fixtures. `stark-policy` uses a Codex-compatible
Starlark `prefix_rule` engine with `allow` / `prompt` / `forbidden` decisions
and `host_executable` paths; `prompt` decisions use the `ask_user` broker. `run_command` also accepts
`prefix_rule` and `sandbox_permissions=require_escalated`; approved prefixes
are cached for the rest of the run. OS-level sandbox enforcement is not
implemented yet.

`run_command` now has a unified-exec lifecycle: numeric session IDs, optional
`tty` via a patched ConPTY backend, optional `yield_time_ms` (returns a session
ID for still-running commands), resumable `process_action=send` polling, a
64-process cap, and a 1 MiB head/tail output buffer. `process_action=list`,
`logs`, `kill`, `wait`, `ready`, and `kill_all` accept either a session ID or
the background process name. `tty` and `yield_time_ms` are exposed to the Rust
agent at runtime; the frozen Python-derived contract is unchanged. Tool calls are recorded as
`tool_started` / `tool_finished` deltas; chat history survives restart.
