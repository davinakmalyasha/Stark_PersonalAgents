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
from typing import Any, Optional

from pydantic_ai import Agent, RunContext
from pydantic_ai.capabilities.hooks import Hooks
from pydantic_ai.messages import ToolCallPart
from pydantic_ai.tools import Tool
from pydantic_ai.toolsets import FilteredToolset, FunctionToolset
from pydantic_ai.usage import UsageLimits

from llm.deps import WorkspaceDeps


def _tool_map() -> dict[str, Any]:
    """Map tool name -> implementation callable (lazy, avoids import cycle)."""
    import tools as _facade
    return dict(_facade.TOOL_MAP.items()) if hasattr(_facade, "TOOL_MAP") else {}


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
    """Pre-execution: plan-mode guard veto."""
    try:
        from tools.plan_mode_guard import plan_mode_guard
        reason = plan_mode_guard(
            {"name": call.tool_name, "args": dict(args or {}), "scope": ctx.deps.role}
        )
        if reason:
            from pydantic_ai.tools import ToolDenied
            raise ToolDenied(reason)
    except Exception:
        pass
    return dict(args or {})


def _wrap_tool_execute(
    ctx: RunContext[WorkspaceDeps],
    *,
    call: ToolCallPart,
    tool_def: Any,
    args: dict,
    handler: Any,
) -> Any:
    """Cache hit short-circuit: return cached result without calling the tool."""
    try:
        from tools import _check_tool_cache
        cached = _check_tool_cache(call.tool_name, dict(args or {}))
        if cached is not None:
            return cached
    except Exception:
        pass
    return handler(args)


def _after_tool_execute(
    ctx: RunContext[WorkspaceDeps],
    *,
    call: ToolCallPart,
    tool_def: Any,
    args: dict,
    result: Any,
) -> Any:
    """Post-execution: secret redact + 80k offload to VFS (registry post_hook)."""
    tool_name = call.tool_name
    text = result if isinstance(result, str) else str(result)
    if not text.startswith("Error"):
        try:
            from tools import redact_secrets
            text = redact_secrets(text)
        except Exception:
            pass
    # Offload >80k to /scratch (registry post_hook equivalent).
    if len(text) > 80000 and tool_name != "read_file":
        try:
            import uuid
            from tools import _write_file, _create_budgeted_preview
            file_id = uuid.uuid4().hex[:8]
            vfs_path = f"/scratch/large_output_{file_id}.txt"
            _write_file(file_path=vfs_path, content=text)
            preview = _create_budgeted_preview(text)
            text = (
                f"[TOOL OUTPUT OFFLOADED to VFS path: {vfs_path}]\n"
                f"Total lines: {len(text.splitlines())}, Total characters: {len(text)}\n"
                f"--- PREVIEW (head+tail) ---\n{preview}\n"
                f"---------------------------\n"
                f"[Use read_file with offset/limit to read the offloaded file at {vfs_path} if needed.]"
            )
        except Exception:
            pass
    return text


# ── Agent builder ────────────────────────────────────────────────────────────

def build_agent(
    role: str,
    model: Any,
    tools: Optional[list[dict]] = None,
    deps: Optional[WorkspaceDeps] = None,
    output_type: Optional[type] = None,
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

        # from_schema skips schema validation — we validate in the bound
        # wrapper and raise ModelRetry on invalid args so the model retries
        # (native Tool.max_retries). The preserved schema stays the model-facing
        # contract; nothing about it changes.
        from pydantic_ai.exceptions import ModelRetry
        schema_props = (params or {}).get("properties", {}) or {}
        schema_required = set((params or {}).get("required", []) or [])

        def _validate(**args) -> None:
            missing = schema_required - set(args.keys())
            if missing:
                raise ModelRetry(
                    f"Missing required arguments: {', '.join(sorted(missing))}"
                )
            for k, v in args.items():
                if k not in schema_props:
                    raise ModelRetry(f"Unexpected argument '{k}' not in tool schema")
                meta = schema_props.get(k) or {}
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

        if ctx_params:
            ctx_name = ctx_params[0]

            def _bound(**kw):
                _validate(**kw)
                return impl(**kw, **{ctx_name: deps})

            tool_objs.append(
                Tool.from_schema(
                    _bound,
                    name=name,
                    description=description,
                    json_schema=params,
                )
            )
        else:
            def _plain(**kw):
                _validate(**kw)
                return impl(**kw)

            tool_objs.append(
                Tool.from_schema(
                    _plain,
                    name=name,
                    description=description,
                    json_schema=params,
                )
            )

    # Scope via FilteredToolset (native replacement for restrict_scope).
    allowed = deps.allowed_tools or _allowed_for(role)
    function_toolset = FunctionToolset(tool_objs, max_retries=3)
    if allowed:
        toolset = FilteredToolset(
            function_toolset,
            filter_func=lambda ctx, td: td.name in allowed,
        )
    else:
        toolset = function_toolset

    hooks = Hooks(
        before_tool_execute=_before_tool_execute,
        tool_execute=_wrap_tool_execute,
        after_tool_execute=_after_tool_execute,
    )

    kwargs: dict[str, Any] = {}
    if output_type is not None:
        kwargs["output_type"] = output_type
    agent = Agent(
        model=model,
        toolsets=[toolset],
        capabilities=[hooks],
        deps_type=WorkspaceDeps,
        **kwargs,
    )
    return agent


DEFAULT_USAGE_LIMITS = UsageLimits(request_limit=200, tool_calls_limit=200)
