# job_message

- **Description:** Steer a RUNNING delegation mid-flight when the user adds scope or changes direction. Fresh jobs: mirrored into the job chat + queued on its steering channel. Resume jobs: queued on the durable Dev child (FIFO). Only works while the job is running — otherwise delegate a follow-up. Never blocks.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** orchestrator
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** job_message(job_id, message) — Steer a RUNNING delegation mid-flight (fresh: steering channel; resume: durable Dev FIFO). Running jobs only; never blocks.
- **Required args (validated):** job_id, message
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry shim)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** thin registry shim: direct execute -> ToolExecutionResult

## Parameters (JSON Schema)

  - `job_id` (string, required): Job id (del-xxxx) from delegate or delegation_status.
  - `message` (string, required): Steering message: the added scope or direction change.

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.
