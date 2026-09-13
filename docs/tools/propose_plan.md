# propose_plan

- **Description:** Present the plan for the CURRENT plan-mode review. Plan mode is user-initiated via the prompt dock ONLY — never call this to start planning on your own, never suggest switching modes. Outside plan mode this is a no-op (use update_plan and keep working). The agent CANNOT change plan/build mode in either direction.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** developer, general, house, dfd, orchestrator
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** (none)
- **Required args (validated):** plan
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry shim)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** thin registry shim: direct execute -> ToolExecutionResult

## Parameters (JSON Schema)

  - `explanation` (string, optional): Short note shown above the plan (e.g. 'Here is the plan to fix the crash'). Keep to one line.
  - `rationale` (string, optional): Why this approach (1-3 sentences). Shown in the plan-review UI.
  - `plan` (array, required): 

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.
