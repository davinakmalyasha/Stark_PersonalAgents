"""native_tools.py --- build pydantic-ai Agents from preserved TOOL_DEFINITIONS.

Track B: full native migration. Uses ONLY pydantic-ai 2.36 primitives:
- ``Tool.from_schema`` from the preserved ``tools._schemas.TOOL_DEFINITIONS``
  (your refined compact schemas — untouched, single source).
- ``FunctionToolset`` to group Tools; ``FilteredToolset`` for per-agent scope
  (replaces ``_registry.restrict_scope``).
- ``Hooks`` (before/after_tool_execute) for cache / plan-mode guard / secret
  redact / offload — the registry pipeline's native equivalent.
- Native parallel tool execution (``ToolManager.parallel_execution_mode``
  defaults to ``'parallel'``).

The real tool implementations (read_file/edit_file/...) stay EXACTLY as they
are — they are the ``execute`` functions bound to each Tool. Nothing about
their logic, prompts, or schemas changes.
"""
from __future__ import annotations

import inspect
import os
import re
import time
from typing import Any, Optional

from decimal import Decimal

from pydantic_ai import Agent, RunContext
from pydantic_ai.capabilities.hooks import Hooks
from pydantic_ai.messages import ToolCallPart
from pydantic_ai.tools import Tool
from pydantic_ai.toolsets import FilteredToolset, FunctionToolset
from pydantic_ai.usage import UsageLimits

from llm.deps import WorkspaceDeps

# P1-3: native concurrency limiter (replaces hot-spin ConcurrencyThrottle for agents)
try:
    from pydantic_ai.concurrency import ConcurrencyLimiter

    _max = int(os.getenv("PAI_MAX_CONCURRENCY", "4"))
    _queued = os.getenv("PAI_MAX_QUEUED", "20")
    try:
        _queued_v = int(_queued) if _queued.strip() not in ("", "none", "None") else None
    except Exception:
        _queued_v = 20
    _NATIVE_LIMITER = ConcurrencyLimiter(max_running=max(1, _max), max_queued=_queued_v, name="native-agent-pool")
except Exception:
    _NATIVE_LIMITER = None  # type: ignore

#: Structured exit frame emitted by run_command: "[OK] Exit 0 (1234ms)".
#: Group 1 is the status tag — exit codes alone are ambiguous because
#: grep/find/diff exit 1 is a normal negative answer.
_EXIT_RE = re.compile(r"\[(OK|ERR)\]\s+Exit\s+(-?\d+)", re.IGNORECASE)


def _tool_map() -> dict[str, Any]:
    """Map tool name -> implementation callable (lazy, avoids import cycle)."""
    import tools as _facade
    out: dict[str, Any] = {}
    if hasattr(_facade, "TOOL_MAP"):
        try:
            out.update(dict(_facade.TOOL_MAP.items()))
        except Exception:
            pass
    # LazyToolMap.items() only covers registered names; pick up module attrs
    # too (test/external tools defined directly on the facade).
    for name in dir(_facade):
        if name.startswith("_") or name in out:
            continue
        attr = getattr(_facade, name)
        if callable(attr):
            out[name] = attr
    return out


def _schemas_by_name() -> dict[str, dict]:
    """Preserved TOOL_DEFINITIONS, keyed by name (single source of schema truth)."""
    from tools._schemas import TOOL_DEFINITIONS
    return {d["name"]: d for d in TOOL_DEFINITIONS}


def _allowed_for(role: str) -> set[str]:
    """Per-agent scope allowlist from workspace_registry (typed, data-driven)."""
    try:
        from agents.workspace_registry import get_spec
        spec = get_spec(role)
        if spec is not None:
            return set(spec.tools)
    except Exception:
        pass
    try:
        from tools._schemas import _SCOPE_MAP
        return set(_SCOPE_MAP.get(role.lower(), []))
    except Exception:
        return set()


# ── Hook implementations (registry pipeline, now native) ────────────────────

def _before_tool_execute(
    ctx: RunContext[WorkspaceDeps],
    *,
    call: ToolCallPart,
    tool_def: Any,
    args: dict,
) -> dict:
    """Pre-execution: plan-mode guard veto; P2-3 deferred approval when PAI_DEFERRED=1.

    Deferral is only allowed here (args are validated by now). Under
    ``PAI_DEFERRED=1`` a plan-mode write is surfaced to the human for
    approve/deny instead of being auto-denied.
    """
    try:
        from tools.plan_mode_guard import plan_mode_guard
        reason = plan_mode_guard(
            {"name": call.tool_name, "args": dict(args or {}), "scope": ctx.deps.role,
             "deps": ctx.deps}
        )
        if reason:
            if os.getenv("PAI_DEFERRED", "0") == "1":
                from pydantic_ai.exceptions import ApprovalRequired
                raise ApprovalRequired(
                    metadata={
                        "reason": reason,
                        "tool": call.tool_name,
                        "chat_id": getattr(ctx.deps, "chat_id", ""),
                    }
                )
            from pydantic_ai.tools import ToolDenied
            raise ToolDenied(reason)
    except Exception as e:
        if e.__class__.__name__ in ("ToolDenied", "ApprovalRequired"):
            raise
    return dict(args or {})


async def _wrap_tool_execute(
    ctx: RunContext[WorkspaceDeps],
    *,
    call: ToolCallPart,
    tool_def: Any,
    args: dict,
    handler: Any,
) -> Any:
    """Cache hit via SkipToolExecution + error-as-ToolFailed mapping (P0-4)."""
    from pydantic_ai.exceptions import SkipToolExecution, ToolFailed

    try:
        from tools import _check_tool_cache

        cached = _check_tool_cache(call.tool_name, dict(args or {}))
        if cached is not None:
            raise SkipToolExecution(result=cached)
    except SkipToolExecution:
        raise
    except Exception:
        pass
    import time as _time
    _t0 = _time.perf_counter()
    result = await handler(args)
    try:
        _ms = int((_time.perf_counter() - _t0) * 1000)
        _deps = ctx.deps
        _tl = getattr(_deps, "_tool_timings", None)
        if _tl is None:
            _tl = {}
            _deps._tool_timings = _tl
        _tl[call.tool_name] = _tl.get(call.tool_name, 0) + _ms
        _tc = getattr(_deps, "_tool_counts", None)
        if _tc is None:
            _tc = {}
            _deps._tool_counts = _tc
        _tc[call.tool_name] = _tc.get(call.tool_name, 0) + 1
        # Ordered execution log {t: tool, f: failed} for natural-repair metrics:
        # a fix cycle = an edit_file that follows an observed failure. Same
        # failure heuristic the eval harness uses for hidden-test mapping.
        _tq = getattr(_deps, "_tool_seq", None)
        if _tq is None:
            _tq = []
            _deps._tool_seq = _tq
        if len(_tq) < 2000:
            try:
                _txt = result if isinstance(result, str) else str(result or "")
                _entry = {"t": call.tool_name, "f": False}
                _head = _txt.lstrip()[:400]
                # Structured failure signals only — a loose "FAIL" substring used
                # to false-positive on passing test output that merely mentions FAIL.
                _failed = (
                    _head.startswith("Error")
                    or _head.startswith("[Error")
                    or "Traceback (most recent call last)" in _txt[:800]
                )
                if call.tool_name == "run_command":
                    # run_command frames results as "[OK] Exit 0 (ms)" /
                    # "[ERR] Exit N (ms)"; the tag is authoritative, so
                    # grep/find/diff exit-1 negative answers stay non-errors.
                    _m = _EXIT_RE.match(_head)
                    if _m:
                        _entry["exit_code"] = int(_m.group(2))
                        _failed = _failed or _m.group(1).upper() == "ERR"
                    if "[TIMEOUT]" in _txt or "exit 124" in _head:
                        _entry["timed_out"] = True
                        _failed = True
                    if not _failed and _m is None:
                        _low = _head.lower()
                        _failed = any(k in _low for k in (
                            "command not found", "econnrefused", "errno",
                        ))
                _entry["f"] = bool(_failed)
                if _failed:
                    _entry["p"] = _txt[:200]
            except Exception:
                _entry = {"t": call.tool_name, "f": False}
                _txt = ""
            _tq.append(_entry)
        # Live map: mutating tools make the projectMap stale; the ProcessHistory
        # processor appends a fresh projectMap+ delta before the next request.
        try:
            from tools.plan_mode_guard import WRITE_CLASS_TOOLS as _WC
        except Exception:
            _WC = ("edit_file", "create_project")
        if call.tool_name in _WC:
            try:
                _deps._map_stale = True
            except Exception:
                pass
    except Exception:
        pass
    # Map legacy "Error..." strings to ToolFailed so the model sees is_error=True
    # and retry budget is not consumed (terminal failure).
    try:
        text = result if isinstance(result, str) else str(result or "")
        if text.startswith("Error") or text.startswith("[Error") or "Traceback" in text[:500]:
            # Only for deterministic tool failures, not model retryable args errors
            # (those are already ModelRetry via args_validator).
            raise ToolFailed(text)
    except ToolFailed:
        raise
    except Exception:
        pass
    return result


def _after_tool_execute(
    ctx: RunContext[WorkspaceDeps],
    *,
    call: ToolCallPart,
    tool_def: Any,
    args: dict,
    result: Any,
) -> Any:
    """Post-execution: secret redact + per-tool cap offload to VFS (registry post_hook).

    Offloaded results return a rich ``ToolReturn`` so history/evals get the
    offload path + line/char counts in ``metadata`` while the model-visible
    text stays byte-identical.
    """
    tool_name = call.tool_name
    text = result if isinstance(result, str) else str(result)
    if not text.startswith("Error"):
        try:
            from tools import redact_secrets
            text = redact_secrets(text)
        except Exception:
            pass
    # Offload past the tool's character cap — unified with
    # tools/_runtime.auto_offload_result (same VFS /scratch + shared preview).
    try:
        from tools._runtime import _tool_cap
        _cap = _tool_cap(str(tool_name))
    except Exception:
        _cap = 16000
    if len(text) > _cap:
        try:
            import uuid
            # Use shared VFS router path (same backend as auto_offload_result)
            from tools._runtime import _create_budgeted_preview as _shared_preview, get_vfs_router
            from tools import _create_budgeted_preview  # fallback compat

            file_id = uuid.uuid4().hex[:8]
            vfs_path = f"/scratch/large_output_{file_id}.txt"
            try:
                _router = get_vfs_router()
                _res = _router.write(vfs_path, text)
                if _res and getattr(_res, "error", None):
                    raise RuntimeError(_res.error)
            except Exception:
                # Fallback to legacy _write_file (still VFS but via facade)
                from tools import _write_file as _legacy_write
                _legacy_write(file_path=vfs_path, content=text)
            try:
                preview = _shared_preview(text, max(1, min(8000, _cap)))
            except Exception:
                preview = _create_budgeted_preview(text)
            model_text = (
                f"[TOOL OUTPUT OFFLOADED to VFS path: {vfs_path}]\n"
                f"Total lines: {len(text.splitlines())}, Total characters: {len(text)}\n"
                f"--- PREVIEW (head+tail) ---\n{preview}\n"
                f"---------------------------\n"
                f"[Use read_file with offset/limit to read the offloaded file at {vfs_path} if needed.]"
            )
            try:
                from pydantic_ai import ToolReturn
                return ToolReturn(
                    return_value=model_text,
                    metadata={
                        "kind": "offload",
                        "offload_path": vfs_path,
                        "total_chars": len(text),
                        "lines": len(text.splitlines()),
                        "tool": tool_name,
                    },
                )
            except Exception:
                return model_text
        except Exception:
            pass
    return text


def _prepare_tools(ctx: RunContext[WorkspaceDeps], tool_defs: list) -> list:
    """P0-5: native prepare_tools — hide write tools in plan mode before model sees them."""
    try:
        from tools.chat_mode import get_mode

        chat_id = str(getattr(ctx.deps, "chat_id", "") or "")
        if chat_id and get_mode(chat_id) == "plan":
            from tools.plan_mode_guard import WRITE_CLASS_TOOLS

            filtered = [td for td in tool_defs if getattr(td, "name", "") not in WRITE_CLASS_TOOLS]
            return filtered
    except Exception:
        pass
    return tool_defs


def _before_tool_validate(
    ctx: RunContext[WorkspaceDeps],
    *,
    call: ToolCallPart,
    tool_def: Any,
    args: Any,
) -> Any:
    """P0-5: veto earlier than before_tool_execute — at validation stage.

    Denial (``ToolDenied``) is allowed here, but deferral (``ApprovalRequired``)
    is NOT: a tool call may only be deferred once its arguments are validated
    (pydantic_ai.tool_manager._validate_hook_deferral_error). ApprovalRequired
    therefore lives in ``_before_tool_execute`` instead.
    """
    try:
        from tools.plan_mode_guard import plan_mode_guard

        _args_dict = dict(args) if isinstance(args, dict) else {}
        reason = plan_mode_guard(
            {"name": call.tool_name, "args": _args_dict, "scope": ctx.deps.role, "deps": ctx.deps}
        )
        if reason:
            from pydantic_ai.tools import ToolDenied

            raise ToolDenied(reason)
    except Exception as e:
        if e.__class__.__name__ == "ToolDenied":
            raise
    return args


def _registry_thinking(role: str) -> Optional[str]:
    """Registry thinking profile for a role (None = built-in role buckets).

    Byte-compat: the registry specs carry today's literal values
    (Developer/DFD high, General/House low); an explicit caller effort always
    wins, and any lookup failure falls back to `_build_model_settings` buckets.
    """
    try:
        from agents.workspace_registry import get_profile

        value = str(get_profile(role).get("thinking") or "inherit").strip().lower()
        return None if value in ("", "inherit") else value
    except Exception:
        return None


def _on_model_request_error(ctx: RunContext[WorkspaceDeps], *, request_context: Any, error: Exception) -> Any:
    """P0-5: surface 429/retry_after for logging; let tenacity/HTTPX handle retry."""
    try:
        from llm.llm import _is_retryable_http_error, _parse_retry_after

        err_str = str(error) or str(getattr(error, "body", "") or "")
        if _is_retryable_http_error(err_str):
            retry_after = _parse_retry_after(err_str)
            # Log for observability; actual backoff is in provider transport (P1-3).
            try:
                from llm.llm import log_terminal

                log_terminal(f"[model_request_error:{ctx.deps.role}] retryable={err_str[:200]} retry_after={retry_after}")
            except Exception:
                pass
    except Exception:
        pass
    # Context-window overflow diagnostics: the window-aware ProcessHistory
    # trigger should keep requests below the limit; when a provider still
    # rejects, say exactly why so the run fails loudly instead of silently.
    try:
        from llm.free_providers import _is_context_window_error
        from llm.model_history import estimate_history_tokens

        err_str = str(error) or str(getattr(error, "body", "") or "")
        if _is_context_window_error(error):
            est = 0
            try:
                est = estimate_history_tokens(getattr(ctx, "messages", []) or [])
            except Exception:
                pass
            try:
                from llm.llm import log_terminal

                log_terminal(
                    f"[context-overflow:{ctx.deps.role}] estimated={est:,} tok "
                    f"window={getattr(ctx.deps, 'context_window', 0):,} — "
                    f"compaction will fire on the next request if history fits; "
                    f"error={err_str[:200]}"
                )
            except Exception:
                pass
    except Exception:
        pass
    # Returning None lets the framework propagate the error (no recovery).
    return None


# ── Agent builder ────────────────────────────────────────────────────────────

def _compact_args(args: dict, cap: int = 400) -> str:
    """Render tool args for the approval prompt (bounded, JSON-dumped)."""
    try:
        import json as _json
        s = _json.dumps(args, ensure_ascii=False, separators=(",", ":"))
        return s[:cap] + ("…" if len(s) > cap else "")
    except Exception:
        return str(args)[:cap]


async def _blocking_ask_user(question: str, chat_id: str = "", timeout: int = 180) -> bool:
    """Blocking human approve/deny via the existing ask_user UX (inbox).

    Runs ask_user in a worker thread (it blocks) and awaits it. On timeout /
    error / cancellation the call is DENIED by policy (safe default).
    """
    import asyncio

    def _ask():
        try:
            from tools.AddOnTool.orchestrationTools import ask_user
            answer = ask_user(
                [{
                    "question": question,
                    "options": ["Approve", "Deny"],
                }],
                timeout=timeout,
                chat_id=chat_id or None,
            )
            return bool(answer) and "approve" in str(answer).lower()
        except Exception:
            return False

    try:
        loop = asyncio.get_running_loop()
        return await asyncio.wait_for(
            asyncio.to_thread(_ask),
            timeout=timeout + 15,
        )
    except asyncio.TimeoutError:
        return False
    except Exception:
        return False



def build_agent(
    role: str,
    model: Any,
    tools: Optional[list[dict]] = None,
    deps: Optional[WorkspaceDeps] = None,
    output_type: Optional[type] = None,
    system_prompt: str = "",
    extra_capabilities: Optional[list] = None,
) -> Agent:
    """Build a pydantic-ai Agent for one role from preserved schemas.

    - Tools come from ``tools`` (OpenAI-native schema dicts) — the EXACT
      schema the loops already send, so the model sees identical descriptions/
      parameters.
    - Scope = ``deps.allowed_tools`` (typed) else ``_allowed_for(role)``.
    - Pipeline hooks: cache / plan-guard (before), redact / offload (after).
    """
    deps = deps or WorkspaceDeps.from_shared_state(role=role)
    impl_map = _tool_map()
    schemas = _schemas_by_name()

    tool_objs: list[Tool] = []
    for t in tools or []:
        fn = t.get("function", t) if isinstance(t, dict) else t
        name = fn.get("name", "")
        if not name:
            continue
        impl = impl_map.get(name)
        # C2: MCP tools (mcp__server__tool) have no Python impl in TOOL_MAP —
        # wire them to the MCP bridge so they are real tools on the native
        # path (previously silently skipped).
        is_mcp = name.startswith("mcp__")
        if impl is None and is_mcp:
            try:
                from tools.mcp.mcp_bridge import mcp_execute

                def _mcp_impl(**kw):
                    return mcp_execute(name, dict(kw), scope=deps.role)

                impl = _mcp_impl
            except Exception:
                impl = None
        if impl is None:
            continue
        # Preserved schema (your refined compact definition) wins; fall back to
        # the wire schema when a tool isn't in TOOL_DEFINITIONS (e.g. mcp__*).
        schema_def = schemas.get(name)
        params = None
        description = ""
        if schema_def is not None:
            params = schema_def.get("parameters") or None
            description = schema_def.get("description", "")
        if params is None:
            params = fn.get("parameters") or {"type": "object", "properties": {}}
        if not description:
            description = fn.get("description", "")

        try:
            sig = inspect.signature(impl)
            ctx_params = [
                p for p in sig.parameters if p.lower() in ("ctx", "deps", "run_ctx", "context")
            ]
        except Exception:
            ctx_params = []

        # B1: native args_validator replaces the hand-rolled _validate closure.
        # from_schema skips schema validation; args_validator re-checks the
        # preserved schema and raises ModelRetry so the model retries (native
        # Tool.max_retries). The preserved schema stays the model-facing
        # contract; nothing about it changes.
        from pydantic_ai.exceptions import ModelRetry
        schema_props = (params or {}).get("properties", {}) or {}
        schema_required = set((params or {}).get("required", []) or [])

        # NOTE: bind per-tool schema via defaults — this def lives in a
        # per-tool loop, so bare closures would all share the LAST tool's
        # schema (late binding). Same for _bound/_plain below.
        def _validate(ctx, _props=schema_props, _req=schema_required, **args) -> None:
            missing = _req - set(args.keys())
            if missing:
                raise ModelRetry(
                    f"Missing required arguments: {', '.join(sorted(missing))}"
                )
            for k, v in args.items():
                if k not in _props:
                    raise ModelRetry(f"Unexpected argument '{k}' not in tool schema")
                meta = _props.get(k) or {}
                if v is None:
                    continue
                t = meta.get("type")
                if t == "string" and not isinstance(v, str):
                    raise ModelRetry(f"Argument '{k}' must be a string")
                if t == "integer" and not isinstance(v, int):
                    raise ModelRetry(f"Argument '{k}' must be an integer")
                if t == "number" and not isinstance(v, (int, float)):
                    raise ModelRetry(f"Argument '{k}' must be a number")
                if t == "boolean" and not isinstance(v, bool):
                    raise ModelRetry(f"Argument '{k}' must be a boolean")
                if t == "array" and not isinstance(v, list):
                    raise ModelRetry(f"Argument '{k}' must be an array")

        # P0-6: write-class tools (edit_file/create_project/
        # delegate/call_subagent/... ) run as BARRIERS: tools emitted before them
        # finish first, tools after them start after they finish.
        # run_command is NOT a universal barrier — only destructive shells
        # need it. Read-only shells (ls/cat/find/grep/git status etc — see
        # tools.plan_mode_guard._is_destructive_shell) do not mutate the file
        # tree and can parallelize with sibling reads. Because sequential is
        # per-tool (not per-call) we mark run_command non-sequential here;
        # destructive detection + cache invalidation happens per-call in
        # tools/MainTool/runCommand.py (only destructive clears _FILE_CACHE).
        try:
            from tools.plan_mode_guard import WRITE_CLASS_TOOLS as _WRITE_CLASS
            _sequential = name in _WRITE_CLASS
        except Exception:
            _sequential = name in ("edit_file", "create_project")

        if ctx_params:
            ctx_name = ctx_params[0]

            def _bound(_impl=impl, _ctx_name=ctx_name, _deps=deps, **kw):
                return _impl(**kw, **{_ctx_name: _deps})

            _tool = Tool.from_schema(
                _bound,
                name=name,
                description=description,
                json_schema=params,
                args_validator=_validate,
                sequential=_sequential,
            )
            tool_objs.append(_tool)
        else:
            def _plain(_impl=impl, **kw):
                return _impl(**kw)

            _tool = Tool.from_schema(
                _plain,
                name=name,
                description=description,
                json_schema=params,
                args_validator=_validate,
                sequential=_sequential,
            )
            tool_objs.append(_tool)

    # Scope via FilteredToolset (native replacement for restrict_scope).
    # The caller already pre-scopes via get_native_tools(role); the registry
    # allowlist is a SECOND gate. A tool runs only if it's in BOTH the passed
    # tools and the scope allowlist. When the intersection is empty (unknown
    # role / external/test tool not in registry), fall back to the passed
    # tool names so nothing is hidden.
    passed_names = set()
    for t in (tools or []):
        fn = t.get("function", t) if isinstance(t, dict) else t
        n = fn.get("name", "") if isinstance(fn, dict) else ""
        if n:
            passed_names.add(n)
    scope_allowed = set(deps.allowed_tools or _allowed_for(role))
    # MCP tools are a per-scope dynamic surface (tools/mcp/mcp_bridge.py) and
    # cannot live in the static spec allow-list; union them so the second gate
    # does not silently drop mcp__* defs the caller passed in.
    try:
        from tools.mcp.mcp_bridge import mcp_tool_names_for_scope

        scope_allowed |= set(mcp_tool_names_for_scope(role))
    except Exception:
        pass
    allowed = scope_allowed & passed_names if scope_allowed else set()
    if not allowed and passed_names:
        allowed = passed_names
    # P0-4: per-tool retry 1, agent-level budget controls total retries.
    # ModelRetry (args_validator) consumes tool budget; ToolFailed is terminal.
    function_toolset = FunctionToolset(tool_objs, max_retries=1)
    if allowed:
        toolset = FilteredToolset(
            function_toolset,
            filter_func=lambda ctx, td: td.name in allowed,
        )
    else:
        toolset = function_toolset

    hooks = Hooks(
        prepare_tools=_prepare_tools,
        before_tool_validate=_before_tool_validate,
        before_tool_execute=_before_tool_execute,
        tool_execute=_wrap_tool_execute,
        after_tool_execute=_after_tool_execute,
        model_request_error=_on_model_request_error,
    )
    capabilities: list = [hooks]
    # P0-3: Instrumentation — OTel gen_ai spans (version 5). Enabled by default,
    # disable with PAI_INSTRUMENT=0. Provides agent_run/model_request/tool spans
    # without manual token-stats double counting.
    if os.getenv("PAI_INSTRUMENT", "1") != "0":
        try:
            from pydantic_ai.capabilities import Instrumentation
            from pydantic_ai.models.instrumented import InstrumentationSettings

            capabilities.append(Instrumentation(InstrumentationSettings(version=5)))
        except Exception:
            pass
    # P0-1: portable thinking via ModelSettings is primary; Thinking capability
    # kept opt-in for env PAI_THINKING_CAP=1 with static per-role mapping:
    # developer/dfd=high, orchestrator/general/house=low.
    if os.getenv("PAI_THINKING_CAP", "0") == "1":
        try:
            from pydantic_ai.capabilities.thinking import Thinking

            _r = (role or "").lower()
            if "developer" in _r or _r in ("dfd", "designer", "design") or "dfd" in _r:
                _thinking_effort = "high"  # type: ignore[assignment]
            elif _r == "orchestrator":
                _thinking_effort = "low"  # type: ignore[assignment]
            else:
                _thinking_effort = "low"  # general/house/supervisor → low
            capabilities.append(Thinking(effort=_thinking_effort))  # type: ignore[arg-type]
        except Exception:
            pass
    # A7: native compaction via ProcessHistory (before each model request)
    try:
        from llm.compaction_native import get_process_history_capability

        _ph = get_process_history_capability()
        if _ph is not None:
            capabilities.append(_ph)
    except Exception:
        pass
    # Vision: tools execute INSIDE the native run, so images they queue
    # (look_at_screen / read_file on images / browser screenshots) must be
    # injected before the NEXT model request — not after the run returns.
    try:
        from tools.vision_channel import get_vision_history_capability

        _vh = get_vision_history_capability()
        if _vh is not None:
            capabilities.append(_vh)
    except Exception:
        pass
    # P2-2: WebSearch + MCP native capabilities (env-gated)
    if os.getenv("PAI_WEB_SEARCH", "0") == "1":
        try:
            from pydantic_ai.capabilities.web_search import WebSearch

            capabilities.append(WebSearch(max_uses=5))
        except Exception:
            pass
    _mcp_url = os.getenv("PAI_MCP_URL", "").strip()
    if _mcp_url:
        try:
            from pydantic_ai.capabilities.mcp import MCP

            capabilities.append(MCP(url=_mcp_url, native=True))
        except Exception:
            try:
                from pydantic_ai.mcp import MCPToolset

                # Fallback: local MCPToolset if native fails
                capabilities.append(MCPToolset(_mcp_url))
            except Exception:
                pass
    # P2-3: deferred/approval handling — when PAI_DEFERRED=1, surface plan-mode
    # writes to the human for approve/deny (blocking ask_user, matching the
    # orchestrator's existing UX) instead of auto-denying.
    if os.getenv("PAI_DEFERRED", "0") == "1":
        try:
            from pydantic_ai.capabilities.deferred_tool_handler import HandleDeferredToolCalls

            async def _defer_handler(ctx: RunContext[WorkspaceDeps], reqs):
                try:
                    from pydantic_ai.tools import DeferredToolResults

                    results = []
                    for req in getattr(reqs, "requests", []) or []:
                        call = getattr(req, "call", None) or getattr(req, "tool_call", None)
                        name = getattr(call, "tool_name", "unknown") if call else "unknown"
                        call_args = {}
                        try:
                            raw_args = getattr(call, "args", None) or {}
                            if isinstance(raw_args, str):
                                import json as _json
                                call_args = _json.loads(raw_args) if raw_args else {}
                        except Exception:
                            call_args = {}
                        chat_id = getattr(ctx.deps, "chat_id", "") or ""
                        reason = ""
                        try:
                            meta = getattr(req, "metadata", None) or {}
                            if isinstance(meta, dict):
                                reason = str(meta.get("reason") or "")
                        except Exception:
                            pass
                        question = (
                            f"Approve {name} call?\nArgs: {_compact_args(call_args)}\n"
                            f"Reason: {reason or 'Deferred tool call'}"
                        )
                        approved = await _blocking_ask_user(question, chat_id=chat_id)
                        if approved:
                            results.append(req.approve())
                        else:
                            from pydantic_ai.tools import ToolDenied
                            results.append(req.deny(f"[BLOCKED] {reason or f'{name} call was not approved by the user.'}"))
                    if results:
                        return DeferredToolResults(results=results)
                except Exception:
                    pass
                return None

            capabilities.append(HandleDeferredToolCalls(handler=_defer_handler))
        except Exception:
            pass
    if extra_capabilities:
        capabilities.extend(extra_capabilities)

    kwargs: dict[str, Any] = {}
    if output_type is not None:
        kwargs["output_type"] = output_type
    if system_prompt:
        kwargs["system_prompt"] = system_prompt
    # P0-4: agent-level retry budgets (env-overridable).
    try:
        _tool_retries = int(os.getenv("PAI_TOOL_RETRIES", "3"))
    except Exception:
        _tool_retries = 3
    try:
        _output_retries = int(os.getenv("PAI_OUTPUT_RETRIES", "1"))
    except Exception:
        _output_retries = 1
    if _NATIVE_LIMITER is not None and os.getenv("PAI_USE_CONCURRENCY", "1") != "0":
        kwargs["max_concurrency"] = _NATIVE_LIMITER
    agent = Agent(
        model=model,
        name=role or "agent",
        toolsets=[toolset],
        capabilities=capabilities,
        deps_type=WorkspaceDeps,
        retries={"tools": _tool_retries, "output": _output_retries},
        **kwargs,
    )
    return agent


def build_native_model(
    role: str,
    model: Any = None,
    *,
    deps: Optional[WorkspaceDeps] = None,
    temp: float = 0.2,
    reasoning_effort: Optional[str] = None,
    reasoning_budget: Optional[int] = None,
    base_url: Optional[str] = None,
    api_key: Optional[str] = None,
) -> tuple[Any, str, int]:
    """Build the provider-backed model for a role and return its window.

    A concrete model object is returned unchanged. String/None inputs use the
    same provider resolution as ``arun_native_agent``. This is the single
    model builder used by both chat turns and same-model compaction.
    """
    from llm.pai_transport import _resolve_provider_cfg, _model_name
    from llm.pai_transport import _build_model_settings, _model_profile_for

    deps = deps or WorkspaceDeps.from_shared_state(role=role)
    if model is not None and not isinstance(model, str):
        mid = str(getattr(model, "model_name", "") or "")
        window = int(getattr(deps, "context_window", 0) or 0)
        if window <= 0 and mid:
            try:
                from llm.model_history import window_for_model

                window = int(window_for_model(mid) or 0)
            except Exception:
                window = 0
        return model, mid, window

    cfg = deps.llm_provider or _resolve_provider_cfg()
    base_url = base_url or str(cfg.get("base") or "")
    api_key = api_key or str(cfg.get("key") or "")
    if model is None:
        model_id = _model_name("deepseek:v4.1-flash")
    else:
        model_id = _model_name(model)
    if reasoning_effort is None:
        reasoning_effort = _registry_thinking(role)
    settings = _build_model_settings(role, model_id, temp, reasoning_effort, reasoning_budget)
    profile = _model_profile_for(model_id)
    from pydantic_ai.models.openai import OpenAIChatModel
    from pydantic_ai.providers.openai import OpenAIProvider

    provider = OpenAIProvider(base_url=base_url, api_key=api_key)
    if profile:
        model_obj = OpenAIChatModel(
            model_name=model_id, provider=provider, settings=settings, profile=profile
        )
    else:
        model_obj = OpenAIChatModel(
            model_name=model_id, provider=provider, settings=settings
        )
    window = 0
    try:
        from llm.model_history import window_for_model

        window = int(window_for_model(model_id) or 0)
    except Exception:
        window = 0
    return model_obj, model_id, window


async def arun_native_agent(
    role: str,
    messages: list,
    tools: Optional[list[dict]] = None,
    model: Any = None,
    temp: float = 0.2,
    reasoning_effort: Optional[str] = None,
    reasoning_budget: Optional[int] = None,
    where: Optional[str] = None,
    deps: Optional[WorkspaceDeps] = None,
    system_prompt: str = "",
    stream_emit: bool = True,
    base_url: Optional[str] = None,
    api_key: Optional[str] = None,
    cancellation_token: Optional[Any] = None,
) -> tuple[str, list, Optional[str]]:
    """Async full pydantic-ai Agent turn with REAL tool execution (A1).

    The agent executes tools internally (parallel by default, retry on invalid
    args). Returns ``(text, new_lc_messages, reasoning)`` where
    ``new_lc_messages`` is the LangChain conversation slice added by this run
    (assistant text/tool-calls + tool results) so the caller's loop can keep
    its history/compaction/session machinery.

    ``model`` may be an OpenAIChatModel instance (provider already resolved)
    or None (resolved from deps like pai_native_turn). ``cancellation_token``
    is a pydantic-ai ``CancellationToken`` (A8) propagated to the run.
    """
    from pydantic_ai import Agent
    from pydantic_ai.exceptions import UnexpectedModelBehavior
    from llm.pai_transport import _emit_stream

    deps = deps or WorkspaceDeps.from_shared_state(role=role)
    model, model_id, _window = build_native_model(
        role,
        model,
        deps=deps,
        temp=temp,
        reasoning_effort=reasoning_effort,
        reasoning_budget=reasoning_budget,
        base_url=base_url,
        api_key=api_key,
    )
    if _window > 0 and not getattr(deps, "context_window", 0):
        deps.context_window = int(_window)
    # Expose the exact chat model to the ProcessHistory processor so
    # compaction uses the same model as the turn.
    deps.compaction_model = model
    deps.compaction_model_id = model_id

    # ── Per-request profiler (PAI_REQ_PROFILE=1) ─────────────────────────────
    # Wraps the model's request methods so EVERY model request records:
    #   (a) the real byte composition of the messages about to be sent,
    #       bucketed by part kind (system-prompt / user-prompt / text /
    #       thinking / tool-call / tool-return ...) — i.e. WHAT each request
    #       actually contains, and
    #   (b) the provider's per-request token usage (input / cache_read /
    #       cache_write / output) — i.e. how much of that request was a miss.
    # The eval report then attributes cache-miss per request from real data.
    # Telemetry-only; off by default; fully guarded — never fails the run.
    if os.getenv("PAI_REQ_PROFILE", "0") == "1":
        try:
            _req_log = getattr(deps, "_req_log", None)
            if _req_log is None:
                _req_log = []
                deps._req_log = _req_log

            def _msg_bytes(messages) -> dict:
                buckets: dict = {}
                for _m in messages:
                    for _p in getattr(_m, "parts", []) or []:
                        _kind = str(getattr(_p, "part_kind", "?") or "?")
                        _c = getattr(_p, "content", "") or ""
                        _n = len(_c) if isinstance(_c, str) else len(str(_c))
                        _a = getattr(_p, "args", None)
                        if _a is not None:
                            _n += len(_a) if isinstance(_a, str) else len(str(_a))
                        buckets[_kind] = buckets.get(_kind, 0) + _n
                        buckets["_parts"] = buckets.get("_parts", 0) + 1
                return buckets

            _orig_req = model.request
            _orig_req_stream = getattr(model, "request_stream", None)

            async def _prof_req(*args, **kwargs):
                _messages = args[0] if args else (kwargs.get("messages") or [])
                _res, _u = await _orig_req(*args, **kwargs)
                try:
                    _row = {"i": len(_req_log), "b": _msg_bytes(_messages)}
                    if _u is not None:
                        _row.update({
                            "in": getattr(_u, "input_tokens", 0) or 0,
                            "cr": getattr(_u, "cache_read_tokens", 0) or 0,
                            "cw": getattr(_u, "cache_write_tokens", 0) or 0,
                            "out": getattr(_u, "output_tokens", 0) or 0,
                        })
                    _req_log.append(_row)
                except Exception:
                    pass
                return _res, _u

            class _ProfStreamCM:
                """Async-context-manager proxy: records the request's byte
                composition at call time and the provider usage once the
                streamed response has been consumed (resp.usage)."""

                def __init__(self, inner_cm, messages, log):
                    self._cm = inner_cm
                    self._msgs = messages
                    self._log = log

                async def __aenter__(self):
                    self._resp = await self._cm.__aenter__()
                    return self._resp

                async def __aexit__(self, *exc):
                    _r = await self._cm.__aexit__(*exc)
                    try:
                        _u = getattr(self._resp, "usage", None)
                        _row = {"i": len(self._log), "b": _msg_bytes(self._msgs), "stream": True}
                        if _u is not None:
                            _row.update({
                                "in": getattr(_u, "input_tokens", 0) or 0,
                                "cr": getattr(_u, "cache_read_tokens", 0) or 0,
                                "cw": getattr(_u, "cache_write_tokens", 0) or 0,
                                "out": getattr(_u, "output_tokens", 0) or 0,
                            })
                        self._log.append(_row)
                    except Exception:
                        pass
                    return _r

            def _prof_req_stream(*args, **kwargs):
                _messages = args[0] if args else (kwargs.get("messages") or [])
                return _ProfStreamCM(_orig_req_stream(*args, **kwargs), _messages, _req_log)

            model.request = _prof_req  # type: ignore[method-assign]
            if _orig_req_stream is not None:
                model.request_stream = _prof_req_stream  # type: ignore[method-assign]
        except Exception:
            pass

    # A6b: accept both LangChain messages and native ModelMessage history.
    # Detect native history by checking for ModelRequest/ModelResponse instances.
    from pydantic_ai.messages import ModelMessage, ModelRequest, ModelResponse
    from llm.pai_transport import _messages_to_pai, _split_user_prompt

    full: list[ModelMessage]
    if messages and isinstance(messages[0], (ModelRequest, ModelResponse)):
        # Already native ModelMessage list — use directly (A6b path)
        full = list(messages)  # type: ignore
    else:
        full = _messages_to_pai(messages)
    user_prompt, history = _split_user_prompt(full)

    events_log: list[dict] = []
    _text_start_seen_nt: dict[int, str] = {}
    # tool_call_id -> (start_monotonic, tool_name, args_json) for ms + finish matching
    _tool_start_nt: dict[str, tuple] = {}
    _tel_debug_on = os.getenv("PAI_TEL_DEBUG", "0") == "1"

    def _tel_dbg():
        try:
            d = getattr(deps, "_tel_dbg", None)
            if d is None:
                d = {"think_start_chars": 0, "think_delta_chars": 0, "deltas_after_start": 0}
                deps._tel_dbg = d
            return d
        except Exception:
            return None

    def _on_event(ctx, event):
        from llm.pai_transport import _emit_tool_frame
        from pydantic_ai.messages import (
            PartStartEvent, PartDeltaEvent, TextPart, ThinkingPart,
            TextPartDelta, ThinkingPartDelta,
            FunctionToolCallEvent, FunctionToolResultEvent, ToolReturnPart,
        )
        try:
            if isinstance(event, FunctionToolCallEvent):
                part = event.part
                cid = event.tool_call_id or ""
                tname = str(getattr(part, "tool_name", "") or "")
                try:
                    import json as _json
                    pargs = getattr(part, "args", {})
                    args_json = pargs if isinstance(pargs, str) else _json.dumps(pargs or {}, default=str)
                except Exception:
                    args_json = ""
                _tool_start_nt[cid] = (time.monotonic(), tname, args_json[:500])
                if stream_emit:
                    _emit_tool_frame(role, "tool_started", tname, args_json[:500],
                                     chat_id=getattr(deps, "chat_id", "") or "")
            elif isinstance(event, FunctionToolResultEvent):
                part = event.part
                cid = getattr(part, "tool_call_id", "") or ""
                started = _tool_start_nt.pop(cid, None)
                ms = int((time.monotonic() - started[0]) * 1000) if started else 0
                tname = started[1] if started else str(getattr(part, "tool_name", "") or "")
                args_json = started[2] if started else ""
                if isinstance(part, ToolReturnPart):
                    content = part.content
                    preview = content if isinstance(content, str) else str(content or "")
                    bad = preview.lstrip().startswith(
                        ("Error:", "Error ", "[ERR]", "[ERROR]", "[BLOCKED]"))
                    ok: bool | None = not bad
                else:
                    preview = str(getattr(part, "content", "") or "") or "validation retry requested"
                    ok = False
                if stream_emit:
                    _emit_tool_frame(role, "tool_finished", tname, args_json,
                                     ok=ok, ms=ms, preview=preview[:300],
                                     chat_id=getattr(deps, "chat_id", "") or "")
            elif isinstance(event, PartStartEvent):
                if isinstance(event.part, ThinkingPart):
                    c = event.part.content or ""
                    if _tel_debug_on:
                        dbg = _tel_dbg()
                        if dbg is not None:
                            dbg["think_start_chars"] += len(c)
                    if c:
                        events_log.append({"kind": "reasoning", "text": c})
                        if stream_emit:
                            _emit_stream(role, "reasoning", c, where or "")
                elif isinstance(event.part, TextPart):
                    c = event.part.content or ""
                    if c:
                        _text_start_seen_nt[event.index] = c
                        events_log.append({"kind": "text_start", "text": c, "index": event.index})
                    else:
                        _text_start_seen_nt[event.index] = ""
            elif isinstance(event, PartDeltaEvent):
                d = event.delta
                if isinstance(d, TextPartDelta):
                    if event.index in _text_start_seen_nt:
                        events_log[:] = [e for e in events_log if not (e.get("kind") == "text_start" and e.get("index") == event.index)]
                        _text_start_seen_nt.pop(event.index, None)
                    c = d.content_delta or ""
                    if c:
                        events_log.append({"kind": "text", "text": c})
                        if stream_emit:
                            _emit_stream(role, "text", c, where or "")
                elif isinstance(d, ThinkingPartDelta):
                    c = d.content_delta or ""
                    if _tel_debug_on and c:
                        dbg = _tel_dbg()
                        if dbg is not None:
                            dbg["think_delta_chars"] += len(c)
                            dbg["deltas_after_start"] += 1
                    if c:
                        events_log.append({"kind": "reasoning", "text": c})
                        if stream_emit:
                            _emit_stream(role, "reasoning", c, where or "")
        except Exception:
            pass
        return event

    hooks = Hooks(event=_on_event)
    agent = build_agent(
        role=role,
        model=model,
        tools=tools,
        deps=deps,
        system_prompt=system_prompt,
        extra_capabilities=[hooks],
    )

    from llm.native_tools import _default_usage_limits as _fresh_limits

    # P1-1: prefer Agent.iter (single AgentRun, correct usage/cancellation,
    # no per-turn Agent rebuild). Falls back to run_stream on error.
    use_iter = os.getenv("PAI_USE_ITER", "1") != "0"
    all_msgs: list = []
    result_output: Any = None
    result_usage: Any = None
    if use_iter:
        try:
            async with agent.iter(
                user_prompt,
                message_history=history,
                deps=deps,
                usage_limits=_fresh_limits(),
                cancellation_token=cancellation_token,
            ) as run:
                async for node in run:
                    # Hooks(event=) already streams deltas; node loop keeps
                    # usage/cancellation correct and drains CallToolsNode properly.
                    # For ModelRequestNode we could stream, but event handler covers it.
                    pass
                # run.result is AgentRunResult
                _res = run.result
                all_msgs = list(_res.all_messages())
                result_output = _res.output
                result_usage = getattr(_res, "usage", None)
        except Exception as _iter_err:
            # Fallback will use run_stream; log for diagnostics.
            try:
                from llm.llm import log_terminal

                log_terminal(f"[arun_native_agent] iter failed, fallback to run_stream: {_iter_err}")
            except Exception:
                pass
            all_msgs = []

    if not all_msgs:
        # Flush deferred start texts before fallback run (otherwise duplicated on next try)
        for _idx, _c in list(_text_start_seen_nt.items()):
            if _c:
                events_log.append({"kind": "text", "text": _c})
        _text_start_seen_nt.clear()
        events_log[:] = [e for e in events_log if e.get("kind") != "text_start"]

        async def _run():
            async with agent.run_stream(
                user_prompt,
                message_history=history,
                deps=deps,
                usage_limits=_fresh_limits(),
                cancellation_token=cancellation_token,
            ) as sres:
                async for _resp in sres.stream_response(debounce_by=None):
                    pass
                return sres

        _sres = await _run()
        all_msgs = list(_sres.all_messages())
        result_output = _sres.output if hasattr(_sres, "output") else None
        result_usage = getattr(_sres, "usage", None)

    # Flush deferred text_start prefixes that never got a delta (full text in start)
    for _idx, _c in list(_text_start_seen_nt.items()):
        if _c:
            events_log.append({"kind": "text", "text": _c})
            try:
                if stream_emit:
                    _emit_stream(role, "text", _c, where or "")
            except Exception:
                pass
    _text_start_seen_nt.clear()
    # Drop any leftover text_start entries that were not flushed
    events_log[:] = [e for e in events_log if e.get("kind") != "text_start"]
    text = "".join(
        p.content
        for m in all_msgs
        if isinstance(m, ModelResponse)
        for p in m.parts
        if getattr(p, "part_kind", "") == "text"
    ).strip()
    if not text:
        _fallback = "".join(e["text"] for e in events_log if e["kind"] == "text").strip()
        text = _fallback or str(result_output if isinstance(result_output, str) else (result_output or ""))
    reasoning = "".join(
        e["text"] for e in events_log if e["kind"] == "reasoning"
    ).strip() or None
    # Fallback: if event log empty but response has thinking parts, use those.
    if not reasoning:
        try:
            _thinking = "".join(
                p.content
                for m in all_msgs
                if isinstance(m, ModelResponse)
                for p in m.parts
                if getattr(p, "part_kind", "") == "thinking"
            ).strip()
            if _thinking:
                reasoning = _thinking
        except Exception:
            pass

    # A6b: always return the native ModelMessage slice (P0-LLM — the LC
    # reverse-convert path _pai_to_lc is deleted).
    prior_ids = {id(m) for m in history}
    new_msgs = [m for m in all_msgs if id(m) not in prior_ids]
    # ── Usage aggregation (telemetry honesty) ──────────────────────────────
    # run_stream's facade can report only the LAST request (observed: requests=1
    # and tool_calls_total=0 for a 53-tool run). The complete aggregate is
    # derivable from each ModelResponse's RequestUsage in the message slice, so
    # build it here and prefer it unless the facade has more requests.
    _agg_tool_calls = 0
    try:
        from pydantic_ai.usage import RunUsage as _RunUsage
        _agg = None
        for _m in all_msgs:
            if isinstance(_m, ModelResponse):
                _u = getattr(_m, "usage", None)
                if _u is not None:
                    if _agg is None:
                        _agg = _RunUsage()
                    try:
                        _agg.incr(_u)
                    except Exception:
                        pass
                for _p in getattr(_m, "parts", []) or []:
                    if isinstance(_p, ToolCallPart):
                        _agg_tool_calls += 1
        if _agg is not None:
            _fb_requests = int(getattr(result_usage, "requests", 0) or 0) if result_usage is not None else 0
            if _fb_requests <= int(getattr(_agg, "requests", 0) or 0):
                result_usage = _agg
    except Exception:
        pass
    # Telemetry (shared prod+eval path): report PydanticAI RunUsage into the
    # shared token stats. Telemetry-only — never affects the run; guarded so
    # it cannot fail the agent turn.
    try:
        from llm.pai_transport import _pai_usage_to_stats
        try:
            _tel_model = model_id
        except NameError:
            _tel_model = getattr(model, "model_name", "") or ""
        _extra = {}
        try:
            _u = result_usage
            _extra["requests"] = getattr(_u, "requests", None)
            _extra["tool_calls_total"] = getattr(_u, "tool_calls", None)
        except Exception:
            pass
        try:
            _tl = getattr(deps, "_tool_timings", None)
            _tc = getattr(deps, "_tool_counts", None)
            _tq = getattr(deps, "_tool_seq", None)
            _rq = getattr(deps, "_req_log", None)
            _ml = getattr(deps, "_map_live", None)
            if _tl:
                _extra["tool_ms"] = dict(_tl)
            if _tc:
                _extra["tool_counts"] = dict(_tc)
            if _tq:
                _extra["tool_seq"] = list(_tq)
            if _rq:
                _extra["req_log"] = list(_rq)
            if _ml:
                _extra["map_live_deltas"] = int(_ml)
            # tool_calls_total must match what actually executed.
            _tc_sum = sum(v for v in (_tc or {}).values() if isinstance(v, int))
            _extra["tool_calls_total"] = max(
                int(_extra.get("tool_calls_total") or 0), _tc_sum, _agg_tool_calls)
        except Exception:
            pass
        if _tel_debug_on:
            try:
                dbg = getattr(deps, "_tel_dbg", None)
                if dbg:
                    _extra["tel_dbg"] = dict(dbg)
            except Exception:
                pass
        _pai_usage_to_stats(
            role, _tel_model, result_usage, where,
            reasoning_effort, reasoning_budget,
            reasoning_chars=len(reasoning or ""),
            backend="paid",
            extra=_extra or None,
        )
    except Exception:
        pass
    return text, new_msgs, reasoning


def _run_coro_blocking(coro):
    """Run an async coro from sync code without blocking the caller's event loop.

    pydantic-ai Agent is async-native (Agent.iter / run_stream are async);
    this helper is the only sync↔async glue. If no running loop, use
    asyncio.run (fast path). If a loop is already running (FastAPI /
    server.realtime), run the coro in a dedicated thread so we don't hit
    'asyncio.run() cannot be called from a running event loop' and we don't
    block the server's loop. Stays 100% pydantic-ai — only the bridge changes.
    """
    import asyncio
    import concurrent.futures

    try:
        loop = asyncio.get_running_loop()
    except RuntimeError:
        loop = None
    if loop is not None and loop.is_running():
        with concurrent.futures.ThreadPoolExecutor(max_workers=1) as _ex:
            fut = _ex.submit(asyncio.run, coro)
            return fut.result()
    return asyncio.run(coro)


def run_native_agent(
    role: str,
    messages: list,
    tools: Optional[list[dict]] = None,
    model: Any = None,
    temp: float = 0.2,
    reasoning_effort: Optional[str] = None,
    reasoning_budget: Optional[int] = None,
    where: Optional[str] = None,
    deps: Optional[WorkspaceDeps] = None,
    system_prompt: str = "",
    stream_emit: bool = True,
    base_url: Optional[str] = None,
    api_key: Optional[str] = None,
    cancellation_token: Optional[Any] = None,
) -> tuple[str, list, Optional[str]]:
    """Sync facade over :func:`arun_native_agent` (A1).

    The agent executes tools internally (parallel by default, retry on invalid
    args). Returns ``(text, new_lc_messages, reasoning)`` where
    ``new_lc_messages`` is the LangChain conversation slice added by this run
    (assistant text/tool-calls + tool results) so the caller's loop can keep
    its history/compaction/session machinery.

    Async callers (server loop) should prefer :func:`arun_native_agent`
    directly; this facade now also handles the running-loop case via a
    thread bridge so it never blocks the server's event loop. Pure pydantic-ai
    upgrade — no transport change.
    """
    return _run_coro_blocking(
        arun_native_agent(
            role=role,
            messages=messages,
            tools=tools,
            model=model,
            temp=temp,
            reasoning_effort=reasoning_effort,
            reasoning_budget=reasoning_budget,
            where=where,
            deps=deps,
            system_prompt=system_prompt,
            stream_emit=stream_emit,
            base_url=base_url,
            api_key=api_key,
            cancellation_token=cancellation_token,
        )
    )


def _default_usage_limits() -> UsageLimits:
    """P0-3: cost/per-request guards. Env-overridable for A/B.

    PAI_UNLIMITED=1 removes every pydantic-ai limit (request/tool/token/cost)
    for long-horizon runs: the agent + the external gate decide when the work
    is done, not an arbitrary counter.
    """
    if os.getenv("PAI_UNLIMITED", "0") == "1":
        return UsageLimits(
            request_limit=None,
            tool_calls_limit=None,
            input_tokens_limit=None,
            output_tokens_limit=None,
            total_tokens_limit=None,
            per_request_input_tokens_limit=None,
            cost_limit=None,
            count_tokens_before_request=False,
        )
    try:
        cost_val = os.getenv("PAI_COST_LIMIT", "2.00").strip()
        cost = Decimal(cost_val) if cost_val else None
    except Exception:
        cost = Decimal("2.00")
    try:
        per_req = int(os.getenv("PAI_PER_REQUEST_INPUT_LIMIT", "80000"))
    except Exception:
        per_req = 80000
    try:
        total = int(os.getenv("PAI_TOTAL_TOKENS_LIMIT", "600000"))
    except Exception:
        total = 600000
    try:
        req_lim = int(os.getenv("PAI_REQUEST_LIMIT", "200"))
    except Exception:
        req_lim = 200
    try:
        tool_lim = int(os.getenv("PAI_TOOL_CALLS_LIMIT", "200"))
    except Exception:
        tool_lim = 200
    # count_tokens_before_request requires model.count_tokens support; enable
    # only when PAI_COUNT_BEFORE=1 (opt-in) to avoid extra latency by default.
    count_before = os.getenv("PAI_COUNT_BEFORE", "0") == "1"
    return UsageLimits(
        request_limit=req_lim,
        tool_calls_limit=tool_lim,
        per_request_input_tokens_limit=per_req,
        total_tokens_limit=total,
        cost_limit=cost,
        count_tokens_before_request=count_before,
    )


import threading as _threading

_WATCH_STOP = _threading.Event()


def chat_cancellation_token(chat_id: str) -> tuple[Any, Any]:
    """Build a pydantic-ai CancellationToken wired to the chat's abort flag (A8+P1-2).

    Returns ``(token, stop_watcher)``. ``stop_watcher()`` unregisters and stops
    the watcher. Abort now cancels via registry ``llm.cancellation.cancel`` for
    immediate (<10ms) delivery, with 0.25s poll as fallback for legacy callers.
    """
    from pydantic_ai import CancellationToken

    token = CancellationToken()
    _WATCH_STOP.clear()
    try:
        from llm.cancellation import register, unregister

        register(chat_id, token)
    except Exception:
        pass

    def _watch() -> None:
        import time

        try:
            from server.realtime.inbox import pending_counts
        except Exception:
            return
        while not _WATCH_STOP.is_set():
            try:
                counts = pending_counts(chat_id)
                if counts.get("abort_pending"):
                    try:
                        token.cancel()
                    except Exception:
                        pass
                    return
            except Exception:
                return
            time.sleep(0.25)

    # Prefer asyncio watcher if loop is running (no thread), else thread.
    _async_task = None
    try:
        import asyncio

        loop = asyncio.get_running_loop()
        if loop and loop.is_running():
            async def _awatch():
                try:
                    from server.realtime.inbox import pending_counts as _pc
                except Exception:
                    return
                while not _WATCH_STOP.is_set():
                    try:
                        if _pc(chat_id).get("abort_pending"):
                            try:
                                token.cancel()
                            except Exception:
                                pass
                            return
                    except Exception:
                        return
                    await asyncio.sleep(0.25)

            _async_task = loop.create_task(_awatch())
        else:
            raise RuntimeError("no running loop")
    except Exception:
        t = _threading.Thread(target=_watch, daemon=True)
        t.start()
        _async_task = None

    def _stop() -> None:
        _WATCH_STOP.set()
        try:
            from llm.cancellation import unregister as _unreg

            _unreg(chat_id)
        except Exception:
            pass
        if _async_task is not None:
            try:
                _async_task.cancel()
            except Exception:
                pass

    return token, _stop
