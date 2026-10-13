# files_changed

- **Description:** Stage 5 (advisory): list the most recent external file changes in the active workspace. The agent can call this to notice when the user (or another tool outside this run) edits a file. Does NOT invalidate the read cache or interrupt a running tool. Requires the optional `watchfiles` package; if not installed, returns a friendly 'not available' string.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** general, dfd
- **Parallel-safe (PARALLEL_SAFE):** True
- **Compact doc:** (none)
- **Required args (validated):** (none)
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry shim)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** thin registry shim: direct execute -> ToolExecutionResult

## Parameters (JSON Schema)

  - `limit` (integer, optional): Max events to return (1-50, default 5).

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.
