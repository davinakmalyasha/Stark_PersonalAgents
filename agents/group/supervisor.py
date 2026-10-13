"""agents.group.supervisor — the deterministic tick (Phase 1).

``docs/group-projects/02-runtime.md`` sections 1-6.

The one rule this module exists to hold:

    **the orchestrator decides what work exists; the supervisor decides when
    that work happens.**

So the supervisor **makes no LLM calls** (RT-01). It answers six questions and
judges none of them:

1. a dependency finished — start the dependent?
2. a lease expired (a member died) — block or re-dispatch?
3. budget or deadline hit — halt dispatch?
4. nothing has happened for N ticks — is this deadlock?
5. an op was queued — apply it
6. the completion predicate passed — close the project

It exists so the orchestrator can *sleep*: a multi-day project that kept an
LLM context resident across the wait would be expensive and compaction-fragile.

What is deliberately NOT here
-----------------------------
- **No member execution.** Phase 1 has no members: items are driven by hand
  through ops. ``dispatch`` is present but inert until a runner is supplied
  (Phase 2) — it never claims an item it cannot run.
- **No LLM.** Ever. If a tick needs judgement, the answer is to emit an event
  that wakes the orchestrator.
- **No per-project thread.** One supervisor thread for all projects, so the
  single-writer guarantee is structural rather than a lock per project
  (``02`` section 2.1).
"""
from __future__ import annotations

import threading
import time
from collections.abc import Callable
from typing import Any

from . import events as ev
from .store import ProjectStore, evaluate_completion

#: Default lease for a claimed item. Phase 2 renews while there is progress.
LEASE_SECONDS = 30 * 60.0

#: Consecutive no-event ticks before a project is called stalled (``02`` 6.2).
STALL_TICKS = 50

#: Fraction of budget at which a one-off warning is emitted.
BUDGET_WARN_AT = 0.8

#: Op kinds this phase can apply. Anything else is rejected, loudly, with the
#: op left visible on the board rather than silently dropped.
_PHASE_1_OPS = frozenset({
    "add_milestone", "add_item", "update_item", "cancel_item",
    "invalidate_item", "pause", "resume", "close", "set_completion",
})


class OpRejected(Exception):
    """A queued op is invalid. The project is left untouched."""


def recover_all_projects(store: ProjectStore, project_ids: list[str]) -> int:
    """Boot recovery (``02`` §5.1): full rebuild, never trust the tables.

    Distinct from tick phase 0 on purpose. Phase 0 replays only the tail, which
    keeps the hot path cheap but cannot repair a row an *earlier* event created.
    Boot is where the strong guarantee is bought: rebuild every projection from
    the log, so a crash mid-transaction, a manual edit, or a bug cannot survive
    a restart.

    Returns the number of projects rebuilt.
    """
    rebuilt = 0
    for project_id in project_ids:
        store.rebuild(project_id)
        rebuilt += 1
    return rebuilt


class Supervisor:
    """Ticks one or more projects. One instance per process."""

    def __init__(self, store: ProjectStore, *,
                 runner: Any = None,
                 lease_seconds: float = LEASE_SECONDS,
                 stall_ticks: int = STALL_TICKS,
                 clock: Callable[[], float] = time.time,
                 log: Callable[[str], None] | None = None) -> None:
        self.store = store
        self.runner = runner          # Phase 2 fills this in; None in Phase 1
        self.lease_seconds = lease_seconds
        self.stall_ticks = stall_ticks
        self.clock = clock
        self._log = log or (lambda msg: None)
        self._idle: dict[str, int] = {}
        self._last_seq: dict[str, int] = {}
        self._invalidate_burst: dict[str, dict] = {}
        self._lock = threading.RLock()

    # ── the tick ────────────────────────────────────────────────────────

    def tick(self, project_id: str) -> dict:
        """Run all phases once. Never raises for a project-level problem."""
        report: dict = {"project": project_id, "phases": {}, "halted": None}
        try:
            report["phases"]["recover"] = self.store.ensure_consistent(project_id)

            applied = self._apply_ops(project_id)
            report["phases"]["ops"] = applied

            halt = self._gate(project_id)
            report["halted"] = halt
            if halt:
                self._bump_idle(project_id, report)
                return report

            report["phases"]["reap"] = self._reap(project_id)
            report["phases"]["milestones"] = self._milestones(project_id)
            report["phases"]["readiness"] = self._readiness(project_id)
            report["phases"]["dispatch"] = self._dispatch(project_id)
            report["phases"]["collect"] = self._collect(project_id)
            report["phases"]["verify"] = self._verify(project_id)
            report["phases"]["invalidate"] = self._invalidate(project_id)
            report["phases"]["complete"] = self._complete(project_id)
        except Exception as exc:  # noqa: BLE001 - RT-03: a bad tick must not kill the loop
            self._log(f"[SUPERVISOR] tick failed for {project_id}: {exc}")
            report["error"] = str(exc)
        self._bump_idle(project_id, report)
        return report

    def run_forever(self, project_ids: Callable[[], list[str]],
                    tick_seconds: float = 15.0,
                    stop: threading.Event | None = None) -> None:
        """The heartbeat. Reuses the 15s cadence the task scheduler already has."""
        while not (stop and stop.is_set()):
            for project_id in project_ids():
                self.tick(project_id)
            time.sleep(tick_seconds)

    # ── phase 1: apply ops ──────────────────────────────────────────────

    def _apply_ops(self, project_id: str) -> int:
        applied = 0
        for op in self.store.ops(project_id, state="pending"):
            try:
                self._apply_one(project_id, op)
            except OpRejected as exc:
                self.store.append(project_id, ev.OP_REJECTED, "supervisor",
                                  data={"op_id": op["id"], "error": str(exc)})
            except Exception as exc:  # noqa: BLE001 - any op failure rejects that op
                self.store.append(project_id, ev.OP_REJECTED, "supervisor",
                                  data={"op_id": op["id"], "error": f"{type(exc).__name__}: {exc}"})
            else:
                self.store.append(project_id, ev.OP_APPLIED, "supervisor",
                                  data={"op_id": op["id"]})
                applied += 1
        return applied

    def _apply_one(self, project_id: str, op: dict) -> None:
        kind = op.get("kind") or ""
        payload = op.get("payload") or {}
        if kind not in _PHASE_1_OPS:
            raise OpRejected(f"{kind!r} is not implemented in Phase 1")

        if kind == "pause":
            self.store.append(project_id, ev.PROJECT_PAUSED, "orchestrator")
            return
        if kind == "resume":
            self.store.append(project_id, ev.PROJECT_RESUMED, "orchestrator")
            return
        if kind == "close":
            self.store.append(project_id, ev.PROJECT_CLOSED, "orchestrator",
                              data={"status": payload.get("status") or "done"})
            return
        if kind == "set_completion":
            # Completion lives on the project row; re-emitting the create event
            # would rewrite history, so update the projection via a close-free
            # path: record it as a plan revision carrying the new spec.
            raise OpRejected("set_completion is Phase 3 (needs completion.json write)")
        if kind == "add_milestone":
            if not str(payload.get("id") or "").strip():
                raise OpRejected("add_milestone requires an id")
            self.store.append(project_id, ev.MILESTONE_CREATED, "orchestrator",
                              milestone_id=payload["id"], data=payload)
            return
        if kind == "add_item":
            self._op_add_item(project_id, payload)
            return
        if kind == "update_item":
            self._op_update_item(project_id, payload)
            return
        if kind == "cancel_item":
            self._require_item(project_id, payload)
            self.store.append(project_id, ev.ITEM_CANCELLED, "orchestrator",
                              item_id=payload["item_id"])
            return
        if kind == "invalidate_item":
            self._require_item(project_id, payload)
            self.store.append(project_id, ev.ITEM_INVALIDATED, "orchestrator",
                              item_id=payload["item_id"],
                              data={"reason": payload.get("reason") or "operator"})
            return

    def _op_add_item(self, project_id: str, payload: dict) -> None:
        milestone_id = str(payload.get("milestone_id") or "")
        if milestone_id not in {m["id"] for m in self.store.milestones(project_id)}:
            raise OpRejected(f"unknown milestone {milestone_id!r}")
        if not (payload.get("acceptance") or []):
            # DM-06: an item without acceptance criteria can never be "done".
            raise OpRejected("item requires non-empty acceptance criteria")
        depends = list(payload.get("depends_on") or [])
        # DM-14: dependencies are item->item WITHIN one milestone.
        sibling_ids = {i["id"] for i in self.store.items(project_id)
                       if i["milestone_id"] == milestone_id}
        foreign = [d for d in depends if d not in sibling_ids]
        if foreign:
            raise OpRejected(
                f"cross-milestone dependency not allowed (DM-14): {foreign}"
            )
        self._check_cycle(project_id, payload.get("id"), depends, milestone_id)
        self.store.append(project_id, ev.ITEM_CREATED, "orchestrator",
                          milestone_id=milestone_id, item_id=payload.get("id"),
                          data=payload)

    def _op_update_item(self, project_id: str, payload: dict) -> None:
        self._require_item(project_id, payload)
        raise OpRejected("update_item is Phase 3 (no field-level event yet)")

    def _require_item(self, project_id: str, payload: dict) -> None:
        item_id = str(payload.get("item_id") or "")
        if not item_id:
            raise OpRejected("item_id is required")
        if self.store.item(item_id) is None:
            raise OpRejected(f"unknown item {item_id!r}")

    def _check_cycle(self, project_id: str, new_id: str | None,
                     depends: list[str], milestone_id: str) -> None:
        """DM-04: reject a dependency that would close a cycle."""
        if not new_id:
            return
        graph: dict[str, list[str]] = {}
        for item in self.store.items(project_id):
            if item["milestone_id"] == milestone_id:
                graph[item["id"]] = list(item.get("depends_on") or [])
        graph[str(new_id)] = list(depends)
        seen: set[str] = set()

        def walk(node: str, path: list[str]) -> None:
            if node in path:
                raise OpRejected(f"dependency cycle: {' -> '.join(path + [node])}")
            if node in seen:
                return
            for nxt in graph.get(node, []):
                walk(nxt, path + [node])
            seen.add(node)

        walk(str(new_id), [])

    # ── phase 2: gates ──────────────────────────────────────────────────

    def _gate(self, project_id: str) -> str | None:
        """Return a halt reason, or None to keep dispatching."""
        project = self.store.project(project_id)
        if not project:
            return "no project"
        status = project["status"]
        if status in ("done", "failed", "cancelled"):
            return "closed"
        if status == "paused":
            return "paused"
        if project.get("halt_reason") in ("budget", "deadline"):
            return project["halt_reason"]

        cap = project.get("budget_usd")
        spent = float(project.get("spent_usd") or 0)
        if cap:
            cap = float(cap)
            if spent >= cap:
                self.store.append(project_id, ev.BUDGET_STOP, "supervisor",
                                  data={"spent_usd": spent, "budget_usd": cap})
                return "budget"
            if not project.get("warned") and spent >= cap * BUDGET_WARN_AT:
                self.store.append(project_id, ev.BUDGET_WARN, "supervisor",
                                  data={"spent_usd": spent, "budget_usd": cap})

        deadline = project.get("deadline")
        if deadline and self.clock() >= float(deadline):
            self.store.append(project_id, ev.DEADLINE_REACHED, "supervisor",
                              data={"deadline": deadline})
            return "deadline"
        return None

    # ── phase 3: reap expired leases ────────────────────────────────────

    def _reap(self, project_id: str) -> int:
        now = self.clock()
        reaped = 0
        for item in self.store.items(project_id, status="doing"):
            expires = item.get("lease_expires")
            if expires is None or float(expires) > now:
                continue
            self.store.append(project_id, ev.LEASE_EXPIRED, "supervisor",
                              item_id=item["id"],
                              data={"lease_owner": item.get("lease_owner")})
            attempts = int(item.get("attempts") or 0)
            if attempts >= int(item.get("max_attempts") or 3):
                self.store.append(project_id, ev.ITEM_FAILED, "supervisor",
                                  item_id=item["id"],
                                  data={"reason": "lease_expired"})
            else:
                self.store.append(project_id, ev.ITEM_BLOCKED, "supervisor",
                                  item_id=item["id"],
                                  data={"reason": "lease_expired"})
            reaped += 1
        return reaped

    # ── phase 4a: milestones ────────────────────────────────────────────

    def _milestones(self, project_id: str) -> int:
        """Advance milestone readiness and completion.

        Milestones are walked in ``seq_no`` order and the local copy is updated
        as we go, so a milestone that completes in THIS pass is immediately
        visible to its dependents. Without that, finishing ``m1`` would only
        release ``m2`` on the following tick — a 15-second lag per milestone
        for no reason, and a deviation from "readiness is derived every tick".
        """
        changed = 0
        milestones = {m["id"]: dict(m) for m in self.store.milestones(project_id)}
        ordered = sorted(milestones.values(),
                         key=lambda m: (m.get("seq_no") or 0, m["id"]))
        items = self.store.items(project_id)

        for milestone in ordered:
            if milestone["status"] == "todo":
                deps = [milestones.get(d) for d in (milestone.get("depends_on") or [])]
                if any(d is None for d in deps):
                    continue  # dangling dep: leave it visible, do not guess
                if any(d["status"] == "failed" for d in deps):
                    self.store.append(project_id, ev.MILESTONE_FAILED, "supervisor",
                                      milestone_id=milestone["id"])
                    milestone["status"] = "failed"
                    changed += 1
                elif all(d["status"] == "done" for d in deps):
                    self.store.append(project_id, ev.MILESTONE_READY, "supervisor",
                                      milestone_id=milestone["id"])
                    milestone["status"] = "ready"
                    changed += 1

            if milestone["status"] in ("ready", "doing"):
                mine = [i for i in items if i["milestone_id"] == milestone["id"]]
                # BR-09: done when every item is done. An invalidated item keeps
                # the milestone open until the plan replaces it.
                if mine and all(i["status"] == "done" for i in mine):
                    self.store.append(project_id, ev.MILESTONE_DONE, "supervisor",
                                      milestone_id=milestone["id"])
                    milestone["status"] = "done"
                    changed += 1
        return changed

    # ── phase 4b: item readiness ────────────────────────────────────────

    def _readiness(self, project_id: str) -> int:
        changed = 0
        milestones = {m["id"]: m for m in self.store.milestones(project_id)}
        items = {i["id"]: i for i in self.store.items(project_id)}
        for item in items.values():
            if item["status"] != "todo":
                continue
            milestone = milestones.get(item["milestone_id"])
            if milestone is None or milestone["status"] not in ("ready", "doing"):
                continue  # RT-15: never dispatch into a gated milestone
            deps = [items.get(d) for d in (item.get("depends_on") or [])]
            if any(d is None for d in deps):
                continue
            if any(d["status"] == "failed" for d in deps):
                continue  # BR-01: stays todo; reason is visible via the dep
            if all(d["status"] == "done" for d in deps):
                self.store.append(project_id, ev.ITEM_READY, "supervisor",
                                  item_id=item["id"])
                changed += 1
        return changed

    # ── phase 5: dispatch ───────────────────────────────────────────────

    def _dispatch(self, project_id: str) -> int:
        """Claim ready items — but ONLY when a runner exists to execute them.

        Phase 1 has no members, so this is inert by design: claiming an item we
        cannot run would leave it `doing` until the lease expired, which is
        worse than leaving it `ready`.
        """
        if self.runner is None:
            return 0
        claimed = 0
        for item in self.store.items(project_id, status="ready"):
            owner = self.runner.member_for(item) if hasattr(self.runner, "member_for") else None
            self.store.append(
                project_id, ev.ITEM_CLAIMED, "supervisor", item_id=item["id"],
                data={"lease_owner": owner,
                      "lease_expires": self.clock() + self.lease_seconds},
            )
            claimed += 1
        return claimed

    # ── phases 6-7: collect + verify (Phase 2) ──────────────────────────

    def _collect(self, project_id: str) -> int:
        if self.runner is None:
            return 0
        return int(self.runner.collect(project_id) or 0)

    def _verify(self, project_id: str) -> int:
        if self.runner is None:
            return 0
        return int(self.runner.verify(project_id) or 0)

    # ── phase 8: invalidation bursts ────────────────────────────────────

    def _invalidate(self, project_id: str) -> int:
        """Coalesce invalidated items into ONE orchestrator wake (BR-12, RT-14).

        One discovery often invalidates several items — "the vendor changed the
        CSV format" is not five separate problems. Five re-plan calls would
        describe the same thing five times and race each other.
        """
        pending = [i["id"] for i in self.store.items(project_id, status="invalidated")]
        burst = self._invalidate_burst.get(project_id)
        if not pending:
            if burst:
                self._invalidate_burst.pop(project_id, None)
            return 0
        new_ids = sorted(set(pending) - set((burst or {}).get("ids") or []))
        if not new_ids:
            return 0
        merged = sorted(set(pending))
        self._invalidate_burst[project_id] = {"ids": merged, "at": self.clock()}
        self.store.append(project_id, ev.PLAN_REVISED, "supervisor",
                          data={"invalidated": merged, "new": new_ids})
        return len(new_ids)

    # ── phase 9: completion ─────────────────────────────────────────────

    def _complete(self, project_id: str) -> str | None:
        project = self.store.project(project_id)
        if not project or project["status"] in ("done", "failed", "cancelled"):
            return None
        passed, reason = evaluate_completion(self.store, project_id)
        if passed:
            self.store.append(project_id, ev.PROJECT_CLOSED, "supervisor",
                              data={"status": "done", "reason": reason})
            return reason
        return None

    # ── stall detection ─────────────────────────────────────────────────

    def _bump_idle(self, project_id: str, report: dict) -> None:
        """Count ticks that produced no events, and stall on too many.

        Counts EVENTS, not activity: a project doing real work emits events
        continuously, so a quiet-but-working member never looks idle. Legitimate
        waits are exempt (``02`` 6.2) — otherwise every overnight approval wait
        would look like a failure.
        """
        head = self.store.max_seq(project_id)
        prev = self._last_seq.get(project_id)
        self._last_seq[project_id] = head
        if prev is None or head > prev:
            self._idle[project_id] = 0
            return

        project = self.store.project(project_id)
        if not project or project["status"] in ("done", "failed", "cancelled", "paused"):
            return
        if self._has_legitimate_wait(project_id):
            self._idle[project_id] = 0
            return

        self._idle[project_id] = self._idle.get(project_id, 0) + 1
        if self._idle[project_id] >= self.stall_ticks:
            self._idle[project_id] = 0
            self.store.append(project_id, ev.PROJECT_STALLED, "supervisor",
                              data={"idle_ticks": self.stall_ticks})
            report["halted"] = "stalled"

    def _has_legitimate_wait(self, project_id: str) -> bool:
        """Waits that must NOT be counted as a stall (``02`` 6.2).

        Without this, every project that pauses overnight for an approval — or
        while the orchestrator re-plans — would look like a failure.
        """
        return any(
            i["status"] == "invalidated"                              # re-plan open
            or (i["status"] == "blocked"
                and i.get("blocked_reason") == "awaiting_approval")   # waiting on you
            for i in self.store.items(project_id)
        )
