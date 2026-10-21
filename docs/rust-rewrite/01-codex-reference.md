# 01 — Codex Reference

Reference: [openai/codex](https://github.com/openai/codex), Apache-2.0.
This document records how Codex is built, what we copy, and what we
deliberately leave behind.

Codex is a **coding agent CLI/harness**, not the cloud Codex service. The Rust
implementation lives in [`codex-rs/`](https://github.com/openai/codex/tree/main/codex-rs).

---

## 1. Scale and shape

| Fact | Value |
|---|---|
| Stars | ~130k |
| Commits | ~12.5k |
| License | Apache-2.0 |
| Rust workspace | ~170 crates, `edition = "2024"` |
| Build | Cargo **and** Bazel (`BUILD.bazel`, `MODULE.bazel`) |
| Distribution | Prebuilt binaries per platform; npm/Homebrew wrappers |
| Legacy | `codex-cli/` TypeScript wrapper remains; the engine is Rust |

`codex-rs/Cargo.toml` marks every crate `version = "0.0.0"`. **The crates are
not published**; `cargo add codex-core` will not work. The workspace is a
monorepo, not a library.

---

## 2. Crate architecture

Codex is organized by responsibility, not by layer. The important groups:

### 2.1 Engine
| Crates | Responsibility |
|---|---|
| `core`, `core-api`, `core-plugins` | Agent loop, model client, tool dispatch, conversation state, safety |
| `protocol` | Typed wire messages between engine and frontends |
| `app-server`, `app-server-protocol`, `app-server-transport`, `app-server-daemon`, `app-server-client` | JSON-RPC server so TUI, IDE, and desktop drive one engine |
| `exec`, `exec-server`, `exec-server-protocol` | Headless execution |
| `cli` | Argument parsing/entry point |
| `tui` | `ratatui` terminal UI |

### 2.2 Tools and code intelligence
`tools`, `apply-patch`, `file-search`, `file-watcher`, `git-utils`,
`shell-command`, `tree-sitter` (+ Bash/PowerShell grammars), `bm25`,
`utils/output-truncation`, `utils/fuzzy-match`, `utils/stream-parser`.

### 2.3 Policy and sandboxing
`execpolicy` (Starlark policy), `sandboxing`, `linux-sandbox` (Landlock +
seccomp + bwrap), `windows-sandbox-rs`, `windows-sandbox-service`,
`process-hardening`, `network-proxy`, `secrets`, `guardian-context`,
`guardian-reviewer`, `guardian-v2`.

### 2.4 Agent extensions
`ext/agent`, `ext/agent-message-board`, `ext/mcp`, `ext/memories`,
`ext/skills`, `ext/web-search`, `ext/image-generation`, `ext/queue`,
`ext/goal`, `ext/items`, `ext/guardian-*`, `agent-graph-store`,
`agent-identity`, `agent-roles`.

### 2.5 Providers, state, observability
`model-provider`, `model-provider-info`, `models-manager`, `ollama`,
`lmstudio`, `login`, `keyring-store`, `state`, `thread-store`,
`rollout`, `rollout-trace`, `history`, `otel`, `analytics`, `backend-client`.

### 2.6 Platform utilities
`utils/*` (paths, process, pty, image, audio, templates, plugins),
`bwrap`, `uds`, `stdio-to-uds`, `realtime-webrtc`, `voice-host`,
`codex-home`, `config`, `config-schema`, `features`.

**Key pattern:** `core` is the only place that knows the agent loop.
Frontends talk to it through `protocol`/`app-server` (JSON-RPC), not by
importing `core` internals. This is the pattern Stark should copy for its
React UI.

---

## 3. Exact stack (from `codex-rs/Cargo.toml`)

These are the versions Codex pins. Stark uses the same choices unless a note
says otherwise.

### Runtime & async
| Crate | Version | Use |
|---|---|---|
| `tokio` | `1` | Async runtime |
| `futures` | `0.3` | Streams/joins |
| `async-stream` | `0.3.6` | `stream!` macros |
| `async-channel` | `2.3.1` | Channels |
| `crossbeam-channel` | `0.5.15` | Blocking channels |
| `tokio-stream` | `0.1.18` | Stream adapters |
| `tokio-util` | `0.7.18` | `CancellationToken`, codecs |

### HTTP, SSE, WebSocket
| Crate | Version | Use |
|---|---|---|
| `reqwest` | `0.12` (`cookies`) | Model/provider HTTP |
| `eventsource-stream` | `0.2.3` | SSE parsing |
| `axum` | `0.8` | App server |
| `http` | `1.3.1` | HTTP types |
| `hyper-util` | `0.1.20` | Server plumbing |
| `tokio-tungstenite` | `0.28` | WebSocket client |
| `tungstenite` | `0.27` | WebSocket core |
| `url` / `urlencoding` | `2` / `2.1` | URLs |

### Serialization & schemas
| Crate | Version | Use |
|---|---|---|
| `serde` | `1` (`rc`) | Types |
| `serde_json` | `1` | JSON |
| `serde_with` | `3.17` | Helpers |
| `schemars` | `0.8.22` | Tool JSON Schema |
| `ts-rs` | `11` | TypeScript type export |
| `toml` / `toml_edit` | `0.9.5` / `0.24` | Config |
| `serde_yaml` | `0.9` | YAML |

### Persistence
| Crate | Version | Use |
|---|---|---|
| `sqlx` | `=0.9.0` (`runtime-tokio`, `sqlite-bundled`, `tls-rustls`, `chrono`, `json`, `migrate`, `uuid`, `time`) | SQLite |
| `sqlx-macros` | `=0.9.0` (`derive`, `migrate`) | migrations |
| `sqlx-sqlite` | `=0.9.0` (`offline`) | offline metadata |
| `libsqlite3-sys` | `0.37` | bundled SQLite |

### CLI, config, logging
| Crate | Version | Use |
|---|---|---|
| `clap` / `clap_complete` | `4` | CLI |
| `anyhow` | `1` | App errors |
| `thiserror` | `2.0.17` | Library errors |
| `color-eyre` | `0.6.3` | Error reports |
| `tracing` | `0.1.44` | Structured logs |
| `tracing-subscriber` | `0.3.22` | Subscribers |
| `tracing-appender` | `0.2.3` | File logs |
| `tracing-opentelemetry` | `0.32` | OTel bridge |
| `opentelemetry` / `-otlp` / `-sdk` | `0.31` | OTel |
| `sentry` | `0.46` | Optional crash reporting |

### Files, search, git
| Crate | Version | Use |
|---|---|---|
| `ignore` | `0.4.23` | gitignore-aware walking |
| `globset` / `glob` | `0.4` / `0.3` | Globs |
| `walkdir` | `2.5` | Directory walk |
| `regex` / `regex-automata` / `regex-syntax` | `1.12.3` / `0.4.13` / `0.8.8` | Search |
| `memchr` | `2.7.6` | Fast scanning |
| `tree-sitter` | `0.25.10` | Parsing |
| `tree-sitter-bash` / `-powershell` | `0.25` / `=0.26.4` | Shell grammars |
| `gix` / `gix-url` | `0.89` / `0.40` | Git |
| `similar` / `diffy` | `2.7` / `0.4.2` | Diffs |
| `notify` | `8.2` | File watcher |
| `file-id` | `0.2.3` | Stable file IDs |
| `bm25` | `2.3.2` | Ranking |

### Process, terminal, OS
| Crate | Version | Use |
|---|---|---|
| `portable-pty` | `0.9` | PTY |
| `vt100` | `0.16.2` | Terminal emulation |
| `ratatui` / `crossterm` | `0.30.2` / `0.29` | TUI |
| `landlock` | `0.4.4` | Linux FS sandbox |
| `seccompiler` | `0.5` | Linux syscall filter |
| `rustix` | `1.1.4` | OS APIs |
| `windows-sys` | `0.61.2` | Windows APIs |
| `winreg` | `0.55` | Registry |
| `uds_windows` | `1.1` | Windows UDS |
| `dirs` | `6` | Known folders |
| `keyring` | `3.6` | Credentials |
| `which` | `8` | Executable lookup |

### MCP
| Crate | Version | Use |
|---|---|---|
| `rmcp` | `=3.5.1` pinned to a git rev | Official Rust MCP SDK |
| `schemars` | `0.8.22` | Tool schemas (`#[tool]` macros) |

For Stark, use `rmcp` from crates.io at the current stable (`3.6.x` at the
time of writing) unless a pinned rev is required.

### Media (only if Rust voice later)
`image 0.25.9`, `symphonia 0.6.0`, `utils/audio`, `realtime-webrtc`.

### Dev/test
`insta 1.46.3`, `wiremock 0.6`, `pretty_assertions 1.4.1`,
`assert_cmd 2`, `predicates 3`, `test-log 0.2.19`, `serial_test 3.2`,
`tempfile 3.23`, `tokio-test 0.4`, `divan 0.1.21`.

---

## 4. Patterns to copy

1. **Core/protocol split.** `core` owns the loop; `protocol`/`app-server`
   expose it over JSON-RPC to any frontend. Stark should expose a typed
   `stark-protocol` and let the React UI/CLI consume it.
2. **One typed tool contract.** Tool metadata (name, description, JSON Schema,
   output truncation) is data, not ad-hoc prompt strings.
3. **Model client as a thin SSE consumer.** `reqwest` + `eventsource-stream`;
   no vendor SDK abstraction in the loop.
4. **Typed streaming events.** Deltas are an enum, not strings.
5. **Policy outside execution.** `execpolicy` decides; tools execute.
6. **Sandbox is a platform layer.** Linux (Landlock/seccomp/bwrap) and Windows
   have separate implementations; the policy layer is shared.
7. **Patch format as a first-class parser.** `apply-patch` accepts
   `*** Add File` / `*** Update File` / `*** Delete File` / `*** Move to`.
8. **File search is a subsystem.** `ignore` + globs + tree-sitter + BM25.
9. **Extensions are crates with a narrow interface.** `ext/*` do not reach
   into `core`.
10. **Observability from day one.** `tracing` spans around turns, tools, and
    provider calls; OTel export optional.
11. **Config is schema-validated.** `config-schema` + `schemars`.
12. **One binary, many platforms.** Cargo profiles tuned for size/speed;
    Bazel optional.

---

## 5. What NOT to copy

| Area | Why |
|---|---|
| **Bazel** | Codex needs it at its scale; Stark is one small workspace. Cargo only. |
| **~170 crates at day one** | Start with 8–10; split only when ownership/compile time demands it. |
| **OpenAI-specific crates** (`chatgpt`, `cloud-tasks`, `backend-client`, `analytics`, `login` internals) | Stark has its own auth/pairing and providers. |
| **`realtime-webrtc` / `voice-host`** | Stark keeps Python Moonshine/Pocket TTS as a sidecar initially. |
| **`code-mode` / `v8-poc`** | Not needed for Stark's tool model. |
| **Codex's TUI** | Stark has React/Electron; `ratatui` is optional for a CLI. |
| **Codex's extension set** | Stark's extensions are its own domains (house, DFD, groups). |
| **Codex's model/auth providers** | Stark uses DeepSeek/OpenCode/Command/free pool via OpenAI-compatible HTTP. |

**Do not** add `codex-*` crates as dependencies. **Do not** fork the
monorepo. **Do not** copy files without preserving Apache-2.0 `NOTICE` and
file-level attribution.

---

## 6. License and reuse

Codex is Apache-2.0. Reuse is permitted, including commercial use, with:

- a copy of the license,
- retention of `NOTICE`/attribution,
- a statement of changes if you modify files.

For Stark: prefer **independent implementation from observed behavior** over
copying. If a specific algorithm is copied (e.g. the patch parser or command
policy tokenizer), put the attribution in `NOTICE` and a header comment in the
file. Never copy a whole crate "because it is easier"; the maintenance cost is
the fork.

---

## 7. Reference checkout (read-only)

```bash
git clone --depth 1 https://github.com/openai/codex.git D:\ref\codex
```

Read in this order:

1. `codex-rs/Cargo.toml` — dependency choices and profiles.
2. `codex-rs/core/` — loop, model client, tool dispatch, safety.
3. `codex-rs/protocol/` + `codex-rs/app-server/` — frontend boundary.
4. `codex-rs/docs/protocol_v1.md` — wire protocol.
5. `codex-rs/execpolicy/` — command policy.
6. `codex-rs/apply-patch/` — patch parser.
7. `codex-rs/file-search/` + `file-watcher/` — code intelligence.
8. `codex-rs/rmcp-client/` — MCP client.
9. `codex-rs/linux-sandbox/` + `windows-sandbox-rs/` — sandboxing.
10. `codex-rs/core/tests/` — behavior fixtures.

Optional comparisons:
- [block/goose](https://github.com/aaif-goose/goose) — Rust agent, MCP, ACP.
- [Rig](https://github.com/0xPlaygrounds/rig) — published Rust agent library
  (reference only; do not depend on it).
- [Swiftide](https://github.com/bosun-ai/swiftide) — typed tasks/RAG
  (reference only).

---

## 8. Mapping Codex → Stark

| Codex concept | Stark Rust crate | Notes |
|---|---|---|
| `core` loop | `stark-core` | Add approvals/steering/compaction |
| `protocol` + `app-server` | `stark-contract` + `stark-server` | SSE/WS instead of JSON-RPC for the UI |
| `exec` | `stark-cli` | Optional headless mode |
| `tui` | `stark-cli` (optional) | React is the primary UI |
| `tools` + `apply-patch` | `stark-tools` | 42 tools; port descriptions verbatim |
| `execpolicy` | `stark-policy` | Port `_cmdpolicy` semantics |
| `sandboxing`, `linux-sandbox`, `windows-sandbox-rs` | `stark-policy::sandbox` | Windows first; Landlock later |
| `file-search`, `file-watcher` | `stark-tools::search` | tree-sitter + ignore |
| `rmcp-client` | `stark-tools::mcp` | `rmcp` |
| `state` / `thread-store` | `stark-store` | Reuse existing SQLite/JSONL |
| `agent-graph-store` | `stark-agents::graph` | Durable graph |
| `ext/memories` | `stark-memory` | Compaction, repo map |
| `otel` | `stark-server`/`stark-core` | `tracing` + OTel optional |
| `model-provider` | `stark-providers` | Hand-rolled OpenAI-compatible client |
| `agent-roles` | `stark-agents::registry` | Workspace + specialist specs |
| `ext/agent-message-board` | `stark-groups` | Events + inbox |
| `login`/`keyring` | `stark-policy::secrets` | Windows Credential Manager later |

---

## 9. "Hand-rolled" means

We write, in our own crates:

- the model request/response types and SSE parser,
- the agent loop and tool-call protocol,
- retry/fallback/rate-limit policy,
- tool registry/dispatch/approval,
- conversation history and compaction,
- durable graph and checkpoints,
- group-project event store and supervisor,
- policy/envelope enforcement.

We do **not** depend on:

- `rig`, `rig-core`, `rig-harness`
- `swiftide`
- `langchain-rust`
- `kalosm`
- `async-openai`
- `openai` SDK wrappers
- `langgraph` ports or workflow frameworks

We **do** use infrastructure libraries (`tokio`, `reqwest`,
`eventsource-stream`, `serde`, `schemars`, `rmcp`, `axum`, `sqlx`,
`tree-sitter`, `tracing`, `notify`, `ignore`, `globset`, `similar`, `gix`).
That is the same distinction Codex makes: libraries, not agent frameworks.
