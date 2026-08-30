"""
Stage 5: advisory file watcher.

Per the mega-plan: an opt-in tool that surfaces external file changes to
the agent. Advisory only — does NOT invalidate the in-flight read cache,
does NOT interrupt a running tool, does NOT change the LLM's session JSONL.

Behavior:
- Lazy: starts a `watchfiles` observer on first call (or first registry
  registration). The observer records paths + timestamps in a small
  in-process ring buffer (capped at 200 events).
- `files_changed(limit=5)` returns the most recent events as a single
  text block the model can read.
- If `watchfiles` is not installed, `files_changed` returns a friendly
  "not available" string. The harness does not crash.

Wired via `tools/_watcher_tool.py` as a no-execute function on the
registry, so it's a callable the agent can invoke.
"""

from __future__ import annotations

import os
import threading
import time
from collections import deque
from typing import Optional

_RING: deque = deque(maxlen=200)
_LOCK = threading.RLock()
_OBSERVER = None
_STARTED = False

# Phase 7: 300ms debounce — a burst of edits produces ONE invalidation
# (writes _increment_cache_version + marks the repo-map state dirty) instead
# of one per change event.
_DEBOUNCE_MS = 0.3
_debounce_timer = None


def _invalidate_from_change() -> None:
    """Single coalesced invalidation: bump the tool cache version and mark the
    repository-map state dirty so refresh_code_map_context skips
    topology_snapshot() when the workspace was untouched (memory_manager.py:113)."""
    try:
        from tools._runtime import _increment_cache_version
        _increment_cache_version()
    except Exception:
        pass
    try:
        from sync.state_sync import safe_get_state
        ws = safe_get_state().get("project_path") or os.environ.get("HARNESS_ROOT", "")
        if ws:
            from memory.memory_manager import _repository_map_state_file, _load_repository_map_state, _save_repository_map_state
            state = _load_repository_map_state(ws)
            state["dirty"] = True
            _save_repository_map_state(ws, state)
    except Exception:
        pass


def _schedule_invalidation() -> None:
    global _debounce_timer
    with _LOCK:
        if _debounce_timer is not None and _debounce_timer.is_alive():
            # Burst in progress — reset the window.
            _debounce_timer.cancel()
        _debounce_timer = threading.Timer(_DEBOUNCE_MS, _invalidate_from_change)
        _debounce_timer.daemon = True
        _debounce_timer.start()


def _try_start(workspace_path: str) -> None:
    global _OBSERVER, _STARTED
    if _STARTED:
        return
    _STARTED = True
    try:
        from watchfiles import awatch  # type: ignore
    except Exception:
        # watchfiles is optional. The tool will report "not available" later.
        return

    def _on_change(changes):
        with _LOCK:
            for change_type, path in changes:
                _RING.append({
                    "ts": time.time(),
                    "change": str(change_type),
                    "path": str(path),
                })
        # Phase 7: debounced cache invalidation + repo-map dirty flag.
        _schedule_invalidation()

    def _run():
        try:
            import asyncio
            asyncio.run(awatch(workspace_path, on_change=_on_change))
        except Exception:
            pass

    t = threading.Thread(target=_run, daemon=True, name="file-watcher")
    t.start()


def note(workspace_path: str) -> None:
    """Called from `tools/_registry.execute_tool_detailed` (lazy, on first
    tool execution) to start the observer once we have a workspace to watch."""
    if workspace_path and os.path.isdir(workspace_path):
        _try_start(workspace_path)


def files_changed(limit: int = 5) -> str:
    """Tool body. Returns the most recent external file changes as text."""
    if not _STARTED:
        # Lazy self-start: first tool call begins observing the workspace.
        try:
            from sync.state_sync import safe_get_state
            ws = (safe_get_state() or {}).get("project_path") or ""
            if ws:
                note(ws)
        except Exception:
            pass
    if not _STARTED:
        return ("[file-watcher] not started. Set up a workspace first "
                "or install the `watchfiles` package for live external-change tracking.")
    with _LOCK:
        items = list(_RING)[-max(1, min(limit, 50)):]
    if not items:
        return "[file-watcher] no external changes recorded yet."
    lines = [f"[file-watcher] last {len(items)} external change(s):"]
    for it in reversed(items):
        ts = time.strftime("%H:%M:%S", time.localtime(it["ts"]))
        lines.append(f"  {ts}  {it['change']:>6}  {it['path']}")
    return "\n".join(lines)
