# Group Projects — Concept Model

**Version**: 1.0
**Date**: 2026-10-01
**Author**: Stark design session
**Status**: Draft
**Depends on**: `00-vision.md`

Defines *what a group project is*: its entities, its on-disk shape, and the split between authored intent and harness-owned run state.

---

## 1. Entities

**Hierarchy: project → milestone → item → attempt.** Milestones exist so a long project stays readable; the flat form (`00` §6.5) does not scale past a few dozen items.

| Entity | Meaning | Lifespan | Owner of truth |
|---|---|---|---|
| **GroupProject** | A unit of long-running work with a goal, a budget, a deadline, and a done-condition | Days–weeks | `events` |
| **Manifest** (`PROJECT.md`) | Human/orchestrator-authored intent: goal, why, done-conditions, budget, deadline, autonomy envelope | Edited by hand / orchestrator | The file itself |
| **Milestone** | A coarse checkpoint grouping items; carries ordering between phases | Until done/cancelled | `events` |
| **Item** | One node of work with an owner member and its own acceptance criteria | Until done/invalidated/cancelled | `events` |
| **Attempt** | One execution of an item by a member | Per lease | `events` |
| **Member** | A participating agent: a materialised `agents/<name>.json` + a `chat_id` | Project lifetime | `agents/*.json` (definition) |
| **Lease** | A time-boxed exclusive claim on an item, held by one member | Minutes–hours | `events` |
| **Event** | An append-only record of every state transition, tool-visible | Permanent | **`state.db` — the only truth** |
| **Operation** | A requested mutation submitted by the orchestrator, applied by the supervisor | Until applied | `events` |
| **Message** | Point-to-point (or broadcast) agent-to-agent note | Ephemeral | `events` |
| **Approval** | A pending human decision on a gated action | Until granted/rejected | `events` |

Everything above except `PROJECT.md` and the materialised capability files is **state**, and all state lives in the event log. Tables like `items` are *projections* of it (§7).

**Not an entity: the group.** There is no group object. The project *is* the group. A project with one member is a group of one — no separate concept, no separate code path (`00` §7).

---

## 2. Project Folder — a self-contained bundle

**A group project folder carries everything its members need.** Not a pointer back to the harness: the agents, the tasks, the chosen tools, the MCP servers, the skills, and this project's own dependencies all live *inside* the folder. When the orchestrator creates a project it **materialises that capability set into the folder** — a new project gets its own copy, chosen by the orchestrator, editable afterwards.

The orchestrator decides which members a project needs, which tools and MCP servers those members need, which skills apply, and what extra packages the work requires. That decision is written into the folder.

Root is configurable — `STARK_PROJECTS_ROOT`, default `D:\StarkProjects` (`05` §4.3). Chosen over `.deep_agents/projects/` (wiped on reset, coupling project state to harness runtime state) and over `D:\MyProject\projects\` (rediscovered by `project_router.py:28` as a workspace). A cloud deployment sets this to a path on a persistent volume (`06` §2).

```
D:\StarkProjects\landing-page-coffee\
│
├── PROJECT.md                 ── AUTHORED  intent · done-conditions · budget
│                                         deadline · autonomy envelope · environment
├── AGENTS.md                  ── AUTHORED  human notes for members (CLAUDE.md-style)
│
├── agents\                    ── MATERIALISED  the chosen members
│   ├── frontend.json
│   └── copywriter.json
│
├── skills\                    ── MATERIALISED  chosen skill content
│   ├── brand-voice\SKILL.md
│   └── landing-patterns\SKILL.md
│
├── tools\                     ── MATERIALISED  chosen tool bindings
│   └── bindings.json          (allowlist, caps, params per agent)
│
├── mcp\                       ── MATERIALISED  chosen MCP servers
│   └── servers.json           (credentials by env-var NAME only)
│
├── deps\                      ── AUTHORED  this project's own dependencies
│   ├── requirements.txt                      beyond the harness runtime
│   ├── package.json
│   └── install.lock
│
├── .gp\
│   ├── state.db               ── HARNESS   events (truth) + projections
│   ├── plan.md                ── HARNESS   rendered milestones + items, for humans
│   ├── lock.json              ── HARNESS   harness version + content digests
│   └── runs\
│       └── <chat_id>.jsonl    ── HARNESS   per-member transcripts
│
├── context\
│   ├── decisions.md           ── SHARED    ADR-style: decided + why
│   ├── findings.md            ── SHARED    research, cited
│   └── artifacts\             ── SHARED    the landing page itself
│
└── inbox.md                   ── SHARED    approvals + open questions
```

`deps/` is what makes *"install what the task demands"* portable: a project-local `requirements.txt` / `package.json`, installed into the project rather than into the harness. Local and cloud run the same install command.

### 2.1 The Portability Contract

Six rules. A folder obeying them runs unchanged on your laptop and on a cloud host (`06` §2).

| # | Rule | Why |
|---|---|---|
| **P-1** | Every path in project files is **relative to the project root** | `item.workdir` is `artifacts/reports`, never `D:\MyProject\out`. An absolute path is the single most common reason a folder won't move. |
| **P-2** | Secrets are **env-var names**, resolved by the host | Already the MCP pattern (`bearer_token_env_var`, `env_vars`). Your `.env` locally; a systemd `EnvironmentFile` on the box. Values never enter a file that gets copied. |
| **P-3** | `state.db` records `owner` as a **host id**, never an absolute path | `localhost` locally, `box-01` on the cloud. One owner per project, no fencing (`06` §2.5). |
| **P-4** | Dependencies live in `deps/`, installed into the project | The harness runtime is never mutated by a project. |
| **P-5** | Outside the folder, nothing is required but the pinned runtime, Python, node, and env vars | This is what "self-contained" has to mean if §13 resolves to option A. |
| **P-6** | `lock.json` records the harness version + content digests | Makes a folder *reproducible*, not merely self-describing. Without it, regenerating a materialised agent is archaeology. |

**What is deliberately not in the folder:** the harness runtime, `.deep_agents/`, absolute paths of any kind, credential values.

### 2.2 Who owns what

| Path | Written by | Read by | Never |
|---|---|---|---|
| `PROJECT.md` | Human, or orchestrator proposing a diff | Supervisor, orchestrator, members | Overwritten by the harness |
| `AGENTS.md` | Human | Members, at prompt-build time | Modified by members |
| `agents/*.json` | Orchestrator at creation; humans afterwards | Member runtime | Re-resolved from the registry mid-run |
| `skills/*/SKILL.md` | Orchestrator at creation; humans afterwards | Member prompt builder | Modified by members |
| `tools/bindings.json` | Orchestrator at creation; humans afterwards | Member toolset build | Widened by members |
| `mcp/servers.json` | Orchestrator at creation; humans afterwards | MCP bridge (per project) | Carrying credential **values** |
| `deps/*` | Orchestrator at creation; humans afterwards | Install step | Written into the harness env |
| `.gp/state.db` | Supervisor **only** | Everyone, via tools | Any table used as the source of truth (§7) |
| `.gp/plan.md` | Harness, rendered from projections | Human | Used as a source of truth |
| `.gp/lock.json` | Harness at creation; refreshed on `materialize_agent` | Rebuild tooling | Hand-edited |
| `.gp/runs/*.jsonl` | Session log manager | Member on wake, human via UI | Parsed for state |
| `context/*.md` | Members | All members | Left stale without review |
| `inbox.md` | Harness (append), human (answer) | Orchestrator, supervisor | Edited by members directly |

### 2.3 Why materialise rather than reference

The alternative — folder references `scope: developer`, tools resolved live from `tools/_schemas.py:29`, skills globbed from `.agents/skills/`, MCP from `.deep_agents/mcp_servers.json` — is less duplication but **not self-contained**. A folder then means nothing outside your machine, and two projects cannot carry different versions of the same agent.

Materialising makes the folder the unit: zippable, movable, inspectable, and handable to someone else with its own capability set.

**Cost, and how it is contained:** a materialised agent is a snapshot. Fixing `developer` in the registry does not reach projects already created. Each materialised file records `derived_from` + `derived_at_version`, and `lock.json` (`P-6`) records the harness version, so regeneration is a command rather than archaeology (`05` §9.2). *How much* of the harness itself is copied is open — see §13.

---

## 3. The Two-State Rule

> **Authored state lives in `.md`. Run state lives in the event log. Never the reverse, and never both.**

| | Authored (`.md`) | Run state (events → projections) |
|---|---|---|
| Audience | Human + model | Harness |
| Mutability | Slow, deliberate, diffable | Append-only, never updated |
| Blast radius of corruption | A stale comment | A projection rebuild |
| Compaction-safe? | Yes — read on demand | Yes — queried, never replayed into a context |
| Example | Goal, done-conditions, autonomy | `item.done`, `lease.expired`, `plan.revised` |

**Why this is non-negotiable:** Stark already learned this lesson with the orchestrator's job plan. `_sync_orchestrator_job_plan()` (`delegation.py:251`) and `_job_plan_block()` (`orchestrator_graph.py:234`) exist precisely because the orchestrator used to *hand-write* its task list. That fix generalised the harness's view off the model's narration. The group project applies the same fix as a standing rule.

**The failure mode being prevented:** if the board is LLM-authored, then one compaction, one crash, or one rogue member destroys it. Multi-day work cannot depend on any single context window surviving.

**And the second-order reason, from §7:** even a harness-written board goes stale the moment the model changes. If the tables are the record, changing the status vocabulary later means a migration on every live project. If the log is the record, it means editing a projection.

**Corollary:** `.agent-memory/orchestrator_jobs.jsonl` is a **history**, not a board — and can never be one. Its path resolves to the **harness root**, not to a project:

```python
# agents/delegation.py:1192-1194
def _ledger_path() -> str:
    harness = os.environ.get("HARNESS_ROOT") or os.path.dirname(...)
    return os.path.join(harness, ".agent-memory", "orchestrator_jobs.jsonl")
```

One file for every chat and every workspace on the machine. It is also **capped at 500 lines and truncated on rotation** (`delegation.py:1231`). A single machine-wide, lossy, append-only log cannot hold per-project state — which is exactly why a project gets its own `state.db`.

*(Corrected 2026-10-01: an earlier version of this section called the ledger per-workspace. It is harness-level. The per-workspace `.agent-memory/` files — `project.md`, `repository_map.md` — are a separate concern; the job ledger is not one of them.)*

---

## 4. Milestones and Items

### 4.1 Milestones

A flat `todo/doing/done` list stops being readable somewhere around a few dozen items. Milestones are the coarse layer that keeps a long project legible, and they carry the ordering between phases so item-level dependencies stay small.

| | |
|---|---|
| States | `todo · ready · doing · blocked · done · failed · cancelled` |
| `done` when | every item in it is `done` |
| `ready` when | all its milestone-level dependencies are `done` |
| Fields | `id`, `title`, `detail`, `status`, `depends_on`, `seq_no`, `acceptance` (optional — an optional project-level check for the phase) |
| Project `done` when | every milestone is `done` |

**Dependencies are declared at one level only.** Either:

- a **milestone** depends on another **milestone** (the coarse plan), or
- an **item** depends on another **item within the same milestone** (the fine detail).

Never an item in `m3` depending on an item in `m1`. That mixed graph is exactly how a long-horizon plan becomes unreadable — you can no longer see the phases. Cross-phase ordering is expressed by milestone dependencies, and that is all it needs to be.

**A project with no milestones gets one implicit milestone** named `all work`. Small projects never think about this; large ones get the structure for free.

### 4.2 Progress, the thing a flat board cannot tell you

```
Landing page for the coffee business
├─ m1  Research          ✅ done      (4 items, $0.31)
├─ m2  Brand & copy      ✅ done      (3 items, $0.44)
├─ m3  Build             🔄 doing      (2/5 items, $1.20)  ← current
├─ m4  Accessibility     ⏸ todo       (blocked on m3)
└─ m5  Ship              ⏸ todo       (blocked on m4)

milestones 2/5 · items 9/14 · budget $1.95 / $4.00 · 1 awaiting approval
```

That last line is the payoff. A flat board of 14 `done` ticks tells you nothing about whether the project is on track.

### 4.3 Item lifecycle

```
                    ┌──────────┐
                    │   TODO   │  authored by the planning pass or an op
                    └────┬─────┘
                         │ milestone ready + item deps satisfied
                         ▼
        ┌────────────────────────────────────┐
        │              READY                 │  ← the only dispatchable state
        └──┬─────────────┬─────────────┬────┘
           │             │             │
  lease    │             │ budget      │ hard dep failed, or
  claimed  │             │ exhausted   │ max_attempts reached
           ▼             ▼             ▼
       ┌────────┐   ┌──────────┐   ┌──────────┐
       │ DOING  │   │ BLOCKED  │   │  FAILED  │
       └───┬────┘   └────┬─────┘   └────┬─────┘
           │            │ dep/lease    │ operator raises
  verify   │            │ clears       │ max_attempts, or
  passes   │            │              │ budget raised
           ▼            └──────┬──────┘
       ┌────────┐              │ resumes
       │  DONE  │◀─────────────┘
       └────┬───┘
            │ operator cancels, or manifest re-opens
            ▼
       ┌──────────┐        ╱ any member (or the human) can invalidate
       │ CANCELLED│       ╱  "this is wrong now" — not a failure
       └──────────┘      ▼
                  ┌────────────┐
                  │INVALIDATED │───→ back to the re-plan pool
                  └────────────┘     attempts NOT incremented
```

**Only `READY` items are dispatchable.**

### 4.4 `invalidated` — the state long horizons need

A long-horizon project constantly discovers that part of its plan is wrong: the vendor changed the format, the assumption about the API turned out to be false, the design decision got reversed. None of that is `failed` (we couldn't do it) or `blocked` (we're waiting). It needs its own state.

| Rule | Behaviour |
|---|---|
| Does not increment `attempts` | Invalidation is a *plan* error, not an *execution* error. Counting it would let a bad plan exhaust every item's retries. |
| Emits `plan.revised` | Re-planning is requested explicitly, not inferred from a pile of failures. |
| Wakes the orchestrator **once per burst** | Five items invalidated by one discovery = one re-plan, not five. |
| Stays visible on the board | The human sees *"the plan changed here"* rather than inferring it from unexplained `blocked` items. |
| Never auto-retried | An invalidated item waits for the orchestrator to replace, rescope, or cancel it. |

This is the difference Grok Bot cannot draw, because it has no plan to invalidate (`00` §5).

### 4.5 Item fields

| Field | Type | Notes |
|---|---|---|
| `id` | str | Stable, sortable, human-quotable (`m2.i7`) |
| `milestone_id` | str | **Required.** Exactly one parent milestone (DM-13) |
| `title` | str | One line, imperative |
| `detail` | str | Full brief passed to the member verbatim |
| `status` | enum | `todo · ready · doing · blocked · done · failed · cancelled · invalidated` |
| `depends_on` | list[str] | Item ids **within the same milestone** (DM-14) |
| `owner_member` | str \| null | Member name (`agents/<name>.json`). `null` → any eligible member may claim. |
| `acceptance` | list[Criterion] | Machine-evaluated where possible (§4.6). Non-empty before `ready` (DM-06). |
| `workdir` | str \| null | **Relative** to the project root (P-1). Defaults to the project root. |
| `attempts` / `max_attempts` | int | `max_attempts` default 3. Untouched by invalidation. |
| `lease_owner` / `lease_expires` | str / ts | Held while `doing` |
| `tags` | list[str] | Used by completion predicates |
| `cost_tokens` / `cost_usd` | int / float | Accumulated across attempts |
| `invalidated_reason` | str \| null | Set when invalidated; survives until the item is replaced |

### 4.6 Acceptance criteria

Each criterion is one of:

| Kind | Evaluated by | Example |
|---|---|---|
| `check` | A named harness check (file exists, glob non-empty, command exit 0, regex on a file) | `check: artifacts/report.md exists and is > 200 bytes` |
| `manual` | Human, via inbox. Item parks until answered. | `manual: does this match the vendor's expected format?` |
| `llm` | A bounded single-call judge with the item's output as input. Recorded in events. | `llm: does this CSV have one row per invoice, no dupes?` |
| `budget` | Always-on. | `budget: total spend < $2.00` |

`check` and `budget` are deterministic and free. `llm` is cheap but non-deterministic, so its verdict is recorded as an event and can be overridden. `manual` is the escape hatch that makes human-gated projects expressible without a special mode.

---

## 5. Members

**A member is a materialised agent file plus a durable chat.** Not a new agent type, and not a live registry lookup.

- `agents/<name>.json` — the agent definition: system prompt, tool bindings, MCP scope, skills, model + thinking profile, compaction budget, per-agent tool caps, permission envelope. Resolved from `WorkspaceSpec` (`agents/workspace_registry.py:31`) or an existing specialist profile at creation, then owned by the project.
- `chat_id` — a project-scoped child chat, giving the member a durable identity, its own transcript, compaction, and resume.

This reuses, for free: session JSONL persistence (`server/session/log.py`), `WorkspaceDeps` per-run isolation (`llm/deps.py:19`), pydantic-ai `FilteredToolset` enforcement, the existing trace UI, and `find_recent_session()` (`server/session/log.py:784`).

### 5.1 Membership declarations

The orchestrator writes these at creation (`03` §2.2). They land in `PROJECT.md` and are materialised into `agents/`.

```yaml
members:
  - name: frontend
    derived_from: developer
    derived_at_version: "2.0.0"
    model: deepseek/deepseek-v4.1-flash
    thinking: high
    tool_caps: { edit_file: 200, run_command: 60 }
    tools: [readFile, editFile, runCommand, searchCode]
    skills: [landing-patterns]
    mcp: []
  - name: copywriter
    derived_from: general
    derived_at_version: "2.0.0"
    model: xiaomi/mimo-v2.6-flash
    thinking: high
    tools: [readFile, editFile, webSearch]
    skills: [brand-voice]
    mcp: []
```

### 5.2 Model & thinking profile

Every materialised member pins its model and thinking profile explicitly, so a folder's behaviour does not drift because someone changed the global mode dropdown.

**Group projects run on the `command` provider** (`llm/provider_modes.py` mode `command` — the OpenAI-compatible Command Code Provider API), with two model slots and `thinking: high` on both:

| Slot | Model | Thinking | Used by |
|---|---|---|---|
| Reasoning / authoring | `xiaomi/mimo-v2.6-flash` | `high` | Orchestrator-tier members, planning passes, copywriting/refinement |
| Execution / building | `deepseek/deepseek-v4.1-flash` | `high` | Workspace-tier members: code, files, commands, tool loops |

This is a deliberate change to the current `command` matrix, where the orchestrator tier runs `xiaomi/mimo-v2.6-flash` at **`think_bias: low`** (`provider_modes.py:76-82`) and the workspace tier inherits `deepseek-v4.1-flash`. Group projects raise the mimo path to high and pin both models per member instead of relying on tier defaults. See `05` §9 for the exact change.

### 5.3 The default member

If `members` is empty or absent, the **orchestrator is the sole member** and executes items directly instead of delegating. One code path; a single-agent project is a project with one member.

This also subsumes the auto-created durable Dev child in `create_project()` (`project_router.py:438`) — the Dev child becomes "the default member," not a separate concept (`05` §4).

### 5.4 Capabilities

Members are ordinary workspace agents. They get no new capability class. A member can still spawn subagents (`task` / `call_subagent`) and still run its own short-lived fan-out (`delegate`) inside its item.

**Members may not widen their own capability set.** `agents/`, `skills/`, `tools/bindings.json`, and `mcp/servers.json` are written by the orchestrator or a human — never by a member. A member that wants a new tool asks; the orchestrator decides (`03` §3). This is the same boundary as the membership ops (`04` §2).

---

## 6. Leases

A lease is an exclusive, expiring claim on a `READY` item.

| Rule | Value | Why |
|---|---|---|
| Default duration | 30 min, renewable | Long enough for a real agent loop |
| Renewal | Member renews while making progress | Detects hangs without hard-killing work |
| Expiry | Item → `BLOCKED` with `reason=lease_expired`, attempt +1 | Recovers from a crashed member |
| Steal | Allowed if `lease_expires` passed 2× duration | Covers a frozen process |
| Force release | Operator op `release_item` | Manual escape hatch |

Leases are what make "N members, shared board" safe without a global lock. Two members cannot work the same item because only one can hold the lease.

---

## 7. Events — the Only Truth

> **The event log is what remembers. The board is only what you look at.**

Append-only, ordered, **mandatory**. This is the per-action audit log that Grok Bot lacks (`00` §5) — and, more importantly, it is the **single source of truth** for all run state. Everything else in `state.db` is a projection of it.

### 7.1 The truth

```sql
CREATE TABLE events (
  seq          INTEGER PRIMARY KEY AUTOINCREMENT,
  project_id   TEXT NOT NULL,
  ts           REAL    NOT NULL,
  kind         TEXT    NOT NULL,
  actor        TEXT    NOT NULL,
  milestone_id TEXT,
  item_id      TEXT,
  data         TEXT
);
CREATE INDEX ix_events_project ON events(project_id, seq);
```

| Field | Notes |
|---|---|
| `seq` | Monotonic. **Total order.** Also `AUTOINCREMENT` so a rolled-back transaction never reuses a seq. |
| `ts` | Unix epoch float — cheap comparisons for lease expiry |
| `kind` | See §7.3 |
| `actor` | `supervisor · orchestrator · member:<name> · human` |
| `milestone_id` / `item_id` | Nullable; item events carry both |
| `data` | JSON blob |

Rows are **never updated and never deleted.**

### 7.2 The projections

```sql
CREATE TABLE projects    (id, owner, status, budget_usd, spent_usd, deadline, completion_json, …);
CREATE TABLE milestones  (id, project_id, title, status, seq_no, …);
CREATE TABLE items       (id, project_id, milestone_id, title, status, attempts, lease_owner,
                           lease_expires, acceptance_json, …);
CREATE TABLE approvals   (id, project_id, item_id, tier, payload_json, idem_key, state, …);
CREATE TABLE projection_checkpoint (project_id PRIMARY KEY, last_seq INTEGER);
```

**Rules that make this safe:**

| Rule | Why |
|---|---|
| The supervisor appends an event **and** updates the projection in **one transaction** | The projection is never behind, and never ahead of, the log |
| A projection is **never** a source for a decision the log contradicts | The log wins |
| On boot, every projection is **rebuilt from events** via `rebuild(project_id)` | A crash mid-transaction, a manual DB edit, or a bug cannot leave a wrong board |
| `projection_checkpoint.last_seq` records the last applied seq | Boot rebuilds only the **tail**, not all history |

`leases` live inside the `items` projection rather than their own table: expiry is time-based, so the supervisor updates it incrementally each tick, and `rebuild()` recomputes it by pairing `item.claimed` with `item.released` / `item.lease_expired`.

**The rebuild is tested, not assumed** (DM-16): a test drops every projection table, calls `rebuild()`, and asserts the result matches the pre-drop state. This is the single most valuable test in the feature, because it is what proves the log is actually sufficient.

### 7.3 Event vocabulary

Getting this list right is now a real commitment: old projects replay through it forever, so a missing `kind` is a permanent hole in the history.

```
project.created · project.resumed · project.paused · project.closed · project.stalled

milestone.created · milestone.ready · milestone.doing · milestone.done · milestone.failed

item.created · item.ready · item.claimed · item.released · item.progress
item.done · item.failed · item.blocked · item.invalidated · item.cancelled

plan.revised · idea.proposed

op.submitted · op.applied · op.rejected

approval.requested · approval.granted · approval.rejected

message.sent · message.read
lease.expired · budget.warn · budget.stop · deadline.reached
member.added · member.removed · member.halted
capability.changed · materialised.derived
```

Every state transition in §4.1 and §4.3 has exactly one corresponding event kind. That correspondence is asserted by a test — the same discipline as `fixed-concept.md` §6.

### 7.4 Why this matters for long horizons

| Without events-as-truth | With it |
|---|---|
| Changing the status vocabulary later = data migration on every project | Change the projection; the log already has the facts |
| Adding milestones later = backfill guesswork | Rebuild from `item.created` events, grouping by whatever rule the new code has |
| A corrupted row is permanent corruption | Rebuild the row |
| "What happened while I slept?" = read the table | Replay the log, with actor + seq + ts on every entry |
| New views (burndown, Gantt, cost-per-milestone) need new queries over mutable state | Same — but the query is stable, because the log is append-only |

**The cost, stated plainly:** the event vocabulary is now an API you cannot break, and `rebuild()` is O(events) unless the checkpoint is used. Both are worth it — but the vocabulary deserves a design pass before implementation, not after.

Retention: keep all state-transition events forever. Tool-level `tool.*` events, if added later, are capped at N per item.

---

## 8. Messages

| Field | Notes |
|---|---|
| `from` | Member alias or `human`/`orchestrator` |
| `to` | Member alias, or `null` for broadcast |
| `body` | Text |
| `in_reply_to` | Optional message id |
| `item_id` | Optional — ties the message to work |
| `read_at` | Nullable — so a wake knows what's new |

**Delivery model:** a message is *not* injected until the recipient next wakes. Broadcasts land in every member's inbox; each member decides on wake whether to read them. No member ever reads another member's transcript.

**Why not a shared group transcript:**

| Property | Shared transcript | Point-to-point + board |
|---|---|---|
| "What's left?" | Re-read everything | `SELECT … WHERE status != 'done'` |
| Token cost per member | Grows with team size × history | Grows with own items only |
| Convergence | Unreliable — agents argue | Enforced by DAG + statuses |
| Determinism | None | Read-only projection |
| Human view | Natural | Rendered, slightly lossy |

Chat is a **view** (see `05` §8 for the UI), not the transport.

---

## 9. Context Files

| File | Contract |
|---|---|
| `context/decisions.md` | Append-only ADR-style. `## <date> — <decision>` + what was chosen, what was rejected, why. Read by every member at wake; never auto-rewritten. |
| `context/findings.md` | Research results with citations. Append-only. Source of truth for facts a member discovered. |
| `context/artifacts/` | Real outputs: `.xlsx`, `.md`, `.csv`, code. The folder the autonomy envelope trusts. |
| `inbox.md` | `## PENDING APPROVAL` + `## OPEN QUESTIONS` blocks. Harness appends; human answers. Feeds `approve`/`reject`/`answer` ops. |

Members are told: **write down what you learned, not just what you concluded.** A member that discovers something and dies loses it; a member that appends it to `findings.md` leaves it for the group.

---

## 10. The Manifest (`PROJECT.md`)

Authored. H1 + prose for humans, fenced YAML for the harness. This is the document that answers "what's inside a group project" at the *intent* level.

```markdown
# Landing page for the coffee business

## Why
New business, no site. Need a landing page that matches the brand voice
and passes the accessibility check before launch.

## Done when
- [ ] `artifacts/index.html` builds and renders
- [ ] Brand voice matches `skills/brand-voice/SKILL.md`
- [ ] Lighthouse accessibility >= 90
- [ ] Copywriter and frontend agents have signed off in `context/decisions.md`

## Milestones
- m1  Research          — understand the product and the audience
- m2  Brand & copy      — depends on m1
- m3  Build             — depends on m2
- m4  Accessibility     — depends on m3
- m5  Ship              — depends on m4

## Members
- frontend   — builds the page  (deepseek v4.1 flash, high thinking)
- copywriter — brand voice      (mimo v2.6 flash,     high thinking)

## Budget
- tokens: 2_000_000
- usd: 4.00
- deadline: 2026-10-05T17:00:00

## Autonomy
project_folder: autonomous
external_read:  autonomous
external_write: needs_approval
destructive:    blocked

## Environment
agents:   [frontend, copywriter]
skills:   [brand-voice, landing-patterns]
mcp:      []
deps:     []
tools:
  frontend:   [readFile, editFile, runCommand, searchCode]
  copywriter: [readFile, editFile, webSearch]
---
```yaml
schema: 1
id: landing-page-coffee
owner: localhost            # host id, never a path (P-3)
provider:
  mode: command
  slots:
    reasoning: { model: "xiaomi/mimo-v2.6-flash",       thinking: high }
    execution: { model: "deepseek/deepseek-v4.1-flash", thinking: high }
milestones:
  - { id: m1, title: Research }
  - { id: m2, title: Brand & copy,   depends_on: [m1] }
  - { id: m3, title: Build,          depends_on: [m2] }
  - { id: m4, title: Accessibility,  depends_on: [m3] }
  - { id: m5, title: Ship,           depends_on: [m4] }
completion:
  kind: all_milestones_done
  also_require: [no_item_status_failed, no_item_status_invalidated]
idempotency:
  external_write_key: "deploy:{env}:{sha}"
```

**Milestones are optional.** Omit the `milestones:` list and the project gets one implicit milestone named `all work` (BR-11) — a three-item project never thinks about them.

**The `Environment` block is the orchestrator's materialisation decision**, recorded as intent and then written into the folder as `agents/`, `skills/`, `tools/bindings.json`, and `mcp/servers.json` (§2). The orchestrator proposes it in `plan_execution` and the human confirms with the rest of the plan.

### 10.1 Done-conditions

The `## Done when` prose is **authored intent for humans**. The `completion:` block is the **machine-evaluated predicate** the supervisor enforces. They must agree; the supervisor reads only the YAML.

Supported predicates:

| `kind` | Passes when |
|---|---|
| `all_milestones_done` | Every milestone is `done` |
| `all_items_done` | No item is `todo`/`ready`/`doing`/`blocked`/`invalidated` |
| `+ also_require` | The above, plus named predicates — e.g. `no_item_status_failed`, `no_item_status_invalidated` |
| `deadline` | The wall-clock deadline passed (project auto-closes) |
| `tag_threshold` | `count(done WHERE tag = X) >= N` |
| `budget` | Spend cap reached (project auto-closes) |
| `manual` | A human sets `done` from the inbox |

Note that `no_item_status_invalidated` in `also_require` means **"the plan was revised and every replacement succeeded."** Without it, a project can close while invalidated items are still sitting unresolved.

**An idle project never runs forever.** If N consecutive ticks (default 50 ≈ 12 min at 15 s) produce zero events, the supervisor emits `project.stalled`, halts dispatch, and raises an inbox question. A project with no termination predicate and no idle guard is a money leak.

---

## 11. Contracts

### Data model

| ID | Requirement | Priority | Acceptance criterion |
|---|---|---|---|
| DM-01 | Run state persists only in `.gp/state.db`; `.md` files hold authored intent only | Must | Given any harness restart, Given `state.db` present, When the supervisor loads, Then all item/milestone/member/lease state is restored with no `.md` parsing |
| DM-02 | The supervisor is the only writer of `state.db` | Must | Given an orchestrator or member process, When it attempts a state write, Then the write is rejected or queued as an `op` |
| DM-03 | Every state transition appends exactly one event with a monotonic `seq` | Must | Given a full item lifecycle, When events are read, Then they reconstruct the item's current status |
| DM-04 | Dependencies are acyclic, at every level | Must | Given an op adding a milestone→milestone or item→item dependency, When it would form a cycle, Then the op is rejected with `err_cycle` |
| DM-05 | An item is dispatchable iff `status='ready'` **and** its milestone is not `done`/`cancelled` | Must | Given any item, When the supervisor selects work, Then only `ready` items in a live milestone are eligible |
| DM-06 | `acceptance` is non-empty on every item before it can reach `ready` | Must | Given an item with empty `acceptance`, When it would become `ready`, Then it stays `todo` with `err_no_acceptance` |
| DM-07 | A member is a valid `agents/*.json` file in the project folder, with a resolvable runtime and a unique name | Must | Given a member declaration missing its agent file, When the project loads, Then it is rejected with `err_member_unmaterialised` |
| DM-08 | Only one lease per item; `lease_owner` is unique while held | Must | Given a held lease, When a second member claims, Then the claim returns `err_lease_held` |
| DM-09 | Every materialised `agents/*.json` records `derived_from` + `derived_at_version` | Must | Given a materialised agent, When inspected, Then its provenance is readable without the registry |
| DM-10 | `mcp/servers.json` references credentials by env-var **name** only | Must | Given the materialised MCP config, When scanned for values, Then no secret value is present |
| DM-11 | Members cannot write `agents/`, `skills/`, `tools/bindings.json`, or `mcp/servers.json` | Must | Given a member attempting a capability write, When attempted, Then it is rejected — the member asks, the orchestrator decides |
| DM-12 | `events` is the **only** source of truth; every other table is a rebuildable projection | Must | Given any projection table, When dropped, Then `rebuild(project_id)` reconstructs it from events alone |
| DM-13 | Every item belongs to exactly one milestone | Must | Given a project with zero milestones, When created, Then one implicit milestone `all work` exists and owns every item |
| DM-14 | Dependencies are declared at **one** level only | Must | Given an item in `m3` depending on an item in `m1`, When the op is submitted, Then it is rejected with `err_cross_milestone_dep` |
| DM-15 | `invalidated` never increments `attempts` and always emits `plan.revised` | Must | Given an invalidated item, When its projection is read, Then `attempts` is unchanged and a `plan.revised` event exists |
| DM-16 | Every projection has a tested `rebuild()` | Must | Given a live project, When all projection tables are dropped and `rebuild()` runs, Then the rebuilt state equals the dropped state, field for field |
| DM-17 | `projection_checkpoint.last_seq` bounds the rebuild window | Must | Given a project with 10 000 events and `last_seq = 9800`, When `rebuild()` runs, Then it replays 200 events, not 10 000 |
| DM-18 | Every path stored in any project file is relative | Must | Given the project folder, When scanned for absolute paths (`^[A-Za-z]:\\` or `^/`), Then none is found outside `lock.json` digests |
| DM-19 | The event vocabulary covers every state in §4.1 and §4.3 | Must | Given the state enums and the `kind` list, When asserted, Then every state has exactly one corresponding `kind` |

### Business rules

| ID | Condition | Action | Exception |
|---|---|---|---|
| BR-01 | An item's dependencies are all `done` **and** its milestone is `ready`/`doing` | Set `ready`, clear `blocked_reason` | Any dep `failed` → stay `todo`, reason `dep_failed` |
| BR-02 | `lease_expires` passes | Set `blocked`, `attempts += 1`, reason `lease_expired` | `attempts >= max_attempts` → `failed` |
| BR-03 | All acceptance criteria pass | Set `done`, release lease, append event | Any `manual` criterion pending → stay `doing`, request approval |
| BR-04 | Budget or deadline reached | Emit `budget.stop`/`deadline.reached`, halt dispatch, close project | Operator raised the cap → resume |
| BR-05 | Zero events across N ticks | Emit `project.stalled`, halt dispatch, inbox question | Human resumed → continue |
| BR-06 | An op is submitted | Queue it; apply on the next tick in submission order | Invalid → `op.rejected` event, project unaffected |
| BR-07 | A member requests approval for an `external_write` | Pause that item, append to `inbox.md`, emit `approval.requested` | Project closed → reject |
| BR-08 | A human approves a pending approval | Re-check idempotency key, perform write once, record key | Key already used → `err_duplicate`, skip write |
| BR-09 | Every item in a milestone is `done` | Set the milestone `done`, emit `milestone.done` | Any item `invalidated` → milestone stays open, re-plan |
| BR-10 | A milestone's dependencies are all `done` | Set the milestone `ready`, then evaluate its items | Any dep `failed` → milestone `failed`, cascade to its items |
| BR-11 | A project is created with no milestones | Create one implicit milestone named `all work`, owning every item | — |
| BR-12 | An item is invalidated | Release the lease, set `invalidated` + reason, emit `plan.revised`, **`attempts` unchanged**, wake the orchestrator | More invalidations within one burst → coalesce into a single wake |
| BR-13 | An invalidated item has sat unresolved past the re-plan window | Emit `plan.revised` again, escalating to the human inbox | Orchestrator replaced it → resolved, no escalation |
| BR-14 | A member or the human discovers new work mid-flight | Emit `idea.proposed` and add an item to the current milestone | Orchestrator declines → `item.cancelled`, no propagation |

---

## 12. Resolved decisions

| Decision | Rationale | Alternatives considered |
|---|---|---|
| SQLite per project, not a shared DB | Portable: the folder is the only cloud-migratable artifact. Isolates blast radius. Matches the existing raw-`sqlite3` house style (`chat_store.py:45`). | Shared `chats.db` tables (couples project lifetime to harness lifetime; single point of failure); Postgres (a real migration away from raw sqlite3 for no v1 gain) |
| Authored `.md` + run-state SQLite | Compaction-safe; corruption-isolated; human-diffable intent | Everything in SQLite (opaque to humans); everything in `.md` (loses multi-day work to one compaction) |
| Board, not chat | Deterministic "what's left"; no token blowup; enforces convergence | AutoGen-style group transcript (agents argue, don't converge); chat + shared plan (same problem) |
| **`events` is the only truth; tables are projections** | Changing the model later costs no migration; a corrupted row is repairable; new views need no new queries over mutable state | Tables as the record (simple writes, but every model change is a real migration; corruption is permanent) |
| **Rebuild-on-boot, bounded by `last_seq`** | A crash or a manual edit cannot leave a wrong board, and boot is O(tail) not O(history) | Trust the tables on boot (faster, but then the log is decorative rather than authoritative) |
| **Milestones between project and item** | A flat list stops being readable past a few dozen items; milestones carry phase ordering and give a real progress signal | Flat items only (unreadable at scale); free-form tags (no structure, no ordering) |
| **Deps at one level only** | The coarse plan is milestone-level, the fine detail item-level. A mixed graph is unreadable | Mixed item-level deps across milestones (exact how long-horizon plans become unfollowable) |
| **`invalidated` as a distinct state** | A discovered-wrong plan is not a failure and not a block; counting it as either exhausts retries or hides the change | No such state (shows up as unexplained `blocked` or as `failed`); `done` with a note (loses the signal entirely) |
| `READY` as the single dispatchable state | One place to reason about scheduling; readiness is derived, not stored | Multiple ready-ish states; storing readiness (goes stale the moment a dep moves) |
| Leases, not locks | Members are long-running and may hang; expiring claims self-heal | Advisory locks (die with the process); no coordination (double work) |
| *Superseded:* members as a live `(scope, chat_id)` registry lookup | Zero new agent types; reuses transcripts, deps, ACLs, UI | A new "group agent" tier (new registry, new runtime, new UI — all for no new capability). Replaced by materialisation — the runtime is still reused, but via `derived_from` |
| Manifest carries the autonomy envelope | Declared blast radius; reviewable before work starts | Global config (not project-scoped); per-action inference (Dots' approach — costs an inference per action, unpredictable) |
| Materialise capabilities into the folder | The folder becomes the unit: zippable, movable, per-project agent versions | Live registry lookup (less duplication, but the folder means nothing outside your machine) |
| Members pin model + thinking explicitly | A folder's behaviour cannot drift because someone moved the global mode dropdown | Inheriting tier defaults from `MODE_MATRIX` |
| **Relative paths only (P-1)** | The single most common reason a folder won't move between hosts | Absolute paths with rebasing (rebasing is a bug waiting to happen) |
| **Project-local `deps/`** | "Install what the task demands" without mutating the harness, and the install is portable | Installing into the harness env (a project breaks the next one) |

---

## 13. Open — Copy Boundary

The folder materialises agents, skills, tool bindings, and MCP config (§2). **How much of the harness itself travels with the folder is undecided.**

| Option | Folder carries | Gets harness bug fixes | Self-contained with no harness |
|---|---|---|---|
| **A. Declarations only** *(recommended)* | `agents/`, `skills/`, `tools/bindings.json`, `mcp/servers.json`, `lock.json` | **Yes** | No — needs a pinned runtime |
| B. Full copy | Everything incl. `tools/`, `llm/`, `agents/` | **No** — one fork per project | **Yes** |
| C. Orchestrator's choice per project | Default A; vendor a frozen copy when a project must outlive harness changes | Depends per project | Depends per project |

The analogy that settles it either way: nobody copies `node_modules` into a project — they copy `package.json` + `package-lock.json` and install. **Declarations copy, dependencies install, the runtime pins.** `lock.json` (harness version + content digests) is what makes a folder reproducible rather than merely self-describing.

**This blocks nothing before Phase 3.** Phase 0 (durability), Phase 1 (supervisor), and Phase 2 (real members) are identical under A, B, and C. Decide before Phase 3.