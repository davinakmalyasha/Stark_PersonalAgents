# 06 — Orchestration

Routing, delegation, specialists, subagents, and plan mode.

---

## 1. Entry routing

`resolve_agent(workspace_kind, workspace_path, harness_root)`:

- `house` → House; `personal` → General; `dfd` → DFD; `design` → UI
- project path → Developer
- harness root → Orchestrator

On router error: fall back to Orchestrator, never drop the request.

---

## 2. Orchestrator

The Orchestrator never codes. It routes work via two tools:

- `delegate` — parallel workspace jobs (Developer, General, House, UI, DFD)
- `agent` — direct durable-agent control (spawn/list/status/send/diff/…)
- Group/canvas/schedule tools for projects, watches, timed wakes

Park/resume: delegation results arrive as `[AGENT RESULT]` mailbox messages;
the supervisor wakes the parent via follow-up. A stall surfaces to the user,
never a silent hang.

---

## 3. Delegation

`stark-core::delegation`:

- `delegate` accepts items with `agent`, `task`, `outcome`, `acceptance`,
  `title`, plus optional `workspace` / `project` / `isolate`.
- Each item creates or reuses a durable **workspace** agent and queues a
  mailbox task; the supervisor runs it.
- `workspace` resolves by registry id/name/path (or virtual id);
  `project` fuzzy-matches a registered workspace name.
- Developer delegation requires a project target — it never silently uses
  the harness root. General → `_general`, House → `_house`, DFD → `_dfd`.
- A failure is reported per item, not as a global run failure.
- Child runs inherit the target workspace and tool scopes, never the parent
  conversation.
- `delegation_status` reads the agent store (with `wait_seconds` polling);
  `job_message` sends into the mailbox; `job_cancel` cancels live + marks
  the record.

---

## 4. Specialists and subagents

Specs live in `stark-core::agents`: Developer, General, House, DFD, UI,
DevOps, Explore, Orchestrator.

Legacy v1:

- `call_subagent` actions: `run` (synchronous, persisted), `spawn`
  (supervisor-driven), `status`, `message`, `stop`, `list`.
- Subagents see only the task + workspace memory index. No parent history.
- Their report is a tool result; the spawner decides next steps.
- Level-2 subagents are read-only unless the parent passes `write_access`
  (nested worktree).

Developer v2 (`stark-core::codex_agents`, Pi-simplified):

- `spawn_agent`, `message_agent` (queue or wake), `wait_agent`,
  `list_agents`, `interrupt_agent`, `close_agent`.
- Task paths are `/root/<root-id>/<task_name>`; `fork_turns=all|none`.
- Children share the parent workspace and inherit write tools.
- Max depth 1, max 6 open children; child approvals fail non-interactively.

---

## 5. Plan mode

- Legacy scopes hide write/execute tools from the spec list and block them
  in `dispatch` and in code mode.
- Developer keeps the same tool catalog in plan/build for request-cache
  stability; `plan_mode_denial` blocks writes (`job kill`, mutating `bash`,
  `write`/`edit`, delegation) while reads stay available.
- `todo_write` replaces the plan dock for Developer; `ask_user_question` with
  plan-review intent presents the plan for approval (server flips plan to
  build on approve; the composer Approve button does the same via API).
- The plan block is appended to the prompt when a plan exists.
- Plans persist per chat/scope and appear in `AppState.plan`.

---

## 6. Schedules, wakes, canvas

- `schedule`: one-off/recurring timed wake for a chat.
- `give_note`: sticky notes for a chat/project.
- `canvas_mode`: watch agent chats; on completion/failure enqueue a wake.
- One scheduler task + one wake-dispatcher task, both DB-backed; no
  per-schedule threads.

---

## 7. Acceptance

- A two-job delegation runs, reports per item, and can be steered/cancelled.
- Developer delegation without a project target is refused, not misrouted.
- Plan mode hides and blocks write tools; the plan survives restart.
- Schedules/wakes fire once, persist, and do not duplicate after restart.
