# 17 — Instructions, memory, usage, and approvals

## AGENTS.md instructions

Codex-style project instructions are loaded from:

1. `<harness>/.deep_agents/AGENTS.override.md`, then `AGENTS.md`;
2. every directory from the project root (nearest `.git`) down to the
   workspace, with `AGENTS.override.md` preferred over `AGENTS.md`.

The concatenated text is capped at 32KB and appended to the system prompt as
`[PROJECT INSTRUCTIONS]`. It is not long-term memory and not a repo map.

`project.md` and generated repo maps are intentionally not used.

## Workspace memory

Memory is split into two layers, following the useful part of the Codex and
Claude Code designs:

1. **Deterministic capture.** Every completed workspace/orchestrator turn writes
   a stage-1 record to SQLite. This is cheap and always runs:
   request, outcome, chat, agent, timestamp. Secrets are redacted before disk.
2. **Model-assisted distillation/consolidation.** Background work turns pending
   stage-1 records into durable entries, a compact index, and topic files. If the
   model fails or returns invalid JSON, deterministic consolidation is used.

The injected context is intentionally small:

- `MEMORY.md` — compact index, injected at run start (capped by
  `STARK_MEMORY_MAX_CHARS`, default 8KB).
- `topics/<topic>.md` — details, read on demand with the `memory` tool.
- `summary.md` — long-form consolidated memory.

Layout per workspace:

```text
.deep_agents/memory/<workspace_id>/
  MEMORY.md              # compact index, injected
  summary.md             # long-form summary, on demand
  topics/
    decisions.md
    facts.md
    commands.md
    pitfalls.md
    preferences.md
    tasks.md
    recent.md
    notes.md
```

Memory is scoped by workspace. Worktrees and subdirectories of the same
workspace share the same memory because they share the workspace id. Different
projects, House, DFD, and General do not see each other's memory. The
orchestrator only sees its own workspace memory; delegated agents receive their
target workspace's memory.

`AGENTS.md` remains the manual instruction layer and is always separate from
learned memory. A repo map is not injected by default; the agent should search
or read the working tree, or use an explicit repo-map tool if one is added later.

### The `memory` tool

The compact index is already in context. The tool provides the long-form path:

- `index` / `summary` — read the index or long-form summary
- `list` — list topic files
- `read topic=...` — read a topic and record usage
- `search query=...` — search durable entries
- `remember text=...` — add a durable note immediately
- `forget entry_id=...` / `pin entry_id=...` — remove or pin an entry
- `status` — counts and last consolidation metadata

Read-only agents and plan mode can read memory but cannot write it.

### Background consolidation

The model pass runs when pending stage-1 records reach
`STARK_MEMORY_CONSOLIDATE_AFTER` (default 8), or when you click **Consolidate**
in the memory UI. It is per-workspace locked, so two runs cannot rewrite the
same memory at once.

Configuration:

```text
STARK_MEMORY=on|off                       default on
STARK_MEMORY_MAX_CHARS=8000               index/summary cap
STARK_MEMORY_CONSOLIDATE_AFTER=8          pending records before auto-consolidation
STARK_MEMORY_MODEL=                       optional model override
```

If `STARK_MEMORY_MODEL` is empty, the current run model is used. The override
must be one of the curated Stark models.

### Routes

- `GET /api/memory` — workspace list + status
- `GET /api/memory/{workspace_id}?records=30` — status, index, summary, topics,
  entries, recent stage-1 records
- `PUT /api/memory/{workspace_id}` — save a manually edited summary
- `POST /api/memory/{workspace_id}/consolidate` — `{"mode":"model"}` or
  `{"mode":"deterministic"}`
- `POST /api/memory/{workspace_id}/notes` — add a durable note
- `GET /api/memory/{workspace_id}/topics/{topic}` — read a topic
- `DELETE /api/memory/{workspace_id}/entries/{entry_id}` — forget an entry
- `POST /api/memory/{workspace_id}/entries/{entry_id}/pin` — pin/unpin
- `DELETE /api/memory/{workspace_id}` — reset memory

### Memory UI

`#/memory` shows the workspace list, injected index, long-form summary, durable
entries, topic files, and recent captured turns. It supports:

- model consolidation and deterministic rebuild;
- editing the long-form summary;
- adding a durable note;
- pinning/forgetting entries;
- viewing topic files;
- clearing a workspace's memory.

The sidebar agent panel links to the memory page for the active workspace.

### What we intentionally do not do

- No Python memory daemon.
- No embeddings or vector search; the index plus Markdown topics is enough for
  this scale.
- No per-turn model call. Capture is deterministic; the model only runs during
  distillation/consolidation.
- No default repo-map injection. A stale map in every prompt is worse than
  letting the agent search the current code.

## Token usage and pricing

Provider usage now records:

- input tokens
- cached input tokens
- cache-miss input tokens
- output tokens
- reasoning tokens
- provider-reported cost

`AppState.token_usage` accumulates cached/miss tokens and cost, and each call
includes Codex-style fields (`input`, `output`, `cache_hits`, `cache_misses`,
`cost`, `reasoning_tokens`, `timestamp`). Agent records persist the same usage.

When the provider does not report cost, pricing can be supplied in USD per
million tokens:

```text
STARK_PRICING_INPUT
STARK_PRICING_CACHED_INPUT
STARK_PRICING_OUTPUT
```

or per model:

```text
STARK_PRICING_<MODEL>_INPUT
STARK_PRICING_<MODEL>_CACHED_INPUT
STARK_PRICING_<MODEL>_OUTPUT
```

No rate limiter is implemented.

## DFD approval

`create_client` now asks in-app when `approved_by` is absent:

- question: “Approve creating DFD client …?”
- options: Approve / Deny
- approval marker: `user:<chat_id>`

Existing `telegram:` and `approval:` markers remain accepted for audit
compatibility. Telegram itself is not a dependency.

## Agent API

- `GET /api/agents` — agent list with access, worktree, pending counts
- `GET /api/agents/{agent_id}` — agent + pending mailbox + history + children
- `POST /api/agents/{agent_id}/message` — steer a running agent or queue a trigger
- `GET /api/agents/{agent_id}/diff`, `POST .../merge`, `POST .../discard`

The durable agent scheduler and mailbox are documented in
[16-durable-agents.md](./16-durable-agents.md).

## Status ordering

The server persists the terminal agent status (`completed`/`failed`/`cancelled`)
before sending the stream `done` event, and the client refreshes the sidebar
tree as soon as the run ends. Follow-up runs keep their depth and agent id.
