"""tools.vision_channel — inline image hand-off from tools to agent loops.

DeepSeek's multimodal API accepts images in USER messages only (images inside
system/assistant/tool roles hard-400). Tools like ``browser_screenshot`` and
``read_file`` therefore cannot embed pixels in their ToolMessage result.
Instead they QUEUE an image here; the agent loop drains the queue right after
appending the turn's ToolMessages and injects one synthetic HumanMessage
carrying the image content blocks.

ContextVar (not a global list): tool execution runs inside the calling agent's
context — including ThreadPoolExecutor children, which inherit a copy — so each
agent loop drains only its own images and concurrent agents never cross-
contaminate.
"""
import contextvars
import uuid

pending_images: contextvars.ContextVar[list] = contextvars.ContextVar("pending_vision_images", default=None)

# Inline-image size guard: base64 payloads beyond this are skipped (the disk
# copy always exists as fallback). ~6 MB raw ≈ 8 MB base64.
MAX_INLINE_BYTES = 6 * 1024 * 1024


def queue_image(url: str, note: str = "") -> bool:
    """Queue one image (data URL or https URL) for injection at the next
    message boundary. Returns False when over the size guard."""
    if not url or not isinstance(url, str):
        return False
    # Rough guard on raw bytes for data URLs ("data:image/png;base64,....")
    if url.startswith("data:") and "," in url:
        b64 = url.split(",", 1)[1]
        if len(b64) * 3 // 4 > MAX_INLINE_BYTES:
            return False
    q = pending_images.get()
    q = list(q) if q else []
    q.append({"url": url, "note": note or "", "id": uuid.uuid4().hex[:8]})
    pending_images.set(q)
    return True


def queue_image_file(path: str, mime: str = "", note: str = "") -> bool:
    """Queue an image file by reading it as a base64 data URL."""
    import base64
    import os
    try:
        if not os.path.isfile(path):
            return False
        if os.path.getsize(path) > MAX_INLINE_BYTES:
            return False
        ext = os.path.splitext(path)[1].lower().lstrip(".")
        mime_map = {"png": "image/png", "jpg": "image/jpeg", "jpeg": "image/jpeg",
                    "gif": "image/gif", "webp": "image/webp"}
        mt = mime or mime_map.get(ext)
        if not mt:
            return False
        with open(path, "rb") as f:
            b64 = base64.b64encode(f.read()).decode("ascii")
        return queue_image(f"data:{mt};base64,{b64}", note=note or os.path.basename(path))
    except Exception:
        return False


def drain_images() -> list[dict]:
    """Atomically take all queued images (returns [], then resets)."""
    q = pending_images.get()
    if not q:
        return []
    pending_images.set([])
    return q


def build_image_message(images: list[dict], header: str = "[Inline image(s) from tool result]") -> list:
    """Build ONE HumanMessage whose content carries a short text part plus the
    queued image blocks. Returns [message] or [] — safe to splat into messages."""
    if not images:
        return []
    from langchain_core.messages import HumanMessage
    parts = [{"type": "text", "text": header}]
    for img in images:
        if img.get("note"):
            parts[0]["text"] += f"\n- {img['note']}"
        parts.append({"type": "image_url", "image_url": {"url": img["url"]}})
    try:
        return [HumanMessage(content=parts)]
    except Exception:
        return []


def drain_into_messages(messages: list, header: str = "[Inline image(s) from tool result]") -> int:
    """Convenience for agent loops: drain the queue and append the synthetic
    user message. Returns the number of images injected."""
    imgs = drain_images()
    if not imgs:
        return 0
    msgs = build_image_message(imgs, header)
    if msgs:
        messages.extend(msgs)
        return len(imgs)
    return 0


def drain_attachment_images(messages: list, cap: int = 4) -> int:
    """Zone-2 builder helper: take image paths stashed by the API layer under
    shared_state['attachment_image_paths'] and inject them as ONE user message
    BEFORE the first LLM call, so user-uploaded pictures are visible from turn
    one. Consumes-and-clears exactly once (first agent to build Zone-2 wins —
    the pre-router sends a run to either Orchestrator or Dev, never both).

    Plain threads do not inherit ContextVars, so the API layer cannot queue
    through this module's ContextVar; shared_state is the reliable channel.
    """
    try:
        from sync.state_sync import safe_get_state, safe_update_state
        paths = safe_get_state().get("attachment_image_paths") or []
        if not paths:
            return 0
        safe_update_state({"attachment_image_paths": []})
        injected = 0
        for p in paths[:max(0, int(cap))]:
            if queue_image_file(p, note="user attachment"):
                injected += 1
        if injected:
            return drain_into_messages(messages, header="[User attached image(s)]")
    except Exception:
        pass
    return 0