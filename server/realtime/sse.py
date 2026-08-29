"""
SSE helpers — Phase 7 moved from server/api_server.py.
"""
import json


def format_sse(event: str, data: dict) -> str:
    """Format a Server-Sent Event."""
    return f"event: {event}\ndata: {json.dumps(data)}\n\n"


def mirror_publish(chat_id: str, event: str, data: dict) -> None:
    """Fan one SSE frame out to mirroring clients via the live event bus.

    No-op unless someone subscribed to /api/stream/{chat_id} — zero overhead
    and zero behavior change for the client that initiated the run.
    """
    try:
        from server.realtime.bus import publish as _bus_publish
        _bus_publish(chat_id, event, data)
    except Exception:
        pass
