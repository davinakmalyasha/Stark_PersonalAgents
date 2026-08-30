# edit_file

- **Description:** Edit or create files via exact replacement.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** developer
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** edit_file(file_path, old_string?, new_string?, diff?, verify?) — The ONE file tool: new_string only (no old_string) = create/overwrite; old_string+new_string = targeted replace; diff = batch SEARCH/REPLACE blocks. Opt-in verify=py_compile|tsc|shell runs after write. Boilerplate → template(action="apply") first.
- **Required args (validated):** file_path
- **Deps-migrated:** True (accepts ctx=WorkspaceDeps via registry shim)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema | approval via Hooks.before_tool_execute (plan-mode + core-config veto)
- **Pipeline:** direct shim call (no hooks)

## Parameters (JSON Schema)

  - `file_path` (string, required): 
  - `old_string` (string, optional): Exact existing text.
  - `new_string` (string, optional): Replacement; omit old_string to create/overwrite.
  - `diff` (string, optional): Batch mode. (1) SEARCH/REPLACE blocks: <<<<<<< SEARCH\n[old]\n=======\n[new]\n>>>>>>> REPLACE; (2) unified diff (git @@ hunks, uses file_path); (3) multi-file patch: '*** Add File: path', '*** Update File: path', '*** Delete File: path', '*** Move to: path' headers — applied per-file transactionally with [OK]/[ERROR] summary.
  - `verify` (string, optional): 'py_compile' | 'tsc' | shell command — runs after write.

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.
