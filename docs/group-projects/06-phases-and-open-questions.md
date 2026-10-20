# Group Projects — Phases & Open Questions

**Version**: 1.0
**Date**: 2026-10-01
**Author**: Stark design session
**Status**: Draft — phases unstarted; §2–§5 are live questions
**Depends on**: `05-integration-map.md`

Build order, gates, and everything still undecided. **§2–§5 are living sections** — they are meant to change as decisions are made.

---

## 1. Phase Plan

Each phase has a **test project** that must pass before the next begins. Test projects are synthetic: a real one would contaminate the design with a domain we do not understand yet.

---

### Phase 0 — Durability foundation  ✅ BUILT

**Full spec: `07-phase-0-implementation.md`, now including a build record (§11).** This is the only phase with its own implementation document, because it is the only one that touches code which exists today.

**Status: done — gate 4/4.** Three deliverables shipped; the four gate assertions pass; the same 46 pre-existing test failures throughout (pytest-fixture tests the zero-arg runner cannot drive, unrelated to Phase 0). One design refinement was needed — `entry_json`, the fidelity payload (`07` §4.6) — and one pre-existing test-infrastructure bug was found (`07` §11.3, finding 4).

| | |
|---|---|
| **Scope** | Steps 1–2 of `05` §13. Three deliverables: (1) `JobStore` in a new `.deep_agents/jobs.db`; (2) `_DELEGATIONS` (`delegation.py:44`) becomes a read-through cache; (3) `run_task_background` (`task_scheduler.py:147`) sets a scoped `active_state`. |
| **Why first** | Everything else assumes durable job state. `_DELEGATIONS` is process-local — its own comment says entries *"survive until process exit"* (`:43`) — and `start_task_scheduler` spawns a thread per due task (`:204`), so two tasks in one tick already publish each other's reports. |
| **Test project** | *None.* This phase fixes the existing system; it is not a step toward group projects. |
| **Gate** | Four assertions (`07` §8): no regressions; a job survives `kill` + restart; **two concurrent tasks produce independent reports**; and `_DELEGATIONS.clear()` followed by `status_table()` still answers from the store. |
| **Effort** | Small — three contained changes, one new module, one new table. |
| **Not in scope** | No supervisor, no projects, no `events`, no UI. See `07` §9. |

**Assertion 3 should be written first, against unfixed code.** It is a live bug that is silent in production and looks like a model failure; a test that fails loudly is the only honest way to prove it existed.

**Correction to an earlier claim in this document set.** `05` §5 called the `state_sync` read *"the highest-risk item in the whole integration."* Reading the code showed the scoping mechanism already exists and is proven at `delegation.py:622-627` — the scheduler simply never sets it. High **consequence**, low **effort**, and fixed in Phase 0 rather than deferred.

---

### Phase 1 — Supervisor without an LLM  ✅ BUILT

**Build record: `02-runtime.md` §12.** The event vocabulary (D-13) was settled first, as this section required, and is now frozen code in `agents/group/events.py`.

**Status: done — gate met.** `pytest tests/ -q` → **198 passed, 0 failed** (+15 over Phase 0). The synthetic ledger completes; a deadlocked project stalls rather than spins; projections provably rebuild from the log. Two real bugs were found and fixed by the tests (the budget gate could never fire; milestone completion lagged a tick), and one over-claim in `02` §3.0 was corrected.

**What Phase 1 deliberately does not do:** `dispatch` is inert — it never claims an item without a runner, because claiming work it cannot run is worse than leaving it ready. Phase 2 supplies the runner.

---

### Phase 2 — Real members  ✅ BUILT

**Build record: `01-concept-model.md` §14.**

**Status: done — gate met.** `pytest tests/ -q` → **211 passed, 0 failed** (+13 over Phase 1). A project folder is created, its capability set is materialised into it, members run items through an injectable runner, and acceptance criteria decide done.

**Two real bugs were found, both latent since earlier phases:** nothing requeued a lease-expired item (it stayed `blocked` forever — Phase 1 never noticed because it had no members to die), and a `manual` criterion caused a false stall because the wait detector checked status instead of reason.

**Honest limit:** members are exercised with a **fake runner**. The suite covers dispatch, harvest, criteria, and transitions; it does **not** cover a real LLM run. `LocalRunner._execute_node` is the one seam that would call a model.

---

### Phase 3 — Discoverability  ✅ BUILT

**Build record: `03-orchestrator-tools.md` §12.** The copy boundary this phase was gated on is settled at `01` §13 — **option A**, decided by what Phase 2 built.

**Status: done — gate met.** `pytest tests/ -q` → **229 passed, 0 failed** (+18 over Phase 2). Routing is a tool call with a rationale and a deterministic classifier (12/12 on the case set, negatives included); `@task` forces without re-routing; capability changes reach the folder and only their own project.

**A real bug was caught by the gate test:** the portability check's Windows-path regex matched the `s:/` inside `https://`, so *every* project with an MCP server would have been rejected as non-portable. Also fixed: `_verify` demanded a runner it did not need, and `CAPABILITY_CHANGED` was writable but not reducible.

**Honest limit:** the routing test asserts the deterministic **classifier**, not a model's choice — a routing test that needed a model would not be run. The LLM planner is wired and inspected, not asserted.

---

### Phase 4 — Surface  🔨 INTEGRATION DONE, UI DEFERRED

**Build record: `05-integration-map.md` §15.**

**Status: wired and running.** `pytest tests/ -q` → **245 passed, 0 failed** (+16). The registry, the scheduler, the boot wiring and the tool registration all landed — group projects are now reachable from the running app. A **real project has been created and completed on a real model** (`D:\StarkProjects\hello-report`).

**The real run found three bugs that 229 passing tests could not** (`05` §15.4): nothing dispatched a memberless project, the budget ceiling could never fire, and a member working silently was counted as a deadlock. All three were invisible to a suite that never ran the thing.

**Deliberately deferred by decision:** the frontend board, the approvals panel, Telegram approval. `05` §11 remains the design.

---

### Phase 5 — Beyond v1 (not scheduled)

| Item | Why deferred |
|---|---|
| Curated MCP install catalogue | Requires an approval UX that Phase 4 delivers (`04` §6) |
| Skill distillation from trajectories | Needs enough completed items for the logs to be worth mining |
| Routines (recurring items) | `scheduled` shape is already served by `task_scheduler.py`; do not duplicate |
| Per-project containers / real sandbox | `06` §3 |
| Cloud deployment (worker host) | `06` §2 — concept documented, v1 is local |

---

## 2. Concept — Cloud Deployment

**Status: concept only. Not the current focus. v1 is local; the provider is undecided.**

### 2.1 What "the cloud" means here

The cloud is **where group project folders live and run.** Not a relocated copy of Stark — nothing moves, and the local harness keeps chat, voice, House, DFD, routing, and the frontend exactly as they are.

A group project folder is self-contained (`01` §2): its agents, skills, tool bindings, MCP config, tasks, context, and run state all sit inside it. The orchestrator creates that folder *in the cloud* and decides what goes into it. **That is the entire extent of the cloud concept.**

```
orchestrator  ──creates──►  cloud: projects/landing-page-coffee/
                                ├── agents/    2 chosen agents
                                ├── skills/    chosen skill content
                                ├── tools/     chosen bindings
                                ├── mcp/       chosen servers (creds by name)
                                ├── .gp/       state.db · plan · runs
                                └── context/   findings · decisions · artifacts
```

### 2.2 Why the cloud buys exactly two things

**Cloud does not make agents smarter.** Same pydantic-ai loops, same MCP bridge, same tools, same models. It buys:

1. **Availability** — work continues while your laptop is closed.
2. **Blast radius** — a real boundary, so `autonomy: blocked` means something and installs do not pollute the dev machine.

Dots and Grok Bot ship cloud VMs because they are *selling a computer 24/7*. Not because a local agent cannot do the work.

### 2.3 What actually has to run there

Not the harness — the **agent runtime**. The box needs the supervisor, member execution, the MCP bridge, and the provider layer. It does **not** need voice (`sounddevice`, `moonshine-voice`, `pocket-tts` — no audio device), Electron, the SPA build, House, DFD, `routing/router.py`, or `torch`.

The repo already supports this: it has several entrypoints that bypass the app (`run_general_direct()`, `stream_direct()` at `project_router.py:637`, `_build_del_graph()` at `delegation.py:1289`). A headless worker-host entrypoint is idiomatic, not a refactor.

| Already portable | Why |
|---|---|
| `_find_bash()` `_shellenv.py:38` | Windows candidates, **then** `shutil.which("bash")` at `:69` — `/bin/bash` passes the system32 filter |
| `_resolve_venv_scripts()` `:78` | Loops `Scripts` **and** `bin` (`:101`) |
| Credential injection | Already per-call via `needs_credentials()`, deliberately kept out of `BASH_ENV` — the right cloud model |
| 3 of 6 MCP servers | `github`, `render` are remote HTTP by URL; the 3 stdio ones are all `npx -y` |
| Excel | `openpyxl` already in `requirements.txt` |

| Needs work | Where |
|---|---|
| Headless mode | `STARK_HEADLESS=1` — skip Telegram poller (`api_server.py:589`), voice, SPA mount, warm-up (`:579`) |
| Windows paths | `STARK_PROJECTS_ROOT` (`05` §4.3) plus `chat_store.py:18-20`, `workspace_registry.py:14` |
| Secrets as env vars | Not `.env` on disk. You already reference MCP credentials by name — that model extends. |
| Supervisor start-up | Must not be `__main__`-only (`05` §10) |

### 2.4 Host requirements (not a recommendation)

Any host that meets these works. **No provider is chosen.**

| Requirement | Value | Why non-negotiable |
|---|---|---|
| **Persistent disk** | `STARK_PROJECTS_ROOT` on a real block volume | An ephemeral disk loses the project. This alone rules out container layers and HF Spaces' 50 GB non-persistent disk. |
| **No forced sleep** | — | A supervisor nobody visits must keep running |
| **No idle reclamation** | — | A supervisor is idle by design; anything that reclaims idle instances will reclaim it |
| **RAM** | 2 GB minimum, 4 GB comfortable | 2-4 concurrent members. 4 GB is the community consensus floor. |
| **vCPU** | 2 | Agent loops are I/O-bound; this is ample |
| **Disk** | 40 GB | Project folders + artifacts |
| **Service manager** | `systemd` or equivalent | The supervisor must survive a crash and reboot. **Docker is not required** — plain Python + `systemd` is simpler. |
| **Reachable** | Tailscale/WireGuard, or Telegram-only | For approvals while away |

Free-tier options researched, with their honest limits:

| | RAM | Disk | Sleeps | Idle reclaim | Verdict |
|---|---|---|---|---|---|
| **Oracle A1.Flex** | **12 GB** | **200 GB** | No | **Yes, 7 days** | Best free spec by far; capacity is a gamble ("out of capacity" is common); reclaim is recoverable — you press start and `state.db` resumes the project mid-flight |
| Oracle E2.1.Micro | 1 GB | persistent | No | Yes | Guaranteed capacity; 1 GB is tight |
| GCP e2-micro | 1 GB | 30 GB persistent | No | **No** | Most reliable free option; 1 GB tight, shared core |
| HF Spaces CPU Basic | 16 GB | **ephemeral** | **48 h, fixed** | — | **Ruled out**: ephemeral disk loses the project, and the sleep timer is not configurable on cpu-basic |

**Oracle's 7-day idle reclamation is survivable by design, not by luck** — that is what `02` §5 resume is for. It does break one thing: you cannot answer an approval while the host is reclaimed, which is exactly when you want to. That is the whole of the cost.

Paid, if you would rather not think about it: the open-source always-on agent community converges on **Hetzner** (2 vCPU / 4 GB, €3.79-5.83/mo) as the default, with Contabo cited for RAM-per-euro. Not chosen here — just recorded because the research was done.

### 2.5 Ownership: one host, no fencing

Only group projects live in the cloud, so only the cloud ticks them. **There is exactly one supervisor per project and no handoff to protect.** No epoch counters, no fencing tokens, no migration protocol.

The only guard needed:

> A project records `owner` in `state.db`. The supervisor ticks only if it is the owner.

During development `owner = localhost`, so v1 works exactly as designed. When a cloud host exists, `owner` is that host, permanently. Projects do not *move* — they are *created* on whichever host will own them.

**Never rsync a live SQLite file.** WAL mode (`chat_store.py:45`) plus a network filesystem or a mid-write copy corrupts. The only sync is a one-shot copy at creation, before the supervisor starts.

### 2.6 The local focus, stated plainly

v1 and every Phase 0-4 gate are **local**. The cloud is not a prerequisite for any of them, and nothing in the design changes when it arrives:

- Group projects are created in `D:\StarkProjects\` and run in-process.
- The same code runs later with `STARK_PROJECTS_ROOT` pointed at a volume.
- Moving a project later = copy the folder, start the executor.

The one thing Phase 0 buys regardless: fixing the process-global `state_sync` read (`05` §5) is what makes concurrent members safe *and* what makes a second host possible. Same fix, both benefits.

### 2.7 Cost reality

| | |
|---|---|
| Compute | Free tier or €4/mo |
| Inference | `llm/free_providers.py` (1,296 lines) routes across free providers with per-provider cooldowns, driven by `config/free_catalog.json`. Your `.env` also carries DeepSeek, OpenRouter, Groq, Gemini, Mistral, NVIDIA, Cerebras, and more. |

Two consequences, both already in the design:

- **A 24/7 loop exhausts free-tier caps in days.** The budget ceiling (`01` §10.1) and the idle-stall guard (`02` §6.2) go from decorative to load-bearing.
- **Rate limits are per-IP.** A cloud host's IP is a fresh datacenter address — possibly less contended than your home line, possibly throttled harder. The open-source agent community documents provider IPs being rate-limited specifically. Measure on day one; keep a paid provider as fallback.

---

## 3. Open Question — Sandbox vs. Declared Envelope

**Status: deferred. Not required for v1.**

The project's permission envelope (`04` §7) is a **declaration**. Without a real sandbox it is a **courtesy**. In-process, a member that ignores its instructions can still write outside the folder — `enforce_fs_permission()` (`_runtime.py:88`) is a guard, not a jail.

**Question:** does the envelope need to be a real boundary before the feature is trustworthy enough to hand to someone else?

**Interim answer:** no, for personal use on your own machine. Yes, before any hand-off.

Mitigation that already exists: the `never` tier removes tools from the materialised allowlist entirely (`04` §2), which is stronger than blocking at runtime. But it does not constrain a member holding `run_command` — a shell can reach anywhere. That makes the next question load-bearing.

**Open sub-question:** should `run_command` be `never` by default, with `blocked` as an opt-in for projects that need builds and tests? This is a real fork and is not yet decided. Leaning default-`never`, because a member with a shell can defeat every other tier.

**If a real sandbox is wanted later:** one container per project, with the folder bind-mounted and the folder's `mcp/servers.json` credentials injected inside it. Note this reverses a recorded decision — `.opencode/plans/run-command-batch-abc.md:12` explicitly rejected Docker/sandboxing — so it deserves its own spec, not a footnote here.

---

## 4. Open Questions — Design

**Rewritten after Phase 4**, because the old table said "Blocks: Phase 2 / Phase 3" for phases that are now built and shipped. A blocker that silently stopped blocking is worse than no row at all — it reads as *handled* when it was merely passed. Each row now carries a **Status** instead.

| # | Question | Status |
|---|---|---|
| D-1 | Is `run_command` `never` or `blocked` by default? (`06` §3) | **OPEN.** Never implemented. Does not bite while projects have no members; bites the moment one does, because a member with a shell defeats every other autonomy tier. |
| D-2 | Per-member cumulative conversation vs per-item? | **RESOLVED BY BUILD.** Cumulative — `ProjectContext.member_chat_id` is deterministic per member, so continuity is the feature and `ProcessHistory` handles growth. |
| D-3 | Is the orchestrator notified of failure streaks? | **PARTIAL.** It wakes on `item.failed` (exhaustion) and `item.blocked`, which are both in the wake set. It is **not** notified of the *first* failure — a retry emits `item.ready`, which is not a failure signal. `03` §7 specifies both; only exhaustion is wired. |
| D-4 | Are `llm` verdicts advisory or final? | **RESOLVED, differently from the lean.** A failing verdict is final and triggers the normal retry ladder, not an advisory note. The override path is `group_update(invalidate_item)` — the orchestrator can reject a verdict, but has to say so explicitly rather than have the verdict auto-soften. |
| D-5 | Stall threshold: is 50 idle ticks right? | **RESOLVED.** `STALL_TICKS = 50`, injectable per supervisor. Exemptions: invalidated, parked, and **an item `doing` with a live lease** — the last one added after the first real run, where an item worked silently for 22 ticks. |
| D-6 | Are `context/decisions.md` and `findings.md` per-item or per-project? | **RESOLVED BY BUILD.** One per project, appended with a timestamp and optional source; the item id lives in the entry when relevant. |
| D-7 | Inline the acceptance criteria, or reference them? | **RESOLVED BY BUILD.** Inlined (`runner.build_prompt`). A member that has to fetch its own success criteria will rationalise. |
| D-8 | Does `plan_execution` need the environment inventory? | **RESOLVED BY BUILD.** It receives the agent, skill and MCP catalogues, so a plan only proposes things that exist. |
| D-9 | Telegram approval: inline command or fetched summary? | **OPEN — deferred with the surface.** Lean unchanged: a fetched summary, because approving blind is how mistakes happen. |
| D-10 | Is the completion predicate ever LLM-judged? | **RESOLVED.** No. `llm` criteria are per-item; the project predicate stays deterministic (`evaluate_completion`). |
| D-11 | How many projects may be live at once? | **OPEN.** No cap. Irrelevant for one project, relevant the moment there are two — the slot semaphore is still per-delegation, not per-project. |
| D-12 | What happens to a removed member's in-flight item? | **RESOLVED BY BUILD.** `remove_member` releases the lease and blocks the item with reason `reassignment`. Never silently reassigned. |
| ~~D-13~~ | ~~Is the event vocabulary final?~~ | **RESOLVED, THEN AMENDED BY AU-05.** Frozen in `agents/group/events.py`; the state↔kind correspondence is asserted (DM-19), and replaying an unimplemented kind raises `UnknownEventKind` rather than ignoring it. AU-05 added `write.reserved`, `write.done` and `write.skipped` as audit-only kinds (the ledger is derived by scanning events, not projected; the reducer treats them as no-ops like `capability.changed`). Renaming any kind remains forbidden. |
| D-14 | Invalidation re-plan window before escalating to a human? | **OPEN.** Bursts are coalesced (RT-14) but never escalate — an invalidated item that nobody resolves sits there. The stall exemption makes it quiet, which is exactly the risk. |
| D-15 | Does an invalidated item's successor reuse the id? | **RESOLVED BY CONVENTION.** New id. No successor is ever created automatically; the orchestrator supplies one. |
| D-16 | Should milestone `acceptance` exist? | **OPEN — half-built.** The column exists and round-trips, but nothing evaluates it. Milestones complete on "all items done" only. |
| D-17 | Do milestones ever reopen? | **RESOLVED BY ABSENCE.** There is no reopen path. A `done` milestone is terminal; reopening means a new milestone depending on it. |
| D-18 | Should the events log be capped or rotated? | **OPEN — Phase 5.** Unbounded today. Correct for state transitions; would matter for tool-level events, which do not exist yet. |

---

## 5. Open Questions — Process

| # | Question | Status |
|---|---|---|
| P-1 | Is the routing regression test in CI? | **RESOLVED.** `tests/test_group_routing.py` asserts the **deterministic classifier** in both directions — no model, so it runs offline and is in CI. A routing test that needed a model would not be run. |
| P-2 | Who writes the `PROJECT.md` prose? | **RESOLVED BY BUILD.** The orchestrator renders it from structured arguments at creation (`groupTools._render_manifest`); the human edits it afterwards. The tool takes milestones and members, not markdown — a model hand-writing fenced YAML in one call gets it wrong sometimes. |
| P-3 | Does `docs/Structure.md` get a Zone Z5? | **RESOLVED.** Z5 exists, marked DESIGNED, not built. Worth revisiting now that it *is* built. |
| P-4 | Migration story for existing `tasks_json` tasks? | **UNCHANGED.** Leave them. They are the `scheduled` shape and remain valid. |
| P-5 | Does `fixed-concept.md` need amendment? | **DONE.** §5 now carries the supervisor's single-writer rule and the one-thread decision. |
| P-6 | ~~Where does the Phase 0 job store live?~~ | **RESOLVED.** `.deep_agents/jobs.db`; `JobStore` is one interface with two backings. `07` §6. |
| P-7 | ~~Is the `state_sync` fix a blocker?~~ | **RESOLVED.** ~6 lines, pattern already at `delegation.py:622`. `07` §5. |
| P-8 | Is `tests/run_all.py` still needed? | **NEW.** CI now uses pytest. `run_all.py` was fixed (repo root on `sys.path`; fixture tests SKIP instead of failing) and kept for the zero-dependency path. Decide whether to keep both or retire it. |
| ~~P-6~~ | ~~Where does the Phase 0 job store live?~~ **RESOLVED.** New `.deep_agents/jobs.db`, not a table in `chats.db` and not the JSONL ledger. `JobStore` is one interface with two backings — harness-level in Phase 0, per-project in Phase 2. Rationale and rejected alternatives: `07-phase-0-implementation.md` §6. |
| ~~P-7~~ | ~~Is the `state_sync` fix a blocker?~~ **RESOLVED.** No. It is a ~6-line fix using a pattern already proven at `delegation.py:622-627`; the scheduler simply never sets `active_state`. It is Phase 0, deliverable 3. Corrected in `05` §5. |

---

## 6. Definition of Done

Group projects are done when all of these hold:

| # | Criterion |
|---|---|
| 1 | You can say *"sort out the Q3 invoices"* and get a project — without typing a keyword |
| 2 | You say *"every morning check the deploy"* and get a `scheduled` task — not a project |
| 3 | The project survives `kill -9` mid-item and resumes without duplicate or lost work |
| 4 | A member's conversation continues across items |
| 5 | Nothing leaves the project folder without an approval you granted |
| 6 | You can read, from the events log alone, exactly what happened while you were asleep |
| 7 | You can add an item, a milestone, a member, or an MCP server to a **running** project by voice |
| 8 | Budget, deadline, and completion are enforced without you noticing |
| 9 | A project with no possible progress stops itself and asks |
| 10 | **Delete every projection table, run `rebuild()`, and nothing is lost** |
| 11 | **Copy the folder to another root and it loads and resumes unchanged** |
| 12 | **A member discovers the plan is wrong, invalidates 5 items, and one re-plan happens** |
| 13 | **You can read the progress line: milestones N/M · items N/M · budget spent/cap** |
| 14 | `docs/group-projects/06` §2 has a decided answer, and the codebase matches it |

Criterion 10 is the architectural one: if the log can rebuild the board, then every future change to the model is free. Criterion 11 is the portability one — a folder that only works on one root is not self-contained, it is just relocated. Criterion 6 is the trust one: it is the audit log Grok Bot does not have (`00` §5.2), and it is the difference between "the agents did something while I slept" and "I know what they did."

---

## 7. What Would Make This Fail

Recorded now, honestly, so it can be recognised early rather than defended against.

| Failure | Symptom | Why it happens |
|---|---|---|
| **The board is LLM-maintained after all** | State vanishes after a compaction | `update_plan`-style authorship creeps back in because it is the path of least resistance. `01` §3 is the guard. |
| **Orchestrator re-reasons every tick** | Token cost explodes; plans drift | The supervisor boundary (`02` §1) leaks. Watch for LLM calls inside the tick. |
| **`state_sync` cross-talk** | Members report each other's results | Phase 0's fix regressed, or a new code path reads the global. |
| **The project never finishes** | Board stuck in `doing` forever | Lease renewal without progress. The `blocked` fallback must not be suppressed. |
| **A table quietly becomes the source of truth** | Projections drift; `rebuild()` fails | Direct writes appear "just this once". Phase 0 stops hiding it — which is the point of running phase 0 every tick rather than only at boot. |
| **The event vocabulary keeps growing** | Old projects cannot replay | A missing `kind` is permanent. D-13 says settle it in Phase 1. |
| **Invalidation is used as a failure retry** | `attempts` climbs, plan churns | `attempts` must never increment on `invalidated` (DM-15). If it does, the distinction is decorative. |
| **Milestone gating leaks** | Items dispatched into a blocked phase | Readiness merged back into one pass, so the milestone check is skipped. RT-15 guards it. |
| **An absolute path sneaks in** | The folder won't move | One `workdir` written as an absolute path. DM-18 scans for exactly this. |
| **Routing over-triggers** | Trivial jobs become 3-day projects | Descriptions missing when-**not**-to-use clauses. Caught by `03` §2.3 negative cases. |
| **Chat becomes the bottleneck** | Token cost scales with team size × history | Point-to-point messages collapsed into a shared transcript. `01` §8. |
| **Approval fatigue** | You approve without reading, or stop approving | First-occurrence gating (`04` §4.1) exists for this; if it fails, narrow the envelope. |
| **The supervisor silently does not run** | Projects stall with no error | `start_group_scheduler()` only under `__main__` (`05` §10). Emit a heartbeat. |

The first three are the dangerous ones, because all of them fail *quietly* and all of them get attributed to the model rather than to the architecture.