"""
Tasks router â€” Phase 7 moved from server/api_server.py:882-954.
"""
import uuid
from fastapi import APIRouter; from fastapi.responses import JSONResponse
from pydantic import BaseModel

from server.store.workspace_registry import GENERAL_WORKSPACE_ID
from server.store.chat_store import get_chat_tasks, save_chat_tasks
from server.services.task_scheduler import (
    workspace_path_for_id, task_now, update_task,
)

router = APIRouter(tags=["tasks"])


class TaskRequest(BaseModel):
    prompt: str = ""
    attachments: list[dict] = []
    mode: str = "saved"
    schedule: dict | None = None


class TaskUpdateRequest(BaseModel):
    status: str | None = None
    prompt: str | None = None
    attachments: list[dict] | None = None
    mode: str | None = None
    schedule: dict | None = None


@router.get("/api/workspaces/{workspace_id}/chats/{chat_id}/tasks")
async def list_tasks(workspace_id: str, chat_id: str):
    try:
        return JSONResponse(content=get_chat_tasks(workspace_path_for_id(workspace_id), chat_id))
    except ValueError as exc:
        return JSONResponse(status_code=404, content={"error": str(exc)})


@router.post("/api/workspaces/{workspace_id}/chats/{chat_id}/tasks")
async def create_task(workspace_id: str, chat_id: str, req: TaskRequest):
    try:
        workspace_path = workspace_path_for_id(workspace_id)
        if not req.prompt.strip() and not req.attachments:
            return JSONResponse(status_code=400, content={"error": "A prompt or attachment is required"})
        now = task_now()
        schedule = req.schedule or {}
        status = "scheduled" if req.mode == "scheduled" else "ready"
        task = {"id": f"task-{uuid.uuid4().hex[:12]}", "workspaceId": workspace_id, "chatId": chat_id, "prompt": req.prompt, "attachments": req.attachments, "mode": req.mode, "status": status, "schedule": schedule, "createdAt": now.isoformat(), "updatedAt": now.isoformat()}
        tasks = get_chat_tasks(workspace_path, chat_id)
        tasks.insert(0, task)
        if not save_chat_tasks(workspace_path, chat_id, tasks):
            return JSONResponse(status_code=404, content={"error": "Chat not found"})
        return JSONResponse(content=task)
    except ValueError as exc:
        return JSONResponse(status_code=404, content={"error": str(exc)})


@router.post("/api/workspaces/{workspace_id}/chats/{chat_id}/tasks/{task_id}/start")
async def start_task(workspace_id: str, chat_id: str, task_id: str):
    try:
        workspace_path = workspace_path_for_id(workspace_id)
        task = next((item for item in get_chat_tasks(workspace_path, chat_id) if item.get("id") == task_id), None)
        if not task:
            return JSONResponse(status_code=404, content={"error": "Task not found"})
        updated = update_task(workspace_path, chat_id, task_id, status="running", lastRunAt=task_now().isoformat()) or {**task, "status": "running"}
        return JSONResponse(content=updated)
    except ValueError as exc:
        return JSONResponse(status_code=404, content={"error": str(exc)})


@router.patch("/api/workspaces/{workspace_id}/chats/{chat_id}/tasks/{task_id}")
async def update_task(workspace_id: str, chat_id: str, task_id: str, req: TaskUpdateRequest):
    try:
        workspace_path = workspace_path_for_id(workspace_id)
        if req.status is not None and req.status not in {"ready", "scheduled", "running", "completed", "failed"}:
            return JSONResponse(status_code=400, content={"error": "Invalid task status"})
        changes = req.model_dump(exclude_unset=True, exclude_none=True)
        if "prompt" in changes and not str(changes["prompt"]).strip() and not changes.get("attachments"):
            return JSONResponse(status_code=400, content={"error": "A prompt or attachment is required"})
        if changes.get("mode") not in (None, "saved", "scheduled"):
            return JSONResponse(status_code=400, content={"error": "Invalid task mode"})
        if req.status == "completed":
            changes["completedAt"] = task_now().isoformat()
        updated = update_task(workspace_path, chat_id, task_id, **changes)
        if not updated:
            return JSONResponse(status_code=404, content={"error": "Task not found"})
        return JSONResponse(content=updated)
    except ValueError as exc:
        return JSONResponse(status_code=404, content={"error": str(exc)})


@router.delete("/api/workspaces/{workspace_id}/chats/{chat_id}/tasks/{task_id}")
async def delete_task(workspace_id: str, chat_id: str, task_id: str):
    try:
        workspace_path = workspace_path_for_id(workspace_id)
        tasks = get_chat_tasks(workspace_path, chat_id)
        filtered = [task for task in tasks if task.get("id") != task_id]
        if len(filtered) == len(tasks):
            return JSONResponse(status_code=404, content={"error": "Task not found"})
        save_chat_tasks(workspace_path, chat_id, filtered)
        return JSONResponse(content={"ok": True})
    except ValueError as exc:
        return JSONResponse(status_code=404, content={"error": str(exc)})



