# Tool output limits (caps + offload)

Every model-facing tool result is bounded in **characters** (not bytes) and
recoverable: output past the cap is written to the VFS at
`/scratch/large_tool_results/<tool>_<id>.txt` and the model receives a
head+tail preview plus a pointer it can page with `read_file(offset, limit)`.

## Default caps

| Cap key | Default (chars) | Applies to |
| --- | --- | --- |
| `read_file` | 50000 | read_file (first page also paged at 2000 lines) |
| `run_command` | 30000 | run_command |
| `run_command_timeout` | 30000 | run_command timeout path |
| `logs` | 30000 | run_command `process_action=logs` |
| `subagent` | 32000 | call_subagent, delegate, subagent replies |
| `mcp` | 25000 | MCP tools (`mcp__*` also map here) |
| `browser_act` | 24000 | browser_act / browser_navigate |
| `search_code` | 16000 | search_code |
| `edit_file` | 10000 | edit_file (including verify output) |
| `web_search` | 10000 | web_search |
| `ask_user` | 4000 | ask_user |
| `github_apply_gate` | 4000 | github_apply_gate |
| `update_plan` | 2000 | update_plan status result |
| *(unlisted tools)* | 16000 | fallback |

The preview returned for an offloaded result is
`min(8000, cap)` characters, split head 25% / tail 75%, with individual
lines trimmed to 512 chars. `read_file` trims lines wider than 2000 chars.

## Overrides

Every cap key accepts an environment override:

```
CAP_<KEY>=<chars>
```

Examples: `CAP_RUN_COMMAND=50000`, `CAP_SUBAGENT=64000`, `CAP_MCP=40000`.
Aliases resolve to their key first (`call_subagent`/`delegate` -> `subagent`,
`browser_navigate` -> `browser_act`, `mcp__*` -> `mcp`), so
`CAP_SUBAGENT` covers all subagent surfaces.

## Enforcement

Caps are applied at the two execution boundaries, not per loop:

- `tools/_registry.py::execute_tool_detailed` — classic and free-pool loops
  (developer, general, house, dfd, orchestrator, subagent_manager,
  subagentTools).
- `llm/native_tools.py::_after_tool_execute` — the pydantic-ai native path
  (paid models and specialist subagents).

Tools with their own semantic limits (`search_code` match count and
`max_matches`, `read_file` paging, MCP manager) self-cap first; the
boundary cap is a backstop.

## run_command execution limits (time, not chars)

| Setting | Default | Env override | Behavior |
| --- | --- | --- | --- |
| Wall timeout | 120000 ms | `CMD_DEFAULT_TIMEOUT_MS` | Hard ceiling per foreground call (max 600000; explicit `timeout` wins). |
| Idle timeout | 60000 ms | `CMD_IDLE_TIMEOUT_MS` | Kill when there is no output **and** no CPU activity for this long; `idle_timeout=0` disables. |
| CPU activity threshold | 0.10 | `CMD_IDLE_CPU_MIN` | Fraction of one core over the 0.5s sample window treated as "working" (silent builds survive). |
| Auto-background window | 10000 ms | `CMD_BG_ACTIVITY_WINDOW_MS` | On a wall timeout, commands CPU-active inside this window are backgrounded instead of killed. |
| Background readiness | 15000 ms | `CMD_READY_TIMEOUT_MS` | `background=true` waits this long for `expect` or a listening port, then reports `[READY] ...`. |
| Repeat compression | on | `CMD_COMPRESS_REPEATS=0` disables | Runs of >=5 identical lines collapse to one line + `[repeated xN]`. |
| Login shell | off | `AGENT_BASH_LOGIN=1` | `bash -c` by default (fast, no MSYS profile noise); `-lc` opts back in. |
| MSYS path conversion | on | `AGENT_MSYS_NO_PATHCONV=1` disables | Stop Git Bash converting `/foo` args for native programs (also honors `MSYS2_ARG_CONV_EXCL`). |
| Secret-file reads | blocked | `CMD_ALLOW_SECRET_READS=1` allows | Shell reads of `.env`, `~/.ssh`, `~/.aws`, `~/.config/gh` are vetoed (OpenCode-style default). |

`grep`/`rg`/`find`/`diff`/`test` (and `git diff|grep`) exit 1 are normal
negative answers: they frame as `[OK] Exit 1 (no match, Nms)` and do not count
as failures in telemetry or the VERIFY gate.

Tree termination uses psutil (TERM -> 2s grace -> KILL) so background servers and
their grandchildren never survive a timeout or `process_action='kill'`. Each
run's background processes are killed when the chat run finishes
(`_procman.cleanup_owner`) and at process exit.
