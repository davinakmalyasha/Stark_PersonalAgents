# read_file

- **Description:** Read one file, or several in ONE call via file_paths (each gets its own [FILE] section; offset/limit apply to every file in the batch) — when 2+ files are needed, batch them in a single call to save turns. A full read returns the first page under a [FILE] header (lines x-y of N); when PARTIAL is shown, page onward with offset/limit. A repeat full read of an unchanged file returns a cached stub — use offset/limit to re-read a specific slice.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** developer, ui, deploy
- **Parallel-safe (PARALLEL_SAFE):** True
- **Compact doc:** read_file(file_path?, file_paths?, offset?, limit?) — Read one file, or batch several via file_paths in ONE call ([FILE] sections; offset 1-based, applies per file in a batch). Full read returns first page + header; PARTIAL -> page with offset/limit; unchanged repeat -> cached stub.
- **Required args (validated):** (none)
- **Deps-migrated:** True (accepts ctx=WorkspaceDeps via registry shim)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | cache via Hooks.before_tool_execute | FilteredToolset scope
- **Pipeline:** direct shim call (no hooks)

## Parameters (JSON Schema)

  - `file_path` (string, optional): Single file path.
  - `file_paths` (array, optional): Batch of file paths to read in one call.
  - `offset` (integer, optional): Start line (1-based; 0 = first line).
  - `limit` (integer, optional): Max lines.

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.
