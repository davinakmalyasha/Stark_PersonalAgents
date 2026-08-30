# github_apply_gate

- **Description:** Create a repo and/or install the tiered CI/security gate: T1 fast-checks / T2 staging / T3 release workflows + .harness-ci.json, dev/staging/prod branch ladder, waits for first run, enforces prod branch protection. Non-destructive otherwise; stack auto-detected. Confirms first (gate=ask).
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** deploy
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** github_apply_gate(repo, stack?, codeql_on?, create?, private?, description?, gate?) — The ONE gate tool: create the repo (create=True) and/or install the security/perf gate (seed + branch protection). Confirms with the user (gate: ask|yes|no).
- **Required args (validated):** repo
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** standard registry: pre_hooks(cache) -> guards(plan-mode) -> execute -> post_hooks(redact/offload)

## Parameters (JSON Schema)

  - `repo` (string, required): 'owner/name' (plain name when create=true).
  - `stack` (string, optional): 'auto'(default) | python | node | go | php | generic.
  - `create` (boolean, optional): 
  - `private` (boolean, optional): 
  - `description` (string, optional): 
  - `gate` (string, optional): 'ask'(default) | 'yes' | 'no'.
  - `update_levels` (boolean, optional): Re-render level-owned files after a branch rename.

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. Optional `input_schema` in `tools/_tool_schemas.py` (opt-in Stage1 validation).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Registry auto-derives scope visibility from `_SCOPE_MAP` (runtime `restrict_scope`).
