# outline

- **Description:** Signature outline of ONE file — every function/class/method with its definition line, bodies omitted (~1/10 the tokens of a full read). Use before read_file: pick the symbol here, then read_file(offset=line, limit=N) for just its body.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** developer
- **Parallel-safe (PARALLEL_SAFE):** True
- **Compact doc:** outline(file_path) — Every signature in one file with definition lines, no bodies (~1/10 the tokens). Then read_file(offset=line, limit=N) for one body.
- **Required args (validated):** file_path
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry shim)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** thin registry shim: direct execute -> ToolExecutionResult

## Parameters (JSON Schema)

  - `file_path` (string, required): File to outline.

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.
