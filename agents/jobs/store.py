"""agents.jobs — durable job state for delegation (Phase 0).

See ``docs/group-projects/07-phase-0-implementation.md`` sections 3-4.

What's here
-----------
- ``jobs.db``: a harness-level SQLite file at ``<harness>/.deep_agents/jobs.db``.
- :class:`JobStore`: ``upsert`` / ``get`` / ``list_for_parent`` /
  ``claim_running`` / ``mark``.
- Boot reconciliation: a job found ``running`` when the store first loads is
  marked ``failed(process_restart)``, because the process running it is gone.
- ``project_id`` / ``item_id`` columns exist and are unused (NULL). Phase 1
  sets them for group-project jobs, so no ``ALTER TABLE`` is needed then.

What's deliberately NOT here
----------------------------
- **No scheduler and no queue.** ``agents/delegation.py``'s slot semaphore
  still decides who runs. This module only *records*.
- **No event log.** Phase 0 keeps a plain table. Phase 1 introduces ``events``
  as the source of truth for group projects and makes job rows a projection of
  it (``01-concept-model.md`` section 7).
- **Not the ledger.** ``.agent-memory/orchestrator_jobs.jsonl`` stays a capped
  *history* and is untouched.
- **No cross-process coordination.** One harness process owns the file, which
  matches the existing single-writer assumption everywhere else.
"""
from __future__ import annotations

import json
import os
import sqlite3
import threading
import time
from collections.abc import Iterable
from typing import Any

#: Terminal statuses — a job in one of these is finished forever.
TERMINAL = frozenset({"done", "failed", "cancelled"})

_SCHEMA = """
CREATE TABLE IF NOT EXISTS jobs (
  id             TEXT PRIMARY KEY,
  parent_chat_id TEXT,
  project_id     TEXT,
  item_id        TEXT,
  agent          TEXT NOT NULL DEFAULT '',
  chat_id        TEXT,
  title          TEXT,
  status         TEXT NOT NULL,
  attempts       INTEGER DEFAULT 0,
  lease_owner    TEXT,
  lease_expires  REAL,
  workdir        TEXT,
  usage_json     TEXT,
  result         TEXT,
  error          TEXT,
  artifact       TEXT,
  created_at     REAL NOT NULL,
  updated_at     REAL NOT NULL,
  finished_at    REAL,
  entry_json     TEXT
);
CREATE INDEX IF NOT EXISTS ix_jobs_parent  ON jobs(parent_chat_id, status);
CREATE INDEX IF NOT EXISTS ix_jobs_project ON jobs(project_id, status);
"""

#: Columns above are the QUERYABLE projection (status, parent, project, ...).
#: ``entry_json`` carries the FULL delegation entry so a rehydrated row is
#: indistinguishable from a live one. Without it, read paths that expect the
#: live shape break — e.g. ``delegation.status_table`` needs ``agent_display``
#: and ``undelivered_count`` needs ``delivered``, neither of which is a column.
#: Columns are authoritative for what ``mark()`` changes; the JSON is merged
#: so both stay consistent.


def db_path() -> str:
    """``<harness>/.deep_agents/jobs.db``.

    Resolved from ``HARNESS_ROOT`` when set, else from this file's location
    (``agents/jobs/store.py`` -> three dirnames up). Mirrors
    ``delegation.py:_ledger_path`` so both land in the same harness root.
    """
    harness = os.environ.get("HARNESS_ROOT") or os.path.dirname(
        os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    )
    return os.path.join(harness, ".deep_agents", "jobs.db")


class JobStore:
    """Durable ``jobs`` table. The supervisor of writes is the caller."""

    def __init__(self, path: str | None = None) -> None:
        self.path = path or db_path()
        self._lock = threading.RLock()
        parent = os.path.dirname(self.path)
        if parent:
            os.makedirs(parent, exist_ok=True)
        self._db = sqlite3.connect(self.path, check_same_thread=False)
        self._db.row_factory = sqlite3.Row
        # WAL matters here: the scheduler thread, the delegation fan-out
        # threads and the watchdog all touch this file concurrently.
        self._db.execute("PRAGMA journal_mode=WAL")
        self._db.execute("PRAGMA synchronous=NORMAL")
        with self._lock:
            self._db.executescript(_SCHEMA)
            self._db.commit()
        self.reconcile_on_load()

    # ── reconciliation ──────────────────────────────────────────────────

    def reconcile_on_load(self) -> int:
        """Fail jobs left ``running`` by a previous process (07 section 4.4).

        Deliberately the least clever option: there is no supervisor and no
        lease machinery in Phase 0, so pretending to resume would be a second,
        worse scheduler. ``queued`` jobs are left alone — still visible, still
        cancellable — because the parent chat that requested them is gone.
        """
        with self._lock:
            cur = self._db.execute(
                "UPDATE jobs SET status='failed', error=?, updated_at=?, finished_at=? "
                "WHERE status='running'",
                ("process_restart", time.time(), time.time()),
            )
            self._db.commit()
            return cur.rowcount or 0

    # ── writes ──────────────────────────────────────────────────────────

    def upsert(self, entry: dict) -> None:
        """Insert or fully replace one job row from a delegation entry dict."""
        now = time.time()
        usage = entry.get("usage")
        row = (
            str(entry.get("id") or ""),
            str(entry.get("parent") or ""),
            entry.get("project_id"),
            entry.get("item_id"),
            str(entry.get("agent_key") or entry.get("agent") or ""),
            str(entry.get("chat_id") or ""),
            str(entry.get("title") or ""),
            str(entry.get("status") or "queued"),
            int(entry.get("attempts") or 0),
            entry.get("lease_owner"),
            entry.get("lease_expires"),
            str(entry.get("workspace") or entry.get("workdir") or ""),
            json.dumps(usage) if isinstance(usage, (dict, list)) else (usage or ""),
            str(entry.get("result") or ""),
            str(entry.get("error") or ""),
            str(entry.get("artifact") or ""),
            float(entry.get("created_at") or now),
            now,
            float(entry.get("finished_at") or 0) or None,
            _encode_entry(entry),
        )
        with self._lock:
            self._db.execute(
                "INSERT OR REPLACE INTO jobs ("
                " id, parent_chat_id, project_id, item_id, agent, chat_id, title,"
                " status, attempts, lease_owner, lease_expires, workdir,"
                " usage_json, result, error, artifact, created_at, updated_at,"
                " finished_at, entry_json)"
                " VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                row,
            )
            self._db.commit()

    def mark(self, job_id: str, status: str, **fields: Any) -> None:
        """Set a status and any extra columns in one statement.

        The single persistence entry point for status transitions, so a
        transition either lands with its payload or not at all. Also merges the
        change into ``entry_json`` so a rehydrated row keeps the full shape.
        """
        now = time.time()
        sets = ["status = ?", "updated_at = ?"]
        params: list[Any] = [status, now]
        for key in ("result", "error", "artifact", "workdir", "chat_id",
                    "lease_owner", "lease_expires", "attempts", "title",
                    "parent_chat_id", "agent"):
            if key in fields:
                sets.append(f"{key} = ?")
                params.append(fields[key])
        if "usage" in fields and isinstance(fields["usage"], (dict, list)):
            sets.append("usage_json = ?")
            params.append(json.dumps(fields["usage"]))
        finished_at = None
        if status in TERMINAL:
            finished_at = now
            sets.append("finished_at = ?")
            params.append(finished_at)

        with self._lock:
            entry = self._read_entry_json(job_id)
            if entry is not None:
                entry.update(fields)
                entry["status"] = status
                entry["updated_at"] = now
                if finished_at is not None:
                    entry["finished_at"] = finished_at
                sets.append("entry_json = ?")
                params.append(_encode_entry(entry))
            params.append(job_id)
            self._db.execute(f"UPDATE jobs SET {', '.join(sets)} WHERE id = ?", params)
            self._db.commit()

    def _read_entry_json(self, job_id: str) -> dict | None:
        row = self._db.execute(
            "SELECT entry_json FROM jobs WHERE id = ?", (job_id,)
        ).fetchone()
        if not row or not row["entry_json"]:
            return None
        try:
            data = json.loads(row["entry_json"])
        except (TypeError, ValueError):
            return None
        return data if isinstance(data, dict) else None

    def delete(self, job_id: str) -> None:
        """Remove one row. Used by test helpers, not by the run path."""
        with self._lock:
            self._db.execute("DELETE FROM jobs WHERE id = ?", (job_id,))
            self._db.commit()

    def claim_running(self, parent_chat_id: str, limit: int) -> list[dict]:
        """Atomically move up to ``limit`` queued jobs for a parent to running."""
        with self._lock:
            rows = self._db.execute(
                "SELECT id FROM jobs WHERE parent_chat_id = ? AND status = 'queued' "
                "ORDER BY created_at LIMIT ?",
                (parent_chat_id, int(limit)),
            ).fetchall()
            ids = [r["id"] for r in rows]
            for job_id in ids:
                self._db.execute(
                    "UPDATE jobs SET status='running', updated_at=? WHERE id=?",
                    (time.time(), job_id),
                )
            self._db.commit()
        return [row for row in (self.get(job_id) for job_id in ids) if row]

    # ── reads ───────────────────────────────────────────────────────────

    def get(self, job_id: str) -> dict | None:
        with self._lock:
            row = self._db.execute(
                "SELECT * FROM jobs WHERE id = ?", (job_id,)
            ).fetchone()
        return _to_dict(row) if row else None

    def list_for_parent(self, parent_chat_id: str,
                        status: str | None = None) -> list[dict]:
        sql = "SELECT * FROM jobs WHERE parent_chat_id = ?"
        params: list[Any] = [parent_chat_id]
        if status:
            sql += " AND status = ?"
            params.append(status)
        sql += " ORDER BY created_at"
        with self._lock:
            rows = self._db.execute(sql, params).fetchall()
        return [_to_dict(r) for r in rows]

    def all_ids(self) -> Iterable[str]:
        with self._lock:
            rows = self._db.execute("SELECT id FROM jobs").fetchall()
        return [r["id"] for r in rows]

    def close(self) -> None:
        """Release the connection. Safe to call more than once."""
        with self._lock:
            try:
                self._db.close()
            except sqlite3.Error:
                pass


def _encode_entry(entry: dict) -> str:
    """JSON-encode a delegation entry, coercing anything exotic to str."""
    try:
        return json.dumps(entry, default=str)
    except (TypeError, ValueError):
        return "{}"


def _to_dict(row: sqlite3.Row) -> dict:
    """Rebuild the delegation entry shape from a row.

    The full entry comes from ``entry_json`` (fidelity); the explicit columns
    are laid over the top because ``mark()`` keeps them authoritative for the
    fields it manages. A column that is NULL never clobbers a value the JSON
    carries — e.g. ``finished_at`` is set by ``mark()`` on both sides, while
    ``delivered`` and ``agent_display`` exist only in the payload.
    """
    columns = dict(row)
    raw_entry = columns.pop("entry_json", None)
    data: dict = {}
    if raw_entry:
        try:
            decoded = json.loads(raw_entry)
            if isinstance(decoded, dict):
                data = decoded
        except (TypeError, ValueError):
            data = {}
    for key, value in columns.items():
        if value is not None or key not in data:
            data[key] = value
    # Aliases matching the live delegation entry keys.
    data.setdefault("parent", data.get("parent_chat_id") or "")
    data.setdefault("workspace", data.get("workdir") or "")
    raw_usage = columns.get("usage_json")
    if raw_usage and not isinstance(data.get("usage"), dict):
        try:
            data["usage"] = json.loads(raw_usage)
        except (TypeError, ValueError):
            data["usage"] = {}
    if not isinstance(data.get("usage"), dict):
        data["usage"] = {}
    return data


# ── singleton ───────────────────────────────────────────────────────────

_STORE: JobStore | None = None
_STORE_LOCK = threading.Lock()


def get_store() -> JobStore:
    """Process-wide store. First call opens the file and reconciles it."""
    global _STORE
    if _STORE is None:
        with _STORE_LOCK:
            if _STORE is None:
                _STORE = JobStore()
    return _STORE


def reset_store_for_tests() -> None:
    """Drop the singleton (tests only) so a fresh path can be used."""
    global _STORE
    with _STORE_LOCK:
        if _STORE is not None:
            _STORE.close()
        _STORE = None
