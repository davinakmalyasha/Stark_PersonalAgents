"""Durable sticky notes for chats and group projects (orchestrator-owned)."""
from __future__ import annotations

import json
import threading
import uuid
from datetime import datetime

from server.store.chat_store import _get_db

_LOCK = threading.RLock()
_READY = False


def _normalize_todos(value) -> list[dict]:
    out: list[dict] = []
    for item in value or []:
        if isinstance(item, str):
            text = item.strip()
            done = False
        elif isinstance(item, dict):
            text = str(item.get("text") or item.get("title") or "").strip()
            done = bool(item.get("done"))
        else:
            continue
        if text:
            out.append({"text": text, "done": done})
    return out


def _init() -> None:
    global _READY
    if _READY:
        return
    with _LOCK:
        if _READY:
            return
        db = _get_db()
        db.execute("""
            CREATE TABLE IF NOT EXISTS agent_notes (
                id TEXT PRIMARY KEY,
                scope_type TEXT NOT NULL,
                scope_id TEXT NOT NULL,
                workspace_path TEXT DEFAULT '',
                title TEXT DEFAULT '',
                body TEXT DEFAULT '',
                todos_json TEXT DEFAULT '[]',
                created_by TEXT DEFAULT 'orchestrator',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
        """)
        db.execute("CREATE INDEX IF NOT EXISTS idx_agent_notes_scope ON agent_notes(scope_type, scope_id)")
        _READY = True

def _row(row) -> dict:
    try:
        todos = json.loads(row[6] or "[]")
    except Exception:
        todos = []
    return {
        "id": row[0],
        "scopeType": row[1],
        "scopeId": row[2],
        "workspacePath": row[3] or "",
        "title": row[4] or "",
        "body": row[5] or "",
        "todos": todos,
        "createdBy": row[7] or "orchestrator",
        "createdAt": row[8],
        "updatedAt": row[9],
    }


def list_notes(scope_type: str, scope_id: str) -> list[dict]:
    _init()
    with _LOCK:
        rows = _get_db().execute(
            "SELECT id, scope_type, scope_id, workspace_path, title, body, todos_json,"
            " created_by, created_at, updated_at FROM agent_notes"
            " WHERE scope_type=? AND scope_id=? ORDER BY updated_at DESC",
            (scope_type, scope_id),
        ).fetchall()
    return [_row(r) for r in rows]


def get_note(note_id: str) -> dict | None:
    _init()
    with _LOCK:
        row = _get_db().execute(
            "SELECT id, scope_type, scope_id, workspace_path, title, body, todos_json,"
            " created_by, created_at, updated_at FROM agent_notes WHERE id=?",
            (note_id,),
        ).fetchone()
    return _row(row) if row else None


def create_note(scope_type: str, scope_id: str, title: str, body: str = "",
                todos=None, workspace_path: str = "", created_by: str = "orchestrator") -> dict:
    _init()
    now = datetime.now().isoformat()
    note_id = f"note-{uuid.uuid4().hex[:10]}"
    title = str(title or "").strip() or "Note"
    with _LOCK:
        _get_db().execute(
            "INSERT INTO agent_notes (id, scope_type, scope_id, workspace_path, title, body,"
            " todos_json, created_by, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?)",
            (note_id, scope_type, scope_id, workspace_path or "", title, str(body or ""),
             json.dumps(_normalize_todos(todos), ensure_ascii=False), created_by, now, now),
        )
    return get_note(note_id) or {}


def update_note(note_id: str, *, title: str | None = None, body: str | None = None,
                todos=None) -> dict | None:
    _init()
    note = get_note(note_id)
    if not note:
        return None
    fields = []
    values = []
    if title is not None:
        fields.append("title=?")
        values.append(str(title).strip() or "Note")
    if body is not None:
        fields.append("body=?")
        values.append(str(body))
    if todos is not None:
        fields.append("todos_json=?")
        values.append(json.dumps(_normalize_todos(todos), ensure_ascii=False))
    if not fields:
        return note
    fields.append("updated_at=?")
    values.append(datetime.now().isoformat())
    values.append(note_id)
    with _LOCK:
        _get_db().execute(f"UPDATE agent_notes SET {', '.join(fields)} WHERE id=?", values)
    return get_note(note_id)


def delete_note(note_id: str) -> bool:
    _init()
    with _LOCK:
        cur = _get_db().execute("DELETE FROM agent_notes WHERE id=?", (note_id,))
    return bool(cur.rowcount)


def delete_notes_of(scope_type: str, scope_id: str) -> int:
    _init()
    with _LOCK:
        cur = _get_db().execute(
            "DELETE FROM agent_notes WHERE scope_type=? AND scope_id=?",
            (scope_type, scope_id),
        )
    return int(cur.rowcount or 0)
