# call_subagent

- **Description:** Specialist subagent lifecycle (see Subagents list). action=run: spawn and block until the final report (default). action=spawn: start it in the background and keep working on other parts, then collect with status. action=status: status + latest result of a spawned child (wait_seconds≤300). action=message: steer a running child (queued) or send a follow-up. action=stop: interrupt a running child. action=list: this session's subagents. Prefer spawn+status for deep/long verification so you are not blocked.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** developer
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** call_subagent(name, task, action?, child_id?, message?, wait_seconds?) — Specialist subagent lifecycle: run (block) / spawn (background) / status / message / stop / list (e.g. UI for browser verification).
- **Required args (validated):** (none)
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry shim)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** thin registry shim: direct execute -> ToolExecutionResult

## Parameters (JSON Schema)

  - `name` (string, optional): Subagent type from the Subagents list, e.g. UI | Deploy. Required for run/spawn.
  - `task` (string, optional): Goal + targets (files/URL/repo) + acceptance check + constraints. Context, not method — do not prescribe design direction or step-by-step process. Required for run/spawn.
  - `action` (string, optional): Lifecycle action (default run).
  - `child_id` (string, optional): Spawned child id (from action=spawn) for status/message/stop.
  - `message` (string, optional): Follow-up/steering message for action=message.
  - `wait_seconds` (integer, optional): For status: poll up to this many seconds (max 300) before returning.

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.
