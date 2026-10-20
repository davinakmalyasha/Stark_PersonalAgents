# Group Projects — Autonomy & Approvals

**Version**: 1.0
**Date**: 2026-10-01
**Author**: Stark design session
**Status**: Draft
**Depends on**: `01-concept-model.md`, `02-runtime.md`

Defines the tiered autonomy model, where it is enforced in existing code, and the approval flow.

---

## 1. The Concept

OpenAI's Dots checks every action against your instructions, Custom Rules, and safety requirements, then resolves to *proceed*, *needs approval*, or *you must do it* (`openai.com/index/introducing-dots/`, 2026-09-29). That inference happens **per action**.

Group projects declare the same thing **once, per project**, before any work starts:

```yaml
# PROJECT.md
autonomy:
  project_folder:  autonomous      # write freely inside the project folder
  external_read:   autonomous      # GET / query anything
  external_write:  needs_approval  # email, tickets, PRs, payments
  destructive:     blocked         # delete, force-push, prod, spend
```

| Property | Dots (inference) | Group projects (declaration) |
|---|---|---|
| When decided | Per action, at run time | Once, before the run |
| Cost | An inference per consequential action | Zero — it's YAML |
| Predictability | Best case after a few surprises | Known up front |
| Reviewable before work | No | Yes — read `PROJECT.md` |
| Blast radius | Discovered from a log | Declared in the manifest |
| Hand-off safety | Unclear | The envelope travels with the project |
| Adaptability to new actions | Automatic | Manifest must be extended |

**Why declaration wins here.** Stark's operator owns the machine and owns the project folder. There is no reason to pay an inference per action to discover what a static file already says. And a declared envelope means you can read the blast radius *before* committing to a project — which is precisely what makes a group project safe to hand to someone else.

The one thing inference genuinely buys is automatic coverage of actions you did not anticipate. Stark gets that from the **default-deny** rule in §3 instead.

---

## 2. Tiers

| Tier | Meaning | Examples |
|---|---|---|
| `autonomous` | Do it; log it | Read/write inside the project folder; web/GitHub/MCP reads; run commands; append to `context/`; commit inside the project repo |
| `needs_approval` | Queue it, continue other work | Send email; open a PR; post to Slack; write to a DB; any MCP `write` tool; publish a report |
| `blocked` | Refuse and tell the human | `rm -rf`, force-push, credentials, payments, prod deploys, anything on `_cmdpolicy.danger()` |
| `never` | Not offered to members at all | System files outside the project; the harness's own source; other projects' folders |

**`never` is not `blocked`.** `blocked` means the member tried and was refused (you see the attempt). `never` means the tool is not in the member's toolset at all (no attempt, no event). `never` is implemented via the scope ACL (`05` §6); `blocked` via the approval hook.

---

## 3. Default-Deny

> **Anything not explicitly permitted by the project's envelope is `needs_approval`.**

An enumerated allowlist is a list of things someone thought of. A default-deny rule catches everything else — including the actions you did not predict.

```
classify(action, project) ->
    never          if path/tool outside the project's permitted surface
    blocked        if _cmdpolicy.danger() or credential access or tier == 'blocked'
    autonomous     if action is project_folder_write or external_read, and tier allows
    needs_approval otherwise
```

This is the only place in the design where we take something *from* Dots rather than depart from it: their safety posture is that built-in safety requirements always apply and cannot be overridden. Same idea, applied to our file.

---

## 4. The Approval Flow

```
member wants an external_write
        │
        ▼
 classify() → needs_approval
        │
        ├─ item stays DOING, reason = awaiting_approval
        ├─ idempotency key computed          (§6)
        ├─ events: approval.requested        (with the pending payload)
        ├─ inbox.md ← ## PENDING APPROVAL     (what, why, exact diff/target)
        └─ realtime bus publish → UI notification
        │
        ▼
 human reads inbox.md / the UI / Telegram
        │
        ├─ approve ─→ op 'approve' ─→ supervisor tick ─→ re-check key ─→ perform ONCE
        │                                                    └─ mark approval.granted
        └─ reject  ─→ op 'reject'  ─→ item → blocked(rejected) ──→ orchestrator re-plan
```

**The item does not die while waiting.** It stays `DOING` on an `awaiting_approval` lease, which is exempt from idle detection (`02` §6.2) — otherwise every overnight wait would look like a stalled project.

**While an item waits, its lease is renewed.** A member parked on human approval for two days should not lose its lease.

**Approval is a scope grant, not a one-shot.** Approving one email is approving *that* email. A pattern of five approvals of the same shape is a signal the envelope is wrong — the orchestrator may propose a `set_autonomy` op to cover the case, and the human decides.

### 4.1 Always human

Regardless of envelope, these never automate. Grok Bot and Dots both hold this line, and so do we:

| Never automated | Rationale |
|---|---|
| Credentials, passwords, tokens | Irreversible leak if wrong; also a security-boundary violation |
| Payments / money movement | Irreversible |
| Production deploys / destructive ops | Blast radius exceeds the project folder |
| Deleting anything outside the project folder | Irreversible, unbounded |
| Sending to an external party on the project's first occurrence | Cold-start check that the output is right |

The last one is a judgement call worth stating explicitly: the **first** outward-facing action of a given shape requires approval; subsequent identical shapes do not. It catches "the agent emailed the wrong vendor" before it emails forty more.

---

## 5. Enforcement Map

Where each tier is enforced in **existing** code. No new enforcement layer.

| Tier | Enforced by | Existing hook |
|---|---|---|
| `never` — tool not offered | Materialised allowlist | `tools/bindings.json` ∩ pydantic-ai `FilteredToolset` (`llm/native_tools.py`) |
| `never` — path outside project | Filesystem permission | `enforce_fs_permission()` (`tools/_runtime.py:88`), project root from `WorkspaceDeps` |
| `blocked` — dangerous command | Command policy | `check_command()` / `_danger()` (`tools/MainTool/_cmdpolicy.py`) |
| `needs_approval` — gated action | Approval hook | `_check_and_request_approval()` (`tools/_runtime.py:512`) |
| MCP write tool | Per-tool approval | `McpManager.call(..., approval="prompt")` (`mcp_bridge.py:539`) |
| Credential-bearing command | Credential policy | `needs_credentials()` (`tools/MainTool/_cmdpolicy.py`) |
| Plan-mode veto | Tool hiding | pydantic-ai `Tool.prepare` → returns `None` to hide |

**The change is narrow and additive:**

1. `tools/bindings.json` is read at member-toolset-build time and intersected with the registry's `_SCOPE_MAP` (`tools/_schemas.py:29`) — a project can only ever *narrow* what the base scope allows, never widen it.
2. `_check_and_request_approval()` gains one input: the **project envelope**, threaded through `WorkspaceDeps` (`llm/deps.py:19`), which already carries `project_path` and the tool allowlist per run.
3. `enforce_fs_permission()` gains the project root, so "inside the project folder" is a computable predicate rather than a config entry.
4. Nothing in `tools/` learns about group projects. The envelope, the allowlist, and the MCP config arrive as data.

That is the whole enforcement integration. A member cannot reach an unguarded path because it was never in its materialised allowlist, and cannot reach a guarded one without the envelope.

---

## 6. Idempotency on External Writes

A multi-day project will retry a failed item. If the write is an email, the retry must not send it twice.

| Field | Notes |
|---|---|
| `key` | `"{item_id}:{shape}:{payload_sha}"`. `shape` from `policy.shape_of()`; `payload_sha` is SHA-256 of the canonical MCP `args` (JSON, sorted keys) or the command string. |
| `state` | `fresh` → `reserved` → `done` \| `skipped`. A `reserved` with no later `done` is `unknown` — the crash case. |
| Stored on | The **event log** (`write.reserved`, `write.done`, `write.skipped`). No table: every table is a projection, so a `writes` table would be a second source of truth. |

Include `item_id` in the key: two items legitimately sending the same email are two events, not a duplicate. The key is **path-free by construction** — `shape` is `server/tool` or the command roots, and the payload is only ever a hash — because `check_portability()` does not scan `.gp/` (`06` §7).

Only actions the envelope **gated** are deduped: tier `needs_approval`, including shapes already granted. An `autonomous` action is the project's own business and repeating it is often correct (re-running a build after a fix must not be suppressed). File writes are deliberately **not** deduped: writing the same path twice is naturally idempotent, and a key containing a resolved path would break portability.

MCP calls are covered when the server config marks them `approval: "prompt"` (the existing per-tool setting); a server configured otherwise is not gated, so it is not deduped either. Group materialisation copies the catalogue's setting (`04` §7), so the catalogue owns that decision.

```
perform_write(key, payload):
    state = ledger(key)
    if state == "done":    emit write.skipped; return "already performed; skipped"
    if state == "unknown": park for a human ("may or may not have been sent")
    emit write.reserved
    perform(payload)
    emit write.done
```

`write.reserved` and `write.done` are separate commits on purpose: a crash *between* the external call and `write.done` leaves the key reserved, which surfaces to the human as *"this may or may not have been sent — please check"* rather than silently duplicating. Approving that question authorizes **one** retry; rejecting it blocks the item for a re-plan. Honest about the ambiguity beats confidently wrong.

The three `write.*` kinds are an explicit amendment to D-13 (`06` §4): they are audit-only, and replaying them is a no-op in the same way `capability.changed` is.

**Audit log is mandatory from day one.** Every `classify()` decision — autonomous or gated — appends an event with the tier that was applied (`01` §7). This is the log Grok Bot does not have (`00` §5), and it is what lets you answer "what did this project actually do while I was asleep?"

---

## 7. Per-Project Permission Envelope

The envelope is the project's blast radius, and it travels with the folder. Because capability is materialised (`01` §2), **the rules, the tools, the skills, and the MCP servers all sit in one folder** — handing that folder to someone else hands them the complete picture, not just the intent.

```yaml
# PROJECT.md
autonomy:
  project_folder:  autonomous
  external_read:   autonomous
  external_write:  needs_approval
  destructive:     blocked

fs:
  writable: ["artifacts/", "context/", "."]
  readable: ["."]                 # relative to the project folder
  hidden:   ["../", "~", "D:/"]
```

```yaml
# tools/bindings.json      (materialised — the ONLY tools these members get)
{
  "agents": {
    "frontend": {
      "allow": ["readFile", "editFile", "runCommand", "searchCode"],
      "caps":  { "editFile": 200, "runCommand": 60 }
    },
    "copywriter": {
      "allow": ["readFile", "editFile", "webSearch"],
      "caps":  { "editFile": 40 }
    }
  }
}
```

```yaml
# mcp/servers.json         (materialised — credentials by NAME only)
{
  "servers": {
    "sentry": {
      "url": "https://mcp.sentry.dev/mcp",
      "bearer_token_env_var": "SENTRY_ACCESS_TOKEN",
      "enabled_tools": ["list_issues", "get_issue"],
      "approval": "prompt"
    }
  }
}
```

**On credentials:** MCP servers already reference credentials by *name* — `env_vars`, `bearer_token_env_var`, `http_headers` (`mcp_bridge.py`). That is the correct pattern and materialisation must preserve it: a member calls a tool; the bridge injects the credential at the transport; the model never sees the value. This is the same discipline Claude Cowork uses by keeping connector tokens outside the sandbox, and it is why DM-10 forbids secret values in `mcp/servers.json`.

---

## 8. The Human Surface

| Channel | Already exists | Use |
|---|---|---|
| `inbox.md` | new | The canonical queue. Survives restart, greppable, diffable. |
| Orchestrator chat | yes | "approve the last one" / "why is X blocked?" |
| Telegram bot | `Telegram/` | Push `approval.requested`, accept approve/reject. Highest-value channel — you are away from the UI. |
| Voice | `Voice/` | Approval by voice, read-back of what is being approved. |
| UI | `frontend/src/components/Canvas/AddTaskModal.tsx` as a pattern | An approvals panel. `05` §8. |

**Read-back before acceptance on voice.** Approving an email by voice is fine; approving an unknown email by voice is how mistakes happen. The TTS reads the target, the action, and the exact payload, then waits.

---

## 9. Contracts

| ID | Requirement | Priority | Acceptance criterion |
|---|---|---|---|
| AU-01 | Classification is default-deny | Must | Given an action not named in the envelope, When classified, Then the tier is `needs_approval` |
| AU-02 | The envelope is authored before the first dispatch | Must | Given a new project, When it has no `autonomy:` block, Then it is rejected at creation |
| AU-03 | Credentials and money are never automated at any tier | Must | Given a gated action touching credentials or payment, When classified, Then the tier is `blocked` regardless of the envelope |
| AU-04 | Every classification appends an event with the tier applied | Must | Given a full project run, When events are read, Then every consequential action has exactly one classification event |
| AU-05 | External writes are idempotent by key | Must | Given a retried item, When the write is re-performed, Then the second attempt emits `write.skipped` and does not repeat the effect |
| AU-06 | An `awaiting_approval` item is exempt from stall detection | Must | Given a project idle solely on approvals, When N ticks pass, Then no `project.stalled` is emitted |
| AU-07 | An `awaiting_approval` lease is renewed | Must | Given a project paused on approval for > lease duration, When the tick runs, Then the item is not `blocked` |
| AU-08 | The first occurrence of an outward-facing action shape requires approval | Must | Given a `telegram:send` action, When no prior approved instance exists, Then approval is required |
| AU-09 | No secret value reaches a member's context | Must | Given an MCP call needing a credential, When it executes, Then the token is injected at the transport and absent from the prompt and result |
| AU-10 | Rejecting an approval blocks the item and notifies the orchestrator | Must | Given a rejection, When applied, Then the item is `blocked(rejected)` and a re-plan is requested |
| AU-11 | `never`-tier tools are absent from the member toolset | Must | Given a member in a project with no `github` allow, When its tools are built, Then no `mcp__github__*` tool is present |

---

## 10. Resolved decisions

| Decision | Rationale | Alternatives considered |
|---|---|---|
| Declare autonomy per project | Zero marginal cost; reviewable before work; travels with the folder | Per-action inference (Dots' approach — an inference per action, unpredictable, unreviewable up front); global config (not project-scoped; can't hand a project off safely) |
| Default-deny | Catches unanticipated actions; `never`/`blocked` cannot be widened by adding a tool | Allowlist (`blocked` can be bypassed by naming a new tier) |
| Keep credits/passwords/payments always-human | Irreversible; also a security boundary | Envelope-configurable (one manifest typo leaks a token) |
| First-occurrence gate for outward actions | Catches "emailed the wrong vendor" before 40 more | No gate (irreversible at scale); every occurrence gated (defeats autonomy) |
| Envelope on `WorkspaceDeps`, not a new layer | Additive; `tools/` learns nothing about group projects | A new guard in `tools/` (parallel enforcement that can disagree) |
| Secrets by name, injected at transport | Matches the existing MCP design; model never holds a token | Inject into the member prompt (leaks into transcripts and compaction) |
| Idempotency reserve/mark split | A crash mid-write surfaces honestly as "may have sent" | Single-shot (silently duplicates on retry); skip idempotency (duplicates on every retry) |
| `inbox.md` canonical, channels are views | Survives restart; greppable; works with no UI | Telegram-only (no durable record); DB-only (not diffable by hand) |
| Reuse the existing approval hook | One enforcement path | A new guard layer (two systems, eventually one bypassed) |