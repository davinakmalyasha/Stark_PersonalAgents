"""
StreamBus — per-chat token delta bus for live SSE streaming (P10).

Agent loops push text/reasoning deltas here as they generate; the API server's
SSE loop drains the bus for the active chat and emits ``event: delta`` frames.
Thread-safe; per-chat keyed; entries are drained (read-once). Never raises.
"""

from __future__ import annotations

import os
import threading
import time
from collections import deque

_BUS: dict[str, deque] = {}
_LOCK = threading.RLock()
_ACTIVE: dict[str, float] = {}  # chat_id -> last emit timestamp
_MAX_ENTRIES = 2000  # per-chat safety cap


def enabled() -> bool:
    """Master switch (TOKEN_STREAMING=0 disables deltas; legacy
    BSA_TOKEN_STREAMING still honored for existing .env files)."""
    val = os.environ.get("TOKEN_STREAMING")
    if val is None:
        val = os.environ.get("BSA_TOKEN_STREAMING", "1")
    return val.lower() not in ("0", "false", "no", "off")


def emit(chat_id: str, kind: str, text: str, agent: str = "", model: str = "") -> None:
    """Push one delta (kind: text | reasoning | usage | end)."""
    if not enabled() or not chat_id or not text:
        return
    try:
        with _LOCK:
            q = _BUS.setdefault(chat_id, deque())
            if len(q) >= _MAX_ENTRIES:
                return
            q.append({
                "kind": kind,
                "text": str(text),
                "agent": agent,
                "model": model,
                "ts": time.time(),
            })
            _ACTIVE[chat_id] = time.time()
    except Exception:
        pass


def drain(chat_id: str, limit: int = 60) -> list[dict]:
    """Return up to ``limit`` pending deltas for a chat (read-once)."""
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
    """Drop chat buses idle for ``older_than`` seconds (best-effort)."""
    try:
        now = time.time()
        with _LOCK:
            stale = [cid for cid, ts in _ACTIVE.items() if now - ts > older_than]
            for cid in stale:
                _BUS.pop(cid, None)
                _ACTIVE.pop(cid, None)
    except Exception:
        pass
