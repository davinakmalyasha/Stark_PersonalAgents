"""screen_bridge — server → overlay control channel for live screen sharing.

The Electron voice overlay owns the desktop capture stream and the single
``/api/voice/ws`` connection. Agent tools run in worker threads and need to
reach INTO that overlay:

  look_at_screen    → fresh capture           (request_frame → "screen-capture")
  set_screen_share  → switch sharing mode     (request_mode  → "screen-mode")

This module keeps the live socket, an outbox drained by the WS push loop, and
pending futures per request. Results come back two ways:

  * frames: ``POST /api/vision/frame`` (server/routers/screen.py) resolves the
    waiter with the saved attachment;
  * mode switches / capture failures: the overlay sends ``screen-ack`` on the
    WS, resolved by ``resolve_ws_ack`` from the receive loop.

The overlay also broadcasts ``screen-state`` whenever the user toggles sharing
from the menu, keeping ``state()`` accurate.

Single-overlay invariant: the desktop app opens exactly one voice overlay, so
one active socket is tracked at a time; a new connection replaces the old.
"""
from __future__ import annotations

import asyncio
import json
import threading
import uuid
from concurrent.futures import Future, TimeoutError as FutureTimeout

FRAME_TIMEOUT_S = 12.0
MODE_TIMEOUT_S = 8.0
VALID_MODES = ("on", "off", "continuous")

_LOCK = threading.Lock()
_WS = None
_LOOP: asyncio.AbstractEventLoop | None = None
_OUTBOX: asyncio.Queue | None = None
_WAKE: asyncio.Event | None = None
_FRAME_WAITERS: dict[str, Future] = {}
_MODE_WAITERS: dict[str, Future] = {}
_STATE: dict = {"sharing": False, "mode": "off", "source": ""}


def attach(websocket, loop: asyncio.AbstractEventLoop, outbox: asyncio.Queue, wake: asyncio.Event) -> None:
    """Register the overlay socket + its outbox (called from the WS endpoint)."""
    global _WS, _LOOP, _OUTBOX, _WAKE
    with _LOCK:
        _WS, _LOOP, _OUTBOX, _WAKE = websocket, loop, outbox, wake


def detach(websocket) -> None:
    """Unregister on socket close, reset sharing state, fail in-flight requests."""
    global _WS, _LOOP, _OUTBOX, _WAKE
    with _LOCK:
        if _WS is not websocket:
            return
        _WS, _LOOP, _OUTBOX, _WAKE = None, None, None, None
        # The overlay is gone: nothing is being shared until it reconnects and
        # the user enables sharing again — otherwise continuous mode could keep
        # injecting a stale frame for up to the freshness window.
        _STATE["sharing"] = False
        _STATE["mode"] = "off"
        _STATE["source"] = ""
        waiters = list(_FRAME_WAITERS.values()) + list(_MODE_WAITERS.values())
        _FRAME_WAITERS.clear()
        _MODE_WAITERS.clear()
    for future in waiters:
        _finish(future, {"ok": False, "error": "Voice overlay disconnected"})


def connected() -> bool:
    with _LOCK:
        return _WS is not None


def state() -> dict:
    """Last known overlay sharing state (updated by ``screen-state`` frames)."""
    with _LOCK:
        return dict(_STATE)


def update_state(msg: dict) -> None:
    """Overlay broadcast: user toggled sharing from the menu."""
    with _LOCK:
        _STATE["sharing"] = bool(msg.get("sharing"))
        mode = str(msg.get("mode") or "").strip().lower()
        if mode in VALID_MODES:
            _STATE["mode"] = mode
        _STATE["source"] = str(msg.get("source") or "")


def request_frame(timeout: float = FRAME_TIMEOUT_S) -> dict:
    """Ask the overlay for a fresh frame of the shared screen/window.

    Blocking — call from a worker thread (agent tool loop). Resolved by the
    frame endpoint after it saves the upload. Returns
    ``{ok, path, attachment}`` or ``{ok: False, error}``.
    """
    req_id = uuid.uuid4().hex
    waiter: Future = Future()
    if not _register(_FRAME_WAITERS, req_id, waiter):
        return {"ok": False, "error": "Voice overlay is not running"}
    if not _send({"type": "screen-capture", "req_id": req_id}):
        _pop_waiter(_FRAME_WAITERS, req_id)
        return {"ok": False, "error": "Voice overlay is not reachable"}
    try:
        return waiter.result(timeout=timeout)
    except FutureTimeout:
        _pop_waiter(_FRAME_WAITERS, req_id)
        return {"ok": False, "error": f"Screen capture timed out after {int(timeout)}s"}


def request_mode(mode: str, timeout: float = MODE_TIMEOUT_S) -> dict:
    """Ask the overlay to switch sharing mode ("on" | "off" | "continuous")."""
    mode = (mode or "").strip().lower()
    if mode not in VALID_MODES:
        return {"ok": False, "error": f"Invalid mode '{mode}' (use on, off, or continuous)"}
    req_id = uuid.uuid4().hex
    waiter: Future = Future()
    if not _register(_MODE_WAITERS, req_id, waiter):
        return {"ok": False, "error": "Voice overlay is not running"}
    if not _send({"type": "screen-mode", "req_id": req_id, "mode": mode}):
        _pop_waiter(_MODE_WAITERS, req_id)
        return {"ok": False, "error": "Voice overlay is not reachable"}
    try:
        return waiter.result(timeout=timeout)
    except FutureTimeout:
        _pop_waiter(_MODE_WAITERS, req_id)
        return {"ok": False, "error": f"Screen mode switch timed out after {int(timeout)}s"}


def resolve_frame(req_id: str, attachment: dict) -> bool:
    """Resolve a pending capture with the saved frame (called by the endpoint)."""
    if not req_id:
        return False
    future = _pop_waiter(_FRAME_WAITERS, req_id)
    if future is None:
        return False
    _finish(future, {"ok": True, "path": attachment.get("path", ""), "attachment": attachment})
    return True


def resolve_ws_ack(msg: dict) -> bool:
    """Resolve a pending capture/mode request from an overlay ``screen-ack``.

    Success acks for captures are unnecessary (the upload resolves them); this
    path carries failures and all mode-switch confirmations. Successful mode
    acks also refresh the cached state, since not every overlay version
    broadcasts a separate ``screen-state`` frame.
    """
    req_id = str(msg.get("req_id") or "")
    if not req_id:
        return False
    future = _pop_waiter(_FRAME_WAITERS, req_id) or _pop_waiter(_MODE_WAITERS, req_id)
    if future is None:
        return False
    if msg.get("ok"):
        mode = str(msg.get("mode") or "").strip().lower()
        if mode in VALID_MODES:
            with _LOCK:
                _STATE["mode"] = mode
                _STATE["sharing"] = bool(msg.get("sharing", True))
        _finish(future, {
            "ok": True,
            "mode": str(msg.get("mode") or ""),
            "sharing": bool(msg.get("sharing")),
            "path": str(msg.get("path") or ""),
        })
    else:
        _finish(future, {"ok": False, "error": str(msg.get("error") or "Screen request failed")})
    return True


# ── internals ──────────────────────────────────────────────────────────────


def _register(waiters: dict, req_id: str, future: Future) -> bool:
    with _LOCK:
        if _WS is None or _LOOP is None or _OUTBOX is None or _WAKE is None:
            return False
        waiters[req_id] = future
        return True


def _pop_waiter(waiters: dict, req_id: str) -> Future | None:
    with _LOCK:
        return waiters.pop(req_id, None)


def _finish(future: Future, result: dict) -> None:
    if not future.done():
        future.set_result(result)


def _send(payload: dict) -> bool:
    """Thread-safe enqueue onto the overlay outbox (drained by the WS loop)."""
    with _LOCK:
        loop, outbox, wake = _LOOP, _OUTBOX, _WAKE
    if loop is None or outbox is None or wake is None:
        return False

    async def _put() -> None:
        outbox.put_nowait(json.dumps(payload))
        wake.set()

    try:
        asyncio.run_coroutine_threadsafe(_put(), loop).result(timeout=2.0)
        return True
    except Exception:
        return False
