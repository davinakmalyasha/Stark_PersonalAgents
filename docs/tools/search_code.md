# search_code

- **Description:** Regex-search file contents ('content', default) or match path globs ('files'). Multi-pattern results ranked.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** developer, ui
- **Parallel-safe (PARALLEL_SAFE):** True
- **Compact doc:** search_code(pattern?, patterns?, path?, glob?) — Regex search across files. Multi-pattern via patterns=[...] ranked.
- **Required args (validated):** (none)
- **Deps-migrated:** True (accepts ctx=WorkspaceDeps via registry shim)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema | cache via Hooks | FilteredToolset
- **Pipeline:** direct shim call (no hooks)

## Parameters (JSON Schema)

  - `pattern` (string, optional): Regex (content mode) or glob/filename fragment (files mode).
  - `patterns` (array, optional): 
  - `path` (string, optional): 
  - `glob` (string, optional): File filter, e.g. '*.py'.
  - `mode` (string, optional): 

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.
