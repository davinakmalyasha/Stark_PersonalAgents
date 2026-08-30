# ask_user

- **Description:** Ask the caller structured questions and wait for answers. Use on material ambiguity, a decision only the caller can make, or plan approval. Each question needs 2-4 options (use multiSelect=true if more than one may apply). For plan-approval flows, set intent.kind="plan-review" on the question and put the approving option's label in intent.approve.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** developer, orchestrator, general, house, dfd
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** ask_user(questions, timeout?) — Ask the human a structured question and wait for the answer (use when the request is ambiguous, a decision is needed, or a plan needs approval via plan-review intent).
- **Required args (validated):** questions
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** standard registry: pre_hooks(cache) -> guards(plan-mode) -> execute -> post_hooks(redact/offload)

## Parameters (JSON Schema)

  - `questions` (array, required): 
  - `timeout` (integer, optional): Seconds to wait before aborting.

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. Optional `input_schema` in `tools/_tool_schemas.py` (opt-in Stage1 validation).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Registry auto-derives scope visibility from `_SCOPE_MAP` (runtime `restrict_scope`).
