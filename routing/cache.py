"""
Lightweight TTL cache for project index scans.

list_projects() hits disk (workspaces.json + sessions) per request.
We memoize for 3s and invalidate on index writes.
"""
import time
import threading

_TTL = 3.0
_lock = threading.RLock()
_cached_projects: list[dict] | None = None
_cached_at: float = 0.0


def get_cached(list_fn):
    global _cached_projects, _cached_at
    now = time.time()
    with _lock:
        if _cached_projects is not None and (now - _cached_at) < _TTL:
            return _cached_projects
        fresh = list_fn()
        _cached_projects = fresh
        _cached_at = now
        return fresh


def invalidate():
    global _cached_projects, _cached_at
    with _lock:
        _cached_projects = None
        _cached_at = 0.0
