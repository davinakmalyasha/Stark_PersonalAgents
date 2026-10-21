"""Canvas-mode status for the chat UI."""
from fastapi import APIRouter
from fastapi.responses import JSONResponse

from server.store import agent_wakes

router = APIRouter(tags=["canvas"])


@router.get("/api/canvas/{chat_id}/status")
async def canvas_status(chat_id: str):
    watch = agent_wakes.active_watch(chat_id)
    return JSONResponse(content={"active": bool(watch), "watch": watch})
