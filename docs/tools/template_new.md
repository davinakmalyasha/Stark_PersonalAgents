# template_new

- **Description:** Author a new template: write .j2 files into templates/<level>/<name>/files/ with edit_file first, then call this with metadata to write template.json and validate rendering. Levels: stack, feature, component, snippet, project, config, routine, spec.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** (not exposed to any agent scope)
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** template_new(name, level, description, sample_vars, variables?, files?, uses?, notes?) — Author a new template (write .j2 files first, then metadata; auto-validates render).
- **Required args (validated):** description, level, name, sample_vars
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry shim)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** thin registry shim: direct execute -> ToolExecutionResult

## Parameters (JSON Schema)

  - `name` (string, required): Lowercase kebab-case template name, e.g. 'feature-billing'.
  - `level` (string, required): Category dir: stack, feature, component, snippet, project, config, routine, spec.
  - `description` (string, required): ONE-line description shown in template_list.
  - `sample_vars` (object, required): Example variables the CI uses to validate rendering, e.g. {'feature_name': 'User'}.
  - `variables` (array, optional): Optional variable specs: [{'name': ..., 'type': 'string|int|bool|list', 'required': bool, 'default': ..., 'description': ...}] or plain names ['var_a'].
  - `files` (array, optional): Optional file specs [{'src': 'x.py.j2', 'dest': '{{ var }}/x.py'}] — defaults to all *.j2 files in files/ with dest = name minus .j2.
  - `uses` (array, optional): Optional composition recipe (template names or {template, variables} dicts).
  - `notes` (string, optional): Optional conventions/adaptation notes shown in the detailed view.

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.
