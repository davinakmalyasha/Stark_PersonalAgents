# house_run_scene

- **Description:** Activate a house scene (applies all its device actions at once). Available scenes are listed in house_get_state, e.g. scene-morning, scene-night, scene-away, scene-movie, scene-focus.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** house
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** house_run_scene(scene_id) — Activate a scene (applies all actions at once).
- **Required args (validated):** scene_id
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry shim)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** thin registry shim: direct execute -> ToolExecutionResult

## Parameters (JSON Schema)

  - `scene_id` (string, required): Scene id, e.g. 'scene-night'.

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.
