"""
Screen router — live screen-sharing frames from the Electron voice overlay.

The overlay's "look at screen" mode captures the user's selected screen or
window and POSTs JPEG frames here. Frames are written under
``.deep_agents/live_screen/<chat>/``:

  frames/<ts>_<rand>.<ext>   one file per captured frame
  latest.<ext>               rolling "most recent" copy (overwritten)

The response ``attachment`` is a ready-made entry for ``POST /api/run``: it
flows through the existing attachment → vision_channel path into the agent's
next user message. ``screenFrame`` marks it so runs.py keeps live frames out
of the persisted chat log.

  POST /api/vision/frame     {image_b64, mime, chat_id, label, source, req_id}
                             → {ok, attachment}
"""
import base64
import os
import re
import time
import uuid

from fastapi import APIRouter
from fastapi.responses import JSONResponse
from pydantic import BaseModel

router = APIRouter(tags=["screen"])

_ROOT = os.environ.get("HARNESS_ROOT") or os.path.dirname(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
)
_SCREEN_DIR = os.path.join(_ROOT, ".deep_agents", "live_screen")

_MAX_FRAME_BYTES = 8 * 1024 * 1024
_PRUNE_MAX_AGE_S = 48 * 3600
_PRUNE_INTERVAL_S = 3600
_MIME_EXT = {
    "image/jpeg": "jpg",
    "image/jpg": "jpg",
    "image/png": "png",
    "image/webp": "webp",
}
_EXT_MIME = {ext: mime for mime, ext in _MIME_EXT.items()}
_EXT_MIME["jpg"] = "image/jpeg"
_LABEL_NOTES = {
    "turn": "Live screen — what the user was viewing when they spoke",
    "continuous": "Live screen (continuous share) — the user's current view",
    "tool": "Live screen — captured on request",
}

_last_prune = 0.0


class ScreenFrameRequest(BaseModel):
    image_b64: str
    mime: str = "image/jpeg"
    chat_id: str = ""
    label: str = "turn"
    source: str = ""
    req_id: str = ""


@router.post("/api/vision/frame")
async def receive_screen_frame(req: ScreenFrameRequest):
    """Save one live screen frame and return it as a /api/run attachment."""
    mime = (req.mime or "").strip().lower()
    ext = _MIME_EXT.get(mime)
    if ext is None:
        return JSONResponse(status_code=400, content={"ok": False, "error": f"Unsupported mime '{req.mime}'"})
    try:
        raw = base64.b64decode(req.image_b64, validate=True)
    except Exception:
        return JSONResponse(status_code=400, content={"ok": False, "error": "image_b64 is not valid base64"})
    if not raw:
        return JSONResponse(status_code=400, content={"ok": False, "error": "empty frame"})
    if len(raw) > _MAX_FRAME_BYTES:
        return JSONResponse(status_code=400, content={"ok": False, "error": "frame larger than 8 MB"})

    try:
        attachment = _save_frame(raw, ext, req)
    except Exception as e:
        return JSONResponse(status_code=500, content={"ok": False, "error": f"failed to store frame: {e}"})

    if req.req_id:
        try:
            from server.services.screen_bridge import resolve_frame
            resolve_frame(req.req_id, attachment)
        except Exception:
            pass
    return JSONResponse(content={"ok": True, "attachment": attachment})


# ── storage ────────────────────────────────────────────────────────────────


def _save_frame(raw: bytes, ext: str, req: ScreenFrameRequest) -> dict:
    folder = os.path.join(_SCREEN_DIR, _safe_name(req.chat_id or "_default"))
    latest = os.path.join(folder, f"latest.{ext}")
    os.makedirs(folder, exist_ok=True)
    _maybe_prune()

    # Continuous mode runs every few seconds: only the rolling latest copy is
    # kept (no per-frame file churn — ~60 MB/h otherwise). Turn/tool captures
    # are kept as history.
    if req.label == "continuous":
        _replace(latest, raw)
        path = latest
        name = f"latest.{ext}"
    else:
        frames_dir = os.path.join(folder, "frames")
        os.makedirs(frames_dir, exist_ok=True)
        name = f"{int(time.time() * 1000)}_{uuid.uuid4().hex[:4]}.{ext}"
        path = os.path.join(frames_dir, name)
        with open(path, "wb") as handle:
            handle.write(raw)
        _replace(latest, raw)

    note = _LABEL_NOTES.get(req.label, _LABEL_NOTES["turn"])
    if req.source:
        note += f" (source: {req.source})"
    _last_frame_state(path, latest)
    return {
        "id": name,
        "name": f"screen_{name}",
        "type": _EXT_MIME.get(ext, "image/jpeg"),
        "size": len(raw),
        "path": path,
        "isImage": True,
        "screenFrame": True,
        "note": note,
    }


def _safe_name(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9_.-]+", "_", value.strip())[:80] or "_default"


def _replace(path: str, raw: bytes) -> None:
    """Atomic overwrite (readers never see a half-written latest frame)."""
    tmp = path + ".tmp"
    try:
        with open(tmp, "wb") as handle:
            handle.write(raw)
        os.replace(tmp, path)
    except Exception:
        try:
            if os.path.isfile(tmp):
                os.remove(tmp)
        except Exception:
            pass


def _maybe_prune() -> None:
    """Drop frame files older than 48h across all chats, at most once/hour."""
    global _last_prune
    now = time.time()
    if now - _last_prune < _PRUNE_INTERVAL_S:
        return
    _last_prune = now
    cutoff = now - _PRUNE_MAX_AGE_S
    try:
        for chat in os.listdir(_SCREEN_DIR):
            frames_dir = os.path.join(_SCREEN_DIR, chat, "frames")
            if not os.path.isdir(frames_dir):
                continue
            for name in os.listdir(frames_dir):
                path = os.path.join(frames_dir, name)
                try:
                    if os.path.getmtime(path) < cutoff:
                        os.remove(path)
                except Exception:
                    pass
    except Exception:
        pass


def _last_frame_state(frame_path: str, latest_path: str) -> None:
    """Expose the newest frame in shared state (observability + continuous mode)."""
    try:
        from sync.state_sync import safe_update_state
        safe_update_state({
            "live_screen_last_frame": frame_path,
            "live_screen_latest": latest_path,
            "live_screen_last_at": time.time(),
        })
    except Exception:
        pass
