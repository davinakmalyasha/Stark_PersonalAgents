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

The Rust server defaults to `127.0.0.1:8124`; Python remains on `:8123`.
Point the UI at it with:

```bash
VITE_API_BASE=http://127.0.0.1:8124 npm run dev
```

Implemented routes: `POST /api/run`, `GET /api/stream/{chat_id}`,
`POST /api/abort`, `GET /api/health`, `GET /api/state`, `GET /api/history`,
`GET /api/runs/active`, `GET /api/ask/pending`, `POST /api/ask/answer`,
`GET /api/models`, `GET /api/provider`.

Phase 1 limits: `run_command` is foreground-only, `edit_file` supports Modes
A/B, `ask_user` uses the question broker/UI polling, and there is no policy or
sandbox yet (Phase 2). Tool calls are recorded as `tool_started` /
`tool_finished` deltas and chat history survives restart.
