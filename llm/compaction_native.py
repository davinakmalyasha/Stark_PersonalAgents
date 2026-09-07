"""compaction_native.py — ProcessHistory processor for native ModelMessage history (A7).

Replaces manual compact_history loop with pydantic-ai ProcessHistory capability.
The processor is invoked before each model request; it checks token estimate
and truncates via compact_history_native when over threshold.
"""
from __future__ import annotations

import os
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
        # Over threshold — compact. Try semantic summary via direct.model_request + FallbackModel (P2-1)
        summary = None
        if os.getenv("PAI_COMPACTION_LLM", "1") != "0":
            try:
                import asyncio
                from llm.model_history import history_to_debug_text
                from pydantic_ai import direct as _direct

                async def _summarize():
                    try:
                        from llm.free_provider import build_free_fallback
                    except Exception:
                        return None
                    # Build fallback model for compaction (prefer non-reasoning, large context)
                    try:
                        fb = build_free_fallback(capability="compaction", prefer_reasoning=False, max_context_tokens=1000000)
                    except Exception:
                        try:
                            fb = build_free_fallback(capability="coding", prefer_reasoning=False)
                        except Exception:
                            return None
                    if fb is None:
                        return None
                    debug = history_to_debug_text(messages)
                    # Truncate to 30k chars to keep prompt manageable (~7k tokens)
                    if len(debug) > 30000:
                        debug = debug[:15000] + "\n...[truncated]...\n" + debug[-15000:]
                    prompt = (
                        "Summarize the following agent conversation for context compaction. "
                        "Preserve key decisions, tool calls, file modifications, and remaining tasks. "
                        "Be concise (<=400 words).\n\n" + debug
                    )
                    from pydantic_ai.messages import ModelRequest, UserPromptPart

                    req = [ModelRequest(parts=[UserPromptPart(content=prompt)])]
                    try:
                        # 8s timeout to avoid blocking processor
                        resp = await asyncio.wait_for(
                            _direct.model_request(fb, req, model_settings={"temperature": 0.2}),
                            timeout=8.0,
                        )
                        # Extract text from ModelResponse
                        txt = ""
                        for p in getattr(resp, "parts", []) or []:
                            if getattr(p, "part_kind", "") == "text":
                                txt += str(getattr(p, "content", "") or "")
                        txt = txt.strip()
                        if txt:
                            return txt[:2000]
                    except asyncio.TimeoutError:
                        return None
                    except Exception:
                        return None
                    return None

                # Only run summarizer if we are in async context and not already over hard limit by too much
                # Use asyncio.wait_for with fallback to placeholder if no loop or timeout
                try:
                    # If already in running loop, await directly; else skip (processor is async so loop exists)
                    summary = await _summarize()
                except Exception:
                    summary = None
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
