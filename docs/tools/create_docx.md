# create_docx

- **Description:** Create a Word document (.docx) with an optional title heading and paragraph content (one paragraph per line).
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** general
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** create_docx(file_path, title?, content?) — Create a Word document (.docx).
- **Required args (validated):** file_path
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** standard registry: pre_hooks(cache) -> guards(plan-mode) -> execute -> post_hooks(redact/offload)

## Parameters (JSON Schema)

  - `file_path` (string, required): Output .docx path.
  - `title` (string, optional): Optional document title (heading level 1).
  - `content` (string, optional): Document body — one paragraph per line.

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. Optional `input_schema` in `tools/_tool_schemas.py` (opt-in Stage1 validation).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Registry auto-derives scope visibility from `_SCOPE_MAP` (runtime `restrict_scope`).
