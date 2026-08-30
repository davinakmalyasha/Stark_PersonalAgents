# delegate

- **Description:** Spawn one or more workspace agents (Dev / General / House) IN PARALLEL. Every item gets its own visible chat; final replies arrive as [DELEGATION RESULT] events while you keep coordinating.

BATCHING — per user turn: put EVERY independent job from the current request into a SINGLE delegate(items=[...]) call. Any agent mix is allowed and runs in parallel — solo scope (just Dev, just House, or just General) or mixed (Dev + House, Dev + General, all three). The sole batching criterion is independence. If job B needs job A's output, do NOT bundle B with A: delegate A first, wait for its [DELEGATION RESULT], then delegate B with A's result in the task. Never split a single request across multiple delegate calls for tidiness or sequencing — one call, all independent items.

CHAT CONTINUATION — chat_id is optional. Omit it for a new job (default). Set it ONLY when this item is a follow-up, correction, or bug on a chat you can identify from a prior [DELEGATION RESULT] in this session. If the chat_id you pass does not exist, the runtime silently starts a new chat — so never guess an id.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** orchestrator
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** delegate(items=[{agent, task, title?, chat_id?}]) — Spawn Dev/General/House agents IN PARALLEL; each gets its own chat, results arrive as [DELEGATION RESULT] events.
- **Required args (validated):** items
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** standard registry: pre_hooks(cache) -> guards(plan-mode) -> execute -> post_hooks(redact/offload)

## Parameters (JSON Schema)

  - `items` (array, required): Delegation items to launch concurrently.

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. Optional `input_schema` in `tools/_tool_schemas.py` (opt-in Stage1 validation).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Registry auto-derives scope visibility from `_SCOPE_MAP` (runtime `restrict_scope`).
