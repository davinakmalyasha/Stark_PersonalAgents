"""
model_history.py — native pydantic-ai ModelMessage history helpers (A6b).

Replaces the LangChain message storage pattern with pydantic-ai's
ModelMessage + ModelMessagesTypeAdapter JSON persistence.

Design per fixed-concept.md A6b:
- History is List[ModelMessage] (ModelRequest | ModelResponse)
- Persistence via ModelMessagesTypeAdapter (JSON round-trip)
- No LangChain SystemMessage/HumanMessage/AIMessage/ToolMessage in new paths
- Legacy LC / OpenAI-dict transcripts convert at the boundary via
  ``pai_transport._messages_to_pai`` (P0-LLM).

Usage:
  from llm.model_history import dump_history, load_history, build_initial_history
  history: list[ModelMessage] = build_initial_history(system_prompt, user_task)
  # pass to Agent.run(message_history=history)
  json_blob = dump_history(history)
  restored = load_history(json_blob)
"""
from __future__ import annotations

import json
from typing import Any

from pydantic_ai import ModelMessagesTypeAdapter
from pydantic_ai.messages import (
    ModelMessage,
    ModelRequest,
    ModelResponse,
    SystemPromptPart,
    TextPart,
    ThinkingPart,
    ToolCallPart,
    ToolReturnPart,
    UserPromptPart,
)


def dump_history(messages: list[ModelMessage]) -> str:
    """Serialize ModelMessage history to JSON (for task.json / session store)."""
    return ModelMessagesTypeAdapter.dump_json(messages).decode("utf-8")


def load_history(json_str: str) -> list[ModelMessage]:
    """Deserialize JSON blob back to List[ModelMessage]."""
    if not json_str:
        return []
    return ModelMessagesTypeAdapter.validate_json(json_str)


def dump_history_python(messages: list[ModelMessage]) -> list[dict]:
    """Python-object dump (for debugging / JSONL)."""
    return ModelMessagesTypeAdapter.dump_python(messages, mode="json")  # type: ignore


def load_history_python(data: list[dict]) -> list[ModelMessage]:
    """Validate from python objects."""
    return ModelMessagesTypeAdapter.validate_python(data)


def build_initial_history(
    system_prompt: str,
    user_task: str,
    *,
    chat_id: str = "",
) -> list[ModelMessage]:
    """Build the initial ModelMessage history: system + user."""
    history: list[ModelMessage] = []
    if system_prompt:
        history.append(ModelRequest(parts=[SystemPromptPart(content=system_prompt)]))
    if user_task:
        history.append(ModelRequest(parts=[UserPromptPart(content=user_task)]))
    return history


def append_user_history(history: list[ModelMessage], content: str) -> list[ModelMessage]:
    """Append a user turn to existing history (returns new list)."""
    return history + [ModelRequest(parts=[UserPromptPart(content=content)])]


def get_system_prompt(history: list[ModelMessage]) -> str | None:
    """Extract system prompt from history if present."""
    for m in history:
        if isinstance(m, ModelRequest):
            for p in m.parts:
                if isinstance(p, SystemPromptPart):
                    return p.content
    return None


def estimate_history_tokens(history: list[ModelMessage]) -> int:
    """Rough token estimate: ~4 chars per token."""
    total_chars = 0
    for m in history:
        for p in getattr(m, "parts", []):
            content = getattr(p, "content", "") or ""
            if isinstance(content, str):
                total_chars += len(content)
            elif isinstance(content, (list, dict)):
                total_chars += len(json.dumps(content))
            # ToolCallPart.args may be dict
            args = getattr(p, "args", None)
            if isinstance(args, dict):
                total_chars += len(json.dumps(args))
            elif isinstance(args, str):
                total_chars += len(args)
    return max(1, total_chars // 4)


def _content_to_text(value: Any) -> str:
    """Full-fidelity part content for the compaction renderer.

    No character caps: the whole point of v2 compaction is that the summarizer
    sees everything. Only binary payloads are replaced by placeholders.
    """
    if isinstance(value, str):
        return value
    if isinstance(value, (list, tuple)):
        out = [_content_to_text(item) for item in value]
        return "\n".join(part for part in out if part)
    if isinstance(value, dict):
        kind = str(value.get("type") or "")
        if "image" in kind or "audio" in kind or "file" in kind:
            name = value.get("filename") or value.get("name") or ""
            return f"[Attached {kind or 'media'}{': ' + str(name) if name else ''}]"
        if isinstance(value.get("text"), str):
            return value["text"]
        try:
            return json.dumps(value, ensure_ascii=False)
        except Exception:
            return str(value)
    return str(value or "")


def render_history_blocks(history: list[ModelMessage]) -> list[str]:
    """Render the FULL transcript as one block per message, uncapped.

    Used by compaction: the summarizer must see everything, including tool
    arguments/results and reasoning that ``history_to_debug_text`` truncates.
    Blocks are the chunking unit, so a message is never split unless it alone
    exceeds the compactor window.
    """
    blocks: list[str] = []
    for message in history:
        lines: list[str] = []
        for part in getattr(message, "parts", []) or []:
            kind = getattr(part, "part_kind", "")
            if kind == "system-prompt":
                lines.append("[SYSTEM]\n" + _content_to_text(getattr(part, "content", "")))
            elif kind == "user-prompt":
                lines.append("[USER]\n" + _content_to_text(getattr(part, "content", "")))
            elif kind == "text":
                lines.append("[ASSISTANT]\n" + _content_to_text(getattr(part, "content", "")))
            elif kind == "thinking":
                lines.append("[ASSISTANT REASONING]\n" + _content_to_text(getattr(part, "content", "")))
            elif kind == "tool-call":
                name = getattr(part, "tool_name", "") or ""
                args = getattr(part, "args", "")
                lines.append(f"[TOOL CALL {name}]\n" + _content_to_text(args))
            elif kind == "tool-return":
                name = getattr(part, "tool_name", "") or ""
                lines.append(f"[TOOL RESULT {name}]\n" + _content_to_text(getattr(part, "content", "")))
            elif kind == "retry-prompt":
                lines.append("[RETRY]\n" + _content_to_text(getattr(part, "content", "")))
            else:
                content = getattr(part, "content", "")
                rendered = _content_to_text(content)
                if rendered:
                    lines.append(f"[{kind.upper() or 'PART'}]\n{rendered}")
        block = "\n\n".join(line for line in lines if line)
        if block.strip():
            blocks.append(block)
    return blocks


def render_full_history(history: list[ModelMessage]) -> str:
    """The entire transcript as plain text (no caps) for the summarizer."""
    return "\n\n".join(render_history_blocks(history))


def history_to_debug_text(history: list[ModelMessage]) -> str:
    """Render history as plain text for logging / compaction summary."""
    lines: list[str] = []
    for m in history:
        if isinstance(m, ModelRequest):
            for p in m.parts:
                if isinstance(p, SystemPromptPart):
                    lines.append(f"[SYSTEM] {p.content[:200]}")
                elif isinstance(p, UserPromptPart):
                    content = p.content
                    if isinstance(content, str):
                        lines.append(f"[USER] {content[:500]}")
                    else:
                        lines.append(f"[USER] {str(content)[:500]}")
                elif isinstance(p, ToolReturnPart):
                    lines.append(f"[TOOL {p.tool_name}] {str(p.content)[:300]}")
        elif isinstance(m, ModelResponse):
            for p in m.parts:
                if isinstance(p, TextPart):
                    lines.append(f"[ASSISTANT] {p.content[:500]}")
                elif isinstance(p, ThinkingPart):
                    lines.append(f"[THINKING] {p.content[:300]}")
                elif isinstance(p, ToolCallPart):
                    lines.append(f"[CALL {p.tool_name} {p.tool_call_id}] {str(p.args)[:300]}")
    return "\n".join(lines)


# ── Native compaction helpers (A6b/A7) ────────────────────────────────────

#: Paid canonical models with their known context windows.
_PAID_WINDOWS = {
    "deepseek-v4.1-flash": 1_000_000,
    "deepseek-v4.1-vision": 1_000_000,
    "deepseek-v4-pro": 1_000_000,
}


def window_for_model(model_id: str) -> int:
    """Best-effort context window for a model id (0 = unknown, no clamp).

    Free-pool models resolve from config/free_catalog.json; paid canonical
    slugs use the static map above. Callers clamp the compaction trigger to
    this value so a 256k model compacts long before the provider rejects.
    """
    m = str(model_id or "").strip().lower()
    if not m:
        return 0
    for key, win in _PAID_WINDOWS.items():
        if key in m:
            return int(win)
    try:
        from llm.free_providers import model_context_window

        return int(model_context_window(m, 0) or 0)
    except Exception:
        return 0


def compact_history_native(
    history: list[ModelMessage],
    *,
    policy=None,
    full_project_map: str | None = None,
    summary_text: str | None = None,
    project_memory_text: str | None = None,
    user_messages: list[str] | None = None,
) -> list[ModelMessage]:
    """Rebuild history in the Codex compaction shape:

        [System, project-memory, <current-context>map, ...user messages..., summary]

    The summary is LAST because the model is trained to continue from a
    handoff summary in last position. Only user asks are retained verbatim
    (bounded by ``COMPACT_USER_MESSAGE_MAX_TOKENS`` upstream); assistant and
    tool turns live on inside the summary. project.md and the map are always
    re-anchored fresh and are never summarized.
    """
    if len(history) <= 2:
        return history
    system_msg = history[0] if history and isinstance(history[0], ModelRequest) else None
    compacted: list[ModelMessage] = []
    if system_msg is not None:
        compacted.append(system_msg)
    if project_memory_text:
        compacted.append(ModelRequest(parts=[UserPromptPart(content=(
            "<project-memory>\n" + project_memory_text + "\n</project-memory>"
        ))]))
    if full_project_map:
        compacted.append(ModelRequest(parts=[UserPromptPart(content=f"<current-context>\n{full_project_map}\n</current-context>")]))
    if user_messages is None:
        try:
            from memory.compaction_engine import collect_retained_user_messages
            user_messages = collect_retained_user_messages(history)
        except Exception:
            user_messages = []
    for text in user_messages or []:
        if str(text or "").strip():
            compacted.append(ModelRequest(parts=[UserPromptPart(content=str(text))]))
    # Summary LAST (Codex: the resuming model continues from the handoff).
    summary = summary_text or "[Conversation truncated for length — earlier turns summarized]"
    compacted.append(ModelRequest(parts=[UserPromptPart(content=summary)]))
    return compacted
