# delegate

- **Description:** Spawn one or more workspace agents (Developer / General / House) IN PARALLEL. Every item gets its own visible chat; final replies arrive as [DELEGATION RESULT] events while you keep coordinating.

BRIEF — you define WHAT, never HOW. Each item carries outcome (what Sir gets when it is done, in Sir's terms) and acceptance (how success will be judged — observable evidence). Never prescribe libraries, frameworks, architecture, step lists, file-by-file plans, or design direction; the specialist is the pro and owns the method (it may deviate and must report deviations). Name a command in acceptance only if Sir named it or it is the project's standard entry point; otherwise describe the behavior and let the specialist choose the proof. One or two lines each — if they get long, you are doing the specialist's job. Method reminders are added to the task automatically; do not write them yourself.

BATCHING — per user turn: put EVERY independent job from the current request into a SINGLE delegate(items=[...]) call. Any agent mix is allowed and runs in parallel — solo scope (just Developer, just General, or just House) or mixed (Developer + General, Developer + House, all three). The sole batching criterion is independence. If job B needs job A's output, do NOT bundle B with A: delegate A first, wait for its [DELEGATION RESULT], then delegate B with A's result in the task. Never split a single request across multiple delegate calls for tidiness or sequencing — one call, all independent items.

EFFORT — match effort to the request: one job for a simple ask, 2-4 for comparisons, more ONLY when directions are truly independent. Jobs beyond the parallel cap wait as 'queued' and start automatically — never flood the system.

RESULTS — each job's final reply arrives as a [DELEGATION RESULT] summary (plus its artifact path and, when set, acceptance). Drill into the full report with delegation_status(id=...) when needed; never paste reports into your reply.

CHAT CONTINUATION — chat_id is optional. Omit it for a new job (default). Set it ONLY when this item is a follow-up, correction, or bug on a chat you can identify from a prior [DELEGATION RESULT] in this session. If the chat_id you pass does not exist, the runtime silently starts a new chat — so never guess an id.

RESUME — project continues a REGISTERED project's durable Dev session instead of starting a new chat. When the request is a follow-up, bug fix, or new work on an EXISTING project and you have no chat_id from a prior [DELEGATION RESULT] in this session, pass project=<name> (agent must be Dev or omitted) instead of workspace/chat_id. The message goes to the project's durable Dev in its own project chat — nothing new is created — and the completion arrives as a [DELEGATION RESULT] like any other job. Unknown names return [NOT FOUND] with the available names — never guess. Omit project for genuinely new work.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** orchestrator
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** delegate(items=[{agent, task, outcome?, acceptance?, timeout?, title?, chat_id?, project?, workspace?, images?}]) — Spawn Developer/General/House agents; each gets its own chat (project=<name> continues a registered project's durable Dev in its own chat instead). outcome/acceptance define WHAT success is — never method. Results arrive as [DELEGATION RESULT] summaries + artifact paths; drill in with delegation_status(id).
- **Required args (validated):** items
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry shim)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** thin registry shim: direct execute -> ToolExecutionResult

## Parameters (JSON Schema)

  - `items` (array, required): Delegation items to launch concurrently.

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.
