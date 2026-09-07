"""cancellation.py --- native CancellationToken registry for inbox abort (P1-2).

Maps chat_id -> CancellationToken so server/realtime/inbox.abort() can
cancel the in-flight pydantic-ai run immediately, instead of waiting for
the 0.25s poll in chat_cancellation_token. Thread-safe.
"""
from __future__ import annotations

import threading
from typing import Any

_TOKENS: dict[str, Any] = {}
_LOCK = threading.RLock()


def register(chat_id: str, token: Any) -> None:
    if not chat_id:
        return
    with _LOCK:
        _TOKENS[str(chat_id)] = token


def unregister(chat_id: str) -> None:
    if not chat_id:
        return
    with _LOCK:
        _TOKENS.pop(str(chat_id), None)


def cancel(chat_id: str) -> bool:
    """Cancel the token for chat_id if registered. Returns True if cancelled."""
    if not chat_id:
        return False
    tok = None
    with _LOCK:
        tok = _TOKENS.get(str(chat_id))
    if tok is None:
        return False
    try:
        tok.cancel()
        return True
    except Exception:
        return False


def get(chat_id: str) -> Any | None:
    with _LOCK:
        return _TOKENS.get(str(chat_id))
