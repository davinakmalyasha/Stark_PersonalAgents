# house_set_device

- **Description:** Control a house device. state keys depend on type: light {power, brightness 0-100, colorTemp 2200-6500}; thermostat {targetTemp 10-35, mode off|heat|cool|auto}; lock {locked}; plug {power}; blinds {position 0-100}; camera {recording}. Sensors are read-only.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** house
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** house_set_device(device_id, state) — Actuate a device: light {power,brightness,colorTemp}, thermostat {targetTemp,mode}, lock {locked}, plug {power}, blinds {position}, camera {recording}. Sensors read-only.
- **Required args (validated):** device_id, state
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry shim)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** thin registry shim: direct execute -> ToolExecutionResult

## Parameters (JSON Schema)

  - `device_id` (string, required): Device id to actuate.
  - `state` (object, required): State patch, e.g. {"power": true, "brightness": 60} or {"targetTemp": 21.5}.

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.
