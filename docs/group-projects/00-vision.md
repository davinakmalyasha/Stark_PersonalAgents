# Group Projects — Vision & Concept

**Version**: 1.0
**Date**: 2026-10-01
**Author**: Stark design session
**Status**: Draft — agreed direction, nothing built
**Handoff**: Implementation begins at `05-integration-map.md`

---

## 1. The Problem

Stark today is a **prompting harness**. The human is the loop:

```
you type  →  orchestrator decides  →  delegates  →  workers run  →  report back  →  you decide what's next
                 ▲                                                                     │
                 └─────────────────────────────────────────────────────────────────────┘
```

That works for one task at a time. It fails as soon as work is:

- **Multi-session** — a project that outlives a single run.
- **Multi-step** — more than a handful of dependent sub-tasks.
- **Repetitive** — the same shape of job, many times.
- **Under-specified** — you know the goal, not the decomposition.

The failure mode is specific and familiar: you end up re-prompting the same orchestrator over and over, restating context it already lost, nudging it toward a next step it should have seen. The model is doing the work *and* acting as your project manager for a plan neither of you wrote down.

Group projects replace the human-in-the-loop-per-step with **a leader that plans once, and a runtime that keeps going**.

---

## 2. The Concept

> You describe a goal. An orchestrator authors a plan — a DAG of items, each with its own acceptance criteria — **and chooses the capability set the project needs**. The orchestrator then materialises that whole environment into a self-contained project folder: the agents, their skills, their tools, their MCP servers, plus the board. A deterministic supervisor runs it to completion across sessions, days, and restarts. You approve the outward-facing parts and read the report at the end.

```
   "build me a landing page for the coffee business"
                                    │
                     orchestrator.plan_execution()   ← ONE strong-model call
                                    │
                     · plan DAG + acceptance criteria
                     · environment: 2 agents, 2 skills, tools, MCP
                    (command mode · mimo-v2.6-flash + deepseek-v4.1-flash
                     both thinking:high)
                                    │
                    ┌───────────────┴───────────────┐
                    │      project folder           │
        PROJECT.md  (authored)   .gp/state.db (run truth)
        agents/  skills/  tools/  mcp/   context/
                    └───────────────┬───────────────┘
                                    │
                       supervisor.tick()   ← NO LLM, deterministic
                                    │
              ┌─────────────────────┴─────────────────────┐
              │                                           │
        member: frontend                          member: copywriter
        agents/frontend.json                    agents/copywriter.json
        deepseek v4.1 flash, high                mimo v2.6 flash, high
              │                                           │
              └──────────── verify against AC ───────────┘
                                    │
                     done → next ready item → dispatch …
```

**The single sentence version:** *the orchestrator decides what work exists and what environment it needs; the supervisor decides when that work happens.*

**The project folder is the unit.** It is not a pointer back to the harness — the capability set lives inside it (`01` §2).

---

## 3. Why Not Just Use Dots or Grok Bot?

Both were researched (Sept/Oct 2026). Neither is a substitute; both are worth studying.

| | OpenAI **Dots** (2026-09-29) | xAI **Grok Bot** (2026-08-11) |
|---|---|---|
| Shape | One personal always-on agent, GPT-6 Astra | Team of named "AI teammates" |
| Machine | Own cloud computer per dot | **One shared** cloud computer per account |
| Persistence | Durable state, learns preferences | Durable state, skills, routines |
| Background behaviour | **"Proactive research" — read-only tools only** when unobserved | Continues; no dry-run mode |
| Multi-agent | Teams of dots are *envisioned*, not shipped | Bots message each other, group chats, handoff |
| Permissions | Custom Rules (allow / require approval / block) + auto-review | Approvals per action |
| Identity | Personal; *specialist dots* in enterprise pilot only | Per-Bot role |
| Dev access | **None announced — closed product** | Desktop/iOS apps; no public agent API |

**What these products are actually selling:** a machine that stays on, plus a natural-language front door. Neither exposes the plan, the board, or the budget. Neither lets you say "here is a deadline, here is a budget, here is what needs my approval."

**Where Stark is structurally different:**

1. Stark already has a **three-tier agent architecture** (orchestrator → workspace agent → subagent) and a **scoped tool warehouse**. Dots is one flat agent; Grok's bots share one machine and therefore have no security boundary between them.
2. Stark can **express a plan as data**. Both products' multi-agent story is conversational.
3. Stark's operator **owns the machine**, so the permission envelope can be declared and auditable rather than inferred.

**Conclusion:** the surface (always-on, voice-first, no harness) is worth copying. The internals (plan-as-data, declared autonomy, durable board) are where Stark should be better.

---

## 4. What We Steal

| From | Steal | Why it fits Stark |
|---|---|---|
| Dots | **Inspectable machine / live takeover** — talk to a *member*, not just the orchestrator | `subagent_manager.send_message()` (`:606`) + `read_child_session()` (`:747`) + `/api/steer` already exist. Mostly a UI surfacing job. |
| Dots | **Custom Rules as an artifact** (allow / require-approval / block) | Re-point `_cmdpolicy.py` + `enforce_fs_permission()` (`_runtime.py:88`) at a **per-project** ruleset instead of global config. |
| Dots | **Auto-review outcome** → becomes *declared* autonomy tiers | `_check_and_request_approval()` (`_runtime.py:512`) is the hook. See `04-autonomy-and-approvals.md`. |
| Dots | **Background work when unobserved** | Already implied by the supervisor tick; Stark's version is declared rather than inferred. |
| Grok | **skill vs routine** vocabulary — *how* vs *when* | Gives `tools/skills.py` (currently unwired, `:15-21`) its first real consumer. Maps cleanly onto items. |
| Grok | **"What requires mandatory human approval" as an explicit skill field** | Confirms the autonomy envelope belongs *inside the materialised skill*, not in global config. A skill that emails must declare that it needs approval. |
| Grok | **"May pause routines after a long absence if there's no response"** | The stall guard, already productised by someone who shipped it. Ours is deterministic and emits an event (`02` §6.2). |
| Grok | **Learn-by-demonstration's *outcome*** | Stark distills a skill draft from N successful trajectories already in `orchestrator_jobs.jsonl` + artifacts, and asks for approval. No screen recording needed. |
| Grok | **Handoff without you as router** | The DAG *is* the router. No conversation required to route work. |
| Grok | **Routines** (schedule/event-triggered, 50/bot cap, last-20 run history) | Informs item history retention — and its caps tell us it is a *recurring-job* system, not a project system (§5). |

## 5. What We Deliberately Do Not Steal

### 5.1 Grok Bot is a routine runner, not a project manager

This is the sharpest line in the design, and it is worth being exact about.

From x.ai's docs, Grok Bot's **entire** state model is:

| Its primitive | What it holds |
|---|---|
| **Skill** | how to do a task — when to use, required inputs, steps, how to validate, what to return, what requires approval |
| **Routine** | when to run a workflow — schedule or event. **Max 50 per bot. Last 20 run records kept.** |
| **Memory** | user preferences, project context, history, lessons from corrections |
| **Threads** | conversation + bot-to-bot messages |

**There is no board. There is no project object.** Grok Bot tracks *the routine*, not *the work*: each run is independent, progress lives in conversation and memory, and history is deliberately capped at 20 runs per routine — which tells you it is built for recurring jobs, not for multi-week undertakings.

It cannot answer *"what's left on this project?"* because it has no project to ask about.

That is not a flaw. It is a different product — and the boundary is exactly where this design sits:

| | Grok Bot | Group projects |
|---|---|---|
| Unit of work | A routine run | A project, over milestones, to a done-condition |
| "What's left?" | Re-read the conversation | `SELECT` from the projection |
| History | 20 runs per routine | Every event, forever |
| Plan can change? | No plan to invalidate | `item.invalidated` + `plan.revised` |
| Progress | Implicit in the thread | milestones 2/5 · items 9/14 · budget $1.95/$4.00 |
| Multi-agent | Threads + handoff | Milestone DAG + leases + one shared board |

**If what you actually want is a recurring job, `plan_execution`'s `scheduled` shape already routes you to the existing task scheduler — not to a group project** (`03` §2.2). Group projects are for the other case.

### 5.2 Anti-patterns

| Anti-pattern | Why |
|---|---|
| Shared machine as unit of isolation | Grok's own docs: *"The computer is assigned per user, not per Bot. Do not use separate Bots as a security boundary."* Stark's unit is the **project folder** + per-member `WorkspaceDeps`. Strictly stronger — do not regress to shared. |
| No per-action audit log | Grok Bot has no dry-run mode and no complete per-action log. Stark's **event log is mandatory from day one**, not a later feature. |
| **Mutable tables as the record** | Grok keeps run history but caps it at 20; Dots keeps activity view. Both keep a *view*, not a log. Stark's `events` table is append-only and never updated — so the model can change later without a migration (`01` §7). |
| **A flat item list** | Grok has no list at all. A flat list stops being readable past a few dozen items. Milestones carry phase ordering and give a real progress signal (`01` §4). |
| Per-action risk *inference* | Dots infers risk per action via auto-review. Declaring the envelope once per project (`04`) is cheaper, more predictable, and inspectable before the work starts. |
| Flat single-agent model | Dots. Stark keeps the three tiers; a group project is a *composition* of them, not a replacement. |
| Cloud-first | Both products assume a cloud VM. v1 runs locally; the cloud is where project folders live and **nothing else moves** (`06` §2) — it buys availability and blast-radius containment, **not capability**. |

---

## 6. The Five Load-Bearing Ideas

Everything else in these docs is mechanism. These five are the concept.

### 6.1 The board is the coordination primitive, not chat

Members read **rows**, claim with a **lease**, post results. Inter-agent messages are **point-to-point**, delivered into a recipient's inbox and injected when it next wakes. There is no shared transcript that all members read.

*Why:* agents coordinating through a common conversation do not converge — they burn tokens re-reading each other's narration and argue about stale context. A board makes "what is left" a query, not a discussion.

*Corollary:* group chat is a **human-facing view** over rows and messages, never the transport.

### 6.2 Autonomy is declared per project, not inferred per action

`PROJECT.md` states, up front, what the group may do without asking: autonomous inside the folder, gated on anything that leaves it, blocked for destructive operations. See `04-autonomy-and-approvals.md`.

*Why:* declared blast radius means a project can be handed to someone else safely, and you can review the rules before any work happens rather than reconstructing them from a log afterwards.

### 6.3 The plan is authored once, not re-reasoned every turn

The current orchestrator re-decides "what next?" from a growing history on every turn — expensive, and anchored to its own earlier narration. Instead: **one planning pass, strong model, no tools**, emitting a DAG where every node carries its own acceptance criteria. Execution is then mechanical.

*Why it matters:* those per-item acceptance criteria **are** the "until it's done" predicate. So the plan format and the completion condition are one decision, not two.

### 6.4 The folder is the unit, and it carries its own capability

The orchestrator does not just plan — it **chooses and materialises the environment**. Two agents for a landing page, four skills, a tool allowlist, an MCP config: all written into the project folder, not referenced from the harness.

*Why:* the folder becomes portable, inspectable, and handable. Two projects can run different versions of the same agent. And a project handed to someone else carries its own rules, tools, and skills rather than pointing at your machine.

*Cost, contained:* a materialised agent is a snapshot, so every one records `derived_from` + `derived_at_version` and regeneration is a command rather than archaeology (`05` §9.2). How much of the harness itself travels with the folder is still open (`01` §13).

### 6.5 The events log is what remembers; the board is only what you look at

`events` is the single source of truth — append-only, never updated. `items`, `milestones`, `leases`, `approvals` are **projections** of it: maintained incrementally for speed, rebuilt from events on boot, self-healing within one tick of any divergence.

*Why:* a two-week project will outlive whatever you decided today. If the tables are the record, changing the status vocabulary, adding milestones, or repairing a corrupt row later means a migration on every live project. If the log is the record, it means editing a projection — and the log already contains every fact needed to rebuild any view.

*Corollary:* the event `kind` vocabulary is now an API you cannot break. It deserves a design pass before implementation, not after.

*Corollary 2:* this is why stall detection counts **events**, not CPU (`02` §6.2). A project doing real work emits events continuously; a quiet-but-working member is still busy. That is exactly the signal a cloud host's idle-reclamation rule would misread (`06` §2.4).

---

## 7. Scope

### In Scope

- Orchestrator-initiated group projects from voice or text, with `@task` as a force path.
- **A self-contained project folder** carrying its agents, skills, tool bindings, MCP config, project deps, tasks, context, and run state (`01` §2). The orchestrator chooses that set.
- **A six-rule portability contract** so the same folder runs locally and in the cloud unchanged (`01` §2.1).
- **Milestones between project and item**, dependencies declared at one level only, so long projects stay readable and progress is reportable (`01` §4).
- **`invalidated` as an item state** — for a long-horizon plan that turns out to be wrong — with coalesced re-planning (`01` §4.4).
- **An append-only `events` log as the only truth**, every other table a rebuildable projection (`01` §7).
- A deterministic supervisor that runs the board across sessions, restarts, and days — and makes no LLM calls, so the model can sleep (`02` §1).
- **Provider profile: `command` mode, `deepseek/deepseek-v4.1-flash` and `xiaomi/mimo-v2.6-flash`, both `thinking: high`**, pinned per member (`01` §5.2).
- Per-project autonomy envelope with approval gates and a human inbox.
- The orchestrator mutating a **running** project (add items, milestones, members, MCP, skills, tools; adjust budget, deadline, autonomy).
- Single-member projects using the identical code path (orchestrator as sole member).
- **The cloud as the place project folders live** — concept documented, v1 local (`06` §2).

### Out of Scope (v1)

- **Cloud deployment.** The concept, the host requirements, and the free-tier survey are documented (`06` §2). No provider is chosen and v1 runs locally.
- **Per-project container or VM sandbox.** Reverses a recorded decision (`.opencode/plans/run-command-batch-abc.md:12`) and needs its own spec (`06` §3).
- **Installing new software or MCP servers autonomously.** v1 materialises only what is already installed. Install is human-approved; see `04` §6.
- **A new agent tier.** A group project is a composition of existing scopes, not a fourth tier.
- **Modifying `delegate`'s semantics.** It stays the within-run primitive (`02` §8).
- **Multi-user projects.** Stark is single-user by design (`specs/mobile-remote.md`); the permission envelope is designed for hand-off but multi-tenant auth is not v1.
- **Deciding the copy boundary** — how much of the harness travels inside a folder. Open at `01` §13; blocks nothing before Phase 3.

---

## 8. Reading Order

| Doc | Read it for |
|---|---|
| `00-vision.md` | This. The concept and why. |
| `01-concept-model.md` | What a group project **is** — entities, folder layout, portability, milestones, the event log. |
| `02-runtime.md` | How it **runs** — supervisor, tick phases, dispatch, leases, resume, member model profile. |
| `03-orchestrator-tools.md` | The **tool surface** and how the orchestrator learns to use it. |
| `04-autonomy-and-approvals.md` | The **safety model** — tiers, enforcement, approvals. |
| `05-integration-map.md` | What **changes in your code**, file by file. |
| `06-phases-and-open-questions.md` | Build order, gates, the cloud concept, and what is still undecided. |
| `07-phase-0-implementation.md` | **The only buildable spec.** Phase 0's three deliverables, schema, gate. |

`05` and `06` are the two that grow during implementation. `07` is the only one that is *ready to execute* — every other doc describes something that cannot be built until Phase 1's schema is frozen.

---

## 9. Nothing has been built

These seven documents are the entire deliverable. **No code exists.** Every module, table, route, and tool named in `02`-`05` is a proposal to be implemented, and the line references point at code as it stands today so the proposals can be checked against it.