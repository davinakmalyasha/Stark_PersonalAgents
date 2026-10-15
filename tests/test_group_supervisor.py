"""Phase 1 tests — supervisor, milestones, events-as-truth.

Test project: ``synthetic-ledger`` (``06-phases-and-open-questions.md``,
Phase 1) — 5 milestones x 12 items, a hand-written dependency graph, and no
members. Items are driven by hand through events, because Phase 1 has no
runner: the point of this phase is that the *board* advances correctly with no
model in the loop.

The import goes through ``tests/_util.py::agents_shim`` because
``agents/__init__.py`` pulls the whole agent stack, which needs the real
``tools`` package that other suites in this directory replace with a stub.
"""
from __future__ import annotations

import os
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.dirname(HERE)
if REPO_ROOT not in sys.path:
    sys.path.insert(0, REPO_ROOT)
if HERE not in sys.path:
    sys.path.insert(0, HERE)

from _util import agents_shim

_CHECK = [{"kind": "check", "expr": "artifacts/out.md exists"}]


def ledger_spec(budget_usd: float = 10.0) -> dict:
    """5 milestones x 12 items, no members."""
    def item(item_id, title, depends_on=None):
        spec = {"id": item_id, "title": title, "acceptance": list(_CHECK)}
        if depends_on:
            spec["depends_on"] = depends_on
        return spec

    return {
        "goal": "reconcile the synthetic ledger",
        "owner": "localhost",
        "budget_usd": budget_usd,
        "deadline": None,
        "completion": {
            "kind": "all_milestones_done",
            "also_require": ["no_item_status_failed"],
        },
        "milestones": [
            {"id": "m1", "title": "Research", "items": [
                item("i1", "collect"),
                item("i2", "normalise"),
                item("i3", "summarise", ["i2"]),
            ]},
            {"id": "m2", "title": "Match", "depends_on": ["m1"], "items": [
                item("i4", "load POs"), item("i5", "join"), item("i6", "flag"),
            ]},
            {"id": "m3", "title": "Reconcile", "depends_on": ["m2"], "items": [
                item("i7", "diff"), item("i8", "explain deltas"),
            ]},
            {"id": "m4", "title": "Review", "depends_on": ["m3"], "items": [
                item("i9", "human pass"), item("i10", "sign off"),
            ]},
            {"id": "m5", "title": "Report", "depends_on": ["m4"], "items": [
                item("i11", "write report"), item("i12", "publish"),
            ]},
        ],
    }


class Rig:
    """A temp project + store + supervisor, torn down on exit."""

    def __init__(self, **sup_kwargs):
        self._tmp = tempfile.TemporaryDirectory()
        self._shim = agents_shim("agents.group.events", "agents.group.store",
                                 "agents.group.supervisor")
        modules = self._shim.__enter__()
        self.ev = modules["agents.group.events"]
        self.store_mod = modules["agents.group.store"]
        self.sup_mod = modules["agents.group.supervisor"]
        self.path = os.path.join(self._tmp.name, ".gp", "state.db")
        self.store = self.store_mod.ProjectStore(self.path)
        self.sup = self.sup_mod.Supervisor(self.store, **sup_kwargs)
        self.pid = "synthetic-ledger"

    def create(self, spec: dict | None = None) -> None:
        self.store.append(self.pid, self.ev.PROJECT_CREATED, "human",
                          data=spec or ledger_spec())
        self.store.append(self.pid, self.ev.PROJECT_RESUMED, "supervisor")

    def drive(self, max_ticks: int = 40) -> dict:
        """Tick, complete whatever became ready, repeat. No members involved.

        Returns the final tick report. Stops on done / halt so a stall or
        deadlock surfaces as a finite result rather than a hang. It does NOT
        stop on "nothing ready this tick": milestone completion legitimately
        lags its items by one tick, so a quiet tick is not a dead one.
        """
        report = {}
        for _ in range(max_ticks):
            report = self.sup.tick(self.pid)
            project = self.store.project(self.pid)
            if project["status"] in ("done", "failed", "cancelled"):
                return report
            if report.get("halted"):
                return report
            for item in self.store.items(self.pid, status="ready"):
                self.store.append(self.pid, self.ev.ITEM_DONE, "test",
                                  item_id=item["id"])
        return report

    def close(self) -> None:
        try:
            self.store.close()
        finally:
            self._shim.__exit__(None, None, None)
            self._tmp.cleanup()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()
        return False


# ════════════════════════════════════════════════════════════════════════════
#  The synthetic ledger reaches done
# ════════════════════════════════════════════════════════════════════════════


def test_synthetic_ledger_completes():
    with Rig() as rig:
        rig.create()
        assert rig.store.project(rig.pid)["status"] == "running"
        assert len(rig.store.milestones(rig.pid)) == 5
        assert len(rig.store.items(rig.pid)) == 12

        rig.drive()

        assert rig.store.project(rig.pid)["status"] == "done", (
            "the synthetic ledger did not reach done"
        )
        assert all(m["status"] == "done" for m in rig.store.milestones(rig.pid))
        assert all(i["status"] == "done" for i in rig.store.items(rig.pid))


def test_milestone_order_is_respected():
    """A later milestone's items must never become ready early (RT-15)."""
    with Rig() as rig:
        rig.create()
        rig.sup.tick(rig.pid)
        ready = {i["id"] for i in rig.store.items(rig.pid, status="ready")}
        assert ready == {"i1", "i2"}, f"m1 should offer i1+i2, got {sorted(ready)}"
        # m2 depends on m1, so none of its items may be ready yet.
        assert not (ready & {"i4", "i5", "i6"}), "m2 leaked before m1 finished"

        for _ in range(3):
            rig.sup.tick(rig.pid)
            for item in rig.store.items(rig.pid, status="ready"):
                rig.store.append(rig.pid, rig.ev.ITEM_DONE, "test", item_id=item["id"])
        assert rig.store.project(rig.pid) and True  # ran without error


def test_milestone_completion_propagates_in_the_same_tick():
    """RT-13: completing m1 releases m2 without a second tick of lag."""
    with Rig() as rig:
        rig.create()
        rig.sup.tick(rig.pid)
        for item_id in ("i1", "i2"):
            rig.store.append(rig.pid, rig.ev.ITEM_DONE, "test", item_id=item_id)
        rig.sup.tick(rig.pid)
        rig.store.append(rig.pid, rig.ev.ITEM_DONE, "test", item_id="i3")

        # ONE tick must both finish m1 AND release m2's items.
        rig.sup.tick(rig.pid)
        by_id = {m["id"]: m for m in rig.store.milestones(rig.pid)}
        assert by_id["m1"]["status"] == "done"
        assert by_id["m2"]["status"] == "ready", (
            "m2 waited a tick after its dependency completed"
        )
        ready = {i["id"] for i in rig.store.items(rig.pid, status="ready")}
        assert ready == {"i4", "i5", "i6"}, f"m2 items not released, got {sorted(ready)}"


def test_dependency_within_a_milestone_gates():
    """i3 depends on i2, so it must not be ready while i2 is open (BR-01)."""
    with Rig() as rig:
        rig.create()
        rig.sup.tick(rig.pid)
        ready = {i["id"] for i in rig.store.items(rig.pid, status="ready")}
        assert "i3" not in ready, "i3 became ready before its dependency i2"
        rig.store.append(rig.pid, rig.ev.ITEM_DONE, "test", item_id="i2")
        rig.sup.tick(rig.pid)
        ready = {i["id"] for i in rig.store.items(rig.pid, status="ready")}
        assert "i3" in ready, "i3 did not become ready after i2 completed"


# ════════════════════════════════════════════════════════════════════════════
#  Leases
# ════════════════════════════════════════════════════════════════════════════


def test_expired_lease_is_reaped_then_requeued_until_max_attempts():
    """RT-05: every `doing` item has a bounded lifetime.

    A lease expiry means the member died, not that the work is impossible, so
    the item blocks (recorded in the log) and goes straight back in the queue.
    It never sits `blocked` on the board: the retry is bounded by `attempts`,
    which converts it to a failure instead of looping forever.
    """
    with Rig() as rig:
        rig.create()
        rig.sup.tick(rig.pid)

        # First expiry: reaped, then requeued in the same tick.
        rig.store.append(rig.pid, rig.ev.ITEM_CLAIMED, "member:x", item_id="i1",
                         data={"lease_owner": "x", "lease_expires": 1.0})
        assert rig.store.item("i1")["status"] == "doing"
        rig.sup.tick(rig.pid)
        kinds = [e["kind"] for e in rig.store.events(rig.pid)]
        assert rig.ev.LEASE_EXPIRED in kinds, "expired lease produced no event"
        assert rig.ev.ITEM_BLOCKED in kinds, "expiry did not record a block"
        item = rig.store.item("i1")
        assert item["status"] == "ready", f"not requeued: {item['status']}"
        assert item["lease_owner"] is None, "the dead member kept its lease"

        # Keep killing it; the attempts ceiling must win.
        for _ in range(8):
            if rig.store.item("i1")["status"] == "failed":
                break
            rig.store.append(rig.pid, rig.ev.ITEM_CLAIMED, "member:x", item_id="i1",
                             data={"lease_owner": "x", "lease_expires": 1.0})
            rig.sup.tick(rig.pid)
        assert rig.store.item("i1")["status"] == "failed", (
            f"never failed after repeated expiries: {rig.store.item('i1')['status']}"
        )
        assert rig.store.item("i1")["attempts"] == 3


def test_live_lease_is_not_reaped():
    with Rig() as rig:
        rig.create()
        rig.sup.tick(rig.pid)
        rig.store.append(rig.pid, rig.ev.ITEM_CLAIMED, "member:x", item_id="i1",
                         data={"lease_owner": "x",
                               "lease_expires": rig.sup.clock() + 3600})
        rig.sup.tick(rig.pid)
        assert rig.store.item("i1")["status"] == "doing", "a live lease was reaped"


def test_an_item_being_worked_is_not_a_stall():
    """A member running silently is not a deadlock.

    Found by the first real run: a live LLM item produced no events for 22
    consecutive ticks while it worked. Without this exemption anything slower
    than the stall threshold would be declared deadlocked mid-work.
    """
    with Rig(stall_ticks=3) as rig:
        rig.create()
        rig.sup.tick(rig.pid)
        rig.store.append(rig.pid, rig.ev.ITEM_CLAIMED, "member:x", item_id="i1",
                         data={"lease_owner": "x",
                               "lease_expires": rig.sup.clock() + 10_000})
        for _ in range(10):
            rig.sup.tick(rig.pid)
        assert rig.store.project(rig.pid)["halt_reason"] != "stalled", (
            "an item actively being worked was reported as a deadlock"
        )
        assert rig.store.item("i1")["status"] == "doing"


def test_an_expired_lease_stops_exempting_the_item():
    """The exemption is self-limiting: expiry hands the item to the reaper."""
    with Rig(stall_ticks=3) as rig:
        rig.create()
        rig.sup.tick(rig.pid)
        rig.store.append(rig.pid, rig.ev.ITEM_CLAIMED, "member:x", item_id="i1",
                         data={"lease_owner": "x", "lease_expires": 1.0})
        for _ in range(8):
            rig.sup.tick(rig.pid)
        # The lease expired, so the item is no longer `doing` with an owner; the
        # project may legitimately stall once nothing else can progress.
        assert rig.store.item("i1")["status"] in ("ready", "blocked", "failed")


# ════════════════════════════════════════════════════════════════════════════
#  Invalidation
# ════════════════════════════════════════════════════════════════════════════


def test_invalidation_burst_wakes_the_orchestrator_once():
    """RT-14 / BR-12: five invalidations produce ONE plan.revised."""
    with Rig() as rig:
        rig.create()
        rig.sup.tick(rig.pid)
        for item_id in ("i1", "i2"):
            rig.store.append(rig.pid, rig.ev.ITEM_INVALIDATED, "member:dev",
                             item_id=item_id, data={"reason": "vendor changed format"})

        before = sum(1 for e in rig.store.events(rig.pid)
                     if e["kind"] == rig.ev.PLAN_REVISED)
        rig.sup.tick(rig.pid)
        after = [e for e in rig.store.events(rig.pid)
                 if e["kind"] == rig.ev.PLAN_REVISED]
        assert len(after) == before + 1, f"expected one plan.revised, got {len(after) - before}"

        # A second tick with the same invalidated set must NOT wake again.
        rig.sup.tick(rig.pid)
        again = [e for e in rig.store.events(rig.pid)
                 if e["kind"] == rig.ev.PLAN_REVISED]
        assert len(again) == len(after), "the burst re-fired on a quiet tick"


def test_invalidation_does_not_burn_attempts():
    """DM-15: a plan error is not an execution error."""
    with Rig() as rig:
        rig.create()
        rig.sup.tick(rig.pid)
        rig.store.append(rig.pid, rig.ev.ITEM_CLAIMED, "member:x", item_id="i1",
                         data={"lease_owner": "x", "lease_expires": 1e12})
        assert rig.store.item("i1")["attempts"] == 1
        rig.store.append(rig.pid, rig.ev.ITEM_INVALIDATED, "member:x", item_id="i1",
                         data={"reason": "wrong assumption"})
        assert rig.store.item("i1")["status"] == "invalidated"
        assert rig.store.item("i1")["attempts"] == 1, (
            "invalidated must not increment attempts"
        )


# ════════════════════════════════════════════════════════════════════════════
#  Gates
# ════════════════════════════════════════════════════════════════════════════


def test_budget_stop_halts_dispatch():
    with Rig() as rig:
        rig.create(ledger_spec(budget_usd=1.0))
        rig.sup.tick(rig.pid)
        rig.store.append(rig.pid, rig.ev.ITEM_DONE, "test", item_id="i1",
                         data={"cost_usd": 1.5})
        report = rig.sup.tick(rig.pid)
        assert report["halted"] == "budget", report
        assert rig.store.project(rig.pid)["halt_reason"] == "budget"
        # Halting must not close the project: it stays readable and resumable.
        assert rig.store.project(rig.pid)["status"] == "running"


def test_deadlock_stalls_instead_of_spinning():
    """BR-05: no possible progress must stop and ask, not loop forever."""
    with Rig(stall_ticks=3) as rig:
        rig.create()
        # A milestone whose dependency can never complete.
        rig.store.append(rig.pid, rig.ev.MILESTONE_CREATED, "human",
                         milestone_id="mX", data={"id": "mX", "title": "orphan"})
        rig.store.append(rig.pid, rig.ev.MILESTONE_CREATED, "human",
                         milestone_id="mY",
                         data={"id": "mY", "title": "blocked", "depends_on": ["nope"]})
        for _ in range(6):
            rig.sup.tick(rig.pid)
        assert rig.store.project(rig.pid)["halt_reason"] == "stalled", (
            "a deadlocked project did not stall"
        )


# ════════════════════════════════════════════════════════════════════════════
#  Events are the truth
# ════════════════════════════════════════════════════════════════════════════


def _snapshot(rig, store=None):
    store = store or rig.store
    return {
        "project": {k: v for k, v in (store.project(rig.pid) or {}).items()
                    if k != "updated_at"},
        "milestones": sorted((m["id"], m["status"]) for m in store.milestones(rig.pid)),
        "items": sorted((i["id"], i["status"], i["attempts"], i["milestone_id"])
                        for i in store.items(rig.pid)),
    }


def test_rebuild_reproduces_the_projections_exactly():
    """DM-16 — the test that proves the log is sufficient."""
    with Rig() as rig:
        rig.create()
        for _ in range(3):
            rig.sup.tick(rig.pid)
            for item in rig.store.items(rig.pid, status="ready"):
                rig.store.append(rig.pid, rig.ev.ITEM_DONE, "test", item_id=item["id"])
        rig.sup.tick(rig.pid)
        rig.store.append(rig.pid, rig.ev.ITEM_INVALIDATED, "member:x",
                         item_id="i1", data={"reason": "test"})

        before = _snapshot(rig)
        rig.store.rebuild(rig.pid)
        after = _snapshot(rig)
        assert before == after, f"rebuild diverged:\n before={before}\n after={after}"

        # And it is idempotent.
        rig.store.rebuild(rig.pid)
        assert _snapshot(rig) == before


def test_phase0_replays_an_event_whose_projection_write_did_not_land():
    """RT-12: a projection that LAGS the log self-heals within one tick.

    This is the divergence a crash mid-transaction, or a code path that wrote
    an event without persisting it, leaves behind — and the only divergence a
    tail replay can repair.

    It deliberately does NOT claim to repair an arbitrary corrupt row that
    leaves the checkpoint current: replaying only the tail cannot recreate a
    row an earlier event created. That is what ``rebuild()`` is for, and
    ``docs/group-projects/02-runtime.md`` section 3.0 says so.
    """
    with Rig() as rig:
        rig.create()
        rig.sup.tick(rig.pid)
        # Simulate the event landing but its projection update never committing.
        head = rig.store.max_seq(rig.pid)
        rig.store._db.execute(
            "INSERT INTO events (project_id, ts, kind, actor, milestone_id,"
            " item_id, data) VALUES (?,?,?,?,?,?,?)",
            (rig.pid, rig.sup.clock(), rig.ev.ITEM_DONE, "test", None, "i1", "{}"),
        )
        rig.store._db.commit()
        assert rig.store.item("i1")["status"] == "ready", "precondition"
        assert rig.store.max_seq(rig.pid) == head + 1
        assert rig.store.last_seq(rig.pid) == head, "checkpoint should lag"

        rig.sup.tick(rig.pid)
        assert rig.store.item("i1")["status"] == "done", (
            "the lagging projection was not replayed"
        )


def test_rebuild_repairs_arbitrary_corruption():
    """The stronger guarantee: rebuild() fixes a row no tail replay could."""
    with Rig() as rig:
        rig.create()
        rig.sup.tick(rig.pid)
        before = _snapshot(rig)

        # Corrupt a row the way a hand-edit would, leaving the checkpoint current.
        rig.store._db.execute("UPDATE items SET status = 'nonsense' WHERE id = 'i2'")
        rig.store._db.execute("DELETE FROM items WHERE id = 'i1'")
        rig.store._db.commit()
        assert _snapshot(rig) != before, "precondition: corruption applied"

        rig.store.rebuild(rig.pid)
        assert _snapshot(rig) == before, "rebuild did not repair the corruption"


def test_reopening_the_store_keeps_everything():
    """The folder owns its state: reopening the same file loses nothing."""
    with Rig() as rig:
        rig.create()
        rig.sup.tick(rig.pid)
        rig.store.append(rig.pid, rig.ev.ITEM_DONE, "test", item_id="i1")
        before = _snapshot(rig)
        events_before = rig.store.max_seq(rig.pid)
        rig.store.close()

        reopened = rig.store_mod.ProjectStore(rig.path)
        rig.store = reopened  # let Rig.close() own the reopened handle
        try:
            assert reopened.max_seq(rig.pid) == events_before
            assert _snapshot(rig, reopened) == before
        finally:
            reopened.close()


def test_unknown_event_kind_is_loud():
    """DM-19 by construction: a kind with no reducer must never be silent."""
    with Rig() as rig:
        rig.create()
        try:
            rig.store.append(rig.pid, "nonsense.happened", "test")
        except rig.ev.UnknownEventKind:
            pass
        else:
            raise AssertionError("an unknown event kind was accepted")
