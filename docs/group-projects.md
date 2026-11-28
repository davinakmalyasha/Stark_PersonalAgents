# Group projects

Group projects are durable, multi-agent work boards. A project has its own
metadata folder under `<harness>/.deep_agents/groups/<id>`, and a separate
**work folder** where agents read and write real files. The work folder is the
folder chosen in the new-project dialog; it is never the metadata folder.

## Lifecycle

```
draft -> planning -> plan_ready -> scheduled -> running -> paused -> blocked -> done
```

- **draft** — no plan yet; add milestones/items manually.
- **planning** — the planner agent is drafting the plan.
- **plan_ready** — review the board, edit it, then approve and start.
- **scheduled** — a start time is in the future.
- **running** — the runner claims work items.
- **paused** — no new work starts; a current item may finish.
- **blocked** — an item exhausted its retries, planning failed, or the work
  folder is missing.
- **done** — the current cycle is complete.

`Stop` cancels active runs and releases claimed items back to `todo`. `Close`
marks the current cycle done and disables repeat. `Move to Trash` moves only the
metadata folder; the work folder is untouched.

## Team slots

Each selected agent has an amount (1–8). Every unit is a member row and one
parallel slot. A four-Developer team is four rows:

- Developer 1
- Developer 2
- Developer 3
- Developer 4

The planner assigns items to slots by display name. Unassigned items go to the
least-busy slot with capacity. A slot can run at most one item at a time. The
project `max_parallel` cap is enforced in addition to per-slot capacity.

## Planning

New UI projects default to **plan first, review before work starts**. The
planner (Orchestrator by default) receives the goal, work folder, and team, and
returns a strict JSON plan. The plan is validated and stored as `plan_ready`.
Nothing executes until `POST /api/groups/{id}/approve-plan` or a manual start.

A project created with explicit milestones (for example through the
`create_group` tool) keeps that plan and can start immediately.

If planning fails, the project becomes `blocked` with `last_error`; retry with
`POST /api/groups/{id}/plan` or edit the board manually.

## Repeat cycles

A repeat does **not** reset the existing board. At the next interval:

- the current cycle stays complete with all its done milestones/results;
- a fresh copy of the plan is created under the next cycle number;
- previous cycles remain visible under **Past cycles**;
- if the previous cycle is still running, that occurrence is skipped.

The schedule anchor is preserved so a daily 09:00 project keeps starting at
09:00 rather than drifting to the time the previous run finished.

## API

```text
GET    /api/groups
POST   /api/groups
GET    /api/groups/{id}
PATCH  /api/groups/{id}
DELETE /api/groups/{id}                 move to Trash

GET    /api/groups/trash
POST   /api/groups/trash/{folder}/restore
DELETE /api/groups/trash/{folder}       permanent delete

POST   /api/groups/{id}/start
POST   /api/groups/{id}/pause
POST   /api/groups/{id}/stop
POST   /api/groups/{id}/close
POST   /api/groups/{id}/restart
POST   /api/groups/{id}/plan
POST   /api/groups/{id}/approve-plan
POST   /api/groups/{id}/messages
GET    /api/groups/{id}/runs

POST   /api/groups/{id}/update          {op, params}
POST   /api/groups/{id}/items/{item}/approval
```

`POST /api/groups` accepts:

```json
{
  "title": "Polish the game",
  "goal": "Ship the next playable build",
  "workspace_id": "ws-...",
  "workspace_path": "D:/games/polish",
  "members": [
    {"agent_key": "developer", "amount": 2},
    {"agent_key": "ui", "amount": 1}
  ],
  "start_mode": "now",
  "start_at": null,
  "recurrence": "daily",
  "repeat_active": true,
  "plan_review": true,
  "auto_plan": true,
  "planner_agent": "orchestrator",
  "max_parallel": 2
}
```

Omit `milestones` to let the planner draft them. Pass explicit milestones with
the same shape used by the `create_group` tool when the orchestration plan is
already known.

### `update` operations

Lifecycle: `start`, `pause`, `stop`, `close`, `restart_cycle`.

Board: `add_milestone`, `update_milestone`, `delete_milestone`, `add_item`,
`update_item`, `delete_item`, `retry_item`, `cancel_item`, `invalidate_item`,
`reopen_item`, `complete_item`.

Team: `add_member` (`agent_key`, `amount`, `display`), `remove_member`
(`member_id`).

Settings: `set_settings`, `set_workspace`, `set_schedule`, `set_planning`,
`set_plan`, `approve_plan`, `message`.

Milestone statuses are `todo`, `active`, `done`, `blocked`, `cancelled`.
Item statuses are `todo`, `doing`, `awaiting_approval`, `done`, `failed`,
`cancelled`, `invalidated`.

## Usage and logs

Every planner/item run writes a `group_runs` row with provider-reported input
tokens, output tokens, and cost. The detail response aggregates usage per
member and per project. Reports, assignments, and approvals are stored in
`group_messages` and mirrored to the durable event log. The activity feed is
filterable and includes run history; worker chats can be opened as normal
chats from their `chat_id`.

## Work-folder safety

- The work folder must exist and is canonicalized before a run.
- Agents never receive the metadata folder as their workspace.
- Trash only moves `.deep_agents/groups/<id>`; the work folder is not deleted.
- Trash/restore stays under the groups root and rejects path traversal.
- A project with a missing work folder becomes blocked and offers **Relink
  folder** in settings.
- Old projects without a stored work folder are backfilled from their origin
  workspace when possible; otherwise they require a relink.

## Migration

Each project DB is upgraded in place with `PRAGMA user_version = 2`. Existing
projects gain the new project/milestone/item columns, `members`, `group_runs`,
and `group_messages`. Existing item owners are converted into member rows; a
project with no owner gets one Developer slot. Existing done milestones are
kept. Migration is idempotent and covered by store tests.
