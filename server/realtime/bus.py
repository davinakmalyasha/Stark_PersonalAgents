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

# Async waiters for event-driven wake (emit -> set event so runs.py doesn't poll 50ms)
_BUS_ASYNC_EVENTS: dict[str, "asyncio.Event"] = {}
_BUS_ASYNC_LOOPS: dict[str, "asyncio.AbstractEventLoop"] = {}
try:
    import asyncio as _asyncio  # noqa: used in get_bus_event / emit
except Exception:
    _asyncio = None  # type: ignore

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

def _notify_async_waiters(chat_id: str) -> None:
    """Wake any async waiter via call_soon_threadsafe (thread-safe emit -> async wait)."""
    if _asyncio is None:
        return
    try:
        with _LOCK:
            evt = _BUS_ASYNC_EVENTS.get(chat_id)
            loop = _BUS_ASYNC_LOOPS.get(chat_id)
        if evt is not None and loop is not None:
            try:
                if loop.is_closed():
                    return
                loop.call_soon_threadsafe(evt.set)
            except Exception:
                # Fallback direct set if loop call fails (e.g., caller is same thread)
                try:
                    evt.set()
                except Exception:
                    pass
        elif evt is not None:
            try:
                evt.set()
            except Exception:
                pass
    except Exception:
        pass


def get_bus_event(chat_id: str):
    """Get/create an asyncio.Event for this chat so runs.py can await bus deltas event-driven.

    Must be called from the async context that will await it (captures its loop).
    """
    if _asyncio is None:
        return None
    try:
        loop = _asyncio.get_running_loop()
    except RuntimeError:
        loop = None
    with _LOCK:
        evt = _BUS_ASYNC_EVENTS.get(chat_id)
        if evt is None:
            try:
                evt = _asyncio.Event()
            except Exception:
                return None
            _BUS_ASYNC_EVENTS[chat_id] = evt
            if loop is not None:
                _BUS_ASYNC_LOOPS[chat_id] = loop
        else:
            # refresh loop if caller now has a running loop
            if loop is not None:
                _BUS_ASYNC_LOOPS[chat_id] = loop
        return evt
    return evt


def clear_bus_event(chat_id: str) -> None:
    try:
        with _LOCK:
            _BUS_ASYNC_EVENTS.pop(chat_id, None)
            _BUS_ASYNC_LOOPS.pop(chat_id, None)
    except Exception:
        pass


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
    _notify_async_waiters(chat_id)


def emit_event(chat_id: str, payload: dict) -> None:
    """Structured frames (tool_started/tool_finished, ...) on the same transport.

    Drained and SSE-forwarded exactly like token deltas; the frontend
    distinguishes frames by ``kind``. Best-effort: never raises.
    """
    if not enabled() or not chat_id or not isinstance(payload, dict):
        return
    try:
        with _LOCK:
            q = _BUS.setdefault(chat_id, deque())
            if len(q) >= _MAX_ENTRIES:
                return
            frame = dict(payload)
            frame.setdefault("ts", time.time())
            q.append(frame)
            _ACTIVE[chat_id] = time.time()
    except Exception:
        pass
    _notify_async_waiters(chat_id)

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
                _BUS_ASYNC_EVENTS.pop(cid, None)
                _BUS_ASYNC_LOOPS.pop(cid, None)
    except Exception:
        pass
