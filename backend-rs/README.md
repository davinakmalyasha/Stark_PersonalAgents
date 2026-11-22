# backend-rs

Stark's Rust backend: an OpenAI-compatible streaming provider, the hand-rolled
agent loop, tool dispatch, SQLite/JSONL persistence, and the run/SSE server.
This is the only backend; there is no Python server.

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

## Tool contract

`specs/tool-specs.json` is the model-facing contract: `contract_version` 6,
49 tools, nine scopes (`developer`, `coder`, `general`, `house`, `dfd`,
`orchestrator`, `devops`, `explore`, `ui`). It is validated by
`stark-contract`; implementations are registered in
`stark-tools/src/inventory.rs`. The Rust inventory is the source of truth for
tool names and labels.

## Run the server

`STARK_HARNESS_ROOT` is required — the server fails closed without it. Provider
routing and per-agent configuration live in
`<harness>/.deep_agents/config/providers.json`; environment variables supply
keys only.

```bash
export STARK_HARNESS_ROOT="D:/MyProject/LangChain"
export STARK_PORT=8124
cargo run -p stark-server
```

The server binds `127.0.0.1:8124` by default. Set `STARK_HOST=0.0.0.0` to
expose the API to a phone's Rune client (pairing token required). The Rune client starts a run
with `POST /api/chats/{chat_id}/run`, subscribes to
`GET /api/chats/{chat_id}/events`, and recovers from
`GET /api/chats/{chat_id}/log`. Other routes cover workspaces, chats,
sessions, notes, groups, snapshots, pairing, and voice.

## Crates

- `stark-server` — axum routes, SSE, run admission, pairing.
- `stark-core` — agent loop, tools, delegation, compaction, rollout traces.
- `stark-providers` — OpenAI-compatible streaming, retries, tool-call deltas.
- `stark-tools` — filesystem, shell, patch, MCP, search, browser, retention.
- `stark-store` — SQLite, JSONL sessions, snapshots, backups.
- `stark-policy` / `stark-sandbox` — approvals, prefix rules, process sandbox.
- `stark-contract` — tool-spec schema and validation.
- `stark-voice` — native STT/TTS.

## Disk retention

Harness-generated files under the harness and workspace `.deep_agents/`
directories are swept at startup (in a background task) and hourly
(`stark-tools/src/retention.rs`). Tool output limits are unchanged; only files
on disk are affected, and nothing is model-visible.

| Env var | Default | Effect |
|---|---|---|
| `STARK_RETENTION_SCRATCH_FILES` | `200` | Newest `scratch/*.log`, `*.txt`, `*.png` files kept |
| `STARK_RETENTION_SCRATCH_MB` | `50` | Total scratch bytes kept |
| `STARK_RETENTION_SCRATCH_DAYS` | `7` | Drop scratch files older than this |
| `STARK_RETENTION_SNAPSHOT_RUNS` | `20` | Rewind snapshot runs kept per chat |
| `STARK_RETENTION_SNAPSHOT_MB` | `1024` | Total snapshot bytes kept across workspaces |
| `STARK_RETENTION_SNAPSHOT_DAYS` | `14` | Drop snapshot runs older than this |
| `STARK_RETENTION_IMPORT_DAYS` | `7` | Drop orphan import staging older than this |

Set any value to `0` to disable that dimension. Only files the harness created
are touched: `scratch/*.log`, `scratch/*.txt`, `scratch/*.png`,
`<workspace>/.deep_agents/snapshots/<chat>/<run>/`, and
`.deep_agents/imports/<id>/` (never the staging directory named by
`pending_import.json`). Uploads, rollouts, sessions, memory, and database
backups are never swept. The server logs a warning when the harness drive has
less than 2 GB free.

## Provider and admission knobs

| Env var | Default | Effect |
|---|---|---|
| `STARK_MAX_CONCURRENT_RUNS` | `8` | Process-wide cap on simultaneous agent runs; excess starts get HTTP 429 (`0` disables) |
| `STARK_CONTENT_FILTER_GUIDANCE` | built-in | Text inside `<content_filter_guidance>` after a filtered response |
| `STARK_REQUEST_MAX_RETRIES` | `4` | HTTP handshake retries (server `Retry-After` deadlines are honored, not clamped) |
| `STARK_STREAM_MAX_RETRIES` | `5` | Mid-stream retries; content-filter, `finish_reason: error`, and empty tool-call turns count here |
| `STARK_STREAM_IDLE_TIMEOUT_SEC` | `300` | Abort a stream that sends no data for this long |

## Backups and retries

`chats.db` is snapshotted once per UTC day with `VACUUM INTO`; a restart on the
same day reuses the existing snapshot instead of rewriting it. The newest 7
daily snapshots are kept; a snapshot that fails `PRAGMA quick_check(1)` is
rebuilt.

Retry advice is read from `Retry-After` (seconds or HTTP date) and from
`retry_after_ms` / `retry_after_seconds` / `retry_after` body fields. Deadlines
are honored as sent — there is no local cap — and an SSE error frame carries
its advice into the stream retry.
