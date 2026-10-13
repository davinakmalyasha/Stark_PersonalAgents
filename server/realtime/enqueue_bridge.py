"""enqueue_bridge.py — C1 bridge between inbox and RunContext.enqueue.

When the orchestrator runs inside a pydantic-ai Agent (agent.iter), child
completions and steering should be delivered via RunContext.enqueue so the
agent wakes natively, not via the polling inbox. This module holds the active
RunContext per chat_id and provides helpers that try enqueue first, fallback
to inbox.
"""
from __future__ import annotations

import threading
from typing import Any

_LOCK = threading.RLock()
_ACTIVE: dict[str, Any] = {}  # chat_id -> RunContext


def register_active_ctx(chat_id: str, ctx: Any) -> None:
    """Register the active RunContext for a chat (called from delegate tool)."""
    if not chat_id or ctx is None:
        return
    with _LOCK:
        _ACTIVE[str(chat_id)] = ctx


def unregister_active_ctx(chat_id: str) -> None:
    """Remove active context (called when orchestrator run finishes)."""
    if not chat_id:
        return
    with _LOCK:
        _ACTIVE.pop(str(chat_id), None)


def try_enqueue(chat_id: str, content: str) -> bool:
    """Try to deliver via RunContext.enqueue; return True if enqueued.

    Falls back to False so caller can use inbox.follow_up / inbox.steer.
    """
    if not chat_id or not content:
        return False
    ctx = None
    with _LOCK:
        ctx = _ACTIVE.get(str(chat_id))
    if ctx is None:
        return False
    try:
        # RunContext.enqueue is async; we are in a ThreadPool thread, so we
        # need to handle both sync and async. Pydantic-ai's enqueue is
        # actually sync-queued (it puts into an internal queue). We try
        # to call it and if it returns a coroutine, schedule it.
        maybe = ctx.enqueue(content)
        # If it returned a coroutine, we need to run it. Since we're in a
        # thread, we can create a new loop or use asyncio.run
        import asyncio
        import inspect
        if inspect.isawaitable(maybe):
            try:
                # Try to get running loop
                loop = asyncio.get_running_loop()
                # If there's a running loop, schedule as task
                loop.create_task(maybe)  # type: ignore
            except RuntimeError:
                # No running loop — run it
                asyncio.run(maybe)
        return True
    except Exception:
        return False


def enqueue_or_follow_up(chat_id: str, content: str) -> None:
    """Deliver one ordered completion to the orchestrator's inbox.

    The current orchestrator graph explicitly parks on ``park_wait``.  Sending
    the same result through both ``RunContext.enqueue`` and that inbox can make
    one child completion appear twice or arrive before the delegation step has
    settled.  Keep the inbox as the canonical transport; ``try_enqueue`` stays
    available for a future fully native runner that does not use graph parking.
    """
    try:
        from inbox import follow_up
        follow_up(chat_id, content)
    except Exception:
        pass


def enqueue_or_steer(chat_id: str, content: str) -> None:
    """Deliver steering via enqueue if active, plus inbox.steer."""
    enqueued = try_enqueue(chat_id, content)
    try:
        from inbox import steer
        steer(chat_id, content)
    except Exception:
        pass
