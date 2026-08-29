"""
Realtime/inbox router — Phase 7 moved from server/api_server.py:1218-1250.
Steer / follow-up / abort + pending counts via server.realtime.inbox.
"""
from fastapi import APIRouter
from pydantic import BaseModel

from server.realtime import inbox

router = APIRouter(tags=["inbox"])


class InboxRequest(BaseModel):
    chat_id: str = ""
    message: str = ""


@router.post("/api/steer")
async def api_steer(req: InboxRequest):
    """Queue a steering message: delivered after the current step's tool calls."""
    ok = inbox.steer(req.chat_id, req.message)
    return {"ok": ok, "pending": inbox.pending_counts(req.chat_id)}


@router.post("/api/followup")
async def api_followup(req: InboxRequest):
    """Queue a follow-up message: delivered only after the agent is idle."""
    ok = inbox.follow_up(req.chat_id, req.message)
    return {"ok": ok, "pending": inbox.pending_counts(req.chat_id)}


@router.post("/api/abort")
async def api_abort(req: InboxRequest):
    """Abort the current run; queued steering/follow-up are preserved."""
    inbox.abort(req.chat_id, keep_inbox=True)
    return {"ok": True, "pending": inbox.pending_counts(req.chat_id)}


@router.get("/api/inbox")
async def api_inbox(chat_id: str = ""):
    return inbox.pending_counts(chat_id)
