"""Durable wake queue and canvas-mode watch state."""
from __future__ import annotations

import json
import threading
import uuid
from datetime import datetime, timezone

from server.store.chat_store import _get_db

_LOCK = threading.RLock()
_READY = False


def _now() -> str:
    return datetime.now(timezone.utc).isoformat()


def _parse(value: str | None) -> datetime | None:
    if not value:
        return None
    try:
        parsed = datetime.fromisoformat(str(value).replace("Z", "+00:00"))
        if parsed.tzinfo is None:
            parsed = parsed.astimezone()
        return parsed.astimezone(timezone.utc)
    except Exception:
        return None


def _init() -> None:
    global _READY
    if _READY:
        return
    with _LOCK:
        if _READY:
            return
        db = _get_db()
        db.execute("""
            CREATE TABLE IF NOT EXISTS pending_wakes (
                id TEXT PRIMARY KEY,
                workspace_path TEXT DEFAULT '',
                chat_id TEXT NOT NULL,
                text TEXT NOT NULL,
                metadata_json TEXT DEFAULT '{}',
                mode TEXT DEFAULT 'orchestrator',
                status TEXT DEFAULT 'queued',
                attempts INTEGER DEFAULT 0,
                not_before TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
        """)
        db.execute("CREATE INDEX IF NOT EXISTS idx_pending_wakes_due ON pending_wakes(status, not_before)")
        db.execute("""
            CREATE TABLE IF NOT EXISTS canvas_watches (
                id TEXT PRIMARY KEY,
                orchestrator_chat_id TEXT NOT NULL,
                workspace_path TEXT DEFAULT '',
                mode TEXT DEFAULT 'all',
                targets_json TEXT DEFAULT '[]',
                wake_on_json TEXT DEFAULT '["completed","failed"]',
                max_actions INTEGER DEFAULT 20,
                actions_used INTEGER DEFAULT 0,
                until TEXT,
                status TEXT DEFAULT 'active',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
        """)
        db.execute("CREATE INDEX IF NOT EXISTS idx_canvas_watches_chat ON canvas_watches(orchestrator_chat_id, status)")
        db.execute("""
            CREATE TABLE IF NOT EXISTS canvas_events (
                id TEXT PRIMARY KEY,
                watch_id TEXT NOT NULL,
                source_chat_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                payload_json TEXT DEFAULT '{}',
                created_at TEXT NOT NULL
            )
        """)
        db.execute("CREATE INDEX IF NOT EXISTS idx_canvas_events_watch ON canvas_events(watch_id, created_at)")
        _READY = True

def enqueue_wake(workspace_path: str, chat_id: str, text: str,
                 metadata: dict | None = None, mode: str = "orchestrator",
                 not_before: str | None = None) -> dict:
    _init()
    wake_id = f"wake-{uuid.uuid4().hex[:10]}"
    now = _now()
    when = not_before or now
    with _LOCK:
        _get_db().execute(
            "INSERT INTO pending_wakes (id, workspace_path, chat_id, text, metadata_json,"
            " mode, status, attempts, not_before, created_at, updated_at)"
            " VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            (wake_id, workspace_path or "", chat_id, str(text or ""),
             json.dumps(metadata or {}, ensure_ascii=False), mode or "orchestrator",
             "queued", 0, when, now, now),
        )
    return get_wake(wake_id) or {}


def _wake_row(row) -> dict:
    try:
        meta = json.loads(row[4] or "{}")
    except Exception:
        meta = {}
    return {
        "id": row[0], "workspacePath": row[1] or "", "chatId": row[2],
        "text": row[3], "metadata": meta, "mode": row[5], "status": row[6],
        "attempts": row[7], "notBefore": row[8],
        "createdAt": row[9], "updatedAt": row[10],
    }


def get_wake(wake_id: str) -> dict | None:
    _init()
    with _LOCK:
        row = _get_db().execute(
            "SELECT id, workspace_path, chat_id, text, metadata_json, mode, status,"
            " attempts, not_before, created_at, updated_at FROM pending_wakes WHERE id=?",
            (wake_id,),
        ).fetchone()
    return _wake_row(row) if row else None


def due_wakes(limit: int = 20) -> list[dict]:
    _init()
    moment = _now()
    with _LOCK:
        rows = _get_db().execute(
            "SELECT id, workspace_path, chat_id, text, metadata_json, mode, status,"
            " attempts, not_before, created_at, updated_at FROM pending_wakes"
            " WHERE status='queued' AND not_before<=? ORDER BY created_at ASC LIMIT ?",
            (moment, limit),
        ).fetchall()
    return [_wake_row(r) for r in rows]


def mark_wake(wake_id: str, status: str, *, not_before: str | None = None,
              increment_attempt: bool = False) -> None:
    _init()
    now = _now()
    sets = ["status=?", "updated_at=?"]
    values: list = [status, now]
    if not_before is not None:
        sets.append("not_before=?")
        values.append(not_before)
    if increment_attempt:
        sets.append("attempts=COALESCE(attempts,0)+1")
    values.append(wake_id)
    with _LOCK:
        _get_db().execute(f"UPDATE pending_wakes SET {', '.join(sets)} WHERE id=?", values)


def recover_dispatching() -> int:
    _init()
    with _LOCK:
        cur = _get_db().execute(
            "UPDATE pending_wakes SET status='queued', updated_at=? WHERE status='dispatching'",
            (_now(),),
        )
    return int(cur.rowcount or 0)


def list_wakes(chat_id: str = "", limit: int = 50) -> list[dict]:
    _init()
    sql = ("SELECT id, workspace_path, chat_id, text, metadata_json, mode, status,"
           " attempts, not_before, created_at, updated_at FROM pending_wakes")
    params: list = []
    if chat_id:
        sql += " WHERE chat_id=?"
        params.append(chat_id)
    sql += " ORDER BY created_at DESC LIMIT ?"
    params.append(limit)
    with _LOCK:
        rows = _get_db().execute(sql, params).fetchall()
    return [_wake_row(r) for r in rows]


def _watch_row(row) -> dict:
    try:
        targets = json.loads(row[4] or "[]")
    except Exception:
        targets = []
    try:
        wake_on = json.loads(row[5] or "[]")
    except Exception:
        wake_on = []
    return {
        "id": row[0], "orchestratorChatId": row[1], "workspacePath": row[2] or "",
        "mode": row[3], "targets": targets, "wakeOn": wake_on,
        "maxActions": int(row[6] or 0), "actionsUsed": int(row[7] or 0),
        "until": row[8], "status": row[9], "createdAt": row[10], "updatedAt": row[11],
    }


def enter_watch(orchestrator_chat_id: str, workspace_path: str, mode: str,
                targets: list[str] | None, wake_on: list[str] | None = None,
                max_actions: int = 20, until: str | None = None) -> dict:
    _init()
    now = _now()
    watch_id = f"watch-{uuid.uuid4().hex[:10]}"
    with _LOCK:
        _get_db().execute(
            "UPDATE canvas_watches SET status='closed', updated_at=? WHERE orchestrator_chat_id=? AND status='active'",
            (now, orchestrator_chat_id),
        )
        _get_db().execute(
            "INSERT INTO canvas_watches (id, orchestrator_chat_id, workspace_path, mode,"
            " targets_json, wake_on_json, max_actions, actions_used, until, status,"
            " created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
            (watch_id, orchestrator_chat_id, workspace_path or "", mode or "all",
             json.dumps(targets or [], ensure_ascii=False),
             json.dumps(wake_on or ["completed", "failed"], ensure_ascii=False),
             max(1, int(max_actions or 20)), 0, until, "active", now, now),
        )
    return get_watch(watch_id) or {}


def get_watch(watch_id: str) -> dict | None:
    _init()
    with _LOCK:
        row = _get_db().execute(
            "SELECT id, orchestrator_chat_id, workspace_path, mode, targets_json,"
            " wake_on_json, max_actions, actions_used, until, status, created_at, updated_at"
            " FROM canvas_watches WHERE id=?", (watch_id,),
        ).fetchone()
    return _watch_row(row) if row else None


def active_watch(orchestrator_chat_id: str) -> dict | None:
    _init()
    with _LOCK:
        row = _get_db().execute(
            "SELECT id, orchestrator_chat_id, workspace_path, mode, targets_json,"
            " wake_on_json, max_actions, actions_used, until, status, created_at, updated_at"
            " FROM canvas_watches WHERE orchestrator_chat_id=? AND status='active'"
            " ORDER BY created_at DESC LIMIT 1", (orchestrator_chat_id,),
        ).fetchone()
    if not row:
        return None
    watch = _watch_row(row)
    deadline = _parse(watch.get("until"))
    if deadline is not None and deadline <= datetime.now(timezone.utc):
        close_watch(watch["id"])
        return None
    return watch


def active_watches() -> list[dict]:
    _init()
    with _LOCK:
        rows = _get_db().execute(
            "SELECT id, orchestrator_chat_id, workspace_path, mode, targets_json,"
            " wake_on_json, max_actions, actions_used, until, status, created_at, updated_at"
            " FROM canvas_watches WHERE status='active' ORDER BY created_at DESC",
        ).fetchall()
    out = []
    for row in rows:
        watch = _watch_row(row)
        deadline = _parse(watch.get("until"))
        if deadline is not None and deadline <= datetime.now(timezone.utc):
            close_watch(watch["id"])
            continue
        out.append(watch)
    return out


def close_watch(watch_id: str) -> None:
    _init()
    with _LOCK:
        _get_db().execute(
            "UPDATE canvas_watches SET status='closed', updated_at=? WHERE id=?",
            (_now(), watch_id),
        )


def close_watch_for_chat(orchestrator_chat_id: str) -> int:
    _init()
    with _LOCK:
        cur = _get_db().execute(
            "UPDATE canvas_watches SET status='closed', updated_at=?"
            " WHERE orchestrator_chat_id=? AND status='active'",
            (_now(), orchestrator_chat_id),
        )
    return int(cur.rowcount or 0)


def bump_watch(watch_id: str) -> int:
    _init()
    with _LOCK:
        _get_db().execute(
            "UPDATE canvas_watches SET actions_used=COALESCE(actions_used,0)+1, updated_at=? WHERE id=?",
            (_now(), watch_id),
        )
        row = _get_db().execute(
            "SELECT actions_used FROM canvas_watches WHERE id=?", (watch_id,),
        ).fetchone()
    return int(row[0] or 0) if row else 0


def record_event(watch_id: str, source_chat_id: str, kind: str, payload: dict) -> dict:
    _init()
    event_id = f"cev-{uuid.uuid4().hex[:10]}"
    now = _now()
    with _LOCK:
        _get_db().execute(
            "INSERT INTO canvas_events (id, watch_id, source_chat_id, kind, payload_json, created_at)"
            " VALUES (?,?,?,?,?,?)",
            (event_id, watch_id, source_chat_id, kind,
             json.dumps(payload or {}, ensure_ascii=False), now),
        )
    return {"id": event_id, "watchId": watch_id, "sourceChatId": source_chat_id,
            "kind": kind, "payload": payload, "createdAt": now}


def list_events(watch_id: str, limit: int = 50) -> list[dict]:
    _init()
    with _LOCK:
        rows = _get_db().execute(
            "SELECT id, source_chat_id, kind, payload_json, created_at"
            " FROM canvas_events WHERE watch_id=? ORDER BY created_at DESC LIMIT ?",
            (watch_id, max(1, min(int(limit or 50), 200))),
        ).fetchall()
    out = []
    for row in rows:
        try:
            payload = json.loads(row[3] or "{}")
        except Exception:
            payload = {}
        out.append({"id": row[0], "sourceChatId": row[1], "kind": row[2],
                    "payload": payload, "createdAt": row[4]})
    return out


def matches(watch: dict, source_chat_id: str) -> bool:
    if not watch or watch.get("status") != "active" or not source_chat_id:
        return False
    if source_chat_id == watch.get("orchestratorChatId"):
        return False
    if str(watch.get("mode") or "all") == "all":
        return True
    for target in watch.get("targets") or []:
        target = str(target)
        if target == source_chat_id:
            return True
        if target.startswith("group:") and source_chat_id.startswith(f"gp-{target.split(':', 1)[1]}-"):
            return True
    return False
