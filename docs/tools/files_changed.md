# files_changed

- **Description:** Stage 5 (advisory): list the most recent external file changes in the active workspace. The agent can call this to notice when the user (or another tool outside this run) edits a file. Does NOT invalidate the read cache or interrupt a running tool. Requires the optional `watchfiles` package; if not installed, returns a friendly 'not available' string.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** orchestrator, general, dfd
- **Parallel-safe (PARALLEL_SAFE):** True
- **Compact doc:** (none)
- **Required args (validated):** (none)
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** standard registry: pre_hooks(cache) -> guards(plan-mode) -> execute -> post_hooks(redact/offload)

## Parameters (JSON Schema)

  - `limit` (integer, optional): Max events to return (1-50, default 5).

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. Optional `input_schema` in `tools/_tool_schemas.py` (opt-in Stage1 validation).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Registry auto-derives scope visibility from `_SCOPE_MAP` (runtime `restrict_scope`).
