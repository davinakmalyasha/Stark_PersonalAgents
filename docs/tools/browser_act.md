# browser_act

- **Description:** Browser automation: navigate, click, fill, type, press, extract, screenshot, close. Selector = CSS or @ref from the navigate snapshot. Start servers with run_command(background=true) first.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** general, house, ui
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** browser_act(action, selector?, text?, key?, what?, path?) — click / fill / type / press / extract / screenshot / close.
- **Required args (validated):** action
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry shim)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** thin registry shim: direct execute -> ToolExecutionResult

## Parameters (JSON Schema)

  - `action` (string, required): 
  - `url` (string, optional): 
  - `wait_ms` (integer, optional): Wait after action, ms.
  - `selector` (string, optional): CSS or @ref from snapshot.
  - `text` (string, optional): 
  - `key` (string, optional): 
  - `what` (string, optional): 'text'|'html'|'value'|'title'|'url'.
  - `path` (string, optional): Screenshot save path.

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.
