# delegation_status

- **Description:** List all delegations spawned this session with live statuses (queued / running / done / failed / cancelled) and their chat ids. Call when you need to know what is still outstanding — typically before a wrap-up summary, after a long wait, or when you are unsure whether everything has settled. Pass id for ONE job's full evidence bundle (task, outcome, acceptance, full report, recent chat, file digest); wait_seconds (0-300) waits briefly for a job first; status filters the board.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** orchestrator
- **Parallel-safe (PARALLEL_SAFE):** True
- **Compact doc:** delegation_status(id?, wait_seconds?, status?) — Board of this session's delegations (queued/running/done/failed/cancelled) with chats and elapsed time; id=<del-id> drills into one job's full evidence (task, outcome, acceptance, full artifact report, recent chat, file digest).
- **Required args (validated):** (none)
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry shim)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** thin registry shim: direct execute -> ToolExecutionResult

## Parameters (JSON Schema)

  - `id` (string, optional): Job id (del-xxxx) for the full evidence bundle of one job. Omit for the board.
  - `wait_seconds` (integer, optional): Wait up to this long for a job before answering (default 0 = answer now).
  - `status` (string, optional): Board filter: only show jobs in this state.

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.
