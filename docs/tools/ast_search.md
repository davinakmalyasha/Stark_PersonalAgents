# ast_search

- **Description:** Structural search over Python files (stdlib AST, no text matching). Warehouse tool: not assigned to any agent scope yet; enable per specialist. Results are workspace-relative and bounded like search_code.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** (not exposed to any agent scope)
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** ast_search(rule, name?, path?, glob?, min_lines?, max_params?, max_results?) — Structural search over Python (call/import/class_bases/decorator/bare_except/long_function/many_params/no_docstring).
- **Required args (validated):** rule
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry shim)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** thin registry shim: direct execute -> ToolExecutionResult

## Parameters (JSON Schema)

  - `rule` (string, required): Structural query. call/import/class_bases/decorator require name; long_function/many_params use min_lines/max_params.
  - `name` (string, optional): Function/method/module/class/decorator name for name-based rules (dotted names allowed).
  - `path` (string, optional): Directory or single file to search (default: workspace root).
  - `glob` (string, optional): Comma-separated file filters, '!' excludes. Python files only.
  - `min_lines` (integer, optional): long_function threshold in lines (default 60).
  - `max_params` (integer, optional): many_params threshold; functions above it are reported (default 5).
  - `max_results` (integer, optional): Matches shown inline (default 100).

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.
