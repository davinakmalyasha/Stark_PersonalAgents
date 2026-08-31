"""
pai_transport.py --- pydantic-ai native transport for the agent loops.

Single LLM-call transport (replaces the langchain ChatOpenAI path):

- ``pai_native_turn`` is the tool-calling loop entry: takes a LangChain
  transcript + native tool schemas, runs ONE model turn through pydantic-ai's
  OpenAI-compatible provider (DeepSeek / OpenCode Go / Command Code gateway),
  and returns ``(text, tool_calls, reasoning_content)`` — the exact shape the
  agent loops already unpack (developer_agent.py:840-849).

  ``tool_calls`` entries use the internal ``{"tool", "args", "id"}``
  convention (see llm.native_conv.normalize_calls).

- ``pai_structured`` is the single-shot structured-output path: feeds the same
  provider and returns a validated pydantic result (used by subagent
  ``response_format``, replacing the regex-based ``_parse_schema_response``).

Design rules:
  * No langchain. Transcript conversion happens here (lc messages are the
    repo-wide storage format; this module is the only place that talks to the
    provider wire format).
  * Streaming deltas are emitted into the shared StreamBus with the same
    ``(kind, text)`` contract as the old langchain path, so the frontend keeps
    seeing live text/reasoning/tool deltas.
  * Usage (from ``result.usage()``) is fed to ``update_token_stats`` with the
    same fields as before (input/output/cache-hit tokens, reasoning chars).
  * The caller retains the model-fallback queue + rate-limit retry loop
    (llm.py); this module raises on failure so the caller can decide.
"""

from __future__ import annotations

import json
import os
import uuid
from typing import Any

from pydantic_ai import Agent
from pydantic_ai.messages import (
    ModelMessage,
    ModelRequest,
    ModelResponse,
    PartDeltaEvent,
    PartStartEvent,
    SystemPromptPart,
    TextPart,
    TextPartDelta,
    ThinkingPart,
    ThinkingPartDelta,
    ToolCallPart,
    ToolReturnPart,
    UserPromptPart,
)
from pydantic_ai.models.openai import OpenAIChatModel
from pydantic_ai.providers.openai import OpenAIProvider
from pydantic_ai.usage import RequestUsage

from llm.native_conv import normalize_calls

# Typed per-run workspace context (pydantic-ai deps). Imported lazily-capable:
# deps.py is stdlib-only, safe to import at module load.
from llm.deps import WorkspaceDeps

# Same gateway resolution as the old langchain client (llm.py:_resolve_llm_provider)
_DIRECT_BASE = "https://api.deepseek.com"
_OPENCODE_BASE = "https://opencode.ai/zen/go/v1"


def _resolve_provider_cfg() -> dict:
    """Return the active provider cfg dict (base/key/flash/pro/...) via llm.py resolver."""
    try:
        from llm.llm import _resolve_llm_provider
        cfg = _resolve_llm_provider()
        if isinstance(cfg, dict) and cfg.get("base"):
            return cfg
    except Exception:
        pass
    return {
        "base": os.getenv("DEEPSEEK_BASE_URL", _DIRECT_BASE),
        "key": os.getenv("DEEPSEEK_API_KEY") or os.getenv("OPENCODE_API_KEY") or None,
    }


def _resolve_base_url() -> str:
    """Return the active gateway base URL (dropdown-aware)."""
    cfg = _resolve_provider_cfg()
    base = cfg.get("base")
    if isinstance(base, str) and base.strip():
        return base.strip()
    return os.getenv("DEEPSEEK_BASE_URL", _DIRECT_BASE)


def _resolve_api_key(base_url: str | None = None) -> str | None:
    """Return API key for the active provider (dropdown-aware)."""
    cfg = _resolve_provider_cfg()
    cfg_key = cfg.get("key") if isinstance(cfg, dict) else None
    cfg_base = cfg.get("base") if isinstance(cfg, dict) else None
    # If no explicit base_url or it matches the active provider, use its key
    if not base_url or (cfg_base and base_url == cfg_base):
        if isinstance(cfg_key, str) and cfg_key.strip():
            return cfg_key.strip()
    # Host-based fallback (for explicit overrides / tests)
    if isinstance(base_url, str):
        if "opencode.ai" in base_url:
            return os.getenv("OPENCODE_API_KEY") or (cfg_key if isinstance(cfg_key, str) else None)
        if "commandcode.ai" in base_url or "command-code" in base_url:
            return os.getenv("COMMAND_CODE_API_KEY") or (cfg_key if isinstance(cfg_key, str) else None)
    if isinstance(cfg_key, str) and cfg_key.strip():
        return cfg_key.strip()
    return os.getenv("COMMAND_CODE_API_KEY") or os.getenv("OPENCODE_API_KEY") or os.getenv("DEEPSEEK_API_KEY") or None


def _model_name(model: str) -> str:
    """Map the repo model slug to a provider model id (same mapping as llm._map_model)."""
    try:
        from llm.llm import _map_model
        return _map_model(model or "")
    except Exception:
        return model or "deepseek-chat"


def _is_reasoning_model(model: str) -> bool:
    m = (model or "").lower()
    return any(x in m for x in ["r1", "pro", "o1", "o3", "reasoning", "thinking", "fallback", "automatic", "flash", "mimo"])


def _build_model_settings(
    role: str | None,
    model: str,
    temp: float,
    reasoning_effort: str | None,
    reasoning_budget: int | None,
) -> dict:
    """Build OpenAIChatModel settings mirroring llm.py:_get_deepseek_client thinking logic."""
    settings: dict[str, Any] = {"temperature": temp}
    extra_body: dict | None = None
    effort: str | None = None

    # Explicit caller intent
    if reasoning_effort is not None:
        e = str(reasoning_effort).strip().lower()
        if e in ("disabled", "none", ""):
            extra_body = {"thinking": {"type": "disabled"}}
            effort = None
        else:
            budget_override = reasoning_budget
            if e == "max":
                e = "high"
                if budget_override is None:
                    try:
                        budget_override = int(os.getenv("REASONING_MAX_BUDGET", "32768"))
                    except Exception:
                        budget_override = 32768
            extra_body = {"thinking": {"type": "enabled"}}
            effort = e
            if budget_override is not None:
                try:
                    extra_body["thinking"]["budget_tokens"] = int(budget_override)
                except Exception:
                    pass
    elif _is_reasoning_model(model):
        is_orchestrator = (
            role in ["Developer", "DeveloperFixing"]
            or (role and any(x in role.lower() for x in ["developer", "orchestrator", "designer"]))
        )
        is_heavy = is_orchestrator or (
            role in ["Supervisor", "sa", "Coder"]
            or (role and any(x in role.lower() for x in ["fixing", "coder", "architect", "complex"]))
        )
        if is_orchestrator or is_heavy:
            extra_body = {"thinking": {"type": "enabled"}}
            effort = "high"
        else:
            extra_body = {"thinking": {"type": "disabled"}}

    # Prefix-cache pin (same global as llm.py)
    if extra_body is not None:
        try:
            from llm.llm import _THINKING_PINS, _THINKING_PINS_LOCK  # type: ignore
            pin_key = (role or "", model or "")
            with _THINKING_PINS_LOCK:
                if pin_key not in _THINKING_PINS:
                    _THINKING_PINS[pin_key] = dict(extra_body or {})
                else:
                    pinned = _THINKING_PINS[pin_key]
                    if pinned:
                        extra_body = dict(pinned)
                    cur = dict(extra_body or {})
                    # handle type mismatch same as llm.py
                    try:
                        thinking_type = (cur.get("thinking") or {}).get("type")
                        pinned_type = (pinned.get("thinking") or {}).get("type")
                        if thinking_type is not None and pinned_type is not None and thinking_type != pinned_type:
                            extra_body = dict(pinned)
                        elif pinned.get("thinking") and cur.get("thinking"):
                            merged = dict(cur)
                            merged["thinking"] = dict(pinned["thinking"])
                            extra_body = merged
                    except Exception:
                        pass
                    # keep effort consistent with pinned type: if pinned disabled, drop effort
                    try:
                        pinned_type = (extra_body.get("thinking") or {}).get("type") if isinstance(extra_body, dict) else None
                        if pinned_type == "disabled":
                            effort = None
                    except Exception:
                        pass
        except Exception:
            pass

    if extra_body is not None:
        settings["extra_body"] = extra_body
    if effort is not None:
        # pydantic-ai OpenAI field
        settings["openai_reasoning_effort"] = effort
    return settings


def _model_profile_for(model: str) -> dict | None:
    """Profile for DeepSeek-style reasoning_content field."""
    m = (model or "").lower()
    # DeepSeek, mimo, and flash family use reasoning_content field
    if any(x in m for x in ["deepseek", "mimo", "flash", "r1", "reasoning"]):
        return {
            "openai_chat_thinking_field": "reasoning_content",
            "openai_chat_send_back_thinking_parts": "field",
        }
    return None


# ── Transcript conversion (LangChain messages -> pydantic-ai ModelMessage) ──


def _lc_messages_to_pai(messages: list) -> list[ModelMessage]:
    """Convert a LangChain transcript into pydantic-ai ModelMessages.

    Lossless for everything the agent loops store: system/user/assistant
    (with tool_calls + reasoning_content) / tool results. Image content-block
    lists pass through as-is inside user parts.

    Empty-content user turns (agent loops emit a bare HumanMessage("") as a
    continuation marker after tool calls) are dropped — some gateways
    (command-code) reject ``content: ""``.
    """
    out: list[ModelMessage] = []
    for m in messages or []:
        kind = type(m).__name__
        content = m.content if hasattr(m, "content") else ""
        if kind == "SystemMessage":
            out.append(ModelRequest(parts=[SystemPromptPart(content=str(content))]))
        elif kind == "HumanMessage":
            if content is None:
                continue
            if isinstance(content, str) and not str(content).strip():
                # Empty continuation marker — drop (gateways reject empty user).
                continue
            out.append(ModelRequest(parts=[UserPromptPart(content=_as_user_content(content))]))
        elif kind == "AIMessage":
            parts: list = []
            reasoning = None
            if hasattr(m, "additional_kwargs") and isinstance(m.additional_kwargs, dict):
                reasoning = m.additional_kwargs.get("reasoning_content")
            if reasoning:
                parts.append(ThinkingPart(content=str(reasoning)))
            tcs = getattr(m, "tool_calls", None) or []
            if tcs:
                for tc in tcs:
                    if not isinstance(tc, dict):
                        continue
                    args = tc.get("args") or {}
                    parts.append(
                        ToolCallPart(
                            tool_name=str(tc.get("name") or tc.get("tool", "") or ""),
                            args=args if isinstance(args, str) else args,
                            tool_call_id=str(tc.get("id") or f"call-{uuid.uuid4().hex[:8]}"),
                        )
                    )
            text = str(content or "")
            if text:
                parts.insert(0, TextPart(content=text))
            out.append(ModelResponse(parts=parts))
        elif kind == "ToolMessage":
            out.append(
                ModelRequest(
                    parts=[
                        ToolReturnPart(
                            tool_name=str(getattr(m, "name", "") or ""),
                            content=str(content),
                            tool_call_id=str(getattr(m, "tool_call_id", "") or "call-0"),
                        )
                    ]
                )
            )
    return out


def _as_user_content(content: Any):
    """Keep image content-block lists intact; stringify everything else."""
    if isinstance(content, str):
        return content
    return content  # sequence of parts passes through untouched


def _split_user_prompt(history: list[ModelMessage]) -> tuple[Any, list[ModelMessage]]:
    """Extract the first user part as the pydantic-ai ``user_prompt``.

    pydantic-ai appends ``user_prompt`` as a user message when it is not None,
    and an empty string still hits gateways that reject empty content. We pull
    the first ``UserPromptPart`` out of the converted history so the prompt is
    non-empty and the history keeps its full context. Returns ``(prompt, history)``
    with the first user message removed from history.
    """
    if not history:
        return "", history
    for i, m in enumerate(history):
        if isinstance(m, ModelRequest):
            for p in m.parts:
                if isinstance(p, UserPromptPart):
                    remaining = list(history)
                    remaining.pop(i)
                    return p.content, remaining
    return "", history


def _pai_to_lc(all_messages: list, prior: list) -> list:
    """Convert the pydantic-ai messages ADDED by a run back to LangChain.

    ``prior`` is the history we passed in; everything after it in
    ``all_messages`` is new (assistant text/tool-calls + tool results).
    Returns LangChain messages (AIMessage with tool_calls, ToolMessage).
    """
    from langchain_core.messages import AIMessage, ToolMessage, HumanMessage

    prior_ids = {id(m) for m in prior}
    added = [m for m in all_messages if id(m) not in prior_ids]
    out: list = []
    for m in added:
        if isinstance(m, ModelResponse):
            text_parts = []
            reasoning = ""
            calls = []
            for p in m.parts:
                kind = getattr(p, "part_kind", "")
                if kind == "text":
                    text_parts.append(str(p.content))
                elif kind == "thinking":
                    reasoning += str(p.content or "")
                elif kind == "tool-call":
                    args = p.args
                    if isinstance(args, str):
                        try:
                            args = json.loads(args or "{}")
                        except Exception:
                            args = {}
                    calls.append(
                        {
                            "name": p.tool_name,
                            "args": args if isinstance(args, dict) else {},
                            "id": getattr(p, "tool_call_id", "") or "",
                        }
                    )
            kwargs: dict = {}
            if calls:
                kwargs["tool_calls"] = calls
            if reasoning:
                kwargs["additional_kwargs"] = {"reasoning_content": reasoning}
            out.append(AIMessage(content="".join(text_parts), **kwargs))
        elif isinstance(m, ModelRequest):
            for p in m.parts:
                from pydantic_ai.messages import ToolReturnPart
                if isinstance(p, ToolReturnPart):
                    out.append(
                        ToolMessage(
                            content=str(p.content),
                            name=str(p.tool_name or ""),
                            tool_call_id=str(p.tool_call_id or ""),
                        )
                    )
    return out


def _result_to_native(
    parts: list,
    response: ModelResponse,
) -> tuple[str, list[dict], str | None]:
    """Extract (text, tool_calls, reasoning) from a ModelResponse parts list."""
    text_parts: list[str] = []
    reasoning_parts: list[str] = []
    call_parts: list[dict] = []
    for part in parts or []:
        kind = getattr(part, "part_kind", "")
        if kind == "text":
            text_parts.append(str(part.content))
        elif kind == "thinking":
            reasoning_parts.append(str(part.content))
        elif kind == "tool-call":
            args = part.args
            if isinstance(args, str):
                try:
                    args = json.loads(args or "{}")
                except Exception:
                    args = {}
            call_parts.append(
                {
                    "tool": part.tool_name,
                    "args": args if isinstance(args, dict) else {},
                    "id": getattr(part, "tool_call_id", "") or "",
                }
            )
    text = "".join(text_parts).strip()
    reasoning = "".join(reasoning_parts).strip() or None
    # DeepSeek with thinking may return empty content — fall back to reasoning.
    if not text and reasoning and not call_parts:
        text = reasoning
    return text, call_parts, reasoning


def _pai_usage_to_stats(
    role: str,
    model: str,
    usage: RequestUsage | None,
    where: str | None,
    reasoning_effort: str | None,
    reasoning_budget: int | None,
    reasoning_chars: int = 0,
) -> None:
    try:
        from llm.llm_stats import update_token_stats
    except Exception:
        return
    u = usage or RequestUsage()
    # RequestUsage uses cache_read_tokens / cache_write_tokens (genai-prices);
    # legacy code used cache_hit_tokens. Map accordingly.
    cache_hit = getattr(u, "cache_read_tokens", 0) or getattr(u, "cache_hit_tokens", 0) or 0
    # Some providers put cache hits in details dict
    if not cache_hit and isinstance(getattr(u, "details", None), dict):
        try:
            cache_hit = int(u.details.get("cached_tokens") or u.details.get("cache_read_tokens") or 0)
        except Exception:
            pass
    update_token_stats(
        role,
        model,
        u.input_tokens or 0,
        u.output_tokens or 0,
        cache_hit or 0,
        where,
        reasoning_chars=reasoning_chars,
        reasoning_effort=reasoning_effort,
        reasoning_budget=reasoning_budget,
    )


def _emit_stream(role: str, kind: str, text: str, where: str) -> None:
    """Emit a delta into the shared StreamBus (same contract as before)."""
    try:
        from llm.llm import _stream_emit
        from llm.llm import _effective_stream_chat_id
        _stream_emit(_effective_stream_chat_id(), kind, text, agent=role, model="")
    except Exception:
        pass


def pai_native_turn(
    role: str,
    messages: list,
    tools: list[dict] | None = None,
    model: str = "deepseek:v4-flash",
    temp: float = 0.2,
    reasoning_effort: str | None = None,
    reasoning_budget: int | None = None,
    where: str | None = None,
    stream_emit: bool = True,
    base_url: str | None = None,
    api_key: str | None = None,
    deps: WorkspaceDeps | None = None,
) -> tuple[str, list[dict], str | None]:
    """One model turn. Returns ``(text, tool_calls, reasoning_content)``.

    Sync facade over :func:`apai_native_turn`. Raises on provider failure so
    the caller's model-fallback queue can decide. ``base_url``/``api_key``
    override the resolved gateway (test/alt-provider use). ``deps`` carries the
    typed per-run workspace context; when None the caller retains the legacy
    global-shared_state path (pre-migration compatibility).

    Async callers (server loop) MUST use :func:`apai_native_turn` — this sync
    facade spins its own event loop and blocks the server.
    """
    import asyncio

    return asyncio.run(
        apai_native_turn(
            role=role,
            messages=messages,
            tools=tools,
            model=model,
            temp=temp,
            reasoning_effort=reasoning_effort,
            reasoning_budget=reasoning_budget,
            where=where,
            stream_emit=stream_emit,
            base_url=base_url,
            api_key=api_key,
            deps=deps,
        )
    )


async def apai_native_turn(
    role: str,
    messages: list,
    tools: list[dict] | None = None,
    model: str = "deepseek:v4-flash",
    temp: float = 0.2,
    reasoning_effort: str | None = None,
    reasoning_budget: int | None = None,
    where: str | None = None,
    stream_emit: bool = True,
    base_url: str | None = None,
    api_key: str | None = None,
    deps: WorkspaceDeps | None = None,
    cancellation_token: Any = None,
) -> tuple[str, list[dict], str | None]:
    """Async one model turn (A1). Returns ``(text, tool_calls, reasoning_content)``.

    Raises on provider failure so the caller's model-fallback queue can decide.
    ``cancellation_token`` (A8) propagates to the underlying run.
    """
    # Dropdown-aware resolution: prefer typed deps when provided.
    if base_url is None or api_key is None:
        cfg = deps.llm_provider if deps is not None else None
        if cfg is None:
            cfg = _resolve_provider_cfg()
        if base_url is None:
            base_url = str(cfg.get("base") or _resolve_base_url())
        if api_key is None:
            # Use cfg key when base matches, else host-based fallback
            cfg_base = str(cfg.get("base") or "")
            cfg_key = cfg.get("key")
            if cfg_base and base_url == cfg_base and isinstance(cfg_key, str) and cfg_key.strip():
                api_key = cfg_key.strip()
            else:
                api_key = _resolve_api_key(base_url)
                if not api_key and isinstance(cfg_key, str) and cfg_key.strip():
                    api_key = cfg_key.strip()
    model_id = _model_name(model)

    settings = _build_model_settings(role, model_id, temp, reasoning_effort, reasoning_budget)
    profile = _model_profile_for(model_id)
    provider = OpenAIProvider(
        base_url=base_url,
        api_key=api_key,
    )
    if profile:
        llm = OpenAIChatModel(
            model_name=model_id,
            provider=provider,
            settings=settings,
            profile=profile,
        )
    else:
        llm = OpenAIChatModel(
            model_name=model_id,
            provider=provider,
            settings=settings,
        )

    tool_defs = None
    if tools:
        tool_defs = []
        for t in tools:
            fn = t.get("function", t) if isinstance(t, dict) else t
            name = fn.get("name", "")
            if not name:
                continue
            tool_defs.append(
                {
                    "name": name,
                    "description": fn.get("description", ""),
                    "parameters": fn.get("parameters") or {"type": "object", "properties": {}},
                }
            )
    if tool_defs:
        # Track B: pydantic-ai native agent is the default execution path.
        # PAI_NATIVE_TOOLS=0 opts back into the schema-only single-turn path
        # (raw model request, no agent loop — the caller's loop owns tool
        # execution). Any other value (unset / "1" / "true") = native.
        use_native = os.environ.get("PAI_NATIVE_TOOLS", "1") != "0"
        if use_native:
            from llm.native_tools import build_agent
            agent = build_agent(
                role=deps.role if deps is not None else role,
                model=llm,
                tools=tool_defs,
                deps=deps,
            )
            native_agent = True
        else:
            agent = None
            native_agent = False
    else:
        agent = None
        native_agent = False

    # A6b: accept native ModelMessage history directly.
    from pydantic_ai.messages import ModelRequest as _MR, ModelResponse as _MResp

    if messages and isinstance(messages[0], (_MR, _MResp)):
        msg_history_full: list[ModelMessage] = list(messages)  # type: ignore
    else:
        msg_history_full = _lc_messages_to_pai(messages)
    # pydantic-ai appends `user_prompt` as a user message when not None, so an
    # empty "" would hit gateways that reject empty content. Extract the FIRST
    # user part from the history as the prompt (matching pydantic-ai's own
    # resume convention) and leave the rest as history.
    user_prompt, msg_history = _split_user_prompt(msg_history_full)
    if native_agent:
        with agent.override(model=llm):
            if stream_emit:
                # Native agent: live stream via snapshot diffing (P1).
                async def _run():
                    from llm.native_tools import DEFAULT_USAGE_LIMITS
                    async with agent.run_stream(
                        user_prompt,
                        message_history=msg_history,
                        usage_limits=DEFAULT_USAGE_LIMITS,
                        deps=deps or WorkspaceDeps.from_shared_state(role=role),
                        cancellation_token=cancellation_token,
                    ) as sres:
                        text_acc: list[str] = []
                        reasoning_acc: list[str] = []
                        prev_text = ""
                        prev_reasoning = ""
                        async for resp in sres.stream_response(debounce_by=None):
                            cur_text = "".join(p.content for p in resp.parts if getattr(p, "part_kind", "") == "text")
                            cur_reasoning = "".join(p.content for p in resp.parts if getattr(p, "part_kind", "") == "thinking")
                            if len(cur_reasoning) > len(prev_reasoning):
                                delta = cur_reasoning[len(prev_reasoning):]
                                reasoning_acc.append(delta)
                                _emit_stream(role, "reasoning", delta, where or "")
                                prev_reasoning = cur_reasoning
                            if len(cur_text) > len(prev_text):
                                delta = cur_text[len(prev_text):]
                                text_acc.append(delta)
                                _emit_stream(role, "text", delta, where or "")
                                prev_text = cur_text
                        return sres, text_acc, reasoning_acc

                sres, text_acc, reasoning_acc = await _run()
                reasoning = "".join(reasoning_acc).strip() or None
                text = "".join(text_acc).strip()
                try:
                    resp = sres.response
                    final_text = "".join(p.content for p in resp.parts if getattr(p, "part_kind", "") == "text").strip()
                    final_reasoning = "".join(p.content for p in resp.parts if getattr(p, "part_kind", "") == "thinking").strip() or None
                    if final_text and len(final_text) > len(text):
                        _emit_stream(role, "text", final_text[len(text):], where or "")
                        text = final_text
                    if final_reasoning and not reasoning:
                        reasoning = final_reasoning
                        _emit_stream(role, "reasoning", reasoning, where or "")
                    elif final_reasoning and reasoning and len(final_reasoning) > len(reasoning):
                        _emit_stream(role, "reasoning", final_reasoning[len(reasoning):], where or "")
                        reasoning = final_reasoning
                except Exception:
                    resp = sres.response
                usage = sres.usage
                resp = sres.response
                call_parts: list[dict] = []
                for part in resp.parts:
                    if getattr(part, "part_kind", "") == "tool-call":
                        args = part.args
                        if isinstance(args, str):
                            try:
                                args = json.loads(args or "{}")
                            except Exception:
                                args = {}
                        call_parts.append(
                            {
                                "tool": part.tool_name,
                                "args": args if isinstance(args, dict) else {},
                                "id": getattr(part, "tool_call_id", "") or "",
                            }
                        )
            else:
                from llm.native_tools import DEFAULT_USAGE_LIMITS
                res = agent.run_sync(
                    user_prompt,
                    message_history=msg_history,
                    usage_limits=DEFAULT_USAGE_LIMITS,
                    deps=deps or WorkspaceDeps.from_shared_state(role=role),
                )
                text, call_parts, reasoning = _result_to_native(res.output.parts, res)
                usage = res.usage
    else:
        # Single-turn (registry path): raw model request — no agent loop.
        # The caller's loop owns tool execution; we return ONE model turn.
        from pydantic_ai.models import ModelRequestParameters
        from pydantic_ai.tools import ToolDefinition
        from pydantic_ai.messages import (
            TextPartDelta,
            ThinkingPartDelta,
            PartStartEvent,
            ToolCallPartDelta,
            PartDeltaEvent,
        )

        fn_tool_defs = []
        for td in tool_defs or []:
            fn_tool_defs.append(
                ToolDefinition(
                    name=td["name"],
                    parameters_json_schema=td["parameters"],
                    description=td["description"] or "",
                )
            )
        req_params = ModelRequestParameters(function_tools=fn_tool_defs)

        if stream_emit:
            async def _stream():
                call_parts_acc: list[dict] = []
                tool_parts: dict[int, dict] = {}
                text_start_seen: dict[int, str] = {}
                async with llm.request_stream(msg_history_full, settings, req_params) as sresp:
                    text_acc: list[str] = []
                    reasoning_acc: list[str] = []
                    async for event in sresp:
                        if isinstance(event, PartStartEvent):
                            from pydantic_ai.messages import ThinkingPart, ToolCallPart, TextPart
                            if isinstance(event.part, TextPart):
                                content = event.part.content or ""
                                if content:
                                    # Some providers deliver the full text in the
                                    # start event; others deliver a partial prefix
                                    # followed by deltas. Defer emission until we
                                    # know which.
                                    text_start_seen[event.index] = content
                                else:
                                    text_start_seen[event.index] = ""
                            elif isinstance(event.part, ThinkingPart):
                                reasoning_acc.append(event.part.content or "")
                            elif isinstance(event.part, ToolCallPart):
                                raw_args = event.part.args
                                parsed_args = {}
                                if isinstance(raw_args, dict):
                                    parsed_args = dict(raw_args)
                                elif isinstance(raw_args, str) and raw_args:
                                    try:
                                        parsed_args = json.loads(raw_args)
                                        if not isinstance(parsed_args, dict):
                                            parsed_args = {}
                                    except Exception:
                                        parsed_args = {}
                                tool_parts[event.index] = {
                                    "tool": event.part.tool_name,
                                    "args": parsed_args,
                                    "id": getattr(event.part, "tool_call_id", "") or "",
                                }
                        elif isinstance(event, PartDeltaEvent):
                            delta = event.delta
                            if isinstance(delta, TextPartDelta):
                                # If a start prefix exists for this part, it was a
                                # partial prefix — drop it (deltas are the truth).
                                if event.index in text_start_seen:
                                    text_start_seen.pop(event.index, None)
                                text_acc.append(delta.content_delta or "")
                                _emit_stream(role, "text", delta.content_delta or "", where or "")
                            elif isinstance(delta, ThinkingPartDelta):
                                reasoning_acc.append(delta.content_delta or "")
                                _emit_stream(role, "reasoning", delta.content_delta or "", where or "")
                            elif isinstance(delta, ToolCallPartDelta):
                                entry = tool_parts.setdefault(event.index, {"tool": "", "args": {}, "id": ""})
                                if delta.tool_name_delta:
                                    entry["tool"] += delta.tool_name_delta
                                if delta.tool_call_id:
                                    entry["id"] = delta.tool_call_id
                                ad = delta.args_delta
                                if isinstance(ad, dict):
                                    entry["args"].update(ad)
                                elif isinstance(ad, str) and ad:
                                    try:
                                        parsed = json.loads(ad)
                                        if isinstance(parsed, dict):
                                            entry["args"].update(parsed)
                                    except Exception:
                                        pass
                    call_parts_acc.extend(
                        v for k, v in sorted(tool_parts.items())
                    )
                    # Any start-prefix that never got a delta = full text delivered
                    # in the start event (command gateway behavior).
                    for idx in sorted(text_start_seen):
                        content = text_start_seen[idx] or ""
                        if content:
                            text_acc.append(content)
                            _emit_stream(role, "text", content, where or "")
                    return text_acc, reasoning_acc, call_parts_acc

            text_acc, reasoning_acc, call_parts = await _stream()
            text = "".join(text_acc).strip()
            reasoning = "".join(reasoning_acc).strip() or None
            usage = None
        else:
            resp = llm.request(msg_history_full, settings, req_params)
            text, call_parts, reasoning = _result_to_native(resp.parts, resp)
            usage = getattr(resp, "usage", None)
            if not text and reasoning and not call_parts:
                text = reasoning

    _pai_usage_to_stats(
        role, model_id, usage, where, reasoning_effort, reasoning_budget,
        reasoning_chars=len(reasoning or ""),
    )
    return text, call_parts, reasoning


def pai_structured(
    messages: list,
    result_type: type,
    model: str = "deepseek:v4-flash",
    temp: float = 0.2,
    where: str | None = None,
    base_url: str | None = None,
    api_key: str | None = None,
    deps: WorkspaceDeps | None = None,
):
    """Single-shot structured output. Returns the validated result instance.

    Sync facade over :func:`apai_structured`. ``result_type`` may be a pydantic
    model/TypeDict OR a raw JSON-schema dict (specialist ``response_format``
    configs). Raises on failure (caller keeps its existing fallback behavior).
    ``base_url``/``api_key`` override the resolved gateway (test/alt-provider
    use). ``deps`` carries the typed per-run workspace context (fallback:
    legacy global path).
    """
    import asyncio

    return asyncio.run(
        apai_structured(
            messages=messages,
            result_type=result_type,
            model=model,
            temp=temp,
            where=where,
            base_url=base_url,
            api_key=api_key,
            deps=deps,
        )
    )


async def apai_structured(
    messages: list,
    result_type: type,
    model: str = "deepseek:v4-flash",
    temp: float = 0.2,
    where: str | None = None,
    base_url: str | None = None,
    api_key: str | None = None,
    deps: WorkspaceDeps | None = None,
    cancellation_token: Any = None,
):
    """Async single-shot structured output (A1). Returns the validated result."""
    from pydantic import TypeAdapter
    from pydantic_ai import Agent

    # Dropdown-aware provider wiring: prefer typed deps when provided.
    if base_url is None or api_key is None:
        cfg = deps.llm_provider if deps is not None else None
        if cfg is None:
            cfg = _resolve_provider_cfg()
        if base_url is None:
            base_url = str(cfg.get("base") or _resolve_base_url())
        if api_key is None:
            cfg_base = str(cfg.get("base") or "")
            cfg_key = cfg.get("key")
            if cfg_base and base_url == cfg_base and isinstance(cfg_key, str) and cfg_key.strip():
                api_key = cfg_key.strip()
            else:
                api_key = _resolve_api_key(base_url)
                if not api_key and isinstance(cfg_key, str) and cfg_key.strip():
                    api_key = cfg_key.strip()
    model_id = _model_name(model)

    # Structured output benefits from disabled thinking for cleaner JSON
    settings = _build_model_settings("Structured", model_id, temp, "disabled", None)
    # Keep temperature from caller if settings already has it
    settings["temperature"] = temp
    profile = _model_profile_for(model_id)
    provider = OpenAIProvider(base_url=base_url, api_key=api_key)
    if profile:
        llm = OpenAIChatModel(model_name=model_id, provider=provider, settings=settings, profile=profile)
    else:
        llm = OpenAIChatModel(model_name=model_id, provider=provider, settings=settings)
    agent = Agent(model=llm, output_type=result_type)

    # A6b: accept native ModelMessage history.
    if messages and isinstance(messages[0], (ModelRequest, ModelResponse)):
        _full: list[ModelMessage] = list(messages)  # type: ignore
    else:
        _full = _lc_messages_to_pai(messages)
    user_prompt, msg_history = _split_user_prompt(_full)
    res = await agent.run(
        user_prompt, message_history=msg_history, cancellation_token=cancellation_token
    )
    _pai_usage_to_stats("Structured", model_id, res.usage, where, None, None)
    return res.output
