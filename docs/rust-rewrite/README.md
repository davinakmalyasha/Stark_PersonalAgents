# Stark — Rust Backend Design Docs

Status: **the Rust backend in `backend-rs` is the only backend.**
The Python tree was removed at `571781c` (`pre-python-removal` is the
rollback tag). These docs describe the current Rust system, not a migration.

Reference architecture: **OpenAI Codex CLI (`codex-rs`)** — checkpoint
compaction, provider session affinity, durable agent tree, staged memory
with a compact injected index plus on-demand detail.

---

## Locked decisions

| Decision | Choice |
|---|---|
| Reference | `openai/codex` (`codex-rs`) — architecture and stack |
| Agent loop | **Hand-rolled** on `tokio` + `reqwest` + `eventsource-stream` |
| Frameworks | **None.** No `rig`, `swiftide`, `langchain-rust`, `kalosm`, `async-openai`, LangGraph port |
| Language/toolchain | Rust **stable 1.99.x** (or newer), **edition 2024**, MSVC on Windows |
| Placement | `backend-rs/` in the same repo |
| Branch | `master`; `rust-rewrite` and other work branches are history |
| Platform | **Windows first**, Linux/macOS later |
| Persistence | SQLite (`.deep_agents/chats.db`) + JSONL sessions, readable across restarts |
| Voice | Native Rust (Moonshine STT + Pocket TTS), no Python sidecar |
| Tool contract | `backend-rs/specs/tool-specs.json`; v2 Developer surface + v1 legacy scopes |
| Dropped tools | `ast_search`, `github_apply_gate` were never implemented and are not exposed |
| Admin surface | Removed — no `#/admin`, no `/api/agentsadmin` |
| Definition of done | Existing frontend works against Rust with no UI changes |

---

## Reading order

| # | Document | What it answers |
|---|---|---|
| 01 | [Codex Reference](./01-codex-reference.md) | How Codex is built; what to copy; what not to copy; license |
| 02 | [Architecture](./02-architecture.md) | Workspace, crates, dependency direction, concurrency, data flow |
| 03 | [Tech Stack](./03-tech-stack.md) | Exact crates/versions, build profiles, lints, toolchain |
| 04 | [Tool System](./04-tool-system.md) | Tool registry, schemas, dispatcher, policy, approvals |
| 05 | [Agent Runtime](./05-agent-runtime.md) | Provider client, agent loop, streaming, cancellation, compaction, sessions |
| 06 | [Orchestration](./06-orchestration.md) | Routing, durable graph, delegation, specialists, subagents |
| 07 | [Group Projects](./07-group-projects.md) | Event-sourced store, supervisor tick, leases, approvals, recovery |
| 08 | [Server Contract](./08-server-contract.md) | Axum routes, SSE/WS, AppState/Delta, auth, lifecycle, static SPA |
| 09 | [Persistence](./09-persistence.md) | SQLite schemas, JSONL, snapshots, migrations, compatibility |
| 12 | [Setup](./12-setup.md) | Rust-only toolchain, backend, frontend, desktop builds |
| 12 | [Voice](./12-voice.md) | Native Rust voice: STT/TTS models, asset prep, packaging |
| 13 | [Canvas & MCP](./13-canvas-mcp.md) | Canvas watches, wake loop, MCP stdio/HTTP, approvals |
| 14 | [Code Mode & Compaction](./14-code-mode-compaction.md) | QuickJS tool programs, checkpoint compaction, run budgets |
| 15 | [Provider Affinity](./15-provider-affinity.md) | opencode-go session/affinity headers and cache routing |
| 16 | [Durable Agents](./16-durable-agents.md) | Orchestrator → workspace → subagent tree, mailboxes, supervisor |
| 17 | [Instructions, Memory & Costs](./17-memory-instructions-costs.md) | AGENTS.md, workspace memory, usage/pricing, DFD approval |
| 18 | [DeepSeek / Codex Parity](./18-deepseek-parity.md) | Developer v2 tool surface, sources, deviations, behavior |

Deleted as Python-era history: `00`, `10`, `11`, `12-setup-and-branching`,
`Structure.md`, `group-projects/`, `memory/fixed-concept.md`,
`tools/ast_search.md`, `tools/github_apply_gate.md`, `tools/update_feature.md`,
`tools/update_project_meta.md`, `tools/create_project.md`,
`tools/propose_plan.md`, `tools/_SCOPE_MAP.md`.

---

## Hard rules

1. **The frozen contract is the prompt API.** Tool descriptions and parameter
   schemas live in `backend-rs/specs/tool-specs.json`, enforced by
   `stark-contract` tests. No edits without updating the tests.
2. **No agent framework.** The loop, tool dispatch, retries, approvals, and
   context management are ours.
3. **Policy lives outside tools.** A tool never decides whether it is allowed;
   the dispatcher classifies the action against the envelope and file access.
4. **Events are the only truth for group projects.** Tables are projections.
5. **One writer per group project.** A single supervisor task owns transitions.
6. **Read-only means read-only at dispatch**, not just hidden specs.
7. **Never commit secrets.** `.env` stays ignored; no keys in docs or tests.
8. **Reference only.** Codex is cloned outside the repo; it is not vendored,
   forked, or added as a Cargo dependency.

---

## Glossary

| Term | Meaning |
|---|---|
| **Harness** | The agent runtime: loop, tools, policy, persistence, server |
| **Run** | One user request executing one agent turn/graph |
| **Chat** | A persistent conversation; owns a session JSONL and UI stream |
| **Workspace** | A folder/domain an agent is scoped to (`_general`, `_house`, project path) |
| **Scope** | The tool allowlist for an agent role (`developer`, `general`, …) |
| **File access** | `read` / `write` / `worktree` — enforced at dispatch |
| **Envelope** | A group project's declared autonomy tiers |
| **Projection** | A SQLite table derived from the event log |
| **Delta** | A streaming chunk (`text`, `reasoning`, `usage`, `tool_started`, …) |
| **State** | A full `AppState` snapshot sent over SSE |
| **Op** | A requested group-project mutation applied by the supervisor |
| **Lease** | A time-boxed exclusive claim on a group-project item |
