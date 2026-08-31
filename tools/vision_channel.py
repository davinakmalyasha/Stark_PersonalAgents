"""tools.vision_channel — inline image hand-off from tools to agent loops.

DeepSeek's multimodal API accepts images in USER messages only (images inside
system/assistant/tool roles hard-400). Tools like ``browser_screenshot`` and
``read_file`` therefore cannot embed pixels in their ToolMessage result.
Instead they QUEUE an image here; the agent loop drains the queue right after
appending the turn's ToolMessages and injects one synthetic user message
carrying the image content blocks.

The golden-standard channel is ``WorkspaceDeps.pending_images`` — the same
per-run typed context that already flows into every tool (bound as the
ctx/deps parameter) and every agent loop. Tools queue into deps.pending_images;
loops drain it after appending the turn's tool results and inject the image as
a native UserPromptPart (BinaryContent/ImageUrl). No ContextVar, no
shared_state double-channel.

The ContextVar channel is KEPT as a legacy fallback for the free-pool path and
non-deps tool callers (raw ``execute_tool_detailed``); the shared_state
attachment stash is consumed via the same queue so exactly one path drains it.
"""
import uuid

# Legacy ContextVar channel — retained for non-deps tool callers (free pool).
import contextvars
pending_images: contextvars.ContextVar[list] = contextvars.ContextVar("pending_vision_images", default=None)

# Inline-image size guard: base64 payloads beyond this are skipped (the disk
# copy always exists as fallback). ~6 MB raw ≈ 8 MB base64.
MAX_INLINE_BYTES = 6 * 1024 * 1024


def _deps_or_ctx(ctx=None) -> list:
    """Return the mutable pending_images list for the active channel.

    Prefers WorkspaceDeps.pending_images (golden standard) when the caller
    passed deps; falls back to the legacy ContextVar.
    """
    if ctx is not None and hasattr(ctx, "pending_images"):
        lst = ctx.pending_images
        if lst is None:
            lst = []
            ctx.pending_images = lst
        return lst
    q = pending_images.get()
    if q is None:
        q = []
        pending_images.set(q)
    return q


def queue_image(url: str, note: str = "", ctx=None) -> bool:
    """Queue one image (data URL or https URL) for injection at the next
    message boundary. Returns False when over the size guard.

    ``ctx`` is the optional WorkspaceDeps — when provided the image lands in
    the typed per-run channel (golden standard); otherwise the legacy
    ContextVar channel is used.
    """
    if not url or not isinstance(url, str):
        return False
    # Rough guard on raw bytes for data URLs ("data:image/png;base64,....")
    if url.startswith("data:") and "," in url:
        b64 = url.split(",", 1)[1]
        if len(b64) * 3 // 4 > MAX_INLINE_BYTES:
            return False
    q = _deps_or_ctx(ctx)
    q.append({"url": url, "note": note or "", "id": uuid.uuid4().hex[:8]})
    return True


def queue_image_file(path: str, mime: str = "", note: str = "", ctx=None) -> bool:
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
        return queue_image(f"data:{mt};base64,{b64}", note=note or os.path.basename(path), ctx=ctx)
    except Exception:
        return False


def drain_images(ctx=None) -> list[dict]:
    """Atomically take all queued images (returns [], then resets)."""
    if ctx is not None and hasattr(ctx, "pending_images"):
        q = ctx.pending_images or []
        ctx.pending_images = []
        return q
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


def drain_into_messages(messages: list, header: str = "[Inline image(s) from tool result]", ctx=None) -> int:
    """Convenience for agent loops: drain the queue and append the synthetic
    user message. Returns the number of images injected."""
    imgs = drain_images(ctx=ctx)
    if not imgs:
        return 0
    msgs = build_image_message(imgs, header)
    if msgs:
        messages.extend(msgs)
        return len(imgs)
    return 0


def drain_attachment_images(messages: list, cap: int = 4, ctx=None) -> int:
    """Zone-2 builder helper: take image paths stashed by the API layer under
    shared_state['attachment_image_paths'] and inject them as ONE user message
    BEFORE the first LLM call, so user-uploaded pictures are visible from turn
    one. Consumes-and-clears exactly once (first agent to build Zone-2 wins —
    the pre-router sends a run to either Orchestrator or Dev, never both).

    The shared_state stash is a one-time consumption channel (the API router
    writes it); the images themselves land in the deps channel when ``ctx`` is
    provided, else the legacy ContextVar.
    """
    try:
        from sync.state_sync import safe_get_state, safe_update_state
        paths = safe_get_state().get("attachment_image_paths") or []
        if not paths:
            return 0
        safe_update_state({"attachment_image_paths": []})
        injected = 0
        for p in paths[:max(0, int(cap))]:
            if queue_image_file(p, note="user attachment", ctx=ctx):
                injected += 1
        if injected:
            return drain_into_messages(messages, header="[User attached image(s)]", ctx=ctx)
    except Exception:
        pass
    return 0


# ── Native ModelMessage helpers (A6b) ─────────────────────────────────────

def build_image_message_native(images: list[dict], header: str = "[Inline image(s) from tool result]") -> list:
    """Build ONE ModelRequest whose UserPromptPart carries text + images (A6b).

    Returns [ModelRequest] or [] — safe to extend onto history: list[ModelMessage].
    Uses BinaryContent.from_data_uri for data: URLs and ImageUrl for https.
    """
    if not images:
        return []
    try:
        from pydantic_ai.messages import BinaryContent, ImageUrl, ModelRequest, UserPromptPart
        text = header
        for img in images:
            if img.get("note"):
                text += f"\n- {img['note']}"
        content: list = [text]
        for img in images:
            url = img.get("url") or ""
            if url.startswith("data:"):
                try:
                    content.append(BinaryContent.from_data_uri(url))
                except Exception:
                    content.append(url)
            elif url.startswith("http"):
                try:
                    content.append(ImageUrl(url=url))
                except Exception:
                    content.append(url)
            else:
                content.append(url)
        # If only text + no images parsed, keep text-only
        if len(content) == 1:
            return [ModelRequest(parts=[UserPromptPart(content=text)])]
        return [ModelRequest(parts=[UserPromptPart(content=content)])]
    except Exception:
        return []


def drain_into_messages_native(history: list, header: str = "[Inline image(s) from tool result]", ctx=None) -> int:
    """Drain queued images into native history (ModelMessage). Returns count.

    ``ctx`` is the optional WorkspaceDeps — when provided the images are drained
    from the typed per-run channel (golden standard); otherwise the legacy
    ContextVar channel.
    """
    imgs = drain_images(ctx=ctx)
    if not imgs:
        return 0
    msgs = build_image_message_native(imgs, header)
    if msgs:
        history.extend(msgs)
        return len(imgs)
    return 0


def drain_attachment_images_native(history: list, cap: int = 4, ctx=None) -> int:
    """Native variant of drain_attachment_images for ModelMessage history."""
    try:
        from sync.state_sync import safe_get_state, safe_update_state
        paths = safe_get_state().get("attachment_image_paths") or []
        if not paths:
            return 0
        safe_update_state({"attachment_image_paths": []})
        injected = 0
        for p in paths[:max(0, int(cap))]:
            if queue_image_file(p, note="user attachment", ctx=ctx):
                injected += 1
        if injected:
            return drain_into_messages_native(history, header="[User attached image(s)]", ctx=ctx)
    except Exception:
        pass
    return 0