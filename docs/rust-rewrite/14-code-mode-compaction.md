# 14 — Code mode, compaction, and run budgets (Rust)

## Code mode

`run_code` is the legacy QuickJS code-mode tool. It is **not** part of the
Developer v2 default surface; DeepSeek Harness keeps PTC/code mode in a separate
`ptc` preset. It remains implemented for tests and for agents that opt into it
explicitly.

`run_code` runs a short JavaScript program against the same tool registry as
the normal agent loop.

Tool calls are synchronous. Their text result is returned directly; use
`JSON.parse` for structured output. A failing tool throws a JavaScript error
that the program can catch. `console.log` output is returned separately.

Why it exists:

- one model turn can perform many reads/searches/edits;
- intermediate tool output stays inside the program and never enters the
  conversation;
- tool schemas do not all need to be sent on every request.

### Execution

- Engine: QuickJS via `rquickjs`, built into `stark-tools`.
- No host filesystem, network, process, import, or timer APIs. The only host
  surface is `tools`, `search`, and `console`.
- `tools.<name>({...})` goes through the normal `dispatch` path, so approvals,
  plan-mode checks, MCP, and the process manager still apply.
- Inner tool calls emit `tool_started` / `tool_finished` deltas, so the trace
  shows the nested work.
- `run_code` is developer-only and blocked in plan mode.

### Limits

| Limit | Default | Maximum |
|---|---|---|
| Wall clock | 30s | 120s |
| Inner tool calls | 50 | 100 |
| Code size | 64KB | 64KB |
| Result text | 32KB | 32KB |
| JS memory | 64MB | 64MB |
| Stack | 512KB | 512KB |

`ask_user`, `delegate`, `call_subagent`, job control, schedules, canvas mode,
and recursive `run_code` are not available inside a program.

The interpreter is a convenience boundary, not a hostile-code security
boundary. It has no ambient capabilities, but a real OS sandbox is a separate
project.

## Compaction

Compaction follows opencode's checkpoint model.

1. The request ceiling is `window - max(15% of window, 16k)`.
2. When the estimated prompt reaches the ceiling, older history is summarized
   into a structured Markdown checkpoint.
3. The newest `keep` tokens are kept as `<recent-context>`.
4. The next request starts with one synthetic user message:

```text
<conversation-checkpoint>
<summary>…</summary>
<recent-context>…</recent-context>
</conversation-checkpoint>
```

5. A later compaction updates the existing checkpoint instead of summarizing a
   summary.
6. The compaction entry is persisted in the chat session JSONL with
   `checkpoint_format: "opencode-v1"`. Legacy `[CONTEXT SUMMARY]` entries still
   load.

Defaults and overrides:

| Setting | Default |
|---|---|
| Context window | `STARK_MODEL_CONTEXT` or `STARK_MODEL_CONTEXT_<MODEL>`; otherwise 500000 |
| Output window | `STARK_MODEL_OUTPUT` or `STARK_MODEL_OUTPUT_<MODEL>`; otherwise 32000 |
| Trigger | `COMPACTION_CHECKPOINT_TOKENS` or `<ROLE>_COMPACTION_CHECKPOINT_TOKENS` |
| Keep tail | `COMPACTION_KEEP_TOKENS`; otherwise 30000 for windows ≥400k, else 15000 |
| Summary output | `COMPACTION_SUMMARY_OUTPUT_TOKENS`; otherwise 8000 |
| Summary model | same provider/model as the run |

The summary template sections are: Objective, Requirements, Decisions, Work
State (Completed / Active / Blocked), Next Move, Relevant Files, and Important
Context. A later compaction receives the “update the existing checkpoint”
prompt.

There is no microcompaction step: old tool results are not cleared separately.

## Run budgets

The fixed `MAX_STEPS = 12` limit is gone. A run now ends on:

- a final answer;
- user cancellation;
- wall-clock budget (`STARK_RUN_MAX_SECONDS`, default 3600);
- token budget (`STARK_RUN_MAX_TOKENS`, default 4M);
- tool-call budget (`STARK_RUN_MAX_TOOL_CALLS`, default 500);
- the same tool + arguments repeated 4× (read-only) or 5× (write/command);
- 5 consecutive tool errors.

These are safety rails, not a step count. Compaction handles long contexts.

## Performance changes in this pass

- Frontend streaming deltas are coalesced into one React update per animation
  frame.
- Provider HTTP client enables HTTP/2.
- Code mode reduces model round-trips and keeps intermediate tool output out of
  context.

Prompt-cache zoning/configuration and transport compression are explicitly
deferred.

## Dropped tools

`ast_search` and `github_apply_gate` were never implemented in Rust and are
no longer exposed in the frozen tool contract (`tool-specs.json`).
