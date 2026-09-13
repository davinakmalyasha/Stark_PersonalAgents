# ask_user

- **Description:** Ask the caller structured questions and wait for answers. Use on material ambiguity, a decision only the caller can make, or plan approval. Each question needs 2-4 options (use multiSelect=true if more than one may apply). For plan-approval flows, set intent.kind="plan-review" on the question and put the approving option's label in intent.approve.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** developer, general, house, dfd, ui, deploy, orchestrator
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** ask_user(questions, timeout?) — Ask the human a structured question and wait for the answer (use when the request is ambiguous, a decision is needed, or a plan needs approval via plan-review intent).
- **Required args (validated):** questions
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry shim)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** thin registry shim: direct execute -> ToolExecutionResult

## Parameters (JSON Schema)

  - `questions` (array, required): 
  - `timeout` (integer, optional): Seconds to wait before aborting.

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.
