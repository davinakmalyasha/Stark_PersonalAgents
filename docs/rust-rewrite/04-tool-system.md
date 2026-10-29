# 04 — Tool System

45 tools in the frozen contract (`backend-rs/specs/tool-specs.json`,
enforced by `stark-contract` tests). The model sees these strings; changing
them changes behavior.

Dropped and no longer exposed: `ast_search`, `github_apply_gate`
(never implemented). Also unexposed: `browser_navigate/extract/screenshot/close`
(legacy variants of the unified `browser_act`), `update_feature`,
`update_project_meta`, `create_project`, `propose_plan`.

---

## 1. Source of truth

| Artifact | Source |
|---|---|
| Tool definitions | `backend-rs/specs/tool-specs.json` (frozen) |
| Scope ACL | `scope_map` in the same JSON |
| Runtime | `stark-tools` (`dispatch`, per-tool modules) |
| Policy | `stark-policy` (read-only classifier) |
| Approvals | in-app question broker (`ask_user`) |
| Snapshots | `stark-tools::checkpoint` (per-workspace `.deep_agents/snapshots`) |
| MCP | `stark-tools::mcp` |

**Rule:** descriptions and parameter schemas are frozen. A wording change is
a product change: update the JSON, the `stark-contract` tests, and the
`docs/tools/*.md` page together.

---

## 2. Registry and scope resolution

- A tool is defined **once**; scope assignment is data (`scope_map`).
- `tool_specs_for_scope_with_access(mode, scope, access)` filters by scope,
  plan mode (`PLAN_WRITE_TOOLS`), and read-only access.
- Read-only agents lose write tools from their spec list — but the backend
  `dispatch` denial is the real boundary, not the hidden spec.
- Unknown tool names are rejected.

---

## 3. Dispatch pipeline

Every tool call goes through `stark_tools::dispatch`:

```
plan_mode_denial
  → read_only_denial (access == "read")
  → mcp__* → MCP bridge (blocked when read-only)
  → run_code → QuickJS bridge (blocked when read-only)
  → per-tool handler
```

Result: `ToolOutcome { content, is_error, exit_code?, plan?, children? }`.
Denial is a `[BLOCKED] ...` tool result, not a crash.

---

## 4. Read-only enforcement

`FileAccess::Read` agents (Orchestrator, House, Explore, level-2 subagents):

- Blocked: `edit_file`, `run_code`, `call_subagent`, document/project writes,
  device/scene writes, `schedule`, `canvas_mode`, `give_note`, MCP tools.
- `run_command`: allowed only if `stark_policy::check_read_only` passes —
  no redirection, no substitution, no `sed -i`, no `find -delete`, no package
  installs, git read subcommands only.
- `browser_act`: only `navigate`, `extract`, `screenshot`, `close`.
- `memory`: reads allowed; `remember`/`forget`/`pin` blocked.
- `agent`: `spawn`/`send`/`list`/`status`/`report` allowed (orchestration is
  not a write); `merge`/`discard` blocked (they change the tree).

---

## 5. Approvals and `ask_user`

- In-app question broker; no Telegram, no external approval channel.
- `/api/ask/pending` lists batches; `/api/ask/answer` resolves them.
- Denial is a `[BLOCKED]` tool result.
- DFD `create_client` asks in-app when `approved_by` is absent
  (marker `user:<chat_id>`).

---

## 6. Output handling

- Large outputs are truncated with a `[PARTIAL ...]` note and a resume hint
  (`read_file(offset=…)`, narrower `search_code`).
- Secrets are redacted before stage-1 memory capture.
- Line endings tolerated (CRLF/LF) in patches.

---

## 7. Snapshots and rewind

`stark-tools::checkpoint`:

```
<workspace>/.deep_agents/snapshots/<chat_id>/<run_id>/
```

- Snapshot bytes before a write; best-effort (a failed snapshot never blocks).
- Restore by run or message (`/api/chats/{id}/runs/{run_id}/restore`,
  `/rewind`).

---

## 8. Tool modules

| Area | Module | Tools |
|---|---|---|
| Files | `read`, `edit`, `search`, `changes` | `read_file`, `edit_file`, `search_code`, `files_changed` |
| Shell | `command` | `run_command` (foreground, background sessions, process actions) |
| Plans | `update_plan`, `ask` | `update_plan`, `ask_user` |
| Agents | `agents_tools` (via `stark-core`) | `agent` (spawn/list/status/send/report/cancel/diff/merge/discard) |
| Delegation | `stark-core::delegation` | `delegate`, `delegation_status`, `job_message`, `job_cancel` |
| Subagents | `stark-core` run path | `call_subagent` (run/spawn/status/message/stop/list) |
| Code mode | `codemode` | `run_code` (QuickJS, developer-only) |
| Browser | `browser` | `browser_act` (via `agent-browser` subprocess) |
| Screen | `screen` | `look_at_screen`, `set_screen_share` |
| Docs | `docs` | `read_pdf`, `create_docx`, `create_xlsx` |
| House | `house` | `house_get_state/device/history`, `house_set_device`, `house_run_scene` |
| DFD | `dfd` | `dfd_bridge`, `create_client` |
| Groups | `stark-core::groups` | `create_group`, `group_board/item_detail/events/update` |
| Schedules | `stark-core::schedules` | `schedule` |
| Notes | `stark-core::notes` | `give_note` |
| Canvas | `stark-core::canvas` | `canvas_mode` |
| Memory | `memory` | `memory` (index/summary/list/read/search/remember/forget/pin/status) |

---

## 9. Acceptance for the tool layer

- `stark-contract` frozen-contract test passes (counts, hidden set, scopes).
- Scoped agents see exactly their ACL (scope tests in `stark-tools`).
- Read-only dispatch tests pass (edits, mutating shell, browser writes,
  memory writes blocked; reads allowed).
- `run_command` background lifecycle works (start, logs, wait, send, kill).
- Approval flow round-trips through the UI endpoints.
- Snapshots restore exact bytes.
