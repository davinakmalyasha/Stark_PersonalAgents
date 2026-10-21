# look_at_screen

- **Description:** Capture the user's live screen RIGHT NOW and attach it inline (vision). Use when the answer depends on what the user is currently viewing: a highlighted passage, a chart, a page, an error, or when they say 'this'/'here'/'what I'm looking at'. Turns the overlay's screen sharing on if it is off. The frame reaches you as an image before your next reply.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** general
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** look_at_screen() — Capture the user's live screen now and attach it inline (vision). For 'this'/'here'/highlighted content questions; enables sharing if off.
- **Required args (validated):** (none)
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry shim)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** thin registry shim: direct execute -> ToolExecutionResult

## Parameters (JSON Schema)

  - (none)

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.
