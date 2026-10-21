"""
Chat history builder — Lane A extraction from memory/chat_context.py.
New code should import from server.context.chat_history.
Now reads via hybrid store (server.store.chat_store.get_chat) which hits SQLite+JSONL.

Important: assistant replies are persisted with ``isTrace=True`` because the UI
renders them as an activity trace. That flag must NOT hide them from the model —
they are the conversation's assistant turns. We therefore load traces and use
``content`` only, never the trace metadata.
"""
import re

_AGENT_ROLES = {"agent", "assistant"}


def clean_agent_text(text: str) -> str:
    """Strip tool/thinking envelopes from a persisted final response."""
    clean = re.sub(r'```tool\s*\n.*?\n```', '', text or '', flags=re.DOTALL)
    clean = re.sub(r'<tool>.*?</tool>', '', clean, flags=re.DOTALL)
    clean = re.sub(r'<thinking>.*?</thinking>', '', clean, flags=re.DOTALL)
    return clean.strip()


def _load_messages(workspace_path: str, chat_id: str, exclude_message_id: str = "") -> list[dict]:
    if not workspace_path or not chat_id:
        return []
    try:
        from server.store.chat_store import get_chat
    except Exception:
        from workspace_manager import get_chat  # fallback
    chat_data = get_chat(workspace_path or "_general", chat_id, include_traces=True, include_usage=False)
    if not chat_data:
        return []
    messages = chat_data.get("messages", [])
    if exclude_message_id:
        messages = [m for m in messages if m.get("id") != exclude_message_id]
    return messages


def _clip(text: str, max_chars: int) -> str:
    if len(text) <= max_chars:
        return text
    keep = max(0, max_chars - 320)
    return text[:keep] + f"\n...[{len(text) - keep - 120} chars truncated]...\n" + text[-120:]


def build_chat_context(
    workspace_path: str,
    chat_id: str,
    max_messages: int = 20,
    max_chars_per_msg: int = 2000,
    exclude_message_id: str = "",
) -> str:
    """Return a role-labelled transcript of the prior conversation.

    Includes user and assistant turns in order. The current user message can be
    excluded by id because the caller appends its own request block; otherwise
    the model sees the same instruction twice.
    """
    try:
        messages = _load_messages(workspace_path, chat_id, exclude_message_id)
        recent = messages[-max_messages:]
        lines: list[str] = []
        for m in recent:
            role = str(m.get("role") or "").lower()
            text = m.get("content") or ""
            if not text:
                continue
            if role == "user":
                lines.append(f"[USER]: {_clip(text, max_chars_per_msg)}")
            elif role in _AGENT_ROLES:
                clean = clean_agent_text(text)
                if clean:
                    lines.append(f"[ASSISTANT]: {_clip(clean, max_chars_per_msg)}")
            # system/error rows are UI-only; they are not conversation turns.
        return "\n".join(lines)
    except Exception as e:
        print(f"[CHAT CONTEXT] Error building context: {e}")
        return ""


def build_chat_history_messages(
    workspace_path: str,
    chat_id: str,
    max_messages: int = 30,
    max_chars_per_msg: int = 3000,
    exclude_message_id: str = "",
):
    """Return prior conversation as native pydantic-ai ModelMessage turns.

    Consecutive user (or assistant) messages are merged so the result is a
    provider-safe alternation. The current request is excluded by id and is
    appended by the caller as the final user turn.
    """
    try:
        from pydantic_ai.messages import (
            ModelRequest, ModelResponse, TextPart, UserPromptPart,
        )
    except Exception:
        return []

    messages = _load_messages(workspace_path, chat_id, exclude_message_id)
    recent = messages[-max_messages:]
    turns: list = []

    def flush(role: str, parts: list[str]) -> None:
        if not parts:
            return
        text = _clip("\n\n".join(parts), max_chars_per_msg)
        if role == "user":
            turns.append(ModelRequest(parts=[UserPromptPart(content=text)]))
        else:
            turns.append(ModelResponse(parts=[TextPart(content=text)]))

    buf_role = ""
    buf: list[str] = []
    try:
        for m in recent:
            role = str(m.get("role") or "").lower()
            if role not in ("user",) and role not in _AGENT_ROLES:
                continue
            text = m.get("content") or ""
            if not text:
                continue
            norm = "user" if role == "user" else "agent"
            clean = text if norm == "user" else clean_agent_text(text)
            if not clean:
                continue
            if norm != buf_role and buf:
                flush(buf_role, buf)
                buf = []
            buf_role = norm
            buf.append(clean)
        flush(buf_role, buf)
    except Exception as e:
        print(f"[CHAT CONTEXT] Error building native history: {e}")
        return []
    return turns
