# run_command

- **Description:** Run a shell command (Git Bash, POSIX). Foreground blocks and streams output; server-like commands (npm run dev, vite, uvicorn, python -m http.server...) that exceed the timeout are auto-moved to background. Background runs return a process name managed via process_action. Windows tools work as subprocesses: powershell.exe -NoProfile -Command '...' or cmd.exe /c '...'.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** developer, general, house, dfd, deploy, explore, ui
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** run_command(command, timeout?, idle_timeout?, background?, process_action?, workdir?, expect?, tail_lines?, ready_timeout?, input?) — Git Bash (POSIX): pipes, &&, $( ), jq, gh. timeout ms (default 120000; <1000 = seconds; max 600000); idle_timeout kills commands idle by output AND CPU (default 60000; 0 = off); server-like/busy commands auto-background on timeout. process_action=list|logs|kill|wait|ready|send|kill_all. background+expect waits for readiness and reports the listening URL. workdir sets cwd, expect checks output regex.
- **Required args (validated):** (none)
- **Deps-migrated:** True (accepts ctx=WorkspaceDeps via registry shim)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema | redact via Hooks.after_tool_execute | offload via after_tool_execute
- **Pipeline:** direct shim call (no hooks)

## Parameters (JSON Schema)

  - `command` (string, optional): Shell command; also names the process for process_action='logs'|'kill'|'wait'.
  - `timeout` (integer, optional): Wall-clock limit in ms (default 120000, max 600000; values <1000 are seconds). Use >=60000 for installs; background=true for servers.
  - `idle_timeout` (integer, optional): Kill after this long with no output and no CPU activity, in ms (default 60000; 0 disables). Server-like/busy commands are backgrounded instead of killed.
  - `background` (boolean, optional): 
  - `process_action` (string, optional): 'list' | 'logs' | 'kill' | 'wait' | 'ready' | 'send' | 'kill_all' (command carries the process name).
  - `workdir` (string, optional): Working directory (default project root).
  - `expect` (string, optional): Regex expected in output; typed pass/fail. For background starts it is awaited up to ready_timeout.
  - `tail_lines` (integer, optional): Lines returned by process_action='logs' (default 200).
  - `ready_timeout` (integer, optional): Background readiness wait in ms (default 15000; 0 = don't wait). Also used by process_action='ready'.
  - `input` (string, optional): process_action='send': text written to the background process stdin.

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.
