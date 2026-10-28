"""Group projects router — the board the human sees (Phase 4 surface).

``docs/group-projects/05-integration-map.md`` §11.

**Read-only, plus three lifecycle ops.** Deliberately not a second mutation
surface: every write routes through the same :class:`GroupToolkit` the
orchestrator uses, so there is still exactly one writer and no table is ever
written directly. That is the guard against ``06`` §7's *"a table quietly
becomes the source of truth"*, and it is why the detail endpoint returns the
projection **tables** rather than ``GroupToolkit.board()``'s formatted string.
``board()`` renders prose for a model to read; serving that to a UI would mean
parsing it back, which is the same failure wearing a different hat.

The one write the model cannot make is :meth:`resolve_approval`. A member asks
for permission; only a person grants it. So it is an HTTP route with no
counterpart in ``_OPS`` — an approval the orchestrator can grant itself is not
an approval.
"""
from __future__ import annotations

from fastapi import APIRouter
from fastapi.responses import JSONResponse
from pydantic import BaseModel
import re
import uuid

from agents.group import events as ev
from agents.group import messages as group_messages

router = APIRouter(tags=["groups"])

#: Ops the board may perform. Narrower than ``group_update``'s set on purpose:
#: the board monitors and answers, it does not edit the plan. Adding items,
#: members and capabilities stays with the orchestrator, which validates them
#: against the plan and the live catalogues.
UI_OPS = ("pause", "resume", "close")

#: How many events the detail view returns by default.
EVENT_TAIL = 200


def _registry():
    """Lazy: keeps this module importable without a projects root on disk."""
    from server.store import group_registry
    return group_registry


def _off_board(exc: Exception) -> bool:
    """An unknown project is a 404, not a 400.

    ``OffBoard`` also stringifies with its own class name, which is not
    language to show a person — so the caller reports the message alone.
    """
    from agents.group.tools import OffBoard
    return isinstance(exc, OffBoard)


class ApprovalRequest(BaseModel):
    grant: bool = True
    note: str = ""


class GroupOpRequest(BaseModel):
    op: str = ""
    params: dict = {}


class CreateGroupRequest(BaseModel):
    goal: str
    workspace_id: str = ""


def _progress(project_id: str) -> dict:
    """Counts plus a small live preview for one panel row.

    Never raises — a broken project still has to be *listed*, because a project
    that vanishes from the board is worse than one shown as broken.
    """
    out = {"milestones_done": 0, "milestones_total": 0,
           "items_done": 0, "items_total": 0, "awaiting_approval": 0,
           "goal": "", "active_item": None, "active_member": None,
           "participants": [], "last_message": None, "last_activity_ts": 0}
    try:
        store = _registry().open_store(project_id)
        project = store.project(project_id) or {}
        milestones = store.milestones(project_id)
        items = store.items(project_id)
        events = store.events_tail(project_id, 80)
    except Exception:  # noqa: BLE001 - an unreadable store is data, not a crash
        return out
    out["goal"] = str(project.get("goal") or "")
    out["milestones_total"] = len(milestones)
    out["milestones_done"] = sum(1 for m in milestones if m.get("status") == "done")
    out["items_total"] = len(items)
    out["items_done"] = sum(1 for i in items if i.get("status") == "done")
    out["awaiting_approval"] = sum(
        1 for i in items if i.get("blocked_reason") == ev.AWAITING_APPROVAL)
    active = next(
        (i for i in items if i.get("status") == "doing"),
        next((i for i in items if i.get("status") == "ready"), None),
    )
    if active:
        out["active_item"] = {
            "id": active.get("id"), "title": active.get("title"),
            "status": active.get("status"),
        }
        out["active_member"] = active.get("lease_owner") or active.get("owner_member")
    participants = set()
    for item in items:
        for key in ("owner_member", "lease_owner"):
            value = str(item.get(key) or "").strip()
            if value:
                participants.add(value)
    for event in events[-80:]:
        actor = str(event.get("actor") or "").strip()
        if actor and not actor.startswith("member:"):
            participants.add(actor)
        elif actor.startswith("member:"):
            participants.add(actor.split(":", 1)[1])
    out["participants"] = sorted(participants)
    try:
        recent_messages = group_messages.from_events(events)
        out["last_message"] = group_messages.preview(
            recent_messages[-1] if recent_messages else None
        )
    except Exception:  # noqa: BLE001 - preview is optional
        pass
    if events:
        last = events[-1]
        try:
            out["last_activity_ts"] = float(last.get("ts") or 0)
        except (TypeError, ValueError):
            out["last_activity_ts"] = 0
    return out


# ── reads ───────────────────────────────────────────────────────────────────


def _starter_manifest(goal: str):
    """Use the deterministic planner floor to create a runnable starter board."""
    from agents.group import manifest as manifest_mod
    from agents.group import planner
    from agents.workspace_registry import WORKSPACE_REGISTRY

    clean_goal = str(goal or "").strip()
    if not clean_goal:
        raise ValueError("goal is required")
    catalog = [
        {
            "name": spec.key,
            "key": spec.key,
            "display": spec.display,
            "description": spec.description,
            "when_to_use": spec.orchestrator_blurb,
            "tools": list(spec.tools),
            "model": spec.model,
            "thinking": spec.thinking,
        }
        for spec in WORKSPACE_REGISTRY.values()
    ]
    environment = planner.propose_environment(clean_goal, agents=catalog)
    if not environment.get("members"):
        spec = WORKSPACE_REGISTRY["developer"]
        environment["members"] = [{
            "name": spec.key,
            "derived_from": spec.key,
            "model": spec.model,
            "thinking": spec.thinking,
            "tools": list(spec.tools),
            "skills": [],
            "mcp": [],
            "why": "default implementer for a new project",
        }]
    # `ask_user` is structurally forbidden in a group project: nobody can
    # answer it while the member is unattended. `run_command` is allowed in
    # the manifest but defaults to the `blocked` tier below, so the runtime
    # can ask for approval without the member silently losing the tool.
    for member in environment["members"]:
        member.pop("why", None)
        tools = list(member.get("tools") or [])
        member["tools"] = [tool for tool in tools if tool != "ask_user"]
    milestones = planner.propose_plan(clean_goal, environment)
    base = re.sub(r"[^a-z0-9]+", "-", clean_goal.lower()).strip("-")[:42].strip("-") or "group-project"
    manifest_id = f"{base}-{uuid.uuid4().hex[:6]}"
    data = {
        "schema": 1,
        "id": manifest_id,
        "goal": clean_goal,
        "owner": "localhost",
        "members": environment["members"],
        "milestones": milestones,
        "environment": {"skills": [], "mcp": [], "deps": {}},
        "autonomy": {
            "project_folder": "autonomous",
            "external_read": "autonomous",
            "external_write": "needs_approval",
            "destructive": "blocked",
            "run_command": "blocked",
        },
        "completion": {"kind": "all_milestones_done"},
    }
    authored_title = clean_goal.splitlines()[0][:96]
    authored = manifest_mod.Manifest(
        title=authored_title,
        prose=f"# {authored_title}\n\n## Why\n{clean_goal}",
        data=data,
    )
    authored.validate()
    return authored


@router.post("/api/groups")
async def create_group(req: CreateGroupRequest):
    goal = (req.goal or "").strip()
    if not goal:
        return JSONResponse(status_code=400, content={"error": "goal is required"})
    try:
        registry = _registry()
        manifest = _starter_manifest(goal)
        result = registry.toolkit().create_group_project(
            registry.root(), manifest_mod_render(manifest),
        )
        if not getattr(result, "ok", False):
            return JSONResponse(status_code=400, content={"error": str(result.message)})
        project_id = str((result.data or {}).get("project_id") or manifest.id)
        registry.set_origin_workspace(project_id, req.workspace_id.strip() or None)
        return JSONResponse(status_code=201, content={
            "id": project_id,
            "path": str((result.data or {}).get("path") or ""),
            "title": manifest.title or goal,
        })
    except Exception as exc:  # noqa: BLE001 - creation errors are API errors
        return JSONResponse(status_code=400, content={"error": f"{type(exc).__name__}: {exc}"})


def manifest_mod_render(manifest) -> str:
    from agents.group.manifest import render
    return render(manifest)


@router.get("/api/groups")
async def list_groups(workspace_id: str = ""):
    """Projects visible from one workspace.

    Scoping lives in the registry (``list_projects_for``) so the rule has
    exactly one home. ``None``/empty means "every project", which is what a
    non-coding workspace shows.
    """
    try:
        rows = _registry().list_projects_for(workspace_id or None)
    except Exception as exc:  # noqa: BLE001
        return JSONResponse(status_code=500,
                            content={"error": f"{type(exc).__name__}: {exc}"})
    projects = []
    for row in rows:
        projects.append({**row, **_progress(row["id"])})
    return JSONResponse(content={
        "projects": projects,
        "scoped": bool(workspace_id) and _registry().workspace_kind(workspace_id) == "project",
    })


@router.get("/api/groups/{project_id}")
async def get_group(project_id: str, events: int = EVENT_TAIL):
    """One project in full: the tables, plus the event tail.

    The tail is what satisfies Definition-of-Done criterion 6 — you can read,
    from the event log alone, exactly what happened while you were asleep.
    """
    found = _registry().resolve(project_id)
    if found is None:
        return JSONResponse(status_code=404,
                            content={"error": f"unknown project {project_id!r}"})
    paths, store, _context = found
    try:
        limit = max(1, min(int(events or EVENT_TAIL), 500))
    except (TypeError, ValueError):
        limit = EVENT_TAIL

    # Whether this project's members hold a shell. A shell reaches anywhere, so
    # the rest of the envelope is advisory once one is opted in (``06`` §3) —
    # the modal says so rather than letting the declaration imply more than it
    # delivers.
    shell_enabled = False
    try:
        from agents.group import manifest as manifest_mod
        from agents.group import materialize as materialize_mod
        shell_enabled = (
            "run_command"
            not in materialize_mod.never_tools(manifest_mod.load(paths.manifest).autonomy)
        )
    except Exception:  # noqa: BLE001, S110 - a broken manifest is data, not a crash
        pass

    events_tail = store.events_tail(project_id, limit)
    try:
        members = []
        for spec in getattr(_context, "manifest_members", []) or []:
            name = str(spec.get("name") or "").strip()
            if not name:
                continue
            agent = (_context.members or {}).get(name) or {}
            members.append({
                "name": name,
                "display": str(agent.get("display") or spec.get("display") or name),
                "kind": str(agent.get("kind") or spec.get("kind") or "member"),
                "derived_from": str(agent.get("derived_from") or spec.get("derived_from") or ""),
                "model": str(agent.get("model") or spec.get("model") or ""),
            })
        if not members:
            members = [{"name": "orchestrator", "display": "Orchestrator", "kind": "supervisor"}]
    except Exception:  # noqa: BLE001 - team display is optional
        members = []
    return JSONResponse(content={
        "id": project_id,
        "path": paths.project_dir,
        "shell_enabled": shell_enabled,
        "project": store.project(project_id) or {},
        "members": members,
        "messages": group_messages.from_events(events_tail),
        "milestones": store.milestones(project_id),
        "items": store.items(project_id),
        "events": events_tail,
    })


# ── the human's answer ──────────────────────────────────────────────────────


@router.post("/api/groups/{project_id}/items/{item_id}/approval")
async def resolve_approval(project_id: str, item_id: str, req: ApprovalRequest):
    """Grant or reject a parked item. The only way an item leaves
    ``awaiting_approval`` — without this it parked silently and forever."""
    try:
        result = _registry().toolkit().resolve_approval(
            project_id, item_id=item_id, grant=bool(req.grant),
            note=(req.note or "").strip())
    except Exception as exc:  # noqa: BLE001
        if _off_board(exc):
            return JSONResponse(status_code=404, content={"error": str(exc)})
        return JSONResponse(status_code=400,
                            content={"error": f"{type(exc).__name__}: {exc}"})
    if not getattr(result, "ok", False):
        return JSONResponse(status_code=404,
                            content={"error": str(getattr(result, "message", "failed"))})
    return JSONResponse(content={"ok": True, "message": str(result.message)})


# ── lifecycle ───────────────────────────────────────────────────────────────


@router.post("/api/groups/{project_id}/update")
async def group_op(project_id: str, req: GroupOpRequest):
    """Pause, resume or close. Queued — applied on the next supervisor tick.

    ``queued: True`` is the honest answer, not decoration: the UI must say
    "applies within ~15s" rather than spin as though it were instantaneous.
    """
    op = str(req.op or "").strip()
    if op not in UI_OPS:
        return JSONResponse(status_code=400,
                            content={"error": f"op must be one of {', '.join(UI_OPS)}"})
    try:
        method = getattr(_registry().toolkit(), op)
        result = method(project_id, **dict(req.params or {}))
    except TypeError as exc:
        return JSONResponse(status_code=400, content={"error": f"bad params: {exc}"})
    except Exception as exc:  # noqa: BLE001
        if _off_board(exc):
            return JSONResponse(status_code=404, content={"error": str(exc)})
        return JSONResponse(status_code=400,
                            content={"error": f"{type(exc).__name__}: {exc}"})
    return JSONResponse(content={"ok": True, "queued": True, "message": str(result)})
