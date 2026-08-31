"""compaction_native.py — ProcessHistory processor for native ModelMessage history (A7).

Replaces manual compact_history loop with pydantic-ai ProcessHistory capability.
The processor is invoked before each model request; it checks token estimate
and truncates via compact_history_native when over threshold.
"""
from __future__ import annotations

from typing import Any

from pydantic_ai.messages import ModelMessage


async def native_history_processor(*args: Any, **kwargs: Any) -> list[ModelMessage]:
    """HistoryProcessor for ProcessHistory — truncates when over checkpoint.

    Accepts both (ctx, messages) and (messages) calling conventions to be
    robust to pydantic-ai's takes_run_context detection.
    """
    # Normalize args: handle (ctx, messages) or (messages)
    ctx: Any = None
    messages: list[ModelMessage] | None = None
    if len(args) == 2:
        # Could be (ctx, messages) or (messages, ctx) — detect by deps attr
        if hasattr(args[0], "deps") and isinstance(args[1], list):
            ctx, messages = args[0], args[1]
        elif hasattr(args[1], "deps") and isinstance(args[0], list):
            messages, ctx = args[0], args[1]
        else:
            # Assume (ctx, messages)
            ctx, messages = args[0], args[1]
    elif len(args) == 1:
        if isinstance(args[0], list):
            messages = args[0]
            ctx = kwargs.get("ctx")
        else:
            ctx = args[0]
            messages = kwargs.get("messages") or kwargs.get("history")
    else:
        messages = kwargs.get("messages") or kwargs.get("history") or []
        ctx = kwargs.get("ctx")
    if messages is None:
        messages = []
    try:
        from llm.model_history import compact_history_native, estimate_history_tokens
        from memory.context_compaction import get_compaction_policy

        role = getattr(getattr(ctx, "deps", None), "role", "") or "developer"
        # Normalize role: Orchestrator -> orchestrator, Developer -> developer, etc.
        role_key = str(role).lower() if isinstance(role, str) else "developer"
        # Map WorkspaceDeps roles to policy keys
        if role_key not in ("developer", "orchestrator", "general", "house", "dfd"):
            role_key = "developer"
        policy = get_compaction_policy(role_key)
        est = estimate_history_tokens(messages)
        if est < policy.checkpoint_trigger and est < policy.hard_limit:
            return messages
        # Over threshold — compact. Try to get a semantic summary via free pool
        # if available; fallback to placeholder.
        summary = None
        try:
            # Use free pool compactor for semantic summary (best-effort, non-blocking)
            from memory.context_compaction import _build_free_compaction_summary
            # _build_free_compaction_summary expects LC messages; for native we
            # pass a debug text version via history_to_debug_text
            from llm.model_history import history_to_debug_text
            # Build a pseudo-LC list for the compactor's token estimation
            # (we just need a summary string; use history_to_debug_text as payload)
            debug = history_to_debug_text(messages)
            # Call free compactor with a truncated debug payload (cap 20k chars)
            # to avoid context overflow. This is best-effort; if it fails we
            # fall back to placeholder.
            summary = None  # keep placeholder for now to avoid free pool latency in processor
        except Exception:
            summary = None
        if summary is None:
            summary = "[Native compaction — earlier turns summarized due to length]"
        # Preserve full project map if present in history (look for <current-context>)
        full_map = None
        for m in messages:
            for p in getattr(m, "parts", []) or []:
                try:
                    from pydantic_ai.messages import UserPromptPart
                    if isinstance(p, UserPromptPart) and isinstance(p.content, str) and "<current-context>" in p.content:
                        # Extract map block
                        full_map = p.content
                        break
                except Exception:
                    continue
        return compact_history_native(messages, policy=policy, full_project_map=full_map, summary_text=summary)
    except Exception:
        # Never break the run on compaction error
        return messages


def get_process_history_capability():
    """Return ProcessHistory capability wrapping native_history_processor."""
    try:
        from pydantic_ai.capabilities.process_history import ProcessHistory
        return ProcessHistory(processor=native_history_processor)
    except Exception:
        return None
