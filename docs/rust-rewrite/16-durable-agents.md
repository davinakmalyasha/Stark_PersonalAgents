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

Developer uses the Codex multi-agent v2 tools (`spawn_agent`, `send_message`,
`followup_task`, `wait_agent`, `list_agents`, `interrupt_agent`,
`close_agent`). Its children share the parent workspace, inherit write tools,
use task paths (`/root/<task_name>`), default `fork_turns=all`, and are limited
to depth 1 and 6 open children. The `agent` tool below is the legacy v1 surface
used by Orchestrator, General, House, DFD, and UI until they are migrated.

- **Orchestrator** coordinates and routes. It does not own a workspace or write files.
- **Workspace agent** is durable, one per `(workspace_id, scope)`, and is the single
  writer for its workspace. It has its own chat/session and can spawn subagents.
- **Subagent** is a child of a workspace agent. It is hidden from the normal
  chat list, cannot spawn further agents, and returns a compact report.

Legacy tool `agent`:

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

`status`: `idle | queued | running | interrupted | completed | failed | cancelled | closed`

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

- Legacy v1: max depth 2; subagents cannot spawn.
- Developer v2: max depth 1; max 6 open children; children share the parent
  workspace and inherit write tools.
- Only the orchestrator can spawn workspace agents.
- Only workspace agents can spawn subagents.
- Legacy workspace agents are the only writers; v1 subagents default to
  read-only unless isolated in a worktree.

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

## Worktree isolation (legacy v1 agents)

The legacy `agent` tool can isolate spawned write-capable agents in a git
worktree on their own branch. Developer v2 does **not** use this: Codex-style
children share the parent workspace and rely on disjoint write sets.

- Developer/UI/DevOps spawns default to `worktree`; level-2 subagents default
  to `read` unless `write_access=true` creates a nested worktree.
- General/DFD spawns write directly; House/Explore/Orchestrator are read-only.
- The child chat targets the worktree path, so tools, checkpoints, and
  `AGENTS.md` work unchanged. The parent tree is untouched until merge.
- After each run the worktree auto-commits (excluding `.deep_agents`) and
  records the diff stat on the agent.
- The parent merges explicitly (`agent` action `merge`, `POST
  /api/agents/{id}/merge`) after reviewing the diff; conflicts abort cleanly
  and keep the worktree. `discard` removes the worktree and branch.
- `STARK_WORKTREES=auto|required|off` controls the non-git fallback:
  `auto` warns and writes directly, `required` fails, `off` disables.
- `delegate` resolves `workspace`/`project` before spawning; Developer
  delegation requires a project target and never silently uses the harness root.
- `memory`, `agent` diff/merge/discard, and the tree UI share the same metadata:
  `access`, branch, base, diff stat, and state
  (`active|merged|discarded|missing|failed`).

## Agent tree controls

`GET /api/agents` reports access, worktree, pending-message counts, and
activity timestamps. `GET /api/agents/{id}` adds the pending mailbox, full
history, and children. `POST /api/agents/{id}/message` steers a running agent
or queues a trigger for an idle one. Merge/discard/diff have matching routes.
The sidebar tree shows status text, relative activity, model, an access badge,
and pending counts, with a detail drawer (overview/mailbox/worktree), send,
merge (confirmed, diff-first), discard, and cancel.

## Intentional omission

- Telegram bot — DFD approval is in-app now.
