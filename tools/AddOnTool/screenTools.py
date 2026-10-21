"""screenTools — live screen-share tools for the voice overlay.

The user's screen is captured by the Electron voice overlay ("look at
screen"), never by this process. These tools talk to that overlay over the
live ``/api/voice/ws`` connection through ``server.services.screen_bridge``:

  look_at_screen    → fresh frame of the shared screen/window, queued into the
                      vision channel as an inline image for the next model turn.
  set_screen_share  → on / off / continuous sharing, mirrored in the overlay UI.

Both fail honestly when the overlay is not running — they never fabricate a
capture.
"""
from __future__ import annotations

_MODE_ALIASES = {
    "on": "on",
    "start": "on",
    "share": "on",
    "look": "on",
    "off": "off",
    "stop": "off",
    "hide": "off",
    "continuous": "continuous",
    "always": "continuous",
    "live": "continuous",
}

_OVERLAY_HINT = (
    "The live voice overlay is not available. Ask the user to open live mode "
    "(Ctrl+G) in the Personal workspace."
)


def _hint_for(error: str) -> str:
    """Only append the open-the-overlay hint when the failure IS a missing overlay."""
    low = (error or "").lower()
    if "not running" in low or "not reachable" in low or "disconnected" in low or "timed out" in low:
        return " " + _OVERLAY_HINT
    return ""


def look_at_screen(ctx: object = None) -> str:
    """Capture the user's live screen right now and attach it inline (vision).

    Use when the answer depends on what the user is CURRENTLY looking at —
    a highlighted passage, a chart, an error, a page section — or when they say
    "this", "here", "what I'm looking at". Turns sharing on if it is off (the
    overlay shows the preview so the user knows). The captured frame is queued
    into the vision channel and reaches the model before its next reply.
    """
    from server.services.screen_bridge import request_frame, state

    result = request_frame()
    if not result.get("ok"):
        err = result.get("error") or "unknown error"
        return f"Error: cannot look at the screen — {err}.{_hint_for(err)}"

    attachment = result.get("attachment") or {}
    path = result.get("path") or attachment.get("path") or ""
    if not path:
        return "Error: the screen frame was captured but no file path was returned."

    try:
        from tools.vision_channel import queue_image_file

        attached = queue_image_file(
            path,
            note="Live screen — captured by look_at_screen() just now",
            ctx=ctx,
        )
    except Exception as e:
        return f"Error: frame saved at {path} but the inline image could not be queued: {e}"
    if not attached:
        return f"Error: frame saved at {path} but it is too large to attach inline (max 6 MB)."

    mode = (state() or {}).get("mode", "")
    tail = " Screen sharing was turned on for this capture." if mode == "on" else ""
    return (
        "[IMAGE] Live screen attached inline (vision) — the picture follows as an "
        "image block in the next user message. It was captured a moment ago, so "
        "describe only what is actually visible; if text is unreadable, say so." + tail
    )


def set_screen_share(mode: str = "", ctx: object = None) -> str:
    """Switch the overlay's screen-sharing mode.

    mode: "on" (preview + a fresh frame attached to each voice turn),
    "off" (stop sharing), or "continuous" (also refresh the latest frame every
    few seconds so look_at_screen is always near-real-time). Ask the user before
    turning continuous on — it keeps the capture running until turned off.
    """
    from server.services.screen_bridge import request_mode, state

    wanted = _MODE_ALIASES.get((mode or "").strip().lower(), "")
    if not wanted:
        return "Error: mode must be one of 'on', 'off', or 'continuous'."

    result = request_mode(wanted)
    if not result.get("ok"):
        err = result.get("error") or "unknown error"
        return f"Error: cannot change screen sharing — {err}.{_hint_for(err)}"

    current = result.get("mode") or (state() or {}).get("mode", wanted)
    return f"Screen sharing is now '{current}'. The overlay preview reflects the change."
