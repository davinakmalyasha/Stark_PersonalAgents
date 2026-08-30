# browser_screenshot

- **Description:** Take a screenshot of the current browser page. Saves to a temp file and returns the path. Use for visual verification of pages.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** (not exposed to any agent scope)
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** browser_screenshot(path?) — Screenshot of current browser page.
- **Required args (validated):** (none)
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** standard registry: pre_hooks(cache) -> guards(plan-mode) -> execute -> post_hooks(redact/offload)

## Parameters (JSON Schema)

  - `path` (string, optional): Optional file path to save the screenshot. Auto-named if empty.

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. Optional `input_schema` in `tools/_tool_schemas.py` (opt-in Stage1 validation).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Registry auto-derives scope visibility from `_SCOPE_MAP` (runtime `restrict_scope`).
