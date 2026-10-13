# Group Projects — Phase 0 Implementation

**Version**: 1.0
**Date**: 2026-10-01
**Author**: Stark design session
**Status**: **BUILT** — gate 4/4. See §11 for the build record and what it revealed.
**Depends on**: `05-integration-map.md` §13 (steps 1–2), `01-concept-model.md` §7

Phase 0 makes the **existing** delegation system durable. It fixes the foundation group projects will sit on, and it is worth doing even if group projects never ship.

---

## 1. What Phase 0 Delivers

Three changes. No new agent, no supervisor, no project folders.

| # | Deliverable | Touches |
|---|---|---|
| 1 | **`JobStore`** — durable job rows for delegation | new module `agents/jobs/store.py` |
| 2 | **`_DELEGATIONS` becomes a read-through cache** in front of it | `agents/delegation.py` |
| 3 | **`run_task_background` scopes its own state** | `server/services/task_scheduler.py` |

**What you get if the gate passes:**

- Delegations survive a process restart. Today they do not — `_DELEGATIONS` is process-local and its own comment says entries *"survive until process exit"* (`delegation.py:43`).
- Concurrent scheduled tasks stop misreporting each other's results. Today they **do** cross-talk, and §5 shows exactly how.

**What you do not get:** group projects. Those start at Phase 1. Phase 0 should not be judged on them.

---

## 2. Why Phase 0 First

### 2.1 It is decoupled from every open design question

| Open question | Does Phase 0 touch it? |
|---|---|
| D-13 event `kind` vocabulary (`06` §4) | **No.** The vocabulary is permanent once the first *project* exists. Zero projects exist in Phase 0. |
| Copy boundary (`01` §13) | **No.** That is materialisation into folders — Phase 3. |
| Milestones, `invalidated`, events-as-truth | **No.** All Phase 1. Phase 0's `jobs` table is a plain table; it becomes a projection in Phase 1. |
| Cloud host (`06` §2.4) | **No.** |
| `run_command` default tier (`06` §3) | **No.** |

### 2.2 It is independently valuable

Both fixes are improvements to what you already run, not groundwork for something speculative:

- A restart today silently loses every in-flight delegation. The job row survives; the *knowledge* does not.
- Two tasks due in the same 15-second tick race each other's reports (§5). This is a live, reproducible bug.

### 2.3 Building found what writing could not

Ten minutes of reading produced two findings that no amount of further doc refinement would have:

1. **The `state_sync` fix is ~6 lines, not a refactor.** The scoping mechanism already exists, is correct, and is proven in production at `delegation.py:623`. `05` §5 called this *"the highest-risk item in the whole integration"* — that was about the **consequence** (silent cross-talk, misattributed to the model), not the **effort**. Corrected in `05`.
2. **The placement gap.** `05` §3.2 said the `jobs` table lives in the project's `state.db`. But Phase 0 has no project folder and no `state.db` — and the ledger proves delegation is *harness*-scoped, not project-scoped. Resolved in §6.

---

## 3. Deliverable 1 — `JobStore`

### 3.1 Module

```
agents/jobs/
├── __init__.py     # exports JobStore, get_store()
└── store.py        # schema, DAO, connection
```

Named `agents/jobs/` rather than `agents/group/` because in Phase 0 there are no group projects — this is the harness-level store for `delegate`. Phase 1 adds the project-level backing behind the same interface (`05` §3.2). Where the shared interface ends up living is deliberately undecided; whichever module owns both backings is the natural home.

Module docstring follows house style: what is here, and **what is deliberately not here** (`agents/delegation.py:1-20`, `tools/skills.py:1-22`).

### 3.2 Schema

```sql
CREATE TABLE IF NOT EXISTS jobs (
  id             TEXT PRIMARY KEY,
  parent_chat_id TEXT,
  project_id     TEXT,            -- NULL in Phase 0; set in Phase 1
  item_id        TEXT,            -- NULL in Phase 0; set in Phase 1
  agent          TEXT NOT NULL,
  chat_id        TEXT,
  title          TEXT,
  status         TEXT NOT NULL,   -- queued|running|done|failed|cancelled
  attempts       INTEGER DEFAULT 0,
  lease_owner    TEXT,            -- Phase 1; NULL in Phase 0
  lease_expires  REAL,
  workdir        TEXT,
  usage_json     TEXT,
  result         TEXT,
  error          TEXT,
  artifact       TEXT,
  created_at     REAL NOT NULL,
  updated_at     REAL NOT NULL,
  finished_at    REAL,
  entry_json     TEXT             -- ← added during build; see §4.6
);
CREATE INDEX IF NOT EXISTS ix_jobs_parent ON jobs(parent_chat_id, status);
CREATE INDEX IF NOT EXISTS ix_jobs_project ON jobs(project_id, status);
```

**`project_id` and `item_id` exist now and are NULL.** Phase 1 sets them for group-project jobs; because the columns are already there, Phase 1 needs no `ALTER TABLE` on existing databases. This matches the additive-migration habit already in the repo (`chat_store.py:88-94` adds `tasks_json` exactly this way).

**Nothing is `AUTOINCREMENT` here.** Job ids come from `delegation.py` (the `del-…` scheme), not from the rowid — unlike `events.seq` in Phase 1, which needs monotonicity across rolled-back transactions and therefore does use it (`01` §7.1).

### 3.3 Connection

Raw `sqlite3`, matching `server/store/chat_store.py:45`:

```python
sqlite3.connect(path, check_same_thread=False)
db.execute("PRAGMA journal_mode=WAL")
db.execute("PRAGMA synchronous=NORMAL")
```

WAL matters more here than in `chat_store`: the scheduler thread, the delegation fan-out threads, and the watchdog all touch this file concurrently.

### 3.4 API

Deliberately small. Phase 1 extends it with the projection hooks; Phase 0 does not need them.

```python
class JobStore:
    def upsert(self, entry: dict) -> None: ...
    def get(self, job_id: str) -> dict | None: ...
    def list_for_parent(self, parent_chat_id: str,
                        status: str | None = None) -> list[dict]: ...
    def claim_running(self, parent_chat_id: str,
                      limit: int) -> list[dict]: ...
    def mark(self, job_id: str, status: str, **fields) -> None: ...

def get_store() -> JobStore: ...      # lazy singleton
```

`mark()` writes `updated_at` and, when `status` is terminal, `finished_at`. It is the single persistence entry point — see §4.3.

### 3.5 What the store is not

| Not | Why |
|---|---|
| The board | `01` §3 — `.agent-memory/orchestrator_jobs.jsonl` stays a *history*. Different table, different job. |
| The event log | Phase 1 introduces `events` as truth for *projects*. Phase 0's `jobs` is a plain table with no event log behind it. |
| A queue | It records jobs; `delegation.py`'s slot semaphore (`:50`) still decides who runs. Phase 0 does not add a scheduler. |

---

## 4. Deliverable 2 — `_DELEGATIONS` as a Read-Through Cache

### 4.1 The current shape

`_DELEGATIONS: dict[str, dict] = {}` (`delegation.py:44`) — one insertion site, many read sites:

| Kind | Lines |
|---|---|
| **Insertion** | `:852` — `_DELEGATIONS[del_id] = entry` — the only one |
| In-place mutation | `:371`, `:410`, `:429`, `:496`, `:518`, `:524`, `:536`, `:602`, `:614`, `:686` — `cur = _DELEGATIONS.get(del_id)` then mutate `cur` |
| Single reads | `:916`, `:938`, `:973`, `:1051`, `:1066`, `:1307`, `:1355` |
| Iteration | `:259`, `:950`, `:964`, `:985`, `:995` |
| Test reset | `:1364` `reset_delegations()` → `:1368` `_DELEGATIONS.clear()` |

**One insertion point is the good news.** The in-place mutations are what need care.

### 4.2 Cache semantics: write-through + read-through

| Operation | Behaviour |
|---|---|
| **Insert** (`:852`) | Write to dict **and** store |
| **Status transition** | Write to dict, then `store.mark(...)` |
| **Single read** | `_job(del_id)` helper: dict first, `store.get()` on miss, populate the dict |
| **Iteration** | Union of `dict.values()` and `store.list_for_parent(...)`, de-duplicated by id |

`_job(del_id)` replaces every bare `_DELEGATIONS.get(del_id)`. That helper is the entire read path, and it is what makes the object a *cache* rather than a second source of truth.

### 4.3 What actually persists

Not every field. The store exists for durability, not mirroring.

| Persisted | Not persisted |
|---|---|
| Insertion (the full initial entry) | Transient counters |
| Every status transition: `queued → running → done \| failed \| cancelled` | Live progress strings that are already streamed to the UI |
| `result`, `error`, `usage_json`, `artifact` on completion | Anything reconstructible from the artifact |
| `attempts` | |

**Rule:** if you would need it to answer *"what happened to this job?"* after a restart, it persists. If you would only need it while watching, it does not.

### 4.4 Restart behaviour

On `get_store()` first use, reconcile:

| Status found at boot | Action | Why |
|---|---|---|
| `running` | → `failed`, `error="process_restart"` | Honest — the work did die with the process. Phase 1's supervisor replaces this with lease/reap; Phase 0 must not pretend to have that. |
| `queued` | leave `queued` | Still visible, still cancellable. Not auto-started — the parent chat that requested it is gone. |
| `done` / `failed` / `cancelled` | leave | Terminal. |

This is deliberately the *least* clever option. Phase 0 has no supervisor and no leases, so anything more ambitious would be a second, worse scheduler.

### 4.5 Cache-is-a-cache

The property that makes it real:

```python
_DELEGATIONS.clear()               # pretend we restarted
status_table(parent_chat_id)       # must still answer, from the store
job_detail(del_id)                 # must still answer
```

That is gate assertion #4 (§8). Without it, `_DELEGATIONS` is still the source of truth and the store is decoration.

### 4.6 `entry_json` — the fidelity payload

**Added during build.** The columns in §3.2 are a *queryable projection*, not the whole entry. A delegation entry carries fields with no column — `agent_display`, `delivered`, `started`, `task`, `outcome`, `acceptance`, `mode`, `project`, `child_id`, `images`, `timeout` — and read paths depend on them:

| Read path | Needs | Would fail with columns alone |
|---|---|---|
| `status_table()` | `agent_display`, `started` | `KeyError: 'agent_display'` |
| `undelivered_count()` | `delivered` | every job looks undelivered |
| `_sync_orchestrator_job_plan()` | `started`, `title` | plan renders blank |

So each row also stores `entry_json` — the full entry, JSON-encoded (`default=str` for anything exotic).

**Consistency rule:** `upsert()` writes both; `mark()` updates the columns *and* merges the change into `entry_json` inside the same transaction. On read, `entry_json` supplies the shape and the columns are laid over the top, with a NULL column never clobbering a JSON value.

**Why not just add the missing columns?** The entry shape belongs to `delegation.py` and can grow. Columns would need a migration per field, and a missed one fails at read time with a `KeyError` — which is exactly how this was found. The payload makes the round-trip shape-agnostic; the columns stay for querying.

**Phase 1 note:** this becomes a projection properly — `events` will be the truth and `entry_json` will be rebuilt from it (`01` §7). Nothing here needs to change for that; the payload is just a projection that is currently maintained by hand.

---

## 5. Deliverable 3 — Scheduler State Scoping

### 5.1 The bug

`delegation.py` already isolates each job's state correctly:

```python
# agents/delegation.py:622-627
state_token = None
try:
    from sync.state_sync import active_state, new_scoped_state
    state_token = active_state.set(new_scoped_state({
        "chat_id": entry["chat_id"],
        "project_path": entry["workspace"],
    }))
except Exception:
    state_token = None
```
```python
# agents/delegation.py:680-681
from sync.state_sync import active_state
active_state.reset(state_token)
```

`task_scheduler.py` does **not**. It runs the agent loop and then reads the report from whatever `_target_state()` returns — which, in a thread that never set `active_state`, is the root `shared_state` (`sync/state_sync.py:39`).

### 5.2 The reproduction

`start_task_scheduler` (`task_scheduler.py:181`) spawns **a thread per due task** (`:204`), so concurrency is already possible — two tasks due in the same 15-second tick both run.

```
tick N — tasks A (chat X) and B (chat Y) both due
  ├─ Thread(A): run_graph_with_verify(A) → writes shared_state["outputs"]["agent_report"], shared["outputs"]["agent_report"] = A
  └─ Thread(B): run_graph_with_verify(B) → writes shared_state["outputs"]["agent_report"] = B

  Thread(A): safe_get_state().get("outputs", {}).get("agent_report")  → B   ← wrong
  Thread(B): safe_get_state().get("outputs", {}).get("agent_report")  → B   ← right, by luck
```

Both chats receive the same report. Which one is non-deterministic — it depends purely on write ordering. The failure is silent, and because the report looks plausible, it gets attributed to the model rather than to the scheduler.

### 5.3 The fix

Wrap the body in a scoped state, mirroring `delegation.py:622-627` / `:680-681`:

```python
def run_task_background(workspace_id: str, workspace_path: str, chat_id: str, task: dict) -> None:
    task_id = task["id"]
    update_task(workspace_path, chat_id, task_id, status="running",
                lastRunAt=task_now().isoformat())

    from sync.state_sync import active_state, new_scoped_state
    state_token = active_state.set(new_scoped_state({
        "chat_id": chat_id,
        "project_path": workspace_path,
    }))
    try:
        try:
            # ... unchanged body: prompt, add_message, run_general_direct /
            #     run_graph_with_verify, report read, recurrence bookkeeping ...
            from sync.state_sync import safe_get_state
            report = safe_get_state().get("outputs", {}).get("agent_report", "")
            # ...
        except Exception as exc:
            update_task(workspace_path, chat_id, task_id, status="failed", error=str(exc))
    finally:
        active_state.reset(state_token)
```

**Six lines of new code.** The `try/finally` is the only structural change; the existing broad `except` stays exactly where it is, nested inside.

### 5.4 Why this works

- `active_state` is a `contextvars.ContextVar` (`state_sync.py:9`), and `_target_state()` falls through to `shared_state` only when it is unset (`:39`).
- Each task already runs in its own thread (`:204`), and Python contextvars are per-thread — so **no lock is required**. The isolation is structural.
- `safe_get_state()` returns a proper copy (`:46-66`), so nothing downstream can mutate the scoped state by reference.
- `new_scoped_state()` (`:118`) snapshots provider/model settings from root while resetting output/log/plan/token fields — exactly the semantics a per-task run wants.

The `finally: reset` is hygiene rather than a correctness requirement today (each thread is fresh and dies at the end). It is kept because it matches `delegation.py:681` and because it becomes load-bearing the moment anyone introduces a thread pool.

---

## 6. Store Location

**Decision: a new `.deep_agents/jobs.db`.**

| Option | Verdict |
|---|---|
| **New `.deep_agents/jobs.db`** | **Chosen.** Zero risk to `chat_store.py` (load-bearing: WAL, per-chat locks, FTS5 triggers). The module is the natural seed of Phase 1's store. Additive and deletable if Phase 0 is abandoned. |
| A `jobs` table inside `chats.db` | Rejected. Reuses `_get_db()`, WAL and the additive-migration pattern, and delegation *is* chat-scoped — but it edits a load-bearing module for no Phase 0 benefit. |
| Extend `orchestrator_jobs.jsonl` | Rejected. Capped at 500 lines and truncated (`delegation.py:1231`). It is a history (`01` §3), not a queryable store. |

**Why the location needed deciding at all:** `05` §3.2 says the `jobs` table shares `state.db` with the event log — true in Phase 1, where `state.db` is per-project. But Phase 0 has no projects, and the existing ledger proves delegation is **harness-scoped**, not project-scoped:

```python
# agents/delegation.py:1192-1194
def _ledger_path() -> str:
    harness = os.environ.get("HARNESS_ROOT") or os.path.dirname(...)
    return os.path.join(harness, ".agent-memory", "orchestrator_jobs.jsonl")
```

One file for every chat on the machine. So Phase 0's store is harness-level, and Phase 1 gives `JobStore` two backings behind one interface — which is what `05` §3.2 already meant by "same `JobStore`".

---

## 7. Migration

**None. Leave `.agent-memory/orchestrator_jobs.jsonl` exactly as it is.**

- ~500 entries, 134 KB, capped and truncated by design
- Its schema is the ledger's, not the store's
- `01` §3 already declares it a history, not a board
- Importing it would add ~500 rows that all instantly reconcile to terminal statuses

New delegations write to both: the store (durable, queryable) and the ledger (history, unchanged). Removing the ledger is a separate decision with its own commit.

---

## 8. Gate

Phase 0 is done when all four hold.

| # | Assertion | How |
|---|---|---|
| 1 | **No regressions** | `python tests/run_all.py` passes |
| 2 | **Jobs survive a restart** | Start a delegation, `kill` the process mid-job, restart, then `store.get(del_id)` returns the row with its last status. `running` at boot reads back as `failed(process_restart)`. |
| 3 | **Concurrent tasks are isolated** | Two tasks in different chats made due in the same tick → each chat receives its **own** report. This is the regression test for §5.1 and it must fail against the current code before the fix. |
| 4 | **The cache is a cache** | `_DELEGATIONS.clear()`, then `status_table(parent)` and `job_detail(id)` still answer correctly from the store. |

Assertion 3 is the one worth writing *first*, against unfixed code. A bug that is silent in production is best proved by a test that fails loudly.

Assertion 4 is Phase 0's equivalent of DM-16 (`01` §11): it is what proves the store is real rather than a second dict that happens to be written to.

---

## 9. Not in Phase 0

Stated explicitly so the blast radius does not grow.

| Not done | When |
|---|---|
| Supervisor, tick, milestones, leases, `events` | Phase 1 |
| Project folders, materialisation, `state.db` | Phase 2 |
| `plan_execution`, `create_group_project`, `@task` | Phase 3 |
| Fixing `start_group_scheduler()`'s `__main__`-only wiring (`05` §10) | Phase 1 — nothing needs it yet |
| Raising the `command` mode orchestrator thinking tier (`05` §9.1) | Phase 3 |
| Removing the JSONL ledger | separate decision |
| Wiring `tools/skills.py` | Phase 2 |
| Any UI | Phase 4 |

---

## 10. Risks

| Risk | Severity | Mitigation |
|---|---|---|
| The in-place mutations at `delegation.py:371-686` miss a persistence hook | **Medium** | The single insertion point (`:852`) and a bounded set of status transitions make this auditable. Gate assertion 4 catches a missed transition: rehydrated status would be wrong. |
| Boot reconciliation is too aggressive | Low | Only `running` → `failed(process_restart)`. Nothing else is touched. `queued` is left visible and cancellable. |
| `jobs.db` connection contention under WAL | Low | Three writers at most (scheduler, fan-out threads, watchdog). WAL is designed for this. |
| The `try/finally` in `run_task_background` changes an error path | Low | The existing broad `except` is nested *inside* the `finally`, so failure handling is unchanged. Assertion 3 covers the happy path; run the existing task tests for the rest. |
| Someone reads `_DELEGATIONS` directly again later | Medium | `_job(del_id)` is the documented read path. A test asserting no bare `_DELEGATIONS.get` remains would be cheap — worth adding while the grep is 20 lines long. |

### 10.1 What building will probably reveal

Recorded so it is expected rather than alarming:

- The exact status-transition points may not all be where §4.3 implies. `delegation.py` is 1,370 lines and the transitions are implied by control flow, not declared.
- `reset_delegations()` (`:1364`) clears the dict for tests — it may need to clear or scope the store too, and that will only become obvious when the existing tests run against the new store.
- The `agent` / `title` / `workdir` fields may not all be available at the single insertion point at `:852`, which would mean persisting at the first status transition instead.

None of these change the design. All of them are the reason §2.3 exists.

---

## 11. Build Record

**Status: BUILT.** Gate 4/4. Committed to `development`.

### 11.1 What shipped

| File | Change |
|---|---|
| `agents/jobs/__init__.py` | new — exports `JobStore`, `get_store`, `db_path` |
| `agents/jobs/store.py` | new — schema, DAO, WAL connection, boot reconcile |
| `agents/delegation.py` | `_store` / `_persist` / `_job` / `_all_jobs`; persistence at the 4 status transitions + insertion; reads routed through the store; `reset_delegations` clears both |
| `server/services/task_scheduler.py` | `run_task_background` sets and resets a scoped `active_state` |
| `tests/test_task_isolation.py` | new — gate 3, written first and failing |
| `tests/test_job_store.py` | new — gate 2 and gate 4 |

### 11.2 Gate results

| # | Assertion | Result |
|---|---|---|
| 1 | No regressions | **PASS** — 133 → 137 passed, the same 46 pre-existing failures throughout |
| 2 | Jobs survive a restart; `running` reconciles to `failed(process_restart)` | **PASS** — `test_jobs_survive_a_restart` |
| 3 | Concurrent tasks produce independent reports | **PASS** — verified FAILING before the fix (`chat-A received ['report::B']`) |
| 4 | `_DELEGATIONS` is a cache | **PASS** — `test_cache_is_a_cache`, plus an end-to-end check against a real `jobs.db` |

The 46 pre-existing failures are all `missing 1 required positional argument` — pytest-fixture-style tests that the zero-arg `tests/run_all.py` runner cannot drive. They are unrelated to Phase 0 and were failing before it.

### 11.3 What building actually revealed

Three findings the spec predicted, and one it did not.

| # | Finding | Outcome |
|---|---|---|
| 1 | **The row shape ≠ the live entry shape.** | **Real and material.** The `jobs` columns are a *subset*: `agent_display`, `delivered`, `started`, `task`, `outcome`, `acceptance` are not columns. Rehydrated rows broke `status_table` (`KeyError: 'agent_display'`) and `undelivered_count` (every job looked undelivered). Fixed by adding **`entry_json`** — the full entry as a payload alongside the queryable columns. `mark()` merges into it so both stay consistent. This is the one design refinement Phase 0 needed. |
| 2 | `reset_delegations()` needed to clear the store. | **Confirmed** — and it needed a new `JobStore.delete()` to do it. |
| 3 | Status-transition points were where §4.3 predicted. | **Confirmed** — insertion at the single site, plus four transitions (`_finalize_entry`, `_run_item`, `_run_resume_item`, `cancel_all`). All now persist. |
| 4 | **Not predicted: the test suite is not isolated.** | `tests/_util.py::tools_stub()` replaces `sys.modules["tools"]` with a minimal stub and **never restores it**, so any later test module that imports the real agent stack dies with `ImportError: cannot import name 'ToolExecutionResult' from 'tools'`. Both new test files work around it by installing a path-only `agents` package shim that loads `agents.delegation`/`agents.jobs.store` without executing `agents/__init__.py`. **This is a pre-existing test-infrastructure bug worth its own fix.** |

### 11.4 Cost notes

- `job_detail()` takes ~8.5s on first call standalone, because it does `import tools.watcher` and `import workspace_manager`. Pre-existing; in the full suite `tools` is stubbed so the import fails fast and it costs nothing. Not introduced here, not fixed here.
- The new tests add ~0.1s to the suite (17.6s total, from 15.9s). An earlier version that imported the real agent stack cost 12s; the module shim removed that.

### 11.5 Deliberately still open

| Item | Where |
|---|---|
| `scratch/gen10_current.pid` is tracked — a runtime PID file in version control | noted, not fixed |
| `error_patterns.py` is a new module with **zero** imports anywhere — dormant | noted, not wired |
| The `tools_stub()` isolation bug | §11.3 finding 4 |
| Whether `_DELEGATIONS` should stop being read directly anywhere else | §10 risk row 5 |
