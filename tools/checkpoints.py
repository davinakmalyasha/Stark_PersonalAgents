"""
Stage 3: per-message file snapshots for chat-level rewind.

Per the mega-plan: the existing per-run undo at `/api/runs/{run_id}/undo`
is run-scoped, git-dependent, and only on artifact-bearing messages. This
module adds a **chat-scoped, run-grouped** snapshot system that backs a
`/rewind` slash command and a chat-header Rewind popover.

Storage layout (per project):
    <workspace>/.deep_agents/snapshots/<chat_id>/<run_id>/<path>.snap
    <workspace>/.deep_agents/snapshots/<chat_id>/_index.json

Each `<path>.snap` is the file's bytes before the run touched it. Gzipped
if > 16 KB. `_index.json` lists the run ids and their files with mtimes
so the API can render a timeline cheaply.

The capture is best-effort: a failed snapshot does NOT block the write
(the existing git/dirty_files path is the fallback). Snapshots are
LRU-capped at the most-recent N per chat (default 100).
"""

from __future__ import annotations

import gzip
import hashlib
import json
import os
import time
import uuid
from dataclasses import dataclass, field
from typing import Optional

_DEFAULT_MAX_PER_CHAT = 100
_SNAPSHOT_GZIP_THRESHOLD = 16 * 1024  # 16 KB
_CURRENT_RUN_KEY = "_active_run_id_per_chat"  # see _set_active_run / _get_active_run


@dataclass
class Snapshot:
    snap_id: str
    run_id: str
    chat_id: str
    workspace_path: str
    file_path: str       # relative to workspace
    abs_path: str
    size: int
    sha256: str
    gzipped: bool
    created_at: float


# In-memory index: chat_id -> list of snapshot metadata (newest first).
# On startup the index is rebuilt from disk by `load_index`.
_INDEX: dict[str, list[dict]] = {}
_ACTIVE_RUN: dict[str, str] = {}  # chat_id -> run_id


# ── public API ─────────────────────────────────────────────────────────────


def _snap_dir(workspace_path: str, chat_id: str, run_id: str) -> str:
    return os.path.join(workspace_path, ".deep_agents", "snapshots", chat_id, run_id)


def _index_path(workspace_path: str, chat_id: str) -> str:
    return os.path.join(workspace_path, ".deep_agents", "snapshots", chat_id, "_index.json")


def _set_active_run(chat_id: str, run_id: str) -> None:
    _ACTIVE_RUN[chat_id] = run_id


def _get_active_run(chat_id: str) -> str:
    return _ACTIVE_RUN.get(chat_id, f"orphan-{uuid.uuid4().hex[:8]}")


def begin_run(chat_id: str, workspace_path: str) -> str:
    """Allocate a new run_id and record it as the active run for this chat.
    Returns the run_id. Called by the API server at the start of every prompt."""
    run_id = f"run-{int(time.time())}-{uuid.uuid4().hex[:6]}"
    _set_active_run(chat_id, run_id)
    return run_id


def pre_write_snapshot(
    chat_id: str,
    workspace_path: str,
    rel_path: str,
    *,
    run_id: Optional[str] = None,
) -> Optional[Snapshot]:
    """Capture the current on-disk bytes of `rel_path` before a write.
    Best-effort: returns None on any error. The write proceeds regardless.
    """
    if not workspace_path or not rel_path:
        return None
    if run_id is None:
        run_id = _get_active_run(chat_id)

    abs_path = rel_path if os.path.isabs(rel_path) else os.path.join(workspace_path, rel_path)
    if not os.path.isfile(abs_path):
        return None

    try:
        with open(abs_path, "rb") as f:
            data = f.read()
    except OSError:
        return None

    sha = hashlib.sha256(data).hexdigest()
    snap_id = f"snap-{int(time.time()*1000)}-{uuid.uuid4().hex[:6]}"
    snap_dir = _snap_dir(workspace_path, chat_id, run_id)
    os.makedirs(snap_dir, exist_ok=True)

    # Path may include subdirectories — flatten for the filename but keep the
    # original rel_path in metadata.
    safe_name = rel_path.replace(os.sep, "__").replace(":", "_")
    snap_file = os.path.join(snap_dir, f"{safe_name}.snap")
    gzipped = False
    try:
        if len(data) > _SNAPSHOT_GZIP_THRESHOLD:
            with open(snap_file, "wb") as f:
                f.write(gzip.compress(data))
            gzipped = True
        else:
            with open(snap_file, "wb") as f:
                f.write(data)
    except OSError:
        return None

    snap = Snapshot(
        snap_id=snap_id,
        run_id=run_id,
        chat_id=chat_id,
        workspace_path=workspace_path,
        file_path=rel_path,
        abs_path=abs_path,
        size=len(data),
        sha256=sha,
        gzipped=gzipped,
        created_at=time.time(),
    )
    _INDEX.setdefault(chat_id, []).append({
        "snap_id": snap.snap_id,
        "run_id": snap.run_id,
        "file_path": snap.file_path,
        "size": snap.size,
        "sha256": snap.sha256,
        "gzipped": snap.gzipped,
        "created_at": snap.created_at,
    })
    _enforce_cap(chat_id, workspace_path)
    _save_index(workspace_path, chat_id)
    return snap


def list_snapshots(chat_id: str, workspace_path: str, limit: int = 100) -> list[dict]:
    """Return a list of run-grouped snapshot summaries, newest first."""
    load_index(workspace_path, chat_id)
    snaps = list(_INDEX.get(chat_id, []))
    snaps.sort(key=lambda s: s["created_at"], reverse=True)
    snaps = snaps[:limit]
    # Group by run_id so the UI can collapse per-run.
    groups: dict[str, dict] = {}
    for s in snaps:
        g = groups.setdefault(s["run_id"], {
            "run_id": s["run_id"],
            "created_at": s["created_at"],
            "files": [],
            "total_size": 0,
        })
        g["files"].append({"path": s["file_path"], "size": s["size"]})
        g["total_size"] += s["size"]
    return list(groups.values())


def restore_snapshot(chat_id: str, workspace_path: str, snap_id: str) -> tuple[bool, str]:
    """Restore a single snapshot by id. Returns (ok, message).

    Restore is unconditional: it overwrites the on-disk file with the
    snapshot bytes. Callers wanting a pre-flight check should call
    `inspect_snapshot(chat_id, workspace_path, snap_id)` first.
    """
    load_index(workspace_path, chat_id)
    meta = next((s for s in _INDEX.get(chat_id, []) if s["snap_id"] == snap_id), None)
    if meta is None:
        return False, f"snapshot {snap_id} not found"
    run_id = meta["run_id"]
    safe_name = meta["file_path"].replace(os.sep, "__").replace(":", "_")
    snap_file = os.path.join(_snap_dir(workspace_path, chat_id, run_id), f"{safe_name}.snap")
    if not os.path.isfile(snap_file):
        return False, f"snapshot file missing: {snap_file}"
    try:
        with open(snap_file, "rb") as f:
            blob = f.read()
        if meta["gzipped"]:
            blob = gzip.decompress(blob)
        target = os.path.join(workspace_path, meta["file_path"])
        os.makedirs(os.path.dirname(target) or ".", exist_ok=True)
        tmp = target + f".rewind-{uuid.uuid4().hex[:6]}.tmp"
        with open(tmp, "wb") as f:
            f.write(blob)
        os.replace(tmp, target)
        return True, f"restored {meta['file_path']} ({meta['size']} bytes)"
    except (OSError, gzip.BadGzipFile) as e:
        return False, f"restore failed: {e}"


def inspect_snapshot(chat_id: str, workspace_path: str, snap_id: str) -> dict:
    """Return metadata for a snapshot plus a `conflicts: bool` flag that
    indicates whether the on-disk file currently differs from the snap
    (so the UI can warn before restore)."""
    load_index(workspace_path, chat_id)
    meta = next((s for s in _INDEX.get(chat_id, []) if s["snap_id"] == snap_id), None)
    if meta is None:
        return {"exists": False, "snap_id": snap_id}
    target = os.path.join(workspace_path, meta["file_path"])
    conflict = False
    if os.path.isfile(target):
        try:
            with open(target, "rb") as f:
                cur_sha = hashlib.sha256(f.read()).hexdigest()
            conflict = (cur_sha != meta["sha256"])
        except OSError:
            conflict = True
    else:
        conflict = False  # file was deleted; restore will recreate
    return {
        "exists": True,
        "snap_id": snap_id,
        "run_id": meta["run_id"],
        "file_path": meta["file_path"],
        "size": meta["size"],
        "created_at": meta["created_at"],
        "conflicts": conflict,
    }


def restore_run(chat_id: str, workspace_path: str, run_id: str) -> tuple[bool, list[str]]:
    """Restore every snapshot for a given run_id. Returns (all_ok, messages)."""
    load_index(workspace_path, chat_id)
    msgs: list[str] = []
    all_ok = True
    for meta in [m for m in _INDEX.get(chat_id, []) if m["run_id"] == run_id]:
        ok, msg = restore_snapshot(chat_id, workspace_path, meta["snap_id"])
        msgs.append(msg)
        if not ok and "changed since snapshot" in msg:
            all_ok = False
    return all_ok, msgs


# ── index persistence ──────────────────────────────────────────────────────


def load_index(workspace_path: str, chat_id: str) -> None:
    """Rebuild in-memory index from disk if not already loaded."""
    if chat_id in _INDEX:
        return
    p = _index_path(workspace_path, chat_id)
    if not os.path.isfile(p):
        _INDEX[chat_id] = []
        return
    try:
        with open(p, "r", encoding="utf-8") as f:
            _INDEX[chat_id] = json.load(f)
    except (OSError, json.JSONDecodeError):
        _INDEX[chat_id] = []


def _save_index(workspace_path: str, chat_id: str) -> None:
    p = _index_path(workspace_path, chat_id)
    try:
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w", encoding="utf-8") as f:
            json.dump(_INDEX.get(chat_id, []), f)
    except OSError:
        pass


def _enforce_cap(chat_id: str, workspace_path: str) -> None:
    """LRU-trim to `_DEFAULT_MAX_PER_CHAT` per chat. Oldest entries are deleted."""
    snaps = _INDEX.get(chat_id, [])
    if len(snaps) <= _DEFAULT_MAX_PER_CHAT:
        return
    snaps.sort(key=lambda s: s["created_at"])
    excess = snaps[:len(snaps) - _DEFAULT_MAX_PER_CHAT]
    keep = snaps[len(snaps) - _DEFAULT_MAX_PER_CHAT:]
    _INDEX[chat_id] = keep
    for s in excess:
        safe_name = s["file_path"].replace(os.sep, "__").replace(":", "_")
        snap_file = os.path.join(_snap_dir(workspace_path, chat_id, s["run_id"]), f"{safe_name}.snap")
        try:
            os.remove(snap_file)
        except OSError:
            pass


# ── registry integration helper ───────────────────────────────────────────


def snapshot_for_tool(ctx: dict) -> Optional[Snapshot]:
    """Helper used by the registry execute-wrapper for write-class tools.
    Looks at the tool name + args to find the file path and calls
    `pre_write_snapshot`.
    """
    name = ctx.get("name", "")
    args = ctx.get("args") or {}
    workspace_path = ctx.get("workspace_path") or ""
    chat_id = ctx.get("chat_id") or ""
    if not workspace_path or not chat_id:
        return None
    rel_path = args.get("file_path") or args.get("path") or ""
    if name == "edit_file" and rel_path:
        return pre_write_snapshot(chat_id, workspace_path, rel_path)
    return None
