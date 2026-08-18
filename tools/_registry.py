"""
Tool Registry — Scoped tool registry + execution pipeline.

A tool is a :class:`ToolDefinition`: model-facing schema plus an ``execute``
function. Registration is idempotent and returns a disposer; per-scope
restrictions (allow/deny) filter both visibility and execution.

The execution pipeline:
    pre_execute hooks  ->  guards  ->  execute wrappers  ->  post_execute hooks
"""
from __future__ import annotations

import contextvars
import inspect
import threading
from dataclasses import dataclass
from typing import Any, Callable, Optional

from ._schemas import TOOL_DEFINITIONS, get_native_tools, _TOOL_DOCS, _TOOL_REQUIRED_ARGS

active_tool_scope: contextvars.ContextVar[str] = contextvars.ContextVar(
    "active_tool_scope", default="default"
)

PreDecision = Optional[tuple[str, str]]


@dataclass
class ToolDefinition:
    name: str
    description: str = ""
    parameters: Optional[dict] = None  # JSON Schema object root
    execute: Optional[Callable] = None  # fn(**args) -> result (string convention)
    timeout_ms: int = 0
    is_concurrency_safe: bool = False
    source: str = ""


class ToolRegistry:
    """Scoped tool registry with the guarded execution pipeline."""

    def __init__(self) -> None:
        self._tools: dict[str, ToolDefinition] = {}
        self._restrictions: dict[str, dict] = {}  # scope -> {allow, deny}
        self._pre_hooks: list[Callable] = []
        self._guards: list[Callable] = []
        self._execute_wrappers: list[Callable] = []
        self._post_hooks: list[Callable] = []
        self._lock = threading.RLock()

    def register(self, definition: ToolDefinition) -> Callable[[], None]:
        with self._lock:
            self._tools[definition.name] = definition

        def _dispose() -> None:
            with self._lock:
                if self._tools.get(definition.name) is definition:
                    del self._tools[definition.name]

        return _dispose

    def register_many(self, definitions: list[ToolDefinition]) -> Callable[[], None]:
        disposers = [self.register(d) for d in definitions]

        def _dispose() -> None:
            for d in disposers:
                d()

        return _dispose

    def get(self, name: str) -> Optional[ToolDefinition]:
        return self._tools.get(name)

    def restrict_scope(
        self, scope: str, allow: Optional[list[str]] = None, deny: Optional[list[str]] = None
    ) -> None:
        with self._lock:
            cur = self._restrictions.setdefault(scope, {"allow": None, "deny": set()})
            if allow is not None:
                cur["allow"] = set(allow)
            if deny is not None:
                cur["deny"] = set(deny)

    def _visible(self, name: str, scope: str) -> bool:
        if name not in self._tools:
            return False
        r = self._restrictions.get(scope)
        if r:
            if r["allow"] is not None and name not in r["allow"]:
                return False
            if name in r["deny"]:
                return False
        return True

    def _resolve_scope(self, scope: Optional[str]) -> str:
        return scope or active_tool_scope.get()

    def names_for(self, scope: Optional[str] = None) -> list[str]:
        scope = self._resolve_scope(scope)
        return [n for n in self._tools if self._visible(n, scope)]

    def schemas_for(self, scope: Optional[str] = None, include_description: bool = False) -> list[dict]:
        scope = self._resolve_scope(scope)
        out: list[dict] = []
        for name in sorted(self._tools):
            if not self._visible(name, scope):
                continue
            d = self._tools[name]
            params = d.parameters or {
                "type": "object",
                "properties": {},
                "required": [],
            }
            fn: dict[str, Any] = {"name": name, "parameters": params}
            if include_description and d.description:
                fn["description"] = d.description
            out.append({"type": "function", "function": fn})
        return out

    def compact_defs_for(self, scope: Optional[str] = None) -> dict[str, str]:
        scope = self._resolve_scope(scope)
        lines: dict[str, str] = {}
        for name in sorted(self._tools):
            if not self._visible(name, scope):
                continue
            d = self._tools[name]
            lines[name] = d.description or f"{name}() — Execute {name}"
        return lines

    def register_pre_hook(self, fn: Callable[[dict], PreDecision]) -> Callable[[], None]:
        self._pre_hooks.append(fn)
        return lambda: self._pre_hooks.remove(fn) if fn in self._pre_hooks else None

    def register_guard(self, fn: Callable[[dict], Optional[str]]) -> Callable[[], None]:
        self._guards.append(fn)
        return lambda: self._guards.remove(fn) if fn in self._guards else None

    def register_execute_wrapper(
        self, fn: Callable[[dict, Callable[[], str]], str]
    ) -> Callable[[], None]:
        self._execute_wrappers.append(fn)
        return lambda: self._execute_wrappers.remove(fn) if fn in self._execute_wrappers else None

    def register_post_hook(
        self, fn: Callable[[dict, str, bool], Optional[tuple[str, bool]]]
    ) -> Callable[[], None]:
        self._post_hooks.append(fn)
        return lambda: self._post_hooks.remove(fn) if fn in self._post_hooks else None

    @staticmethod
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

    def execute(self, name: str, args: dict, scope: Optional[str] = None) -> str:
        scope = self._resolve_scope(scope)
        d = self.get(name)
        if d is None or not self._visible(name, scope):
            available = ", ".join(self.names_for(scope)[:60])
            return f"Error: Unknown tool '{name}'. Available tools: {available}"

        ctx: dict = {
            "name": name,
            "args": self._normalize_args(name, d.execute or (lambda **kw: ""), args),
            "scope": scope,
            "definition": d,
        }

        for hook in self._pre_hooks:
            try:
                decision = hook(ctx)
            except Exception as e:
                decision = ("deny", f"pre-hook error: {e}")
            if decision is None:
                continue
            kind, payload = decision
            if kind == "deny":
                return f"[BLOCKED] {payload}"
            if kind == "allow":
                return payload

        for guard in self._guards:
            try:
                reason = guard(ctx)
            except Exception as e:
                reason = f"guard error: {e}"
            if reason:
                return f"[BLOCKED] {reason}"

        def _run() -> str:
            func = d.execute
            if func is None:
                return f"Error: tool '{name}' has no implementation"
            try:
                result = func(**ctx["args"])
            except TypeError as e:
                return f"Error: Invalid arguments for {name}: {e}"
            except Exception as e:
                return f"Error executing {name}: {e}"
            return result if isinstance(result, str) else str(result)

        chain: Callable[[], str] = _run
        for wrapper in reversed(self._execute_wrappers):
            prev = chain
            chain = lambda prev=prev, wrapper=wrapper: wrapper(ctx, prev)

        try:
            result = chain()
        except TypeError as e:
            return f"Error: Invalid arguments for {name}: {e}"
        except Exception as e:
            return f"Error executing {name}: {e}"

        is_error = result.startswith("Error")

        for hook in self._post_hooks:
            try:
                updated = hook(ctx, result, is_error)
            except Exception:
                updated = None
            if updated is not None:
                result, is_error = updated
        return result


_registry: Optional[ToolRegistry] = None
_registry_lock = threading.RLock()


def get_registry() -> ToolRegistry:
    global _registry
    if _registry is None:
        with _registry_lock:
            if _registry is None:
                _registry = ToolRegistry()
    return _registry


def reset_registry() -> None:
    global _registry
    with _registry_lock:
        _registry = None


# ── Lazy Hooks & ToolMap Assembly ───────────────────────────────────────────

def _tool_cache_pre_hook(ctx: dict):
    try:
        from tools import _check_tool_cache
        cached = _check_tool_cache(ctx["name"], ctx["args"])
        if cached is not None:
            return ("allow", cached)
    except Exception:
        pass
    return None


def _tool_result_post_hook(ctx: dict, result: str, is_error: bool):
    name = ctx["name"]
    try:
        from tools import redact_secrets, _set_tool_cache, _write_file, _create_budgeted_preview
    except Exception:
        return (result, is_error)

    if not is_error and name in ("run_command", "exec_command", "web_fetch", "web_search"):
        result = redact_secrets(result)

    if not is_error and not result.startswith("Error"):
        _set_tool_cache(name, ctx["args"], result)

    if not is_error and len(result) > 80000 and name != "read_file":
        import uuid
        file_id = uuid.uuid4().hex[:8]
        vfs_path = f"/scratch/large_output_{file_id}.txt"
        _write_file(vfs_path, result)
        orig_result_len = len(result)
        preview = _create_budgeted_preview(result)
        result = (
            f"[TOOL OUTPUT OFFLOADED to VFS path: {vfs_path}]\n"
            f"Total lines: {len(result.splitlines())}, Total characters: {orig_result_len}\n"
            f"--- PREVIEW (head+tail) ---\n"
            f"{preview}\n"
            f"---------------------------\n"
            f"[Use read_file with offset/limit to read the offloaded file at {vfs_path} if needed.]"
        )
        try:
            from memory.context_compaction import truncation_note
            result += truncation_note(orig_result_len, 80000)
        except Exception:
            pass
    return (result, is_error)


_tool_registry = None
_tool_registry_lock = threading.Lock()


def validate_tool_definitions(tool_map: dict) -> None:
    seen = set()
    for d in TOOL_DEFINITIONS:
        name = d.get("name", "")
        if not name:
            raise ValueError("TOOL_DEFINITIONS entry missing 'name'")
        if name in seen:
            raise ValueError(f"TOOL_DEFINITIONS has duplicate tool '{name}'")
        seen.add(name)
        params = d.get("parameters") or {}
        props = params.get("properties", {})
        for req in params.get("required", []):
            if req not in props:
                raise ValueError(f"tool '{name}': required arg '{req}' not in properties")
        if d.get("exposed") is not False and name not in tool_map:
            raise ValueError(f"tool '{name}' is exposed to the model but missing from TOOL_MAP")
    for doc_name in _TOOL_DOCS:
        if doc_name not in tool_map:
            raise ValueError(f"_TOOL_DOCS references unknown tool '{doc_name}'")


def ensure_tool_registry():
    global _tool_registry
    if _tool_registry is not None:
        return _tool_registry
    with _tool_registry_lock:
        if _tool_registry is not None:
            return _tool_registry

        import tools as _facade
        tool_map = _facade.TOOL_MAP
        validate_tool_definitions(tool_map)
        reg = get_registry()
        _defs_by_name = {d["name"]: d for d in TOOL_DEFINITIONS}
        definitions = []
        for name, fn in tool_map.items():
            defn = _defs_by_name.get(name, {})
            definitions.append(ToolDefinition(
                name=name,
                description=(_TOOL_DOCS.get(name)
                             or defn.get("description", "")
                             or f"{name}() — Execute {name}"),
                parameters=defn.get("parameters") or None,
                execute=fn,
                source="tools.TOOL_MAP",
            ))
        reg.register_many(definitions)
        reg.register_pre_hook(_tool_cache_pre_hook)
        reg.register_post_hook(_tool_result_post_hook)
        _tool_registry = reg
        _facade._tool_registry = reg
        return reg


def execute_tool(name: str, args: dict) -> str:
    try:
        return ensure_tool_registry().execute(name, args)
    except Exception:
        import tools as _facade
        return _facade._legacy_execute_tool(name, args)


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
    _TOOL_NAMES = [
        'read_file', 'edit_file', 'run_command', 'search_code', 'task',
        'start_async_task', 'check_async_task', 'list_async_tasks',
        'browser_navigate', 'browser_act', 'browser_extract', 'browser_screenshot',
        'browser_click', 'browser_fill', 'browser_close', 'github_apply_gate',
        'apply_template', 'template_list', 'template_new', 'create_docx',
        'create_xlsx', 'read_pdf', 'update_feature', 'update_project_meta',
        'ask_user', 'send_message', 'list_agents', 'interrupt_agent', 'report',
        'create_project', 'find_project_dev'
    ]

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


def _legacy_execute_tool(name: str, args: dict) -> str:
    if name not in TOOL_MAP:
        return f"Error: Unknown tool '{name}'. Available tools: {', '.join(TOOL_MAP.keys())}"
    try:
        func = TOOL_MAP.get(name)
        if func is None:
            return f"Error: Tool '{name}' has no implementation"
        return func(**args)
    except Exception as e:
        return f"Error executing {name}: {e}"

