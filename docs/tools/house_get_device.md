# house_get_device

- **Description:** Read one house device: current state, writable capabilities, room and online status. Use to check a device before or after actuating it.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** house
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** house_get_device(device_id) — One device's state, capabilities, room, online status.
- **Required args (validated):** device_id
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** standard registry: pre_hooks(cache) -> guards(plan-mode) -> execute -> post_hooks(redact/offload)

## Parameters (JSON Schema)

  - `device_id` (string, required): Device id, e.g. 'light-living-1', 'thermostat-bedroom', 'lock-garage'.

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. Optional `input_schema` in `tools/_tool_schemas.py` (opt-in Stage1 validation).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Registry auto-derives scope visibility from `_SCOPE_MAP` (runtime `restrict_scope`).
