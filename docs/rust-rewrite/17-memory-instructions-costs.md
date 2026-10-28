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

Completed workspace/orchestrator turns create a stage-1 record in
`.deep_agents/chats.db`:

```text
memory_stage1(id, workspace_id, chat_id, agent_id, request, summary,
              raw_memory, created_at, usage_count, last_usage,
              selected_for_phase2, consolidated_at)
memory_summary(workspace_id, summary, updated_at, consolidated_count)
```

Consolidation folds the newest 30 records into one bounded Markdown summary,
stores it in `memory_summary`, and writes
`.deep_agents/memory/<workspace>.md`. The next run injects at most
`STARK_MEMORY_MAX_CHARS` (default 8000) as `[WORKSPACE MEMORY]`.

Configuration:

```text
STARK_MEMORY=on|off          default on
STARK_MEMORY_MAX_CHARS=8000
```

Routes:

- `GET /api/memory/{workspace_id}` — status + summary
- `POST /api/memory/{workspace_id}/consolidate`
- `DELETE /api/memory/{workspace_id}` — reset

This follows Codex’s stage-1/consolidation shape without the Python memory
daemon or embeddings. Consolidation is deterministic in this version; a
model-assisted extractor can replace it later without changing the schema.

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

- `GET /api/agents` — flat agent list
- `GET /api/agents/{agent_id}` — agent + unconsumed mailbox

The durable agent scheduler and mailbox are documented in
[16-durable-agents.md](./16-durable-agents.md).

## Not yet implemented

- Model-assisted memory extraction/consolidation
- Memory UI
- Agent-tree UI
- Active cancellation propagation
- Worktree isolation for write subagents
- Legacy delegate/call_subagent migration to the durable store
