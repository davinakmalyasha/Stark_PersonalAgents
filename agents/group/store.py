"""agents.group.store — the event log and its projections (Phase 1).

``docs/group-projects/01-concept-model.md`` section 7 and
``02-runtime.md`` section 3.

The rule this module exists to enforce:

    **events are the only truth; every other table is a projection of them.**

So there is exactly one place that knows how an event changes state —
:func:`_reduce` — and two callers:

- :meth:`ProjectStore.append` writes one event and applies it incrementally,
  in a single transaction.
- :meth:`ProjectStore.rebuild` replays the whole log into a fresh state and
  replaces every projection row.

Because both go through the same pure reducer, ``rebuild()`` is provably
equivalent to the incremental path — which is what ``DM-16`` asserts and what
makes every future change to the projection model free.

What is deliberately NOT here
-----------------------------
- **No scheduler.** ``supervisor.py`` decides when things happen; this module
  only records what did.
- **No LLM.** Nothing in this file calls a model.
- **No cross-process coordination.** One process owns the file, matching the
  single-writer rule in ``02`` section 4.
- **No in-memory state cache.** Every mutation re-reads the projection. The
  volume is dozens of rows, and a cache is a second source of truth waiting to
  go stale.
"""
from __future__ import annotations

import json
import os
import sqlite3
import threading
import time
from typing import Any

from . import events as ev

_SCHEMA = """
-- ── THE TRUTH ────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS events (
  seq          INTEGER PRIMARY KEY AUTOINCREMENT,
  project_id   TEXT NOT NULL,
  ts           REAL NOT NULL,
  kind         TEXT NOT NULL,
  actor        TEXT NOT NULL,
  milestone_id TEXT,
  item_id      TEXT,
  data         TEXT
);
CREATE INDEX IF NOT EXISTS ix_events_project ON events(project_id, seq);

-- ── THE PROJECTIONS ──────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS projects (
  id              TEXT PRIMARY KEY,
  owner           TEXT,
  status          TEXT NOT NULL DEFAULT 'created',
  halt_reason     TEXT,
  warned          INTEGER DEFAULT 0,
  goal            TEXT,
  budget_usd      REAL,
  spent_usd       REAL DEFAULT 0,
  budget_tokens   INTEGER,
  spent_tokens    INTEGER DEFAULT 0,
  deadline        REAL,
  completion_json TEXT,
  updated_at      REAL
);
CREATE TABLE IF NOT EXISTS milestones (
  id              TEXT PRIMARY KEY,
  project_id      TEXT NOT NULL,
  title           TEXT,
  detail          TEXT,
  status          TEXT NOT NULL DEFAULT 'todo',
  seq_no          INTEGER DEFAULT 0,
  depends_on_json TEXT,
  acceptance_json TEXT,
  updated_at      REAL
);
CREATE INDEX IF NOT EXISTS ix_milestones_project ON milestones(project_id, seq_no);
CREATE TABLE IF NOT EXISTS items (
  id                 TEXT PRIMARY KEY,
  project_id         TEXT NOT NULL,
  milestone_id       TEXT,
  title              TEXT,
  detail             TEXT,
  status             TEXT NOT NULL DEFAULT 'todo',
  attempts           INTEGER DEFAULT 0,
  max_attempts       INTEGER DEFAULT 3,
  owner_member       TEXT,
  workdir            TEXT,
  lease_owner        TEXT,
  lease_expires      REAL,
  depends_on_json    TEXT,
  acceptance_json    TEXT,
  tags_json          TEXT,
  cost_tokens        INTEGER DEFAULT 0,
  cost_usd           REAL DEFAULT 0,
  result             TEXT,
  error              TEXT,
  blocked_reason     TEXT,
  invalidated_reason TEXT,
  updated_at         REAL
);
CREATE INDEX IF NOT EXISTS ix_items_project ON items(project_id, status);
CREATE INDEX IF NOT EXISTS ix_items_milestone ON items(milestone_id, status);
CREATE TABLE IF NOT EXISTS ops (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  project_id   TEXT NOT NULL,
  kind         TEXT NOT NULL,
  payload_json TEXT,
  actor        TEXT,
  state        TEXT NOT NULL DEFAULT 'pending',
  error        TEXT,
  created_at   REAL,
  applied_at   REAL
);
CREATE INDEX IF NOT EXISTS ix_ops_pending ON ops(project_id, state);
CREATE TABLE IF NOT EXISTS projection_checkpoint (
  project_id TEXT PRIMARY KEY,
  last_seq   INTEGER NOT NULL DEFAULT 0
);
"""


class RebuildMismatch(Exception):
    """Raised when rebuild() cannot reproduce the incremental projection."""


# ════════════════════════════════════════════════════════════════════════════
#  The reducer — the ONLY place that knows how an event changes state
# ════════════════════════════════════════════════════════════════════════════


class State:
    """In-memory projection state. Rebuilt from events, never persisted."""

    def __init__(self, project_id: str) -> None:
        self.project_id = project_id
        self.project: dict = {}
        self.milestones: dict[str, dict] = {}
        self.items: dict[str, dict] = {}
        self.ops: dict[int, dict] = {}


def _new_project(project_id: str, data: dict) -> dict:
    return {
        "id": project_id,
        "owner": data.get("owner") or "localhost",
        "status": "created",
        "halt_reason": None,
        "warned": 0,
        "goal": data.get("goal") or "",
        "budget_usd": data.get("budget_usd"),
        "spent_usd": 0.0,
        "budget_tokens": data.get("budget_tokens"),
        "spent_tokens": 0,
        "deadline": data.get("deadline"),
        "completion_json": json.dumps(data.get("completion") or
                                      {"kind": "all_milestones_done"}),
        "updated_at": time.time(),
    }


def _new_milestone(project_id: str, spec: dict, seq_no: int) -> dict:
    return {
        "id": str(spec.get("id") or ""),
        "project_id": project_id,
        "title": spec.get("title") or "",
        "detail": spec.get("detail") or "",
        "status": "todo",
        "seq_no": int(spec.get("seq_no", seq_no)),
        "depends_on_json": json.dumps(list(spec.get("depends_on") or [])),
        "acceptance_json": json.dumps(spec.get("acceptance") or []),
        "updated_at": time.time(),
    }


def _new_item(project_id: str, milestone_id: str, spec: dict) -> dict:
    return {
        "id": str(spec.get("id") or ""),
        "project_id": project_id,
        "milestone_id": str(spec.get("milestone_id") or milestone_id or ""),
        "title": spec.get("title") or "",
        "detail": spec.get("detail") or "",
        "status": "todo",
        "attempts": 0,
        "max_attempts": int(spec.get("max_attempts", 3)),
        "owner_member": spec.get("owner_member"),
        "workdir": spec.get("workdir") or "",
        "lease_owner": None,
        "lease_expires": None,
        "depends_on_json": json.dumps(list(spec.get("depends_on") or [])),
        "acceptance_json": json.dumps(spec.get("acceptance") or []),
        "tags_json": json.dumps(list(spec.get("tags") or [])),
        "cost_tokens": 0,
        "cost_usd": 0.0,
        "result": "",
        "error": "",
        "blocked_reason": None,
        "invalidated_reason": None,
        "updated_at": time.time(),
    }


def _reduce(state: State, event: dict) -> set[tuple[str, str]]:
    """Apply one event to ``state``. Returns touched ``(table, id)`` keys.

    Pure with respect to I/O — the only side effect is mutating ``state``.
    Both the incremental write path and ``rebuild()`` call exactly this.
    """
    kind = event.get("kind") or ""
    data = event.get("data") or {}
    item_id = event.get("item_id")
    milestone_id = event.get("milestone_id")
    touched: set[tuple[str, str]] = set()

    if kind not in ev.REDUCED_KINDS:
        raise ev.UnknownEventKind(kind, event.get("seq"))

    def touch_item() -> dict | None:
        entry = state.items.get(item_id or "")
        if entry is not None:
            entry["updated_at"] = time.time()
            touched.add(("items", entry["id"]))
        return entry

    # ── project ──────────────────────────────────────────────────────────
    if kind == ev.PROJECT_CREATED:
        state.project = _new_project(state.project_id, data)
        touched.add(("projects", state.project_id))
        for index, spec in enumerate(data.get("milestones") or []):
            milestone = _new_milestone(state.project_id, spec, index + 1)
            if not milestone["id"]:
                continue
            state.milestones[milestone["id"]] = milestone
            touched.add(("milestones", milestone["id"]))
            for item_spec in spec.get("items") or []:
                item = _new_item(state.project_id, milestone["id"], item_spec)
                if item["id"]:
                    state.items[item["id"]] = item
                    touched.add(("items", item["id"]))
        # Items declared flat (no milestone) land in the implicit one.
        for item_spec in data.get("items") or []:
            milestone_id_for_item = item_spec.get("milestone_id") or "all work"
            if milestone_id_for_item not in state.milestones:
                implicit = _new_milestone(
                    state.project_id,
                    {"id": milestone_id_for_item, "title": "all work"},
                    len(state.milestones) + 1,
                )
                state.milestones[implicit["id"]] = implicit
                touched.add(("milestones", implicit["id"]))
            item = _new_item(state.project_id, milestone_id_for_item, item_spec)
            if item["id"]:
                state.items[item["id"]] = item
                touched.add(("items", item["id"]))
        return touched

    if kind in (ev.PROJECT_RESUMED,):
        state.project["status"] = "running"
        state.project["halt_reason"] = None
        state.project["updated_at"] = time.time()
        touched.add(("projects", state.project_id))
        return touched

    if kind == ev.PROJECT_PAUSED:
        state.project["status"] = "paused"
        state.project["updated_at"] = time.time()
        touched.add(("projects", state.project_id))
        return touched

    if kind == ev.PROJECT_STALLED:
        state.project["status"] = "stalled"
        state.project["halt_reason"] = data.get("reason") or "stalled"
        state.project["updated_at"] = time.time()
        touched.add(("projects", state.project_id))
        return touched

    if kind == ev.PROJECT_CLOSED:
        state.project["status"] = data.get("status") or "done"
        state.project["updated_at"] = time.time()
        touched.add(("projects", state.project_id))
        return touched

    if kind == ev.BUDGET_WARN:
        state.project["warned"] = 1
        state.project["updated_at"] = time.time()
        touched.add(("projects", state.project_id))
        return touched

    if kind in (ev.BUDGET_STOP, ev.DEADLINE_REACHED):
        state.project["halt_reason"] = ("budget" if kind == ev.BUDGET_STOP
                                        else "deadline")
        state.project["updated_at"] = time.time()
        touched.add(("projects", state.project_id))
        return touched

    if kind == ev.PLAN_REVISED or kind == ev.IDEA_PROPOSED:
        # Signals, not state. The orchestrator wakes on them; the board does
        # not change shape.
        return touched

    # ── milestones ───────────────────────────────────────────────────────
    if kind == ev.MILESTONE_CREATED:
        milestone = _new_milestone(
            state.project_id, {"id": milestone_id, **data},
            len(state.milestones) + 1,
        )
        state.milestones[milestone["id"]] = milestone
        touched.add(("milestones", milestone["id"]))
        return touched

    if kind in (ev.MILESTONE_READY, ev.MILESTONE_DONE, ev.MILESTONE_FAILED):
        milestone = state.milestones.get(milestone_id or "")
        if milestone is not None:
            milestone["status"] = {
                ev.MILESTONE_READY: "ready",
                ev.MILESTONE_DONE: "done",
                ev.MILESTONE_FAILED: "failed",
            }[kind]
            milestone["updated_at"] = time.time()
            touched.add(("milestones", milestone["id"]))
        return touched

    # ── items ────────────────────────────────────────────────────────────
    if kind == ev.ITEM_CREATED:
        item = _new_item(state.project_id, milestone_id or "", {"id": item_id, **data})
        if item["id"]:
            state.items[item["id"]] = item
            touched.add(("items", item["id"]))
        return touched

    if kind == ev.ITEM_READY:
        entry = touch_item()
        if entry is not None:
            entry["status"] = "ready"
            entry["blocked_reason"] = None
        return touched

    if kind == ev.ITEM_CLAIMED:
        entry = touch_item()
        if entry is not None:
            entry["status"] = "doing"
            entry["attempts"] = int(entry.get("attempts") or 0) + 1
            entry["lease_owner"] = data.get("lease_owner")
            entry["lease_expires"] = data.get("lease_expires")
        return touched

    if kind == ev.ITEM_RELEASED:
        entry = touch_item()
        if entry is not None:
            entry["lease_owner"] = None
            entry["lease_expires"] = None
        return touched

    if kind == ev.ITEM_PROGRESS:
        # A finished attempt, recorded WITHOUT a status change. Phase 6 writes
        # this; phase 7 decides the transition from the acceptance criteria.
        # Keeping the two apart is what lets a run be harvested and judged
        # separately, and what makes a retry visible on the board.
        entry = touch_item()
        if entry is not None:
            if data.get("result"):
                entry["result"] = str(data["result"])
            if data.get("error"):
                entry["error"] = str(data["error"])
            entry["cost_tokens"] = int(entry.get("cost_tokens") or 0) + int(data.get("cost_tokens") or 0)
            entry["cost_usd"] = float(entry.get("cost_usd") or 0) + float(data.get("cost_usd") or 0)
            entry["lease_owner"] = None
            entry["lease_expires"] = None
            if state.project:
                state.project["spent_tokens"] = (
                    int(state.project.get("spent_tokens") or 0) + int(data.get("cost_tokens") or 0))
                state.project["spent_usd"] = (
                    float(state.project.get("spent_usd") or 0) + float(data.get("cost_usd") or 0))
                state.project["updated_at"] = time.time()
                touched.add(("projects", state.project_id))
        return touched

    if kind == ev.ITEM_DONE:
        entry = touch_item()
        if entry is not None:
            entry["status"] = "done"
            entry["lease_owner"] = None
            entry["lease_expires"] = None
            spent_tokens = int(data.get("cost_tokens") or 0)
            spent_usd = float(data.get("cost_usd") or 0)
            entry["cost_tokens"] = int(entry.get("cost_tokens") or 0) + spent_tokens
            entry["cost_usd"] = float(entry.get("cost_usd") or 0) + spent_usd
            # The project total is accumulated here rather than summed on read:
            # the budget gate is checked on every tick and must not walk the
            # item table each time. Phase 2 feeds real usage into this payload.
            if state.project:
                state.project["spent_tokens"] = (
                    int(state.project.get("spent_tokens") or 0) + spent_tokens)
                state.project["spent_usd"] = (
                    float(state.project.get("spent_usd") or 0) + spent_usd)
                state.project["updated_at"] = time.time()
                touched.add(("projects", state.project_id))
        return touched

    if kind == ev.ITEM_FAILED:
        entry = touch_item()
        if entry is not None:
            entry["status"] = "failed"
            entry["lease_owner"] = None
            entry["lease_expires"] = None
            if data.get("reason"):
                entry["blocked_reason"] = data["reason"]
        return touched

    if kind == ev.ITEM_BLOCKED:
        entry = touch_item()
        if entry is not None:
            entry["status"] = "blocked"
            entry["blocked_reason"] = data.get("reason") or "blocked"
            entry["lease_owner"] = None
            entry["lease_expires"] = None
        return touched

    if kind == ev.ITEM_CANCELLED:
        entry = touch_item()
        if entry is not None:
            entry["status"] = "cancelled"
            entry["lease_owner"] = None
            entry["lease_expires"] = None
        return touched

    if kind == ev.ITEM_INVALIDATED:
        entry = touch_item()
        if entry is not None:
            # NOTE: attempts is deliberately NOT incremented. Invalidation is
            # a plan error, not an execution error (01 section 4.4, DM-15).
            entry["status"] = "invalidated"
            entry["invalidated_reason"] = data.get("reason") or "plan revised"
            entry["lease_owner"] = None
            entry["lease_expires"] = None
        return touched

    if kind == ev.APPROVAL_REQUESTED:
        # Park the item WITHOUT changing its status. `doing` is correct: the
        # work exists and is still ours, we are just waiting on a human. The
        # reason is what tells the stall detector this is a legitimate wait.
        entry = touch_item()
        if entry is not None:
            entry["blocked_reason"] = ev.AWAITING_APPROVAL
        return touched

    if kind == ev.APPROVAL_GRANTED:
        entry = touch_item()
        if entry is not None:
            entry["blocked_reason"] = None
        return touched

    if kind == ev.APPROVAL_REJECTED:
        # AU-10: a rejection blocks the item and asks the orchestrator to
        # re-plan. It is not a failure of the member's work.
        entry = touch_item()
        if entry is not None:
            entry["status"] = "blocked"
            entry["blocked_reason"] = "approval_rejected"
            entry["lease_owner"] = None
            entry["lease_expires"] = None
        return touched

    if kind == ev.LEASE_EXPIRED:
        # Bookkeeping signal; item.blocked carries the state change.
        return touched

    # ── operations ───────────────────────────────────────────────────────
    if kind == ev.OP_SUBMITTED:
        op_id = data.get("op_id")
        if op_id is not None:
            state.ops[int(op_id)] = {
                "id": int(op_id),
                "project_id": state.project_id,
                "kind": data.get("op_kind") or "",
                "payload_json": json.dumps(data.get("payload") or {}),
                "actor": event.get("actor") or "",
                "state": "pending",
                "error": None,
                "created_at": event.get("ts"),
                "applied_at": None,
            }
            touched.add(("ops", str(int(op_id))))
        return touched

    if kind in (ev.OP_APPLIED, ev.OP_REJECTED):
        op_id = data.get("op_id")
        if op_id is not None:
            op = state.ops.get(int(op_id))
            if op is not None:
                op["state"] = "applied" if kind == ev.OP_APPLIED else "rejected"
                op["error"] = data.get("error")
                op["applied_at"] = event.get("ts")
                touched.add(("ops", str(int(op_id))))
        return touched

    return touched  # pragma: no cover — PHASE_1_REDUCED is exhaustive above


# ════════════════════════════════════════════════════════════════════════════
#  The store
# ════════════════════════════════════════════════════════════════════════════


def db_path(project_dir: str) -> str:
    """``<project_dir>/.gp/state.db`` — the project folder owns its state."""
    return os.path.join(project_dir, ".gp", "state.db")


class ProjectStore:
    """One project's events plus their projections."""

    def __init__(self, path: str) -> None:
        self.path = path
        self._lock = threading.RLock()
        parent = os.path.dirname(path)
        if parent:
            os.makedirs(parent, exist_ok=True)
        self._db = sqlite3.connect(path, check_same_thread=False)
        self._db.row_factory = sqlite3.Row
        self._db.execute("PRAGMA journal_mode=WAL")
        self._db.execute("PRAGMA synchronous=NORMAL")
        with self._lock:
            self._db.executescript(_SCHEMA)
            self._migrate()
            self._db.commit()

    def _migrate(self) -> None:
        """Additive column migrations, matching ``chat_store.py:88-94``.

        Must stay additive: a project folder from an earlier version has to
        open, and its event log has to replay, without losing anything.
        """
        wanted = {"items": ("result", "error")}
        for table, columns in wanted.items():
            existing = {row["name"] for row in
                        self._db.execute(f"PRAGMA table_info({table})").fetchall()}
            for column in columns:
                if column not in existing:
                    self._db.execute(f"ALTER TABLE {table} ADD COLUMN {column} TEXT")

    # ── events: the truth ───────────────────────────────────────────────

    def append(self, project_id: str, kind: str, actor: str, *,
               milestone_id: str | None = None,
               item_id: str | None = None,
               data: dict | None = None) -> int:
        """Write one event AND apply it to the projections, in one transaction.

        Returns the new ``seq``. The projection is never behind the log, and
        never ahead of it, because both happen in the same commit.
        """
        if kind not in ev.ALL_KINDS:
            raise ev.UnknownEventKind(kind)
        with self._lock:
            try:
                seq = self._append_locked(project_id, kind, actor,
                                          milestone_id, item_id, data or {})
                self._db.commit()
            except Exception:
                self._db.rollback()
                raise
        return seq

    def submit_op(self, project_id: str, op_kind: str, payload: dict,
                  actor: str = "orchestrator") -> int:
        """Queue one operation. Returns the op id.

        The op row and its ``op.submitted`` event are written in ONE
        transaction, so an orphan op (a row with no event) is impossible —
        which is what keeps ``rebuild()`` able to reproduce the queue.

        The orchestrator never writes projections directly; it submits ops and
        the supervisor applies them (``02`` section 4).
        """
        with self._lock:
            try:
                cur = self._db.execute(
                    "INSERT INTO ops (project_id, kind, payload_json, actor,"
                    " state, created_at) VALUES (?,?,?,?,?,?)",
                    (project_id, op_kind,
                     json.dumps(payload or {}, default=str), actor,
                     "pending", time.time()),
                )
                op_id = int(cur.lastrowid)
                seq = self._append_locked(
                    project_id, ev.OP_SUBMITTED, actor,
                    None, None,
                    {"op_id": op_id, "op_kind": op_kind, "payload": payload or {}},
                )
                self._db.commit()
            except Exception:
                self._db.rollback()
                raise
        del seq
        return op_id

    def _append_locked(self, project_id: str, kind: str, actor: str,
                       milestone_id: str | None, item_id: str | None,
                       data: dict) -> int:
        """append() without its own transaction/lock — caller owns both."""
        cur = self._db.execute(
            "INSERT INTO events (project_id, ts, kind, actor,"
            " milestone_id, item_id, data) VALUES (?,?,?,?,?,?,?)",
            (project_id, time.time(), kind, actor, milestone_id, item_id,
             json.dumps(data or {}, default=str)),
        )
        seq = int(cur.lastrowid)
        state = self._load_state(project_id)
        _reduce(state, {
            "seq": seq, "kind": kind, "actor": actor,
            "milestone_id": milestone_id, "item_id": item_id,
            "ts": time.time(), "data": data or {},
        })
        self._persist_state(state, project_id)
        self._db.execute(
            "INSERT INTO projection_checkpoint (project_id, last_seq)"
            " VALUES (?, ?) ON CONFLICT(project_id)"
            " DO UPDATE SET last_seq = excluded.last_seq",
            (project_id, seq),
        )
        return seq

    def events(self, project_id: str, since: int = 0,
               limit: int | None = None) -> list[dict]:
        sql = ("SELECT seq, ts, kind, actor, milestone_id, item_id, data"
               " FROM events WHERE project_id = ? AND seq > ? ORDER BY seq")
        params: list[Any] = [project_id, int(since)]
        if limit:
            sql += " LIMIT ?"
            params.append(int(limit))
        with self._lock:
            rows = self._db.execute(sql, params).fetchall()
        out = []
        for row in rows:
            record = dict(row)
            try:
                record["data"] = json.loads(record["data"] or "{}")
            except (TypeError, ValueError):
                record["data"] = {}
            out.append(record)
        return out

    def max_seq(self, project_id: str) -> int:
        with self._lock:
            row = self._db.execute(
                "SELECT MAX(seq) AS m FROM events WHERE project_id = ?",
                (project_id,),
            ).fetchone()
        return int(row["m"] or 0) if row else 0

    def last_seq(self, project_id: str) -> int:
        with self._lock:
            row = self._db.execute(
                "SELECT last_seq FROM projection_checkpoint WHERE project_id = ?",
                (project_id,),
            ).fetchone()
        return int(row["last_seq"] or 0) if row else 0

    # ── projections ─────────────────────────────────────────────────────

    def project(self, project_id: str) -> dict | None:
        with self._lock:
            row = self._db.execute(
                "SELECT * FROM projects WHERE id = ?", (project_id,)
            ).fetchone()
        return dict(row) if row else None

    def milestones(self, project_id: str) -> list[dict]:
        with self._lock:
            rows = self._db.execute(
                "SELECT * FROM milestones WHERE project_id = ? ORDER BY seq_no, id",
                (project_id,),
            ).fetchall()
        return [_decode(row) for row in rows]

    def items(self, project_id: str, status: str | None = None,
              milestone_id: str | None = None) -> list[dict]:
        sql = "SELECT * FROM items WHERE project_id = ?"
        params: list[Any] = [project_id]
        if status:
            sql += " AND status = ?"
            params.append(status)
        if milestone_id:
            sql += " AND milestone_id = ?"
            params.append(milestone_id)
        sql += " ORDER BY milestone_id, id"
        with self._lock:
            rows = self._db.execute(sql, params).fetchall()
        return [_decode(row) for row in rows]

    def item(self, item_id: str) -> dict | None:
        with self._lock:
            row = self._db.execute(
                "SELECT * FROM items WHERE id = ?", (item_id,)
            ).fetchone()
        return _decode(row) if row else None

    def ops(self, project_id: str, state: str | None = None) -> list[dict]:
        sql = "SELECT * FROM ops WHERE project_id = ?"
        params: list[Any] = [project_id]
        if state:
            sql += " AND state = ?"
            params.append(state)
        sql += " ORDER BY id"
        with self._lock:
            rows = self._db.execute(sql, params).fetchall()
        return [_decode(row) for row in rows]

    # ── rebuild and repair ──────────────────────────────────────────────

    def rebuild(self, project_id: str) -> None:
        """Reconstruct every projection from the event log alone (DM-16).

        This is a deliberately separate code path from the incremental write.
        The duplication is what makes "events are the only truth" *verifiable*
        rather than aspirational: if the two disagree, one of them is wrong and
        :meth:`verify_rebuild` says so.
        """
        state = State(project_id)
        last = 0
        for record in self.events(project_id):
            _reduce(state, record)
            last = int(record["seq"])
        with self._lock:
            try:
                self._db.execute("DELETE FROM projects WHERE id = ?", (project_id,))
                self._db.execute("DELETE FROM milestones WHERE project_id = ?", (project_id,))
                self._db.execute("DELETE FROM items WHERE project_id = ?", (project_id,))
                self._db.execute("DELETE FROM ops WHERE project_id = ?", (project_id,))
                self._persist_state(state, project_id)
                self._db.execute(
                    "INSERT INTO projection_checkpoint (project_id, last_seq)"
                    " VALUES (?, ?) ON CONFLICT(project_id)"
                    " DO UPDATE SET last_seq = excluded.last_seq",
                    (project_id, last),
                )
                self._db.commit()
            except Exception:
                self._db.rollback()
                raise

    def ensure_consistent(self, project_id: str) -> int:
        """Tick phase 0: replay the tail if the projections lag the log.

        Cheap when consistent — one indexed ``MAX(seq)`` comparison. This is
        what makes divergence self-heal within one tick, whatever caused it:
        a crash, a manual edit, or a bug.
        """
        head = self.max_seq(project_id)
        applied = self.last_seq(project_id)
        if head <= applied:
            return 0
        state = self._load_state(project_id)
        count = 0
        for record in self.events(project_id, since=applied):
            _reduce(state, record)
            count += 1
        with self._lock:
            self._persist_state(state, project_id)
            self._db.execute(
                "INSERT INTO projection_checkpoint (project_id, last_seq)"
                " VALUES (?, ?) ON CONFLICT(project_id)"
                " DO UPDATE SET last_seq = excluded.last_seq",
                (project_id, head),
            )
            self._db.commit()
        return count

    # ── internals ───────────────────────────────────────────────────────

    def _load_state(self, project_id: str) -> State:
        state = State(project_id)
        project = self.project(project_id)
        if project:
            state.project = project
        for milestone in self.milestones(project_id):
            state.milestones[milestone["id"]] = milestone
        for item in self.items(project_id):
            state.items[item["id"]] = item
        for op in self.ops(project_id):
            state.ops[int(op["id"])] = op
        return state

    def _persist_state(self, state: State, project_id: str) -> None:
        if state.project:
            _upsert(self._db, "projects", "id", state.project)
        for milestone in state.milestones.values():
            _upsert(self._db, "milestones", "id", milestone)
        for item in state.items.values():
            _upsert(self._db, "items", "id", item)
        for op in state.ops.values():
            _upsert(self._db, "ops", "id", op)

    def close(self) -> None:
        with self._lock:
            try:
                self._db.close()
            except sqlite3.Error:
                pass


_COLUMNS = {
    "projects": ("id", "owner", "status", "halt_reason", "warned", "goal",
                 "budget_usd", "spent_usd", "budget_tokens", "spent_tokens",
                 "deadline", "completion_json", "updated_at"),
    "milestones": ("id", "project_id", "title", "detail", "status", "seq_no",
                   "depends_on_json", "acceptance_json", "updated_at"),
    "items": ("id", "project_id", "milestone_id", "title", "detail", "status",
              "attempts", "max_attempts", "owner_member", "workdir",
              "lease_owner", "lease_expires", "depends_on_json",
              "acceptance_json", "tags_json", "cost_tokens", "cost_usd",
              "result", "error", "blocked_reason", "invalidated_reason",
              "updated_at"),
    "ops": ("id", "project_id", "kind", "payload_json", "actor", "state",
            "error", "created_at", "applied_at"),
}


def _upsert(db: sqlite3.Connection, table: str, key: str, row: dict) -> None:
    columns = [c for c in _COLUMNS[table] if c in row]
    placeholders = ",".join("?" for _ in columns)
    updates = ",".join(f"{c}=excluded.{c}" for c in columns if c != key)
    db.execute(
        f"INSERT INTO {table} ({','.join(columns)}) VALUES ({placeholders})"
        f" ON CONFLICT({key}) DO UPDATE SET {updates}",
        [row[c] for c in columns],
    )


def _decode(row: sqlite3.Row) -> dict:
    data = dict(row)
    for key in ("depends_on_json", "acceptance_json", "tags_json", "payload_json"):
        if key in data:
            field = key[:-5]
            try:
                data[field] = json.loads(data[key] or "null")
            except (TypeError, ValueError):
                data[field] = None
    return data


# ── completion predicates ───────────────────────────────────────────────────


def evaluate_completion(store: ProjectStore, project_id: str) -> tuple[bool, str]:
    """Evaluate the manifest's ``completion`` spec against the projections.

    Deterministic by construction (``01`` section 10.1): the project predicate
    is never LLM-judged. Returns ``(passed, reason)``.
    """
    project = store.project(project_id)
    if not project:
        return False, "no project"
    try:
        spec = json.loads(project.get("completion_json") or "{}")
    except (TypeError, ValueError):
        spec = {}
    kind = spec.get("kind") or "all_milestones_done"
    milestones = store.milestones(project_id)
    items = store.items(project_id)

    if kind == "budget":
        cap = project.get("budget_usd")
        if cap and float(project.get("spent_usd") or 0) >= float(cap):
            return True, "budget reached"
        return False, "budget not reached"

    if kind == "manual":
        return False, "awaiting manual completion"

    if kind == "tag_threshold":
        tag = spec.get("tag")
        need = int(spec.get("count") or 0)
        have = sum(1 for item in items
                   if item["status"] == "done" and tag in (item.get("tags") or []))
        return (have >= need), f"tag {tag}: {have}/{need}"

    # default: all_milestones_done (also covers all_items_done via also_require)
    if kind == "all_items_done":
        live = [i for i in items if i["status"] not in ev.ITEM_TERMINAL]
        if live:
            return False, f"{len(live)} item(s) unfinished"
    else:
        live = [m for m in milestones if m["status"] not in ev.MILESTONE_TERMINAL]
        if live:
            return False, f"{len(live)} milestone(s) unfinished"

    for requirement in spec.get("also_require") or []:
        if requirement == "no_item_status_failed":
            bad = [i["id"] for i in items if i["status"] == "failed"]
            if bad:
                return False, f"failed items: {', '.join(bad[:5])}"
        elif requirement == "no_item_status_invalidated":
            bad = [i["id"] for i in items if i["status"] == "invalidated"]
            if bad:
                return False, f"invalidated items unresolved: {', '.join(bad[:5])}"
    return True, "all milestones done"
