# set_screen_share

- **Description:** Change the live overlay's screen-sharing mode: 'on' (preview + one fresh frame attached to each voice turn), 'off' (stop sharing), 'continuous' (also refresh the latest frame every few seconds so look_at_screen is near-real-time). Ask the user before turning 'continuous' on; turn it off when the task is done.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** general
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** set_screen_share(mode) — Live screen sharing: on / off / continuous (continuous refreshes every few seconds for near-real-time look_at_screen).
- **Required args (validated):** mode
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry shim)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** thin registry shim: direct execute -> ToolExecutionResult

## Parameters (JSON Schema)

  - `mode` (string, required): Desired sharing mode.

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.
