# Stark Rust Rewrite — Design Docs

Status: **design set, implementation not started**
Scope: replace the Python backend with a Rust backend that preserves the
existing product contract — features, tool descriptions/schemas, HTTP/SSE API,
persistence, and the multi-agent concept — while following the architecture
and technology choices of **OpenAI Codex CLI (`codex-rs`)**.

This directory is the source of truth for the rewrite. If a future decision
conflicts with these docs, update the doc first, then the code.

---

## Locked decisions

| Decision | Choice |
|---|---|
| Reference | `openai/codex` (`codex-rs`) — architecture and stack |
| Agent loop | **Hand-rolled** on `tokio` + `reqwest` + `eventsource-stream` |
| Frameworks | **None.** No `rig`, `swiftide`, `langchain-rust`, `kalosm`, `async-openai`, LangGraph port |
| Language/toolchain | Rust **stable 1.99.x** (or newer), **edition 2024**, MSVC on Windows |
| Placement | `backend-rs/` in the same repo; Python stays runnable side-by-side |
| Branch | `rust-rewrite`, created after a safety snapshot of the current work |
| Platform | **Windows first**, Linux/macOS later |
| Persistence | Rust reads/writes the **existing** SQLite + JSONL formats |
| Voice/Telegram | Stay as **Python sidecars** until the Rust core reaches parity |
| Tool contract | Descriptions and JSON Schemas are **frozen**; export from Python, diff in CI |
| Definition of done | Existing frontend + Telegram + voice work against Rust with no UI changes |

---

## Reading order

| # | Document | What it answers |
|---|---|---|
| 00 | [Current System](./00-current-system.md) | What exists today: tiers, features, 42 registered tools, API, persistence, invariants |
| 01 | [Codex Reference](./01-codex-reference.md) | How Codex is built; what to copy; what not to copy; license |
| 02 | [Architecture](./02-architecture.md) | Target workspace, crates, dependency direction, concurrency, data flow |
| 03 | [Tech Stack](./03-tech-stack.md) | Exact crates/versions, build profiles, lints, toolchain |
| 04 | [Tool System](./04-tool-system.md) | Tool registry, schemas, dispatcher, policy, approvals, per-tool port notes |
| 05 | [Agent Runtime](./05-agent-runtime.md) | Provider client, agent loop, streaming, cancellation, compaction, sessions |
| 06 | [Orchestration](./06-orchestration.md) | Routing, durable graph, delegation, specialists, subagents |
| 07 | [Group Projects](./07-group-projects.md) | Event-sourced store, supervisor tick, leases, approvals, recovery |
| 08 | [Server Contract](./08-server-contract.md) | Axum routes, SSE/WS, AppState/Delta, auth, lifecycle |
| 09 | [Persistence](./09-persistence.md) | SQLite schemas, JSONL, snapshots, migrations, compatibility |
| 10 | [Parity Testing](./10-parity-testing.md) | Golden fixtures, Python-vs-Rust harness, CI gates |
| 11 | [Migration Plan](./11-migration-plan.md) | Phases, acceptance criteria, estimates, risks, cutover |
| 12 | [Setup & Branching](./12-setup-and-branching.md) | Rust install, repo snapshot, branch, Codex clone, bootstrap |

---

## Hard rules

1. **Port the contract, not the code.** The Python implementation is the
   reference behavior; the Rust implementation is a new codebase.
2. **Tool descriptions and parameter schemas are prompt API.** They are copied
   verbatim from `tools/_schemas.py`; no paraphrasing, no reordering of
   properties without a parity diff.
3. **No agent framework.** The loop, tool dispatch, retries, approvals, and
   context management are ours. This is what Codex, Goose, Forge, and Aichat
   do.
4. **Policy lives outside tools.** A tool never decides whether it is allowed;
   the dispatcher classifies the action against the envelope.
5. **Events are the only truth for group projects.** Tables are projections.
6. **One writer per group project.** A single supervisor task owns transitions.
7. **No silent behavior changes.** Any intentional deviation is recorded in
   `11-migration-plan.md` under "Accepted deviations".
8. **Never commit secrets.** `.env` stays ignored; no keys in docs or tests.
9. **Reference only.** Codex is cloned outside the repo; it is not vendored,
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
| **Envelope** | A group project's declared autonomy tiers |
| **Projection** | A SQLite table derived from the event log |
| **Delta** | A streaming chunk (`text`, `reasoning`, `usage`, `tool_started`, …) |
| **State** | A full `AppState` snapshot sent over SSE |
| **Op** | A requested group-project mutation applied by the supervisor |
| **Lease** | A time-boxed exclusive claim on a group-project item |
