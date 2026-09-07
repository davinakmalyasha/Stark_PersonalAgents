# update_plan

- **Description:** Drive the live job tracker (the dock the boss watches). Call this whenever the set of jobs changes — at spawn, as results land, or when you abort. Each call is a FULL REPLACEMENT: send every step every time, with the current status. At most one step may be in_progress. Use status=completed (not a separate failed value) for any job that has settled — put the reason in the step text when not successful (e.g. 'Build snake game — failed: tests did not pass').
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** developer, orchestrator, general, house, dfd
- **Parallel-safe (PARALLEL_SAFE):** True
- **Compact doc:** update_plan(explanation?, plan=[{step, status}]) — Maintain the task plan. FULL replacement: always send every step. At most one step in_progress.
- **Required args (validated):** plan
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry shim)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** thin registry shim: direct execute -> ToolExecutionResult

## Parameters (JSON Schema)

  - `explanation` (string, optional): Short note shown above the plan (e.g. 'Spawned 3 jobs, waiting for results'). Keep to one line.
  - `plan` (array, required): 

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.
