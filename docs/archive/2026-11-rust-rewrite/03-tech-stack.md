# 03 — Tech Stack

Stark follows Codex's stack: **libraries, not agent frameworks**.
Exact versions live in `backend-rs/Cargo.toml` (workspace) — this doc
describes intent, not pins.

---

## 1. Toolchain

| Item | Choice |
|---|---|
| Rust | stable **1.99.x**; `rustup update stable` |
| Edition | **2024** |
| Target | local: `x86_64-pc-windows-gnu`; release: `x86_64-pc-windows-msvc` |
| Build system | **Cargo only** (no Bazel) |
| Formatter/linter | `rustfmt`, `clippy -D warnings` |
| TS bindings | `ts-rs` |
| Schema | `schemars` (tool JSON Schema) |
| Async | `tokio` multi-thread |

`rust-toolchain.toml`:

```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy", "rust-analyzer"]
profile = "minimal"
```

Do not force a target in `rust-toolchain.toml`. Pin the exact patch version
in CI for reproducibility; developers may use newer stable.

Large local paths live on D: because C: is the small OS volume:

```text
CARGO_HOME=D:\cargo
RUSTUP_HOME=D:\rustup
CARGO_TARGET_DIR=D:\cargo-target\stark
```

---

## 2. Core dependencies (by area)

### Runtime / async
`tokio` (full), `futures-util`, `tokio-stream`, `tokio-util`
(`CancellationToken`), `dashmap`.

### HTTP / SSE
`reqwest` (rustls), `eventsource-stream`, `axum`, `tower-http`
(`cors`, `fs`, `trace`).

### Serialization / schema
`serde`, `serde_json`, `schemars`, `ts-rs`, `uuid`, `chrono`.

### Persistence
`sqlx` (pinned `=0.9.0`, `runtime-tokio`, SQLite, `chrono`, `json`).
Runtime `sqlx::query` calls — no compile-time query macros.

### Errors / logging
`anyhow` (binaries), `thiserror` (libraries), `tracing`,
`tracing-subscriber`.

### Files / search
`ignore` (gitignore-aware walking), `globset`, `regex`, `zip`.

### Policy
Hand-rolled read-only command classifier in `stark-policy`
(tokenize → classify → block mutating verbs). No policy engine crate.

### Process / OS
`portable-pty` (ConPTY sessions), `windows-sys` (process/screen APIs).

### Scripting
`rquickjs` (code mode). No host filesystem, network, or timer APIs.

### Voice
`sherpa-rs`-style ONNX runtime via `ort`-equivalent bindings in
`stark-voice`, `hound` (audio IO), `tokio-tungstenite` (voice WS).

### PDF / docs
`pdf-extract`, `zip` (docx/xlsx are zip XML).

---

## 3. Stark-specific additions (not in Codex)

Kept isolated in `stark-tools` so the core stays Codex-like:
screen capture (`xcap`/`windows`), PDF/docs tools, house/DFD bridges,
`browser_act` via the `agent-browser` subprocess.

---

## 4. Forbidden dependencies

Do not add these to the Rust workspace:

| Crate | Why forbidden |
|---|---|
| `rig`, `swiftide`, `langchain-rust`, `kalosm` | Agent frameworks; hide the loop/approvals |
| `async-openai`, `openai` SDK wrappers | We hand-roll the OpenAI-compatible client |
| `langgraph` ports / workflow engines | We hand-roll the durable graph |
| `codex-*` crates | Unpublished `0.0.0` workspace crates |
| Embedding / vector DBs | Rejected: index + Markdown topics are enough at this scale |
| Telegram bot crates | Dropped surface; approval is in-app |

Infrastructure crates (tokio, reqwest, axum, sqlx, tracing) are allowed.
The line is: **no crate that owns the agent loop, tool policy, or workflow
semantics**.

---

## 5. Windows specifics

- **Toolchain:** release uses MSVC; local dev uses GNU + WinLibs on D:.
- **Long paths:** `git config --system core.longpaths true`.
- **Target dir:** `CARGO_TARGET_DIR=D:\cargo-target\stark` (also
  `CARGO_HOME`, `RUSTUP_HOME` off C:).
- **Defender:** exclude build dirs for speed.
- **Paths:** `PathBuf`; normalize `\`/`/`; case-insensitive workspace
  containment; reject `..` escapes.
- **Line endings:** patch matching tolerates CRLF/LF.
- **Single instance:** bind the port; if occupied, exit quietly.

---

## 6. CI

```yaml
# .github/workflows/rust-validate.yml
- cargo fmt --all --check
- cargo clippy --workspace --all-targets -- -D warnings
- cargo test --workspace
```

Plus: frontend `tsc` + Vite build, MSVC clippy/build for releases,
Tauri NSIS bundle, installed smoke test.

Optional: `cargo audit`, `cargo deny`, `cargo nextest`, `insta`.

---

## 7. Dev tooling

| Tool | Command |
|---|---|
| Format | `cargo fmt --all` |
| Lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| Test | `cargo test --workspace` |
| Voice assets | `node backend-rs/scripts/prepare-voice-models.mjs` |
| Docs | `cargo doc --workspace --no-deps --open` |
