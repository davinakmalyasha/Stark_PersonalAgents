# 12 — Setup (Rust only)

## Toolchain

- Rust stable (`rustup update stable`), edition 2024.
- Node 24 + npm for the frontend.
- Windows: MSVC Build Tools for release builds (`x86_64-pc-windows-msvc`);
  GNU MinGW-w64 works for local dev.
- Optional: LLVM `clang-cl` + `lld-link` (or VS Build Tools) and libclang
  for the voice STT bindgen; `cargo-xwin` when building MSVC without VS.

Keep large dirs off C: (`CARGO_TARGET_DIR`, `CARGO_HOME`, `RUSTUP_HOME`).

## Backend

```bash
cd backend-rs
cargo fmt --all
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Run the server for dev:

```bash
cargo run -p stark-server
# STARK_PORT=8124 STARK_HARNESS_ROOT=<repo> cargo run -p stark-server
```

Developer uses Git Bash for the `bash` tool. Install Git for Windows and
ensure `bash.exe` is on `PATH` or at the standard Git for Windows location.
WSL is not required. `read`, `write`, `edit`, `glob`, and
`grep` resolve relative paths from the session cwd (shown in the system prompt
Zone 1 runtime context) and stay inside the workspace root. Per-command
`bash.workdir` overrides without a persistent `cd`.

Web tools:

```bash
# Search (DeepSeek Search API)
STARK_SEARCH_API_KEY=<deepseek-key>
# optional override; defaults to https://api.deepseek.com
STARK_SEARCH_BASE_URL=https://api.deepseek.com
```

`web_fetch` is a local HTTP GET with an SSRF guard; it does not need a search
provider.

Release (MSVC + voice):

```bash
export CARGO_TARGET_DIR=D:/cargo-target/stark-msvc
export XWIN_CACHE_DIR=D:/cargo-target/xwin
cargo-xwin build --release -p stark-server --features voice-stt \
  --target x86_64-pc-windows-msvc
```

Voice model assets (release only):

```bash
node backend-rs/scripts/prepare-voice-models.mjs --jarvis-state path/to/jarvis.safetensors
```

## Frontend

```bash
cd frontend
npm ci
npm run build
```

## Desktop app

```bash
export STARK_SERVER_BIN=<path/to/release/stark-server.exe>
export STARK_MODELS_DIR=<path/to/models>
export STARK_ONNXRUNTIME_DLL=<path/to/onnxruntime.dll>
npm run tauri build
```

The Tauri launcher starts the Rust sidecar automatically; no Python,
no venv, no separate backend process to manage.
