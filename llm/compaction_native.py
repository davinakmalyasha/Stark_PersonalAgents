"""compaction_native.py — ProcessHistory processor for native ModelMessage history (A7).

The processor is invoked before each model request; when the token estimate
crosses the checkpoint trigger it delegates to the shared Codex-parity engine
(``memory.compaction_engine``): the ENTIRE transcript is summarized (chunked
and rolled when the pinned compactor window cannot fit it in one request), the
project.md checker runs, and the map is re-anchored fresh. History is rebuilt
Codex-style — repo context, then retained user asks, then the summary LAST.
"""
from __future__ import annotations

import dataclasses
import os
from typing import Any

from pydantic_ai.messages import ModelMessage, ModelResponse
from pydantic_ai.tools import RunContext


def policy_for_deps(deps: Any):
    """Compaction policy for one run's deps.

    Precedence: shared policy keyed by the real role name (specialists keep
    their own key), then the registry spec's ``compaction`` overrides, then a
    window clamp (75% trigger / 85% hard) so a sub-400k model compacts before
    the provider rejects the request (a 256k laguna specialist triggers ~196k).
    """
    from memory.context_compaction import CompactionPolicy, get_compaction_policy

    role = getattr(deps, "role", "") or "developer"
    policy = get_compaction_policy(str(role) or "developer")

    overrides = getattr(deps, "compaction", None) or {}
    if overrides:
        def _ov(name: str, fallback: int) -> int:
            try:
                value = int(overrides.get(name) or 0)
                return value if value > 0 else fallback
            except Exception:
                return fallback

        policy = CompactionPolicy(
            role=policy.role,
            checkpoint_trigger=_ov("checkpoint_tokens", policy.checkpoint_trigger),
            hard_limit=_ov("hard_limit", policy.hard_limit),
            summary_output_tokens=_ov("summary_output_tokens", policy.summary_output_tokens),
        )

    return clamp_policy(policy, int(getattr(deps, "context_window", 0) or 0))


def clamp_policy(policy, window: int):
    """Clamp a compaction policy below a model window (75% trigger / 85% hard)."""
    window = int(window or 0)
    if window <= 0:
        return policy
    from memory.context_compaction import CompactionPolicy

    return CompactionPolicy(
        role=policy.role,
        checkpoint_trigger=min(policy.checkpoint_trigger, max(4096, int(window * 0.75))),
        hard_limit=min(policy.hard_limit, max(8192, int(window * 0.85))),
        summary_output_tokens=policy.summary_output_tokens,
    )


def _trim_stale_thinking(messages: list[ModelMessage]) -> list[ModelMessage]:
    """Drop old chain-of-thought from history while keeping the API contract.

    Reasoning models bill prior thinking as cached input on every later request
    (DeepSeek re-sends ``reasoning_content`` in each assistant turn). Every
    major harness (Codex reasoning summaries, Anthropic ephemeral thinking,
    OpenCode compaction) keeps old CoT OUT of active context — the actions it
    produced already live in the tool results. This mirrors that: only the
    LATEST assistant response keeps its thinking verbatim (it is the one the
    model is still acting on); older ThinkingParts are emptied. The part stays
    so the provider's field contract holds — pydantic-ai itself backfills the
    field with an empty string for DeepSeek, so empty is API-legal.
    """
    try:
        last_thinking_idx = -1
        for i, m in enumerate(messages):
            if isinstance(m, ModelResponse):
                for p in getattr(m, "parts", []) or []:
                    if getattr(p, "part_kind", "") == "thinking" and getattr(p, "content", ""):
                        last_thinking_idx = i
                        break
        if last_thinking_idx < 0:
            return messages
        out = list(messages)
        for i in range(last_thinking_idx):
            m = out[i]
            if not isinstance(m, ModelResponse):
                continue
            for j, p in enumerate(m.parts):
                if getattr(p, "part_kind", "") == "thinking" and getattr(p, "content", ""):
                    try:
                        m.parts[j] = dataclasses.replace(p, content="")
                    except Exception:
                        pass
        return out
    except Exception:
        return messages


async def native_history_processor(
    ctx: RunContext[Any],
    messages: list[ModelMessage] | None = None,
    *args: Any,
    **kwargs: Any,
) -> list[ModelMessage]:
    """HistoryProcessor for ProcessHistory — truncates when over checkpoint.

    First parameter is typed ``RunContext`` so pydantic-ai's
    takes_run_context detection passes it for real; legacy direct callers
    that pass ``(messages,)`` are still tolerated below.
    """
    if isinstance(ctx, list):
        # Legacy direct call: native_history_processor(messages)
        messages = ctx
        ctx = kwargs.get("ctx") or None
    if messages is None:
        messages = kwargs.get("messages") or kwargs.get("history") or []
    if not isinstance(messages, list):
        messages = list(messages)
    if args:
        if isinstance(args[-1], list) and messages is kwargs.get("messages"):
            messages = args[-1]
    # Thinking trimming is OPT-IN (PAI_TRIM_THINKING=1) and OFF by default:
    # benchmark evidence showed it BACKFIRES on DeepSeek pricing. Re-sending
    # the unchanged prefix is billed as a cache READ (~$0.0028/M); trimming
    # old ThinkingParts shifts the prefix on every request, which fragments
    # the provider cache and converts ~50x-cheaper reads into misses (miss
    # ratio doubled 776k -> 1.6M on the same 175 tasks). Keep the prefix
    # byte-stable; old thinking is effectively free to resend.
    if os.getenv("PAI_TRIM_THINKING", "0") == "1":
        try:
            messages = _trim_stale_thinking(messages)
        except Exception:
            pass
    # ── Live projectMap+ deltas (PAI_LIVE_MAP=1, default) ──────────────
    # A mutating tool call sets deps._map_stale in the tool hook. Before the
    # NEXT model request we rescan once and append the capped delta as a
    # <current-context> user message, so edits inside a single AgentRun stay
    # visible to the model. Byte-identical re-sends are provider cache-hits
    # after their first appearance (end-of-user-input prefix unit). Runs
    # before the size check so it fires on every request, not just compactions.
    if os.getenv("PAI_LIVE_MAP", "1") != "0":
        try:
            _ldeps = getattr(ctx, "deps", None) if ctx is not None else None
            if _ldeps is not None and getattr(_ldeps, "_map_stale", False):
                _proj = str(getattr(_ldeps, "project_path", "") or "")
                if _proj:
                    try:
                        from memory.memory_manager import force_delta_refresh
                        _d = force_delta_refresh(_proj)
                        _ldeps._map_stale = False  # state now matches disk
                        if _d:
                            _ldeps._map_live = int(getattr(_ldeps, "_map_live", 0) or 0) + 1
                            _block = "<current-context>\n%s\n</current-context>" % _d
                            _dup = False
                            for _m in reversed(messages):
                                for _p in getattr(_m, "parts", []) or []:
                                    if getattr(_p, "part_kind", "") == "user-prompt":
                                        if str(getattr(_p, "content", "") or "") == _block:
                                            _dup = True
                                        break
                                else:
                                    continue
                                break
                            if not _dup:
                                from pydantic_ai.messages import UserPromptPart as _UP
                                from pydantic_ai.messages import ModelRequest as _MR
                                messages = [*messages, _MR(parts=[_UP(content=_block)])]
                    except Exception:
                        try:
                            _ldeps._map_stale = False
                        except Exception:
                            pass
        except Exception:
            pass
    try:
        from llm.model_history import compact_history_native, estimate_history_tokens

        _deps = getattr(ctx, "deps", None)
        policy = policy_for_deps(_deps)
        est = estimate_history_tokens(messages)
        if est < policy.checkpoint_trigger and est < policy.hard_limit:
            return messages
        # Over threshold — compact through the shared Codex-parity engine:
        # full-transcript render (no caps), chunked/rolled summary when the
        # compactor window cannot fit everything, project.md checker, and the
        # map sidecar refresh in parallel. project.md and the map are never
        # summarized; they are re-anchored fresh below.
        try:
            _pp = str(getattr(getattr(ctx, "deps", None), "project_path", "") or "")
        except Exception:
            _pp = ""

        from memory.compaction_engine import run_compaction_summary

        engine = await run_compaction_summary(messages, _pp)
        summary = str(engine.get("summary") or "")
        if not summary:
            summary = "[Native compaction — earlier turns summarized due to length]"
        if engine.get("chunks"):
            print(
                f"[COMPACTION] {engine['chunks']} chunk(s), "
                f"{engine.get('ledgers', 0)} ledger(s), "
                f"overlap={engine.get('overlap_tokens', 0):,} tok, "
                f"trimmed={engine.get('trimmed', 0)} via {engine.get('model')} "
                f"(window {engine.get('window'):,})"
            )
        # Re-anchor the LATEST project map: a fresh full rebuild beats the
        # stale copy embedded in old history (post-anchor edits existed only
        # as deltas). Falls back to the in-history <current-context> block.
        full_map = str(engine.get("full_map") or "") or None
        if not full_map:
            for m in messages:
                for p in getattr(m, "parts", []) or []:
                    try:
                        from pydantic_ai.messages import UserPromptPart
                        if isinstance(p, UserPromptPart) and isinstance(p.content, str) and "<current-context>" in p.content:
                            _body = p.content.split("<current-context>", 1)[1].rsplit("</current-context>", 1)[0].strip()
                            if _body.startswith("projectMap"):
                                full_map = _body
                                break
                    except Exception:
                        continue
                if full_map:
                    break
        return compact_history_native(
            messages,
            policy=policy,
            full_project_map=full_map,
            summary_text=summary,
            project_memory_text=str(engine.get("project_memory") or ""),
            user_messages=list(engine.get("user_messages") or []),
        )
    except Exception:
        # Never break the run on compaction error
        return messages


def _split_compaction_blocks(text: str) -> tuple[str, str]:
    """Compatibility wrapper — the parser now lives in the shared engine."""
    from memory.compaction_engine import split_compaction_blocks
    return split_compaction_blocks(text)


def get_process_history_capability():
    """Return ProcessHistory capability wrapping native_history_processor."""
    try:
        from pydantic_ai.capabilities.process_history import ProcessHistory
        return ProcessHistory(processor=native_history_processor)
    except Exception:
        return None
