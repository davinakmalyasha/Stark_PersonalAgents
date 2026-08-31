"""
Inbox — steering & follow-up queues (Pi-style) plus turn abort.

Two FIFO queues per chat:

- **steering**: interrupt intent. Delivered at the next step boundary (after
  the current step's tool calls finish) so the agent can change course mid-run.
- **follow-up**: work queued for after the agent has fully finished.

Delivery modes (``INBOX_STEERING_MODE`` / ``INBOX_FOLLOWUP_MODE``):
``one-at-a-time`` (default — one queued message is delivered per boundary) or
``all`` (every queued message is delivered at once).

Abort requests a cancellation of the current turn while preserving queued
steering/follow-up (``keep_inbox=True``). The loop polls :func:`consume_abort`.

Thread-safe; keyed by chat id. Never raises on misuse (callers guard their own
error handling). Keeps no messages in memory once delivered.
"""

from __future__ import annotations

import os
import threading
from collections import deque

_mode_cache: dict[tuple[str, str], str] = {}


def _mode(kind: str) -> str:
    """Resolve a delivery mode once per process (env may change mid-run)."""
    key = (kind, os.environ.get("INBOX_MODE_OVERRIDE", ""))
    mode = _mode_cache.get(key)
    if mode is None:
        if kind == "steering":
            mode = os.environ.get("INBOX_STEERING_MODE", "one-at-a-time")
        else:
            mode = os.environ.get("INBOX_FOLLOWUP_MODE", "one-at-a-time")
        _mode_cache[key] = mode
    return mode


class _ChatInbox:
    __slots__ = ("steering", "follow_up", "abort_requested", "lock")

    def __init__(self) -> None:
        self.steering: deque[str] = deque()
        self.follow_up: deque[str] = deque()
        self.abort_requested = False
        self.lock = threading.RLock()


_inboxes: dict[str, _ChatInbox] = {}
_inboxes_lock = threading.RLock()


def _inbox(chat_id: str) -> _ChatInbox:
    with _inboxes_lock:
        ib = _inboxes.get(chat_id)
        if ib is None:
            ib = _ChatInbox()
            _inboxes[chat_id] = ib
        return ib


def _drain(queue: deque[str], mode: str) -> list[str]:
    if not queue:
        return []
    if mode == "all":
        out = list(queue)
        queue.clear()
        return out
    return [queue.popleft()]


# ── Public API ───────────────────────────────────────────────────────────────


def steer(chat_id: str, message: str) -> bool:
    """Queue a steering message (interrupt intent). Returns True if queued."""
    if not message or not str(message).strip():
        return False
    with _inbox(chat_id).lock:
        _inbox(chat_id).steering.append(str(message))
    return True


def follow_up(chat_id: str, message: str) -> bool:
    """Queue a follow-up message (delivered only after the agent is idle)."""
    if not message or not str(message).strip():
        return False
    with _inbox(chat_id).lock:
        _inbox(chat_id).follow_up.append(str(message))
    return True


def drain_steering(chat_id: str) -> list[str]:
    """Deliver queued steering for the next step boundary."""
    ib = _inbox(chat_id)
    with ib.lock:
        return _drain(ib.steering, _mode("steering"))


def drain_follow_up(chat_id: str) -> list[str]:
    """Deliver queued follow-up work (only when the agent is idle)."""
    ib = _inbox(chat_id)
    with ib.lock:
        return _drain(ib.follow_up, _mode("follow_up"))


def abort(chat_id: str, keep_inbox: bool = True) -> None:
    """Request cancellation of the current turn.

    ``keep_inbox=True`` preserves queued steering/follow-up for a later turn.
    Also cancels any registered pydantic-ai CancellationToken (P1-2) for
    immediate delivery.
    """
    ib = _inbox(chat_id)
    with ib.lock:
        ib.abort_requested = True
        if not keep_inbox:
            ib.steering.clear()
            ib.follow_up.clear()
    # P1-2: immediate token cancel (no 0.25s poll latency)
    try:
        from llm.cancellation import cancel as _cancel_token

        _cancel_token(chat_id)
    except Exception:
        pass


def consume_abort(chat_id: str) -> bool:
    """True once if an abort was requested since the last poll."""
    ib = _inbox(chat_id)
    with ib.lock:
        if ib.abort_requested:
            ib.abort_requested = False
            return True
        return False


def pending_counts(chat_id: str) -> dict:
    ib = _inbox(chat_id)
    with ib.lock:
        return {
            "steering": len(ib.steering),
            "follow_up": len(ib.follow_up),
            "abort_pending": ib.abort_requested,
        }


def clear_inbox_registry() -> None:
    with _inboxes_lock:
        _inboxes.clear()


def render_delivery(label: str, messages: list[str]) -> str:
    """Format delivered messages for injection into the agent's context."""
    body = "\n".join(f"- {m}" for m in messages)
    return f"[{label}]\n{body}"
