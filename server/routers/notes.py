"""Sticky-note API for chats and group projects."""
from __future__ import annotations

from fastapi import APIRouter
from fastapi.responses import JSONResponse
from pydantic import BaseModel

from server.store import agent_notes

router = APIRouter(tags=["notes"])


class NoteCreate(BaseModel):
    scope_type: str = "chat"
    scope_id: str = ""
    title: str = "Note"
    body: str = ""
    todos: list = []


class NoteUpdate(BaseModel):
    title: str | None = None
    body: str | None = None
    todos: list | None = None


def _scope(value: str) -> str:
    value = str(value or "").lower()
    return value if value in ("chat", "group") else ""


@router.get("/api/notes/{scope_type}/{scope_id}")
async def list_notes(scope_type: str, scope_id: str):
    kind = _scope(scope_type)
    if not kind or not scope_id:
        return JSONResponse(status_code=400, content={"error": "invalid scope"})
    return JSONResponse(content={"notes": agent_notes.list_notes(kind, scope_id)})


@router.post("/api/notes")
async def create_note(req: NoteCreate):
    kind = _scope(req.scope_type)
    if not kind or not req.scope_id:
        return JSONResponse(status_code=400, content={"error": "invalid scope"})
    note = agent_notes.create_note(
        kind, req.scope_id, req.title, req.body, req.todos, created_by="user",
    )
    return JSONResponse(content=note)


@router.patch("/api/notes/{note_id}")
async def update_note(note_id: str, req: NoteUpdate):
    note = agent_notes.update_note(
        note_id, title=req.title, body=req.body, todos=req.todos,
    )
    if not note:
        return JSONResponse(status_code=404, content={"error": "note not found"})
    return JSONResponse(content=note)


@router.delete("/api/notes/{note_id}")
async def delete_note(note_id: str):
    if not agent_notes.delete_note(note_id):
        return JSONResponse(status_code=404, content={"error": "note not found"})
    return JSONResponse(content={"ok": True})
