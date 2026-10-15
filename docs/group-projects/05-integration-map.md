# Group Projects — Integration Map

**Version**: 1.0
**Date**: 2026-10-01
**Author**: Stark design session
**Status**: Draft
**Depends on**: `01`, `02`, `03`, `04`

What changes in the existing codebase. Every line reference is verified against the tree at commit time.

---

## 1. Summary

| Verdict | Count | Substance |
|---|---|---|
| **Extends** | 7 | Existing mechanism gains a project-scoped variant. Backwards compatible. |
| **Changes** | 8 | Signature or behaviour change. Contained; needs a migration note. |
| **Dies** | 1 | The auto-created Dev child, subsumed by the default-member concept. |
| **New** | ~8 modules | Group-project-specific. Additive. |

**No agent tier is added. No loop is hand-rolled. `tools/` does not learn what a group project is.**

---

## 2. New Modules

```
agents/jobs/                    ← PHASE 0 — harness-level job store for `delegate`
├── __init__.py                 #   (spec: 07-phase-0-implementation.md)
└── store.py                    #   jobs.db: schema, DAO, boot reconcile

agents/group/                   ← PHASE 1+ — group projects
├── __init__.py
├── store.py          # events (TRUTH) + projections; DAO; rebuild(); checkpoints
├── supervisor.py     # the 11-phase tick (02 §3). One thread. Sole writer.
├── project.py        # folder scaffolding, PROJECT.md parse/write, manifest model,
│                     #   AGENTS.md, deps/ handling
├── materialize.py    # WorkspaceSpec -> agents/<name>.json with provenance;
│                     #   copy a skill; write tools/bindings.json; filter mcp/servers.json
├── planner.py        # plan_execution: the one-shot DAG + environment authoring call
├── dispatch.py       # ready -> lease -> JobRunner handoff; harvest + verify
└── recovery.py       # boot-time rebuild from events (02 §5.1)

server/services/
└── group_scheduler.py  # boot wiring, alongside start_task_scheduler()

server/routers/
└── group_projects.py   # CRUD + board + events + ops REST surface

tools/AddOnTool/
└── groupTools.py       # plan_execution, create_group_project, the op surface,
│                       #   and the member toolset

tools/_group_scope.py   # tools/bindings.json ∩ _SCOPE_MAP, + envelope on WorkspaceDeps
```

`JobStore` is one interface with two backings — harness-level (`agents/jobs/`) and per-project (`agents/group/store.py`). Phase 0 builds the first; Phase 2 adds the second (§3.2). If they converge later, the harness module is the natural place for the shared interface.

### 2.1 The schema, in full

`store.py` owns exactly this, and the split is not negotiable (`01` §7.2):

```sql
-- TRUTH: append-only, never updated, never deleted
CREATE TABLE events (
  seq          INTEGER PRIMARY KEY AUTOINCREMENT,
  project_id   TEXT NOT NULL,
  ts           REAL NOT NULL,
  kind         TEXT NOT NULL,
  actor        TEXT NOT NULL,
  milestone_id TEXT,
  item_id      TEXT,
  data         TEXT
);
CREATE INDEX ix_events_project ON events(project_id, seq);

-- PROJECTIONS: incremental on write, rebuilt from events on boot
CREATE TABLE projects     (id PRIMARY KEY, owner, status, budget_usd, spent_usd,
                           deadline, completion_json, milestone_count, …);
CREATE TABLE milestones   (id PRIMARY KEY, project_id, title, status, seq_no,
                           acceptance_json, …);
CREATE TABLE items        (id PRIMARY KEY, project_id, milestone_id, title, status,
                           attempts, max_attempts, owner_member, workdir,
                           lease_owner, lease_expires, acceptance_json, tags_json,
                           cost_tokens, cost_usd, invalidated_reason, …);
CREATE TABLE approvals    (id PRIMARY KEY, project_id, item_id, tier, payload_json,
                           idem_key, idem_state, state, created_at, …);
CREATE TABLE projection_checkpoint (project_id PRIMARY KEY, last_seq INTEGER);
```

**Raw `sqlite3`, no ORM** — matching `server/store/chat_store.py:45`, including `journal_mode=WAL`, `synchronous=NORMAL`, and `check_same_thread=False`.

Two design consequences worth stating:

- **`seq` is `AUTOINCREMENT`, not a plain `INTEGER PRIMARY KEY`.** With a plain rowid, a rolled-back transaction can reuse a number, and a reused `seq` silently corrupts replay. `AUTOINCREMENT` costs a little and makes the guarantee real.
- **`rebuild()` is a separate code path from the tick's incremental writes.** That is deliberate duplication: `rebuild()` is written once, tested against the incremental path by DM-16, and is the only thing that makes "events are the only truth" verifiable rather than aspirational.

`agents/group/` follows the existing house style: module docstring stating what is here *and what is deliberately not here* (`agents/delegation.py:1-20`, `tools/skills.py:1-22`).

---

## 3. `agents/delegation.py` — Split, Do Not Fork

**The most important integration decision.** Delegation is the execution primitive; a group project is the container above it. Split delegation into two concerns and make the durable half the truth.

### 3.1 Today

```python
_DELEGATIONS: dict[str, dict] = {}          # :44  process-local — lost on restart
_SLOT_COND = threading.Condition(_LOCK)     # :50  flood guard
_ORCH_MAX_PARALLEL = int(env("ORCH_MAX_PARALLEL", "6"))   # :56
_ORCH_JOB_TIMEOUT   = int(env("ORCH_JOB_TIMEOUT", "3600")) # :64
```

| Symbol | Line | Verdict | Change |
|---|---|---|---|
| `_DELEGATIONS` | `:44` | **Change** | Becomes a read-through cache in front of `JobStore`. Same dict interface, so every call site keeps working. |
| `_SLOT_COND` / `_acquire_slot` / `_release_slot` | `:50`, `:71`, `:79` | **Extend** | Shared with the supervisor (`02` §7). Add an optional `reserved` floor for project work. |
| `ORCH_MAX_PARALLEL` | `:56` | **Extend** | A second knob, `GP_MAX_PARALLEL`, defaults to half. Project work gets a floor so a chatty orchestrator cannot starve a multi-day project. |
| `start_delegations` | `:697` | **Extend** | Optional `project_id` / `item_id` params. Both `None` = today's behaviour, unchanged. |
| `_run_item` | `:601` | **Change** | Result returns into the job row, not the process-global report. |
| `_write_job_artifact` | `:116` | **Keep** | Job artifacts are the harvest source on restart (`02` §5). |
| `_ledger_path` | `:1192` | **Keep** | Still useful as a *history*. Not the board (`01` §3). |
| `_append_job_ledger` | `:1197` | **Keep** | Feeds the skill-distillation source (`00` §4). |
| `_truncate_ledger_locked` | `:1231` | **Keep** | 500-line cap is right for a history. |
| `park_wait` | `:1150` | **Change** | Stays for within-run parking. Project wake comes from board events instead. |
| `_sync_orchestrator_job_plan` | `:251` | **Keep** | Already the right instinct — the harness derives the view from real jobs. Group projects generalise it. |

### 3.2 `JobStore`

```python
# agents/group/store.py
class JobStore:
    """Durable job state. Truth for in-flight work; _DELEGATIONS is a cache."""
    def upsert(self, job: dict) -> None: ...
    def get(self, job_id: str) -> dict | None: ...
    def claim_running(self, project_id: str, limit: int) -> list[dict]: ...
```

**One interface, two backings.** This was ambiguous until Phase 0 forced the question (`07` §6):

| Backing | Where | Serves | Introduced |
|---|---|---|---|
| **Harness-level** | `.deep_agents/jobs.db` | `delegate` — chat-scoped, machine-wide | **Phase 0** |
| **Project-level** | `<project>/.gp/state.db` | group-project item execution | Phase 2 |

The harness backing exists first because delegation is harness-scoped, not project-scoped — proven by the existing ledger, whose path resolves to `<harness>/.agent-memory/orchestrator_jobs.jsonl` (`delegation.py:1192`), one file for every chat on the machine.

In a project's `state.db` the `jobs` table sits alongside the event log and **every job-state change is an event** — a job is a projection like any other, and `job.finished` is what lets phase 6 harvest results after a restart. The harness-level table has no event log behind it in Phase 0; that arrives with Phase 1's schema.

Both backings share the schema, with `project_id` / `item_id` present and `NULL` for harness-level jobs — so the Phase 0 table needs no `ALTER TABLE` to become Phase 1's.

Additive migration follows the house pattern already used in `chat_store.py:88-94`:

```sql
CREATE TABLE IF NOT EXISTS jobs (
  id TEXT PRIMARY KEY, parent_chat_id TEXT,
  project_id TEXT, item_id TEXT,                 -- NULL for `delegate` jobs
  agent TEXT NOT NULL, chat_id TEXT, title TEXT, status TEXT NOT NULL,
  attempts INTEGER DEFAULT 0,
  lease_owner TEXT, lease_expires REAL,          -- Phase 1; NULL in Phase 0
  workdir TEXT, usage_json TEXT, result TEXT, error TEXT, artifact TEXT,
  created_at REAL NOT NULL, updated_at REAL NOT NULL, finished_at REAL
);
CREATE INDEX IF NOT EXISTS ix_jobs_parent ON jobs(parent_chat_id, status);
CREATE INDEX IF NOT EXISTS ix_jobs_project ON jobs(project_id, status);
```

Full Phase 0 schema, cache semantics, and gate: `07-phase-0-implementation.md` §3–§4, §8.

### 3.3 Two lifetimes, one store

| | `delegate` | Group project |
|---|---|---|
| `project_id` | `NULL` | set |
| `item_id` | `NULL` | set |
| Bound | `ORCH_JOB_TIMEOUT` (`:64`) | budget / deadline / predicate |
| Caller | orchestrator, in-conversation | supervisor, on tick |

A job with both `NULL` behaves exactly as it does today. That is the backwards-compatibility guarantee (`02` §7).

---

## 4. `agents/project_router.py` — Extract, Then Split

`create_project` (`:438`) currently does five things: make a folder, `git init`, write a language gitignore (`:273` `_GITIGNORE_PYTHON` + Node/generic variants), write a README (`:387` `_README_TEMPLATE`, via `_scaffold_gitignore_and_readme` `:416`), and register a workspace + chat + **durable Dev child**.

### 4.1 What survives

| Piece | Verdict | Why |
|---|---|---|
| Folder creation + `git init` | **Keep** | A project that produces code wants a repo. |
| Language gitignore + README (`:273`, `:387`, `:416`) | **Extract** → `_scaffold_folder(path, name, language, concept)` | Genuinely reusable. Do not throw away working scaffolding. |
| Workspace + chat registration | **Keep** | Gives the project a registry entry and a durable chat. |
| **Durable Dev child** (`find_project_dev` `:573`) | **Dies** | In a group project, *members* fill this role. Becomes "the default member" (`01` §5.2), conditional on the manifest declaring no explicit members. |

### 4.2 Two callers, one scaffolder

```
create_project(name, concept, ...)            # unchanged semantics
    └── _scaffold_folder()                    # extracted

create_group_project(manifest, members, items)
    ├── _scaffold_folder(language=from manifest or sniffed)
    ├── PROJECT.md            ← authored
    ├── .gp/state.db          ← new schema
    ├── context/ artifacts/
    └── members               ← 0 → orchestrator is the sole member
```

`create_project` is **not** deleted — the user asked to repurpose it, and the repo-scaffolding half is worth keeping. What dies is the *hardcoded* Dev child, not the function. If the intent later is to remove the dev-project path entirely, that is a separate decision for a separate commit.

### 4.3 Path root

```python
# today
PROJECTS_ROOT = r"D:\MyProject"                        # :28

# new — group projects live in their own root, not the scanned tree
STARK_PROJECTS_ROOT = os.environ.get("STARK_PROJECTS_ROOT", r"D:\StarkProjects")
```

Rationale (`01` §2): `D:\MyProject` is scanned by `project_router.py` (`:28`, `:466`), so a group project folder there would be rediscovered as a workspace. `.deep_agents/` is the directory that gets wiped on reset, and these hold weeks of state. The env var exists so a cloud deployment is a config change (`06` §2), not a code change — the box sets `STARK_PROJECTS_ROOT=/data/projects` on a persistent volume.

---

## 5. `sync/state_sync.py` — The Concurrency Fix

> **Corrected 2026-10-01 after reading the code.** This section previously called itself *"the highest-risk item in the whole integration."* That was about the **consequence** — silent cross-talk, misattributed to the model — not the **effort**. The mechanism to fix it already exists and is proven in production. The fix is ~6 lines. See `07-phase-0-implementation.md` §5.

`shared_state` (`:13`) is a **process global**. `task_scheduler.run_task_background` (`:147`) reads the report out of it at `:163`:

```python
report = safe_get_state().get("outputs", {}).get("agent_report", "")   # :163
```

**It already races today.** `start_task_scheduler` (`:181`) spawns a thread per due task (`:204`), so two tasks due in the same 15-second tick both read the root state and both post whichever report was written last. The failure is silent and the output looks plausible, which is exactly why it gets blamed on the model.

### 5.1 The fix already exists in the codebase

`delegation.py:622-627` sets a scoped state and `:680-681` resets it. `active_state` is a `contextvars.ContextVar` (`state_sync.py:9`) and `_target_state()` only falls through to `shared_state` when it is unset (`:39`). Because each task already runs in its own thread, the isolation is structural — **no lock needed**.

`task_scheduler.py` simply never sets it. The fix is to copy the pattern.

| Symbol | Line | Verdict | Change |
|---|---|---|---|
| `shared_state` | `:13` | **Change** | Stays the cross-process coordination surface (`fixed-concept.md` §5) — live log, session bus. **Stops** being the source of truth for a run's result. |
| `active_state` / `active_store` | `:5`, `:9` | **Keep** | Correct as written. The scheduler must *use* them, not change them. |
| `_target_state()` | `:37` | **Keep** | The fall-through to `shared_state` is right; the bug is a caller that never sets a scope. |
| `safe_get_state` | `:42` | **Keep** | Returns a correct copy (`:46-66`). Scoped callers get their own state. |
| `task_scheduler.run_task_background` | `task_scheduler.py:147` | **Change** | Set `active_state` at entry, `reset` in `finally`. **Phase 0, deliverable 3.** |
| `WorkspaceDeps` | `llm/deps.py:19` | **Extend** | Carries `project_id`, `item_id`, and the autonomy envelope. Already the per-run isolation vehicle. |

`fixed-concept.md` §5 already declares the correct rule — *"shared_state remains only as a cross-process coordination surface, never the source of truth for a run's context."* This change enforces a rule the doc already states.

### 5.2 Two different problems, two different fixes

Worth separating, because they were conflated:

| Problem | Fix | Phase |
|---|---|---|
| Scheduled tasks read each other's reports | Set `active_state` in `run_task_background` — ~6 lines, pattern exists | **Phase 0** |
| Group-project members report each other's results | Results come from the job row, not from scoped state at all | **Phase 1** |

Phase 0 removes the live bug. Phase 1's `JobStore` makes scoped-state-for-results unnecessary in the first place, because a member's result is a row rather than a global read.

---

## 6. MCP & Environment Composition

| Symbol | Line | Verdict | Change |
|---|---|---|---|
| `McpManager` | `mcp_bridge.py:419` | **Keep** | Owns transport + installed servers. Unchanged. |
| `tool_defs_for_scope(scope)` | `:503` | **Extend** | Called with the project scope derived from the folder's `mcp/servers.json`. |
| `call(fqname, args, scope, …)` | `:539` | **Extend** | Gains the envelope for approval classification. |
| `.deep_agents/mcp_servers.json` | — | **Keep as catalogue** | The machine-level record of *installed* servers. Untouched. |
| `scopes` array | — | **Extend** | A project mints `gp:<project_id>`; attachment is project-owned. |

**Attachment is now a file, not a `state.db` row.** Each project carries its own `mcp/servers.json` (`01` §2) — a materialised, project-owned copy of the servers *that project* uses:

```jsonc
// <project>/mcp/servers.json   (project-owned; credentials by NAME only)
{ "servers": {
    "sentry": { "url": "…", "bearer_token_env_var": "SENTRY_ACCESS_TOKEN",
                "enabled_tools": ["list_issues"], "approval": "prompt" }
} }
```

Resolution: `mcp/servers.json` (project) ∩ the catalogue's transport config, filtered by `enabled_tools` and by `tools/bindings.json`. The project's file says *what*; the catalogue says *how to reach it*. Copying the file must never copy secret values (DM-10).

**A project only attaches what it needs.** That resolves a cloud wrinkle for free: `agents-proxy-admin` points at `http://localhost:8080` (`mcp_servers.json:4`), a service on your machine. On a cloud box it simply is not attached, because no project asked for it. Narrower-by-design beats copying your whole catalogue to the box.

**Zero consumer edits** to attach an MCP server to a project — which is exactly the "easy to wire a new MCP" first-class requirement in `fixed-concept.md` §3.5.

**v1 attaches only what is installed.** Installing a server means running a command; the orchestrator may not do that (`04` §2, default-deny). A curated install catalogue with one-time human approval is Phase 4 (`06` §5) — this reverses nothing, it just defers the decision.

---

## 7. `server/services/task_scheduler.py` — Refactor, Don't Replace

The user asked for this to be refactored rather than bypassed. Agreed — and here is the honest version of what that means.

| Symbol | Line | Verdict | Change |
|---|---|---|---|
| `start_task_scheduler` | `:181` | **Split** | Becomes two loops sharing a cadence helper. The `time.sleep(15)` at `:212` is the heartbeat the supervisor needs. |
| `notify_tasks_changed` | `:38` | **Keep** | Also fires the supervisor wake after a mutation, so a newly-ready item need not wait a full tick. |
| `run_task_background` | `:147` | **Change** | Reads the result from the job row, not `state_sync` (`:163`). |
| `next_recurring_time` | `:115` | **Keep** | Belongs to `scheduled` shape, not group projects (`03` §2.2). |
| `get_chat_tasks` / `save_chat_tasks` | `chat_store.py:495` / `:533` | **Keep** | Same relationship. |
| REST CRUD | `routers/tasks.py:32,40,59,72,93` | **Keep** | Unchanged. |

**The refactor, stated honestly:**

| | Today | After |
|---|---|---|
| Trigger | `schedule`/`recurrence` → fire a prompt | Project predicate / deadline / budget |
| Scoping | one `workspace_id` + `chat_id` | a project folder |
| Persistence | `chats.tasks_json` blob (`chat_store.py:71`) | `.gp/state.db` rows |
| Concurrency | one run, global state | N isolated members |
| Success | `state_sync.outputs.agent_report` | board transition + event |
| Termination | "once" / daily / weekly / monthly | completion predicate (`01` §10.1) |

So the shape of the feature is the refactor you asked for; the *implementation* of the task path is not extensible, for three concrete reasons:

1. **No DAG, no parents** — a project needs dependencies, a task has none.
2. **`state_sync` result read** (`task_scheduler.py:163`) — races immediately under concurrency (§5).
3. **Per-chat scoping** — a project outlives its chat.

`group_scheduler.py` sits beside it. Both loops share a cadence helper. The 15-second poll that already exists funds the supervisor's heartbeat (`02` §2).

---

## 8. Skills — Wiring the Dead Module

`tools/skills.py:15-21` is explicit that it registers no tool, injects nothing, and changes nothing. Verified: `llm/` contains **zero** references to skills.

So "the orchestrator sets up skills" does not work today. It becomes real in three changes:

| Change | Where |
|---|---|
| Skill **copy** into `<project>/skills/<name>/SKILL.md` at creation | `attach_skill` op → `agents/group/project.py` |
| Member prompt injection of attached skills (body + `allowed_tools`) | Member prompt builder (`02` §8) |
| Skill `allowed_tools` **narrows** the member's scope | `tools/_group_scope.py` |

`Skill.allowed_tools` (`:42`) becomes load-bearing: a skill can only ever **narrow** what a member may do. That is a useful property — a spreadsheet skill cannot grant `run_command`, and a project attaching an untrusted skill cannot escalate.

**Materialisation vs. reference.** `attach_skill` copies the `SKILL.md` body into the folder rather than globbing `.agents/skills/*/SKILL.md` at runtime. This is what makes the folder self-contained, and it means editing a skill inside a project does not affect the global catalogue or any other project — which is correct, because the project's copy is now the thing its members run.

**Routines = items that repeat** (Grok's skill/routine split, `00` §4). A routine is a board item with a recurrence, not a separate feature.

---

## 9. Registry & Routing

| Symbol | Line | Verdict | Change |
|---|---|---|---|
| `WorkspaceSpec` | `workspace_registry.py:31` | **Keep as template** | Materialised into `<project>/agents/*.json` at creation. Not consulted mid-run. |
| `WORKSPACE_REGISTRY` | `:111` | **Keep** | The template source and the provenance reference. Unchanged. |
| `MODE_MATRIX` | `provider_modes.py:60` | **Change** | `command` mode's orchestrator tier moves from `think_bias: low` to `high` (§9.1). |
| `AGENT_MODELS` | `:100` | **Keep, bypassed** | Members pin `model` + `thinking` in their materialised file, not here (`01` §5.2). |
| `_SCOPE_MAP` | `_schemas.py:29` | **Extend** | Becomes the **ceiling**: `tools/bindings.json` intersects it, never widens it. |
| `TOOL_DEFINITIONS` | `_schemas.py:46` | **Extend** | Group tools registered here. Descriptions are routing policy (`03` §2.1). |
| `_TOOL_DOCS` | `_schemas.py:670` | **Extend** | One-liners that must not contradict the schemas. |
| `enforce_fs_permission` | `_runtime.py:88` | **Change** | Gains the project root so "inside the folder" is computable (`04` §5). |
| `_check_and_request_approval` | `_runtime.py:512` | **Change** | Gains the envelope via `WorkspaceDeps`. The whole enforcement integration. |
| `decide_route` | `routing/router.py` | **Change** | Existing "create a project…" wording → ORCHESTRATOR (per `docs/Structure.md:29`). New "resume project X" → supervisor. |
| Orchestrator system prompt | `orchestrator_agent.py:72` | **Change** | Gains the tool-routing guidance, the `solo`/`group`/`scheduled` framing, and the two model slots. |

### 9.1 Provider profile — the one behavioural change to the LLM layer

Group projects run on the `command` mode (Command Code Provider API, OpenAI-compatible) with two model slots, **both at `thinking: high`** (`01` §5.2):

| Slot | Model | Tier | Current | Target |
|---|---|---|---|---|
| reasoning / authoring | `xiaomi/mimo-v2.6-flash` | orchestrator | `model: _COMMAND_ORCH_MODEL_DEFAULT` (`:57`), docstring says *"thinking low"* (`:19`) | **thinking `high`** |
| execution / building | `deepseek/deepseek-v4.1-flash` | workspace | `_CANONICAL_FLASH` (`:55`), docstring says *"thinking high"* (`:20`) | unchanged — already high |

Two consequences:

1. **The orchestrator tier's thinking policy changes** from low to high. This affects *every* orchestrator turn in `command` mode, not only group projects. If that is unwanted, express group-project reasoning on the workspace tier instead — but the higher tier is where the planning pass belongs, so raising it is the correct fix.
2. **`AGENT_MODELS` is bypassed for members.** A materialised agent carries its own `model` + `thinking`, so the same agent name can run different models in different projects. `AGENT_MODELS` remains authoritative for the *non-project* agents.

Everything else in `provider_modes.py` is untouched: `MODES`, the `free`/`deepseek`/`opencode` matrices, `get_tier()` (`:166`), and the env-override precedence contract all stay as they are.

### 9.2 Two sources of truth — the tension this creates

`fixed-concept.md` §3 declares the registry "the single source of truth for every agent." Materialising agents into project folders creates a second source, deliberately.

Resolution: **the registry is the template; the folder is the instance.** The registry supplies a starting definition at creation; the folder owns its copy from then on. Editing `agents/frontend.json` inside a project does not touch the registry, and editing the registry does not touch the project.

Contained by two fields (DM-09): every materialised agent records `derived_from` and `derived_at_version`, so `materialize_agent` can regenerate from the current template when a project is deliberately refreshed. Without those fields, divergence is archaeology.

**Import-cycle constraint:** `tools/_schemas.py:15` loads the registry **by file path** to break the `tools ↔ agents` cycle. `workspace_registry.py` must stay stdlib-only. The `gp:<project_id>` scope keys are therefore computed lazily at member-toolset-build time, never registered as static specs.

---

## 10. Boot Wiring

```python
# server/api_server.py — currently only under __main__ (:546)
start_task_scheduler()          # :546  existing
start_group_scheduler()         # NEW — and see the caveat below
```

**Caveat worth fixing regardless:** `start_task_scheduler()` is called only inside `if __name__ == "__main__"`. Under `uvicorn server.api_server:app` the scheduler never starts — which is exactly how the `--reload` and test paths behave. A group supervisor has the same trap, and it is worse: a supervisor that silently does not start looks identical to a project that is stalled.

**Fix (small, worth doing on its own merits):** move both to the `lifespan` handler (`api_server.py:180`), so they start under `__main__`, `uvicorn`, and reload alike.

```python
@asynccontextmanager
async def _lifespan(app):
    start_task_scheduler()
    start_group_scheduler()
    yield
    stop_group_scheduler()
```

---

## 11. Frontend

Reuses existing components rather than new views:

| Need | Reuse |
|---|---|
| Board view (todo/doing/done) | `components/Canvas/AddTaskModal.tsx` patterns; `PlanDock.tsx` |
| Live member activity | `SubagentActivityPanel`, `ExecutionTrace`, `ActivityTimeline` |
| Multi-session board | `AgentCanvas`, `chatHierarchy.ts` |
| Member list | `delegationMatch.ts`, `orchestrationTools.list_agents` |
| Approvals panel | New — but modelled on `AddTaskModal` |
| Project switcher | `SidebarProjectList`, `WorkspaceSwitcher` |

**Streaming:** the supervisor runs outside any request, so progress reaches the UI over the **realtime event bus** (`server/realtime/bus.py`), not the SSE run stream (`runs.py:189`). The existing phone-mirroring path (`runs.py:506`) already subscribes to that bus, so remote visibility comes almost free.

---

## 12. Risks

| Risk | Severity | Mitigation |
|---|---|---|
| `shared_state` cross-talk in scheduled tasks | **High consequence / low effort** | **Live bug today** — two tasks in one tick both post the last-written report (`§5.2`). ~6 lines: set `active_state` in `run_task_background`, pattern from `delegation.py:622`. **Phase 0, deliverable 3.** |
| Group members report each other's results | **High** | §5.2 — distinct from the above: member results come from the job row, not from scoped state. Phase 1. |
| Supervisor silently not started under `uvicorn` | **High** | §10 — move to `lifespan`. Emit `supervisor.heartbeat` so a dead loop is visible |
| Description-driven routing is brittle | Medium | `03` §2.3 regression test with negative cases |
| Slot starvation between `delegate` and projects | Medium | `GP_MAX_PARALLEL` floor (`02` §7) |
| `.md` state believed authoritative | Medium | `01` §3 two-state rule; never parse `.md` for state |
| Lease-expiry storm after a long restart | Medium | Grace window on boot (`02` §5.1); backoff on re-dispatch |
| Project folder grows without bound | Low | `artifacts/` retention policy; `events` cap |
| Skills' `allowed_tools` mis-scoped | Low | Enforce as an intersection, never a union |

---

## 13. Migration Order

Dependency-ordered so each step is independently testable.

| # | Step | Depends on | Gate |
|---|---|---|---|
| 1 | `JobStore` + `_DELEGATIONS` as cache — **spec: `07-phase-0-implementation.md`** | — | Existing delegation tests pass; a job survives a restart |
| 2 | **Fix `state_sync` result reads** — **spec: `07` §5** | 1 | Two concurrent tasks produce independent reports |
| 3 | Supervisor + all 11 tick phases + `rebuild()`, **no LLM** | 1, 2 | A synthetic DAG completes; projections provably rebuildable (DM-16) |
| 4 | `create_group_project` + folder + manifest + `deps/` | 3 | A project is created, survives restart, resumes, and moves between roots |
| 5 | `WorkspaceDeps` envelope + approval integration | 4 | A gated action queues an approval |
| 6 | Member execution + `materialize.py` | 3, 5 | One real member completes one real item, in a materialised agent |
| 7 | Orchestrator tools + `plan_execution` | 6 | Routing regression test passes |
| 8 | MCP/skill/tool overlay | 5 | An attached MCP tool is visible to exactly one member |
| 9 | `@task` force path | 7 | `@task` skips `plan_execution` |
| 10 | UI board (milestones) + approvals | 6 | Board renders; an approval works end to end |

Steps 3–6 are the v1 spine. Steps 7–9 are what makes it *discoverable*. Step 10 can lag — but observability cannot lag entirely, or every phase-1 bug is invisible. A minimal read-only board should ship in Phase 1.

---

## 14. What This Does Not Touch

Stated explicitly so a future reader does not "helpfully" extend the blast radius.

| Not touched | Why |
|---|---|
| `specialist_loop.py` | Members are workspace agents. Different loop. |
| `orchestrator_graph.py` as the project loop | Correct for within-run; wrong for multi-day (`02` §9.1). Remains the orchestrator's within-run driver. |
| `subagent_manager.py` | Durable continuable children remain the *sub*agent mechanism. A group member is not a continuable child. |
| `provider_modes.py` | Members are existing roles. |
| pydantic-ai internals | `fixed-concept.md` §4 — we build on the framework, we do not reimplement it. |
| `.deep_agents/` schema | Project state is in the project folder (`01` §2). |
| The SSE run pipeline | The supervisor is out-of-band (`11`). |

---

## 15. Build Record — Phase 4, Stages 1–2

**Status: WIRED AND RUNNING.** `pytest tests/ -q` → **245 passed, 0 failed** (229 before, +16).

### 15.1 What was wrong before this

`agents/group/` was 11 modules, 229 passing tests, and **zero callers.** Verified by grep: nothing under `server/` imported it, no group tools existed in `tools/_schemas.py`, and `server/services/` had no group scheduler. A complete engine with no ignition.

### 15.2 What shipped

| File | Change |
|---|---|
| `server/store/group_registry.py` | **The folder is the registry.** Projects are discovered by scanning `STARK_PROJECTS_ROOT` for `*/PROJECT.md` — no index file to drift, and a folder copied in from another machine is found on the next scan. Builds the `ProjectContext` from the manifest and the project's *own* materialised agents |
| `server/services/group_scheduler.py` | One thread for every project, 15s cadence, per-project `try/except` (RT-03). Boot does a **full rebuild** per project before the first tick |
| `server/api_server.py` | Both schedulers start in `_lifespan` — so they also run under `uvicorn server.api_server:app` and `--reload` |
| `tools/_schemas.py`, `tools/AddOnTool/groupTools.py`, `tools/__init__.py` | Six group tools visible to the orchestrator, descriptions written as routing policy |
| `agents/workspace_registry.py` | The orchestrator scope gains the six tools |
| `tests/fixtures/agent_registry_golden.json` | Updated — **the golden caught the scope change, as designed** |

### 15.3 The real run

The first project ever run for real: `D:\StarkProjects\hello-report`, one milestone, one item, one acceptance criterion, real LLM via `command` mode.

```
project.created → project.resumed → milestone.ready → item.ready → item.claimed
→ item.progress (member:orchestrator) → item.done → milestone.done → project.closed
```

Artifact on disk: `'Hello there, and welcome — so glad you could join us!'`

The item took **22 ticks of silence** while the model worked, then verified and closed. The whole chain — dispatch, run, collect, verify against criteria, milestone completion, project closure — ran unattended on a real model.

### 15.4 What the real run found that 229 tests did not

| # | Finding | Outcome |
|---|---|---|
| 1 | **Nothing dispatched a project with no members.** `01` §5.3 says a project with no members runs with *the orchestrator as sole member*; nothing implemented it, so `_dispatch` found no member and returned 0. Every routing test passed, every supervisor test passed, and a real project sat still. | **Real gap, latent since Phase 1.** Added `ProjectContext.effective_members()`. The default maps to the **`general` workspace agent**, not the orchestrator node: the orchestrator is a coordinator whose node delegates, so using it as a runner target would be circular. |
| 2 | **The budget ceiling could never fire.** `spend` stayed `$0.00` through a complete real run. `LocalRunner` read a `cost_usd` key that no agent node returns — cost lives in the **scoped `state_sync`** token usage, which is how delegation reads it (`delegation.py:739`). | **Real bug.** The runner now sets a scoped `active_state` per run and reads usage before the scope resets. Confirmed on a second real run: `spend $0.01/$1.00`. |
| 3 | **A member working silently looked like a deadlock.** The item produced no events for 22 consecutive ticks. At the real 15s cadence that is 5.5 minutes; anything slower than the 50-tick threshold (~12 min) would be declared stalled *mid-work*. | **Real bug, found only by running.** An item `doing` with a live lease is now a legitimate wait — self-limiting, because lease expiry hands it to the reaper, and a blocked item is no longer exempt. |
| 4 | **`add_item` silently dropped an item with no `id`.** The reducer cannot address a row without one, so the item vanished with no error at all. | Refused now, with a usable message, in both the toolkit and the tool. |
| 5 | **`restore_tools()` was needed.** The offline `tools` stub installed by other suites was never restored, so a later test that imported the facade got the stub. | Fixed in `_util.py`. This trap was flagged in Phase 0 and finally bit here. |

Findings 1–3 share a shape worth naming: **every one of them was invisible to a test suite that never ran the thing.** The tests asserted the board, the criteria, and the transitions correctly — and a real project still did nothing, for free, forever.

### 15.5 Deliberately not done

| Item | Why |
|---|---|
| Frontend board, approvals panel, member trace | Deferred by decision — API only for now (`05` §11 stays design) |
| Telegram approval | After a UI panel exists |
| `board`/`events` returning structured JSON | Text is enough to prove the reads work; a UI wants JSON |
| D-1 `run_command` tier, D-11 concurrency cap | Neither bites with a single memberless project |

### 15.6 Still open

| Item | Where |
|---|---|
| `tests/run_all.py` reports false failures and CI runs it | `07` §11.2 — now the only red thing left |
| `scratch/gen10_current.pid` tracked; `error_patterns.py` dormant | `07` §11.5 |
| The stale D-cell table (blockers listed for built phases) | `06` §4 |
| Real-LLM coverage for `LocalRunner` and the planner | no phase claims it — the real run is the evidence, and it is not repeatable in CI |