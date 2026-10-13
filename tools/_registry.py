"""
Tool registry shim — thin execution entry (replaces the legacy pipeline).

Phase 1 of the native migration: the scoped registry (allow/deny visibility),
the pre/post hooks (cache / plan-guard / redact / offload) and the
context-var scope machinery are GONE. pydantic-ai does all of that natively
in ``llm/native_tools.py`` (FilteredToolset = scope, Hooks = cache/guard/
redact/offload, parallel tool calls, ModelRetry validation).

What survives here is only what the remaining non-native callers need:

- :class:`ToolExecutionResult` — the structured outcome formatter (orchestrator
  and developer loops wrap tool results with ``from_content``).
- :func:`execute_tool_detailed` — direct call to the tool implementation
  (no scope filtering, no hooks) with ``ctx=deps``-kwarg detection for
  migrated tools.
- :class:`_LazyToolMap` + ``TOOL_MAP`` — the lazy facade used by
  ``native_tools._tool_map()``.
"""
from __future__ import annotations

import inspect
import re
from dataclasses import dataclass, replace
from typing import Any, Callable, Optional

from ._schemas import get_native_tool_names

@dataclass(frozen=True)
class ToolExecutionResult:
    """Structured result for one tool invocation.

    Tool implementations keep their compact text contract for compatibility,
    while the harness also exposes deterministic execution metadata.  The
    metadata is derived at the execution boundary so the loops do not have to
    infer success from arbitrary output prose.
    """

    name: str
    args: dict
    content: str
    is_error: bool
    exit_code: Optional[int] = None
    timed_out: bool = False
    offloaded: bool = False

    @classmethod
    def from_content(cls, name: str, args: dict, content: Any) -> "ToolExecutionResult":
        text = str(content or "")
        first_line = text.lstrip().splitlines()[0] if text.strip() else ""

        exit_code = None
        is_ok = None
        match = re.match(r"\[(OK|ERR)\]\s+Exit\s+(-?\d+)", first_line, re.IGNORECASE)
        if match:
            is_ok = match.group(1).upper() == "OK"
            exit_code = int(match.group(2))
        else:
            match = re.search(r"Exit\s+code\s+(-?\d+)", first_line, re.IGNORECASE)
            if match:
                exit_code = int(match.group(1))

        timed_out = "[TIMEOUT]" in text or "timed out" in text.lower()
        offloaded = "offloaded" in text.lower() or "/scratch/" in text.lower()
        explicit_error = text.lstrip().startswith((
            "Error:",
            "Error ",
            "[ERR]",
            "[ERROR]",
            "[BLOCKED]",
            "[REJECTED]",
            "[NOT FOUND]",
            "Tool execution error",
        ))
        # The [OK]/[ERR] tag is authoritative; exit code is only a fallback.
        is_error = explicit_error or timed_out or (exit_code is not None and is_ok is False)
        return cls(
            name=name,
            args=dict(args or {}),
            content=text,
            is_error=is_error,
            exit_code=exit_code,
            timed_out=timed_out,
            offloaded=offloaded,
        )


def _normalize_args(name: str, func: Callable, args: dict) -> dict:
    args = dict(args or {})
    try:
        param_names = list(inspect.signature(func).parameters.keys())
    except Exception:
        param_names = []
    if "file_path" in param_names and "file_path" not in args:
        for alt in ("path", "filepath", "file"):
            if alt in args:
                args["file_path"] = args.pop(alt)
                break
    if "command" in param_names and "command" not in args:
        if "cmd" in args:
            args["command"] = args.pop("cmd")
    if "name" in param_names and "name" not in args:
        for alt in ("agent", "subagent"):
            if alt in args:
                args["name"] = args.pop(alt)
                break
    if "task" in param_names and "task" not in args:
        for alt in ("instruction", "instructions", "prompt", "body"):
            if alt in args:
                args["task"] = args.pop(alt)
                break
    return args


_CTX_PARAM_CACHE: dict[int, Optional[str]] = {}


def _ctx_param_name(func: Callable) -> Optional[str]:
    """Name of the first ctx/deps/run_ctx/context param on a migrated tool,
    or None when the tool takes no typed workspace deps. Cached per-function."""
    try:
        fid = id(func)
        if fid in _CTX_PARAM_CACHE:
            return _CTX_PARAM_CACHE[fid]
        params = list(inspect.signature(func).parameters.keys())
    except Exception:
        return None
    for p in params:
        if p.lower() in ("ctx", "deps", "run_ctx", "context"):
            _CTX_PARAM_CACHE[fid] = p
            return p
    _CTX_PARAM_CACHE[fid] = None
    return None


def _cap_tool_result(res: ToolExecutionResult) -> ToolExecutionResult:
    """Apply the tool's character cap via the shared offload path.

    This is the single enforcement point for every registry-driven loop
    (developer, general, house, dfd, orchestrator, subagent_manager,
    subagentTools). Tools that already self-cap at the same value are no-ops.
    """
    from ._runtime import _tool_cap, auto_offload_result

    capped = auto_offload_result(res.content, res.name, max_chars=_tool_cap(res.name))
    if capped == res.content:
        return res
    return replace(res, content=capped, offloaded=True)


def _emit_tool_frame_lazy(deps: Any, kind: str, name: str, args: Any = None,
                          ok: Any = None, ms: int = 0, preview: str = "") -> None:
    """Best-effort tool_started/tool_finished frame for the live step pills.

    Lazy imports (this module must not import llm at load time — native_tools
    imports tools). Silent when there is no chat to attribute to (tests,
    offline callers).
    """
    try:
        chat_id = getattr(deps, "chat_id", "") or ""
        if not chat_id:
            return
        import json as _json
        try:
            args_json = _json.dumps(args or {}, default=str)[:500]
        except Exception:
            args_json = ""
        from llm.pai_transport import _emit_tool_frame
        _emit_tool_frame(getattr(deps, "role", "") or "", kind, name,
                         args_json, ok=ok, ms=ms, preview=preview, chat_id=chat_id)
    except Exception:
        pass


def execute_tool_detailed(
    name: str,
    args: dict,
    scope: Optional[str] = None,
    deps: Any = None,
) -> ToolExecutionResult:
    """Execute one tool directly and return the structured outcome.

    ``scope`` is accepted for backward compatibility and ignored — the caller's
    loop already filtered the call against its own allowlist, and the native
    path scopes via ``FilteredToolset``. No hooks, no guards. Results are
    capped at the tool's character budget (full output is offloaded).

    Emits tool_started/tool_finished frames around the call (best-effort) so
    the UI can render Codex-style live step pills without log scraping.
    """
    import time as _time
    _t0 = _time.time()
    _emit_tool_frame_lazy(deps, "tool_started", name, args)
    try:
        res = _execute_tool_detailed_inner(name, args, scope=scope, deps=deps)
    except Exception as e:
        _emit_tool_frame_lazy(deps, "tool_finished", name, args,
                              ok=False, ms=int((_time.time() - _t0) * 1000),
                              preview=f"Error: {e}"[:300])
        raise
    _emit_tool_frame_lazy(deps, "tool_finished", name, args,
                          ok=not res.is_error, ms=int((_time.time() - _t0) * 1000),
                          preview=(res.content or "")[:300])
    return res


def _execute_tool_detailed_inner(
    name: str,
    args: dict,
    scope: Optional[str] = None,
    deps: Any = None,
) -> ToolExecutionResult:
    """The actual dispatch (see execute_tool_detailed for the contract)."""
    import tools as _facade

    impl = getattr(_facade, name, None)
    if not callable(impl) and str(name).startswith("mcp__"):
        # MCP tools have no Python impl in the facade; dispatch to the bridge
        # (mirrors llm/native_tools.py's native-path wiring) so free-pool and
        # registry-driven loops can execute scoped MCP tools too.
        try:
            from tools.mcp.mcp_bridge import mcp_execute

            result = mcp_execute(str(name), dict(args or {}), scope=scope or "")
        except Exception as e:
            result = f"Error executing {name}: {e}"
        return _cap_tool_result(ToolExecutionResult.from_content(name, dict(args or {}), str(result)))
    if not callable(impl):
        return ToolExecutionResult.from_content(
            name, args, f"Error: Unknown tool '{name}'"
        )

    norm_args = _normalize_args(name, impl, dict(args or {}))
    try:
        ctx_param = _ctx_param_name(impl)
        if ctx_param is not None:
            result = impl(**norm_args, **{ctx_param: deps})
        else:
            result = impl(**norm_args)
    except TypeError as e:
        result = f"Error: Invalid arguments for {name}: {e}"
    except Exception as e:
        result = f"Error executing {name}: {e}"
    if not isinstance(result, str):
        result = str(result)
    return _cap_tool_result(ToolExecutionResult.from_content(name, norm_args, result))


def execute_tool(name: str, args: dict, deps: Any = None) -> str:
    """Legacy string-returning entry point (thin wrapper over the shim)."""
    return execute_tool_detailed(name, args, deps=deps).content


class _LazyToolMap(dict):
    # Runtime exposure is derived from the same canonical definitions used by
    # native schemas and compact fallback documentation.
    _TOOL_NAMES = list(get_native_tool_names())

    def __getitem__(self, item):
        import tools as _facade
        return getattr(_facade, item)

    def __contains__(self, item):
        import tools as _facade
        return hasattr(_facade, item)

    def get(self, item, default=None):
        import tools as _facade
        return getattr(_facade, item, default)

    def items(self):
        import tools as _facade
        return [(n, getattr(_facade, n)) for n in self._TOOL_NAMES if hasattr(_facade, n)]

    def keys(self):
        return [k for k, _ in self.items()]


TOOL_MAP = _LazyToolMap()
