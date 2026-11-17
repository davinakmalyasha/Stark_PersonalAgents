# 07 — Group Projects

Durable, event-sourced multi-agent projects. This is the most behavior-sensitive
subsystem: a rewrite that changes one reducer rule corrupts every existing
project.

This document is the Rust implementation contract.

---

## 1. Model

```
Project → Milestone → Item → Attempt
```

- **Project**: goal, budget, deadline, done-condition, envelope.
- **Manifest** (`PROJECT.md`): authored intent.
- **Milestone**: coarse checkpoint; gates its items.
- **Item**: one unit of work with acceptance criteria.
- **Attempt**: one execution of an item by a member.
- **Member**: a materialized agent + chat.
- **Lease**: time-boxed exclusive claim on an item.
- **Event**: append-only truth.
- **Operation**: requested mutation, applied by the supervisor.
- **Approval**: pending human decision on a gated action.

**There is no group object. The project is the group.**

---

## 2. Folder

```
<project>/
├── PROJECT.md              authored intent + envelope
├── AGENTS.md               human notes
├── agents/                 chosen members (*.json)
├── skills/                 chosen skills
├── tools/bindings.json     allowlist/caps per agent
├── mcp/servers.json        credentials by env-var name
├── deps/                   project dependencies
├── context/
│   ├── decisions.md
│   └── findings.md
└── .gp/
    ├── state.db            events (truth) + projections
    ├── plan.md             rendered plan for humans
    ├── lock.json           harness version + digests
    └── runs/<chat>.jsonl   member transcripts
```

A project folder is self-contained: handing it to someone else hands them the
complete capability set.

---

## 3. Event store

### 3.1 Schema (exact)

```sql
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

CREATE TABLE projects (
  id TEXT PRIMARY KEY, owner TEXT, status TEXT NOT NULL DEFAULT 'created',
  halt_reason TEXT, warned INTEGER DEFAULT 0, goal TEXT,
  budget_usd REAL, spent_usd REAL DEFAULT 0,
  budget_tokens INTEGER, spent_tokens INTEGER DEFAULT 0,
  deadline REAL, completion_json TEXT, updated_at REAL
);
CREATE TABLE milestones (
  id TEXT PRIMARY KEY, project_id TEXT NOT NULL, title TEXT, detail TEXT,
  status TEXT NOT NULL DEFAULT 'todo', seq_no INTEGER DEFAULT 0,
  depends_on_json TEXT, acceptance_json TEXT, updated_at REAL
);
CREATE TABLE items (
  id TEXT PRIMARY KEY, project_id TEXT NOT NULL, milestone_id TEXT,
  title TEXT, detail TEXT, status TEXT NOT NULL DEFAULT 'todo',
  attempts INTEGER DEFAULT 0, max_attempts INTEGER DEFAULT 3,
  owner_member TEXT, workdir TEXT, lease_owner TEXT, lease_expires REAL,
  depends_on_json TEXT, acceptance_json TEXT, tags_json TEXT,
  cost_tokens INTEGER DEFAULT 0, cost_usd REAL DEFAULT 0,
  result TEXT, error TEXT, blocked_reason TEXT, invalidated_reason TEXT,
  updated_at REAL
);
CREATE TABLE ops (
  id INTEGER PRIMARY KEY AUTOINCREMENT, project_id TEXT NOT NULL,
  kind TEXT NOT NULL, payload_json TEXT, actor TEXT,
  state TEXT NOT NULL DEFAULT 'pending', error TEXT,
  created_at REAL, applied_at REAL
);
CREATE TABLE projection_checkpoint (
  project_id TEXT PRIMARY KEY, last_seq INTEGER NOT NULL DEFAULT 0
);
```

Foreign keys are logical, not enforced. Do not add
columns without a migration and a documented deviation.

### 3.2 Event vocabulary (frozen)

```
project.created|resumed|paused|closed|stalled
milestone.created|ready|done|failed
item.created|ready|claimed|released|progress|done|failed|blocked|invalidated|cancelled
plan.revised
idea.proposed
op.submitted|applied|rejected
lease.expired
budget.warn|stop
deadline.reached
approval.requested|granted|rejected
message.sent|read
member.added|removed|halted
capability.changed
write.reserved|done|skipped
```

Statuses:
- Item: `todo|ready|doing|blocked|done|failed|cancelled|invalidated`
- Milestone: `todo|ready|doing|blocked|done|failed|cancelled`
- Project: `created|running|paused|stalled|done|failed|cancelled`

Rules:
- `item.invalidated` is **not** a failure/block; it never increments attempts.
- Every non-initial status has exactly one event kind that enters it.
- Unknown kinds fail replay loudly; never silently ignore.
- `write.*` and `capability.changed` are no-ops for projections but remain
  first-class audit events.

Rust:
```rust
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventKind { /* variants above */ }

pub struct Event {
    pub seq: i64,
    pub project_id: ProjectId,
    pub ts: f64,
    pub kind: EventKind,
    pub actor: String,
    pub milestone_id: Option<MilestoneId>,
    pub item_id: Option<ItemId>,
    pub data: serde_json::Value,
}
```

Use `#[serde(rename_all = "snake_case")]` carefully: the wire strings are
dotted (`project.created`), not snake_case. Prefer explicit `#[serde(rename =
"project.created")]` on each variant to avoid a mismatch.

### 3.3 Append transaction

One transaction per event:

```sql
BEGIN IMMEDIATE;
  INSERT INTO events (...);
  apply_projection(event);
  UPDATE projection_checkpoint SET last_seq = ?;
COMMIT;
```

`apply_projection` is the **only** reducer. Incremental and replay paths must
call the same function. `rebuild()` replays from `seq = 0`; boot never trusts
tables.

`PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=5000;`

---

## 4. Supervisor

One task per process. One writer. No per-project threads.

```
loop:
  for project in due_projects():
    tick(project)
  sleep 15s
```

On error: log, never die.

### Tick phases

```
0. RECOVER      tail replay from projection_checkpoint
1. APPLY OPS    drain ops in submission order; one event per op
2. GATE         budget / deadline / paused / stalled → maybe close
3. REAP         expired leases, crashed attempts, orphan threads
4a. MILESTONES  deps satisfied → ready; all items done → done
4b. READINESS   todo → ready within a live milestone
5. DISPATCH     claim up to free slots for ready items
6. COLLECT      harvest finished item results; evaluate criteria
7. VERIFY       check / llm / budget; manual → inbox
8. INVALIDATE   coalesce invalidations → one re-plan wake
9. COMPLETE     evaluate the completion predicate
10. EMIT        append events, publish to realtime bus
```

Phases 1, 5, 8 change who is working; everything else is bookkeeping.

### Rules
- The supervisor never calls an LLM, never plans, never judges quality.
- The orchestrator decides *what work exists*; the supervisor decides *when it
  happens*.
- Ops are the only mutation channel; a rejected op emits `op.rejected` with a
  reason and does not abort the tick.
- A lease expiry → `item.blocked`, attempts += 1.
- A parked approval keeps the item `doing` and renews the lease.
- Dispatch respects: live milestone, dependencies, budget, deadline, free slots,
  member capability.

### Rust
```rust
pub struct Supervisor { store: Arc<GroupStore>, bus: EventBus, slots: Semaphore }
impl Supervisor {
    pub async fn run(self, cancel: CancellationToken) { /* interval 15s */ }
    async fn tick(&self, project: ProjectId) -> Result<()>;
}
```

`tick` is a sequence of private async methods, one per phase, each unit-tested.

---

## 5. Criteria

Port `agents/group/criteria.py`.

| Kind | Evaluation | Cost |
|---|---|---|
| `check` | file/glob/regex/min-bytes/command | free |
| `manual` | human via inbox; item stays `doing` | one approval |
| `llm` | bounded judge call | one call |
| `budget` | supervisor gate | free |

Paths are relative to the project root and must not escape it. Command checks
are bounded (`COMMAND_TIMEOUT=120`, output cap 4000).

Rust:
```rust
pub enum Criterion { Check(CheckSpec), Manual, Llm(JudgeSpec), Budget }
pub async fn evaluate(root: &Path, c: &Criterion) -> CriterionResult;
```

Use `globset`, `regex`, and `tokio::process` for command checks. Keep the
result strings (`pass`/`fail`/`pending`/`unsupported`) identical.

---

## 6. Autonomy and approvals

Port `agents/group/policy.py` (pure):

```rust
pub enum Tier { Autonomous, NeedsApproval, Blocked, Never }

pub fn classify(
    action: &Action,
    envelope: Option<&Envelope>,
    grants: &Grants,
) -> Tier;
```

Order:
1. Inside the project folder → free zone (except dangerous).
2. Dangerous outside → `Blocked` (credentials, money, destructive).
3. Envelope tier.
4. Default → `NeedsApproval`.

Envelope keys: `project_folder`, `external_read`, `external_write`,
`destructive`.

Approval flow:
```
member wants external_write
  → classify → needs_approval
  → item stays doing (awaiting_approval), lease renewed
  → approval.requested event + inbox + UI
  → human approve/reject → op → supervisor tick
  → re-check key → perform once → approval.granted
```

Always human regardless of envelope: credentials, payments, production
deployments, deletes outside the folder, first outward-facing action of a
shape.

### Idempotency
Key: `"{item_id}:{shape}:{payload_sha}"` (SHA-256 of canonical JSON args or
command). States derived from events: `fresh → reserved → done | skipped`.
A reserved-without-done key asks a human ("may or may not have been sent").
Do not dedupe autonomous actions or file writes.

---

## 7. Materialization and ops

- Orchestrator chooses agents, tools, MCP servers, skills, deps.
- `create_group` writes `PROJECT.md`, `agents/*.json`, `skills/`, `tools/
  bindings.json`, `mcp/servers.json`, `deps/`, `.gp/`.
- `group_update` ops: `add_item`, `add_milestone`, `invalidate_item`,
  `cancel_item`, `set_autonomy`, `pause`, `resume`, `close`, `replan`,
  `message`, `approve`, `reject`, `set_budget`, `set_deadline`, …
- One op per call; supervisor applies in order; exactly one event.

Rust:
```rust
pub enum Op { AddItem(..), AddMilestone(..), InvalidateItem{..}, /* ... */ }
pub struct OpRow { pub id: i64, pub kind: String, pub payload: serde_json::Value, pub state: OpState }
```

Op names/strings are frozen; they are stored in `ops.kind`. Do not rename them.

---

## 8. Crash recovery

- Boot: for each project, `rebuild()` from events; update checkpoint.
- Mid-run crash: leases are expired by the next tick; attempts increment.
- Reserved external write: appears as `unknown`; ask a human.
- Supervisor restart is safe because ops/events are durable.
- Do not trust projection tables on boot.

Acceptance test: kill between every pair of tick phases; after restart the
project state must equal a clean replay.

---

## 9. Rust module layout

```
stark-groups/
  src/
    ids.rs
    events.rs        # EventKind, Event, vocabulary
    reducer.rs       # apply(event, &mut Projection)
    store.rs         # sqlx append/rebuild/projections
    supervisor.rs    # tick phases
    criteria.rs
    policy.rs        # pure classify
    materialize.rs   # folder + manifest
    tools.rs         # group_* tools
  migrations/
    0001_state.sql
  tests/
    replay.rs
    tick.rs
    approvals.rs
```

---

## 10. Acceptance

- An existing `.gp/state.db` opens and replays to the same board.
- `rebuild()` from events equals the incremental projection.
- `rebuild()` equals incremental projection for every fixture.
- A real project completes end-to-end on a real model.
- Kill/restart at each tick phase resumes correctly.
- Approval + idempotency dedupes a repeated external write.
- Budget/deadline/stall gates produce the same statuses and events.
- Manual/LLM checks behave identically (including pending).
