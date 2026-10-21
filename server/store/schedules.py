"""Orchestrator schedules — time-triggered wakes, no manual UI."""
from __future__ import annotations

import threading
import uuid
from datetime import datetime, timedelta, timezone

from server.store.chat_store import _get_db

_LOCK = threading.RLock()
_READY = False


def _now() -> datetime:
    return datetime.now(timezone.utc)


def parse_time(value: str | None) -> datetime | None:
    if not value:
        return None
    try:
        text = str(value).strip().lower().replace(".", ":")
        import re as _re
        clock = _re.fullmatch(r"(\d{1,2})(?::(\d{2}))?\s*(am|pm)?", text)
        if clock:
            hour = int(clock.group(1))
            minute = int(clock.group(2) or 0)
            suffix = clock.group(3) or ""
            if suffix == "pm" and hour < 12:
                hour += 12
            elif suffix == "am" and hour == 12:
                hour = 0
            if 0 <= hour <= 23 and 0 <= minute <= 59:
                local = datetime.now().astimezone().replace(hour=hour, minute=minute, second=0, microsecond=0)
                if local <= datetime.now().astimezone():
                    local += timedelta(days=1)
                return local.astimezone(timezone.utc)
        parsed = datetime.fromisoformat(text.replace("Z", "+00:00"))
        if parsed.tzinfo is None:
            parsed = parsed.astimezone()
        return parsed.astimezone(timezone.utc)
    except Exception:
        return None


def _next_after(current: datetime, recurrence: str) -> datetime | None:
    rec = (recurrence or "none").strip().lower()
    if rec in ("", "none", "once"):
        return None
    if rec == "daily":
        return current + timedelta(days=1)
    if rec == "weekly":
        return current + timedelta(days=7)
    if rec == "monthly":
        month = current.month % 12 + 1
        year = current.year + (1 if current.month == 12 else 0)
        return current.replace(year=year, month=month, day=min(current.day, 28))
    if rec.startswith("interval-"):
        try:
            minutes = int(rec.split("-", 1)[1])
        except Exception:
            return None
        return current + timedelta(minutes=minutes) if minutes > 0 else None
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
            CREATE TABLE IF NOT EXISTS orchestrator_schedules (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                prompt TEXT NOT NULL,
                workspace_path TEXT DEFAULT '',
                chat_id TEXT NOT NULL,
                run_at TEXT NOT NULL,
                next_run_at TEXT NOT NULL,
                recurrence TEXT DEFAULT 'none',
                status TEXT DEFAULT 'scheduled',
                last_run_at TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
        """)
        db.execute("CREATE INDEX IF NOT EXISTS idx_orch_schedules_due ON orchestrator_schedules(status, next_run_at)")
        try:
            db.execute("UPDATE chats SET tasks_json=NULL WHERE tasks_json IS NOT NULL")
        except Exception:
            pass
        _READY = True

def _row(row) -> dict:
    return {
        "id": row[0], "name": row[1], "prompt": row[2],
        "workspacePath": row[3] or "", "chatId": row[4],
        "runAt": row[5], "nextRunAt": row[6], "recurrence": row[7],
        "status": row[8], "lastRunAt": row[9],
        "createdAt": row[10], "updatedAt": row[11],
    }


def create_schedule(name: str, prompt: str, workspace_path: str, chat_id: str,
                    run_at: str, recurrence: str = "none") -> dict:
    _init()
    due = parse_time(run_at)
    if due is None:
        raise ValueError("at must be an ISO timestamp or HH:MM")
    now = _now().isoformat()
    schedule_id = f"sched-{uuid.uuid4().hex[:10]}"
    rec = (recurrence or "none").strip().lower()
    with _LOCK:
        _get_db().execute(
            "INSERT INTO orchestrator_schedules (id, name, prompt, workspace_path, chat_id,"
            " run_at, next_run_at, recurrence, status, created_at, updated_at)"
            " VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            (schedule_id, str(name or "Scheduled wake"), str(prompt or name or "Scheduled wake"),
             workspace_path or "", chat_id, due.isoformat(), due.isoformat(), rec,
             "scheduled", now, now),
        )
    return get_schedule(schedule_id) or {}


def get_schedule(schedule_id: str) -> dict | None:
    _init()
    with _LOCK:
        row = _get_db().execute(
            "SELECT id, name, prompt, workspace_path, chat_id, run_at, next_run_at,"
            " recurrence, status, last_run_at, created_at, updated_at"
            " FROM orchestrator_schedules WHERE id=?", (schedule_id,),
        ).fetchone()
    return _row(row) if row else None


def list_schedules(chat_id: str = "", include_done: bool = False) -> list[dict]:
    _init()
    sql = ("SELECT id, name, prompt, workspace_path, chat_id, run_at, next_run_at,"
           " recurrence, status, last_run_at, created_at, updated_at"
           " FROM orchestrator_schedules")
    where = []
    params: list = []
    if chat_id:
        where.append("chat_id=?")
        params.append(chat_id)
    if not include_done:
        where.append("status IN ('scheduled','queued','running')")
    if where:
        sql += " WHERE " + " AND ".join(where)
    sql += " ORDER BY next_run_at ASC"
    with _LOCK:
        rows = _get_db().execute(sql, params).fetchall()
    return [_row(r) for r in rows]


def due_schedules(now: str | None = None, limit: int = 20) -> list[dict]:
    _init()
    moment = now or _now().isoformat()
    with _LOCK:
        rows = _get_db().execute(
            "SELECT id, name, prompt, workspace_path, chat_id, run_at, next_run_at,"
            " recurrence, status, last_run_at, created_at, updated_at"
            " FROM orchestrator_schedules WHERE status='scheduled' AND next_run_at<=?"
            " ORDER BY next_run_at ASC LIMIT ?", (moment, limit),
        ).fetchall()
    return [_row(r) for r in rows]


def update_schedule(schedule_id: str, **changes) -> dict | None:
    _init()
    allowed = {"name", "prompt", "run_at", "next_run_at", "recurrence", "status", "last_run_at"}
    fields = []
    values = []
    for key, value in changes.items():
        if key in allowed:
            fields.append(f"{key}=?")
            values.append(value)
    if not fields:
        return get_schedule(schedule_id)
    fields.append("updated_at=?")
    values.append(_now().isoformat())
    values.append(schedule_id)
    with _LOCK:
        _get_db().execute(f"UPDATE orchestrator_schedules SET {', '.join(fields)} WHERE id=?", values)
    return get_schedule(schedule_id)


def cancel_schedule(schedule_id: str) -> bool:
    return update_schedule(schedule_id, status="cancelled") is not None


def mark_enqueued(schedule_id: str) -> dict | None:
    return update_schedule(schedule_id, status="queued")


def mark_after_run(schedule_id: str, fired_at: str | None = None) -> dict | None:
    schedule = get_schedule(schedule_id)
    if not schedule:
        return None
    fired = parse_time(fired_at) or _now()
    nxt = _next_after(fired, schedule["recurrence"])
    if nxt is None:
        return update_schedule(schedule_id, status="completed", last_run_at=fired.isoformat())
    return update_schedule(
        schedule_id, status="scheduled", last_run_at=fired.isoformat(),
        next_run_at=nxt.isoformat(),
    )
