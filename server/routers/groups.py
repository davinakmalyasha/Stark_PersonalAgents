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

from agents.group import events as ev

router = APIRouter(tags=["groups"])

#: Ops the board may perform. Narrower than ``group_update``'s set on purpose:
#: the board monitors and answers, it does not edit the plan. Adding items,
#: members and capabilities stays with the orchestrator, which validates them
#: against the plan and the live catalogues.
UI_OPS = ("pause", "resume", "close")

#: How many events the detail view returns by default.
EVENT_TAIL = 60


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


def _progress(project_id: str) -> dict:
    """Cheap counts for one panel row. Never raises — a broken project still
    has to be *listed*, because a project that vanishes from the board is
    worse than one shown as broken."""
    out = {"milestones_done": 0, "milestones_total": 0,
           "items_done": 0, "items_total": 0, "awaiting_approval": 0}
    try:
        store = _registry().open_store(project_id)
        milestones = store.milestones(project_id)
        items = store.items(project_id)
    except Exception:  # noqa: BLE001 - an unreadable store is data, not a crash
        return out
    out["milestones_total"] = len(milestones)
    out["milestones_done"] = sum(1 for m in milestones if m.get("status") == "done")
    out["items_total"] = len(items)
    out["items_done"] = sum(1 for i in items if i.get("status") == "done")
    out["awaiting_approval"] = sum(
        1 for i in items if i.get("blocked_reason") == ev.AWAITING_APPROVAL)
    return out


# ── reads ───────────────────────────────────────────────────────────────────


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

    return JSONResponse(content={
        "id": project_id,
        "path": paths.project_dir,
        "shell_enabled": shell_enabled,
        "project": store.project(project_id) or {},
        "milestones": store.milestones(project_id),
        "items": store.items(project_id),
        "events": store.events(project_id, limit=limit),
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
