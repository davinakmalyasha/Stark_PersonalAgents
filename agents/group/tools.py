"""agents.group.tools — the orchestrator and member tool surface (Phase 3).

``docs/group-projects/03-orchestrator-tools.md``.

Two audiences, deliberately separated:

- **Orchestrator tools** decide what work exists. Every mutation goes through
  the op queue, so there is still exactly one writer (``02`` section 4). Reads
  bypass the queue — queueing a read for 15 seconds is absurd (``03`` §3.2).
- **Member tools** are what a member may do to the board. Narrow on purpose:
  a member cannot add items, add members, or change its own capability set.

The descriptions below are **routing policy, not documentation** (``03``
§2.1). They state when to reach for a tool *and* when not to, because the
expensive failure is wrapping a trivial job in a three-day project.
"""
from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass
from typing import Any

from . import events as ev
from . import messages as group_messages
from . import planner as planner_mod

#: Injected so this module never imports a store implementation.
Resolver = Callable[[str], Any]


# ── routing policy ──────────────────────────────────────────────────────────

TOOL_DOCS = {
    "plan_execution": (
        "Decide the SHAPE of a request before starting anything: solo, group, or "
        "scheduled. Use when a request could be more than a single self-contained "
        "action. Returns a rationale, a proposed environment (which members, "
        "skills, MCP servers), and a plan preview. Always call this BEFORE "
        "create_group_project. Do NOT use for a single self-contained request "
        "('rename these files') or for recurring work ('every morning check X') "
        "-- the first is solo, the second is scheduled and belongs to the task "
        "scheduler."
    ),
    "create_group_project": (
        "Start a durable project that keeps working until it is done, across "
        "sessions and restarts. Use only after plan_execution returned "
        "shape='group' and the human confirmed the preview. Do NOT use for a "
        "single self-contained request or for recurring work; those are `solo` "
        "and `scheduled`. Returns the project id; effects are async, so read the "
        "board to follow progress."
    ),
    "board": (
        "Read the current state of a project: milestones, items, statuses, spend, "
        "and anything awaiting a human. Read-only and immediate. Use this to "
        "answer 'what is left?' or 'why is this stuck?'."
    ),
    "job_control": (
        "Pause, resume, close or cancel work on a project. Pausing keeps all "
        "state and halts dispatch; closing marks the project finished."
    ),
}


@dataclass
class ToolResult:
    ok: bool
    message: str
    data: dict | None = None

    def __str__(self) -> str:
        return f"[{'OK' if self.ok else 'ERR'}] {self.message}"


class OffBoard(Exception):
    """A tool was used on a project the resolver does not know."""


class GroupToolkit:
    """The whole surface, bound to a resolver.

    ``resolve(project_id) -> (store, context) | None`` is supplied by the app
    (reading the workspaces registry) or by a test (a lambda). Nothing here
    knows how a project is found on disk.
    """

    def __init__(self, resolve: Resolver,
                 *, create: Callable[..., Any] | None = None) -> None:
        self._resolve = resolve
        self._create = create

    # ── internals ───────────────────────────────────────────────────────

    def _handle(self, project_id: str) -> tuple[Any, Any]:
        found = self._resolve(project_id)
        if not found:
            raise OffBoard(f"unknown project {project_id!r}")
        store, context = found
        return store, context

    def _submit(self, project_id: str, kind: str, payload: dict,
                actor: str = "orchestrator") -> int:
        store, _context = self._handle(project_id)
        return store.submit_op(project_id, kind, payload, actor=actor)

    # ════════════════════════════════════════════════════════════════════
    #  Orchestrator — creation
    # ════════════════════════════════════════════════════════════════════

    def plan_execution(self, goal: str, *, horizon: str = "hours",
                       reversible: bool = True, needs_human: bool = False,
                       constraints: str = "",
                       agents: list[dict] | None = None,
                       skills: list[dict] | None = None,
                       mcp: list[dict] | None = None,
                       planner: Callable[..., dict] | None = None) -> dict:
        """The routing decision. See :func:`agents.group.planner.plan_execution`."""
        preview = planner_mod.plan_execution(
            goal, horizon=horizon, reversible=reversible, needs_human=needs_human,
            constraints=constraints, agents=agents, skills=skills, mcp=mcp,
            planner=planner,
        )
        return preview.to_dict()

    def create_group_project(self, root: str, manifest_text: str, **options: Any) -> ToolResult:
        """Materialise the folder and seed the event log.

        ``create`` is injected (defaults to
        :func:`agents.group.project.create_group_project`) so this module never
        imports the folder machinery at import time.
        """
        if self._create is None:
            from .project import create_group_project as _create
            self._create = _create
        from .manifest import parse
        try:
            manifest = parse(manifest_text)
            paths, _store = self._create(root, manifest, **options)
        except Exception as exc:  # noqa: BLE001 - surfaced to the caller as a result
            return ToolResult(False, f"{type(exc).__name__}: {exc}")
        return ToolResult(True, f"created {manifest.id}", {"project_id": manifest.id,
                                                           "path": paths.project_dir})

    # ════════════════════════════════════════════════════════════════════
    #  Orchestrator — mutation (all queued)
    # ════════════════════════════════════════════════════════════════════

    def add_item(self, project_id: str, *, item_id: str, milestone_id: str,
                 title: str, detail: str, acceptance: list[dict],
                 depends_on: list[str] | None = None,
                 owner_member: str = "", workdir: str = "") -> ToolResult:
        """Add one item. ``item_id`` is required — without it the item cannot
        be addressed by later ops, so the supervisor refuses it."""
        if not str(item_id or "").strip():
            return ToolResult(False, "add_item needs an item_id (e.g. 'm2.i1')")
        if not acceptance:
            return ToolResult(False, "an item needs acceptance criteria")
        op = self._submit(project_id, "add_item", {
            "id": item_id, "milestone_id": milestone_id, "title": title,
            "detail": detail, "acceptance": acceptance,
            "depends_on": list(depends_on or []),
            "owner_member": owner_member, "workdir": workdir,
        })
        return ToolResult(True, f"queued add_item (#{op}); read the board to confirm")

    def add_milestone(self, project_id: str, *, milestone_id: str, title: str,
                      depends_on: list[str] | None = None) -> ToolResult:
        op = self._submit(project_id, "add_milestone", {
            "id": milestone_id, "title": title, "depends_on": list(depends_on or []),
        })
        return ToolResult(True, f"queued add_milestone (#{op})")

    def invalidate_item(self, project_id: str, *, item_id: str,
                        reason: str = "") -> ToolResult:
        op = self._submit(project_id, "invalidate_item",
                          {"item_id": item_id, "reason": reason})
        return ToolResult(True, f"queued invalidate_item (#{op})")

    def cancel_item(self, project_id: str, *, item_id: str) -> ToolResult:
        op = self._submit(project_id, "cancel_item", {"item_id": item_id})
        return ToolResult(True, f"queued cancel_item (#{op})")

    def add_member(self, project_id: str, *, name: str, derived_from: str,
                   model: str = "", thinking: str = "",
                   tools: list[str] | None = None,
                   skills: list[str] | None = None,
                   mcp: list[str] | None = None) -> ToolResult:
        op = self._submit(project_id, "add_member", {
            "name": name, "derived_from": derived_from, "model": model,
            "thinking": thinking, "tools": tools, "skills": skills, "mcp": mcp,
        })
        return ToolResult(True, f"queued add_member (#{op})")

    def remove_member(self, project_id: str, *, name: str) -> ToolResult:
        op = self._submit(project_id, "remove_member", {"name": name})
        return ToolResult(True, f"queued remove_member (#{op})")

    def attach_mcp(self, project_id: str, *, server: str,
                   overrides: dict | None = None) -> ToolResult:
        op = self._submit(project_id, "attach_mcp",
                          {"server": server, "overrides": overrides or {}})
        return ToolResult(True, f"queued attach_mcp (#{op})")

    def detach_mcp(self, project_id: str, *, server: str) -> ToolResult:
        op = self._submit(project_id, "detach_mcp", {"server": server})
        return ToolResult(True, f"queued detach_mcp (#{op})")

    def attach_skill(self, project_id: str, *, name: str) -> ToolResult:
        op = self._submit(project_id, "attach_skill", {"name": name})
        return ToolResult(True, f"queued attach_skill (#{op})")

    def detach_skill(self, project_id: str, *, name: str) -> ToolResult:
        op = self._submit(project_id, "detach_skill", {"name": name})
        return ToolResult(True, f"queued detach_skill (#{op})")

    def attach_tool(self, project_id: str, *, member: str,
                    tools: list[str]) -> ToolResult:
        op = self._submit(project_id, "attach_tool",
                          {"member": member, "tools": list(tools)})
        return ToolResult(True, f"queued attach_tool (#{op})")

    def pause(self, project_id: str) -> ToolResult:
        return ToolResult(True, f"queued pause (#{self._submit(project_id, 'pause', {})})")

    def resume(self, project_id: str) -> ToolResult:
        return ToolResult(True, f"queued resume (#{self._submit(project_id, 'resume', {})})")

    def close(self, project_id: str, *, status: str = "done") -> ToolResult:
        op = self._submit(project_id, "close", {"status": status})
        return ToolResult(True, f"queued close (#{op})")

    # ════════════════════════════════════════════════════════════════════
    #  Orchestrator — reads (these do NOT queue)
    # ────────────────────────────────────────────────────────────────════

    def board(self, project_id: str) -> str:
        store, _context = self._handle(project_id)
        project = store.project(project_id) or {}
        milestones = store.milestones(project_id)
        items = store.items(project_id)
        lines = [f"[BOARD] {project_id}  status={project.get('status')}"]
        if project.get("halt_reason"):
            lines.append(f"halted: {project['halt_reason']}")
        cap = project.get("budget_usd")
        spent = float(project.get("spent_usd") or 0)
        lines.append(
            f"milestones {sum(1 for m in milestones if m['status'] == 'done')}"
            f"/{len(milestones)} · "
            f"items {sum(1 for i in items if i['status'] == 'done')}/{len(items)} · "
            + (f"spend ${spent:.2f}/${float(cap):.2f}" if cap else f"spend ${spent:.2f}")
        )
        for milestone in milestones:
            lines.append(f"  {milestone['id']} [{milestone['status']}] {milestone['title']}")
            for item in items:
                if item["milestone_id"] != milestone["id"]:
                    continue
                extra = ""
                if item.get("blocked_reason"):
                    extra = f" ({item['blocked_reason']})"
                elif item["status"] == "doing" and item.get("lease_owner"):
                    extra = f" ({item['lease_owner']})"
                lines.append(f"    {item['id']} [{item['status']}] "
                             f"{item['title']}{extra}")
        waiting = [i["id"] for i in items if i.get("blocked_reason") == ev.AWAITING_APPROVAL]
        if waiting:
            lines.append(f"awaiting approval: {', '.join(waiting)}")
        return "\n".join(lines)

    def item_detail(self, project_id: str, *, item_id: str) -> str:
        store, _context = self._handle(project_id)
        item = store.item(item_id)
        if item is None:
            return f"[ERR] unknown item {item_id!r}"
        lines = [f"[ITEM {item_id}] {item['title']}  status={item['status']}",
                 f"  milestone: {item['milestone_id']}",
                 f"  attempts: {item['attempts']}/{item['max_attempts']}"]
        if item.get("result"):
            lines.append(f"  result: {str(item['result'])[:400]}")
        if item.get("error"):
            lines.append(f"  error: {str(item['error'])[:400]}")
        if item.get("invalidated_reason"):
            lines.append(f"  invalidated: {item['invalidated_reason']}")
        for criterion in item.get("acceptance") or []:
            from .criteria import describe
            lines.append(f"  - {describe(criterion)}")
        return "\n".join(lines)

    def events(self, project_id: str, *, since: int = 0, limit: int = 50) -> str:
        store, _context = self._handle(project_id)
        records = store.events(project_id, since=since, limit=limit)
        lines = [f"[EVENTS] {project_id}  showing {len(records)}"]
        for record in records:
            target = record.get("item_id") or record.get("milestone_id") or ""
            lines.append(f"  {record['seq']:>5} {record['kind']:<22} "
                         f"{record['actor']:<16} {target}")
        return "\n".join(lines)

    # ════════════════════════════════════════════════════════════════════
    #  Member tools — narrow on purpose
    # ════════════════════════════════════════════════════════════════════

    def set_item_status(self, project_id: str, *, item_id: str, status: str,
                        note: str = "", member: str = "") -> ToolResult:
        """A member's only way to change the board (``03`` §3.3)."""
        allowed = {"done": ev.ITEM_DONE, "failed": ev.ITEM_FAILED,
                   "blocked": ev.ITEM_BLOCKED}
        kind = allowed.get(status)
        if kind is None:
            return ToolResult(False, f"a member may set {sorted(allowed)}, not {status!r}")
        store, _context = self._handle(project_id)
        store.append(project_id, kind, f"member:{member or '?'}", item_id=item_id,
                     data={"reason": note} if note else {})
        return ToolResult(True, f"{item_id} -> {status}")

    def request_approval(self, project_id: str, *, item_id: str,
                         detail: str = "") -> ToolResult:
        """Ask for a human decision. The item parks; it is not a stall."""
        store, _context = self._handle(project_id)
        store.append(project_id, ev.APPROVAL_REQUESTED, "member", item_id=item_id,
                     data={"detail": detail})
        return ToolResult(True, f"{item_id} parked awaiting approval")

    def resolve_approval(self, project_id: str, *, item_id: str, grant: bool,
                         note: str = "") -> ToolResult:
        """The human's answer to :meth:`request_approval`.

        The other half of a pair that shipped half-built: ``request_approval``
        parked an item and every reducer for the answer already existed
        (``APPROVAL_GRANTED`` clears the block, ``APPROVAL_REJECTED`` blocks
        the item and releases the lease) — but nothing ever emitted either
        one, and the stall detector exempts ``AWAITING_APPROVAL``, so a
        parked item was silent **forever**.

        Deliberately NOT an orchestrator op and deliberately absent from
        ``_OPS``: an approval the model can grant itself is not an approval.
        The only caller is a person, through the board.
        """
        store, _context = self._handle(project_id)
        if store.item(item_id) is None:
            return ToolResult(False, f"unknown item {item_id!r}")
        kind = ev.APPROVAL_GRANTED if grant else ev.APPROVAL_REJECTED
        payload: dict = {"note": note} if note else {}
        if grant:
            # Carry the SHAPE onto the grant (AU-08). This is what lets the
            # re-dispatched member be told "yes" instead of asking again: the
            # gate recognises a shape a human has already approved.
            from .runner import requested_shape, requested_write_key
            shape = requested_shape(store, project_id, item_id)
            if shape:
                payload["shape"] = shape
            # AU-05: an ambiguous write (reserved, never completed) parks with
            # a write_key. Carrying it onto the grant is how the ledger knows a
            # human explicitly authorized one retry, rather than the original
            # approval still being in force.
            write_key = requested_write_key(store, project_id, item_id)
            if write_key:
                payload["write_key"] = write_key
        store.append(project_id, kind, "human", item_id=item_id, data=payload)
        try:
            item = store.item(item_id) or {}
            group_messages.send(
                store, project_id,
                sender="owner",
                recipient=str(item.get("lease_owner") or item.get("owner_member") or "member"),
                body=(f"Approved {item_id}." if grant else f"Rejected {item_id}.")
                     + (f" Note: {note}" if note else ""),
                kind="owner", item_id=item_id,
            )
        except Exception:  # noqa: BLE001 - approval already applied
            pass
        if grant:
            return ToolResult(True, f"{item_id} approved")
        return ToolResult(True, f"{item_id} rejected — blocked awaiting a re-plan")

    def append_finding(self, project_id: str, *, text: str, source: str = "") -> ToolResult:
        """Append to ``context/findings.md``. Write down what you LEARNED."""
        _store, context = self._handle(project_id)
        return self._append_context(context, "findings.md", text, source)

    def record_decision(self, project_id: str, *, text: str,
                        rejected: str = "", source: str = "") -> ToolResult:
        """Append to ``context/decisions.md`` — chosen, rejected, and why."""
        _store, context = self._handle(project_id)
        suffix = f"\n  rejected: {rejected}" if rejected else ""
        return self._append_context(context, "decisions.md", text, source + suffix)

    def request_capability(self, project_id: str, *, member: str,
                           need: str) -> ToolResult:
        """Ask for a tool, skill or MCP server the member does not have.

        A member may never widen its own set (``01`` §5.4): it asks, and the
        orchestrator decides. This exists so that constraint is workable rather
        than obstructive.
        """
        store, _context = self._handle(project_id)
        store.append(project_id, ev.IDEA_PROPOSED, f"member:{member}",
                     data={"kind": "capability_request", "need": need})
        return ToolResult(True, f"recorded capability request from {member}")

    def _append_context(self, context: Any, filename: str, text: str,
                        source: str = "") -> ToolResult:
        import os


        paths = getattr(context, "paths", None)
        if paths is None:
            return ToolResult(False, "no project folder configured")
        target = os.path.join(paths.context_dir, filename)
        os.makedirs(os.path.dirname(target), exist_ok=True)
        import datetime
        stamp = datetime.datetime.now(datetime.UTC).isoformat(timespec="seconds")
        line = f"\n## {stamp}\n{text.strip()}\n"
        if source:
            line += f"\nsource: {source.strip()}\n"
        with open(target, "a", encoding="utf-8", newline="\n") as handle:
            handle.write(line)
        return ToolResult(True, f"appended to {filename}")

    # ── the declared member surface, for the audit test ────────────────

    @staticmethod
    def member_tools() -> tuple[str, ...]:
        """What a member may call. Asserted narrow by the audit test.

        Deliberately excludes every membership, policy and environment op: a
        member cannot add items, add members, or grant itself a tool.
        """
        return (
            "claim_item", "set_item_status", "append_finding",
            "record_decision", "post_message", "request_approval",
            "request_capability",
        )

    @staticmethod
    def orchestrator_tools() -> tuple[str, ...]:
        return (
            "plan_execution", "create_group_project",
            "add_item", "add_milestone", "invalidate_item", "cancel_item",
            "add_member", "remove_member", "attach_tool",
            "attach_mcp", "detach_mcp", "attach_skill", "detach_skill",
            "pause", "resume", "close",
            "board", "item_detail", "events",
        )
