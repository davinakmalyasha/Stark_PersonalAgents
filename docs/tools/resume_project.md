# resume_project

- **Description:** Resume an existing project's durable Dev agent in one call: resolves the project by name/path and delivers the follow-up message. Replaces find_project_dev + send_message + list_agents. If the project is not found, returns [NOT FOUND] with available names.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** orchestrator
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** resume_project(name, message, timeout?) — One-call resume: find the project's Dev and deliver the message (replaces find_project_dev + send_message).
- **Required args (validated):** message, name
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** standard registry: pre_hooks(cache) -> guards(plan-mode) -> execute -> post_hooks(redact/offload)

## Parameters (JSON Schema)

  - `name` (string, required): Project name or folder path (fuzzy match, e.g. 'my-app').
  - `message` (string, required): Follow-up instruction to deliver to the project's Dev agent.
  - `timeout` (integer, optional): Seconds to wait for the reply (default 1800).

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. Optional `input_schema` in `tools/_tool_schemas.py` (opt-in Stage1 validation).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Registry auto-derives scope visibility from `_SCOPE_MAP` (runtime `restrict_scope`).
