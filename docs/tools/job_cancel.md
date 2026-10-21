# job_cancel

- **Description:** Cancel ONE running delegation when the user says stop. Fresh jobs: cooperative abort of the job chat. Resume jobs: interrupt of the durable Dev child. Settles the entry as CANCELLED and reports back as a [DELEGATION RESULT]. Already-settled jobs are a no-op.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** orchestrator
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** job_cancel(job_id, reason?) — Cancel ONE running delegation (fresh: abort; resume: interrupt). Reports back as [DELEGATION RESULT] CANCELLED.
- **Required args (validated):** job_id
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry shim)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** thin registry shim: direct execute -> ToolExecutionResult

## Parameters (JSON Schema)

  - `job_id` (string, required): Job id (del-xxxx) from delegate or delegation_status.
  - `reason` (string, optional): One-clause reason shown in the wrap-up (default: cancelled by the user).

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.
