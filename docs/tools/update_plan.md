# update_plan

- **Description:** Maintain the plan dock. Call when jobs change (spawn, progress, abort). FULL REPLACEMENT each call: send every step with its current status; at most one in_progress. Settled jobs use status=completed — put the reason in the step text when not successful. For small tweaks use ops instead of resending everything (mutually exclusive with plan): [{op, match?, step?, status?}] with op=add (needs step) | set_status (needs match+status) | rename (needs match+step) | remove (needs match); match is a case-insensitive substring of the step text.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** developer, general, house, dfd
- **Parallel-safe (PARALLEL_SAFE):** True
- **Compact doc:** update_plan(explanation?, plan?, ops?) — Maintain the task plan. FULL replacement: always send every step (at most one in_progress), or patch small tweaks via ops=[{op: add|set_status|rename|remove, match?, step?, status?}] (mutually exclusive with plan).
- **Required args (validated):** (none)
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry shim)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** thin registry shim: direct execute -> ToolExecutionResult

## Parameters (JSON Schema)

  - `explanation` (string, optional): Short note shown above the plan (e.g. 'Spawned 3 jobs, waiting for results'). Keep to one line.
  - `ops` (array, optional): Incremental patch (alternative to plan).
  - `plan` (array, optional): 

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.
