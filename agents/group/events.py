"""agents.group.events — the event vocabulary (Phase 1, decision D-13).

Once the first project exists these strings are an **API that old projects
replay through forever**. A kind that is missing at creation time is a
permanent hole in the history, so the list below is explicit, frozen, and
grouped by the concern that owns it.

Design rules
------------
- ``kind`` is ``<subject>.<verb>``, lower-case, dots only. No spaces, no
  version suffixes.
- Every state in :data:`ITEM_STATUS` and :data:`MILESTONE_STATUS` has exactly
  one kind that puts a subject into it (DM-19). ``test_group_events`` asserts
  this correspondence, so adding a state without a kind fails the suite.
- ``item.invalidated`` is NOT a failure and NOT a block (``01`` section 4.4):
  it means the plan was wrong. It never increments ``attempts``.

Adding a kind later is allowed for genuinely new concerns. Renaming one is
not — it breaks every stored project.

**D-13 amendment (AU-05).** The external-write ledger added ``write.reserved``,
``write.done`` and ``write.skipped``. They are audit-only kinds: the ledger is
derived by scanning events, so the reducer treats them as no-ops exactly like
``capability.changed``. The amendment is recorded in
``docs/group-projects/06-phases-and-open-questions.md`` (D-13).
"""
from __future__ import annotations

# ── project ─────────────────────────────────────────────────────────────────
PROJECT_CREATED = "project.created"
PROJECT_RESUMED = "project.resumed"
PROJECT_PAUSED = "project.paused"
PROJECT_CLOSED = "project.closed"
PROJECT_STALLED = "project.stalled"

# ── milestones ──────────────────────────────────────────────────────────────
MILESTONE_CREATED = "milestone.created"
MILESTONE_READY = "milestone.ready"
MILESTONE_DONE = "milestone.done"
MILESTONE_FAILED = "milestone.failed"

# ── items ───────────────────────────────────────────────────────────────────
ITEM_CREATED = "item.created"
ITEM_READY = "item.ready"
ITEM_CLAIMED = "item.claimed"
ITEM_RELEASED = "item.released"
ITEM_PROGRESS = "item.progress"
ITEM_DONE = "item.done"
ITEM_FAILED = "item.failed"
ITEM_BLOCKED = "item.blocked"
ITEM_INVALIDATED = "item.invalidated"
ITEM_CANCELLED = "item.cancelled"

# ── plan ────────────────────────────────────────────────────────────────────
PLAN_REVISED = "plan.revised"
IDEA_PROPOSED = "idea.proposed"

# ── operations (the only mutation channel) ──────────────────────────────────
OP_SUBMITTED = "op.submitted"
OP_APPLIED = "op.applied"
OP_REJECTED = "op.rejected"

# ── gates and leases ────────────────────────────────────────────────────────
LEASE_EXPIRED = "lease.expired"
BUDGET_WARN = "budget.warn"
BUDGET_STOP = "budget.stop"
DEADLINE_REACHED = "deadline.reached"

# ── Phase 2+ (declared so the vocabulary is complete; not reduced in Phase 1) ─
APPROVAL_REQUESTED = "approval.requested"
APPROVAL_GRANTED = "approval.granted"
APPROVAL_REJECTED = "approval.rejected"
MESSAGE_SENT = "message.sent"
MESSAGE_READ = "message.read"
MEMBER_ADDED = "member.added"
MEMBER_REMOVED = "member.removed"
MEMBER_HALTED = "member.halted"
CAPABILITY_CHANGED = "capability.changed"

# ── external writes (AU-05; the deliberate D-13 amendment) ──────────────────
#
# The idempotency ledger is DERIVED from these events, not stored in a table
# (``04`` section 6). ``write.reserved`` is written before the effect,
# ``write.done`` after it succeeds, and ``write.skipped`` when a retry finds
# the same key already done. A ``reserved`` with no later ``done`` is the crash
# case: the action may or may not have happened, so the retry asks a human
# instead of silently duplicating it.
#
# D-13 froze the vocabulary, so this is an explicit amendment rather than a
# casual addition: three audit-only kinds, reduced as no-ops so old and new
# projects replay through the same reducer.
WRITE_RESERVED = "write.reserved"
WRITE_DONE = "write.done"
WRITE_SKIPPED = "write.skipped"


#: Every kind the system may ever write. Used to validate on append.
ALL_KINDS = frozenset({
    PROJECT_CREATED, PROJECT_RESUMED, PROJECT_PAUSED, PROJECT_CLOSED, PROJECT_STALLED,
    MILESTONE_CREATED, MILESTONE_READY, MILESTONE_DONE, MILESTONE_FAILED,
    ITEM_CREATED, ITEM_READY, ITEM_CLAIMED, ITEM_RELEASED, ITEM_PROGRESS,
    ITEM_DONE, ITEM_FAILED, ITEM_BLOCKED, ITEM_INVALIDATED, ITEM_CANCELLED,
    PLAN_REVISED, IDEA_PROPOSED,
    OP_SUBMITTED, OP_APPLIED, OP_REJECTED,
    LEASE_EXPIRED, BUDGET_WARN, BUDGET_STOP, DEADLINE_REACHED,
    APPROVAL_REQUESTED, APPROVAL_GRANTED, APPROVAL_REJECTED,
    MESSAGE_SENT, MESSAGE_READ,
    MEMBER_ADDED, MEMBER_REMOVED, MEMBER_HALTED, CAPABILITY_CHANGED,
    WRITE_RESERVED, WRITE_DONE, WRITE_SKIPPED,
})

#: Kinds the reducer implements. Anything outside this set that is replayed
#: raises :class:`UnknownEventKind` — loudly, never silently ignored.
REDUCED_KINDS = frozenset({
    PROJECT_CREATED, PROJECT_RESUMED, PROJECT_PAUSED, PROJECT_CLOSED, PROJECT_STALLED,
    MILESTONE_CREATED, MILESTONE_READY, MILESTONE_DONE, MILESTONE_FAILED,
    ITEM_CREATED, ITEM_READY, ITEM_CLAIMED, ITEM_RELEASED, ITEM_PROGRESS,
    ITEM_DONE, ITEM_FAILED, ITEM_BLOCKED, ITEM_INVALIDATED, ITEM_CANCELLED,
    PLAN_REVISED, IDEA_PROPOSED,
    OP_SUBMITTED, OP_APPLIED, OP_REJECTED,
    LEASE_EXPIRED, BUDGET_WARN, BUDGET_STOP, DEADLINE_REACHED,
    APPROVAL_REQUESTED, APPROVAL_GRANTED, APPROVAL_REJECTED,
    # Chat is an append-only audit stream, not a projection. The reducer is a
    # no-op; ``agents.group.messages`` reads these events back as the transcript.
    MESSAGE_SENT, MESSAGE_READ,
    # A capability change edits the FOLDER (agents/, skills/, tools/, mcp/),
    # which is the folder's own truth and not a projection of the log. It is
    # reduced as a no-op so replay never raises, while remaining a first-class
    # event that the audit trail and the UI both see.
    CAPABILITY_CHANGED,
    # AU-05's idempotency ledger is derived by scanning these events, so there
    # is nothing to project. They are still events: the audit trail, the
    # rebuild path, and a later reader all need to see them.
    WRITE_RESERVED, WRITE_DONE, WRITE_SKIPPED,
})

#: A parked item keeps its status but carries this reason, so the stall
#: detector can tell "waiting on a human" from "stuck" (``02`` section 6.2).
AWAITING_APPROVAL = "awaiting_approval"

# ── statuses ────────────────────────────────────────────────────────────────

ITEM_STATUS = (
    "todo", "ready", "doing", "blocked", "done", "failed",
    "cancelled", "invalidated",
)
MILESTONE_STATUS = (
    "todo", "ready", "doing", "blocked", "done", "failed", "cancelled",
)
PROJECT_STATUS = (
    "created", "running", "paused", "stalled", "done", "failed", "cancelled",
)

#: A subject in one of these is finished; nothing transitions out (except a
#: new subject taking its place — see ``item.invalidated`` handling).
ITEM_TERMINAL = frozenset({"done", "failed", "cancelled", "invalidated"})
MILESTONE_TERMINAL = frozenset({"done", "failed", "cancelled"})

#: The one kind that puts an item into each non-initial status (DM-19).
ITEM_STATUS_KIND = {
    "ready": ITEM_READY,
    "doing": ITEM_CLAIMED,
    "blocked": ITEM_BLOCKED,
    "done": ITEM_DONE,
    "failed": ITEM_FAILED,
    "cancelled": ITEM_CANCELLED,
    "invalidated": ITEM_INVALIDATED,
}
MILESTONE_STATUS_KIND = {
    "ready": MILESTONE_READY,
    "done": MILESTONE_DONE,
    "failed": MILESTONE_FAILED,
}


class UnknownEventKind(Exception):
    """Raised when replay meets a kind the reducer does not implement.

    Deliberately loud: silently ignoring an event would make ``rebuild()``
    produce a state that quietly disagrees with the incremental path, which is
    exactly the failure the event log exists to prevent.
    """

    def __init__(self, kind: str, seq: int | None = None) -> None:
        where = f" at seq {seq}" if seq is not None else ""
        super().__init__(
            f"unknown event kind {kind!r}{where}. If this kind is new, add a "
            f"reducer for it in agents/group/store.py before writing it."
        )
