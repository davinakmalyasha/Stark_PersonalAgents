"""
Unified realtime bus — merged Desktop/event_bus.py + stream_bus.py.
Single module for mobile mirroring (event) + token deltas (stream). Thread-safe.
"""
from __future__ import annotations
import os
import queue
import threading
import time
from collections import deque

# Event bus (mobile mirroring)
_evt_lock = threading.Lock()
_subscribers: dict[str, list[queue.Queue]] = {}
_MAX_QUEUE = 800

def publish(chat_id: str, event: str, data: dict) -> None:
    with _evt_lock:
        subs = list(_subscribers.get(chat_id or "", []))
    for q in subs:
        try:
            q.put_nowait((event, data))
        except queue.Full:
            pass

def subscribe(chat_id: str) -> queue.Queue:
    q: queue.Queue = queue.Queue(maxsize=_MAX_QUEUE)
    with _evt_lock:
        _subscribers.setdefault(chat_id or "", []).append(q)
    return q

def unsubscribe(chat_id: str, q: queue.Queue) -> None:
    with _evt_lock:
        lst = _subscribers.get(chat_id or "", [])
        if q in lst:
            lst.remove(q)
        if lst is not None and len(lst) == 0:
            _subscribers.pop(chat_id or "", None)

def subscriber_count(chat_id: str) -> int:
    with _evt_lock:
        return len(_subscribers.get(chat_id or "", []))

# Stream bus (token deltas)
_BUS: dict[str, deque] = {}
_LOCK = threading.RLock()
_ACTIVE: dict[str, float] = {}
_MAX_ENTRIES = 2000

def enabled() -> bool:
    """Stream bus on/off.

    Single flag PAI_INSTRUMENT (default on) controls both native
    instrumentation and the token-delta stream bus. Legacy TOKEN_STREAMING /
    BSA_TOKEN_STREAMING env vars are honored as overrides when set, so
    existing deployments keep working; new deployments use PAI_INSTRUMENT.
    """
    val = os.environ.get("TOKEN_STREAMING")
    if val is None:
        val = os.environ.get("BSA_TOKEN_STREAMING")
    if val is not None:
        return val.lower() not in ("0", "false", "no", "off")
    instr = os.environ.get("PAI_INSTRUMENT", "1")
    return instr.lower() not in ("0", "false", "no", "off")

def emit(chat_id: str, kind: str, text: str, agent: str = "", model: str = "") -> None:
    if not enabled() or not chat_id or not text:
        return
    try:
        with _LOCK:
            q = _BUS.setdefault(chat_id, deque())
            if len(q) >= _MAX_ENTRIES:
                return
            q.append({"kind": kind, "text": str(text), "agent": agent, "model": model, "ts": time.time()})
            _ACTIVE[chat_id] = time.time()
    except Exception:
        pass

def drain(chat_id: str, limit: int = 60) -> list[dict]:
    if not enabled() or not chat_id:
        return []
    try:
        with _LOCK:
            q = _BUS.get(chat_id)
            if not q:
                return []
            out = []
            for _ in range(min(limit, len(q))):
                out.append(q.popleft())
            if not q:
                _BUS.pop(chat_id, None)
            return out
    except Exception:
        return []

def active(chat_id: str) -> bool:
    try:
        with _LOCK:
            return chat_id in _BUS and bool(_BUS.get(chat_id))
    except Exception:
        return False

def clear(chat_id: str) -> None:
    try:
        with _LOCK:
            _BUS.pop(chat_id, None)
            _ACTIVE.pop(chat_id, None)
    except Exception:
        pass

def cleanup(older_than: float = 600.0) -> None:
    try:
        now = time.time()
        with _LOCK:
            stale = [cid for cid, ts in _ACTIVE.items() if now - ts > older_than]
            for cid in stale:
                _BUS.pop(cid, None)
                _ACTIVE.pop(cid, None)
    except Exception:
        pass
