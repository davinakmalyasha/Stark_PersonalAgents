# Tool contract v4 (Codex-aligned agent surfaces)

`tool-specs.json` is the single model-facing tool contract for the Rust server.
It now contains two surfaces:

- **Workspace tools** — the v2 surface used by Developer, Coder, General, House,
  DFD, UI, DevOps, and Explore.
- **Orchestrator tools** — read-only `bash` plus `memory`, delegation, and coordination.

Legacy names (`read_file`, `edit_file`, `search_code`, `run_command`,
`call_subagent`, `ask_user`, `todo_write`) and the removed `write` tool are
not in the contract or any `scope_map`. A dispatch denylist remains for old
transcripts.

## Developer sources

| Source | Pin | Used for |
|---|---|---|
| `deepseek-ai/deepseek-harness` | `f3803c9` (0.2.2-alpha.2, 2026-10-27) | `bash`, `job`, `read`, `apply_patch`, `edit`, `glob`, `grep`, `update_plan`, `ask_user_question`, `skill`, `web_search`, `web_fetch` |
| `openai/codex` | `79be2e2` (2026-10-29) | `spawn_agent`, `message_agent`, `wait_agent`, `list_agents`, `interrupt_agent`, `close_agent` |

Pi simplifications on top of the sources (documented, not byte-identical):

1. `read_image` folded into `read` (images return inline parts).
2. `job_list`/`job_output`/`job_kill` merged into one `job(action, ...)` tool.
3. `send_message`/`followup_task` merged into one `message_agent(target, message, wake?)` tool.
4. `working_directory` deleted; cwd is Zone 1 runtime context in the prompt.
5. `exit_plan_mode` deleted; approval is `ask_user_question` with plan-review
   intent (auto-flips plan to build) plus the composer Approve button.

Other deviations (unchanged):

- The system identity says **Stark**, not DeepSeek Harness.
- Windows uses Git Bash for `bash` instead of DeepSeek's `pwsh`.
- Codex v2 delegation replaces DeepSeek's `subagent`/`subagent_fork`.
- DeepSeek's optional `goal_*`, `schedule_*`, `workflow`, `present`, and
  plugin-manager tools are omitted.
- Stark's `memory` tool, worktree isolation, and merge/discard tools are
  removed from the Developer surface.

## Developer manifest (17)

```
bash, job,
read, apply_patch, edit, glob, grep,
update_plan, ask_user_question,
skill, web_search, web_fetch,
spawn_agent, message_agent, wait_agent,
interrupt_agent, close_agent
```

``run_code` is not part of the default Developer surface; `tool_search` is General-only and Developer delegates MCP work. DeepSeek Harness keeps
PTC/code mode in a separate preset.

## Editing

`tool-specs.json` is validated by `stark-contract` and consumed by
`stark-tools::tool_specs_for_scope_with_access`. Do not add a tool to
`scope_map.developer` without also adding one `ToolMeta` entry to the
unified inventory in `stark-tools/src/inventory.rs` and implementing a
dispatch arm, then regenerating labels with
`node backend-rs/specs/generate-labels.mjs`.

`docs/tools/README.md` points at the contract and the v2 parity doc; the old
per-tool pages were deleted. The Developer surface is documented by the
contract itself and `docs/rust-rewrite/18-deepseek-parity.md`.
