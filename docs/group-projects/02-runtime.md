# Group Projects — Runtime

**Version**: 1.0
**Date**: 2026-10-01
**Author**: Stark design session
**Status**: Draft
**Depends on**: `01-concept-model.md`

Defines *how a group project runs*: the supervisor, its tick, dispatch, verification, the single-writer rule, leases, and restart/resume.

---

## 1. The Core Rule

> **The orchestrator decides what work exists. The supervisor decides when that work happens.**

And the reason a separate supervisor exists at all:

> **So the LLM doesn't have to be awake.**

Today, everything requires your message: you send a prompt, the orchestrator delegates, results hit your inbox, you decide what's next. For a project that runs for days, that model is unusable — the orchestrator would hold a resident LLM context across the entire wait, and a single compaction would stand between you and losing the thread.

The supervisor lets the orchestrator **sleep** and wake only for judgement calls.

### 1.1 The six questions

The supervisor answers exactly these, and makes no judgement on any of them:

| Question | Answer |
|---|---|
| An item's dependency finished — start it? | All deps `done` + milestone live → `ready` |
| A lease expired (member crashed)? | → `blocked`, `attempts += 1` |
| Budget or deadline hit? | Halt dispatch |
| Nothing has happened for N ticks? | Deadlock → `project.stalled`, ask the human |
| You approved something while asleep? | Resume the item |
| Completion predicate satisfied? | Close, wake the orchestrator for the report |

### 1.2 What it never does

Think. Plan. Call the LLM. Write code. Judge quality. Choose members.

### 1.3 The honest alternative

Fold all six into the existing `park_wait()` (`delegation.py:1150`). That is literally what happens today, and it is a legitimate choice for a first implementation.

| | Separate supervisor | Inline in the orchestrator graph |
|---|---|---|
| Orchestrator while waiting | **Asleep.** No tokens, no context decay | Resident. Cost + compaction fragility |
| Wake triggers | Board events **and** a tick | Only inbox events |
| Crash mid-wait | Boot recovery from events (`§5.1`) | Needs its own resume path |
| Cost | One extra thread | None |
| Risk | Two things to reason about | LLM context is load-bearing for liveness |

It is ~200 lines of deterministic bookkeeping either way. The question is only whether it lives beside the orchestrator or inside it.

### 1.4 Division of labour

| Concern | Owner | LLM? | Cost |
|---|---|---|---|
| Authoring the plan | Orchestrator | yes | once per project |
| Choosing the environment (agents/tools/MCP/skills) | Orchestrator | yes | once per project |
| Deciding an item failed / a dep is unsatisfiable | Orchestrator | yes | on events only |
| Re-planning after an invalidation burst | Orchestrator | yes | on events only |
| Writing the final report | Orchestrator | yes | once |
| Deciding *when* to dispatch | **Supervisor** | **no** | ~0 |
| Milestone progression | **Supervisor** | **no** | ~0 |
| Computing readiness | **Supervisor** | **no** | ~0 |
| Evaluating `check` / `budget` criteria | **Supervisor** | **no** | ~0 |
| Enforcing budget / deadline / leases | **Supervisor** | **no** | ~0 |
| Applying queued operations | **Supervisor** | **no** | ~0 |
| Rebuilding projections on boot | **Supervisor** | **no** | ~0 |

**Why the supervisor makes no LLM calls.** A long-running project's control flow must be replayable and cheap. Every LLM call inside the loop is (a) expensive at 15-second granularity, and (b) a source of non-determinism that makes crash recovery ambiguous. If the supervisor decides everything except judgement calls, restarting is just "rebuild projections from the tail, continue."

This is Temporal's workflow/activity split. **The workflow must not call the LLM**; judgement lives in activities, which are safe to lose and redo.

---

## 2. The Supervisor

A single long-lived thread, started where `start_task_scheduler()` is started today (`server/api_server.py:546`, currently only under `__main__` — see `05` §10). Reuses the existing 15-second poll cadence (`task_scheduler.py:212`) as its heartbeat, so the refactor this feature requires pays for the heartbeat it needs.

**One supervisor per process. One writer. No per-project threads.**

```python
def supervisor_loop() -> None:
    recover_all_projects()          # once, on boot
    while True:
        try:
            for project_id in due_projects():
                tick(project_id)
        except Exception:
            log.exception("supervisor tick failed")   # never die
        time.sleep(TICK_SECONDS)     # 15
```

### 2.1 Why one thread, not one per project

| | One thread per project | One supervisor thread (chosen) |
|---|---|---|
| Single-writer guarantee | Needs a lock per project; ops can still interleave | Structural — only the supervisor writes |
| Failure isolation | A hung project thread leaks | Per-project try/except |
| DB contention | N writers | 1 |
| Ordering of ops | Undefined | Global order by `seq` |
| Cost | N threads | 1 |

Parallelism lives in the **member execution** path (existing slot semaphore, §7), not the scheduler.

---

## 3. Tick Phases

```
 tick(project_id)
 │
 ├─ 0. RECOVER      ensure projections match events (tail replay from last_seq)
 ├─ 1. APPLY OPS    drain the op queue in submission order
 ├─ 2. GATE         budget · deadline · paused · stalled  →  maybe close
 ├─ 3. REAP         expired leases · crashed attempts · orphan threads
 ├─ 4a. MILESTONES  milestone deps satisfied → ready; all items done → done
 ├─ 4b. READINESS   todo → ready, within a live milestone
 ├─ 5. DISPATCH     claim up to free slots for ready items
 ├─ 6. COLLECT      harvest finished item results, evaluate criteria
 ├─ 7. VERIFY       check / llm / budget criteria; manual → inbox
 ├─ 8. INVALIDATE   apply item.invalidated → coalesce → one re-plan wake
 ├─ 9. COMPLETE     evaluate the completion predicate
 └─10. EMIT         append events, publish to the realtime bus
```

**Phases 1, 5, and 8 are the only ones that change who is working.** Everything else is bookkeeping.

Phases 4a and 4b are separate because a milestone gates its items: an item can be fully unblocked and still not be dispatchable if its milestone is waiting on an earlier phase. Collapsing them into one pass is how a "ready" item ends up dispatched into a milestone that cannot progress.

### 3.0 Phase 0 — recover before anything else

```
 0. RECOVER
     if projections.last_seq < events.max(seq):
         replay events (last_seq, max(seq)] into the projections
         advance projection_checkpoint
     emit nothing if already consistent
```

This runs on every tick, not just at boot, and it is cheap when consistent: one indexed `MAX(seq)` comparison.

**What it does and does not fix — corrected after building Phase 1.** An earlier version of this section claimed the board is *"self-healing within one tick of any divergence, whether the cause was a crash, a manual edit, or a bug."* That was wrong, and the Phase 1 tests prove it.

A tail replay re-applies **only the events after the checkpoint**, so it can only repair divergence where the events landed but their projection writes did not:

| Divergence | Repaired by |
|---|---|
| An event written but its projection update never committed | **Phase 0 tail replay**, within one tick |
| A row corrupted by hand, or deleted, with the checkpoint current | **Only `rebuild()`** — replaying the tail cannot recreate a row an *earlier* event created |
| Anything at all, at boot | **`rebuild()`** — boot never trusts the tables (`§5.1`) |

The narrow case is not a weakness in practice, because `append()` writes the event, the projection, and the checkpoint in **one transaction** — so a crash cannot produce the wide case. What tail replay actually guards is a future code path that writes an event without persisting it, and that is worth catching within 15 seconds.

`ensure_consistent()` is honest about this: it returns the number of events replayed, and `0` means "checkpoint is current", not "the board is correct."

Full `rebuild()` from `seq = 0` exists for repair and for the DM-16 test, and is not on the hot path.

### 3.1 Phase 1 — apply operations

Ops are the orchestrator's only mutation channel (`03` §5). Applied in submission order, each in one transaction, each producing exactly one event:

```sql
BEGIN;
  INSERT INTO events (...)          -- exactly one, same txn
  apply_projection(op)              -- maintain the projection
  UPDATE projection_checkpoint ...
COMMIT;
```

A rejected op emits `op.rejected` with the reason and does not abort the tick. This is what makes voice-driven mid-run edits as safe as creation-time edits — **there is only one write path.**

### 3.2 Phase 4a — milestones

```
for each milestone where status = 'todo':
    if all(dep.status == 'done' for dep in depends_on):
        if any(dep.status == 'failed' for dep in depends_on):
            status = 'failed'                       # cascade to its items
        else:
            status = 'ready'; emit milestone.ready

for each milestone where status in ('ready', 'doing'):
    if all(item.status == 'done' for item in items):
        status = 'done'; emit milestone.done       # BR-09
```

Milestone readiness is **derived every tick, never stored** — same rule as items. Storing it guarantees staleness the moment a dependency moves.

### 3.3 Phase 4b — readiness

```
for each item where status = 'todo':
    if item.milestone.status not in ('ready', 'doing'):
        continue                                    # milestone still gated
    if all(dep.status == 'done' for dep in item.depends_on):
        if any(dep.status == 'failed' for dep in item.depends_on):
            blocked_reason = 'dep_failed'
        else:
            status = 'ready'; emit item.ready
```

### 3.4 Phase 5 — dispatch

For each `ready` item, while free slots remain:

1. Resolve the member: `owner_member` if set, else the first member whose `agents/*.json` declares the capability the item needs.
2. **Try to acquire the lease** atomically. `UPDATE items SET lease_owner=?, lease_expires=? WHERE id=? AND status='ready'` — if `rowcount == 0`, another supervisor run or member got there first; skip.
3. Set `doing`, `attempts += 1`, emit `item.claimed`.
4. Hand the job to the **existing** `JobRunner` path (`05` §3) with the member's materialised agent definition and the item's `workdir`.
5. Record the job id on the item so the supervisor can harvest it later.

### 3.5 Phase 7 — verification

| Criterion | Evaluated by | On pass | On fail |
|---|---|---|---|
| `check` | Named harness check | — | `attempts += 1`; `< max` → `ready` again, `>= max` → `failed` |
| `budget` | Always true by definition | — | n/a |
| `llm` | One bounded judge call, output + criterion | Verdict appended to events | Same as `check` |
| `manual` | Human, via inbox | Item → `done` | Item stays `doing`, reason `awaiting_approval` |

`llm` verdicts are recorded and **overridable by the orchestrator** — the judge is advisory, not authoritative. This matters: an LLM judge is a heuristic, and letting it hard-fail an item three times on a bad rubric would be a silent deadlock.

**A retry does not re-plan.** The orchestrator is notified once per failure streak (first failure and exhaustion), not on every attempt, so a flaky item cannot cause a re-plan storm.

### 3.6 Phase 8 — invalidation

Invalidation is the phase that makes long horizons work (`01` §4.4). It is last so that a re-plan sees a settled board rather than one still churning.

```
 collect item.invalidated events from ops + members this tick
 release each lease; set status=invalidated + reason; attempts UNCHANGED
 emit plan.revised (once, with the affected ids in data)
 if burst not already open:
     open a burst; wake the orchestrator ONCE            # BR-12
 else:
     add to the open burst; do not wake again
```

**Coalescing is the point.** One discovery frequently invalidates five items — "the vendor changed the CSV format" is not five separate problems. Five re-plan calls would be five LLM turns describing the same thing, and each would race the next.

**The burst has a timeout** (default 10 min). If invalidations are still arriving, the burst closes and a new one opens, so the orchestrator gets a steady signal instead of one call and then silence. An invalidation that sits unresolved past the re-plan window escalates to the human inbox (BR-13) — a project cannot sit forever with a known-wrong plan.

**`attempts` is never incremented here.** An invalidated item is a *plan* error, not an execution error, and counting it would let one bad assumption exhaust every item's retries in the milestone.

---

## 4. Single-Writer Rule

> **The orchestrator never writes project state. It submits operations; the supervisor applies them.**

This is the rule that makes live mutation safe, and it is the one that changes most from how Stark does things today (`02` §9).

| Property | Consequence |
|---|---|
| One writer | No optimistic-locking version column needed, no lost updates |
| Total order | Ops apply in submission order; `seq` is meaningful |
| Single audit trail | Every state change is an event, including operator edits |
| Creation ≡ mutation | `create_group_project` is itself an op. No second code path. |
| Symmetry | Voice-initiated mid-run edit behaves identically to a creation-time edit |

**Cost of this rule:** an op is not visible until the next tick (≤ 15 s). That is acceptable — a 15-second latency on "add another agent" is invisible, and it is what buys the guarantees above.

**Rejected alternative:** optimistic concurrency (`revision` column, retry on mismatch). Correct, but it distributes a single-writer invariant across every writer, and every future writer must remember to participate. The queue makes the invariant structural.

---

## 5. Leases & Crash Recovery

| Situation | Detection | Action |
|---|---|---|
| Member crashed mid-item | `lease_expires` passes (BR-02) | Item → `blocked`, `reason=lease_expired`, `attempts += 1` |
| Process hung but alive | No renewal past 2× duration | Lease stealable; item re-dispatched |
| Harness restarted | Boot-time `recover_all_projects()` | Rebuild projections from events; items with `doing` + expired lease → `blocked`; unexpired → grace window, then blocked |
| Projection diverged mid-run | Tick phase 0: `last_seq < MAX(seq)` | Replay the tail — self-healing within one tick |
| Job finished while we were down | Job artifact exists on disk | Harvest in phase 6, evaluate criteria |
| Job vanished | No artifact, no thread | Lease expiry handles it — nothing special needed |

**No item is ever permanently stuck.** Lease expiry is a total backstop: every `doing` item has a bounded lifetime regardless of what happened to the process running it.

### 5.1 Restart/resume

```
boot:
  for each project with state.db:
     full rebuild() from events          # DM-16 — never trust the tables
     set projection_checkpoint.last_seq = MAX(seq)
     for each item where status='doing':
        if lease_expires and < deadline > now:
            → blocked(lease_expired), attempts += 1
     emit project.resumed
```

Resume reads **rows**, not history. It does not replay conversation into any context. A member's transcript resumes from its own JSONL via `find_recent_session()` (`server/session/log.py:784`) and `build_session_context_native()` (`:977`) — the same path a normal multi-session chat already uses.

**Rebuild-on-boot is the strongest form of the guarantee**, and it is why DM-16 is the most valuable test in the feature: drop every projection table, rebuild, and assert equality. If that passes, the event log is provably sufficient.

---

## 6. Gates, Budget, Idle

### 6.1 Gate order

Evaluated before dispatch, most severe first. Any gate that trips **halts dispatch** and emits an event; the project stays readable and resumable.

| Gate | Trips on | Effect |
|---|---|---|
| `closed` | Completion predicate passed, or operator closed | Project → `done`; leases released |
| `budget` | `cost_usd >= cap` or `cost_tokens >= cap` | `budget.stop`; halt; inbox question |
| `deadline` | `now >= deadline` | `deadline.reached`; halt; inbox question |
| `paused` | Operator paused | Skip dispatch; state preserved |
| `stalled` | Zero events across N ticks | `project.stalled`; halt; inbox question |
| `warned` | 80% of budget | `budget.warn`; continue |

**Budget accounting** is accumulated from real usage — the same values `delegation.py` already records per job (`usage`, `cost`) — not estimated up front. Estimates are for the `budget.warn` projection only.

### 6.2 Idle detection

`events_since_last_tick == 0` for N consecutive ticks → `project.stalled`.

The supervisor must never spin a project that cannot make progress. Three real causes, three real outcomes:

| Cause | Detection | Resolution |
|---|---|---|
| Everything blocked on `manual` | `awaiting_approval` items exist and inbox is empty | Legitimate wait — exempt from stall counting |
| Everything blocked on `dep_failed` | Failed deps | Orchestrator re-plan; suppress stall noise |
| Everything invalidated | `invalidated` items awaiting a replacement | Re-plan burst is open — exempt |
| Genuinely deadlocked | No members, no ready items, nothing pending | Stalled → inbox question |

Exempting legitimate waits matters. Without it, every project that pauses overnight for your approval would look like a failure.

Note that stall detection counts **events, not ticks of work.** A project doing real work emits events continuously, so it never looks idle even while its CPU is quiet. That is deliberate — the thing that must never be reclaimed is "busy", and a quiet worker is busy.

**This is the same mechanism Grok Bot ships:** per x.ai's docs, Grok Bot *"may ask whether to keep routines running after a long period away and pause them if there's no response."* Ours is the same instinct, made deterministic.

---

## 7. Relationship to `delegate`

Two primitives, different lifetimes, **same `JobStore`**.

| | `delegate` | Group project |
|---|---|---|
| Lifetime | Within one run | Across runs, days, restarts |
| Scope | One chat | One project folder |
| Cardinality | Fire-and-forget fan-out | Persistent DAG |
| Durability | Job rows; in-memory registry is a cache | Job rows are canonical |
| Bound | `ORCH_JOB_TIMEOUT`, default 3600 s (`delegation.py:64`) | Budget / deadline / completion predicate |
| Result delivery | `[DELEGATION RESULT]` into the parent inbox | Board transition + event |
| Who routes | Orchestrator in-conversation | The DAG |

**A member may call `delegate` inside its own item** — that is short-lived sub-fan-out and is entirely fine. The confusion to avoid is the orchestrator creating *both* a group project and a parallel `delegate` batch for one intent. Rule: **`delegate` for "do these now"; group project for "get this done eventually."**

The existing slot semaphore (`_SLOT_COND`, `delegation.py:50`; `ORCH_MAX_PARALLEL` default 6, `:56`) is the shared concurrency governor for both. Group projects should reuse it rather than adding a second limiter, but should be able to reserve a floor for project work so a chatty orchestrator cannot starve a multi-day project.

---

## 8. Member Execution

When the supervisor dispatches an item, the actual work is the **existing** workspace-agent path. No new agent runtime. What changes is only **where the definition is read from**: the project's own `agents/*.json`, not the live registry.

| Input | Source |
|---|---|
| Agent definition | `<project>/agents/<member>.json` — the materialised file |
| Runtime entrypoint | the agent file's `derived_from` → `WorkspaceSpec.runtime` (`agents/workspace_registry.py:31`) |
| Model / provider | the agent file's `model` + `thinking`, over `command` mode (`provider_modes.py:76-89`) |
| Tool allowlist | the agent file's `tools` ∩ `tools/bindings.json` (`05` §6) |
| MCP servers | `<project>/mcp/servers.json` (project-owned, credentials by name) |
| Skills | `<project>/skills/*/SKILL.md`, injected into the system prompt |
| `project_path` | Item `workdir` or project root |
| Conversation | Member's `chat_id` transcript |
| Prompt | Item `detail` + acceptance criteria + relevant `context/*.md` |

**Provider profile for group projects** (`01` §5.2): `command` mode throughout, two model slots, `thinking: high` on both —

| Slot | Model | Used by |
|---|---|---|
| reasoning / authoring | `xiaomi/mimo-v2.6-flash` | orchestrator tier, planning passes, refinement members |
| execution / building | `deepseek/deepseek-v4.1-flash` | workspace tier: code, files, commands, tool loops |

Both are pinned per member at materialisation, so a folder's behaviour is stable regardless of the global mode dropdown. Raising the mimo path from today's `think_bias: low` to `high` is the only behavioural change to the matrix (`05` §9).

The prompt builder hands the member, explicitly:

1. Its item's `title` and `detail` — the whole job, verbatim.
2. The item's `acceptance` criteria — what "done" means, in the member's own words.
3. Which `context/` files to read.
4. The autonomy envelope — what it may do without asking.
5. How to signal completion (`set_item_status`) and how to request approval.
6. The item's `id`, so every tool call is attributable.

**The member's conversation is cumulative per member, with item boundaries as explicit turns.** Continuity across items is the point (a member should learn the vendor's format), and `ProcessHistory` compaction already handles growth. This is why item state *cannot* live in the conversation — it must be in the board, or a compaction erases it.

---

## 9. What Changes From Today

| Today | Group project | Why |
|---|---|---|
| One orchestrator run, user-driven | Supervisor tick, event-driven | "Runs until done" is impossible without a heartbeat that does not need a user turn |
| Orchestrator wakes from `park_wait()` (`delegation.py:1150`) on inbox events | Wakes on board events: `item.done`, `item.failed`, `item.invalidated`, `approval.granted`, `lease.expired`, `milestone.done` | The orchestrator's wake set must be project events, not just delegation results |
| Orchestrator graph calls the LLM inside control flow | Supervisor never calls the LLM | Replayable control flow; §1 |
| Result read from global `state_sync.outputs.agent_report` (`task_scheduler.py:163`) | Result read from the job row | `shared_state` is process-global (`state_sync.py:13`); unsafe under concurrency |
| Job state in `_DELEGATIONS` in-memory dict (`delegation.py:44`) | Job state in SQLite; dict is a cache | Restart loses in-flight work today |
| Tables are the record | `events` is the record; tables are projections (`01` §7) | Changing the model later must not cost a migration |
| One run at a time, by design (`specs/mobile-remote.md:70`) | N concurrent members, isolated by `WorkspaceDeps` | Required for any real group |
| Task = prompt + schedule, per chat (`server/routers/tasks.py`) | Project = manifest + milestones + board + predicate, per folder | A schedule is the wrong primitive for "until done" |

### 9.1 The orchestrator graph is not the project loop

Worth stating plainly, because it is the most tempting wrong move:

`orchestrator_graph.py` persists full message history to `orch_state_{chatId}.json` on every step (`:679`) so a crash can resume. That works for a single turn. For a multi-day project it means the *project's* durability is coupled to replaying a growing transcript — which is expensive, and which compaction then has to summarise. The project loop must keep small, queryable state instead. The existing graph remains the right driver for **within-run** orchestration; it is simply not the project supervisor.

### 9.2 The two loops, side by side

```
ORCHESTRATOR (LLM)                         SUPERVISOR (no LLM)
──────────────────                         ──────────────────
sleeps between events                      wakes every 15s
                                          0. recover projections
plan_execution ──► plan + environment       1. apply ops
create_group_project                        2. gates
                                          3. reap leases
on item.failed        ──── wakes ────►       4. milestones → readiness
on plan.revised       ──── wakes ────►       5. dispatch
on approval.requested ──── wakes ────►       6. collect
on project.completed  ──── wakes ────►       7. verify
                                          8. invalidate
author a plan, choose members,              9. completion predicate
re-plan, write the report                 10. emit events
```

The left column costs tokens and can be asleep for hours. The right column costs nothing and never sleeps.

---

## 10. Contracts

| ID | Requirement | Priority | Acceptance criterion |
|---|---|---|---|
| RT-01 | The supervisor makes no LLM calls | Must | Given supervisor code, When audited for LLM entrypoints, Then none are reachable from the tick |
| RT-02 | The supervisor is the only writer of `state.db` | Must | Given N concurrent agents, When all attempt mutations, Then all mutations go through the op queue |
| RT-03 | The supervisor survives a project-level exception | Must | Given a tick that raises, When the next tick runs, Then the supervisor is alive and the project is unchanged |
| RT-04 | Readiness is derived, never stored — at **both** levels | Must | Given a milestone or item dep transitions to `done`, When the next tick runs, Then dependents become `ready` with no stored-flag update |
| RT-05 | Every `doing` item has a bounded lifetime | Must | Given any `doing` item, When `2 × lease_duration` passes with no renewal, Then it is `blocked` or re-dispatched |
| RT-06 | Resume does not replay conversation | Must | Given a restart mid-item, When the supervisor resumes, Then no transcript is loaded into any context |
| RT-07 | A halted project is fully resumable | Must | Given `budget.stop`, When the cap is raised, Then dispatch resumes without data loss |
| RT-08 | Budget is measured from real usage | Must | Given N completed items, When budget is queried, Then it equals the sum of recorded job usage |
| RT-09 | Ops apply in submission order | Must | Given ops A then B, When the tick applies them, Then A's event `seq` precedes B's |
| RT-10 | `delegate` and group project share one concurrency governor | Must | Given both active, When slots are exhausted, Then neither bypasses the semaphore |
| RT-11 | Projections are rebuilt from events on boot | Must | Given a boot with divergent projections, When `recover_all_projects()` runs, Then every projection equals `rebuild()` output |
| RT-12 | A projection that **lags** the event log self-heals within one tick | Must | Given an event whose projection write did not commit, When the next tick runs, Then phase 0 replays the tail and the projection is correct. **Arbitrary corruption with a current checkpoint is explicitly out of scope** — only `rebuild()` repairs that (§3.0) |
| RT-13 | A milestone's completion propagates in the same tick | Must | Given the last item of a milestone becomes `done`, When the tick ends, Then the milestone is `done` — no second tick of lag |
| RT-14 | Invalidations coalesce into one orchestrator wake per burst | Must | Given 5 invalidations in one tick, When counted, Then the orchestrator was woken exactly once |
| RT-15 | An item in a non-live milestone is never dispatched | Must | Given an item with all deps `done` whose milestone is `todo`, When the supervisor selects work, Then it is not eligible |

---

## 11. Resolved decisions

| Decision | Rationale | Alternatives considered |
|---|---|---|
| Supervisor makes no LLM calls | Replayable, cheap, deterministic control flow; survives a week | LLM-driven loop (what Stark does now): expensive per tick, non-deterministic resume, compaction-dependent |
| **Separate thread, so the LLM can sleep** | A resident context across a multi-day wait is expensive and compaction-fragile; six deterministic questions do not need a model | Inline in the orchestrator graph via `park_wait` — legitimate for a first cut, and the documented alternative (§1.3) |
| One supervisor thread for all projects | Structural single-writer guarantee | One thread per project (leaks, contends, no ordering); an external scheduler (new dependency; no APScheduler today) |
| **Phase 0 runs every tick, not just at boot** | Makes "events are truth" safe in practice — divergence self-heals in ≤ 15 s | Boot-only rebuild (a mid-run corruption persists until restart); trusting the tables (the log becomes decorative) |
| Ops queue instead of optimistic locking | Makes the invariant structural, not a convention every writer must honour | `revision` column + retry (correct, but distributes the invariant); direct writes (loses ordering and the audit trail) |
| Reuse the 15 s poll | Already exists (`task_scheduler.py:212`); the refactor pays for the heartbeat | APScheduler or cron (new dependency); event-driven wake (requires the state to live in-process, which breaks resume) |
| Readiness derived each tick, per level | Cannot go stale | Store readiness (stale the instant a dep moves) |
| **Phase 8 last** | Re-planning must see a settled board, not one still churning from this tick's verification | Invalidation handled mid-tick (the re-plan sees contradictory state) |
| **Invalidation bursts coalesced with a timeout** | One discovery often invalidates five items; five re-plan calls describe the same thing and race each other | Wake per invalidation (re-plan storm); never wake (a known-wrong plan sits forever) |
| Lease expiry as the total backstop | Every stuck item self-heals without a janitor process | A separate reaper (extra moving part); heartbeat-keepalive-only (a frozen process looks alive) |
| Stall detection counts **events**, not activity | A busy project always emits events, so a quiet-but-working member never looks idle | CPU/wall-clock activity sampling (wrong signal; workers are I/O-bound and legitimately idle) |
| Reuse the delegation slot semaphore | One governor, not two | A second limiter for projects (starvation risk either way) |
| Cumulative per-member conversations | Continuity across items is the feature; compaction already handles growth | One conversation per item (loses the learning, and multiplies cold starts) |

---

## 12. Build Record

**Status: BUILT.** Gate met. `pytest tests/ -q` → **198 passed, 0 failed** (183 before Phase 1, +15).

### 12.1 What shipped

| File | Contents |
|---|---|
| `agents/group/events.py` | **The frozen vocabulary (D-13).** 37 kinds, the status enums, the `ITEM_STATUS_KIND` / `MILESTONE_STATUS_KIND` correspondence (DM-19), and `UnknownEventKind` |
| `agents/group/store.py` | `ProjectStore` — events table (truth) + projects/milestones/items/ops/checkpoint (projections), the single pure `_reduce`, `append` / `submit_op` / `rebuild` / `ensure_consistent`, and `evaluate_completion` |
| `agents/group/supervisor.py` | `Supervisor` — all 11 tick phases, `recover_all_projects` for boot |
| `tests/test_group_supervisor.py` | 15 tests: the synthetic ledger, gating, leases, invalidation bursts, gates, rebuild equivalence, corruption repair |
| `tests/_util.py` | new `agents_shim()` — imports `agents.*` submodules without executing `agents/__init__.py` |

### 12.2 The whole module is one reducer

`_reduce(state, event) -> touched_keys` is the only place that knows how an event changes state. Two callers use it: `append()` (incremental, one transaction) and `rebuild()` (full replay). Because there is exactly one implementation, `rebuild()` is provably equivalent to the incremental path — which is the thing DM-16 asserts and the reason every future change to the projection model is free.

`test_rebuild_reproduces_the_projections_exactly` drives a project partway, rebuilds, and compares. It also asserts idempotence.

### 12.3 What building revealed

| # | Finding | Outcome |
|---|---|---|
| 1 | **The budget gate could never fire.** `item.done` updated only the *item's* cost; the project's `spent_usd` was never accumulated, so `_gate` always saw 0. | **Real bug.** The reducer now accumulates to the project row on `item.done`. Accumulated rather than summed on read, because the gate runs every tick and must not walk the item table. Caught by `test_budget_stop_halts_dispatch`. |
| 2 | **Milestone completion did not propagate within a tick.** `_milestones` read deps from a snapshot taken at the start of the pass, so finishing `m1` only released `m2` on the *next* tick — 15 seconds of dead time per milestone, and a deviation from "readiness is derived every tick". | **Real bug.** The pass now walks milestones in `seq_no` order and updates its own local copy, so a completion is visible to dependents in the same pass. Locked in by `test_milestone_completion_propagates_in_the_same_tick`. |
| 3 | **`ensure_consistent` heals less than this document claimed.** §3.0 originally said divergence self-heals *"whether the cause was a crash, a manual edit, or a bug."* It cannot: replaying the tail cannot recreate a row an **earlier** event created. | **Doc corrected, not the code.** §3.0 now states precisely what tail replay repairs (an event whose projection write did not commit) and what needs `rebuild()` (anything else). RT-12 corrected to match. A test asserts each side. |
| 4 | The `tools` stub trap generalises. | `tests/_util.py::agents_shim()` now exists so any future test can import `agents.*` safely. It also made the suite **faster** — 125s before Phase 1, 86s after. |

### 12.4 Deliberate Phase 1 limits

| Limit | Why it is correct for this phase |
|---|---|
| `dispatch` is inert — it never claims without a runner | Claiming an item it cannot run would leave it `doing` until the lease expired, which is worse than leaving it `ready`. Phase 2 supplies the runner. |
| No members, no approvals, no messages | Their kinds are declared in the vocabulary but not reduced, so replaying one raises `UnknownEventKind` rather than silently ignoring it. |
| `update_item` and `set_completion` are rejected ops with a clear message | Both need event kinds that carry field-level deltas. Deferred rather than faked. |
| `add_milestone` / `add_item` are the only plan mutations | Enough to prove the op queue; `reprioritize` and the rest are Phase 3. |

### 12.5 Still open

| Item | Where |
|---|---|
| `tests/run_all.py` reports 46 false failures and CI runs it | `07` §11.2 |
| Wiring the supervisor into server boot (`start_group_scheduler`, `05` §10) | Phase 4 — nothing needs it until projects are created from the UI |
| `scratch/gen10_current.pid` tracked; `error_patterns.py` dormant | `07` §11.5 |