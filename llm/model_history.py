"""
model_history.py — native pydantic-ai ModelMessage history helpers (A6b).

Replaces the LangChain message storage pattern with pydantic-ai's
ModelMessage + ModelMessagesTypeAdapter JSON persistence.

Design per fixed-concept.md A6b:
- History is List[ModelMessage] (ModelRequest | ModelResponse)
- Persistence via ModelMessagesTypeAdapter (JSON round-trip)
- No LangChain SystemMessage/HumanMessage/AIMessage/ToolMessage in new paths
- Converters _lc_messages_to_pai / _pai_to_lc remain for backward compat
  but new code builds ModelMessage directly.

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

def _estimate_single(msg: ModelMessage) -> int:
    """Token estimate for one ModelMessage (4 chars/token)."""
    total = 0
    for p in getattr(msg, "parts", []) or []:
        c = getattr(p, "content", "") or ""
        if isinstance(c, str):
            total += len(c)
        elif isinstance(c, (list, dict)):
            total += len(json.dumps(c))
        args = getattr(p, "args", None)
        if isinstance(args, dict):
            total += len(json.dumps(args))
        elif isinstance(args, str):
            total += len(args)
        # ToolReturnPart.content may be large
        if isinstance(p, ToolReturnPart):
            total += len(str(p.content or ""))
    return max(1, total // 4)


def select_tail_history(
    history: list[ModelMessage],
    budget_tokens: int = 20000,
    keep_recent_turns: int = 2,
    min_keep_messages: int = 3,
) -> list[int]:
    """Select tail indices for native history (mirrors context_compaction.select_tail_indices).

    Never includes index 0 (system). Keeps whole messages as atomic units
    (ModelMessage already groups ToolCall/ToolReturn correctly).
    """
    if len(history) <= 2:
        return list(range(len(history)))
    # Exclude system prompt at 0
    body_start = 1
    if len(history) <= body_start:
        return list(range(len(history)))
    # Greedy from end while budget allows
    selected: list[int] = []
    total = 0
    for idx in range(len(history) - 1, body_start - 1, -1):
        est = _estimate_single(history[idx])
        if total + est <= budget_tokens or len(selected) < min_keep_messages:
            selected.append(idx)
            total += est
            if len(selected) >= keep_recent_turns and total >= budget_tokens:
                break
        elif total + est > budget_tokens and len(selected) >= min_keep_messages:
            break
        else:
            selected.append(idx)
            total += est
    selected.sort()
    # Ensure at least min_keep_messages if possible
    if len(selected) < min_keep_messages and len(history) > 1:
        selected = list(range(max(1, len(history) - min_keep_messages), len(history)))
    return selected


def compact_history_native(
    history: list[ModelMessage],
    *,
    policy=None,
    full_project_map: str | None = None,
    summary_text: str | None = None,
) -> list[ModelMessage]:
    """Native counterpart to checkpoint_compact for ModelMessage history.

    Rebuilds [System, <current-context>fullMap?, summary, tail] when history
    exceeds policy tail budget. Used by ProcessHistory or manual compaction.
    """
    if len(history) <= 2:
        return history
    # Resolve policy
    try:
        from memory.context_compaction import get_compaction_policy
        pol = policy or get_compaction_policy("developer")
    except Exception:
        pol = None
    tail_budget = getattr(pol, "tail_budget_tokens", 20000) if pol else 20000
    tail_indices = select_tail_history(history, budget_tokens=tail_budget)
    # Tail messages (exclude system + project map)
    tail = [history[i] for i in tail_indices if i != 0]
    # System prompt (preserve)
    system_msg = history[0] if history and isinstance(history[0], ModelRequest) else None
    compacted: list[ModelMessage] = []
    if system_msg is not None:
        compacted.append(system_msg)
    if full_project_map:
        compacted.append(ModelRequest(parts=[UserPromptPart(content=f"<current-context>\n{full_project_map}\n</current-context>")]))
    # Summary
    summary = summary_text or "[Conversation truncated for length — earlier turns summarized]"
    compacted.append(ModelRequest(parts=[UserPromptPart(content=f"[COMPACTION SUMMARY]\n{summary}")]))
    compacted.extend(tail)
    return compacted
