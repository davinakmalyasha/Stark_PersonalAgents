# 03 — Tech Stack

Stark follows Codex's stack: **libraries, not agent frameworks**. Every crate
below is either used by Codex, or is a domain-specific addition needed for
Stark's non-coding tools. Versions are the ones Codex pins in
`codex-rs/Cargo.toml` unless marked otherwise.

---

## 1. Toolchain

| Item | Choice |
|---|---|
| Rust | stable **1.99.x** (1.99.1 at time of writing); `rustup update stable` |
| Edition | **2024** |
| Target | CI: `x86_64-pc-windows-msvc`; local Phase 0: `x86_64-pc-windows-gnu` |
| Linker | CI: MSVC via Visual Studio Build Tools; local: WinLibs MinGW-w64 |
| Build system | **Cargo only** (no Bazel) |
| Formatter/linter | `rustfmt`, `clippy -D warnings` |
| TS bindings | `ts-rs` |
| Schema | `schemars` |
| Async | `tokio` multi-thread |

`rust-toolchain.toml`:

```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy", "rust-analyzer"]
profile = "minimal"
```

Do not force a target in `rust-toolchain.toml`: the host selects GNU locally
and MSVC in CI. Pin the exact patch version in CI (`1.99.1`) for
reproducibility; developers may use newer stable.

Local Phase 0 toolchain lives entirely on D: because C: is the small OS volume:

```text
CARGO_HOME=D:\cargo
RUSTUP_HOME=D:\rustup
CARGO_TARGET_DIR=D:\cargo-target\stark
MinGW=D:\toolchains\winlibs\mingw64\bin
```

The Rust source is identical under either toolchain; only the linker/ABI
environment differs.

---

## 2. Core dependencies

### Runtime / async
| Crate | Version | Why |
|---|---|---|
| `tokio` | `1` (`full`) | Runtime, process, time, sync |
| `futures` | `0.3` | Streams/joins |
| `async-stream` | `0.3.6` | Provider/SSE streams |
| `tokio-stream` | `0.1.18` | Stream adapters |
| `tokio-util` | `0.7.18` | `CancellationToken`, codecs |
| `crossbeam-channel` | `0.5.15` | Blocking bridges (voice, sync code) |
| `async-channel` | `2.3.1` | Bounded async channels |

### HTTP / SSE / WebSocket
| Crate | Version | Why |
|---|---|---|
| `reqwest` | `0.12` (`rustls-tls`, `stream`, `json`) | Provider HTTP |
| `eventsource-stream` | `0.2.3` | Parse OpenAI-compatible SSE |
| `axum` | `0.8` | HTTP server |
| `tower-http` | `0.6` | CORS, static files, tracing (added; Codex uses raw axum + tiny_http) |
| `http` | `1.3.1` | HTTP types |
| `hyper-util` | `0.1.20` | Server plumbing |
| `tokio-tungstenite` | `0.28` | Voice WS proxy |
| `tungstenite` | `0.27` | WS core |
| `url` / `urlencoding` | `2` / `2.1` | URLs |

### Serialization / schema
| Crate | Version | Why |
|---|---|---|
| `serde` | `1` (`derive`, `rc`) | All wire/DB/JSONL types |
| `serde_json` | `1` | JSON |
| `serde_with` | `3.17` | Helpers |
| `schemars` | `0.8.22` | Tool JSON Schema (same as Codex) |
| `ts-rs` | `11` | Generate `frontend/src/types` |
| `toml` / `toml_edit` | `0.9.5` / `0.24` | Config |
| `serde_yaml` | `0.9` | Manifest/YAML |

### Persistence
| Crate | Version | Why |
|---|---|---|
| `sqlx` | `=0.9.0` (`runtime-tokio`, `sqlite-bundled`, `chrono`, `json`, `migrate`, `uuid`, `time`) | SQLite, matching Codex |
| `sqlx-macros` | `=0.9.0` (`derive`, `migrate`) | Migrations |
| `libsqlite3-sys` | `0.37` | Bundled SQLite (WAL fix) |
| `uuid` | `1` (`v4`, `serde`) | IDs |
| `chrono` | `0.4.43` | Timestamps |
| `time` | `0.3.47` | sqlx time support |

Use **runtime queries** (`sqlx::query`) first. Compile-time `query!` macros
require `DATABASE_URL`/offline metadata; add them after the schema stabilizes.

### Errors / logging / observability
| Crate | Version | Why |
|---|---|---|
| `anyhow` | `1` | Binary/server edges |
| `thiserror` | `2.0.17` | Library errors |
| `color-eyre` | `0.6.3` | CLI error reports |
| `tracing` | `0.1.44` | Spans/events |
| `tracing-subscriber` | `0.3.22` (`env-filter`, `json`) | Logging |
| `tracing-appender` | `0.2.3` | File logs |
| `tracing-opentelemetry` | `0.32` | OTel bridge (optional) |
| `opentelemetry` / `-otlp` / `-sdk` | `0.31` | Export (optional) |

### Files / search / parsing / git
| Crate | Version | Why |
|---|---|---|
| `ignore` | `0.4.23` | `.gitignore`-aware walking |
| `globset` | `0.4` | Glob filters |
| `walkdir` | `2.5` | Directory walk |
| `regex` | `1.12.3` | `search_code`, policy |
| `memchr` | `2.7.6` | Fast scans |
| `tree-sitter` | `0.25.10` | Repo map, symbol annotation |
| `tree-sitter-python` | current | Python parsing |
| `tree-sitter-typescript` | current | TS/TSX parsing |
| `tree-sitter-rust` | current | Rust parsing |
| `gix` | `0.89` | Git status/diff (or shell out to `git`) |
| `similar` / `diffy` | `2.7` / `0.4.2` | Diffs/patches |
| `notify` | `8.2` | File watcher |
| `bm25` | `2.3.2` | Search ranking (optional) |

### Policy
| Crate | Version | Why |
|---|---|---|
| `starlark` | `0.14.2` | Codex-compatible `prefix_rule`/`host_executable` policy files |
| `multimap` | `0.10` | Rule index by program |
| `shlex` | `1.3` | Command tokenization and rule examples |

### Process / OS / security
| Crate | Version | Why |
|---|---|---|
| `portable-pty` | `0.9` | Interactive shells (if needed) |
| `vt100` | `0.16.2` | Terminal emulation (optional) |
| `rustix` | `1.1.4` | OS APIs |
| `windows-sys` | `0.61.2` | Windows process/job/screen APIs |
| `winreg` | `0.55` | Registry (deep links) |
| `dirs` | `6` | Known folders |
| `keyring` | `3.6` | Credentials (optional) |
| `landlock` | `0.4.4` | Linux sandbox (later) |
| `seccompiler` | `0.5` | Linux syscall filter (later) |
| `rustls` / `rustls-native-certs` | `0.23` / `0.8` | TLS |

### MCP
| Crate | Version | Why |
|---|---|---|
| `rmcp` | `3.6.x` (Codex pins `=3.5.1` git) | Official Rust MCP SDK |

### CLI / UI (optional)
| Crate | Version | Why |
|---|---|---|
| `clap` / `clap_complete` | `4` | `stark` CLI |
| `ratatui` / `crossterm` | `0.30.2` / `0.29` | TUI if we mirror Codex |
| `pulldown-cmark` / `syntect` | `0.10` / `5` | Markdown/code rendering |

---

## 3. Stark-specific additions (not in Codex)

These are for the domain tools Codex does not have. Keep them isolated in
`stark-tools` so the core stays Codex-like.

| Need | Crate | Notes |
|---|---|---|
| PDF text | `pdf-extract` (or `lopdf`) | `read_pdf`; preserve page selection |
| DOCX | `docx-rs` | `create_docx` |
| XLSX | `rust_xlsxwriter` | `create_xlsx` |
| Screen capture | `xcap` or `windows` + `windows-sys` | `look_at_screen`, screen share |
| Embeddings (later) | `fastembed` | Same BGE-family model; feature-gated |
| Vector search (later) | `sqlite-vec` or `lancedb` | Prefer SQLite extension |
| Telegram (later) | `teloxide` | Only after API parity; sidecar first |
| Voice (later) | `sherpa-rs` / `whisper-rs` / `piper` | Only after Python sidecar is stable |
| Browser automation | **subprocess** to `agent-browser` | Already a Rust binary; do not reimplement |
| GitHub | **subprocess** to `gh` / `git` | Same as Python |
| Docker | **subprocess** to `docker` | Same as Python |

---

## 4. Forbidden dependencies

Do not add these to the Rust workspace:

| Crate | Why forbidden |
|---|---|
| `rig`, `rig-core`, `rig-agent`, `rig-harness` | Agent framework; hides the loop/approvals |
| `swiftide` | Agent/task framework; pre-1.0 |
| `langchain-rust` | Framework; semantics differ |
| `kalosm` | Local-model framework; not a harness |
| `async-openai` | We hand-roll the OpenAI-compatible client |
| `openai` (official SDK wrapper) | Same |
| `langgraph` ports / workflow engines | We hand-roll the durable graph |
| `codex-*` crates | Unpublished `0.0.0` workspace crates |

Infrastructure crates (tokio, reqwest, axum, sqlx, rmcp, tree-sitter, tracing)
are allowed. The line is: **no crate that owns the agent loop, tool policy,
or workflow semantics**.

---

## 5. Workspace `Cargo.toml` sketch

```toml
[workspace]
resolver = "2"
members = ["crates/*", "xtask"]

[workspace.package]
edition = "2024"
license = "Apache-2.0"
version = "0.1.0"

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
futures = "0.3"
async-stream = "0.3.6"
tokio-util = { version = "0.7.18", features = ["rt"] }
reqwest = { version = "0.12", default-features = false, features = ["json", "stream", "rustls-tls"] }
eventsource-stream = "0.2.3"
axum = "0.8"
tower-http = { version = "0.6", features = ["cors", "fs", "trace"] }
serde = { version = "1", features = ["derive", "rc"] }
serde_json = "1"
schemars = "0.8.22"
ts-rs = "11"
sqlx = { version = "=0.9.0", default-features = false, features = [
  "runtime-tokio", "sqlite-bundled", "chrono", "json", "migrate", "uuid", "time",
] }
rmcp = { version = "3.6", features = ["client", "transport-child-process"] }
tracing = "0.1.44"
tracing-subscriber = { version = "0.3.22", features = ["env-filter", "json"] }
anyhow = "1"
thiserror = "2.0.17"
ignore = "0.4.23"
globset = "0.4"
regex = "1.12.3"
tree-sitter = "0.25.10"
notify = "8.2"
similar = "2.7"
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4.43", features = ["serde"] }
clap = { version = "4", features = ["derive"] }
tokio-tungstenite = "0.28"

[profile.dev]
debug = "line-tables-only"

[profile.release]
lto = "thin"
debug = "line-tables-only"
split-debuginfo = "off"
strip = false
codegen-units = 4

[workspace.lints.clippy]
unwrap_used = "deny"
expect_used = "deny"
await_holding_lock = "deny"
redundant_clone = "deny"
needless_collect = "deny"
uninlined_format_args = "deny"
manual_let_else = "deny"
```

---

## 6. Windows specifics

- **Toolchain:** CI uses MSVC (`1.99.1` on `windows-latest`). Local Phase 0 on
  this machine uses GNU (`x86_64-pc-windows-gnu` + WinLibs) on D: because C: is
  the small OS volume. If a dependency requires MSVC, install Build Tools with
  the "Desktop development with C++" workload and `rustup default stable-msvc`.
- **Long paths:** `git config --system core.longpaths true`; keep the repo path
  short (`D:\MyProject\LangChain` is fine).
- **Target dir:** Rust builds are large. Set
  `CARGO_TARGET_DIR=D:\cargo-target\stark`. Also set
  `CARGO_HOME=D:\cargo` and `RUSTUP_HOME=D:\rustup` so the registry and
  toolchains stay off C:.
- **Defender:** exclude `backend-rs\target` and `D:\cargo-target` for build
  speed.
- **Process groups:** use `CREATE_NEW_PROCESS_GROUP` for background commands;
  port `_procman` ownership/cleanup semantics.
- **Paths:** use `PathBuf`; normalize `\`/`/`; compare case-insensitively for
  workspace containment; reject `..` escapes.
- **Line endings:** patch matching must tolerate CRLF/LF, as the Python
  `edit_file` does.
- **Signals:** Ctrl-C is a console control event; use `tokio::signal::ctrl_c`
  and a graceful shutdown path.
- **Single instance:** bind the port; if occupied, exit quietly (current
  behavior).

---

## 7. CI

```yaml
# .github/workflows/rust-validate.yml (Phase 0)
- cargo fmt --all --check
- cargo clippy --workspace --all-targets -- -D warnings
- cargo test --workspace
- python -B scripts/export_tool_specs.py --check   # schema/description parity
```

Optional:
- `cargo audit` (advisories)
- `cargo deny` (licenses/bans)
- `cargo nextest run` (faster test runner)
- `insta` snapshot review in PRs

---

## 8. Dev tooling

| Tool | Command |
|---|---|
| Watch/test | `cargo watch -x test` or `cargo nextest run` |
| Format | `cargo fmt --all` |
| Lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| Docs | `cargo doc --workspace --no-deps --open` |
| Expand macros | `cargo expand` (optional) |
| Coverage | `cargo llvm-cov` (optional) |
| Benchmark | `divan` (Codex uses it) |
