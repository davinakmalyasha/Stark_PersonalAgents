# read_file

- **Description:** Read a file, or several via file_paths (per-file sections).
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** developer, ui, deploy
- **Parallel-safe (PARALLEL_SAFE):** True
- **Compact doc:** read_file(file_path?, file_paths?, offset?, limit?) — Read file(s). Single file via file_path; multi-file via file_paths=[paths].
- **Required args (validated):** (none)
- **Deps-migrated:** True (accepts ctx=WorkspaceDeps via registry)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | cache via Hooks.before_tool_execute | FilteredToolset scope
- **Pipeline:** cached (mtime-checked) | redacted(no) | offload(no, read keeps page budget)

## Parameters (JSON Schema)

  - `file_path` (string, optional): 
  - `file_paths` (array, optional): 
  - `offset` (integer, optional): Start line.
  - `limit` (integer, optional): Max lines.

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. Optional `input_schema` in `tools/_tool_schemas.py` (opt-in Stage1 validation).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Registry auto-derives scope visibility from `_SCOPE_MAP` (runtime `restrict_scope`).
