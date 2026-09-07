# run_command

- **Description:** Run a shell command. Foreground blocks and streams logs; background returns a key managed via process_action.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** developer, ui, deploy, general, house, dfd
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** run_command(command, timeout?, background?, process_action?, shell?, workdir?, expect?, stream?) — Full POSIX bash (default): pipes, &&, $( ), jq, gh pipelines all work. shell='cmd' for legacy, workdir sets cwd, expect checks output regex, stream for live output.
- **Required args (validated):** (none)
- **Deps-migrated:** True (accepts ctx=WorkspaceDeps via registry shim)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema | redact via Hooks.after_tool_execute | offload via after_tool_execute
- **Pipeline:** direct shim call (no hooks)

## Parameters (JSON Schema)

  - `command` (string, optional): Shell command; also names the process for process_action='logs'|'kill'.
  - `timeout` (integer, optional): Foreground timeout ms.
  - `background` (boolean, optional): 
  - `process_action` (string, optional): 'list' | 'logs' | 'kill'.
  - `shell` (string, optional): 'bash' only (Git Bash 5.3.15); 'cmd' is removed — use bash.
  - `workdir` (string, optional): Working directory (default project root).
  - `expect` (string, optional): Regex expected in output; typed pass/fail.

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.
