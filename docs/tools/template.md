# template

- **Description:** Boilerplate catalog: list, keyword search, show details, apply (renders files; skips existing unless overwrite).
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** developer, deploy
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** template(action, template?, category?, query?, target_dir?, variables?, overwrite?, dry_run?, quiet?) — Unified template: list/search/show/apply (replaces apply_template + template_list).
- **Required args (validated):** action
- **Deps-migrated:** True (accepts ctx=WorkspaceDeps via registry)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema | cache via Hooks | FilteredToolset
- **Pipeline:** cached

## Parameters (JSON Schema)

  - `action` (string, required): 
  - `template` (string, optional): Name for show/apply.
  - `category` (string, optional): 
  - `query` (string, optional): Keywords for search/list.
  - `target_dir` (string, optional): Apply destination (default workspace root).
  - `variables` (object, optional): Template variables.
  - `overwrite` (boolean, optional): 
  - `dry_run` (boolean, optional): 
  - `quiet` (boolean, optional): 

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. Optional `input_schema` in `tools/_tool_schemas.py` (opt-in Stage1 validation).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Registry auto-derives scope visibility from `_SCOPE_MAP` (runtime `restrict_scope`).
