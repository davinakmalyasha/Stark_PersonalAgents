# Tool contract v10 (Codex-aligned agent surfaces)

`tool-specs.json` is the single model-facing tool contract for the Rust server.
It now contains two surfaces:

- **Workspace tools** — the v2 surface used by Developer, General, House,
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

## Developer manifest (16)

```
bash, job,
read, apply_patch, edit, glob, grep,
update_plan, ask_user_question,
skill,
spawn_agent, message_agent, wait_agent,
interrupt_agent, close_agent,
execute
```

The coding agent (`developer`) defaults to OpenCode's **base Code
Mode**. Core tools (`bash`, `read`, `apply_patch`, `edit`, `glob`, `grep`)
stay direct; the long-tail tools are catalog-only and run through `execute`
alongside the direct-only tools (`job`, `update_plan`, `skill`,
`ask_user_question`, and delegation). Base Code Mode exposes the `fetch`
global only.

`full_codemode` is opt-in and defaults off, matching OpenCode release builds.
Enabling it pins the file/shell tools into the catalog, hides `glob`, `grep`,
and `webfetch`, adds the `path` and `Bun.file`/`Bun.write`/`Bun.Glob` globals,
and adds the script `timeout`/`background` inputs. `bash` loses
`run_in_background` and defaults to a 10-minute foreground wait. A direct call
to a catalog tool is repaired into an implicit one-call program and returns a
`<tool-repair>` note. `code_mode: "only"` selects the execute-only full
surface; `"hybrid"` is the base surface; `"off"` removes `execute` and keeps
every tool direct. `full_codemode: true` promotes `hybrid` to full.

`tool_search` is General-only and Developer delegates MCP work. The Code Mode
partition follows OpenCode v2's `execute` contract; the catalog is rendered
into the system prompt from the frozen schemas with OpenCode's TypeScript
signatures. Programs run in the Bun sidecar (`../codemode-worker`, Bun +
`@opencode/codemode` on psychoscript) and call back into Rust dispatch; MCP
tools use dotted paths there (`github.get_issue`). Code Mode instructions are persisted per chat: the initial catalog and later
`CodeModeInstructions.update`/`removed` blocks are appended in OpenCode's
order, including the complete-to-partial `search` note. Running scripts also
publish throttled `tool_progress` rows and file diffs (100 ms trailing cap).

`execute` is declared
freeform (`{ type: "text", field: "code" }`); a model that declares
`"freeform": ["text", "lark"]` receives an OpenAI custom tool (with the exact
`// @options:` grammar in full mode), while providers that advertise no
freeform format receive the same tool as a JSON function, exactly as OpenCode
does.

## Editing

`tool-specs.json` is validated by `stark-contract` and consumed by
`stark-tools::tool_specs_for_scope_with_access`. Do not add a tool to
`scope_map.developer` without also adding one `ToolMeta` entry to the
unified inventory in `stark-tools/src/inventory.rs` and implementing a
dispatch arm.

The Developer surface is documented by the contract itself and the
`stark-tools` inventory. The old per-tool pages and design docs were deleted;
the code is the source of truth.

## Custom agents and the frozen contract

`tool-specs.json` remains the frozen wire contract for the eight built-in
scopes. Custom agents do **not** add entries to `scope_map`; they use the
runtime registry's closed ACL (`stark-core/src/agent_registry.rs`). This keeps
contract tests stable while making agent add/remove a data operation.

Rules for custom definitions:

- `tools` is the complete allowlist; omitted tools means an invalid definition.
- Unknown tools/skills/MCP servers are rejected on save.
- Read-only agents cannot receive mutating tools.
- Custom Code Mode is opt-in `hybrid` and requires `execute` in the closed
  list. Custom `only`/`full_codemode` are rejected and remain built-in
  Developer features.
- `routing` is custom-only, unique-match, and ignored when an explicit agent
  or chat agent key exists. Paths are normalized absolute globs; `repos`
  match a normalized `host/owner/repo` git remote. When both are present,
  both must match. Relative or empty rules fail save validation and are
  dropped with a warning on load.
- Explicit workspace → agent bindings are user-written only; routing never
  persists one.
- `skill_hashes` optionally pins a skill to the SHA-256 of its `SKILL.md`.
- Symlinked skill directories and `SKILL.md` files are ignored.
- Agent definitions have a stable `definition_hash`; chats remember the hash
  used by their last run.
- Audit events are durable, model-invisible, and contain no prompt or file
  contents.
- Built-ins can narrow their compiled caps but never widen them.

Schema: `agent-def.schema.json`. API: `docs/agent-builder-api.md`.
