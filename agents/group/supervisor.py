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
from dataclasses import dataclass, field
from typing import Any

from . import criteria as criteria_mod
from . import events as ev
from .runner import ItemRunner, RunContext
from .store import ProjectStore, evaluate_completion

#: Default lease for a claimed item. Phase 2 renews while there is progress.
LEASE_SECONDS = 30 * 60.0


@dataclass
class ProjectContext:
    """What the supervisor needs to run items of one project.

    Built by the caller (a registry in the app, a lambda in tests) so the
    supervisor never needs to know how projects are stored on disk. That is
    the same seam as :class:`ItemRunner`, and for the same reason: the tick
    stays testable and free of I/O assumptions.
    """

    project_id: str
    root: str
    members: dict[str, dict] = field(default_factory=dict)
    autonomy: dict = field(default_factory=dict)
    manifest_members: list[dict] = field(default_factory=list)

    #: The folder, for capability ops that edit files rather than rows.
    paths: Any = None
    #: Injection points for materialising NEW capabilities (Phase 3). Kept
    #: here rather than imported so `agents/group/` stays importable without
    #: the whole agent stack.
    spec_lookup: Callable[[str], Any] | None = None
    profile_lookup: Callable[[str], Any] | None = None
    skill_lookup: Callable[[str], Any] | None = None
    mcp_catalogue: dict = field(default_factory=dict)
    #: Optional `(criterion, root) -> (bool, str)` for `llm` criteria. With no
    #: judge an `llm` criterion is `unsupported`, never guessed.
    judge: Callable[..., Any] | None = None

    def member_chat_id(self, member_name: str) -> str:
        """One durable chat per member (``02`` section 8).

        Deterministic, so a restart resumes the same conversation rather than
        starting a fresh one.
        """
        return f"gp-{self.project_id}-{member_name}"

    def effective_members(self) -> dict[str, dict]:
        """The members, or the sole-member default (``01`` §5.3).

        A project with no members is a project of one. The default is the
        ``general`` workspace agent **not** the orchestrator node: the
        orchestrator is a *coordinator* whose node delegates, so using it as a
        runner target would be circular. "The orchestrator does the work"
        means it stops delegating and runs.
        """
        if self.members:
            return self.members
        spec = None
        if self.spec_lookup is not None:
            spec = self.spec_lookup("general")
        return {SOLE_MEMBER: {
            "name": SOLE_MEMBER,
            "derived_from": "general",
            "runtime": (getattr(spec, "runtime", "") if spec
                        else "") or "agents.general_agent:general_node",
            "state_builder": (getattr(spec, "state_builder", "plain") if spec
                              else "plain") or "plain",
            "tools": list(getattr(spec, "tools", ()) or ()) if spec else [],
            "model": "",
            "thinking": "inherit",
        }}

    def default_member(self) -> str | None:
        names = list(self.effective_members())
        return names[0] if names else None

    def member_ceiling(self, member_name: str) -> list[str] | None:
        """The scope map's allowlist for a member, if a lookup is available."""
        if self.spec_lookup is None:
            return None
        agent = self.effective_members().get(member_name) or {}
        key = str(agent.get("derived_from") or member_name)
        spec = self.spec_lookup(key)
        if spec is None:
            return None
        return list(getattr(spec, "tools", ()) or ())

#: Consecutive no-event ticks before a project is called stalled (``02`` 6.2).
STALL_TICKS = 50

#: Fraction of budget at which a one-off warning is emitted.
BUDGET_WARN_AT = 0.8

#: Op kinds the supervisor can apply. Anything else is rejected, loudly, with
#: the op left visible on the board rather than silently dropped.
_SUPPORTED_OPS = frozenset({
    # plan
    "add_milestone", "add_item", "update_item", "cancel_item",
    "invalidate_item", "pause", "resume", "close", "set_completion",
    # members
    "add_member", "remove_member", "attach_tool",
    # environment
    "attach_mcp", "detach_mcp", "attach_skill", "detach_skill",
})

#: Ops that edit the project FOLDER rather than the database. They still go
#: through the queue, so there is one writer and one event per change.
_FOLDER_OPS = frozenset({
    "add_member", "remove_member", "attach_tool",
    "attach_mcp", "detach_mcp", "attach_skill", "detach_skill",
})


#: Blocked reasons that are worth another attempt. A lease expiry means the
#: member died, not that the work is impossible, so the item goes back to the
#: queue — and `attempts` (already incremented by the reaper) eventually
#: converts it to a failure rather than looping forever.
RETRYABLE_BLOCK_REASONS = frozenset({"lease_expired"})

#: Blocked reasons that need the orchestrator or a human to act, and must
#: therefore not be mistaken for a deadlock.
WAITING_BLOCK_REASONS = frozenset({ev.AWAITING_APPROVAL, "approval_rejected"})


#: The member name used when a project declares none: a project of one
#: (``01`` §5.3). It maps to the ``general`` workspace agent, not to the
#: orchestrator node — see :meth:`ProjectContext.effective_members`.
SOLE_MEMBER = "orchestrator"


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

    def __init__(self, store: ProjectStore | None = None, *,
                 store_for: Callable[[str], ProjectStore] | None = None,
                 runner: ItemRunner | None = None,
                 context_for: Callable[[str], ProjectContext | None] | None = None,
                 lease_seconds: float = LEASE_SECONDS,
                 stall_ticks: int = STALL_TICKS,
                 clock: Callable[[], float] = time.time,
                 log: Callable[[str], None] | None = None) -> None:
        self.store = store
        #: Each project owns its own `state.db`, so one Supervisor instance can
        #: serve many projects by swapping this at the top of each tick. That
        #: is safe because there is exactly ONE supervisor thread (``02``
        #: §2.1) — the single-writer guarantee is structural, not a lock.
        self.store_for = store_for
        self.runner = runner          # None keeps dispatch inert
        self.context_for = context_for
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
        if self.store_for is not None:
            resolved = self.store_for(project_id)
            if resolved is not None:
                self.store = resolved
        if self.store is None:
            return {"project": project_id, "phases": {}, "halted": "no store"}
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
        if kind not in _SUPPORTED_OPS:
            raise OpRejected(f"{kind!r} is not implemented")

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
            raise OpRejected("set_completion needs a field-level event; deferred")
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
        if kind in _FOLDER_OPS:
            self._op_capability(project_id, kind, payload)
            return

    # ── capability ops: these edit the folder, not the database ─────────

    def _op_capability(self, project_id: str, kind: str, payload: dict) -> None:
        """Apply an environment change to the project folder.

        Still an op: queued, applied by the single writer, and followed by
        exactly one event. The *file* is the project's own copy, so a change
        here never touches the catalogue or any other project
        (``03`` section 3.2).
        """
        from . import materialize as mat

        context = self.context_for(project_id) if self.context_for else None
        paths = getattr(context, "paths", None) if context else None
        if paths is None:
            raise OpRejected(f"{kind!r} needs a project folder, which is not configured")

        if kind == "attach_mcp":
            server = str(payload.get("server") or "")
            if not server:
                raise OpRejected("attach_mcp requires a server name")
            entry = mat.add_mcp_server(
                paths, context.mcp_catalogue or {}, server,
                payload.get("overrides"),
            )
            self._capability_event(project_id, "mcp", server,
                                   {"server": server, "tools": entry.get("enabled_tools") or []})
            return

        if kind == "detach_mcp":
            server = str(payload.get("server") or "")
            if not mat.remove_mcp_server(paths, server):
                raise OpRejected(f"MCP server {server!r} is not attached")
            self._capability_event(project_id, "mcp", server, {"server": server})
            return

        if kind == "attach_skill":
            name = str(payload.get("name") or "")
            found = (context.skill_lookup or (lambda _n: None))(name)
            if found is None:
                raise OpRejected(f"skill {name!r} was not found")
            mat.add_skill(paths, found)
            self._capability_event(project_id, "skill", name, {"skill": name})
            return

        if kind == "detach_skill":
            name = str(payload.get("name") or "")
            if not mat.remove_skill(paths, name):
                raise OpRejected(f"skill {name!r} is not attached")
            self._capability_event(project_id, "skill", name, {"skill": name})
            return

        if kind == "attach_tool":
            member = str(payload.get("member") or "")
            if not member:
                raise OpRejected("attach_tool requires a member")
            tools = [str(t) for t in (payload.get("tools") or [])]
            if not tools:
                raise OpRejected("attach_tool requires at least one tool")
            ceiling = context.member_ceiling(member) if context else None
            try:
                agent = mat.set_member_tools(paths, member, tools, ceiling=ceiling)
            except mat.MaterializationError as exc:
                raise OpRejected(str(exc)) from exc
            self._capability_event(project_id, "tools", member,
                                   {"member": member, "tools": agent.get("tools") or []})
            return

        if kind == "add_member":
            name = str(payload.get("name") or "")
            derived = str(payload.get("derived_from") or "")
            if not name or not derived:
                raise OpRejected("add_member requires name and derived_from")
            spec = (context.spec_lookup or (lambda _k: None))(derived)
            profile = None if spec is not None else (
                (context.profile_lookup or (lambda _k: None))(derived))
            if spec is None and profile is None:
                raise OpRejected(f"unknown agent {derived!r}")
            try:
                agent = (mat.agent_from_spec(
                            spec, name, model=str(payload.get("model") or ""),
                            thinking=str(payload.get("thinking") or ""),
                            tools=payload.get("tools"),
                            skills=list(payload.get("skills") or []),
                            mcp=list(payload.get("mcp") or []))
                         if spec is not None
                         else mat.agent_from_profile(profile, name))
                mat.write_agent(agent, paths.agents_dir)
            except mat.MaterializationError as exc:
                raise OpRejected(str(exc)) from exc
            self._capability_event(project_id, "member", name,
                                   {"member": name, "derived_from": derived})
            return

        if kind == "remove_member":
            name = str(payload.get("name") or "")
            if not mat.remove_agent(paths, name):
                raise OpRejected(f"member {name!r} does not exist")
            for item in self.store.items(project_id, status="doing"):
                if item.get("owner_member") == name:
                    # Never silently reassign work: release it and let the
                    # board show that it needs an owner (D-12).
                    self.store.append(project_id, ev.ITEM_BLOCKED, "supervisor",
                                      item_id=item["id"],
                                      data={"reason": "reassignment"})
            self._capability_event(project_id, "member", name, {"member": name})
            return

        raise OpRejected(f"{kind!r} is not implemented")  # pragma: no cover

    def _capability_event(self, project_id: str, subject: str, name: str,
                          data: dict) -> None:
        self.store.append(project_id, ev.CAPABILITY_CHANGED, "orchestrator",
                          data={"subject": subject, "name": name, **data})

    def _op_add_item(self, project_id: str, payload: dict) -> None:
        milestone_id = str(payload.get("milestone_id") or "")
        if milestone_id not in {m["id"] for m in self.store.milestones(project_id)}:
            raise OpRejected(f"unknown milestone {milestone_id!r}")
        # An item with no id is not a naming preference — without it the
        # reducer cannot address the row at all, and the item would be dropped
        # silently. Refuse instead.
        item_id = str(payload.get("id") or "").strip()
        if not item_id:
            raise OpRejected("add_item requires an item `id` (e.g. 'm2.i1')")
        if self.store.item(item_id) is not None:
            raise OpRejected(f"item {item_id!r} already exists")
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
        self._check_cycle(project_id, item_id, depends, milestone_id)
        self.store.append(project_id, ev.ITEM_CREATED, "orchestrator",
                          milestone_id=milestone_id, item_id=item_id,
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
            status = item["status"]
            if status == "todo":
                pass
            elif status == "blocked":
                # A transient block (the member died) goes back in the queue.
                # A block that needs a human (`approval_rejected`) does not —
                # requeueing it would just re-run work nobody approved.
                if item.get("blocked_reason") not in RETRYABLE_BLOCK_REASONS:
                    continue
            else:
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
        """Claim ready items and hand them to the runner.

        Inert without a runner: claiming an item we cannot run would leave it
        ``doing`` until its lease expired, which is worse than leaving it
        ``ready``. That is why Phase 1 could tick with no members at all.
        """
        if self.runner is None or self.context_for is None:
            return 0
        context = self.context_for(project_id)
        if context is None:
            return 0

        started = 0
        members = context.effective_members()
        for item in self.store.items(project_id, status="ready"):
            member_name = self.runner.member_for(item) or context.default_member()
            member = members.get(member_name or "")
            if member is None:
                # An item with nobody able to take it. Leave it ready and
                # visible rather than claiming it into a dead end.
                continue
            self.store.append(
                project_id, ev.ITEM_CLAIMED, "supervisor", item_id=item["id"],
                data={"lease_owner": member_name,
                      "lease_expires": self.clock() + self.lease_seconds},
            )
            claimed = self.store.item(item["id"]) or item
            self.runner.start(RunContext(
                project_id=project_id,
                project_root=context.root,
                item=claimed,
                member=member,
                member_name=member_name or "",
                chat_id=context.member_chat_id(member_name or ""),
                autonomy=context.autonomy,
            ))
            started += 1
        return started

    # ── phase 6: collect what finished ──────────────────────────────────

    def _collect(self, project_id: str) -> int:
        """Harvest finished runs into the item — WITHOUT changing its status.

        The run result and the verdict are deliberately separate events: a
        result is a fact about an attempt, a verdict is a judgement about the
        work. Keeping them apart is what lets phase 7 be re-run, makes a retry
        legible on the board, and keeps ``item.progress`` honest.
        """
        if self.runner is None:
            return 0
        collected = 0
        for result in self.runner.collect(project_id):
            item = self.store.item(result.item_id)
            if item is None or item["status"] != "doing":
                # Cancelled, invalidated or reaped while it ran. The lease
                # already decided its fate; a late result must not resurrect it.
                continue
            actor = f"member:{item.get('lease_owner') or '?'}"
            self.store.append(
                project_id, ev.ITEM_PROGRESS, actor, item_id=result.item_id,
                data={"result": result.report, "error": result.error,
                      "cost_usd": result.cost_usd, "cost_tokens": result.cost_tokens},
            )
            collected += 1
        return collected

    # ── phase 7: verify against the acceptance criteria ─────────────────

    def _verify(self, project_id: str) -> int:
        """Judge harvested items against their own acceptance criteria.

        Deterministic wherever it can be (``01`` section 4.6). A ``manual``
        criterion parks the item — it stays ``doing`` and counts as a
        legitimate wait, so it never looks like a stall.

        Note this does NOT require a runner. Verification is about criteria,
        not about executing anything, and a project whose items are driven by
        hand still needs its results judged.
        """
        if self.context_for is None:
            return 0
        context = self.context_for(project_id)
        if context is None:
            return 0

        changed = 0
        for item in self.store.items(project_id, status="doing"):
            if item.get("blocked_reason") == ev.AWAITING_APPROVAL:
                # The member asked permission and returned anyway. Settling it
                # here would mark the work done *behind the approval it just
                # requested* — which is the exact thing the park exists to
                # prevent. Only a human clears this (``resolve_approval``).
                continue
            if not item.get("result") and not item.get("error"):
                continue  # still running
            if item.get("error"):
                changed += self._settle_failure(
                    project_id, item, f"run failed: {item['error']}")
                continue

            evaluation = criteria_mod.evaluate_item(item, context.root,
                                                    getattr(context, "judge", None))
            if evaluation.passed:
                self.store.append(project_id, ev.ITEM_DONE, "supervisor",
                                  item_id=item["id"])
                changed += 1
            elif evaluation.pending:
                # Park it — but only announce it once, or the inbox fills with
                # the same question every 15 seconds.
                if item.get("blocked_reason") != ev.AWAITING_APPROVAL:
                    self.store.append(
                        project_id, ev.APPROVAL_REQUESTED, "supervisor",
                        item_id=item["id"],
                        data={"pending": [r.detail for r in evaluation.pending]},
                    )
                    changed += 1
            elif evaluation.failed:
                changed += self._settle_failure(project_id, item,
                                                evaluation.summary())
            # `unsupported` criteria (llm, Phase 3) leave the item `doing`
            # rather than guessing a verdict — an unverifiable item must not
            # silently become done or silently become failed.
        return changed

    def _settle_failure(self, project_id: str, item: dict, reason: str) -> int:
        """Retry while attempts remain; otherwise fail for good (BR-02)."""
        attempts = int(item.get("attempts") or 0)
        max_attempts = int(item.get("max_attempts") or 3)
        if attempts < max_attempts:
            self.store.append(project_id, ev.ITEM_READY, "supervisor",
                              item_id=item["id"],
                              data={"retry": attempts, "reason": reason[:400]})
        else:
            self.store.append(project_id, ev.ITEM_FAILED, "supervisor",
                              item_id=item["id"], data={"reason": reason[:400]})
        return 1

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

        Three cases, and the third is the one a real run taught us:

        - an **invalidated** item: a re-plan is already open;
        - a parked item (reason set): waiting on you, status unchanged;
        - an item **actively running with a live lease**: a member emitting no
          events is not idle. A real LLM item took 22 ticks of silence to
          finish; without this, anything slower than the stall threshold
          (50 ticks ≈ 12 min) would be declared deadlocked mid-work. The
          exemption is self-limiting — when the lease expires the reaper
          blocks the item, and a blocked item is no longer exempt.
        """
        return any(
            i["status"] == "invalidated"                              # re-plan open
            or i.get("blocked_reason") in WAITING_BLOCK_REASONS        # waiting on you
            or (i["status"] == "doing" and i.get("lease_owner"))       # working
            for i in self.store.items(project_id)
        )
