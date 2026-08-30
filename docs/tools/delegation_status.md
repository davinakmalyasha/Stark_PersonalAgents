# delegation_status

- **Description:** List all delegations spawned this session with live statuses (running / done / failed) and their chat ids. Call when you need to know what is still outstanding — typically before a wrap-up summary, after a long wait, or when you are unsure whether everything has settled.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** orchestrator
- **Parallel-safe (PARALLEL_SAFE):** True
- **Compact doc:** delegation_status() — List this session's delegations with live statuses and chats.
- **Required args (validated):** (none)
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** standard registry: pre_hooks(cache) -> guards(plan-mode) -> execute -> post_hooks(redact/offload)

## Parameters (JSON Schema)

  - (none)

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. Optional `input_schema` in `tools/_tool_schemas.py` (opt-in Stage1 validation).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Registry auto-derives scope visibility from `_SCOPE_MAP` (runtime `restrict_scope`).
