# propose_plan

- **Description:** Stage 4: submit a plan proposal to the user. Use ONLY in plan mode when you want the user to review and (manually) press /build to execute. Outside plan mode this tool is a no-op alias for update_plan. The agent CANNOT transition out of plan mode on its own — the user must press /build.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** developer, orchestrator, general, house, dfd
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
