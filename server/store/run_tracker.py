"""
Active run tracker — Phase 7 moved from server/api_server.py.
Per-chat active runs: allows parallel pipelines across workspaces.
Frontend poll /api/runs/active still expects single ActiveRun shape → returns
the most recent running entry for backward compat, plus `runs` list.
"""
import threading
from datetime import datetime, timezone

_active_run: dict = {"running": False, "workspace_path": "", "chat_id": "", "started_at": ""}
_active_run_lock = threading.Lock()
_active_runs: dict[str, dict] = {}
_active_runs_lock = threading.Lock()


def _active_run_key(workspace_path: str, chat_id: str) -> str:
    return (chat_id or "") if chat_id else (workspace_path or "_general")


def set_active_run(workspace_path: str, chat_id: str) -> None:
    key = _active_run_key(workspace_path, chat_id)
    info = {
        "running": True,
        "workspace_path": workspace_path or "",
        "chat_id": chat_id or "",
        "started_at": datetime.now(timezone.utc).isoformat(),
    }
    with _active_runs_lock:
        _active_runs[key] = info
    with _active_run_lock:
        _active_run.update(info)


def clear_active_run(chat_id: str = "", workspace_path: str = "") -> None:
    if not chat_id and not workspace_path:
        with _active_runs_lock:
            _active_runs.clear()
        with _active_run_lock:
            _active_run.update({"running": False, "workspace_path": "", "chat_id": "", "started_at": ""})
        return
    key = _active_run_key(workspace_path, chat_id)
    with _active_runs_lock:
        _active_runs.pop(key, None)
        remaining = list(_active_runs.values())
        with _active_run_lock:
            if remaining:
                latest = max(remaining, key=lambda r: r.get("started_at", ""))
                _active_run.update(latest)
            else:
                _active_run.update({"running": False, "workspace_path": "", "chat_id": "", "started_at": ""})


def snapshot(chat_id: str = "", workspace_path: str = "") -> dict:
    """Return the active-run shape for GET /api/runs/active."""
    if chat_id or workspace_path:
        key = _active_run_key(workspace_path, chat_id)
        with _active_runs_lock:
            info = _active_runs.get(key)
            if info:
                return dict(info)
        with _active_run_lock:
            if _active_run.get("chat_id") == chat_id and _active_run.get("workspace_path") == workspace_path:
                return dict(_active_run)
        return {"running": False, "workspace_path": workspace_path or "", "chat_id": chat_id or "", "started_at": ""}
    with _active_runs_lock:
        runs = list(_active_runs.values())
    with _active_run_lock:
        legacy = dict(_active_run)
    if not runs:
        return legacy
    if len(runs) == 1:
        out = dict(runs[0])
        out["runs"] = runs
        return out
    out = dict(legacy)
    out["runs"] = runs
    out["running"] = True
    return out
