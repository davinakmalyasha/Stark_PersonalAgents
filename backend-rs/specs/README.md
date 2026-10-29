# Tool contract v2

`tool-specs.json` is the single model-facing tool contract for the Rust server.
It now contains two surfaces:

- **v1 tools** — legacy Stark tools still used by General, House, DFD, UI,
  DevOps, Explore, and Orchestrator.
- **v2 Developer tools** — the 24-tool Developer surface copied from DeepSeek
  Harness `standard` and Codex multi-agent v2.

## Developer v2 sources

| Source | Pin | Used for |
|---|---|---|
| `deepseek-ai/deepseek-harness` | `f3803c9` (0.2.2-alpha.2, 2026-10-27) | `bash`, `job_*`, `read`, `write`, `edit`, `read_image`, `glob`, `grep`, `working_directory`, `todo_write`, `exit_plan_mode`, `ask_user_question`, `skill`, `web_search`, `web_fetch` |
| `openai/codex` | `79be2e2` (2026-10-29) | `spawn_agent`, `send_message`, `followup_task`, `wait_agent`, `list_agents`, `interrupt_agent`, `close_agent` |

The model-facing names, descriptions, parameter descriptions, and required
lists are intended to be byte-compatible with those sources. The only
documented deviations are:

1. The system identity says **Stark**, not DeepSeek Harness.
2. Windows uses Git Bash for `bash` instead of DeepSeek's `pwsh`.
3. Codex v2 delegation replaces DeepSeek's `subagent`/`subagent_fork` tools.
4. DeepSeek's optional `goal_*`, `schedule_*`, `workflow`, `present`, and
   plugin-manager tools are omitted.
5. Stark's `memory` tool, worktree isolation, and merge/discard tools are
   removed from the Developer surface for strict parity.

## Developer v2 manifest

```
bash, job_kill, job_list, job_output,
read, write, edit, read_image, glob, grep,
working_directory, todo_write, exit_plan_mode, ask_user_question,
skill, web_search, web_fetch,
spawn_agent, send_message, followup_task, wait_agent,
list_agents, interrupt_agent, close_agent
```

`run_code` is not part of the default Developer surface. DeepSeek Harness keeps
PTC/code mode in a separate preset.

## Editing

`tool-specs.json` is validated by `stark-contract` and consumed by
`stark-tools::tool_specs_for_scope_with_access`. Do not add a tool to
`scope_map.developer` without also adding it to `IMPLEMENTED` in
`stark-tools/src/lib.rs` and implementing a dispatch arm.

`docs/tools/README.md` points at the contract and the v2 parity doc; the old
per-tool pages were deleted. The v2 Developer surface is documented by the
contract itself and `docs/rust-rewrite/18-deepseek-parity.md`.
