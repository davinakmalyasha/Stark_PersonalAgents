# Group Projects — Orchestrator Tool Surface

**Version**: 1.0
**Date**: 2026-10-01
**Author**: Stark design session
**Status**: Draft
**Depends on**: `02-runtime.md`, `04-autonomy-and-approvals.md`

Defines the tools the orchestrator gains, how it learns to reach for them without being told, and the `@task` force path.

---

## 1. Routing Philosophy

> **The orchestrator should discover group projects the way an experienced colleague does — from the request — not from a keyword you have to remember.**

You should be able to say *"sort out the Q3 invoices"* to a running orchestrator, by voice, and have it recognise this as a project rather than a single task. Not because you typed a command, but because the tool's own description makes the decision obvious.

This is the same doctrine as `fixed-concept.md` §3, extended one level: agents select tools by scope, and the orchestrator additionally selects *the shape of the work* by what the tools say they are for.

**It also has to work by voice**, where syntax is exactly what you cannot rely on.

---

## 2. What This Requires

Three things. Without all three, natural routing will be unreliable.

### 2.1 Descriptions become load-bearing policy

Today tool descriptions are documentation. Under this design they are **the routing policy** — the mechanism by which the orchestrator decides a request is a project.

That means the text the model actually sees must carry the decision. Two places matter:

| Where | What |
|---|---|
| `TOOL_DEFINITIONS[].function.description` (`tools/_schemas.py:46`) | Full schema description; the primary routing surface |
| `_TOOL_DOCS` one-liners (`tools/_schemas.py:670`) | Injected into the system prompt; must not contradict the schema |

**Consequences:**

- State trigger conditions **and** when-not-to-use. "Use when the goal needs multiple dependent steps or outlives one run. Do **not** use for a single self-contained request."
- Never let a docstring describe an *implementation* ("creates a SQLite row") where it should describe a *capability* ("starts a durable project that runs until done").
- Any docstring edit is a routing change. Treat it as a behavioural edit, not a docs edit.

### 2.2 Make the judgement call a tool, not a hunch

"Is this a group project?" is currently an invisible model decision — if the orchestrator guesses, you cannot see why, and you cannot override it.

So make it explicit. One tool, one call, one visible decision:

```
plan_execution(
    goal: str,
    horizon: "minutes" | "hours" | "days",
    reversible: bool,          # can every step be undone inside a folder?
    needs_human: bool,         # will any step require a decision only you can make?
    constraints: str = ""      # deadline, budget, must-not-touch
) →
    {
      shape: "solo" | "group" | "scheduled",
      rationale: str,
      environment: {
        members: [ {name, derived_from, model, thinking, tools, skills, mcp, why} ],
        skills:  [ {name, why} ],
        mcp:     [ {server, tools, why} ],
        reusable: [ "nothing here is worth reusing" ]   # or an existing project folder
      },
      plan_preview: [ {title, detail, acceptance[], depends_on[], owner_member} ],
      blocking_questions: str[],
      est_cost_tokens: int
    }
```

**`environment` is the orchestrator deciding what gets materialised into the folder** (`01` §2). It is not a lookup — the orchestrator picks the members, their tools, their skills, and their MCP servers, and says *why* for each. Those files are then written into the project as `agents/`, `skills/`, `tools/bindings.json`, and `mcp/servers.json`.

Every member it proposes carries `model` + `thinking` explicitly, drawn from the two group-project slots (`01` §5.2): `xiaomi/mimo-v2.6-flash` or `deepseek/deepseek-v4.1-flash`, both `thinking: high`.

`reusable` matters: when a goal resembles work a previous project already did, the orchestrator should propose **starting from that project's agent/skill definitions** rather than authoring new ones.

Three returns for `shape`, and they are genuinely different problems:

| `shape` | Means | Example |
|---|---|---|
| `solo` | One agent, one turn, self-contained | "rename these 40 files to kebab-case" |
| `group` | Multiple dependent steps, multiple members, outlives a run | "reconcile Q3 invoices against POs" |
| `scheduled` | Not a project — a recurring job | "every weekday at 08:00, check the deploy status" |

**`scheduled` matters.** Group projects are not a replacement for the existing task scheduler; they are for a different shape of work. Without this return value the orchestrator will reach for a group project whenever someone says "every day", which is the wrong tool. This is also the refactor seam the existing `tasks.py` gains (`05` §7).

The orchestrator presents `plan_preview` and asks for confirmation before `create_group_project` — unless invoked via the `@task` force path (`02` §4).

### 2.3 Routing needs a regression test

This is the guard nobody has today. `tests/fixtures/agent_registry_golden.json` locks the registry, but **not descriptions**. Change a description, routing changes silently, and no test fails.

```python
# tests/test_orchestrator_routing.py
ROUTES = [
    ("sort out the q3 vendor invoices, match them to POs",
     "create_group_project"),
    ("rename these 40 files to kebab-case",
     None),                        # must NOT create a project
    ("every weekday at 8 check the deploy status",
     None),                        # must NOT create a project
    ("find out why auth is failing in staging and fix it",
     "create_group_project"),
    ("write a python script that parses the csv",
     None),
]

@pytest.mark.parametrize("prompt,expected", ROUTES)
def test_orchestrator_routes_correctly(prompt, expected):
    calls = run_orchestrator_turn(prompt)      # tools recorded, no side effects
    actual = "create_group_project" if any(c.name == "create_group_project" for c in calls) else None
    assert actual == expected
```

Negative cases matter more than positive ones. The expensive failure is the orchestrator wrapping a trivial job in a three-day project — that is where the token burn and the confusion come from. Both directions are asserted.

---

## 3. Tool Inventory

### 3.1 Creation

| Tool | Purpose |
|---|---|
| `plan_execution` | Decide the shape of the work; return a plan preview. The routing decision. |
| `create_group_project` | Materialise the project: folder, `PROJECT.md`, `.gp/state.db`, members, items. **Implemented as an op** (`02` §4). |

### 3.2 Mutation — the op surface

All are **ops**: submitted to the supervisor queue, applied on its tick, never writing `state.db` directly.

| Group | Ops |
|---|---|
| **Items** | `add_item` · `update_item` · `cancel_item` · `reprioritize` |
| **Members** | `add_member` · `remove_member` · `reassign_item` · `set_member_model` |
| **Environment** | `attach_mcp` · `detach_mcp` · `attach_skill` · `detach_skill` · `attach_tool` · `detach_tool` · `materialize_agent` |
| **Policy** | `set_budget` · `extend_deadline` · `set_autonomy` |
| **Human loop** | `request_human` · `approve` · `reject` · `answer` |
| **Lifecycle** | `pause` · `resume` · `close` · `set_completion` |
| **Read** | `board` · `item_detail` · `events` · `budget_status` · `capabilities` |

`board`, `item_detail`, `events`, `budget_status`, and `capabilities` are **read-only** and must not be ops — they bypass the queue, since queueing a read for 15 seconds is absurd.

The `Environment` ops write into the folder, not `state.db` (`01` §2): `attach_mcp` edits `mcp/servers.json`, `attach_skill` writes `skills/<name>/SKILL.md`, `attach_tool` edits `tools/bindings.json`, `materialize_agent` writes `agents/<name>.json`. Each is still an op — still queued, still single-writer, still one event.

### 3.3 Member tool surface

Members get a small, separate set:

| Tool | Purpose |
|---|---|
| `claim_item` | Take a lease on a `ready` item |
| `set_item_status` | `done` / `blocked` / progress notes — the only way a member changes the board |
| `append_finding` | Append to `context/findings.md` |
| `record_decision` | Append to `context/decisions.md` |
| `post_message` | Point-to-point or broadcast (`01` §8) |
| `request_approval` | The member's side of the autonomy gate (`04` §4) |
| `request_capability` | **Ask** for a tool / skill / MCP the member lacks |

Members get **no** `add_item`, no `add_member`, no `attach_mcp`, no `attach_skill`, and no direct write to `agents/`, `skills/`, `tools/bindings.json`, or `mcp/servers.json`. Board membership and the capability set are the orchestrator's and yours, not the workers' (`01` §5.4, DM-11).

`request_capability` exists so that constraint is workable rather than obstructive: a member that discovers it needs `run_command` asks, the orchestrator decides and issues the op, the folder is updated, and the member proceeds.

---

## 4. The `@task` Force Path

`@task` does not enable a feature. It **skips the detection step.**

| Entry | Path |
|---|---|
| Plain request | `plan_execution` → `solo` \| `group` \| `scheduled` → maybe `create_group_project` |
| `@task …` | `create_group_project` directly. No detection, no confirmation. |

Two entries, one implementation. The force path is ~5 lines of argument prefill and buys you a reliable escape hatch when detection guesses wrong — which it will, occasionally.

**Design note:** `@task` must not be a *parser*. It is not a keyword the router scans for; it is a flag the orchestrator sees in the prompt and responds to by skipping `plan_execution`. Anything smarter grows a second routing system that can disagree with the first.

---

## 5. Ops Are the Only Mutation Channel

Recall from `02` §4: **the orchestrator never writes project state.** Every tool in §3.2 submits an op.

```python
def add_item(project_id: str, title: str, detail: str,
             acceptance: list[dict], depends_on: list[str] = (),
             owner_member: str = "", workdir: str = "") -> str:
    op = {
        "kind": "add_item",
        "payload": {...},
        "submitted_by": "orchestrator",
        "submitted_at": now_iso(),
    }
    return queue_op(project_id, op)      # → applied next tick, one event
```

Consequences worth being explicit about:

- `add_item` returns `queued`, not `added`. Reading the board back would be stale.
- An op can be rejected (`err_cycle`, `err_no_acceptance`, `err_lease_held`) with no effect on the project.
- Every op — yours included — produces exactly one event (`02` §3.1).
- Voice-initiated edits mid-run take the identical path as creation-time edits. There is only one write path, so the two cannot drift.

---

## 6. Description Writing Rules

Concrete guidance for whoever writes these strings.

**Include:**
1. One sentence: what it does.
2. One sentence: **when to reach for it**, in the user's vocabulary, not the schema's.
3. One sentence: **when not to**, naming the sibling tool it loses to.
4. What it returns and whether the effect is immediate or queued.

**Template:**

```
description = """
Start a durable project that keeps working until it is done, across sessions.

Use when the request involves several dependent steps, needs more than one kind
of expertise, or should keep going after the conversation ends. Typical signals:
"reconcile", "audit", "get this done by Friday", "figure out why ... and fix it",
or a goal that spans tools.

Do NOT use for a single self-contained request ("rename these files") or for
recurring work ("every morning check X") — the first is `solo`, the second is
`scheduled`.

Returns a project id. Effects are queued and visible after the supervisor tick
(<= 15 s); call `board` to read back.
"""
```

**Anti-patterns:**

| Don't | Because |
|---|---|
| "Creates a group project record in state.db" | Describes implementation; the model needs capability |
| "This tool allows the user to create projects" | Restates the name; no decision content |
| Omitting the when-not-to | Causes over-triggering — the expensive direction |
| Two descriptions for one tool with different audiences | `_TOOL_DOCS` and the schema must not disagree |

---

## 7. Orchestrator Responsibilities

What the orchestrator is for, stated as a contract:

| Responsibility | Trigger | Not responsible for |
|---|---|---|
| Author the plan (one pass) | Project creation | Deciding when each item runs |
| Decide `solo`/`group`/`scheduled` | On a request | Micro-routing between tools |
| Notify human of `blocked`/`failed` streaks | First failure + exhaustion | Per-attempt chatter |
| Re-plan a slice | Blocked-on-unsatisfiable, exhausted, or scope change | Re-planning on every attempt |
| Summarise context at a boundary | Before compaction, and on request | Maintaining item state |
| Write the final report | Completion predicate passes | Running anything |
| Answer "why is X stuck?" | Human asks | Tracking state itself (it reads the board) |

**Explicitly not the orchestrator's job:** choosing the next ready item, holding leases, enforcing budget, or detecting stalls. Every one of those is the supervisor's. The moment the orchestrator does them, the project is only as durable as the orchestrator's context window.

---

## 8. Contracts

| ID | Requirement | Priority | Acceptance criterion |
|---|---|---|---|
| OT-01 | Every mutation tool enqueues an op; none writes `state.db` | Must | Given any group-project tool, When inspected, Then it calls `queue_op` and performs no direct DB write |
| OT-02 | `plan_execution` returns exactly one of `solo`/`group`/`scheduled` | Must | Given any goal, When `plan_execution` returns, Then `shape` is one of the three and `rationale` is non-empty |
| OT-03 | `plan_execution` returns a complete plan preview when `shape == 'group'` | Must | Given `shape='group'`, When it returns, Then every preview item has non-empty `acceptance` |
| OT-03b | `plan_execution` returns an `environment` block naming every member, skill, MCP server, and tool to materialise | Must | Given `shape='group'`, When it returns, Then `environment.members` is non-empty and each member carries `model` + `thinking` + `why` |
| OT-03c | Every proposed member pins a valid group-project model slot | Must | Given a proposed member, When validated, Then `model` is one of the two `command`-mode slots and `thinking` is `high` |
| OT-04 | Routing regression test covers positive and negative cases | Must | Given the test suite, When run, Then trivial and recurring prompts are asserted **not** to create a project |
| OT-05 | `@task` skips detection but reuses the same implementation | Must | Given `@task <goal>`, When handled, Then `create_group_project` runs with no intervening `plan_execution` |
| OT-06 | Tool descriptions state both trigger and non-trigger conditions | Must | Given any group-project tool, When audited, Then its description names when not to use it |
| OT-07 | Read tools bypass the op queue | Must | Given `board`, When called, Then it returns immediately without waiting for a tick |
| OT-08 | Members cannot mutate membership, policy, or the capability set | Must | Given a member's toolset, When audited, Then no membership/policy/environment op is present and `request_capability` is |
| OT-09 | An op rejection leaves the project unchanged and emits one event | Must | Given an invalid op, When the tick applies it, Then exactly one `op.rejected` event exists and no item changed |
| OT-10 | The orchestrator is notified once per failure streak | Must | Given an item failing 3 attempts, When counted, Then the orchestrator was notified at most twice |
| OT-11 | Environment ops write to folder files, never to `state.db` | Must | Given `attach_skill`, When applied, Then `skills/<name>/SKILL.md` exists and `state.db` holds only the event |

---

## 9. Resolved decisions

| Decision | Rationale | Alternatives considered |
|---|---|---|
| Natural routing via descriptions | Works by voice; matches `fixed-concept.md` §3 doctrine; the orchestrator genuinely knows better than a keyword | `@task` only (you must remember syntax; defeats the voice case); a parser (becomes a second routing system that disagrees with the first) |
| `plan_execution` as an explicit tool | Makes the judgement auditable and overridable; yields the rationale to the user | Silent model judgement (invisible, undebuggable, unoverridable); a rules-based router (can't weigh reversibility or horizon) |
| Three shapes, incl. `scheduled` | Stops group projects absorbing recurring work they are wrong for | Two shapes (orchestrator reaches for a project on "every day", the wrong tool) |
| Routing regression test | Description edits are silent behavioural changes; nothing else catches it | Manual QA (won't be run); golden-file on descriptions only (locks the string, not the behaviour) |
| Mutations via an op queue | Single-writer, ordered, one audit trail, creation ≡ mutation | Direct writes (races, no ordering); optimistic locking (spreads the invariant across writers) |
| Read tools bypass the queue | 15 s latency on a read is absurd | Everything through the queue (uniform but unusable for reads) |
| Narrow member toolset | Board membership is a supervisory concern; workers cannot expand their own permissions | Members get full ops (privilege escalation by self-append) |

---

## 12. Build Record — Phase 3

**Status: BUILT.** `pytest tests/ -q` → **229 passed, 0 failed** (211 before, +18).

### 12.1 What shipped

| File | Contents |
|---|---|
| `agents/group/planner.py` | `classify_shape` (a scored heuristic — **no model call**), `parse_directive` for `@task`, `propose_environment`, `propose_plan`, and `plan_execution` with an injectable LLM planner |
| `agents/group/tools.py` | `GroupToolkit` — the orchestrator surface (creation, mutation via ops, reads that bypass the queue) and the member surface, plus `TOOL_DOCS` written as routing policy |
| `agents/group/materialize.py` | `add_mcp_server` / `remove_mcp_server` / `add_skill` / `remove_skill` / `set_member_tools` / `remove_agent` — live capability changes that edit the folder |
| `agents/group/supervisor.py` | Seven new ops, all folder-writing, all still queued; `ProjectContext` carries the folder and the injection points; `llm` criteria reach an injected judge |
| `agents/group/criteria.py` | `llm` criteria evaluated through an injected `judge`, `unsupported` when none is configured |
| `tests/test_group_routing.py` | 18 tests, including the routing regression both ways |

### 12.2 The routing decision is now auditable

"Is this a group project?" was an invisible model judgement. It is now:

- a **tool call** (`plan_execution`) with a `rationale` string, and
- a **scored heuristic** by default, so the regression test runs offline.

The classifier gets **12/12** across the case set, including every negative. That matters more than the positives: the expensive failure is wrapping a trivial job in a three-day project, not missing one.

`@task` is a **prefix flag**, not a parser (section 4). It changes confirmation, not classification — asserted by a test, because the moment it starts influencing the shape there are two routers that can disagree.

### 12.3 What building revealed

| # | Finding | Outcome |
|---|---|---|
| 1 | **The portability check rejected every URL.** `_WINDOWS_ABS` was `[A-Za-z]:[\\/]`, which matches the `s:/` inside `https://` — so `mcp/servers.json` reported *"holds an absolute path 'https://mcp.sentry.dev/mcp'"*. Since attaching an MCP server is the whole point of this phase, the check would have refused every interesting project. | **Real bug, caught by the test that attaches a server and re-checks portability.** Fixed with a lookbehind: `(?<![A-Za-z0-9])[A-Za-z]:[\\/]`. Worth recording because it is a *false positive* in a guard — the kind that gets "fixed" by weakening the guard rather than the regex. |
| 2 | **`_verify` required a runner.** It returned early when `self.runner is None`, so a project whose items are driven by hand could never have its results judged — even though verification is about *criteria*, not about executing anything. | **Design smell, fixed.** Only `_collect` needs a runner. A hand-driven project now verifies normally. |
| 3 | **`CAPABILITY_CHANGED` was in `ALL_KINDS` but not `REDUCED_KINDS`.** Writing one would have made the log unreplayable — `rebuild()` would raise `UnknownEventKind` on a project that had ever changed its capabilities. | Added as an explicit **no-op** reducer with the reason stated: capability files are the folder's own truth, not projections of the log, but the event still exists for the audit trail. |

### 12.4 Deliberate Phase 3 limits

| Limit | Why |
|---|---|
| The LLM planner is injectable, and **no test exercises it** | Same seam as the runner. `plan_execution(planner=...)` is wired and inspected; a model call is not asserted anywhere. |
| The routing test asserts the **classifier**, not the model's choice | A routing test that needed a model would not be run. The classifier is the floor the model refines, and the floor is what can be regression-tested. |
| `set_completion` and `update_item` still reject | They need field-level event deltas. Faking them would corrupt the log; refusing is honest. |
| `board`, `item_detail`, `events` return text, not structured data | Enough to prove the reads bypass the queue and answer correctly. A UI wants JSON; that is Phase 4. |
| Phase 3 does **not** register the tools in `tools/_schemas.py` | The registry wiring is an integration step; the surface is what this phase had to decide, and `TOOL_DOCS` carries the routing text that belongs there. |

### 12.5 Still open

| Item | Where |
|---|---|
| Registering the group tools in the live tool registry so the orchestrator actually sees them | Phase 4 integration |
| `tests/run_all.py` reports false failures and CI runs it | `07` §11.2 |
| Wiring `start_group_scheduler` into server boot | Phase 4 |
| Real-LLM coverage for `LocalRunner` and the LLM planner | no phase claims it yet |