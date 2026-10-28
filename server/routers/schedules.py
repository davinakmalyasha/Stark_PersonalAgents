"""HTTP API for chat schedules shown in the task panel."""
from __future__ import annotations

from fastapi import APIRouter
from fastapi.responses import JSONResponse
from pydantic import BaseModel

from server.store import schedules as schedule_store
from server.store.chat_store import find_chat_workspace_id, get_chat
from server.store.workspace_registry import get_workspace

router = APIRouter(tags=["schedules"])
RECURRENCES = {"none", "daily", "weekly", "monthly"}


class ScheduleRequest(BaseModel):
    workspace_id: str = ""
    chat_id: str
    name: str = "Scheduled task"
    prompt: str
    at: str
    recurrence: str = "none"


def _public(record: dict) -> dict:
    chat_id = str(record.get("chatId") or "")
    workspace_id = find_chat_workspace_id(chat_id)
    return {
        "id": record["id"],
        "name": record["name"],
        "prompt": record["prompt"],
        "workspaceId": workspace_id,
        "chatId": chat_id,
        "nextRunAt": record["nextRunAt"],
        "recurrence": record["recurrence"],
        "status": record["status"],
    }


@router.get("/api/schedules")
async def list_schedules(workspace_id: str = ""):
    records = schedule_store.list_schedules()
    items = [_public(record) for record in records]
    if workspace_id:
        items = [item for item in items if item["workspaceId"] == workspace_id]
    return JSONResponse(content={"schedules": items})


@router.post("/api/schedules")
async def create_schedule(req: ScheduleRequest):
    recurrence = (req.recurrence or "none").strip().lower()
    if recurrence not in RECURRENCES:
        return JSONResponse(status_code=400, content={
            "error": "recurrence must be none, daily, weekly, or monthly"
        })
    chat_id = req.chat_id.strip()
    prompt = req.prompt.strip()
    if not prompt:
        return JSONResponse(status_code=400, content={"error": "prompt is required"})
    workspace_id = find_chat_workspace_id(chat_id)
    if not workspace_id:
        return JSONResponse(status_code=404, content={"error": "Chat not found"})
    if req.workspace_id and req.workspace_id != workspace_id:
        return JSONResponse(status_code=400, content={"error": "Chat does not belong to that workspace"})
    workspace = get_workspace(workspace_id)
    if not workspace or not get_chat(workspace["path"], chat_id):
        return JSONResponse(status_code=404, content={"error": "Chat not found"})
    try:
        record = schedule_store.create_schedule(
            name=req.name.strip() or "Scheduled task",
            prompt=prompt,
            workspace_path=str(workspace.get("path") or ""),
            chat_id=chat_id,
            run_at=req.at,
            recurrence=recurrence,
        )
    except ValueError as exc:
        return JSONResponse(status_code=400, content={"error": str(exc)})
    except Exception as exc:  # noqa: BLE001 - return a useful API error
        return JSONResponse(status_code=500, content={"error": f"{type(exc).__name__}: {exc}"})
    return JSONResponse(status_code=201, content=_public(record))


@router.delete("/api/schedules/{schedule_id}")
async def delete_schedule(schedule_id: str):
    if not schedule_store.cancel_schedule(schedule_id):
        return JSONResponse(status_code=404, content={"error": "Schedule not found"})
    return JSONResponse(content={"ok": True, "id": schedule_id, "status": "cancelled"})
