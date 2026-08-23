"""Small, conservative context-size estimates used by the harness.

The provider tokenizer is not available in the hot path, so this module keeps a
cheap estimate.  Message estimation deliberately includes native tool-call
metadata; counting only ``message.content`` under-counts the request that is
actually sent to the model.
"""

import json


def _json_size(value) -> int:
    """Return a stable character count for arbitrary message metadata."""
    try:
        return len(json.dumps(value, ensure_ascii=False, default=str, sort_keys=True))
    except Exception:
        return len(str(value))


def estimate_tokens(text: str) -> int:
    """Conservative heuristic: roughly three characters per mixed code token."""
    if not text:
        return 0
    return max(1, len(text) // 3)


def estimate_message_tokens(message) -> int:
    """Estimate the serialized size of one LangChain or dict message.

    Tool calls, tool-call arguments, and provider metadata are part of the
    prompt even when they are not present in ``content``.  IDs and response
    metadata are intentionally omitted because they are not sent back as
    model-visible prompt content in the normal adapter path.
    """
    if isinstance(message, dict):
        payload = {
            "role": message.get("role") or message.get("type", ""),
            "content": message.get("content", ""),
            "tool_calls": message.get("tool_calls", []),
            "additional_kwargs": message.get("additional_kwargs", {}),
        }
    else:
        payload = {
            "role": type(message).__name__,
            "content": getattr(message, "content", ""),
            "tool_calls": getattr(message, "tool_calls", []),
            "additional_kwargs": getattr(message, "additional_kwargs", {}),
        }
    metadata_chars = _json_size({
        "role": payload["role"],
        "tool_calls": payload["tool_calls"],
        "additional_kwargs": payload["additional_kwargs"],
    })
    return estimate_tokens(str(payload["content"])) + max(1, metadata_chars // 3)


def estimate_messages(messages) -> int:
    """Estimate the total model-visible size of an ordered message list."""
    return sum(estimate_message_tokens(message) for message in (messages or []))
