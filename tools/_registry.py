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
- :func:`pre_validate_tool_args` — cheap required-args pre-check.
"""
from __future__ import annotations

import inspect
import re
import threading
from dataclasses import dataclass
from typing import Any, Callable, Optional

from ._schemas import get_native_tool_names, _TOOL_REQUIRED_ARGS

#: Watcher lazy-start: the legacy registry bootstrap started the file
#: observer once a workspace existed. The shim keeps that behavior on first
#: execution (best-effort, idempotent inside tools.watcher).
_watcher_tried = False
_watcher_lock = threading.Lock()


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
        match = re.match(r"\[(?:OK|ERR)\]\s+Exit\s+(-?\d+)", first_line, re.IGNORECASE)
        if match:
            exit_code = int(match.group(1))
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
            "Tool execution error",
        ))
        is_error = explicit_error or timed_out or (exit_code is not None and exit_code != 0)
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


def _ctx_param_name(func: Callable) -> Optional[str]:
    """Name of the first ctx/deps/run_ctx/context param on a migrated tool,
    or None when the tool takes no typed workspace deps."""
    try:
        params = list(inspect.signature(func).parameters.keys())
    except Exception:
        params = []
    for p in params:
        if p.lower() in ("ctx", "deps", "run_ctx", "context"):
            return p
    return None


def _ensure_watcher() -> None:
    global _watcher_tried
    with _watcher_lock:
        if _watcher_tried:
            return
        _watcher_tried = True
    try:
        from sync.state_sync import safe_get_state
        _ws = (safe_get_state() or {}).get("project_path") or ""
        if _ws:
            from tools.watcher import note as _watcher_note
            _watcher_note(_ws)
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
    path scopes via ``FilteredToolset``. No hooks, no guards.
    """
    import tools as _facade

    impl = getattr(_facade, name, None)
    if not callable(impl):
        return ToolExecutionResult.from_content(
            name, args, f"Error: Unknown tool '{name}'"
        )

    norm_args = _normalize_args(name, impl, dict(args or {}))
    _ensure_watcher()
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
    return ToolExecutionResult.from_content(name, norm_args, result)


def execute_tool(name: str, args: dict, deps: Any = None) -> str:
    """Legacy string-returning entry point (thin wrapper over the shim)."""
    return execute_tool_detailed(name, args, deps=deps).content


def pre_validate_tool_args(tool_name: str, args: dict) -> tuple[bool, str]:
    if tool_name not in _TOOL_REQUIRED_ARGS:
        return True, ""

    required = _TOOL_REQUIRED_ARGS[tool_name]
    missing = required - set(args.keys())
    if missing:
        return False, f"Missing required args for '{tool_name}': {', '.join(sorted(missing))}"

    for key in ("file_path", "path", "pattern", "command", "content", "diff"):
        if key in args and not isinstance(args[key], str):
            return False, f"Expected string for arg '{key}' in '{tool_name}', got {type(args[key]).__name__}"

    for key in ("timeout", "offset", "limit", "background"):
        if key in args and args[key] is not None and not isinstance(args[key], (int, float, bool)):
            return False, f"Expected number/bool for arg '{key}' in '{tool_name}', got {type(args[key]).__name__}"

    return True, ""


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
