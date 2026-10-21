# edit_file

- **Description:** Edit or create files. Mode A — targeted change: old_string -> new_string (must match exactly and uniquely; identical strings error). Mode B — create or overwrite a WHOLE file: file_path + new_string only. Mode C — batch/multi-file diff: '*** Add File: p' / '*** Update File: p' / '*** Delete File: p' / '*** Move to: p' headers, optionally wrapped in '*** Begin Patch' / '*** End Patch'. Update bodies use @@ hunks: lines prefixed '-' are removed, '+' are added, and ' ' (space) are unchanged context lines that anchor the hunk — line numbers are optional and ignored, so hunks tolerate drift (preferred over SEARCH/REPLACE for multi-hunk and multi-file edits). Applied transactionally with [OK]/[ERROR] summary. verify = 'py_compile' | 'tsc' | shell command, runs after the write.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** developer, deploy, ui
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** edit_file(file_path, old_string?, new_string?, diff?, verify?) — A: old_string+new_string = targeted unique replace; B: new_string only = create/overwrite whole file; C: diff = '*** Add/Update/Delete/Move File' headers (optional *** Begin Patch/End Patch) with @@ hunks: '-' removed, '+' added, ' ' context anchors (numbers optional). verify=py_compile|tsc|shell after write.
- **Required args (validated):** file_path
- **Deps-migrated:** True (accepts ctx=WorkspaceDeps via registry shim)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema | approval via Hooks.before_tool_execute (plan-mode + core-config veto)
- **Pipeline:** direct shim call (no hooks)

## Parameters (JSON Schema)

  - `file_path` (string, required): 
  - `old_string` (string, optional): Exact existing text (Mode A).
  - `new_string` (string, optional): Replacement (Mode A) or full file content (Mode B: omit old_string).
  - `diff` (string, optional): Mode C payload: '*** File:' headers (optional *** Begin Patch / *** End Patch envelope) with @@ hunks — '-' removed, '+' added, ' ' context anchors; line numbers optional and ignored. Legacy SEARCH/REPLACE blocks and numbered unified diffs are also accepted.
  - `verify` (string, optional): 'py_compile' | 'tsc' | shell command — runs after write.

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.
