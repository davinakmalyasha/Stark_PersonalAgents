# 18 — DeepSeek / Codex Parity

Developer is the first agent migrated to the v2 tool surface. Other agents keep
the v1 Stark tools until they are migrated deliberately.

## Sources

| Surface | Repository | Pin |
|---|---|---|
| Core coding tools | `deepseek-ai/deepseek-harness` | `f3803c9` (0.2.2-alpha.2, 2026-10-27) |
| Delegation | `openai/codex` | `79be2e2` (2026-10-29) |

The DeepSeek source is the `standard` preset at
`packages/bundle/web-app/presets/standard.patch.yml` and the generated
`docs/tool-catalog.md`. The Codex source is
`codex-rs/core/src/tools/handlers/multi_agents_spec.rs` (v2).

## Developer manifest

| Group | Tools |
|---|---|
| Shell + jobs | `bash`, `job_list`, `job_output`, `job_kill` |
| Files | `read`, `write`, `edit`, `read_image` |
| Discovery | `glob`, `grep` |
| Session | `working_directory`, `todo_write`, `exit_plan_mode`, `ask_user_question` |
| Knowledge | `skill`, `web_search`, `web_fetch` |
| Delegation | `spawn_agent`, `send_message`, `followup_task`, `wait_agent`, `list_agents`, `interrupt_agent`, `close_agent` |

Removed from Developer: `read_file`, `edit_file`, `run_command`,
`search_code`, `ask_user`, `update_plan`, `call_subagent`, `run_code`,
`agent`, `memory`.

## Intentional deviations

1. Identity says `Stark`, not `DeepSeek Harness`.
2. Windows uses Git Bash (`bash -c`), not `pwsh`.
3. Codex v2 replaces DeepSeek's `subagent`/`subagent_fork`.
4. DeepSeek's optional goal/schedule/workflow/present/plugin-manager tools are
   omitted.
5. Memory, worktree isolation, and merge/discard are removed from Developer.
   Developer subagents share the parent workspace and inherit write tools.

## Behavior

- `read` is 1-based, line-numbered, and marks the file observed.
- `write`/`edit` require a prior `read` for an existing file.
- `edit` supports literal unique replacement and `replace_all`.
- `glob` returns up to 100 files in mtime order; `grep` returns up to 250
  matches. Larger results are written to `.deep_agents/scratch/`.
- `bash` runs Git Bash, tails long output, reports `[exit code: N]`, and
  returns a job id for background work.
- `working_directory` changes the session cwd for v2 file/shell tools.
- `todo_write` writes the existing plan store and renders in the plan dock.
- `exit_plan_mode` asks for approval and sets the chat to build mode on
  approval.
- `ask_user_question` accepts DeepSeek option objects (`label`, `description`)
  and `multi_select`.
- `skill` reads `.agents/skills/<name>/SKILL.md` with a path guard and 8 KB cap.
- `web_search` calls DeepSeek `/v1/tools/search`; `web_fetch` uses a local HTTP
  fetch with an SSRF guard.
  - `STARK_SEARCH_API_KEY` (or `DEEPSEEK_API_KEY`) authenticates search.
  - `STARK_SEARCH_BASE_URL` overrides the search base; default
    `https://api.deepseek.com`.
  - Fetch is local HTTP; the DeepSeek Fetch API is an optional future fallback.
- Codex delegation uses task paths (`/root/<root-id>/<task_name>` in Stark so
  multiple root chats can share one agent database), shares the parent
  workspace, defaults `fork_turns=all`, and limits depth to 1 and open children
  to 6.

## Tests

- `stark-tools::v2_surface_tests` freezes the 24-name Developer manifest and
  verifies plan/build catalogs match.
- `ds_fs`, `ds_shell`, `ds_session`, `skills`, and `ds_web` unit tests cover the
  core behavior.
- `codex_agents` tests cover spawn/list/message/follow-up/close and the depth
  limit.
- `deepseek_prompt` tests cover identity, persona, skills, and plan text.
