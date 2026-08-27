"""
Live run event bus (mobile mirroring).

/api/run streams SSE frames only to the client that started the run. This hub
lets ANY number of additional clients (the phone, another browser window)
subscribe to the same frames per chat_id, so every screen shows the identical
live session. Publishing is thread-safe: the pipeline runs in a worker thread
while SSE generators await on the asyncio side.
"""
import queue
import threading

_subs_lock = threading.Lock()
_subscribers: dict[str, list[queue.Queue]] = {}

_MAX_QUEUE = 800


def publish(chat_id: str, event: str, data: dict) -> None:
    """Fan one frame out to every subscriber of this chat (never blocks)."""
    with _subs_lock:
        subs = list(_subscribers.get(chat_id or "", []))
    for q in subs:
        try:
            q.put_nowait((event, data))
        except queue.Full:
            # A stalled viewer must not slow the live run down.
            pass


def subscribe(chat_id: str) -> queue.Queue:
    """Register a new listener queue for a chat's events."""
    q: queue.Queue = queue.Queue(maxsize=_MAX_QUEUE)
    with _subs_lock:
        _subscribers.setdefault(chat_id or "", []).append(q)
    return q


def unsubscribe(chat_id: str, q: queue.Queue) -> None:
    """Remove a listener queue (called when its SSE stream closes)."""
    with _subs_lock:
        lst = _subscribers.get(chat_id or "", [])
        if q in lst:
            lst.remove(q)
        if lst is not None and len(lst) == 0:
            _subscribers.pop(chat_id or "", None)


def subscriber_count(chat_id: str) -> int:
    with _subs_lock:
        return len(_subscribers.get(chat_id or "", []))
