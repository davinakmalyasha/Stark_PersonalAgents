"""
Snapshots / chat mode router â€” Phase 7 moved from server/api_server.py:466-546.
"""
from fastapi import APIRouter; from fastapi.responses import JSONResponse
from pydantic import BaseModel

router = APIRouter(tags=["snapshots"])


class ModeRequest(BaseModel):
    mode: str  # "plan" | "build"


def _resolve_ws(workspace_path: str) -> str:
    if workspace_path:
        return workspace_path
    try:
        from sync.state_sync import safe_get_state
        return safe_get_state().get("project_path", "")
    except Exception:
        return ""


@router.get("/api/chats/{chat_id}/snapshots")
async def get_chat_snapshots(chat_id: str, workspace_path: str = "", limit: int = 100):
    """List run-grouped snapshots for a chat. Newest first."""
    try:
        from tools.checkpoints import list_snapshots
        ws = _resolve_ws(workspace_path)
        if not ws:
            return JSONResponse(content={"groups": [], "workspace_path": ""})
        groups = list_snapshots(chat_id, ws, limit=limit)
        return JSONResponse(content={"groups": groups, "workspace_path": ws, "chat_id": chat_id})
    except Exception as exc:
        return JSONResponse(status_code=500, content={"error": f"snapshot list failed: {exc}"})


@router.post("/api/chats/{chat_id}/snapshots/{snap_id}/restore")
async def restore_chat_snapshot(chat_id: str, snap_id: str, workspace_path: str = "", force: bool = False):
    """Restore a single snapshot. Returns 409 on sha mismatch unless `force=true`."""
    try:
        from tools.checkpoints import restore_snapshot, inspect_snapshot
        ws = _resolve_ws(workspace_path)
        if not ws:
            return JSONResponse(status_code=400, content={"error": "no workspace_path"})
        if not force:
            info = inspect_snapshot(chat_id, ws, snap_id)
            if info.get("exists") and info.get("conflicts"):
                return JSONResponse(
                    status_code=409,
                    content={"ok": False, "message": "file has changed since snapshot; retry with force=true", "info": info},
                )
        ok, msg = restore_snapshot(chat_id, ws, snap_id)
        return JSONResponse(content={"ok": ok, "message": msg})
    except Exception as exc:
        return JSONResponse(status_code=500, content={"error": f"restore failed: {exc}"})


@router.post("/api/chats/{chat_id}/runs/{run_id}/restore")
async def restore_chat_run(chat_id: str, run_id: str, workspace_path: str = ""):
    """Restore every snapshot for a given run_id (the rewind primitive used by /rewind)."""
    try:
        from tools.checkpoints import restore_run
        ws = _resolve_ws(workspace_path)
        if not ws:
            return JSONResponse(status_code=400, content={"error": "no workspace_path"})
        all_ok, msgs = restore_run(chat_id, ws, run_id)
        return JSONResponse(content={"ok": all_ok, "messages": msgs})
    except Exception as exc:
        return JSONResponse(status_code=500, content={"error": f"restore-run failed: {exc}"})


@router.post("/api/chat/{chat_id}/mode")
async def set_chat_mode(chat_id: str, req: ModeRequest):
    """Set the chat mode â€” per-chat dropdown (survives restart). User-only prompt-dock control. Agents have no mode-switch capabilityâ†’build."""
    try:
        from server.store.mode_store import set_mode
        VALID = {"plan", "build"}
        if req.mode not in VALID:
            return JSONResponse(
                status_code=400,
                content={"error": f"invalid mode {req.mode!r}; expected one of {list(VALID)}"},
            )
        set_mode(chat_id, req.mode)
        return JSONResponse(content={"ok": True, "chat_id": chat_id, "mode": req.mode})
    except Exception as exc:
        return JSONResponse(status_code=500, content={"error": f"set mode failed: {exc}"})


@router.get("/api/chat/{chat_id}/state")
async def get_chat_state(chat_id: str):
    """Return the chat's current mode (and any future state)."""
    try:
        from server.store.mode_store import get_mode
        mode = get_mode(chat_id)
        return JSONResponse(content={"chat_id": chat_id, "mode": mode})
    except Exception as exc:
        return JSONResponse(status_code=500, content={"error": f"get state failed: {exc}"})



