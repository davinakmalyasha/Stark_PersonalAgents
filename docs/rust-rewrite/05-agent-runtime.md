# 05 — Agent Runtime

Hand-rolled loop, same approach as Codex: `reqwest` + SSE, typed events,
explicit retries, explicit cancellation.

---

## 1. Provider layer (`stark-providers`)

No SDK. The wire client is built directly:

```rust
pub enum ProviderEvent {
    Text(String),
    Reasoning(String),
    ToolCall { index, id, name, arguments },
    Usage(Usage),
    Done { finish_reason },
}
```

- `POST {base_url}/chat/completions` with `stream: true`, parsed with
  `eventsource-stream`; tool-call deltas accumulated by index.
- Provider-specific fields handled: `reasoning_content`, `reasoning`,
  cached/reasoning tokens, cost.
- `Usage` carries input, cached, output, reasoning tokens, and cost;
  env-based pricing fallback when the provider omits cost.
- `stream_chat_for_session` adds opencode-go affinity headers
  (`x-opencode-session` + variants) when the mode is `opencode`.
- `complete_text_for_session` collects one non-tool response — used by
  memory consolidation, never by the agent loop.
- Only four curated models are offered:
  `deepseek-v4.1-flash`, `mimo-v2.6-flash`, `mimo-v2.6-pro`,
  `muse-spark-1.3-contributor`.

---

## 2. Agent loop (`stark-core`)

```
loop:
  cancel? → abort
  drain steering + follow-ups into history
  provider.stream_chat(messages, tool_specs)
    emit Delta(text/reasoning/usage/tool_started/tool_finished)
  no tool calls → final answer
  dispatch tools in order (policy → approval → execute)
  append tool results
  compact if over budget (opencode checkpoint model)
  check wall-clock / token / tool-call budgets, repeats, consecutive errors
```

- A denied call becomes a `[BLOCKED] ...` tool result, never a loop error.
- A tool failure is `[ERR]` content the model can react to.
- No fixed `MAX_STEPS`; budgets are wall-clock/token/tool-call counts.
- Deltas: `text`, `reasoning`, `tool_started`, `tool_finished`, `state`,
  `end`, `done` — the UI parses these; do not rename kinds.

### Cancellation and steering

- `CancellationToken` checked before each provider call, between chunks,
  and before each tool call.
- `POST /api/abort` cancels; the final state is still persisted.
- Steering drains between turns; follow-ups queue the next turn on the
  same chat.

---

## 3. Prompts and history

System prompt, fixed order:

1. Role instructions (`stark-core::agents` specs).
2. Project instructions (`AGENTS.md` hierarchy, 32KB cap).
3. Workspace memory index (`MEMORY.md`, capped).
4. Read-only notice when `FileAccess::Read`.
5. Plan block when a plan exists.

Message history derives from the per-chat session JSONL plus the current
turn. Compaction uses the opencode checkpoint model (see doc 14).

---

## 4. Agent registry (`stark-core::agents`)

```rust
pub struct AgentSpec { key, display, plan_scope, tier, instructions }
pub enum FileAccess { Read, Write, Worktree }
```

- Orchestrator: routes, delegates, coordinates.
- Workspace agents: Developer, General, House, DFD, UI (+ DevOps/Explore
  specialists for subagents).
- Workspace agents are the only writers; subagents default to read-only
  unless the parent passes `write_access` (nested worktree).
- `default_access(scope, depth, spawned)` decides; `STARK_WORKTREES`
  controls the non-git fallback.

### Subagents

- Self-contained task: goal, constraints, paths, acceptance. No parent
  history — the parent passes what matters.
- `call_subagent` run blocks and persists the record; `spawn` is
  supervisor-driven.

---

## 5. Acceptance

- Token/cost totals match provider usage within tolerance.
- Abort stops promptly; final state persists.
- Steering applies on the next turn, never mid-request.
- Compaction keeps tool calls paired and stays under the window.
