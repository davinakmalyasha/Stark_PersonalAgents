# 16 — Durable agents (orchestrator → workspace → subagent)

## Model

Three levels, one execution loop:

```text
Orchestrator (level 0)
  ├── workspace agent: Developer / General / House / DFD / UI (level 1)
  │     ├── subagent (level 2)
  │     └── subagent (level 2)
  └── workspace agent ...
```

- **Orchestrator** coordinates and routes. It does not own a workspace or write files.
- **Workspace agent** is durable, one per `(workspace_id, scope)`, and is the single
  writer for its workspace. It has its own chat/session and can spawn subagents.
- **Subagent** is a child of a workspace agent. It is hidden from the normal
  chat list, cannot spawn further agents, and returns a compact report.

The tool is `agent`:

```text
spawn   kind=workspace|subagent, agent=Developer|General|..., task, title, workspace_id?, workspace_path?, wait_seconds?
list    direct children of the current agent
status  agent_id, wait_seconds?
send    agent_id, message
report  message, quiet?
```

`spawn` only writes durable records and a mailbox message. The server supervisor
starts the actual run. This keeps tool calls fast and makes restart recovery
possible.

## Persistence

`agents` and `agent_messages` live in `.deep_agents/chats.db`:

```text
agents(id, kind, path, parent_id, chat_id, workspace_id, scope, role,
       nickname, status, model, depth, created_at, updated_at,
       last_active, result, usage_json)

agent_messages(id, sender_id, recipient_id, kind, body, metadata_json,
               delivery, created_at, consumed_at)
```

`status`: `idle | queued | running | interrupted | completed | failed | cancelled`

`delivery`:
- `trigger` — wake an idle agent;
- `queue` — deliver only at the next turn.

Session history remains the existing per-chat JSONL. A subagent gets a child
chat, so its full transcript is queryable without putting it in the parent
context.

## Supervisor

The server `agent_loop` runs every second:

1. `recover_running` marks stale `running` agents `interrupted` at startup.
2. Find `idle`/`queued` agents with unconsumed `trigger` messages.
3. Claim one (one active turn per agent).
4. Build a `RunContext` from the agent record and run the prompt.
5. Persist status, result, and usage.
6. Enqueue a `result` message for the parent and wake it.
7. If more messages arrived while running, set the agent back to `queued`.

A workspace agent reuses its existing chat. Spawning the same workspace/scope
again queues a new task instead of creating a duplicate workspace agent.

## Failure semantics

- Messages and spawn edges are persisted **before** execution.
- A crash leaves queued messages intact and marks running agents `interrupted`.
- A half-finished turn is not replayed automatically; explicit `send` resumes it.
- Mailbox delivery is at-least-once; agents should treat duplicate reports as
  idempotent.

## Limits

- Max depth 2: subagents cannot spawn.
- Only the orchestrator can spawn workspace agents.
- Only workspace agents can spawn subagents.
- Workspace agents are the only writers; subagents should default to read-only
  unless isolated in a worktree.

## Agent tree UI

The sidebar renders `/api/agents` as a collapsible tree:

- status dot per agent;
- role/kind/path;
- click opens the agent's chat;
- cancel button for queued/running agents;
- 5-second refresh.

The tree is read-only apart from cancellation. Full mailbox/send controls remain
a later UI pass.

## Legacy tools migrated

`delegate` now creates/reuses durable workspace agents, and `call_subagent`
uses the durable subagent store for list/status/message/stop. The old
`JOBS`/`CHILDREN` in-memory registries are gone. `call_subagent` with
`action=run` still executes synchronously, but persists the agent record and
result.

## Not implemented yet

- Worktree isolation for write-capable subagents.
- Full mailbox/send UI controls in the agent tree.
- Model-assisted long-term memory consolidation (stage-1/summary store exists).
- Telegram bot (DFD approval is in-app now).
